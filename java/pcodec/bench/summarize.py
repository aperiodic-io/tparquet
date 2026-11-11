"""Turns bench/results/*.txt (each with its .base timed just before it) into the table of
bench/RESULTS.md: per POC, the geometric mean over the Bench cases of POC/baseline throughput,
for encoding and decoding, and the best case of each."""

from __future__ import annotations

import math
import pathlib
import re
import sys

ROW = re.compile(r"^\|\s*(.+?)\s*\|\s*([\d.]+)\s*\|[^|]*\|[^|]*\|\s*([\d.]+)\s*\|")


def table(path: pathlib.Path) -> dict[str, tuple[float, float]]:
    """each case's median throughputs over the repeated runs in the file"""
    rows: dict[str, list[tuple[float, float]]] = {}
    for line in path.read_text().splitlines():
        m = ROW.match(line)
        if m:
            rows.setdefault(m.group(1), []).append((float(m.group(2)), float(m.group(3))))

    def median(xs: list[float]) -> float:
        s = sorted(xs)
        return s[len(s) // 2] if len(s) % 2 else (s[len(s) // 2 - 1] + s[len(s) // 2]) / 2

    return {k: (median([e for e, _ in v]), median([d for _, d in v])) for k, v in rows.items()}


def main() -> None:
    here = pathlib.Path(__file__).parent / (sys.argv[1] if len(sys.argv) > 1 else "results")
    print("| POC | encode | decode | best encode case | best decode case |")
    print("|---|---:|---:|---|---|")
    for p in sorted(here.glob("*.txt")):
        name = p.stem
        base = p.with_suffix(".base")
        if not base.exists():
            print(f"| {name} | - | - | {p.read_text().strip()} | |")
            continue
        poc, ref = table(p), table(base)
        common = [k for k in poc if k in ref]
        if not common:
            print(f"| {name} | - | - | no timings | |")
            continue
        enc = [poc[k][0] / ref[k][0] for k in common]
        dec = [poc[k][1] / ref[k][1] for k in common]
        ge = math.exp(sum(map(math.log, enc)) / len(enc))
        gd = math.exp(sum(map(math.log, dec)) / len(dec))
        be = max(common, key=lambda k: poc[k][0] / ref[k][0])
        bd = max(common, key=lambda k: poc[k][1] / ref[k][1])
        print(
            f"| {name} | {ge:.3f} | {gd:.3f} | {be} {poc[be][0] / ref[be][0]:.2f} |"
            f" {bd} {poc[bd][1] / ref[bd][1]:.2f} |"
        )


if __name__ == "__main__":
    sys.exit(main())
