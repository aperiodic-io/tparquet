package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Bounded exhaustive equivalence: for every sequence of a given length over a given alphabet
 * (every value of a type, for 8- and 16-bit types), the Java compressor gives the reference's
 * outcome and bytes; for every suffix of a given length after a valid prefix, the Java decoder
 * gives the reference's outcome and numbers. Both sides fold the outcomes into a digest; a
 * differing block is bisected to its first input.
 */
class ExhaustiveParityTest {
  static Oracle o;

  @BeforeAll
  static void setUp() {
    o = Oracle.get();
  }

  static final long FNV0 = 0xcbf29ce484222325L;

  static long fold(long h, Parity.Outcome out) {
    long x = FNV0;
    x = (x ^ out.status) * 0x100000001b3L;
    if (out.status == 0) {
      for (byte b : out.body) {
        x = (x ^ (b & 0xffL)) * 0x100000001b3L;
      }
    }
    return Long.rotateLeft((h ^ x) * 0x9e3779b97f4a7c15L, 29);
  }

  // ------------------------------------------------------------ compressor

  static long[] sequence(long i, int len, long[] alphabet) {
    long[] seq = new long[len];
    for (int pos = len - 1; pos >= 0; pos--) {
      seq[pos] = alphabet[(int) Long.remainderUnsigned(i, alphabet.length)];
      i = Long.divideUnsigned(i, alphabet.length);
    }
    return seq;
  }

  static long javaCompressDigest(NumberType t, Oracle.Cfg cfg, int len, long[] alphabet, long start, long count) {
    ChunkConfig config = cfg.toConfig();
    long h = FNV0;
    for (long i = start; i < start + count; i++) {
      long[] seq = sequence(i, len, alphabet);
      h = fold(h, Parity.java(() -> Pco.compress(t, seq, config)));
    }
    return h;
  }

  static long oracleCompressDigest(NumberType t, Oracle.Cfg cfg, int len, long[] alphabet, long start, long count) {
    byte[] head = Oracle.compressRequest(t, cfg, new long[0]);
    byte[] alpha = Oracle.rawBytes(t, alphabet);
    ByteBuffer b = ByteBuffer.allocate(head.length + 8 + alpha.length + 16).order(ByteOrder.LITTLE_ENDIAN);
    b.put(head);
    b.put(0, (byte) 9);
    b.putInt(len).putInt(alphabet.length).put(alpha).putLong(start).putLong(count);
    return ByteBuffer.wrap(o.call(b.array()).body).order(ByteOrder.LITTLE_ENDIAN).getLong();
  }

  interface Digest {
    long of(long start, long count);
  }

  static void bisect(String what, Digest java, Digest ref, long total) {
    long block = 1 << 16;
    for (long s = 0; s < total; s += block) {
      long c = Math.min(block, total - s);
      if (java.of(s, c) != ref.of(s, c)) {
        long lo = s;
        long n = c;
        while (n > 1) {
          long half = n / 2;
          if (java.of(lo, half) != ref.of(lo, half)) {
            n = half;
          } else {
            lo += half;
            n -= half;
          }
        }
        throw new AssertionError(what + ": differs first at input " + lo);
      }
    }
    System.out.println(what + ": all " + total + " inputs agree");
  }

  static void allSequences(NumberType t, Oracle.Cfg cfg, int len, long[] alphabet) {
    assumeTrue(o != null, "oracle not built");
    long total = 1;
    for (int k = 0; k < len; k++) {
      total *= alphabet.length;
    }
    bisect(
        t + " " + cfg + " every sequence of " + len + " over " + alphabet.length + " values",
        (s, c) -> javaCompressDigest(t, cfg, len, alphabet, s, c),
        (s, c) -> oracleCompressDigest(t, cfg, len, alphabet, s, c),
        total);
  }

  static long[] every(NumberType t) {
    long[] a = new long[1 << t.bits()];
    for (int i = 0; i < a.length; i++) {
      a[i] = i;
    }
    return a;
  }

  /** edge values of a type: extremes, zeros, small values, powers of two, NaN payloads, ... */
  static long[] edges(NumberType t) {
    List<Long> v = new ArrayList<>();
    long mask = t.latent.mask;
    if (t.isFloat()) {
      FloatType f = FloatType.of(t);
      long one = f.one();
      long[] xs = {
        0, f.signMask, one, f.neg(one), f.exp2(1), f.exp2(-1), f.fromF64(0.1), f.fromF64(-0.1),
        f.fromF64(3.0), f.fromF64(0.3), f.fromF64(100.25), f.expMask, f.signMask | f.expMask,
        f.expMask | 1, f.defaultNan, f.expMask | f.quietBit | 5, 1, f.signMask | 1,
        f.expMask - 1, f.maxForSampling(), f.fromF64(1e-3), f.fromF64(1024.0), one + 1, one - 1
      };
      for (long x : xs) {
        v.add(x & mask);
      }
    } else {
      long[] xs = {0, 1, 2, 3, 7, 10, 100, 255, 256, 1000, mask, mask - 1, t.latent.mid,
        t.latent.mid - 1, t.latent.mid + 1, 1L << (t.bits() / 2), 12345, mask / 3, 64, 65, 999, 5};
      for (long x : xs) {
        v.add(x & mask);
      }
    }
    return v.stream().distinct().mapToLong(Long::longValue).toArray();
  }

  static Oracle.Cfg cfg(int mode, int delta, long order) {
    Oracle.Cfg c = new Oracle.Cfg();
    c.mode = mode;
    c.delta = delta;
    c.deltaOrder = order;
    c.enable8Bit = true;
    return c;
  }

  @Test
  void everyPairOfBytesAndEveryShortValue() {
    allSequences(NumberType.U8, cfg(0, 0, 0), 1, every(NumberType.U8));
    allSequences(NumberType.I8, cfg(0, 0, 0), 2, every(NumberType.I8));
    allSequences(NumberType.U16, cfg(0, 0, 0), 1, every(NumberType.U16));
    allSequences(NumberType.I16, cfg(0, 0, 0), 1, every(NumberType.I16));
  }

  @Test
  void everyF16() {
    allSequences(NumberType.F16, cfg(0, 0, 0), 1, every(NumberType.F16));
    Oracle.Cfg fm = cfg(2, 1, 0);
    fm.modeF64 = 0.1;
    Oracle.Cfg fq = cfg(3, 1, 0);
    fq.modeK = 3;
    allSequences(NumberType.F16, fm, 1, every(NumberType.F16));
    allSequences(NumberType.F16, fq, 1, every(NumberType.F16));
    // 2 numbers over 256 f16 values spread across the bit patterns
    long[] spread = new long[256];
    for (int i = 0; i < 256; i++) {
      spread[i] = (i * 257L) & 0xffff;
    }
    allSequences(NumberType.F16, fm, 2, spread);
  }

  @Test
  void everyTripleOfEdgeValues() {
    for (NumberType t : new NumberType[] {NumberType.I32, NumberType.U32, NumberType.I64, NumberType.U64, NumberType.F32, NumberType.F64, NumberType.F16}) {
      for (Oracle.Cfg c : new Oracle.Cfg[] {cfg(0, 0, 0), cfg(1, 3, 0), cfg(1, 2, 2), cfg(1, 4, 1)}) {
        allSequences(t, c, 3, edges(t));
      }
    }
  }

  @Test
  void everySequenceAroundTheSampleThreshold() {
    // 10 numbers: the size from which pcodec samples for modes and delta encodings
    for (NumberType t : new NumberType[] {NumberType.I32, NumberType.I64, NumberType.F32, NumberType.F64}) {
      long[] e = edges(t);
      allSequences(t, cfg(0, 0, 0), 10, new long[] {e[0], e[2], e[4]});
      allSequences(t, cfg(0, 0, 0), 11, new long[] {e[2], e[8]});
      allSequences(t, cfg(0, 0, 0), 12, new long[] {e[1], e[7]});
    }
  }

  @Test
  void everySequenceWithTryModes() {
    for (NumberType t : new NumberType[] {NumberType.I32, NumberType.F64, NumberType.F32}) {
      long[] e = edges(t);
      Oracle.Cfg im = cfg(4, 1, 0);
      im.modeBase = 3;
      Oracle.Cfg fm = cfg(2, 1, 0);
      fm.modeF64 = 0.1;
      Oracle.Cfg fq = cfg(3, 1, 0);
      fq.modeK = 4;
      for (Oracle.Cfg c : new Oracle.Cfg[] {im, fm, fq}) {
        allSequences(t, c, 2, e);
        allSequences(t, c, 9, new long[] {e[2], e[5], e[9]});
      }
    }
  }

  // ------------------------------------------------------------ decoder

  static long javaDecodeDigest(NumberType t, byte[] prefix, int suffixLen, long start, long count) {
    byte[] file = java.util.Arrays.copyOf(prefix, prefix.length + suffixLen);
    long h = FNV0;
    for (long i = start; i < start + count; i++) {
      for (int b = 0; b < suffixLen; b++) {
        file[prefix.length + b] = (byte) (i >>> (8 * (suffixLen - 1 - b)));
      }
      h = fold(h, Parity.java(() -> Parity.decodeBody(t, file)));
    }
    return h;
  }

  static long oracleDecodeDigest(NumberType t, byte[] prefix, int suffixLen, long start, long count) {
    ByteBuffer b = ByteBuffer.allocate(2 + 4 + prefix.length + 4 + 16).order(ByteOrder.LITTLE_ENDIAN);
    b.put((byte) 10).put((byte) t.typeByte).putInt(prefix.length).put(prefix).putInt(suffixLen);
    b.putLong(start).putLong(count);
    return ByteBuffer.wrap(o.call(b.array()).body).order(ByteOrder.LITTLE_ENDIAN).getLong();
  }

  static void allSuffixes(String what, NumberType t, byte[] prefix, int suffixLen) {
    assumeTrue(o != null, "oracle not built");
    bisect(
        what + ": every " + suffixLen + "-byte suffix after " + prefix.length + " bytes",
        (s, c) -> javaDecodeDigest(t, prefix, suffixLen, s, c),
        (s, c) -> oracleDecodeDigest(t, prefix, suffixLen, s, c),
        1L << (8 * suffixLen));
  }

  @Test
  void decoderOnEveryShortSuffix() {
    assumeTrue(o != null, "oracle not built");
    // valid files of several shapes, cut at every byte, followed by every 2-byte suffix
    Object[][] files = {
      {NumberType.I32, cfg(0, 0, 0), Corpus.generate(NumberType.I32, "walk", 300, 1)},
      {NumberType.F64, cfg(0, 0, 0), Corpus.generate(NumberType.F64, "prices", 300, 1)},
      {NumberType.F32, cfg(0, 0, 0), Corpus.generate(NumberType.F32, "floatMult", 50, 2)},
      {NumberType.I64, cfg(5, 1, 0), Corpus.generate(NumberType.I64, "constant", 20, 1)},
      {NumberType.U32, cfg(1, 3, 0), Corpus.generate(NumberType.U32, "periodic", 40, 3)},
      {NumberType.U16, cfg(1, 4, 2), Corpus.generate(NumberType.U16, "trend", 30, 3)},
    };
    for (Object[] f : files) {
      NumberType t = (NumberType) f[0];
      Oracle.Answer good = o.compress(t, (Oracle.Cfg) f[1], (long[]) f[2]);
      int step = Math.max(1, good.body.length / 12);
      for (int cut = 0; cut <= good.body.length; cut += step) {
        allSuffixes(t + " cut " + cut, t, java.util.Arrays.copyOf(good.body, cut), 2);
      }
    }
  }

  @Test
  void decoderOnEveryThreeByteBody() {
    assumeTrue(o != null, "oracle not built");
    // a header and chunk preamble of one i64, then every 3-byte chunk meta and page
    Oracle.Answer good = o.compress(NumberType.I64, cfg(0, 0, 0), new long[] {5});
    int bodyStart = 4 + 1 + 1 + 1 + 2; // magic, version, uniform type, n hint, format version
    byte[] prefix = java.util.Arrays.copyOf(good.body, bodyStart + 4);
    allSuffixes("i64 one number", NumberType.I64, prefix, 3);
  }
}
