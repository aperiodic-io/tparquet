"""Fixtures of the tests: a DuckDB connection with the built extension loaded.

The extension comes from $TPARQUET_DUCKDB_EXTENSION (a `tparquet.duckdb_extension` that
`duckdb/build.sh VERSION` built for the DuckDB version of the Python `duckdb` package the tests
run with; `make test` runs them for each).
"""

import faulthandler
import os
import pathlib
import subprocess

import pytest
from hypothesis import HealthCheck, settings

import duckdb

_slow_ok = [HealthCheck.too_slow, HealthCheck.function_scoped_fixture, HealthCheck.data_too_large]
# HYPOTHESIS_PROFILE: "default", "thorough" (more examples) or "mutation" (few fixed examples:
# `make mutation` runs the suite once per mutant)
settings.register_profile("default", max_examples=25, deadline=None, suppress_health_check=_slow_ok)
settings.register_profile("thorough", max_examples=400, deadline=None, suppress_health_check=_slow_ok)
settings.register_profile(
    "mutation", max_examples=8, deadline=None, derandomize=True, database=None, suppress_health_check=_slow_ok
)
settings.load_profile(os.environ.get("HYPOTHESIS_PROFILE", "default"))

# $TPARQUET_DUCKDB_TEST_TIMEOUT (seconds) ends the process when one test runs longer, even inside
# DuckDB's C++ code: faulthandler's watchdog dumps the stacks and exits (a mutant that hangs).
_TEST_TIMEOUT = float(os.environ.get("TPARQUET_DUCKDB_TEST_TIMEOUT", "0"))


@pytest.fixture(autouse=True)
def _test_deadline():
    if _TEST_TIMEOUT > 0:
        faulthandler.dump_traceback_later(_TEST_TIMEOUT, exit=True)
    yield
    faulthandler.cancel_dump_traceback_later()


ROOT = pathlib.Path(__file__).resolve().parents[2]
TESTDATA = ROOT / "testdata"
FILES = TESTDATA / "files"
EXPECTED = TESTDATA / "expected"
PAYLOADS = TESTDATA / "payloads"

#: the DuckDB version the tests run with ("v2.0.0", "v1.5.6")
DUCKDB_VERSION = duckdb.connect().execute("SELECT library_version FROM pragma_version()").fetchone()[0]
#: DuckDB 1.x: what DuckDB itself does differently from 2.0 (its types, its reader and binder)
DUCKDB_1 = DUCKDB_VERSION.startswith("v1.")


def extension_path() -> str:
    path = os.environ.get("TPARQUET_DUCKDB_EXTENSION")
    if not path:
        raise RuntimeError("set TPARQUET_DUCKDB_EXTENSION to the built tparquet.duckdb_extension")
    return path


def connect(**config) -> duckdb.DuckDBPyConnection:
    """A new in-memory database with the extension loaded."""
    con = duckdb.connect(config={"allow_unsigned_extensions": "true", **config})
    con.execute(f"LOAD '{extension_path()}'")
    return con


@pytest.fixture
def con():
    c = connect()
    yield c
    c.close()


@pytest.fixture
def plain():
    """A connection without the extension."""
    c = duckdb.connect()
    yield c
    c.close()


def cli() -> str:
    """The tparquet CLI ($TPARQUET_CLI; `make test` builds it)."""
    path = os.environ.get("TPARQUET_CLI")
    if not path:
        raise RuntimeError("set TPARQUET_CLI to the tparquet binary")
    return path


def run_cli(*args) -> subprocess.CompletedProcess:
    return subprocess.run([cli(), *map(str, args)], capture_output=True, text=True)


def encode(src, dst, policy: str = "pcodec"):
    """Transcodes a standard Parquet file to PCODEC with the Rust CLI."""
    out = run_cli("encode", src, dst, "--policy", policy)
    assert out.returncode == 0, out.stderr
    return dst


def rows(con, sql):
    return con.execute(sql).fetchall()


def codecs(con, path) -> dict[str, set[str]]:
    """The codecs of each column of a file, from its footer."""
    out: dict[str, set[str]] = {}
    for column, codec in rows(con, f"SELECT path_in_schema, compression FROM tparquet_metadata('{path}')"):
        out.setdefault(column, set()).add(codec)
    return out


def copy_to(con, query, path, **options):
    """COPY (query) TO path (FORMAT tparquet, options...)."""
    opts = "".join(f", {k} {v}" for k, v in options.items())
    con.execute(f"COPY ({query}) TO '{path}' (FORMAT tparquet{opts})")
    return path


def describe(con, sql):
    """(name, type) of every column of the query `sql`."""
    return [(r[0], r[1]) for r in con.execute(f"DESCRIBE {sql}").fetchall()]
