package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.Arrays;
import java.util.List;
import java.util.SplittableRandom;
import org.junit.jupiter.api.Test;

/** The partial-quicksort histogram: the reference's vectors, and its invariants on random data. */
class HistogramTest {
  static String bins(List<Histogram.Bin> bins) {
    StringBuilder sb = new StringBuilder();
    for (Histogram.Bin b : bins) {
      sb.append('(').append(b.count).append(',').append(Long.toUnsignedString(b.lower)).append(',')
          .append(Long.toUnsignedString(b.upper)).append(')');
    }
    return sb.toString();
  }

  static List<Histogram.Bin> run(long[] v, int log) {
    return Histogram.histogram(v.clone(), Latent.U32, log);
  }

  @Test
  void referenceVectors() {
    assertEquals("", bins(run(new long[0], 2)));
    assertEquals("(1,8,8)", bins(run(new long[] {8}, 0)));
    for (int seed = 0; seed < 16; seed++) {
      long[] v = new long[100];
      for (int i = 0; i < 100; i++) {
        v[i] = i;
      }
      shuffle(v, seed);
      assertEquals("(25,0,24)(25,25,49)(25,50,74)(25,75,99)", bins(run(v, 2)));
      long[] z = new long[100];
      z[0] = 1;
      shuffle(z, seed);
      assertEquals("(99,0,0)(1,1,1)", bins(run(z, 2)));
    }
  }

  static void shuffle(long[] v, long seed) {
    SplittableRandom r = new SplittableRandom(seed);
    for (int i = v.length - 1; i > 0; i--) {
      int j = r.nextInt(i + 1);
      long t = v[i];
      v[i] = v[j];
      v[j] = t;
    }
  }

  @Test
  void binsAreExactSortedAndTight() {
    SplittableRandom r = new SplittableRandom(1);
    for (int trial = 0; trial < 3000; trial++) {
      int n = r.nextInt(2000);
      int log = r.nextInt(9);
      long[] v = new long[n];
      int spread = 1 + r.nextInt(r.nextBoolean() ? 5 : 1 << 20);
      for (int i = 0; i < n; i++) {
        v[i] = r.nextInt(spread);
        if (r.nextInt(50) == 0) {
          v[i] = 0xffffffffL - r.nextInt(3);
        }
      }
      if (r.nextInt(4) == 0) {
        Arrays.sort(v);
      }
      List<Histogram.Bin> h = run(v, log);
      long[] sorted = v.clone();
      Arrays.sort(sorted);
      int at = 0;
      long prevUpper = -1;
      assertTrue(h.size() <= 1 << log);
      for (Histogram.Bin b : h) {
        assertTrue(b.count > 0);
        assertEquals(sorted[at], b.lower, "tight lower");
        assertEquals(sorted[at + (int) b.count - 1], b.upper, "tight upper");
        assertTrue(Long.compareUnsigned(prevUpper, b.lower) < 0 || prevUpper == -1, "disjoint");
        prevUpper = b.upper;
        at += (int) b.count;
      }
      assertEquals(n, at, "every latent in a bin");
    }
  }

  @Test
  void sortUtilsMatchAFullSort() {
    SplittableRandom r = new SplittableRandom(2);
    for (int trial = 0; trial < 500; trial++) {
      long[] v = new long[r.nextInt(300)];
      for (int i = 0; i < v.length; i++) {
        v[i] = r.nextLong();
      }
      long[] h = v.clone();
      SortUtils.heapsort(h, 0, h.length);
      long[] want = v.clone();
      for (int i = 0; i < want.length; i++) {
        want[i] ^= Long.MIN_VALUE;
      }
      Arrays.sort(want);
      for (int i = 0; i < want.length; i++) {
        want[i] ^= Long.MIN_VALUE;
      }
      assertEquals(Arrays.toString(want), Arrays.toString(h));
      if (v.length > 0) {
        long pivot = SortUtils.choosePivot(v, 0, v.length);
        long[] p = v.clone();
        long res = SortUtils.partition(p, 0, p.length, pivot);
        int left = (int) res;
        for (int i = 0; i < p.length; i++) {
          assertEquals(i < left, Latent.lt(p[i], pivot));
        }
        long[] b = v.clone();
        SortUtils.breakPatterns(b, 0, b.length);
        long[] s1 = b.clone();
        Arrays.sort(s1);
        long[] s2 = v.clone();
        Arrays.sort(s2);
        assertEquals(Arrays.toString(s2), Arrays.toString(s1), "a permutation");
      }
    }
  }
}
