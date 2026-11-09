"""Mutation check of the fork's changes to DuckDB's parquet extension (duckdb/FORK.md).

No C++ mutation tool (Mull, Dextool) is installable on the build machines, so this applies the
hand-picked mutations of mutants.json to target/duckdb/VERSION/src one at a time, rebuilds the
extension incrementally (ninja tparquet_loadable_extension), runs the tests against it and
requires them to fail. A mutant that does not build counts as unviable, a hang (the timeout) as
caught. The source file is restored after every mutant, and the extension rebuilt from the
restored tree at the end.

    python3 duckdb/mutation/run.py --version 2.0.0 [--jobs 4] [--only ID ...] -- PYTHON...

PYTHON... is the command that runs Python with that version's `duckdb` (`make mutation` gives
it). Writes target/duckdb/VERSION/mutation.out/results.json and exits 1 if any mutant survived.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time


def run(cmd: list[str], timeout: float | None = None, log: pathlib.Path | None = None, env=None) -> int:
    out = open(log, "ab") if log else subprocess.DEVNULL
    try:
        return subprocess.run(cmd, stdout=out, stderr=subprocess.STDOUT, timeout=timeout, env=env).returncode
    except subprocess.TimeoutExpired:
        return -1
    finally:
        if log:
            out.close()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--jobs", default=str(os.cpu_count() or 2))
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--test-timeout", type=float, default=900)
    ap.add_argument("python", nargs="+")
    args = ap.parse_args()
    here = pathlib.Path(__file__).resolve().parent
    root = here.parent.parent
    src = root / "target/duckdb" / args.version / "src"
    build = root / "target/duckdb" / args.version / "build"
    built = build / "extension/parquet/tparquet.duckdb_extension"
    out = root / "target/duckdb" / args.version / "mutation.out"
    out.mkdir(parents=True, exist_ok=True)
    log = out / "log.txt"
    log.write_text("")
    for p in (src / ".tparquet-fork", build / "build.ninja"):
        if not p.exists():
            print(f"mutants: {p} is missing: run `duckdb/build.sh {args.version}` first", file=sys.stderr)
            return 2

    def rebuild() -> int:
        return run(["ninja", "-C", str(build), "-j", args.jobs, "tparquet_loadable_extension"], log=log)

    env = {
        **os.environ,
        "TPARQUET_DUCKDB_EXTENSION": str(built),
        "HYPOTHESIS_PROFILE": "mutation",
        "TPARQUET_DUCKDB_TEST_TIMEOUT": "120",
    }

    def tests() -> int:
        cmd = [*args.python, "-m", "pytest", "-x", "-q", "-p", "no:cacheprovider", "-m", "not slow", str(here.parent / "tests")]
        return run(cmd, timeout=args.test_timeout, log=log, env=env)

    spec = json.loads((here / "mutants.json").read_text())
    mutants = [m for m in spec["mutants"] if args.version in m.get("versions", [args.version])]
    if args.only:
        mutants = [m for m in mutants if m["id"] in args.only]
    results = []
    try:
        for m in mutants:
            path = src / m["file"]
            original = path.read_text()
            if original.count(m["old"]) != 1:
                print(f"mutants: {m['id']}: `old` occurs {original.count(m['old'])} times in {m['file']}", file=sys.stderr)
                return 2
            start = time.time()
            path.write_text(original.replace(m["old"], m["new"]))
            try:
                if rebuild() != 0:
                    status = "unviable"
                else:
                    rc = tests()
                    status = "caught" if rc != 0 else "missed"
            finally:
                path.write_text(original)
            results.append({"id": m["id"], "status": status, "seconds": round(time.time() - start)})
            print(f"{status:8} {m['id']} ({results[-1]['seconds']}s)", flush=True)
    finally:
        rebuild()
        (out / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    missed = [r["id"] for r in results if r["status"] == "missed"]
    counts = {s: sum(r["status"] == s for r in results) for s in ("caught", "missed", "unviable")}
    print(f"mutants: {counts}")
    if missed:
        print("missed: " + ", ".join(missed))
    return 1 if missed else 0


if __name__ == "__main__":
    raise SystemExit(main())
