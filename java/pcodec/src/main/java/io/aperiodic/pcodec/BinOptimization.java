package io.aperiodic.pcodec;

import java.util.ArrayList;
import java.util.List;

/** {@code pco::bin_optimization}: merges histogram bins to minimize the estimated bit size. */
final class BinOptimization {
  private BinOptimization() {}

  private static final float SINGLE_BIN_SPEEDUP_WORTH_IN_BITS_PER_NUM = 0.1f;
  private static final float TRIVIAL_OFFSET_SPEEDUP_WORTH_IN_BITS_PER_NUM = 0.1f;

  private static final float Z = 0.674f;
  private static final int SIGNIF_MASK = 0x7FFFFF;
  private static final int Z_SIGNIF = Float.floatToRawIntBits(Z) & SIGNIF_MASK;
  private static final float B = 2.0f / Z;
  private static final float C = -B / (6.0f * Z);
  private static final float A = -B - C;

  /** {@code log2_approx} (for positive normal x) */
  static float log2Approx(float x) {
    int bits = Float.floatToRawIntBits(x);
    int exp = bits >>> 23;
    int signif = bits & SIGNIF_MASK;
    int highBit = Integer.compareUnsigned(signif, Z_SIGNIF) > 0 ? 1 : 0;
    int logInt = exp + highBit - 127;
    int exp2 = 0x7F ^ highBit;
    float normalized = Float.intBitsToFloat((exp2 << 23) | signif);
    return RustMath.u64ToF32(logInt & 0xffffffffL) + A + normalized * (B + C * normalized);
  }

  /** {@code Bin::<L>::exact_bit_size} */
  static int binExactBitSize(Latent lt, int ansSizeLog) {
    return ansSizeLog + lt.bits + offsetBitsBits(lt);
  }

  /** {@code bits_to_encode_offset_bits::<L>()} */
  static int offsetBitsBits(Latent lt) {
    return 32 - Integer.numberOfLeadingZeros(lt.bits);
  }

  private static float binCost(
      Latent lt, float binMetaCost, long lower, long upper, int count, float totalCountLog2) {
    float countF = RustMath.u64ToF32(count & 0xffffffffL);
    float ansCost = totalCountLog2 - log2Approx(countF);
    float offsetCost = (float) lt.bitsToEncode(lt.sub(upper, lower));
    return binMetaCost + (ansCost + offsetCost) * countF;
  }

  /** the inclusive bin index ranges [j, i] to merge */
  static List<int[]> chooseOptimizedPartitioning(
      Latent lt, List<Histogram.Bin> bins, int ansSizeLog) {
    int nb = bins.size();
    int[] cCounts = new int[nb + 1];
    float[] bestCosts = new float[nb + 1];
    int c = 0;
    bestCosts[0] = 0.0f;
    for (int k = 0; k < nb; k++) {
      c += (int) bins.get(k).count;
      cCounts[k + 1] = c;
      bestCosts[k + 1] = Float.NaN;
    }
    int totalCount = c;
    long[] lowers = new long[nb];
    long[] uppers = new long[nb];
    for (int k = 0; k < nb; k++) {
      lowers[k] = bins.get(k).lower;
      uppers[k] = bins.get(k).upper;
    }
    float totalCountLog2 = log2Approx(RustMath.u64ToF32(c & 0xffffffffL));
    int[] bestJs = new int[nb];
    float binMetaCost = (float) binExactBitSize(lt, ansSizeLog);
    for (int i = 0; i < nb; i++) {
      float bestCost = Float.MAX_VALUE;
      int bestJ = -1;
      long upper = uppers[i];
      int cCountI = cCounts[i + 1];
      for (int j = i; j >= 0; j--) {
        long lower = lowers[j];
        float cost =
            bestCosts[j]
                + binCost(lt, binMetaCost, lower, upper, cCountI - cCounts[j], totalCountLog2);
        if (cost < bestCost) {
          bestCost = cost;
          bestJ = j;
        }
      }
      bestCosts[i + 1] = bestCost;
      bestJs[i] = bestJ;
    }
    float bestCost = bestCosts[nb];

    float singleBinCost =
        binCost(lt, binMetaCost, lowers[0], uppers[nb - 1], totalCount, totalCountLog2);
    float totalF = RustMath.u64ToF32(totalCount & 0xffffffffL);
    if (singleBinCost < bestCost + SINGLE_BIN_SPEEDUP_WORTH_IN_BITS_PER_NUM * totalF) {
      List<int[]> single = new ArrayList<>();
      single.add(new int[] {0, nb - 1});
      return single;
    }

    boolean allTrivial = true;
    for (Histogram.Bin bin : bins) {
      if (bin.lower != bin.upper) {
        allTrivial = false;
        break;
      }
    }
    if (allTrivial) {
      float s = -0.0f;
      for (Histogram.Bin bin : bins) {
        s += binCost(lt, binMetaCost, bin.lower, bin.upper, (int) bin.count, totalCountLog2);
      }
      if (s < bestCost + TRIVIAL_OFFSET_SPEEDUP_WORTH_IN_BITS_PER_NUM * totalF) {
        List<int[]> res = new ArrayList<>();
        for (int k = 0; k < nb; k++) {
          res.add(new int[] {k, k});
        }
        return res;
      }
    }

    List<int[]> res = new ArrayList<>();
    int i = nb - 1;
    while (true) {
      int j = bestJs[i];
      res.add(0, new int[] {j, i});
      if (j > 0) {
        i = j - 1;
      } else {
        break;
      }
    }
    return res;
  }

  /** {@code optimize_bins} */
  static List<BinInfo> optimizeBins(Latent lt, List<Histogram.Bin> bins, int ansSizeLog) {
    List<int[]> partitioning = chooseOptimizedPartitioning(lt, bins, ansSizeLog);
    List<BinInfo> res = new ArrayList<>(partitioning.size());
    for (int symbol = 0; symbol < partitioning.size(); symbol++) {
      int j = partitioning.get(symbol)[0];
      int i = partitioning.get(symbol)[1];
      long count = 0;
      for (int k = j; k <= i; k++) {
        count += bins.get(k).count;
      }
      long lower = bins.get(j).lower;
      long upper = bins.get(i).upper;
      res.add(new BinInfo((int) count, lower, upper, lt.bitsToEncode(lt.sub(upper, lower)), symbol));
    }
    return res;
  }

  /** {@code BinCompressionInfo} */
  static final class BinInfo {
    int weight;
    final long lower;
    final long upper;
    final int offsetBits;
    final int symbol;

    BinInfo(int weight, long lower, long upper, int offsetBits, int symbol) {
      this.weight = weight;
      this.lower = lower;
      this.upper = upper;
      this.offsetBits = offsetBits;
      this.symbol = symbol;
    }
  }

  /** {@code TrainedBins} */
  static final class Trained {
    final List<BinInfo> infos;
    final int ansSizeLog;
    final int[] counts;

    Trained(List<BinInfo> infos, int ansSizeLog, int[] counts) {
      this.infos = infos;
      this.ansSizeLog = ansSizeLog;
      this.counts = counts;
    }
  }

  /** {@code train_infos}; {@code latents} is scratch (it gets reordered). */
  static Trained trainInfos(Latent lt, long[] latents, int unoptimizedBinsLog) {
    if (latents.length == 0) {
      return new Trained(new ArrayList<>(), 0, new int[0]);
    }
    int nLatents = latents.length;
    List<Histogram.Bin> unoptimized = Histogram.histogram(latents, lt, unoptimizedBinsLog);
    int nLogCeil = nLatents <= 1 ? 0 : (31 - Integer.numberOfLeadingZeros(nLatents - 1)) + 1;
    int estimatedAnsSizeLog = Math.min(Math.min(unoptimizedBinsLog + 2, 12), nLogCeil);
    List<BinInfo> optimized = optimizeBins(lt, unoptimized, estimatedAnsSizeLog);
    int[] counts = new int[optimized.size()];
    for (int k = 0; k < counts.length; k++) {
      counts[k] = optimized.get(k).weight;
    }
    int[] q = Ans.quantizeWeights(counts.clone(), nLatents, estimatedAnsSizeLog);
    for (int k = 0; k < counts.length; k++) {
      optimized.get(k).weight = q[k];
    }
    return new Trained(optimized, q[q.length - 1], counts);
  }
}
