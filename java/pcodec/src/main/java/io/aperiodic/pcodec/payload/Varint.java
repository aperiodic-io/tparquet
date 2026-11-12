package io.aperiodic.pcodec.payload;

import java.io.ByteArrayOutputStream;

/** Unsigned LEB128, at most 10 bytes, below 2^64 (spec section 4). */
final class Varint {
  private Varint() {}

  static void put(ByteArrayOutputStream out, long v) {
    while (Long.compareUnsigned(v, 0x80) >= 0) {
      out.write((int) (v & 0x7f) | 0x80);
      v >>>= 7;
    }
    out.write((int) v);
  }

  /** reads one varint at {@code at[0]}, before {@code end}, advancing {@code at[0]} */
  static long get(byte[] src, int[] at, int end) {
    long v = 0;
    for (int i = 0; i < 10; i++) {
      if (at[0] >= end) {
        throw new PayloadException("truncated");
      }
      int b = src[at[0]++] & 0xff;
      long part = b & 0x7f;
      if (i == 9 && part > 1) {
        throw new PayloadException("truncated");
      }
      v |= part << (7 * i);
      if (b < 0x80) {
        return v;
      }
    }
    throw new PayloadException("truncated");
  }
}
