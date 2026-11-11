package io.aperiodic.pcodec;

import java.util.Arrays;

/**
 * Little-endian bit packing, least significant bit first, as {@code pco::bit_writer}: a value is
 * OR-ed into the buffer at the current bit position, and sections end byte-aligned with {@link
 * #finishByte}. The reference stages bytes in fixed buffers and flushes them; the bytes are the
 * same.
 */
final class BitWriter {
  private byte[] buf = new byte[256];
  private long bitIdx;

  /** Writes the low {@code n} bits of {@code x} ({@code x} must have no bits above them). */
  void writeUint(long x, int n) {
    int byteIdx = (int) (bitIdx >>> 3);
    int off = (int) (bitIdx & 7);
    ensure(byteIdx + 9L);
    if (x != 0) {
      long lo = x << off;
      for (int i = 0; i < 8; i++) {
        buf[byteIdx + i] |= (byte) (lo >>> (8 * i));
      }
      if (off > 0) {
        buf[byteIdx + 8] |= (byte) (x >>> (64 - off));
      }
    }
    bitIdx += n;
  }

  void writeBool(boolean b) {
    writeUint(b ? 1 : 0, 1);
  }

  void writeAlignedBytes(byte[] bytes) {
    if ((bitIdx & 7) != 0) {
      throw PcoException.invalidArgument(
          "cannot write aligned bytes to unaligned writer (" + (bitIdx & 7) + " bits past byte)");
    }
    int byteIdx = (int) (bitIdx >>> 3);
    ensure((long) byteIdx + bytes.length + 1);
    System.arraycopy(bytes, 0, buf, byteIdx, bytes.length);
    bitIdx += 8L * bytes.length;
  }

  void finishByte() {
    bitIdx = (bitIdx + 7) & ~7L;
  }

  /** The bytes written so far (the last one partial if not aligned). */
  byte[] toByteArray() {
    return Arrays.copyOf(buf, (int) ((bitIdx + 7) >>> 3));
  }

  /** the largest buffer a Java array holds */
  static final int MAX_CAPACITY = Integer.MAX_VALUE - 8;

  private void ensure(long len) {
    if (len > buf.length) {
      buf = Arrays.copyOf(buf, grownCapacity(buf.length, len));
    }
  }

  /** doubling from {@code cap} until {@code need} fits, up to the largest Java array */
  static int grownCapacity(int cap, long need) {
    if (need > MAX_CAPACITY) {
      throw PcoException.invalidArgument("compressed output of " + need + " bytes exceeds a Java array");
    }
    long c = cap;
    while (c < need) {
      c = Math.min(2 * c, MAX_CAPACITY);
    }
    return (int) c;
  }
}
