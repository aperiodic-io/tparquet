# Verification of pcodec-java against the reference

The claim: for every input, pcodec-java produces the output of the reference, pco 1.0.4 and
tparquet-core as built for x86-64 Linux:

- **Encoding.** `Pco.compress(type, nums, config)` returns the bytes of
  `pco::standalone::simple_compress(nums, config)`, or fails where the reference fails: an error
  where it returns an error, a `PcoPanic` where it panics. `PcodecPayload.compress` returns the
  bytes of `tparquet_core::compress` in every mode but ZSTD (see *Limits*).
- **Decoding.** `Pco.decompress` and `PcodecPayload.decompress` accept exactly the inputs the
  reference accepts, decode them to the same numbers (bit for bit, NaN payloads included), and
  end at the same byte. They reject the rest; `Pco` with the reference's error kind
  (corruption, insufficient data, invalid argument).

No method proves two programs in two languages equivalent for all inputs. The claim is instead
established in layers. Each layer covers what the one below cannot:

1. **Construction.** The port is a transliteration, module by module and function by function,
   of the reference's source at the pinned versions. Nothing was reimplemented from the format
   description.
2. **Proofs** (SMT, all inputs). Every place where the translation is not literal is proved
   equivalent to the Rust expression. That covers unsigned arithmetic in signed `long`s,
   branchless Rust idioms written with branches, Rust's float casts and rounding, and bit I/O.
   The transforms that make pcodec lossless are proved inverted by their decoders.
3. **Complete domains** (exhaustive). The leaf functions whose results come from the
   reference's platform (libm, f16 conversions) are compared with the reference on their whole
   input domain, or on the whole of the part pcodec uses.
4. **Bounded exhaustive equivalence** (every input within a bound). Compressor and decompressor
   are compared with the reference on every input of small bounded domains.
5. **Differential testing** (structured and random inputs). The whole implementation is
   compared with the reference on large corpora of every type, data shape, size and
   configuration, valid and corrupted.
6. **Recorded vectors and mutation testing.** The reference's answers are recorded so that every
   build checks them without Rust, and mutation testing shows the checks detect changes to
   the code.

A failure in any layer is a difference from the reference. The layers found six (see
*Differences found*). All are fixed and each has a test.

## 1. Construction

| Java | pco 1.0.4 / tparquet-core |
|---|---|
| `BitWriter`, `BitReader` | `bit_writer.rs`, `bit_reader.rs` (bit for bit; the reference's staging buffers are an optimization with no effect on the bytes) |
| `Ans` | `ans/spec.rs`, `ans/encoding.rs`, `ans/decoding.rs` |
| `Histogram`, `SortUtils` | `histograms.rs`, `sort_utils.rs` (the partial quicksort, pivots, pattern breaking, heapsort, exactly) |
| `BinOptimization` | `bin_optimization.rs`, `wrapped/chunk_compressor.rs::train_infos` (f32 arithmetic, `log2_approx`) |
| `Delta`, `DeltaEncoding` | `delta/*.rs`, `metadata/delta_encoding.rs` |
| `Modes`, `FloatMult`, `FloatType` | `mode/*.rs`, `sampling.rs`, `data_types/*.rs` |
| `ChunkMeta`, `Mode` | `metadata/*.rs` |
| `ChunkCompressor`, `ChunkDecompressor`, `Pco` | `wrapped/*.rs`, `chunk_latent_*.rs`, `page_latent_decompressor.rs`, `standalone/*.rs` |
| `GlibcMath`, `RustMath` | the platform functions the reference calls (below) |
| `payload/*` | `rust/core/src/{lib,decimal,varint}.rs` |

Rust semantics with no Java equivalent are made explicit:

- Latents are zero-extended in `long`s, and every operation wraps at the latent's width.
- Rust's float-to-int casts saturate.
- `round` rounds half away from zero.
- `f32::max` ignores a NaN operand.
- Float sums are left folds from -0.0.
- `max_by` keeps the last of equal maxima.
- Overflow wraps, since the reference is built in release mode.

Float arithmetic in the float modes goes through `FloatType`. It works on raw bits and applies
x86-64 SSE's NaN rules: the first NaN operand, quieted, or the default NaN. Java leaves NaN
payloads unspecified; they must come out right, because a NaN's bits reach the compressed bytes
in FloatMult mode.

Where the reference panics on an input it accepts, the Java code throws `PcoPanic` at the same
point. That happens once: conv1 encoding of a batch shorter than its order. Where the reference
cannot decode its own output, the Java code reproduces that too. That happens with Dict mode
and a delta encoding (`RoundTripTest.referenceCannotDecodeDictWithDelta`).

## 2. Proofs (`verification/proofs.py`, Z3)

93 theorems, all proved, in under a minute. Each theorem is stated for all inputs of its widths. It is proved when Z3 shows its negation
unsatisfiable. The Java side is transcribed from the named method, the Rust side from pco.

| Theorem | Widths |
|---|---|
| `NumberType.toLatentOrdered`/`fromLatentOrdered` equal pco's branchless `to_latent_ordered`/`from_latent_ordered` (floats); `x ^ MID` equals `x.wrapping_sub(MIN)` and `wrapping_add(MIN)` (signed); the maps are inverses | 8, 16, 32, 64 |
| the float latent map preserves order (`a < b` implies `latent(a) < latent(b)`, IEEE comparison), with -0.0 just below +0.0 | f32, f64 |
| `Latent` add, sub, mul, toggle-center and unsigned compare on zero-extended `long`s equal the wrapping operations of the latent type; unsigned div/rem too | 8, 16, 32 (div/rem: 8, 16) |
| Classic, IntMult (`mult * base + adj == x`), FloatQuant (`join(split(x)) == x` for every bit pattern and every legal `k`; the secondary has at most `k` bits) | all types (IntMult: 8) |
| `intFloatFromLatent(intFloatToLatent(m)) == m` for every integer-valued, infinite or NaN `m`, which makes FloatMult lossless | f32, f64 |
| consecutive delta: `decode(encode(x)) == x`, batch decoding as the decoder does it | n <= 6, order <= 4 |
| lookback delta: `decode(encode(x)) == x` for every valid choice of lookbacks | n <= 5, state <= 2 |
| `BitWriter.writeUint` then `BitReader.readUint` returns the value, and leaves earlier bits alone | 1..64 bits, every bit offset |
| `Varint`: `get(put(v)) == v` within 10 bytes | every u64 |
| `RustMath.roundF64`/`roundF32` equal IEEE roundToIntegral(RNA), bit for bit | every finite f64, f32 |
| `RustMath.f64ToUnsigned` equals Rust's `f64 as u64`, `as u32`, `as u16` | every f64 |
| `Latent.toF64` and `RustMath.u64ToF32` equal Rust's `u64 as f64`, `u64 as f32` | every u64 |

The remaining wide division lemmas are the definition of unsigned division: zero extension
preserves the integer, and `Long.divideUnsigned` is integer division. Bit-blasting a 32- or
64-bit divider exceeds a CI budget.

## 3. Complete domains (`MathParityTest`)

Both sides fold every output into a 64-bit digest, and a differing block is bisected to its
first input.

| Function | Domain | Result |
|---|---|---|
| `f16::from_f32` (half 2.7.1, F16C) | every f32 bit pattern (2^32) | equal |
| `f16::to_f32`, `to_f64` | every f16 bit pattern (2^16) | equal |
| `f64::log2` (glibc 2.39, `__log2_fma`) on counts | every integer 1..2^24 (bin weights, chunk sizes) | equal |
| `f64::log2` | within 1024 ulps of every power of two (4.3M), 2M random positive doubles, 8M probabilities, 2M near 1 | equal |
| `f64::cbrt` | 2M random positive doubles, 8M probabilities | equal |
| `10.0_f64.powf(k)` (glibc pow) | every integer k in -400..400, the whole domain pcodec uses | equal |
| Rust casts and rounding (`as u64/i64/u32/u16`, `round`, `u64 as f64/f32`, `f64 as f16`) | 2M random bit patterns each | equal |
| all of the above | about 2,000 special values each: zeros, subnormals, rounding ties, cast limits, infinities, NaNs, exact cubes, and cbrt's hard cases (inputs that take CORE-MATH's refinement path, found by search) | equal |

`vectors/math.txt` records the reference's digests on slices of these domains and on the
special values, so that builds without Rust (`MathVectorsTest`) check them too.

These functions matter because pcodec's heuristics depend on them. `log2` of bin weights sets
the size estimates that choose the delta encoding. `cbrt` and `log2` of probabilities score
IntMult and FloatQuant. `powf` snaps FloatMult bases to decimals. A last-bit difference changes
the output whenever it flips one of those decisions.

## 4. Bounded exhaustive equivalence (`ExhaustiveParityTest`)

Every input in each domain gives the reference's outcome and bytes.

| Domain | Inputs |
|---|---|
| every u8, every pair of i8, every u16, every i16 | 256 + 65,536 + 65,536 + 65,536 |
| every f16 (all 65,536 bit patterns, NaNs included) under Auto, TryFloatMult(0.1) and TryFloatQuant(3); pairs of 256 spread f16 values | 196,608 + 65,536 |
| every triple of 21 to 24 edge values (extremes, zeros, powers of two, NaN payloads, infinities, subnormals, decimals) of i32, u32, i64, u64, f32, f64, f16, under Auto, Classic with lookback, consecutive(2) and conv1(1) | 28 domains, about 330,000 |
| every sequence of 10 over 3 values, and of 11 and 12 over 2, of i32, i64, f32, f64 (the sampling thresholds) | 260,000 |
| TryIntMult(3), TryFloatMult(0.1), TryFloatQuant(4): every pair of edge values and every 9-sequence over 3 values | 9 domains |
| the decoder: six valid files of different modes and delta encodings, cut at 13 points each, followed by every 2-byte suffix | 78 x 65,536 |
| the decoder: a header and chunk preamble followed by every 3-byte body | 16,777,216 |

## 5. Differential testing (`ParityTest`, `PayloadParityTest`)

The corpus (`Corpus`) generates 22 data shapes. They are chosen for what pcodec's heuristics
react to: uniform, small, constant, random walk, trend, quadratic, multiples with and without
noise, periodic, spiky, few distinct values, decimal, float multiples, quantized, special values,
sorted, sparse, timestamps, prices, bimodal, steps, and mixtures. Sizes run from 1 to 600,000,
around the batch (256), sample (10) and page (2^18) boundaries. Every one of the 11 number types
is covered. At scale 1 (each CI run; the weekly run is at scale 8):

| Suite | Cases | Of which the reference refuses |
|---|---:|---:|
| default configuration, every type, shape and size, and decoding each file back | 20,328 | 0 |
| chunk metadata (mode, delta encoding, every bin, ANS table sizes), compared as the reference's `Debug` prints it | 961 | 408 |
| every level 0..13 | 11,880 | 440 (level 13) |
| every mode spec, legal and illegal parameters | 60,896 | 34,672 |
| every delta spec, including conv1 up to order 33 | 53,334 | 6,072 errors, 1,578 panics |
| modes and delta specs combined, random levels | 3,445 | 2,255 |
| paging specs (equal pages, exact pages, wrong sums) | 5,533 | 979 |
| files of more than one chunk (up to 600,000 numbers) | 64 | 0 |
| decoder on corrupted files (bit flips, truncation, random bytes, appended bytes, wrong type), every mode and delta spec | 10,416 | 5,606 |
| payload encoder, every hint and level, DECIMAL on and off, and decoding back | 1,640 | 62 |
| payload decoder on corrupted payloads | 5,632 | 3,986 |

Every case agrees: the same status (ok, error, panic); when ok the same bytes; when the
reference returns a pco error, the same error kind (`PcoException.kind()` against the `pco
{kind:?} error` of the reference's message; the exhaustive suites fold the kind into their
digests).
At scale 8, the weekly run's, the same suites make 1,320,799 cases (12,624 of them panics of the
reference), and every one agrees.

## 6. Recorded vectors, conformance files, mutation testing

- `vectors/pco-1.0.4.bin.gz`: 8,123 recorded requests and answers of the reference. They cover
  compression under random configurations, chunk metadata, decoding, decoding corrupted files,
  and targeted cases: every conv1 order and dict mode (decoded and corrupted too), lookback and
  consecutive deltas, mode parameters, levels, paging specs, and large inputs whose histograms
  meet bad pivots. `VectorsTest` replays them without Rust. CI re-records them and
  `vectors/math.txt` and fails if a byte changes.
- `testdata/payloads` (`PayloadVectorsTest`): every valid vector decodes, every invalid one is
  refused, and every vector the reference encoder wrote is what the Java encoder writes.
- `testdata/files` (`ParquetFilesTest`): every PCODEC page of every Parquet file decodes to the
  reference's bytes, the payload modes per column are those of the manifest, and re-encoding
  each page gives back its payload byte for byte (ZSTD pages excepted).
- PIT (`make mutants`, and the `mutation` CI job) mutates the library and runs the suites that
  need no oracle against each mutant: the vectors, unit tests and round trips. A mutant that
  survives is a change to the code those suites cannot see. Each round of survivors so far was a
  missing test or dead code, and was fixed: targeted vectors, math special values, unit tests,
  dead code removed. That took the score from 87% to 91% (2,654 of 2,929 mutants, the
  checker `Modes.Margins` and its hooks included). The build fails below 90%. The checker is
  itself tested, without the oracle, in `OrderIndependenceTest`: its intervals hold the
  quotient of every summation order and are no wider than Higham's bound, its comparisons
  are exact at their boundaries, and it flags a constructed IntMult score tie between two
  bases that both win (2^54 - 1 and 2^54 + 1, one f64).
  The remaining survivors fall into three groups:
  - the decision margins of the mode and delta heuristics (a comparison mutated from `<` to `<=`,
    or a constant changed, that no recorded input sits close enough to the threshold to show);
  - CORE-MATH's rounding corrections in `cbrt`, which only change exact cubes and the hardest
    cases;
  - bounds checks that a later check duplicates, which change the error a corrupt input gets but
    not whether it is refused.

  `hashCode` is excluded, since any constant satisfies its contract. PIT cannot afford the
  oracle-backed suites, which judge the first group on every CI run.

## Differences found

The port matched the reference on most cases from the first run. These are the differences the
layers found, all fixed:

1. **Lookback hash constant** (differential testing): a mistyped 64-bit constant in
   `choose_lookbacks` made 3 of about 1,200 cases choose other lookbacks.
2. **Dict index check** (recorded vectors): the reference checks every entry of its 256-latent
   scratch array, stale ones past the batch included, not only the batch. On corrupted files it
   therefore rejects some that a batch-only check accepts.
3. **cbrt is not glibc's** (complete domains): Rust's standard library takes `cbrt` from
   `compiler_builtins` (the libm crate's port of CORE-MATH's correctly rounded cbrt), not from
   glibc. Its `log2` and `pow` are glibc's. No output byte had differed; the domain check found
   it.
4. **glibc pow is not correctly rounded** at 10^23 and 10^210 (complete domains). `GlibcMath.pow10`
   carries the two values.
5. **NaN payloads**: Java does not specify them; `FloatType` computes x86-64's.
6. **The test suites themselves**: the f16 TryFloatMult/TryFloatQuant domains first ran with a
   zero parameter (always an error), and the corruption suite first skipped Dict mode, which is
   how item 2 had escaped it.
7. **Version before bounds** (comparing error kinds, exhaustive decoding): the reference checks
   the format version before the reader's bounds, reading a missing minor byte as its padding's
   0, so a header cut after an unsupported major version (5 and up) is corruption; the port
   reported insufficient data. Both rejected the input; the parity suites first compared only
   that. Recorded vectors now cover every such cut.

## Limits

**Nondeterminism of the reference, and how it is still verified.** pco 1.0.4 iterates Rust
`HashMap`s, whose order is seeded per process, in three heuristics. Its output can therefore
depend on the process, not only on the input. Each case is verified as follows:

- **Dict mode** orders dictionary values of equal counts by HashMap order. Its bytes vary
  between reference processes when counts tie.
  - `DictParityTest` uses a traced reference (`oracle-traced/`). This is pco 1.0.4 as published,
    plus one statement that records the dictionary it chose, and `prepare.sh` checks that the
    diff is exactly that. The test gives the Java encoder the reference's dictionary order and
    requires the reference's bytes. Every case agrees: 5,808 at scale 1, tied counts and
    fallbacks included.
  - With that, Dict mode is verified completely, conditioned on the one choice the reference
    leaves to chance. Java's own default for that choice is first appearance.
- **IntMult** picks the best-scoring GCD and breaks equal scores in HashMap order.
- **The bits-saved estimates** of IntMult, FloatMult and FloatQuant sum terms in HashMap order.

  For these two, `Modes.Margins` (a verification hook that is off in production) does the
  following on every input the parity suites compress, and on `OrderIndependenceTest`'s corpus:
  - It computes each sum exactly, with Higham's bound on the error of any summation order. The
    bound is zero when all terms are multiples of a common power of two and every partial sum is
    exact.
  - It checks that every decision taken on an estimate lies on one side of its threshold for
    every order. Those decisions are a threshold, the choice between FloatMult candidates, and
    the winning bid.
  - It checks that every IntMult score tie leads to the same outcome, whichever tied candidate
    is taken.

  Every parity case passes, and so do the 19,360 corpus cases at scale 8. On those inputs the
  reference has exactly one possible output, whatever its HashMap seeds, and the Java output
  equals it.

**Platform of the reference.** The reference's output depends on its platform in the same
heuristics:

- libm's `log2` and `pow`, and the toolchain's `cbrt`;
- the half crate's F16C conversion path;
- x86 NaN propagation.

pcodec-java reproduces the reference on x86-64 Linux with glibc 2.39 (ubuntu-24.04, the CI
runners) and a stable Rust toolchain. A reference built for another platform (macOS libm,
aarch64 without F16C) can itself produce different bytes for the same input. Decoding does not
depend on the platform, except FloatMult's NaN payloads.

**ZSTD mode is not pcodec.** A ZSTD payload (FORMAT.md 4.4) is a Zstandard frame from libzstd,
the payload format's fallback for pages pcodec does not model, and the upstream profile drops
it (spec/UPSTREAM.md). Its bytes are libzstd's, so a pure Java pcodec cannot reproduce them;
that would take a byte-exact port of libzstd 1.5.7. `PcodecPayload` takes Zstandard as a
plug-in (`ZstdCodec`), and ZSTD decoding is verified (the conformance vectors, with a pure Java
Zstandard). Every pcodec byte is verified, in PCO and DECIMAL payloads and in the standalone
format.

**Bounds.** The proofs and the exhaustive domains are complete within their bounds. Beyond them
(long inputs, wide types), the evidence is the differential testing above, about 1.3 million
cases at the weekly scale. The structure of the code supports it: long inputs go through the
same functions in 256-number batches.
