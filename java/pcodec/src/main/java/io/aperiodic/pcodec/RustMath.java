package io.aperiodic.pcodec;

/**
 * Rust's semantics where Java's differ: unsigned conversions, saturating float-to-int casts,
 * {@code round} (half away from zero), {@code min}/{@code max} (a NaN operand is ignored) and
 * float sums (a left fold from -0.0).
 */
final class RustMath {
  private RustMath() {}

  /** {@code u64 as f32}: round to nearest, ties to even. */
  static float u64ToF32(long x) {
    if (x >= 0) {
      return (float) x;
    }
    return ((float) ((x >>> 1) | (x & 1))) * 2.0f;
  }

  /** {@code u64 as f64} */
  static double u64ToF64(long x) {
    return Latent.toF64(x);
  }

  /** {@code f32 as u32}, saturating, NaN to 0; returned zero-extended. */
  static long f32ToU32(float x) {
    return f64ToUnsigned(x, 32);
  }

  /** {@code f as uN} for N = bits: saturating, NaN to 0; zero-extended. */
  static long f64ToUnsigned(double x, int bits) {
    if (!(x > 0)) {
      return 0; // NaN, zeros and negatives
    }
    if (bits == 64) {
      if (x >= 0x1p64) {
        return -1L;
      }
      if (x >= 0x1p63) {
        return ((long) (x - 0x1p63)) ^ Long.MIN_VALUE;
      }
      return (long) x;
    }
    double limit = Math.scalb(1.0, bits);
    if (x >= limit) {
      return (1L << bits) - 1;
    }
    return (long) x;
  }

  /** {@code f as i64}: Java's cast, which saturates and maps NaN to 0, as Rust's does. */
  static long f64ToI64(double x) {
    return (long) x;
  }

  /** {@code f as i32} */
  static int f64ToI32(double x) {
    return (int) x;
  }

  static double roundF64(double x) {
    if (Double.isNaN(x) || Double.isInfinite(x)) {
      return x;
    }
    double a = Math.abs(x);
    if (a >= 0x1p52) {
      return x;
    }
    double t = Math.floor(a);
    if (a - t >= 0.5) {
      t += 1.0;
    }
    return Math.copySign(t, x);
  }

  static float roundF32(float x) {
    if (Float.isNaN(x) || Float.isInfinite(x)) {
      return x;
    }
    float a = Math.abs(x);
    if (a >= 0x1p23f) {
      return x;
    }
    float t = (float) Math.floor(a);
    if (a - t >= 0.5f) {
      t += 1.0f;
    }
    return Math.copySign(t, x);
  }

  static float maxF32(float a, float b) {
    if (Float.isNaN(a)) {
      return b;
    }
    if (Float.isNaN(b)) {
      return a;
    }
    return a >= b ? a : b;
  }

  static double maxF64(double a, double b) {
    if (Double.isNaN(a)) {
      return b;
    }
    if (Double.isNaN(b)) {
      return a;
    }
    return a >= b ? a : b;
  }

  static double minF64(double a, double b) {
    if (Double.isNaN(a)) {
      return b;
    }
    if (Double.isNaN(b)) {
      return a;
    }
    return a <= b ? a : b;
  }

  /** {@code iter.sum::<f32>()}: a fold from -0.0 */
  static float sumF32(float[] xs) {
    float s = -0.0f;
    for (float x : xs) {
      s += x;
    }
    return s;
  }

  /** {@code f64::total_cmp} */
  static int totalCmp(double a, double b) {
    long x = Double.doubleToRawLongBits(a);
    long y = Double.doubleToRawLongBits(b);
    x ^= (x >> 63) >>> 1;
    y ^= (y >> 63) >>> 1;
    return Long.compare(x, y);
  }
}
