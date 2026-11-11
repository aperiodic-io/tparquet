"""DataFrame.tparquet.write, and the registration of the `tparquet` namespaces."""

from __future__ import annotations

import io
import os
import pathlib
import re
from collections.abc import Callable

import polars as pl
import pytest
import tparquet
from polars.testing import assert_frame_equal

import polars_tparquet as pt
from conftest import ticks

Write = Callable[..., pathlib.Path]


def test_importing_registers_the_namespaces() -> None:
    assert hasattr(pl.DataFrame({"a": [1]}), "tparquet")
    assert hasattr(pl.LazyFrame({"a": [1]}), "tparquet")


def test_write_round_trips_and_returns_the_codecs(tmp_path: pathlib.Path) -> None:
    df = ticks(5000)
    codecs = df.tparquet.write(tmp_path / "t.parquet")
    # Auto: PCODEC where it is smaller (the small cycling qty compresses better with ZSTD)
    assert codecs == {"ts": "PCODEC", "price": "PCODEC", "qty": "ZSTD", "sym": "ZSTD"}
    assert_frame_equal(pt.read_tparquet(tmp_path / "t.parquet"), df)
    assert tparquet.inspect(tmp_path / "t.parquet")["pcodec_version"] == "1"


def test_write_options(tmp_path: pathlib.Path) -> None:
    df = ticks(5000)
    base = df.tparquet.write(tmp_path / "base.parquet", policy="base", base="snappy")
    assert set(base.values()) == {"SNAPPY"}
    forced = df.tparquet.write(tmp_path / "f.parquet", policy="pcodec", columns={"price": "base"}, level=0)
    assert forced == {"ts": "PCODEC", "price": "ZSTD", "qty": "PCODEC", "sym": "ZSTD"}
    df.tparquet.write(tmp_path / "rg.parquet", row_group_size=1000)
    assert tparquet.ParquetFile(tmp_path / "rg.parquet").metadata.num_row_groups == 5
    with pytest.raises(tparquet.PcodecError, match="level"):
        df.tparquet.write(tmp_path / "bad.parquet", level=13)


def test_the_default_level_is_8(tmp_path: pathlib.Path) -> None:
    df = ticks(20_000).with_columns(noisy=(pl.col("ts") * 7919) % 100_003)
    default, eight, nine = (tmp_path / f"{n}.parquet" for n in ("default", "8", "9"))
    df.tparquet.write(default)
    df.tparquet.write(eight, level=8)
    df.tparquet.write(nine, level=9)
    assert default.read_bytes() == eight.read_bytes() != nine.read_bytes()
    df.lazy().tparquet.sink(default)
    df.lazy().tparquet.sink(eight, level=8)
    df.lazy().tparquet.sink(nine, level=9)
    assert default.read_bytes() == eight.read_bytes() != nine.read_bytes()


def test_write_to_a_file_object() -> None:
    buf = io.BytesIO()
    ticks(100).tparquet.write(buf)
    assert_frame_equal(pt.read_tparquet(io.BytesIO(buf.getvalue())), ticks(100))


def test_a_write_the_writer_rejects_leaves_no_file(tmp_path: pathlib.Path) -> None:
    # the column names are checked only once the file is created
    with pytest.raises(tparquet.PcodecError, match=r'^no column named "nope"$'):
        ticks(10).tparquet.write(tmp_path / "t.parquet", columns={"nope": "pcodec"})
    assert os.listdir(tmp_path) == []


def test_a_failed_write_keeps_the_file_it_would_replace(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "t.parquet"
    ticks(10).tparquet.write(path)
    before = path.read_bytes()
    with pytest.raises(tparquet.PcodecError):
        ticks(20).tparquet.write(path, columns={"nope": "pcodec"})
    assert path.read_bytes() == before
    assert os.listdir(tmp_path) == ["t.parquet"]


def test_write_replaces_a_file_once_complete(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    path = tmp_path / "t.parquet"
    ticks(10).tparquet.write(path)
    before = path.read_bytes()
    sinks: list[object] = []
    write_table = tparquet.write_table

    def recording(data: tparquet.ArrowData, sink: tparquet.Sink, **kwargs: object) -> dict[str, str]:
        sinks.append(sink)
        codecs = write_table(data, sink, **kwargs)  # type: ignore[arg-type]
        assert path.read_bytes() == before  # still the old file
        return codecs

    monkeypatch.setattr(tparquet.polars, "write_table", recording)
    ticks(20).tparquet.write(path)
    (sink,) = sinks
    assert isinstance(sink, str)
    assert os.path.dirname(sink) == str(tmp_path)
    assert re.fullmatch(r"\.t\.parquet\.[0-9a-f]{32}\.tmp", os.path.basename(sink))
    assert os.listdir(tmp_path) == ["t.parquet"]
    assert_frame_equal(pt.read_tparquet(path), ticks(20))


def test_write_through_a_symlink_writes_the_file_it_links_to(tmp_path: pathlib.Path) -> None:
    real = tmp_path / "data" / "t.parquet"
    real.parent.mkdir()
    link = tmp_path / "link.parquet"
    link.symlink_to(real)  # dangling until written
    ticks(10).tparquet.write(link)
    assert link.is_symlink()
    assert_frame_equal(pt.read_tparquet(real), ticks(10))


def test_write_errors_name_the_path(tmp_path: pathlib.Path) -> None:
    with pytest.raises(FileNotFoundError, match=re.escape(str(tmp_path / "no"))) as e:
        ticks(10).tparquet.write(tmp_path / "no" / "t.parquet")
    assert ".tmp" not in str(e.value)
    (tmp_path / "d").mkdir()
    with pytest.raises(IsADirectoryError):
        ticks(10).tparquet.write(tmp_path / "d")
    assert sorted(os.listdir(tmp_path)) == ["d"]


def test_an_object_column_is_refused(tmp_path: pathlib.Path) -> None:
    """Polars exports an Object column's values as the addresses of the Python objects; its own
    writer refuses the dtype, and so does this one."""
    df = pl.DataFrame({"a": [1, 2], "o": pl.Series([object(), 3], dtype=pl.Object)})
    message = r"^cannot write column 'o' of dtype Object: its values are Python objects$"
    with pytest.raises(pl.exceptions.ComputeError, match=message):
        df.tparquet.write(tmp_path / "t.parquet")
    with pytest.raises(pl.exceptions.ComputeError, match=message):
        df.tparquet.write(io.BytesIO())
    with pytest.raises(pl.exceptions.ComputeError, match=message):
        df.lazy().tparquet.sink(tmp_path / "t.parquet")
    with pytest.raises(pl.exceptions.ComputeError, match=message):
        df.lazy().tparquet.sink(io.BytesIO())
    assert os.listdir(tmp_path) == []
    # the other columns can be written
    assert df.drop("o").tparquet.write(tmp_path / "t.parquet") == {"a": "PCODEC"}


def test_write_to_a_file_name_that_is_not_utf_8(tmp_path: pathlib.Path) -> None:
    raw = os.fsencode(tmp_path) + b"/caf\xe9.parquet"
    ticks(3).tparquet.write(raw)
    assert_frame_equal(pt.read_tparquet(raw), ticks(3))
