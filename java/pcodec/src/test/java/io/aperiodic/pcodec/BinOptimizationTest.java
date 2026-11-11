package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;

/** Bin optimization: the reference's test vectors. */
class BinOptimizationTest {
  static List<Histogram.Bin> bins(long[][] b) {
    List<Histogram.Bin> res = new ArrayList<>();
    for (long[] x : b) {
      res.add(new Histogram.Bin(x[0], x[1], x[2]));
    }
    return res;
  }

  static String infos(List<BinOptimization.BinInfo> infos) {
    StringBuilder sb = new StringBuilder();
    for (BinOptimization.BinInfo i : infos) {
      sb.append('(').append(i.weight).append(',').append(i.lower).append(',').append(i.upper).append(',')
          .append(i.offsetBits).append(',').append(i.symbol).append(')');
    }
    return sb.toString();
  }

  @Test
  void referenceVectors() {
    List<Histogram.Bin> b =
        bins(new long[][] {{100, 1, 16}, {100, 33, 48}, {100, 49, 64}, {100, 65, 74}, {50, 75, 79}});
    assertEquals("(100,1,16,4,0)(200,33,64,5,1)(150,65,79,4,2)", infos(BinOptimization.optimizeBins(Latent.U32, b, 10)));
    List<Histogram.Bin> enveloped = bins(new long[][] {{1000, 0, 150}, {1000, 200, 200}});
    assertEquals("(1000,0,150,8,0)(1000,200,200,0,1)", infos(BinOptimization.optimizeBins(Latent.U32, enveloped, 10)));
  }

  @Test
  void log2ApproxIsExactAtPowersOfTwoAndMonotone() {
    for (int e = 0; e < 32; e++) {
      assertEquals((float) e, BinOptimization.log2Approx((float) (1L << e)));
    }
    float prev = Float.NEGATIVE_INFINITY;
    for (int i = 1; i <= 100; i++) {
      float a = BinOptimization.log2Approx(i);
      assertTrue(a >= prev);
      assertTrue(Math.abs(a - (float) (Math.log(i) / Math.log(2))) < 0.0076f);
      prev = a;
    }
  }

  @Test
  void trainedBinsAscendByLowerBound() {
    // ChunkCompressor relies on this instead of sorting them as the reference does
    java.util.SplittableRandom r = new java.util.SplittableRandom(11);
    for (int round = 0; round < 300; round++) {
      int n = 1 + r.nextInt(5000);
      long[] latents = new long[n];
      int shape = r.nextInt(4);
      for (int i = 0; i < n; i++) {
        latents[i] = shape == 0 ? r.nextLong() : shape == 1 ? r.nextInt(17) : shape == 2 ? (long) r.nextInt(2_000_000) + (1L << 40) : (i % 3) * 1000L;
      }
      BinOptimization.Trained t = BinOptimization.trainInfos(Latent.U64, latents, 1 + r.nextInt(8));
      for (int i = 1; i < t.infos.size(); i++) {
        assertTrue(Long.compareUnsigned(t.infos.get(i - 1).lower, t.infos.get(i).lower) < 0, "bins out of order");
      }
    }
  }
}
