package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

/**
 * Bounded exhaustive checks of the invariants the heuristics must keep whatever they decide
 * (their decisions themselves are compared with the reference by the parity suites): every
 * input of the bounded domain is tried. VERIFICATION.md, "Coverage".
 */
class InvariantsTest {
  /** the latents tried: the extremes of u32 and a few small values */
  static final long[] VALUES = {0, 1, 2, 7, 0xfffffffeL, 0xffffffffL};

  /** calls f on every array of length len over VALUES */
  interface Each {
    void accept(long[] v);
  }

  static void everyArray(int len, Each f) {
    int[] digits = new int[len];
    long[] v = new long[len];
    while (true) {
      for (int i = 0; i < len; i++) {
        v[i] = VALUES[digits[i]];
      }
      f.accept(v.clone());
      int i = 0;
      while (i < len && ++digits[i] == VALUES.length) {
        digits[i++] = 0;
      }
      if (i == len) {
        return;
      }
    }
  }

  static long[] sorted(long[] v) {
    long[] s = v.clone();
    for (int i = 0; i < s.length; i++) {
      s[i] ^= Long.MIN_VALUE;
    }
    Arrays.sort(s);
    for (int i = 0; i < s.length; i++) {
      s[i] ^= Long.MIN_VALUE;
    }
    return s;
  }

  @Test
  void partitionPermutesAndSplitsAtThePivotForEveryArrayUpToSix() {
    for (int l = 0; l <= 6; l++) {
      final int len = l;
      everyArray(len, v -> {
        for (long pivot : VALUES) {
          long[] a = v.clone();
          long r = SortUtils.partition(a, 0, a.length, pivot);
          int k = (int) r;
          assertArrayEquals(sorted(v), sorted(a), "a permutation");
          for (int i = 0; i < a.length; i++) {
            assertEquals(i < k, Latent.lt(a[i], pivot), Arrays.toString(v) + " pivot " + pivot);
          }
          boolean bad = 1 + Math.min(k, a.length - k) < a.length / 8;
          assertEquals(bad, (r >>> 32) != 0);
        }
        long[] h = v.clone();
        SortUtils.heapsort(h, 0, h.length);
        assertArrayEquals(sorted(v), h, "heapsort sorts");
        if (len > 0) {
          long p = SortUtils.choosePivot(v.clone(), 0, len);
          assertTrue(Arrays.stream(v).anyMatch(x -> x == p), "the pivot is an element");
        }
      });
    }
  }

  @Test
  void histogramBinsCoverEveryValueOnceInOrderForEveryArrayUpToSix() {
    for (int l = 1; l <= 6; l++) {
      final int len = l;
      for (int nBinsLog = 0; nBinsLog <= 3; nBinsLog++) {
        final int log = nBinsLog;
        everyArray(len, v -> {
          List<Histogram.Bin> bins = Histogram.histogram(v.clone(), Latent.U32, log);
          assertTrue(bins.size() >= 1 && bins.size() <= (1 << log), "bin count");
          long total = 0;
          for (int b = 0; b < bins.size(); b++) {
            Histogram.Bin bin = bins.get(b);
            assertTrue(!Latent.lt(bin.upper, bin.lower), "lower <= upper");
            if (b > 0) {
              assertTrue(Latent.lt(bins.get(b - 1).upper, bin.lower), "ordered, disjoint");
            }
            total += bin.count;
            long in = Arrays.stream(v).filter(x -> !Latent.lt(x, bin.lower) && !Latent.lt(bin.upper, x)).count();
            assertEquals(bin.count, in, "a bin counts the values in its range");
          }
          assertEquals(len, total, "every value in a bin");
        });
      }
    }
  }

  @Test
  void trainedBinsCoverEveryLatentAndWeighToTheTableForEveryArrayUpToFive() {
    for (int len = 1; len <= 5; len++) {
      for (int nBinsLog = 0; nBinsLog <= 2; nBinsLog++) {
        final int log = nBinsLog;
        everyArray(len, v -> {
          BinOptimization.Trained t = BinOptimization.trainInfos(Latent.U32, v.clone(), log);
          long weights = 0;
          for (int b = 0; b < t.infos.size(); b++) {
            BinOptimization.BinInfo info = t.infos.get(b);
            assertTrue(info.weight >= 1, "a positive weight");
            weights += info.weight;
            if (b > 0) {
              assertTrue(Latent.lt(t.infos.get(b - 1).lower, info.lower), "ascending");
            }
          }
          assertEquals(1L << t.ansSizeLog, weights, "the weights fill the ANS table");
          for (long x : v) {
            boolean covered = false;
            for (BinOptimization.BinInfo info : t.infos) {
              long off = x - info.lower;
              covered |= !Latent.lt(x, info.lower) && (info.offsetBits == 64 || Long.compareUnsigned(off, 1L << info.offsetBits) < 0);
            }
            assertTrue(covered, x + " lies in a bin's offset range");
          }
        });
      }
    }
  }

  @Test
  void quantizedWeightsArePositiveAndFillTheTableForEveryCountVector() {
    for (int nSymbols = 2; nSymbols <= 4; nSymbols++) {
      int[] counts = new int[nSymbols];
      int combos = (int) Math.pow(7, nSymbols);
      for (int c = 0; c < combos; c++) {
        int x = c;
        long total = 0;
        boolean any = false;
        for (int i = 0; i < nSymbols; i++) {
          counts[i] = 1 + x % 7;
          x /= 7;
          total += counts[i];
          any = true;
        }
        for (int maxLog = 1; maxLog <= 5 && any; maxLog++) {
          int[] q = Ans.quantizeWeights(counts.clone(), total, maxLog);
          int sizeLog = q[q.length - 1];
          // at least enough states for every symbol: the limit is raised to ceil(log2 n)
          int limit = Math.max(maxLog, 64 - Long.numberOfLeadingZeros(nSymbols - 1L));
          assertTrue(sizeLog >= 0 && sizeLog <= limit, "size log within the limit");
          long sum = 0;
          for (int i = 0; i < nSymbols; i++) {
            assertTrue(q[i] >= 1, Arrays.toString(counts) + " " + maxLog);
            sum += q[i];
          }
          assertEquals(1L << sizeLog, sum, Arrays.toString(counts) + " " + maxLog);
        }
      }
    }
  }

  @Test
  void equalPagesSumToTheCountWithinTheLimitAndDifferByAtMostOne() {
    for (long n = 0; n <= 400; n++) {
      for (long max = 1; max <= 60; max++) {
        long[] pages = PagingSpec.equalPagesUpTo(max).nPerPage(n);
        long sum = 0;
        long lo = Long.MAX_VALUE;
        long hi = 0;
        for (long p : pages) {
          assertTrue(p >= 1 && p <= max, n + " " + max);
          sum += p;
          lo = Math.min(lo, p);
          hi = Math.max(hi, p);
        }
        assertEquals(n, sum);
        assertTrue(pages.length == 0 || hi - lo <= 1, "equal pages");
      }
    }
  }

  @Test
  void longIndexMapIsALinkedHashMapOnEverySequenceUpToSeven() {
    long[] keys = {0, 1, Long.MIN_VALUE, -1};
    for (int len = 0; len <= 7; len++) {
      int combos = 1 << (2 * len);
      for (int c = 0; c < combos; c++) {
        LongIndexMap m = new LongIndexMap(c % 3);
        Map<Long, Integer> want = new LinkedHashMap<>();
        List<Long> seq = new ArrayList<>();
        for (int i = 0; i < len; i++) {
          long k = keys[(c >>> (2 * i)) & 3];
          seq.add(k);
          want.putIfAbsent(k, want.size());
          assertEquals((int) want.get(k), m.add(k), seq.toString());
        }
        assertEquals(want.size(), m.size());
        for (long k : keys) {
          assertEquals(want.getOrDefault(k, -1), (Integer) m.get(k));
        }
      }
    }
  }
}
