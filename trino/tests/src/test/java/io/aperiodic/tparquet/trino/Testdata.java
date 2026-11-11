package io.aperiodic.tparquet.trino;

import io.aperiodic.tparquet.TparquetReader;
import io.aperiodic.tparquet.TparquetWriter;
import org.apache.parquet.example.data.Group;
import org.apache.parquet.hadoop.ParquetFileReader;
import org.apache.parquet.hadoop.ParquetWriter;
import org.apache.parquet.hadoop.metadata.CompressionCodecName;
import org.apache.parquet.hadoop.metadata.FileMetaData;
import org.apache.parquet.io.LocalInputFile;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;

/** The conformance vectors of {@code testdata/} (system property {@code tparquet.testdata}), and standard copies of them. */
final class Testdata
{
    private Testdata() {}

    static Path dir()
    {
        String d = System.getProperty("tparquet.testdata");
        return Path.of(d == null ? "../../testdata" : d);
    }

    static Path file(String name)
    {
        return dir().resolve("files").resolve(name + ".parquet");
    }

    static Path expected(String name)
    {
        return dir().resolve("expected").resolve(name + ".jsonl");
    }

    /** The names of every {@code files/*.parquet}, sorted. */
    static List<String> vectorNames()
    {
        try (Stream<Path> s = Files.list(dir().resolve("files"))) {
            return s.map(p -> p.getFileName().toString()).filter(n -> n.endsWith(".parquet"))
                    .map(n -> n.substring(0, n.length() - ".parquet".length())).sorted().toList();
        }
        catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    /**
     * Whether Trino 484 cannot open a vector, whatever its codec: key_value_metadata's footer has a
     * key without a value, and ParquetMetadata.keyValueMetaData (upstream trino-parquet) collects
     * the pairs with toImmutableMap, which refuses a null value (a NullPointerException). Its
     * standard copy, which has the same pairs, fails the same way.
     */
    static boolean trinoRefuses(String name)
    {
        return name.equals("key_value_metadata");
    }

    /**
     * The columns of a vector that Trino reads as their stored bits taken as signed, whatever the
     * codec, with their width: Trino has no unsigned integers, and its reader does not look at an
     * integer annotation's signedness (ColumnReaderFactory), so the UINT_32 and UINT_64 values above
     * the signed range are negative numbers in Trino, in the standard copy too.
     */
    static Map<String, Integer> readAsSigned(String name)
    {
        return name.equals("integer_types") ? Map.of("u32", 32, "u64", 64) : Map.of();
    }

    /** The PCODEC vectors: all but the standard control file. */
    static List<String> pcodecVectorNames()
    {
        return vectorNames().stream().filter(n -> !n.equals("standard_control")).toList();
    }

    /**
     * A standard copy of a vector: the same rows, schema and key-value metadata (but the codec key),
     * written ZSTD by java/tparquet's writer (parquet-java), as java/tparquet-spark's tests make them.
     */
    static void standardCopy(Path vector, Path copy)
            throws IOException
    {
        FileMetaData meta;
        try (ParquetFileReader r = ParquetFileReader.open(new LocalInputFile(vector))) {
            meta = r.getFooter().getFileMetaData();
        }
        Map<String, String> kv = new HashMap<>(meta.getKeyValueMetaData());
        kv.remove("parquet.pcodec.version");
        try (TparquetReader in = TparquetReader.open(vector);
                ParquetWriter<Group> w = TparquetWriter.groups(copy, meta.getSchema(),
                        TparquetWriter.Options.defaults().withCodec(CompressionCodecName.ZSTD).withExtraMetadata(kv))) {
            for (Group g = in.read(); g != null; g = in.read()) {
                w.write(g);
            }
        }
    }
}
