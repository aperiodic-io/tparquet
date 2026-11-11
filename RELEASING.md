# Releasing

A release publishes, from [`.github/workflows/release.yml`](.github/workflows/release.yml):

| Registry | Package | Built from |
|---|---|---|
| crates.io | `tparquet-core` | `rust/core` |
| crates.io | `tparquet-parquet` | `rust/parquet`, the arrow-rs `parquet` fork ([`FORK.md`](rust/parquet/FORK.md)) |
| crates.io | `tparquet` | `rust/tparquet`: the library and the `tparquet` CLI |
| PyPI | `tparquet` | `python/`: abi3 wheels for CPython 3.10+ on Linux x86_64 and aarch64, macOS arm64 and x86_64, Windows x64, and the sdist |
| PyPI | `polars-tparquet` | `polars-tparquet/` |

The other packages (Go, Java, Spark, Trino, DuckDB, DataFusion, C++) are not published to a
registry; [`INSTALL.md`](INSTALL.md) builds them from a checkout. `tparquet-datafusion` cannot
go to crates.io, as it works by patching the `parquet` crate DataFusion itself uses.

## Once: the secrets

Add two repository secrets (Settings, Secrets and variables, Actions):

- **`CARGO_REGISTRY_TOKEN`**: a crates.io API token (crates.io, Account Settings, API Tokens)
  with the scopes `publish-new` and `publish-update`, limited to the crates `tparquet*`. After
  the first release, `publish-new` is no longer needed.
- **`PYPI_TOKEN`**: a PyPI API token (pypi.org, Account settings, API tokens). For the first
  release it must have the scope "Entire account": the projects do not exist yet, and a token
  limited to a project can only be made once the project exists. After the first release,
  replace it with a token limited to `tparquet` and `polars-tparquet`, or move both projects to
  PyPI's trusted publishing for this workflow and drop the token.

The publishing jobs run in the GitHub environment `release`, which the first run creates. Add
required reviewers to it (Settings, Environments, release) so that a release waits for an
approval before anything is uploaded.

## Every release

1. Pick the version, `X.Y.Z`, and set it in all four places, which must agree:
   `rust/core/Cargo.toml`, `rust/tparquet/Cargo.toml`, `python/Cargo.toml` (maturin takes the
   Python package's version from it) and `polars-tparquet/pyproject.toml`. Update the
   requirements on `tparquet-core` in `rust/parquet/Cargo.toml` and `rust/tparquet/Cargo.toml`
   if its version left their range, then `cargo update --workspace`.
2. `tparquet-parquet` keeps upstream's version (60.1.0). If the fork changed since its last
   release, give it a new version first ([`FORK.md`](rust/parquet/FORK.md) says how), since a
   published version cannot be replaced.
3. Check:

   ```bash
   scripts/check-publish.sh --allow-dirty vX.Y.Z
   ```

   It checks that the versions agree with the tag, that the three crates package and build from
   their packaged sources, and that `tparquet` ships only the library and the CLI. CI runs it on
   every pull request (`rust.yml`, `publish-check`).
4. Merge the version change to `main`.
5. For a release that changes the build (and before the first one), run the workflow by hand:
   Actions, release, Run workflow, on `main`. It builds and checks everything, every wheel on its
   own platform included, and publishes nothing.
6. Tag the commit on `main` and push the tag:

   ```bash
   git tag vX.Y.Z
   ```

   ```bash
   git push origin vX.Y.Z
   ```

   The workflow checks the versions against the tag, builds the wheels, the sdist and the
   Polars package, and then publishes to crates.io and PyPI.

A version that crates.io or PyPI already has is skipped, so a release that failed half-way can
be run again (Actions, the failed run, Re-run failed jobs). A published version can be yanked
(`cargo yank`, PyPI's project settings) but never replaced: fix forward with a new version.

## After the first release

- Add co-owners on crates.io, so that publishing does not depend on one account:
  `cargo owner --add github:aperiodic-io:<team> tparquet` (and for `tparquet-core` and
  `tparquet-parquet`).
- Narrow the tokens as above.
