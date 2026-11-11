# polars-tparquet

A [Polars](https://pola.rs) plugin for [tparquet](../README.md), Parquet for time series: scan,
read, write and sink Parquet files with the PCODEC codec, which Polars' own Parquet reader does
not know.

```python
import polars as pl
import polars_tparquet as pt

# lazy, over one file, a list, a glob or a directory
lf = pt.scan_tparquet("trades/**/*.parquet", include_file_paths="file")
daily = (
    lf.filter(pl.col("ts") >= 1_735_689_600_000)
    .group_by(pl.col("ts") // 86_400_000)
    .agg(pl.col("price").mean())
    .collect()
)

df = pt.read_tparquet("trades.parquet", columns=["ts", "price"], n_rows=1_000)

df.tparquet.write("out.parquet")                               # {"ts": "PCODEC", "price": "PCODEC", ...}
lf.filter(pl.col("qty") > 0).tparquet.sink("filtered.parquet")  # streamed, never all in memory
```

Install it from a checkout of the repository together with the `tparquet` package, which is not
on PyPI yet (it needs a Rust toolchain; [`INSTALL.md`](../INSTALL.md#polars) has the details):

```bash
python -m pip install ./python ./polars-tparquet
```

Importing `polars_tparquet` registers the `tparquet` namespace on `DataFrame` and `LazyFrame`.

## Reading

`scan_tparquet(source, *, batch_size=None, include_file_paths=None) -> LazyFrame`

- `source` is a path (`str`, `bytes` or `os.PathLike`), a binary file object, a glob, a
  directory, or a list of these, in any mix. A leading `~` is the home directory.
  - A path that exists is that file or directory, whatever characters its name holds
    (`trades[1].parquet`).
  - Otherwise a path holding `*`, `?` or `[...]` is a glob (`**` also matches directories): the
    files it matches, in sorted order; directories it matches are skipped.
  - A directory is every `.parquet` file below it (any case of the suffix), in sorted order.
    Hidden files and directories (names starting with `.`, such as macOS `._` files or another
    writer's unfinished file) are skipped, as a glob skips them; symlinks to files are read,
    symlinks to directories are not followed.
  - A file object is read to its end once, when scanned (from where it stands; a pipe works);
    it may be listed more than once. Relative paths are resolved when scanned, so a later
    `chdir` does not change what is read.
  - A file named twice (or matched by two globs) is read twice. A list is read in the order
    given. All files must have the same schema; a file that differs raises `SchemaError`
    naming it, and a file whose footer cannot be read raises the tparquet error with its name
    in front.
- Each file is a Polars IO source. The query's projection, row limit and predicate reach the
  reader, and the predicate also skips row groups whose integer, date and datetime statistics
  rule it out. Nothing is read until the query runs; the schema comes from the footers.
- `include_file_paths="file"` adds a String column holding each row's file path, as given (or as
  the glob or directory gave it; a byte of the name that is not UTF-8 shows as U+FFFD).
- Files without PCODEC (any standard Parquet file) are read too.

`read_tparquet(source, *, columns=None, n_rows=None, batch_size=None, include_file_paths=None)`
is `scan_tparquet(...)` with the columns (names, or indices as `pl.read_parquet` takes them, in the
order given) and row limit applied, collected.

## Writing

`df.tparquet.write(target, *, policy="auto", columns=None, level=8, base="zstd(3)", row_group_size=None)`
writes a DataFrame and returns the codec of every leaf column. Like `sink` below, it writes a
path through a temporary file, so a write that fails leaves the target as it was. A column of
dtype `Object` is refused (`ComputeError`), as Polars' own writer refuses it.

`lf.tparquet.sink(target, *, ..., chunk_size=None)` runs a lazy query and writes its result
batch by batch (`LazyFrame.collect_batches`), so a result larger than memory can be written. A
path is written as a hidden temporary file beside it (`.out.parquet.<random>.tmp`), which replaces
it once complete: nothing sees a half-written file, a query that fails (or is interrupted) leaves
the target as it was, and a query may write over a file it reads. The file replaced keeps its
permission bits, and one the user may not write is refused (`PermissionError`), as writing it in
place would be; a target that is a symlink keeps linking to the file it names, which is replaced.
A file object is written in place.

The options: with `policy="auto"` each INT32/INT64/FLOAT/DOUBLE column gets whichever of PCODEC
and the base codec is smaller on the first batch with rows; `"pcodec"` and `"base"` force one.
`columns` maps a column name or dotted leaf path to its own policy, `level` is the pcodec level
(0 to 12), `base` the codec of the other columns (`"zstd(3)"`, `"snappy"`, `"uncompressed"`...),
and `row_group_size` the most rows per row group.

Known bug (tparquet's Polars reader, to be fixed there): over a single file, a row limit before
a filter, such as `scan_tparquet("one.parquet").head(n).filter(...)`, gives wrong rows: the limit
is applied after the filter. `filter(...).head(n)`, and scans of several files, are right.

Not supported: `Int128`/`UInt128` (no Parquet Arrow type) and, for now, writing a `Null`
column or to a path that is not UTF-8 (tparquet's Arrow import and path handling; a `Null`
column Polars wrote is read). A scan reads the footer when scanned and the pages when the query
runs: replace a file and a scan made before reads it no more (an error); scan it again.

## How it works

The package is pure Python. Decoding and encoding are tparquet's Rust core (the `tparquet`
package, a dependency), and batches reach Polars through the Arrow C stream interface, without
pyarrow. Polars has no loadable extensions; this plugin uses its public plugin APIs: IO sources
(`register_io_source`) for reading and namespaces for writing.

## Development

```
uv sync --python 3.14 --reinstall-package tparquet   # builds tparquet from ../python (needs Rust)
uv run --no-sync pytest
uv run --no-sync ruff check src tests scripts && uv run --no-sync mypy
HYPOTHESIS_PROFILE=mutation TPARQUET_TESTDATA=$PWD/../testdata uv run --no-sync python scripts/mutmut_check.py
```

The conformance tests read every vector of `testdata/` through the plugin and compare it with
what Polars' own reader reads from a standard (non-PCODEC) copy of the same file, and with the
expected rows of `testdata/expected/`, floats bit for bit. `HYPOTHESIS_PROFILE=thorough` runs the
property tests with 400 examples each (about a minute and a half).
