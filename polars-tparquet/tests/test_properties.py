"""Property tests: any frame of any dtype written through the plugin reads back the same, split
or not, bit for bit; and any query over a scan gives what it gives over the frame in memory."""

from __future__ import annotations

import io
import pathlib
from collections.abc import Callable
from typing import Any, Literal

import polars as pl
from hypothesis import given, note
from hypothesis import strategies as st
from polars.testing import assert_frame_equal
from polars.testing.parametric import dataframes
from tparquet import Policy

import polars_tparquet as pt
from conftest import canonical, float_bits, ticks

T0 = 1_700_000_000_000


@st.composite
def frames(draw: st.DrawFn) -> pl.DataFrame:
    n = draw(st.integers(1, 300))
    i64 = draw(st.lists(st.one_of(st.none(), st.integers(-(2**63), 2**63 - 1)), min_size=n, max_size=n))
    f64 = draw(st.lists(st.one_of(st.none(), st.floats(allow_nan=True, allow_infinity=True)), min_size=n, max_size=n))
    f32 = draw(st.lists(st.floats(width=32), min_size=n, max_size=n))
    s = draw(st.lists(st.one_of(st.none(), st.text(max_size=8)), min_size=n, max_size=n))
    return pl.DataFrame(
        {
            "i64": pl.Series(i64, dtype=pl.Int64),
            "f64": pl.Series(f64, dtype=pl.Float64),
            "f32": pl.Series(f32, dtype=pl.Float32),
            "s": pl.Series(s, dtype=pl.String),
        }
    )


@given(df=frames(), parts=st.integers(1, 4), policy=st.sampled_from(["auto", "pcodec", "base"]))
def test_write_then_scan_any_frame(tmp_path: pathlib.Path, df: pl.DataFrame, parts: int, policy: Policy) -> None:
    size = -(-df.height // parts)
    paths = []
    for k in range(parts):
        part = df.slice(k * size, size)
        if part.height:
            paths.append(tmp_path / f"{k}.parquet")
            part.tparquet.write(paths[-1], policy=policy)
    assert_frame_equal(pt.scan_tparquet(paths).collect(), df, check_exact=True)
    sunk = tmp_path / "sunk.parquet"
    pt.scan_tparquet(paths).tparquet.sink(sunk, policy=policy)
    assert_frame_equal(pt.read_tparquet(sunk), df, check_exact=True)


#: Every dtype the plugin writes. Left out: Null (tparquet's Arrow import rejects Polars' Null
#: arrays, see test_dtypes.test_a_null_column_is_written), Int128 and UInt128 (no Parquet Arrow
#: type), Object (refused) and the testing-only Extension dtype.
SUPPORTED = [
    pl.Boolean,
    pl.Int8,
    pl.Int16,
    pl.Int32,
    pl.Int64,
    pl.UInt8,
    pl.UInt16,
    pl.UInt32,
    pl.UInt64,
    pl.Float16,
    pl.Float32,
    pl.Float64,
    pl.Decimal,
    pl.Date,
    pl.Datetime,
    pl.Duration,
    pl.Time,
    pl.String,
    pl.Binary,
    pl.Categorical,
    pl.Enum,
    pl.List,
    pl.Array,
    pl.Struct,
]


def _values(df: pl.DataFrame) -> pl.DataFrame:
    """`df` rebuilt from its values. The strategies put values under nulls ("masked out"), and
    Polars 2.0.0 compares those in arrays of structs: a frame is not equal to its own values."""
    return pl.DataFrame([pl.Series(s.name, s.to_list(), dtype=s.dtype) for s in df.iter_columns()])


def _split(df: pl.DataFrame, parts: int) -> list[pl.DataFrame]:
    size = max(1, -(-df.height // parts))
    return [df.slice(k * size, size) for k in range(parts)]


@given(
    df=dataframes(allowed_dtypes=SUPPORTED, max_cols=5, max_size=40),
    parts=st.integers(1, 3),
    chunk_size=st.integers(1, 50),
    row_group_size=st.integers(1, 50),
    policy=st.sampled_from(["auto", "pcodec", "base"]),
)
def test_any_frame_of_any_dtype_round_trips(
    df: pl.DataFrame, parts: int, chunk_size: int, row_group_size: int, policy: Policy
) -> None:
    files = []
    for part in _split(df, parts):
        files.append(io.BytesIO())
        part.tparquet.write(files[-1], policy=policy, row_group_size=row_group_size)
    want = _values(df)
    # floats bit for bit, in the columns that are not nested (the canonical JSON of nested
    # columns with masked-out values panics in Polars 2.0.0; the vectors check nested floats)
    flat = [n for n, t in df.schema.items() if not t.is_nested()]
    read = pt.scan_tparquet([io.BytesIO(f.getvalue()) for f in files]).collect()
    assert_frame_equal(read, want, check_exact=True)
    assert canonical(read.select(flat)) == canonical(df.select(flat))
    sunk = io.BytesIO()
    df.lazy().tparquet.sink(sunk, policy=policy, chunk_size=chunk_size, row_group_size=row_group_size)
    out = pt.read_tparquet(io.BytesIO(sunk.getvalue()))
    assert_frame_equal(out, want, check_exact=True)
    assert canonical(out.select(flat)) == canonical(df.select(flat))


@given(
    bits=st.lists(st.one_of(st.none(), st.integers(0, 2**64 - 1)), max_size=200),
    policy=st.sampled_from(["auto", "pcodec", "base"]),
)
def test_any_float_bits_round_trip(bits: list[int | None], policy: Policy) -> None:
    df = pl.DataFrame(
        {
            "f64": pl.Series(bits, dtype=pl.UInt64).reinterpret(dtype=pl.Float64),
            "f32": pl.Series([None if b is None else b >> 32 for b in bits], dtype=pl.UInt32).reinterpret(
                dtype=pl.Float32
            ),
        }
    )
    buf = io.BytesIO()
    df.tparquet.write(buf, policy=policy)
    out = pt.read_tparquet(io.BytesIO(buf.getvalue()))
    assert float_bits(out["f64"]) == bits
    assert float_bits(out["f32"]) == [None if b is None else b >> 32 for b in bits]


Step = Callable[[pl.LazyFrame], pl.LazyFrame]


#: The comparisons a predicate step draws from.
COMPARE: dict[str, Callable[[pl.Expr, Any], pl.Expr]] = {
    "<": lambda c, v: c < v,
    "<=": lambda c, v: c <= v,
    ">": lambda c, v: c > v,
    ">=": lambda c, v: c >= v,
    "==": lambda c, v: c == v,
    "!=": lambda c, v: c != v,
}
Closed = Literal["both", "left", "right", "none"]


@st.composite
def steps(draw: st.DrawFn) -> tuple[str, Step]:
    """One step of a query over ticks(): its description and the step."""
    kind = draw(
        st.sampled_from(["ts", "qty", "null", "sym", "between", "or", "slice", "head", "tail", "select", "index"])
    )
    n = draw(st.integers(0, 4000))
    t = T0 + 250 * n
    sym = draw(st.sampled_from(["BTC", "ETH", "SOL", "XRP"]))
    op = draw(st.sampled_from(sorted(COMPARE)))
    compare = COMPARE[op]
    width = draw(st.integers(0, 600))
    closed: Closed = draw(st.sampled_from(["both", "left", "right", "none"]))
    options: dict[str, tuple[str, Step]] = {
        "ts": (f"ts {op} {t}", lambda lf: lf.filter(compare(pl.col("ts"), t))),
        "qty": (f"qty {op} {n % 60}", lambda lf: lf.filter(compare(pl.col("qty"), n % 60))),
        "null": ("qty is null", lambda lf: lf.filter(pl.col("qty").is_null())),
        "sym": (f"sym {op} {sym}", lambda lf: lf.filter(compare(pl.col("sym"), sym))),
        "between": (
            f"ts in {t}..+{width} ({closed})",
            lambda lf: lf.filter(pl.col("ts").is_between(t, t + 250 * width, closed=closed)),
        ),
        "or": (f"ts < {t} or sym == {sym}", lambda lf: lf.filter((pl.col("ts") < t) | (pl.col("sym") == sym))),
        "slice": (f"slice({n}, {width})", lambda lf: lf.slice(n, width)),
        "head": (f"head({width})", lambda lf: lf.head(width)),
        "tail": (f"tail({width})", lambda lf: lf.tail(width)),
        "select": ("select qty, ts", lambda lf: lf.select("qty", "ts", "price", "sym")),
        "index": ("with_row_index", lambda lf: lf.with_row_index(f"i{len(lf.collect_schema())}")),
    }
    return options[kind]


@given(
    # two files at least: over one file a limit before a filter gives wrong rows (python/), see
    # test_queries.test_a_limit_before_a_filter_over_one_file; over a union Polars keeps the limit
    # above the sources
    sizes=st.lists(st.integers(0, 1500), min_size=2, max_size=3),
    row_group_size=st.integers(1, 700),
    query=st.lists(steps(), max_size=4),
    engine=st.sampled_from(["in-memory", "streaming"]),
)
def test_any_query_over_a_scan_gives_what_it_gives_in_memory(
    tmp_path: pathlib.Path,
    sizes: list[int],
    row_group_size: int,
    query: list[tuple[str, Step]],
    engine: Literal["in-memory", "streaming"],
) -> None:
    parts, start = [], 0
    for size in sizes:
        parts.append(ticks(size, offset=start))
        start += size
    paths = []
    for k, part in enumerate(parts):
        paths.append(tmp_path / f"{k}.parquet")
        part.tparquet.write(paths[-1], row_group_size=row_group_size)
    lf, want = pt.scan_tparquet(paths), pl.concat(parts).lazy()
    for _, step in query:
        lf, want = step(lf), step(want)
    note(" | ".join(d for d, _ in query))
    assert_frame_equal(lf.collect(engine=engine), want.collect(engine=engine), check_exact=True)
