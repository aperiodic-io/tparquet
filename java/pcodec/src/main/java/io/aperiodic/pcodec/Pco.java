package io.aperiodic.pcodec;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * pcodec's standalone format, as {@code pco::standalone} 1.0.4 writes and reads it: {@link
 * #compress} is {@code simple_compress}, byte for byte; {@link #decompress} is {@code
 * simple_decompress}, accepting and rejecting the same files.
 *
 * <p>Numbers are raw bits in a {@code long} (see {@link NumberType}); the typed overloads convert
 * Java arrays.
 */
public final class Pco {
  private Pco() {}

  static final byte[] MAGIC_HEADER = {112, 99, 111, 33}; // "pco!"
  static final int MAGIC_TERMINATION_BYTE = 0;
  static final int CURRENT_STANDALONE_VERSION = 3;
  static final int BITS_TO_ENCODE_N_ENTRIES = 24;
  static final int BITS_TO_ENCODE_VARINT_POWER = 6;
  static final int FORMAT_MAJOR = 4;
  static final int FORMAT_MINOR = 1;

  // ------------------------------------------------------------ compression

  /** {@code simple_compress}: a standalone file (no uniform type, an n hint of the count). */
  public static byte[] compress(NumberType type, long[] nums, ChunkConfig config) {
    BitWriter w = new BitWriter();
    writeHeader(w, nums.length, 0);
    long[] nPerPage = config.pagingSpec.nPerPage(nums.length);
    int start = 0;
    for (long pageNL : nPerPage) {
      int pageN = (int) pageNL;
      long[] chunk = Arrays.copyOfRange(nums, start, start + pageN);
      ChunkCompressor cc =
          ChunkCompressor.create(type, chunk, config.withPagingSpec(PagingSpec.exact(pageN)));
      w.writeAlignedBytes(new byte[] {(byte) type.typeByte});
      w.writeUint(pageN - 1, BITS_TO_ENCODE_N_ENTRIES);
      cc.writeMeta(w);
      cc.writePage(0, w);
      start += pageN;
    }
    w.writeAlignedBytes(new byte[] {MAGIC_TERMINATION_BYTE});
    return w.toByteArray();
  }

  static void writeHeader(BitWriter w, long nHint, int uniformTypeByte) {
    w.writeAlignedBytes(MAGIC_HEADER);
    w.writeUint(CURRENT_STANDALONE_VERSION, 8);
    w.writeAlignedBytes(new byte[] {(byte) uniformTypeByte});
    int power = nHint == 0 ? 1 : 64 - Long.numberOfLeadingZeros(nHint);
    w.writeUint(power - 1, BITS_TO_ENCODE_VARINT_POWER);
    w.writeUint(nHint, power); // nHint has exactly `power` bits
    w.finishByte();
    w.writeAlignedBytes(new byte[] {(byte) FORMAT_MAJOR, (byte) FORMAT_MINOR});
  }

  public static byte[] compress(int[] nums, ChunkConfig config) {
    long[] raw = new long[nums.length];
    for (int i = 0; i < nums.length; i++) {
      raw[i] = nums[i] & 0xffffffffL;
    }
    return compress(NumberType.I32, raw, config);
  }

  public static byte[] compress(long[] nums, ChunkConfig config) {
    return compress(NumberType.I64, nums, config);
  }

  public static byte[] compress(float[] nums, ChunkConfig config) {
    long[] raw = new long[nums.length];
    for (int i = 0; i < nums.length; i++) {
      raw[i] = Float.floatToRawIntBits(nums[i]) & 0xffffffffL;
    }
    return compress(NumberType.F32, raw, config);
  }

  public static byte[] compress(double[] nums, ChunkConfig config) {
    long[] raw = new long[nums.length];
    for (int i = 0; i < nums.length; i++) {
      raw[i] = Double.doubleToRawLongBits(nums[i]);
    }
    return compress(NumberType.F64, raw, config);
  }

  /** The chunk metadata a chunk of these numbers would get (one chunk, the config's pages). */
  public static ChunkMeta chunkMeta(NumberType type, long[] nums, ChunkConfig config) {
    return ChunkCompressor.create(type, nums, config).meta;
  }

  // ------------------------------------------------------------ decompression

  /** A parsed standalone header. */
  public static final class Header {
    public final int standaloneVersion;
    /** the uniform type, or null */
    public final NumberType uniformType;
    public final long nHint;
    public final int formatMajor;
    public final int formatMinor;
    /** byte offset of the first chunk */
    final int bodyStart;

    Header(int sv, NumberType ut, long nHint, int major, int minor, int bodyStart) {
      this.standaloneVersion = sv;
      this.uniformType = ut;
      this.nHint = nHint;
      this.formatMajor = major;
      this.formatMinor = minor;
      this.bodyStart = bodyStart;
    }
  }

  public static Header readHeader(byte[] src, int offset, int length) {
    BitReader r = new BitReader(src, offset, offset + length);
    byte[] magic = r.readAlignedBytes(4);
    r.checkInBounds();
    if (!Arrays.equals(magic, MAGIC_HEADER)) {
      throw PcoException.corruption(
          "magic header does not match " + Arrays.toString(MAGIC_HEADER) + "; instead found "
              + Arrays.toString(magic));
    }
    int sv = r.readInt(8);
    NumberType uniform = null;
    long nHint = 0;
    if (sv < 2) {
      r.bitIdx -= 8;
    } else {
      if (sv >= 3) {
        int b = r.readAlignedBytes(1)[0] & 0xff;
        if (b != MAGIC_TERMINATION_BYTE) {
          uniform = NumberType.fromTypeByte(b);
          if (uniform == null) {
            throw PcoException.corruption("unknown number type byte: " + b);
          }
        }
      }
      int power = 1 + r.readInt(BITS_TO_ENCODE_VARINT_POWER);
      nHint = r.readUint(power);
      r.drainEmptyByte("standalone size hint");
    }
    r.checkInBounds();
    if (sv > CURRENT_STANDALONE_VERSION) {
      throw PcoException.corruption(
          "file's standalone version (" + sv + ") exceeds max supported ("
              + CURRENT_STANDALONE_VERSION + "); consider upgrading pco");
    }
    int major = r.readAlignedBytes(1)[0] & 0xff;
    int minor = major >= 4 ? r.readAlignedBytes(1)[0] & 0xff : 0;
    // as the reference: the version is checked first (a missing minor byte reads as the
    // padding's 0), the bounds after
    if (major > FORMAT_MAJOR) {
      throw PcoException.corruption(
          "File's format version (" + major + "." + minor + ") definitely cannot be decompressed"
              + " by this library version");
    }
    r.checkInBounds();
    return new Header(sv, uniform, nHint, major, minor, r.bytePos());
  }

  /** The result of decoding: the numbers, and where the pcodec file ends. */
  public static final class Decoded {
    public final NumberType type;
    public final long[] nums;
    /** byte offset just after the footer */
    public final int end;

    Decoded(NumberType type, long[] nums, int end) {
      this.type = type;
      this.nums = nums;
      this.end = end;
    }
  }

  /** a sink for decoded batches; return false to stop early */
  public interface BatchSink {
    void accept(long[] batch, int len);
  }

  /**
   * Decodes a standalone file holding numbers of {@code type} ({@code simple_decompress::<T>}),
   * passing each batch of at most 256 numbers to {@code sink}. Returns the byte offset after the
   * footer; bytes after it are not read. {@code maxNumbers} bounds the count before any batch of
   * a chunk that would exceed it is decoded (a negative value means no bound).
   */
  public static int decompress(
      byte[] src, int offset, int length, NumberType type, long maxNumbers, BatchSink sink) {
    return offset + decompressPadded(BitReader.padCopy(src, offset, length), length, type, maxNumbers, sink);
  }

  private static int decompressPadded(byte[] src, int length, NumberType type, long maxNumbers, BatchSink sink) {
    int offset = 0;
    Header h = readHeader(src, offset, length);
    int end = offset + length;
    int pos = h.bodyStart;
    long total = 0;
    long[] batch = new long[ChunkDecompressor.FULL_BATCH_N];
    while (true) {
      BitReader pre = new BitReader(src, pos, end);
      int b = pre.readAlignedBytes(1)[0] & 0xff;
      pre.checkInBounds();
      if (b == MAGIC_TERMINATION_BYTE) {
        return pre.bytePos();
      }
      if (h.uniformType != null && h.uniformType.typeByte != b) {
        throw PcoException.corruption(
            "chunk's number type of " + b + " does not match file's uniform number type of "
                + h.uniformType);
      }
      if (b != type.typeByte) {
        throw PcoException.corruption(
            "requested chunk decompression with " + type + " does not match chunk's number type of "
                + b);
      }
      int n = pre.readInt(BITS_TO_ENCODE_N_ENTRIES) + 1;
      pre.checkInBounds();
      BitReader r = new BitReader(src, pre.bytePos(), end, true);
      ChunkMeta meta = ChunkMeta.readFrom(r, h.formatMajor, type.latent);
      ChunkDecompressor cd = new ChunkDecompressor(type, meta, n);
      if (maxNumbers >= 0 && total + n > maxNumbers) {
        throw new CountExceeded(total + n);
      }
      cd.readPageMeta(r);
      while (!cd.finished()) {
        int len = cd.readBatch(r, batch, 0);
        sink.accept(batch, len);
      }
      total += n;
      pos = r.bytePos();
    }
  }

  /** thrown by {@link #decompress(byte[], int, int, NumberType, long, BatchSink)} */
  public static final class CountExceeded extends RuntimeException {
    private static final long serialVersionUID = 1L;
    public final long count;

    CountExceeded(long count) {
      super("the file holds at least " + count + " numbers");
      this.count = count;
    }
  }

  /** Decodes a whole standalone file (bytes after the footer are ignored, as in the reference). */
  public static Decoded decompress(byte[] src, NumberType type) {
    List<long[]> parts = new ArrayList<>();
    long[] count = new long[1];
    int end =
        decompress(
            src,
            0,
            src.length,
            type,
            -1,
            (b, len) -> {
              parts.add(Arrays.copyOf(b, len));
              count[0] += len;
            });
    long[] out = new long[(int) count[0]];
    int at = 0;
    for (long[] p : parts) {
      System.arraycopy(p, 0, out, at, p.length);
      at += p.length;
    }
    return new Decoded(type, out, end);
  }

  /** The number type of the first chunk (or the uniform type), or null for a file without chunks. */
  public static NumberType peekNumberType(byte[] src) {
    Header h = readHeader(src, 0, src.length);
    if (h.uniformType != null) {
      return h.uniformType;
    }
    if (h.bodyStart >= src.length) {
      throw PcoException.insufficientData("unable to peek number type from empty bytes");
    }
    int b = src[h.bodyStart] & 0xff;
    if (b == MAGIC_TERMINATION_BYTE) {
      return null;
    }
    NumberType t = NumberType.fromTypeByte(b);
    if (t == null) {
      throw PcoException.corruption("peeked unknown number type byte: " + b);
    }
    return t;
  }

  public static int[] decompressInts(byte[] src) {
    long[] raw = decompress(src, NumberType.I32).nums;
    int[] out = new int[raw.length];
    for (int i = 0; i < raw.length; i++) {
      out[i] = (int) raw[i];
    }
    return out;
  }

  public static long[] decompressLongs(byte[] src) {
    return decompress(src, NumberType.I64).nums;
  }

  public static float[] decompressFloats(byte[] src) {
    long[] raw = decompress(src, NumberType.F32).nums;
    float[] out = new float[raw.length];
    for (int i = 0; i < raw.length; i++) {
      out[i] = Float.intBitsToFloat((int) raw[i]);
    }
    return out;
  }

  public static double[] decompressDoubles(byte[] src) {
    long[] raw = decompress(src, NumberType.F64).nums;
    double[] out = new double[raw.length];
    for (int i = 0; i < raw.length; i++) {
      out[i] = Double.longBitsToDouble(raw[i]);
    }
    return out;
  }
}
