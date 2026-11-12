package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Random;
import org.junit.jupiter.api.Test;

/**
 * The reference's other HashMap-order choices (Modes.Margins): the order of the bit-savings
 * sums of IntMult, FloatMult and FloatQuant, and IntMult's score ties. For every input here,
 * every decision taken on them is proved independent of the summation order (the exact sum and
 * Higham's bound for any order lie on one side of the threshold or of the other bid) and no
 * scores tie; so the reference's output on these inputs is one, whatever its HashMap seeds, and
 * the parity suites compare the Java output with it.
 */
class OrderIndependenceTest {
  @Test
  void noDecisionDependsOnTheReferencesHashMapOrder() {
    int scale = Integer.getInteger("pcodec.parity.scale", 1);
    List<String> ambiguous = new ArrayList<>();
    int cases = 0;
    for (Corpus.Case c : Corpus.cases(NumberType.values(), new int[] {10, 40, 300, 2000, 20000}, 2 * scale)) {
      ChunkConfig cfg = new ChunkConfig().withEnable8Bit(true);
      Modes.Margins.start();
      int n;
      try {
        Modes.chooseWinningBid(Modes.chooseModeBids(c.type, c.nums, cfg));
      } finally {
        n = Modes.Margins.stop();
      }
      cases++;
      if (n > 0) {
        ambiguous.add(c + ": " + Modes.Margins.REASONS.get());
      }
    }
    assertTrue(ambiguous.isEmpty(), ambiguous.size() + " of " + cases + " order-dependent: " + ambiguous);
    System.out.println(cases + " cases: every mode decision independent of the summation order");
  }

  @Test
  void theCheckDetectsAnOrderDependentSum() {
    // the margin check itself: a sum whose terms cancel so that its exact value sits on the
    // threshold is flagged
    Modes.Margins.start();
    double[] terms = {1e16, 0.5 * 4, -1e16};
    Modes.Margins.record(terms, 3, 0.5, 4);
    Modes.Margins.threshold(Modes.Margins.lo(), Modes.Margins.hi(), 0.5, true);
    assertTrue(Modes.Margins.stop() == 1);
    Modes.Margins.start();
    Modes.Margins.record(new double[] {4.0, 4.0}, 2, 2.0, 4);
    Modes.Margins.threshold(Modes.Margins.lo(), Modes.Margins.hi(), 0.5, true);
    assertTrue(Modes.Margins.stop() == 0);
  }

  // ------------------------------------------------------------ the checker itself

  @Test
  void countsOnlyBetweenStartAndStop() {
    assertEquals(0, Modes.Margins.stop(), "stop without start");
    assertFalse(Modes.Margins.on());
    Modes.Margins.flag("ignored while off");
    Modes.Margins.start();
    assertTrue(Modes.Margins.on());
    assertTrue(Modes.Margins.REASONS.get().isEmpty(), "start clears the reasons");
    Modes.Margins.flag("a");
    Modes.Margins.flag("b");
    assertEquals(Arrays.asList("a", "b"), Modes.Margins.REASONS.get());
    assertEquals(2, Modes.Margins.stop());
    assertFalse(Modes.Margins.on());
    Modes.Margins.flag("ignored while off");
    assertEquals(Arrays.asList("a", "b"), Modes.Margins.REASONS.get());
    assertEquals(0, Modes.Margins.stop());
    Modes.Margins.start();
    assertTrue(Modes.Margins.REASONS.get().isEmpty(), "start clears the reasons");
    assertEquals(0, Modes.Margins.stop());
  }

  /** a sum whose order matters: a large term and many small ones that do not divide evenly */
  static double[] inexactTerms(Random r, int n) {
    double[] t = new double[n];
    for (int i = 0; i < n; i++) {
      t[i] = (r.nextDouble() - 0.5) * Math.scalb(1.0, r.nextInt(40) - 20);
    }
    t[0] = 1e6;
    return t;
  }

  static double foldedSum(double[] t) {
    double s = -0.0;
    for (double x : t) {
      s += x;
    }
    return s;
  }

  @Test
  void theIntervalHoldsTheQuotientOfEveryOrderAndIsTight() {
    Random r = new Random(7);
    for (int round = 0; round < 200; round++) {
      int n = 2 + r.nextInt(60);
      int len = 1 + r.nextInt(1000);
      double[] t = inexactTerms(r, n);
      double value = foldedSum(t) / len;
      Modes.Margins.start();
      Modes.Margins.record(t, n, value, len);
      Modes.Margins.stop();
      double lo = Modes.Margins.lo();
      double hi = Modes.Margins.hi();
      assertTrue(lo < hi, "an inexact sum gives an interval");
      List<Double> l = new ArrayList<>();
      for (double x : t) {
        l.add(x);
      }
      for (int perm = 0; perm < 50; perm++) {
        java.util.Collections.shuffle(l, r);
        double[] p = new double[n];
        for (int i = 0; i < n; i++) {
          p[i] = l.get(i);
        }
        double q = foldedSum(p) / len;
        assertTrue(lo <= q && q <= hi, q + " outside [" + lo + ", " + hi + "]");
      }
      // no wider than Higham's bound on both sides, plus the rounding of the quotient
      double abs = 0;
      for (double x : t) {
        abs += Math.abs(x);
      }
      double ku = (n - 1) * 0x1p-53;
      double width = 2 * abs * (ku / (1 - ku)) / len;
      assertTrue(hi - lo <= width * (1 + 1e-6) + 8 * Math.ulp(Math.max(Math.abs(lo), Math.abs(hi))),
          "[" + lo + ", " + hi + "] wider than " + width);
      assertTrue(hi - lo >= width * (1 - 1e-6), "[" + lo + ", " + hi + "] narrower than " + width);
    }
  }

  @Test
  void anExactSumGivesThePointAndOnlyThen() {
    // integers, and multiples of 2^-30 below 2^23: every partial sum of every order is exact
    double[][] exact = {{3, -7, 12, 1e6}, {0x1p-30, 3 * 0x1p-30, -0x1p-30, 0x1p22}, {0, 0}, {}};
    for (double[] t : exact) {
      Modes.Margins.start();
      Modes.Margins.record(t, t.length, 1.25, 4);
      Modes.Margins.stop();
      assertEquals(1.25, Modes.Margins.lo(), Arrays.toString(t));
      assertEquals(1.25, Modes.Margins.hi(), Arrays.toString(t));
    }
    // a multiple of 2^-30 at 2^23 and above, or of 2^-60 next to 1: some order rounds
    double[][] inexact = {{0x1p-30, 0x1p23, -0x1p23}, {1.0, 0x1p-60, -1.0}, {0x1p-52, 3.0, 0x1p-52}};
    for (double[] t : inexact) {
      Modes.Margins.start();
      Modes.Margins.record(t, t.length, 0.0, 1);
      Modes.Margins.stop();
      assertTrue(Modes.Margins.lo() < Modes.Margins.hi(), Arrays.toString(t));
    }
    // only the first nTerms count
    Modes.Margins.start();
    Modes.Margins.record(new double[] {3, 0x1p-60, 1e300}, 1, 3.0, 1);
    Modes.Margins.stop();
    assertEquals(3.0, Modes.Margins.lo());
    assertEquals(3.0, Modes.Margins.hi());
  }

  @Test
  void whileOffTheIntervalIsTheValue() {
    double[] t = inexactTerms(new Random(3), 30);
    Modes.Margins.record(t, t.length, 0.75, 9);
    assertEquals(0.75, Modes.Margins.lo());
    assertEquals(0.75, Modes.Margins.hi());
  }

  @Test
  void overlapIsClosedAndIgnoresEqualPoints() {
    assertTrue(Modes.Margins.overlap(0, 1, 1, 2));
    assertTrue(Modes.Margins.overlap(1, 2, 0, 1));
    assertTrue(Modes.Margins.overlap(0, 3, 1, 2));
    assertTrue(Modes.Margins.overlap(1, 2, 0, 3));
    assertFalse(Modes.Margins.overlap(0, 1, Math.nextUp(1.0), 2));
    assertFalse(Modes.Margins.overlap(Math.nextUp(1.0), 2, 0, 1));
    assertFalse(Modes.Margins.overlap(1, 1, 1, 1), "the same point");
    assertTrue(Modes.Margins.overlap(1, 1, 0, 2));
    assertTrue(Modes.Margins.overlap(0, 2, 1, 1));
    assertTrue(Modes.Margins.overlap(1, 1, 1, 2));
    assertTrue(Modes.Margins.overlap(1, 2, 2, 2));
    assertFalse(Modes.Margins.overlap(1, 1, 2, 2));
  }

  static int thresholdFlags(double lo, double hi, double threshold, boolean strict) {
    Modes.Margins.start();
    Modes.Margins.threshold(lo, hi, threshold, strict);
    return Modes.Margins.stop();
  }

  @Test
  void thresholdFlagsExactlyTheIntervalsOnBothSidesOfIt() {
    double t = 0.5;
    double up = Math.nextUp(t);
    double down = Math.nextDown(t);
    // value > t
    assertEquals(1, thresholdFlags(t, up, t, true));
    assertEquals(1, thresholdFlags(down, up, t, true));
    assertEquals(0, thresholdFlags(down, t, t, true));
    assertEquals(0, thresholdFlags(t, t, t, true));
    assertEquals(0, thresholdFlags(up, 1, t, true));
    assertEquals(0, thresholdFlags(0, down, t, true));
    // value >= t
    assertEquals(1, thresholdFlags(down, t, t, false));
    assertEquals(1, thresholdFlags(down, up, t, false));
    assertEquals(0, thresholdFlags(t, up, t, false));
    assertEquals(0, thresholdFlags(t, t, t, false));
    assertEquals(0, thresholdFlags(up, 1, t, false));
    assertEquals(0, thresholdFlags(0, down, t, false));
  }

  static Modes.Bid bid(Mode m, double saved, double lo, double hi) {
    Modes.Bid b = new Modes.Bid(m, saved, xs -> null);
    b.lo = lo;
    b.hi = hi;
    return b;
  }

  @Test
  void theWinningBidIsFlaggedWhenAnotherCouldOvertakeIt() {
    Modes.Bid classic = bid(Mode.CLASSIC, 0.0, 0.0, 0.0);
    Modes.Bid a = bid(Mode.intMult(3), 1.0, 0.9, 1.1);
    Modes.Bid b = bid(Mode.intMult(5), 1.2, 1.05, 1.3);
    Modes.Bid c = bid(Mode.intMult(7), 2.0, 1.9, 2.1);
    Modes.Bid sameAsB = bid(Mode.intMult(5), 1.2, 1.05, 1.3);
    Object[][] cases = {
      {Arrays.asList(classic, a), a, 0},
      {Arrays.asList(classic, a, b), b, 1},
      {Arrays.asList(classic, b, a), b, 1},
      {Arrays.asList(classic, a, c), c, 0},
      {Arrays.asList(classic, sameAsB, b), b, 0},
      {Arrays.asList(classic, a, b, c), c, 0},
    };
    for (Object[] k : cases) {
      @SuppressWarnings("unchecked")
      List<Modes.Bid> bids = (List<Modes.Bid>) k[0];
      Modes.Margins.start();
      Modes.Bid won = Modes.chooseWinningBid(bids);
      int n = Modes.Margins.stop();
      assertTrue(won == k[1]);
      assertEquals(k[2], n, bids.toString());
    }
    // while off nothing is counted
    assertTrue(Modes.chooseWinningBid(Arrays.asList(classic, a, b)) == b);
    assertEquals(0, Modes.Margins.stop());
  }

  /** triples {x, x + g, x + 2g}: their triple GCD is g */
  static void triples(long[] s, int from, int count, long g, long x0) {
    for (int i = 0; i < count; i++) {
      long x = x0 + 11 * i;
      s[from + 3 * i] = x;
      s[from + 3 * i + 1] = x + g;
      s[from + 3 * i + 2] = x + 2 * g;
    }
  }

  @Test
  void intMultKeepsEveryCandidateWithTheBestScore() {
    // 2^61 + 1 and 2^61 + 3 are the same f64, so their scores tie; 2^40 + 1 scores lower
    long a = (1L << 61) + 1;
    long b = (1L << 61) + 3;
    long c = (1L << 40) + 1;
    long[] s = new long[3 * 25];
    triples(s, 0, 5, c, 100);
    triples(s, 15, 10, a, 200);
    triples(s, 45, 10, b, 300);
    List<Long> before = Arrays.asList(99L);
    Modes.IntMult.TIED.get().clear();
    Modes.IntMult.TIED.get().addAll(before);
    long[] off = Modes.IntMult.chooseCandidateBase(s);
    assertEquals(a, off[0], "equal scores go to the first");
    assertEquals(before, Modes.IntMult.TIED.get(), "untouched while off");
    Modes.Margins.start();
    long[] on = Modes.IntMult.chooseCandidateBase(s);
    Modes.Margins.stop();
    assertEquals(a, on[0]);
    assertEquals(off[1], on[1]);
    assertEquals(Arrays.asList(a, b), Modes.IntMult.TIED.get());
    // a later, better candidate replaces the tied ones
    long[] s2 = Arrays.copyOf(s, 3 * 37);
    triples(s2, 75, 12, (1L << 62) + 1, 400);
    Modes.Margins.start();
    long[] best = Modes.IntMult.chooseCandidateBase(s2);
    Modes.Margins.stop();
    assertEquals((1L << 62) + 1, best[0]);
    assertEquals(Arrays.asList((1L << 62) + 1), Modes.IntMult.TIED.get());
    Modes.IntMult.TIED.get().clear();
  }

  /** nums of length n whose mode sample (chooseModeSample's fixed permutation) is `sample` */
  static long[] withSample(long[] sample, int n) {
    long[] idx = new long[n];
    for (int i = 0; i < n; i++) {
      idx[i] = i;
    }
    long[] order = Modes.chooseModeSample(idx, (x, out) -> {
      out[0] = x;
      return true;
    });
    assertEquals(sample.length, order.length);
    long[] nums = new long[n];
    for (int i = 0; i < order.length; i++) {
      nums[(int) order[i]] = sample[i];
    }
    return nums;
  }

  /** triples {x, x + g, x + 2g} with x = 3ig + 5, i from i0: distinct quotients by g */
  static void spreadTriples(long[] s, int from, int count, long g, int i0) {
    for (int i = 0; i < count; i++) {
      long x = 3 * (i0 + i) * g + 5;
      s[from + 3 * i] = x;
      s[from + 3 * i + 1] = x + g;
      s[from + 3 * i + 2] = x + 2 * g;
    }
  }

  @Test
  void intMultFlagsAScoreTieBetweenBasesThatBothWin() {
    // 2^54 - 1 and 2^54 + 1 are the same f64, so their scores tie
    long a = (1L << 54) - 1;
    long b = (1L << 54) + 1;
    long[] s = new long[60];
    spreadTriples(s, 0, 10, a, 0);
    spreadTriples(s, 30, 10, b, 10);
    long[] nums = withSample(s, 10 + 40 * 50);
    Modes.Margins.start();
    long[] base = Modes.IntMult.chooseBase(NumberType.U64, nums);
    Modes.Margins.stop();
    assertEquals(Arrays.asList(a, b), Modes.IntMult.TIED.get());
    assertEquals(a, base[0]);
    assertEquals(Arrays.asList("int mult score tie with different outcomes"), Modes.Margins.REASONS.get());
    // without the tie (b's triples are a's): one candidate, nothing to flag
    spreadTriples(s, 30, 10, a, 10);
    nums = withSample(s, 10 + 40 * 50);
    Modes.Margins.start();
    base = Modes.IntMult.chooseBase(NumberType.U64, nums);
    assertEquals(0, Modes.Margins.stop(), Modes.Margins.REASONS.get().toString());
    assertEquals(a, base[0]);
    assertEquals(Arrays.asList(a), Modes.IntMult.TIED.get());
    Modes.IntMult.TIED.get().clear();
  }
}
