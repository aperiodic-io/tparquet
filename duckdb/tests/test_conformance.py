"""Every conformance vector, read through read_tparquet, gives exactly the expected rows."""

import json

import pyarrow as pa
import pyarrow.parquet as pq
import pytest
from canonical import lines
from conftest import DUCKDB_1, EXPECTED, FILES, connect

NAMES = sorted(p.stem for p in FILES.glob("*.parquet"))

# DuckDB 1.x has no TIMESTAMP WITH TIME ZONE in nanoseconds: it reads temporal's ts_ns_utc as
# microseconds, as its read_parquet does (test_a_nanosecond_utc_timestamp_reads_as_read_parquet)
LOSSY = {"temporal"} if DUCKDB_1 else set()


def vector(name):
    return pytest.param(name, marks=pytest.mark.xfail(strict=True, reason="DuckDB 1.x: no TIMESTAMPTZ_NS")) if name in LOSSY else name


def test_every_vector_is_covered():
    assert len(NAMES) == 24
    assert sorted(p.stem for p in EXPECTED.glob("*.jsonl")) == NAMES


@pytest.mark.parametrize("name", [vector(n) for n in NAMES])
def test_canonical_rows_match_byte_for_byte(con, name):
    path = FILES / f"{name}.parquet"
    table = con.sql(
        f"SELECT * EXCLUDE (file_row_number) FROM read_tparquet('{path}', file_row_number := true) "
        "ORDER BY file_row_number"
    ).to_arrow_table()
    got = lines(table, pq.read_schema(path))
    want = (EXPECTED / f"{name}.jsonl").read_text(encoding="utf-8")
    assert got == want


@pytest.mark.parametrize("name", ["multiple_row_groups", "mixed", "pages_v1"])
def test_one_thread_reads_in_file_order(con, name):
    con.execute("SET threads = 1")
    path = FILES / f"{name}.parquet"
    table = con.sql(f"SELECT * FROM read_tparquet('{path}')").to_arrow_table()
    assert lines(table, pq.read_schema(path)) == (EXPECTED / f"{name}.jsonl").read_text(encoding="utf-8")


MANIFEST = {f["name"]: f for f in json.loads((FILES / "manifest.json").read_text())["files"]}


@pytest.mark.parametrize("name", NAMES)
def test_rows_and_row_groups_match_the_manifest(con, name):
    path = FILES / f"{name}.parquet"
    assert con.execute(f"SELECT count(*) FROM read_tparquet('{path}')").fetchone() == (MANIFEST[name]["rows"],)
    assert pq.ParquetFile(path).metadata.num_row_groups == MANIFEST[name]["row_groups"]


@pytest.mark.parametrize("name", [vector(n) for n in NAMES])
def test_every_single_column_reads_its_expected_values(con, name):
    path = FILES / f"{name}.parquet"
    schema = pq.read_schema(path)
    want = [json.loads(line) for line in (EXPECTED / f"{name}.jsonl").read_text(encoding="utf-8").splitlines()]
    for column in schema.names:
        table = con.sql(
            f'SELECT "{column}" FROM read_tparquet(\'{path}\', file_row_number := true) ORDER BY file_row_number'
        ).to_arrow_table()
        got = [json.loads(line) for line in lines(table, pa.schema([schema.field(column)])).splitlines()]
        assert got == [{column: r[column]} for r in want], column


@pytest.mark.parametrize("name", [vector(n) for n in NAMES])
def test_four_threads_read_the_same_rows(name):
    c = connect()
    c.execute("SET threads = 4")
    path = FILES / f"{name}.parquet"
    table = c.sql(f"SELECT * FROM read_tparquet('{path}')").to_arrow_table()
    c.close()
    got = sorted(lines(table, pq.read_schema(path)).splitlines())
    assert got == sorted((EXPECTED / f"{name}.jsonl").read_text(encoding="utf-8").splitlines())
