package io.aperiodic.pcodec.payload;

import io.aperiodic.pcodec.ChunkConfig;
import io.aperiodic.pcodec.NumberType;
import io.aperiodic.pcodec.Pco;
import io.aperiodic.pcodec.PcoException;
import io.aperiodic.pcodec.PcoPanic;
import java.io.ByteArrayOutputStream;
import java.util.Arrays;

/**
 * The PCODEC Parquet compression codec: the page payload of {@code spec/FORMAT.md}, in pure
 * Java. Byte for byte what {@code tparquet-core} (the reference encoder) writes in RAW, PCO and
 * DECIMAL modes, and the same decisions to accept or reject a payload.
 *
 * <p>{@code compress} turns the bytes Parquet would compress into a payload; {@code decompress}
 * turns a payload back into exactly those bytes, and fails unless it produces exactly the
 * expected size. Payloads are untrusted: a malformed one raises a {@link PayloadException}.
 */
public final class PcodecPayload {
  /** {@code CompressionCodec} value of PCODEC in the Parquet footer (spec section 2). */
  public static final int CODEC_ID = 20547;
  public static final String KV_KEY = "parquet.pcodec.version";
  public static final String KV_VALUE = "1";
  public static final int FORMAT_VERSION = 1;
  public static final int DEFAULT_LEVEL = 8;
  public static final int MAX_LEVEL = 12;
  static final int ZSTD_LEVEL = 3;

  public static final int MODE_RAW = 0;
  public static final int MODE_PCO = 1;
  public static final int MODE_DECIMAL = 2;
  public static final int MODE_ZSTD = 3;

  private final ZstdCodec zstd;

  /** A codec without Zstandard (see {@link ZstdCodec}). */
  public PcodecPayload() {
    this(null);
  }

  public PcodecPayload(ZstdCodec zstd) {
    this.zstd = zstd;
  }

  // ------------------------------------------------------------ encode

  public byte[] compress(byte[] src, ParquetType hint) {
    return compress(src, 0, src.length, hint, DEFAULT_LEVEL, true);
  }

  /**
   * Encodes {@code src[off, off + len)}.
   *
   * @param hint the column's physical type, or null for none
   * @param level pcodec level 0..12
   * @param decimal whether to try DECIMAL mode for FLOAT and DOUBLE
   */
  public byte[] compress(byte[] src, int off, int len, ParquetType hint, int level, boolean decimal) {
    if (level < 0 || level > MAX_LEVEL) {
      throw new PayloadException("compression level " + level + " above " + MAX_LEVEL);
    }
    if (len == 0) {
      return new byte[] {FORMAT_VERSION, MODE_RAW};
    }
    ChunkConfig cfg = new ChunkConfig().withCompressionLevel(level);
    byte[] best;
    if (hint != null && len % hint.width == 0) {
      long[] vals = values(src, off, len, hint.width);
      if (hint == ParquetType.INT32 || hint == ParquetType.INT64) {
        best = pcoPayload(hint, vals, cfg);
      } else {
        best = floatPayload(hint, vals, decimal, cfg);
      }
    } else if (zstd != null) {
      byte[] z = zstd.compress(src, off, len, ZSTD_LEVEL);
      best = new byte[2 + z.length];
      best[0] = FORMAT_VERSION;
      best[1] = MODE_ZSTD;
      System.arraycopy(z, 0, best, 2, z.length);
    } else {
      best = null;
    }
    if (best != null && best.length < len + 2) {
      return best;
    }
    byte[] raw = new byte[len + 2];
    raw[0] = FORMAT_VERSION;
    raw[1] = MODE_RAW;
    System.arraycopy(src, off, raw, 2, len);
    return raw;
  }

  static long[] values(byte[] src, int off, int len, int width) {
    long[] v = new long[len / width];
    for (int k = 0; k < v.length; k++) {
      long x = 0;
      int at = off + k * width;
      for (int i = width - 1; i >= 0; i--) {
        x = (x << 8) | (src[at + i] & 0xffL);
      }
      v[k] = x;
    }
    return v;
  }

  private static byte[] pcoPayload(ParquetType t, long[] vals, ChunkConfig cfg) {
    byte[] pco = Pco.compress(t.numberType, vals, cfg);
    byte[] p = new byte[3 + pco.length];
    p[0] = FORMAT_VERSION;
    p[1] = MODE_PCO;
    p[2] = (byte) t.dtype;
    System.arraycopy(pco, 0, p, 3, pco.length);
    return p;
  }

  private static byte[] floatPayload(ParquetType t, long[] vals, boolean decimal, ChunkConfig cfg) {
    byte[] plain = pcoPayload(t, vals, cfg);
    if (!decimal) {
      return plain;
    }
    boolean isFloat = t == ParquetType.FLOAT;
    int e = Decimal.exponent(vals, isFloat);
    if (e < 0) {
      return plain;
    }
    Decimal.Split split = Decimal.split(vals, isFloat, e);
    if (split == null) {
      return plain;
    }
    ByteArrayOutputStream p = new ByteArrayOutputStream();
    p.write(FORMAT_VERSION);
    p.write(MODE_DECIMAL);
    p.write(t.dtype);
    p.write(e);
    Varint.put(p, split.exceptions.length);
    long next = 0;
    for (int i : split.exceptions) {
      Varint.put(p, i - next);
      next = i + 1L;
    }
    for (int i : split.exceptions) {
      for (int b = 0; b < t.width; b++) {
        p.write((int) (vals[i] >>> (8 * b)));
      }
    }
    byte[] pco = Pco.compress(NumberType.I64, split.qs, cfg);
    p.write(pco, 0, pco.length);
    byte[] d = p.toByteArray();
    return d.length < plain.length ? d : plain;
  }

  // ------------------------------------------------------------ decode

  /** The mode of a payload, after checking its version. */
  public static int mode(byte[] payload) {
    return mode(payload, 0, payload.length);
  }

  static int mode(byte[] payload, int off, int len) {
    if (len < 2) {
      throw new PayloadException("truncated");
    }
    int v = payload[off] & 0xff;
    if (v != FORMAT_VERSION) {
      throw new PayloadException("unsupported version " + v);
    }
    int m = payload[off + 1] & 0xff;
    if (m > MODE_ZSTD) {
      throw new PayloadException("unknown mode " + m);
    }
    return m;
  }

  /** Decodes {@code payload}, which must produce exactly {@code expectedLen} bytes. */
  public byte[] decompress(byte[] payload, int expectedLen) {
    return decompress(payload, 0, payload.length, expectedLen);
  }

  /**
   * Decodes {@code payload[off, off + len)}, which must produce exactly {@code expectedLen}
   * bytes. Nothing proportional to {@code expectedLen} (untrusted, from a page header) is
   * allocated before the payload shows it can produce that many bytes.
   */
  public byte[] decompress(byte[] payload, int off, int len, int expectedLen) {
    return decode(payload, off, len, null, 0, expectedLen);
  }

  /** Decodes into {@code dst[dstOff, dstOff + dstLen)}, which it must fill exactly. */
  public void decompress(byte[] payload, int off, int len, byte[] dst, int dstOff, int dstLen) {
    java.util.Objects.checkFromIndexSize(dstOff, dstLen, dst.length);
    decode(payload, off, len, dst, dstOff, dstLen);
  }

  /** decodes into dst at dstOff (or, without dst, into a new array), returning the array */
  private byte[] decode(byte[] payload, int off, int len, byte[] dst, int dstOff, int expectedLen) {
    java.util.Objects.checkFromIndexSize(off, len, payload.length);
    if (expectedLen < 0) {
      throw new IllegalArgumentException("negative expected length " + expectedLen);
    }
    int mode = mode(payload, off, len);
    int bodyOff = off + 2;
    int bodyLen = len - 2;
    switch (mode) {
      case MODE_RAW:
        check(expectedLen, bodyLen);
        if (dst == null) {
          return Arrays.copyOfRange(payload, bodyOff, bodyOff + bodyLen);
        }
        System.arraycopy(payload, bodyOff, dst, dstOff, bodyLen);
        return dst;
      case MODE_ZSTD:
        {
          // RFC 8878 3.1.1.2: a block decodes to at most 128 KiB and costs at least 4 bytes
          if ((long) expectedLen > (long) (bodyLen / 4) * (128 << 10)) {
            throw new PayloadException("truncated");
          }
          byte[] out = dst == null ? new byte[expectedLen] : dst;
          zstdInto(payload, bodyOff, bodyLen, out, dst == null ? 0 : dstOff, expectedLen);
          return out;
        }
      default:
        {
          Sink s = new Sink(dst, dstOff, expectedLen);
          numbers(mode, payload, bodyOff, bodyLen, s, expectedLen);
          return dst == null ? s.bytes() : dst;
        }
    }
  }

  private void zstdInto(byte[] src, int off, int len, byte[] dst, int dstOff, int dstLen) {
    if (len == 0) {
      throw new PayloadException("truncated");
    }
    if (zstd == null) {
      throw new PayloadException("zstd: ZSTD mode needs a ZstdCodec");
    }
    int n;
    try {
      n = zstd.decompress(src, off, len, dst, dstOff, dstLen);
    } catch (RuntimeException e) {
      throw new PayloadException("zstd: " + e.getMessage(), e);
    }
    if (n < 0) {
      throw new PayloadException("decodes to " + (dstLen + 1L) + " bytes, expected " + dstLen);
    }
    check(dstLen, n);
  }

  static void check(long expected, long actual) {
    if (expected != actual) {
      throw new PayloadException("decodes to " + actual + " bytes, expected " + expected);
    }
  }

  static int count(int len, int width) {
    if (len % width != 0) {
      throw new PayloadException(
          "expected size " + len + " is not a multiple of the value width " + width);
    }
    return len / width;
  }

  /**
   * Decoded bytes: written into the caller's buffer, or into one grown as numbers arrive, never
   * beyond the expected size.
   */
  static final class Sink {
    private byte[] out;
    private final int base;
    private final int cap;
    private final boolean grow;
    private int len;

    /** into {@code dst[base, base + cap)}, or, without dst, into an array of at most cap */
    Sink(byte[] dst, int base, int cap) {
      this.grow = dst == null;
      this.out = grow ? new byte[Math.min(cap, 1 << 16)] : dst;
      this.base = grow ? 0 : base;
      this.cap = cap;
    }

    void put(long v, int width) {
      if (len + width > cap) {
        throw new PayloadException("decodes to more than " + cap + " bytes");
      }
      if (grow && len + width > out.length) {
        out = Arrays.copyOf(out, (int) Math.min(cap, Math.max(len + width, 2L * out.length)));
      }
      int at = base + len;
      for (int b = 0; b < width; b++) {
        out[at + b] = (byte) (v >>> (8 * b));
      }
      len += width;
    }

    byte[] bytes() {
      return out.length == len ? out : Arrays.copyOf(out, len);
    }

    byte[] raw() {
      return out;
    }

    /** where byte 0 of the decoded bytes is in raw() */
    int base() {
      return base;
    }
  }

  private static ParquetType dtype(int b) {
    ParquetType t = ParquetType.fromParquet(b);
    if (t == null) {
      throw new PayloadException("unknown dtype " + b);
    }
    return t;
  }

  private static void numbers(int mode, byte[] p, int off, int len, Sink sink, int expectedLen) {
    if (mode == MODE_PCO) {
      if (len < 1) {
        throw new PayloadException("truncated");
      }
      ParquetType t = dtype(p[off] & 0xff);
      int n = count(expectedLen, t.width);
      pcoBatches(p, off + 1, len - 1, t.numberType, n, t.width, (batch, k) -> {
        for (int i = 0; i < k; i++) {
          sink.put(batch[i], t.width);
        }
      });
      return;
    }
    if (len < 2) {
      throw new PayloadException("truncated");
    }
    ParquetType t = dtype(p[off] & 0xff);
    int e = p[off + 1] & 0xff;
    if (e > Decimal.MAX_EXPONENT) {
      throw new PayloadException("exponent " + e + " above 18");
    }
    if (t == ParquetType.INT32 || t == ParquetType.INT64) {
      throw new PayloadException("unknown dtype " + t.dtype);
    }
    boolean isFloat = t == ParquetType.FLOAT;
    int width = t.width;
    int n = count(expectedLen, width);
    int[] at = {off + 2};
    int end = off + len;
    long k = Varint.get(p, at, end);
    if (Long.compareUnsigned(k, n) > 0) {
      throw new PayloadException("exception count or position out of range");
    }
    int deltasAt = at[0];
    long next = 0;
    for (long j = 0; j < k; j++) {
      long d = Varint.get(p, at, end);
      long pos = next + d;
      if (Long.compareUnsigned(pos, next) < 0) {
        throw new PayloadException("exception count or position out of range");
      }
      if (Long.compareUnsigned(pos, n) >= 0) {
        throw new PayloadException("exception count or position out of range");
      }
      next = pos + 1;
    }
    long excBytes = k * width;
    if (excBytes > end - at[0]) {
      throw new PayloadException("truncated");
    }
    int excAt = at[0];
    int pcoAt = excAt + (int) excBytes;
    pcoBatches(p, pcoAt, end - pcoAt, NumberType.I64, n, width, (batch, c) -> {
      for (int i = 0; i < c; i++) {
        sink.put(Decimal.decode(batch[i], e, isFloat), width);
      }
    });
    byte[] dst = sink.raw();
    int base = sink.base();
    int[] dAt = {deltasAt};
    long nx = 0;
    for (long j = 0; j < k; j++) {
      long pos = nx + Varint.get(p, dAt, end);
      System.arraycopy(p, excAt + (int) (j * width), dst, base + (int) pos * width, width);
      nx = pos + 1;
    }
  }

  interface Batches {
    void accept(long[] batch, int len);
  }

  /** decodes the pcodec file, which must hold exactly n numbers and end the payload */
  private static void pcoBatches(
      byte[] p, int off, int len, NumberType t, int n, int width, Batches put) {
    int end;
    long[] done = {0};
    try {
      end =
          Pco.decompress(
              p,
              off,
              len,
              t,
              n,
              (batch, k) -> {
                put.accept(batch, k);
                done[0] += k;
              });
    } catch (Pco.CountExceeded c) {
      throw new PayloadException(
          "decodes to " + c.count * width + " bytes, expected " + (long) n * width);
    } catch (PcoException | PcoPanic e) {
      throw new PayloadException("pcodec: " + e.getMessage(), e);
    }
    if (end != off + len) {
      throw new PayloadException((off + len - end) + " bytes after the pcodec file");
    }
    check((long) n * width, done[0] * width);
  }
}
