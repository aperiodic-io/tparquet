package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.util.ArrayList;
import java.util.List;
import java.util.SplittableRandom;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Differential tests against the Rust reference (pco 1.0.4): every output byte and every
 * accept/reject decision of the Java implementation must equal the reference's.
 */
class ParityTest {
  static Oracle o;
  static int scale;

  static final NumberType[] ALL = NumberType.values();

  @BeforeAll
  static void setUp() {
    o = Oracle.get();
    scale = Integer.getInteger("pcodec.parity.scale", 1);
  }

  /** collects mismatches; fails with the first few */
  static final class Report {
    int cases;
    final int[] statuses = new int[3];
    final List<String> fails = new ArrayList<>();
    final String name = StackWalker.getInstance().walk(f -> f.skip(1).findFirst().get().getMethodName());

    void check(String what, Oracle.Answer want, Parity.Outcome got) {
      cases++;
      statuses[want.status]++;
      String d = Parity.compare(want, got);
      if (d != null) {
        fails.add(what + ": " + d);
      }
    }

    void done() {
      if (!fails.isEmpty()) {
        StringBuilder sb = new StringBuilder(fails.size() + " of " + cases + " cases differ\n");
        for (int i = 0; i < Math.min(20, fails.size()); i++) {
          sb.append(fails.get(i)).append('\n');
        }
        throw new AssertionError(sb.toString());
      }
      System.out.println(
          name + ": " + cases + " cases agree (reference ok " + statuses[0] + ", errors " + statuses[1]
              + ", panics " + statuses[2] + ")");
    }
  }

  static void compress(Report rep, NumberType t, Oracle.Cfg cfg, long[] nums, String what) {
    Oracle.Answer want = o.compress(t, cfg, nums);
    Parity.Outcome got = Parity.java(() -> Pco.compress(t, nums, cfg.toConfig()));
    rep.check(what + " " + cfg, want, got);
    if (want.ok()) {
      // the Java decoder reads the reference's bytes back exactly
      Oracle.Answer dec = o.decompress(t, want.body);
      rep.check(what + " decode", dec, Parity.java(() -> Parity.decodeBody(t, want.body)));
    }
  }

  @Test
  void defaultConfigEveryTypeShapeAndSize() {
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    for (Corpus.Case c : Corpus.cases(ALL, Corpus.SIZES, 1 + scale)) {
      Oracle.Cfg cfg = new Oracle.Cfg();
      cfg.enable8Bit = c.type.bits() == 8;
      compress(rep, c.type, cfg, c.nums, c.toString());
    }
    rep.done();
  }

  @Test
  void chunkMetadataEqualsTheReferences() {
    // the chosen mode, delta encoding, bins and ANS table sizes, compared as the reference's
    // Debug output prints them
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    SplittableRandom r = new SplittableRandom(11);
    for (Corpus.Case c : Corpus.cases(ALL, new int[] {1, 12, 300, 3000}, scale)) {
      Oracle.Cfg[] modes = modeSpecs(c.type, r);
      Oracle.Cfg cfg = r.nextBoolean() ? new Oracle.Cfg() : modes[r.nextInt(modes.length)];
      Oracle.Cfg d = deltaSpecs()[r.nextInt(deltaSpecs().length)];
      cfg.delta = r.nextBoolean() ? 0 : d.delta;
      cfg.deltaOrder = d.deltaOrder;
      cfg.enable8Bit = true;
      if (cfg.mode == 5 && !dictReproducible(c.type, c.nums)) {
        continue;
      }
      Oracle.Cfg f = cfg;
      rep.check(c + " " + cfg, o.meta(c.type, cfg, c.nums),
          Parity.java(() -> Pco.chunkMeta(c.type, c.nums, f.toConfig()).toString().getBytes(java.nio.charset.StandardCharsets.UTF_8)));
    }
    rep.done();
  }

  @Test
  void everyCompressionLevel() {
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    NumberType[] types = {NumberType.I32, NumberType.I64, NumberType.F32, NumberType.F64, NumberType.U16};
    for (Corpus.Case c : Corpus.cases(types, new int[] {7, 300, 3000, 20000}, scale)) {
      for (int level = 0; level <= 13; level++) {
        Oracle.Cfg cfg = new Oracle.Cfg();
        cfg.level = level;
        compress(rep, c.type, cfg, c.nums, c.toString());
      }
    }
    rep.done();
  }

  static Oracle.Cfg[] modeSpecs(NumberType t, SplittableRandom r) {
    List<Oracle.Cfg> res = new ArrayList<>();
    for (int mode = 0; mode <= 5; mode++) {
      Oracle.Cfg c = new Oracle.Cfg();
      c.mode = mode;
      res.add(c);
    }
    long[] intBases = {0, 1, 2, 3, 7, 10, 100, 1000, 1L << 31, -1L, 1L << 40};
    for (long b : intBases) {
      Oracle.Cfg c = new Oracle.Cfg();
      c.mode = 4;
      c.modeBase = b;
      res.add(c);
    }
    double[] floatBases = {0.0, -0.0, 0.1, 0.01, 1.0, -0.5, 3.0, 1e-300, 1e300, Double.NaN,
        Double.POSITIVE_INFINITY, 1e-40, 65504.0, 1e-7};
    for (double b : floatBases) {
      Oracle.Cfg c = new Oracle.Cfg();
      c.mode = 2;
      c.modeF64 = b;
      res.add(c);
    }
    for (int k : new int[] {0, 1, 2, 5, 9, 10, 11, 20, 22, 23, 24, 31, 32, 40, 52, 53, 63, 64, 65}) {
      Oracle.Cfg c = new Oracle.Cfg();
      c.mode = 3;
      c.modeK = k;
      res.add(c);
    }
    return res.toArray(new Oracle.Cfg[0]);
  }

  static Oracle.Cfg[] deltaSpecs() {
    List<Oracle.Cfg> res = new ArrayList<>();
    for (int d : new int[] {0, 1, 3}) {
      Oracle.Cfg c = new Oracle.Cfg();
      c.delta = d;
      res.add(c);
    }
    for (long order = 0; order <= 8; order++) {
      Oracle.Cfg c = new Oracle.Cfg();
      c.delta = 2;
      c.deltaOrder = order;
      res.add(c);
    }
    for (long order : new long[] {0, 1, 2, 3, 6, 7, 13, 32, 33}) {
      Oracle.Cfg c = new Oracle.Cfg();
      c.delta = 4;
      c.deltaOrder = order;
      res.add(c);
    }
    return res.toArray(new Oracle.Cfg[0]);
  }

  /** dict mode orders equal counts by HashMap order in the reference: compare only without ties */
  static boolean dictReproducible(NumberType t, long[] nums) {
    java.util.Map<Long, Integer> counts = new java.util.HashMap<>();
    for (long x : nums) {
      counts.merge(x, 1, Integer::sum);
    }
    return new java.util.HashSet<>(counts.values()).size() == counts.size();
  }

  @Test
  void everyModeSpec() {
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    SplittableRandom r = new SplittableRandom(1);
    for (Corpus.Case c : Corpus.cases(ALL, new int[] {1, 12, 333, 2000}, scale)) {
      for (Oracle.Cfg cfg : modeSpecs(c.type, r)) {
        cfg.enable8Bit = c.type.bits() == 8 || r.nextBoolean();
        if (cfg.mode == 5 && !dictReproducible(c.type, c.nums)) {
          continue;
        }
        compress(rep, c.type, cfg, c.nums, c.toString());
      }
    }
    rep.done();
  }

  @Test
  void everyDeltaSpec() {
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    for (Corpus.Case c : Corpus.cases(ALL, new int[] {1, 5, 40, 513, 1030, 5000}, scale)) {
      for (Oracle.Cfg cfg : deltaSpecs()) {
        cfg.enable8Bit = true;
        compress(rep, c.type, cfg, c.nums, c.toString());
      }
    }
    rep.done();
  }

  @Test
  void modesAndDeltasTogether() {
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    SplittableRandom r = new SplittableRandom(2);
    for (Corpus.Case c : Corpus.cases(ALL, new int[] {300, 2500}, scale)) {
      Oracle.Cfg[] modes = modeSpecs(c.type, r);
      Oracle.Cfg[] deltas = deltaSpecs();
      for (int k = 0; k < 6; k++) {
        Oracle.Cfg cfg = modes[r.nextInt(modes.length)];
        Oracle.Cfg d = deltas[r.nextInt(deltas.length)];
        cfg.delta = d.delta;
        cfg.deltaOrder = d.deltaOrder;
        cfg.level = r.nextInt(13);
        cfg.enable8Bit = true;
        if (cfg.mode == 5 && !dictReproducible(c.type, c.nums)) {
          continue;
        }
        compress(rep, c.type, cfg, c.nums, c.toString());
      }
    }
    rep.done();
  }

  @Test
  void pagingSpecs() {
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    SplittableRandom r = new SplittableRandom(3);
    for (Corpus.Case c : Corpus.cases(new NumberType[] {NumberType.I64, NumberType.F64, NumberType.F32, NumberType.U32}, new int[] {0, 1, 100, 1001}, scale)) {
      for (long up : new long[] {0, 1, 3, 7, 100, 256, 1000, 1L << 18}) {
        Oracle.Cfg cfg = new Oracle.Cfg();
        cfg.pagingValues = new long[] {up};
        cfg.delta = r.nextInt(5);
        cfg.deltaOrder = r.nextInt(8);
        if (c.n == 0 || up == 0) {
          continue; // n == 0 with 0: both divide by zero only when n > 0
        }
        compress(rep, c.type, cfg, c.nums, c.toString());
      }
      for (int k = 0; k < 4; k++) {
        Oracle.Cfg cfg = new Oracle.Cfg();
        cfg.paging = 1;
        int parts = 1 + r.nextInt(5);
        long[] p = new long[parts];
        long left = c.n;
        for (int i = 0; i < parts - 1; i++) {
          p[i] = left == 0 ? 0 : r.nextInt((int) left + 1);
          left -= p[i];
        }
        p[parts - 1] = left + (r.nextInt(8) == 0 ? 1 : 0);
        cfg.pagingValues = p;
        compress(rep, c.type, cfg, c.nums, c.toString());
      }
    }
    rep.done();
  }

  @Test
  void multiChunkFiles() {
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    for (NumberType t : new NumberType[] {NumberType.I32, NumberType.I64, NumberType.F32, NumberType.F64}) {
      for (String shape : new String[] {"walk", "prices", "uniform", "mixed"}) {
        for (int n : new int[] {(1 << 18) + 1, 600000}) {
          long[] nums = Corpus.generate(t, shape, n, 7);
          compress(rep, t, new Oracle.Cfg(), nums, t + "/" + shape + "/n=" + n);
        }
      }
    }
    rep.done();
  }

  @Test
  void decoderAgreesOnCorruptFiles() {
    assumeTrue(o != null, "oracle not built");
    Report rep = new Report();
    SplittableRandom r = new SplittableRandom(4);
    for (Corpus.Case c : Corpus.cases(ALL, new int[] {1, 30, 300, 1500}, 2 * scale)) {
      Oracle.Cfg[] modes = modeSpecs(c.type, r);
      Oracle.Cfg[] deltas = deltaSpecs();
      Oracle.Cfg cfg = modes[r.nextInt(modes.length)];
      Oracle.Cfg d = deltas[r.nextInt(deltas.length)];
      cfg.enable8Bit = true;
      cfg.delta = d.delta;
      cfg.deltaOrder = d.deltaOrder;
      Oracle.Answer good = o.compress(c.type, cfg, c.nums);
      if (!good.ok()) {
        continue;
      }
      for (int k = 0; k < 24; k++) {
        byte[] bad = good.body.clone();
        switch (r.nextInt(4)) {
          case 0:
            bad[r.nextInt(bad.length)] ^= (byte) (1 << r.nextInt(8));
            break;
          case 1:
            bad = java.util.Arrays.copyOf(bad, r.nextInt(bad.length));
            break;
          case 2:
            for (int j = 0; j < 4; j++) {
              bad[8 + r.nextInt(bad.length - 8)] = (byte) r.nextInt();
            }
            break;
          default:
            bad = java.util.Arrays.copyOf(bad, bad.length + 1 + r.nextInt(5));
            break;
        }
        byte[] f = bad;
        NumberType t = r.nextInt(10) == 0 ? ALL[r.nextInt(ALL.length)] : c.type;
        rep.check(c + " corrupt " + k, o.decompress(t, f), Parity.java(() -> Parity.decodeBody(t, f)));
      }
    }
    rep.done();
  }
}
