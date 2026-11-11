"""Corrupt and truncated files: every way of reading one raises a Python exception
(PcodecError, or the OSError of the I/O), never crashes the interpreter and never returns
wrong rows silently where the footer says otherwise. The full sweep is `slow`."""

from __future__ import annotations

import contextlib
import io
import os
import pathlib
import random
import subprocess
import sys
import textwrap

import pyarrow as pa
import pytest
from hypothesis import given
from hypothesis import strategies as st

import tparquet as ppc
from conftest import TESTDATA, manifest
from tparquet import canonical

VECTORS = [TESTDATA / f["file"] for f in manifest()]


def every_read(data: bytes) -> None:
    """Reads `data` every way the package offers; raises what the first failure raises."""
    f = ppc.ParquetFile(io.BytesIO(data))
    _ = f.metadata, f.integer_bounds(), f._column_bounds(), repr(f)
    canonical.dumps(f.read(batch_size=97))
    pa.RecordBatchReader.from_stream(f.stream(limit=10)).read_all()
    ppc.inspect(io.BytesIO(data))
    ppc.verify(io.BytesIO(data), io.BytesIO(data))
    ppc.transcode(io.BytesIO(data), io.BytesIO())
    ppc.transcode(io.BytesIO(data), io.BytesIO(), target="standard")


def fails_cleanly(data: bytes) -> None:
    with contextlib.suppress(ppc.PcodecError, pa.ArrowException):
        every_read(data)


def small_file() -> bytes:
    buf = io.BytesIO()
    table = pa.table(
        {
            "i": pa.array(range(500), pa.int64()),
            "f": pa.array([i / 8 for i in range(500)]),
            "l": pa.array([[i, None][: i % 3] for i in range(500)], pa.list_(pa.int32())),
            "s": pa.array([str(i % 9) for i in range(500)]),
        }
    )
    ppc.write_table(table, buf, policy="pcodec", row_group_size=200)
    return buf.getvalue()


@pytest.mark.parametrize("cut", [0, 1, 3, 4, 7, 8, 100, -1, -4, -5, -8, -9, -12])
def test_truncated_files(cut: int) -> None:
    data = small_file()
    with pytest.raises(ppc.PcodecError):
        every_read(data[:cut])


def test_a_footer_that_claims_more_than_the_file_holds() -> None:
    data = bytearray(small_file())
    # the footer length (the 4 bytes before the closing magic) past the start of the file
    data[-8:-4] = (len(data) * 2).to_bytes(4, "little")
    with pytest.raises(ppc.PcodecError):
        every_read(bytes(data))
    data[-8:-4] = (2**32 - 1).to_bytes(4, "little")
    with pytest.raises(ppc.PcodecError):
        every_read(bytes(data))


def test_the_middle_of_a_file_cut_out(tmp_path: pathlib.Path) -> None:
    data = small_file()
    # the footer is intact, the pages it points at are not where it says
    shifted = data[:4] + data[200:]
    with pytest.raises(ppc.PcodecError):
        every_read(shifted)
    # as a path too: the error names the file
    path = tmp_path / "shifted.parquet"
    path.write_bytes(shifted)
    with pytest.raises(ppc.PcodecError, match=r"shifted\.parquet"):
        ppc.read_table(path)


def test_a_file_truncated_after_it_was_opened(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "t.parquet"
    path.write_bytes(small_file())
    f = ppc.ParquetFile(path)
    with open(path, "r+b") as handle:
        handle.truncate(100)
    with pytest.raises(ppc.PcodecError, match=r"t\.parquet"):
        f.read()


@given(seed=st.integers(0, 2**32 - 1), flips=st.integers(1, 12))
def test_flipped_bytes_raise_cleanly(seed: int, flips: int) -> None:
    r = random.Random(seed)
    data = bytearray(small_file())
    for _ in range(flips):
        data[r.randrange(len(data))] = r.randrange(256)
    fails_cleanly(bytes(data))


@given(payload=st.binary(max_size=200), at=st.integers(0, 2000))
def test_garbage_spliced_in(payload: bytes, at: int) -> None:
    data = small_file()
    at = min(at, len(data))
    fails_cleanly(data[:at] + payload + data[at:])


@pytest.mark.slow
@pytest.mark.parametrize("path", VECTORS, ids=[p.stem for p in VECTORS])
def test_every_prefix_of_every_vector(path: pathlib.Path) -> None:
    data = path.read_bytes()
    step = max(1, len(data) // 400)
    for n in range(0, len(data), step):
        fails_cleanly(data[:n])


@pytest.mark.slow
@pytest.mark.parametrize("path", VECTORS, ids=[p.stem for p in VECTORS])
def test_random_damage_to_every_vector(path: pathlib.Path) -> None:
    r = random.Random(path.stem)
    original = path.read_bytes()
    for _ in range(200):
        data = bytearray(original)
        for _ in range(r.randint(1, 8)):
            data[r.randrange(len(data))] = r.randrange(256)
        fails_cleanly(bytes(data))


def test_the_interpreter_survives_in_a_subprocess() -> None:
    # a crash (abort, segfault) would kill this process: run a sweep in a child
    code = textwrap.dedent(
        """
        import io, random, sys
        sys.path.insert(0, sys.argv[1])
        import test_corrupt as t
        data = t.small_file()
        r = random.Random(7)
        for n in range(0, len(data), 37):
            t.fails_cleanly(data[:n])
        for _ in range(150):
            d = bytearray(data)
            for _ in range(r.randint(1, 6)):
                d[r.randrange(len(d))] = r.randrange(256)
            t.fails_cleanly(bytes(d))
        print("survived")
        """
    )
    tests = str(pathlib.Path(__file__).parent)
    out = subprocess.run(
        [sys.executable, "-c", code, tests], capture_output=True, text=True, env={**os.environ, "RUST_BACKTRACE": "0"}
    )
    assert out.returncode == 0, out.stderr[-2000:]
    assert out.stdout.strip() == "survived"
