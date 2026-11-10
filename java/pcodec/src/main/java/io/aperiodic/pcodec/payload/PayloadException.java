package io.aperiodic.pcodec.payload;

/** A payload that cannot be decoded, or an option refused (the reference's {@code Error}). */
public final class PayloadException extends RuntimeException {
  private static final long serialVersionUID = 1L;

  PayloadException(String message) {
    super("pcodec payload: " + message);
  }

  PayloadException(String message, Throwable cause) {
    super("pcodec payload: " + message, cause);
  }
}
