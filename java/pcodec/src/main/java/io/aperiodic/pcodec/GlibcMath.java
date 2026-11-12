package io.aperiodic.pcodec;

/**
 * The libm functions the reference calls, bit for bit as it gets them on x86-64 Linux (glibc
 * 2.39, the libm of ubuntu-24.04): Rust's {@code f64::log2} and {@code f64::cbrt} call the
 * platform libm, and pcodec's mode heuristics depend on them.
 *
 * <p>{@link #log2} is glibc's {@code __log2_fma}, the variant glibc's ifunc selects on CPUs with
 * FMA and AVX2 (every x86-64 CI runner), transcribed from the compiled code, where GCC contracted
 * more multiply-adds than the C source shows. {@link #cbrt} is not glibc's (see there), and
 * {@link #pow10} is glibc's pow on the integer exponents pcodec gives it. VERIFICATION.md says
 * how each is checked against the reference.
 */
final class GlibcMath {
  private GlibcMath() {}

  private static final double INV_LN2_HI = 0x1.7154765200000p+0;
  private static final double INV_LN2_LO = 0x1.705fc2eefa200p-33;
  private static final double[] B = {
    -0x1.71547652b82fep-1,
    0x1.ec709dc3a03f7p-2,
    -0x1.71547652b7c3fp-2,
    0x1.2776c50f05be4p-2,
    -0x1.ec709dd768fe5p-3,
    0x1.a61761ec4e736p-3,
    -0x1.7153fbc64a79bp-3,
    0x1.484d154f01b4ap-3,
    -0x1.289e4a72c383cp-3,
    0x1.0b32f285aee66p-3,
  };
  private static final double[] A = {
    -0x1.71547652b8339p-1,
    0x1.ec709dc3a04bep-2,
    -0x1.7154764702ffbp-2,
    0x1.2776c50034c48p-2,
    -0x1.ec7b328ea92bcp-3,
    0x1.a6225e117f92ep-3,
  };
  /** {invc, logc} for each of the 64 subintervals */
  private static final double[] T = {
    0x1.724286bb1acf8p+0, -0x1.1095feecdb000p-1,
    0x1.6e1f766d2cca1p+0, -0x1.08494bd76d000p-1,
    0x1.6a13d0e30d48ap+0, -0x1.00143aee8f800p-1,
    0x1.661ec32d06c85p+0, -0x1.efec5360b4000p-2,
    0x1.623fa951198f8p+0, -0x1.dfdd91ab7e000p-2,
    0x1.5e75ba4cf026cp+0, -0x1.cffae0cc79000p-2,
    0x1.5ac055a214fb8p+0, -0x1.c043811fda000p-2,
    0x1.571ed0f166e1ep+0, -0x1.b0b67323ae000p-2,
    0x1.53909590bf835p+0, -0x1.a152f5a2db000p-2,
    0x1.5014fed61adddp+0, -0x1.9217f5af86000p-2,
    0x1.4cab88e487bd0p+0, -0x1.8304db0719000p-2,
    0x1.49539b4334feep+0, -0x1.74189f9a9e000p-2,
    0x1.460cbdfafd569p+0, -0x1.6552bb5199000p-2,
    0x1.42d664ee4b953p+0, -0x1.56b23a29b1000p-2,
    0x1.3fb01111dd8a6p+0, -0x1.483650f5fa000p-2,
    0x1.3c995b70c5836p+0, -0x1.39de937f6a000p-2,
    0x1.3991c4ab6fd4ap+0, -0x1.2baa1538d6000p-2,
    0x1.3698e0ce099b5p+0, -0x1.1d98340ca4000p-2,
    0x1.33ae48213e7b2p+0, -0x1.0fa853a40e000p-2,
    0x1.30d191985bdb1p+0, -0x1.01d9c32e73000p-2,
    0x1.2e025cab271d7p+0, -0x1.e857da2fa6000p-3,
    0x1.2b404cf13cd82p+0, -0x1.cd3c8633d8000p-3,
    0x1.288b02c7ccb50p+0, -0x1.b26034c14a000p-3,
    0x1.25e2263944de5p+0, -0x1.97c1c2f4fe000p-3,
    0x1.234563d8615b1p+0, -0x1.7d6023f800000p-3,
    0x1.20b46e33eaf38p+0, -0x1.633a71a05e000p-3,
    0x1.1e2eefdcda3ddp+0, -0x1.494f5e9570000p-3,
    0x1.1bb4a580b3930p+0, -0x1.2f9e424e0a000p-3,
    0x1.19453847f2200p+0, -0x1.162595afdc000p-3,
    0x1.16e06c0d5d73cp+0, -0x1.f9c9a75bd8000p-4,
    0x1.1485f47b7e4c2p+0, -0x1.c7b575bf9c000p-4,
    0x1.12358ad0085d1p+0, -0x1.960c60ff48000p-4,
    0x1.0fef00f532227p+0, -0x1.64ce247b60000p-4,
    0x1.0db2077d03a8fp+0, -0x1.33f78b2014000p-4,
    0x1.0b7e6d65980d9p+0, -0x1.0387d1a42c000p-4,
    0x1.0953efe7b408dp+0, -0x1.a6f9208b50000p-5,
    0x1.07325cac53b83p+0, -0x1.47a954f770000p-5,
    0x1.05197e40d1b5cp+0, -0x1.d23a8c50c0000p-6,
    0x1.03091c1208ea2p+0, -0x1.16a2629780000p-6,
    0x1.0101025b37e21p+0, -0x1.720f8d8e80000p-8,
    0x1.fc07ef9caa76bp-1, 0x1.6fe53b1500000p-7,
    0x1.f4465d3f6f184p-1, 0x1.11ccce10f8000p-5,
    0x1.ecc079f84107fp-1, 0x1.c4dfc8c8b8000p-5,
    0x1.e573a99975ae8p-1, 0x1.3aa321e574000p-4,
    0x1.de5d6f0bd3de6p-1, 0x1.918a0d08b8000p-4,
    0x1.d77b681ff38b3p-1, 0x1.e72e9da044000p-4,
    0x1.d0cb5724de943p-1, 0x1.1dcd2507f6000p-3,
    0x1.ca4b2dc0e7563p-1, 0x1.476ab03dea000p-3,
    0x1.c3f8ee8d6cb51p-1, 0x1.7074377e22000p-3,
    0x1.bdd2b4f020c4cp-1, 0x1.98ede8ba94000p-3,
    0x1.b7d6c006015cap-1, 0x1.c0db86ad2e000p-3,
    0x1.b20366e2e338fp-1, 0x1.e840aafcee000p-3,
    0x1.ac57026295039p-1, 0x1.0790ab4678000p-2,
    0x1.a6d01bc2731ddp-1, 0x1.1ac056801c000p-2,
    0x1.a16d3bc3ff18bp-1, 0x1.2db11d4fee000p-2,
    0x1.9c2d14967feadp-1, 0x1.406464ec58000p-2,
    0x1.970e4f47c9902p-1, 0x1.52dbe093af000p-2,
    0x1.920fb3982bcf2p-1, 0x1.651902050d000p-2,
    0x1.8d30187f759f1p-1, 0x1.771d2cdeaf000p-2,
    0x1.886e5ebb9f66dp-1, 0x1.88e9c857d9000p-2,
    0x1.83c97b658b994p-1, 0x1.9a80155e16000p-2,
    0x1.7f405ffc61022p-1, 0x1.abe186ed3d000p-2,
    0x1.7ad22181415cap-1, 0x1.bd0f2aea0e000p-2,
    0x1.767dcf99eff8cp-1, 0x1.ce0a43dbf4000p-2,
  };

  private static final long LO = Double.doubleToRawLongBits(1.0 - 0x1.5b51p-5);
  private static final long HI = Double.doubleToRawLongBits(1.0 + 0x1.6ab2p-5);
  private static final long OFF = 0x3fe6000000000000L;

  static double log2(double x) {
    long ix = Double.doubleToRawLongBits(x);
    int top = (int) (ix >>> 48);
    if (Long.compareUnsigned(ix - LO, HI - LO) < 0) {
      if (ix == 0x3ff0000000000000L) {
        return 0;
      }
      double r = x - 1.0;
      double hi = INV_LN2_HI * r;
      double r2 = r * r;
      double lo0 = Math.fma(INV_LN2_HI, r, -hi);
      double r4 = r2 * r2;
      double b01 = Math.fma(r, B[1], B[0]);
      double lo = Math.fma(r, INV_LN2_LO, lo0);
      double y = Math.fma(b01, r2, hi);
      double hiMinusY = hi - y;
      double t = Math.fma(b01, r2, hiMinusY);
      double b23 = Math.fma(r, B[3], B[2]);
      t = t + lo;
      double b45 = Math.fma(r, B[5], B[4]);
      double q1 = Math.fma(b45, r2, b23);
      double b67 = Math.fma(r, B[7], B[6]);
      double b89 = Math.fma(r, B[9], B[8]);
      double q2 = Math.fma(b89, r2, b67);
      double q = Math.fma(q2, r4, q1);
      q = Math.fma(q, r4, t);
      return y + q;
    }
    if (Integer.compareUnsigned(top - 0x0010, 0x7ff0 - 0x0010) >= 0) {
      if (ix * 2 == 0) {
        return Double.NEGATIVE_INFINITY;
      }
      if (ix == 0x7ff0000000000000L) {
        return x;
      }
      if ((top & 0x8000) != 0 || (top & 0x7ff0) == 0x7ff0) {
        return Double.NaN;
      }
      ix = Double.doubleToRawLongBits(x * 0x1p52);
      ix -= 52L << 52;
    }
    long tmp = ix - OFF;
    int i = (int) ((tmp >>> 46) & 63);
    int k = (int) (tmp >> 52);
    long iz = ix - (tmp & 0xfff0000000000000L);
    double invc = T[2 * i];
    double logc = T[2 * i + 1];
    double z = Double.longBitsToDouble(iz);
    double t3 = (double) k + logc;
    double r = Math.fma(z, invc, -1.0);
    double a01 = Math.fma(r, A[1], A[0]);
    double t1 = INV_LN2_HI * r;
    double t2a = Math.fma(INV_LN2_HI, r, -t1);
    double hi = t1 + t3;
    double lo = t3 - hi;
    double t2 = Math.fma(r, INV_LN2_LO, t2a);
    double r2 = r * r;
    lo = lo + t1;
    lo = lo + t2;
    double a23 = Math.fma(r, A[3], A[2]);
    double r4 = r2 * r2;
    double a45 = Math.fma(r, A[5], A[4]);
    double p = Math.fma(a23, r2, a01);
    p = Math.fma(a45, r4, p);
    double y = Math.fma(r2, p, lo);
    return y + hi;
  }

  // ------------------------------------------------------------ cbrt

  private static final double[] ESCALE = {1.0, 0x1.428a2f98d728bp+0, 0x1.965fea53d6e3dp+0};
  private static final double[] CC = {
    0x1.1b0babccfef9cp-1, 0x1.2c9a3e94d1da5p-1, -0x1.4dc30b1a1ddbap-3, 0x1.7a8d3e4ec9b07p-6
  };
  private static final double[] RSC = {1.0, -1.0, 0.5, -0.5, 0.25, -0.25};

  /**
   * Rust's {@code f64::cbrt}, which is not glibc's: the toolchain links its own {@code cbrt}
   * from {@code compiler_builtins} (the libm crate's port of CORE-MATH's correctly rounded
   * cbrt). This is that port, rounding to nearest.
   */
  static double cbrt(double x) {
    final long sigMask = (1L << 52) - 1;
    final double u0 = 0x1.5555555555555p-2;
    final double u1 = 0x1.c71c71c71c71cp-3;
    long hx = Double.doubleToRawLongBits(x);
    long mant = hx & sigMask;
    long sign = hx >>> 63;
    int e = (int) (hx >>> 52) & 0x7ff;
    if (((e + 1) & 0x7ff) < 2) {
      long ix = hx & 0x7fffffffffffffffL;
      if (e == 0x7ff || ix == 0) {
        return x + x;
      }
      int nz = Long.numberOfLeadingZeros(ix) - 11;
      mant <<= nz;
      mant &= sigMask;
      e = e - (nz - 1);
    }
    e = e + 3072;
    long cvt1 = mant | (0x3ffL << 52);
    long cvt5 = cvt1;
    int et = Integer.divideUnsigned(e, 3);
    int it = Integer.remainderUnsigned(e, 3);
    cvt5 += ((long) it) << 52;
    cvt5 |= sign << 63;
    double zz = Double.longBitsToDouble(cvt5);
    long isc = Double.doubleToRawLongBits(ESCALE[it]);
    isc |= sign << 63;
    double z = Double.longBitsToDouble(cvt1);
    double r = 1.0 / z;
    double rr = r * RSC[(it << 1) | (int) sign];
    double z2 = z * z;
    double c0 = CC[0] + z * CC[1];
    double c2 = CC[2] + z * CC[3];
    double y = c0 + z2 * c2;
    double y2 = y * y;
    double h = y2 * (y * r) - 1.0;
    y -= (h * y) * (u0 - u1 * h);
    y *= Double.longBitsToDouble(isc);
    y2 = y * y;
    double y2l = Math.fma(y, y, -y2);
    double y3 = y2 * y;
    double y3l = Math.fma(y, y2, -y3) + y * y2l;
    h = ((y3 - zz) + y3l) * rr;
    double dy = h * (y * u0);
    double y1 = y - dy;
    dy = (y - y1) - dy;
    double ady = Math.abs(dy);
    double off = 0x1p-53;
    double ady0 = Math.abs(ady - off);
    double ady1 = Math.abs(ady - (0x1p-52 + off));
    if (ady0 < 0x1p-75 || ady1 < 0x1p-75) {
      y2 = y1 * y1;
      y2l = Math.fma(y1, y1, -y2);
      y3 = y2 * y1;
      y3l = Math.fma(y1, y2, -y3) + y1 * y2l;
      h = ((y3 - zz) + y3l) * rr;
      dy = h * (y1 * u0);
      y = y1 - dy;
      dy = (y1 - y) - dy;
      y1 = y;
      ady = Math.abs(dy);
      ady0 = Math.abs(ady - off);
      ady1 = Math.abs(ady - (0x1p-52 + off));
      if (ady0 < 0x1p-98 || ady1 < 0x1p-98) {
        double azz = Math.abs(zz);
        if (azz == 0x1.9b78223aa307cp+1) {
          y1 = Math.copySign(0x1.79d15d0e8d59cp+0, zz);
        }
        if (azz == 0x1.a202bfc89ddffp+2) {
          y1 = Math.copySign(0x1.de87aa837820fp+0, zz);
        }
      }
    }
    long cvt3 = Double.doubleToRawLongBits(y1);
    cvt3 = cvt3 + (((long) (et - 342 - 1023)) << 52);
    long m0 = cvt3 << 30;
    long m1 = m0 >>> 63;
    if (Long.compareUnsigned(m0 ^ m1, 1L << 30) <= 0) {
      long cvt4 = Double.doubleToRawLongBits(y1);
      cvt4 = (cvt4 + (164L << 15)) & 0xffffffffffff0000L;
      if (Math.abs((Double.longBitsToDouble(cvt4) - y1) - dy) < 0x1p-60 || Math.abs(zz) == 1.0) {
        cvt3 = (cvt3 + (1L << 15)) & 0xffffffffffff0000L;
      }
    }
    return Double.longBitsToDouble(cvt3);
  }

  // ------------------------------------------------------------ pow(10, k)

  /**
   * {@code 10.0_f64.powf(k)} (glibc's pow) for integer-valued k: the correctly rounded power,
   * except where glibc's pow, accurate to 0.52 ULP, rounds the other way. That happens at two
   * exponents of the domain; MathParityTest checks every integer k from -400 to 400.
   */
  static double pow10(double k) {
    if (Double.isNaN(k)) {
      return Double.NaN;
    }
    if (k > 400) {
      return Double.POSITIVE_INFINITY;
    }
    if (k < -400) {
      return 0.0;
    }
    int e = (int) k;
    if (e == 23) {
      return Double.longBitsToDouble(0x44b52d02c7e14af7L);
    }
    if (e == 210) {
      return Double.longBitsToDouble(0x6b88557f31326bbcL);
    }
    return new java.math.BigDecimal(java.math.BigInteger.ONE, -e).doubleValue();
  }
}
