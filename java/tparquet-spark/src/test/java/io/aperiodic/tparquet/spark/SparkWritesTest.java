package io.aperiodic.tparquet.spark;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import io.aperiodic.tparquet.Inspect;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;
import org.apache.parquet.hadoop.ParquetFileReader;
import org.apache.parquet.hadoop.metadata.BlockMetaData;
import org.apache.parquet.hadoop.metadata.ColumnChunkMetaData;
import org.apache.parquet.hadoop.metadata.CompressionCodecName;
import org.apache.parquet.hadoop.metadata.ParquetMetadata;
import org.apache.parquet.io.LocalInputFile;
import org.apache.spark.sql.Dataset;
import org.apache.spark.sql.Row;
import org.apache.spark.sql.SaveMode;
import org.apache.spark.sql.SparkSession;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;
import org.junit.jupiter.params.provider.ValueSource;

/** The tparquet data source writes PCODEC files that Spark (and its parquet source) reads back. */
class SparkWritesTest {
  @TempDir Path dir;

  static Dataset<Row> trades(long n) {
    return Spark.session().range(n).selectExpr(
        "id",
        "timestamp_millis(1735689600000 + id * 37) as ts",
        "cast(id % 7 as int) as venue",
        // double literals: 100.0 alone would be a DECIMAL in Spark SQL
        "case when id % 11 = 0 then null else cast(1000 + (id * 7919 % 977) / 100.0D as double) end as price",
        "cast((id % 300) / 4.0 as float) as qty",
        "case when id % 13 = 0 then null else concat('S', cast(id % 5 as string)) end as sym",
        "id % 2 = 0 as buy",
        "array(id, id + 1, case when id % 3 = 0 then null else id + 2 end) as levels",
        "named_struct('bid', 100.0D + id / 8.0D, 'ask', 100.25D + id / 8.0D) as quote",
        "map('a', cast(id as int)) as tags",
        "cast(id * 3 as decimal(18, 4)) as amount",
        "date_add(date'2025-01-01', cast(id % 50 as int)) as day");
  }

  static List<Path> parquetFiles(Path out) throws IOException {
    try (Stream<Path> s = Files.walk(out)) {
      return s.filter(p -> p.getFileName().toString().endsWith(".parquet")).sorted().toList();
    }
  }

  static ParquetMetadata footer(Path file) throws IOException {
    try (ParquetFileReader r = ParquetFileReader.open(new LocalInputFile(file))) {
      return r.getFooter();
    }
  }

  @Test
  void writesPcodecFilesThatReadBack() throws IOException {
    Dataset<Row> df = trades(20_000);
    Path out = dir.resolve("t");
    df.write().format("tparquet").save(out.toString());
    List<Path> files = parquetFiles(out);
    assertThat(files).isNotEmpty().allMatch(p -> p.getFileName().toString().endsWith(".pcodec.parquet"));
    for (Path f : files) {
      ParquetMetadata footer = footer(f);
      for (BlockMetaData b : footer.getBlocks()) {
        for (ColumnChunkMetaData c : b.getColumns()) {
          assertThat(c.getCodec()).isSameAs(CompressionCodecName.PCODEC);
        }
      }
      assertThat(footer.getFileMetaData().getKeyValueMetaData())
          .containsEntry("parquet.pcodec.version", "1").containsKey("org.apache.spark.version");
    }
    List<String> want = Spark.rows(df.orderBy("id"));
    assertThat(Spark.rows(Spark.session().read().format("tparquet").load(out.toString()).orderBy("id"))).isEqualTo(want);
    assertThat(Spark.rows(Spark.session().read().parquet(out.toString()).orderBy("id"))).isEqualTo(want);
  }

  /** Spark's nullable numeric columns are PCODEC's own modes: V2 pages, PLAIN values (spec section 3). */
  @Test
  void numericColumnsAreEncodedForTheirType() throws IOException {
    Path out = dir.resolve("t");
    // INT64 timestamps (Spark's default INT96 is not PCODEC's: int64TimestampsArePcodecAndInt96IsTheZstdFallback)
    trades(50_000).coalesce(1).write().format("tparquet").option("outputTimestampType", "TIMESTAMP_MICROS").save(out.toString());
    Map<String, List<String>> modes = Inspect.modes(parquetFiles(out).get(0));
    assertThat(modes.get("id")).as(modes.toString()).containsOnly("DATA_PAGE_V2:PCO");
    assertThat(modes.get("ts")).containsOnly("DATA_PAGE_V2:PCO");
    assertThat(modes.get("venue")).containsOnly("DATA_PAGE_V2:PCO");
    assertThat(modes.get("day")).containsOnly("DATA_PAGE_V2:PCO");
    assertThat(modes.get("amount")).containsOnly("DATA_PAGE_V2:PCO");
    assertThat(modes.get("price")).isSubsetOf("DATA_PAGE_V2:DECIMAL", "DATA_PAGE_V2:PCO").contains("DATA_PAGE_V2:DECIMAL");
    assertThat(modes.get("quote.bid")).isSubsetOf("DATA_PAGE_V2:DECIMAL", "DATA_PAGE_V2:PCO");
    assertThat(modes.get("levels.list.element")).containsOnly("DATA_PAGE_V2:PCO");
  }

  /** The session's settings the timestamp tests change: the session is shared by every test. */
  @AfterEach
  void resetTimestampConf() {
    Spark.session().conf().unset(OUTPUT_TIMESTAMP_TYPE);
    Spark.session().conf().unset(INT96_REBASE_MODE);
  }

  static final String OUTPUT_TIMESTAMP_TYPE = "spark.sql.parquet.outputTimestampType";
  static final String INT96_REBASE_MODE = "spark.sql.parquet.int96RebaseModeInWrite";

  /** The physical type and logical annotation of the {@code ts} column of the one file in {@code out}. */
  static String timestampType(Path out) throws IOException {
    var ts = footer(parquetFiles(out).get(0)).getFileMetaData().getSchema().getType("ts").asPrimitiveType();
    return ts.getPrimitiveTypeName() + " " + ts.getLogicalTypeAnnotation();
  }

  /**
   * Writes {@code df} with Spark's Parquet source and with tparquet (with {@code options}): the
   * timestamp column has the same type in both, and both read back as {@code df}. Returns the type.
   */
  String sameTimestampsAsSparksParquet(Dataset<Row> df, Map<String, String> options) throws IOException {
    Path parquet = Files.createTempDirectory(dir, "parquet").resolve("t");
    Path tparquet = Files.createTempDirectory(dir, "tparquet").resolve("t");
    df.coalesce(1).write().parquet(parquet.toString());
    df.coalesce(1).write().format("tparquet").options(options).save(tparquet.toString());
    assertThat(timestampType(tparquet)).isEqualTo(timestampType(parquet));
    List<String> want = Spark.rows(Spark.session().read().parquet(parquet.toString()).orderBy("id"));
    assertThat(Spark.rows(Spark.session().read().parquet(tparquet.toString()).orderBy("id"))).isEqualTo(want);
    return timestampType(tparquet);
  }

  /** With Spark's defaults, tparquet writes Spark's default timestamp type: INT96 (Spark 4.3's SQLConf). */
  @Test
  void timestampsAreSparksDefaultInt96() throws IOException {
    assertThat(sameTimestampsAsSparksParquet(trades(1000), Map.of())).isEqualTo("INT96 null");
  }

  /** The session's spark.sql.parquet.outputTimestampType is honored, as Spark's Parquet source honors it. */
  @ParameterizedTest
  @CsvSource({
    "INT96, INT96 null",
    "TIMESTAMP_MICROS, 'INT64 TIMESTAMP(MICROS,true)'",
    "TIMESTAMP_MILLIS, 'INT64 TIMESTAMP(MILLIS,true)'"})
  void timestampsFollowTheSessionConf(String conf, String type) throws IOException {
    Spark.session().conf().set(OUTPUT_TIMESTAMP_TYPE, conf);
    assertThat(sameTimestampsAsSparksParquet(trades(1000), Map.of())).isEqualTo(type);
  }

  /** The outputTimestampType option overrides the session's setting, in either direction. */
  @ParameterizedTest
  @CsvSource({
    "TIMESTAMP_MILLIS, TIMESTAMP_MICROS, 'INT64 TIMESTAMP(MICROS,true)'",
    "TIMESTAMP_MICROS, INT96, INT96 null",
    "INT96, TIMESTAMP_MILLIS, 'INT64 TIMESTAMP(MILLIS,true)'"})
  void theOutputTimestampTypeOptionOverridesTheSessionConf(String conf, String option, String type) throws IOException {
    Spark.session().conf().set(OUTPUT_TIMESTAMP_TYPE, conf);
    Path out = dir.resolve("t");
    Dataset<Row> df = trades(1000);
    df.coalesce(1).write().format("tparquet").option("outputTimestampType", option).save(out.toString());
    assertThat(timestampType(out)).isEqualTo(type);
    assertThat(Spark.rows(Spark.session().read().parquet(out.toString()).orderBy("id"))).isEqualTo(Spark.rows(df.orderBy("id")));
    // and it is what Spark's Parquet source writes with that session setting
    Spark.session().conf().set(OUTPUT_TIMESTAMP_TYPE, option);
    assertThat(sameTimestampsAsSparksParquet(df, Map.of())).isEqualTo(type);
  }

  /** INT64 timestamps are PCODEC's own; INT96, which PCODEC does not model, is the payload's ZSTD fallback. */
  @Test
  void int64TimestampsArePcodecAndInt96IsTheZstdFallback() throws IOException {
    Dataset<Row> df = trades(50_000).coalesce(1);
    df.write().format("tparquet").option("outputTimestampType", "TIMESTAMP_MICROS").save(dir.resolve("micros").toString());
    df.write().format("tparquet").save(dir.resolve("int96").toString());
    assertThat(Inspect.modes(parquetFiles(dir.resolve("micros")).get(0)).get("ts")).containsOnly("DATA_PAGE_V2:PCO");
    assertThat(Inspect.modes(parquetFiles(dir.resolve("int96")).get(0)).get("ts")).isNotEmpty()
        .allMatch(m -> m.endsWith(":ZSTD") || m.endsWith(":RAW")).anyMatch(m -> m.endsWith(":ZSTD"));
  }

  /**
   * Timestamps before the Gregorian switch (1582) and before 1900 go through Spark's INT96 rebase
   * as in Spark's own writer: the same files' rows and the same legacy-INT96 marker in the footer.
   */
  @ParameterizedTest
  @ValueSource(strings = {"CORRECTED", "LEGACY"})
  void ancientInt96TimestampsAreRebasedAsSparkDoes(String mode) throws IOException {
    Spark.session().conf().set(INT96_REBASE_MODE, mode);
    Dataset<Row> df = Spark.session().sql("SELECT * FROM VALUES "
        + "(0, timestamp'0001-01-01 00:00:00'), (1, timestamp'1000-03-01 12:34:56.789012'), "
        + "(2, timestamp'1582-10-04 23:59:59.999999'), (3, timestamp'1582-10-15 00:00:00'), "
        + "(4, timestamp'1899-12-31 23:59:59.999999'), (5, timestamp'1900-01-01 00:00:00'), "
        + "(6, timestamp'2025-01-01 00:00:00.000001'), (7, null) AS t(id, ts)");
    Path parquet = dir.resolve("parquet");
    Path tparquet = dir.resolve("tparquet");
    df.coalesce(1).write().parquet(parquet.toString());
    df.coalesce(1).write().format("tparquet").save(tparquet.toString());
    assertThat(timestampType(tparquet)).isEqualTo("INT96 null").isEqualTo(timestampType(parquet));
    String legacy = "org.apache.spark.legacyINT96";
    assertThat(footer(parquetFiles(tparquet).get(0)).getFileMetaData().getKeyValueMetaData().containsKey(legacy))
        .isEqualTo(mode.equals("LEGACY"))
        .isEqualTo(footer(parquetFiles(parquet).get(0)).getFileMetaData().getKeyValueMetaData().containsKey(legacy));
    List<String> want = Spark.rows(df.orderBy("id"));
    assertThat(Spark.rows(Spark.session().read().parquet(parquet.toString()).orderBy("id"))).isEqualTo(want);
    assertThat(Spark.rows(Spark.session().read().parquet(tparquet.toString()).orderBy("id"))).isEqualTo(want);
  }

  @Test
  void theLevelOptionReachesTheEncoder() throws IOException {
    Dataset<Row> df = trades(50_000).coalesce(1);
    df.write().format("tparquet").option("level", "0").save(dir.resolve("l0").toString());
    df.write().format("tparquet").option("level", "12").save(dir.resolve("l12").toString());
    assertThat(Files.size(parquetFiles(dir.resolve("l0")).get(0))).isGreaterThan(Files.size(parquetFiles(dir.resolve("l12")).get(0)));
  }

  @Test
  void badOptionsAreRefused() {
    Dataset<Row> df = trades(10);
    assertThatThrownBy(() -> df.write().format("tparquet").option("level", "13").save(dir.resolve("a").toString()))
        .hasStackTraceContaining("level 13 outside 0..12");
    assertThatThrownBy(() -> df.write().format("tparquet").option("compression", "zstd").save(dir.resolve("b").toString()))
        .hasStackTraceContaining("tparquet writes PCODEC: compression=zstd is not supported");
    df.write().format("tparquet").option("compression", "pcodec").save(dir.resolve("c").toString());
  }

  @Test
  void partitionedAppendsAndSql() throws IOException {
    SparkSession s = Spark.session();
    Path out = dir.resolve("p");
    trades(3000).write().format("tparquet").partitionBy("venue").save(out.toString());
    trades(3000).selectExpr("id + 3000 as id", "ts", "venue", "price", "qty", "sym", "buy", "levels", "quote", "tags", "amount", "day")
        .write().format("tparquet").mode(SaveMode.Append).partitionBy("venue").save(out.toString());
    assertThat(parquetFiles(out)).allMatch(p -> p.getParent().getFileName().toString().startsWith("venue="));
    s.sql("CREATE TABLE trades_t USING tparquet LOCATION '" + out + "'");
    try {
      s.sql("ALTER TABLE trades_t RECOVER PARTITIONS");
      assertThat(s.sql("SELECT count(*), count(distinct venue), sum(id) FROM trades_t").first().toString())
          .isEqualTo("[6000,7,17997000]");
      assertThat(s.sql("SELECT venue, count(*) FROM trades_t WHERE venue = 3 GROUP BY venue").first().toString())
          .isEqualTo("[3,858]");
      s.sql("CREATE TABLE trades_copy USING tparquet AS SELECT * FROM trades_t WHERE price > 1005");
      long n = s.sql("SELECT count(*) FROM trades_t WHERE price > 1005").first().getLong(0);
      assertThat(s.sql("SELECT count(*) FROM trades_copy").first().getLong(0)).isEqualTo(n).isPositive();
      s.sql("INSERT INTO trades_copy SELECT * FROM trades_t WHERE id < 10");
      assertThat(s.sql("SELECT count(*) FROM trades_copy").first().getLong(0)).isEqualTo(n + 10);
    } finally {
      s.sql("DROP TABLE IF EXISTS trades_t");
      s.sql("DROP TABLE IF EXISTS trades_copy");
    }
  }

  /** Row-group statistics of PCODEC files still prune: a selective filter reads fewer rows. */
  @Test
  void filtersPruneRowGroups() throws IOException {
    SparkSession s = Spark.session();
    Path out = dir.resolve("pruned");
    trades(200_000).coalesce(1).write().format("tparquet").option("rowGroupSize", String.valueOf(64 * 1024)).save(out.toString());
    assertThat(footer(parquetFiles(out).get(0)).getBlocks()).hasSizeGreaterThan(4);
    Dataset<Row> q = s.read().parquet(out.toString()).where("id >= 190000");
    assertThat(q.count()).isEqualTo(10_000);
    // the scan read far fewer rows than the file has
    Dataset<Row> counted = s.read().parquet(out.toString()).where("id >= 190000");
    counted.collect();
    long scanned = scannedRows(counted);
    assertThat(scanned).isLessThan(100_000).isGreaterThanOrEqualTo(10_000);
  }

  static long scannedRows(Dataset<Row> df) {
    var plan = df.queryExecution().executedPlan();
    List<Long> rows = new ArrayList<>();
    plan.foreach(node -> {
      if (node.nodeName().contains("Scan")) {
        var m = node.metrics().get("numOutputRows");
        if (m.isDefined()) {
          rows.add((Long) m.get().value());
        }
      }
      return null;
    });
    return rows.stream().mapToLong(Long::longValue).sum();
  }

  /** A corrupt PCODEC payload fails the task with the codec's message, never a crash. */
  @Test
  void aCorruptFileIsAnError() throws IOException {
    Path out = dir.resolve("c");
    trades(5000).coalesce(1).write().format("tparquet").save(out.toString());
    Path f = parquetFiles(out).get(0);
    byte[] bytes = Files.readAllBytes(f);
    // the first page payload of the first column: after its page header, the version byte
    long start = footer(f).getBlocks().get(0).getColumns().get(0).getStartingPos();
    int at = (int) start;
    while (bytes[at] != 1 || bytes[at + 1] < 0 || bytes[at + 1] > 3) {
      at++;
    }
    bytes[at] = 9;
    Files.write(f, bytes);
    Files.deleteIfExists(f.resolveSibling("." + f.getFileName() + ".crc"));
    assertThatThrownBy(() -> Spark.session().read().parquet(out.toString()).collect())
        .hasStackTraceContaining("pcodec payload: unsupported version 9");
  }
}
