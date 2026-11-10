package io.aperiodic.pcodec.payload;

/** DECIMAL mode (spec section 4.3): floats as {@code q / 10^e} with exact exceptions. */
final class Decimal {
  private Decimal() {}

  static final int MAX_EXPONENT = 18;
  static final int PROBE = 4096;
  /** P[e], the f64 nearest to 10^e (exact for e <= 22) */
  static final double[] POW10 = {
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18
  };

  /** the spec's decode of q at exponent e, as raw bits of the width */
  static long decode(long q, int e, boolean isFloat) {
    double d = (double) q / POW10[e];
    if (isFloat) {
      return Float.floatToRawIntBits((float) d) & 0xffffffffL;
    }
    return Double.doubleToRawLongBits(d);
  }

  private static double toF64(long bits, boolean isFloat) {
    // only non-NaN values can be exact; a NaN gives NaN here either way
    return isFloat ? (double) Float.intBitsToFloat((int) bits) : Double.longBitsToDouble(bits);
  }

  /** {@code exact}: q with decode(q, e) == v bit for bit, else Long.MIN_VALUE */
  static long exact(long bits, boolean isFloat, int e) {
    double x = round(toF64(bits, isFloat) * POW10[e]);
    if (!Double.isFinite(x) || Math.abs(x) >= 4.611686018427388e18) {
      return Long.MIN_VALUE;
    }
    long q = (long) x;
    return decode(q, e, isFloat) == bits ? q : Long.MIN_VALUE;
  }

  /** Rust's {@code f64::round}: half away from zero */
  static double round(double x) {
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

  /** smallest exponent at which at least 99% of the probe is exact, or -1 */
  static int exponent(long[] vals, boolean isFloat) {
    int n = Math.min(vals.length, PROBE);
    if (n == 0) {
      return -1;
    }
    for (int e = 0; e <= MAX_EXPONENT; e++) {
      long hits = 0;
      for (int i = 0; i < n; i++) {
        if (exact(vals[i], isFloat, e) != Long.MIN_VALUE) {
          hits++;
        }
      }
      if (hits * 100 >= (long) n * 99) {
        return e;
      }
    }
    return -1;
  }

  static final class Split {
    final long[] qs;
    final int[] exceptions;

    Split(long[] qs, int[] exceptions) {
      this.qs = qs;
      this.exceptions = exceptions;
    }
  }

  /** the integers and exception positions at e, or null above 1% exceptions */
  static Split split(long[] vals, boolean isFloat, int e) {
    int limit = vals.length / 100;
    long[] qs = new long[vals.length];
    int[] exc = new int[limit];
    int nExc = 0;
    long prev = 0;
    for (int i = 0; i < vals.length; i++) {
      long q = exact(vals[i], isFloat, e);
      if (q != Long.MIN_VALUE) {
        prev = q;
        qs[i] = q;
      } else {
        if (nExc == limit) {
          return null;
        }
        exc[nExc++] = i;
        qs[i] = prev;
      }
    }
    return new Split(qs, java.util.Arrays.copyOf(exc, nExc));
  }
}
