# tparquet-core

The page payload of PCODEC, the Parquet compression codec of
[tparquet](https://github.com/aperiodic-io/tparquet) (specified in
[`spec/FORMAT.md`](https://github.com/aperiodic-io/tparquet/blob/main/spec/FORMAT.md)): it
compresses and decompresses one Parquet page body, with no Parquet dependency. Decoding treats
the payload as untrusted: no panics, and no allocation the payload has not shown it needs.

For reading and writing Parquet files, use the [`tparquet`](https://crates.io/crates/tparquet)
crate.
