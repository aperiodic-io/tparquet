"""Every Polars dtype that Parquet can hold, written, sunk and scanned, with nulls everywhere."""

from __future__ import annotations

import datetime as dt
import io
import os
import pathlib
from decimal import Decimal

import polars as pl
import pytest
import tparquet
from polars.testing import assert_frame_equal

import polars_tparquet as pt
from conftest import float_bits

D = dt.datetime
TIMES = [D(2020, 1, 1, 1, 2, 3, 456789), None, D(1970, 1, 1), D(1900, 1, 1, 0, 0, 0, 1), D(2262, 1, 1)]
ZONES = [None, "UTC", "Asia/Kolkata", "America/New_York", "Australia/Lord_Howe"]

#: One column per case: five values, the second null.
COLUMNS: dict[str, pl.Series] = {
    "Int8": pl.Series([1, None, -128, 127, 0], dtype=pl.Int8),
    "Int16": pl.Series([1, None, -32768, 32767, 0], dtype=pl.Int16),
    "Int32": pl.Series([1, None, -(2**31), 2**31 - 1, 0], dtype=pl.Int32),
    "Int64": pl.Series([1, None, -(2**63), 2**63 - 1, 0], dtype=pl.Int64),
    "UInt8": pl.Series([1, None, 0, 255, 128], dtype=pl.UInt8),
    "UInt16": pl.Series([1, None, 0, 65535, 32768], dtype=pl.UInt16),
    "UInt32": pl.Series([1, None, 0, 2**32 - 1, 2**31], dtype=pl.UInt32),
    "UInt64": pl.Series([1, None, 0, 2**64 - 1, 2**63], dtype=pl.UInt64),
    "Float16": pl.Series([1.5, None, -0.0, float("inf"), 65504.0], dtype=pl.Float16),
    "Float32": pl.Series([1.5, None, -0.0, float("nan"), 3.4e38], dtype=pl.Float32),
    "Float64": pl.Series([1.5, None, -0.0, float("-inf"), 5e-324], dtype=pl.Float64),
    "Decimal(9, 2)": pl.Series([Decimal("1.25"), None, Decimal("-9999999.99"), Decimal(0), Decimal("0.01")]),
    "Decimal(18, 4)": pl.Series([Decimal("1.25"), None, Decimal(-1), Decimal(0), Decimal("0.0001")]).cast(
        pl.Decimal(18, 4)
    ),
    "Decimal(38, 0)": pl.Series([10**37, None, -(10**37), 0, 1]).cast(pl.Decimal(38, 0)),
    "Date": pl.Series([dt.date(2020, 1, 1), None, dt.date(1970, 1, 1), dt.date(1, 1, 1), dt.date(9999, 12, 31)]),
    "Time": pl.Series([dt.time(1, 2, 3), None, dt.time(0), dt.time(23, 59, 59, 999999), dt.time(12)]),
    "Boolean": pl.Series([True, None, False, True, False]),
    "String": pl.Series(["a", None, "", 'ünïcödé "quoted"\n\x00', "x" * 1000]),
    "Binary": pl.Series([b"a", None, b"", b"\x00\xff", b"x" * 1000]),
    "Categorical": pl.Series(["a", None, "b", "a", "c"], dtype=pl.Categorical),
    "Enum": pl.Series(["a", None, "b", "a", "c"], dtype=pl.Enum(["c", "b", "a"])),
    "List(Int64)": pl.Series([[1, 2], None, [], [None], [3]], dtype=pl.List(pl.Int64)),
    "List(String)": pl.Series([["a"], None, [], [None, ""], ["b", "c"]]),
    "List(List(Int16))": pl.Series([[[1]], None, [[]], [None, [2]], [[3, 4]]], dtype=pl.List(pl.List(pl.Int16))),
    "List(Struct)": pl.Series([[{"a": 1.0, "b": "x"}], None, [], [None], [{"a": None, "b": None}]]),
    "Array(Int32, 2)": pl.Series([[1, 2], None, [3, 4], [None, 5], [6, 7]], dtype=pl.Array(pl.Int32, 2)),
    "Array(Float64, (2, 2))": pl.Series(
        [
            [[1.0, 2.0], [3.0, 4.0]],
            None,
            [[None, 0.0], [-0.0, 1.0]],
            [[5.0, 6.0], [7.0, 8.0]],
            [[9.0, 9.5], [1.0, 2.0]],
        ],
        dtype=pl.Array(pl.Float64, (2, 2)),
    ),
    "Map(Int32, Float64)": pl.Series(
        [
            [{"key": 1, "value": 2.5}, {"key": 2, "value": None}],
            None,
            [],
            [{"key": -1, "value": -0.0}],
            [{"key": 0, "value": 1.0}],
        ],
        dtype=pl.List(pl.Struct({"key": pl.Int32, "value": pl.Float64})),
    ).cast(pl.Map(pl.Int32, pl.Float64)),
    "Struct": pl.Series([{"x": 1, "y": "a"}, None, {"x": None, "y": None}, {"x": 3, "y": "c"}, {"x": 4, "y": "d"}]),
    "Struct(List, Struct)": pl.Series(
        [
            {"l": [1.5], "s": {"t": D(2020, 1, 1)}},
            None,
            {"l": None, "s": None},
            {"l": [], "s": {"t": None}},
            {"l": [None, 2.5], "s": {"t": D(1970, 1, 1)}},
        ]
    ),
}
for unit in ("ms", "us", "ns"):
    for zone in ZONES:
        naive = pl.Series(TIMES).cast(pl.Datetime(unit))
        COLUMNS[f"Datetime({unit}, {zone})"] = naive.dt.replace_time_zone(zone) if zone else naive
    COLUMNS[f"Duration({unit})"] = pl.Series(
        [dt.timedelta(1), None, dt.timedelta(0), dt.timedelta(microseconds=-5), dt.timedelta(days=-100_000)]
    ).cast(pl.Duration(unit))


def frame(name: str) -> pl.DataFrame:
    return COLUMNS[name].alias("c").to_frame().with_row_index("i")


@pytest.mark.parametrize("name", COLUMNS)
def test_every_dtype_round_trips(tmp_path: pathlib.Path, name: str) -> None:
    df = frame(name)
    path = tmp_path / "w.parquet"
    df.tparquet.write(path)  # type: ignore[attr-defined]
    assert_frame_equal(pt.read_tparquet(path), df, check_exact=True)
    assert_frame_equal(pt.scan_tparquet(path).collect(engine="streaming"), df, check_exact=True)
    assert_frame_equal(pt.scan_tparquet(path).filter(pl.col("i") > 1).collect(), df.slice(2), check_exact=True)
    sunk = tmp_path / "s.parquet"
    df.lazy().tparquet.sink(sunk, chunk_size=2, row_group_size=3)  # type: ignore[attr-defined]
    assert_frame_equal(pt.read_tparquet(sunk), df, check_exact=True)
    # the oracle: Polars' own reader, on a standard copy
    standard = tmp_path / "standard.parquet"
    tparquet.transcode(path, standard, target="standard")
    assert_frame_equal(pl.read_parquet(standard), df, check_exact=True)
    # and a file Polars wrote reads the same through the plugin
    df.write_parquet(tmp_path / "polars.parquet")
    assert_frame_equal(pt.read_tparquet(tmp_path / "polars.parquet"), df, check_exact=True)


@pytest.mark.parametrize("name", COLUMNS)
def test_every_dtype_all_null(tmp_path: pathlib.Path, name: str) -> None:
    df = pl.DataFrame({"c": pl.Series([None] * 5, dtype=COLUMNS[name].dtype)})
    for policy in ("auto", "pcodec", "base"):
        df.tparquet.write(tmp_path / f"{policy}.parquet", policy=policy)  # type: ignore[attr-defined]
        assert_frame_equal(pt.read_tparquet(tmp_path / f"{policy}.parquet"), df)
    df.lazy().tparquet.sink(tmp_path / "s.parquet")  # type: ignore[attr-defined]
    assert_frame_equal(pt.read_tparquet(tmp_path / "s.parquet"), df)


@pytest.mark.parametrize("name", COLUMNS)
def test_every_dtype_without_rows(tmp_path: pathlib.Path, name: str) -> None:
    df = frame(name).clear()
    df.tparquet.write(tmp_path / "w.parquet")  # type: ignore[attr-defined]
    df.lazy().tparquet.sink(tmp_path / "s.parquet")  # type: ignore[attr-defined]
    for path in (tmp_path / "w.parquet", tmp_path / "s.parquet"):
        assert_frame_equal(pt.read_tparquet(path), df)
        assert tparquet.ParquetFile(path).metadata.num_row_groups == 0


def test_every_dtype_in_one_frame(tmp_path: pathlib.Path) -> None:
    df = pl.DataFrame([s.alias(n) for n, s in COLUMNS.items()])
    for policy in ("auto", "pcodec", "base"):
        df.tparquet.write(tmp_path / "w.parquet", policy=policy, row_group_size=2)  # type: ignore[attr-defined]
        assert_frame_equal(pt.read_tparquet(tmp_path / "w.parquet"), df, check_exact=True)
    df.lazy().tparquet.sink(tmp_path / "s.parquet", chunk_size=1)  # type: ignore[attr-defined]
    assert_frame_equal(pt.read_tparquet(tmp_path / "s.parquet"), df, check_exact=True)
    picked = pt.read_tparquet(tmp_path / "s.parquet", columns=["Struct", "Int8", "Enum"])
    assert_frame_equal(picked, df.select("Struct", "Int8", "Enum"))


def test_float_bits_are_kept(tmp_path: pathlib.Path) -> None:
    """NaN payloads, signalling NaNs and signed zeros: assert_frame_equal does not compare bits."""
    f64 = [0x7FF8_0000_0000_0000, 0xFFF8_DEAD_BEEF_0001, 0x7FF0_0000_0000_0001, 0x8000_0000_0000_0000, 1, None]
    f32 = [0x7FC0_0000, 0xFFC0_BEEF, 0x7F80_0001, 0x8000_0000, 1, None]
    f16 = [0x7E00, 0xFE01, 0x7C01, 0x8000, 1, None]
    df = pl.DataFrame(
        {
            "f64": pl.Series(f64, dtype=pl.UInt64).reinterpret(dtype=pl.Float64),
            "f32": pl.Series(f32, dtype=pl.UInt32).reinterpret(dtype=pl.Float32),
            "f16": pl.Series(f16, dtype=pl.UInt16).reinterpret(dtype=pl.Float16),
        }
    )
    for policy in ("pcodec", "base", "auto"):
        df.tparquet.write(tmp_path / "w.parquet", policy=policy)  # type: ignore[attr-defined]
        df.lazy().tparquet.sink(tmp_path / "s.parquet", policy=policy, chunk_size=4)  # type: ignore[attr-defined]
        for path in ("w.parquet", "s.parquet"):
            out = pt.read_tparquet(tmp_path / path)
            assert [float_bits(out[c]) for c in df.columns] == [f64, f32, f16]


def test_categories_differing_between_batches(tmp_path: pathlib.Path) -> None:
    """A sink's batches each carry the categories they hold."""
    lf = pl.LazyFrame({"c": [f"v{i % 37}" for i in range(1000)]}, schema={"c": pl.Categorical})
    lf = lf.filter(pl.int_range(pl.len()) % 3 != 0)
    lf.tparquet.sink(tmp_path / "s.parquet", chunk_size=10)  # type: ignore[attr-defined]
    assert_frame_equal(pt.read_tparquet(tmp_path / "s.parquet"), lf.collect())


@pytest.mark.parametrize("dtype", [pl.Int128, pl.UInt128], ids=str)
def test_a_128_bit_integer_is_refused_and_leaves_no_file(tmp_path: pathlib.Path, dtype: pl.DataType) -> None:
    """Neither tparquet nor Parquet's Arrow mapping has 128-bit integers (Polars' own writer
    writes them, as a type tparquet does not read)."""
    df = pl.DataFrame({"c": pl.Series([1, None], dtype=dtype)})
    with pytest.raises(tparquet.PcodecError, match="not supported"):
        df.tparquet.write(tmp_path / "w.parquet")  # type: ignore[attr-defined]
    with pytest.raises(tparquet.PcodecError, match="not supported"):
        df.lazy().tparquet.sink(tmp_path / "s.parquet")  # type: ignore[attr-defined]
    assert os.listdir(tmp_path) == []
    df.write_parquet(tmp_path / "polars.parquet")
    with pytest.raises(tparquet.PcodecError, match=r"polars\.parquet: .*128.*not supported"):
        pt.scan_tparquet(tmp_path / "polars.parquet")


def test_a_null_column_written_by_polars_is_read() -> None:
    df = pl.DataFrame({"n": pl.Series([None] * 3, dtype=pl.Null), "i": [1, 2, 3]})
    buf = io.BytesIO()
    df.write_parquet(buf)
    assert_frame_equal(pt.read_tparquet(io.BytesIO(buf.getvalue())), df)


def test_a_null_column_is_written(tmp_path: pathlib.Path) -> None:
    df = pl.DataFrame({"n": pl.Series([None] * 3, dtype=pl.Null), "i": [1, 2, 3]})
    df.tparquet.write(tmp_path / "w.parquet")  # type: ignore[attr-defined]
    assert_frame_equal(pt.read_tparquet(tmp_path / "w.parquet"), df)


def test_a_column_that_cannot_be_written_leaves_no_file(tmp_path: pathlib.Path) -> None:
    # Int128 has no Arrow type in Rust (Null columns, which used to fail, are written now)
    lf = pl.LazyFrame({"i": [1, 2, 3]}).with_columns(n=pl.lit(1, dtype=pl.Int128))
    with pytest.raises(tparquet.PcodecError):
        lf.collect().tparquet.write(tmp_path / "w.parquet")  # type: ignore[attr-defined]
    with pytest.raises(tparquet.PcodecError):
        lf.tparquet.sink(tmp_path / "s.parquet")  # type: ignore[attr-defined]
    assert os.listdir(tmp_path) == []
