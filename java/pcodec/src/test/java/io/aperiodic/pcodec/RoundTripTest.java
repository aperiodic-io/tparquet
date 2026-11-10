package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import java.util.SplittableRandom;
import org.junit.jupiter.api.Test;

/** Without the reference: every file the Java encoder writes decodes to its numbers, bit for bit. */
class RoundTripTest {
  @Test
  void losslessForEveryTypeShapeAndConfig() {
    SplittableRandom r = new SplittableRandom(9);
    int n = 0;
    for (Corpus.Case c : Corpus.cases(NumberType.values(), new int[] {0, 1, 9, 10, 257, 1000, 3001}, 1)) {
      for (int k = 0; k < 3; k++) {
        Oracle.Cfg cfg = new Oracle.Cfg();
        cfg.enable8Bit = true;
        if (k > 0) {
          Oracle.Cfg[] modes = ParityTest.modeSpecs(c.type, r);
          Oracle.Cfg[] deltas = ParityTest.deltaSpecs();
          cfg = modes[r.nextInt(modes.length)];
          Oracle.Cfg d = deltas[r.nextInt(deltas.length)];
          cfg.delta = d.delta;
          cfg.deltaOrder = d.deltaOrder;
          cfg.level = r.nextInt(13);
          cfg.enable8Bit = true;
        }
        if (cfg.mode == 5 && cfg.delta != 1) {
          // pco 1.0.4 cannot always decode its own Dict mode files with a delta encoding: its
          // dict index check reads stale delta-decoded scratch latents (see
          // referenceCannotDecodeDictWithDelta); Java reproduces that, so this is no round trip
          continue;
        }
        byte[] file;
        try {
          file = Pco.compress(c.type, c.nums, cfg.toConfig());
        } catch (PcoException | PcoPanic e) {
          continue; // an invalid configuration for this type (checked against the reference elsewhere)
        }
        Pco.Decoded d = Pco.decompress(file, c.type);
        assertArrayEquals(c.nums, d.nums, c + " " + cfg);
        assertEquals(file.length, d.end);
        n++;
      }
    }
    assertTrue(n > 2500, n + " round trips");
  }

  @Test
  void referenceCannotDecodeDictWithDelta() {
    // a pco 1.0.4 bug, reproduced on purpose: the dict index check of join_latents reads the
    // whole 256-entry scratch array, and past the batch it holds a single bin's lower bound
    // that the delta decoding never touched
    long[] nums = Corpus.generate(NumberType.F64, "steps", 9, 0);
    ChunkConfig cfg = new ChunkConfig().withModeSpec(ModeSpec.TRY_DICT).withDeltaSpec(DeltaSpec.tryConsecutive(4));
    byte[] file = Pco.compress(NumberType.F64, nums, cfg);
    PcoException e = org.junit.jupiter.api.Assertions.assertThrows(PcoException.class, () -> Pco.decompress(file, NumberType.F64));
    assertEquals("dict index exceeded dict length 1", e.getMessage());
  }

  @Test
  void typedApi() {
    int[] ints = {1, -2, Integer.MIN_VALUE, Integer.MAX_VALUE, 0};
    assertArrayEquals(ints, Pco.decompressInts(Pco.compress(ints, new ChunkConfig())));
    long[] longs = {1, -2, Long.MIN_VALUE, Long.MAX_VALUE, 0};
    assertArrayEquals(longs, Pco.decompressLongs(Pco.compress(longs, new ChunkConfig())));
    float[] floats = {1.5f, -0.0f, Float.NaN, Float.MIN_VALUE};
    float[] fd = Pco.decompressFloats(Pco.compress(floats, new ChunkConfig()));
    for (int i = 0; i < floats.length; i++) {
      assertEquals(Float.floatToRawIntBits(floats[i]), Float.floatToRawIntBits(fd[i]));
    }
    double[] doubles = {1.5, -0.0, Double.longBitsToDouble(0x7ff0000000000123L), Double.MIN_VALUE};
    double[] dd = Pco.decompressDoubles(Pco.compress(doubles, new ChunkConfig()));
    for (int i = 0; i < doubles.length; i++) {
      assertEquals(Double.doubleToRawLongBits(doubles[i]), Double.doubleToRawLongBits(dd[i]));
    }
    assertEquals(NumberType.F64, Pco.peekNumberType(Pco.compress(doubles, new ChunkConfig())));
    assertEquals(null, Pco.peekNumberType(Pco.compress(new double[0], new ChunkConfig())));
  }

  @Test
  void decoderFailsOnlyWithPcoException() {
    // untrusted input: whatever the bytes, decoding returns or throws PcoException, never
    // anything else (the reference decoder does not panic either)
    SplittableRandom r = new SplittableRandom(10);
    byte[] good = Pco.compress(NumberType.F64, Corpus.generate(NumberType.F64, "prices", 2000, 1), new ChunkConfig());
    for (int trial = 0; trial < 20000; trial++) {
      byte[] b = good.clone();
      int flips = 1 + r.nextInt(8);
      for (int i = 0; i < flips; i++) {
        b[r.nextInt(b.length)] = (byte) r.nextInt();
      }
      if (r.nextInt(4) == 0) {
        b = java.util.Arrays.copyOf(b, r.nextInt(b.length + 1));
      }
      NumberType t = NumberType.values()[r.nextInt(NumberType.values().length)];
      try {
        Pco.decompress(b, r.nextInt(3) == 0 ? t : NumberType.F64);
      } catch (PcoException e) {
        // expected
      } catch (RuntimeException e) {
        fail("trial " + trial + ": " + e);
      }
    }
  }
}
