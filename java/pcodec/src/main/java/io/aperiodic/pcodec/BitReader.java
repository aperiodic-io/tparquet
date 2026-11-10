package io.aperiodic.pcodec;

/**
 * Reads what {@link BitWriter} writes. Reads past the end of the data return zero bits, as the
 * reference's zero-padded buffers do; {@link #checkInBounds} fails once the position has gone
 * past the end, at the points where the reference checks.
 */
final class BitReader {
  final byte[] src;
  private final int start;
  private final int end;
  /** bit position relative to {@code start} */
  long bitIdx;

  BitReader(byte[] src, int start, int end) {
    this.src = src;
    this.start = start;
    this.end = end;
  }

  long u64At(long byteIdx) {
    long abs = start + byteIdx;
    if (abs + 8 <= end) {
      long r = 0;
      int i = (int) abs;
      for (int k = 7; k >= 0; k--) {
        r = (r << 8) | (src[i + k] & 0xffL);
      }
      return r;
    }
    long r = 0;
    for (int k = 7; k >= 0; k--) {
      long j = abs + k;
      r = (r << 8) | (j < end ? (src[(int) j] & 0xffL) : 0);
    }
    return r;
  }

  /** The {@code n} bits (0 to 64) at {@code bitIdx}, without moving. */
  long peekAt(long at, int n) {
    if (n == 0) {
      return 0;
    }
    long byteIdx = at >>> 3;
    int off = (int) (at & 7);
    long v = u64At(byteIdx) >>> off;
    if (n > 64 - off - 0 && off > 0) {
      v |= u64At(byteIdx + 8) << (64 - off);
    }
    return n == 64 ? v : v & ((1L << n) - 1);
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
