# Fork of DuckDB's parquet extension

The tparquet DuckDB extension is DuckDB's own parquet extension (`extension/parquet` of the DuckDB
source tree) with the PCODEC codec of `spec/FORMAT.md` added and its functions renamed, built as
a loadable extension. Nothing of the Parquet reading and writing is new code: the reader, the
writer, the statistics, filter pushdown, `count(*)` from the footer, the multi-file reader
(globs, lists, `union_by_name`, `hive_partitioning`, `filename`, `file_row_number`), remote
files through DuckDB's file system (httpfs, S3, registered file systems) for reading and
writing, encryption and the metadata functions are DuckDB's, as DuckDB tests and ships them.

The DuckDB source tree is large (96 MB), so the fork is one patch per DuckDB version over the
DuckDB source distribution on PyPI (`duckdb-VERSION.tar.gz`, whose `external/duckdb` is DuckDB's
complete source tree; github.com is not reachable from the build machines):

| DuckDB | source | sha256 | patch |
|---|---|---|---|
| 1.5.6 | `https://files.pythonhosted.org/packages/source/d/duckdb/duckdb-1.5.6.tar.gz` | `166a91dbfacfc0c9f08cc76c0243cb6d3d4296bfab5bad72a3cfb63140a5b7c8` | `1.5.6.patch`, 11 files |
| 2.0.0 | `https://files.pythonhosted.org/packages/source/d/duckdb/duckdb-2.0.0.tar.gz` | `09de511ef0dc76a5213e8d3d87a8b89bb4d3e8a6fe73a9f60dc001269946b44f` | `2.0.0.patch`, 10 files |

The sha256 values are the ones PyPI publishes for the files (`https://pypi.org/pypi/duckdb/VERSION/json`).
A C++ extension is built for one DuckDB version and platform: DuckDB loads it only into that
version (the footer names it). The two patches make the same changes; the code around them
differs between the versions (1.5.6's `ParquetWriter` keeps its options as members, function
set names are `string`s rather than `Identifier`s, the extension is built with
`build_static_extension`/`build_loadable_extension` rather than `build_extension_library`).

- `prepare.sh VERSION [DEST]` downloads the source distribution (cached in `target/duckdb/dl`),
  checks its sha256, extracts its DuckDB tree and applies `VERSION.patch` into
  `target/duckdb/VERSION/src`.
- `build.sh VERSION` builds the codec's C ABI (`rust/capi`, `libtparquet_capi.a`) and the
  extension (`ninja tparquet_loadable_extension`) into `build/vVERSION/tparquet.duckdb_extension`.
  The build is DuckDB's own: `EXTENSION_STATIC_BUILD=1` links DuckDB into the extension with
  every symbol hidden but the entry point `tparquet_duckdb_cpp_init`, as DuckDB builds the
  extensions it distributes; the extension's copy of the parquet code therefore never mixes with
  the host's built-in one (Python's `duckdb` loads its library with `RTLD_LOCAL`, and its
  parquet extension is built in). `DISABLE_BUILTIN_EXTENSIONS=1` leaves the other extensions out
  of the build.
- `scripts/check-duckdb-fork.sh` downloads and verifies both releases, checks that this file
  records their sha256 values, that each patch applies cleanly and changes exactly the files
  listed below; `scripts/check-duckdb-fork.sh --update VERSION` rewrites `VERSION.patch` from an
  edited `target/duckdb/VERSION/src`.

## Design

**Next to DuckDB's own parquet extension.** DuckDB builds the parquet extension into its
releases, and a function registered under an existing name replaces DuckDB's
(`OnCreateConflict::ALTER_ON_CONFLICT`). So the extension registers every function under a name
of its own, `tparquet` for `parquet`: `read_tparquet`, `tparquet_scan`, `read_single_tparquet_file`,
`tparquet_metadata`, `tparquet_schema`, `tparquet_kv_metadata`, `tparquet_file_metadata`,
`tparquet_bloom_probe`, `tparquet_full_metadata`, the pragma `add_tparquet_key`, and the COPY
format `tparquet`. It leaves out what DuckDB's parquet extension registers already: the ZSTD
compression file system, the replacement scan of `FROM 'f.parquet'`, the two variant functions;
the settings (`binary_as_string`, ...) are registered again, which DuckDB ignores (`emplace`), so
both extensions read the same settings. `read_parquet` stays DuckDB's: it cannot read PCODEC
files (spec section 2) and writes standard Parquet.

**The codec.** PCODEC is a block codec over the bytes Parquet would otherwise compress (spec
section 3), so it goes where the other codecs are: `ColumnReader::DecompressInternal` (which is
given the exact decompressed size: the page's, or the values section's of a V2 page) and
`ColumnWriter::CompressPage`, calling `pcp_decompress` and `pcp_compress` of `rust/capi`. A
payload that does not decode to exactly the expected size, or is malformed, is an
`InvalidInputException` that names the file.

**Writing.** `COPY ... (FORMAT tparquet)` writes PCODEC by default (`COMPRESSION pcodec`); the
other codecs of DuckDB's writer (`zstd`, `snappy`, ...) write standard files. In a PCODEC file,
the INT32, INT64, FLOAT and DOUBLE columns (DuckDB's integers, floats, dates, times, timestamps
and decimals up to 18 digits) get PCODEC, PLAIN encoding and no dictionary (spec, writer
recommendations), with the column's physical type as the type hint and DECIMAL mode for floats;
the other columns get ZSTD at DuckDB's default level. `COMPRESSION_LEVEL` is the pcodec level
(0 to 12, default 8). The footer gets `parquet.pcodec.version = "1"` (spec section 2). DuckDB's
writer writes `DATA_PAGE` (V1) pages only, so the codec input of a nullable column's page holds
its levels before its values; the payload encoder handles that (spec section 4), and the ratio
target of the acceptance tests holds (`tests/test_acceptance.py`: 0.63 of DuckDB's ZSTD level 9
on the market ticks).

## Changes

The same items in both versions (`1.5.6.patch`, `2.0.0.patch`), but where a row says otherwise:

| File | Item | Change | Why |
|---|---|---|---|
| `third_party/parquet/parquet_types.h` | `CompressionCodec::type` | `PCODEC = 20547` | the codec id of spec section 2 |
| `third_party/parquet/parquet_types.cpp` | `_kCompressionCodecValues`, `_kCompressionCodecNames`, `_CompressionCodec_VALUES_TO_NAMES` | `PCODEC` entry, 9 values | the Thrift reader (`SafeEnumCast`) refuses a value missing from the table ("TProtocolException: Invalid data") |
| `extension/parquet/column_reader.cpp` | `ColumnReader::DecompressInternal` | `PCODEC`: `pcp_decompress`, error with `pcp_last_error` | decoding |
| `extension/parquet/column_writer.cpp` | `ColumnWriter::CompressPage` | switches on the column's codec; `PCODEC`: `pcp_compress` with the physical type and the level; the ZSTD columns of a PCODEC file at DuckDB's default ZSTD level | encoding |
| `extension/parquet/include/parquet_writer.hpp` | new `ParquetWriter::GetCodec(schema_idx)` | `PCODEC` for INT32/INT64/FLOAT/DOUBLE columns of a PCODEC file, ZSTD for the others | the per-column codec |
| `extension/parquet/writer/primitive_column_writer.cpp` | `PrimitiveColumnWriter::RegisterToRowGroup` | the column chunk's codec from `GetCodec(SchemaIndex())` | the footer names the codec each column chunk has |
| `extension/parquet/include/writer/templated_column_writer.hpp` | `StandardColumnWriter::FinalizeAnalyze` | PCODEC columns: no dictionary, PLAIN | spec, writer recommendations |
| `extension/parquet/parquet_writer.cpp` | `ParquetWriter::ParquetWriter` | `parquet.pcodec.version = "1"` in a PCODEC file's footer | spec section 2 |
| `extension/parquet/parquet_extension.cpp` | `ParquetWriteBindData::codec`, `ParquetWriteBind` | default `PCODEC`; `COMPRESSION pcodec`; level 0 to 12 for PCODEC (default -1: pcodec's 8), ZSTD levels for ZSTD | the COPY options |
| `extension/parquet/parquet_extension.cpp` | `EnumUtil::ToChars/FromString<CompressionCodec>` | `PCODEC` | the serialization of a COPY plan (without it, DuckDB throws on PCODEC) |
| `extension/parquet/parquet_extension.cpp` | `LoadInternal`, `DUCKDB_CPP_EXTENSION_ENTRY` | the functions under `tparquet` names; without the compression file system, replacement scan and variant functions; the entry point `tparquet_duckdb_cpp_init` | next to DuckDB's parquet extension (Design); DuckDB calls `<file name>_duckdb_cpp_init` |
| `scripts/append_metadata.cmake` | new (1.5.6 only) | DuckDB 2.0.0's script, unchanged | the build appends the extension's footer with it, and the 1.5.6 source distribution leaves `scripts/` out (`prepare.sh` writes `scripts/null.txt`, its one NUL byte) |
| `extension/parquet/CMakeLists.txt` | the extension's targets; `TPARQUET_CAPI_INCLUDE_DIR`, `TPARQUET_CAPI_LIBRARY` | the extension is named `tparquet` (targets, entry point, exported symbol, file name); `tparquet.h` on the include path, `libtparquet_capi.a` linked | the build |

## Tests

`tests/` (pytest, run by `make test` against the build of each DuckDB version with that
version's Python `duckdb`):

- `test_conformance.py`: every `testdata/files` vector, read with `read_tparquet`, gives exactly
  its expected rows (whole, and column by column); row counts and row groups match the manifest.
- `test_write.py`: COPY: the codec, encoding and dictionary of every column, the footer key, the
  round trip of every type, the Rust reference decoder (`tparquet decode`, `verify`) reading what
  DuckDB wrote, the ratio against ZSTD, levels, the other codecs, partitions, `COPY FROM`, empty
  and all-null columns, special floats.
- `test_load.py`: the extension's footer (DuckDB version, `CPP`), its functions under its own
  names, DuckDB's `read_parquet` and `COPY (FORMAT parquet)` unchanged (and refusing PCODEC).
- `test_queries.py`, `test_types.py`, `test_properties.py`: queries, every DuckDB type and
  generated tables (Hypothesis) give over `read_tparquet` what DuckDB's own Parquet gives.
- `test_pushdown.py`: row groups skipped by their statistics (a damaged row group read only if
  not skipped), on every type, PCODEC columns included; float edge cases against `read_parquet`.
- `test_remote.py`: files of a registered fsspec file system and over HTTP (httpfs): single
  files, lists, globs, failing reads, `COPY FROM` and `COPY TO` a remote file.
- `test_hostile.py`: truncated, zeroed and randomly damaged vectors are errors, never a crash
  or an internal error.
- `test_signing.py`: the signature `sign.py` writes verifies with the public key; on 2.0.0, a
  signed build loads from an extension repository that trusts the key, and tampered, unsigned
  and other-key files are refused (DuckDB 1.5.6 has no repositories with keys of their own).

Where DuckDB 1.5.6 itself behaves differently from 2.0.0 (no `TIMESTAMPTZ_NS`, fewer filters
pruned, its error messages, no `extension_name` in `duckdb_functions()`, an ordered COPY that
does not keep its order), the tests say so (`conftest.DUCKDB_1`) and expect what DuckDB's own
parquet extension does.
- `test_acceptance.py`: DuckDB alone refuses PCODEC; the standard control file reads as with
  `read_parquet`; the ratio target with a bit-exact round trip; bounded memory.

DuckDB's own tests of the parquet extension (its sqllogictests) are not run: they need DuckDB's
unittest runner, a full DuckDB build.

## Mutation testing

`mutation/run.py` applies the hand-picked mutations of `mutation/mutants.json` (every item the
patch adds or changes) to `target/duckdb/VERSION/src` one at a time, rebuilds the extension
incrementally, runs the tests and requires them to fail (`make mutation`), as `cpp/mutation`
does for the Arrow fork.
