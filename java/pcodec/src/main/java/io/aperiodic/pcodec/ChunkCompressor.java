package io.aperiodic.pcodec;

import java.util.ArrayList;
import java.util.List;

/** {@code pco::wrapped::ChunkCompressor}: one chunk, split into pages, ready to write. */
final class ChunkCompressor {
  static final int FULL_BATCH_N = 256;
  static final int MAX_ENTRIES = 1 << 24;
  private static final float LOOKBACK_REQUIRED_BYTE_SAVINGS_PER_N = 0.25f;
  private static final int LIMITED_UNOPTIMIZED_BINS_LOG = 6;

  // ------------------------------------------------------------ latent compressor

  /** {@code ChunkLatentCompressor} */
  static final class LatentCompressor {
    final Latent lt;
    // CompressionTable
    final int searchSizeLog;
    final long[] searchLowers;
    final BinOptimization.BinInfo[] infos;
    final Ans.Encoder encoder;
    final double avgBitsPerLatent;
    final boolean isTrivial;
    final boolean needsAns;
    final long[] latents;
    final long[] scratchLowers = new long[FULL_BATCH_N];
    final int[] scratchSymbols = new int[FULL_BATCH_N];

    LatentCompressor(Latent lt, BinOptimization.Trained trained, ChunkMeta.Bin[] bins, long[] latents) {
      this.lt = lt;
      this.needsAns = bins.length != 1;
      // the reference sorts the bins by lower bound; trainInfos gives them in that order already
      // (BinOptimizationTest)
      this.infos = trained.infos.toArray(new BinOptimization.BinInfo[0]);
      this.searchSizeLog =
          infos.length <= 1 ? 0 : 1 + (31 - Integer.numberOfLeadingZeros(infos.length - 1));
      this.searchLowers = new long[Math.max(1 << searchSizeLog, infos.length)];
      for (int i = 0; i < searchLowers.length; i++) {
        searchLowers[i] = i < infos.length ? infos[i].lower : lt.max();
      }
      int[] weights = new int[bins.length];
      for (int i = 0; i < bins.length; i++) {
        weights[i] = bins[i].weight;
      }
      this.encoder = new Ans.Encoder(Ans.Spec.fromWeights(trained.ansSizeLog, weights));
      this.avgBitsPerLatent = avgBitsPerLatent(bins, trained.ansSizeLog);
      this.isTrivial = bins.length == 0 || (bins.length == 1 && bins[0].offsetBits == 0);
      this.latents = latents;
      long defaultLower = infos.length == 1 ? infos[0].lower : 0;
      java.util.Arrays.fill(scratchLowers, defaultLower);
    }

    static double avgBitsPerLatent(ChunkMeta.Bin[] bins, int ansSizeLog) {
      double totalWeight = (double) (1 << ansSizeLog);
      double s = -0.0;
      for (ChunkMeta.Bin bin : bins) {
        double w = RustMath.u64ToF64(bin.weight & 0xffffffffL);
        double ansBits = ansSizeLog - GlibcMath.log2(w);
        s += (ansBits + bin.offsetBits) * w / totalWeight;
      }
      return s;
    }

    /** a page's ANS values and offsets ({@code PageDissectedVar}) */
    static final class Dissected {
      final int[] ansVals;
      final int[] ansBits;
      final long[] offsets;
      final int[] offsetBits;
      final int[] ansFinalStates = new int[ChunkMeta.ANS_INTERLEAVING];

      Dissected(int n, int defaultState) {
        ansVals = new int[n];
        ansBits = new int[n];
        offsets = new long[n];
        offsetBits = new int[n];
        java.util.Arrays.fill(ansFinalStates, defaultState);
      }
    }

    Dissected dissectPage(int start, int end) {
      if (isTrivial) {
        return new Dissected(0, encoder.defaultState());
      }
      int pageN = end - start;
      Dissected d = new Dissected(pageN, encoder.defaultState());
      int nBatches = (pageN + FULL_BATCH_N - 1) / FULL_BATCH_N;
      int[] searchIdxs = new int[FULL_BATCH_N];
      for (int b = nBatches - 1; b >= 0; b--) {
        int rs = b * FULL_BATCH_N;
        int re = Math.min((b + 1) * FULL_BATCH_N, pageN);
        int batchN = re - rs;
        // binary search, one element at a time through all depths (the reference goes depth by
        // depth over the batch; each element's path is the same either way), so the index and
        // the latent stay in registers. Indices from batchN on are left as they are: only the
        // batch's are read.
        for (int i = 0; i < batchN; i++) {
          long x = latents[start + rs + i];
          int idx = 0;
          for (int bisection = (1 << searchSizeLog) >>> 1; bisection > 0; bisection >>>= 1) {
            if (!Latent.lt(x, searchLowers[idx + bisection])) {
              idx += bisection;
            }
          }
          searchIdxs[i] = idx;
        }
        if (infos.length < (1 << searchSizeLog)) {
          for (int i = 0; i < FULL_BATCH_N; i++) {
            searchIdxs[i] = Math.min(searchIdxs[i], infos.length - 1);
          }
        }
        // dissect bins
        if (infos.length <= 1) {
          int obits = infos.length == 1 ? infos[0].offsetBits : 0;
          for (int i = 0; i < batchN; i++) {
            d.offsetBits[rs + i] = obits;
          }
        } else {
          for (int i = 0; i < batchN; i++) {
            BinOptimization.BinInfo info = infos[searchIdxs[i]];
            scratchLowers[i] = info.lower;
            scratchSymbols[i] = info.symbol;
            d.offsetBits[rs + i] = info.offsetBits;
          }
        }
        // offsets
        for (int i = 0; i < batchN; i++) {
          d.offsets[rs + i] = lt.sub(latents[start + rs + i], scratchLowers[i]);
        }
        encodeAnsInReverse(d, rs, batchN);
      }
      return d;
    }

    private void encodeAnsInReverse(Dissected d, int rs, int len) {
      if (encoder.sizeLog == 0) {
        for (int i = 0; i < len; i++) {
          d.ansBits[rs + i] = 0;
        }
        return;
      }
      int finalBaseI = (len / ChunkMeta.ANS_INTERLEAVING) * ChunkMeta.ANS_INTERLEAVING;
      int finalJ = len % ChunkMeta.ANS_INTERLEAVING;
      for (int j = finalJ - 1; j >= 0; j--) {
        encodeOne(d, rs, finalBaseI + j, j);
      }
      for (int baseI = finalBaseI - ChunkMeta.ANS_INTERLEAVING; baseI >= 0; baseI -= ChunkMeta.ANS_INTERLEAVING) {
        for (int j = ChunkMeta.ANS_INTERLEAVING - 1; j >= 0; j--) {
          encodeOne(d, rs, baseI + j, j);
        }
      }
    }

    private void encodeOne(Dissected d, int rs, int i, int j) {
      int state = d.ansFinalStates[j];
      long r = encoder.encode(state, scratchSymbols[i]);
      int bitlen = (int) (r >>> 32);
      d.ansVals[rs + i] = state & ((1 << bitlen) - 1);
      d.ansBits[rs + i] = bitlen;
      d.ansFinalStates[j] = (int) r;
    }

    void writeDissectedBatch(Dissected d, int batchStart, BitWriter w) {
      if (batchStart >= d.offsets.length) {
        return;
      }
      int end = Math.min(batchStart + FULL_BATCH_N, d.offsets.length);
      w.writeBatch(d.ansVals, d.ansBits, d.offsets, d.offsetBits, batchStart, end, needsAns);
    }
  }

  // ------------------------------------------------------------ chunk compressor

  /** a page's range of each latent variable, and its delta state */
  static final class PageInfo {
    final int pageN;
    final int[] start = new int[3];
    final int[] end = new int[3];
    final long[][] deltaState = new long[3][];
    final boolean[] present = new boolean[3];

    PageInfo(int pageN) {
      this.pageN = pageN;
    }
  }

  final ChunkMeta meta;
  final LatentCompressor[] clcs;
  final List<PageInfo> pageInfos;

  private ChunkCompressor(ChunkMeta meta, LatentCompressor[] clcs, List<PageInfo> pageInfos) {
    this.meta = meta;
    this.clcs = clcs;
    this.pageInfos = pageInfos;
  }

  /** latents of all three variables, after delta encoding, and the page infos */
  private static final class Encoded {
    final long[][] latents;
    final List<PageInfo> pageInfos;

    Encoded(long[][] latents, List<PageInfo> pageInfos) {
      this.latents = latents;
      this.pageInfos = pageInfos;
    }
  }

  private static Encoded deltaEncodeAndBuildPageInfos(
      Latent primaryLatent,
      Latent numberLatent,
      DeltaEncoding delta,
      long[] nPerPage,
      long[] primary,
      long[] secondary) {
    long[][] latents = {null, primary, secondary};
    Latent[] types = {Latent.U32, primaryLatent, numberLatent};
    List<PageInfo> pageInfos = new ArrayList<>(nPerPage.length);
    int n = primary.length;
    long[] deltaLatents = delta.latentType() != null ? new long[n] : null;
    int deltaLen = 0;
    int start = 0;
    for (long pageNL : nPerPage) {
      int pageN = (int) pageNL;
      int end = start + pageN;
      long[] pageDelta = null;
      if (delta.kind == DeltaEncoding.Kind.LOOKBACK) {
        pageDelta = Delta.chooseLookbacks(types[1], delta, primary, start, end);
      }
      PageInfo info = new PageInfo(pageN);
      for (int key = 1; key <= 2; key++) {
        if (latents[key] == null) {
          continue;
        }
        DeltaEncoding enc = delta.forLatentVar(key);
        info.present[key] = true;
        info.deltaState[key] = Delta.encodeInPlace(types[key], enc, pageDelta, latents[key], start, end);
        info.start[key] = Math.min(start + enc.nLatentsPerState(), end);
        info.end[key] = end;
      }
      if (deltaLatents != null) {
        info.present[0] = true;
        info.deltaState[0] = new long[0];
        info.start[0] = deltaLen;
        info.end[0] = deltaLen + pageDelta.length;
        System.arraycopy(pageDelta, 0, deltaLatents, deltaLen, pageDelta.length);
        deltaLen += pageDelta.length;
      }
      pageInfos.add(info);
      start = end;
    }
    if (deltaLatents != null) {
      latents[0] = java.util.Arrays.copyOf(deltaLatents, deltaLen);
    }
    return new Encoded(latents, pageInfos);
  }

  /** a candidate and its bin counts per latent variable */
  private static final class Candidate {
    final ChunkCompressor cc;
    final int[][] binCounts;

    Candidate(ChunkCompressor cc, int[][] binCounts) {
      this.cc = cc;
      this.binCounts = binCounts;
    }
  }

  private static Candidate newCandidate(
      Latent numberLatent,
      long[] primary,
      long[] secondary,
      PagingSpec paging,
      Mode mode,
      DeltaEncoding delta,
      int unoptimizedBinsLog) {
    long[] nPerPage = paging.nPerPage(primary.length);
    Latent[] types = {Latent.U32, mode.primaryLatentType(numberLatent), numberLatent};
    Encoded enc =
        deltaEncodeAndBuildPageInfos(types[1], numberLatent, delta, nPerPage, primary, secondary);
    ChunkMeta.LatentVar[] vars = new ChunkMeta.LatentVar[3];
    LatentCompressor[] clcs = new LatentCompressor[3];
    int[][] binCounts = new int[3][];
    for (int key = 0; key < 3; key++) {
      long[] latents = enc.latents[key];
      if (latents == null) {
        continue;
      }
      int ubl = key == DeltaEncoding.KEY_SECONDARY
          ? Math.min(unoptimizedBinsLog, LIMITED_UNOPTIMIZED_BINS_LOG)
          : unoptimizedBinsLog;
      int total = 0;
      for (PageInfo p : enc.pageInfos) {
        total += p.end[key] - p.start[key];
      }
      long[] contiguous = new long[total];
      int at = 0;
      for (PageInfo p : enc.pageInfos) {
        int len = p.end[key] - p.start[key];
        System.arraycopy(latents, p.start[key], contiguous, at, len);
        at += len;
      }
      BinOptimization.Trained trained = BinOptimization.trainInfos(types[key], contiguous, ubl);
      ChunkMeta.Bin[] bins = new ChunkMeta.Bin[trained.infos.size()];
      for (int i = 0; i < bins.length; i++) {
        BinOptimization.BinInfo info = trained.infos.get(i);
        bins[i] = new ChunkMeta.Bin(info.weight, info.lower, info.offsetBits);
      }
      clcs[key] = new LatentCompressor(types[key], trained, bins, latents);
      vars[key] = new ChunkMeta.LatentVar(trained.ansSizeLog, types[key], bins);
      binCounts[key] = trained.counts;
    }
    ChunkMeta meta = ChunkMeta.create(mode, delta, vars, numberLatent);
    return new Candidate(new ChunkCompressor(meta, clcs, enc.pageInfos), binCounts);
  }

  private static float calculateCompressedSampleSize(
      Latent lt, long[] sample, int unoptimizedBinsLog, DeltaEncoding delta) {
    Candidate c =
        newCandidate(
            lt,
            sample.clone(),
            null,
            PagingSpec.exact(sample.length),
            Mode.CLASSIC,
            delta,
            unoptimizedBinsLog);
    long size = c.cc.meta.maxSize() + c.cc.pageSizeHintInner(0);
    return RustMath.u64ToF32(size);
  }

  /** {@code choose_delta_sample}, or null */
  static long[] chooseDeltaSample(long[] primary) {
    int n = primary.length;
    int target = Modes.calcSampleN(n);
    if (target < 0) {
      return null;
    }
    int groupN = Math.min(200, n);
    int nGroups = (target + 199) / 200;
    int nominal = nGroups * groupN;
    int groupStride = groupN + (Math.max(n - nominal, 0) / (Math.max(nGroups, 2) - 1));
    long[] sample = new long[nominal];
    for (int i = 0; i < nGroups; i++) {
      int groupStart = groupStride * i;
      if (groupStart + groupN > n) {
        throw new PcoPanic("delta sample group out of range");
      }
      System.arraycopy(primary, groupStart, sample, i * groupN, groupN);
    }
    return sample;
  }

  private static DeltaEncoding chooseAutoDeltaEncoding(
      Latent lt, long[] primary, int unoptimizedBinsLog) {
    long[] sample = chooseDeltaSample(primary);
    if (sample == null) {
      return DeltaEncoding.NO_OP;
    }
    int sampleN = sample.length;
    DeltaEncoding best = DeltaEncoding.NO_OP;
    float bestCost = calculateCompressedSampleSize(lt, sample, unoptimizedBinsLog, DeltaEncoding.NO_OP);
    float lookbackPenalty = LOOKBACK_REQUIRED_BYTE_SAVINGS_PER_N * RustMath.u64ToF32(sampleN);
    if (bestCost > lookbackPenalty) {
      float cost =
          calculateCompressedSampleSize(lt, sample, unoptimizedBinsLog, Delta.newLookback(sampleN))
              + lookbackPenalty;
      if (cost < bestCost) {
        best = Delta.newLookback(primary.length);
        bestCost = cost;
      }
    }
    for (int order = 1; order <= DeltaEncoding.MAX_CONSECUTIVE_ORDER; order++) {
      DeltaEncoding enc = DeltaEncoding.consecutive(order, false);
      float cost = calculateCompressedSampleSize(lt, sample, unoptimizedBinsLog, enc);
      if (cost < bestCost) {
        best = enc;
        bestCost = cost;
      } else {
        break;
      }
    }
    return best;
  }

  static int chooseUnoptimizedBinsLog(int level, int n) {
    double l2 = Math.floor(GlibcMath.log2((double) n));
    int logN = (int) RustMath.f64ToUnsigned(l2, 32);
    int fast = Math.max(logN - 4, 0);
    if (Integer.compareUnsigned(level, fast) <= 0) {
      return level;
    }
    return fast + (level - fast) / 2;
  }

  private static DeltaEncoding chooseDeltaEncoding(
      Latent lt, long[] primary, ChunkConfig config, int ubl) {
    DeltaSpec spec = config.deltaSpec;
    switch (spec.kind) {
      case AUTO:
        return chooseAutoDeltaEncoding(lt, primary, ubl);
      case NO_OP:
        return DeltaEncoding.NO_OP;
      case TRY_CONSECUTIVE:
        return spec.order == 0 ? DeltaEncoding.NO_OP : DeltaEncoding.consecutive((int) spec.order, false);
      case TRY_LOOKBACK:
        return Delta.newLookback(primary.length);
      default:
        {
          if (spec.order == 0) {
            return DeltaEncoding.NO_OP;
          }
          if (lt == Latent.U64) {
            throw PcoException.invalidArgument(
                "Conv1 delta encoding cannot be used with 64-bit latents");
          }
          DeltaEncoding c = Delta.chooseConv1Config(lt, (int) spec.order, primary);
          return c == null ? DeltaEncoding.NO_OP : c;
        }
    }
  }

  /** {@code guarantee::baseline_chunk_meta} */
  static ChunkMeta baselineChunkMeta(Latent lt) {
    ChunkMeta.LatentVar primary =
        new ChunkMeta.LatentVar(0, lt, new ChunkMeta.Bin[] {new ChunkMeta.Bin(1, 0, lt.bits)});
    return ChunkMeta.create(
        Mode.CLASSIC, DeltaEncoding.NO_OP, new ChunkMeta.LatentVar[] {null, primary, null}, lt);
  }

  /** {@code guarantee::chunk_size} */
  static long baselineChunkSize(Latent lt, long n) {
    return baselineChunkMeta(lt).maxSize() + (n * lt.bits + 7) / 8;
  }

  private boolean shouldFallback(Latent lt, long n, int[][] binCounts) {
    if (meta.deltaEncoding.kind == DeltaEncoding.Kind.NO_OP && meta.mode.kind == Mode.Kind.CLASSIC) {
      return false;
    }
    long nPages = pageInfos.size();
    long worstBits = 7 * nPages;
    for (int key = 0; key < 3; key++) {
      ChunkMeta.LatentVar v = meta.perLatentVar[key];
      if (v == null) {
        continue;
      }
      for (int i = 0; i < v.bins.length; i++) {
        worstBits += (binCounts[key][i] & 0xffffffffL) * v.bins[i].worstCaseBitsPerLatent(v.ansSizeLog);
      }
    }
    long worstSize = meta.maxSize() + nPages * meta.exactPageMetaSize() + (worstBits + 7) / 8;
    return worstSize > baselineChunkSize(lt, n);
  }

  private static ChunkCompressor fallback(NumberType t, long[] nums, ChunkConfig config) {
    Latent lt = t.latent;
    long[] primary = Modes.classicSplit(t, nums).primary;
    long[] nPerPage = config.pagingSpec.nPerPage(nums.length);
    Encoded enc = deltaEncodeAndBuildPageInfos(lt, lt, DeltaEncoding.NO_OP, nPerPage, primary, null);
    List<BinOptimization.BinInfo> infos = new ArrayList<>();
    infos.add(new BinOptimization.BinInfo(1, 0, lt.max(), lt.bits, 0));
    ChunkMeta meta = baselineChunkMeta(lt);
    LatentCompressor clc =
        new LatentCompressor(
            lt,
            new BinOptimization.Trained(infos, 0, new int[] {nums.length}),
            meta.perLatentVar[1].bins,
            enc.latents[1]);
    return new ChunkCompressor(meta, new LatentCompressor[] {null, clc, null}, enc.pageInfos);
  }

  /** {@code ChunkCompressor::new}: this is where the bulk of compression happens */
  static ChunkCompressor create(NumberType t, long[] nums, ChunkConfig config) {
    Latent lt = t.latent;
    config.validate(lt);
    int n = nums.length;
    if (n == 0) {
      throw PcoException.invalidArgument("cannot compress empty chunk");
    }
    if (n > MAX_ENTRIES) {
      throw PcoException.invalidArgument(
          "count may not exceed " + MAX_ENTRIES + " per chunk (was " + n + ")");
    }
    Modes.Bid bid = Modes.chooseWinningBid(Modes.chooseModeBids(t, nums, config));
    if (!Modes.modeIsValid(t, bid.mode)) {
      throw PcoException.invalidArgument(
          "The chosen mode of " + bid.mode + " was invalid for type " + t);
    }
    Modes.Split split = bid.split.split(nums);
    int ubl = chooseUnoptimizedBinsLog(config.compressionLevel, n);
    DeltaEncoding delta = chooseDeltaEncoding(bid.mode.primaryLatentType(lt), split.primary, config, ubl);
    Candidate c =
        newCandidate(lt, split.primary, split.secondary, config.pagingSpec, bid.mode, delta, ubl);
    if (c.cc.shouldFallback(lt, n, c.binCounts)) {
      return fallback(t, nums, config);
    }
    return c.cc;
  }

  /** {@code page_size_hint_inner(page_idx, 1.0)}, the only use the standalone format makes of it */
  long pageSizeHintInner(int pageIdx) {
    PageInfo info = pageInfos.get(pageIdx);
    long bodyBits = 0;
    for (int key = 0; key < 3; key++) {
      if (clcs[key] == null) {
        continue;
      }
      long nStored = info.end[key] - info.start[key];
      double numsBits = (double) nStored * clcs[key].avgBitsPerLatent;
      bodyBits += RustMath.f64ToUnsigned(Math.ceil(numsBits), 64);
    }
    return meta.exactPageMetaSize() + (bodyBits + 7) / 8;
  }

  void writeMeta(BitWriter w) {
    meta.writeTo(w);
  }

  void writePage(int pageIdx, BitWriter w) {
    PageInfo info = pageInfos.get(pageIdx);
    LatentCompressor.Dissected[] dissected = new LatentCompressor.Dissected[3];
    for (int key = 0; key < 3; key++) {
      if (clcs[key] != null) {
        dissected[key] = clcs[key].dissectPage(info.start[key], info.end[key]);
      }
    }
    // page meta
    for (int key = 0; key < 3; key++) {
      if (clcs[key] == null) {
        continue;
      }
      Latent lt = clcs[key].lt;
      for (long s : info.deltaState[key]) {
        w.writeUint(s, lt.bits);
      }
      int def = clcs[key].encoder.defaultState();
      for (int s : dissected[key].ansFinalStates) {
        w.writeUint((s - def) & 0xffffffffL, clcs[key].encoder.sizeLog);
      }
    }
    w.finishByte();
    for (int batchStart = 0; batchStart < info.pageN; batchStart += FULL_BATCH_N) {
      for (int key = 0; key < 3; key++) {
        if (clcs[key] != null) {
          clcs[key].writeDissectedBatch(dissected[key], batchStart, w);
        }
      }
    }
    w.finishByte();
  }
}
