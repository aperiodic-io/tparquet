package io.aperiodic.pcodec;

import java.util.Arrays;

/** Comparing a Java outcome with the reference's. */
final class Parity {
  private Parity() {}

  /** a Java outcome in the oracle's terms: status 0 ok, 1 error, 2 panic */
  static final class Outcome {
    final int status;
    final byte[] body;
    final String detail;

    Outcome(int status, byte[] body, String detail) {
      this.status = status;
      this.body = body;
      this.detail = detail;
    }

    @Override
    public String toString() {
      return status == 0 ? "ok(" + body.length + " bytes)" : (status == 1 ? "error: " : "panic: ") + detail;
    }
  }

  interface Run {
    byte[] run();
  }

  static Outcome java(Run r) {
    try {
      return new Outcome(0, r.run(), null);
    } catch (PcoException e) {
      return new Outcome(1, null, e.getMessage());
    } catch (PcoPanic | IndexOutOfBoundsException | ArithmeticException | NegativeArraySizeException e) {
      return new Outcome(2, null, e.toString());
    }
  }

  /** null if the outcomes agree (status, and bytes when ok), else a description */
  static String compare(Oracle.Answer want, Outcome got) {
    if (want.status != got.status) {
      return "reference " + want + ", java " + got;
    }
    if (want.status == 0 && !Arrays.equals(want.body, got.body)) {
      int i = 0;
      while (i < Math.min(want.body.length, got.body.length) && want.body[i] == got.body[i]) {
        i++;
      }
      return "bytes differ at " + i + " (reference " + want.body.length + " bytes, java "
          + got.body.length + ")";
    }
    return null;
  }

  /** the oracle's decompress body for a Java decode: n, values, end */
  static byte[] decodeBody(NumberType t, byte[] file) {
    Pco.Decoded d = Pco.decompress(file, t);
    byte[] raw = Oracle.rawBytes(t, d.nums);
    java.nio.ByteBuffer b = java.nio.ByteBuffer.allocate(8 + raw.length).order(java.nio.ByteOrder.LITTLE_ENDIAN);
    b.putInt(d.nums.length).put(raw).putInt(d.end);
    return b.array();
  }
}
