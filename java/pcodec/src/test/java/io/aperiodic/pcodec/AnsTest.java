package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;

/** tANS: the reference's own test vectors, and exhaustive inversion on small tables. */
class AnsTest {
  @Test
  void spreadMatchesReference() {
    assertArrayEquals(
        new int[] {0, 3, 2, 3, 2, 3, 3, 3, 3, 1, 3, 2, 3, 3, 3, 3},
        Ans.Spec.fromWeights(4, new int[] {1, 1, 3, 11}).stateSymbols);
    assertArrayEquals(new int[] {0}, Ans.Spec.fromWeights(0, new int[] {1}).stateSymbols);
    assertArrayEquals(new int[] {0, 0}, Ans.Spec.fromWeights(1, new int[] {2}).stateSymbols);
    assertArrayEquals(new int[] {0}, Ans.Spec.fromWeights(0, new int[0]).stateSymbols);
    assertThrows(PcoException.class, () -> Ans.Spec.fromWeights(3, new int[] {1, 2}));
  }

  @Test
  void quantizeWeightsMatchesReference() {
    assertArrayEquals(new int[] {1}, Ans.quantizeWeightsTo(new int[] {777}, 777, 0));
    assertArrayEquals(new int[] {1, 1}, Ans.quantizeWeightsTo(new int[] {777, 1}, 778, 1));
    assertArrayEquals(new int[] {3, 1}, Ans.quantizeWeightsTo(new int[] {777, 1}, 778, 2));
    assertArrayEquals(new int[] {1, 1, 3, 2, 1}, Ans.quantizeWeightsTo(new int[] {2, 3, 6, 5, 1}, 17, 3));
    assertArrayEquals(new int[] {1, 1}, Ans.quantizeWeightsTo(new int[] {1, 1}, 2, 1));
    assertArrayEquals(new int[] {7, 9, 4}, Ans.quantizeWeights(new int[] {77, 100}, 177, 4));
    assertArrayEquals(new int[] {1, 1, 1}, Ans.quantizeWeights(new int[] {77, 77}, 154, 4));
    assertArrayEquals(new int[] {1, 0}, Ans.quantizeWeights(new int[] {5}, 5, 4));
  }

  /** every weight vector of a table of 2^sizeLog states, for sizeLog <= 4 */
  static List<int[]> compositions(int total) {
    List<int[]> res = new ArrayList<>();
    for (int mask = 0; mask < 1 << (total - 1); mask++) {
      List<Integer> parts = new ArrayList<>();
      int run = 1;
      for (int i = 0; i < total - 1; i++) {
        if ((mask & (1 << i)) != 0) {
          parts.add(run);
          run = 1;
        } else {
          run++;
        }
      }
      parts.add(run);
      res.add(parts.stream().mapToInt(Integer::intValue).toArray());
    }
    return res;
  }

  @Test
  void decoderInvertsEncoderForEveryTableUpTo16States() {
    int tables = 0;
    for (int sizeLog = 0; sizeLog <= 4; sizeLog++) {
      for (int[] weights : compositions(1 << sizeLog)) {
        tables++;
        Ans.Spec spec = Ans.Spec.fromWeights(sizeLog, weights);
        Ans.Encoder enc = new Ans.Encoder(spec);
        Ans.Decoder dec = new Ans.Decoder(spec, new int[0]);
        int nSym = weights.length;
        // every symbol sequence of length 3, and one long one
        int seqs = nSym * nSym * nSym;
        for (int s = 0; s <= seqs; s++) {
          int[] symbols;
          if (s < seqs) {
            symbols = new int[] {s % nSym, (s / nSym) % nSym, s / nSym / nSym};
          } else {
            symbols = new int[50];
            for (int i = 0; i < 50; i++) {
              symbols[i] = (i * 7 + 3) % nSym;
            }
          }
          // encode in reverse, write the renormalization bits forward
          int state = enc.defaultState();
          long[] vals = new long[symbols.length];
          int[] bits = new int[symbols.length];
          for (int i = symbols.length - 1; i >= 0; i--) {
            long r = enc.encode(state, symbols[i]);
            bits[i] = (int) (r >>> 32);
            vals[i] = state & ((1L << bits[i]) - 1);
            state = (int) r;
          }
          BitWriter w = new BitWriter();
          for (int i = 0; i < symbols.length; i++) {
            w.writeUint(vals[i], bits[i]);
          }
          byte[] b = w.toByteArray();
          BitReader r = new BitReader(b, 0, b.length);
          int idx = state - (1 << sizeLog);
          for (int i = 0; i < symbols.length; i++) {
            assertEquals(symbols[i], spec.stateSymbols[idx]);
            idx = dec.nextStateIdxBase[idx] + r.readInt(dec.bitsToRead[idx]);
          }
          assertEquals(0, idx, "the decoder ends in the encoder's initial state");
        }
      }
    }
    assertEquals(1 + 2 + 8 + 128 + 32768, tables);
  }
}
