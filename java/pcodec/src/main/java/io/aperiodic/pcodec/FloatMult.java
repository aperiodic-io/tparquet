package io.aperiodic.pcodec;

import java.util.Arrays;

/** {@code pco::mode::float_mult}: numbers as {@code mult * base + adj ULPs}. */
final class FloatMult {
  private FloatMult() {}

  private static final int REQUIRED_PRECISION_BITS = 6;
  private static final double SNAP_THRESHOLD_ABSOLUTE = 0.02;
  private static final double SNAP_THRESHOLD_DECIMAL_RELATIVE = 0.01;
  private static final int INTERESTING_TRAILING_ZEROS = 5;
  private static final double REQUIRED_TRAILING_ZEROS_FREQUENCY = 0.5;
  private static final double REQUIRED_GCD_PAIR_FREQUENCY = 0.001;

  static final class Config {
    final long base;
    final long invBase;

    Config(long base, long invBase) {
      this.base = base;
      this.invBase = invBase;
    }

    static Config fromBase(FloatType f, long base) {
      return new Config(base, f.inv(base));
    }

    static Config fromInvBase(FloatType f, long invBase) {
      return new Config(f.inv(invBase), invBase);
    }
  }

  static Modes.Split splitLatents(NumberType t, FloatType f, long[] nums, Config c) {
    Latent lt = t.latent;
    long[] primary = new long[nums.length];
    long[] adj = new long[nums.length];
    for (int i = 0; i < nums.length; i++) {
      long num = nums[i];
      long mult = f.round(f.mul(num, c.invBase));
      primary[i] = f.intFloatToLatent(mult);
      adj[i] =
          lt.toggleCenter(lt.sub(t.toLatentOrdered(num), t.toLatentOrdered(f.mul(mult, c.base))));
    }
    return new Modes.Split(primary, adj);
  }

  private static long insignificantFloatTo(FloatType f, long x) {
    int spare = Math.max(f.precisionBits - REQUIRED_PRECISION_BITS, 0);
    return f.mul(x, f.exp2(-spare));
  }

  private static boolean isApproxZero(FloatType f, long small, long big) {
    return f.le(small, insignificantFloatTo(f, big));
  }

  private static boolean isSmallRemainder(FloatType f, long remainder, long original) {
    return f.le(remainder, f.mul(original, f.exp2(-16)));
  }

  private static boolean isImprecise(FloatType f, long value, long err) {
    return f.le(value, f.mul(err, f.exp2(REQUIRED_PRECISION_BITS)));
  }

  /** {@code approx_pair_gcd}; -1 for none (a float's bits are never -1 as a long) */
  static long approxPairGcd(FloatType f, long greater, long lesser) {
    if (isApproxZero(f, lesser, greater) || f.eq(lesser, greater)) {
      return -1;
    }
    long machineEps = f.exp2(-f.precisionBits);
    long gv = greater;
    long ge = f.zero();
    long lv = lesser;
    long le = f.zero();
    while (true) {
      long prev = gv;
      // rem_assign(p_greater, p_lesser)
      long ratio = f.round(f.div(gv, lv));
      ge = f.add(ge, f.add(f.mul(ratio, le), f.mul(gv, machineEps)));
      gv = f.abs(f.sub(gv, f.mul(ratio, lv)));
      if (isSmallRemainder(f, gv, prev) || f.le(gv, ge)) {
        return lv;
      }
      if (isApproxZero(f, gv, greater) || isImprecise(f, gv, ge)) {
        return -1;
      }
      long tv = gv;
      long te = ge;
      gv = lv;
      ge = le;
      lv = tv;
      le = te;
    }
  }

  static Config chooseConfigByTrailingZeros(NumberType t, FloatType f, long[] sample) {
    int precisionBits = f.precisionBits;
    long k = Integer.MAX_VALUE;
    int count = 0;
    for (long x : sample) {
      int tz = f.trailingZeros(x);
      if (!f.eq(x, f.zero()) && tz >= INTERESTING_TRAILING_ZEROS) {
        int kPrime = f.exponent(x) - Math.max(precisionBits - tz, 0);
        count++;
        k = Math.min(k, kPrime);
      }
    }
    long required =
        Math.max(
            RustMath.f64ToUnsigned(Math.ceil(sample.length * REQUIRED_TRAILING_ZEROS_FREQUENCY), 64),
            Modes.MIN_SAMPLE);
    if (count < required) {
      return null;
    }
    Latent lt = t.latent;
    int lshift = lt.bits - precisionBits - 1;
    long explicitMantissa = lt.mid;
    long[] intSample = new long[sample.length];
    int len = 0;
    int ki = (int) k;
    for (long x : sample) {
      int exponent = f.exponent(x);
      int kPrime = exponent - Math.max(precisionBits - f.trailingZeros(x), 0);
      if (kPrime >= ki && exponent < ki + lt.bits) {
        int rshift = lt.bits - 1 - (exponent - ki);
        long shifted = ((x << lshift) & lt.mask) | explicitMantissa;
        intSample[len++] = shifted >>> rshift;
      }
    }
    if (len >= required) {
      long[] cand = Modes.IntMult.chooseCandidateBase(Arrays.copyOf(intSample, len));
      long intBase = cand == null ? 1 : cand[0];
      long base = f.mul(f.fromLatentNumerical(intBase), f.exp2(ki));
      return Config.fromBase(f, base);
    }
    return null;
  }

  static long approxSampleGcdEuclidean(FloatType f, long[] sample) {
    long[] gcds = new long[sample.length / 2 + 1];
    int n = 0;
    for (int i = 0; i + 1 < sample.length; i += 2) {
      long a = sample[i];
      long b = sample[i + 1];
      long g = approxPairGcd(f, f.max(a, b), f.min(a, b));
      if (g != -1) {
        gcds[n++] = g;
      }
    }
    long required =
        1 + RustMath.f64ToUnsigned(Math.ceil(sample.length * REQUIRED_GCD_PAIR_FREQUENCY), 64);
    if (n < required) {
      return -1;
    }
    long[] sorted = Arrays.copyOf(gcds, n);
    // positive finite floats: their bits sort as their values
    Arrays.sort(sorted);
    long pointZeroOne = f.fromF64(0.01);
    for (double percentile : new double[] {0.1, 0.3, 0.5}) {
      long candidate = sorted[(int) RustMath.f64ToUnsigned(percentile * n, 64)];
      long similar = 0;
      long tol = f.mul(pointZeroOne, candidate);
      for (int i = 0; i < n; i++) {
        if (f.lt(f.abs(f.sub(sorted[i], candidate)), tol)) {
          similar++;
        }
      }
      if (similar >= required) {
        return candidate;
      }
    }
    return -1;
  }

  static long centerSampleBase(FloatType f, long base, long[] sample) {
    long invBase = f.inv(base);
    long tweakSum = f.zero();
    long tweakWeight = f.zero();
    for (long x : sample) {
      long mult = f.round(f.mul(x, invBase));
      int multExponent = f.exponent(mult);
      // `as Bitlen` reinterprets a negative exponent as a large unsigned value
      if (Integer.compareUnsigned(multExponent, f.precisionBits) < 0 && !f.eq(mult, f.zero())) {
        long overshoot = f.sub(f.mul(mult, base), x);
        long weight = f.fromF64((double) (f.precisionBits - multExponent));
        tweakSum = f.add(tweakSum, f.mul(weight, f.div(overshoot, mult)));
        tweakWeight = f.add(tweakWeight, weight);
      }
    }
    return f.sub(base, f.div(tweakSum, tweakWeight));
  }

  static double pow10(double k) {
    return GlibcMath.pow10(k);
  }

  static Config snapToIntReciprocal(FloatType f, long base) {
    long invBase = f.inv(base);
    long roundInvBase = f.round(invBase);
    double log10 = Math.log10(f.toF64(invBase));
    long decimalInvBase = f.fromF64(pow10(RustMath.roundF64(log10)));
    if (f.lt(f.abs(f.sub(invBase, roundInvBase)), f.fromF64(SNAP_THRESHOLD_ABSOLUTE))) {
      return Config.fromInvBase(f, roundInvBase);
    } else if (f.lt(
        f.div(f.abs(f.sub(invBase, decimalInvBase)), invBase),
        f.fromF64(SNAP_THRESHOLD_DECIMAL_RELATIVE))) {
      return Config.fromInvBase(f, decimalInvBase);
    }
    return Config.fromBase(f, base);
  }

  static Config chooseConfigByEuclidean(FloatType f, long[] sample) {
    long base = approxSampleGcdEuclidean(f, sample);
    if (base == -1) {
      return null;
    }
    base = centerSampleBase(f, base, sample);
    return snapToIntReciprocal(f, base);
  }

  /** {@code bits_saved_per_num_over_classic}; NaN for none */
  static double bitsSavedPerNumOverClassic(NumberType t, FloatType f, Config c, long[] sample) {
    Latent lt = t.latent;
    double saved =
        Modes.estBitsSavedPerNum(
            sample,
            (x, out) -> {
              long mult = f.round(f.mul(x, c.invBase));
              out[0] = f.intFloatToLatent(mult);
              int interBaseBits = Math.max(f.precisionBits - f.exponent(mult), 0);
              // saturating_sub on Bitlen: a negative exponent as u32 is huge, giving 0
              if (f.exponent(mult) < 0) {
                interBaseBits = 0;
              }
              long approxUnsigned = t.toLatentOrdered(f.mul(mult, c.base));
              long xu = t.toLatentOrdered(x);
              long absAdj = Latent.max(xu, approxUnsigned) - Latent.min(xu, approxUnsigned);
              int adjBits = 1 + 2 * (lt.bits - lt.leadingZeros(absAdj));
              return (double) interBaseBits - (double) adjBits;
            });
    return saved >= Modes.MULT_REQUIRED_BITS_SAVED_PER_NUM ? saved : Double.NaN;
  }

  static Modes.Bid computeBid(NumberType t, FloatType f, long[] sample) {
    Config[] candidates = {
      chooseConfigByTrailingZeros(t, f, sample), chooseConfigByEuclidean(f, sample)
    };
    Config best = null;
    double bestSaved = 0;
    for (Config c : candidates) {
      if (c == null) {
        continue;
      }
      double s = bitsSavedPerNumOverClassic(t, f, c, sample);
      if (Double.isNaN(s)) {
        continue;
      }
      if (best == null || RustMath.totalCmp(s, bestSaved) >= 0) {
        best = c;
        bestSaved = s;
      }
    }
    if (best == null) {
      return null;
    }
    Config c = best;
    return new Modes.Bid(
        Mode.floatMult(t.toLatentOrdered(c.base)), bestSaved, xs -> splitLatents(t, f, xs, c));
  }
}
