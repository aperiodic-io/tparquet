"""Acceptance: DuckDB alone refuses PCODEC; standard files read the same; the ratio target."""

import os
import subprocess
import sys

import pyarrow as pa
import pyarrow.parquet as pq
import pytest
from canonical import lines
from conftest import FILES, extension_path

import duckdb

TPARQUET_VECTORS = sorted(p.stem for p in FILES.glob("*.parquet") if p.stem != "standard_control")


@pytest.mark.parametrize("name", TPARQUET_VECTORS)
def test_a_builtin_read_parquet_fails_loudly_on_pcodec(plain, name):
    """(a) DuckDB 2.0.0's own reader cannot read PCODEC and says so; it never returns rows."""
    with pytest.raises(duckdb.Error) as e:
        plain.execute(f"SELECT * FROM read_parquet('{FILES}/{name}.parquet')").fetchall()
    # DuckDB's Thrift parser rejects the unknown CompressionCodec value 20547 in the footer
    assert str(e.value).splitlines()[0] == "TProtocolException: Invalid data"


def test_b_standard_control_reads_like_builtin_read_parquet(con):
    """(b) a plain Parquet file reads through read_tparquet exactly as DuckDB reads it."""
    f = FILES / "standard_control.parquet"
    ours = con.sql(f"SELECT * FROM read_tparquet('{f}')").to_arrow_table()
    theirs = con.sql(f"SELECT * FROM read_parquet('{f}')").to_arrow_table()
    schema = pq.read_schema(f)
    assert lines(ours, schema) == lines(theirs, schema)
    assert lines(ours, schema) == (FILES.parent / "expected" / "standard_control.jsonl").read_text()


# deterministic market-like ticks: millisecond timestamps with random gaps, prices on a 0.01
# tick grid (a random walk), lot sizes in steps of 0.001, a side flag; hash() gives the noise
MARKET = """
SELECT
    epoch_ms((1735828200000 + sum((hash(i) % 40)::BIGINT) OVER w)::BIGINT)::TIMESTAMP_MS AS ts,
    round(25000 + (sum((hash(i * 3 + 1) % 21)::BIGINT - 10) OVER w) * 0.01, 2)::DOUBLE AS price,
    ((((hash(i * 7 + 2) % 500)::BIGINT) + 1) * 0.001)::DOUBLE AS qty,
    hash(i * 11 + 3) % 2 = 0 AS buy
FROM range({rows}) r(i)
WINDOW w AS (ORDER BY i ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)
"""


def test_c_ratio_against_duckdb_zstd_9_with_a_bit_exact_round_trip(con, tmp_path):
    """(c) at most 0.8 of DuckDB's COPY (FORMAT parquet, COMPRESSION zstd, COMPRESSION_LEVEL 9)."""
    con.execute(f"CREATE TABLE ticks AS {MARKET.format(rows=2_000_000)}")
    zstd = tmp_path / "zstd9.parquet"
    con.execute(f"COPY ticks TO '{zstd}' (FORMAT parquet, COMPRESSION zstd, COMPRESSION_LEVEL 9)")
    ours = tmp_path / "pcodec.parquet"
    con.execute(
        f"COPY ticks TO '{ours}' (FORMAT tparquet, COMPRESSION_LEVEL 8)"
    )
    ratio = ours.stat().st_size / zstd.stat().st_size
    print(f"\ntparquet {ours.stat().st_size} bytes, DuckDB zstd 9 {zstd.stat().st_size} bytes, ratio {ratio:.3f}")
    with open(os.environ.get("TPARQUET_DUCKDB_RATIO_LOG", os.devnull), "a") as log:
        log.write(f"{ours.stat().st_size} {zstd.stat().st_size} {ratio:.4f}\n")
    assert ratio <= 0.8
    back = f"read_tparquet('{ours}')"
    for a, b in [("ticks", back), (back, "ticks")]:
        assert con.execute(f"SELECT count(*) FROM (SELECT * FROM {a} EXCEPT ALL SELECT * FROM {b})").fetchall() == [(0,)]
    # bit for bit, in order: the doubles compared as their 64-bit patterns, against DuckDB's own
    # Parquet round trip (which reads TIMESTAMP_MS back as TIMESTAMP, as read_tparquet does)
    want = con.sql(f"SELECT * FROM read_parquet('{zstd}')").to_arrow_table()
    got = con.sql(f"SELECT * FROM {back}").to_arrow_table()
    assert got.schema == want.schema
    for name in ["price", "qty"]:
        a, b = (t.column(name).combine_chunks().view(pa.uint64()) for t in (got, want))
        assert a.equals(b), name
    for name in ["ts", "buy"]:
        assert got.column(name).combine_chunks().equals(want.column(name).combine_chunks()), name


@pytest.mark.slow
def test_reading_streams_with_bounded_memory(tmp_path):
    """A scan holds a row group's pages, not the file: reading 400 MB of values (written as a
    small PCODEC file) keeps the process's peak memory far below that."""
    path = tmp_path / "big.parquet"
    code = f"""
import duckdb, resource, sys
c = duckdb.connect(config={{'allow_unsigned_extensions': 'true', 'threads': 2}})
c.execute("LOAD '{extension_path()}'")
if sys.argv[1] == 'write':
    c.execute("COPY (SELECT i AS a, i * 3 AS b, i % 1000 AS c, (i * 0.25)::DOUBLE AS d, i * 7 AS e "
              "FROM range(10000000) r(i)) TO '{path}' "
              "(FORMAT tparquet, ROW_GROUP_SIZE 1000000)")
else:
    before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    r = c.execute("SELECT sum(a), sum(b), sum(c), sum(d), sum(e) FROM read_tparquet('{path}')").fetchall()
    assert r[0][0] == 49999995000000, r
    print(before, resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
"""
    subprocess.run([sys.executable, "-I", "-c", code, "write"], check=True)
    assert path.stat().st_size < 50_000_000
    out = subprocess.run([sys.executable, "-I", "-c", code, "read"], check=True, capture_output=True, text=True)
    before, after = map(int, out.stdout.split())
    grown_mb = (after - before) / 1024
    print(f"\npeak RSS {before // 1024} MB before the scan, {after // 1024} MB after (400 MB of values read)")
    assert grown_mb < 200
