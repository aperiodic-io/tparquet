"""The writer options of DataFrame.tparquet.write and LazyFrame.tparquet.sink: each is used, and a
bad one is refused with a message that names it, before anything is created or run."""

from __future__ import annotations

import io
import os
import pathlib
from collections.abc import Callable
from typing import Any

import polars as pl
import pytest
import tparquet
from polars.testing import assert_frame_equal

import polars_tparquet as pt
from conftest import VECTORS, ticks

#: Writes a frame with the given options: by DataFrame.tparquet.write or LazyFrame.tparquet.sink.
Writes = Callable[..., dict[str, str]]


def _write(df: pl.DataFrame, target: Any, **options: Any) -> dict[str, str]:
    codecs: dict[str, str] = df.tparquet.write(target, **options)  # type: ignore[attr-defined]
    return codecs


def _sink(df: pl.DataFrame, target: Any, **options: Any) -> dict[str, str]:
    codecs: dict[str, str] = df.lazy().tparquet.sink(target, **options)  # type: ignore[attr-defined]
    return codecs


@pytest.fixture(params=[_write, _sink], ids=["write", "sink"])
def writes(request: pytest.FixtureRequest) -> Writes:
    how: Writes = request.param
    return how


BAD: list[tuple[dict[str, Any], type[Exception], str | None]] = [
    ({"policy": "fast"}, tparquet.PcodecError, r'^unknown policy "fast": expected auto, pcodec or base$'),
    ({"policy": "PCODEC"}, tparquet.PcodecError, r'^unknown policy "PCODEC"'),
    ({"level": -1}, tparquet.PcodecError, r"^level -1 is not in 0\.\.=12$"),
    ({"level": 13}, tparquet.PcodecError, r"^level 13 is not in 0\.\.=12$"),
    ({"level": 2**70}, tparquet.PcodecError, rf"^level {2**70} is out of range$"),
    ({"level": "8"}, TypeError, "level"),
    ({"level": 8.0}, TypeError, "level"),
    ({"base": "lz77"}, tparquet.PcodecError, r'^base codec "lz77": .*unsupported compression lz77'),
    ({"base": "pcodec"}, tparquet.PcodecError, r"^the base codec cannot be PCODEC$"),
    ({"base": "zstd(99)"}, tparquet.PcodecError, r'^base codec "zstd\(99\)"'),
    ({"base": ""}, tparquet.PcodecError, r'^base codec ""'),
    ({"columns": {"ts": "fast"}}, tparquet.PcodecError, r'^unknown policy "fast"'),
    ({"columns": {"nope": "pcodec"}}, tparquet.PcodecError, r'^no column named "nope"$'),
    ({"columns": {"TS": "pcodec"}}, tparquet.PcodecError, r'^no column named "TS"$'),
    ({"columns": {"ts.x": "pcodec"}}, tparquet.PcodecError, r'^no column named "ts\.x"$'),
    ({"row_group_size": 0}, tparquet.PcodecError, r"^row_group_size must be positive, got 0$"),
    ({"row_group_size": -5}, tparquet.PcodecError, r"^row_group_size must be positive, got -5$"),
]


@pytest.mark.parametrize(("options", "error", "message"), BAD, ids=[repr(o) for o, _, _ in BAD])
def test_a_bad_option_is_refused_and_leaves_no_file(
    tmp_path: pathlib.Path, writes: Writes, options: dict[str, Any], error: type[Exception], message: str | None
) -> None:
    with pytest.raises(error, match=message):
        writes(ticks(10), tmp_path / "t.parquet", **options)
    with pytest.raises(error, match=message):
        writes(ticks(10), io.BytesIO(), **options)
    assert os.listdir(tmp_path) == []


def test_a_bad_option_runs_no_query(tmp_path: pathlib.Path) -> None:
    calls: list[int] = []

    def counted(df: pl.DataFrame) -> pl.DataFrame:
        calls.append(df.height)
        return df

    lf = ticks(10).lazy().map_batches(counted, schema=ticks(1).schema)
    for options in ({"level": 13}, {"base": "lz77"}, {"columns": {"nope": "pcodec"}}, {"chunk_size": -1}):
        with pytest.raises((tparquet.PcodecError, ValueError)):
            lf.tparquet.sink(tmp_path / "t.parquet", **options)  # type: ignore[attr-defined]
    assert calls == []
    assert os.listdir(tmp_path) == []


@pytest.mark.parametrize("chunk_size", [0, -1])
def test_a_bad_chunk_size(tmp_path: pathlib.Path, chunk_size: int) -> None:
    with pytest.raises(ValueError, match=rf"^chunk_size must be positive, got {chunk_size}$"):
        ticks(10).lazy().tparquet.sink(tmp_path / "x.parquet", chunk_size=chunk_size)  # type: ignore[attr-defined]
    assert os.listdir(tmp_path) == []


@pytest.mark.parametrize("level", range(13))
def test_every_level_round_trips(writes: Writes, level: int) -> None:
    buf = io.BytesIO()
    assert writes(ticks(500), buf, policy="pcodec", level=level)["ts"] == "PCODEC"
    assert_frame_equal(pt.read_tparquet(io.BytesIO(buf.getvalue())), ticks(500))


@pytest.mark.parametrize(
    ("base", "codec"),
    [
        ("zstd(3)", "ZSTD"),
        ("zstd(1)", "ZSTD"),
        ("zstd(22)", "ZSTD"),
        ("ZSTD(3)", "ZSTD"),
        ("snappy", "SNAPPY"),
        ("SNAPPY", "SNAPPY"),
        ("uncompressed", "UNCOMPRESSED"),
        ("gzip(6)", "GZIP"),
        ("brotli(1)", "BROTLI"),
        ("lz4_raw", "LZ4_RAW"),
    ],
)
def test_every_base_codec(writes: Writes, base: str, codec: str) -> None:
    buf = io.BytesIO()
    codecs = writes(ticks(500), buf, policy="base", base=base)
    assert set(codecs.values()) == {codec}
    assert_frame_equal(pt.read_tparquet(io.BytesIO(buf.getvalue())), ticks(500))


def test_the_default_base_is_zstd(writes: Writes) -> None:
    assert set(writes(ticks(500), io.BytesIO(), policy="base").values()) == {"ZSTD"}


@pytest.mark.parametrize(
    ("policy", "want"),
    [
        ("pcodec", {"ts": "PCODEC", "price": "PCODEC", "qty": "PCODEC", "sym": "ZSTD"}),
        ("base", {"ts": "ZSTD", "price": "ZSTD", "qty": "ZSTD", "sym": "ZSTD"}),
        ("auto", {"ts": "PCODEC", "price": "PCODEC", "qty": "ZSTD", "sym": "ZSTD"}),
    ],
)
def test_the_policies(writes: Writes, policy: str, want: dict[str, str]) -> None:
    """A string column never gets PCODEC: it applies to INT32, INT64, FLOAT and DOUBLE only."""
    assert writes(ticks(5000), io.BytesIO(), policy=policy) == want


def test_the_default_policy_is_auto(writes: Writes) -> None:
    assert writes(ticks(5000), io.BytesIO()) == writes(ticks(5000), io.BytesIO(), policy="auto")


def test_a_column_policy_overrides_the_policy(writes: Writes) -> None:
    codecs = writes(ticks(5000), io.BytesIO(), policy="base", columns={"qty": "pcodec", "sym": "pcodec"})
    assert codecs == {"ts": "ZSTD", "price": "ZSTD", "qty": "PCODEC", "sym": "ZSTD"}
    codecs = writes(ticks(5000), io.BytesIO(), policy="pcodec", columns={"ts": "auto", "price": "base"})
    assert codecs == {"ts": "PCODEC", "price": "ZSTD", "qty": "PCODEC", "sym": "ZSTD"}


def test_a_leaf_path_or_a_struct_takes_a_policy(writes: Writes) -> None:
    nested = pt.read_tparquet(next(p for p in VECTORS if p.stem == "nested_struct"))
    buf = io.BytesIO()
    codecs = writes(nested, buf, policy="base", columns={"book.quote.bid": "pcodec", "book.tags.list.item": "pcodec"})
    assert codecs == {
        "book.id": "ZSTD",
        "book.quote.ask": "ZSTD",
        "book.quote.bid": "PCODEC",
        "book.quote.size": "ZSTD",
        "book.tags.list.item": "PCODEC",
    }
    assert_frame_equal(pt.read_tparquet(io.BytesIO(buf.getvalue())), nested)
    whole = writes(nested, io.BytesIO(), policy="base", columns={"book": "pcodec"})
    assert set(whole.values()) == {"PCODEC"}
    struct = writes(nested, io.BytesIO(), policy="base", columns={"book.quote": "pcodec"})
    assert [k for k, v in struct.items() if v == "PCODEC"] == ["book.quote.ask", "book.quote.bid", "book.quote.size"]


@pytest.mark.parametrize(
    ("rows", "size", "groups"), [(1000, 1, 1000), (1000, 999, 2), (1000, 1000, 1), (1000, 10**9, 1)]
)
def test_row_group_size(writes: Writes, rows: int, size: int, groups: int) -> None:
    buf = io.BytesIO()
    writes(ticks(rows), buf, row_group_size=size)
    meta = tparquet.ParquetFile(io.BytesIO(buf.getvalue())).metadata
    assert meta.num_row_groups == groups
    assert max(meta.row_group_rows) == min(size, rows)
    assert_frame_equal(pt.read_tparquet(io.BytesIO(buf.getvalue())), ticks(rows))


def test_options_are_keywords(tmp_path: pathlib.Path) -> None:
    with pytest.raises(TypeError):
        ticks(1).tparquet.write(tmp_path / "t.parquet", "pcodec")  # type: ignore[attr-defined]
    with pytest.raises(TypeError):
        ticks(1).lazy().tparquet.sink(tmp_path / "t.parquet", "pcodec")  # type: ignore[attr-defined]
    assert os.listdir(tmp_path) == []


def test_what_is_not_a_target(writes: Writes) -> None:
    for bad, name in ((3, "int"), (None, "NoneType"), (io.StringIO(), None)):
        with pytest.raises(TypeError, match=f"got {name}$" if name else None):
            writes(ticks(10), bad)
