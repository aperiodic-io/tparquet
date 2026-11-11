# PCODEC compression codec for Apache Parquet — format specification, version 1

Status: draft 1. The key words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## 1. Summary

PCODEC is a Parquet *block compression codec*, used exactly like `ZSTD` or `SNAPPY`: it is set
per column chunk in `ColumnMetaData.codec` and applies to the compressed part of every page of
that chunk. It is aimed at numeric columns (`INT32`, `INT64`, `FLOAT`, `DOUBLE`, and every
logical type stored in them: timestamps, dates, times, decimals backed by integers), where it
uses [pcodec](https://github.com/pcodec/pcodec) on the typed values, optionally after turning
decimal floats into exact integers.

Nothing else in the file changes: schema, encodings, levels, statistics, page indexes, bloom
filters and key-value metadata are standard Parquet.

**Losslessness does not depend on the column.** For any byte string `x`,
`decompress(compress(x, hint), len(x)) == x`, whatever the type hint and whatever the page
layout. The hint only affects the compression ratio. A reader therefore never needs the column
type to decode a page.

## 2. Codec identifier

```
CompressionCodec.PCODEC = 20547   // 0x5043, "PC"
```

This value is outside the range used by the Parquet specification (0–7 at the time of
writing) so that it cannot collide with a future standard codec. A reader that does not
implement PCODEC meets an unknown codec value and fails; it MUST NOT be able to read such a
page silently.

Writers SHOULD add this key-value pair to `FileMetaData.key_value_metadata` of every file
that has at least one PCODEC column chunk:

```
parquet.pcodec.version = "1"
```

so that tools can explain the failure. Readers MUST NOT require it.

## 3. Where the codec applies

The codec input is, for each page, exactly the bytes Parquet would otherwise compress:

| Page | Codec input |
|---|---|
| `DATA_PAGE_V2` | the values section (levels stay uncompressed, as the Parquet spec says) |
| `DATA_PAGE` (V1) | repetition levels + definition levels + values |
| `DICTIONARY_PAGE` | the dictionary values |

The decoder is always given the expected decompressed size, `uncompressed_page_size` (minus
the level lengths for V2 pages), and MUST fail if the payload does not produce exactly that
many bytes.

### Expected size 0

The codec input can be empty: the values section of a `DATA_PAGE_V2` page whose values are
all null, or the dictionary page of a column chunk without non-null values. Such a page still
has a payload, and a writer MUST write a valid one: one that decodes to zero bytes, such as
RAW `01 00`, which the reference encoder writes for an empty input (section 5). A writer
that stores such a V2 values section uncompressed (`is_compressed = false`, zero bytes) writes
no payload at all, which is equally valid.

A decoder given expected size 0 decodes the payload like any other: every valid payload that
produces zero bytes (RAW with an empty body, PCO or DECIMAL whose pcodec file holds no
numbers, ZSTD frames without content) MUST be accepted, and an invalid one, the empty
(zero-byte) payload included, MUST be rejected. A Parquet reader MAY skip the codec for a page
or values section of expected size 0 without looking at its bytes, as the arrow-rs reader
does; a writer MUST NOT rely on that.

### Writer recommendations (ratio, not correctness)

To give pcodec a plain array of numbers, a writer SHOULD, for a PCODEC column chunk:

- use `PLAIN` encoding and no dictionary;
- write `DATA_PAGE_V2` pages, or `DATA_PAGE` pages only when the column has
  `max_definition_level = max_repetition_level = 0`;
- pass the column's physical type to the encoder as the type hint;
- use PCODEC only for `INT32`, `INT64`, `FLOAT` and `DOUBLE` columns.

A writer that ignores these still writes valid files; the payload falls back to `ZSTD` or
`RAW` mode (section 4).

## 4. Payload

All integers are little-endian. `varint` is unsigned LEB128, at most 10 bytes, encoding a
value below 2^64: 7 bits per byte, least significant group first, the high bit of every byte
but the last set.

Encoders MUST write the minimal form, the shortest encoding of the value (only `00` for 0;
otherwise no last byte `00`). Decoders MUST accept non-minimal forms up to 10 bytes (for
example `80 00` and `80 80 80 80 80 80 80 80 80 00` for 0), and MUST reject a varint that does
not end within 10 bytes, or whose tenth byte is above `01` (a value of 2^64 or more).

```
payload := version:u8 mode:u8 body
version := 1
```

A decoder MUST reject any other version.

| mode | name | body |
|---|---|---|
| 0 | RAW | the input bytes, verbatim |
| 1 | PCO | `dtype:u8` `pco` |
| 2 | DECIMAL | `dtype:u8` `exponent:u8` `k:varint` `positions` `exceptions` `pco` |
| 3 | ZSTD | one Zstandard frame (RFC 8878) |

A decoder MUST reject any other mode.

`dtype` uses the Parquet `Type` values:

| dtype | Parquet type | width | pcodec number type |
|---|---|---|---|
| 1 | `INT32` | 4 | `i32` |
| 2 | `INT64` | 8 | `i64` |
| 4 | `FLOAT` | 4 | `f32` |
| 5 | `DOUBLE` | 8 | `f64` |

A decoder MUST reject any other dtype. `pco` is one pcodec *standalone* file (pcodec ≥ 1.0
standalone format) that runs to the end of the payload: a decoder MUST reject a payload with
any byte after the end of that file (after its footer), in PCO and DECIMAL mode alike.

### 4.1 RAW (mode 0)

The decoded bytes are `body`. Its length MUST equal the expected size.

### 4.2 PCO (mode 1)

`pco` holds `n` numbers of the dtype's number type, with `n · width` equal to the expected
size. The decoded bytes are the numbers in order, each written as `width` little-endian bytes
(the Parquet `PLAIN` representation, bit for bit, including NaN payloads and signed zeros).

### 4.3 DECIMAL (mode 2)

For `FLOAT` and `DOUBLE` values that are short decimals (prices, sizes, rates). The dtype MUST
be 4 or 5, and `exponent` MUST be at most 18.

- `n` = expected size / width, and the expected size MUST be a multiple of `width`.
- `k` is the number of exceptions, `k ≤ n`.
- `positions` is `k` varints `d_0 … d_{k-1}`. Exception `j` is at index
  `p_j = d_0 + … + d_j + j`, so positions strictly increase. Every `p_j` MUST be below `n`.
- `exceptions` is `k` values, `width` bytes each: the exact bits of the value at `p_j`.
- `pco` holds exactly `n` `i64` numbers `q_0 … q_{n-1}`.

Value `i` is:

- if `i` is an exception position, the exception's bits;
- otherwise, for `DOUBLE`, `f64(q_i) / P[exponent]`, and for `FLOAT`,
  `f32(f64(q_i) / P[exponent])`.

`P[e]` is the `f64` nearest to 10^e (exact for e ≤ 18), `f64(q)` converts an `i64` to the
nearest `f64` with ties to even, `/` is IEEE 754 binary64 division with round-to-nearest-even,
and `f32(x)` rounds a `f64` to the nearest `f32` with ties to even. These operations are
exactly specified by IEEE 754, so every conforming implementation decodes the same bits.

The encoder chooses `q_i` and the exceptions; any choice that decodes to the input is valid.
At an exception position `q_i` is unused, and the reference encoder repeats the previous
regular `q` there (0 before the first) to keep the pcodec stream smooth.

### 4.4 ZSTD (mode 3)

`body` is Zstandard data (RFC 8878): one frame, or several concatenated, as written by the
Parquet `ZSTD` codec. It decodes to the expected size; a decoder MUST NOT produce more output
than that. An empty body holds no frame: a decoder MUST reject it, even for expected size 0.
In ZSTD mode, zero bytes are a frame without content, such as `28 b5 2f fd 20 00 01 00 00`.

## 5. Encoder (informative)

The reference encoder (`rust/core`), given input bytes, an optional type hint and a level
0–12 (pcodec's compression level, default 8):

1. Empty input → RAW: the payload `01 00` (section 3, expected size 0).
2. A hint with a width that divides the input length → PCO for that dtype. For `FLOAT` and
   `DOUBLE` it also tries DECIMAL: it probes the first 4096 values for the smallest exponent
   at which at least 99% of them are exact, then encodes all values, giving up if more than
   1% are exceptions. It keeps the smaller of PCO and DECIMAL.
3. No hint, or a width that does not divide the length → ZSTD at level 3.
4. If the result is not smaller than RAW, it emits RAW.

## 6. Choosing the codec per column (informative)

The writers in this repository offer `auto` selection: before the first row group is written,
each eligible column (section 3) of the first batch is written to a scratch column chunk with
the base codec (ZSTD by default, with the writer's normal encodings) and with PCODEC (`PLAIN`,
no dictionary). The smaller one is used for that column in the whole file. A user can force
either codec per column.

Writers that write every page of a file with PCODEC columns as `DATA_PAGE_V2` (as the Rust
and Go writers do: their page version is per file) also compare the whole first batch: when
no column is forced to PCODEC and the base codec with the writer's normal page version is
smaller in total than the per-column choice, every column keeps the base codec. V2 pages
store repetition and definition levels uncompressed, which can cost the other columns more
than PCODEC saves. (The Rust writer compares each column's two candidates with V2 pages; the
Go writer compares the base codec with its normal pages against PCODEC with V2 pages.)

## 7. Security

Payloads are untrusted input. A decoder MUST bound its output by the expected size, MUST check
every length and position before use, and MUST NOT panic or read out of bounds on any input.
The reference decoder is fuzzed.

## 8. Versioning

A future incompatible payload change uses a new `version` byte; new modes may be added under
version 1, and decoders reject modes they do not know. The codec identifier never changes.
