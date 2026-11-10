package io.aperiodic.pcodec;

import java.util.ArrayList;
import java.util.List;

/**
 * {@code pco::histograms}: up to {@code 2^nBinsLog} bins of tight, exclusive ranges with exact
 * counts, by a partial quicksort.
 */
final class Histogram {
  static final class Bin {
    final long count;
    final long lower;
    final long upper;

    Bin(long count, long lower, long upper) {
      this.count = count;
      this.lower = lower;
      this.upper = upper;
    }
  }

  /** a bound: tight (the exact extreme) or loose (only a bound) */
  private static final class Bound {
    final boolean tight;
    final long x;

    Bound(boolean tight, long x) {
      this.tight = tight;
      this.x = x;
    }
  }

  private final Latent lt;
  private final long n;
  private final long nBins;
  private final int nBinsLog;
  private long nApplied;
  private long nextAvailBinIdx;
  private boolean hasIncomplete;
  private long incCount;
  private long incLower;
  private long incUpper;
  private final List<Bin> dst = new ArrayList<>();

  private Histogram(Latent lt, long n, int nBinsLog) {
    this.lt = lt;
    this.n = n;
    this.nBins = 1L << nBinsLog;
    this.nBinsLog = nBinsLog;
  }

  static List<Bin> histogram(long[] latents, Latent lt, int nBinsLog) {
    return histogram(latents, lt, nBinsLog, 1 + (63 - Long.numberOfLeadingZeros(latents.length + 1L)));
  }

  /** with a given bad pivot limit (the reference's is 1 + log2(n + 1); tests force smaller) */
  static List<Bin> histogram(long[] latents, Latent lt, int nBinsLog, int badPivotLimit) {
    Histogram h = new Histogram(lt, latents.length, nBinsLog);
    h.recurse(latents, 0, latents.length, new Bound(false, 0), new Bound(false, lt.max()), badPivotLimit);
    return h.dst;
  }

  /**
   * {@code apply_sorted} over sorted runs, as the reference's tests drive it: the completed bins,
   * then the incomplete one (count 0 if none).
   */
  static List<Bin> runSorted(long[][] runs, long n, int nBinsLog) {
    Histogram h = new Histogram(Latent.U32, n, nBinsLog);
    for (long[] run : runs) {
      h.applySorted(run, 0, run.length);
    }
    List<Bin> res = new ArrayList<>(h.dst);
    res.add(h.hasIncomplete ? new Bin(h.incCount, h.incLower, h.incUpper) : new Bin(0, 0, 0));
    return res;
  }

  long binIdxForTest(long cCount) {
    return binIdx(cCount);
  }

  long cCountForTest(long binIdx) {
    return cCount(binIdx);
  }

  static Histogram builderForTest(long n, int nBinsLog) {
    return new Histogram(Latent.U32, n, nBinsLog);
  }

  private static long sliceMin(long[] v, int from, int to) {
    long m = -1L;
    for (int i = from; i < to; i++) {
      m = Latent.min(m, v[i]);
    }
    return m;
  }

  private static long sliceMax(long[] v, int from, int to) {
    long m = 0;
    for (int i = from; i < to; i++) {
      m = Latent.max(m, v[i]);
    }
    return m;
  }

  private void applyIncomplete(long[] v, int from, int to, Bound lower, Bound upper) {
    if (from == to) {
      return;
    }
    if (hasIncomplete) {
      incUpper = upper.tight ? upper.x : sliceMax(v, from, to);
      incCount += to - from;
    } else {
      long lb = lower.tight ? lower.x : sliceMin(v, from, to);
      long ub = upper.tight ? upper.x : sliceMax(v, from, to);
      hasIncomplete = true;
      incCount = to - from;
      incLower = lb;
      incUpper = ub;
    }
    nApplied += to - from;
  }

  private boolean completeBin(long binIdx) {
    if (hasIncomplete) {
      nextAvailBinIdx = binIdx + 1;
      dst.add(new Bin(incCount, incLower, incUpper));
      hasIncomplete = false;
      return true;
    }
    return false;
  }

  private long binIdx(long cCount) {
    return Long.divideUnsigned(cCount << nBinsLog, n);
  }

  private long cCount(long binIdx) {
    return ((binIdx + 1) * n + nBins - 1) >>> nBinsLog;
  }

  private void applyConstantRun(long[] v, int from, int to) {
    long start = nApplied;
    int len = to - from;
    long mid = start + len / 2;
    long end = start + len;
    long binIdx = binIdx(mid);
    if (binIdx > nextAvailBinIdx) {
      long spare = binIdx - 1;
      if (!completeBin(spare)) {
        binIdx = spare;
      }
    }
    Bound c = new Bound(true, v[from]);
    applyIncomplete(v, from, to, c, c);
    if (end >= cCount(binIdx)) {
      completeBin(binIdx);
    }
  }

  private void applySorted(long[] v, int from, int to) {
    while (from < to) {
      long targetBinIdx = binIdx(nApplied);
      long targetCCount = cCount(targetBinIdx);
      long targetI = targetCCount - nApplied;
      int len = to - from;
      if (targetI >= len) {
        applyIncomplete(v, from, to, new Bound(true, v[from]), new Bound(true, v[to - 1]));
        if (targetI == len) {
          completeBin(targetBinIdx);
        }
        break;
      }
      int l = (int) targetI - 1;
      int r = (int) targetI;
      long targetX = v[from + l];
      while (l > 0 && v[from + l - 1] == targetX) {
        l--;
      }
      while (r < len && v[from + r] == targetX) {
        r++;
      }
      if (l > 0) {
        applyIncomplete(v, from, from + l, new Bound(true, v[from]), new Bound(true, v[from + l - 1]));
      }
      applyConstantRun(v, from + l, from + r);
      from = from + r;
    }
  }

  private void recurse(long[] v, int from, int to, Bound lb, Bound ub, int badPivotLimit) {
    if (from == to) {
      return;
    }
    long targetBinIdx = binIdx(nApplied);
    long targetCCount = cCount(targetBinIdx);
    long end = nApplied + (to - from);
    if (end <= targetCCount) {
      applyIncomplete(v, from, to, lb, ub);
      if (end == targetCCount) {
        completeBin(targetBinIdx);
      }
      return;
    }
    long looseLb = lb.x;
    if (looseLb == ub.x || to - from == 1) {
      applyConstantRun(v, from, to);
      return;
    }
    long tentative = SortUtils.choosePivot(v, from, to);
    long pivot;
    Bound lhsUb;
    Bound rhsLb;
    if (Latent.lt(looseLb, tentative)) {
      pivot = tentative;
      lhsUb = new Bound(false, lt.sub(tentative, 1));
      rhsLb = new Bound(true, tentative);
    } else {
      pivot = lt.add(tentative, 1);
      lhsUb = new Bound(true, tentative);
      rhsLb = new Bound(false, pivot);
    }
    long pr = SortUtils.partition(v, from, to, pivot);
    int lhsCount = (int) pr;
    boolean wasBad = (pr >>> 32) != 0;
    int mid = from + lhsCount;
    if (wasBad) {
      badPivotLimit -= 1;
      if (badPivotLimit == 0) {
        SortUtils.heapsort(v, from, mid);
        SortUtils.heapsort(v, mid, to);
        applySorted(v, from, to);
        return;
      }
      SortUtils.breakPatterns(v, from, mid);
      SortUtils.breakPatterns(v, mid, to);
    }
    recurse(v, from, mid, lb, lhsUb, badPivotLimit);
    recurse(v, mid, to, rhsLb, ub, badPivotLimit);
  }
}
