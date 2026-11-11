"""Where files are read from and written to: str, bytes and os.PathLike paths, file objects
of every kind (seekable or not, with only read or write), the errors those raise inside the
reads and writes, and paths that exist or are directories."""

from __future__ import annotations

import io
import os
import pathlib
import socket
import threading
from collections.abc import Callable
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import tparquet as ppc

TABLE = pa.table({"a": pa.array(range(3000), pa.int64()), "s": pa.array([str(i % 7) for i in range(3000)])})


def data() -> bytes:
    buf = io.BytesIO()
    ppc.write_table(TABLE, buf, row_group_size=1000)
    return buf.getvalue()


# ---------------------------------------------------------------- paths


@pytest.mark.parametrize("kind", ["str", "pathlib", "bytes", "fspath_str", "fspath_bytes"])
def test_path_kinds(tmp_path: pathlib.Path, kind: str) -> None:
    target = tmp_path / "t.parquet"

    class FsPath:
        def __init__(self, p: str | bytes) -> None:
            self.p = p

        def __fspath__(self) -> str | bytes:
            return self.p

    path: Any = {
        "str": str(target),
        "pathlib": target,
        "bytes": os.fsencode(target),
        "fspath_str": FsPath(str(target)),
        "fspath_bytes": FsPath(os.fsencode(target)),
    }[kind]
    assert ppc.write_table(TABLE, path)["a"] == "PCODEC"
    assert ppc.read_table(path).equals(TABLE)
    with ppc.Writer(path, TABLE.schema) as w:
        w.write(TABLE)
    assert ppc.ParquetFile(path).metadata.num_rows == 3000
    out = tmp_path / "o.parquet"
    ppc.transcode(path, out, target="standard")
    assert ppc.verify(path, out) is None
    assert ppc.inspect(path)["rows"] == 3000


def test_an_existing_file_is_replaced(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "t.parquet"
    path.write_bytes(b"x" * 10_000_000)
    ppc.write_table(TABLE.slice(0, 10), path)
    assert ppc.read_table(path).equals(TABLE.slice(0, 10))
    assert path.stat().st_size < 10_000


def test_sinks_that_cannot_be_created(tmp_path: pathlib.Path) -> None:
    with pytest.raises(IsADirectoryError) as e:
        ppc.write_table(TABLE, tmp_path)
    assert e.value.filename == str(tmp_path)
    with pytest.raises(FileNotFoundError) as f:
        ppc.Writer(tmp_path / "missing" / "t.parquet", TABLE.schema)
    assert f.value.filename == str(tmp_path / "missing" / "t.parquet")
    with pytest.raises(IsADirectoryError):
        ppc.transcode(io.BytesIO(data()), tmp_path)
    assert sorted(tmp_path.iterdir()) == []


def test_missing_and_unreadable_sources(tmp_path: pathlib.Path) -> None:
    with pytest.raises(FileNotFoundError) as e:
        ppc.ParquetFile(tmp_path / "nope.parquet")
    assert e.value.filename == str(tmp_path / "nope.parquet")
    calls: list[Callable[[Any], object]] = [
        ppc.read_table,
        ppc.inspect,
        lambda p: ppc.verify(p, p),
        lambda p: ppc.transcode(p, io.BytesIO()),
    ]
    for f in calls:
        with pytest.raises(FileNotFoundError):
            f(tmp_path / "nope.parquet")


# ---------------------------------------------------------------- file objects to read


def test_a_real_file_opened_rb(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "t.parquet"
    path.write_bytes(data())
    with open(path, "rb") as f:
        assert ppc.read_table(f).equals(TABLE)
    with open(path, "rb", buffering=0) as raw:
        assert ppc.ParquetFile(raw).read().equals(TABLE)


def test_a_file_object_is_read_from_where_it_is() -> None:
    buf = io.BytesIO(b"junk" + data())
    buf.seek(4)
    assert ppc.read_table(buf).equals(TABLE)
    # and it was read to the end: the file object is not needed afterwards
    assert buf.read() == b""


def test_a_non_seekable_stream() -> None:
    a, b = socket.socketpair()
    payload = data()

    def send() -> None:
        with a:
            a.sendall(payload)

    t = threading.Thread(target=send)
    t.start()
    with b, b.makefile("rb") as stream:
        assert not stream.seekable()
        assert ppc.read_table(stream).equals(TABLE)
    t.join()


def test_an_object_with_only_read() -> None:
    payload = data()

    class OnlyRead:
        def read(self) -> bytes:
            return payload

    assert ppc.read_table(OnlyRead()).equals(TABLE)
    assert ppc.ParquetFile(OnlyRead()).metadata.num_rows == 3000


def test_read_errors_of_file_objects_propagate() -> None:
    class Failing(io.RawIOBase):
        def readable(self) -> bool:
            return True

        def readinto(self, b: Any) -> int:
            raise ConnectionResetError("the peer went away")

    with pytest.raises(ConnectionResetError, match="the peer went away"):
        ppc.read_table(io.BufferedReader(Failing()))


def test_reading_returns_what_read_returns_or_says_why() -> None:
    class Text:
        def read(self) -> str:
            return "PAR1"

    with pytest.raises(TypeError, match=r"^the file object must return bytes from read\(\), got str$"):
        ppc.read_table(Text())  # type: ignore[arg-type]
    with pytest.raises(TypeError, match=r"^expected a path or a binary file object, got list$"):
        ppc.read_table([])  # type: ignore[arg-type]
    with pytest.raises(ppc.PcodecError):
        ppc.read_table(io.BytesIO(b""))


# ---------------------------------------------------------------- file objects to write


def test_a_real_file_opened_wb(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "t.parquet"
    with open(path, "wb") as f:
        ppc.write_table(TABLE, f)
        assert not f.closed
    assert ppc.read_table(path).equals(TABLE)
    with open(path, "wb", buffering=0) as raw, ppc.Writer(raw, TABLE.schema) as w:
        w.write(TABLE)
    assert ppc.read_table(path).equals(TABLE)


def test_an_object_with_only_write() -> None:
    chunks: list[bytes] = []

    class OnlyWrite:
        def write(self, b: bytes) -> int:
            chunks.append(bytes(b))
            return len(b)

    ppc.write_table(TABLE, OnlyWrite())
    assert ppc.read_table(io.BytesIO(b"".join(chunks))).equals(TABLE)
    chunks.clear()
    ppc.transcode(io.BytesIO(data()), OnlyWrite(), target="standard")
    assert pq.read_table(io.BytesIO(b"".join(chunks))).equals(TABLE)


def test_a_socket_as_a_sink() -> None:
    a, b = socket.socketpair()
    received = bytearray()

    def receive() -> None:
        with b:
            while chunk := b.recv(1 << 16):
                received.extend(chunk)

    t = threading.Thread(target=receive)
    t.start()
    with a, a.makefile("wb") as stream:
        ppc.write_table(TABLE, stream)
    t.join()
    assert ppc.read_table(io.BytesIO(bytes(received))).equals(TABLE)


@pytest.mark.parametrize("where", ["write", "flush"])
@pytest.mark.parametrize("call", ["write_table", "writer", "transcode"])
def test_errors_raised_by_a_sink_reach_the_caller(where: str, call: str) -> None:
    # (not an io.RawIOBase, whose finalizer would flush again)
    class Broken:
        def write(self, b: Any) -> int:
            if where == "write":
                raise PermissionError("no space for you")
            return len(b)

        def flush(self) -> None:
            if where == "flush":
                raise BrokenPipeError("flush failed")

    error = PermissionError if where == "write" else BrokenPipeError

    def run() -> None:
        if call == "write_table":
            ppc.write_table(TABLE, Broken(), policy="pcodec")
        elif call == "writer":
            with ppc.Writer(Broken(), TABLE.schema, policy="pcodec") as w:
                w.write(TABLE)
        else:
            ppc.transcode(io.BytesIO(data()), Broken())

    with pytest.raises(error):
        run()


def test_a_sink_error_does_not_leave_the_writer_open() -> None:
    fail = [False]

    class Flaky:
        def write(self, b: bytes) -> int:
            if fail[0]:
                raise OSError("gone")
            return len(b)

    w = ppc.Writer(Flaky(), TABLE.schema, policy="pcodec")
    w.write(TABLE)
    fail[0] = True
    with pytest.raises(OSError, match="gone"):
        w.close()
    assert w.closed
    with pytest.raises(ppc.PcodecError, match="closed"):
        w.write(TABLE)


@pytest.mark.parametrize("value", [-1, "x", 2**70, 1.5])
def test_a_write_that_returns_nonsense(value: object) -> None:
    class Odd:
        def write(self, b: bytes) -> object:
            return value

    with pytest.raises((TypeError, OverflowError, ValueError)):
        ppc.write_table(TABLE, Odd())


def test_a_write_that_returns_zero_is_an_error_not_a_hang() -> None:
    class Stuck:
        def write(self, b: bytes) -> int:
            return 0

    with pytest.raises((OSError, ppc.PcodecError), match="write"):
        ppc.write_table(TABLE, Stuck())


def test_short_writes_and_none_counts() -> None:
    out = io.BytesIO()

    class Trickle:
        def write(self, b: bytes) -> int:
            return out.write(bytes(b)[:7])

    ppc.write_table(TABLE, Trickle())
    assert ppc.read_table(io.BytesIO(out.getvalue())).equals(TABLE)
    counted = io.BytesIO()

    class NoCount:
        def write(self, b: bytes) -> None:
            counted.write(b)

    with ppc.Writer(NoCount(), TABLE.schema) as w:
        w.write(TABLE)
    assert ppc.read_table(io.BytesIO(counted.getvalue())).equals(TABLE)


def test_objects_that_are_neither() -> None:
    with pytest.raises(TypeError, match=r"^expected a path or a binary file object, got int$"):
        ppc.write_table(TABLE, 3)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match=r"^expected a path or a binary file object, got NoneType$"):
        ppc.Writer(None, TABLE.schema)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match=r"^expected a path or a binary file object, got object$"):
        ppc.transcode(io.BytesIO(data()), object())  # type: ignore[arg-type]
