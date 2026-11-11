# Fork of the arrow-rs `parquet` crate

This directory is the `parquet` crate as published on crates.io, with the PCODEC codec of
`spec/FORMAT.md` added. The crate name stays `parquet`; the root workspace points every
`parquet = "60.1.0"` dependency here with `[patch.crates-io]`.

| | |
|---|---|
| upstream | `parquet` 60.1.0 |
| source | `https://static.crates.io/crates/parquet/parquet-60.1.0.crate` |
| sha256 | `806eaa0f1c0436dcb43bee8e6a82bf2efff90521a60172f8281cb9ef577b43a4` |

The crate is vendored as published (its normalized `Cargo.toml`), without `Cargo.toml.orig`.
`FORK.md` and `FORK.patch` are not part of upstream.

`FORK.patch` is the complete diff from upstream to this tree. `scripts/check-fork.sh`
downloads the upstream crate, checks the sha256 above, and fails unless the diff of the
git-tracked files equals `FORK.patch`; `scripts/check-fork.sh --update` rewrites it after an
intended change, which must also be listed below.

## Changes

| File | Item | Change | Why |
|---|---|---|---|
| `Cargo.toml` | `[features]` | new feature `pcodec = ["dep:tparquet-core"]`, in `default` | the codec, default on like the other codecs |
| `Cargo.toml` | `[dependencies]`, `[dev-dependencies]` | `tparquet-core = { path = "../core" }` (optional; and for tests) | the payload encoder and decoder; unit tests build every codec, as upstream does with `cfg(any(feature, test))` |
| `src/basic.rs` | `CompressionCodec` (thrift enum) | `PCODEC = 20547` | the codec id of spec section 2 |
| `src/basic.rs` | `Compression` | variant `PCODEC(PcodecLevel)` | the writer-side codec choice |
| `src/basic.rs` | `From<CompressionCodec> for Compression`, `From<Compression> for CompressionCodec` | `PCODEC` arms | conversion to and from the footer |
| `src/basic.rs` | `FromStr for Compression` | `PCODEC`/`pcodec`, optionally with a level, `PCODEC(level)` | string parsing as for the other codecs; without a level it is the default 8 |
| `src/basic.rs` | `pub use crate::compression::…` | adds `PcodecLevel` | public like `ZstdLevel` (the `compression` module is private unless `experimental`) |
| `src/compression.rs` | `CodecOptions`, `CodecOptionsBuilder` (struct, `Default`, `build`) | field `physical_type: Option<Type>`, setter `CodecOptionsBuilder::set_physical_type` | the PCODEC type hint; other codecs ignore it |
| `src/compression.rs` | `create_codec` | `PCODEC` arm; `pcodec` added to the `cfg_attr` feature lists of its lint expectations | creates `PcodecCodec` |
| `src/compression.rs` | new `mod pcodec_codec`: `PcodecCodec::{new, compress, decompress}` | the codec over `tparquet_core`; `decompress` without `uncompress_size` is an error; `decompress` appends with `tparquet_core::decompress_append`, which allocates and writes nothing proportional to `uncompress_size` before the payload shows it can produce that many bytes | PCODEC needs the expected size (spec section 3); that size is the untrusted `uncompressed_page_size` (spec section 7) |
| `src/compression.rs` | new `PcodecLevel` (`CompressionLevel`, `try_new`, `compression_level`, `Default`) | levels 0..=12, default 8 | as `ZstdLevel` |
| `src/column/writer/mod.rs` | `GenericColumnWriter::new` | `CodecOptionsBuilder::default().set_physical_type(descr.physical_type())` | data pages and dictionary pages get the type hint |
| `src/file/writer.rs` | `SerializedFileWriter::write_metadata` | calls `with_pcodec_version` on the key-value metadata | spec section 2 |
| `src/file/writer.rs` | new `with_pcodec_version` | drops the `parquet.pcodec.version` pairs of the key-value metadata given by the caller, then adds `parquet.pcodec.version = "1"` when a column chunk uses PCODEC | spec section 2; the writer owns the key, so metadata copied from another file cannot give a stale value, a duplicate, or the key in a file without PCODEC |
| `src/arrow/array_reader/fixed_len_byte_array.rs` | `FixedLenByteArrayReader::consume_batch` | reads an `INTERVAL` column as `Interval(MonthDayNano)` when the Arrow schema hint asks for it (upstream: `NYI`): months, days, and the unsigned milliseconds in nanoseconds | upstream reads such a column as `DayTime` unless the file's own Arrow schema says otherwise, dropping the months of every file other writers (DuckDB, Spark) wrote; the DuckDB extension asks for `MonthDayNano` |
| `tests/arrow_writer/mod.rs` | | `mod pcodec;` | the fork's Arrow tests |
| `tests/arrow_writer/pcodec.rs` | new | Arrow round trips with PCODEC | see Tests |

Unit tests added: `arrow::array_reader::fixed_len_byte_array::tests::test_interval_month_day_nano`; `basic::tests::test_pcodec_compression`; `compression::tests::{test_codec_pcodec,
test_codec_pcodec_physical_type, test_codec_pcodec_requires_size,
test_codec_pcodec_untrusted_size, test_codec_options_physical_type}`; `column::writer::tests::test_column_writer_pcodec_physical_type`;
`file::writer::tests::test_pcodec_version_metadata` (with its helper `pcodec_kv_metadata`)
and `file::writer::tests::test_write_metadata_page_indexes`, which pins upstream's page index
decisions in `write_metadata` (a function the fork changes) so that its mutants are caught.

### Decompression call sites without the expected size

`Codec::decompress` takes `uncompress_size: Option<usize>`. Every call site in the crate:

- `file/serialized_reader.rs` `decode_page`: the only call outside tests. It passes
  `Some(uncompressed_page_size - levels)` and skips the call for an empty values section. Used
  by the sync reader, the async reader, the push decoder and the dictionary reader
  (`file/metadata/dictionary.rs`).
- `benches/compression.rs`: passes `Some`.
- `compression.rs` tests (`test_codec_without_size`) and the module doc example pass `None`;
  they do not use PCODEC.

`PcodecCodec::decompress` returns `PcodecCodec unsupported without uncompress_size` for `None`
(as `LZ4RawCodec` does), tested by `test_codec_pcodec_requires_size`.

### The expected size is untrusted

`uncompressed_page_size` comes from the page header, so a page of a few bytes can claim almost
2 GiB. Upstream `decode_page` reserves that capacity (`Vec::with_capacity`) for every codec
before calling it, but writes none of it; upstream's `SNAPPY`, `LZ4_RAW` and `LZ4` (Hadoop)
codecs then zero-fill the claim with `resize`, and `BROTLI` sizes its buffer by it. The fork
leaves `decode_page` and those codecs as upstream has them. `PcodecCodec::decompress` appends
with `tparquet_core::decompress_append`: a RAW body must have the expected length, a ZSTD body
must be long enough to hold it, and PCO and DECIMAL output grows as the numbers are decoded,
so the reserved capacity is written only with decoded bytes; a failed allocation is an error.
`test_codec_pcodec_untrusted_size` checks the output's capacity after such a page, and
`rust/tparquet/tests/hostile_page.rs` reads such pages through `SerializedPageReader`: the
peak allocation is upstream's reservation and nothing more, and the resident size does not
grow by it. That reservation still aborts, as upstream, where the address space cannot hold
it.

## Tests

- `cargo test -p parquet --lib -- pcodec` and `cargo test -p parquet --test arrow_writer -- pcodec`:
  the fork's tests (round trip of every physical type PCODEC models, required and optional,
  V1 and V2 pages, with and without dictionary pages, levels 0, 8 and 12, logical types,
  lists in a struct, NaN payloads and signed zeros bit for bit, the fallback for strings and
  booleans, page types, codec in the metadata, the footer key, string parsing).
- The upstream suite: `cargo test -p parquet --all-features --no-fail-fast` (`make
  fork-upstream-tests`), without the `parquet-testing` and `arrow-testing` data. With the fork:
  1806 tests pass and 340 fail (335 test functions and 5 doc tests); every failure is missing
  data: 199 panic in `arrow::util::test_util::parquet_test_data`, 4 in `arrow_test_data`, and
  137 in `tests/variant_integration.rs` on a `LazyLock` poisoned by that panic. The unmodified
  tree (test binaries only) fails the same 335 test functions, plus
  `parquet-fromcsv::tests::test_command_help`, which only failed there because that run
  started the binaries without `CARGO_MANIFEST_DIR`.

## Mutation testing

The repository's `.cargo/mutants.toml` excludes `rust/parquet/**`. The fork's own functions are
mutated with:

```
cargo mutants --no-config -p parquet \
  -f rust/parquet/src/basic.rs -f rust/parquet/src/compression.rs \
  -f rust/parquet/src/column/writer/mod.rs -f rust/parquet/src/file/writer.rs \
  --re 'PcodecLevel|PcodecCodec|with_pcodec_version|CodecOptionsBuilder::set_physical_type|CodecOptionsBuilder::build|create_codec|<impl From<CompressionCodec> for Compression>::from|<impl From<Compression> for CompressionCodec>::from|<impl FromStr for Compression>::from_str|write_metadata' \
  --cargo-arg=--lib -- -- basic:: compression:: column::writer:: file::writer:: \
  --skip test_byte_stream_split_extended_roundtrip --skip test_int96_interop \
  --skip test_rewrite_missing_column_index --skip test_rewrite_no_page_indexes
```

(the four skipped tests need `parquet-testing`). `make fork-mutants` runs it. cargo-mutants
generates no mutants in `GenericColumnWriter::new` (a constructor) and none in the
`CompressionCodec` enum; the one-line change in `new` is covered by
`test_column_writer_pcodec_physical_type`.

Result (47 mutants): 36 caught and 7 unviable on the first run; the two `||` mutants in the
upstream part of `write_metadata` survived and are caught since
`test_write_metadata_page_indexes`; `CodecOptionsBuilder::build` replaced by
`Default::default()` times out (twice, the file is listed by two `-f` patterns):
`CodecOptions::default()` calls `build`, so the mutant recurses forever and the tests hang.
A hang is a detected mutant: `make fork-mutants` accepts cargo-mutants' exit code 3 when
`missed.txt` is empty.

The `Interval(MonthDayNano)` arm of `FixedLenByteArrayReader::consume_batch` (an upstream function
too large to mutate whole) was mutated on its diff alone:

```
cargo mutants --no-config -p parquet -f rust/parquet/src/arrow/array_reader/fixed_len_byte_array.rs \
  --in-diff <diff of the change> --cargo-arg=--lib -- -- arrow::array_reader::fixed_len_byte_array
```

Result (8 mutants, each listed twice): 6 caught, 2 unviable.
