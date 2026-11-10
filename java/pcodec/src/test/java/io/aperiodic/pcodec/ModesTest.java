package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

/** The modes: the reference's unit vectors for int mult, float quant, float mult and validity. */
class ModesTest {
  static long f32(float x) {
    return Float.floatToRawIntBits(x) & 0xffffffffL;
  }

  static long f64(double x) {
    return Double.doubleToRawLongBits(x);
  }

  @Test
  void intMult() {
    Modes.Split s = Modes.IntMult.splitLatents(NumberType.U32, new long[] {8, 1, 5}, 4);
    assertArrayEquals(new long[] {2, 0, 1}, s.primary);
    assertArrayEquals(new long[] {0, 1, 1}, s.secondary);
    assertEquals(0, Modes.IntMult.calcGcd(0, 0));
    assertEquals(1, Modes.IntMult.calcGcd(0, 1));
    assertEquals(1, Modes.IntMult.calcGcd(1, 0));
    assertEquals(2, Modes.IntMult.calcGcd(2, 0));
    assertEquals(1, Modes.IntMult.calcGcd(2, 3));
    assertEquals(3, Modes.IntMult.calcGcd(6, 3));
    assertEquals(6, Modes.IntMult.calcGcd(12, 30));
    assertEquals(4, Modes.IntMult.calcTripleGcd(1, 5, 9));
    assertEquals(3, Modes.IntMult.calcTripleGcd(8, 5, 2));
    assertEquals(0, Modes.IntMult.calcTripleGcd(3, 3, 3));
    assertEquals(5, Modes.IntMult.calcTripleGcd(5, 0, 10));
    assertNull(Modes.IntMult.chooseCandidateBase(new long[] {0, 4, 8}));
    assertEquals(4, Modes.IntMult.chooseCandidateBase(new long[] {0, 4, 8, 10, 14, 18, 20, 24, 28})[0]);
    assertEquals(10, Modes.IntMult.chooseCandidateBase(new long[] {1, 11, 21, 31, 41, 51, 61, 71, 82})[0]);
    assertNull(Modes.IntMult.chooseCandidateBase(new long[] {1, 11, 22, 31, 41, 51, 61, 71, 82}));
    // false position
    assertTrue(Math.abs(Modes.IntMult.solveRootByFalsePosition(x -> x * x - 1.0, -0.9, 2.0) - 1.0) < 1e-4);
    assertTrue(Double.isNaN(Modes.IntMult.solveRootByFalsePosition(x -> x * x, -0.9, 2.0)));
    assertEquals(0.5, Modes.IntMult.solveRootByFalsePosition(x -> 0.0, 0.0, 1.0));
  }

  @Test
  void floatQuantSplitsAsTheReference() {
    float eps = Math.ulp(1.0f);
    long[] nums = {f32(Float.NEGATIVE_INFINITY), f32(-1.0f - eps), f32(-1.0f), f32(-0.0f), f32(0.0f), f32(1.0f), f32(1.0f + eps), f32(Float.POSITIVE_INFINITY)};
    long[] ys = {0b00000000000000111111111111111111L, 0b00000010000000111111111111111111L,
        0b00000010000000111111111111111111L, 0b00000011111111111111111111111111L,
        0b00000100000000000000000000000000L, 0b00000101111111000000000000000000L,
        0b00000101111111000000000000000000L, 0b00000111111111000000000000000000L};
    long[] ms = {0, 1, 0, 0, 0, 0, 1, 0};
    Modes.Split s = Modes.FloatQuant.splitLatents(NumberType.F32, FloatType.F32, nums, 5);
    assertArrayEquals(ys, s.primary);
    assertArrayEquals(ms, s.secondary);
    // estimate_best_k
    long[] sample = new long[22];
    float[] base = {1.0f, 1.25f, -1.5f, 1.75f, -0.875f, 0.75f, 0.625f};
    for (int i = 0; i < 21; i++) {
      sample[i] = f32(base[i % 7]);
    }
    sample[21] = f32(1.0f) + 1;
    double[] kb = Modes.FloatQuant.estimateBestKAndBitsSaved(FloatType.F32, sample);
    assertEquals(21, (int) kb[0]);
    assertTrue(kb[1] < 21.0 && kb[1] > 10.0);
    long[] ones = new long[20];
    java.util.Arrays.fill(ones, f64(1.0));
    double[] kb2 = Modes.FloatQuant.estimateBestKAndBitsSaved(FloatType.F64, ones);
    assertEquals(52, (int) kb2[0]);
    assertEquals(52.0, kb2[1]);
    // compute_bid
    long[] hundred = new long[100];
    for (int i = 0; i < 100; i++) {
      hundred[i] = f32(i);
    }
    Modes.Bid bid = Modes.FloatQuant.computeBid(NumberType.F32, FloatType.F32, hundred);
    assertEquals(Mode.floatQuant(17), bid.mode);
    assertEquals(17.0, bid.bitsSavedPerNum);
    long[] zeroOne = new long[100];
    for (int i = 0; i < 100; i++) {
      zeroOne[i] = f32(i % 2);
    }
    assertNull(Modes.FloatQuant.computeBid(NumberType.F32, FloatType.F32, zeroOne));
  }

  @Test
  void floatMultAsTheReference() {
    FloatType f = FloatType.F64;
    assertEquals(-1, FloatMult.approxPairGcd(f, f64(0.0), f64(0.0)));
    assertEquals(-1, FloatMult.approxPairGcd(f, f64(1.0), f64(0.0)));
    assertEquals(-1, FloatMult.approxPairGcd(f, f64(1.0), f64(1.0)));
    assertEquals(f64(1.0), FloatMult.approxPairGcd(f, f64(1.0), f64(2.0)));
    assertEquals(f64(3.0), FloatMult.approxPairGcd(f, f64(6.0), f64(3.0)));
    assertEquals(f64(0.009999999999999787), FloatMult.approxPairGcd(f, f64(10.01), f64(0.009999999999999787)));
    assertEquals(-1, FloatMult.approxPairGcd(FloatType.F32, f32((float) Math.pow(2, 100)), f32(3.0f)));
    // choose_config_by_trailing_zeros finds base 3
    long[] s = new long[25];
    double[] xs = {0.0, 3.0, 6.0, 21.0, Math.pow(2, 100)};
    for (int i = 0; i < 25; i++) {
      s[i] = f32((float) xs[i % 5]);
    }
    FloatMult.Config c = FloatMult.chooseConfigByTrailingZeros(NumberType.F32, FloatType.F32, s);
    assertNotNull(c);
    assertEquals(f32(3.0f), c.base);
    // choose a mult mode for i * 1.5
    long[] nums = new long[2000];
    for (int i = 0; i < 2000; i++) {
      nums[i] = f64(i * 1.5);
    }
    Modes.Bid bid = Modes.chooseWinningBid(Modes.chooseModeBids(NumberType.F64, nums, new ChunkConfig()));
    assertEquals(Mode.floatMult(NumberType.F64.toLatentOrdered(f64(1.5))), bid.mode);
    // and a quant mode for numbers with 20 zero low bits
    long[] q = new long[1000];
    for (int i = 0; i < 1000; i++) {
      q[i] = f64(1.0) + ((long) i << 20);
    }
    assertEquals(Mode.floatQuant(20), Modes.chooseWinningBid(Modes.chooseModeBids(NumberType.F64, q, new ChunkConfig())).mode);
  }

  @Test
  void modeValidity() {
    assertTrue(Modes.modeIsValid(NumberType.U32, Mode.CLASSIC));
    assertTrue(Modes.modeIsValid(NumberType.U32, Mode.dict(new long[] {1, 3})));
    for (long b : new long[] {1, 77, 0xffffffffL}) {
      assertTrue(Modes.modeIsValid(NumberType.U32, Mode.intMult(b)));
    }
    assertFalse(Modes.modeIsValid(NumberType.U32, Mode.intMult(0)));
    assertFalse(Modes.modeIsValid(NumberType.U32, Mode.floatQuant(3)));
    for (float b : new float[] {0.777f, 0.000000000000000000000000000000000000003416741f}) {
      assertTrue(Modes.modeIsValid(NumberType.F32, Mode.floatMult(NumberType.F32.toLatentOrdered(f32(b)))));
    }
    for (float b : new float[] {0.0f, -0.0f, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY, Float.NaN}) {
      assertFalse(Modes.modeIsValid(NumberType.F32, Mode.floatMult(NumberType.F32.toLatentOrdered(f32(b)))));
    }
    for (int k : new int[] {1, 22, 23}) {
      assertTrue(Modes.modeIsValid(NumberType.F32, Mode.floatQuant(k)));
    }
    for (int k : new int[] {0, 24, 32}) {
      assertFalse(Modes.modeIsValid(NumberType.F32, Mode.floatQuant(k)));
    }
    assertFalse(Modes.modeIsValid(NumberType.F32, Mode.intMult(77)));
  }

  @Test
  void orderedLatents() {
    assertEquals(0, NumberType.I32.toLatentOrdered(0x80000000L));
    assertEquals(0x7fffffffL, NumberType.I32.toLatentOrdered(0xffffffffL));
    assertEquals(0x80000000L, NumberType.I32.toLatentOrdered(0));
    assertEquals(0xffffffffL, NumberType.I32.toLatentOrdered(0x7fffffffL));
    assertTrue(NumberType.F32.toLatentOrdered(f32(Float.NEGATIVE_INFINITY)) < NumberType.F32.toLatentOrdered(f32(-0.0f)));
    assertTrue(NumberType.F32.toLatentOrdered(f32(-0.0f)) < NumberType.F32.toLatentOrdered(f32(0.0f)));
    assertTrue(NumberType.F32.toLatentOrdered(f32(0.0f)) < NumberType.F32.toLatentOrdered(f32(Float.POSITIVE_INFINITY)));
  }

  @Test
  void dictUsesAForcedOrderAndRejectsOneThatIsNotTheReferencesSort() {
    // counts: 7 x3, 5 x2, 9 x2, 1 x1; the reference may put 5 and 9 either way round
    long[] nums = {7, 5, 9, 7, 1, 9, 5, 7};
    NumberType t = NumberType.U32;
    long l7 = t.toLatentOrdered(7), l5 = t.toLatentOrdered(5), l9 = t.toLatentOrdered(9), l1 = t.toLatentOrdered(1);
    assertArrayEquals(new long[] {l7, l5, l9, l1}, Modes.Dict.computeBid(t, nums).mode.dict);
    long[][] bad = {
      {l7, l9, l5}, // another size
      {l7, l9, l5, l1, l1},
      {l5, l7, l9, l1}, // counts not descending
      {l7, l9, l1, l5},
      {l7, l9, l9, l1}, // not a permutation
      {l7, l9, l5, l7 + 1000}, // a value not in the data
    };
    try {
      Modes.Dict.FORCED_ORDER.set(new long[] {l7, l9, l5, l1});
      Modes.Bid b = Modes.Dict.computeBid(t, nums);
      assertArrayEquals(new long[] {l7, l9, l5, l1}, b.mode.dict);
      assertArrayEquals(new long[] {0, 2, 1, 0, 3, 1, 2, 0}, b.split.split(nums).primary);
      for (long[] f : bad) {
        Modes.Dict.FORCED_ORDER.set(f);
        assertThrows(IllegalArgumentException.class, () -> Modes.Dict.computeBid(t, nums), java.util.Arrays.toString(f));
      }
    } finally {
      Modes.Dict.FORCED_ORDER.remove();
    }
    assertArrayEquals(new long[] {l7, l5, l9, l1}, Modes.Dict.computeBid(t, nums).mode.dict);
  }
}
