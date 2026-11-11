# tparquet (Python)

Read and write Parquet files that use PCODEC, a Parquet compression codec for numeric columns
(`CompressionCodec` 20547, see `spec/FORMAT.md`). Any Parquet file reads, PCODEC or not.

```python
import tparquet as tpq

choice = tpq.write_table(table, "trades.parquet")      # {"price": "PCODEC", "sym": "ZSTD", ...}
table = tpq.read_table("trades.parquet", columns=["ts", "price"])
stream = tpq.read_batches("trades.parquet")              # __arrow_c_stream__, for any Arrow library
```

- `write_table(data, path, policy="auto", columns=None, level=8, base="zstd(3)", row_group_size=None,
  encodings=None)`: `data` is anything with `__arrow_c_stream__` or `__arrow_c_array__`; `policy` is
  `"auto"` (PCODEC or the base codec per column, whichever is smaller on the first batch), `"pcodec"`
  or `"base"`, `columns` maps a column name or dotted leaf path to its own policy, and `encodings`
  maps one to the Parquet encoding of its base-codec pages (`"ALP"`, `"BYTE_STREAM_SPLIT"`,
  `"DELTA_BINARY_PACKED"`..., no dictionary; PCODEC columns stay PLAIN).
- `Writer(path, schema, ...)`: a streaming context manager with the same options.
- `ParquetFile(path)`: `.schema`, `.metadata`, `.iter_batches()`, `.read()`.
- `transcode`, `inspect`, `verify`; `tparquet.codec` for the page payload alone.
- `tparquet.pandas`: `read_parquet`, `to_parquet` (index and dtypes as with pyarrow).
- `tparquet.polars`: `scan_parquet` (projection, limit, predicate and row-group pruning
  pushed down), `read_parquet`, `write_parquet`.
- `tparquet.duckdb`: `register`, `sql_view`, `read_parquet`.

Paths are `str`, `bytes` or `os.PathLike` (names that are not UTF-8 work), or binary file objects
(a text-mode file is a `TypeError`; a file object to read from is read to its end once). Errors are
`PcodecError` (a `ValueError`) or the `OSError` of the failed I/O; read errors of a file opened from
a path start with the path. Readers without PCODEC support (pyarrow, Polars, DuckDB) fail on PCODEC
files instead of returning wrong data; `transcode(..., target="standard")` writes a copy they read.

- A `ParquetFile` keeps its file open (one descriptor, until it is garbage collected), so it reads
  the file whose footer it read even if the path is replaced or removed afterwards.
- The options and the schema are checked before a path is created or truncated. A write that fails
  (`write_table`, `transcode`, a `Writer` that is aborted or fails to close) removes the file it
  created; `abort()` does nothing once the writer is closed or aborted.
- The data given to a `Writer` must have the schema's column names, in order, as well as its types.
- `base` is a codec as the `parquet` crate names it (`zstd(3)`, `gzip(6)`, `brotli(5)`, `snappy`,
  `lz4`, `lz4_raw`, `uncompressed`); PCODEC and LZO are not base codecs. Unions, and month-day-nano
  intervals, cannot be written. A dictionary of booleans is written, but the `parquet` crate cannot
  read it back.
- Integer options must be `int`s of 64 bits (else `TypeError` or `PcodecError`), and `columns` a
  sequence of names, not a string.
- `tparquet.polars.scan_parquet` applies a limit before the predicate, as Polars means
  `head(n).filter(p)`; it prunes row groups only on columns stored as Polars holds them (not
  `timestamp[s]`, which Polars reads in milliseconds).

The core needs no Python dependency; `read_table` and `ParquetFile.read` return pyarrow objects.

## Install

Not on PyPI yet: build it from a checkout of the repository (it needs a Rust toolchain), with
the extras for the libraries you use (`pyarrow`, `pandas`, `polars`, `duckdb`):

```bash
python -m pip install "./python[pandas]"
```

[`INSTALL.md`](../INSTALL.md#python-pyarrow-pandas) has uv, installing from GitHub, wheels and
other platforms.

## Development

```
uv sync --python 3.14
uv run --no-sync maturin develop --uv
uv run --no-sync pytest                    # -m "not slow" skips the acceptance tests
uv run --no-sync ruff check src tests && uv run --no-sync mypy --strict
HYPOTHESIS_PROFILE=mutation TPARQUET_TESTDATA=$PWD/../testdata uv run --no-sync python scripts/mutmut_check.py
TPARQUET_PYTHON=$PWD/.venv/bin/python PYO3_PYTHON=$PWD/.venv/bin/python cargo mutants -p tparquet-python
```
