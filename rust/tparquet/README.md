# tparquet

Parquet for time series: Parquet files whose numeric columns use PCODEC, a compression codec
that runs [pcodec](https://github.com/pcodec/pcodec) inside standard Parquet column chunks.
Trade and tick data comes out at about half its ZSTD size, lossless, with the same schema, row
groups, statistics and page indexes. The codec is specified in
[`spec/FORMAT.md`](https://github.com/aperiodic-io/tparquet/blob/main/spec/FORMAT.md).

This crate has a writer that picks PCODEC or the base codec per column, a reader for any
Parquet file (PCODEC or not), `transcode`, `inspect` and `verify`, and the `tparquet` CLI.

```bash
cargo install tparquet
```

```bash
tparquet encode trades.parquet trades.pcodec.parquet
```

```toml
[dependencies]
tparquet = "0.1"
```

```rust,ignore
use tparquet::{Writer, WriterOptions};

let mut w = Writer::try_new(File::create("trades.parquet")?, schema, WriterOptions::default())?;
w.write(&batch)?;                       // the first batch picks each column's codec
w.close()?;

let reader = tparquet::read::open("trades.parquet")?.build()?;   // Arrow record batches
```

The Parquet implementation is `tparquet-parquet`, the arrow-rs `parquet` crate 60.1.0 with the
codec added, re-exported as `tparquet::parquet`. Use Arrow 60 crates with it. Other crates of
your build that use upstream `parquet` do not get the codec. To give it to all of them,
see [`INSTALL.md`](https://github.com/aperiodic-io/tparquet/blob/main/INSTALL.md#rust).

Python, Polars, DuckDB, Go, Java, Spark, Trino, DataFusion, C and C++ packages are in the
[repository](https://github.com/aperiodic-io/tparquet).
