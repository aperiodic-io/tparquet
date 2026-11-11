"""Damaged, truncated and hostile files: every one gives an error (never a crash, a hang or an
internal error of DuckDB), and the database stays usable."""

import os
import random

import pyarrow as pa
import pyarrow.parquet as pq
import pytest
from conftest import FILES

import duckdb

VECTORS = sorted(FILES.glob("*.parquet"))


def scan(con, path):
    """Reads every column of `path`; None when it reads, else the error message."""
    try:
        con.execute(f"SELECT * FROM read_tparquet('{path}')").fetchall()
        return None
    except duckdb.Error as e:
        return str(e)


def check_error(message, path, con=None):
    """An error of DuckDB's parquet reader, not an internal one (which would invalidate the
    database)."""
    assert "INTERNAL" not in message.upper().split(":")[0], message
    assert "internal error" not in message.lower(), message
    if con is not None:
        assert con.execute("SELECT 42").fetchall() == [(42,)]


def test_zeroed_bytes_anywhere_in_a_file_give_errors(con, tmp_path):
    src = (FILES / "primitives_optional.parquet").read_bytes()
    damaged = tmp_path / "damaged.parquet"
    errors = 0
    for at in range(4, len(src) - 8, 3):
        data = bytearray(src)
        data[at : at + 4] = bytes(4)
        damaged.write_bytes(bytes(data))
        message = scan(con, damaged)
        if message is not None:
            check_error(message, damaged)
            errors += 1
    assert errors > 0
    assert con.execute("SELECT 42").fetchall() == [(42,)]


@pytest.mark.parametrize("vector", VECTORS, ids=lambda p: p.stem)
def test_every_truncation_of_a_vector_is_an_error(con, tmp_path, vector):
    src = vector.read_bytes()
    cut = tmp_path / "cut.parquet"
    # every length near both ends (the footer and the magic), and a spread in between
    lengths = sorted({*range(64), *range(len(src) - 64, len(src)), *range(0, len(src), max(1, len(src) // 97))})
    for n in lengths:
        cut.write_bytes(src[:n])
        message = scan(con, cut)
        assert message is not None, n
        check_error(message, cut)


def test_random_damage_never_crashes(con, tmp_path):
    rnd = random.Random(int(os.environ.get("TPARQUET_DUCKDB_FUZZ_SEED", "20547")))
    damaged = tmp_path / "damaged.parquet"
    for _ in range(int(os.environ.get("TPARQUET_DUCKDB_FUZZ_CASES", "300"))):
        vector = rnd.choice(VECTORS)
        data = bytearray(vector.read_bytes())
        for _ in range(rnd.randrange(1, 8)):
            at = rnd.randrange(len(data))
            data[at] = rnd.randrange(256)
        damaged.write_bytes(bytes(data))
        message = scan(con, damaged)
        if message is not None:
            check_error(message, damaged)
    assert con.execute("SELECT 42").fetchall() == [(42,)]


def test_a_file_that_is_only_the_magic(con, tmp_path):
    for content in [b"PAR1", b"PAR1PAR1", b"PAR1\x00\x00\x00\x00PAR1", b"PAR1\xff\xff\xff\x7fPAR1"]:
        p = tmp_path / "magic.parquet"
        p.write_bytes(content)
        check_error(scan(con, p), p)


def test_a_decimal_beyond_its_precision_reads_as_read_parquet_reads_it(con, tmp_path):
    # a standard file with a value beyond its precision (19 digits in DECIMAL(18, 0)), as a
    # damaged file can hold; the buffers are built by hand, pyarrow would refuse the value.
    # read_tparquet is DuckDB's reader: it does what read_parquet does with it
    values = (10**18).to_bytes(16, "little", signed=True) + (5).to_bytes(16, "little", signed=True)
    arr = pa.Array.from_buffers(pa.decimal128(18, 0), 2, [None, pa.py_buffer(values)])
    path = tmp_path / "wide.parquet"
    pq.write_table(pa.table({"d": arr}), path)

    def outcome(function):
        try:
            return con.execute(f"SELECT d::VARCHAR FROM {function}('{path}')").fetchall()
        except duckdb.Error as e:
            return str(e).replace("read_tparquet", "read_parquet")

    assert outcome("read_tparquet") == outcome("read_parquet")
