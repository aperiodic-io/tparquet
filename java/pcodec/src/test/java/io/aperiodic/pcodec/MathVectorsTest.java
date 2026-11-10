package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;

/**
 * The reference's digests of the platform math (MathParityTest's functions) over fixed domains,
 * recorded in vectors/math.txt, so that every build checks the libm ports, the f16
 * conversions and the casts without Rust. The domains are smaller than MathParityTest's (which
 * runs against the reference itself) but still millions of inputs.
 */
class MathVectorsTest {
  /** function, seed, start, count: each a slice of MathParityTest's domains */
  static final long[][] DOMAINS = {
    {0, 0, 0x3f000000L, 1 << 22}, // f32 to f16 around 0.5..2^9, every pattern
    {0, 0, 0x38000000L, 1 << 21}, // f32 to f16 around the f16 subnormals
    {0, 0, 0x7f800000L, 1 << 20}, // f32 infinities and NaNs to f16
    {0, 0, 0xff7ff000L, 1 << 20}, // negative, top of the range, and NaNs
    {1, 0, 0, 1 << 16}, // every f16 to f32 and f64
    {2, 0, 0, 1 << 22}, // log2 of every count to 2^22
    {3, 21, 0, 1 << 19}, // log2 of random positive doubles
    {4, 22, 0, 1 << 19}, // cbrt of random positive doubles
    {5, 23, 0, 1 << 20}, // log2 and cbrt of probabilities
    {6, 0, 0, 801}, // 10^k for every k in -400..400
    {7, 24, 0, 1 << 19}, // float to int casts and rounding
    {8, 25, 0, 1 << 19}, // int to float casts
    {9, 26, 0, 1 << 19}, // f64 to f16
    {10, 0, 0, 2048L * 2098}, // log2 near every power of two
    {11, 27, 0, 1 << 19}, // log2 near 1
  };

  static Path path() {
    return Paths.get(System.getProperty("pcodec.vectors", "vectors"), "math.txt");
  }

  /** the lines of math.txt, from the oracle */
  static List<String> record() {
    List<String> lines = new ArrayList<>();
    for (long[] d : DOMAINS) {
      long h = MathParityTest.oracleDigest((int) d[0], d[1], d[2], d[3]);
      lines.add(d[0] + " " + d[1] + " " + d[2] + " " + d[3] + " " + Long.toHexString(h));
    }
    return lines;
  }

  @Test
  void javaGivesTheRecordedDigests() throws Exception {
    List<String> lines = Files.readAllLines(path(), StandardCharsets.UTF_8);
    assertEquals(DOMAINS.length, lines.size());
    for (int i = 0; i < DOMAINS.length; i++) {
      long[] d = DOMAINS[i];
      String[] f = lines.get(i).split(" ");
      assertEquals(d[0] + " " + d[1] + " " + d[2] + " " + d[3], f[0] + " " + f[1] + " " + f[2] + " " + f[3]);
      long want = Long.parseUnsignedLong(f[4], 16);
      assertEquals(Long.toHexString(want), Long.toHexString(MathParityTest.javaDigest((int) d[0], d[1], d[2], d[3])),
          "function " + d[0] + " from " + d[2]);
    }
  }
}
