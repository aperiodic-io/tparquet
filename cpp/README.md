# tparquet for C and C++

PCODEC ([spec](../spec/FORMAT.md)) for C and C++, two ways:

- **C++**: a small fork of [Apache Arrow C++](https://arrow.apache.org/docs/cpp/) 26.0.1 (Arrow
  and Parquet) that adds the codec ([`arrow/FORK.md`](arrow/FORK.md)), and a library on top,
  `tparquet::tparquet`: `ReadTable`, `WriteTable` and a streaming `FileWriter` with per-column
  PCODEC and automatic codec choice. With the fork, every `parquet::arrow` and
  `parquet::ParquetFileReader` API reads and writes PCODEC files.
- **C**: `libtparquet_read` ([`rust/cread`](../rust/cread), header
  [`tparquet_read.h`](../rust/cread/include/tparquet_read.h)), the Rust reader as an
  [Arrow C stream](https://arrow.apache.org/docs/format/CStreamInterface.html). It needs no
  Arrow C++ at all.

The codec payload itself is the Rust core, reached through the C ABI of
[`rust/capi`](../rust/capi) ([`tparquet.h`](../rust/capi/include/tparquet.h)), which the fork
links into `libarrow.so`.

## C++

```cpp
#include <tparquet/tparquet.h>

ARROW_ASSIGN_OR_RAISE(auto table, tparquet::ReadTable("trades.parquet", {"ts", "price"}));

tparquet::WriteOptions options;                 // Auto, base codec ZSTD(3), pcodec level 8
options.columns = {{"venue", tparquet::Policy::kBase}, {"price", tparquet::Policy::kPcodec}};
ARROW_RETURN_NOT_OK(tparquet::WriteTable(*table, "out.parquet", options));

// streaming: Auto columns are decided on the first batch with rows
ARROW_ASSIGN_OR_RAISE(auto writer, tparquet::FileWriter::Open(schema, sink, options));
ARROW_RETURN_NOT_OK(writer->WriteRecordBatch(batch));
ARROW_RETURN_NOT_OK(writer->Close());
for (const auto& c : writer->codecs()) std::cout << c.path << " " << c.codec << "\n";
```

- **Policies** (`kAuto`, `kPcodec`, `kBase`) apply to the `INT32`, `INT64`, `FLOAT` and
  `DOUBLE` leaves (every logical type stored in them); every other column gets the base codec.
  `WriteOptions::columns` names a top-level field or a dotted leaf path
  (`book.quote.bid`, `levels.list.element`); the longest name that covers a leaf wins.
- **PCODEC columns** are written as spec section 3 recommends: `PLAIN`, no dictionary, the
  column's physical type as the type hint, and `DATA_PAGE_V2` pages (the page version is per
  file: a file with a PCODEC column has V2 pages unless `data_page_version` says otherwise).
- **Auto** (spec section 6): on the first batch with rows, each Auto leaf gets PCODEC when that
  is smaller than the base codec (both with V2 pages); when no column is forced to PCODEC and
  the base codec everywhere with V1 pages is smaller in total, the file keeps the base codec.
- **Defaults**: decimals of precision up to 18 are stored as `INT32`/`INT64` (so PCODEC can
  compress them, and as arrow-rs writes them); the Arrow schema is stored in the footer.
- The footer of a file with a PCODEC column chunk gets `parquet.pcodec.version = "1"` (the
  fork adds it, whatever writes the file).
- `tparquet::CanonicalJsonLines(table)` gives the canonical JSON of
  [`testdata/README.md`](../testdata/README.md), for comparing tables bit for bit.

Errors are `arrow::Status`es. A corrupt PCODEC page is an `IOError` from the codec ("pcodec
decompress failed: pcodec payload: ..."), which the low-level `parquet::` readers throw as a
`parquet::ParquetException`.

### Using it from CMake

```cmake
find_package(tparquet 0.1 REQUIRED CONFIG)    # also finds the patched Arrow and Parquet
target_link_libraries(app PRIVATE tparquet::tparquet)
target_link_libraries(c_app PRIVATE tparquet::read)   # the C stream reader, if installed
```

with `CMAKE_PREFIX_PATH` naming the tparquet install prefix and the patched Arrow's
(`target/cpp/install;target/cpp/arrow-release/install` after `make -C cpp install`).
[`examples/downstream`](examples/downstream) is such a project (`make -C cpp downstream`).
The package requires exactly Arrow 26.0.1; it must be the fork, built with
`ARROW_WITH_PCODEC=ON` (an upstream Arrow builds and links, but refuses PCODEC files).

## C: the Arrow C stream reader

```c
#include "tparquet_read.h"

struct ArrowArrayStream stream;
const char *columns[] = {"ts", "price"};
if (tpr_open("trades.parquet", columns, 2, 0 /* batch size: default */, &stream) != TPR_OK) {
    char msg[256];
    tpr_last_error(msg, sizeof msg);
    /* ... */
}
/* stream.get_schema / get_next / get_last_error / release, or hand the stream to any Arrow
 * consumer: arrow::ImportRecordBatchReader, pyarrow, DuckDB, nanoarrow, ... */
```

- `tpr_open(path, columns, n_columns, batch_size, out)`: the file's rows, optionally projected
  to top-level columns in the requested order (unknown or repeated names are
  `TPR_ERR_ARGUMENT`); the stream's schema is the file's Arrow schema with its metadata.
- `tpr_file_info(path, &rows, &row_groups)`, `tpr_file_schema(path, &schema)`.
- Errors: a return code (`TPR_ERR_ARGUMENT`, `TPR_ERR_IO`, `TPR_ERR_PARQUET`, `TPR_ERR_PANIC`)
  and `tpr_last_error` (per thread); stream callbacks return errno values (`EIO`, `EINVAL`)
  and the stream's `get_last_error`. Pages are decoded as the stream is read, so a corrupt page
  is an `EIO` from `get_next`. Every callback runs under `catch_unwind`: no Rust panic reaches
  C.

```sh
cargo build --release -p tparquet-cread     # target/release/libtparquet_read.{a,so}
cc app.c -I rust/cread/include target/release/libtparquet_read.a -lpthread -ldl -lm
```

`rust/capi` (`tparquet.h`) stays the codec-only ABI, without allocations.

## Building

[`INSTALL.md`](../INSTALL.md#c) covers using the installed package from another project.

Ubuntu 24.04 packages (the CI list is in `.github/workflows/cpp.yml`):

```sh
sudo apt-get install cmake ninja-build ccache thrift-compiler libthrift-dev libzstd-dev \
  libsnappy-dev liblz4-dev libbrotli-dev zlib1g-dev liburiparser-dev rapidjson-dev \
  libgtest-dev libgmock-dev libboost-dev clang libclang-rt-18-dev
make -C cpp build     # or: make -C cpp test
```

`make -C cpp build`:

1. builds `libtparquet_capi.a` and `libtparquet_read` with cargo;
2. `cpp/arrow/prepare.sh` downloads `apache-arrow-26.0.1.tar.gz` from dlcdn.apache.org
   (once, into `target/cpp/dl`), checks its sha512 and applies `FORK.patch` into
   `target/cpp/arrow-src`;
3. `cpp/arrow/deps.sh` builds xsimd 14.3.0 and simdjson 4.6.11 (Arrow 26 needs newer ones
   than Ubuntu 24.04 has) from Debian's source tarballs into `target/cpp/deps`;
4. `cpp/arrow/build.sh release` builds a minimal shared Arrow + Parquet (ZSTD, Snappy, LZ4,
   Brotli, zlib; system dependencies; no compute, dataset, filesystems, etc.) into
   `target/cpp/arrow-release/install` (about 110 build steps; re-runs only rerun ninja);
5. builds the library, the CLI and the tests in `target/cpp/build-release`.

| target | |
|---|---|
| `make -C cpp build` | everything above |
| `make -C cpp test` | the C++ tests, then the C smoke test of `rust/cread` |
| `make -C cpp asan` | Arrow (clang, Debug, ASan, UBSan, SanitizerCoverage) and the tests, run with 200 property cases |
| `make -C cpp fuzz-smoke` | both libFuzzer targets, `FUZZ_TIME` seconds each (default 60) |
| `make -C cpp mutants` | the mutation check of the fork's changed functions |
| `make -C cpp install` / `downstream` | the CMake package into `target/cpp/install`; a downstream project built against it |
| `make -C cpp check-fork` | `scripts/check-cpp-fork.sh` |

`JOBS` (default 3) sets the build parallelism.

## What the fork changes

See [`arrow/FORK.md`](arrow/FORK.md) for the table of every change. In short: `PCODEC = 20547`
in the Parquet thrift enum and its generated code, the footer conversions (`LoadEnumSafe`
accepts exactly 20547 beyond the standard range), `Compression::PCODEC` and a
`PcodecCodecOptions` in Arrow, the codec (`compression_pcodec.cc`, over `pcp_compress` and
`pcp_decompress`) in Arrow's codec factory behind `ARROW_WITH_PCODEC`, the physical type as the
type hint in the page writer, and the footer key. 15 files; the codec is the only new one.
`scripts/check-cpp-fork.sh` checks the release's sha512, that the patch applies cleanly, that it
touches exactly the files `FORK.md` lists, and that its generated thrift code is what the thrift
compiler generates for the patched `parquet.thrift`.

## Testing

`target/cpp/build-release/tparquet-tests` (gtest; 47 tests):

| suite | |
|---|---|
| `Canonical` (12) | the canonical JSON writer alone: every type, escaping, base64, float bits, NaN payloads, lists and list views, slices, structs, maps, dictionaries, unsupported types |
| `Codec` (6) | the fork's codec through `arrow::util::Codec` and `parquet::GetCodec`: every payload vector of `testdata/payloads` (valid ones bit for bit and only at their exact size, invalid ones as `IOError`s), names, levels, availability, the type hint and DECIMAL flag in the payload bytes, argument errors, random payloads |
| `Conformance` (7) | every `testdata/files` vector through `parquet::arrow::FileReader` (whole and in batches of 100), `ReadTable`, and the C stream reader imported with `arrow::ImportRecordBatchReader`: all give exactly `expected/<name>.jsonl`, and the two readers agree; footer key, codecs, physical types and page types per column against `files/manifest.json`; other codec values are still refused |
| `Writer` (14) | round trips of every type the vectors cover through Pcodec, Base and Auto, V1/V2/default pages, one and four row groups, read back by the fork and the Rust reader; codecs, encodings, dictionary pages, page types and the footer key per chunk; the physical type in the payload's dtype byte; levels; per-column policies; both Auto rules; empty tables; user metadata; the streaming lifecycle |
| `Properties` (3) | random tables round trip (random types, nulls, policies, page versions, row group sizes, levels; read back by both readers); random corruption of PCODEC column chunks, and truncated files, are errors and never crashes |
| `Cli` (5) | `tparquet-cpp-vectors check` and `write` (below) |

`make -C cpp asan` runs the same suite built with clang, ASan and UBSan against the ASan build
of Arrow. The libFuzzer targets in [`fuzz/`](fuzz) are `pcodec_decompress_fuzz` (the codec's
`Decompress`, seeded with the payload vectors) and `parquet_read_fuzz` (`ReadTable`, the
canonical JSON of every value and Arrow's own Parquet fuzz entry point, seeded with the vector
files; the ASan Arrow build carries SanitizerCoverage, so Arrow and Parquet give coverage
feedback). The Rust decoder behind both is fuzzed with coverage in `rust/core/fuzz`.

### The acceptance hook

```sh
target/cpp/build-release/tparquet-cpp-vectors check DIR         # "<n> vectors match", or exit 1
target/cpp/build-release/tparquet-cpp-vectors write OUT SRC...  # every SRC vector through the C++ writer
```

`check` reads every `DIR/files/*.parquet` with the fork and compares it with
`DIR/expected/<name>.jsonl`; on a difference it prints
`tparquet-cpp-vectors: <name>: line N differs from expected/<name>.jsonl` and exits 1, as
`acceptance/cross/vectors.py` and `gen-testdata --check` do. `write` reads each vector with the
fork, writes it with `Policy::kPcodec` and copies its expected rows.

### Mutation testing

No C++ mutation tool can be installed on the build machines (no `mull` or `dextool` package for
Ubuntu 24.04, Mull's package repository is not reachable, nothing on PyPI), so
[`mutation/run.py`](mutation/run.py) is a small script-based check: it applies the hand-picked
mutations of [`mutation/mutants.json`](mutation/mutants.json), one at a time, to the fork's
changed functions in `target/cpp/arrow-src` (every function of the patch that is not a
declaration or build wiring), rebuilds Arrow incrementally, runs the C++ tests and requires them
to fail. Header-only changes whose mutation would rebuild all of Arrow (the enums, the options
class, the generated thrift code, the CMake files) are not mutated: without them nothing
compiles or links. Result: 37 mutants, 37 caught (details in
[`arrow/FORK.md`](arrow/FORK.md)).

The C library's Rust code is mutated with `cargo mutants -p tparquet-cread` (the root
`.cargo/mutants.toml`, no exclusion needed): 24 mutants, 15 caught, 9 unviable, none missed.
