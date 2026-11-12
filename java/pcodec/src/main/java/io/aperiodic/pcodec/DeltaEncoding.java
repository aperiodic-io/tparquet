package io.aperiodic.pcodec;

/**
 * How a chunk was delta encoded ({@code pco::metadata::DeltaEncoding}), and, through {@link
 * #forLatentVar}, how each latent variable is ({@code LatentVarDeltaEncoding}).
 */
public final class DeltaEncoding {
  public enum Kind {
    NO_OP,
    CONSECUTIVE,
    LOOKBACK,
    CONV1
  }

  static final int BITS_VARIANT = 4;
  static final int BITS_ORDER = 3;
  static final int BITS_CONV_QUANTIZATION = 5;
  static final int BITS_CONV_N_WEIGHTS = 5;
  static final int BITS_LOOKBACK_WINDOW_N_LOG = 5;
  static final int BITS_LOOKBACK_STATE_N_LOG = 4;
  static final int MAX_CONSECUTIVE_ORDER = 7;
  static final int MAX_CONV1_ORDER = 32;
  static final int MAX_CONV1_QUANTIZATION = (1 << BITS_CONV_QUANTIZATION) - 1;
  static final int MAX_LOOKBACK_WINDOW_N_LOG = 24;
  static final int MAX_BIT_SIZE =
      BITS_VARIANT + BITS_CONV_QUANTIZATION + BITS_CONV_N_WEIGHTS + 64 + MAX_CONV1_ORDER * 32;

  static final DeltaEncoding NO_OP = new DeltaEncoding(Kind.NO_OP, 0, false, 0, 0, 0, 0, null);

  public final Kind kind;
  /** consecutive order */
  public final int order;
  public final boolean secondaryUsesDelta;
  public final int windowNLog;
  public final int stateNLog;
  public final int quantization;
  final long bias;
  final long[] weights;

  private DeltaEncoding(
      Kind kind,
      int order,
      boolean secondaryUsesDelta,
      int windowNLog,
      int stateNLog,
      int quantization,
      long bias,
      long[] weights) {
    this.kind = kind;
    this.order = order;
    this.secondaryUsesDelta = secondaryUsesDelta;
    this.windowNLog = windowNLog;
    this.stateNLog = stateNLog;
    this.quantization = quantization;
    this.bias = bias;
    this.weights = weights;
  }

  static DeltaEncoding consecutive(int order, boolean secondaryUsesDelta) {
    return new DeltaEncoding(Kind.CONSECUTIVE, order, secondaryUsesDelta, 0, 0, 0, 0, null);
  }

  static DeltaEncoding lookback(int windowNLog, int stateNLog, boolean secondaryUsesDelta) {
    return new DeltaEncoding(
        Kind.LOOKBACK, 0, secondaryUsesDelta, windowNLog, stateNLog, 0, 0, null);
  }

  static DeltaEncoding conv1(int quantization, long bias, long[] weights) {
    return new DeltaEncoding(Kind.CONV1, 0, false, 0, 0, quantization, bias, weights);
  }

  int stateN() {
    return 1 << stateNLog;
  }

  int windowN() {
    return 1 << windowNLog;
  }

  /** {@code DeltaConv1Config::bias::<S>()}: truncated to the convolution type's width */
  long conv1Bias(int convBits) {
    return Conv.wrap(bias, convBits);
  }

  long[] conv1Weights(int convBits) {
    long[] w = new long[weights.length];
    for (int i = 0; i < w.length; i++) {
      w[i] = Conv.wrap(weights[i], convBits);
    }
    return w;
  }

  public long conv1Bias() {
    return bias;
  }

  public long[] conv1Weights() {
    return weights == null ? null : weights.clone();
  }

  /** the latent type of the delta latent variable, or null */
  Latent latentType() {
    return kind == Kind.LOOKBACK ? Latent.U32 : null;
  }

  static final int KEY_DELTA = 0;
  static final int KEY_PRIMARY = 1;
  static final int KEY_SECONDARY = 2;

  DeltaEncoding forLatentVar(int key) {
    if (kind == Kind.NO_OP || key == KEY_DELTA) {
      return NO_OP;
    }
    switch (kind) {
      case CONSECUTIVE:
        return key == KEY_PRIMARY || secondaryUsesDelta ? consecutive(order, false) : NO_OP;
      case LOOKBACK:
        return key == KEY_PRIMARY || secondaryUsesDelta
            ? lookback(windowNLog, stateNLog, false)
            : NO_OP;
      default:
        return key == KEY_PRIMARY ? this : NO_OP;
    }
  }

  /** {@code n_latents_per_state} of a latent variable's encoding */
  int nLatentsPerState() {
    switch (kind) {
      case NO_OP:
        return 0;
      case CONSECUTIVE:
        return order;
      case LOOKBACK:
        return 1 << stateNLog;
      default:
        return weights.length;
    }
  }

  void writeTo(BitWriter w) {
    w.writeUint(kind.ordinal(), BITS_VARIANT);
    switch (kind) {
      case NO_OP:
        break;
      case CONSECUTIVE:
        w.writeUint(order, BITS_ORDER);
        w.writeBool(secondaryUsesDelta);
        break;
      case LOOKBACK:
        w.writeUint(windowNLog - 1, BITS_LOOKBACK_WINDOW_N_LOG);
        w.writeUint(stateNLog, BITS_LOOKBACK_STATE_N_LOG);
        w.writeBool(secondaryUsesDelta);
        break;
      default:
        w.writeUint(quantization, BITS_CONV_QUANTIZATION);
        w.writeUint(NumberType.I64.toLatentOrdered(bias), 64);
        w.writeUint(weights.length - 1, BITS_CONV_N_WEIGHTS);
        for (long weight : weights) {
          w.writeUint(NumberType.I32.toLatentOrdered(weight & 0xffffffffL), 32);
        }
        break;
    }
  }

  /** {@code DeltaEncoding::read_from} for format versions with delta variants (3 and later) */
  static DeltaEncoding readFrom(BitReader r, int formatMajor) {
    if (formatMajor < 3) {
      int order = r.readInt(BITS_ORDER);
      return order == 0 ? NO_OP : consecutive(order, false);
    }
    int variant = r.readInt(BITS_VARIANT);
    switch (variant) {
      case 0:
        return NO_OP;
      case 1:
        {
          int order = r.readInt(BITS_ORDER);
          if (order == 0) {
            throw PcoException.corruption("Consecutive delta encoding order must not be 0");
          }
          return consecutive(order, r.readBool());
        }
      case 2:
        {
          int windowNLog = 1 + r.readInt(BITS_LOOKBACK_WINDOW_N_LOG);
          int stateNLog = r.readInt(BITS_LOOKBACK_STATE_N_LOG);
          if (windowNLog > MAX_LOOKBACK_WINDOW_N_LOG) {
            throw PcoException.corruption(
                "LZ delta encoding window size log of "
                    + windowNLog
                    + " exceeds max of "
                    + MAX_LOOKBACK_WINDOW_N_LOG);
          }
          if (stateNLog > windowNLog) {
            throw PcoException.corruption(
                "LZ delta encoding state size log exceeded window size log: "
                    + stateNLog
                    + " vs "
                    + windowNLog);
          }
          return lookback(windowNLog, stateNLog, r.readBool());
        }
      case 3:
        {
          int quantization = r.readInt(BITS_CONV_QUANTIZATION);
          long bias = NumberType.I64.fromLatentOrdered(r.readUint(64));
          int order = 1 + r.readInt(BITS_CONV_N_WEIGHTS);
          long[] weights = new long[order];
          for (int i = 0; i < order; i++) {
            weights[i] = (int) NumberType.I32.fromLatentOrdered(r.readUint(32));
          }
          return conv1(quantization, bias, weights);
        }
      default:
        throw PcoException.corruption("unknown delta encoding value: " + variant);
    }
  }

  /** Rust's {@code Debug} of the reference's {@code DeltaEncoding} */
  @Override
  public String toString() {
    switch (kind) {
      case NO_OP:
        return "NoOp";
      case CONSECUTIVE:
        return "Consecutive { order: " + order + ", secondary_uses_delta: " + secondaryUsesDelta + " }";
      case LOOKBACK:
        return "Lookback { config: DeltaLookbackConfig { state_n_log: " + stateNLog + ", window_n_log: "
            + windowNLog + " }, secondary_uses_delta: " + secondaryUsesDelta + " }";
      default:
        StringBuilder w = new StringBuilder();
        for (int i = 0; i < weights.length; i++) {
          w.append(i == 0 ? "" : ", ").append(weights[i]);
        }
        return "Conv1(DeltaConv1Config { quantization: " + quantization + ", bias: " + bias + ", weights: ["
            + w + "] })";
    }
  }

  /** wrapping arithmetic in the signed convolution type */
  static final class Conv {
    private Conv() {}

    static long wrap(long x, int convBits) {
      return convBits == 64 ? x : (x << (64 - convBits)) >> (64 - convBits);
    }
  }
}
