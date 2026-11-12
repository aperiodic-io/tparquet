# Contributing to tparquet

tparquet is open source under the [Apache License 2.0](LICENSE), and contributions of any size
are welcome: an issue, a question, a fix to a typo, a new reader or writer.

## Issues

- A file tparquet barely shrinks, or makes bigger, is the most useful issue you can open.
  Include what `tparquet inspect` prints for the original and the encoded file, what wrote
  the original, and what the columns hold. A sample file helps if you can share one.
- For a bug, give the command or code you ran, what happened, what you expected, and the
  versions (`tparquet --version`, or the package version you installed).
- For a reader or writer you need, say which engine and version, and how you would use it.
- For a change to the format, open an issue before the pull request: every implementation reads
  the same [spec](spec/FORMAT.md) and [vectors](testdata/README.md), so a format change touches
  all of them.

## Pull requests

Every change follows the rules in [`AGENTS.md`](AGENTS.md). In short:

- A failing test comes first: write it, see it fail for the right reason, then make it pass.
  The commit message says which test failed first, or why no test could.
- Every implementation passes the vectors in `testdata/`. A change to the payload format
  changes [`spec/FORMAT.md`](spec/FORMAT.md) and the vectors in the same commit.
- Every package is mutation tested: cargo-mutants for Rust, gremlins for Go, mutmut for Python,
  PIT for Java, and the scripted mutants of the C++ and DuckDB forks. A surviving mutant is a
  missing test or dead code.
- The forks stay small. Every change to a fork is listed in its `FORK.md` and checked against
  the upstream release by the `check-*` scripts in [`scripts/`](scripts).
- Decoders treat their input as untrusted: they never panic and never allocate without a bound.
  The fuzz targets are in [`rust/core/fuzz`](rust/core/fuzz) and [`cpp/fuzz`](cpp/fuzz).

Keep a pull request to one change, and run the checks of what you changed before you push:

```sh
make check            # Rust: fmt, clippy, tests, C smoke test, fork check, vectors
make -C go test       # Go
make python-test      # Python
make polars-test      # the Polars plugin
make -C duckdb test   # DuckDB extension
make java-test        # Java, the parquet-java fork, Spark
make trino-test       # Trino
make datafusion-test  # DataFusion and its parquet fork
make cpp-test         # C++ and the Arrow C++ fork
make acceptance       # cross-language
```

[`INSTALL.md`](INSTALL.md) has the toolchains each one needs. CI runs all of them on every pull
request, so you don't need every toolchain to open one.

## License

By contributing, you agree that your contribution is licensed under the Apache License 2.0, as
section 5 of the license says.
