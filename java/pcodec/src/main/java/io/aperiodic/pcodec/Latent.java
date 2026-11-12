package io.aperiodic.pcodec;

/**
 * An unsigned latent type of pcodec (u8, u16, u32 or u64). Latents are held in a {@code long},
 * zero-extended: every operation here wraps at the latent's width, as Rust's {@code wrapping_*}
 * methods on the unsigned type do.
 */
final class Latent {
  static final Latent U8 = new Latent(0, 8, 16);
  static final Latent U16 = new Latent(1, 16, 32);
  static final Latent U32 = new Latent(2, 32, 64);
  // 64-bit convolutions are refused (as in the reference); its Conv type is nominally i64
  static final Latent U64 = new Latent(3, 64, 64);

  /** 0..3 for u8, u16, u32, u64 */
  final int id;
  final int bits;
  final long mask;
  /** {@code 1 << (bits - 1)} */
  final long mid;
  /** bits of the signed convolution type {@code L::Conv} */
  final int convBits;

  private Latent(int id, int bits, int convBits) {
    this.id = id;
    this.bits = bits;
    this.mask = bits == 64 ? -1L : (1L << bits) - 1;
    this.mid = 1L << (bits - 1);
    this.convBits = convBits;
  }

  long max() {
    return mask;
  }

  long add(long a, long b) {
    return (a + b) & mask;
  }

  long sub(long a, long b) {
    return (a - b) & mask;
  }

  long mul(long a, long b) {
    return (a * b) & mask;
  }

  long toggleCenter(long x) {
    return (x + mid) & mask;
  }

  int leadingZeros(long x) {
    return Long.numberOfLeadingZeros(x) - (64 - bits);
  }

  /** {@code L::BITS - max_offset.leading_zeros()} */
  int bitsToEncode(long x) {
    return 64 - Long.numberOfLeadingZeros(x);
  }

  /** the latent type's {@code from_u64}: truncation */
  long fromU64(long x) {
    return x & mask;
  }

  static boolean lt(long a, long b) {
    return Long.compareUnsigned(a, b) < 0;
  }

  static long min(long a, long b) {
    return lt(b, a) ? b : a;
  }

  static long max(long a, long b) {
    return lt(a, b) ? b : a;
  }

  static long div(long a, long b) {
    return Long.divideUnsigned(a, b);
  }

  static long rem(long a, long b) {
    return Long.remainderUnsigned(a, b);
  }

  /** The latent as an f64, as Rust's {@code u64 as f64} (round to nearest, ties to even). */
  static double toF64(long x) {
    if (x >= 0) {
      return (double) x;
    }
    // halve with the low bit kept sticky, convert (rounding once), double
    return ((double) ((x >>> 1) | (x & 1))) * 2.0;
  }


  /** {@code L::from_conv(s)}: the signed value truncated to the latent's width. */
  long fromConv(long s) {
    return s & mask;
  }

  @Override
  public String toString() {
    return "u" + bits;
  }
}
