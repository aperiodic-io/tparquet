package io.aperiodic.pcodec;

/** {@code pco::sort_utils}, on a range {@code [from, to)} of unsigned latents. */
final class SortUtils {
  private SortUtils() {}

  /** {@code choose_pivot} */
  static long choosePivot(long[] v, int from, int to) {
    int len = to - from;
    int[] abc = {len / 4, len / 2, (len * 3) / 4};
    if (len >= 8) {
      if (len >= 50) {
        for (int k = 0; k < 3; k++) {
          int[] t = {abc[k] - 1, abc[k], abc[k] + 1};
          sort3(v, from, t, 0, 1, 2);
          abc[k] = t[1];
        }
      }
      sort3(v, from, abc, 0, 1, 2);
    }
    return v[from + abc[1]];
  }

  private static void sort2(long[] v, int from, int[] idx, int a, int b) {
    if (Latent.lt(v[from + idx[b]], v[from + idx[a]])) {
      int t = idx[a];
      idx[a] = idx[b];
      idx[b] = t;
    }
  }

  private static void sort3(long[] v, int from, int[] idx, int a, int b, int c) {
    sort2(v, from, idx, a, b);
    sort2(v, from, idx, b, c);
    sort2(v, from, idx, a, b);
  }

  /** {@code break_patterns} (64-bit usize) */
  static void breakPatterns(long[] v, int from, int to) {
    int len = to - from;
    if (len >= 8) {
      long seed = len;
      long modulus = Long.highestOneBit(len) == len ? len : Long.highestOneBit(len) << 1;
      int pos = len / 4 * 2;
      for (int i = 0; i < 3; i++) {
        long r = seed;
        r ^= r << 13;
        r ^= r >>> 7;
        r ^= r << 17;
        seed = r;
        long other = r & (modulus - 1);
        if (Long.compareUnsigned(other, len) >= 0) {
          other -= len;
        }
        int a = from + pos - 1 + i;
        int b = from + (int) other;
        long t = v[a];
        v[a] = v[b];
        v[b] = t;
      }
    }
  }

  /**
   * {@code partition} (Lomuto): the count of elements below the pivot, and in bit 32 whether the
   * pivot was bad.
   */
  static long partition(long[] v, int from, int to, long pivot) {
    int leftIdx = 0;
    int len = to - from;
    for (int pos = 0; pos < len; pos++) {
      long value = v[from + pos];
      boolean isLt = Latent.lt(value, pivot);
      v[from + pos] = v[from + leftIdx];
      v[from + leftIdx] = value;
      leftIdx += isLt ? 1 : 0;
    }
    boolean bad = 1 + Math.min(leftIdx, len - leftIdx) < len / 8;
    return (bad ? 1L << 32 : 0) | leftIdx;
  }

  /** {@code heapsort} */
  static void heapsort(long[] v, int from, int to) {
    int len = to - from;
    for (int i = len / 2 - 1; i >= 0; i--) {
      siftDown(v, from, len, i);
    }
    for (int i = len - 1; i >= 1; i--) {
      long t = v[from];
      v[from] = v[from + i];
      v[from + i] = t;
      siftDown(v, from, i, 0);
    }
  }

  private static void siftDown(long[] v, int from, int len, int node) {
    while (true) {
      int child = 2 * node + 1;
      if (child >= len) {
        break;
      }
      if (child + 1 < len) {
        child += Latent.lt(v[from + child], v[from + child + 1]) ? 1 : 0;
      }
      if (!Latent.lt(v[from + node], v[from + child])) {
        break;
      }
      long t = v[from + node];
      v[from + node] = v[from + child];
      v[from + child] = t;
      node = child;
    }
  }
}
