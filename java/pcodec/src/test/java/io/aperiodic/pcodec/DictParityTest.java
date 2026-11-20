package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assumptions.assumeTrue;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Arrays;
import java.util.SplittableRandom;
import org.junit.jupiter.api.Test;

/**
 * Dict mode where counts tie: the reference's dictionary order is its HashMap's, so each
 * reference process may write different bytes. The traced reference (oracle-traced/, pco 1.0.4
 * plus a record of the dictionary) reports the order it chose with its output; given that same
 * order, the Java encoder must write the same bytes. This verifies Dict mode completely, up to
 * the one choice the reference leaves to chance.
 */
class DictParityTest {
  @Test
  void givenTheReferencesDictionaryJavaWritesItsBytes() {
    Oracle traced = Oracle.startTraced();
    assumeTrue(traced != null, "traced oracle not built");
    int scale = Integer.getInteger("pcodec.parity.scale", 1);
    ParityTest.Report rep = new ParityTest.Report();
    SplittableRandom r = new SplittableRandom(12);
    Oracle.Cfg[] deltas = ParityTest.deltaSpecs();
    try {
      for (Corpus.Case c : Corpus.cases(NumberType.values(), new int[] {1, 10, 50, 400, 3000, 20000}, 2 * scale)) {
        for (int k = 0; k < 2; k++) {
          Oracle.Cfg cfg = new Oracle.Cfg();
          cfg.mode = 5;
          cfg.enable8Bit = true;
          Oracle.Cfg d = deltas[r.nextInt(deltas.length)];
          cfg.delta = k == 0 ? 0 : d.delta;
          cfg.deltaOrder = d.deltaOrder;
          cfg.level = r.nextInt(13);
          byte[] req = Oracle.compressRequest(c.type, cfg, c.nums);
          req[0] = 13;
          Oracle.Answer a = traced.call(req);
          Oracle.Answer want;
          long[] dict = null;
          if (a.ok()) {
            ByteBuffer b = ByteBuffer.wrap(a.body).order(ByteOrder.LITTLE_ENDIAN);
            dict = new long[b.getInt()];
            for (int i = 0; i < dict.length; i++) {
              dict[i] = b.getLong();
            }
            want = new Oracle.Answer(0, Arrays.copyOfRange(a.body, b.position(), a.body.length));
          } else {
            want = a;
          }
          long[] order = dict;
          Parity.Outcome got;
          Modes.Dict.FORCED_ORDER.set(order != null && order.length > 0 ? order : null);
          try {
            got = Parity.java(() -> Pco.compress(c.type, c.nums, cfg.toConfig()));
          } finally {
            Modes.Dict.FORCED_ORDER.remove();
          }
          rep.check(c + " " + cfg, want, got);
        }
      }
    } finally {
      traced.close();
    }
    rep.done();
  }
}
