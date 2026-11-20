"""The verification coverage of the library: every method of every class in target/classes
(javap -p; compiler-generated lambdas, accessors and $values left out) must be matched by an
entry of verification/coverage.txt, and every piece of evidence an entry names must exist.

An entry is a pattern (fnmatch over Class.method, e.g. "Ans.quantizeWeights", "Delta$Matrix.*")
followed by evidence, each "kind:reference":

  proof:<fn>          a theorem group of proofs.py (in its THEOREMS list): Z3, every input
  exhaustive:<T#m>    a test that tries every input of a bounded domain
  vectors:<T#m>       a test against recorded reference outputs (no oracle needed)
  differential:<T>    a test against the live reference (the oracle)
  unit:<T#m>          a unit test with hand-written expectations
  trivial:<reason>    a constructor, accessor, toString or interface declaration

A method gets the evidence of every entry that matches it. The report counts the methods by
their strongest evidence; any method with none, or any reference to a test, test method or
theorem group that does not exist, fails the check.

  python3 verification/coverage.py [--list]     (after mvn compile)
"""

from __future__ import annotations

import ast
import fnmatch
import pathlib
import re
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
LIB = HERE.parent
CLASSES = LIB / "target" / "classes"
TESTS = LIB / "src" / "test" / "java" / "io" / "aperiodic" / "pcodec"
KINDS = ["proof", "exhaustive", "vectors", "differential", "unit", "trivial"]
PKG = "io.aperiodic.pcodec."


def methods() -> list[str]:
    files = sorted(str(p) for p in CLASSES.rglob("*.class"))
    if not files:
        sys.exit("no classes: run mvn compile first")
    out = subprocess.run(["javap", "-p", *files], capture_output=True, text=True, check=True).stdout
    found: list[str] = []
    cls = None
    for line in out.splitlines():
        m = re.match(r".*?\b(?:class|interface|enum) ([\w.$]+)", line)
        if m and line.rstrip().endswith("{"):
            cls = m.group(1).removeprefix(PKG)
            continue
        m = re.search(r"([\w$]+)\(", line)
        if not (m and cls):
            continue
        name = m.group(1)
        if name.startswith(("lambda$", "access$")) or name == "$values":
            continue
        if name == cls.split(".")[-1]:  # javap prints a constructor under its class's name
            name = "<init>"
        qual = f"{cls}.{name}"
        if qual not in found:
            found.append(qual)
    return found


def theorem_groups() -> set[str]:
    tree = ast.parse((HERE / "proofs.py").read_text())
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(getattr(t, "id", "") == "THEOREMS" for t in node.targets):
            return {e.id for e in node.value.elts}
    sys.exit("proofs.py has no THEOREMS list")


def test_exists(ref: str) -> bool:
    cls, _, method = ref.partition("#")
    hits = list(TESTS.rglob(f"{cls}.java"))
    if not hits:
        return False
    return not method or re.search(rf"\b{re.escape(method)}\s*\(", hits[0].read_text()) is not None


def manifest() -> list[tuple[str, list[tuple[str, str]], int]]:
    entries = []
    for no, raw in enumerate((HERE / "coverage.txt").read_text().splitlines(), 1):
        line = raw.split(" # ")[0].strip()
        if not line or line.startswith("#"):
            continue
        pattern, *ev = line.split()
        evidence = []
        for e in ev:
            kind, _, ref = e.partition(":")
            evidence.append((kind, ref))
        entries.append((pattern, evidence, no))
    return entries


def main() -> int:
    groups = theorem_groups()
    entries = manifest()
    errors = []
    for pattern, evidence, no in entries:
        if not evidence:
            errors.append(f"coverage.txt:{no}: {pattern} names no evidence")
        for kind, ref in evidence:
            if kind not in KINDS:
                errors.append(f"coverage.txt:{no}: unknown kind {kind!r}")
            elif kind == "proof" and ref not in groups:
                errors.append(f"coverage.txt:{no}: no theorem group {ref!r} in proofs.py")
            elif kind in ("exhaustive", "vectors", "differential", "unit") and not test_exists(ref):
                errors.append(f"coverage.txt:{no}: no test {ref!r}")
            elif kind == "trivial" and not ref:
                errors.append(f"coverage.txt:{no}: trivial needs a reason")
    all_methods = methods()
    used = set()
    strongest: dict[str, str] = {}
    for m in all_methods:
        kinds = set()
        for pattern, evidence, no in entries:
            if fnmatch.fnmatchcase(m, pattern):
                used.add(no)
                kinds.update(k for k, _ in evidence)
        if not kinds:
            errors.append(f"uncovered: {m}")
            continue
        strongest[m] = min(kinds, key=KINDS.index)
    for pattern, _, no in entries:
        if no not in used:
            errors.append(f"coverage.txt:{no}: {pattern} matches no method")
    if "--list" in sys.argv:
        for m in all_methods:
            print(f"{strongest.get(m, 'NONE'):13} {m}")
    print(f"{len(all_methods)} methods in {len({m.rsplit('.', 1)[0] for m in all_methods})} classes")
    for k in KINDS:
        n = sum(1 for v in strongest.values() if v == k)
        print(f"  {k:13} {n:4}  {100 * n / len(all_methods):5.1f}%")
    for e in errors:
        print("ERROR", e)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
