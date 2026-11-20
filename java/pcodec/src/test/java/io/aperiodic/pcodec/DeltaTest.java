package io.aperiodic.pcodec;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;

import java.util.Arrays;
import java.util.SplittableRandom;
import org.junit.jupiter.api.Test;

/** Delta encodings: the reference's vectors, and decode(encode(x)) == x on random latents. */
class DeltaTest {
  @Test
  void consecutiveReferenceVector() {
    long[] orig = {2, 2, 1, 0xffffffffL, 0};
    long[] d = orig.clone();
    long[] moments = Delta.consecutiveEncode(Latent.U32, 2, d, 0, d.length);
    long[] toDecode = {d[2], d[3], d[4], 1337, 1337};
    Delta.consecutiveDecode(Latent.U32, moments, toDecode, 0, 3);
    assertArrayEquals(Arrays.copyOf(orig, 3), Arrays.copyOf(toDecode, 3));
    Delta.consecutiveDecode(Latent.U32, moments, toDecode, 3, 5);
    assertArrayEquals(orig, toDecode);
  }

  @Test
  void consecutiveRoundTrips() {
    SplittableRandom r = new SplittableRandom(1);
    for (Latent lt : new Latent[] {Latent.U8, Latent.U16, Latent.U32, Latent.U64}) {
      for (int trial = 0; trial < 300; trial++) {
        int n = r.nextInt(600);
        int order = r.nextInt(8);
        long[] orig = new long[n];
        for (int i = 0; i < n; i++) {
          orig[i] = lt.fromU64(r.nextLong());
        }
        long[] d = orig.clone();
        long[] moments = Delta.consecutiveEncode(lt, order, d, 0, n);
        int stored = Math.max(n - order, 0);
        long[] dec = new long[n];
        System.arraycopy(d, n - stored, dec, 0, stored);
        // decode in batches of 256, the decoder's way
        for (int s = 0; s < n; s += 256) {
          Delta.consecutiveDecode(lt, moments, dec, s, Math.min(s + 256, n));
        }
        assertArrayEquals(orig, dec);
      }
    }
  }

  @Test
  void lookbackReferenceVectorAndRoundTrips() {
    long[] orig = new long[100];
    Arrays.fill(orig, 100);
    orig[1] = 200;
    orig[2] = 201;
    orig[3] = 202;
    orig[5] = 203;
    orig[15] = 204;
    orig[50] = 205;
    DeltaEncoding config = DeltaEncoding.lookback(4, 1, false);
    assertEquals(16, config.windowN());
    assertEquals(2, config.stateN());
    SplittableRandom r = new SplittableRandom(2);
    for (int trial = 0; trial < 200; trial++) {
      long[] x = trial == 0 ? orig : new long[r.nextInt(700)];
      if (trial > 0) {
        for (int i = 0; i < x.length; i++) {
          x[i] = r.nextInt(6) == 0 ? r.nextLong() & 0xffffffffL : x[Math.max(0, i - 1 - r.nextInt(Math.min(i + 1, 16)))];
        }
      }
      DeltaEncoding cfg = trial == 0 ? config : DeltaEncoding.lookback(4 + r.nextInt(4), r.nextInt(3), false);
      long[] lookbacks = Delta.chooseLookbacks(Latent.U32, cfg, x, 0, x.length);
      long[] enc = x.clone();
      long[] state = Delta.lookbackEncode(Latent.U32, cfg, lookbacks, enc, 0, enc.length);
      int stateN = cfg.stateN();
      long[] window = Delta.newWindowBuffer(cfg, state);
      int[] pos = {cfg.windowN()};
      long[] out = new long[x.length];
      int stored = Math.max(x.length - stateN, 0);
      long[] latents = new long[256];
      long[] lbs = new long[256];
      for (int s = 0; s < x.length; s += 256) {
        int n = Math.min(256, x.length - s);
        Arrays.fill(latents, 0);
        for (int i = 0; i < n; i++) {
          int src = stateN + s + i;
          latents[i] = src < x.length ? enc[src] : 0;
          lbs[i] = s + i < lookbacks.length ? lookbacks[s + i] : 1;
        }
        assertFalse(Delta.lookbackDecode(Latent.U32, cfg, lbs, 256, pos, window, latents, n));
        System.arraycopy(latents, 0, out, s, n);
      }
      assertArrayEquals(x, out, "trial " + trial + " stored " + stored);
    }
  }

  @Test
  void conv1RoundTrips() {
    SplittableRandom r = new SplittableRandom(3);
    for (Latent lt : new Latent[] {Latent.U8, Latent.U16, Latent.U32}) {
      for (int trial = 0; trial < 100; trial++) {
        int order = 1 + r.nextInt(8);
        int n = order + 1 + r.nextInt(1500);
        if (n % 512 != 0 && n % 512 < order) {
          continue; // the reference panics there (PcoPanic)
        }
        long[] x = new long[n];
        double v = r.nextInt(100);
        for (int i = 0; i < n; i++) {
          v += Corpus.gauss(r);
          x[i] = lt.fromU64((long) Math.abs(v * 3) + (lt.mask >> 2));
        }
        DeltaEncoding cfg = Delta.chooseConv1Config(lt, order, x);
        if (cfg == null) {
          continue;
        }
        long[] enc = x.clone();
        long[] state = Delta.conv1Encode(lt, cfg, enc, 0, n);
        long[] dec = new long[n];
        System.arraycopy(enc, order, dec, 0, n - order);
        long[] st = state.clone();
        long[] batch = new long[256];
        for (int s = 0; s < n; s += 256) {
          int k = Math.min(256, n - s);
          System.arraycopy(dec, s, batch, 0, k);
          Delta.conv1Decode(lt, cfg, st, batch, k);
          System.arraycopy(batch, 0, dec, s, k);
        }
        assertArrayEquals(x, dec);
      }
    }
  }
}
