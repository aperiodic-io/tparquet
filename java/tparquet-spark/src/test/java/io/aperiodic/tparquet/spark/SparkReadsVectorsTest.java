package io.aperiodic.tparquet.spark;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import io.aperiodic.tparquet.TparquetWriter;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;
import org.apache.parquet.example.data.Group;
import org.apache.parquet.hadoop.ParquetFileReader;
import org.apache.parquet.hadoop.ParquetWriter;
import org.apache.parquet.hadoop.metadata.CompressionCodecName;
import org.apache.parquet.hadoop.metadata.FileMetaData;
import org.apache.parquet.io.LocalInputFile;
import org.apache.spark.sql.Dataset;
import org.apache.spark.sql.Row;
import org.apache.spark.sql.SparkSession;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.Arguments;
import org.junit.jupiter.params.provider.MethodSource;

/**
 * Spark reads every conformance vector through the parquet-java fork, with each of its readers,
 * exactly as it reads a standard (ZSTD) copy of the same rows.
 */
class SparkReadsVectorsTest {
  @TempDir static Path standard;

  /** A standard copy of every vector: the same schema and key-value metadata, ZSTD. */
  @BeforeAll
  static void standardCopies() throws Exception {
    for (String name : Spark.vectorNames()) {
      Path src = Spark.testdata().resolve("files").resolve(name + ".parquet");
      FileMetaData meta;
      try (ParquetFileReader r = ParquetFileReader.open(new LocalInputFile(src))) {
        meta = r.getFooter().getFileMetaData();
      }
      Map<String, String> kv = new HashMap<>(meta.getKeyValueMetaData());
      kv.remove("parquet.pcodec.version");
      try (var in = io.aperiodic.tparquet.TparquetReader.open(src);
          ParquetWriter<Group> w = TparquetWriter.groups(standard.resolve(name + ".parquet"), meta.getSchema(),
              TparquetWriter.Options.defaults().withCodec(CompressionCodecName.ZSTD).withExtraMetadata(kv))) {
        for (Group g = in.read(); g != null; g = in.read()) {
          w.write(g);
        }
      }
    }
  }

  @AfterEach
  void resetConf() {
    SparkSession s = Spark.session();
    s.conf().unset("spark.sql.parquet.enableVectorizedReader");
    s.conf().unset("spark.sql.parquet.enableNestedColumnVectorizedReader");
    s.conf().unset("spark.sql.columnVector.offheap.enabled");
    s.conf().unset("spark.sql.legacy.parquet.nanosAsLong");
    s.conf().unset("spark.sql.timeType.enabled");
  }

  /** Spark's three ways of decoding Parquet. */
  enum Reader { VECTORIZED, VECTORIZED_OFF_HEAP, ROWS }

  static Stream<Arguments> cases() {
    return Spark.vectorNames().stream().flatMap(n -> Stream.of(Reader.values()).map(r -> Arguments.of(n, r)));
  }

  static Dataset<Row> read(Path file, Reader reader) {
    SparkSession s = Spark.session();
    boolean vectorized = reader != Reader.ROWS;
    s.conf().set("spark.sql.parquet.enableVectorizedReader", String.valueOf(vectorized));
    s.conf().set("spark.sql.parquet.enableNestedColumnVectorizedReader", String.valueOf(vectorized));
    s.conf().set("spark.sql.columnVector.offheap.enabled", String.valueOf(reader == Reader.VECTORIZED_OFF_HEAP));
    // Spark refuses TIMESTAMP(NANOS) columns (the temporal vector) unless it reads them as their longs
    s.conf().set("spark.sql.legacy.parquet.nanosAsLong", "true");
    // and TIME columns unless its TIME type is on
    s.conf().set("spark.sql.timeType.enabled", "true");
    return s.read().parquet(file.toString());
  }

  /**
   * Spark itself cannot read key_value_metadata with its vectorized readers, whatever the codec:
   * the footer has a key without a value ({@code no.value}), and
   * SpecificParquetRecordReaderBase.toSetMultiMap puts each value in a {@code Set.of}, which
   * refuses null. Its row reader reads the file. The PCODEC file must fail as its standard copy does.
   */
  static boolean sparkRefuses(String name, Reader reader) {
    return name.equals("key_value_metadata") && reader != Reader.ROWS;
  }

  @Test
  void everyVectorIsCopied() {
    assertThat(Spark.vectorNames()).hasSizeGreaterThanOrEqualTo(13);
  }

  @ParameterizedTest(name = "{0} {1}")
  @MethodSource("cases")
  void readsAsTheStandardCopy(String name, Reader reader) {
    Path pcodec = Spark.testdata().resolve("files").resolve(name + ".parquet");
    if (sparkRefuses(name, reader)) {
      for (Path f : List.of(pcodec, standard.resolve(name + ".parquet"))) {
        assertThatThrownBy(() -> Spark.rows(read(f, reader))).as(f.toString())
            .rootCause().isInstanceOf(NullPointerException.class)
            .hasStackTraceContaining("SpecificParquetRecordReaderBase.toSetMultiMap");
      }
      return;
    }
    Dataset<Row> got = read(pcodec, reader);
    Dataset<Row> want = read(standard.resolve(name + ".parquet"), reader);
    assertThat(got.schema()).isEqualTo(want.schema());
    List<String> rows = Spark.rows(got);
    assertThat(rows).as(name).isEqualTo(Spark.rows(want));
    assertThat(rows.size()).isEqualTo(Spark.rows(want).size());
  }

  @Test
  void theDataSourceNameReadsToo() {
    Path pcodec = Spark.testdata().resolve("files").resolve("mixed.parquet");
    Dataset<Row> df = Spark.session().read().format("tparquet").load(pcodec.toString());
    assertThat(Spark.rows(df)).isEqualTo(Spark.rows(read(standard.resolve("mixed.parquet"), Reader.VECTORIZED)));
  }
}
