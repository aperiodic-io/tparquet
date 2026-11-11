# `tparquet-datafusion`: PCODEC Parquet files in Apache DataFusion

[DataFusion](https://datafusion.apache.org) reads Parquet with the arrow-rs `parquet` crate,
which does not know `CompressionCodec` 20547 (`PCODEC`, [`spec/FORMAT.md`](../spec/FORMAT.md)):
a PCODEC file fails at the first page. This directory makes DataFusion read and write them:

- [`parquet/`](parquet) is the `parquet` crate **59.3.2** with the PCODEC codec, the same
  change as the 60.1.0 fork in [`rust/parquet`](../rust/parquet), ported to the 59.x line
  because DataFusion 55.2.0 (the newest release) is built on `parquet ^59.2.0` and arrow 59.
  What it changes is listed in [`parquet/FORK.md`](parquet/FORK.md) and checked against
  upstream by [`scripts/check-fork.sh`](../scripts/check-fork.sh).
- `tparquet-datafusion` (this crate) adds the `TPARQUET` format: `CREATE EXTERNAL TABLE ...
  STORED AS TPARQUET` and `COPY ... TO ... STORED AS TPARQUET` / `INSERT INTO`, which writes
  PCODEC for the numeric columns.

It is its own Cargo workspace (with its own `Cargo.lock`), so that DataFusion's dependency
tree, on arrow 59, stays out of the root workspace, on arrow 60.

## Use

[`INSTALL.md`](../INSTALL.md#datafusion) has the git and path forms of these dependencies.

```toml
[dependencies]
datafusion = "=55.2.0"
tparquet-datafusion = { path = "datafusion" }      # this directory

[patch.crates-io]
parquet = { path = "datafusion/parquet" }          # the fork: every crate gets PCODEC
```

The `[patch.crates-io]` line is what makes DataFusion read PCODEC: with it, plain
`STORED AS PARQUET`, `ctx.read_parquet`, `ctx.register_parquet` and `SELECT * FROM
'file.parquet'` read PCODEC files, standard ones as before, with nothing else to do. The fork
enables the codec by default (its `pcodec` feature).

```rust
use datafusion::prelude::*;

let ctx = SessionContext::new();
tparquet_datafusion::register_tparquet(&ctx)?;

// write: PCODEC for INT32/INT64/FLOAT/DOUBLE columns, ZSTD (DataFusion's `compression`) for the rest
ctx.sql("COPY (SELECT * FROM 'trades.parquet') TO 'trades.pcodec.parquet' STORED AS TPARQUET").await?.collect().await?;
ctx.sql("COPY trades TO 'by_day/' STORED AS TPARQUET PARTITIONED BY (day) OPTIONS (policy 'auto', pcodec_level 10)")
    .await?.collect().await?;

// read: like PARQUET (pruning, page index, bloom filters, projection, filter pushdown, LIMIT)
ctx.sql("CREATE EXTERNAL TABLE trades STORED AS TPARQUET LOCATION 'by_day/'").await?;
ctx.sql("SELECT day, sum(qty) FROM trades WHERE price > 100 GROUP BY day").await?.show().await?;
```

```rust
use tparquet_datafusion::{DataFrameExt, Policy, SessionContextExt, TparquetOptions};

let df = ctx.read_tparquet("trades.pcodec.parquet", ParquetReadOptions::default()).await?;
df.write_tparquet("copy.parquet", TparquetOptions::default().with_column("id", Policy::Base), None).await?;
```

### What `register_tparquet` adds

| | |
|---|---|
| `TparquetFormatFactory` | the file format `tparquet` (for `COPY ... STORED AS TPARQUET` and `INSERT INTO`), `register_file_format` |
| table factory `TPARQUET` | `CREATE EXTERNAL TABLE ... STORED AS TPARQUET`, a `ListingTableFactory`: files, directories (`*.parquet`), globs, hive partitions |
| `TparquetFormat` | reads as DataFusion's `ParquetFormat` (it delegates everything); writes with `TparquetSink` |

`register_tparquet_state(&mut SessionState)` does the same for a state built with
`SessionStateBuilder`.

### Writing options

`COPY ... OPTIONS (...)` and `CREATE EXTERNAL TABLE ... OPTIONS (...)` take every option of
DataFusion's Parquet format (`compression`, `max_row_group_size`, `statistics_enabled`,
`bloom_filter_on_write`, `data_page_row_count_limit`, `writer_version`,
`'compression::col'`, `'metadata::key'`, ...; and the session's
`datafusion.execution.parquet.*` settings), plus:

| option | default | |
|---|---|---|
| `policy` | `'pcodec'` | `'pcodec'`, `'auto'` or `'base'`, for every `INT32`/`INT64`/`FLOAT`/`DOUBLE` leaf column |
| `'policy::<column>'` | | the policy of one column: a top-level name or a dotted leaf path (`'policy::book.levels'`); the longest name that covers a leaf wins |
| `pcodec_level` | `8` | pcodec compression level, 0 to 12 |

- `pcodec`: PCODEC with `PLAIN` encoding, no dictionary, and the type hint (spec section 3,
  writer recommendations). A file with a PCODEC column is written with `DATA_PAGE_V2` pages
  (the page version is per file in arrow-rs).
- `auto`: spec section 6. On the first batch with rows of each file, each eligible column
  is written with the base codec and with PCODEC (both with V2 pages) and keeps the smaller;
  when only `auto` columns would get PCODEC and the base codec with DataFusion's own page
  version is smaller in total, the file keeps the base codec everywhere (V2 pages store
  levels uncompressed, which can cost the other columns more than PCODEC saves).
- `base`: the base codec, DataFusion's `compression` option (`zstd(3)` by default).

Other columns always get the base codec. DataFusion parses its `compression` option itself
and refuses `pcodec`: PCODEC is chosen with `policy`, not with `compression`. As for DataFusion's
own `'compression::col'` options, SQL lowercases the option keys, so `'policy::Price'` names a
column `price`; a policy for a column the output does not have is an error. Files with
encryption options are refused (the sink does not write encrypted files).

The sink writes each file with the fork's `AsyncArrowWriter` (DataFusion's
`allow_single_file_parallelism` does not apply), records sorting columns for an ordered input
as DataFusion's Parquet sink does, and reports `rows_written` and `bytes_written` metrics.

## What the fork changes

See [`parquet/FORK.md`](parquet/FORK.md): the codec id `PCODEC = 20547`,
`Compression::PCODEC(PcodecLevel)`, `PcodecCodec` over [`tparquet-core`](../rust/core) (by
path), the column's physical type as the type hint, and the `parquet.pcodec.version = "1"`
footer key; 7 files, the same code as `rust/parquet/FORK.patch`. Every PCODEC payload is
untrusted: a corrupt payload is a `ParquetError`, which DataFusion reports as an error.

## Testing

```sh
make -C datafusion check          # fmt-check, clippy, test, fork-test, vectors, check-fork
make -C datafusion test           # the connector: unit tests and tests/it
make -C datafusion fork-test      # the fork's own tests
make -C datafusion mutants        # cargo mutants over the connector (.cargo/mutants.toml)
make -C datafusion fork-mutants   # cargo mutants over the fork's changed functions
cargo run -p tparquet-datafusion --bin tparquet-datafusion-vectors -- check ../testdata
```

The integration tests (`tests/it`, one binary):

- `conformance`: every `testdata/files/*.parquet` through `STORED AS TPARQUET`, `STORED AS
  PARQUET` (without registering anything), `read_parquet`, `register_parquet`, the
  `SessionContextExt` methods and `SELECT * FROM 'file'` gives exactly
  `testdata/expected/<name>.jsonl`; every payload vector of `testdata/payloads/` decodes (or
  fails) as listed through DataFusion, from a Parquet page that carries it; the canonical
  JSON port (`src/canonical.rs`, from `rust/tparquet`) and the page-header reader of the
  tests (`tests/it/common/inspect.rs`) are checked against the vectors and their manifest.
- `write`: `COPY ... STORED AS TPARQUET` per policy, options, levels, nested columns, the
  cost of V2 pages, empty tables, every vector round-tripped, `PARTITIONED BY`, `INSERT
  INTO`, `write_tparquet`, sorting columns, metrics; the codecs, pages, encodings and payload
  modes of every file written are checked with the fork's metadata API and the page headers.
- `sql`: the same queries (projections, filters, aggregates, windows, `LIMIT`, joins with
  standard tables) over a standard file and its TPARQUET copy; row-group pruning by
  statistics, page-index pruning and bloom filters (from the scan metrics), projection and
  limit pushdown, directories, globs and hive partitions.
- `corrupt`: corrupt payloads of every vector are DataFusion errors; random bytes in a page
  and shifted pages never panic (proptest).
- `properties`: random tables round-trip bit for bit with every policy, level, row group
  size and page version (proptest).
- `vectors_cli`: the acceptance binary below.
- `api`: the factory's options, the `FileFormat` methods called directly, the sink of a
  `COPY` plan and the files it reports, encryption options refused.

### Mutation testing

`make mutants` runs cargo-mutants over the connector, `make fork-mutants`
([`scripts/fork-mutants.sh`](scripts/fork-mutants.sh)) over the fork's changed functions.
Both test the mutants **in place** (`--in-place`, one at a time, each reverted after its
test; do not edit the tree meanwhile): cargo-mutants copies the workspace, this directory, to
a scratch tree, where the fork's path dependency `../../rust/core` does not resolve. The
exclusions are in [`.cargo/mutants.toml`](.cargo/mutants.toml), each with its reason.
`fork-mutants.sh` spells out parquet 59.x's `experimental!(mod compression)` for the run
(cargo-mutants does not expand macros, and would not see `compression.rs` otherwise) and
restores `lib.rs` afterwards.

Results: the connector, 143 mutants: 123 caught, 20 unviable, none missed; the fork, 62
mutants: 53 caught, 7 unviable, none missed, 2 timeouts (a mutant of `CodecOptionsBuilder::build`
that recurses forever, see [`parquet/FORK.md`](parquet/FORK.md)).

## Cross-language acceptance

`tparquet-datafusion-vectors` is DataFusion's reader and writer for
[`acceptance/cross/run.sh`](../acceptance/cross/run.sh):

```text
tparquet-datafusion-vectors check DIR          # reads DIR/files/*.parquet, compares with DIR/expected/<name>.jsonl
tparquet-datafusion-vectors write OUT SRC...   # rewrites every SRC vector with COPY ... STORED AS TPARQUET
```

`check` prints `<n> vectors match`, or names the first differing line
(`tparquet-datafusion-vectors: temporal: line 2 differs from expected/temporal.jsonl`) and
exits 1, as `gen-testdata --check` and `acceptance/cross/vectors.py` do; usage errors exit 2.
