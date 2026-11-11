package io.aperiodic.tparquet.trino;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.trino.parquet.metadata.BlockMetadata;
import io.trino.spi.Plugin;
import io.trino.testing.MaterializedRow;
import org.apache.parquet.hadoop.ParquetFileReader;
import org.apache.parquet.hadoop.metadata.CompressionCodecName;
import org.apache.parquet.io.LocalInputFile;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.MethodSource;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

/**
 * End to end: Trino's query engine with the Hive connector, loaded from a plugin directory into
 * which install-tparquet.sh put the forks (as on a Trino install), running SQL over directories of
 * PCODEC files: every conformance vector, and generated trades in several files of several row
 * groups, each compared with standard (ZSTD) copies of the same rows; joins with a standard table;
 * row groups skipped by their statistics; a corrupt payload is a query error.
 */
class TrinoQueriesTest
{
    @TempDir
    static Path tmp;
    static HiveRunner trino;
    static Plugin hive;
    static String jniLibrary;

    static final long TRADES = 600_000;
    static final long PER_FILE = 200_000;

    /**
     * The vectors' columns as Hive table columns. Hive has no TIME type: temporal's time64_us is left
     * out; nor does Trino have unsigned integers: integer_types' u32 and u64, which Trino reads as
     * signed (Testdata.readAsSigned), are left out.
     */
    static final Map<String, String> COLUMNS = new LinkedHashMap<>();

    static {
        String primitives = "i32 integer, i64 bigint, f32 real, f64 double";
        String trades = "ts timestamp(9), price double, qty double, symbol varchar, venue varchar, is_buy boolean, note varchar";
        String pages = "i32 integer, f64 double, i64_opt bigint, random bigint, tag varchar";
        COLUMNS.put("all_null_pages", "sparse bigint, tail double, none integer");
        COLUMNS.put("auto_choice", "ts timestamp(9), regime bigint, level double");
        COLUMNS.put("decimal_floats", "price_cents double, amount_1e8 double, f32_halves real, f32_tenths real, with_exceptions double");
        COLUMNS.put("deep_nesting", "legs array(row(id bigint, pts array(row(x double, y integer))))");
        COLUMNS.put("dictionary_fallback", "id bigint, px double, small integer");
        COLUMNS.put("empty", primitives);
        COLUMNS.put("float_specials", "f64 double, f32 real, f64_opt double");
        COLUMNS.put("integer_types", "i8 tinyint, i16 smallint, i32 integer, u8 smallint, u16 integer, i64 bigint, "
                + "dec1_0 decimal(1, 0), dec9_9 decimal(9, 9), dec10_2 decimal(10, 2), dec18_0 decimal(18, 0)");
        COLUMNS.put("key_value_metadata", primitives);
        COLUMNS.put("large_page", "digits array(integer)");
        COLUMNS.put("lists", "ints array(bigint), levels array(double)");
        COLUMNS.put("maps", "by_id map(integer, double), by_tag map(varchar, bigint)");
        COLUMNS.put("mixed", trades);
        COLUMNS.put("multiple_row_groups", trades);
        COLUMNS.put("nested_struct", "book row(id integer, quote row(bid double, ask double, size bigint), tags array(integer))");
        COLUMNS.put("pages_v1", pages);
        COLUMNS.put("pages_v2", pages);
        COLUMNS.put("payload_modes", "raw bigint, pco integer, decimal double, zstd varchar, zstd_levels bigint");
        COLUMNS.put("pcodec_levels", "level0 bigint, level1 double, level2 integer, level3 real, level4 bigint, level5 double, "
                + "level6 integer, level7 real, level8 bigint, level9 double, level10 integer, level11 real, level12 bigint");
        COLUMNS.put("primitives_optional", primitives);
        COLUMNS.put("primitives_required", primitives);
        COLUMNS.put("standard_control", trades);
        COLUMNS.put("statistics", primitives);
        COLUMNS.put("temporal", "ts_ms_utc timestamp(9), ts_us timestamp(9), ts_ns_utc timestamp(9), ts_ms_ny timestamp(9), date32 date, dec18_4 decimal(18, 4), dec9_2 decimal(9, 2)");
    }

    @BeforeAll
    static void start()
            throws Exception
    {
        Path home = TrinoInstall.home(tmp.resolve("trino"));
        TrinoInstall.install(home);

        Path data = Files.createDirectories(tmp.resolve("data"));
        for (String name : Testdata.vectorNames()) {
            Files.copy(Testdata.file(name), Files.createDirectories(data.resolve("vectors").resolve(name)).resolve(name + ".parquet"));
            Testdata.standardCopy(Testdata.file(name), Files.createDirectories(data.resolve("standard").resolve(name)).resolve(name + ".parquet"));
        }
        Path pcodec = Files.createDirectories(data.resolve("trades_pcodec"));
        Path zstd = Files.createDirectories(data.resolve("trades_zstd"));
        for (long from = 0; from < TRADES; from += PER_FILE) {
            Trades.write(pcodec.resolve("part-" + from + ".parquet"), from, from + PER_FILE, CompressionCodecName.PCODEC);
            Trades.write(zstd.resolve("part-" + from + ".parquet"), from, from + PER_FILE, CompressionCodecName.ZSTD);
        }
        Trades.writeSymbols(Files.createDirectories(data.resolve("symbols")).resolve("symbols.parquet"));
        Path corrupt = Files.createDirectories(data.resolve("corrupt"));
        Corrupt.damageFirstPayload(pcodec.resolve("part-0.parquet"), "price", corrupt.resolve("part-0.parquet"));

        // as on a Trino install: the plugin's tparquet-jni loads its bundled library, not this JVM's
        // tparquet.jni.library (which the test class path's copy may have loaded already)
        jniLibrary = System.clearProperty("tparquet.jni.library");
        hive = TrinoInstall.hivePlugin(home);
        trino = new HiveRunner(hive, data);
        for (Map.Entry<String, String> e : COLUMNS.entrySet()) {
            trino.table("v_" + e.getKey(), e.getValue(), "vectors/" + e.getKey());
            trino.table("s_" + e.getKey(), e.getValue(), "standard/" + e.getKey());
        }
        trino.table("trades_pcodec", Trades.COLUMNS, "trades_pcodec");
        trino.table("trades_zstd", Trades.COLUMNS, "trades_zstd");
        trino.table("symbols", "symbol varchar, sector varchar", "symbols");
        trino.table("trades_corrupt", Trades.COLUMNS, "corrupt");
    }

    @AfterAll
    static void stop()
    {
        if (trino != null) {
            trino.close();
        }
        if (jniLibrary != null) {
            System.setProperty("tparquet.jni.library", jniLibrary);
        }
    }

    static Stream<String> vectors()
    {
        return COLUMNS.keySet().stream();
    }

    /** A vector's table columns, name and type, split at the top-level commas of its COLUMNS entry. */
    static List<String[]> columns(String name)
    {
        List<String[]> out = new ArrayList<>();
        String columns = COLUMNS.get(name);
        int depth = 0;
        int start = 0;
        for (int i = 0; i <= columns.length(); i++) {
            char c = i < columns.length() ? columns.charAt(i) : ',';
            if (c == '(') {
                depth++;
            }
            else if (c == ')') {
                depth--;
            }
            else if (c == ',' && depth == 0) {
                out.add(columns.substring(start, i).trim().split(" ", 2));
                start = i + 1;
            }
        }
        return out;
    }

    /** The select list of a vector's table for set operations: Trino cannot order maps, so a map is its entries. */
    static String setColumns(String name)
    {
        List<String> out = new ArrayList<>();
        for (String[] c : columns(name)) {
            out.add(c[1].startsWith("map(") ? "map_entries(" + c[0] + ") AS " + c[0] : c[0]);
        }
        return String.join(", ", out);
    }

    /** Trino refuses the vector and its standard copy alike, when it opens the split (Testdata.trinoRefuses). */
    static void assertTrinoRefuses(String name)
    {
        for (String table : List.of("v_" + name, "s_" + name)) {
            assertThatThrownBy(() -> trino.execute("SELECT count(*) FROM " + table)).as(table)
                    .hasMessageStartingWith("Error opening Hive split ")
                    .hasStackTraceContaining("java.lang.NullPointerException")
                    .hasStackTraceContaining("io.trino.parquet.metadata.ParquetMetadata.keyValueMetaData");
        }
    }

    static Map<String, Long> manifestRows()
            throws IOException
    {
        Map<String, Long> out = new HashMap<>();
        JsonNode m = new ObjectMapper().readTree(Testdata.dir().resolve("files").resolve("manifest.json").toFile());
        for (JsonNode f : m.get("files")) {
            out.put(f.get("name").asText(), f.get("rows").asLong());
        }
        return out;
    }

    @Test
    void everyVectorHasATable()
    {
        assertThat(COLUMNS.keySet()).containsExactlyInAnyOrderElementsOf(Testdata.vectorNames());
    }

    @Test
    void everyTableColumnIsATopLevelColumnOfItsVector()
            throws IOException
    {
        for (String name : COLUMNS.keySet()) {
            List<String> fields;
            try (ParquetFileReader r = ParquetFileReader.open(new LocalInputFile(Testdata.file(name)))) {
                fields = r.getFooter().getFileMetaData().getSchema().getFields().stream().map(org.apache.parquet.schema.Type::getName).toList();
            }
            assertThat(columns(name).stream().map(c -> c[0]).toList()).as(name).isNotEmpty().isSubsetOf(fields);
        }
    }

    @ParameterizedTest(name = "{0}")
    @MethodSource("vectors")
    void everyVectorQueriesAsItsStandardCopy(String name)
            throws IOException
    {
        if (Testdata.trinoRefuses(name)) {
            assertTrinoRefuses(name);
            return;
        }
        long rows = manifestRows().get(name);
        assertThat(trino.value("SELECT count(*) FROM v_" + name)).isEqualTo(rows);
        assertThat(trino.value("SELECT count(*) FROM s_" + name)).isEqualTo(rows);
        // one file, one split: the rows in file order
        List<MaterializedRow> got = trino.rows("SELECT * FROM v_" + name);
        assertThat(got).hasSize((int) rows).containsExactlyElementsOf(trino.rows("SELECT * FROM s_" + name));
        // and as sets, the way SQL compares them
        String select = "SELECT " + setColumns(name) + " FROM ";
        assertThat(trino.rows(select + "v_" + name + " EXCEPT ALL " + select + "s_" + name)).isEmpty();
        assertThat(trino.rows(select + "s_" + name + " EXCEPT ALL " + select + "v_" + name)).isEmpty();
    }

    @ParameterizedTest(name = "{0}")
    @MethodSource("vectors")
    void floatsAreTheirBits(String name)
    {
        if (Testdata.trinoRefuses(name)) {
            assertTrinoRefuses(name);
            return;
        }
        // the IEEE 754 bits of every top-level float column, NaN payloads and signed zeros included
        List<String> bits = new ArrayList<>();
        for (String[] c : columns(name)) {
            if (c[1].equals("double")) {
                bits.add("to_hex(to_ieee754_64(" + c[0] + "))");
            }
            else if (c[1].equals("real")) {
                bits.add("to_hex(to_ieee754_32(" + c[0] + "))");
            }
        }
        if (bits.isEmpty()) {
            return;
        }
        String select = "SELECT " + String.join(", ", bits) + " FROM ";
        assertThat(trino.rows(select + "v_" + name)).containsExactlyElementsOf(trino.rows(select + "s_" + name));
    }

    @Test
    void thePluginReadsThroughTheInstalledForks()
            throws Exception
    {
        ClassLoader plugin = hive.getClass().getClassLoader();
        assertThat(plugin).isNotSameAs(TrinoQueriesTest.class.getClassLoader());
        Path dir = tmp.resolve("trino").resolve("plugin").resolve("hive");
        assertThat(jarOf(plugin, "io.trino.parquet.ParquetCompressionUtils")).isEqualTo(dir.resolve("trino-parquet-484-tparquet.jar"));
        assertThat(jarOf(plugin, "org.apache.parquet.format.CompressionCodec")).isEqualTo(dir.resolve("parquet-format-structures-1.18.1-tparquet.jar"));
        assertThat(jarOf(plugin, "org.apache.parquet.hadoop.metadata.CompressionCodecName")).isEqualTo(dir.resolve("parquet-common-1.18.1-tparquet.jar"));
        assertThat(jarOf(plugin, "org.apache.parquet.column.ColumnDescriptor")).isEqualTo(dir.resolve("parquet-column-1.18.1.jar"));
        assertThat(jarOf(plugin, "io.aperiodic.tparquet.Pcodec").getFileName().toString()).startsWith("tparquet-jni-");
        // the plugin's own copy of the codec, which loaded the library bundled in its jar
        Class<?> pcodec = Class.forName("io.aperiodic.tparquet.Pcodec", true, plugin);
        assertThat(pcodec).isNotSameAs(io.aperiodic.tparquet.Pcodec.class);
        assertThat(pcodec.getField("CODEC_ID").getInt(null)).isEqualTo(20547);
    }

    private static Path jarOf(ClassLoader loader, String className)
            throws Exception
    {
        return Path.of(Class.forName(className, false, loader).getProtectionDomain().getCodeSource().getLocation().toURI());
    }

    @Test
    void theTradesHaveManyRowGroups()
            throws IOException
    {
        try (Stream<Path> files = Files.list(tmp.resolve("data").resolve("trades_pcodec"))) {
            for (Path f : files.toList()) {
                assertThat(TrinoParquet.footer(f).getBlocks()).as(f.toString()).hasSizeGreaterThanOrEqualTo(5);
            }
        }
    }

    @Test
    void scansEveryFileAndRowGroup()
    {
        assertThat(trino.value("SELECT count(*) FROM trades_pcodec")).isEqualTo(TRADES);
        assertThat(trino.value("SELECT count(DISTINCT \"$path\") FROM trades_pcodec")).isEqualTo(3L);
        assertThat(trino.value("SELECT count(*) FROM trades_pcodec WHERE bid IS NULL")).isEqualTo(TRADES / 10);
        assertThat(trino.value("SELECT sum(id) FROM trades_pcodec")).isEqualTo(TRADES * (TRADES - 1) / 2);
    }

    @Test
    void aggregatesAreTheStandardCopysAggregates()
    {
        String aggregates = "SELECT count(*), sum(size), min(price), max(price), min(ts), max(ts), count_if(price > 1010), "
                + "checksum(ts), checksum(id), checksum(price), checksum(qty), checksum(size), checksum(symbol), checksum(bid) FROM ";
        assertThat(trino.rows(aggregates + "trades_pcodec")).isEqualTo(trino.rows(aggregates + "trades_zstd"));
        String groups = "SELECT symbol, count(*), sum(size), checksum(price), approx_distinct(qty) FROM %s GROUP BY symbol ORDER BY symbol";
        List<MaterializedRow> bySymbol = trino.rows(groups.formatted("trades_pcodec"));
        assertThat(bySymbol).hasSize(8).isEqualTo(trino.rows(groups.formatted("trades_zstd")));
    }

    @Test
    void joinsWithAStandardTable()
    {
        String join = "SELECT s.sector, count(*), sum(t.size) FROM %s t JOIN symbols s ON t.symbol = s.symbol GROUP BY s.sector ORDER BY 1";
        List<MaterializedRow> sectors = trino.rows(join.formatted("trades_pcodec"));
        assertThat(sectors).hasSize(5).isEqualTo(trino.rows(join.formatted("trades_zstd")));
        // every row of the PCODEC table is a row of the standard one
        assertThat(trino.value("SELECT count(*) FROM trades_pcodec p JOIN trades_zstd z "
                + "ON p.id = z.id AND p.ts = z.ts AND p.price = z.price AND p.qty = z.qty AND p.size = z.size AND p.symbol = z.symbol "
                + "AND p.bid IS NOT DISTINCT FROM z.bid")).isEqualTo(TRADES);
    }

    @Test
    void aSelectiveFilterSkipsRowGroupsByTheirStatistics()
            throws IOException
    {
        String filter = "SELECT count(*), sum(size) FROM %s WHERE id BETWEEN 300000 AND 300999";
        assertThat(trino.rows(filter.formatted("trades_pcodec"))).isEqualTo(trino.rows(filter.formatted("trades_zstd")));
        assertThat(trino.value("SELECT count(*) FROM trades_pcodec WHERE id BETWEEN 300000 AND 300999")).isEqualTo(1000L);
        // the row groups of part-200000 that hold ids 300000..300999
        long overlapping = 0;
        for (BlockMetadata block : TrinoParquet.footer(tmp.resolve("data").resolve("trades_pcodec").resolve("part-200000.parquet")).getBlocks()) {
            long first = 200_000 + block.fileRowCountOffset();
            if (first <= 300_999 && first + block.rowCount() > 300_000) {
                overlapping += block.rowCount();
            }
        }
        assertThat(overlapping).isGreaterThanOrEqualTo(1000).isLessThan(TRADES / 10);
        // with the page indexes off, only the row groups' statistics can skip: Trino reads exactly
        // the overlapping row groups of the one file whose statistics match
        assertThat(trino.inputRows(filter.formatted("trades_pcodec"), Map.of("parquet_use_column_index", "false"))).isEqualTo(overlapping);
        // with them (the default) it can skip pages of those row groups too
        assertThat(trino.inputRows(filter.formatted("trades_pcodec"))).isBetween(1000L, overlapping);
        long all = trino.inputRows("SELECT count(*), sum(size) FROM trades_pcodec WHERE size > 0");
        assertThat(all).isEqualTo(TRADES);
        // on the timestamps too
        Object ts = trino.value("SELECT CAST(ts AS varchar) FROM trades_pcodec WHERE id = 450000");
        String byTime = "SELECT count(*) FROM trades_pcodec WHERE ts >= TIMESTAMP '" + ts + "' AND ts < TIMESTAMP '" + ts + "' + INTERVAL '10' SECOND";
        assertThat(trino.value(byTime)).isEqualTo(100L);
        assertThat(trino.inputRows(byTime)).isLessThan(TRADES / 10);
    }

    @Test
    void aCorruptPayloadIsAQueryError()
    {
        // Trino's message for any page that does not decompress (upstream PageReader), caused by the fork's
        assertThatThrownBy(() -> trino.execute("SELECT sum(price) FROM trades_corrupt"))
                .hasMessage("Could not decompress page")
                .hasStackTraceContaining("io.trino.parquet.ParquetCorruptionException: Malformed Parquet file. "
                        + "Invalid PCODEC page: pcodec payload: unsupported version 2 [local:///corrupt/part-0.parquet]");
        // the other columns of the file still read, and the engine runs on
        assertThat(trino.value("SELECT count(*) FROM trades_corrupt")).isEqualTo(PER_FILE);
        assertThat(trino.value("SELECT sum(size) FROM trades_corrupt")).isEqualTo(trino.value("SELECT sum(size) FROM trades_zstd WHERE id < " + PER_FILE));
        assertThat(trino.value("SELECT count(*) FROM trades_pcodec")).isEqualTo(TRADES);
    }
}
