# `tparquet`: a DuckDB extension for Parquet files with the PCODEC codec

DuckDB's own Parquet reader cannot read files whose column chunks use `CompressionCodec`
20547 (`PCODEC`, [`spec/FORMAT.md`](../spec/FORMAT.md)): DuckDB 2.0.0 stops on the footer with

```
TProtocolException: Invalid data
```

(its Thrift parser rejects the unknown codec value; `tests/test_acceptance.py` pins this). This
extension adds a reader and a COPY format that handle PCODEC and standard Parquet alike:

```sql
LOAD 'build/tparquet.duckdb_extension';   -- needs allow_unsigned_extensions

SELECT * FROM read_tparquet('trades/*.parquet');
SELECT symbol, avg(price) FROM read_tparquet(['a.parquet', 'b.parquet'], filename := true) GROUP BY ALL;

COPY (SELECT ts, price, qty FROM ticks) TO 'ticks.parquet'
    (FORMAT tparquet, COLUMN_NAMES ['ts', 'price', 'qty'], POLICY 'auto', LEVEL 8);
COPY ticks FROM 'ticks.parquet' (FORMAT tparquet);
```

It is written in Rust on DuckDB's C extension API, with the `duckdb` crate 1.20000.1
(`loadable-extension`, `vtab-arrow`) and `libduckdb-sys` 1.20000.1 (`loadable-extension-v2`), and
the Parquet fork in [`rust/parquet`](../rust/parquet) through [`rust/tparquet`](../rust/tparquet).

### Two builds

One library, packaged twice (`make build`):

| | `build/tparquet.duckdb_extension` | `build/v2/tparquet.duckdb_extension` |
|---|---|---|
| C API | v1, version `v1.5.6` (stable) | v2 (unstable in DuckDB 2.0.0) |
| loads in | any DuckDB that serves C API v1.5.6 (2.0.0 and later v1.x servers) | DuckDB 2.0.0 only (ABI `C_STRUCT_UNSTABLE`) |
| entrypoint | `tparquet_init_c_api` | `tparquet_init_c_api_v2` |
| remote files (httpfs, S3, registered file systems) | yes | yes |
| globs over remote files | no: list the files | yes (DuckDB's glob) |
| filter pushdown: row groups skipped by statistics | no | yes |
| parallel scans that keep the file order | no: ordered consumers run the scan on one thread | yes (batch indexes) |
| `count(*)` | decodes the first column | decodes the first column |
| `tparquet_metadata` (row counts from footers) | yes | yes |

Everything DuckDB's C API v1 cannot express is what the v2 build adds: the C API v1 hands a
table function no filters and has no batch indexes or glob. Every function of the C API v2 is
marked unstable in DuckDB 2.0.0, and DuckDB loads an extension built on it only in the exact
version it was built for, as it does its own C++ extensions: the v2 build must be rebuilt for
every DuckDB release. Use the v1 build where DuckDB versions vary, the v2 build to read remote
data with filters on DuckDB 2.0.0. Both pass the same SQL test suite (`TPARQUET_DUCKDB_API`
selects the build; tests assert each build's own behavior where they differ).
The `duckdb` crate's arrow (`^60`) unifies with the fork's arrow 60.1.0; `make check-arrow`
fails if a second arrow or parquet ever enters the build.

### What comes from the `duckdb` crate

- `duckdb::ffi` (libduckdb-sys' loadable-extension bindings): every C API call goes through
  the function-pointer table DuckDB hands over at `LOAD` (`duckdb_rs_extension_api_init`).
- `duckdb::vtab::arrow::write_arrow_array_to_vector` writes the scan's Arrow arrays into DuckDB
  vectors. `types::normalize` first reshapes the arrays it would not copy exactly: it rounds
  nanosecond times and time-zoned nanosecond timestamps to microseconds, and it does not take
  dictionaries, half floats or 256-bit decimals.
- Not used: the `VTab` trait and `register_table_function` (no local init state per thread, no
  raw registration, and no COPY functions at all), the `#[duckdb_entrypoint_c_api]` macro (it
  hands over a `Connection` without its raw handle, which raw registration needs; `src/lib.rs`
  has the same few lines), `LogicalTypeHandle` (it has no `TIMESTAMPTZ_NS`), and
  `data_chunk_to_arrow` for COPY (no decimals, enums, `TIME_NS` or `TIMESTAMPTZ_NS`):
  `src/chunk.rs` reads DuckDB's vector layout itself.

## Loading

```sh
make build                      # build/tparquet.duckdb_extension (release)
```

- Python: `duckdb.connect(config={"allow_unsigned_extensions": "true"})`, then
  `con.execute("LOAD 'build/tparquet.duckdb_extension'")`.
- CLI: `duckdb -unsigned`, then `LOAD 'build/tparquet.duckdb_extension';`.

Without `allow_unsigned_extensions` DuckDB refuses the file (it has no signature; see
[Distribution](#distribution)). The CLI path is not covered by the tests: the DuckDB 2.0.0 CLI
binary could not be downloaded in the environment the extension was developed in (no release
asset reachable, no `duckdb-cli` 2.0.0 wheel); the CLI loads extensions through the same engine
code as the Python package.

## Reading: `read_tparquet(path, ...)`

`path` is a file, a glob (`*`, `?`, `[...]`; matches sorted by name) or a list of files and
globs. Named parameters:

| parameter | default | effect |
|---|---|---|
| `filename` | false | adds a `filename` VARCHAR column |
| `file_row_number` | false | adds a `file_row_number` BIGINT column: the row's position in its file, from 0 |
| `union_by_name` | false | combines files by column name; a file without a column gives NULL. The same name must have the same DuckDB type in every file |

Without `union_by_name` every file must have the schema of the first file (the same column
names and DuckDB types, in order), or the scan stops with an error naming both files.

**Column names** are case-insensitive, as everywhere in DuckDB: `Price` in one file and `price`
in another are the same column (named as the first file names it). A file whose columns (or
struct fields) repeat a name, `x` and `X`, gets the names `read_parquet` gives them: `x`,
`X_1`.

**Projection pushdown.** Only the columns the query uses are read (`EXPLAIN` shows
`Projections:`). `count(*)` reads one column, the first: DuckDB asks a C API table function
(v1 or v2), which can declare neither a row id nor an empty projection, for its first column,
and it has no way to hand DuckDB row-group statistics (read_parquet answers `count(*)` from the
footer). For a count without decoding anything, sum the footers' row counts:
`SELECT sum(row_group_num_rows) FROM tparquet_metadata('trades/*.parquet')`.

**Filter pushdown (v2 build).** DuckDB offers a C API v2 table function the filters of the
query; read_tparquet reads comparisons with constants (`=`, `<>`, `<`, `<=`, `>`, `>=`, `IN`,
`BETWEEN`, either operand order), `IS [NOT] NULL`, `AND` and `OR` of them, and skips every row
group whose statistics rule them out: signed and unsigned integers, `FLOAT`/`DOUBLE` (NaN is
greater than every number in DuckDB, so `>`, `>=` and `<>` skip only where the NaN count is
known to be zero), `VARCHAR` (byte order), `DATE`, `TIMESTAMP_MS`, `TIMESTAMP`, `TIMESTAMP_NS`,
`TIMESTAMPTZ` stored in the column's own unit, and `DECIMAL` of the column's scale. A column
that a file lacks (`union_by_name`) is NULL there. The filters are not taken over: DuckDB still
applies each one to the rows, so skipping can only save reading, never change a result. A
filter on a cast column (`ts_ms >= TIMESTAMP '...'` compares `CAST(ts_ms AS TIMESTAMP)`) or on
an expression is not read. The v1 build gets no filters: DuckDB applies them after the scan.

**Parallelism.** The scan's unit of work is a row group. DuckDB's threads take row groups from
a shared queue that walks the files in order (opening each footer once); `max_threads` is the
number of row groups bind can see (row groups of the first file times the number of files).
Rows come in file order with any number of threads in both builds. The v2 build reports each
row group as a batch (its index in file order), so DuckDB reads in parallel and puts the rows
back in order for query results, `INSERT` and `COPY`. The v1 build cannot report batches, so
DuckDB keeps the order by running such pipelines on one thread (aggregations and joins still
run in parallel); with `SET preserve_insertion_order = false` it runs them in parallel without
order.

**Memory.** Each thread holds one row-group reader and decodes one batch of at most one DuckDB
vector (2048 rows) at a time; pages are read as needed. Reading 400 MB of values in the slow
test keeps the process's peak RSS where it was before the scan (97 MB).

**Files.** Files are read through DuckDB's file system, so everything DuckDB reads works:
local files, `httpfs` (`https://`, `s3://`, ... with DuckDB's settings and secrets) and file
systems registered with DuckDB (fsspec from Python). Column chunks are read with ranged reads.
Globs over local paths work in both builds; globs over remote paths need DuckDB's glob, which
only the C API v2 has (the v1 build refuses them and names the pattern: list the files). A
leading `~/` is the home directory, as in `COPY ... TO`.

## Footers: `tparquet_metadata(path)`

One row per row group of the files (a file, a glob or a list, as for read_tparquet), read from
the footers only: `file_name`, `row_group_id`, `row_group_num_rows`, `row_group_num_columns`,
`row_group_bytes` (uncompressed) and `row_group_compressed_bytes`.

```sql
SELECT sum(row_group_num_rows) FROM tparquet_metadata('trades/*.parquet');   -- the row count
```

### Types

The Parquet reader returns Arrow types (from the embedded Arrow schema when the file has one);
each maps to the DuckDB type that holds its values exactly:

| Parquet / Arrow | DuckDB |
|---|---|
| boolean | `BOOLEAN` |
| int8, int16, int32, int64 | `TINYINT`, `SMALLINT`, `INTEGER`, `BIGINT` |
| uint8, uint16, uint32, uint64 | `UTINYINT`, `USMALLINT`, `UINTEGER`, `UBIGINT` |
| float16, float32 / float64 | `FLOAT` (half floats widened) / `DOUBLE` |
| decimal32/64/128/256 with at most 38 digits | `DECIMAL(p, s)` |
| string, large string, string view, dictionary of strings | `VARCHAR` |
| binary, large binary, binary view, fixed-size binary | `BLOB` |
| date32, date64 | `DATE` |
| time32 (s, ms), time64 (us) / time64 (ns) | `TIME` / `TIME_NS` |
| timestamp s, ms, us, ns without time zone | `TIMESTAMP_S`, `TIMESTAMP_MS`, `TIMESTAMP`, `TIMESTAMP_NS` |
| timestamp s, ms, us with a time zone / ns with a time zone | `TIMESTAMP WITH TIME ZONE` / `TIMESTAMPTZ_NS` |
| interval, duration | `INTERVAL` |
| list, large list / fixed-size list | `LIST` / `ARRAY` |
| struct, map | `STRUCT`, `MAP` |

DuckDB stores time-zoned timestamps as UTC instants in microseconds (or nanoseconds for
`TIMESTAMPTZ_NS`); the column's own zone (`America/New_York`) is not kept, and DuckDB shows the
instant in its `TimeZone` setting. Decimals of more than 38 digits and Arrow `Null` columns are
refused with the column's name.

Parquet `INTERVAL` columns (months, days, milliseconds, as DuckDB's own Parquet writer stores
them) are read with all three fields, at any depth; durations become microseconds (a duration
that does not fit in 64-bit microseconds is an error). Where read_tparquet and read_parquet
differ on DuckDB's own Parquet files: millisecond timestamps stay `TIMESTAMP_MS` (read_parquet
gives `TIMESTAMP`), `UUID` columns read as their 16 bytes (`BLOB`), and `TIME WITH TIME ZONE`
as the `TIME` in UTC that DuckDB stored.

**Damaged files** give an error that names the file, also where the Parquet reader panics on a
damaged page (its level decoder does; the panic is caught). The vendored `parquet` 60.1.0
cannot read pyarrow's V2 pages of a `FIXED_LEN_BYTE_ARRAY` column (a decimal) under NULL lists:
it checks the values against the page header's `num_nulls`, which pyarrow writes without the
NULL list ancestors (`tests/test_properties.py` pins this).

## Writing: `COPY ... TO ... (FORMAT tparquet, ...)`

DuckDB 2.0's C API has COPY functions in its stable API (`duckdb_create_copy_function`,
`duckdb_register_copy_function`, stable since C API v1.5.6), so the extension registers the
COPY format `tparquet` with a bind, global init, sink and finalize callback. The file is
written with `tparquet::Writer`, whose `Auto` policy (spec section 6) compares PCODEC and
the base codec per numeric column on the first batch; the writer holds back the first 128Ki rows
(or 64 MiB, whichever comes first) so that this comparison sees more than one DuckDB vector.

| option | default | |
|---|---|---|
| `COLUMN_NAMES` | required (C API v1 build) | the column names, in order: `COLUMN_NAMES ['ts', 'price']`; the C API v2 build takes the query's names unless it is given |
| `POLICY` | `'auto'` | `'auto'`, `'pcodec'` or `'base'`, for every `INT32`/`INT64`/`FLOAT`/`DOUBLE` column |
| `LEVEL` | 8 | pcodec level, 0 to 12 |
| `COLUMN_POLICY` | | per column: `['price=pcodec', 'id=base']` |
| `COMPRESSION` | `'zstd'` | codec of the other columns: `zstd`, `snappy`, `gzip`, `brotli`, `lz4`, `uncompressed` |
| `COMPRESSION_LEVEL` | 3 for zstd | level of that codec |
| `ROW_GROUP_ROWS` | 1048576 | rows per row group |

**Why `COLUMN_NAMES`.** The C API gives a COPY function's bind the number and the *types* of the
columns (`duckdb_copy_function_bind_get_column_count/_type`) but not their names, although
DuckDB has them internally (`copy_function-c.cpp` keeps them in its bind info without a getter).
Rather than write `column0, column1, ...` silently, the COPY fails until the names are given.
The other alternatives are worse: the C API has no table in-out functions, and a function
taking a query string would run it on a second connection, which cannot see the caller's
temporary tables or open transaction (and, holding a connection, would keep the database open
forever). DuckDB's C API v2 has `duckdb_v2_copy_to_bind_get_column_name`, but no V2 function
is stable yet: a V2 extension loads only in the exact DuckDB version it was built for. The C API
v2 build (below) uses it: there `COLUMN_NAMES` is optional and only renames the columns (it is
still checked against the number of columns).

**`ROW_GROUP_ROWS`, not `ROW_GROUP_SIZE`.** DuckDB keeps `ROW_GROUP_SIZE` and
`ROW_GROUP_SIZE_BYTES` as options of its generic COPY machinery and does not pass them on.

**Partitions and per-thread files.** `PARTITION_BY` works; DuckDB hands the COPY function the
columns without the partition columns (unless `WRITE_PARTITION_COLUMNS true`), so
`COLUMN_NAMES` names those. The C API v1 cannot give a COPY function its file extension, so
`PARTITION_BY` and `PER_THREAD_OUTPUT` write `data_0.` unless `FILE_EXTENSION 'parquet'` is
given; the C API v2 build sets the extension `parquet` (`duckdb_v2_copy_function_set_file_extension`). `read_tparquet` has no `hive_partitioning`: the partition values are in `filename`.
`FILE_SIZE_BYTES` is refused by DuckDB for this format. A query that fails midway leaves no
file, and an existing file at the path is kept (DuckDB writes to a temporary file first).

**Local files only.** `COPY ... TO` writes with `std::fs` in both builds, not through DuckDB's
file system as `read_tparquet` reads: a remote destination (`s3://`, `https://`, a registered
file system) fails with `cannot create ...`. Write locally and upload, or write a standard
Parquet file with `FORMAT parquet` and transcode it.

**Order.** The C API sets no parallel execution mode for COPY functions, so DuckDB calls the
sink from one thread at a time, in the query's order; the conversion of DuckDB vectors to Arrow
runs in that thread.

**The C API v2 build.** `tparquet_init_c_api_v2` registers the same COPY format on DuckDB's C
API v2 (`package-extension --api v2`; it loads only in DuckDB 2.0.0), with the same options,
the same file and the same type rules: the batches DuckDB hands its batch callback (a column
data collection, scanned chunk by chunk) are converted by the same code as in the C API v1
build (`chunk::VectorView` over the C API v2 vectors). The C API v2 sets no parallel execution
mode either, so outside `PARTITION_BY` and `PER_THREAD_OUTPUT` DuckDB cuts the batches from one
thread in the query's order, and flushes them in that order; the writer holds back the first
rows for `Auto` as above. `COPY ... FROM` reads with the C API v2 scan of `read_tparquet`
(through DuckDB's Arrow importer) and checks the columns as below.

**Types.** Every type of the reading table can be written except `INTERVAL`; in addition
`ENUM` is written as strings, `TIMESTAMP_S` as milliseconds (Parquet has no seconds unit),
`TIMESTAMPTZ` as `timestamp(us, "UTC")`, lists with Parquet's element name `element` and maps
with `key_value`, so the file reads the same with and without its Arrow schema. `UUID`,
`HUGEINT`, `UHUGEINT`, `INTERVAL`, `UNION`, `BIT`, `TIME WITH TIME ZONE`, `BIGNUM`,
`VARIANT` and `GEOMETRY` are refused with the column's name: cast them first.

`COPY t FROM 'f.parquet' (FORMAT tparquet)` reads with `read_tparquet`; the file's
columns must match the table's by position and type (use `INSERT INTO t SELECT ... FROM
read_tparquet(...)` to map or cast). Its options are the flags of `read_tparquet`
(`FILENAME`, `FILE_ROW_NUMBER`, `UNION_BY_NAME`).

One more finding: DuckDB's built-in Parquet writer does not keep NaN payloads (the conformance
vector's `0x7ff0000000000001` comes out as `0xfff8000000000000`); `tparquet` keeps them.

## Ratio

On 2,000,000 deterministic market-like ticks (millisecond timestamps with random gaps, a
random-walk price on a 0.01 grid, lot sizes in 0.001 steps, a side flag;
`tests/test_acceptance.py`), `COPY (FORMAT tparquet, POLICY 'auto', LEVEL 8)` writes
4,998,033 bytes, 0.633 of DuckDB's `COPY (FORMAT parquet, COMPRESSION zstd,
COMPRESSION_LEVEL 9)` at 7,894,952 bytes, and the file reads back bit for bit.

## Packaging

`make build` builds the `cdylib` and appends DuckDB's extension metadata
(`src/metadata.rs`, `src/bin/package-extension.rs`): the last 512 bytes of the file are eight
32-byte fields, stored last first (magic `4`, platform such as `linux_amd64`, the C API version
`v1.5.6`, the extension version `v0.1.0`, the ABI type `C_STRUCT`, three empty fields) and a
256-byte signature (zeros: unsigned), behind the header of a Wasm custom section named
`duckdb_signature`, byte for byte what `extension-ci-tools/scripts/append_extension_metadata.py`
writes. With ABI `C_STRUCT` DuckDB checks the C API version, not the DuckDB version, so the file
loads in any DuckDB that serves C API v1.5.6 or later v1.x. The entrypoint is
`tparquet_init_c_api`, so the file must be named `tparquet.duckdb_extension`.
`package-extension --api v2` writes the footer of the C API v2 build instead: ABI
`C_STRUCT_UNSTABLE` and DuckDB version `v2.0.0`, for which DuckDB calls
`tparquet_init_c_api_v2` and refuses any other DuckDB version.

## Signing

```sh
openssl genrsa -out private.pem 2048 && openssl rsa -in private.pem -pubout -out public.pem
make build sign KEY=private.pem      # signs build/tparquet.duckdb_extension and build/v2/...
```

`sign.sh` asks `package-extension signing-hash` for the digest DuckDB checks
(`ExtensionHelper::CheckExtensionSignature`: the SHA-256 of the SHA-256s of the file's 1 MiB
chunks, without the 256-byte signature), signs it with `openssl pkeyutl -pkeyopt digest:sha256`
(RSA 2048, PKCS#1 v1.5) and puts the signature in the footer (`package-extension
set-signature`). DuckDB 2.0 trusts the public key of an extension repository, so a signed build
loads without `allow_unsigned_extensions` when it is installed from a repository that names
the key. A repository is a directory or URL laid out as
`<prefix>/v2.0.0/<platform>/tparquet.duckdb_extension`:

```sql
SET allow_extension_repositories = 'allowed';
CREATE EXTENSION REPOSITORY tparquet WITH PREFIX 'https://example.com/ext'
    USING PUBLIC KEY '-----BEGIN PUBLIC KEY-----...-----END PUBLIC KEY-----';
INSTALL tparquet FROM tparquet;
LOAD tparquet FROM tparquet;
```

A plain `LOAD 'file'` (or a load without `FROM`) checks only DuckDB's own core and community
keys, so it still needs `allow_unsigned_extensions` for a file signed with another key.
`tests/test_signing.py` signs each build with a new key, installs and loads it from a local
repository with unsigned extensions disallowed, and checks that a tampered file, a file signed
with another key and an unsigned file are refused.

## Tests

```sh
uv sync --python 3.14          # duckdb/.venv: duckdb 2.0.0, pyarrow, pytest (uv.lock pins hashes)
make test                      # unit tests, then every SQL test on both builds cargo made
make sql                       # the SQL tests on both release builds
make thorough                  # the same with many more generated cases (minutes)
make mutation                  # cargo mutants on this crate
```

- Unit tests (`src/*.rs`): the type mapping both ways, array normalization, path expansion,
  bind and projection slots, the row-group queue (also across threads), COPY options, the
  writer, DuckDB vector layouts converted to Arrow (on vectors laid out in memory), the footer.
- `tests/sql.rs` packages the library this build produced and runs pytest on it, so `cargo
  test`, and every mutant, checks what DuckDB actually loads: once packaged as the C API v1
  build and once as the C API v2 build (`TPARQUET_DUCKDB_API=v1|v2` tells the tests which;
  where the builds differ, the tests assert each one's behavior). It also builds the
  `tparquet` CLI the tests transcode and `verify` with.
- `tests/test_conformance.py`: every `testdata/files/*.parquet` read with
  `read_tparquet`, fetched from DuckDB as Arrow and turned into canonical JSON
  (`tests/canonical.py`: float bits through `Array.view`, time-zoned timestamps back to the
  file's unit) equals `testdata/expected/*.jsonl` byte for byte.
- `tests/test_types.py`: every DuckDB type (with NULLs at every level of nesting) through COPY
  and back in order, and DuckDB's own Parquet copy of it through read_tparquet and read_parquet.
- `tests/test_queries.py`: projections, filters, limits, aggregates and joins over a PCODEC file
  give read_parquet's rows, with 1 and 4 threads, also for generated queries.
- `tests/test_properties.py` (Hypothesis): generated tables of every pyarrow type, nested, read
  as read_parquet reads them, through COPY and through the CLI's transcode. The profile is
  `$HYPOTHESIS_PROFILE`: `default`, `thorough` (`make thorough`) or `mutation` (few fixed
  examples, set by `make mutation`).
- `tests/test_hostile.py`: damaged and truncated files give errors that name them.
- `tests/test_pushdown.py`: a damaged row group shows whether it is read: the v2 build skips it
  for every filter its statistics rule out (each type whose statistics are read), the v1 build
  reads it; results equal read_parquet's, also for generated float filters with NaN, infinities
  and signed zeros; threads keep the file order.
- `tests/test_remote.py`: files of a registered fsspec file system (`memory://`) and over HTTP
  through httpfs (a local server with range requests): single files, lists, globs (v2), COPY
  FROM, errors naming the file. The HTTP tests skip only when httpfs cannot be installed.
- `tests/test_metadata.py`: tparquet_metadata reads footers only (it works on a file whose
  column data is damaged), in file order, also re-executed as a prepared statement.
- `tests/test_signing.py`: see [Signing](#signing).
- Unit tests of the shared parts: `src/prune.rs` (predicates against statistics, with a check
  of 20,000 random row groups that a row group with a matching row is never skipped, and the
  statistics of every type a file holds), `src/fs.rs` (file inputs), `src/footer.rs`.
- `tests/test_read.py`, `tests/test_write.py`, `tests/test_load.py`, `tests/test_acceptance.py`,
  and the sqllogictest-style `tests/sql/*.test` (run by `tests/test_sqllogic.py`, as DuckDB's
  C++ runner does not come with the Python package; `__TESTDATA__`, `__TEST_DIR__` and
  `__EXTENSION__` are replaced).

## Distribution

The extension is not submitted anywhere. Notes for doing so:

- **Community extensions.** DuckDB builds, signs and serves community extensions from
  [duckdb/community-extensions](https://github.com/duckdb/community-extensions): a pull request
  adds `extensions/tparquet/description.yml` naming the repository, a commit (`ref`), the
  language (`Rust`), the build system (`cargo`) and the maintainers. Their CI builds with
  [extension-ci-tools](https://github.com/duckdb/extension-ci-tools) for every platform, so the
  repository needs the layout of DuckDB's Rust template
  ([extension-template-rs](https://github.com/duckdb/extension-template-rs)): a top-level
  `Makefile` that includes extension-ci-tools' `makefiles/c_api_extensions/rust.Makefile`
  (with `extension-ci-tools` as a submodule pinned to a commit), `EXTENSION_NAME =
  tparquet`, `USE_UNSTABLE_C_API = 0` and `TARGET_DUCKDB_VERSION = v1.5.6` (the C API
  version), and a crate whose `cdylib` is the extension. Their tooling appends the same footer as
  `package-extension` (and signs it). The workspace's path dependencies (`rust/parquet`,
  `rust/tparquet`) must build from that checkout; the tests here use Python and need a
  `test` target that the community CI can run, or none.
- **Signing.** Either with one's own key, through an extension repository that names it
  ([Signing](#signing)), or by the community CI with DuckDB's community key, which a plain
  `LOAD` trusts. Unsigned builds load with `allow_unsigned_extensions` (or `duckdb -unsigned`).
  The community CI builds C API v1 extensions; the v2 build is for DuckDB 2.0.0 only.
- **Versions.** The footer's C API version `v1.5.6` lets the same build load in later DuckDB
  releases that still serve C API v1, on the same platform (one build per platform: `linux_amd64`,
  `linux_arm64`, `osx_amd64`, `osx_arm64`, `windows_amd64`).
