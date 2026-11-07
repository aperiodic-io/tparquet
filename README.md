<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="brand/tparquet-logo-dark.svg">
    <img alt="tparquet" src="brand/tparquet-logo.svg" width="340">
  </picture>
</h1>

[![Rust](https://github.com/aperiodic-io/tparquet/actions/workflows/rust.yml/badge.svg)](https://github.com/aperiodic-io/tparquet/actions/workflows/rust.yml)
[![Go](https://github.com/aperiodic-io/tparquet/actions/workflows/go.yml/badge.svg)](https://github.com/aperiodic-io/tparquet/actions/workflows/go.yml)
[![Python](https://github.com/aperiodic-io/tparquet/actions/workflows/python.yml/badge.svg)](https://github.com/aperiodic-io/tparquet/actions/workflows/python.yml)
[![DuckDB](https://github.com/aperiodic-io/tparquet/actions/workflows/duckdb.yml/badge.svg)](https://github.com/aperiodic-io/tparquet/actions/workflows/duckdb.yml)
[![Java](https://github.com/aperiodic-io/tparquet/actions/workflows/java.yml/badge.svg)](https://github.com/aperiodic-io/tparquet/actions/workflows/java.yml)
[![Trino](https://github.com/aperiodic-io/tparquet/actions/workflows/trino.yml/badge.svg)](https://github.com/aperiodic-io/tparquet/actions/workflows/trino.yml)
[![C++](https://github.com/aperiodic-io/tparquet/actions/workflows/cpp.yml/badge.svg)](https://github.com/aperiodic-io/tparquet/actions/workflows/cpp.yml)
[![Cross-language](https://github.com/aperiodic-io/tparquet/actions/workflows/acceptance.yml/badge.svg)](https://github.com/aperiodic-io/tparquet/actions/workflows/acceptance.yml)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

**Parquet for time series.** Trade and tick files at about half their ZSTD size (0.49× on
139M trades from four market data providers), lossless, and still Parquet: same schema, row
groups, statistics and page indexes. Read and write them from Python, pandas, Polars, DuckDB,
Go, Rust, Java, Spark, Trino, DataFusion, C and C++.

tparquet adds one thing to Parquet: a compression codec, PCODEC, that runs
[pcodec](https://github.com/pcodec/pcodec) on numeric columns. Prices and sizes that are short
decimals are stored as exact integers. Everything else in the file stays standard Parquet.

## How much smaller

Measured on the sample and preview data of four market data providers: Aperiodic, Tardis.dev,
Crypto Lake and NYSE, 151M rows in 31 datasets. The baseline is Parquet with ZSTD level 9,
written two ways: with the writer's defaults, and *tuned*, with the best encoding chosen per
column (DELTA_BINARY_PACKED, BYTE_STREAM_SPLIT, DELTA_BYTE_ARRAY, ...), as the providers' own
files are.

| Data | Rows | ZSTD(9) | ZSTD(9) tuned | tparquet | vs ZSTD(9) | vs tuned |
|---|---:|---:|---:|---:|---:|---:|
| Trades (crypto perps and spot, US equities) | 138.9M | 737.0 MB | 520.0 MB | 362.2 MB | 0.49× | 0.70× |
| Quotes (top of book) | 6.8M | 80.6 MB | 63.3 MB | 51.0 MB | 0.63× | 0.81× |
| Order books (L2 snapshots and updates) | 5.5M | 126.4 MB | 109.2 MB | 58.6 MB | 0.46× | 0.54× |
| Bars, metrics, derivatives | 288K | 13.1 MB | 8.2 MB | 6.8 MB | 0.52× | 0.83× |
| **All** | 151.5M | 957.1 MB | 700.7 MB | 478.6 MB | **0.50×** | **0.68×** |

Every value comes back bit for bit, and tparquet is smaller on every one of the 31 datasets.
It chooses PCODEC or ZSTD per column, on the first batch: here 478 of the 480 numeric columns
got PCODEC. Strings and flags keep ZSTD.

If your files compress like these trades, 1 TB of ZSTD(9) Parquet becomes about 490 GB (about
700 GB if your writer already tunes its encodings), and a full scan reads half the bytes. On
one thread, tparquet writes at 109 MB/s of Arrow data (ZSTD(9): 104, tuned: 155) and reads at
925 MB/s (ZSTD(9): 1,031, tuned: 1,225). On wide order books, where every column is PCODEC, it
writes about 3× slower and reads at two thirds of ZSTD(9)'s speed.

Every dataset, with its write and read speeds, is in [`bench/RESULTS.md`](bench/RESULTS.md).
`make bench` reproduces it (it downloads about 1.2 GB). The quickest way to find out what
tparquet does for your data is still to run it on one of your own files.

## Try it on your own file

```sh
cargo install --path rust/tparquet                          # the tparquet CLI

tparquet encode trades.parquet trades.pcodec.parquet        # standard -> PCODEC, codec chosen per column
tparquet inspect trades.pcodec.parquet                      # codec and size of every column
tparquet verify trades.parquet trades.pcodec.parquet        # same data, floats bit for bit
tparquet decode trades.pcodec.parquet trades.standard.parquet   # and back to standard Parquet
```

Nothing about your original file changes, and `decode` takes you back whenever you want.

## It's still Parquet

PCODEC is set per column chunk in `ColumnMetaData.codec`, exactly like `ZSTD` or `SNAPPY`.
Schema, row groups, encodings, statistics, page indexes, bloom filters and key-value metadata
are untouched, so your files keep their layout, their partitioning and their statistics.
Strings, booleans and any column pcodec doesn't help keep ZSTD.

The trade-off is that a reader without the codec cannot read the file. It fails with an error
and never returns wrong data: the acceptance tests check this for pyarrow, pandas, Polars,
DuckDB's `read_parquet`, the upstream arrow-rs `parquet` crate, parquet-java and Trino. When a file has to go to such
a reader, `tparquet decode` (or `transcode(..., target="standard")`) writes a standard copy
and `tparquet verify` confirms it holds the same values.

The codec is specified in [`spec/FORMAT.md`](spec/FORMAT.md), so anyone can implement it.

## Where it fits: a storage tier

Until PCODEC is a standard Parquet codec, use tparquet where you control every reader, and
write standard Parquet wherever data leaves: keep the history, the archive or your own copy
of vendor data in tparquet, and `tparquet decode` (or `transcode`) at the edge for customers,
other teams, managed warehouses and BI tools. [`docs/STORAGE-TIER.md`](docs/STORAGE-TIER.md)
has the recipes, and the cases where it does not pay.

Making PCODEC a standard codec, so that any Parquet reader can open these files, is the
proposal in [`spec/UPSTREAM.md`](spec/UPSTREAM.md): what would change in `parquet.thrift`,
the evidence, and the work still needed before it goes to the Apache Parquet community.

## Install

The packages are not on PyPI or crates.io yet. Build them from a checkout of this repository
(the Python and Go packages need a Rust toolchain):

```sh
cargo install --path rust/tparquet           # CLI
pip install ./python ./polars-tparquet       # Python, pandas, Polars, DuckDB (Python 3.10+)
make -C duckdb build                         # DuckDB extension: duckdb/build/tparquet.duckdb_extension
cargo build --release -p tparquet-capi       # static library for Go; see go/README.md
make -C java install                         # Java, Spark: the parquet-java fork; see java/README.md
make -C java spark-jars                      # the jars a Spark 4.3 distribution needs
make -C trino install TRINO_HOME=...         # Trino 484: the forked jars into its connectors
make -C cpp build                            # C++: the Arrow C++ fork and tparquet::*; see cpp/README.md
```

## Use it

Python, with pyarrow, pandas, Polars or DuckDB:

```python
import tparquet as tpq

tpq.write_table(table, "trades.parquet")          # {"price": "PCODEC", "sym": "ZSTD", ...}
table = tpq.read_table("trades.parquet", columns=["ts", "price"])
```

Polars, lazily over many files:

```python
import polars as pl
import polars_tparquet as pt

lf = pt.scan_tparquet("trades/**/*.parquet")      # projection and predicate pushdown into every file
lf.filter(pl.col("qty") > 0).tparquet.sink("out.parquet")
```

DuckDB:

```sql
LOAD 'tparquet.duckdb_extension';
SELECT symbol, avg(price) FROM read_tparquet('trades/*.parquet') GROUP BY ALL;
COPY (SELECT ts, price, qty FROM ticks) TO 'ticks.parquet' (FORMAT tparquet, COLUMN_NAMES ['ts', 'price', 'qty']);
```

Go, on arrow-go with no fork:

```go
import tparquet "github.com/aperiodic-io/tparquet/go" // reading: the import is enough

w, err := tparquet.NewFileWriter(schema, out) // codec chosen per column, base codec ZSTD(3)
```

Rust:

```rust
use tparquet::{Writer, WriterOptions};

let mut w = Writer::try_new(File::create("trades.parquet")?, schema, WriterOptions::default())?;
w.write(&batch)?;                                  // the first batch picks each column's codec
w.close()?;

let reader = tparquet::read::open("trades.parquet")?.build()?;   // Arrow record batches
```

```java
// with the parquet-java fork, any parquet-java reader (and Spark) reads PCODEC files
try (ParquetWriter<Group> w = TparquetWriter.groups(path, schema, TparquetWriter.Options.defaults())) { ... }
```

```scala
df.write.format("tparquet").save("trades")       // Spark: PCODEC, V2 pages, PLAIN numbers
spark.read.parquet("trades")
```

```sql
-- Trino, after trino/install-tparquet.sh: a Hive table of PCODEC files
SELECT venue, avg(price) FROM hive.market.trades GROUP BY venue;
```

```cpp
ARROW_ASSIGN_OR_RAISE(auto table, tparquet::ReadTable("trades.parquet", {"ts", "price"}));
```

Each package's README has the details: [Python](python/README.md),
[Polars](polars-tparquet/README.md), [Go](go/README.md), [DuckDB](duckdb/README.md),
[Java and Spark](java/README.md), [Trino](trino/README.md), [DataFusion](datafusion/README.md),
[C and C++](cpp/README.md).

## tparquet or Vortex?

[Vortex](https://github.com/vortex-data/vortex) is a new columnar file format built for fast
scans and random access on compressed data. tparquet is a smaller change: your files stay
Parquet and the numeric columns get smaller.

Use tparquet when:

- your data is mostly numeric time series (timestamps, prices, sizes, ids, sensor readings) and
  storage or transfer cost is what you are trying to cut;
- your pipelines, catalogs and partition layouts are built around Parquet and you don't want to
  migrate them to a new format;
- you write from Go, or from Python, Polars, DuckDB or Rust;
- you want a one-command way back to plain Parquet.

Use Vortex when scan speed and point lookups matter more than bytes on disk, when your tables
are mostly strings, or when you need its Spark, Trino or Java integrations. Its default
compressor [leaves out](https://vortex.dev/blog/btrblocks-compressor) heavyweight codecs such as
pcodec to keep random access. A tparquet page, like any compressed Parquet page, is decoded
whole.

Use plain ZSTD Parquet when the files must be read by tools you don't control.

## Why you can trust it

- The format is a short, versioned spec: [`spec/FORMAT.md`](spec/FORMAT.md).
- Every implementation reads the same conformance vectors in [`testdata/`](testdata/README.md):
  Parquet files with their expected rows in canonical JSON, and page payloads with their decoded
  bytes.
- Rust, Go, Python, DuckDB, Java, C++ and DataFusion each read the files all of them wrote,
  including Python, DuckDB and the Rust CLI under each policy and in many row groups
  ([`acceptance/cross/run.sh`](acceptance/cross/run.sh)), on every pull request; Spark and
  Trino read every vector exactly as they read a standard copy of the same rows.
- Every package is mutation tested: cargo-mutants for Rust (including the forks' changed
  functions), gremlins for Go, mutmut for Python, PIT for Java, and a scripted mutation check of
  the Arrow C++ fork. A surviving mutant is a missing test.
- Decoders treat payloads as untrusted (no panics, no unbounded allocation) and are fuzzed
  ([`rust/core/fuzz`](rust/core/fuzz), [`cpp/fuzz`](cpp/fuzz)).
- The forks are small: arrow-rs `parquet` 60.1 and 59.3 ([`FORK.md`](rust/parquet/FORK.md),
  [`FORK.md`](datafusion/parquet/FORK.md)), parquet-java
  1.17 and 1.18 ([`FORK.md`](java/parquet-java/FORK.md)), trino-parquet
  ([`FORK.md`](trino/trino-parquet/FORK.md)) and Arrow C++ ([`FORK.md`](cpp/arrow/FORK.md)).
  Every change is listed and checked against the verified upstream release in CI; upstream
  parquet-java's own suite gives its fork the same outcome for every test.

## Packages

| Where | Package | Notes |
|---|---|---|
| Rust | [`rust/tparquet`](rust/tparquet) | writer with automatic codec choice, `transcode`, `inspect`, `verify`, and the `tparquet` CLI |
| Rust, anything on arrow-rs 60 | [`rust/parquet`](rust/parquet) | a small fork of the arrow-rs `parquet` crate 60.1.0 that adds the codec; changes listed in [`FORK.md`](rust/parquet/FORK.md). Use it with `[patch.crates-io] parquet = { path = ... }` |
| Rust, no Parquet | [`rust/core`](rust/core) | the page payload codec alone |
| C | [`rust/capi`](rust/capi) | C ABI over the core ([`tparquet.h`](rust/capi/include/tparquet.h)) |
| Go | [`go/`](go) | arrow-go codec plugin (no fork: `compress.RegisterCodec`) and writer helpers |
| Python, pandas, Polars | [`python/`](python) | pyo3 package: `read_table`, `write_table`, `Writer`; `tparquet.pandas`, `tparquet.polars` (`scan_parquet` with pushdown), `tparquet.duckdb` |
| DuckDB | [`duckdb/`](duckdb) | loadable extension: `read_tparquet(...)`, `COPY ... (FORMAT tparquet)` |
| Java, anything on parquet-java | [`java/`](java) | a small fork of parquet-java 1.17.0 (`parquet-format-structures`, `parquet-common`, `parquet-hadoop`; [`FORK.md`](java/parquet-java/FORK.md)), the codec over JNI ([`tparquet-jni`](java/tparquet-jni), [`rust/jni`](rust/jni)), and [`tparquet`](java/tparquet): `TparquetWriter`, `TparquetReader`, canonical rows |
| Spark | [`java/tparquet-spark`](java/tparquet-spark) | Spark 4.3 reads PCODEC through the parquet-java fork (every reader); the `tparquet` data source writes it ([Java README](java/README.md#spark)) |
| DataFusion | [`datafusion/`](datafusion) | DataFusion 55.2 (arrow-rs 59) on a fork of the `parquet` crate 59.3.2 ([`FORK.md`](datafusion/parquet/FORK.md)): `STORED AS TPARQUET`, `COPY`/`INSERT` writing PCODEC; `STORED AS PARQUET` and `read_parquet` read PCODEC files, with pruning |
| Trino | [`trino/`](trino) | Trino 484 reads tparquet through its connectors that use trino-parquet (Hive tested end to end; the script also installs into Iceberg, Delta Lake and Hudi): [`install-tparquet.sh`](trino/install-tparquet.sh) puts forks of trino-parquet 484 ([`FORK.md`](trino/trino-parquet/FORK.md)) and parquet-java 1.18.1 ([`FORK.md`](java/parquet-java-1.18/FORK.md)) into a Trino install |
| C++ | [`cpp/`](cpp) | a small fork of Apache Arrow C++ 26.0.1 (a patch over the release, [`FORK.md`](cpp/arrow/FORK.md)) and `tparquet::ReadTable`/`WriteTable`/`FileWriter`; CMake package `tparquet` |
| C | [`rust/cread`](rust/cread) | reader as an Arrow C stream ([`tparquet_read.h`](rust/cread/include/tparquet_read.h)), `libtparquet_read` |
| Polars | [`polars-tparquet/`](polars-tparquet) | Polars plugin `polars-tparquet`: `scan_tparquet` over files, lists, globs and directories (lazy, with pushdown), `read_tparquet`, `df.tparquet.write(...)`, `lf.tparquet.sink(...)` (streamed) |

## Development

```sh
make check            # Rust: fmt, clippy, tests, C smoke test, fork check, vectors
make -C go test       # Go
make python-test      # Python (uv, Python 3.14)
make polars-test      # the Polars plugin
make -C duckdb sql    # DuckDB extension
make java-test        # Java, the parquet-java fork, Spark (JDK 21, Maven)
make trino-test       # Trino: the trino-parquet fork, conformance, queries through the Hive connector
make datafusion-test  # DataFusion and its parquet 59.3.2 fork
make cpp-test         # C++ and the Arrow C++ fork (asan, fuzz-smoke, mutants targets in cpp/)
make acceptance       # cross-language (needs the Python package, the DuckDB extension, the Java
                      # packages and the C++ library built)
make bench            # the benchmark on providers' sample data (downloads about 1.2 GB)
```

CI runs all of it on every pull request (`.github/workflows/`). See [`AGENTS.md`](AGENTS.md) for
the rules of the repository.

The acceptance test also measures deterministic synthetic market data, against ZSTD(9): 50,000
trades at 0.31×, and 5,000 L2 book snapshots stored as lists, where pcodec does not win, at
1.07× (`cargo test --release -p tparquet --test acceptance -- --nocapture`).

## License

Apache License 2.0 ([`LICENSE`](LICENSE)). `rust/parquet` is a modified copy of the Apache Arrow
`parquet` crate, `java/parquet-java` and `java/parquet-java-1.18` of Apache Parquet Java modules,
`trino/trino-parquet` of Trino's trino-parquet, and `cpp/arrow` a patch to Apache Arrow C++; see
[`NOTICE`](NOTICE).
