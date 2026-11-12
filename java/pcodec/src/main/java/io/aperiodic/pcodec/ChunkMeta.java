package io.aperiodic.pcodec;

/** The metadata of a chunk ({@code pco::metadata::ChunkMeta}). */
public final class ChunkMeta {
  static final int BITS_ANS_SIZE_LOG = 4;
  static final int BITS_N_BINS = 15;
  static final int MAX_ANS_BITS = 14;
  static final int ANS_INTERLEAVING = 4;
  private static final int FULL_BIN_BATCH_LEN = 128;

  /** {@code Bin} */
  public static final class Bin {
    public final int weight;
    public final long lower;
    public final int offsetBits;

    Bin(int weight, long lower, int offsetBits) {
      this.weight = weight;
      this.lower = lower;
      this.offsetBits = offsetBits;
    }

    int worstCaseBitsPerLatent(int ansSizeLog) {
      return offsetBits + ansSizeLog - (31 - Integer.numberOfLeadingZeros(weight));
    }

    @Override
    public String toString() {
      return "Bin { weight: " + weight + ", lower: " + Long.toUnsignedString(lower) + ", offset_bits: " + offsetBits + " }";
    }
  }

  /** {@code ChunkLatentVarMeta} */
  public static final class LatentVar {
    public final int ansSizeLog;
    final Latent latent;
    public final Bin[] bins;

    LatentVar(int ansSizeLog, Latent latent, Bin[] bins) {
      this.ansSizeLog = ansSizeLog;
      this.latent = latent;
      this.bins = bins;
    }

    public int latentBits() {
      return latent.bits;
    }

    long exactBitSize() {
      return BITS_ANS_SIZE_LOG
          + BITS_N_BINS
          + (long) bins.length * BinOptimization.binExactBitSize(latent, ansSizeLog);
    }

    long exactPageMetaBitSize(DeltaEncoding enc) {
      return (long) ansSizeLog * ANS_INTERLEAVING + (long) latent.bits * enc.nLatentsPerState();
    }

    void writeTo(BitWriter w) {
      w.writeUint(ansSizeLog, BITS_ANS_SIZE_LOG);
      w.writeUint(bins.length, BITS_N_BINS);
      int obb = BinOptimization.offsetBitsBits(latent);
      for (Bin bin : bins) {
        w.writeUint((bin.weight - 1) & 0xffffffffL, ansSizeLog);
        w.writeUint(bin.lower, latent.bits);
        w.writeUint(bin.offsetBits, obb);
      }
    }

    static LatentVar readFrom(BitReader r, Latent latent) {
      int ansSizeLog = r.readInt(BITS_ANS_SIZE_LOG);
      int nBins = r.readInt(BITS_N_BINS);
      r.checkInBounds();
      if ((1 << ansSizeLog) < nBins) {
        throw PcoException.corruption(
            "ANS size log (" + ansSizeLog + ") is too small for number of bins (" + nBins + ")");
      }
      if (nBins == 1 && ansSizeLog > 0) {
        throw PcoException.corruption(
            "Only 1 bin but ANS size log is " + ansSizeLog + " (should be 0)");
      }
      if (ansSizeLog > MAX_ANS_BITS) {
        throw PcoException.corruption(
            "ANS size log (" + ansSizeLog + ") should not be greater than " + MAX_ANS_BITS);
      }
      int obb = BinOptimization.offsetBitsBits(latent);
      Bin[] bins = new Bin[nBins];
      for (int start = 0; start < nBins; start += FULL_BIN_BATCH_LEN) {
        int end = Math.min(start + FULL_BIN_BATCH_LEN, nBins);
        for (int i = start; i < end; i++) {
          int weight = r.readInt(ansSizeLog) + 1;
          long lower = r.readUint(latent.bits);
          int offsetBits = r.readInt(obb);
          if (offsetBits > latent.bits) {
            r.checkInBounds();
            throw PcoException.corruption(
                "offset bits of " + offsetBits + " exceeds type of " + latent.bits + " bits");
          }
          bins[i] = new Bin(weight, lower, offsetBits);
        }
        r.checkInBounds();
      }
      return new LatentVar(ansSizeLog, latent, bins);
    }

    @Override
    public String toString() {
      StringBuilder sb = new StringBuilder();
      for (int i = 0; i < bins.length; i++) {
        sb.append(i == 0 ? "" : ", ").append(bins[i]);
      }
      return "ChunkLatentVarMeta { ans_size_log: " + ansSizeLog + ", bins: "
          + latent.toString().toUpperCase(java.util.Locale.ROOT) + "([" + sb + "]) }";
    }
  }

  public final Mode mode;
  public final DeltaEncoding deltaEncoding;
  /** delta, primary, secondary (delta and secondary may be null) */
  final LatentVar[] perLatentVar;
  final Latent numberLatent;

  ChunkMeta(Mode mode, DeltaEncoding deltaEncoding, LatentVar[] perLatentVar, Latent numberLatent) {
    this.mode = mode;
    this.deltaEncoding = deltaEncoding;
    this.perLatentVar = perLatentVar;
    this.numberLatent = numberLatent;
  }

  public LatentVar deltaVar() {
    return perLatentVar[0];
  }

  public LatentVar primaryVar() {
    return perLatentVar[1];
  }

  public LatentVar secondaryVar() {
    return perLatentVar[2];
  }

  /** {@code ChunkMeta::new}: checks the delta encoding against the rest */
  static ChunkMeta create(
      Mode mode, DeltaEncoding delta, LatentVar[] vars, Latent numberLatent) {
    switch (delta.kind) {
      case LOOKBACK:
        {
          long windowN = delta.windowN();
          for (Bin bin : vars[0].bins) {
            if (Long.compareUnsigned(bin.lower, 1) < 0 || Long.compareUnsigned(bin.lower, windowN) > 0) {
              throw PcoException.corruption(
                  "delta lookback bin had invalid lower bound of "
                      + Long.toUnsignedString(bin.lower)
                      + " outside window [1, "
                      + windowN
                      + "]");
            }
          }
          break;
        }
      case CONV1:
        {
          Latent lt = vars[1].latent;
          if (lt == Latent.U64) {
            throw PcoException.corruption(
                "Conv1 delta encodings are not supported on types larger than 32 bits");
          }
          int maxQuantization = Math.min(DeltaEncoding.MAX_CONV1_QUANTIZATION, lt.convBits - 1);
          if (delta.quantization > maxQuantization) {
            throw PcoException.corruption(
                "Conv1 delta encoding quantization of "
                    + delta.quantization
                    + " exceeds max of "
                    + maxQuantization);
          }
          double sum = -0.0;
          for (long w : delta.weights) {
            sum += (double) Math.abs(w);
          }
          double maxPred = Math.abs((double) delta.bias) + Math.scalb(1.0, lt.bits) * sum;
          if (maxPred >= Math.scalb(1.0, lt.convBits - 1)) {
            throw PcoException.corruption(
                "Conv1 delta encoding weights and bias risk overflowing as high as " + maxPred);
          }
          break;
        }
      default:
        break;
    }
    return new ChunkMeta(mode, delta, vars, numberLatent);
  }

  long maxSize() {
    long bits = (long) mode.maxBitSize(numberLatent) + DeltaEncoding.MAX_BIT_SIZE;
    for (LatentVar v : perLatentVar) {
      if (v != null) {
        bits += v.exactBitSize();
      }
    }
    return (bits + 7) / 8;
  }

  long exactPageMetaSize() {
    long bits = 0;
    for (int key = 0; key < 3; key++) {
      LatentVar v = perLatentVar[key];
      if (v != null) {
        bits += v.exactPageMetaBitSize(deltaEncoding.forLatentVar(key));
      }
    }
    return (bits + 7) / 8;
  }

  void writeTo(BitWriter w) {
    mode.writeTo(w, numberLatent);
    deltaEncoding.writeTo(w);
    for (LatentVar v : perLatentVar) {
      if (v != null) {
        v.writeTo(w);
      }
    }
    w.finishByte();
  }

  static ChunkMeta readFrom(BitReader r, int formatMajor, Latent numberLatent) {
    Mode mode = Mode.readFrom(r, formatMajor, numberLatent);
    DeltaEncoding delta = DeltaEncoding.readFrom(r, formatMajor);
    r.checkInBounds();
    LatentVar[] vars = new LatentVar[3];
    Latent deltaType = delta.latentType();
    if (deltaType != null) {
      vars[0] = LatentVar.readFrom(r, deltaType);
    }
    vars[1] = LatentVar.readFrom(r, mode.primaryLatentType(numberLatent));
    Latent secondaryType = mode.secondaryLatentType(numberLatent);
    if (secondaryType != null) {
      vars[2] = LatentVar.readFrom(r, secondaryType);
    }
    r.drainEmptyByte("nonzero bits in end of final byte of chunk metadata");
    r.checkInBounds();
    return create(mode, delta, vars, numberLatent);
  }

  static String dynLatent(Latent lt, long x) {
    return lt.toString().toUpperCase(java.util.Locale.ROOT) + "(" + Long.toUnsignedString(x) + ")";
  }

  private static String option(LatentVar v) {
    return v == null ? "None" : "Some(" + v + ")";
  }

  /** Rust's {@code Debug} of the reference's {@code ChunkMeta} */
  @Override
  public String toString() {
    return "ChunkMeta { mode: " + mode.debug(numberLatent) + ", delta_encoding: " + deltaEncoding
        + ", per_latent_var: PerLatentVar { delta: " + option(perLatentVar[0]) + ", primary: "
        + perLatentVar[1] + ", secondary: " + option(perLatentVar[2]) + " }, _private: () }";
  }
}
