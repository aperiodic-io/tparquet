# Working on tparquet

## Layout

| Path | What |
|---|---|
| `spec/FORMAT.md` | the file-format specification; the source of truth |
| `spec/UPSTREAM.md` | draft proposal to make PCODEC a standard Parquet codec |
| `docs/` | usage guides (`STORAGE-TIER.md`) |
| `rust/core` | `tparquet-core`: the codec payload (encode, decode), no Parquet dependency |
| `rust/capi` | C ABI over the core (`include/tparquet.h`), linked by Go and the Arrow C++ fork |
| `rust/cread` | C reader: files as Arrow C streams (`include/tparquet_read.h`) |
| `rust/jni` | JNI binding of the core (`java/tparquet-jni`) |
| `rust/parquet` | fork of the arrow-rs `parquet` crate with the PCODEC codec; see `rust/parquet/FORK.md` |
| `rust/tparquet` | high-level Rust API (auto selection, transcode) and the `tparquet` CLI |
| `go/` | Go package: arrow-go codec plugin and writer helpers |
| `python/` | Python package: pyarrow, pandas, Polars, DuckDB |
| `duckdb/` | DuckDB extension: DuckDB's own parquet extension with PCODEC, a patch over DuckDB 1.5.6 and 2.0.0 (`duckdb/FORK.md`) |
| `polars-tparquet/` | Polars plugin package: `scan_tparquet`, `read_tparquet`, `tparquet` namespaces |
| `java/` | Maven: JNI binding, fork of parquet-java 1.17.0 (`java/parquet-java/FORK.md`), `tparquet` library, Spark data source |
| `java/pcodec` | pure Java pcodec 1.0.4 and the payload, byte for byte the Rust reference's (`java/pcodec/VERIFICATION.md`); its own Maven project and Rust oracle |
| `datafusion/` | DataFusion 55.2 connector, its own Cargo workspace on a fork of parquet 59.3.2 (`datafusion/parquet/FORK.md`) |
| `trino/` | Trino 484: fork of trino-parquet (`trino/trino-parquet/FORK.md`), plugin install script; uses `java/parquet-java-1.18` |
| `cpp/` | C++ library and the Arrow C++ 26.0.1 fork, a patch over the release (`cpp/arrow/FORK.md`) |
| `testdata/` | conformance vectors shared by every implementation |
| `acceptance/` | cross-language acceptance tests |

## Rules

- **TDD.** Write the failing test first, run it, see it fail for the right reason, then make it
  pass. Each commit message says which test failed first, or why no test could.
- **Every implementation passes the conformance vectors in `testdata/`.** A change to the payload
  format changes `spec/FORMAT.md` and the vectors in the same commit.
- **Mutation testing.** Rust: `cargo mutants` (config in `.cargo/mutants.toml`); Go: `gremlins`;
  Python: `mutmut`; Java: PIT (`make -C java mutants`, `make -C java/pcodec mutants`); C++: `cpp/mutation` (scripted mutants of
  the Arrow fork) and `duckdb/mutation` (of the DuckDB fork, `make -C duckdb mutation`). A
  surviving mutant is a missing test or dead code: add the test or remove the code. Any
  exclusion carries a comment that says why.
- **The forks stay small.** Every change is listed in the fork's `FORK.md` and checked against
  upstream: `scripts/check-fork.sh` (`rust/parquet`, `datafusion/parquet`), `scripts/check-fork-java.sh`
  (`java/parquet-java`, `java/parquet-java-1.18`, `trino/trino-parquet`), `scripts/check-cpp-fork.sh`
  (`cpp/arrow`), `scripts/check-duckdb-fork.sh` (`duckdb`).
- **Decoders treat payloads as untrusted.** No panics, no unbounded allocation; fuzz targets in
  `rust/core/fuzz`.
- Run the checks of what you changed before pushing (`make check` at the root runs everything).
