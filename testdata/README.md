# Conformance vectors

Every implementation of PCODEC (Rust, Go, Python, DuckDB) reads these files and must produce
exactly the expected rows, and decodes the payload vectors exactly as listed. They are written
by a deterministic generator:

```
cargo run -p tparquet --bin gen-testdata            # rewrites testdata/{files,expected,payloads}
cargo run -p tparquet --bin gen-testdata -- --out DIR
```

CI regenerates them and fails if any byte differs (`make testdata-check`). Do not edit them by
hand; change the generator (`rust/tparquet/src/bin/gen-testdata/`) and regenerate.

## Layout

| Path | What |
|---|---|
| `files/<name>.parquet` | a Parquet file |
| `files/manifest.json` | per file: description, rows, row groups, the `parquet.pcodec.version` footer value (or `null`), the footer's key-value pairs but `ARROW:schema` (`null` for a key without a value), and per leaf column its physical type, codecs, page counts per page type and PCODEC pages per payload mode (as `tparquet inspect --json` reports them; see below) |
| `expected/<name>.jsonl` | the file's rows in canonical JSON (below) |
| `payloads/<name>.bin` | a PCODEC page payload (spec section 4) |
| `payloads/<name>.out` | for a valid payload, the bytes it decodes to |
| `payloads/manifest.json` | per payload: `expected_len` (the size the decoder is given), `valid`, and for a valid one `decoded`, `mode`, `hint`, `level` and `reference_encoding`; for an invalid one `error`, the reference decoder's message (informative: other decoders only have to fail) |

A valid payload with `reference_encoding: true` is what the reference encoder writes for
`decoded` with that `hint` and `level`. One with `false` was made by hand, and no encoder
writes it, but decoders must accept it: `varint_overlong` (non-minimal varints, spec section
4), `pco_no_values` and `decimal_no_values` (pcodec files without numbers) and `zstd_empty_frame` (a frame without
content), the last two for expected size 0 (spec section 3); `hint` and `level` are then
those a checker re-encodes `decoded` with, which gives another payload.

### Payload modes in the file manifest

The `modes` of a column count its PCODEC pages by the mode byte of their payload: `RAW`,
`PCO`, `DECIMAL` and `ZSTD`. Two more labels are `tparquet inspect`'s, not payload modes:
`UNCOMPRESSED` is a `DATA_PAGE_V2` page stored with `is_compressed = false` (no payload at
all), and `EMPTY` is a page whose expected size is 0 (a V2 page whose values are all null): it
has a payload, but readers built on arrow-rs never decode it (spec section 3).

The files cover every physical type PCODEC models, required and optional; lists; a nested
struct; timestamps (ms, us, ns; with and without time zones), date32, time64 and decimals
stored as INT64 and INT32; decimal-valued floats (DECIMAL payloads, with exceptions); NaN
payloads, signed zeros, infinities and subnormals; a row group without rows; several row
groups; DATA_PAGE (V1) and DATA_PAGE_V2 pages, dictionary pages, a V2 page stored
uncompressed; PCODEC numeric columns next to dictionary-encoded ZSTD strings; RAW, PCO,
DECIMAL and ZSTD payloads; and one standard (non-PCODEC) control file, `standard_control`.

They also cover maps (null, empty, null values; `maps`); lists of structs of lists of structs
with a null and an empty value at every level (`deep_nesting`); dictionary-encoded PCODEC
chunks that fall back to `PLAIN` part way (`dictionary_fallback`); every pcodec level
(`pcodec_levels`); one column per payload mode side by side (`payload_modes`); every integer
logical type stored in INT32 and INT64 at its extremes, and decimals at the precision bounds of
both (`integer_types`); Auto choosing PCODEC for some numeric columns and the base codec for
another (`auto_choice`, see the codecs in the manifest); a page of more than 2^18 values, whose
payload holds two pcodec chunks (`large_page`); page statistics in the column index and page
headers, bloom filters and sorting columns (`statistics`); pages whose values are all null
(`EMPTY`) and an empty dictionary page (`all_null_pages`); and the caller's key-value pairs next
to `parquet.pcodec.version` (`key_value_metadata`).

A Parquet reader that does not know PCODEC must fail on every file but `standard_control`
(`acceptance/rust-upstream` checks this for the upstream Rust `parquet` crate).

## Canonical JSON

One line per row, `\n`-terminated, in file order across row groups; an empty file gives an
empty `.jsonl`. Each line is one JSON object, written compactly: no whitespace outside strings,
`,` between members and elements, `:` between key and value.

- The keys are the top-level fields, in schema order, each exactly once.
- `null` for a null value, at any level.
- Integers of every width, signed or unsigned, and everything stored as an integer —
  timestamps (any unit, any time zone: the stored value, not converted), dates, times,
  durations, and decimals (the unscaled integer) — are JSON numbers in plain decimal notation:
  `-12`, `1735689600000`, `18446744073709551615`. No exponent, no fraction, no leading `+`.
- Floats are strings holding their IEEE 754 bits as lower-case hex with a `0x` prefix, zero
  padded: 16 hex digits for doubles, `"0x3ff0000000000000"` for 1.0, 8 for floats,
  `"0x3f800000"` for 1.0f (4 for half floats, `"0x3c00"`). NaN payloads and signed zeros are
  kept bit for bit: `-0.0` is `"0x8000000000000000"`.
- Intervals, of every interval type (year-month, day-time and month-day-nano), are objects
  `{"months":m,"days":d,"nanoseconds":n}` with these three members in this order, each an
  integer as above: the type's own fields, `0` for a field it does not have, and day-time
  milliseconds as nanoseconds (times 1000000, which is exact). One month is
  `{"months":1,"days":0,"nanoseconds":0}`, in a year-month or in a month-day-nano array: the
  same interval has the same form whichever interval type a reader gives a Parquet `INTERVAL`
  column.
- Booleans are `true` and `false`.
- Strings are JSON strings: `"` and `\` are escaped as `\"` and `\\`; U+0008, U+0009, U+000A,
  U+000C and U+000D as `\b`, `\t`, `\n`, `\f`, `\r`; the other characters below U+0020 as
  `\u00XX` with lower-case hex; every other character, including non-ASCII, is written as
  itself in UTF-8 (no `\u` escapes, `/` is not escaped).
- Binary values (binary, large binary, fixed-size binary) are strings holding their standard
  base64 (RFC 4648 section 4, with `=` padding).
- Lists (of any kind) are JSON arrays of their elements.
- Structs are JSON objects with their fields in schema order.
- Maps are arrays of entry objects, `{"key":...,"value":...}`, with the entry struct's field
  names, in stored order.
- Dictionary-encoded values are written as their decoded value.

Two rows are equal exactly when their canonical JSON lines are equal byte for byte, which is
how `tparquet verify` compares files.
