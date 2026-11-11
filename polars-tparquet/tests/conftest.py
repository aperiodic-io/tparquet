"""Fixtures of the polars-tparquet tests."""

from __future__ import annotations

import base64
import json
import os
import pathlib
from collections.abc import Callable
from typing import Any, Union

import polars as pl
import pytest
import tparquet
from hypothesis import HealthCheck, settings
from polars.datatypes import DataTypeClass
from tparquet.polars import write_parquet

_slow_ok = [HealthCheck.too_slow, HealthCheck.function_scoped_fixture]
# HYPOTHESIS_PROFILE: "default", "thorough" (CI weekly) or "mutation" (fixed examples)
settings.register_profile("default", max_examples=40, deadline=None, suppress_health_check=_slow_ok)
settings.register_profile("thorough", max_examples=400, deadline=None, suppress_health_check=_slow_ok)
settings.register_profile("mutation", max_examples=10, deadline=None, derandomize=True, suppress_health_check=_slow_ok)
settings.load_profile(os.environ.get("HYPOTHESIS_PROFILE", "default"))

REPO = pathlib.Path(__file__).resolve().parents[2]
TESTDATA = pathlib.Path(os.environ.get("TPARQUET_TESTDATA", REPO / "testdata"))
VECTORS = sorted((TESTDATA / "files").glob("*.parquet"))


def ticks(n: int, offset: int = 0) -> pl.DataFrame:
    """A small time series: millisecond timestamps, 0.01-tick prices, a size with nulls, a symbol."""
    i = pl.col("i")
    return (
        pl.DataFrame({"i": pl.int_range(offset, offset + n, eager=True)})
        .select(
            ts=1_700_000_000_000 + i * 250,
            price=(10_000 + (i * 37) % 500) / 100,
            qty=pl.when(i % 7 == 0).then(None).otherwise((i % 50) + 1).cast(pl.Int32),
        )
        .with_columns(sym=pl.Series(["BTC", "ETH", "SOL"] * (n // 3 + 1))[:n])
    )


@pytest.fixture
def write(tmp_path: pathlib.Path) -> Callable[..., pathlib.Path]:
    """Writes a frame as a PCODEC file under tmp_path and returns its path."""

    def write(name: str, df: pl.DataFrame, row_group_size: int | None = None) -> pathlib.Path:
        path = tmp_path / name
        path.parent.mkdir(parents=True, exist_ok=True)
        write_parquet(df, path, row_group_size=row_group_size)
        return path

    return write


class Reads:
    """What every `ParquetFile.stream` call of the test asked for."""

    def __init__(self) -> None:
        self.calls: list[dict[str, Any]] = []

    def columns(self) -> list[Any]:
        return [c["columns"] for c in self.calls]

    def row_groups(self) -> list[Any]:
        return [c["row_groups"] for c in self.calls]


@pytest.fixture
def reads(monkeypatch: pytest.MonkeyPatch) -> Reads:
    spy = Reads()
    stream = tparquet.ParquetFile.stream

    def recording(self: tparquet.ParquetFile, *args: Any, **kwargs: Any) -> Any:
        names = ["columns", "row_groups", "batch_size", "limit"]
        call = dict(zip(names, args))
        call.update(kwargs)
        spy.calls.append({n: call.get(n) for n in names})
        return stream(self, *args, **kwargs)

    monkeypatch.setattr(tparquet.ParquetFile, "stream", recording)
    return spy


#: A dtype as Polars gives it: an instance, or a class such as pl.Int64.
DType = Union[pl.DataType, DataTypeClass]

#: Each float dtype: the unsigned integer of its bits, and the hex digits canonical JSON gives them.
_BITS: dict[Any, tuple[pl.DataType, int]] = {
    pl.Float16: (pl.UInt16(), 4),
    pl.Float32: (pl.UInt32(), 8),
    pl.Float64: (pl.UInt64(), 16),
}


def float_bits(s: pl.Series) -> list[int | None]:
    """The IEEE 754 bits of each value of a float Series (None for a null)."""
    bits: list[int | None] = s.reinterpret(dtype=_BITS[s.dtype.base_type()][0]).to_list()
    return bits


def _entries(dtype: pl.Map) -> pl.List:
    """A map as canonical JSON writes it: a list of its entries, `{"key":...,"value":...}`, in
    stored order (a Map value in Python is a dict, which would lose duplicates and the order)."""
    return pl.List(pl.Struct({"key": dtype.key, "value": dtype.value}))


def _stored(e: pl.Expr, dtype: DType) -> pl.Expr:
    """`e` with every value as canonical JSON stores it: floats as their bits, temporal values
    and decimals as their stored integer, dictionaries decoded; nesting kept."""
    if dtype.base_type() in _BITS:
        return e.reinterpret(dtype=_BITS[dtype.base_type()][0])
    if dtype == pl.Time:
        return e.to_physical() // 1000  # Polars holds nanoseconds; the vectors store time64(us)
    if dtype.is_temporal() or dtype.is_decimal():
        return e.to_physical()
    if isinstance(dtype, (pl.Categorical, pl.Enum)):
        return e.cast(pl.String)
    if isinstance(dtype, pl.Map):
        return _stored(e.cast(_entries(dtype)), _entries(dtype))
    if isinstance(dtype, (pl.List, pl.Array)):
        inner = dtype.inner
        assert inner is not None
        # arr.to_list: a cast to List panics in list.eval on values masked out by nulls (Polars 2.1)
        listed = e.arr.to_list() if isinstance(dtype, pl.Array) else e
        return listed.list.eval(_stored(pl.element(), inner))
    if isinstance(dtype, pl.Struct):
        fields = [_stored(e.struct.field(f.name), f.dtype).alias(f.name) for f in dtype.fields]
        return pl.when(e.is_null()).then(None).otherwise(pl.struct(fields))
    return e


def _json(value: Any, dtype: DType) -> str:
    if value is None:
        return "null"
    base = dtype.base_type()
    if base in _BITS:
        return f'"0x{value:0{_BITS[base][1]}x}"'
    if dtype == pl.Boolean:
        return "true" if value else "false"
    if dtype.is_numeric() or dtype.is_temporal():
        return str(int(value))
    if dtype == pl.Binary:
        return json.dumps(base64.b64encode(value).decode())
    if isinstance(dtype, pl.Map):
        return _json(value, _entries(dtype))
    if isinstance(dtype, (pl.List, pl.Array)):
        inner = dtype.inner
        assert inner is not None
        return "[" + ",".join(_json(v, inner) for v in value) + "]"
    if isinstance(dtype, pl.Struct):
        return "{" + ",".join(f"{json.dumps(f.name)}:{_json(value[f.name], f.dtype)}" for f in dtype.fields) + "}"
    return json.dumps(value, ensure_ascii=False)


def canonical(df: pl.DataFrame) -> str:
    """`df` as the canonical JSON of testdata/README.md: one line per row, `\\n`-terminated."""
    stored = df.select(_stored(pl.col(name), dtype).alias(name) for name, dtype in df.schema.items())
    schema = list(df.schema.items())
    return "".join(
        "{" + ",".join(f"{json.dumps(n)}:{_json(v, t)}" for (n, t), v in zip(schema, row)) + "}\n"
        for row in stored.iter_rows()
    )
