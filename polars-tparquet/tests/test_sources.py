"""What scan_tparquet reads: the kinds of paths, globs, directories, file objects and lists."""

from __future__ import annotations

import io
import os
import pathlib
import re
import threading
from collections.abc import Callable

import polars as pl
import pytest
import tparquet
from polars.testing import assert_frame_equal

import polars_tparquet as pt
from conftest import Reads, ticks

Write = Callable[..., pathlib.Path]


class FsPath:
    """An os.PathLike that is not a pathlib path."""

    def __init__(self, path: str) -> None:
        self._path = path

    def __fspath__(self) -> str:
        return self._path


def test_every_kind_of_path(write: Write) -> None:
    df = ticks(10)
    path = write("a.parquet", df)
    for source in (str(path), path, os.fsencode(path), FsPath(str(path))):
        assert_frame_equal(pt.scan_tparquet(source).collect(), df)
        out = pt.scan_tparquet(source, include_file_paths="file").collect()
        assert out["file"].to_list() == [str(path)] * 10


def test_a_home_relative_path(write: Write, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("HOME", str(tmp_path))
    write("data/a.parquet", ticks(10))
    write("data/b.parquet", ticks(5, offset=10))
    want = pl.concat([ticks(10), ticks(5, offset=10)])
    for source in ("~/data/a.parquet", pathlib.Path("~/data/a.parquet"), b"~/data/a.parquet"):
        assert_frame_equal(pt.scan_tparquet(source).collect(), ticks(10))
    assert_frame_equal(pt.scan_tparquet("~/data").collect(), want)
    assert_frame_equal(pt.scan_tparquet("~/data/*.parquet").collect(), want)
    out = pt.scan_tparquet(["~/data/b.parquet"], include_file_paths="file").collect()
    assert out["file"].to_list() == [str(tmp_path / "data" / "b.parquet")] * 5


def test_write_and_sink_to_a_home_relative_path(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("HOME", str(tmp_path))
    ticks(10).tparquet.write("~/w.parquet")  # type: ignore[attr-defined]
    ticks(10).lazy().tparquet.sink(pathlib.Path("~/s.parquet"))  # type: ignore[attr-defined]
    assert sorted(os.listdir(tmp_path)) == ["s.parquet", "w.parquet"]
    assert_frame_equal(pt.read_tparquet([tmp_path / "w.parquet", tmp_path / "s.parquet"]), pl.concat([ticks(10)] * 2))


def test_a_list_mixes_paths_globs_directories_and_file_objects(write: Write, tmp_path: pathlib.Path) -> None:
    parts = [ticks(2), ticks(3, offset=2), ticks(4, offset=5), ticks(5, offset=9), ticks(6, offset=14)]
    a = write("a.parquet", parts[0])
    write("g/b.parquet", parts[1])
    write("g/c.parquet", parts[2])
    write("d/e/f.parquet", parts[3])
    obj = io.BytesIO(write("obj.parquet", parts[4]).read_bytes())
    sources: list[tparquet.Source] = [a, str(tmp_path / "g" / "*.parquet"), tmp_path / "d", obj]
    assert_frame_equal(pt.scan_tparquet(sources).collect(), pl.concat(parts))


def test_a_file_named_twice_is_read_twice(write: Write, tmp_path: pathlib.Path) -> None:
    a = write("a.parquet", ticks(3))
    out = pt.scan_tparquet([a, str(a), str(tmp_path / "*.parquet")], include_file_paths="file").collect()
    assert_frame_equal(out.drop("file"), pl.concat([ticks(3)] * 3))
    assert out["file"].to_list() == [str(a)] * 9


def test_a_path_without_glob_characters_is_one_file(write: Write) -> None:
    path = write("a.parquet", ticks(3))
    assert_frame_equal(pt.scan_tparquet(str(path)).collect(), ticks(3))
    with pytest.raises(FileNotFoundError, match=r"No such file or directory.*b\.parquet"):
        pt.scan_tparquet(str(path.with_name("b.parquet")))


def test_a_file_object_is_read_when_scanned(write: Write) -> None:
    obj = io.BytesIO(write("a.parquet", ticks(10)).read_bytes())
    lf = pt.scan_tparquet(obj)
    obj.close()  # the bytes were taken when scanned
    assert_frame_equal(lf.collect(), ticks(10))
    assert_frame_equal(lf.collect(), ticks(10))
    assert_frame_equal(lf.filter(pl.col("qty") > 3).collect(), ticks(10).filter(pl.col("qty") > 3))


def test_a_file_object_is_read_from_where_it_stands(write: Write) -> None:
    data = write("a.parquet", ticks(10)).read_bytes()
    obj = io.BytesIO(b"header" + data)
    obj.seek(6)
    assert_frame_equal(pt.scan_tparquet(obj).collect(), ticks(10))
    written = io.BytesIO()
    ticks(10).tparquet.write(written)  # type: ignore[attr-defined]
    written.seek(0)
    assert_frame_equal(pt.scan_tparquet(written).collect(), ticks(10))


def test_a_file_object_at_its_end_has_nothing_to_read() -> None:
    written = io.BytesIO()
    ticks(10).tparquet.write(written)  # type: ignore[attr-defined]
    with pytest.raises(ValueError, match="Size is 0"):
        pt.scan_tparquet(written)


def test_a_non_seekable_stream(write: Write) -> None:
    data = write("a.parquet", ticks(1000)).read_bytes()
    r, w = os.pipe()

    def feed() -> None:
        with os.fdopen(w, "wb") as f:
            f.write(data)

    feeder = threading.Thread(target=feed)
    feeder.start()
    with os.fdopen(r, "rb") as pipe:
        assert not pipe.seekable()
        lf = pt.scan_tparquet(pipe)
    feeder.join()
    assert_frame_equal(lf.collect(), ticks(1000))


def test_an_object_with_only_read(write: Write) -> None:
    data = write("a.parquet", ticks(10)).read_bytes()

    class Reader:
        def read(self) -> bytes:
            return data

    assert_frame_equal(pt.scan_tparquet(Reader()).collect(), ticks(10))


def test_what_is_not_a_source(write: Write, tmp_path: pathlib.Path) -> None:
    path = write("a.parquet", ticks(3))
    for bad, name in ((3, "int"), (None, "NoneType"), ((p for p in [path]), "generator"), ([[path]], "list")):
        with pytest.raises(TypeError, match=rf"^expected a path or a binary file object, got {name}$"):
            pt.scan_tparquet(bad)  # type: ignore[arg-type]
    with open(path) as text, pytest.raises((TypeError, UnicodeDecodeError)):
        pt.scan_tparquet(text)  # type: ignore[arg-type]


def test_a_missing_file_in_a_list_is_named(write: Write, tmp_path: pathlib.Path) -> None:
    a = write("a.parquet", ticks(3))
    with pytest.raises(FileNotFoundError, match=r"missing\.parquet"):
        pt.scan_tparquet([a, tmp_path / "missing.parquet"])


def test_a_relative_path_is_the_file_it_named_when_scanned(
    write: Write, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    write("a/f.parquet", ticks(3))
    write("b/f.parquet", ticks(5))
    monkeypatch.chdir(tmp_path / "a")
    scans = [pt.scan_tparquet(s, include_file_paths="file") for s in ("f.parquet", ".", "*.parquet", ["./f.parquet"])]
    monkeypatch.chdir(tmp_path / "b")  # tparquet opens the file again for every read
    for lf in scans:
        out = lf.collect()
        assert_frame_equal(out.drop("file"), ticks(3))
        # the path stays relative, as given
        assert [os.path.normpath(p) for p in out["file"]] == ["f.parquet"] * 3


def test_a_glob_of_any_kind_of_path(write: Write, tmp_path: pathlib.Path) -> None:
    parts = [write("d/a.parquet", ticks(2)), write("d/b.parquet", ticks(3, offset=2))]
    want = pl.concat([ticks(2), ticks(3, offset=2)])
    pattern = tmp_path / "d" / "*.parquet"
    for source in (str(pattern), pattern, os.fsencode(pattern), FsPath(str(pattern))):
        out = pt.scan_tparquet(source, include_file_paths="file").collect()
        assert_frame_equal(out.drop("file"), want)
        assert out["file"].unique(maintain_order=True).to_list() == [str(p) for p in parts]


@pytest.mark.parametrize("name", ["a[1].parquet", "what?.parquet", "x*y.parquet"])
def test_an_existing_file_is_read_as_named_whatever_its_name(write: Write, tmp_path: pathlib.Path, name: str) -> None:
    path = write(name, ticks(3))
    write("a1.parquet", ticks(5))  # what a[1].parquet matches as a glob
    write("whatX.parquet", ticks(5))
    write("xzzy.parquet", ticks(5))
    for source in (str(path), path):
        assert_frame_equal(pt.scan_tparquet(source).collect(), ticks(3))
    # a directory too
    write(f"{name}.d/b.parquet", ticks(4))
    assert_frame_equal(pt.scan_tparquet(tmp_path / f"{name}.d").collect(), ticks(4))


def test_a_glob_reads_the_files_it_matches_not_directories(write: Write, tmp_path: pathlib.Path) -> None:
    a = write("d/a.parquet", ticks(3))
    write("d/sub.parquet/b.parquet", ticks(4))  # a directory named like a file
    assert_frame_equal(pt.scan_tparquet(str(tmp_path / "d" / "*.parquet")).collect(), ticks(3))
    out = pt.scan_tparquet(str(tmp_path / "d" / "**"), include_file_paths="file").collect()
    assert out["file"].unique(maintain_order=True).to_list() == [str(a), str(tmp_path / "d/sub.parquet/b.parquet")]
    (tmp_path / "e" / "f").mkdir(parents=True)
    with pytest.raises(FileNotFoundError, match=r"^no file matches .*e/\*$"):
        pt.scan_tparquet(str(tmp_path / "e" / "*"))


def _files_read(source: object) -> list[str]:
    out = pt.scan_tparquet(source, include_file_paths="file").select("file").collect()  # type: ignore[arg-type]
    return [os.path.relpath(p) for p in out["file"].unique(maintain_order=True)]


def test_a_directory_skips_hidden_files_and_directories(
    write: Write, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    write("d/a.parquet", ticks(3))
    (tmp_path / "d" / "._a.parquet").write_bytes(b"\x00\x05\x16\x07AppleDouble")  # what macOS leaves on USB disks
    write("d/.partial.parquet", ticks(4))  # what a writer elsewhere has not finished
    write("d/.hidden/b.parquet", ticks(5))
    write("d/e/.git/c.parquet", ticks(6))
    write("d/e/f.parquet", ticks(7))
    assert _files_read("d") == ["d/a.parquet", "d/e/f.parquet"]
    # as a glob does
    assert _files_read("d/**/*.parquet") == ["d/a.parquet", "d/e/f.parquet"]
    # a hidden directory named is read: only what lies hidden below it is skipped
    assert _files_read("d/.hidden") == ["d/.hidden/b.parquet"]
    write(".cache/data/g.parquet", ticks(1))
    assert _files_read(tmp_path / ".cache" / "data") == [".cache/data/g.parquet"]
    assert _files_read(["d/.partial.parquet"]) == ["d/.partial.parquet"]


def test_a_directory_reads_parquet_files_whatever_the_case_of_the_suffix(
    write: Write, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    for name in ("d/A.PARQUET", "d/b.Parquet", "d/c.parquet", "d/x/d.pArQuEt"):
        write(name, ticks(2))
    (tmp_path / "d" / "notes.parquet.txt").write_text("not data")
    (tmp_path / "d" / "parquet").write_text("not data")
    assert _files_read("d") == ["d/A.PARQUET", "d/b.Parquet", "d/c.parquet", "d/x/d.pArQuEt"]
    (tmp_path / "e").mkdir()
    (tmp_path / "e" / "notes.txt").write_text("not data")
    with pytest.raises(FileNotFoundError, match=r"^no \.parquet file in e$"):
        pt.scan_tparquet("e")


def test_a_directory_follows_symlinked_files_not_directories(
    write: Write, tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    write("data/a.parquet", ticks(3))
    write("d/b.parquet", ticks(4))
    (tmp_path / "d" / "link.parquet").symlink_to(tmp_path / "data" / "a.parquet")
    (tmp_path / "d" / "sub").symlink_to(tmp_path / "data", target_is_directory=True)
    (tmp_path / "d" / "loop").symlink_to(tmp_path / "d", target_is_directory=True)
    (tmp_path / "d" / "broken.parquet").symlink_to(tmp_path / "missing.parquet")
    assert _files_read("d") == ["d/b.parquet", "d/link.parquet"]
    # a symlinked directory named is read
    assert _files_read("d/sub") == ["d/sub/a.parquet"]


def test_a_file_object_listed_twice_is_read_twice(write: Write) -> None:
    obj = io.BytesIO(write("a.parquet", ticks(3)).read_bytes())
    other = io.BytesIO(write("b.parquet", ticks(2, offset=3)).read_bytes())
    out = pt.scan_tparquet([obj, other, obj]).collect()
    assert_frame_equal(out, pl.concat([ticks(3), ticks(2, offset=3), ticks(3)]))


def test_a_file_that_is_not_parquet_is_named(write: Write, tmp_path: pathlib.Path) -> None:
    a = write("a.parquet", ticks(3))
    bad = tmp_path / "bad.parquet"
    bad.write_bytes(b"this is not a Parquet file")
    with pytest.raises(tparquet.PcodecError, match=rf"^{re.escape(str(bad))}: parquet: .*Corrupt footer"):
        pt.scan_tparquet([a, bad])
    with pytest.raises(tparquet.PcodecError, match=r"^<_io\.BytesIO object at 0x[0-9a-f]+>: parquet: .*too small"):
        pt.scan_tparquet([a, io.BytesIO(b"PAR1")])
    truncated = tmp_path / "truncated.parquet"
    truncated.write_bytes(a.read_bytes()[:-20])
    with pytest.raises(tparquet.PcodecError, match=rf"^{re.escape(str(truncated))}: "):
        pt.scan_tparquet(tmp_path / "trunc*.parquet")
    # an error that is not the file's is not wrapped
    with pytest.raises(ValueError, match=r"^batch_size must be positive, got 0$"):
        pt.scan_tparquet(a, batch_size=0)


def test_batch_size_reaches_the_reader_of_every_source(write: Write, reads: Reads) -> None:
    path = write("a.parquet", ticks(20))
    obj = io.BytesIO(path.read_bytes())
    out = pt.scan_tparquet([path, obj, obj], batch_size=7).collect()
    assert out.height == 60
    assert [c["batch_size"] for c in reads.calls] == [7, 7, 7]


def test_a_text_mode_file_is_refused_with_a_type_error(write: Write) -> None:
    path = write("a.parquet", ticks(3))
    with open(path) as text, pytest.raises(TypeError, match="bytes"):
        pt.scan_tparquet(text)  # type: ignore[arg-type]


def test_a_file_name_that_is_not_utf_8(write: Write, tmp_path: pathlib.Path) -> None:
    data = write("a.parquet", ticks(3)).read_bytes()
    raw = os.fsencode(tmp_path / "d") + b"/caf\xe9.parquet"
    os.mkdir(os.fsencode(tmp_path / "d"))
    with open(raw, "wb") as f:
        f.write(data)
    for source in (raw, os.fsdecode(raw), tmp_path / "d", str(tmp_path / "d" / "*.parquet")):
        assert_frame_equal(pt.scan_tparquet(source).collect(), ticks(3))
    # a Polars string is UTF-8: the path column holds the name with a replacement character
    out = pt.scan_tparquet(raw, include_file_paths="file").collect()
    assert out["file"].to_list() == [str(tmp_path / "d" / "caf�.parquet")] * 3
