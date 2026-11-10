package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * The arithmetic below pcodec against the reference's platform: glibc's log2 and cbrt, Rust's
 * powf, casts and rounding, and the f16 conversions of the half crate. Finite domains are
 * checked completely (every f32 to f16, every f16, log2 of every count up to 2^24, 10^k for
 * every k); the others on large seeded samples. Both sides digest the outputs; a mismatch is
 * bisected down to its first input.
 */
class MathParityTest {
  static Oracle o;
  static long samples;

  @BeforeAll
  static void setUp() {
    o = Oracle.get();
    samples = 2_000_000L * Integer.getInteger("pcodec.parity.scale", 1);
  }

  static long splitmix(long seed, long i) {
    long z = seed + (i + 1) * 0x9e3779b97f4a7c15L;
    z = (z ^ (z >>> 30)) * 0xbf58476d1ce4e5b9L;
    z = (z ^ (z >>> 27)) * 0x94d049bb133111ebL;
    return z ^ (z >>> 31);
  }

  static double positive(long bits) {
    double x = Double.longBitsToDouble(bits & 0x7fffffffffffffffL);
    return Double.isFinite(x) && x > 0.0 ? x : 1.0;
  }

  static long bits(double x) {
    return Double.doubleToRawLongBits(x);
  }

  static long value(int f, long seed, long i) {
    long r = splitmix(seed, i);
    switch (f) {
      case 0:
        return FloatType.f32BitsToF16Bits(i & 0xffffffffL);
      case 1:
        {
          long f32 = FloatType.f16BitsToF32Bits(i & 0xffff);
          return f32 ^ Long.rotateLeft(bits(FloatType.f32BitsToF64(f32)), 32);
        }
      case 2:
        return bits(GlibcMath.log2((double) (i + 1)));
      case 3:
        return bits(GlibcMath.log2(positive(r)));
      case 4:
        return bits(GlibcMath.cbrt(positive(r)));
      case 5:
        {
          double p = (double) ((r >>> 11) + 1) / (double) (1L << 53);
          return bits(GlibcMath.log2(p)) ^ Long.rotateLeft(bits(GlibcMath.cbrt(p)), 17);
        }
      case 6:
        return bits(FloatMult.pow10((double) i - 400.0));
      case 7:
        {
          double x = Double.longBitsToDouble(r);
          float y = Float.intBitsToFloat((int) r);
          return RustMath.f64ToUnsigned(x, 64)
              ^ Long.rotateLeft((long) x, 7)
              ^ Long.rotateLeft(RustMath.f64ToUnsigned(x, 32), 13)
              ^ Long.rotateLeft(Double.isNaN(x) ? 0 : bits(RustMath.roundF64(x)), 19)
              ^ Long.rotateLeft(RustMath.f32ToU32(y), 23)
              ^ Long.rotateLeft(Float.isNaN(y) ? 0 : Float.floatToRawIntBits(RustMath.roundF32(y)) & 0xffffffffL, 29)
              ^ Long.rotateLeft(RustMath.f64ToUnsigned(y, 64), 31)
              ^ Long.rotateLeft(RustMath.f64ToUnsigned(x, 16), 37);
        }
      case 8:
        return bits(Latent.toF64(r))
            ^ Long.rotateLeft(Float.floatToRawIntBits(RustMath.u64ToF32(r)) & 0xffffffffL, 32)
            ^ (Float.floatToRawIntBits(RustMath.u64ToF32(r >>> (r & 63))) & 0xffffffffL);
      case 9:
        return FloatType.F16.fromF64(Double.longBitsToDouble(r));
      case 10:
        {
          long e = i / 2048 - 1074;
          long k = i % 2048 - 1024;
          long p = e < -1022 ? 1L << (e + 1074) : (e + 1023) << 52;
          double x = Double.longBitsToDouble(p + k);
          return Double.isFinite(x) && x > 0.0 ? bits(GlibcMath.log2(x)) : 0;
        }
      case 11:
        {
          double x = 1.0 + ((double) (r >>> 11) / (double) (1L << 53) - 0.5) * 0.09;
          return bits(GlibcMath.log2(x));
        }
      default:
        return 0;
    }
  }

  static long javaDigest(int f, long seed, long start, long count) {
    long h = 0xcbf29ce484222325L;
    for (long i = start; i < start + count; i++) {
      h = (h ^ value(f, seed, i)) * 0x100000001b3L;
    }
    return h;
  }

  static long oracleDigest(int f, long seed, long start, long count) {
    ByteBuffer b = ByteBuffer.allocate(26).order(ByteOrder.LITTLE_ENDIAN);
    b.put((byte) 8).put((byte) f).putLong(seed).putLong(start).putLong(count);
    return ByteBuffer.wrap(o.call(b.array()).body).order(ByteOrder.LITTLE_ENDIAN).getLong();
  }

  /** checks f over [start, start + count) in blocks; bisects a differing block to its first input */
  static void check(int f, long seed, long start, long count) {
    assumeTrue(o != null, "oracle not built");
    long block = 1 << 24;
    for (long s = start; s < start + count; s += block) {
      long c = Math.min(block, start + count - s);
      if (javaDigest(f, seed, s, c) != oracleDigest(f, seed, s, c)) {
        long lo = s;
        long n = c;
        while (n > 1) {
          long half = n / 2;
          if (javaDigest(f, seed, lo, half) != oracleDigest(f, seed, lo, half)) {
            n = half;
          } else {
            lo += half;
            n -= half;
          }
        }
        throw new AssertionError("function " + f + " differs first at input " + lo
            + " (java value " + Long.toHexString(value(f, seed, lo)) + ")");
      }
    }
  }

  @Test
  void everyF32ToF16() {
    check(0, 0, 0, 1L << 32);
  }

  @Test
  void everyF16ToF32AndF64() {
    check(1, 0, 0, 1L << 16);
  }

  @Test
  void log2OfEveryCountUpTo2Pow24() {
    check(2, 0, 0, 1L << 24);
  }

  @Test
  void log2AndCbrtOfRandomPositiveDoubles() {
    check(3, 11, 0, samples);
    check(4, 12, 0, samples);
  }

  @Test
  void log2AndCbrtOfProbabilities() {
    check(5, 13, 0, 4 * samples);
  }

  @Test
  void log2NearEveryPowerOfTwoAndNearOne() {
    check(10, 0, 0, 2048L * 2098);
    check(11, 14, 0, samples);
  }

  @Test
  void pow10OfEveryInteger() {
    check(6, 0, 0, 801);
  }

  @Test
  void castsAndRounding() {
    check(7, 15, 0, samples);
    check(8, 16, 0, samples);
  }

  @Test
  void f64ToF16() {
    check(9, 17, 0, samples);
  }

  @Test
  void digestsDetectDifferences() {
    assumeTrue(o != null, "oracle not built");
    // the check itself: digests of different functions differ
    assertEquals(javaDigest(2, 0, 0, 1000), oracleDigest(2, 0, 0, 1000));
    org.junit.jupiter.api.Assertions.assertNotEquals(javaDigest(2, 0, 0, 1000), oracleDigest(3, 0, 0, 1000));
  }
}
