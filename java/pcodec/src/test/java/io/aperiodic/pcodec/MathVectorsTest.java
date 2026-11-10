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

  /** CORE-MATH cbrt inputs that take its refinement path (found by search) */
  static final long[] CBRT_HARD = {
    0x3ff1f1e6dc071dffL, 0x3ffa670b505cc14bL, 0x400cd890ef063cb9L, 0x401916ba6c759d0eL,
    0x3ff3b50335b9ef42L, 0x400ee756f9c5ba04L, 0x400ac14707096b59L, 0x3ff3369f61436606L,
    0x40190fa671f29631L, 0x3ffca42e765d6599L, 0x3ff3107b313ce86aL, 0x401ee9984eca0061L,
    0x40138be6e2988e7cL, 0x4019aaa9212d7cbcL, 0x3fff1b00909eed7cL, 0x40114735b72680e6L,
    0x4005369caeecac62L, 0x401847f2b550af38L, 0x4001069586787782L, 0x4005e29c04288a28L,
    0x401875dd0e0b862eL, 0x400f8270467c5f3aL, 0x401cb1387eb13cc1L, 0x40199c67db770927L,
    0x401713d6f52d4ec2L, 0x401ce58eb9f9192eL, 0x3ff0bb4d1785b800L, 0x40199b9bf0e3f4c4L,
    0x40192cf5c406f7c5L, 0x400e2b9853db078aL, 0x40082081b7de56daL, 0x400bfd0f3ea42e89L,
    0x4000cfaa9bc81a80L, 0x400d39848612cc4fL, 0x3ffbedf685d8b6dfL, 0x400c3a38e0aa73d0L,
    0x40017ec75a35f3d6L, 0x40157da56bb7928bL, 0x3ff64eb4436730b0L, 0x401f59ac1124a27cL,
    // the two hard cases CORE-MATH corrects by hand
    Double.doubleToRawLongBits(0x1.9b78223aa307cp+1), Double.doubleToRawLongBits(0x1.a202bfc89ddffp+2),
  };

  /** edge values for every function: zeros, subnormals, rounding boundaries, cast limits, ... */
  static double[] specials() {
    java.util.List<Double> v = new ArrayList<>();
    double[] base = {
      0.0, -0.0, Double.MIN_VALUE, -Double.MIN_VALUE, 0x1p-1060, 0x1.8p-1040, Double.MIN_NORMAL,
      -Double.MIN_NORMAL, 0.5, 0.49999999999999994, 1.0, Math.nextUp(1.0), Math.nextDown(1.0),
      1.5, 2.0, 2.5, -2.5, 3.0, 27.0, 0.125, 1.0 - 0x1.5b51p-5, 1.0 + 0x1.6ab2p-5,
      Math.nextDown(1.0 - 0x1.5b51p-5), Math.nextUp(1.0 + 0x1.6ab2p-5), 0x1p16, 0x1p16 - 0.5,
      0x1p31, 0x1p32, Math.nextDown(0x1p32), 0x1p52, 0x1p52 + 0.5, 0x1p53, 0x1p63, Math.nextDown(0x1p63),
      0x1p64, Math.nextDown(0x1p64), 1e300, Double.MAX_VALUE, -1e300, -0.5, -1.0, -7.25,
      Double.POSITIVE_INFINITY, Double.NEGATIVE_INFINITY, Double.NaN,
      Double.longBitsToDouble(0xfff8000000000001L), Double.longBitsToDouble(0x7ff0000000000001L),
      65504.0, 65520.0, 6.1e-5, 5.96e-8, 0x1p-25, 0x1.8p-25, 1e15 + 0.5, 4503599627370495.5,
    };
    for (double b : base) {
      v.add(b);
      v.add(-b);
      for (int k = -3; k <= 3; k++) {
        v.add(b * Math.pow(8, k));
      }
    }
    for (long h : CBRT_HARD) {
      double x = Double.longBitsToDouble(h);
      v.add(x);
      v.add(-x);
      v.add(x * 0x1p-300);
      v.add(x * 0x1p600);
    }
    for (int k = 0; k <= 1000; k++) {
      v.add((double) k * k * k);
      v.add((double) k * k * k / 4096.0);
    }
    double[] res = new double[v.size()];
    for (int i = 0; i < res.length; i++) {
      res[i] = v.get(i);
    }
    return res;
  }

  /** the integer-valued arguments pow10 receives, and the non-finite ones */
  static double[] powArgs() {
    double[] res = new double[810];
    for (int i = 0; i < 803; i++) {
      res[i] = i - 401;
    }
    res[803] = Double.NaN;
    res[804] = Double.POSITIVE_INFINITY;
    res[805] = Double.NEGATIVE_INFINITY;
    res[806] = 1e9;
    res[807] = -1e9;
    res[808] = 1e300;
    res[809] = -1e300;
    return res;
  }

  /** Java's results of the oracle's math functions 0..12 (op 6) */
  static long javaMath(int fn, double x) {
    long r = Double.doubleToRawLongBits(x);
    switch (fn) {
      case 0:
        return Double.doubleToRawLongBits(GlibcMath.log2(x));
      case 1:
        return Double.doubleToRawLongBits(GlibcMath.cbrt(x));
      case 2:
        return Double.doubleToRawLongBits(GlibcMath.pow10(x));
      case 4:
        return RustMath.f64ToUnsigned(x, 64);
      case 5:
        return RustMath.f64ToUnsigned(x, 32);
      case 6:
        return RustMath.f64ToUnsigned(x, 16);
      case 7:
        return RustMath.f64ToI64(x);
      case 8:
        return Double.isNaN(x) ? 0 : Double.doubleToRawLongBits(RustMath.roundF64(x));
      case 9:
        {
          float y = Float.intBitsToFloat((int) FloatType.f64ToF32Bits(x));
          return Float.isNaN(y) ? 0 : Float.floatToRawIntBits(RustMath.roundF32(y)) & 0xffffffffL;
        }
      case 10:
        return Double.doubleToRawLongBits(Latent.toF64(r));
      case 11:
        return Float.floatToRawIntBits(RustMath.u64ToF32(r)) & 0xffffffffL;
      default:
        return FloatType.F16.fromF64(x);
    }
  }

  /** functions with special-value lists; log2 of NaN and negatives only up to the NaN's bits */
  static final int[] SPECIAL_FNS = {0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12};

  static double[] args(int fn) {
    return fn == 2 ? powArgs() : specials();
  }

  /** results that only need to agree as NaN or not (glibc's log2 NaN payloads are not pcodec's) */
  static long canonical(int fn, long bits) {
    if ((fn == 0 || fn == 1) && Double.isNaN(Double.longBitsToDouble(bits))) {
      return 0x7ff8000000000000L;
    }
    return bits;
  }

  static long digest(int fn, long[] results) {
    long h = 0xcbf29ce484222325L;
    for (long x : results) {
      h = (h ^ canonical(fn, x)) * 0x100000001b3L;
    }
    return h;
  }

  static long javaSpecialDigest(int fn) {
    double[] xs = args(fn);
    long[] res = new long[xs.length];
    for (int i = 0; i < xs.length; i++) {
      res[i] = javaMath(fn, xs[i]);
    }
    return digest(fn, res);
  }

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
    for (int fn : SPECIAL_FNS) {
      lines.add("special " + fn + " " + Long.toHexString(digest(fn, MathParityTest.o.mathBits(fn, args(fn)))));
    }
    return lines;
  }

  @Test
  void javaGivesTheRecordedDigests() throws Exception {
    List<String> lines = Files.readAllLines(path(), StandardCharsets.UTF_8);
    assertEquals(DOMAINS.length + SPECIAL_FNS.length, lines.size());
    for (int k = 0; k < SPECIAL_FNS.length; k++) {
      String[] f = lines.get(DOMAINS.length + k).split(" ");
      assertEquals("special " + SPECIAL_FNS[k], f[0] + " " + f[1]);
      assertEquals(f[2], Long.toHexString(javaSpecialDigest(SPECIAL_FNS[k])), "special values of function " + SPECIAL_FNS[k]);
    }
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
