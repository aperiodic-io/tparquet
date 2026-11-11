package io.aperiodic.pcodec;

/** {@code pco::delta}: consecutive, lookback and conv1 delta encodings. */
final class Delta {
  private Delta() {}

  static final int ENCODING_LOOKBACK_MAX_WINDOW_N_LOG = 15;
  static final int ENCODING_LOOKBACK_MIN_WINDOW_N_LOG = 4;
  static final int FULL_BATCH_N = 256;

  static void toggleCenter(Latent lt, long[] v, int from, int to) {
    for (int i = from; i < to; i++) {
      v[i] = lt.toggleCenter(v[i]);
    }
  }

  static DeltaEncoding newLookback(long n) {
    // bits_to_encode_offset(n as u32 - 1)
    int x = (int) n - 1;
    int bits = 32 - Integer.numberOfLeadingZeros(x);
    int w = Math.max(ENCODING_LOOKBACK_MIN_WINDOW_N_LOG, Math.min(ENCODING_LOOKBACK_MAX_WINDOW_N_LOG, bits));
    return DeltaEncoding.lookback(w, 0, false);
  }

  /** {@code encode_in_place} on {@code v[from, to)}; returns the page's delta state */
  static long[] encodeInPlace(
      Latent lt, DeltaEncoding enc, long[] lookbacks, long[] v, int from, int to) {
    switch (enc.kind) {
      case NO_OP:
        return new long[0];
      case CONSECUTIVE:
        return consecutiveEncode(lt, enc.order, v, from, to);
      case LOOKBACK:
        return lookbackEncode(lt, enc, lookbacks, v, from, to);
      default:
        return conv1Encode(lt, enc, v, from, to);
    }
  }

  // ------------------------------------------------------------ consecutive

  static long[] consecutiveEncode(Latent lt, int order, long[] v, int from, int to) {
    long[] moments = new long[order];
    int start = from;
    for (int k = 0; k < order; k++) {
      moments[k] = start < to ? v[start] : 0;
      for (int i = to - 1; i > start; i--) {
        v[i] = lt.sub(v[i], v[i - 1]);
      }
      start += Math.min(to - start, 1);
    }
    toggleCenter(lt, v, start, to);
    return moments;
  }

  static void consecutiveDecode(Latent lt, long[] moments, long[] v, int from, int to) {
    toggleCenter(lt, v, from, to);
    for (int m = moments.length - 1; m >= 0; m--) {
      long moment = moments[m];
      for (int i = from; i < to; i++) {
        long tmp = v[i];
        v[i] = moment;
        moment = lt.add(moment, tmp);
      }
      moments[m] = moment;
    }
  }

  // ------------------------------------------------------------ lookback

  private static final int PROPOSED_LOOKBACKS = 16;
  private static final int BRUTE_LOOKBACKS = 6;
  private static final int REPEATING_LOOKBACKS = 4;
  private static final int[] COARSENESSES = {0, 8};

  private static int hashFn(long x, int hashMask) {
    x = (x ^ (x >>> 32)) * 0x9e3779b97f4a7801L; // 11400714819323197441
    x = x ^ (x >>> 32);
    return (int) x & hashMask;
  }

  /** {@code choose_lookbacks} on {@code v[from, to)}: u32 lookbacks, one per latent past the state */
  static long[] chooseLookbacks(Latent lt, DeltaEncoding config, long[] v, int from, int to) {
    int stateN = config.stateN();
    int len = to - from;
    if (len <= stateN) {
      return new long[0];
    }
    int hashTableN = 1 << (config.windowNLog + 1);
    int windowN = config.windowN();
    if (windowN < PROPOSED_LOOKBACKS) {
      throw new IllegalStateException("we do not support tiny windows during compression");
    }
    int nLookbacks = Math.min(windowN, len);
    int[] lookbackCounts = new int[nLookbacks];
    int[] lookbackGoodnesses = new int[nLookbacks];
    java.util.Arrays.fill(lookbackCounts, 1);
    java.util.Arrays.fill(lookbackGoodnesses, 1);
    long[] lookbacks = new long[len - stateN];
    long[] idxHashTable = new long[COARSENESSES.length * hashTableN];
    long[] proposed = new long[PROPOSED_LOOKBACKS];
    for (int i = 0; i < PROPOSED_LOOKBACKS; i++) {
      proposed[i] = Math.min(i + 1, stateN);
    }
    long bestLookback = 1;
    long repeatingIdx = 0;
    int hashMask = hashTableN - 1;
    for (int i = stateN; i < len; i++) {
      long l = v[from + i];
      int newBrute = Math.min(i, PROPOSED_LOOKBACKS);
      proposed[newBrute - 1] = newBrute;

      // hash_lookup
      int proposalIdx = BRUTE_LOOKBACKS + REPEATING_LOOKBACKS;
      int offset = 0;
      for (int coarseness : COARSENESSES) {
        long bucket = l >>> coarseness;
        int h0 = hashFn(bucket - 1, hashMask);
        int h1 = hashFn(bucket, hashMask);
        int h2 = hashFn(bucket + 1, hashMask);
        int[] hs = {h0, h1, h2};
        for (int h : hs) {
          long lookbackToLast = i - idxHashTable[offset + h];
          proposed[proposalIdx] =
              lookbackToLast <= windowN ? lookbackToLast : Math.min(proposalIdx, i);
          proposalIdx++;
        }
        idxHashTable[offset + h1] = i;
        offset += hashTableN;
      }

      // find_best_lookback
      int bestGoodness = 0;
      long newBest = 0;
      for (long lookback : proposed) {
        int lookbackGoodness = lookbackGoodnesses[(int) lookback - 1];
        long other = v[from + i - (int) lookback];
        long delta = Latent.min(lt.sub(l, other), lt.sub(other, l));
        int goodness = lookbackGoodness + lt.leadingZeros(delta);
        if (goodness > bestGoodness) {
          bestGoodness = goodness;
          newBest = lookback;
        }
      }
      if (newBest != bestLookback) {
        repeatingIdx++;
      }
      proposed[BRUTE_LOOKBACKS + (int) (repeatingIdx % REPEATING_LOOKBACKS)] = newBest;
      bestLookback = newBest;
      lookbacks[i - stateN] = bestLookback;
      int c = ++lookbackCounts[(int) bestLookback - 1];
      if ((c & (c - 1)) == 0) {
        lookbackGoodnesses[(int) bestLookback - 1]++;
      }
    }
    return lookbacks;
  }

  static long[] lookbackEncode(
      Latent lt, DeltaEncoding config, long[] lookbacks, long[] v, int from, int to) {
    int stateN = config.stateN();
    int len = to - from;
    int realStateN = Math.min(len, stateN);
    for (int i = len - 1; i >= realStateN; i--) {
      int lookback = (int) lookbacks[i - stateN];
      v[from + i] = lt.sub(v[from + i], v[from + i - lookback]);
    }
    long[] state = new long[stateN];
    System.arraycopy(v, from, state, stateN - realStateN, realStateN);
    toggleCenter(lt, v, from, to);
    return state;
  }

  /** the working window buffer of a lookback decoder; its position is stored at index 0 of pos */
  static long[] newWindowBuffer(DeltaEncoding config, long[] state) {
    int windowN = config.windowN();
    int bufferN = Math.max(windowN, FULL_BATCH_N) * 2;
    long[] res = new long[bufferN];
    System.arraycopy(state, 0, res, windowN - state.length, state.length);
    return res;
  }

  /** {@code lookback::decode_in_place}; returns whether a lookback was out of the window */
  static boolean lookbackDecode(
      Latent lt,
      DeltaEncoding config,
      long[] lookbacks,
      int nLookbacks,
      int[] windowPos,
      long[] window,
      long[] latents,
      int batchN) {
    toggleCenter(lt, latents, 0, batchN);
    int windowN = config.windowN();
    int stateN = config.stateN();
    int startPos = windowPos[0];
    if (startPos + batchN > window.length) {
      System.arraycopy(window, startPos - windowN, window, 0, windowN);
      startPos = windowN;
    }
    boolean oob = false;
    int n = Math.min(batchN, nLookbacks);
    for (int i = 0; i < n; i++) {
      int pos = startPos + i;
      long lb = lookbacks[i];
      int lookback;
      if (Long.compareUnsigned(lb, windowN) <= 0) {
        lookback = (int) lb;
      } else {
        oob = true;
        lookback = 1;
      }
      window[pos] = lt.add(latents[i], window[pos - lookback]);
    }
    int endPos = startPos + batchN;
    System.arraycopy(window, startPos - stateN, latents, 0, batchN);
    windowPos[0] = endPos;
    return oob;
  }

  // ------------------------------------------------------------ conv1

  private static final int ENCODE_BATCH_SIZE = 512;
  private static final double L2_REGULARIZATION = 0.1;

  private static long predictOne(
      Latent lt, long[] v, int at, long[] weights, long bias, int quantization) {
    int cb = lt.convBits;
    long s = bias;
    for (int k = 0; k < weights.length; k++) {
      s = DeltaEncoding.Conv.wrap(s + DeltaEncoding.Conv.wrap(weights[k] * v[at + k], cb), cb);
    }
    return lt.fromConv(Math.max(s, 0) >> quantization);
  }

  static long[] conv1Encode(Latent lt, DeltaEncoding config, long[] v, int from, int to) {
    long bias = config.conv1Bias(lt.convBits);
    long[] weights = config.conv1Weights(lt.convBits);
    int order = weights.length;
    if (to - from < order) {
      throw new IndexOutOfBoundsException("conv1 page shorter than its order");
    }
    long[] initialState = new long[order];
    System.arraycopy(v, from, initialState, 0, order);
    long[] predictions = new long[ENCODE_BATCH_SIZE + order];
    int len = to - from;
    int start = 0;
    while (start < len) {
      int end = Math.min(start + ENCODE_BATCH_SIZE, len);
      int remaining = len - start;
      if (remaining < order) {
        // the reference slices latents[i..i + order] here and panics
        throw new PcoPanic("conv1 batch of " + remaining + " latents is shorter than order " + order);
      }
      int nPred = Math.min(ENCODE_BATCH_SIZE, Math.max(remaining - order, 0) + 1);
      for (int i = 0; i < nPred; i++) {
        predictions[order + i] = predictOne(lt, v, from + start + i, weights, bias, config.quantization);
      }
      for (int i = 0; i < end - start; i++) {
        int idx = from + start + i;
        v[idx] = lt.add(lt.sub(v[idx], predictions[i]), lt.mid);
      }
      for (int i = 0; i < order; i++) {
        predictions[i] = predictions[ENCODE_BATCH_SIZE + i];
      }
      start = end;
    }
    return initialState;
  }

  static void conv1Decode(Latent lt, DeltaEncoding config, long[] state, long[] latents, int n) {
    long[] weights = config.conv1Weights(lt.convBits);
    long bias = config.conv1Bias(lt.convBits);
    int order = weights.length;
    toggleCenter(lt, latents, 0, n);
    long[] residuals = new long[n + order];
    System.arraycopy(state, 0, residuals, 0, order);
    System.arraycopy(latents, 0, residuals, order, n);
    for (int i = order; i < residuals.length; i++) {
      residuals[i] =
          lt.add(residuals[i], predictOne(lt, residuals, i - order, weights, bias, config.quantization));
    }
    System.arraycopy(residuals, 0, latents, 0, n);
    System.arraycopy(residuals, n, state, 0, order);
  }

  /** column-major matrix */
  private static final class Matrix {
    final double[] data;
    final int h;
    final int w;

    Matrix(int h, int w) {
      this.data = new double[h * w];
      this.h = h;
      this.w = w;
    }

    double get(int i, int j) {
      return data[i + j * h];
    }

    void set(int i, int j, double x) {
      data[i + j * h] = x;
    }

    Matrix intoCholesky() {
      // the upper triangle, which the reference zeroes, is never read
      for (int j = 0; j < h; j++) {
        double s = 0.0;
        for (int k = 0; k < j; k++) {
          double value = get(j, k);
          s = Math.fma(value, value, s);
        }
        double diag = Math.sqrt(RustMath.maxF64(get(j, j) - s, 0.0));
        set(j, j, diag);
        double scale = diag == 0.0 ? 0.0 : 1.0 / diag;
        for (int i = j + 1; i < h; i++) {
          double s2 = 0.0;
          for (int k = 0; k < j; k++) {
            s2 = Math.fma(get(i, k), get(j, k), s2);
          }
          set(i, j, scale * (get(i, j) - s2));
        }
      }
      return this;
    }

    Matrix transposedBackwardSubInto(Matrix y) {
      for (int k = 0; k < y.w; k++) {
        for (int j = h - 1; j >= 0; j--) {
          double diag = y.get(j, k) / get(j, j);
          y.set(j, k, diag);
          for (int i = 0; i < j; i++) {
            y.set(i, k, y.get(i, k) - diag * get(j, i));
          }
        }
      }
      return y;
    }

    Matrix forwardSubInto(Matrix y) {
      for (int k = 0; k < y.w; k++) {
        for (int j = 0; j < h; j++) {
          double diag = y.get(j, k) / get(j, j);
          y.set(j, k, diag);
          for (int i = j + 1; i < h; i++) {
            y.set(i, k, y.get(i, k) - diag * get(i, j));
          }
        }
      }
      return y;
    }
  }

  private static double[] buildInitialAutocovDots(double[] v, int order) {
    int n = v.length;
    double[] dots = new double[order + 1];
    int almostN = (n - order) / ENCODE_BATCH_SIZE * ENCODE_BATCH_SIZE;
    for (int start = 0; start < almostN; start += ENCODE_BATCH_SIZE) {
      for (int sep = 0; sep < order + 1; sep++) {
        double d0 = 0.0;
        double d1 = 0.0;
        double d2 = 0.0;
        double d3 = 0.0;
        for (int i = start; i < start + ENCODE_BATCH_SIZE; i += 4) {
          d0 += v[i] * v[i + sep];
          d1 += v[i + 1] * v[i + sep + 1];
          d2 += v[i + 2] * v[i + sep + 2];
          d3 += v[i + 3] * v[i + sep + 3];
        }
        dots[sep] += (d0 + d1) + (d2 + d3);
      }
    }
    for (int i = almostN; i < n - order; i++) {
      for (int sep = 0; sep < order + 1; sep++) {
        dots[sep] += v[i] * v[i + sep];
      }
    }
    return dots;
  }

  private static Matrix[] buildAutocovMats(double[] v, int order, double regularization) {
    int n = v.length;
    double initialSum = -0.0;
    for (int i = 0; i < n - order; i++) {
      initialSum += v[i];
    }
    double[] initialDots = buildInitialAutocovDots(v, order);
    Matrix xtx = new Matrix(order + 1, order + 1);
    Matrix xty = new Matrix(order + 1, 1);
    // xtx is symmetric, and only its lower triangle is ever read (here, by intoCholesky and by
    // the substitutions), so only that is written; the reference writes both
    for (int i = 0; i < order; i++) {
      xtx.set(i, 0, initialDots[i]);
    }
    xtx.set(order, 0, initialSum);
    xty.set(0, 0, initialDots[order]);
    for (int i = 1; i < order; i++) {
      for (int j = 1; j <= i; j++) {
        double lastDot = xtx.get(i - 1, j - 1);
        double dot = lastDot + (v[n - order + i - 1] * v[n - order + j - 1] - v[i - 1] * v[j - 1]);
        xtx.set(i, j, dot);
      }
      double lastSum = xtx.get(order, i - 1);
      double sum = lastSum + (v[n - order + i - 1] - v[i - 1]);
      xtx.set(order, i, sum);
    }
    for (int i = 1; i < order; i++) {
      double lastDot = xtx.get(order - 1, i - 1);
      double dot = lastDot + (v[n - order + i - 1] * v[n - 1] - v[i - 1] * v[order - 1]);
      xty.set(i, 0, dot);
    }
    xtx.set(order, order, (double) (n - order));
    double lastSum = xtx.get(order, order - 1);
    double sum = lastSum + (v[n - 1] - v[order - 1]);
    xty.set(order, 0, sum);
    for (int i = 0; i < order + 1; i++) {
      xtx.set(i, i, xtx.get(i, i) + regularization);
    }
    return new Matrix[] {xtx, xty};
  }

  /** {@code conv1::choose_config}, or null */
  static DeltaEncoding chooseConv1Config(Latent lt, int order, long[] latents) {
    if (latents.length < order + 1) {
      return null;
    }
    long center = SortUtils.choosePivot(latents, 0, latents.length);
    double[] v = new double[latents.length];
    for (int i = 0; i < v.length; i++) {
      long x = latents[i];
      v[i] = Latent.lt(x, center) ? -Latent.toF64(lt.sub(center, x)) : Latent.toF64(lt.sub(x, center));
    }
    Matrix[] mats = buildAutocovMats(v, order, L2_REGULARIZATION);
    Matrix chol = mats[0].intoCholesky();
    double[] fw = chol.transposedBackwardSubInto(chol.forwardSubInto(mats[1])).data;
    double totalWeight = 0.0;
    double totalAbsWeight = 0.0;
    for (int k = 0; k < order; k++) {
      totalAbsWeight += Math.abs(fw[k]);
      totalWeight += fw[k];
    }
    if (!Double.isFinite(totalWeight) || !Double.isFinite(totalAbsWeight)) {
      return null;
    }
    double floatBias = ((1.0 - totalWeight) * Latent.toF64(center)) + fw[order];
    double convMax = (double) (lt.convBits == 64 ? Long.MAX_VALUE : (1L << (lt.convBits - 1)) - 1);
    double ratio = convMax / (totalAbsWeight * Latent.toF64(lt.max()) + Math.abs(floatBias) + 1.0);
    int q = RustMath.f64ToI32(Math.floor(GlibcMath.log2(ratio))) - 1;
    q = Math.min(q, DeltaEncoding.MAX_CONV1_QUANTIZATION);
    q = Math.min(q, lt.convBits - 1);
    if (q < 0) {
      return null;
    }
    double quantizeFactor = Math.scalb(1.0, q);
    long[] weights = new long[order];
    for (int k = 0; k < order; k++) {
      weights[k] = RustMath.f64ToI64(RustMath.roundF64(fw[k] * quantizeFactor));
    }
    long bias = RustMath.f64ToI64(floatBias * quantizeFactor);
    return DeltaEncoding.conv1(q, bias, weights);
  }
}
