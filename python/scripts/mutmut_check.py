"""Runs mutmut and fails unless every mutant was killed (a timeout counts as killed: the
mutant made the tests hang). Run from python/ after `maturin develop`, with
TPARQUET_TESTDATA pointing at testdata/ (mutmut runs the tests from a copy)."""

from __future__ import annotations

import json
import pathlib
import re
import shutil
import subprocess
import sys

CAUGHT = {"killed", "timeout"}


def refresh_extension() -> None:
    """mutmut copies the package into mutants/ once and keeps files that are already there:
    copy the freshly built extension (and the other non-Python files) again."""
    package = pathlib.Path("src/tparquet")
    target = pathlib.Path("mutants") / package
    if target.is_dir():
        for f in package.iterdir():
            if f.is_file() and f.suffix != ".py":
                shutil.copy2(f, target / f.name)


def results() -> dict[str, str]:
    out = subprocess.run(
        [sys.executable, "-m", "mutmut", "results", "--all", "true"], capture_output=True, text=True, check=True
    ).stdout
    status: dict[str, str] = {}
    for line in out.splitlines():
        m = re.match(r"^\s*(\S+): (.+)$", line)
        if m:
            status[m.group(1)] = m.group(2).strip()
    return status


def exit_codes() -> dict[str, object]:
    """The worker exit code mutmut recorded for every mutant (its .meta files)."""
    codes: dict[str, object] = {}
    for meta in pathlib.Path("mutants").rglob("*.meta"):
        codes.update(json.loads(meta.read_text()).get("exit_code_by_key", {}))
    return codes


def main() -> int:
    refresh_extension()
    subprocess.run([sys.executable, "-m", "mutmut", "run", "--max-children", "2"], check=False)
    status = results()
    # "suspicious" is mutmut's name for a worker exit code it does not know: the worker ended
    # abnormally, which is no verdict on the tests. Report the code, and check each such
    # mutant once more on its own; only a kill (or a timeout) on that run counts.
    suspicious = sorted(name for name, s in status.items() if s == "suspicious")
    if suspicious:
        codes = exit_codes()
        for name in suspicious:
            print(f"suspicious: {name} (worker exit code {codes.get(name)}); checking it again alone")
        subprocess.run([sys.executable, "-m", "mutmut", "run", *suspicious], check=False)
        again = results()
        codes = exit_codes()
        for name in suspicious:
            print(f"  {name}: {again.get(name)} (worker exit code {codes.get(name)})")
            status[name] = again.get(name, status[name])
    counts: dict[str, int] = {}
    for s in status.values():
        counts[s] = counts.get(s, 0) + 1
    print("mutmut:", ", ".join(f"{n} {s}" for s, n in sorted(counts.items())))
    missed = sorted(name for name, s in status.items() if s not in CAUGHT)
    for name in missed:
        print(f"not killed: {name} ({status[name]})")
    if not status:
        print("mutmut reported no mutants")
        return 1
    return 1 if missed else 0


if __name__ == "__main__":
    raise SystemExit(main())
