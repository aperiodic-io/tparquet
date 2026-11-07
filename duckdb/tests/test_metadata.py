"""tparquet_metadata(path): the row groups of files, from their footers only.

DuckDB's C API cannot let a table function answer count(*) from the footer: read_tparquet decodes
its first column for count(*). tparquet_metadata reads nothing but footers, so its row counts sum
to the count without reading any column data.
"""

import duckdb
import pyarrow.parquet as pq
import pytest

from conftest import connect, encode, rows

QUERY = "SELECT i AS id, 'x' || i AS s FROM range({n}) r(i)"


@pytest.fixture(scope="module")
def files(tmp_path_factory):
    d = tmp_path_factory.mktemp("metadata")
    plain = connect()
    for name, n in [("a", 5000), ("b", 2048), ("empty", 0)]:
        plain.execute(f"COPY ({QUERY.format(n=n)}) TO '{d}/{name}.std' (FORMAT parquet, ROW_GROUP_SIZE 2048)")
        encode(d / f"{name}.std", d / f"{name}.parquet")
    plain.close()
    return d


def test_one_row_per_row_group_from_the_footer(con, files):
    got = rows(con, f"SELECT * FROM tparquet_metadata('{files}/a.parquet')")
    meta = pq.ParquetFile(files / "a.parquet").metadata
    expected = []
    for i in range(meta.num_row_groups):
        rg = meta.row_group(i)
        compressed = sum(rg.column(c).total_compressed_size for c in range(rg.num_columns))
        expected.append((f"{files}/a.parquet", i, rg.num_rows, rg.num_columns, rg.total_byte_size, compressed))
    assert got == expected
    assert [d[0] for d in con.execute(f"SELECT * FROM tparquet_metadata('{files}/a.parquet')").description] == [
        "file_name", "row_group_id", "row_group_num_rows", "row_group_num_columns", "row_group_bytes",
        "row_group_compressed_bytes"]


def test_row_counts_sum_to_the_count(con, files):
    assert rows(con, f"SELECT sum(row_group_num_rows) FROM tparquet_metadata('{files}/*.parquet')") == [(7048,)]
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{files}/*.parquet')") == [(7048,)]
    assert rows(
        con, f"SELECT file_name, sum(row_group_num_rows) FROM tparquet_metadata(['{files}/b.parquet', '{files}/a.parquet']) "
        "GROUP BY ALL ORDER BY ALL"
    ) == [(f"{files}/a.parquet", 5000), (f"{files}/b.parquet", 2048)]
    # a file without row groups has no rows here
    assert rows(con, f"SELECT count(*) FROM tparquet_metadata('{files}/empty.parquet')") == [(0,)]


def test_reads_only_the_footer(con, files, tmp_path):
    damaged = tmp_path / "damaged.parquet"
    data = bytearray((files / "a.parquet").read_bytes())
    footer = int.from_bytes(data[-8:-4], "little")
    for k in range(4, len(data) - 8 - footer):
        data[k] = 0xA5
    damaged.write_bytes(bytes(data))
    with pytest.raises(duckdb.Error, match="damaged.parquet"):
        rows(con, f"SELECT sum(id) FROM read_tparquet('{damaged}')")
    assert rows(con, f"SELECT sum(row_group_num_rows) FROM tparquet_metadata('{damaged}')") == [(5000,)]


def test_errors_name_the_file(con, files, tmp_path):
    bad = tmp_path / "bad.parquet"
    bad.write_bytes(b"PAR1 nothing PAR1")
    with pytest.raises(duckdb.Error, match="cannot read .*bad.parquet as Parquet"):
        rows(con, f"SELECT * FROM tparquet_metadata('{bad}')")
    with pytest.raises(duckdb.Error, match="no files match"):
        rows(con, f"SELECT * FROM tparquet_metadata('{files}/none-*.parquet')")
    with pytest.raises(duckdb.Error, match="the path is NULL"):
        rows(con, "SELECT * FROM tparquet_metadata(NULL)")


def test_many_files_over_threads(files, tmp_path):
    for k in range(40):
        (tmp_path / f"f{k:02d}.parquet").write_bytes((files / "a.parquet").read_bytes())
    c = connect()
    c.execute("SET threads = 4")
    got = rows(c, f"SELECT file_name, row_group_id FROM tparquet_metadata('{tmp_path}/f*.parquet')")
    c.close()
    groups = pq.ParquetFile(files / "a.parquet").metadata.num_row_groups
    assert got == [(f"{tmp_path}/f{k:02d}.parquet", g) for k in range(40) for g in range(groups)]


def test_a_prepared_statement_reads_the_footers_again_each_time(con, files):
    con.execute(f"PREPARE q AS SELECT count(*), sum(row_group_num_rows) FROM tparquet_metadata('{files}/a.parquet')")
    first = rows(con, "EXECUTE q")
    assert first[0][1] == 5000
    assert rows(con, "EXECUTE q") == first
    con.execute("DEALLOCATE q")
