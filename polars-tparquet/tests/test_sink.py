"""LazyFrame.tparquet.sink: a query streamed into one file."""

from __future__ import annotations

import io
import os
import pathlib
import re

import polars as pl
import pytest
import tparquet
from polars.testing import assert_frame_equal

import polars_tparquet as pt
from conftest import ticks


def test_sink_streams_the_query_into_one_file(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    sources = []
    for i in range(3):
        sources.append(tmp_path / f"in{i}.parquet")
        ticks(2000, offset=2000 * i).tparquet.write(sources[-1], row_group_size=500)
    lf = pt.scan_tparquet(sources).filter(pl.col("qty") > 5).with_columns(notional=pl.col("price") * pl.col("qty"))
    want = lf.collect()

    # sink never materializes the whole result
    def no_collect(*args: object, **kwargs: object) -> None:
        raise AssertionError("sink called LazyFrame.collect")

    monkeypatch.setattr(pl.LazyFrame, "collect", no_collect)
    codecs = lf.tparquet.sink(tmp_path / "out.parquet", chunk_size=1000, row_group_size=1000)
    monkeypatch.undo()

    assert codecs["notional"] == "PCODEC"
    assert_frame_equal(pt.read_tparquet(tmp_path / "out.parquet"), want)
    groups = tparquet.ParquetFile(tmp_path / "out.parquet").metadata.num_row_groups
    assert groups == -(-want.height // 1000)


def test_sink_options_and_file_objects(tmp_path: pathlib.Path) -> None:
    lf = ticks(3000).lazy()
    assert set(lf.tparquet.sink(tmp_path / "b.parquet", policy="base").values()) == {"ZSTD"}
    buf = io.BytesIO()
    lf.tparquet.sink(buf, level=2, columns={"ts": "base"}, row_group_size=1000)
    out = io.BytesIO(buf.getvalue())
    assert_frame_equal(pt.read_tparquet(out), ticks(3000))
    assert tparquet.ParquetFile(io.BytesIO(buf.getvalue())).metadata.num_row_groups == 3


def test_sink_passes_every_option_to_the_writer(tmp_path: pathlib.Path) -> None:
    lf = ticks(3000).lazy()
    codecs = lf.tparquet.sink(tmp_path / "a.parquet", policy="pcodec", columns={"ts": "base"}, base="snappy")
    assert codecs == {"ts": "SNAPPY", "price": "PCODEC", "qty": "PCODEC", "sym": "SNAPPY"}
    with pytest.raises(tparquet.PcodecError, match="level"):
        lf.tparquet.sink(tmp_path / "b.parquet", level=13)


def test_sink_batches_have_chunk_size_rows(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    heights: list[int] = []
    write = tparquet.Writer.write

    def recording(self: tparquet.Writer, data: pl.DataFrame) -> None:
        heights.append(data.height)
        write(self, data)

    monkeypatch.setattr(tparquet.Writer, "write", recording)
    ticks(1000).lazy().tparquet.sink(tmp_path / "a.parquet", chunk_size=300)
    assert sum(heights) == 1000
    assert max(heights) <= 300
    heights.clear()
    ticks(5).lazy().tparquet.sink(tmp_path / "b.parquet", chunk_size=1)
    assert heights == [1, 1, 1, 1, 1]


def test_sink_of_an_empty_result_writes_the_schema(tmp_path: pathlib.Path) -> None:
    lf = ticks(100).lazy().filter(pl.col("ts") < 0)
    lf.tparquet.sink(tmp_path / "empty.parquet")
    out = pt.read_tparquet(tmp_path / "empty.parquet")
    assert out.height == 0
    assert out.schema == ticks(1).schema


def test_a_failing_query_leaves_no_file(tmp_path: pathlib.Path) -> None:
    def boom(df: pl.DataFrame) -> pl.DataFrame:
        raise RuntimeError("boom")

    lf = ticks(100).lazy().map_batches(boom, schema=ticks(1).schema)
    with pytest.raises(RuntimeError, match="boom"):
        lf.tparquet.sink(tmp_path / "out.parquet")
    assert not (tmp_path / "out.parquet").exists()


def _boom(df: pl.DataFrame) -> pl.DataFrame:
    raise RuntimeError("boom")


def test_a_sink_over_its_own_input_replaces_it(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "t.parquet"
    ticks(1000).tparquet.write(path, row_group_size=100)
    want = ticks(1000).filter(pl.col("qty") > 10)
    pt.scan_tparquet(path).filter(pl.col("qty") > 10).tparquet.sink(path, chunk_size=64)
    assert_frame_equal(pt.read_tparquet(path), want)
    assert os.listdir(tmp_path) == ["t.parquet"]


def test_a_failing_sink_keeps_the_file_it_would_replace(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "t.parquet"
    ticks(10).tparquet.write(path)
    before = path.read_bytes()
    lf = ticks(100).lazy().map_batches(_boom, schema=ticks(1).schema)
    with pytest.raises(RuntimeError, match="boom"):
        lf.tparquet.sink(path)
    assert path.read_bytes() == before
    assert os.listdir(tmp_path) == ["t.parquet"]


def test_options_the_writer_rejects_leave_no_file(tmp_path: pathlib.Path) -> None:
    # the options are checked before the file is created, the column names only after
    with pytest.raises(tparquet.PcodecError, match=r'^no column named "nope"$'):
        ticks(10).lazy().tparquet.sink(tmp_path / "t.parquet", columns={"nope": "pcodec"})
    assert os.listdir(tmp_path) == []


def test_a_footer_that_cannot_be_written_leaves_no_file(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def disk_full(self: tparquet.Writer) -> dict[str, str]:
        raise OSError(28, "No space left on device")

    monkeypatch.setattr(tparquet.Writer, "close", disk_full)
    with pytest.raises(OSError, match="No space left on device"):
        ticks(10).lazy().tparquet.sink(tmp_path / "t.parquet")
    assert os.listdir(tmp_path) == []


@pytest.mark.parametrize("existing", [False, True], ids=["new", "existing"])
def test_an_interrupt_while_writing_leaves_the_target_as_it_was(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, existing: bool
) -> None:
    path = tmp_path / "t.parquet"
    if existing:
        ticks(3).tparquet.write(path)
    before = sorted(os.listdir(tmp_path)), path.read_bytes() if existing else None
    written: list[int] = []
    write = tparquet.Writer.write

    def interrupted(self: tparquet.Writer, data: pl.DataFrame) -> None:
        if written:
            raise KeyboardInterrupt
        written.append(data.height)
        write(self, data)

    monkeypatch.setattr(tparquet.Writer, "write", interrupted)
    with pytest.raises(KeyboardInterrupt):
        ticks(100).lazy().tparquet.sink(path, chunk_size=10)
    assert written == [10]
    assert (sorted(os.listdir(tmp_path)), path.read_bytes() if existing else None) == before


def test_the_target_appears_only_once_complete(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    seen: list[list[str]] = []
    write = tparquet.Writer.write

    def recording(self: tparquet.Writer, data: pl.DataFrame) -> None:
        seen.append(os.listdir(tmp_path))
        write(self, data)

    monkeypatch.setattr(tparquet.Writer, "write", recording)
    ticks(30).lazy().tparquet.sink(tmp_path / "out.parquet", chunk_size=10)
    # a hidden file beside the target, which a scan of the directory skips
    assert len(seen) == 3
    assert all(len(names) == 1 and re.fullmatch(r"\.out\.parquet\.[0-9a-f]{32}\.tmp", names[0]) for names in seen)
    assert len({names[0] for names in seen}) == 1
    assert os.listdir(tmp_path) == ["out.parquet"]


def test_a_sink_through_a_symlink_writes_the_file_it_links_to(tmp_path: pathlib.Path) -> None:
    real = tmp_path / "data" / "t.parquet"
    real.parent.mkdir()
    ticks(3).tparquet.write(real)
    link = tmp_path / "link.parquet"
    link.symlink_to(real)
    ticks(10).lazy().tparquet.sink(link)
    assert link.is_symlink()
    assert_frame_equal(pt.read_tparquet(real), ticks(10))
    assert os.listdir(real.parent) == ["t.parquet"]


def test_a_sink_into_a_missing_directory_names_it(tmp_path: pathlib.Path) -> None:
    with pytest.raises(FileNotFoundError, match=re.escape(str(tmp_path / "no"))) as e:
        ticks(10).lazy().tparquet.sink(tmp_path / "no" / "t.parquet")
    assert ".tmp" not in str(e.value)
    assert os.listdir(tmp_path) == []


def test_a_sink_onto_a_directory_fails_and_leaves_no_file(tmp_path: pathlib.Path) -> None:
    (tmp_path / "d").mkdir()
    with pytest.raises(IsADirectoryError):
        ticks(10).lazy().tparquet.sink(tmp_path / "d")
    assert os.listdir(tmp_path) == ["d"]
    assert os.listdir(tmp_path / "d") == []


@pytest.mark.parametrize("chunk_size", [1, 99, 100, 101, 1000])
def test_chunk_size_boundaries(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, chunk_size: int) -> None:
    heights: list[int] = []
    write = tparquet.Writer.write

    def recording(self: tparquet.Writer, data: pl.DataFrame) -> None:
        heights.append(data.height)
        write(self, data)

    monkeypatch.setattr(tparquet.Writer, "write", recording)
    ticks(100).lazy().tparquet.sink(tmp_path / "a.parquet", chunk_size=chunk_size)
    assert sum(heights) == 100
    assert max(heights) <= chunk_size
    assert len(heights) >= -(-100 // chunk_size)
    assert_frame_equal(pt.read_tparquet(tmp_path / "a.parquet"), ticks(100))


def test_a_scan_with_file_paths_sinks_the_paths(tmp_path: pathlib.Path) -> None:
    a, b = tmp_path / "a.parquet", tmp_path / "b.parquet"
    ticks(3).tparquet.write(a)
    ticks(2).tparquet.write(b)
    lf = pt.scan_tparquet([a, b], include_file_paths="file")
    assert lf.tparquet.sink(tmp_path / "out.parquet")["file"] == "ZSTD"
    assert pt.read_tparquet(tmp_path / "out.parquet")["file"].to_list() == [str(a)] * 3 + [str(b)] * 2


def test_a_scan_of_the_directory_being_sunk_into_skips_the_unfinished_file(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ticks(10).tparquet.write(tmp_path / "a.parquet")
    during: list[int] = []
    write = tparquet.Writer.write

    def scanning(self: tparquet.Writer, data: pl.DataFrame) -> None:
        write(self, data)
        self.flush()
        during.append(pt.scan_tparquet(tmp_path).collect().height)

    monkeypatch.setattr(tparquet.Writer, "write", scanning)
    pt.scan_tparquet(tmp_path).tparquet.sink(tmp_path / "b.parquet", chunk_size=4)
    assert during == [10] * len(during)
    assert len(during) >= 3
    assert pt.scan_tparquet(tmp_path).collect().height == 20


def test_a_file_object_that_fails_raises_its_error(tmp_path: pathlib.Path) -> None:
    class Full(io.RawIOBase):
        def __init__(self) -> None:
            self.written = 0

        def writable(self) -> bool:
            return True

        def write(self, data: object) -> int:
            self.written += len(data)  # type: ignore[arg-type]
            if self.written > 2000:
                raise OSError(28, "No space left on device")
            return len(data)  # type: ignore[arg-type]

    with pytest.raises(OSError, match="No space left on device"):
        ticks(10_000).lazy().tparquet.sink(Full(), chunk_size=100, row_group_size=100)
    with pytest.raises(OSError, match="No space left on device"):
        ticks(10_000).tparquet.write(Full(), row_group_size=100)


def test_an_empty_result_of_nested_columns(tmp_path: pathlib.Path) -> None:
    lf = pl.LazyFrame(
        {
            "l": [[1.5, None]],
            "s": [{"a": 1, "b": ["x"]}],
            "a": pl.Series([[1, 2]], dtype=pl.Array(pl.Int64, 2)),
            "e": pl.Series(["a"], dtype=pl.Enum(["a", "b"])),
        }
    ).filter(pl.lit(False))
    codecs = lf.tparquet.sink(tmp_path / "empty.parquet")
    assert codecs == {"a.list.item": "ZSTD", "e": "ZSTD", "l.list.item": "ZSTD", "s.a": "ZSTD", "s.b.list.item": "ZSTD"}
    out = pt.read_tparquet(tmp_path / "empty.parquet")
    assert out.schema == lf.collect_schema()
    assert out.height == 0


def test_the_codecs_are_chosen_on_the_first_batch_with_rows(tmp_path: pathlib.Path) -> None:
    """Auto picks per column on the first batch with rows (an empty first batch does not count),
    here the first 1000 rows, where qty is smaller with PCODEC than over all 5000."""
    first = ticks(1000).tparquet.write(io.BytesIO())
    assert first != ticks(5000).tparquet.write(io.BytesIO())
    lf = pl.concat([ticks(0).lazy(), ticks(5000).lazy()])
    assert lf.tparquet.sink(tmp_path / "a.parquet", chunk_size=1000) == first


def test_a_sink_on_the_streaming_engine_of_a_large_scan(tmp_path: pathlib.Path) -> None:
    paths = []
    for k in range(4):
        paths.append(tmp_path / f"{k}.parquet")
        ticks(25_000, offset=25_000 * k).tparquet.write(paths[-1], row_group_size=10_000)
    lf = pt.scan_tparquet(paths).filter(pl.col("qty") > 25).select("ts", "qty", "sym")
    lf.tparquet.sink(tmp_path / "out.parquet", row_group_size=8192)
    assert_frame_equal(pt.read_tparquet(tmp_path / "out.parquet"), lf.collect())


def test_a_scan_made_before_its_file_was_replaced_reads_what_it_scanned(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "t.parquet"
    ticks(1000).tparquet.write(path, row_group_size=100)
    lf = pt.scan_tparquet(path)
    lf.filter(pl.col("qty") > 10).tparquet.sink(path)
    assert_frame_equal(lf.collect(), ticks(1000))


@pytest.mark.parametrize("how", ["write", "sink"])
def test_a_replaced_file_keeps_its_permissions(tmp_path: pathlib.Path, how: str) -> None:
    path = tmp_path / "t.parquet"
    ticks(3).tparquet.write(path)
    path.chmod(0o640)
    write = ticks(10).tparquet.write if how == "write" else ticks(10).lazy().tparquet.sink
    write(path)
    assert path.stat().st_mode & 0o777 == 0o640
    assert_frame_equal(pt.read_tparquet(path), ticks(10))
    new = tmp_path / "new.parquet"
    ticks(1).tparquet.write(new)
    umask = os.umask(0)
    os.umask(umask)
    assert new.stat().st_mode & 0o777 == 0o666 & ~umask


@pytest.mark.parametrize("how", ["write", "sink"])
def test_a_file_that_may_not_be_written_is_not_replaced(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, how: str
) -> None:
    path = tmp_path / "t.parquet"
    ticks(3).tparquet.write(path)
    before = path.read_bytes()
    access = os.access
    # root may write any file: pretend this one is read-only to the user
    monkeypatch.setattr(os, "access", lambda p, mode: mode != os.W_OK and access(p, mode))
    write = ticks(10).tparquet.write if how == "write" else ticks(10).lazy().tparquet.sink
    with pytest.raises(PermissionError, match=re.escape(str(path))):
        write(path)
    assert path.read_bytes() == before
    assert os.listdir(tmp_path) == ["t.parquet"]
    ticks(10).tparquet.write(tmp_path / "other.parquet")  # a new file needs no such right
