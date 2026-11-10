package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.List;
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
}
