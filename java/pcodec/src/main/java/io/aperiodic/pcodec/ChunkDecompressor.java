package io.aperiodic.pcodec;

/**
 * Decodes one chunk of one page ({@code wrapped::ChunkDecompressor} and {@code
 * PageDecompressorState}), batch by batch.
 */
final class ChunkDecompressor {
  static final int FULL_BATCH_N = 256;

  /** {@code ChunkLatentDecompressor} with its scratch, and the page state of the variable */
  static final class LatentDecompressor {
    final Latent lt;
    final DeltaEncoding delta;
    final int nBins;
    final long[] stateLowers;
    final Ans.Decoder decoder;
    final boolean hasOffsets;
    final long[] latents = new long[FULL_BATCH_N];
    final int[] offsetBits = new int[FULL_BATCH_N];
    final int[] offsetBitsCsum = new int[FULL_BATCH_N];
    // page state
    final int[] ansStateIdxs = new int[ChunkMeta.ANS_INTERLEAVING];
    long[] deltaState;
    final int[] deltaStatePos = new int[1];

    LatentDecompressor(ChunkMeta.LatentVar var, DeltaEncoding delta) {
      this.lt = var.latent;
      this.delta = delta;
      ChunkMeta.Bin[] bins = var.bins;
      this.nBins = bins.length;
      int maxOffsetBits = 0;
      int[] weights = new int[bins.length];
      int[] binOffsetBits = new int[bins.length];
      for (int i = 0; i < bins.length; i++) {
        maxOffsetBits = Math.max(maxOffsetBits, bins[i].offsetBits);
        weights[i] = bins[i].weight;
        binOffsetBits[i] = bins[i].offsetBits;
      }
      this.hasOffsets = maxOffsetBits > 0;
      Ans.Spec spec = Ans.Spec.fromWeights(var.ansSizeLog, weights);
      this.stateLowers = new long[spec.stateSymbols.length];
      for (int i = 0; i < stateLowers.length; i++) {
        int s = spec.stateSymbols[i];
        stateLowers[i] = s < bins.length ? bins[s].lower : 0;
      }
      this.decoder = new Ans.Decoder(spec, binOffsetBits);
      if (bins.length == 1) {
        int csum = 0;
        for (int i = 0; i < FULL_BATCH_N; i++) {
          offsetBits[i] = bins[0].offsetBits;
          offsetBitsCsum[i] = csum;
          latents[i] = bins[0].lower;
          csum += bins[0].offsetBits;
        }
      }
    }

    void startPage(int[] finalStateIdxs, long[] storedDeltaState) {
      System.arraycopy(finalStateIdxs, 0, ansStateIdxs, 0, ChunkMeta.ANS_INTERLEAVING);
      if (delta.kind == DeltaEncoding.Kind.LOOKBACK) {
        deltaState = Delta.newWindowBuffer(delta, storedDeltaState);
        deltaStatePos[0] = delta.windowN();
      } else {
        deltaState = storedDeltaState;
        deltaStatePos[0] = 0;
      }
    }

    /** {@code read_batch_pre_delta} */
    void readBatchPreDelta(BitReader r, int batchN) {
      if (batchN == 0) {
        return;
      }
      if (nBins > 1) {
        long bitIdx = r.bitIdx;
        int offsetBitIdx = 0;
        for (int i = 0; i < batchN; i++) {
          int j = i % ChunkMeta.ANS_INTERLEAVING;
          int stateIdx = ansStateIdxs[j];
          int btr = decoder.bitsToRead[stateIdx];
          int ansVal = (int) r.peekAt(bitIdx, btr);
          offsetBitsCsum[i] = offsetBitIdx;
          int ob = decoder.offsetBits[stateIdx];
          offsetBits[i] = ob;
          latents[i] = stateLowers[stateIdx];
          bitIdx += btr;
          offsetBitIdx += ob;
          ansStateIdxs[j] = decoder.nextStateIdxBase[stateIdx] + ansVal;
        }
        r.bitIdx = bitIdx;
      } else {
        java.util.Arrays.fill(latents, 0, batchN, stateLowers[0]);
      }
      if (hasOffsets) {
        long base = r.bitIdx;
        for (int i = 0; i < batchN; i++) {
          long offset = r.peekAt(base + (offsetBitsCsum[i] & 0xffffffffL), offsetBits[i]);
          latents[i] = lt.add(latents[i], offset);
        }
        r.bitIdx = base + (offsetBitsCsum[batchN - 1] & 0xffffffffL) + offsetBits[batchN - 1];
      }
    }

    /** {@code read_batch}; delta latents are the delta variable's scratch latents */
    void readBatch(BitReader r, long[] deltaLatents, int nRemainingInPage) {
      int nRemainingPreDelta = Math.max(nRemainingInPage - delta.nLatentsPerState(), 0);
      readBatchPreDelta(r, Math.min(FULL_BATCH_N, nRemainingPreDelta));
      int n = Math.min(nRemainingInPage, FULL_BATCH_N);
      switch (delta.kind) {
        case NO_OP:
          break;
        case CONSECUTIVE:
          Delta.consecutiveDecode(lt, deltaState, latents, 0, n);
          break;
        case LOOKBACK:
          if (Delta.lookbackDecode(
              lt, delta, deltaLatents, deltaLatents.length, deltaStatePos, deltaState, latents, n)) {
            throw PcoException.corruption("delta lookback exceeded window n");
          }
          break;
        default:
          Delta.conv1Decode(lt, delta, deltaState, latents, n);
          break;
      }
    }
  }

  final NumberType type;
  final ChunkMeta meta;
  final LatentDecompressor[] vars = new LatentDecompressor[3];
  private final int n;
  private int nRemaining;

  ChunkDecompressor(NumberType type, ChunkMeta meta, int n) {
    if (!Modes.modeIsValid(type, meta.mode)) {
      throw PcoException.corruption("invalid mode for " + type + " number type: " + meta.mode);
    }
    this.type = type;
    this.meta = meta;
    this.n = n;
    this.nRemaining = n;
    for (int key = 0; key < 3; key++) {
      if (meta.perLatentVar[key] != null) {
        vars[key] = new LatentDecompressor(meta.perLatentVar[key], meta.deltaEncoding.forLatentVar(key));
      }
    }
  }

  int nLatentsPerDeltaState() {
    return meta.deltaEncoding.forLatentVar(DeltaEncoding.KEY_PRIMARY).nLatentsPerState();
  }

  /** {@code PageDecompressorState::new}: reads the page meta */
  void readPageMeta(BitReader r) {
    long[][] states = new long[3][];
    int[][] idxs = new int[3][];
    for (int key = 0; key < 3; key++) {
      LatentDecompressor v = vars[key];
      if (v == null) {
        continue;
      }
      int nState = meta.deltaEncoding.forLatentVar(key).nLatentsPerState();
      states[key] = new long[nState];
      for (int i = 0; i < nState; i++) {
        states[key][i] = r.readUint(v.lt.bits);
      }
      idxs[key] = new int[ChunkMeta.ANS_INTERLEAVING];
      for (int i = 0; i < ChunkMeta.ANS_INTERLEAVING; i++) {
        idxs[key][i] = r.readInt(meta.perLatentVar[key].ansSizeLog);
      }
    }
    r.drainEmptyByte("non-zero bits at end of data page metadata");
    r.checkInBounds();
    int nInBody = Math.max(n - nLatentsPerDeltaState(), 0);
    for (int key = 0; key < 3; key++) {
      LatentDecompressor v = vars[key];
      if (v == null) {
        continue;
      }
      if (v.nBins == 0 && nInBody > 0) {
        throw PcoException.corruption(
            "unable to decompress chunk with no bins and " + nInBody + " latents");
      }
      v.startPage(idxs[key], states[key]);
    }
  }

  boolean finished() {
    return nRemaining == 0;
  }

  /** the size of the next batch */
  int nextBatchN() {
    return Math.min(FULL_BATCH_N, nRemaining);
  }

  /** decodes the next batch (up to 256 numbers) into {@code dst[at..]}, returning its size */
  int readBatch(BitReader r, long[] dst, int at) {
    int batchN = nextBatchN();
    LatentDecompressor d = vars[0];
    if (d != null) {
      int limit = Math.min(Math.max(nRemaining - nLatentsPerDeltaState(), 0), batchN);
      d.readBatchPreDelta(r, limit);
      r.checkInBounds();
    }
    long[] deltaLatents = d == null ? null : d.latents;
    vars[1].readBatch(r, deltaLatents, nRemaining);
    r.checkInBounds();
    if (vars[2] != null) {
      vars[2].readBatch(r, deltaLatents, nRemaining);
      r.checkInBounds();
    }
    join(dst, at, batchN);
    nRemaining -= batchN;
    if (nRemaining == 0) {
      r.drainEmptyByte("expected trailing bits at end of page to be empty");
    }
    return batchN;
  }

  private void join(long[] dst, int at, int batchN) {
    long[] primary = vars[1].latents;
    Latent lt = type.latent;
    switch (meta.mode.kind) {
      case CLASSIC:
        for (int i = 0; i < batchN; i++) {
          dst[at + i] = type.fromLatentOrdered(primary[i]);
        }
        break;
      case INT_MULT:
        {
          long base = meta.mode.base;
          long[] secondary = vars[2].latents;
          for (int i = 0; i < batchN; i++) {
            dst[at + i] = type.fromLatentOrdered(lt.add(lt.mul(primary[i], base), secondary[i]));
          }
          break;
        }
      case FLOAT_MULT:
        {
          FloatType f = FloatType.of(type);
          long base = type.fromLatentOrdered(meta.mode.base);
          long[] secondary = vars[2].latents;
          for (int i = 0; i < batchN; i++) {
            long unadjusted = f.mul(f.intFloatFromLatent(primary[i]), base);
            dst[at + i] =
                type.fromLatentOrdered(
                    lt.toggleCenter(lt.add(type.toLatentOrdered(unadjusted), secondary[i])));
          }
          break;
        }
      case FLOAT_QUANT:
        {
          int k = meta.mode.k;
          long signCutoff = lt.mid >>> k;
          long highMask = ~((1L << k) - 1) & lt.mask;
          long[] secondary = vars[2].latents;
          for (int i = 0; i < batchN; i++) {
            long y = primary[i];
            long flip = Latent.lt(y, signCutoff) ? highMask : lt.mid;
            dst[at + i] = (((y << k) & lt.mask) ^ secondary[i] ^ flip) & lt.mask;
          }
          break;
        }
      default:
        {
          long[] dict = meta.mode.dict;
          // the reference checks the whole scratch array, past the batch too (stale entries of
          // an earlier batch, or zeros)
          for (int i = 0; i < primary.length; i++) {
            if (Long.compareUnsigned(primary[i], dict.length) >= 0) {
              throw PcoException.corruption("dict index exceeded dict length " + dict.length);
            }
          }
          for (int i = 0; i < batchN; i++) {
            dst[at + i] = type.fromLatentOrdered(dict[(int) primary[i]]);
          }
          break;
        }
    }
  }
}
