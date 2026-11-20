# Fork of the arrow-rs `parquet` crate, 59.x, for DataFusion

This directory is the `parquet` crate as published on crates.io, with the PCODEC codec of
`spec/FORMAT.md` added. It is the same change as the 60.1.0 fork in `rust/parquet` (see its
`FORK.md`), ported to the 59.x line because DataFusion 55.2.0, the newest release, depends on
`parquet ^59.2.0` (and arrow 59): `^59.2.0` resolves to 59.3.2, the last 59.x release. The
crate name stays `parquet`; the `datafusion/` workspace points every `parquet` dependency
here with `[patch.crates-io]`.

| | |
|---|---|
| upstream | `parquet` 59.3.2 |
| source | `https://static.crates.io/crates/parquet/parquet-59.3.2.crate` |
| sha256 | `8eb260d566f2fbdabe356d26aaa5d13403aac29b5b4e44463b8f9bd7b2b12680` |

The crate is vendored as published (its normalized `Cargo.toml`), without `Cargo.toml.orig`.
`FORK.md` and `FORK.patch` are not part of upstream.

On crates.io it is `tparquet-parquet` 59.3.2, beside the 60.1.0 of `rust/parquet`. The tree keeps
the name `parquet`, which the `[patch]` needs; `scripts/package-parquet-59.sh` makes the
published package from a copy of it, changing only the package name, description and
repository, and taking `tparquet-core` from crates.io (`RELEASING.md`).

`FORK.patch` is the complete diff from upstream to this tree. `scripts/check-fork.sh`
downloads the upstream crate, checks the sha256 above, and fails unless the diff of the
git-tracked files equals `FORK.patch` (`scripts/check-fork.sh datafusion/parquet` checks this
fork alone); `scripts/check-fork.sh --update` rewrites it after an intended change, which must
also be listed below.

## Changes

| File | Item | Change | Why |
|---|---|---|---|
| `Cargo.toml` | `[features]` | new feature `pcodec = ["dep:tparquet-core"]`, in `default` | the codec, default on like the other codecs |
| `Cargo.toml` | `[dependencies]`, `[dev-dependencies]` | `tparquet-core = { path = "../../rust/core" }` (optional; and for tests) | the payload encoder and decoder; unit tests build every codec, as upstream does with `cfg(any(feature, test))` |
| `src/basic.rs` | `CompressionCodec` (thrift enum) | `PCODEC = 20547` | the codec id of spec section 2 |
| `src/basic.rs` | `Compression` | variant `PCODEC(PcodecLevel)` | the writer-side codec choice |
| `src/basic.rs` | `From<CompressionCodec> for Compression`, `From<Compression> for CompressionCodec` | `PCODEC` arms | conversion to and from the footer |
| `src/basic.rs` | `FromStr for Compression` | `PCODEC`/`pcodec`, optionally with a level, `PCODEC(level)` | string parsing as for the other codecs; without a level it is the default 8 |
| `src/basic.rs` | `pub use crate::compression::…` | adds `PcodecLevel` | public like `ZstdLevel` (the `compression` module is private unless `experimental`) |
| `src/compression.rs` | `CodecOptions`, `CodecOptionsBuilder` (struct, `Default`, `build`) | field `physical_type: Option<Type>`, setter `CodecOptionsBuilder::set_physical_type` | the PCODEC type hint; other codecs ignore it |
| `src/compression.rs` | `create_codec` | `PCODEC` arm | creates `PcodecCodec` |
| `src/compression.rs` | new `mod pcodec_codec`: `PcodecCodec::{new, compress, decompress}` | the codec over `tparquet_core`; `decompress` without `uncompress_size` is an error | PCODEC needs the expected size (spec section 3) |
| `src/compression.rs` | new `PcodecLevel` (`CompressionLevel`, `try_new`, `compression_level`, `Default`) | levels 0..=12, default 8 | as `ZstdLevel` |
| `src/column/writer/mod.rs` | `GenericColumnWriter::new` | `CodecOptionsBuilder::default().set_physical_type(descr.physical_type())` | data pages and dictionary pages get the type hint |
| `src/file/writer.rs` | `SerializedFileWriter::write_metadata` | calls `with_pcodec_version` on the key-value metadata | spec section 2 |
| `src/file/writer.rs` | new `with_pcodec_version` | adds `parquet.pcodec.version = "1"` when a column chunk uses PCODEC and the key is not there | spec section 2 |
| `tests/arrow_writer.rs` | | `#[path = "arrow_writer/pcodec.rs"] mod pcodec;` | the fork's Arrow tests |
| `tests/arrow_writer/pcodec.rs` | new | Arrow round trips with PCODEC | see Tests |

### Differences from the 60.1.0 port

The code is the same as `rust/parquet/FORK.patch`; only the surroundings differ:

- `Cargo.toml`: the core is at `../../rust/core`.
- `FromStr for Compression`: 59.x parses the level as a `u32` already, so the `PCODEC` arm
  passes it to `PcodecLevel::try_new` without a conversion; 59.x's error for an unknown codec
  reads `unsupport compression …`, which `test_pcodec_compression` pins.
- `create_codec`: 59.x silences `unreachable_code`/`unused_variables` with a plain
  `#[allow]`, so there are no lint expectations to extend.
- `test_write_metadata_page_indexes`: 59.x has no `ParquetMetaData::page_index`; the test
  reads `column_index()` and `offset_index()` instead. 59.x's `write_metadata` has one page
  index decision of its own, `!offset_index_disabled()`, which the test pins.
- `tests/arrow_writer.rs` is a single file in 59.x (60.x has `tests/arrow_writer/mod.rs`), so
  the module is included with `#[path]`; `pcodec.rs` is the same file as in `rust/parquet`.

Unit tests added: `basic::tests::test_pcodec_compression`; `compression::tests::{test_codec_pcodec,
test_codec_pcodec_physical_type, test_codec_pcodec_requires_size,
test_codec_options_physical_type}`; `column::writer::tests::test_column_writer_pcodec_physical_type`;
`file::writer::tests::test_pcodec_version_metadata` (with its helper `pcodec_kv_metadata`)
and `file::writer::tests::test_write_metadata_page_indexes`.

### Decompression call sites without the expected size

`Codec::decompress` takes `uncompress_size: Option<usize>`. Every call site in the crate:

- `file/serialized_reader.rs` `decode_page`: the only call outside tests. It passes
  `Some(uncompressed_page_size - levels)` and skips the call for an empty values section. Used
  by the sync reader and the async reader (DataFusion's).
- `benches/compression.rs`: passes `Some`.
- `compression.rs` tests (`test_codec_without_size`) and the module doc example pass `None`;
  they do not use PCODEC.

`PcodecCodec::decompress` returns `PcodecCodec unsupported without uncompress_size` for `None`
(as `LZ4RawCodec` does), tested by `test_codec_pcodec_requires_size`.

## Tests

From `datafusion/`:

- `cargo test -p parquet --lib -- pcodec test_write_metadata_page_indexes` (7 tests) and
  `cargo test -p parquet --test arrow_writer` (12 tests, 9 of them the fork's): round trip of
  every physical type PCODEC models, required and optional, V1 and V2 pages, with and without
  dictionary pages, levels 0, 8 and 12, logical types, lists in a struct, NaN payloads and
  signed zeros bit for bit, the fallback for strings and booleans, page types, codec in the
  metadata, the footer key, string parsing. `make -C datafusion fork-test` runs both.
- The DataFusion connector's tests (`datafusion/tests`) read the conformance vectors and every
  payload vector through this crate.

## Mutation testing

`make -C datafusion fork-mutants` (`datafusion/scripts/fork-mutants.sh`) mutates the fork's
own functions, in place (in a scratch copy of `datafusion/`, the path dependency
`../../rust/core` does not resolve):

```
cargo mutants --no-config -p parquet --in-place --build-timeout 1200 \
  -f parquet/src/basic.rs -f parquet/src/compression.rs \
  -f parquet/src/column/writer/mod.rs -f parquet/src/file/writer.rs \
  --re 'PcodecLevel|PcodecCodec|with_pcodec_version|CodecOptionsBuilder::set_physical_type|CodecOptionsBuilder::build|create_codec|<impl From<CompressionCodec> for Compression>::from|<impl From<Compression> for CompressionCodec>::from|<impl FromStr for Compression>::from_str|write_metadata' \
  --cargo-arg=--lib -- -- basic:: compression:: column::writer:: file::writer:: \
  --skip test_byte_stream_split_extended_roundtrip \
  --skip test_rewrite_missing_column_index --skip test_rewrite_no_page_indexes
```

(run from `datafusion/`; the skipped tests need `parquet-testing`; 59.3.2 has no
`test_int96_interop`). parquet 59.x declares `compression` with its `experimental!` macro,
which cargo-mutants does not expand: without help it finds 22 mutants and none in
`compression.rs`. The script spells the declaration out as the macro expands it for the run
and restores `src/lib.rs` afterwards (`scripts/check-fork.sh` fails if it were not).

Result (62 mutants; `compression.rs` is listed twice, as for the 60.1.0 fork): 53 caught,
7 unviable, 0 missed, 2 timeouts: `CodecOptionsBuilder::build` replaced by
`Default::default()`, once per listing; `CodecOptions::default()` calls `build`, so the
mutant recurses forever and the tests hang. A hang is a detected mutant: the script accepts
cargo-mutants' exit code 3 when `missed.txt` is empty. The `test_write_metadata_page_indexes`
port catches `delete !` in 59.x's `!offset_index_disabled()` and the match guards of
`write_metadata`.
