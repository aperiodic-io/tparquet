"""Property tests of tparquet.polars.scan_parquet: whatever the predicate, the projection and
the limit, and however the file is cut into row groups, the scan returns what Polars returns on
the same frame without any pushdown (the row-group pruning may only skip what cannot match)."""

from __future__ import annotations

import datetime
import io
import zoneinfo
from collections.abc import Callable
from typing import Any, Literal

import polars as pl
import pyarrow as pa
import pytest
from hypothesis import given, settings
from hypothesis import strategies as st
from polars.testing import assert_frame_equal

import tparquet as ppc
import tparquet.polars as ppl

EPOCH = datetime.datetime(1970, 1, 1)
UNITS = {"ms": 1_000, "us": 1_000_000, "ns": 1_000_000_000}

#: column name -> Polars dtype of the generated frame
DTYPES: dict[str, pl.DataType] = {
    "i8": pl.Int8(),
    "i16": pl.Int16(),
    "i32": pl.Int32(),
    "i64": pl.Int64(),
    "u32": pl.UInt32(),
    "f64": pl.Float64(),
    "d": pl.Date(),
    "t_ms": pl.Datetime("ms"),
    "t_us_utc": pl.Datetime("us", "UTC"),
    "t_ns_tokyo": pl.Datetime("ns", "Asia/Tokyo"),
    "s": pl.String(),
    "b": pl.Boolean(),
}
BITS = {"i8": 8, "i16": 16, "i32": 32, "i64": 64}


@st.composite
def series_values(draw: st.DrawFn, name: str, n: int) -> list[Any]:
    """Values for column `name`: sorted runs (what pruning works on) or anything, with nulls."""
    if name in BITS:
        lo, hi = -(2 ** (BITS[name] - 1)), 2 ** (BITS[name] - 1) - 1
        small = draw(st.booleans())
        ints = st.integers(-50, 50) if small else st.integers(lo, hi)
    elif name == "u32":
        ints = st.integers(0, 2**32 - 1)
    elif name in ("d",):
        ints = st.integers(-1000, 1000)
    else:
        ints = st.integers(-(10**6), 10**6)
    values: list[Any] = draw(st.lists(ints, min_size=n, max_size=n))
    if draw(st.integers(0, 3)):
        values.sort()
    if name == "f64":
        values = [v / 4 for v in values]
        if n and draw(st.booleans()):
            values[draw(st.integers(0, n - 1))] = float("nan")
    elif name == "d":
        values = [datetime.date(2000, 1, 1) + datetime.timedelta(days=v) for v in values]
    elif name.startswith("t_"):
        values = [EPOCH + datetime.timedelta(seconds=v) for v in values]
    elif name == "s":
        values = [f"k{v % 13}" for v in values]
    elif name == "b":
        values = [v % 2 == 0 for v in values]
    mask = draw(st.lists(st.booleans(), min_size=n, max_size=n)) if draw(st.booleans()) else [False] * n
    return [None if m else v for v, m in zip(values, mask)]


@st.composite
def frames(draw: st.DrawFn) -> pl.DataFrame:
    n = draw(st.integers(0, 80))
    names = draw(st.lists(st.sampled_from(list(DTYPES)), min_size=1, max_size=4, unique=True))
    columns = {}
    for name in names:
        dtype = DTYPES[name]
        values = draw(series_values(name, n))
        if isinstance(dtype, pl.Datetime):
            s = pl.Series(name, values, dtype=pl.Datetime(dtype.time_unit))
            if dtype.time_zone is not None:
                s = s.dt.replace_time_zone("UTC").dt.convert_time_zone(dtype.time_zone)
        else:
            s = pl.Series(name, values, dtype=dtype)
        columns[name] = s
    return pl.DataFrame(columns)


def literal(draw: st.DrawFn, name: str, df: pl.DataFrame) -> Any:
    """A literal to compare column `name` with: often one of its values, or one next to them."""
    dtype = DTYPES[name]
    present: list[Any] = [v for v in df[name].to_list() if v is not None]
    v: Any
    if draw(st.booleans()) and present:
        v = draw(st.sampled_from(present))
    else:
        v = draw(series_values(name, 1))[0]
        if v is None:
            return None
    if name in BITS or name == "u32":
        v = v + draw(st.sampled_from([-1, 0, 0, 1]))
        kind = draw(st.sampled_from(["plain", "plain", "float", "typed", "cast"]))
        if kind == "float":
            return float(v) + draw(st.sampled_from([0.0, 0.5]))
        if kind == "typed":
            return pl.lit(v, dtype=pl.Int64())
        if kind == "cast" and name in BITS:
            return pl.lit(v).cast(dtype, strict=False)
        return v
    if name == "d" and draw(st.booleans()):
        # a datetime literal against a date column
        return datetime.datetime.combine(v, datetime.time(draw(st.integers(0, 23))))
    if isinstance(dtype, pl.Datetime):
        v = v + datetime.timedelta(microseconds=draw(st.sampled_from([-1, 0, 0, 1])))
        if dtype.time_zone is not None:
            # Polars compares zoned datetimes of one zone only
            v = v.replace(tzinfo=datetime.timezone.utc).astimezone(zoneinfo.ZoneInfo(dtype.time_zone))
        if draw(st.booleans()):
            return pl.lit(v).dt.cast_time_unit(draw(st.sampled_from(["ms", "us", "ns"])))
    return v


@st.composite
def predicates(draw: st.DrawFn, df: pl.DataFrame, depth: int = 0) -> pl.Expr:
    if depth < 2 and draw(st.integers(0, 3)) == 0:
        a = draw(predicates(df, depth + 1))
        b = draw(predicates(df, depth + 1))
        return draw(st.sampled_from([a & b, a | b, ~a, a & ~b]))
    name = draw(st.sampled_from(df.columns))
    c = pl.col(name)
    kind = draw(st.sampled_from(["cmp", "cmp", "cmp", "flip", "between", "is_in", "null"]))
    if kind == "null":
        return c.is_null() if draw(st.booleans()) else c.is_not_null()
    v = literal(draw, name, df)
    if v is None:
        return c.is_null()
    if kind == "between":
        w = literal(draw, name, df)
        if w is None:
            return c.is_not_null()
        closed: Literal["both", "left", "right", "none"] = draw(st.sampled_from(["both", "left", "right", "none"]))
        return c.is_between(v, w, closed=closed)
    if kind == "is_in":
        if isinstance(v, pl.Expr):
            return c == v
        w = literal(draw, name, df)
        return c.is_in([v, w] if type(w) is type(v) else [v])
    op = draw(st.sampled_from(["<", "<=", ">", ">=", "==", "!="]))
    lit = v if isinstance(v, pl.Expr) else pl.lit(v)
    ops: dict[str, Callable[[pl.Expr, pl.Expr], pl.Expr]] = {
        "<": lambda a, b: a < b,
        "<=": lambda a, b: a <= b,
        ">": lambda a, b: a > b,
        ">=": lambda a, b: a >= b,
        "==": lambda a, b: a == b,
        "!=": lambda a, b: a != b,
    }
    return ops[op](lit, c) if kind == "flip" else ops[op](c, lit)


def outcome(run: Any) -> Any:
    """The frame `run()` returns, or the type of the error it raises."""
    try:
        return run()
    except (pl.exceptions.PolarsError, TypeError, ValueError) as e:
        return type(e)


def file_of(df: pl.DataFrame, rows: int) -> io.BytesIO:
    buf = io.BytesIO()
    ppl.write_parquet(df, buf, row_group_size=rows)
    return io.BytesIO(buf.getvalue())


@st.composite
def cases(draw: st.DrawFn) -> tuple[pl.DataFrame, pl.Expr, int]:
    df = draw(frames())
    return df, draw(predicates(df)), draw(st.integers(1, 30))


@given(case=cases(), engine=st.sampled_from(["in-memory", "streaming"]))
def test_a_filter_gives_the_rows_polars_gives(case: tuple[pl.DataFrame, pl.Expr, int], engine: Any) -> None:
    df, predicate, rows = case
    want = outcome(lambda: df.filter(predicate))
    got = outcome(lambda: ppl.scan_parquet(file_of(df, rows)).filter(predicate).collect(engine=engine))
    if isinstance(want, pl.DataFrame):
        assert isinstance(got, pl.DataFrame), got
        assert_frame_equal(got, want)
    else:
        assert not isinstance(got, pl.DataFrame)


@given(case=cases(), limit=st.integers(0, 90), batch=st.sampled_from([None, 1, 7, 64]), data=st.data())
def test_limits_projections_and_filters_compose(
    case: tuple[pl.DataFrame, pl.Expr, int], limit: int, batch: int | None, data: st.DataObject
) -> None:
    df, predicate, rows = case
    keep = data.draw(st.lists(st.sampled_from(df.columns), min_size=1, unique=True))
    queries = [
        lambda lf: lf.head(limit).filter(predicate).select(keep),
        lambda lf: lf.filter(predicate).head(limit).select(keep),
        lambda lf: lf.select([*keep, *predicate.meta.root_names()]).filter(predicate).select(keep),
        lambda lf: lf.filter(predicate).select(pl.len()),
        lambda lf: lf.slice(limit // 2, limit).filter(predicate),
    ]
    for query in queries:
        want = outcome(lambda q=query: q(df.lazy()).collect())
        got = outcome(lambda q=query: q(ppl.scan_parquet(file_of(df, rows), batch_size=batch)).collect())
        if isinstance(want, pl.DataFrame):
            assert isinstance(got, pl.DataFrame), got
            assert_frame_equal(got, want)


@given(case=cases())
def test_skipped_row_groups_cannot_match(case: tuple[pl.DataFrame, pl.Expr, int]) -> None:
    """Every row group the pruner skips has no matching row (checked row group by row group)."""
    df, predicate, rows = case
    f = ppc.ParquetFile(file_of(df, rows))
    schema = pl.DataFrame(f.empty()).schema
    kept = set(ppl._row_groups(f, predicate, schema))
    for i in range(f.num_row_groups):
        if i not in kept:
            group = pl.DataFrame(f.stream(row_groups=[i]))
            matching = outcome(lambda g=group: g.filter(predicate).height)
            assert matching in (0, pl.exceptions.InvalidOperationError, pl.exceptions.ComputeError), i


@pytest.mark.parametrize("unit", ["s", "ms", "us", "ns"])
@pytest.mark.parametrize("tz", [None, "UTC", "America/New_York"])
def test_timestamps_of_every_unit_and_zone(unit: str, tz: str | None) -> None:
    # written by pyarrow-typed data (Polars has no seconds)
    values = [None if i % 9 == 4 else i * 3600 * 10**3 for i in range(60)]
    table = pa.table({"t": pa.array(values, pa.timestamp("ms", tz)).cast(pa.timestamp(unit, tz))})
    buf = io.BytesIO()
    ppc.write_table(table, buf, row_group_size=7)
    df = pl.DataFrame(table)
    zone = None if tz is None else zoneinfo.ZoneInfo(tz)
    for when in [datetime.datetime(1970, 1, 2, 3, tzinfo=zone), datetime.datetime(1970, 1, 1, tzinfo=zone)]:
        for predicate in [
            pl.col("t") < when,
            pl.col("t") >= when,
            pl.col("t") == when,
            pl.col("t").is_between(when, when),
        ]:
            got = ppl.scan_parquet(io.BytesIO(buf.getvalue())).filter(predicate).collect()
            assert_frame_equal(got, df.filter(predicate))


PRUNABLE = ["i8", "i16", "i32", "i64", "d", "t_ms", "t_us_utc", "t_ns_tokyo"]


@st.composite
def comparisons(draw: st.DrawFn) -> tuple[pl.DataFrame, str, str, Any, int]:
    """A frame with one prunable column, a comparison of it with a plain literal of its type,
    and a row group size."""
    name = draw(st.sampled_from(PRUNABLE))
    n = draw(st.integers(1, 60))
    dtype = DTYPES[name]
    values = draw(series_values(name, n))
    if isinstance(dtype, pl.Datetime):
        s = pl.Series(name, values, dtype=pl.Datetime(dtype.time_unit))
        if dtype.time_zone is not None:
            s = s.dt.replace_time_zone("UTC").dt.convert_time_zone(dtype.time_zone)
    else:
        s = pl.Series(name, values, dtype=dtype)
    df = pl.DataFrame({name: s})
    rows = draw(st.integers(1, 15))
    # the edges of the row groups, where pruning decides
    edges: list[Any] = [
        e
        for i in range(0, n, rows)
        for e in (df[name].slice(i, rows).min(), df[name].slice(i, rows).max())
        if e is not None
    ]
    v: Any
    one: Any
    if draw(st.integers(0, 4)) and edges:
        v = draw(st.sampled_from(edges))
    else:
        v = next((x for x in draw(series_values(name, 4)) if x is not None), None)
        if v is None:
            v = {"d": datetime.date(2000, 1, 1)}.get(name, EPOCH if name.startswith("t_") else 0)
    one = {"d": datetime.timedelta(days=1)}.get(name, datetime.timedelta(seconds=1) if name.startswith("t_") else 1)
    if isinstance(dtype, pl.Datetime) and draw(st.booleans()):
        one = datetime.timedelta(microseconds=1) if dtype.time_unit != "ms" else datetime.timedelta(milliseconds=1)
    v = v + one * draw(st.sampled_from([-1, 0, 0, 1]))
    if isinstance(dtype, pl.Datetime) and dtype.time_zone is not None and v.tzinfo is None:
        v = v.replace(tzinfo=datetime.timezone.utc).astimezone(zoneinfo.ZoneInfo(dtype.time_zone))
    if name in BITS:
        v = max(-(2 ** (BITS[name] - 1)), min(2 ** (BITS[name] - 1) - 1, v))
    op = draw(st.sampled_from(["<", "<=", ">", ">=", "==", "between"]))
    return df, name, op, v, rows


# cheap examples: five times the profile's, as the edges of row groups are what matters
@settings(max_examples=settings().max_examples * 5)
@given(case=comparisons(), flip=st.booleans())
def test_simple_comparisons_keep_exactly_the_row_groups_that_can_match(
    case: tuple[pl.DataFrame, str, str, Any, int], flip: bool
) -> None:
    df, name, op, v, rows = case
    # a literal of the column's own type (a Python datetime is in microseconds)
    c, lit = pl.col(name), pl.lit(v) if name in BITS else pl.lit(v, dtype=df.schema[name])
    exprs = {
        "<": (c < lit, lit > c),
        "<=": (c <= lit, lit >= c),
        ">": (c > lit, lit < c),
        ">=": (c >= lit, lit <= c),
        "==": (c == lit, lit == c),
        "between": (c.is_between(lit, lit), c.is_between(lit, lit)),
    }
    f = ppc.ParquetFile(file_of(df, rows))
    kept = ppl._row_groups(f, exprs[op][flip], pl.DataFrame(f.empty()).schema)
    may: Callable[[Any, Any], bool] = {
        "<": lambda lo, hi: lo < v,
        "<=": lambda lo, hi: lo <= v,
        ">": lambda lo, hi: hi > v,
        ">=": lambda lo, hi: hi >= v,
        "==": lambda lo, hi: lo <= v <= hi,
        "between": lambda lo, hi: lo <= v <= hi,
    }[op]
    # a row group can match when a value between its smallest and largest does; one with
    # nulls only has no bounds and is kept
    want = []
    for i in range(f.num_row_groups):
        group = df.slice(i * rows, rows)[name].drop_nulls()
        if not group.len() or may(group.min(), group.max()):
            want.append(i)
    assert kept == want
