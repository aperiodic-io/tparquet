package io.aperiodic.pcodec;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Choosing the mode ({@code choose_mode_bids}) and splitting numbers into latents, as {@code
 * pco::mode} and {@code pco::sampling}. Numbers are raw bits (see {@link NumberType}).
 */
final class Modes {
  private Modes() {}

  static final double MULT_REQUIRED_BITS_SAVED_PER_NUM = 0.5;
  static final double QUANT_REQUIRED_BITS_SAVED_PER_NUM = 1.5;
  static final int MIN_SAMPLE = 10;
  private static final int SAMPLE_RATIO = 40;
  private static final double CLASSIC_MEMORIZABLE_BINS = 256.0;

  /** primary and (or null) secondary latents */
  static final class Split {
    final long[] primary;
    final long[] secondary;

    Split(long[] primary, long[] secondary) {
      this.primary = primary;
      this.secondary = secondary;
    }
  }

  interface Splitter {
    Split split(long[] nums);
  }

  static final class Bid {
    final Mode mode;
    final double bitsSavedPerNum;
    final Splitter split;
    /** the values bitsSavedPerNum could take under any summation order (Margins) */
    double lo;
    double hi;

    Bid(Mode mode, double bitsSavedPerNum, Splitter split) {
      this.mode = mode;
      this.bitsSavedPerNum = bitsSavedPerNum;
      this.split = split;
      this.lo = bitsSavedPerNum;
      this.hi = bitsSavedPerNum;
    }
  }

  /**
   * Verification hook (tests only): the reference sums bit savings in HashMap order and breaks
   * IntMult score ties in HashMap order, both seeded per process. With {@link #start}, every
   * such sum also computes the exact sum and Higham's bound on the error of any summation
   * order, and every decision taken on it (a threshold, a choice between bids, an IntMult tie)
   * that the order could change is counted. A count of 0 proves that the reference's output on
   * that input does not depend on its HashMap order.
   */
  static final class Margins {
    private Margins() {}

    private static final ThreadLocal<int[]> AMBIGUOUS = new ThreadLocal<>();
    static final ThreadLocal<List<String>> REASONS = ThreadLocal.withInitial(ArrayList::new);
    private static final ThreadLocal<double[]> LAST = ThreadLocal.withInitial(() -> new double[2]);

    static void start() {
      AMBIGUOUS.set(new int[1]);
      REASONS.get().clear();
    }

    /** the count of order-dependent decisions since start; stops counting */
    static int stop() {
      int[] c = AMBIGUOUS.get();
      AMBIGUOUS.remove();
      return c == null ? 0 : c[0];
    }

    static boolean on() {
      return AMBIGUOUS.get() != null;
    }

    static void flag(String why) {
      int[] c = AMBIGUOUS.get();
      if (c != null) {
        c[0]++;
        REASONS.get().add(why);
      }
    }

    /** the interval of the last estimate */
    static double lo() {
      return LAST.get()[0];
    }

    static double hi() {
      return LAST.get()[1];
    }

    static void record(double[] terms, int nTerms, double value, int len) {
      if (!on()) {
        LAST.get()[0] = value;
        LAST.get()[1] = value;
        return;
      }
      java.math.BigDecimal exact = java.math.BigDecimal.ZERO;
      java.math.BigDecimal abs = java.math.BigDecimal.ZERO;
      for (int i = 0; i < nTerms; i++) {
        java.math.BigDecimal t = new java.math.BigDecimal(terms[i]);
        exact = exact.add(t);
        abs = abs.add(t.abs());
      }
      // gamma_{k-1} = (k-1)u / (1 - (k-1)u), u = 2^-53, rounded up; or 0 when every term is a
      // multiple of 2^e and the absolute sum is below 2^(53 + e), as then every partial sum in
      // every order is a double, and every order gives the exact sum
      double ku = Math.max(nTerms - 1, 0) * 0x1p-53;
      double gamma = Math.nextUp(ku / (1 - ku));
      int minExp = Integer.MAX_VALUE;
      for (int i = 0; i < nTerms; i++) {
        if (terms[i] != 0) {
          minExp = Math.min(minExp, Math.getExponent(terms[i]) - 52 + Long.numberOfTrailingZeros(
              (Double.doubleToRawLongBits(terms[i]) & 0xfffffffffffffL) | 0x10000000000000L));
        }
      }
      boolean exactSums = minExp == Integer.MAX_VALUE
          || abs.compareTo(new java.math.BigDecimal(Math.scalb(1.0, 53 + minExp))) < 0;
      java.math.BigDecimal bound = exactSums ? java.math.BigDecimal.ZERO : abs.multiply(new java.math.BigDecimal(gamma));
      if (exactSums) {
        // every order gives the same sum, so the same quotient
        LAST.get()[0] = value;
        LAST.get()[1] = value;
        return;
      }
      java.math.BigDecimal n = new java.math.BigDecimal(len);
      java.math.MathContext mc = java.math.MathContext.DECIMAL128;
      double lo = Math.nextDown(exact.subtract(bound).divide(n, mc).doubleValue());
      double hi = Math.nextUp(exact.add(bound).divide(n, mc).doubleValue());
      LAST.get()[0] = Math.min(lo, value);
      LAST.get()[1] = Math.max(hi, value);
    }

    /**
     * whether two estimates could compare differently under some summation order: their
     * intervals overlap, unless both are the same single value (then every order gives that
     * value, and the tie breaks by the bids' fixed list order)
     */
    static boolean overlap(double alo, double ahi, double blo, double bhi) {
      if (alo == ahi && blo == bhi && alo == blo) {
        return false;
      }
      return ahi >= blo && bhi >= alo;
    }

    /** a decision `value > threshold` (strict) or `value >= threshold` the order could flip */
    static void threshold(double lo, double hi, double threshold, boolean strict) {
      boolean ambiguous = strict ? lo <= threshold && hi > threshold : lo < threshold && hi >= threshold;
      if (ambiguous) {
        flag("threshold " + threshold + " in [" + lo + ", " + hi + "]");
      }
    }
  }

  /** {@code choose_winning_bid}: the maximum by {@code total_cmp}, the last of equal maxima */
  static Bid chooseWinningBid(List<Bid> bids) {
    Bid best = null;
    for (Bid b : bids) {
      if (best == null || RustMath.totalCmp(b.bitsSavedPerNum, best.bitsSavedPerNum) >= 0) {
        best = b;
      }
    }
    if (Margins.on()) {
      for (Bid b : bids) {
        if (b != best && !b.mode.equals(best.mode) && Margins.overlap(b.lo, b.hi, best.lo, best.hi)) {
          Margins.flag("bids " + b.mode + " and " + best.mode);
        }
      }
    }
    return best;
  }

  // ------------------------------------------------------------ sampling

  static int calcSampleN(int n) {
    return n >= MIN_SAMPLE ? MIN_SAMPLE + (n - MIN_SAMPLE) / SAMPLE_RATIO : -1;
  }

  /** Xoroshiro128PlusPlus seeded by {@code seed_from_u64(0)} (SplitMix64) */
  static final class Xoroshiro128PlusPlus {
    private long s0;
    private long s1;

    Xoroshiro128PlusPlus(long seed) {
      long x = seed;
      x += 0x9e3779b97f4a7c15L;
      s0 = mix(x);
      x += 0x9e3779b97f4a7c15L;
      s1 = mix(x);
    }

    private static long mix(long z) {
      z = (z ^ (z >>> 30)) * 0xbf58476d1ce4e5b9L;
      z = (z ^ (z >>> 27)) * 0x94d049bb133111ebL;
      return z ^ (z >>> 31);
    }

    long nextU64() {
      long r = Long.rotateLeft(s0 + s1, 17) + s0;
      s1 ^= s0;
      s0 = Long.rotateLeft(s0, 49) ^ s1 ^ (s1 << 21);
      s1 = Long.rotateLeft(s1, 28);
      return r;
    }
  }

  interface Filter {
    /** whether to keep the number; the kept value goes to out[0] */
    boolean apply(long num, long[] out);
  }

  /** {@code choose_mode_sample} (Floyd's algorithm), or null if too few are kept */
  static long[] chooseModeSample(long[] nums, Filter filter) {
    int n = nums.length;
    int target = calcSampleN(n);
    if (target < 0) {
      return null;
    }
    Xoroshiro128PlusPlus rng = new Xoroshiro128PlusPlus(0);
    byte[] visited = new byte[(n + 7) / 8];
    long[] res = new long[target];
    int len = 0;
    long[] out = new long[1];
    for (int j = n - target; j < n; j++) {
      int t = (int) Long.remainderUnsigned(rng.nextU64(), j + 1L);
      int idx = (visited[t >>> 3] & (1 << (t & 7))) != 0 ? j : t;
      visited[idx >>> 3] |= (byte) (1 << (idx & 7));
      if (filter.apply(nums[idx], out)) {
        res[len++] = out[0];
      }
    }
    return len >= MIN_SAMPLE ? Arrays.copyOf(res, len) : null;
  }

  interface PrimaryFn {
    /** the primary latent of a sample value */
    long primary(long x);
  }

  /**
   * {@code est_bits_saved_per_num} with a constant saving per number. The reference sums the
   * savings of infrequent primaries in its HashMap's (random) order; this sums them in order of
   * first appearance (see VERIFICATION.md on summation order).
   */
  static double estBitsSavedPerNum(long[] sample, PrimaryFn primaryFn, double bitsSavedEach) {
    Map<Long, double[]> countsAndSavings = new LinkedHashMap<>();
    for (long x : sample) {
      long p = primaryFn.primary(x);
      double[] e = countsAndSavings.computeIfAbsent(p, key -> new double[] {0, 0.0});
      e[0] += 1;
      e[1] += bitsSavedEach;
    }
    return sumInfrequent(sample.length, countsAndSavings);
  }

  interface PrimaryAndSavings {
    /** sets out[0] to the primary and returns the bits saved */
    double apply(long x, long[] out);
  }

  static double estBitsSavedPerNum(long[] sample, PrimaryAndSavings f) {
    Map<Long, double[]> countsAndSavings = new LinkedHashMap<>();
    long[] out = new long[1];
    for (long x : sample) {
      double saved = f.apply(x, out);
      double[] e = countsAndSavings.computeIfAbsent(out[0], key -> new double[] {0, 0.0});
      e[0] += 1;
      e[1] += saved;
    }
    return sumInfrequent(sample.length, countsAndSavings);
  }

  private static double sumInfrequent(int sampleLen, Map<Long, double[]> countsAndSavings) {
    long cutoff = Math.max(1, RustMath.f64ToUnsigned(sampleLen / CLASSIC_MEMORIZABLE_BINS, 64));
    double s = -0.0;
    double[] terms = Margins.on() ? new double[countsAndSavings.size()] : null;
    int nTerms = 0;
    for (double[] e : countsAndSavings.values()) {
      if ((long) e[0] <= cutoff) {
        s += e[1];
        if (terms != null) {
          terms[nTerms++] = e[1];
        }
      }
    }
    double value = s / sampleLen;
    Margins.record(terms, nTerms, value, sampleLen);
    return value;
  }

  // ------------------------------------------------------------ bids

  static List<Bid> chooseModeBids(NumberType t, long[] nums, ChunkConfig config) {
    List<Bid> bids = new ArrayList<>();
    ModeSpec spec = config.modeSpec;
    if (t.isFloat()) {
      FloatType f = FloatType.of(t);
      switch (spec.kind) {
        case AUTO:
          {
            bids.add(classicBid(t));
            long[] sample = chooseModeSample(nums, (num, out) -> filterSample(f, num, out));
            if (sample != null) {
              Bid fm = FloatMult.computeBid(t, f, sample);
              if (fm != null) {
                bids.add(fm);
              }
              Bid fq = FloatQuant.computeBid(t, f, sample);
              if (fq != null) {
                bids.add(fq);
              }
            }
            return bids;
          }
        case CLASSIC:
          bids.add(classicBid(t));
          return bids;
        case TRY_FLOAT_MULT:
          {
            long base = f.fromF64(spec.floatBase);
            FloatMult.Config c = new FloatMult.Config(base, f.inv(base));
            bids.add(
                new Bid(
                    Mode.floatMult(t.toLatentOrdered(base)),
                    0.0,
                    xs -> FloatMult.splitLatents(t, f, xs, c)));
            return bids;
          }
        case TRY_FLOAT_QUANT:
          {
            int k = spec.k;
            bids.add(new Bid(Mode.floatQuant(k), 0.0, xs -> FloatQuant.splitLatents(t, f, xs, k)));
            return bids;
          }
        case TRY_INT_MULT:
          throw PcoException.invalidArgument("unable to use int mult mode on floats");
        default:
          bids.add(Dict.computeBid(t, nums));
          return bids;
      }
    }
    switch (spec.kind) {
      case AUTO:
        {
          long[] base = IntMult.chooseBase(t, nums);
          bids.add(base != null ? intMultBid(t, base[0]) : classicBid(t));
          return bids;
        }
      case CLASSIC:
        bids.add(classicBid(t));
        return bids;
      case TRY_FLOAT_MULT:
      case TRY_FLOAT_QUANT:
        throw PcoException.invalidArgument("unable to use float mode for ints");
      case TRY_INT_MULT:
        bids.add(intMultBid(t, t.latent.fromU64(spec.intBase)));
        return bids;
      default:
        bids.add(Dict.computeBid(t, nums));
        return bids;
    }
  }

  static boolean modeIsValid(NumberType t, Mode mode) {
    switch (mode.kind) {
      case CLASSIC:
      case DICT:
        return true;
      case INT_MULT:
        return !t.isFloat() && mode.base != 0;
      case FLOAT_MULT:
        {
          if (!t.isFloat()) {
            return false;
          }
          FloatType f = FloatType.of(t);
          long base = t.fromLatentOrdered(mode.base);
          return f.isFinite(base) && f.lt(f.zero(), f.abs(base));
        }
      default:
        return t.isFloat() && mode.k > 0 && mode.k <= FloatType.of(t).precisionBits;
    }
  }

  private static boolean filterSample(FloatType f, long num, long[] out) {
    if (f.isNormal(num)) {
      long a = f.abs(num);
      if (f.le(a, f.maxForSampling())) {
        out[0] = a;
        return true;
      }
    }
    return false;
  }

  static Bid classicBid(NumberType t) {
    return new Bid(Mode.CLASSIC, 0.0, xs -> classicSplit(t, xs));
  }

  static Split classicSplit(NumberType t, long[] nums) {
    long[] p = new long[nums.length];
    for (int i = 0; i < nums.length; i++) {
      p[i] = t.toLatentOrdered(nums[i]);
    }
    return new Split(p, null);
  }

  static Bid intMultBid(NumberType t, long base) {
    return new Bid(Mode.intMult(base), 0.0, xs -> IntMult.splitLatents(t, xs, base));
  }

  // ------------------------------------------------------------ shared entropy helpers

  static double singleCategoryEntropy(double p) {
    if (p == 0.0 || p == 1.0) {
      return 0.0;
    }
    return -p * GlibcMath.log2(p);
  }

  static double worstCaseCategoricalEntropy(double concentratedP, double nCategoriesM1) {
    return singleCategoryEntropy(concentratedP)
        + nCategoriesM1 * singleCategoryEntropy((1.0 - concentratedP) / nCategoriesM1);
  }

  // ------------------------------------------------------------ int mult

  static final class IntMult {
    private IntMult() {}

    private static final double ZETA_OF_2 = Math.PI * Math.PI / 6.0;
    private static final double LCB_RATIO = 1.0;

    static Split splitLatents(NumberType t, long[] nums, long base) {
      long[] mults = new long[nums.length];
      long[] adjs = new long[nums.length];
      for (int i = 0; i < nums.length; i++) {
        long u = t.toLatentOrdered(nums[i]);
        mults[i] = Latent.div(u, base);
        adjs[i] = Latent.rem(u, base);
      }
      return new Split(mults, adjs);
    }

    static long calcGcd(long x, long y) {
      if (x == 0) {
        return y;
      }
      while (true) {
        if (y == 0) {
          return x;
        }
        x = Latent.rem(x, y);
        long tmp = x;
        x = y;
        y = tmp;
      }
    }

    interface F {
      double f(double x);
    }

    /** {@code solve_root_by_false_position}; NaN for none */
    static double solveRootByFalsePosition(F f, double lb, double ub) {
      double tol = 1e-4;
      double flb = f.f(lb);
      double fub = f.f(ub);
      if (flb > 0.0 || fub < 0.0) {
        return Double.NaN;
      }
      while (ub - lb > tol && fub - flb > 0.0) {
        double lbProp = 0.001 + 0.998 * fub / (fub - flb);
        double mid = lbProp * lb + (1.0 - lbProp) * ub;
        double fmid = f.f(mid);
        if (fmid < 0.0) {
          lb = mid;
          flb = fmid;
        } else {
          ub = mid;
          fub = fmid;
        }
      }
      return (lb + ub) / 2.0;
    }

    static long calcTripleGcd(long a, long b, long c) {
      if (Latent.lt(b, a)) {
        long t = a;
        a = b;
        b = t;
      }
      if (Latent.lt(c, b)) {
        long t = b;
        b = c;
        c = t;
      }
      if (Latent.lt(b, a)) {
        long t = a;
        a = b;
        b = t;
      }
      return calcGcd(b - a, c - a);
    }

    private static double powi3(double x) {
      return x * (x * x);
    }

    /** {@code filter_score_triple_gcd}; NaN for none */
    static double filterScoreTripleGcd(double gcd, long triplesWGcdL, long totalTriplesL) {
      double triplesWGcd = (double) triplesWGcdL;
      double totalTriples = (double) totalTriplesL;
      double probPerTriple = triplesWGcd / totalTriples;
      double naturalProbPerTriple = 1.0 / (ZETA_OF_2 * gcd * gcd);
      double stdev = Math.sqrt(naturalProbPerTriple * (1.0 - naturalProbPerTriple) / totalTriples);
      double zScore = (probPerTriple - naturalProbPerTriple) / stdev;
      if (zScore < 3.0) {
        return Double.NaN;
      }
      double triplesWGcdLcb = triplesWGcd - LCB_RATIO * Math.sqrt(triplesWGcd);
      if (triplesWGcdLcb <= 0.0) {
        return Double.NaN;
      }
      double congruence = RustMath.minF64(ZETA_OF_2 * triplesWGcdLcb / totalTriples, 1.0);
      double gcdM1 = gcd - 1.0;
      double gcdM1InvSq = 1.0 / (gcdM1 * gcdM1);
      F f = p -> powi3(p) + powi3(1.0 - p) * gcdM1InvSq - congruence;
      double lb = 1.0 / gcd;
      double ub = GlibcMath.cbrt(congruence) + Math.ulp(1.0);
      double concentratedP = solveRootByFalsePosition(f, lb, ub);
      if (Double.isNaN(concentratedP)) {
        return Double.NaN;
      }
      double entropy = worstCaseCategoricalEntropy(concentratedP, gcdM1);
      double saved = GlibcMath.log2(gcd) - entropy;
      if (saved < MULT_REQUIRED_BITS_SAVED_PER_NUM) {
        return Double.NaN;
      }
      return saved;
    }

    /**
     * {@code choose_candidate_base}: {base, bits of the score} or null. The reference picks the
     * best-scoring GCD from a HashMap, the last of equal scores in its random order; here equal
     * scores go to the GCD that appears first (see VERIFICATION.md).
     */
    static long[] chooseCandidateBase(long[] sample) {
      Map<Long, long[]> counts = new LinkedHashMap<>();
      int nTriples = sample.length / 3;
      for (int i = 0; i + 2 < sample.length && i / 3 < nTriples; i += 3) {
        long g = calcTripleGcd(sample[i], sample[i + 1], sample[i + 2]);
        if (Latent.lt(1, g)) {
          counts.computeIfAbsent(g, key -> new long[1])[0]++;
        }
      }
      long bestGcd = 0;
      double bestScore = Double.NaN;
      long bestKey = 0;
      boolean found = false;
      for (Map.Entry<Long, long[]> e : counts.entrySet()) {
        long gcd = e.getKey();
        double gcdF64 = Latent.toF64(gcd);
        double score = filterScoreTripleGcd(gcdF64, e.getValue()[0], nTriples);
        if (Double.isNaN(score)) {
          continue;
        }
        long key = NumberType.F64.toLatentOrdered(Double.doubleToRawLongBits(score));
        if (Margins.on()) {
          // the reference takes the last of equal scores in HashMap order: keep every
          // candidate with the best score, for the callers to check that any gives one outcome
          if (!found || Long.compareUnsigned(key, bestKey) > 0) {
            TIED.get().clear();
          }
          if (!found || Long.compareUnsigned(key, bestKey) >= 0) {
            TIED.get().add(gcd);
          }
        }
        if (!found || Long.compareUnsigned(key, bestKey) > 0) {
          found = true;
          bestKey = key;
          bestGcd = gcd;
          bestScore = score;
        }
      }
      return found ? new long[] {bestGcd, Double.doubleToRawLongBits(bestScore)} : null;
    }

    /** the candidates of the last chooseCandidateBase with the best score (Margins only) */
    static final ThreadLocal<List<Long>> TIED = ThreadLocal.withInitial(ArrayList::new);

    /** {@code choose_base}: {base} or null */
    static long[] chooseBase(NumberType t, long[] nums) {
      long[] sample =
          chooseModeSample(
              nums,
              (num, out) -> {
                out[0] = t.toLatentOrdered(num);
                return true;
              });
      if (sample == null) {
        return null;
      }
      if (Margins.on()) {
        TIED.get().clear();
      }
      long[] cand = chooseCandidateBase(sample);
      if (cand == null) {
        return null;
      }
      long candidate = cand[0];
      double bitsSavedPerAdj = Double.longBitsToDouble(cand[1]);
      if (Margins.on() && TIED.get().size() > 1) {
        // a score tie: every tied candidate must lead to the same outcome
        List<Long> tied = new ArrayList<>(TIED.get());
        Long outcome = null;
        boolean first = true;
        for (long g : tied) {
          double e = estBitsSavedPerNum(sample, x -> Latent.div(x, g), bitsSavedPerAdj);
          Margins.threshold(Margins.lo(), Margins.hi(), MULT_REQUIRED_BITS_SAVED_PER_NUM, true);
          Long o = e > MULT_REQUIRED_BITS_SAVED_PER_NUM ? g : null;
          if (!first && !java.util.Objects.equals(o, outcome)) {
            Margins.flag("int mult score tie with different outcomes");
          }
          outcome = o;
          first = false;
        }
      }
      double est = estBitsSavedPerNum(sample, x -> Latent.div(x, candidate), bitsSavedPerAdj);
      Margins.threshold(Margins.lo(), Margins.hi(), MULT_REQUIRED_BITS_SAVED_PER_NUM, true);
      return est > MULT_REQUIRED_BITS_SAVED_PER_NUM ? new long[] {candidate} : null;
    }
  }

  // ------------------------------------------------------------ float quant

  static final class FloatQuant {
    private FloatQuant() {}

    static Split splitLatents(NumberType t, FloatType f, long[] nums, int k) {
      Latent lt = t.latent;
      long lowestMax = lt.sub(shl(lt, 1, k), 1);
      long[] primary = new long[nums.length];
      long[] secondary = new long[nums.length];
      for (int i = 0; i < nums.length; i++) {
        long num = nums[i];
        long u = t.toLatentOrdered(num);
        primary[i] = shr(u, k);
        long lowest = u & lowestMax;
        secondary[i] = f.isSignPositive(num) ? lowest : lt.sub(lowestMax, lowest);
      }
      return new Split(primary, secondary);
    }

    static long shl(Latent lt, long x, int k) {
      return k >= 64 ? 0 : (x << k) & lt.mask;
    }

    static long shr(long x, int k) {
      return k >= 64 ? 0 : x >>> k;
    }

    static double[] estimateBestKAndBitsSavedFromHist(int[] cumulative, int sampleLen) {
      double len = sampleLen;
      int bestK = 0;
      double bestSaved = 0.0;
      for (int k = 1; k < cumulative.length; k++) {
        int occ = cumulative[k];
        if (occ == 0) {
          continue;
        }
        double freq = (double) occ / len;
        double nCategories = (double) ((1L << k) - 1);
        double worst = worstCaseCategoricalEntropy(freq, nCategories);
        double saved = k - worst;
        if (saved > bestSaved) {
          bestK = k;
          bestSaved = saved;
        } else {
          break;
        }
      }
      return new double[] {bestK, bestSaved};
    }

    static double[] estimateBestKAndBitsSaved(FloatType f, long[] sample) {
      int[] hist = new int[f.precisionBits + 1];
      for (long x : sample) {
        hist[Math.min(f.precisionBits, f.trailingZeros(x))]++;
      }
      int rev = 0;
      for (int i = hist.length - 1; i >= 0; i--) {
        rev += hist[i];
        hist[i] = rev;
      }
      return estimateBestKAndBitsSavedFromHist(hist, sample.length);
    }

    static Bid computeBid(NumberType t, FloatType f, long[] sample) {
      double[] kb = estimateBestKAndBitsSaved(f, sample);
      int k = (int) kb[0];
      double perInfrequent = kb[1];
      double saved = estBitsSavedPerNum(sample, x -> shr(x, k), perInfrequent);
      double lo = Margins.lo();
      double hi = Margins.hi();
      Margins.threshold(lo, hi, QUANT_REQUIRED_BITS_SAVED_PER_NUM, true);
      if (saved > QUANT_REQUIRED_BITS_SAVED_PER_NUM) {
        Bid b = new Bid(Mode.floatQuant(k), saved, xs -> splitLatents(t, f, xs, k));
        b.lo = lo;
        b.hi = hi;
        return b;
      }
      return null;
    }
  }

  // ------------------------------------------------------------ dict

  static final class Dict {
    private Dict() {}

    /**
     * Verification hook (package-private, tests only): a dictionary order to use instead of
     * first appearance, e.g. the order a reference process chose. Must be a permutation of the
     * order this method would use that keeps counts descending.
     */
    static final ThreadLocal<long[]> FORCED_ORDER = new ThreadLocal<>();

    /**
     * {@code dict::compute_bid}. The reference orders the dictionary by descending count with
     * an unstable sort over a HashMap's (random) order, so its dictionary order among equal
     * counts is not reproducible; this orders equal counts by first appearance, unless
     * {@link #FORCED_ORDER} gives another.
     */
    static Bid computeBid(NumberType t, long[] nums) {
      long[] classic = new long[nums.length];
      Map<Long, int[]> countByUnique = new LinkedHashMap<>();
      for (int i = 0; i < nums.length; i++) {
        classic[i] = t.toLatentOrdered(nums[i]);
        countByUnique.computeIfAbsent(classic[i], key -> new int[1])[0]++;
      }
      List<Map.Entry<Long, int[]>> entries = new ArrayList<>(countByUnique.entrySet());
      entries.sort((a, b) -> Integer.compareUnsigned(b.getValue()[0], a.getValue()[0]));
      long[] dict = new long[entries.size()];
      long[] forced = FORCED_ORDER.get();
      if (forced != null) {
        checkForcedOrder(forced, entries);
      }
      Map<Long, Integer> idx = new HashMap<>();
      for (int i = 0; i < dict.length; i++) {
        dict[i] = forced != null ? forced[i] : entries.get(i).getKey();
        idx.put(dict[i], i);
      }
      long[] indices = new long[nums.length];
      for (int i = 0; i < nums.length; i++) {
        indices[i] = idx.get(classic[i]) & 0xffffffffL;
      }
      Split split = new Split(indices, null);
      return new Bid(Mode.dict(dict), 0.0, xs -> split);
    }

    /** a forced order is the reference's sort: the same values, counts descending */
    private static void checkForcedOrder(long[] forced, List<Map.Entry<Long, int[]>> entries) {
      Map<Long, Integer> counts = new HashMap<>();
      for (Map.Entry<Long, int[]> e : entries) {
        counts.put(e.getKey(), e.getValue()[0]);
      }
      if (forced.length != entries.size()) {
        throw new IllegalArgumentException("forced dictionary has another size");
      }
      for (int i = 0; i < forced.length; i++) {
        Integer c = counts.remove(forced[i]);
        if (c == null || c != entries.get(i).getValue()[0]) {
          throw new IllegalArgumentException("forced dictionary is not a count-descending permutation");
        }
      }
    }
  }
}
