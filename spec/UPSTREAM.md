# Proposal: PCODEC compression codec

Draft for `apache/parquet-format`, laid out like its proposal template
(`proposals/_PROPOSAL_TEMPLATE.md`: Description, Rationale, Spec, Evaluation). Not submitted.
[`FORMAT.md`](FORMAT.md) stays the specification of the codec as tparquet ships it; nothing
here changes it.

| | |
|---|---|
| Author | tparquet maintainers |
| Name | PCODEC: a compression codec for PLAIN-encoded INT32, INT64, FLOAT and DOUBLE pages |
| Issue | to be opened after the [DISCUSS] thread |
| Status | DRAFT (before Discuss) |

This is not the first time pcodec has come to the Parquet community. It was pitched as an
encoding in January 2024 and again, offered as a compression codec, in March 2025; neither
thread reached a decision. The concerns raised then shape this proposal, and
[*Concerns raised before*](#concerns-raised-before) answers each one, or says what is still
missing. The proposal should not go to the list until the items in
[*Before submitting*](#before-submitting) are done: the same objections would get the same
answer.

## Description

A new value of `CompressionCodec`, `PCODEC`, for numeric column chunks. It is a codec in the
existing sense: it applies to the compressed part of every page (FORMAT.md section 3), a
reader decodes a page without knowing the column type, and decoding any payload yields exactly
`uncompressed_page_size` bytes or fails. The payload is a pcodec standalone file of the page's
values, behind a two-byte header (FORMAT.md section 4).

It is **not forward compatible**: a reader that does not know the value cannot read the column
chunk. This is the kind of change `CONTRIBUTING.md` describes for "adding and using a new
compression algorithm": the new enum value is the signal to older readers, writers put it
behind a flag that is off by default, and it is not turned on by default until two years after
the parquet-java release that reads it.

No other part of the format changes: no new page type, encoding, logical type or metadata
field.

## Rationale

**Ratio on numeric data.** Measured on market data (Evaluation), PCODEC pages are about half
the size of ZSTD(9) Parquet pages with default encodings, and about two thirds of ZSTD(9)
Parquet with the best standard encoding chosen per column (DELTA_BINARY_PACKED,
BYTE_STREAM_SPLIT, dictionary). That second figure is the honest one: it is against what a
careful writer already does with today's format.

**Why a codec and not an encoding.** In 2024 the community said that if pcodec were added it
should be "as a specification for an encoding not as a library"; in 2025 its author offered
to frame it as a compression instead. Both positions meet in the format's rules:
`CONTRIBUTING.md` asks that new encodings be "fully specified in this repository and not rely
on external dependencies", and that new compression "have a pure Java implementation that can
be used as a dependency in parquet-java". `Compression.md` already says codec specifications
"are maintained externally" (ZSTD by RFC 8878, LZ4_RAW by the LZ4 block format document). A
pcodec encoding would put a spec several times longer than any existing encoding into
`Encodings.md`; a codec keeps the pcodec format where it is maintained, versioned and pinned,
and asks for a pure Java implementation instead. That cost is accepted (Before submitting,
item 1).

As a codec it also keeps writers simple, which the 2024 pitch named as a problem: an encoding
whose size is unknown until most of the work is done would need pages split by unencoded
size. A codec runs on pages that the writer has already split, as ZSTD does.

**Its place next to ALP.** ALP is now in `Encodings.md` (`ALP = 10`, FLOAT and DOUBLE,
Preview since 2026-08-01). The two answer different needs. ALP is an encoding built for decode
speed. PCODEC trades speed for ratio, as pcodec's author said in 2025 ("much higher" ratio,
"much slower speed"; ALP "probably better for in-memory data transfers", pcodec for network
or disk). The May 2025 sync named that trade-off ("choice between compression vs decoding
speed"). PCODEC is for data that is stored long and read rarely, and it covers integers and
timestamps, which ALP does not. Whether the community wants both is a question for the
Discuss thread, and Evaluation has to answer it with numbers, not positioning.

**Alternatives.**

- *Tuned standard encodings with ZSTD*: the baseline above; PCODEC is about 30% smaller.
- *ALP, then a general codec*: to be measured (Evaluation).
- *Pluggable or self-describing encodings* (WASM decoders, Binary Protocol Extensions, raised
  in 2025): they would let PCODEC ship without a format change. They do not exist yet; if they
  come first, this proposal becomes a plug-in for them.
- *OpenZL*: considered in October 2025; its compatibility guarantees were judged not
  sufficient for Parquet. PCODEC has to meet that bar too: a fixed, versioned payload (Spec).

## Spec

`parquet.thrift`:

```thrift
enum CompressionCodec {
  ...
  LZ4_RAW = 7;
  PCODEC = 8;   // proposed: the next free value
}
```

`Compression.md`, in the style of the existing entries: *A codec for PLAIN-encoded INT32,
INT64, FLOAT and DOUBLE values, based on the pcodec standalone format, version N, wrapped in
the payload defined in `PcodecCompression.md`.* That document is FORMAT.md sections 3, 4 and 7
with:

- the codec value 8 instead of 20547, and section 2's key-value pair dropped (a standard codec
  needs no hint to explain a failure);
- the pcodec format pinned to one written version (Before submitting, item 2), not "pcodec
  ≥ 1.0";
- **the smallest set of modes that keeps the measured gain.** FORMAT.md has four: RAW, PCO,
  DECIMAL and ZSTD. Complexity was the main objection in both threads ("much more complicated
  than all other Parquet encodings"), so every mode beyond RAW and PCO has to earn its spec
  text with data:
  - *DECIMAL* overlaps pcodec's own float modes and ALP's decimal step. It stays only if
    Evaluation shows it saves enough beyond PCO mode.
  - *ZSTD* makes the codec total over any page (V1 pages with levels, columns that are not
    numeric), but duplicates the ZSTD codec. Without it, a writer that sets PCODEC on such a
    page gets RAW. The upstream spec should drop it and leave those pages to ZSTD.

  Files from tparquet whose pages use only RAW and PCO are already valid in this profile;
  pages in the other modes are rewritten (Existing files).

The writer recommendations stay informative: PLAIN encoding, no dictionary (pcodec "cannot be
dictionary encoded"; it compresses the values themselves), V2 pages or columns without
levels, and the physical type passed to the encoder. Codec APIs that take only bytes need a
type hint on the write side; readers need nothing new.

## Evaluation

What exists (from [`bench/RESULTS.md`](../bench/RESULTS.md)): 151.5M rows in 31 datasets from
four market data providers, written with the writer's default page limits (1 MiB or 20,000
rows per page, so every PCODEC page holds at most 20,000 values in its own pcodec file),
against ZSTD level 9:

| | vs ZSTD(9) | vs ZSTD(9), tuned encodings |
|---|---:|---:|
| Trades, 138.9M rows | 0.49× | 0.70× |
| Quotes, 6.8M rows | 0.63× | 0.81× |
| Order books, 5.5M rows | 0.46× | 0.54× |
| All | 0.50× | 0.68× |

All values round-trip bit for bit. Where it is weaker:

- *Read speed*, one thread, MB/s of Arrow data: 925 against 1,031 for ZSTD(9) and 1,225 for
  tuned encodings; about two thirds of ZSTD(9) on wide order books.
- *Write speed*: comparable on trades, about a third of ZSTD(9) on wide order books, much
  slower on small files.
- *Data it does not help*: strings and booleans; order books stored as lists come out at
  1.07× of ZSTD(9).
- *Implementation*: every number above comes from the Rust implementation. A pure Java
  implementation exists ([`java/pcodec`](../java/pcodec), item 1) but its speeds are not yet
  measured.

This is not yet the evidence the community asked for. In 2025 the objection was that the
benchmarks were "rather niche" and that new encodings "should be gated with a large enough
experiment on *real* data showing both efficacy and wide applicability". The answer was that
the question is whether it brings enough "in *some* cases". The process discussion that
followed aimed at a standard list of open datasets ("20 plus" was mentioned). Market data
alone answers neither side, so Evaluation needs (Before submitting, item 3):

- the community's open datasets: the ClickBench columns used by the PFOR proposal, pcodec's
  six paper datasets (taxi, air quality and others), and the ALP paper's float datasets;
- results as distributions (deciles per column), not only averages;
- the share of numeric bytes in each dataset, so the gain on a whole file is clear;
- ALP followed by ZSTD as a baseline for FLOAT and DOUBLE, and BYTE_STREAM_SPLIT followed by
  ZSTD (already part of the tuned baseline here);
- ratio against values per page (5,000, 20,000, 100,000), since pcodec is said to become
  effective around 20,000 values, the parquet-java default;
- the bytes each payload mode saves (the DECIMAL and ZSTD questions under Spec);
- read and write speed of the pure Java implementation, since JVM performance was raised at
  the June 2025 sync.

Success criteria to propose: a ratio gain over the tuned baseline on numeric columns that
holds across datasets outside finance, at decode speeds stated plainly, and two interoperable
implementations reading the shared test files.

## Concerns raised before

From the [2024 thread](https://lists.apache.org/thread/ht95wm8trfx2z4pq91t7170t2qjqg4yw), the
[2025 thread](https://www.mail-archive.com/dev@parquet.apache.org/msg26285.html), the sync
notes of [19 March](https://www.mail-archive.com/dev@parquet.apache.org/msg26303.html) and
[13 May 2025](https://www.mail-archive.com/dev@parquet.apache.org/msg26415.html), and
[pcodec issue #149](https://github.com/pcodec/pcodec/issues/149):

| Concern | Raised | This proposal |
|---|---|---|
| Rust toolchain and FFI; "my organization does not currently allow Rust code in production" | 2024 | Every engine integration here still calls one Rust core. [`java/pcodec`](../java/pcodec) is a pure Java implementation, without native code (item 1) |
| JNI in parquet-java is "a non-starter"; "a pure java implementation of PCO is always desirable"; a Java fallback is needed when native code does not load | 2024 | `CONTRIBUTING.md` makes it a rule for compression. [`java/pcodec`](../java/pcodec) writes and reads exactly what the Rust reference does; parquet-java does not use it yet (item 1) |
| Only Java? "Java is not the only implementation of Parquet" | 2024 | Two implementations, parquet-java first, as `CONTRIBUTING.md` asks; the arrow-rs fork here is the second; a C++ implementation follows the Java one |
| It should be a specification, not a library | 2024 | A codec with an external, pinned format spec, as for ZSTD and LZ4_RAW (Rationale, Spec) |
| The pcodec spec is incomplete: no overview, tANS and latent variables not defined, "data page" means something else | 2024, issue #149 (partly addressed, some items left) | Item 2: the remaining items closed, and the Java implementation written from the spec alone as the test of completeness |
| Encoding or compression? | 2024, 2025 | Compression (Rationale) |
| Is a page self-contained, or is there shared metadata like a dictionary page? | 2024 | Self-contained: each page is one standalone pcodec file with its own header. The per-page cost is in the measured ratios |
| Effective only from about 20,000 values per page; values per page in the benchmarks? | 2024 | At most 20,000 per page in every number above; a sensitivity measurement is in Evaluation |
| Size not known until most of the work is done, so pages would be split differently | 2024 | Not an issue for a codec (Rationale) |
| Patents: the ANS patent controversy | 2024 | pcodec uses tANS; its author said the patent concerns rANS only. To be confirmed through ASF legal before submitting (item 4) |
| How much comes from delta coding and how much from entropy coding? | 2024 | The per-mode breakdown in Evaluation; the author's 2024 answer was that tANS over Huffman gives under 1% |
| Comparison with BYTE_STREAM_SPLIT plus ZSTD | 2024 | Part of the tuned baseline (0.68×) |
| Comparison with ALP | 2025 | Rationale; ALP plus ZSTD in Evaluation |
| Much more complex than other Parquet encodings | 2024, 2025 | The smallest mode set (Spec); a codec spec kept outside `Encodings.md` |
| A single maintainer and little adoption; Parquet files are long-term storage, so don't be an early adopter | 2024 | pcodec reported a second maintainer and users (Zarr, CnosDB) in 2025. tparquet adds implementations across engines, but they share one core. The pinned format, two independent implementations and the shared test files limit what depends on pcodec's maintainers. Backers: item 5 |
| Numeric data is a small share of bytes (about 3% to 7% at Databricks); benchmarks are niche | 2025 | Evaluation: open datasets, numeric byte share, distributions |
| "The bar to add new encodings is/should be very high"; the proposer carries the burden of proof | 2025 | Accepted: implementations, tests and data come with the proposal |
| Selection criteria: an industrial or foundation backer, implementations in other languages, long-term support | March 2025 sync | Items 1 and 5 |
| Faster encodings are wanted; compression versus decoding speed | May 2025 sync | PCODEC is the ratio side of that trade-off, stated plainly with its speeds |
| JVM performance | June 2025 sync | Speeds of the pure Java implementation in Evaluation |
| Adding something few use increases complexity for nothing; extensible encodings may be the way | 2025 | Alternatives (Rationale) |

## Before submitting

1. **A pure Java implementation**, written from the specification alone, usable as a
   parquet-java dependency without native code, passing every vector in
   [`testdata/`](../testdata/README.md), with its speeds measured. It is required by
   `CONTRIBUTING.md`, it answers the toolchain and JNI objections, and it shows the spec is
   complete. A C++ implementation without Rust should follow, for Arrow C++ and the
   independent C++ readers (Velox, DuckDB).
   *Status:* [`java/pcodec`](../java/pcodec) is a pure Java implementation of pcodec 1.0.4 and
   of the payload. It has no runtime dependencies, targets Java 11, passes every vector in
   `testdata/` and reads every page of `testdata/files`. Its output is verified to equal the
   Rust reference's byte for byte ([`VERIFICATION.md`](../java/pcodec/VERIFICATION.md)). It
   answers the toolchain and JNI objections. It is a port of the reference's source, not
   written from the specification alone, so it does not show the spec complete (item 2 still
   stands). Its speeds are not yet measured, and the parquet-java fork does not use it yet.
2. **A complete, pinned pcodec format document.** Close what pcodec issue #149 left open (an
   overview, definitions of the tANS terms and latent variables, the "data page" terminology)
   upstream in pcodec, and cite one version of the format with a compatibility promise.
3. **The evaluation above**, run on open data and published with a script anyone can rerun.
4. **The patent question** confirmed through ASF channels, not only by the author's word.
5. **Backers and maintainers.** Name the people and organizations that will maintain the
   parquet-java and arrow-rs implementations, and ask pcodec's maintainers to take part in the
   proposal; the 2024 and 2025 pitches came from pcodec's author.
6. **Then follow the process** in `proposals/README.md`: a [DISCUSS] thread on
   dev@parquet.apache.org, a GitHub issue from the template, the proposal added to the Active
   Proposals table at Draft/POC once two reference implementations are identified, a formal
   spec change that meets `CONTRIBUTING.md`, and a PMC vote.

## Existing files

tparquet files carry codec value 20547. If the community adopts PCODEC:

- tparquet readers accept both values for as long as files with 20547 may exist.
- Pages in RAW or PCO mode are valid under the new value as they are; only the codec values
  in the metadata differ. Pages in DECIMAL or ZSTD mode are valid only if the upstream spec
  keeps those modes; otherwise the column is re-encoded (`tparquet decode`, then an encode
  with the new value).
- tparquet writers keep writing 20547, or standard Parquet, until the reference libraries that
  read the new value are released; upstream libraries keep writing it off by default for the
  two years `CONTRIBUTING.md` sets for forward-incompatible features.

## Open questions

- Should DECIMAL and ZSTD modes stay? Evaluation decides (Spec).
- Should the codec take the physical type from the column metadata instead of a writer-side
  hint, so codec APIs need no new parameter?
- pcodec's level (0 to 12, default 8) is not stored in the file, as ZSTD's is not. Should the
  spec recommend a default?
- If ALP is followed by a general codec, does PCODEC still win enough on FLOAT and DOUBLE
  columns to justify both, or should the codec be proposed for integers and timestamps first?
