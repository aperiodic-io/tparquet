package io.aperiodic.pcodec.payload;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.ByteArrayOutputStream;
import java.util.Arrays;
import org.junit.jupiter.api.Test;

/** The payload's parts: tparquet-core's own unit tests (decimal.rs, varint.rs), and its errors. */
class PayloadUnitTest {
  static long d(double x) {
    return Double.doubleToRawLongBits(x);
  }

  static long f(float x) {
    return Float.floatToRawIntBits(x) & 0xffffffffL;
  }

  @Test
  void pow10IsExact() {
    for (int e = 0; e < Decimal.POW10.length; e++) {
      assertEquals(Double.parseDouble("1e" + e), Decimal.POW10[e]);
    }
    assertEquals(Decimal.MAX_EXPONENT, Decimal.POW10.length - 1);
  }

  @Test
  void exactAsTheReference() {
    assertEquals(123, Decimal.exact(d(1.23), false, 2));
    assertEquals(-5, Decimal.exact(d(-0.05), false, 2));
    assertEquals(Long.MIN_VALUE, Decimal.exact(d(1.23), false, 1));
    assertEquals(29, Decimal.exact(d(0.29), false, 2));
    assertEquals(5, Decimal.exact(d(5.0), false, 0));
    assertEquals(1, Decimal.exact(f(0.1f), true, 1));
    assertEquals(Long.MIN_VALUE, Decimal.exact(d(-0.0), false, 3));
    assertEquals(0, Decimal.exact(d(0.0), false, 3));
    for (double x : new double[] {Double.NaN, Double.POSITIVE_INFINITY, Double.NEGATIVE_INFINITY}) {
      assertEquals(Long.MIN_VALUE, Decimal.exact(d(x), false, 0));
    }
    assertEquals(Long.MIN_VALUE, Decimal.exact(f(Float.NaN), true, 0));
    double below = 4.611686018427387e18;
    assertEquals((long) below, Decimal.exact(d(below), false, 0));
    assertEquals(-(long) below, Decimal.exact(d(-below), false, 0));
    assertEquals(Long.MIN_VALUE, Decimal.exact(d(Math.pow(2, 62)), false, 0));
    assertEquals(Long.MIN_VALUE, Decimal.exact(d(-Math.pow(2, 62)), false, 0));
    assertEquals((1L << 62) - 1024, Decimal.exact(d(4.611686018427387e17), false, 1));
    assertEquals(Long.MIN_VALUE, Decimal.exact(d(4.611686018427388e17), false, 1));
    float belowF = Float.intBitsToFloat(Float.floatToRawIntBits((float) Math.pow(2, 62)) - 1);
    assertEquals((long) belowF, Decimal.exact(f(belowF), true, 0));
    assertEquals(Long.MIN_VALUE, Decimal.exact(f((float) Math.pow(2, 62)), true, 0));
    assertEquals(Long.MIN_VALUE, Decimal.exact(f(Float.MAX_VALUE), true, 0));
    assertEquals(1, Decimal.exact(d(1e-18), false, 18));
    assertEquals(1, Decimal.exact(f(1e-18f), true, 18));
    assertEquals(Long.MIN_VALUE, Decimal.exact(f(-0.0f), true, 0));
    assertEquals(Long.MIN_VALUE, Decimal.exact(1, true, 18));
    assertEquals(Long.MIN_VALUE, Decimal.exact(1, false, 18));
    assertEquals(Long.MIN_VALUE, Decimal.exact(d(0.125), false, 2));
    assertEquals(125, Decimal.exact(d(0.125), false, 3));
    assertEquals(-125, Decimal.exact(d(-0.125), false, 3));
  }

  @Test
  void exponentAsTheReference() {
    assertEquals(-1, Decimal.exponent(new long[0], false));
    assertEquals(0, Decimal.exponent(new long[] {d(1.0), d(2.0)}, false));
    assertEquals(2, Decimal.exponent(new long[] {d(1.5), d(2.25)}, false));
    assertEquals(-1, Decimal.exponent(new long[] {d(1e-30)}, false));
    assertEquals(-1, Decimal.exponent(new long[] {d(1e300)}, false));
    assertEquals(15, Decimal.exponent(new long[] {d(Math.PI)}, false));
    long[] v = new long[100];
    for (int i = 0; i < 100; i++) {
      v[i] = d(i / 10.0);
    }
    v[0] = d(1e-30);
    assertEquals(1, Decimal.exponent(v, false));
    v[1] = d(3e-30);
    assertEquals(-1, Decimal.exponent(v, false));
    long[] w = new long[Decimal.PROBE + 10 * Decimal.PROBE];
    Arrays.fill(w, 0, Decimal.PROBE, d(1.0));
    Arrays.fill(w, Decimal.PROBE, w.length, d(Math.PI));
    assertEquals(0, Decimal.exponent(w, false));
    w[0] = d(0.5);
    assertEquals(0, Decimal.exponent(w, false));
  }

  @Test
  void splitAsTheReference() {
    long[] v = new long[200];
    for (int i = 0; i < 200; i++) {
      v[i] = d(i);
    }
    v[5] = d(0.5);
    v[50] = d(Double.NaN);
    Decimal.Split s = Decimal.split(v, false, 0);
    assertArrayEquals(new int[] {5, 50}, s.exceptions);
    assertEquals(4, s.qs[5]);
    assertEquals(49, s.qs[50]);
    assertEquals(199, s.qs[199]);
    v[60] = d(0.25);
    assertNull(Decimal.split(v, false, 0));
    long[] u = new long[100];
    for (int i = 0; i < 100; i++) {
      u[i] = d(i + 1);
    }
    u[0] = d(0.5);
    Decimal.Split t = Decimal.split(u, false, 0);
    assertEquals(0, t.qs[0]);
    assertArrayEquals(new int[] {0}, t.exceptions);
    assertNull(Decimal.split(Arrays.copyOf(u, 99), false, 0));
  }

  static byte[] enc(long v) {
    ByteArrayOutputStream o = new ByteArrayOutputStream();
    Varint.put(o, v);
    return o.toByteArray();
  }

  @Test
  void varintsAsTheReference() {
    assertArrayEquals(new byte[] {0}, enc(0));
    assertArrayEquals(new byte[] {1}, enc(1));
    assertArrayEquals(new byte[] {0x7f}, enc(127));
    assertArrayEquals(new byte[] {(byte) 0x80, 1}, enc(128));
    assertArrayEquals(new byte[] {(byte) 0xac, 2}, enc(300));
    byte[] max = enc(-1L);
    assertEquals(10, max.length);
    assertEquals(1, max[9]);
    for (long v : new long[] {0, 1, 127, 128, 255, 16383, 16384, 1L << 35, -2L, -1L}) {
      byte[] b = Arrays.copyOf(enc(v), enc(v).length + 1);
      b[b.length - 1] = (byte) 0xee;
      int[] at = {0};
      assertEquals(v, Varint.get(b, at, b.length));
      assertEquals(b.length - 1, at[0]);
    }
    // non-minimal forms are accepted, overlong and overflowing ones refused
    assertEquals(0, Varint.get(new byte[] {(byte) 0x80, 0}, new int[] {0}, 2));
    byte[] ten = {(byte) 0x80, (byte) 0x80, (byte) 0x80, (byte) 0x80, (byte) 0x80, (byte) 0x80, (byte) 0x80, (byte) 0x80, (byte) 0x80, 0};
    assertEquals(0, Varint.get(ten, new int[] {0}, 10));
    byte[] eleven = Arrays.copyOf(ten, 11);
    eleven[9] = (byte) 0x80;
    assertThrows(PayloadException.class, () -> Varint.get(eleven, new int[] {0}, 11));
    byte[] over = ten.clone();
    over[9] = 2;
    assertThrows(PayloadException.class, () -> Varint.get(over, new int[] {0}, 10));
    assertThrows(PayloadException.class, () -> Varint.get(new byte[] {(byte) 0x80}, new int[] {0}, 1));
  }

  static String error(byte[] payload, int len) {
    return assertThrows(PayloadException.class, () -> new PcodecPayload().decompress(payload, len)).getMessage();
  }

  @Test
  void errorsAsTheReference() {
    PcodecPayload c = new PcodecPayload();
    assertEquals("pcodec payload: truncated", error(new byte[0], 0));
    assertEquals("pcodec payload: truncated", error(new byte[] {1}, 0));
    assertEquals("pcodec payload: unsupported version 2", error(new byte[] {2, 0}, 0));
    assertEquals("pcodec payload: unknown mode 4", error(new byte[] {1, 4}, 0));
    assertEquals("pcodec payload: decodes to 1 bytes, expected 2", error(new byte[] {1, 0, 9}, 2));
    assertEquals("pcodec payload: truncated", error(new byte[] {1, 1}, 8));
    assertEquals("pcodec payload: unknown dtype 3", error(new byte[] {1, 1, 3}, 8));
    assertEquals("pcodec payload: expected size 7 is not a multiple of the value width 8", error(new byte[] {1, 1, 2}, 7));
    assertEquals("pcodec payload: truncated", error(new byte[] {1, 2, 5}, 8));
    assertEquals("pcodec payload: exponent 19 above 18", error(new byte[] {1, 2, 5, 19}, 8));
    assertEquals("pcodec payload: unknown dtype 1", error(new byte[] {1, 2, 1, 0}, 8));
    assertEquals("pcodec payload: exception count or position out of range", error(new byte[] {1, 2, 5, 0, 2}, 8));
    assertEquals("pcodec payload: exception count or position out of range", error(new byte[] {1, 2, 5, 0, 1, 1}, 8));
    assertEquals("pcodec payload: truncated", error(new byte[] {1, 2, 5, 0, 1, 0}, 8));
    assertEquals("pcodec payload: truncated", error(new byte[] {1, 3}, 0));
    assertEquals("pcodec payload: zstd: ZSTD mode needs a ZstdCodec", error(new byte[] {1, 3, 0x28}, 0));
    assertEquals("pcodec payload: truncated", error(new byte[] {1, 3, 0x28, 1, 2}, 1 << 20));
    assertThrows(PayloadException.class, () -> c.compress(new byte[8], 0, 8, ParquetType.INT64, 13, true));
    assertThrows(PayloadException.class, () -> c.compress(new byte[8], 0, 8, ParquetType.INT64, -1, true));
    assertThrows(IllegalArgumentException.class, () -> c.decompress(new byte[] {1, 0}, -1));
    assertArrayEquals(new byte[] {1, 0}, c.compress(new byte[0], ParquetType.DOUBLE));
    // a position delta that overflows u64 is out of range
    byte[] wrap = {1, 2, 5, 0, 2, 0, (byte) 0xff, (byte) 0xff, (byte) 0xff, (byte) 0xff, (byte) 0xff, (byte) 0xff, (byte) 0xff, (byte) 0xff, (byte) 0xff, 1};
    assertEquals("pcodec payload: exception count or position out of range", error(wrap, 16));
  }

  @Test
  void decodesIntoAnArrayAndKeepsTheDecimalChoice() {
    byte[] src = new byte[8 * 300];
    for (int i = 0; i < 300; i++) {
      long b = d(Math.round(i * 1.37 * 100) / 100.0);
      for (int k = 0; k < 8; k++) {
        src[8 * i + k] = (byte) (b >>> (8 * k));
      }
    }
    PcodecPayload c = new PcodecPayload();
    byte[] withDecimal = c.compress(src, 0, src.length, ParquetType.DOUBLE, 8, true);
    byte[] without = c.compress(src, 0, src.length, ParquetType.DOUBLE, 8, false);
    assertEquals(PcodecPayload.MODE_DECIMAL, PcodecPayload.mode(withDecimal));
    assertEquals(PcodecPayload.MODE_PCO, PcodecPayload.mode(without));
    assertTrue(withDecimal.length < without.length);
    byte[] dst = new byte[src.length + 4];
    c.decompress(withDecimal, 0, withDecimal.length, dst, 2, src.length);
    assertArrayEquals(src, Arrays.copyOfRange(dst, 2, 2 + src.length));
    assertEquals(4, ParquetType.FLOAT.width);
    assertNull(ParquetType.fromParquet(3));
  }
}
