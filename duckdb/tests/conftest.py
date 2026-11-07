"""Fixtures of the SQL tests: a DuckDB connection with the built extension loaded.

The extension comes from $TPARQUET_DUCKDB_EXTENSION (a `tparquet.duckdb_extension`, as
`make build` or the cargo test `tests/sql.rs` makes it).
"""

import faulthandler
import os
import pathlib
import subprocess
import sys
import threading
import time

import duckdb
import pytest
from hypothesis import HealthCheck, settings

_slow_ok = [HealthCheck.too_slow, HealthCheck.function_scoped_fixture, HealthCheck.data_too_large]
# HYPOTHESIS_PROFILE: "default", "thorough" (more examples; `make thorough`) or "mutation" (few
# fixed examples: `make mutation` runs the suite once per mutant)
settings.register_profile("default", max_examples=25, deadline=None, suppress_health_check=_slow_ok)
settings.register_profile("thorough", max_examples=400, deadline=None, suppress_health_check=_slow_ok)
settings.register_profile(
    "mutation", max_examples=8, deadline=None, derandomize=True, database=None, suppress_health_check=_slow_ok
)
settings.load_profile(os.environ.get("HYPOTHESIS_PROFILE", "default"))


def _watch_memory(limit_mb: int) -> None:
    """Ends the process when its resident memory passes `limit_mb`."""
    page = os.sysconf("SC_PAGE_SIZE")
    while True:
        with open("/proc/self/statm") as f:
            resident = int(f.read().split()[1]) * page
        if resident > limit_mb << 20:
            print(f"conftest: resident memory {resident >> 20} MB is over {limit_mb} MB", file=sys.stderr)
            os._exit(3)
        time.sleep(0.2)


# $TPARQUET_DUCKDB_MEMORY_LIMIT_MB bounds the resident memory of the test process (`make
# mutation` sets it). A mutant that makes a scan loop forever produces rows without end, about
# 65 MB/s, which exhausts a CI runner before cargo-mutants' timeout. Over the limit the process
# exits, the tests fail, and the mutant is caught. (An address-space limit does not work:
# DuckDB and the other native libraries reserve far more address space than they use.)
if limit := os.environ.get("TPARQUET_DUCKDB_MEMORY_LIMIT_MB"):
    threading.Thread(target=_watch_memory, args=(int(limit),), daemon=True).start()

# $TPARQUET_DUCKDB_TEST_TIMEOUT (seconds, `make mutation` sets it) ends the process when one test
# runs longer, even inside DuckDB's C code: faulthandler's watchdog dumps the stacks and exits.
# An endless mutant then fails in that time, long before cargo-mutants' own timeout, at which
# CI runners were seen to shut down.
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


# Which build of the extension the tests load: "v1" (DuckDB's stable C API v1.5.6, any DuckDB
# that serves it) or "v2" (the unstable C API v2, exactly DuckDB 2.0.0). Tests of what only v2
# can do assert v1's documented behavior instead.
API = os.environ.get("TPARQUET_DUCKDB_API", "v1")
assert API in ("v1", "v2"), API


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
    """The tparquet CLI ($TPARQUET_CLI, built by tests/sql.rs or `make build`)."""
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


def describe(con, sql):
    """(name, type) of every column of the query `sql`."""
    return [(r[0], r[1]) for r in con.execute(f"DESCRIBE {sql}").fetchall()]


def copy_to(con, query, path, **options):
    """COPY (query) TO path (FORMAT tparquet), naming the columns as the query does."""
    names = ", ".join("'" + n.replace("'", "''") + "'" for n, _ in describe(con, query))
    opts = "".join(f", {k} {v}" for k, v in options.items())
    con.execute(f"COPY ({query}) TO '{path}' (FORMAT tparquet, COLUMN_NAMES [{names}]{opts})")
    return path
