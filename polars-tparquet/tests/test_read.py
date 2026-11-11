"""read_tparquet: scan_tparquet, collected."""

from __future__ import annotations

import pathlib
from collections.abc import Callable

import polars as pl
import pytest
from polars.testing import assert_frame_equal

import polars_tparquet as pt
from conftest import Reads, ticks

Write = Callable[..., pathlib.Path]


def test_read_everything(write: Write) -> None:
    df = ticks(100)
    assert_frame_equal(pt.read_tparquet(write("a.parquet", df)), df)


def test_columns_in_the_order_given(write: Write, reads: Reads) -> None:
    path = write("a.parquet", ticks(100))
    out = pt.read_tparquet(path, columns=["sym", "ts"])
    assert out.columns == ["sym", "ts"]
    assert_frame_equal(out, ticks(100).select("sym", "ts"))
    assert sorted(reads.columns()[0]) == ["sym", "ts"]
    with pytest.raises(pl.exceptions.ColumnNotFoundError):
        pt.read_tparquet(path, columns=["nope"])


def test_n_rows(write: Write) -> None:
    paths = [write("a.parquet", ticks(3)), write("b.parquet", ticks(10, offset=3))]
    assert_frame_equal(pt.read_tparquet(paths, n_rows=5), ticks(5))
    assert pt.read_tparquet(paths, n_rows=0).height == 0
    with pytest.raises(ValueError, match="n_rows must not be negative"):
        pt.read_tparquet(paths, n_rows=-1)


def test_file_paths_and_batch_size(write: Write, reads: Reads) -> None:
    a, b = write("a.parquet", ticks(3)), write("b.parquet", ticks(2, offset=3))
    out = pt.read_tparquet([a, b], include_file_paths="path", batch_size=2)
    assert out["path"].to_list() == [str(a)] * 3 + [str(b)] * 2
    assert {c["batch_size"] for c in reads.calls} == {2}


def test_columns_by_index_as_polars_reads_them(write: Write, reads: Reads) -> None:
    path = write("a.parquet", ticks(10))
    for columns in ([1], [3, 0], (2,), [-1]):
        want = pl.read_parquet(_polars_copy(path), columns=list(columns))
        out = pt.read_tparquet(path, columns=columns)
        assert_frame_equal(out, want)
    assert pt.read_tparquet(path, columns=[3, 0]).columns == ["sym", "ts"]
    assert sorted(reads.columns()[-1]) == ["sym", "ts"]
    out = pt.read_tparquet(path, columns=[4], include_file_paths="file")
    assert out.columns == ["file"]
    with pytest.raises(pl.exceptions.ColumnNotFoundError, match="9-th column"):
        pt.read_tparquet(path, columns=[9])
    # one name alone, as Polars' reader takes it
    assert_frame_equal(pt.read_tparquet(path, columns="sym"), ticks(10).select("sym"))


def _polars_copy(path: pathlib.Path) -> pathlib.Path:
    """A copy of `path` that Polars' own reader reads."""
    copy = path.with_name(f"{path.stem}.polars.parquet")
    pt.read_tparquet(path).write_parquet(copy)
    return copy
