package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.aperiodic.pcodec.payload.ParquetType;
import io.aperiodic.pcodec.payload.PayloadException;
import io.aperiodic.pcodec.payload.PcodecPayload;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.DynamicTest;
import org.junit.jupiter.api.TestFactory;

/**
 * The repository's conformance vectors (testdata/payloads): every valid payload decodes to its
 * bytes, every invalid one is refused, and every payload marked reference_encoding is what the
 * Java encoder writes for its input, byte for byte (ZSTD mode excepted: its bytes are the
 * Zstandard library's; ZSTD payloads are decoded with a pure Java Zstandard).
 */
class PayloadVectorsTest {
  static Path dir() {
    return Paths.get(System.getProperty("tparquet.testdata", "../../testdata"), "payloads");
  }

  @SuppressWarnings("unchecked")
  @TestFactory
  List<DynamicTest> vectors() throws IOException {
    Path d = dir();
    Map<String, Object> manifest =
        (Map<String, Object>) Json.parse(new String(Files.readAllBytes(d.resolve("manifest.json")), StandardCharsets.UTF_8));
    assertEquals(java.math.BigInteger.ONE, manifest.get("format_version"));
    PcodecPayload codec = new PcodecPayload(new AircompressorZstd());
    List<DynamicTest> tests = new ArrayList<>();
    for (Object o : (List<Object>) manifest.get("vectors")) {
      Map<String, Object> v = (Map<String, Object>) o;
      String name = (String) v.get("name");
      tests.add(
          DynamicTest.dynamicTest(
              name,
              () -> {
                byte[] payload = Files.readAllBytes(d.resolve((String) v.get("payload")));
                int expectedLen = ((java.math.BigInteger) v.get("expected_len")).intValueExact();
                if (Boolean.TRUE.equals(v.get("valid"))) {
                  byte[] want = Files.readAllBytes(d.resolve((String) v.get("decoded")));
                  assertArrayEquals(want, codec.decompress(payload, expectedLen));
                  String mode = (String) v.get("mode");
                  assertEquals(
                      List.of("RAW", "PCO", "DECIMAL", "ZSTD").indexOf(mode),
                      PcodecPayload.mode(payload));
                  String hint = (String) v.get("hint");
                  ParquetType t = hint == null ? null : ParquetType.valueOf(hint);
                  int level = ((java.math.BigInteger) v.get("level")).intValueExact();
                  if (Boolean.TRUE.equals(v.get("reference_encoding")) && !"ZSTD".equals(mode)) {
                    assertArrayEquals(payload, new PcodecPayload().compress(want, 0, want.length, t, level, true));
                  }
                  // whatever the encoder writes decodes back
                  byte[] again = codec.compress(want, 0, want.length, t, level, true);
                  assertArrayEquals(want, codec.decompress(again, want.length));
                } else {
                  PayloadException e =
                      assertThrows(PayloadException.class, () -> codec.decompress(payload, expectedLen));
                  assertTrue(e.getMessage().startsWith("pcodec payload: "));
                }
              }));
    }
    return tests;
  }
}
