package io.aperiodic.pcodec;

/** Why pcodec could not compress or decompress, with the reference's three error kinds. */
public final class PcoException extends RuntimeException {
  private static final long serialVersionUID = 1L;

  /** The kinds of error of the reference implementation ({@code pco::errors::ErrorKind}). */
  public enum Kind {
    /** The compressed bytes are not a valid pcodec file. */
    CORRUPTION,
    /** The compressed bytes end before the file does. */
    INSUFFICIENT_DATA,
    /** An argument (configuration, numbers) cannot be compressed. */
    INVALID_ARGUMENT,
  }

  private final Kind kind;

  PcoException(Kind kind, String message) {
    super(message);
    this.kind = kind;
  }

  public Kind kind() {
    return kind;
  }

  static PcoException corruption(String message) {
    return new PcoException(Kind.CORRUPTION, message);
  }

  static PcoException insufficientData(String message) {
    return new PcoException(Kind.INSUFFICIENT_DATA, message);
  }

  static PcoException invalidArgument(String message) {
    return new PcoException(Kind.INVALID_ARGUMENT, message);
  }
}
