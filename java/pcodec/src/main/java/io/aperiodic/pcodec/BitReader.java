package io.aperiodic.pcodec;

/**
 * Reads what {@link BitWriter} writes. Reads past the end of the data return zero bits, as the
 * reference's zero-padded buffers do; {@link #checkInBounds} fails once the position has gone
 * past the end, at the points where the reference checks.
 */
final class BitReader {
  private static final java.lang.invoke.VarHandle LONG_LE =
      java.lang.invoke.MethodHandles.byteArrayViewVarHandle(long[].class, java.nio.ByteOrder.LITTLE_ENDIAN);

  final byte[] src;
  private final int start;
  private final int end;
  /**
   * {@code src[base, end)} as little-endian words, zeros past the end, with one more zero word so
   * a value can always read the word after its first
   */
  private final long[] words;
  /** bit offset of {@code start} in {@link #words} */
  private final long wordBit;
  /** bit position relative to {@code start} */
  long bitIdx;

  /** Reads {@code src[start, end)}; the caller has checked that range lies in {@code src}. */
  BitReader(byte[] src, int start, int end) {
    this(src, start, end, toWords(src, start, end), 0);
  }

  private BitReader(byte[] src, int start, int end, long[] words, long wordBit) {
    this.src = src;
    this.start = start;
    this.end = end;
    this.words = words;
    this.wordBit = wordBit;
  }

  /** A reader of {@code src[newStart, end)} sharing this one's words; {@code newStart >= start}. */
  BitReader at(int newStart) {
    return new BitReader(src, newStart, end, words, wordBit + 8L * (newStart - start));
  }

  private static long[] toWords(byte[] src, int start, int end) {
    int len = end - start;
    long[] w = new long[(len >>> 3) + 2];
    int full = len >>> 3;
    for (int k = 0; k < full; k++) {
      w[k] = (long) LONG_LE.get(src, start + 8 * k);
    }
    long r = 0;
    for (int j = end - 1; j >= start + 8 * full; j--) {
      r = (r << 8) | (src[j] & 0xffL);
    }
    w[full] = r;
    return w;
  }

  private long word(long i) {
    return i < words.length ? words[(int) i] : 0;
  }

  /** The {@code n} bits (0 to 64) at {@code bitIdx}, without moving. */
  long peekAt(long at, int n) {
    if (n == 0) {
      return 0;
    }
    long bit = wordBit + at;
    long wi = bit >>> 6;
    int off = (int) (bit & 63);
    // the second word shifted in two steps, so an offset of 0 brings none of it
    long v = (word(wi) >>> off) | ((word(wi + 1) << 1) << (63 - off));
    return v & (-1L >>> (64 - n));
  }

  long readUint(int n) {
    long v = peekAt(bitIdx, n);
    bitIdx += n;
    return v;
  }

  int readInt(int n) {
    return (int) readUint(n);
  }

  boolean readBool() {
    return readUint(1) != 0;
  }

  boolean aligned() {
    return (bitIdx & 7) == 0;
  }

  /** Reads {@code n} aligned bytes (zeros past the end). */
  byte[] readAlignedBytes(int n) {
    if (!aligned()) {
      throw PcoException.invalidArgument(
          "cannot get aligned byte index on misaligned bit reader (" + (bitIdx & 7) + " bits)");
    }
    byte[] res = new byte[n];
    long abs = start + (bitIdx >>> 3);
    for (int i = 0; i < n; i++) {
      long j = abs + i;
      res[i] = j < end ? src[(int) j] : 0;
    }
    bitIdx += 8L * n;
    return res;
  }

  long totalBits() {
    return 8L * (end - start);
  }

  void checkInBounds() {
    if (bitIdx > totalBits()) {
      throw PcoException.insufficientData(
          "[BitReader] out of bounds at bit " + bitIdx + " / " + totalBits());
    }
  }

  /** Seeks to the end of the byte, which must hold only zero bits. */
  void drainEmptyByte(String message) {
    checkInBounds();
    int off = (int) (bitIdx & 7);
    if (off != 0) {
      long abs = start + (bitIdx >>> 3);
      int b = abs < end ? src[(int) abs] & 0xff : 0;
      if ((b >>> off) > 0) {
        throw PcoException.corruption(message);
      }
      bitIdx += 8 - off;
    }
  }

  /** absolute byte offset of the (aligned) position */
  int bytePos() {
    return start + (int) (bitIdx >>> 3);
  }
}
