# pcodec-java

A pure Java implementation of [pcodec](https://github.com/pcodec/pcodec) 1.0.4, the version
tparquet pins, and of the PCODEC page payload of [`spec/FORMAT.md`](../../spec/FORMAT.md). It
needs no native code and has no runtime dependencies. It is the implementation the draft Parquet
proposal ([`spec/UPSTREAM.md`](../../spec/UPSTREAM.md), *Before submitting*, item 1) asks for:
usable as a parquet-java dependency, Java 11 bytecode, passing every vector in
[`testdata/payloads`](../../testdata/payloads).

It is **byte-for-byte compatible with the Rust reference**. For the same numbers and
configuration, `Pco.compress` writes the bytes `pco::standalone::simple_compress` writes, and
`PcodecPayload.compress` writes the bytes `tparquet_core::compress` writes. `Pco.decompress` and
`PcodecPayload.decompress` accept exactly the inputs the reference accepts, decode them to the
same numbers, and reject the rest. [`VERIFICATION.md`](VERIFICATION.md) sets out how this is
established and where it has limits.

## Use

```java
// the standalone format (pco::standalone)
byte[] file = Pco.compress(new double[] {1.5, 2.25, 3.0}, new ChunkConfig());
double[] back = Pco.decompressDoubles(file);

// any configuration of pco 1.0.4
ChunkConfig cfg = new ChunkConfig()
    .withCompressionLevel(10)
    .withModeSpec(ModeSpec.tryFloatMult(0.01))
    .withDeltaSpec(DeltaSpec.tryConsecutive(2))
    .withPagingSpec(PagingSpec.equalPagesUpTo(100_000));

// raw bits of any of the 11 number types (u8..u64, i8..i64, f16, f32, f64)
byte[] f = Pco.compress(NumberType.U16, new long[] {1, 2, 65535}, cfg.withEnable8Bit(false));

// the Parquet page payload (spec/FORMAT.md section 4)
PcodecPayload codec = new PcodecPayload();          // or new PcodecPayload(zstdCodec)
byte[] payload = codec.compress(plainBytes, 0, plainBytes.length, ParquetType.DOUBLE, 8, true);
byte[] page = codec.decompress(payload, plainBytes.length);
```

Numbers are raw bits, zero-extended: `Pco.compress(NumberType.I32, ...)` takes `x & 0xffffffffL`,
not a sign-extended `int`, and rejects a value with bits above its type. A file can hold far more
numbers than it has bytes, so for untrusted input decode with a bound,
`Pco.decompress(file, type, maxNumbers)`, or batch by batch through the `BatchSink` overload.

`PcodecPayload` writes RAW, PCO and DECIMAL payloads exactly as the reference does. ZSTD mode
(FORMAT.md 4.4) goes through a `ZstdCodec` you supply, because Zstandard output depends on the
Zstandard implementation and the upstream profile of the codec drops the mode. Without one, the
encoder writes RAW where the reference would write ZSTD (a valid payload), and decoding a ZSTD
payload fails.

## Speed

`make bench` times Java against the reference on the same numbers (BENCHMARK.md): 14 cases
covering every mode and delta encoding, 2^20 numbers each. Java encodes at 0.60 of the
reference's speed and decodes at 0.29 (geometric means), after ten optimizations chosen by
measuring 40 proofs of concept (`bench/`); Dict-mode encoding is faster than the reference's.

## Layout

| Path | What |
|---|---|
| `src/main/java/io/aperiodic/pcodec` | the port, one class per reference module (`Ans` is `pco::ans`, `Histogram` is `pco::histograms`, ...) |
| `src/main/java/io/aperiodic/pcodec/payload` | the FORMAT.md payload (`tparquet_core`) |
| `oracle/` | the Rust reference behind a binary protocol on stdin/stdout, which the parity tests drive |
| `vectors/` | golden vectors: recorded oracle requests and answers, so the reference's answers are checked without Rust |
| `verification/` | the SMT proofs (`proofs.py`) |
| `VERIFICATION.md` | what is verified, how, and what is not |

## Build and test

```bash
make -C java/pcodec test
```

```bash
make -C java/pcodec parity
```

```bash
make -C java/pcodec vectors-check
```

```bash
make -C java/pcodec proofs
```

```bash
make -C java/pcodec mutants-oracle
```

`test` needs only Maven and a JDK. `parity` builds the oracle with cargo and runs every suite
against it; `SCALE=8` multiplies the random cases. `vectors-check` re-records the golden vectors
and fails if a byte changes. `proofs` needs `pip install z3-solver==4.13.0.0`. `mutants-oracle`
(hours) runs PIT on the library and judges its survivors again with the oracle-backed suites.
`.github/workflows/pcodec-java.yml` runs all of these.
