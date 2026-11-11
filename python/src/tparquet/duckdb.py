"""PCODEC Parquet files in DuckDB-Python, without the native extension (needs duckdb).

DuckDB scans the files through the Arrow C stream: `register` makes a file a table of a
connection, `sql_view` a SQL view, `read_parquet` a relation. Each scan opens a new stream,
so they can be queried any number of times.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import duckdb

from tparquet._core import ParquetFile, Source, _names

if TYPE_CHECKING:
    from collections.abc import Sequence

__all__ = ["ArrowSource", "read_parquet", "register", "sql_view"]


class ArrowSource:
    """A Parquet file as an Arrow source: every `__arrow_c_stream__` call opens a new stream
    of `columns` (all by default) in batches of `batch_size` rows."""

    def __init__(self, source: Source, columns: Sequence[str] | None = None, batch_size: int = 65536) -> None:
        self._file = ParquetFile(source)
        self._columns = _names(columns)
        self._batch_size = batch_size
        # an unknown column is an error now, not when DuckDB first reads the source
        self._file.empty(self._columns)

    def __arrow_c_stream__(self, requested_schema: object | None = None) -> object:
        return self._file.stream(self._columns, None, self._batch_size).__arrow_c_stream__(requested_schema)

    def __arrow_c_schema__(self) -> object:
        return self._file.empty(self._columns).__arrow_c_schema__()

    def __repr__(self) -> str:
        return f"<ArrowSource {self._file!r} columns={self._columns}>"


def register(
    con: duckdb.DuckDBPyConnection, name: str, source: Source, *, columns: Sequence[str] | None = None
) -> duckdb.DuckDBPyConnection:
    """Registers the Parquet file `source` as the table `name` of `con`; returns `con`."""
    return con.register(name, ArrowSource(source, columns))


def _quote(name: str) -> str:
    return '"' + name.replace('"', '""') + '"'


def sql_view(
    con: duckdb.DuckDBPyConnection, name: str, source: Source, *, columns: Sequence[str] | None = None
) -> duckdb.DuckDBPyConnection:
    """Creates (or replaces) the temporary SQL view `name` over the Parquet file `source`, for
    queries and other views of `con`; returns `con`."""
    hidden = f"__tparquet_{name}"
    register(con, hidden, source, columns=columns)
    con.execute(f"create or replace temp view {_quote(name)} as select * from {_quote(hidden)}")
    return con


def read_parquet(
    source: Source, *, connection: duckdb.DuckDBPyConnection | None = None, columns: Sequence[str] | None = None
) -> duckdb.DuckDBPyRelation:
    """A relation over the Parquet file `source`, of `connection` (DuckDB's default connection
    when `None`), like `duckdb.read_parquet`."""
    con = duckdb.default_connection() if connection is None else connection
    return con.from_arrow(ArrowSource(source, columns))
