package io.aperiodic.pcodec;

/** How the compressor chooses the delta encoding ({@code pco::DeltaSpec}). */
public final class DeltaSpec {
  public enum Kind {
    AUTO,
    NO_OP,
    TRY_CONSECUTIVE,
    TRY_LOOKBACK,
    TRY_CONV1
  }

  public static final DeltaSpec AUTO = new DeltaSpec(Kind.AUTO, 0);
  public static final DeltaSpec NO_OP = new DeltaSpec(Kind.NO_OP, 0);
  public static final DeltaSpec TRY_LOOKBACK = new DeltaSpec(Kind.TRY_LOOKBACK, 0);

  public final Kind kind;
  public final long order;

  private DeltaSpec(Kind kind, long order) {
    this.kind = kind;
    this.order = order;
  }

  public static DeltaSpec tryConsecutive(long order) {
    return new DeltaSpec(Kind.TRY_CONSECUTIVE, order);
  }

  public static DeltaSpec tryConv1(long order) {
    return new DeltaSpec(Kind.TRY_CONV1, order);
  }

  @Override
  public String toString() {
    return kind == Kind.TRY_CONSECUTIVE || kind == Kind.TRY_CONV1
        ? kind + "(" + order + ")"
        : kind.toString();
  }
}
