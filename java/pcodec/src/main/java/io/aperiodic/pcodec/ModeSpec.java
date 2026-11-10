package io.aperiodic.pcodec;

/** How the compressor chooses the mode ({@code pco::ModeSpec}). */
public final class ModeSpec {
  public enum Kind {
    AUTO,
    CLASSIC,
    TRY_FLOAT_MULT,
    TRY_FLOAT_QUANT,
    TRY_INT_MULT,
    TRY_DICT
  }

  public static final ModeSpec AUTO = new ModeSpec(Kind.AUTO, 0, 0, 0);
  public static final ModeSpec CLASSIC = new ModeSpec(Kind.CLASSIC, 0, 0, 0);
  public static final ModeSpec TRY_DICT = new ModeSpec(Kind.TRY_DICT, 0, 0, 0);

  public final Kind kind;
  final double floatBase;
  final int k;
  final long intBase;

  private ModeSpec(Kind kind, double floatBase, int k, long intBase) {
    this.kind = kind;
    this.floatBase = floatBase;
    this.k = k;
    this.intBase = intBase;
  }

  public static ModeSpec tryFloatMult(double base) {
    return new ModeSpec(Kind.TRY_FLOAT_MULT, base, 0, 0);
  }

  /** {@code k} is a u32 in the reference */
  public static ModeSpec tryFloatQuant(int k) {
    return new ModeSpec(Kind.TRY_FLOAT_QUANT, 0, k, 0);
  }

  /** {@code base} is a u64 in the reference */
  public static ModeSpec tryIntMult(long base) {
    return new ModeSpec(Kind.TRY_INT_MULT, 0, 0, base);
  }

  @Override
  public String toString() {
    switch (kind) {
      case TRY_FLOAT_MULT:
        return "TryFloatMult(" + floatBase + ")";
      case TRY_FLOAT_QUANT:
        return "TryFloatQuant(" + Integer.toUnsignedString(k) + ")";
      case TRY_INT_MULT:
        return "TryIntMult(" + Long.toUnsignedString(intBase) + ")";
      default:
        return kind.toString();
    }
  }
}
