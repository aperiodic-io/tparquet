# tparquet for Trino

Trino reads tparquet files (Parquet with the PCODEC codec, [`spec/FORMAT.md`](../spec/FORMAT.md))
through its Hive, Iceberg, Delta Lake and Hudi connectors once their plugin directories have three
forked jars and the codec:

| Jar | Replaces | What |
|---|---|---|
| `trino-parquet-484-tparquet.jar` | `trino-parquet-484.jar` | Trino's Parquet reader and writer with PCODEC ([`trino-parquet`](trino-parquet), changes in [`FORK.md`](trino-parquet/FORK.md)) |
| `parquet-format-structures-1.18.1-tparquet.jar` | `parquet-format-structures-1.18.1.jar` | the codec id in the footer's thrift enum ([`java/parquet-java-1.18`](../java/parquet-java-1.18/FORK.md)) |
| `parquet-common-1.18.1-tparquet.jar` | `parquet-common-1.18.1.jar` | `CompressionCodecName.PCODEC` (same fork) |
| `tparquet-jni-<version>.jar` | (added) | the codec over JNI ([`java/tparquet-jni`](../java/tparquet-jni)), with the native library for linux-x86_64 |

Unmodified Trino refuses PCODEC files at the footer ("Required field 'codec' was not present!"); it
never returns wrong data. Trino 484 is built on parquet-java 1.18.1, hence a fork of that version
next to the 1.17.0 one Spark uses ([`java/parquet-java`](../java/parquet-java/FORK.md)).

## Installing into Trino 484

[`INSTALL.md`](../INSTALL.md#trino) also covers the JDKs, clusters and Docker images.

```sh
make -C trino plugin-jars                       # trino/plugin-jars/target/tparquet-jars
trino/install-tparquet.sh /usr/lib/trino        # the server's TRINO_HOME (it has plugin/)
# restart Trino
```

`install-tparquet.sh [--jars DIR] TRINO_HOME` finds every `plugin/*/` directory with a
`trino-parquet-*.jar`, checks that each is Trino 484's (trino-parquet 484, parquet-java 1.18.1)
before it changes anything, moves the three upstream jars to `TRINO_HOME/tparquet-backup/<plugin>/`
(outside `plugin/`: Trino loads every jar of a plugin directory) and copies the four jars in.
Running it again updates them; `install-tparquet.sh --uninstall TRINO_HOME` puts upstream's back.
`make -C trino install TRINO_HOME=...` does both steps.

Then any Parquet table of those connectors reads PCODEC files, e.g. Hive external tables over
directories of files written by any tparquet writer:

```sql
CREATE TABLE hive.market.trades (ts timestamp(3), price double, qty double, symbol varchar)
WITH (format = 'PARQUET', external_location = 's3://bucket/trades/');
SELECT symbol, avg(price) FROM hive.market.trades WHERE ts > TIMESTAMP '2025-01-01 00:00' GROUP BY 1;
```

Statistics and page indexes are standard Parquet (only the page bodies are PCODEC), so filters
skip row groups and pages as for any Parquet file.

**The native library.** Trino gives each plugin and each catalog its own class loader, so the
codec is loaded once per catalog: each copy extracts the library bundled in `tparquet-jni` to a
temporary file of its own (under `java.io.tmpdir`), which the JVM allows. Do not set
`-Dtparquet.jni.library` in `jvm.config` when more than one catalog uses these connectors: one
library file can be loaded by one class loader only (the second catalog fails with "already loaded
in another classloader"). On a platform other than linux-x86_64, build the library
(`cargo build --release -p tparquet-jni`) and put a `tparquet-jni` jar with it under
`io/aperiodic/tparquet/native/<os>-<arch>/` into the plugin directories, or use the property with a
single catalog.

**Writing.** SQL writes (`INSERT`, `CREATE TABLE AS`) keep the connectors' codecs: their codec
settings (`hive.compression-codec`, `iceberg.compression-codec`, ...) are enums of the connectors,
which are not forked. Trino's writer API (`io.trino.parquet.writer.ParquetWriter`) writes PCODEC
(`trino-parquet/FORK.md`, Writing). Write tparquet files with `java/tparquet`, the Rust, Go,
Python, Polars or DuckDB packages, or Spark.

## Building

The reactor builds with JDK 25 (Trino 484 needs it) and depends on `tparquet-jni`, the
parquet-java 1.18.1 fork and `tparquet` (for the tests' files) from [`java/`](../java), built with
JDK 21; `make -C trino java` installs them (and builds the JNI library with cargo).

| Module | Artifact | What |
|---|---|---|
| [`trino-parquet`](trino-parquet) | `io.trino:trino-parquet:484-tparquet` | the fork, vendored from the published sources jar; its unit tests |
| [`plugin-jars`](plugin-jars) | (none) | `target/tparquet-jars` (the four jars to install) and `target/plugin/hive` (Trino 484's Hive plugin directory as released, for the tests) |
| [`tests`](tests) | (tests) | Trino's reader and query engine on tparquet files |
| [`acceptance-upstream`](acceptance-upstream) | (tests) | unmodified trino-parquet refuses every PCODEC vector |

The dependency versions are Trino 484's own: `trino/pom.xml` imports `io.trino:trino-root:484`
as a BOM and pins the forks over it (`trino-parquet/FORK.md`, Build, says why it is not the
parent).

## Testing

```sh
make -C trino test          # every module (installs the java/ modules first)
make -C trino check-fork    # the forks are upstream + FORK.patch (scripts/check-fork-java.sh)
make -C trino mutants       # PIT on the fork's code, every mutant killed
```

- **The fork** (`trino-parquet`, 73 tests): every payload vector of `testdata/payloads` through
  `ParquetCompressionUtils.decompress` (valid ones decode exactly; invalid ones, other sizes,
  negative sizes and empty pages are `ParquetCorruptionException`s); jqwik properties over random
  and damaged payloads; the writer's compressor, type hints, encodings, footer key, and files
  written by Trino's `ParquetWriter` read back.
- **Conformance through Trino's reader** (`TrinoReadsVectorsTest`): every vector of `testdata/files`
  read by trino-parquet into Trino pages gives exactly the rows of a standard (ZSTD) copy of the
  same rows (written by `java/tparquet`) read the same way, floats bit for bit, and exactly its
  expected rows (`testdata/expected`, canonical JSON of the Trino values), with Trino's types
  for every column (timestamps of every unit and zone, dates, times, decimals, lists, structs,
  maps). Two things Trino 484 does not do, whatever the codec, are tested as such, on the vector
  and its standard copy alike: it reads `UINT_32` and `UINT_64` values as their bits taken as
  signed (`integer_types`' `u32` and `u64`, left out of its Hive table), and it cannot open a
  file whose footer has a key without a value (`key_value_metadata`, a `NullPointerException` in
  `ParquetMetadata.keyValueMetaData`).
- **End to end** (`TrinoQueriesTest`): Trino's query engine (`DistributedQueryRunner`) with the
  Hive connector, loaded from a plugin directory into which `install-tparquet.sh` installed the
  jars, the way the server loads plugins (`PluginManager.createClassLoader`); the file metastore
  and Trino's local file system. Over tables of every vector and of 600,000 generated trades in
  three files of several row groups (PCODEC, and a ZSTD copy): the vectors' rows equal their
  standard copies' (in order, as sets, and as float bits); counts, aggregates and checksums,
  `GROUP BY`, a join with a standard table and a join of the PCODEC table with its ZSTD copy on
  every column; a selective filter reads exactly the row groups its statistics cannot exclude
  (page indexes off) and fewer with them; a table with a corrupt payload fails its query ("Could
  not decompress page", caused by "Malformed Parquet file. Invalid PCODEC page: ..."), and the
  engine runs on.
- **Untrusted payloads**: `DamagedFilesTest` (jqwik) damages payload bytes, headers and modes of
  the vectors' PCODEC pages: rows or an exception, never an `Error`; header damage is always a
  `ParquetCorruptionException`.
- **Unmodified Trino**: `UnmodifiedTrinoRefusesPcodecTest` (the Hive plugin directory as
  released, before the install script) and `acceptance-upstream` (trino-parquet 484 on
  parquet-java 1.18.1) refuse every PCODEC vector; the standard control file reads.
- **The install script** (`InstallScriptTest`): on directories laid out like a Trino install
  (hive, iceberg, delta-lake, hudi, and connectors without Parquet) and on the real Hive plugin
  directory: what it replaces, adds and keeps, running it again, `--uninstall`, refusing other
  Trino or parquet-java versions without changing anything. `PluginClassLoadersTest`: two class
  loaders of an installed plugin directory each load the bundled library.
- `ForkJarsTest`: the reader runs on the forks (format-structures with thrift relocated, Java 11
  classes) and upstream's parquet-column 1.18.1.

## Mutation testing

PIT on the fork's new and changed code (`trino-parquet/pom.xml` lists the classes and methods and
says why the rest is excluded): 31 of 31 mutants killed. The parquet-java 1.18.1 fork's enums are
mutation-tested in `java/` (`make -C java mutants`): 10 of 10 and 14 of 14.
