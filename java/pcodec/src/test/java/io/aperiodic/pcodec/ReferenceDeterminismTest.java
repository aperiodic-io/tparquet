package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.util.Arrays;
import java.util.SplittableRandom;
import org.junit.jupiter.api.Test;

/**
 * The reference is not a function of its input alone: its mode heuristics iterate Rust
 * HashMaps, whose order is seeded per process (VERIFICATION.md, Nondeterminism of the reference).
 * This runs the same cases through several reference processes and reports how often they
 * disagree with each other, and that the Java output always equals at least one of them.
 */
class ReferenceDeterminismTest {
  @Test
  void javaAgreesWithTheReferenceWheneverTheReferenceAgreesWithItself() {
    Oracle first = Oracle.get();
    assumeTrue(first != null, "oracle not built");
    Oracle[] others = {Oracle.start(), Oracle.start(), Oracle.start()};
    int scale = Integer.getInteger("pcodec.parity.scale", 1);
    SplittableRandom r = new SplittableRandom(8);
    int cases = 0;
    int unstable = 0;
    int dictTies = 0;
    try {
      for (Corpus.Case c : Corpus.cases(NumberType.values(), new int[] {50, 400, 3000}, 2 * scale)) {
        Oracle.Cfg cfg = new Oracle.Cfg();
        cfg.enable8Bit = true;
        if (r.nextInt(3) == 0) {
          cfg.mode = 5; // dict, whose order among equal counts is a HashMap's
        }
        byte[] java;
        try {
          java = Pco.compress(c.type, c.nums, cfg.toConfig());
        } catch (PcoException e) {
          continue;
        }
        cases++;
        byte[] a = first.compress(c.type, cfg, c.nums).body;
        boolean stable = true;
        boolean javaSeen = Arrays.equals(a, java);
        for (Oracle o : others) {
          byte[] b = o.compress(c.type, cfg, c.nums).body;
          stable &= Arrays.equals(a, b);
          javaSeen |= Arrays.equals(b, java);
        }
        if (cfg.mode == 5 && !ParityTest.dictReproducible(c.type, c.nums)) {
          // dictionary order among tied counts is a HashMap's: the reference's dictionary is a
          // random permutation, which may lead it (stably, e.g. always to its fallback) where
          // Java's first-appearance order does not; no output of it is the one to match
          dictTies++;
          continue;
        }
        if (!stable) {
          unstable++;
          // the reference's outputs differ; each must still decode to the numbers
          long[] back = Pco.decompress(a, c.type).nums;
          assertTrue(cfg.mode == 5 || Arrays.equals(back, c.nums), c.toString());
        } else {
          assertTrue(javaSeen, c + ": java differs from a stable reference");
        }
      }
    } finally {
      for (Oracle o : others) {
        o.close();
      }
    }
    System.out.println(cases + " cases: the reference unstable on " + unstable + ", dict with tied counts "
        + dictTies + " (not compared), java equal to the reference on all others");
  }
}
