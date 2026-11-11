package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/**
 * Rust's semantics where Java's differ (RustMath, SortUtils.breakPatterns), on their edges. The
 * expected permutations of break_patterns come from an independent transliteration of
 * sort_utils.rs in Python.
 */
class RustMathTest {
  @Test
  void minAndMaxIgnoreNan() {
    assertEquals(1.0f, RustMath.maxF32(Float.NaN, 1.0f));
    assertEquals(1.0f, RustMath.maxF32(1.0f, Float.NaN));
    assertEquals(2.0f, RustMath.maxF32(1.0f, 2.0f));
    assertEquals(2.0f, RustMath.maxF32(2.0f, 1.0f));
    assertEquals(1.0, RustMath.maxF64(Double.NaN, 1.0));
    assertEquals(1.0, RustMath.maxF64(1.0, Double.NaN));
    assertEquals(2.0, RustMath.maxF64(1.0, 2.0));
    assertEquals(2.0, RustMath.maxF64(2.0, 1.0));
    assertEquals(1.0, RustMath.minF64(Double.NaN, 1.0));
    assertEquals(1.0, RustMath.minF64(1.0, Double.NaN));
    assertEquals(1.0, RustMath.minF64(1.0, 2.0));
    assertEquals(1.0, RustMath.minF64(2.0, 1.0));
  }

  @Test
  void roundingAndCasts() {
    assertEquals(3.0, RustMath.roundF64(2.5));
    assertEquals(-3.0, RustMath.roundF64(-2.5));
    assertEquals(0.0, RustMath.roundF64(0.49999999999999994));
    assertEquals(Double.doubleToRawLongBits(-0.0), Double.doubleToRawLongBits(RustMath.roundF64(-0.4)));
    assertEquals(4503599627370497.0, RustMath.roundF64(4503599627370497.0));
    assertTrue(Double.isNaN(RustMath.roundF64(Double.NaN)));
    assertEquals(Double.POSITIVE_INFINITY, RustMath.roundF64(Double.POSITIVE_INFINITY));
    assertEquals(3.0f, RustMath.roundF32(2.5f));
    assertEquals(-3.0f, RustMath.roundF32(-2.5f));
    assertEquals(8388609.0f, RustMath.roundF32(8388609.0f));
    assertTrue(Float.isNaN(RustMath.roundF32(Float.NaN)));
    assertEquals(-1L, RustMath.f64ToUnsigned(0x1p64, 64));
    assertEquals(0x8000000000000000L, RustMath.f64ToUnsigned(0x1p63, 64));
    assertEquals(0xfffffffffffff800L, RustMath.f64ToUnsigned(Math.nextDown(0x1p64), 64));
    assertEquals(0, RustMath.f64ToUnsigned(-1.0, 64));
    assertEquals(0, RustMath.f64ToUnsigned(Double.NaN, 32));
    assertEquals(0xffffffffL, RustMath.f64ToUnsigned(1e10, 32));
    assertEquals(0xffffL, RustMath.f64ToUnsigned(65536.0, 16));
    assertEquals(65535, RustMath.f64ToUnsigned(65535.9, 16));
    assertEquals(0xffffffffL, RustMath.f32ToU32(Float.POSITIVE_INFINITY));
    assertEquals(0x1p63f * 2, RustMath.u64ToF32(-1L));
    assertEquals(0x1p64, RustMath.u64ToF64(-1L));
    assertTrue(RustMath.totalCmp(-0.0, 0.0) < 0);
    assertTrue(RustMath.totalCmp(1.0, Double.NaN) < 0);
    assertTrue(RustMath.totalCmp(Double.longBitsToDouble(0xfff8000000000000L), Double.NEGATIVE_INFINITY) < 0);
    assertEquals(0, RustMath.totalCmp(2.0, 2.0));
    // negative second operands: their order among themselves and against positives
    assertTrue(RustMath.totalCmp(1.0, -1.0) > 0);
    assertTrue(RustMath.totalCmp(0.0, -0.0) > 0);
    assertTrue(RustMath.totalCmp(-2.0, -1.0) < 0);
    assertTrue(RustMath.totalCmp(-1.0, -2.0) > 0);
    assertTrue(RustMath.totalCmp(-1.0, Math.nextDown(-1.0)) > 0);
    assertTrue(RustMath.totalCmp(Math.nextDown(-1.0), -1.0) < 0);
    assertEquals(0, RustMath.totalCmp(-3.0, -3.0));
    assertEquals(-0.0f, RustMath.sumF32(new float[0]));
  }

  static String moved(int n) {
    long[] v = new long[n];
    for (int i = 0; i < n; i++) {
      v[i] = i;
    }
    SortUtils.breakPatterns(v, 0, n);
    StringBuilder sb = new StringBuilder();
    for (int i = 0; i < n; i++) {
      if (v[i] != i) {
        sb.append('(').append(i).append(", ").append(v[i]).append(')');
      }
    }
    return sb.toString();
  }

  @Test
  void breakPatternsPermutesAsTheReference() {
    assertEquals("", moved(7));
    assertEquals("(0, 5)(3, 0)(5, 3)", moved(8));
    assertEquals("(0, 3)(1, 5)(3, 0)(5, 1)", moved(9));
    assertEquals("(0, 5)(5, 7)(6, 11)(7, 0)(11, 6)", moved(13));
    assertEquals("(0, 9)(7, 0)(9, 7)", moved(16));
    assertEquals("(0, 7)(7, 0)(8, 9)(9, 8)", moved(17));
    assertEquals("(0, 49)(49, 0)(50, 86)(51, 68)(68, 51)(86, 50)", moved(100));
    assertEquals("(151, 501)(476, 500)(495, 499)(499, 495)(500, 476)(501, 151)", moved(1000));
    assertEquals("(96, 2047)(1089, 2048)(1800, 2049)(2047, 96)(2048, 1089)(2049, 1800)", moved(4097));
  }
}
