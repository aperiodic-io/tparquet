package io.aperiodic.pcodec;

import java.lang.invoke.MethodHandles;
import java.lang.invoke.VarHandle;
import java.nio.ByteOrder;
import java.util.Arrays;

/**
 * Little-endian bit packing, least significant bit first, as {@code pco::bit_writer}: a value is
 * OR-ed in at the current bit position, and sections end byte-aligned with {@link #finishByte}.
 * Like the reference, the pending bits are staged (here in one 64-bit register) and stored as a
 * whole little-endian long when it fills; the bytes are the same.
 */
final class BitWriter {
  private static final VarHandle LONG_LE =
      MethodHandles.byteArrayViewVarHandle(long[].class, ByteOrder.LITTLE_ENDIAN);

  private byte[] buf = new byte[256];
  /** bytes of {@code buf} already final; the staged bits follow them */
  private int bytePos;
  /** the pending bits, least significant first; nothing above {@code stagedBits} */
  private long staged;
  /** how many bits of {@code staged} are pending, 0 to 63 */
  private int stagedBits;

  /** Writes the low {@code n} bits of {@code x} ({@code x} must have no bits above them). */
  void writeUint(long x, int n) {
    // a shift by 64 is a shift by 0 in Java, but stagedBits is always below 64
    staged |= x << stagedBits;
    int total = stagedBits + n;
    if (total >= 64) {
      ensure(bytePos + 8L);
      LONG_LE.set(buf, bytePos, staged);
      bytePos += 8;
      // the bits of x that did not fit; none when the register was empty
      staged = stagedBits == 0 ? 0 : x >>> (64 - stagedBits);
      total -= 64;
    }
    stagedBits = total;
  }

  /**
   * Writes a batch as {@code LatentCompressor.writeDissectedBatch} lays it out: the ANS values
   * {@code ansVals[from..to)} (if {@code withAns}), then the offsets {@code offsets[from..to)},
   * each with its bit count, exactly as {@link #writeUint} one by one would. The register, its
   * bit count and the position stay in locals, and the capacity is checked once, for the batch's
   * worst case (32 bits per ANS value, 64 per offset).
   */
  void writeBatch(
      int[] ansVals, int[] ansBits, long[] offsets, int[] offsetBits, int from, int to, boolean withAns) {
    long maxBits = (long) (to - from) * (withAns ? 96 : 64);
    ensure(bytePos + (maxBits >>> 3) + 8);
    byte[] b = buf;
    int pos = bytePos;
    long st = staged;
    int sb = stagedBits;
    if (withAns) {
      for (int i = from; i < to; i++) {
        long x = ansVals[i] & 0xffffffffL;
        st |= x << sb;
        sb += ansBits[i];
        if (sb >= 64) {
          LONG_LE.set(b, pos, st);
          pos += 8;
          sb -= 64;
          // the bits of x that did not fit (none when it filled the register exactly)
          st = sb == 0 ? 0 : x >>> (ansBits[i] - sb);
        }
      }
    }
    for (int i = from; i < to; i++) {
      long x = offsets[i];
      int n = offsetBits[i];
      int before = sb;
      st |= x << sb;
      sb += n;
      if (sb >= 64) {
        LONG_LE.set(b, pos, st);
        pos += 8;
        sb -= 64;
        st = before == 0 ? 0 : x >>> (64 - before);
      }
    }
    bytePos = pos;
    staged = st;
    stagedBits = sb;
  }

  /**
   * Makes room for {@code moreBytes} more bytes at once (exactly, not by doubling), so that a
   * writer told what is coming copies its buffer at most once.
   */
  void reserve(long moreBytes) {
    long need = bytePos + 8 + moreBytes + 9;
    if (need > buf.length) {
      buf = Arrays.copyOf(buf, (int) Math.min(need, MAX_CAPACITY));
    }
  }

  void writeBool(boolean b) {
    writeUint(b ? 1 : 0, 1);
  }

  void writeAlignedBytes(byte[] bytes) {
    if ((stagedBits & 7) != 0) {
      throw PcoException.invalidArgument(
          "cannot write aligned bytes to unaligned writer (" + (stagedBits & 7) + " bits past byte)");
    }
    flushStagedBytes();
    ensure((long) bytePos + bytes.length + 1);
    System.arraycopy(bytes, 0, buf, bytePos, bytes.length);
    bytePos += bytes.length;
  }

  void finishByte() {
    // the bits above the staged ones are zero, so padding is just counting them
    stagedBits = (stagedBits + 7) & ~7;
    if (stagedBits == 64) {
      ensure(bytePos + 8L);
      LONG_LE.set(buf, bytePos, staged);
      bytePos += 8;
      staged = 0;
      stagedBits = 0;
    }
  }

  /** The bytes written so far (the last one partial if not aligned). */
  byte[] toByteArray() {
    int stagedBytes = (stagedBits + 7) >>> 3;
    byte[] out = Arrays.copyOf(buf, bytePos + stagedBytes);
    for (int i = 0; i < stagedBytes; i++) {
      out[bytePos + i] = (byte) (staged >>> (8 * i));
    }
    return out;
  }

  /** moves the whole staged bytes (all of them, when aligned) to the buffer */
  private void flushStagedBytes() {
    int stagedBytes = stagedBits >>> 3;
    ensure((long) bytePos + stagedBytes);
    for (int i = 0; i < stagedBytes; i++) {
      buf[bytePos + i] = (byte) (staged >>> (8 * i));
    }
    bytePos += stagedBytes;
    staged = stagedBytes == 8 ? 0 : staged >>> (8 * stagedBytes);
    stagedBits -= 8 * stagedBytes;
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
