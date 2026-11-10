package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.aperiodic.pcodec.payload.ParquetType;
import io.aperiodic.pcodec.payload.PcodecPayload;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import org.junit.jupiter.api.DynamicTest;
import org.junit.jupiter.api.TestFactory;

/**
 * The repository's Parquet conformance files (testdata/files), read page by page with a minimal
 * Thrift reader: every PCODEC page decodes with the Java codec to its expected size, the payload
 * modes per column are those of files/manifest.json, the decoded bytes are the reference's
 * (when the oracle is built), and re-encoding a page with the column's type as the hint gives
 * the page's payload back, byte for byte, at one of pcodec's levels (ZSTD pages excepted).
 */
class ParquetFilesTest {
  static final int PCODEC = 20547;

  // ------------------------------------------------------------ Thrift compact protocol

  static final class Thrift {
    final byte[] b;
    int pos;

    Thrift(byte[] b, int pos) {
      this.b = b;
      this.pos = pos;
    }

    long varint() {
      long v = 0;
      int shift = 0;
      while (true) {
        int x = b[pos++] & 0xff;
        v |= (long) (x & 0x7f) << shift;
        if (x < 0x80) {
          return v;
        }
        shift += 7;
      }
    }

    long zigzag() {
      long v = varint();
      return (v >>> 1) ^ -(v & 1);
    }

    Object value(int type) {
      switch (type) {
        case 1:
          return Boolean.TRUE;
        case 2:
          return Boolean.FALSE;
        case 3:
          return (long) b[pos++];
        case 4:
        case 5:
        case 6:
          return zigzag();
        case 7:
          pos += 8;
          return null;
        case 8:
          {
            int n = (int) varint();
            byte[] s = java.util.Arrays.copyOfRange(b, pos, pos + n);
            pos += n;
            return s;
          }
        case 9:
        case 10:
          {
            int h = b[pos++] & 0xff;
            int n = h >>> 4;
            int et = h & 0xf;
            if (n == 15) {
              n = (int) varint();
            }
            List<Object> l = new ArrayList<>();
            for (int i = 0; i < n; i++) {
              l.add(et == 1 || et == 2 ? (Object) (b[pos++] == 1) : value(et));
            }
            return l;
          }
        case 11:
          {
            int n = (int) varint();
            if (n > 0) {
              int kv = b[pos++] & 0xff;
              for (int i = 0; i < n; i++) {
                value(kv >>> 4);
                value(kv & 0xf);
              }
            }
            return null;
          }
        case 12:
          return struct();
        default:
          throw new IllegalStateException("thrift type " + type);
      }
    }

    Map<Integer, Object> struct() {
      Map<Integer, Object> m = new TreeMap<>();
      int last = 0;
      while (true) {
        int h = b[pos++] & 0xff;
        if (h == 0) {
          return m;
        }
        int delta = h >>> 4;
        int type = h & 0xf;
        int id = delta == 0 ? (int) zigzag() : last + delta;
        last = id;
        m.put(id, value(type));
      }
    }
  }

  static long num(Map<Integer, Object> m, int id, long dflt) {
    Object v = m.get(id);
    return v == null ? dflt : (Long) v;
  }

  @SuppressWarnings("unchecked")
  static <T> T get(Map<Integer, Object> m, int id) {
    return (T) m.get(id);
  }

  // ------------------------------------------------------------ the test

  static Path dir() {
    return Paths.get(System.getProperty("tparquet.testdata", "../../testdata"), "files");
  }

  static final String[] MODES = {"RAW", "PCO", "DECIMAL", "ZSTD"};

  @SuppressWarnings("unchecked")
  @TestFactory
  List<DynamicTest> everyPcodecPageOfEveryFile() throws IOException {
    Path d = dir();
    Map<String, Object> manifest =
        (Map<String, Object>) Json.parse(new String(Files.readAllBytes(d.resolve("manifest.json")), StandardCharsets.UTF_8));
    List<DynamicTest> tests = new ArrayList<>();
    for (Object o : (List<Object>) manifest.get("files")) {
      Map<String, Object> f = (Map<String, Object>) o;
      String name = (String) f.get("name");
      tests.add(DynamicTest.dynamicTest(name, () -> checkFile(d.resolve(name + ".parquet"), f)));
    }
    return tests;
  }

  @SuppressWarnings("unchecked")
  static void checkFile(Path p, Map<String, Object> entry) throws IOException {
    byte[] file = Files.readAllBytes(p);
    int footerLen = (file[file.length - 8] & 0xff) | (file[file.length - 7] & 0xff) << 8
        | (file[file.length - 6] & 0xff) << 16 | (file[file.length - 5] & 0xff) << 24;
    assertEquals("PAR1", new String(file, file.length - 4, 4, StandardCharsets.US_ASCII));
    Map<Integer, Object> meta = new Thrift(file, file.length - 8 - footerLen).struct();
    Map<String, Map<String, Long>> modes = new LinkedHashMap<>();
    PcodecPayload codec = new PcodecPayload(new AircompressorZstd());
    Oracle oracle = Oracle.get();
    int pcodecPages = 0;
    for (Object rgo : (List<Object>) meta.get(4)) {
      Map<Integer, Object> rg = (Map<Integer, Object>) rgo;
      for (Object cco : (List<Object>) rg.get(1)) {
        Map<Integer, Object> cc = (Map<Integer, Object>) cco;
        Map<Integer, Object> cm = get(cc, 3);
        List<Object> pathParts = get(cm, 3);
        StringBuilder path = new StringBuilder();
        for (Object part : pathParts) {
          path.append(path.length() == 0 ? "" : ".").append(new String((byte[]) part, StandardCharsets.UTF_8));
        }
        int type = (int) num(cm, 1, -1);
        int codecId = (int) num(cm, 4, -1);
        if (codecId != PCODEC) {
          continue;
        }
        long dictOffset = num(cm, 11, 0);
        long dataOffset = num(cm, 9, 0);
        long start = dictOffset > 0 ? Math.min(dictOffset, dataOffset) : dataOffset;
        long total = num(cm, 7, 0);
        int pos = (int) start;
        while (pos < start + total) {
          Thrift t = new Thrift(file, pos);
          Map<Integer, Object> ph = t.struct();
          int pageType = (int) num(ph, 1, -1);
          int uncompressed = (int) num(ph, 2, -1);
          int compressed = (int) num(ph, 3, -1);
          int body = t.pos;
          int levels = 0;
          boolean isCompressed = true;
          if (pageType == 3) {
            Map<Integer, Object> v2 = get(ph, 8);
            levels = (int) (num(v2, 5, 0) + num(v2, 6, 0));
            Object c = v2.get(7);
            isCompressed = c == null || (Boolean) c;
          }
          pos = body + compressed;
          String col = path.toString();
          if (!isCompressed) {
            modes.computeIfAbsent(col, k -> new TreeMap<>()).merge("UNCOMPRESSED", 1L, Long::sum);
            continue;
          }
          int expected = uncompressed - levels;
          byte[] payload = java.util.Arrays.copyOfRange(file, body + levels, body + compressed);
          if (expected == 0) {
            modes.computeIfAbsent(col, k -> new TreeMap<>()).merge("EMPTY", 1L, Long::sum);
          } else {
            modes.computeIfAbsent(col, k -> new TreeMap<>()).merge(MODES[PcodecPayload.mode(payload)], 1L, Long::sum);
          }
          byte[] decoded = codec.decompress(payload, expected);
          assertEquals(expected, decoded.length);
          pcodecPages++;
          if (oracle != null) {
            assertArrayEquals(oracle.payloadDecode(expected, payload).body, decoded, col + ": the reference decodes the same");
          }
          if (PcodecPayload.mode(payload) != PcodecPayload.MODE_ZSTD) {
            ParquetType hint = ParquetType.fromParquet(type);
            boolean found = false;
            for (int level = 0; level <= 12 && !found; level++) {
              found = java.util.Arrays.equals(payload, new PcodecPayload().compress(decoded, 0, decoded.length, hint, level, true));
            }
            assertTrue(found, col + ": no level re-encodes the page's payload");
          }
        }
      }
    }
    // the manifest's PCODEC pages per payload mode, per column
    for (Object co : (List<Object>) entry.get("columns")) {
      Map<String, Object> c = (Map<String, Object>) co;
      Map<String, Object> want = (Map<String, Object>) c.get("modes");
      if (want == null || want.isEmpty()) {
        continue;
      }
      Map<String, Long> got = modes.getOrDefault((String) c.get("path"), new TreeMap<>());
      Map<String, Long> w = new TreeMap<>();
      for (Map.Entry<String, Object> e : want.entrySet()) {
        w.put(e.getKey(), ((java.math.BigInteger) e.getValue()).longValueExact());
      }
      assertEquals(w, got, c.get("path") + " payload modes");
    }
    if (!"standard_control".equals(entry.get("name"))) {
      assertTrue(pcodecPages > 0 || "empty".equals(entry.get("name")), "no PCODEC pages read");
    }
  }
}
