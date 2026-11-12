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
    /** a PcoException's kind as the reference names it (e.g. "InsufficientData"), else null */
    final String kind;

    Outcome(int status, byte[] body, String detail) {
      this(status, body, detail, null);
    }

    Outcome(int status, byte[] body, String detail, String kind) {
      this.status = status;
      this.body = body;
      this.detail = detail;
      this.kind = kind;
    }

    @Override
    public String toString() {
      return status == 0
          ? "ok(" + body.length + " bytes)"
          : (status == 1 ? "error" + (kind == null ? "" : " " + kind) + ": " : "panic: ") + detail;
    }
  }

  interface Run {
    byte[] run();
  }

  static Outcome java(Run r) {
    try {
      return new Outcome(0, r.run(), null);
    } catch (PcoException e) {
      return new Outcome(1, null, e.getMessage(), rustKind(e.kind()));
    } catch (PcoPanic | IndexOutOfBoundsException | ArithmeticException | NegativeArraySizeException e) {
      return new Outcome(2, null, e.toString());
    }
  }

  /** {@code pco::errors::ErrorKind}'s Debug name */
  static String rustKind(PcoException.Kind k) {
    switch (k) {
      case CORRUPTION:
        return "Corruption";
      case INSUFFICIENT_DATA:
        return "InsufficientData";
      default:
        return "InvalidArgument";
    }
  }

  private static final java.util.regex.Pattern PCO_ERROR = java.util.regex.Pattern.compile("^pco (\\w+) error: ");

  static final String PAYLOAD = "pcodec payload: ";

  /** the kind in a reference error's message (`pco {kind:?} error: ...`), or null if none */
  static String referenceKind(Oracle.Answer a) {
    java.util.regex.Matcher m = PCO_ERROR.matcher(a.message());
    return m.find() ? m.group(1) : null;
  }

  /**
   * null if the outcomes agree (status; bytes when ok; the error kind when the reference's
   * error is a pco error), else a description
   */
  static String compare(Oracle.Answer want, Outcome got) {
    if (want.status != got.status) {
      return "reference " + want + ", java " + got;
    }
    if (want.status == 1) {
      String m = want.message();
      if (m.startsWith(PAYLOAD + "pcodec: ")) {
        // a pco error inside a payload: its kind (the messages' bit positions may differ)
        java.util.regex.Matcher k = PCO_ERROR.matcher(m.substring(PAYLOAD.length() + "pcodec: ".length()));
        if (k.find() && !k.group(1).equals(got.kind)) {
          return "reference " + want + ", java " + got;
        }
      } else if (m.startsWith(PAYLOAD + "zstd: ")) {
        // libzstd's and the Java Zstandard's messages differ
        if (got.detail == null || !got.detail.startsWith(PAYLOAD + "zstd: ")) {
          return "reference " + want + ", java " + got;
        }
      } else if (m.startsWith(PAYLOAD)) {
        // the payload's own errors: the same message
        if (!m.equals(got.detail)) {
          return "reference " + want + ", java " + got;
        }
      } else {
        String k = referenceKind(want);
        if (k != null && !k.equals(got.kind)) {
          return "reference " + want + ", java " + got;
        }
      }
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
