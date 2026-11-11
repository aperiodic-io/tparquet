# tparquet for the JVM

Reading and writing tparquet files (Parquet with the PCODEC codec, [`spec/FORMAT.md`](../spec/FORMAT.md))
from Java and Spark, and anything else built on parquet-java.

| Module | Artifact | What |
|---|---|---|
| [`tparquet-jni`](tparquet-jni) | `io.aperiodic.tparquet:tparquet-jni` | the PCODEC payload codec over JNI ([`rust/jni`](../rust/jni)); no Parquet dependency |
| [`parquet-java`](parquet-java) | `org.apache.parquet:parquet-{format-structures,common,hadoop}:1.17.0-tparquet` | a small fork of parquet-java 1.17.0 that adds the codec; changes listed in [`FORK.md`](parquet-java/FORK.md) |
| [`parquet-java-1.18`](parquet-java-1.18) | `org.apache.parquet:parquet-{format-structures,common}:1.18.1-tparquet` | the codec id in parquet-java 1.18.1, for Trino 484 ([`../trino`](../trino)); changes in [`FORK.md`](parquet-java-1.18/FORK.md) |
| [`tparquet`](tparquet) | `io.aperiodic.tparquet:tparquet` | `TparquetWriter` (spec section 3's writer recommendations), `TparquetReader`, `Canonical` (rows as canonical JSON), `Inspect` (page modes), the `Vectors` CLI |
| [`tparquet-spark`](tparquet-spark) | `io.aperiodic.tparquet:tparquet-spark_2.13` | Spark 4.3: the `tparquet` data source |
| [`acceptance-upstream`](acceptance-upstream) | (tests) | unmodified parquet-java refuses every PCODEC file |
| [`parquet-java/upstream-tests`](parquet-java/upstream-tests) | (tests) | upstream parquet-java's own suite against the fork |

Trino has its own Parquet reader; see [`trino/`](../trino).

The modules target Java 11 (as parquet-java 1.17.0 does) and are built with JDK 21; the Spark
module targets 17. The JNI library is bundled in the `tparquet-jni` jar for linux-x86_64 (built by
`make -C java`); on other platforms build it with `cargo build --release -p tparquet-jni` and pass
`-Dtparquet.jni.library=/path/to/libtparquet_jni.so`, or put it on `java.library.path`.

## Java

[`INSTALL.md`](../INSTALL.md#java-parquet-java) has the steps for Maven and Gradle projects and
for platforms other than Linux x86_64.

```java
import io.aperiodic.tparquet.TparquetWriter;

// PCODEC for the INT32, INT64, FLOAT and DOUBLE columns, V2 pages, PLAIN values
try (ParquetWriter<Group> w = TparquetWriter.groups(path, schema, TparquetWriter.Options.defaults())) {
  w.write(group);
}
```

Any parquet-java reader reads tparquet files once the fork's jars replace upstream's:
`ParquetReader`, `ParquetFileReader`, Avro and Protobuf readers, Hive, Spark. Writers get PCODEC
from `parquet.compression=PCODEC` (or `withCompressionCodec(CompressionCodecName.PCODEC)`), with
`parquet.compression.codec.pcodec.level` (0..12, default 8) and `parquet.compression.codec.pcodec.decimal`
(default true). `TparquetWriter` also sets PLAIN encodings for the numeric columns, which parquet-java's
V2 writer would otherwise delta-encode (and PCODEC could then only ZSTD), and keeps NaN payloads,
which parquet-java's own PLAIN writer canonicalizes.

To use the fork in a Maven build, pin the three modules (they keep upstream's coordinates):

```xml
<dependencyManagement>
  <dependencies>
    <dependency><groupId>org.apache.parquet</groupId><artifactId>parquet-format-structures</artifactId><version>1.17.0-tparquet</version></dependency>
    <dependency><groupId>org.apache.parquet</groupId><artifactId>parquet-common</artifactId><version>1.17.0-tparquet</version></dependency>
    <dependency><groupId>org.apache.parquet</groupId><artifactId>parquet-hadoop</artifactId><version>1.17.0-tparquet</version></dependency>
  </dependencies>
</dependencyManagement>
```

## Spark

Spark 4.3 is built on parquet-java 1.17.0. With the fork in place of its parquet jars it reads
PCODEC files with every reader (vectorized, off-heap, row-based; `spark.read.parquet`, SQL tables),
and the `tparquet` data source writes them:

```sh
make -C java spark-jars      # java/target/spark-jars
```

replace `$SPARK_HOME/jars/parquet-{hadoop,common,format-structures}-1.17.0.jar` with the three
`-tparquet` jars from `java/target/spark-jars`, and add the other three (`tparquet-jni`, `tparquet`,
`tparquet-spark_2.13`). Then:

```python
df.write.format("tparquet").save("trades")                     # PCODEC, V2 pages, PLAIN numbers
df.write.format("tparquet").option("level", "12").partitionBy("day").save("trades")
spark.read.parquet("trades")                                     # or .format("tparquet")
spark.sql("CREATE TABLE trades USING tparquet LOCATION 'trades'")
```

Write options: `level`, `decimal`, `rowGroupSize`, `pageSize`, `outputTimestampType` (`INT96`,
`TIMESTAMP_MICROS` or `TIMESTAMP_MILLIS`). `compression` may only be `pcodec`. Timestamps are
written as `format("parquet")` writes them: the session's `spark.sql.parquet.outputTimestampType`
(Spark's default is INT96), with Spark's INT96 rebase (`spark.sql.parquet.int96RebaseModeInWrite`),
unless the `outputTimestampType` option overrides it. PCODEC does not model INT96, so INT96 columns
are the payload's ZSTD fallback; `outputTimestampType=TIMESTAMP_MICROS` (or the session setting)
writes INT64 timestamps, which PCODEC compresses.

## Testing

```sh
make -C java test                  # every module (installs, so the fork is tested as jars)
make -C java check-fork            # every JVM fork is upstream + FORK.patch: parquet-java 1.17.0 and
                                   # 1.18.1, trino-parquet 484 (scripts/check-fork-java.sh)
make -C java fork-upstream-tests   # upstream's suite: same outcome for every test, fork and upstream
make -C java mutants               # PIT, every mutant killed (the 1.18 fork's enums included)
make -C java jni-mutants           # cargo mutants on rust/jni, judged by the Java suite
make -C java vectors ARGS="check ../testdata"
```

- **Conformance:** every vector of `testdata/` read through the fork gives exactly its expected
  rows (`java/tparquet`, `ConformanceTest`); the payload vectors decode (or fail) as listed, and
  re-encode to the same bytes (`tparquet-jni`); Spark reads every vector, with each of its
  readers, exactly as a standard copy of the same rows (`tparquet-spark`). Spark's vectorized
  readers refuse any file whose footer has a key without a value, whatever its codec
  (`key_value_metadata`): there the PCODEC file fails as its standard copy does.
- **Cross-language:** the Rust reference reader reads the vectors the Java writer writes, and the
  Java CLI reads everyone's (`acceptance/cross/run.sh`).
- **Untrusted payloads:** the native methods check every range and buffer again, refuse aliasing
  buffers, and turn panics into exceptions; random, damaged and corrupt payloads (in files too)
  give errors, never a crash.
- **Upstream:** unmodified parquet-java refuses every PCODEC vector (`acceptance-upstream`), and
  upstream's own test suite gives the fork the outcomes it gives upstream.
- **Mutation testing:** PIT on the Java code and on the fork's changes; cargo mutants on the JNI
  crate.
