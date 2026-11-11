"""The extension loads into the DuckDB it was built for, next to DuckDB's own parquet extension,
and changes nothing of it."""

import pathlib
import shutil

import pytest
from conftest import DUCKDB_1, DUCKDB_VERSION, FILES, connect, extension_path, rows
from sign import metadata

import duckdb

FUNCTIONS = [
    "read_tparquet",
    "tparquet_scan",
    "read_single_tparquet_file",
    "tparquet_metadata",
    "tparquet_schema",
    "tparquet_kv_metadata",
    "tparquet_file_metadata",
    "tparquet_bloom_probe",
    "tparquet_full_metadata",
]
if DUCKDB_1:
    FUNCTIONS.remove("read_single_tparquet_file")  # new in DuckDB 2.0

PCODEC_FILE = FILES / "mixed.parquet"


def test_the_footer_names_this_duckdb_and_the_cpp_abi():
    meta = metadata(pathlib.Path(extension_path()).read_bytes())
    assert meta is not None
    assert meta["duckdb_version"] == DUCKDB_VERSION
    assert meta["abi_type"] == "CPP"
    assert meta["platform"] == rows(duckdb.connect(), "PRAGMA platform")[0][0]


def test_registers_its_functions_under_names_of_its_own(con, plain):
    # none of them is DuckDB's
    assert not set(FUNCTIONS) & {r[0] for r in rows(plain, "SELECT function_name FROM duckdb_functions()")}
    names = {r[0] for r in rows(con, "SELECT function_name FROM duckdb_functions()")}
    assert set(FUNCTIONS) <= names
    assert {"read_parquet", "parquet_scan", "parquet_metadata"} <= names
    if not DUCKDB_1:  # DuckDB 1.x's duckdb_functions() names no extension
        owners = dict(rows(con, "SELECT function_name, any_value(extension_name) FROM duckdb_functions() GROUP BY ALL"))
        for f in FUNCTIONS:
            assert owners[f] == "tparquet", f
        assert owners["read_parquet"] == "parquet"
    assert rows(con, "SELECT extension_name, loaded FROM duckdb_extensions() WHERE extension_name = 'tparquet'") == [
        ("tparquet", True)
    ]


def test_duckdbs_own_parquet_reader_still_refuses_pcodec(con, plain):
    # spec section 2: a reader without PCODEC must fail, and read_parquet is DuckDB's, unchanged
    for c in (plain, con):
        with pytest.raises(duckdb.Error):
            rows(c, f"SELECT * FROM read_parquet('{PCODEC_FILE}')")
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{PCODEC_FILE}')")[0][0] > 0


def test_duckdbs_own_parquet_copy_is_unchanged(con, tmp_path):
    con.execute(f"COPY (SELECT range AS x FROM range(10)) TO '{tmp_path}/std.parquet' (FORMAT parquet)")
    assert rows(con, f"SELECT DISTINCT compression FROM parquet_metadata('{tmp_path}/std.parquet')") == [("SNAPPY",)]
    assert rows(duckdb.connect(), f"SELECT sum(x) FROM read_parquet('{tmp_path}/std.parquet')") == [(45,)]


def test_loads_once_per_database_and_in_many_databases():
    a, b = connect(), connect()
    a.execute(f"LOAD '{extension_path()}'")
    assert rows(a, f"SELECT count(*) FROM read_tparquet('{PCODEC_FILE}')") == rows(
        b, f"SELECT count(*) FROM read_tparquet('{PCODEC_FILE}')"
    )


def test_a_copy_of_another_name_is_refused(tmp_path):
    # DuckDB calls <file name>_duckdb_cpp_init: the extension is tparquet only
    other = tmp_path / "other.duckdb_extension"
    shutil.copy(extension_path(), other)
    c = duckdb.connect(config={"allow_unsigned_extensions": "true"})
    with pytest.raises(duckdb.Error, match="other"):
        c.execute(f"LOAD '{other}'")


def test_needs_allow_unsigned_extensions_when_unsigned():
    c = duckdb.connect()
    with pytest.raises(duckdb.Error, match="signature|signed"):
        c.execute(f"LOAD '{extension_path()}'")
