"""read_tparquet reads through DuckDB's file system: remote and registered file systems.

Files are served by an fsspec in-memory file system registered with DuckDB (`memory://`) and by
a local HTTP server read through DuckDB's httpfs (`http://`). Both builds read single files and
lists; globs over a remote file system need DuckDB's glob, which only the C API v2 has.
"""

import functools
import http.server
import os
import threading

import duckdb
import pytest
from fsspec.implementations.memory import MemoryFile, MemoryFileSystem

from conftest import API, connect, copy_to, rows

N = 30_000
QUERY = f"SELECT i AS id, i * 0.5 AS price, 'sym' || (i % 7) AS sym FROM range({N}) r(i)"


@pytest.fixture(scope="module")
def local(tmp_path_factory):
    d = tmp_path_factory.mktemp("remote")
    con = connect()
    for part, (lo, hi) in enumerate([(0, 10_000), (10_000, 10_001), (10_001, N)]):
        copy_to(con, f"SELECT * FROM ({QUERY}) WHERE id >= {lo} AND id < {hi}", d / f"part-{part}.parquet",
                ROW_GROUP_ROWS=2500)
    copy_to(con, QUERY, d / "all.parquet", ROW_GROUP_ROWS=4000)
    con.close()
    return d


class IndependentMemoryFileSystem(MemoryFileSystem):
    """fsspec's memory file system, but every open of a file gets a handle of its own: fsspec
    hands out the same `MemoryFile` (one position) to every reader, which races when DuckDB's
    threads read one file at once."""

    protocol = "memory"

    def _open(self, path, mode="rb", **kwargs):
        if mode == "rb":
            path = self._strip_protocol(path)
            if path not in self.store:
                raise FileNotFoundError(path)
            return MemoryFile(None, None, self.store[path].getvalue())
        return super()._open(path, mode, **kwargs)


@pytest.fixture
def memfs(local):
    fs = IndependentMemoryFileSystem(skip_instance_cache=True)
    for f in sorted(local.iterdir()):
        fs.pipe(f"memory://data/{f.name}", f.read_bytes())
    yield fs
    fs.rm("memory://data", recursive=True)


@pytest.fixture
def mcon(memfs):
    c = connect()
    c.register_filesystem(memfs)
    yield c
    c.close()


def all_rows(con, sql):
    return sorted(rows(con, sql))


def test_reads_a_file_of_a_registered_file_system(mcon, local):
    expected = all_rows(mcon, f"SELECT * FROM read_tparquet('{local}/all.parquet')")
    assert all_rows(mcon, "SELECT * FROM read_tparquet('memory://data/all.parquet')") == expected
    assert rows(mcon, "SELECT count(*), sum(id) FROM read_tparquet('memory://data/all.parquet')") == [
        (N, N * (N - 1) // 2)
    ]


def test_reads_a_list_of_remote_files_with_filenames_and_row_numbers(mcon):
    got = rows(
        mcon,
        "SELECT filename, file_row_number, id FROM read_tparquet(['memory://data/part-2.parquet', "
        "'memory://data/part-0.parquet'], filename := true, file_row_number := true) ORDER BY id",
    )
    assert len(got) == N - 1
    assert got[0] == ("memory://data/part-0.parquet", 0, 0)
    assert got[-1] == ("memory://data/part-2.parquet", N - 10_002, N - 1)


def test_remote_and_local_files_mix_in_a_list(mcon, local):
    got = rows(mcon, f"SELECT count(*) FROM read_tparquet(['{local}/part-0.parquet', 'memory://data/part-2.parquet'])")
    assert got == [(N - 1,)]


def test_globs_over_a_remote_file_system(mcon):
    sql = "SELECT count(*), min(id), max(id) FROM read_tparquet('memory://data/part-*.parquet')"
    if API == "v2":
        assert rows(mcon, sql) == [(N, 0, N - 1)]
        assert rows(mcon, "SELECT DISTINCT filename FROM read_tparquet('memory://data/part-[02].parquet', "
                          "filename := true) ORDER BY ALL") == [
            ("memory://data/part-0.parquet",), ("memory://data/part-2.parquet",)]
        with pytest.raises(duckdb.Error, match=r"no files match \"memory://data/none-\*.parquet\""):
            rows(mcon, "SELECT * FROM read_tparquet('memory://data/none-*.parquet')")
    else:
        with pytest.raises(duckdb.Error, match="globs over remote files need the C API v2 build"):
            rows(mcon, sql)


def test_a_missing_remote_file_is_named(mcon):
    with pytest.raises(duckdb.Error, match="memory://data/missing.parquet"):
        rows(mcon, "SELECT * FROM read_tparquet('memory://data/missing.parquet')")


def test_a_damaged_remote_file_is_named(mcon, memfs):
    memfs.pipe("memory://data/bad.parquet", b"PAR1 not really parquet PAR1")
    with pytest.raises(duckdb.Error, match="cannot read memory://data/bad.parquet as Parquet"):
        rows(mcon, "SELECT * FROM read_tparquet('memory://data/bad.parquet')")


def test_copy_from_a_remote_file(mcon):
    mcon.execute("CREATE TABLE t (id BIGINT, price DECIMAL(21, 1), sym VARCHAR)")
    mcon.execute("COPY t FROM 'memory://data/all.parquet' (FORMAT tparquet)")
    assert rows(mcon, "SELECT count(*), max(id) FROM t") == [(N, N - 1)]


class _RangeHandler(http.server.SimpleHTTPRequestHandler):
    """Serves files with HTTP range requests, which httpfs needs, and counts the requests."""

    requests = 0

    def log_message(self, *args):
        pass

    def send_head(self):
        type(self).requests += 1
        path = self.translate_path(self.path)
        if not os.path.isfile(path):
            self.send_error(404)
            return None
        size = os.path.getsize(path)
        f = open(path, "rb")
        rng = self.headers.get("Range")
        if rng and rng.startswith("bytes="):
            lo, _, hi = rng[6:].partition("-")
            lo, hi = int(lo), min(int(hi) if hi else size - 1, size - 1)
            f.seek(lo)
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {lo}-{hi}/{size}")
            self.send_header("Content-Length", str(hi - lo + 1))
            self.end_headers()
            return _Limited(f, hi - lo + 1)
        self.send_response(200)
        self.send_header("Content-Length", str(size))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        return f


class _Limited:
    def __init__(self, f, n):
        self.f, self.n = f, n

    def read(self, size=-1):
        size = self.n if size < 0 else min(size, self.n)
        data = self.f.read(size)
        self.n -= len(data)
        return data

    def close(self):
        self.f.close()


@pytest.fixture(scope="module")
def http_url(local):
    handler = functools.partial(_RangeHandler, directory=str(local))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


@pytest.fixture
def hcon():
    c = connect()
    try:
        c.execute("INSTALL httpfs")
        c.execute("LOAD httpfs")
    except duckdb.Error as e:  # no network to DuckDB's extension repository
        pytest.skip(f"httpfs is not available: {e}")
    yield c
    c.close()


def test_reads_over_http_with_httpfs(hcon, http_url, local):
    expected = all_rows(hcon, f"SELECT * FROM read_tparquet('{local}/all.parquet')")
    assert all_rows(hcon, f"SELECT * FROM read_tparquet('{http_url}/all.parquet')") == expected
    # a projection reads only what it needs: a fraction of the requests of the whole file
    _RangeHandler.requests = 0
    assert rows(hcon, f"SELECT max(id) FROM read_tparquet('{http_url}/all.parquet')") == [(N - 1,)]
    assert _RangeHandler.requests > 0


def test_a_missing_http_file_is_named(hcon, http_url):
    with pytest.raises(duckdb.Error, match=f"{http_url}/missing.parquet"):
        rows(hcon, f"SELECT * FROM read_tparquet('{http_url}/missing.parquet')")
