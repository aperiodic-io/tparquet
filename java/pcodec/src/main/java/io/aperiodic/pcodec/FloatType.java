package io.aperiodic.pcodec;

/**
 * The float arithmetic of pcodec's float modes ({@code pco::data_types::float::Float}) for f16,
 * f32 and f64, on raw bits. Each operation rounds once to the type, as Rust's operators do.
 *
 * <p>NaN results follow x86-64 SSE, the platform of the reference: an operation with a NaN
 * operand returns the first NaN operand, quieted; an invalid operation returns the default NaN
 * (sign set, quiet). Java does not specify NaN payloads, so they are computed explicitly here.
 * f16 arithmetic is f32 arithmetic rounded back to f16 ({@code half} with F16C on x86-64).
 */
abstract class FloatType {
  final int bits;
  final int precisionBits;
  final int expOffset;
  final Latent latent;
  final long signMask;
  final long absMask;
  final long expMask;
  final long quietBit;
  final long defaultNan;

  // the masks are given as constants, not derived (MathParityTest and ExhaustiveParityTest
  // check every f16 and f32 bit pattern through them)
  private FloatType(
      int bits,
      int precisionBits,
      int expOffset,
      Latent latent,
      long signMask,
      long absMask,
      long expMask,
      long quietBit,
      long defaultNan) {
    this.bits = bits;
    this.precisionBits = precisionBits;
    this.expOffset = expOffset;
    this.latent = latent;
    this.signMask = signMask;
    this.absMask = absMask;
    this.expMask = expMask;
    this.quietBit = quietBit;
    this.defaultNan = defaultNan;
  }

  static FloatType of(NumberType t) {
    switch (t) {
      case F16:
        return F16;
      case F32:
        return F32;
      case F64:
        return F64;
      default:
        throw new IllegalArgumentException(t + " is not a float");
    }
  }

  // ------------------------------------------------------------ classification

  final boolean isNan(long x) {
    return (x & absMask) > expMask;
  }

  final boolean isFinite(long x) {
    return (x & expMask) != expMask;
  }

  final boolean isNormal(long x) {
    long e = x & expMask;
    return e != 0 && e != expMask;
  }

  final boolean isSignPositive(long x) {
    return (x & signMask) == 0;
  }

  final long abs(long x) {
    return x & absMask;
  }

  final long quiet(long x) {
    return x | quietBit;
  }

  /** {@code exponent()}: the biased exponent field minus the offset */
  final int exponent(long x) {
    return (int) (abs(x) >>> precisionBits) - expOffset;
  }

  /** {@code to_bits().trailing_zeros()} */
  final int trailingZeros(long x) {
    return x == 0 ? bits : Long.numberOfTrailingZeros(x);
  }

  /** {@code exp2(power)}: {@code from_bits(((offset + power) as L) << PRECISION_BITS)} */
  final long exp2(int power) {
    return (((long) (expOffset + power)) << precisionBits) & latent.mask;
  }

  final long zero() {
    return 0;
  }

  /** {@code MAX * 0.5} */
  final long maxForSampling() {
    // MAX is exponent field all ones but the lowest bit, mantissa all ones; half of it lowers
    // the exponent by one
    long max = (expMask - (1L << precisionBits)) | ((1L << precisionBits) - 1);
    return max - (1L << precisionBits);
  }

  // ------------------------------------------------------------ comparison (values, NaN false)

  abstract double value(long x);

  final boolean lt(long a, long b) {
    return value(a) < value(b);
  }

  final boolean le(long a, long b) {
    return value(a) <= value(b);
  }

  final boolean eq(long a, long b) {
    return value(a) == value(b);
  }

  /** {@code F::max} (a NaN operand is ignored) */
  final long max(long a, long b) {
    if (isNan(a)) {
      return b;
    }
    if (isNan(b)) {
      return a;
    }
    return lt(a, b) ? b : a;
  }

  final long min(long a, long b) {
    if (isNan(a)) {
      return b;
    }
    if (isNan(b)) {
      return a;
    }
    return lt(b, a) ? b : a;
  }

  // ------------------------------------------------------------ arithmetic

  /** the operation on non-NaN operands, rounded once */
  abstract long rawOp(int op, long a, long b);

  static final int ADD = 0;
  static final int SUB = 1;
  static final int MUL = 2;
  static final int DIV = 3;

  private long op(int op, long a, long b) {
    if (isNan(a)) {
      return quiet(a);
    }
    if (isNan(b)) {
      return quiet(b);
    }
    long r = rawOp(op, a, b);
    return isNan(r) ? defaultNan : r;
  }

  final long add(long a, long b) {
    return op(ADD, a, b);
  }

  final long sub(long a, long b) {
    return op(SUB, a, b);
  }

  final long mul(long a, long b) {
    return op(MUL, a, b);
  }

  final long div(long a, long b) {
    return op(DIV, a, b);
  }

  /** {@code 1.0 / x} */
  final long inv(long x) {
    return div(one(), x);
  }

  abstract long one();

  /** {@code round()}: half away from zero; a NaN is quieted */
  abstract long round(long x);

  /** {@code from_f64}: {@code x as f32}, or f64 to f32 to f16 */
  abstract long fromF64(double x);

  /** {@code to_f64}: exact */
  abstract double toF64(long x);

  /** {@code from_latent_numerical}: {@code l as F} (an unsigned integer, rounded) */
  abstract long fromLatentNumerical(long l);

  /** {@code abs as Self::L}: saturating, NaN to 0 */
  abstract long toUnsignedSaturating(long absValue);

  /** {@code int_float_to_latent} */
  final long intFloatToLatent(long x) {
    long a = abs(x);
    long gpi = 1L << (precisionBits + 1);
    long gpiFloat = fromLatentNumerical(gpi);
    long absInt;
    if (lt(a, gpiFloat)) {
      absInt = toUnsignedSaturating(a);
    } else {
      absInt = latent.add(gpi, latent.sub(a, gpiFloat));
    }
    if (isSignPositive(x)) {
      return latent.add(latent.mid, absInt);
    }
    return latent.sub(latent.sub(latent.mid, 1), absInt);
  }

  /** {@code int_float_from_latent} */
  final long intFloatFromLatent(long l) {
    long sign = latent.mid;
    long absMaskL = sign - 1;
    long gpi = 1L << (precisionBits + 1);
    long gpiBitsOffset = latent.sub(fromLatentNumerical(gpi), gpi);
    long neg = (~latent.sub(0, l >>> (latent.bits - 1))) & latent.mask;
    long absInt = (l ^ neg) & absMaskL;
    long absFloat;
    if (Latent.lt(absInt, gpi)) {
      absFloat = fromLatentNumerical(absInt);
    } else {
      absFloat = latent.add(absInt, gpiBitsOffset);
    }
    return absFloat | (neg & sign);
  }

  // ------------------------------------------------------------ instances

  static final FloatType F64 =
      new FloatType(
          64, 52, 1023, Latent.U64, 0x8000000000000000L, 0x7fffffffffffffffL,
          0x7ff0000000000000L, 0x0008000000000000L, 0xfff8000000000000L) {
        @Override
        double value(long x) {
          return Double.longBitsToDouble(x);
        }

        @Override
        long rawOp(int op, long a, long b) {
          double x = Double.longBitsToDouble(a);
          double y = Double.longBitsToDouble(b);
          double r;
          switch (op) {
            case ADD:
              r = x + y;
              break;
            case SUB:
              r = x - y;
              break;
            case MUL:
              r = x * y;
              break;
            default:
              r = x / y;
              break;
          }
          return Double.isNaN(r) ? defaultNan : Double.doubleToRawLongBits(r);
        }

        @Override
        long one() {
          return Double.doubleToRawLongBits(1.0);
        }

        @Override
        long round(long x) {
          if (isNan(x)) {
            return quiet(x);
          }
          return Double.doubleToRawLongBits(RustMath.roundF64(Double.longBitsToDouble(x)));
        }

        @Override
        long fromF64(double x) {
          return Double.doubleToRawLongBits(x);
        }

        @Override
        double toF64(long x) {
          return Double.longBitsToDouble(x);
        }

        @Override
        long fromLatentNumerical(long l) {
          return Double.doubleToRawLongBits(Latent.toF64(l));
        }

        @Override
        long toUnsignedSaturating(long a) {
          return RustMath.f64ToUnsigned(Double.longBitsToDouble(a), 64);
        }
      };

  static final FloatType F32 =
      new FloatType(32, 23, 127, Latent.U32, 0x80000000L, 0x7fffffffL, 0x7f800000L, 0x00400000L, 0xffc00000L) {
        @Override
        double value(long x) {
          return Float.intBitsToFloat((int) x);
        }

        @Override
        long rawOp(int op, long a, long b) {
          float x = Float.intBitsToFloat((int) a);
          float y = Float.intBitsToFloat((int) b);
          float r;
          switch (op) {
            case ADD:
              r = x + y;
              break;
            case SUB:
              r = x - y;
              break;
            case MUL:
              r = x * y;
              break;
            default:
              r = x / y;
              break;
          }
          return Float.isNaN(r) ? defaultNan : Float.floatToRawIntBits(r) & 0xffffffffL;
        }

        @Override
        long one() {
          return Float.floatToRawIntBits(1.0f) & 0xffffffffL;
        }

        @Override
        long round(long x) {
          if (isNan(x)) {
            return quiet(x);
          }
          return Float.floatToRawIntBits(RustMath.roundF32(Float.intBitsToFloat((int) x)))
              & 0xffffffffL;
        }

        @Override
        long fromF64(double x) {
          return f64ToF32Bits(x);
        }

        @Override
        double toF64(long x) {
          return f32BitsToF64(x);
        }

        @Override
        long fromLatentNumerical(long l) {
          return Float.floatToRawIntBits(RustMath.u64ToF32(l)) & 0xffffffffL;
        }

        @Override
        long toUnsignedSaturating(long a) {
          return RustMath.f64ToUnsigned(Float.intBitsToFloat((int) a), 32);
        }
      };

  static final FloatType F16 =
      new FloatType(16, 10, 15, Latent.U16, 0x8000L, 0x7fffL, 0x7c00L, 0x0200L, 0xfe00L) {
        @Override
        double value(long x) {
          return f16ToF32((int) x);
        }

        @Override
        long rawOp(int op, long a, long b) {
          long fa = f16BitsToF32Bits(a);
          long fb = f16BitsToF32Bits(b);
          return f32BitsToF16Bits(F32.rawOp(op, fa, fb));
        }

        @Override
        long one() {
          return 0x3C00;
        }

        @Override
        long round(long x) {
          if (isNan(x)) {
            return quiet(x);
          }
          return f32BitsToF16Bits(F32.round(f16BitsToF32Bits(x)));
        }

        @Override
        long fromF64(double x) {
          return f32BitsToF16Bits(f64ToF32Bits(x));
        }

        @Override
        double toF64(long x) {
          return f32BitsToF64(f16BitsToF32Bits(x));
        }

        @Override
        long fromLatentNumerical(long l) {
          return f32BitsToF16Bits(Float.floatToRawIntBits(RustMath.u64ToF32(l)) & 0xffffffffL);
        }

        @Override
        long toUnsignedSaturating(long a) {
          return RustMath.f64ToUnsigned(f16ToF32((int) a), 16);
        }
      };

  // ------------------------------------------------------------ conversions (x86 semantics)

  /** {@code f64 as f32} (cvtsd2ss: a NaN keeps its sign and top payload bits, quieted) */
  static long f64ToF32Bits(double x) {
    if (Double.isNaN(x)) {
      long b = Double.doubleToRawLongBits(x);
      long sign = (b >>> 63) << 31;
      long payload = (b >>> 29) & 0x7fffffL;
      return sign | 0x7f800000L | 0x400000L | payload;
    }
    return Float.floatToRawIntBits((float) x) & 0xffffffffL;
  }

  /** {@code f32 as f64} (cvtss2sd: a NaN is quieted) */
  static double f32BitsToF64(long b) {
    if (F32.isNan(b)) {
      long sign = (b >>> 31) << 63;
      long payload = (b & 0x7fffffL) << 29;
      return Double.longBitsToDouble(sign | 0x7ff0000000000000L | (1L << 51) | payload);
    }
    return (double) Float.intBitsToFloat((int) b);
  }

  static float f16ToF32(int h) {
    return Float.intBitsToFloat((int) f16BitsToF32Bits(h));
  }

  /** f16 to f32, exact (vcvtph2ps: a NaN is quieted) */
  static long f16BitsToF32Bits(long h) {
    long sign = (h & 0x8000L) << 16;
    int exp = (int) ((h >>> 10) & 0x1f);
    long man = h & 0x3ff;
    if (exp == 0x1f) {
      if (man == 0) {
        return sign | 0x7f800000L;
      }
      return sign | 0x7f800000L | 0x400000L | (man << 13);
    }
    if (exp == 0) {
      if (man == 0) {
        return sign;
      }
      // subnormal: normalize
      int e = -1;
      do {
        man <<= 1;
        e++;
      } while ((man & 0x400) == 0);
      man &= 0x3ff;
      return sign | ((long) (127 - 15 - e) << 23) | (man << 13);
    }
    return sign | ((long) (exp - 15 + 127) << 23) | (man << 13);
  }

  /** f32 to f16, round to nearest even (vcvtps2ph: a NaN keeps its top payload bits, quieted) */
  static long f32BitsToF16Bits(long f) {
    long sign = (f >>> 16) & 0x8000L;
    int exp = (int) ((f >>> 23) & 0xff);
    long man = f & 0x7fffffL;
    if (exp == 0xff) {
      if (man == 0) {
        return sign | 0x7c00;
      }
      return sign | 0x7c00 | 0x200 | (man >>> 13);
    }
    int e = exp - 127 + 15;
    if (e >= 0x1f) {
      return sign | 0x7c00;
    }
    if (e <= 0) {
      if (e < -10) {
        return sign; // rounds to zero
      }
      long m = man | 0x800000L;
      int shift = 14 - e;
      long half = 1L << (shift - 1);
      long r = m >>> shift;
      long rem = m & ((1L << shift) - 1);
      if (rem > half || (rem == half && (r & 1) == 1)) {
        r++;
      }
      return sign | r;
    }
    long r = ((long) e << 10) | (man >>> 13);
    long rem = man & 0x1fff;
    if (rem > 0x1000 || (rem == 0x1000 && (r & 1) == 1)) {
      r++;
    }
    return sign | r;
  }
}
