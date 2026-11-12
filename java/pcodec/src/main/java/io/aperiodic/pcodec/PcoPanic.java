package io.aperiodic.pcodec;

/**
 * Where the reference implementation panics (an out-of-bounds slice or index) on an input it
 * accepts, this is thrown instead, so that both fail on the same inputs.
 */
public final class PcoPanic extends RuntimeException {
  private static final long serialVersionUID = 1L;

  PcoPanic(String message) {
    super(message);
  }
}
