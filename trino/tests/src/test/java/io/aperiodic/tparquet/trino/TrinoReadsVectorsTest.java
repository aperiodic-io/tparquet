package io.aperiodic.tparquet.trino;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import io.trino.parquet.metadata.BlockMetadata;
import io.trino.parquet.metadata.ColumnChunkMetadata;
import io.trino.parquet.metadata.ParquetMetadata;
import org.apache.parquet.hadoop.metadata.CompressionCodecName;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.MethodSource;

import java.io.IOException;
import java.math.BigInteger;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

/**
 * Conformance through Trino's reader: every vector of testdata/ read by trino-parquet (the fork)
 * into Trino pages gives exactly the rows of a standard (ZSTD) copy read the same way, floats bit
 * for bit, and exactly its expected rows (testdata/expected, canonical JSON).
 */
class TrinoReadsVectorsTest
{
    @TempDir
    static Path standard;

    @BeforeAll
    static void standardCopies()
            throws IOException
    {
        for (String name : Testdata.vectorNames()) {
            Testdata.standardCopy(Testdata.file(name), standard.resolve(name + ".parquet"));
        }
    }

    static List<String> vectors()
    {
        return Testdata.vectorNames();
    }

    /** Trino's footer reader refuses a footer with a key without a value (Testdata.trinoRefuses). */
    static void assertTrinoRefuses(Path file)
    {
        assertThatThrownBy(() -> TrinoParquet.footer(file)).as(file.toString())
                .isInstanceOf(NullPointerException.class)
                .hasStackTraceContaining("io.trino.parquet.metadata.ParquetMetadata.keyValueMetaData");
    }

    @Test
    void everyVectorIsHere()
    {
        assertThat(vectors()).hasSizeGreaterThanOrEqualTo(13).contains("standard_control");
    }

    @ParameterizedTest(name = "{0}")
    @MethodSource("vectors")
    void readsAsTheStandardCopy(String name)
            throws IOException
    {
        if (Testdata.trinoRefuses(name)) {
            assertTrinoRefuses(Testdata.file(name));
            assertTrinoRefuses(standard.resolve(name + ".parquet"));
            return;
        }
        TrinoParquet.Table got = TrinoParquet.read(Testdata.file(name));
        TrinoParquet.Table want = TrinoParquet.read(standard.resolve(name + ".parquet"));
        assertThat(got.types()).isEqualTo(want.types());
        assertThat(got.names()).isEqualTo(want.names());
        assertThat(got.rows()).isEqualTo(want.rows());
        assertThat(got.lines()).as(name).containsExactlyElementsOf(want.lines());
    }

    @ParameterizedTest(name = "{0}")
    @MethodSource("vectors")
    void readsTheExpectedRows(String name)
            throws IOException
    {
        if (Testdata.trinoRefuses(name)) {
            assertTrinoRefuses(Testdata.file(name));
            return;
        }
        List<String> got = TrinoParquet.lines(Testdata.file(name));
        List<String> want = Files.readAllLines(Testdata.expected(name));
        Map<String, Integer> signed = Testdata.readAsSigned(name);
        if (!signed.isEmpty()) {
            // those columns hold the stored bits taken as signed; the others are exactly as expected
            assertThat(got).hasSameSizeAs(want);
            ObjectMapper json = new ObjectMapper();
            for (int i = 0; i < got.size(); i++) {
                ObjectNode g = (ObjectNode) json.readTree(got.get(i));
                ObjectNode w = (ObjectNode) json.readTree(want.get(i));
                for (Map.Entry<String, Integer> c : signed.entrySet()) {
                    JsonNode gv = g.remove(c.getKey());
                    JsonNode wv = w.remove(c.getKey());
                    assertThat(gv.isNull()).as("%s line %s %s", name, i + 1, c.getKey()).isEqualTo(wv.isNull());
                    if (!wv.isNull()) {
                        BigInteger stored = wv.bigIntegerValue();
                        assertThat(gv.bigIntegerValue()).as("%s line %s %s", name, i + 1, c.getKey())
                                .isEqualTo(stored.testBit(c.getValue() - 1) ? stored.subtract(BigInteger.ONE.shiftLeft(c.getValue())) : stored);
                    }
                }
                got.set(i, json.writeValueAsString(g));
                want.set(i, json.writeValueAsString(w));
            }
        }
        assertThat(got).as(name).containsExactlyElementsOf(want);
    }

    @ParameterizedTest(name = "{0}")
    @MethodSource("vectors")
    void theFooterHasTheCodecOfEveryChunk(String name)
            throws IOException
    {
        if (Testdata.trinoRefuses(name)) {
            assertTrinoRefuses(Testdata.file(name));
            return;
        }
        // the footer through the parquet-java 1.18.1 fork: PCODEC is a codec name, not a refusal
        ParquetMetadata footer = TrinoParquet.footer(Testdata.file(name));
        boolean pcodec = false;
        for (BlockMetadata block : footer.getBlocks()) {
            for (ColumnChunkMetadata c : block.columns()) {
                pcodec |= c.getCodec() == CompressionCodecName.PCODEC;
            }
        }
        assertThat(pcodec).as(name).isEqualTo(!name.equals("standard_control") && !footer.getBlocks().isEmpty());
        if (pcodec) {
            assertThat(footer.getFileMetaData().getKeyValueMetaData()).containsEntry("parquet.pcodec.version", "1");
        }
    }
}
