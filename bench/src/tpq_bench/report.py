"""Results grouped by data type, as Markdown tables and as JSON."""

from __future__ import annotations

import dataclasses
import json
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Sequence

    from tpq_bench.measure import Measurement

# the data types, in the order of the report, and their headings
GROUPS = {
    "trades": "Trades",
    "quotes": "Quotes (L1, top of book)",
    "l2": "Order book (L2: snapshots and updates)",
    "bars": "Bars (OHLCV)",
    "metrics": "Metrics (5m bars)",
    "derivatives": "Derivatives (funding, mark, index, open interest)",
    "observability": "Observability (TSBS DevOps server metrics)",
}


@dataclasses.dataclass(frozen=True)
class Entry:
    provider: str
    data: str
    group: str
    # the file as the provider delivers it
    source_bytes: int
    m: Measurement


def size(n: int) -> str:
    """Decimal units, three significant digits from 1 MB."""
    if n < 1000:
        return f"{n} B"
    if n < 1_000_000:
        return f"{round(n / 1000)} KB"
    if n < 1_000_000_000:
        return f"{n / 1e6:.1f} MB"
    return f"{n / 1e9:.2f} GB"


def groups(entries: Sequence[Entry]) -> list[tuple[str, list[Entry]]]:
    """`entries` by group, the groups in the order of `GROUPS`, entries in their order."""
    for e in entries:
        if e.group not in GROUPS:
            raise ValueError(f"unknown group {e.group!r} of {e.provider} {e.data}")
    return [(g, [e for e in entries if e.group == g]) for g in GROUPS if any(e.group == g for e in entries)]


FORMATS = ("zstd9", "tuned", "tparquet")


def speed(n: int, seconds: float) -> str:
    """MB/s, to the unit."""
    if seconds <= 0:
        return "–"
    return f"{n / 1e6 / seconds:,.0f}"


def _speeds(entries: Sequence[Entry]) -> str:
    n = sum(e.m.arrow_bytes for e in entries)
    cells = [
        speed(n, sum(getattr(e.m, f"{kind}_s_{f}") for e in entries)) for kind in ("write", "read") for f in FORMATS
    ]
    return f"{size(n)} | {' | '.join(cells)} |\n"


def _sizes(label: str, entries: Sequence[Entry]) -> str:
    z9 = sum(e.m.zstd9_bytes for e in entries)
    tuned = sum(e.m.tuned_bytes for e in entries)
    tp = sum(e.m.tparquet_bytes for e in entries)
    rows = sum(e.m.rows for e in entries)
    return (
        f"| {label} | {len(entries)} | {rows:,} | {size(z9)} | {size(tuned)} | {size(tp)} "
        f"| **{tp / z9:.3f}×** | **{tp / tuned:.3f}×** |\n"
    )


SPEED_COLUMNS = (
    "Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |\n"
)


def markdown(entries: Sequence[Entry]) -> str:
    """Summary tables of sizes and speeds, one row per data type, then both tables per data type."""
    by_group = groups(entries)
    out = [
        "| Data type | Datasets | Rows | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) "
        "| tparquet / tuned |\n",
        "|---|---:|---:|---:|---:|---:|---:|---:|\n",
    ]
    out += [_sizes(GROUPS[g], es) for g, es in by_group]
    out.append(_sizes("**All**", entries))
    out += [
        "\nSpeed, in MB/s of Arrow data (the columns' size in memory):\n\n",
        f"| Data type | {SPEED_COLUMNS}",
        "|---|---:|---:|---:|---:|---:|---:|---:|\n",
    ]
    out += [f"| {GROUPS[g]} | {_speeds(es)}" for g, es in by_group]
    out.append(f"| **All** | {_speeds(entries)}")
    for g, es in by_group:
        out += [
            f"\n### {GROUPS[g]}\n\n",
            "| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) "
            "| tparquet / tuned | PCODEC columns |\n",
            "|---|---|---:|---:|---:|---:|---:|---:|---:|---|\n",
        ]
        out += [
            f"| {e.provider} | {e.data} | {e.m.rows:,} | {size(e.source_bytes)} | {size(e.m.zstd9_bytes)} "
            f"| {size(e.m.tuned_bytes)} | {size(e.m.tparquet_bytes)} | {e.m.ratio:.3f}× | {e.m.ratio_tuned:.3f}× "
            f"| {e.m.pcodec_columns} of {len(e.m.codecs)} |\n"
            for e in es
        ]
        out += [f"\n| Provider | Data | {SPEED_COLUMNS}", "|---|---|---:|---:|---:|---:|---:|---:|---:|\n"]
        out += [f"| {e.provider} | {e.data} | {_speeds([e])}" for e in es]
    return "".join(out)


def to_json(entries: Sequence[Entry]) -> str:
    rows = []
    for e in entries:
        m = dataclasses.asdict(e.m)
        codecs, encodings = m.pop("codecs"), m.pop("encodings")
        rows.append(
            {
                "provider": e.provider,
                "data": e.data,
                "group": e.group,
                "source_bytes": e.source_bytes,
                **{k: v for k, v in m.items() if k == "rows" or k.endswith("_bytes")},
                "ratio": e.m.ratio,
                "ratio_tuned": e.m.ratio_tuned,
                **{k: v for k, v in m.items() if k.startswith(("write_s_", "read_s_"))},
                "codecs": codecs,
                "tuned_encodings": encodings,
            }
        )
    return json.dumps(rows, indent=2, ensure_ascii=False) + "\n"  # pragma: no mutate (None is False)
