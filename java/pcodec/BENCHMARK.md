# pcodec-java against the Rust reference

`make bench` times both implementations on the same numbers and configurations: 14 cases of
2^20 numbers each, covering every mode (classic, IntMult, FloatMult, FloatQuant, Dict) and delta
encoding (none, consecutive, lookback, conv1) and several number types. Each side runs as many
warm-up rounds as it measures (15), then reports the median, in MB/s of uncompressed values. The
reference is timed inside its own process, so its numbers exclude the IPC; the Java side runs
with `-Xms2g -Xmx2g`. The compressed size is checked to be identical on both sides.

Measured on a shared 4-core cloud machine (Linux x86-64, OpenJDK 21, Rust stable, release
build); absolute numbers move by about 5% between runs, ratios less.

## Before the optimizations (commit 2d57dce)

| case | java enc | rust enc | ratio | java dec | rust dec | ratio |
|---|---:|---:|---:|---:|---:|---:|
| f64 prices (auto) | 73 | 237 | 0.31 | 279 | 1406 | 0.20 |
| f64 walk (auto) | 74 | 230 | 0.32 | 535 | 1566 | 0.34 |
| f64 floatMult | 80 | 211 | 0.38 | 254 | 1356 | 0.19 |
| f64 quant | 58 | 176 | 0.33 | 288 | 1292 | 0.22 |
| f32 decimal | 42 | 138 | 0.31 | 262 | 1044 | 0.25 |
| i64 timestamps | 128 | 376 | 0.34 | 470 | 2541 | 0.19 |
| i64 uniform | 155 | 281 | 0.55 | 1240 | 2068 | 0.60 |
| i64 mult (intMult) | 91 | 248 | 0.36 | 315 | 1771 | 0.18 |
| i32 walk | 36 | 152 | 0.23 | 329 | 1716 | 0.19 |
| u32 periodic lookback | 38 | 68 | 0.56 | 184 | 467 | 0.39 |
| i32 quadratic conv1(2) | 53 | 95 | 0.56 | 128 | 365 | 0.35 |
| u16 dictish (dict) | 24 | 47 | 0.52 | 133 | 715 | 0.19 |
| i64 classic no-op | 136 | 322 | 0.42 | 492 | 1934 | 0.25 |
| f64 sorted consec(2) | 78 | 190 | 0.41 | 403 | 1091 | 0.37 |
| **geometric mean** | | | **0.39** | | | **0.26** |

## After the ten optimizations

| case | java enc | rust enc | ratio | java dec | rust dec | ratio |
|---|---:|---:|---:|---:|---:|---:|
| f64 prices (auto) | 123 | 214 | 0.57 | 333 | 1344 | 0.25 |
| f64 walk (auto) | 120 | 239 | 0.50 | 516 | 1553 | 0.33 |
| f64 floatMult | 131 | 209 | 0.63 | 386 | 1371 | 0.28 |
| f64 quant | 100 | 177 | 0.56 | 322 | 1317 | 0.24 |
| f32 decimal | 76 | 142 | 0.53 | 307 | 1071 | 0.29 |
| i64 timestamps | 210 | 376 | 0.56 | 531 | 2567 | 0.21 |
| i64 uniform | 184 | 276 | 0.67 | 998 | 1938 | 0.51 |
| i64 mult (intMult) | 153 | 247 | 0.62 | 381 | 1765 | 0.22 |
| i32 walk | 86 | 156 | 0.55 | 372 | 1802 | 0.21 |
| u32 periodic lookback | 40 | 70 | 0.57 | 229 | 479 | 0.48 |
| i32 quadratic conv1(2) | 61 | 118 | 0.52 | 133 | 417 | 0.32 |
| u16 dictish (dict) | 62 | 55 | 1.13 | 144 | 771 | 0.19 |
| i64 classic no-op | 182 | 339 | 0.54 | 614 | 1994 | 0.31 |
| f64 sorted consec(2) | 154 | 259 | 0.59 | 530 | 1409 | 0.38 |
| **geometric mean** | | | **0.60** | | | **0.29** |

Encoding is 1.5 times as fast as before (from 0.39 to 0.60 of the reference's speed), decoding
1.1 times (from 0.26 to 0.29); Dict-mode encoding is now faster than the reference's.

## How the ten were chosen

A JFR profile of every case put the time in `BitWriter.writeUint` (19%), ANS encoding and
dissection (26%), the bounds-checked word reads of the decoder (13%) and the histogram's
partition (12.5%). From it came 40 proofs of concept (`bench/POCS.md`), each a patch that keeps
every output byte (`bench/pocs`; the recorded-vector suites pass with each applied alone). Each
was timed against the baseline, then the 20 leading ones again with three alternating JVM
launches each to get below the launch-to-launch noise (`bench/RESULTS.md`, `bench/run-pocs.sh`).
The ten with the largest measured effect went in:

| POC | what | alone |
|---|---|---|
| 01 staged-writer | the bit writer stages 64 bits in a register and stores whole longs | encode 1.16 |
| 09 element-major-search | the bin search runs all depths per latent, in locals | encode 1.13 |
| 31 word-array-reader | the decoder reads two little-endian words per value from a `long[]` | decode 1.08 |
| 04 ensure-per-batch | capacity checked once per batch instead of per value | encode 1.07 |
| 03 fused-batch-write | a batch's ANS values and offsets written with the writer's state in locals | encode 1.05 |
| 05 presized-writer | the writer sized to the chunk's estimate instead of doubling from 256 bytes | encode 1.05 |
| 36 decode-into-output | whole-file decoding writes each batch straight into the result | decode 1.04 |
| 22 est-bits-primitive-map | the mode estimates count in an insertion-ordered primitive map | encode 1.02, decode 1.04 |
| 23 dict-primitive-map | Dict mode counts and ranks values with primitive arrays | encode 1.02 (Dict 1.27) |
| 21 binopt-table | the bin optimisation looks up `log2Approx` of the counts | encode 1.01 |

01, 03, 04 and 05 all change the bit writer and are merged into one (`BitWriter.writeBatch`
keeps the staged register in locals and checks capacity once per batch). With all ten applied,
every parity suite against the reference agrees as before (`make parity`).

What remains: decoding stays at under a third of the reference's speed, where the reference
reads with unchecked unaligned loads and branch-free ANS tables in a tight loop; the next
candidates are the decoder ones that measured within the noise alone (window reader, packed
decoder tables, local decoder states), which may compound with the word-array reader.
