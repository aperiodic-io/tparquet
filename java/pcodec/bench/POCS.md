# Optimization proofs of concept

Each POC is one self-contained change to `src/main/java`, kept as `bench/pocs/NN-slug.patch`
against the commit named in `bench/RESULTS.md`, that must leave every output byte unchanged
(`VectorsTest`, `RoundTripTest`, `PayloadVectorsTest` and `MathVectorsTest` pass with it applied
alone). `bench/run-pocs.sh` applies each one to a clean copy and times it with `Bench`
(Java only); `bench/RESULTS.md` has the measurements.

The profile they come from (JFR, every `Bench` case, self time): `BitWriter.writeUint` 19%,
`LatentCompressor.encodeAnsInReverse` 14%, the `VarHandle` index checks of `BitReader.u64At`
13%, `SortUtils.partition` 12.5%, `LatentCompressor.dissectPage` 12%, `Delta.chooseLookbacks`
4.6%, `BinOptimization.chooseOptimizedPartitioning` 3.6%, `ChunkDecompressor.join` 2.2%,
`Ans.Encoder.encode` 1.9%, `Pco.checkRaw` 1.4%, `FloatMult.approxPairGcd` 1.1%, HashMap
operations about 2%.

## Encoding: writing bits

1. **staged-writer**: `BitWriter` keeps the pending bits in a 64-bit register and stores whole
   little-endian longs (`VarHandle`) when it fills, as the reference's `BitWriter` stages bytes.
2. **long-or-write**: `writeUint` ORs one unaligned long (`VarHandle`) and one byte instead of a
   loop over 8 bytes.
3. **fused-batch-write**: `writeDissectedBatch` writes the batch's ANS values and offsets with
   the position and buffer in locals, ensuring capacity once per batch.
4. **ensure-per-batch**: capacity checked once per batch (the batch's maximum bits) instead of
   once per value.
5. **presized-writer**: the `BitWriter` starts at the chunk's size estimate (meta size + page
   size hint) instead of 256 bytes, avoiding the doubling copies.

## Encoding: ANS and dissection

6. **flat-next-states**: `Ans.Encoder.nextStates` as one `int[]` with per-symbol offsets.
7. **unpacked-encode**: the encoder step inlined in `encodeOne` without packing state and bit
   count into a `long`.
8. **reuse-dissected**: the per-page `Dissected` arrays allocated once per latent variable and
   reused across pages.
9. **element-major-search**: the bin search runs per element (all depths for one latent)
   instead of per depth over the batch.
10. **no-search-fill**: the search indices initialized by the first depth instead of an
    `Arrays.fill` per batch.
11. **soa-bin-infos**: bin lower bounds, symbols and offset bits in parallel primitive arrays
    instead of `BinInfo` objects.
12. **local-ans-states**: the four interleaved ANS states in local variables in
    `encodeAnsInReverse`, written back once per batch.

## Encoding: histogram and sort

13. **signed-domain-sort**: the histogram's partition compares latents XOR-ed with the sign bit
    as signed longs (one flip per element on entry and exit) instead of unsigned compares.
14. **partition-fewer-writes**: Lomuto partition that skips the two stores when an element
    stays in place, giving the same permutation.
15. **primitive-bounds**: `Histogram.recurse` takes bounds as primitives instead of allocating
    `Bound` objects.
16. **cached-histogram**: a histogram computed once per latent array and shared by the
    candidates that bin the same latents.
17. **pivot-no-alloc**: `choosePivot` without the `int[]` index arrays it allocates per call.

## Encoding: choosing modes, deltas and bins

18. **dedupe-candidates**: the delta candidates that encode identically (e.g. consecutive order
    1 and auto picking it) evaluated once.
19. **lookback-primitive-map**: `Delta.chooseLookbacks` with an open-addressing `long -> int`
    table instead of `HashMap`.
20. **lookback-loop**: the lookback scoring loop with hoisted array reads and no boxing.
21. **binopt-table**: `BinOptimization.chooseOptimizedPartitioning` with `log2Approx` of the
    counts precomputed once per call.
22. **est-bits-primitive-map**: `Modes.estBitsSavedPerNum` with an insertion-ordered primitive
    map instead of `LinkedHashMap<Long, double[]>` (the order of the sums is kept).
23. **dict-primitive-map**: `Modes.Dict.computeBid` with a primitive insertion-ordered map.
24. **gcd-primitive-map**: `IntMult.chooseCandidateBase` counting GCDs in a primitive map.
25. **floatmult-gcd-loop**: `FloatMult.approxPairGcd` on doubles directly for f64.
26. **final-float-types**: `FloatType`'s three types as final classes, so its hot calls are
    monomorphic where the type is known.
27. **checkraw-skip-64**: `Pco.checkRaw` skipped for 64-bit types (every value fits).
28. **single-page-no-copy**: `Pco.compress` passes the numbers without `copyOfRange` when they
    form one page.
29. **reuse-split-arrays**: the mode splits (`splitLatents`) write into reused arrays.
30. **delta-64-specialized**: `Delta.consecutiveEncode` with plain `long` arithmetic for
    64-bit latents (no masking).

## Decoding

31. **word-array-reader**: `BitReader` converts the input to a `long[]` of little-endian words
    once and reads two words per value with shifts.
32. **window-reader**: `BitReader` keeps the current 64-bit window and refills it once per
    up-to-57 bits, as the reference's reader does.
33. **packed-decoder-table**: the ANS decoder's three per-state tables packed into one `int[]`.
34. **local-decoder-states**: the four interleaved decoder states in locals in
    `readBatchPreDelta`.
35. **branchless-join**: `join` for floats with a branchless `fromLatentOrdered`.
36. **decode-into-output**: `Pco.decompress(byte[], NumberType)` decodes each batch straight
    into the output array.
37. **unmasked-64**: `Latent.add`/`sub` without the mask for 64-bit latents.
38. **consecutive-decode-loop**: `Delta.consecutiveDecode` as a tight prefix-sum loop.
39. **fewer-bounds-checks**: `ChunkDecompressor.readBatch` checks bounds once per batch where the
    reference's checks cannot be told apart.
40. **single-word-peek**: `BitReader.peekAt` reads one word when the value fits in it
    (`n + off <= 64`), the second only otherwise.
