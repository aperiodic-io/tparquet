package io.aperiodic.pcodec;

/** tANS, as {@code pco::ans}: the table spread, the encoder, the decoder table and weights. */
final class Ans {
  private Ans() {}

  /** {@code ans::Spec}: the table of {@code 2^sizeLog} states and the symbol of each. */
  static final class Spec {
    final int sizeLog;
    final int[] stateSymbols;
    final int[] symbolWeights;

    private Spec(int sizeLog, int[] stateSymbols, int[] symbolWeights) {
      this.sizeLog = sizeLog;
      this.stateSymbols = stateSymbols;
      this.symbolWeights = symbolWeights;
    }

    int tableSize() {
      return 1 << sizeLog;
    }

    static Spec fromWeights(int sizeLog, int[] weights) {
      int[] w = weights.length == 0 ? new int[] {1} : weights;
      return new Spec(sizeLog, spreadStateSymbols(sizeLog, w), w);
    }
  }

  static int chooseStride(int tableSize) {
    int res = (3 * tableSize) / 5;
    if (res % 2 == 0) {
      res += 1;
    }
    return res;
  }

  static int[] spreadStateSymbols(int sizeLog, int[] weights) {
    long tableSize = 0;
    for (int w : weights) {
      tableSize += w & 0xffffffffL;
    }
    // Weight is u32: the sum wraps
    int ts = (int) tableSize;
    if (sizeLog > 31 || ts != (1 << sizeLog)) {
      throw PcoException.corruption(
          "table size log of " + sizeLog + " does not agree with total weight of " + ts);
    }
    int[] res = new int[ts];
    int step = 0;
    int stride = chooseStride(ts);
    int modTableSize = ts - 1;
    for (int symbol = 0; symbol < weights.length; symbol++) {
      for (int k = 0; k < weights[symbol]; k++) {
        int stateIdx = (stride * step) & modTableSize;
        res[stateIdx] = symbol;
        step++;
      }
    }
    return res;
  }

  /** {@code ans::Encoder} */
  static final class Encoder {
    final int sizeLog;
    private final int[] renormBitCutoff;
    private final int[] minRenormBits;
    private final int[][] nextStates;

    Encoder(Spec spec) {
      int tableSize = spec.tableSize();
      int nSymbols = spec.symbolWeights.length;
      renormBitCutoff = new int[nSymbols];
      minRenormBits = new int[nSymbols];
      nextStates = new int[nSymbols][];
      int[] filled = new int[nSymbols];
      for (int s = 0; s < nSymbols; s++) {
        int weight = spec.symbolWeights[s];
        int maxXs = 2 * weight - 1;
        int mrb = spec.sizeLog - (31 - Integer.numberOfLeadingZeros(maxXs));
        minRenormBits[s] = mrb;
        renormBitCutoff[s] = 2 * weight * (1 << mrb);
        nextStates[s] = new int[weight];
      }
      for (int stateIdx = 0; stateIdx < spec.stateSymbols.length; stateIdx++) {
        int symbol = spec.stateSymbols[stateIdx];
        nextStates[symbol][filled[symbol]++] = tableSize + stateIdx;
      }
      this.sizeLog = spec.sizeLog;
    }

    /** The new state, and in the high 32 bits how many bits of the old state to write. */
    long encode(int state, int symbol) {
      int renormBits =
          Integer.compareUnsigned(state, renormBitCutoff[symbol]) >= 0
              ? minRenormBits[symbol] + 1
              : minRenormBits[symbol];
      int[] ns = nextStates[symbol];
      int next = ns[(state >>> renormBits) - ns.length];
      return ((long) renormBits << 32) | (next & 0xffffffffL);
    }

    int defaultState() {
      return 1 << sizeLog;
    }
  }

  /** {@code ans::Decoder}: per state, the next state base, the bits to read, the offset bits. */
  static final class Decoder {
    final int[] nextStateIdxBase;
    final int[] bitsToRead;
    final int[] offsetBits;

    Decoder(Spec spec, int[] binOffsetBits) {
      int tableSize = spec.tableSize();
      int n = spec.stateSymbols.length;
      nextStateIdxBase = new int[n];
      bitsToRead = new int[n];
      offsetBits = new int[n];
      int[] symbolXs = spec.symbolWeights.clone();
      for (int i = 0; i < n; i++) {
        int symbol = spec.stateSymbols[i];
        int nextStateBase = symbolXs[symbol];
        int btr =
            Integer.numberOfLeadingZeros(nextStateBase) - Integer.numberOfLeadingZeros(tableSize);
        nextStateBase <<= btr;
        nextStateIdxBase[i] = (nextStateBase - tableSize) & 0xffff;
        bitsToRead[i] = btr & 0xff;
        offsetBits[i] = (symbol < binOffsetBits.length ? binOffsetBits[symbol] : 0) & 0xff;
        symbolXs[symbol]++;
      }
    }
  }

  /** {@code quantize_weights_to}: counts to weights summing to {@code 2^sizeLog}, in f32. */
  static int[] quantizeWeightsTo(int[] counts, long totalCount, int sizeLog) {
    if (sizeLog == 0) {
      return new int[] {1};
    }
    int requiredWeightSum = 1 << sizeLog;
    float multiplier = (float) requiredWeightSum / RustMath.u64ToF32(totalCount);
    float[] desiredSurplusPerBin = new float[counts.length];
    for (int i = 0; i < counts.length; i++) {
      float v = RustMath.u64ToF32(counts[i] & 0xffffffffL) * multiplier - 1.0f;
      desiredSurplusPerBin[i] = RustMath.maxF32(v, 0.0f);
    }
    float desiredSurplus = RustMath.sumF32(desiredSurplusPerBin);
    int requiredSurplus = requiredWeightSum - counts.length;
    float surplusMult =
        desiredSurplus == 0.0f ? 0.0f : RustMath.u64ToF32(requiredSurplus & 0xffffffffL) / desiredSurplus;
    float[] floatWeights = new float[counts.length];
    for (int i = 0; i < counts.length; i++) {
      floatWeights[i] = 1.0f + desiredSurplusPerBin[i] * surplusMult;
    }
    int[] weights = new int[counts.length];
    int weightSum = 0;
    for (int i = 0; i < counts.length; i++) {
      weights[i] = (int) RustMath.f32ToU32(RustMath.roundF32(floatWeights[i]));
      weightSum += weights[i];
    }
    int i = 0;
    while (Integer.compareUnsigned(weightSum, requiredWeightSum) > 0) {
      if (Integer.compareUnsigned(weights[i], 1) > 0
          && RustMath.u64ToF32(weights[i] & 0xffffffffL) > floatWeights[i]) {
        weights[i] -= 1;
        weightSum -= 1;
      }
      i++;
    }
    i = 0;
    while (Integer.compareUnsigned(weightSum, requiredWeightSum) < 0) {
      if (RustMath.u64ToF32(weights[i] & 0xffffffffL) < floatWeights[i]) {
        weights[i] += 1;
        weightSum += 1;
      }
      i++;
    }
    return weights;
  }

  /**
   * {@code quantize_weights}: the table size log and the weights. Returns the size log at index
   * {@code weights.length}.
   */
  static int[] quantizeWeights(int[] counts, long totalCount, int maxSizeLog) {
    if (counts.length == 1) {
      return new int[] {1, 0};
    }
    int minSizeLog = 64 - Long.numberOfLeadingZeros(counts.length - 1L);
    int sizeLog = Math.max(minSizeLog, maxSizeLog);
    int[] weights = quantizeWeightsTo(counts, totalCount, sizeLog);
    int powerOf2 = Integer.MAX_VALUE;
    for (int w : weights) {
      powerOf2 = Math.min(powerOf2, Integer.numberOfTrailingZeros(w));
    }
    sizeLog -= powerOf2;
    int[] res = new int[weights.length + 1];
    for (int k = 0; k < weights.length; k++) {
      res[k] = weights[k] >>> powerOf2;
    }
    res[weights.length] = sizeLog;
    return res;
  }
}
