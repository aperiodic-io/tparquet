# `tparquet`: a DuckDB extension for Parquet files with the PCODEC codec

DuckDB's own Parquet reader cannot read files whose column chunks use `CompressionCodec`
20547 (`PCODEC`, [`spec/FORMAT.md`](../spec/FORMAT.md)): it stops on the footer with
`TProtocolException: Invalid data` (its Thrift parser rejects the unknown codec value;
`tests/test_acceptance.py` pins this).

This extension **is DuckDB's own parquet extension**, with the PCODEC codec added and its
functions renamed. It is a small patch over each DuckDB release, not a new reader. The fork, every
changed line and why it changed are in [`FORK.md`](FORK.md). The reader, the writer, statistics
and filter pushdown, `count(*)` from the footer, globs, `union_by_name`, `hive_partitioning`,
remote files, encryption and the metadata functions are DuckDB's code, as DuckDB tests and ships
it.

```sql
LOAD 'build/v2.0.0/tparquet.duckdb_extension';   -- needs allow_unsigned_extensions, or a signed build

SELECT * FROM read_tparquet('trades/*.parquet');
SELECT symbol, avg(price) FROM read_tparquet('s3://bucket/trades/*.parquet', hive_partitioning := true) GROUP BY ALL;

COPY ticks TO 'ticks.parquet' (FORMAT tparquet);                       -- PCODEC, level 8
COPY ticks TO 'ticks.parquet' (FORMAT tparquet, COMPRESSION pcodec, COMPRESSION_LEVEL 12);
COPY ticks TO 'ticks.parquet' (FORMAT tparquet, COMPRESSION zstd);     -- a standard file
COPY ticks FROM 'ticks.parquet' (FORMAT tparquet);
```

## Builds: DuckDB 1.5.6 and 2.0.0

A C++ extension is built against one DuckDB version and loads only into that version on that
platform (its footer names both). There is one patch per version:

| DuckDB | patch | build |
|---|---|---|
| 1.5.6 | `1.5.6.patch` | `build/v1.5.6/tparquet.duckdb_extension` |
| 2.0.0 | `2.0.0.patch` | `build/v2.0.0/tparquet.duckdb_extension` |

[`INSTALL.md`](../INSTALL.md#duckdb-extension) has the steps for every platform and client.

```sh
make build                      # both versions (VERSIONS="2.0.0" for one); needs cmake, ninja, a C++ compiler, cargo
```

`build.sh VERSION` downloads DuckDB's source distribution from PyPI and checks its sha256. It
applies the patch, builds the codec's C ABI (`rust/capi`, linked statically), and builds the
extension with DuckDB's own CMake. The first build compiles DuckDB (it is linked into the
extension with its symbols hidden, as DuckDB builds the extensions it distributes); after that
the build is incremental (`target/duckdb/VERSION/build`).

## Functions

DuckDB already ships its parquet extension, and a function registered under an existing name
replaces DuckDB's. So every function has a name of its own: `parquet` becomes `tparquet`.
`read_parquet` and `COPY (FORMAT parquet)` stay DuckDB's, unchanged.

| tparquet | DuckDB's |
|---|---|
| `read_tparquet`, `tparquet_scan` | `read_parquet`, `parquet_scan` |
| `read_single_tparquet_file` (2.0.0) | `read_single_parquet_file` |
| `tparquet_metadata`, `tparquet_schema`, `tparquet_kv_metadata`, `tparquet_file_metadata`, `tparquet_bloom_probe`, `tparquet_full_metadata` | `parquet_metadata`, ... |
| `PRAGMA add_tparquet_key` | `add_parquet_key` |
| `COPY ... (FORMAT tparquet)` | `COPY ... (FORMAT parquet)` |

Their parameters and options are DuckDB's ([read_parquet](https://duckdb.org/docs/data/parquet/overview),
[COPY](https://duckdb.org/docs/sql/statements/copy)). The extension does not register a
replacement scan (`FROM 'f.parquet'` stays DuckDB's), the `variant` functions, or the ZSTD file
system: DuckDB's parquet extension has them already. Both extensions read the same settings
(`binary_as_string`, ...).

## Writing PCODEC

`COPY ... (FORMAT tparquet)` writes PCODEC by default:

- the `INT32`, `INT64`, `FLOAT` and `DOUBLE` columns (DuckDB's integers, floats, dates, times,
  timestamps, and decimals of up to 18 digits) get PCODEC, PLAIN encoding and no dictionary
  (spec, writer recommendations);
- every other column gets ZSTD at DuckDB's default level;
- the footer gets `parquet.pcodec.version = "1"` (spec section 2).

| option | default | |
|---|---|---|
| `COMPRESSION` | `pcodec` | DuckDB's codecs (`zstd`, `snappy`, `gzip`, `brotli`, `lz4`, `lz4_raw`, `uncompressed`) write standard files, as `FORMAT parquet` does |
| `COMPRESSION_LEVEL` | 8 for pcodec | pcodec's level, 0 to 12; ZSTD levels for zstd |

Every other COPY option (`ROW_GROUP_SIZE`, `PARTITION_BY`, `PER_THREAD_OUTPUT`, `FIELD_IDS`,
`ENCRYPTION_CONFIG`, ...) is DuckDB's. COPY writes through DuckDB's file system, so remote
destinations work as with `FORMAT parquet`.

NaN payloads: the PCODEC columns keep them bit for bit (PLAIN, without a dictionary). DuckDB's
`FORMAT parquet`, and `FORMAT tparquet` with another codec, keep one NaN per dictionary: the
conformance vector's `0x7ff0000000000001` comes out as DuckDB's own NaN.

On 2,000,000 market-like ticks (`tests/test_acceptance.py`), the PCODEC file is at most 0.63
of DuckDB's `COMPRESSION zstd, COMPRESSION_LEVEL 9` file, and it reads back bit for bit.

### Limitations

- **V1 data pages.** DuckDB's writer writes only `DATA_PAGE` (V1) pages, so a nullable column's
  page has its definition levels before its values. The payload encoder (spec section 4) gives
  pages with NULLs the ZSTD payload mode, not pcodec's. Columns without NULLs (also nullable
  ones) get pcodec's PCO or DECIMAL modes. Reading handles V1 and V2 pages either way.
- **DuckDB 1.5.6** has no `TIMESTAMP WITH TIME ZONE` in nanoseconds. Its reader, like its
  `read_parquet`, reads such a column as microseconds (the `temporal` conformance vector is an
  expected failure on 1.5.6). It also has no `read_single_tparquet_file`, and it prunes fewer
  filters by statistics than 2.0.0 (an `OR` of nested `AND`s, `VARCHAR` ranges, `IS NOT NULL`).

## Signing

```sh
openssl genrsa -out private.pem 2048 && openssl rsa -in private.pem -pubout -out public.pem
make build sign KEY=private.pem      # signs build/v1.5.6/... and build/v2.0.0/...
```

`sign.py` computes the digest DuckDB checks (`ExtensionHelper::CheckExtensionSignature`): the
SHA-256 of the SHA-256s of the file's 1 MiB chunks, without the 256-byte signature. It signs the
digest with `openssl pkeyutl -pkeyopt digest:sha256` (RSA 2048, PKCS#1 v1.5) and writes the
signature into the footer.

DuckDB 2.0.0 trusts the public key of an extension repository, so a signed build loads without
`allow_unsigned_extensions` when it is installed from a repository that names the key. A
repository is a directory or URL laid out as
`<prefix>/v2.0.0/<platform>/tparquet.duckdb_extension`:

```sql
SET allow_extension_repositories = 'allowed';
CREATE EXTENSION REPOSITORY tparquet WITH PREFIX 'https://example.com/ext'
    USING PUBLIC KEY '-----BEGIN PUBLIC KEY-----...-----END PUBLIC KEY-----';
INSTALL tparquet FROM tparquet;
LOAD tparquet FROM tparquet;
```

DuckDB 1.5.6 trusts only DuckDB's own keys (it has no repository with a key of its own), so a
1.5.6 build signed with another key still needs `allow_unsigned_extensions`. It is signed the
same way; the tests check its signature with `openssl` against the public key. A plain
`LOAD 'file'` checks only DuckDB's own keys in both versions.

## Tests

```sh
uv sync --python 3.14          # duckdb/.venv: pyarrow, pytest, hypothesis (uv.lock pins hashes)
make test                      # builds both versions and runs tests/ with each version's duckdb
make thorough                  # the same with many more generated cases (minutes)
make mutation                  # the scripted mutants of the patch (MUTATION_VERSION=2.0.0)
make check-fork                # the patches apply to the verified releases and match FORK.md
```

`FORK.md` (Tests) lists what each test file covers. In short:

- the conformance vectors read exactly as expected;
- what COPY writes (codecs, encodings, footer key, the payload modes) reads back, also in the
  Rust reference decoder;
- every DuckDB type, the queries, generated tables, pushdown, remote files and damaged files
  behave over `read_tparquet` as over DuckDB's own Parquet;
- signing works as described above.

Tests that differ between the versions say so (`conftest.DUCKDB_1`).
