package io.aperiodic.pcodec;

import java.util.ArrayList;
import java.util.List;
import java.util.SplittableRandom;

/**
 * Deterministic test data in the shapes pcodec's heuristics react to: every mode (classic, int
 * mult, float mult, float quant, dict), every delta encoding (consecutive orders, lookback
 * patterns, smooth trends for conv1), and edge values (extremes, NaN payloads, signed zeros,
 * infinities, subnormals), at sizes around the batch (256), sample (10) and page boundaries.
 */
final class Corpus {
  private Corpus() {}

  static final String[] SHAPES = {
    "uniform", "small", "constant", "walk", "trend", "quadratic", "mult", "multNoise",
    "periodic", "spiky", "dictish", "decimal", "floatMult", "quant", "special", "sorted",
    "sparse", "timestamps", "prices", "bimodal", "steps", "mixed"
  };

  static final int[] SIZES = {
    1, 2, 3, 9, 10, 11, 40, 49, 50, 100, 255, 256, 257, 511, 512, 513, 1000, 2001, 4096, 10007,
    20000
  };

  /** a case: a type, a shape, a size, a seed, and the numbers */
  static final class Case {
    final NumberType type;
    final String shape;
    final int n;
    final long seed;
    final long[] nums;

    Case(NumberType type, String shape, int n, long seed, long[] nums) {
      this.type = type;
      this.shape = shape;
      this.n = n;
      this.seed = seed;
      this.nums = nums;
    }

    @Override
    public String toString() {
      return type + "/" + shape + "/n=" + n + "/seed=" + seed;
    }
  }

  static long mask(NumberType t, long x) {
    return t.bits() == 64 ? x : x & ((1L << t.bits()) - 1);
  }

  static long fromDouble(NumberType t, double d) {
    switch (t) {
      case F64:
        return Double.doubleToRawLongBits(d);
      case F32:
        return Float.floatToRawIntBits((float) d) & 0xffffffffL;
      case F16:
        return FloatType.f32BitsToF16Bits(Float.floatToRawIntBits((float) d) & 0xffffffffL);
      default:
        throw new IllegalArgumentException();
    }
  }

  /** an integer value of the type from a signed long (wrapping to the width) */
  static long fromLong(NumberType t, long v) {
    return mask(t, v);
  }

  static long special(NumberType t, SplittableRandom r) {
    if (t.isFloat()) {
      FloatType f = FloatType.of(t);
      long sign = f.signMask;
      long exp = f.expMask;
      long man = (1L << f.precisionBits) - 1;
      switch (r.nextInt(10)) {
        case 0:
          return exp | (r.nextLong() & man) | 1; // NaN with payload
        case 1:
          return sign | exp | (r.nextLong() & man) | 1;
        case 2:
          return exp; // inf
        case 3:
          return sign | exp;
        case 4:
          return sign; // -0
        case 5:
          return 0;
        case 6:
          return r.nextLong() & man; // subnormal
        case 7:
          return exp - 1; // MAX
        case 8:
          return sign | (exp - 1);
        default:
          return 1;
      }
    }
    switch (r.nextInt(5)) {
      case 0:
        return 0;
      case 1:
        return mask(t, -1);
      case 2:
        return t.latent.mid;
      case 3:
        return t.latent.mid - 1;
      default:
        return 1;
    }
  }

  static long[] generate(NumberType t, String shape, int n, long seed) {
    SplittableRandom r = new SplittableRandom(seed * 1000003L + shape.hashCode() * 31L + t.ordinal());
    long[] v = new long[n];
    boolean fl = t.isFloat();
    int bits = t.bits();
    double scale = fl ? 1.0 : Math.min(Math.pow(2, bits - 2), 1e15);
    switch (shape) {
      case "uniform":
        for (int i = 0; i < n; i++) {
          v[i] = mask(t, r.nextLong());
          if (fl && FloatType.of(t).isNan(v[i]) && r.nextBoolean()) {
            v[i] = 0;
          }
        }
        break;
      case "small":
        for (int i = 0; i < n; i++) {
          v[i] = fl ? fromDouble(t, r.nextInt(100) - 50) : fromLong(t, r.nextInt(16) - 4);
        }
        break;
      case "constant":
        {
          long c = fl ? fromDouble(t, r.nextDouble() * 1000) : mask(t, r.nextLong());
          for (int i = 0; i < n; i++) {
            v[i] = c;
          }
          break;
        }
      case "walk":
        {
          double x = r.nextDouble() * 100;
          long y = r.nextLong() >> (64 - Math.min(bits, 40));
          for (int i = 0; i < n; i++) {
            x += r.nextGaussian();
            y += r.nextInt(201) - 100;
            v[i] = fl ? fromDouble(t, x) : fromLong(t, y);
          }
          break;
        }
      case "trend":
        {
          double a = r.nextDouble() * 5;
          for (int i = 0; i < n; i++) {
            double x = 1000 + a * i + 30 * Math.sin(i / 17.0) + r.nextGaussian();
            v[i] = fl ? fromDouble(t, x) : fromLong(t, Math.round(x));
          }
          break;
        }
      case "quadratic":
        for (int i = 0; i < n; i++) {
          long x = 7L * i * i - 3L * i + 11;
          v[i] = fl ? fromDouble(t, x * 0.5) : fromLong(t, x);
        }
        break;
      case "mult":
        {
          long base = 2 + r.nextInt(1000);
          for (int i = 0; i < n; i++) {
            long m = r.nextInt(100000) - 5000;
            v[i] = fl ? fromDouble(t, m * 0.25) : fromLong(t, m * base + 7);
          }
          break;
        }
      case "multNoise":
        {
          long base = 3 + r.nextInt(100);
          for (int i = 0; i < n; i++) {
            long m = r.nextInt(10000);
            long adj = r.nextInt(20) == 0 ? r.nextInt((int) base) : 0;
            v[i] = fl ? fromDouble(t, m * 1.7 + adj * 1e-9) : fromLong(t, m * base + adj);
          }
          break;
        }
      case "periodic":
        {
          int period = 3 + r.nextInt(40);
          long[] pat = new long[period];
          for (int i = 0; i < period; i++) {
            pat[i] = fl ? fromDouble(t, r.nextDouble() * 100) : mask(t, r.nextLong() >> 20);
          }
          for (int i = 0; i < n; i++) {
            v[i] = r.nextInt(50) == 0 ? (fl ? fromDouble(t, r.nextDouble()) : mask(t, r.nextLong())) : pat[i % period];
          }
          break;
        }
      case "spiky":
        for (int i = 0; i < n; i++) {
          v[i] = r.nextInt(30) == 0
              ? (fl ? fromDouble(t, r.nextDouble() * 1e9) : mask(t, r.nextLong()))
              : (fl ? fromDouble(t, 5) : fromLong(t, 5));
        }
        break;
      case "dictish":
        {
          long[] dict = new long[1 + r.nextInt(64)];
          for (int i = 0; i < dict.length; i++) {
            dict[i] = fl ? fromDouble(t, r.nextDouble() * 1e6) : mask(t, r.nextLong());
          }
          for (int i = 0; i < n; i++) {
            v[i] = dict[Math.min(dict.length - 1, (int) Math.abs(r.nextGaussian() * dict.length / 3))];
          }
          break;
        }
      case "decimal":
        {
          int digits = r.nextInt(5);
          double p = Math.pow(10, digits);
          for (int i = 0; i < n; i++) {
            double x = Math.round((100 + r.nextGaussian() * 20) * p) / p;
            v[i] = fl ? fromDouble(t, x) : fromLong(t, (long) (x * p));
          }
          break;
        }
      case "floatMult":
        {
          double base = new double[] {0.1, 0.01, 0.05, 1.0 / 3, Math.PI, 0.001, 2.5}[r.nextInt(7)];
          for (int i = 0; i < n; i++) {
            double x = (r.nextInt(20000) - 1000) * base;
            v[i] = fl ? fromDouble(t, x) : fromLong(t, (long) x);
          }
          break;
        }
      case "quant":
        {
          int k = fl ? 1 + r.nextInt(FloatType.of(t).precisionBits) : 4;
          for (int i = 0; i < n; i++) {
            long x = fl ? fromDouble(t, r.nextGaussian() * 1000) : mask(t, r.nextLong());
            long low = r.nextInt(100) == 0 ? 1 : 0;
            v[i] = mask(t, (x >>> k << k) | low);
          }
          break;
        }
      case "special":
        for (int i = 0; i < n; i++) {
          v[i] = r.nextInt(3) == 0 ? special(t, r) : (fl ? fromDouble(t, r.nextGaussian()) : mask(t, r.nextInt(1000)));
        }
        break;
      case "sorted":
        {
          long x = fl ? 0 : mask(t, -1) >>> 2;
          double d = -1000;
          for (int i = 0; i < n; i++) {
            x += r.nextInt(1000);
            d += r.nextDouble();
            v[i] = fl ? fromDouble(t, d) : mask(t, x);
          }
          break;
        }
      case "sparse":
        for (int i = 0; i < n; i++) {
          v[i] = r.nextInt(100) == 0 ? (fl ? fromDouble(t, r.nextDouble()) : mask(t, r.nextInt(1 << 20))) : 0;
        }
        break;
      case "timestamps":
        {
          long ts = 1735689600000L + r.nextInt(1 << 30);
          for (int i = 0; i < n; i++) {
            ts += 1000 * (1 + r.nextInt(3));
            v[i] = fl ? fromDouble(t, ts / 1000.0) : fromLong(t, bits >= 64 ? ts * 1000 : ts / 1000);
          }
          break;
        }
      case "prices":
        {
          double px = 100 + r.nextDouble() * 900;
          for (int i = 0; i < n; i++) {
            px = Math.max(0.01, px + (r.nextInt(5) - 2) * 0.01);
            double rounded = Math.round(px * 100) / 100.0;
            v[i] = fl ? fromDouble(t, rounded) : fromLong(t, Math.round(rounded * 100));
          }
          break;
        }
      case "bimodal":
        for (int i = 0; i < n; i++) {
          double x = r.nextBoolean() ? r.nextGaussian() : 1e6 + r.nextGaussian() * 10;
          v[i] = fl ? fromDouble(t, x) : fromLong(t, (long) (x * (scale / 1e7)));
        }
        break;
      case "steps":
        {
          long level = 0;
          for (int i = 0; i < n; i++) {
            if (r.nextInt(100) == 0) {
              level = r.nextInt(1 << 16);
            }
            v[i] = fl ? fromDouble(t, level * 0.125) : fromLong(t, level);
          }
          break;
        }
      default: // mixed: concatenated segments of other shapes
        {
          int at = 0;
          while (at < n) {
            int len = Math.min(n - at, 1 + r.nextInt(Math.max(1, n / 3)));
            String s = SHAPES[r.nextInt(SHAPES.length - 1)];
            long[] part = generate(t, s, len, r.nextLong());
            System.arraycopy(part, 0, v, at, len);
            at += len;
          }
          break;
        }
    }
    return v;
  }

  static List<Case> cases(NumberType[] types, int[] sizes, int seeds) {
    List<Case> res = new ArrayList<>();
    for (NumberType t : types) {
      for (String shape : SHAPES) {
        for (int n : sizes) {
          for (int s = 0; s < seeds; s++) {
            res.add(new Case(t, shape, n, s, generate(t, shape, n, s)));
          }
        }
      }
    }
    return res;
  }
}
