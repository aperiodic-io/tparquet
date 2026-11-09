# tparquet as a storage tier

Until PCODEC is a standard Parquet codec ([`spec/UPSTREAM.md`](../spec/UPSTREAM.md)), only
readers with the codec can read a tparquet file. The arrangement that gets the savings without
paying for that is a storage tier: tparquet files where you control every reader, and standard
Parquet wherever data leaves that tier.

```
 producers ──► tparquet tier ──────────────► decode at the edge ──► standard Parquet
               (archive, lake, history:      (tparquet decode,       (customers, other teams,
                every reader has the codec)   transcode, verify)      managed warehouses, BI)
```

## Inside the tier

Put data here when it is mostly numeric (timestamps, prices, sizes, ids, readings), kept for
a long time, and read by a small set of engines you operate: Python, Polars, DuckDB with the
extension, Go, Rust, DataFusion, or Spark and Trino with the forked jars. Typical tiers:

- the history of tick, quote and book data behind a research environment;
- the archive under a hot store that holds only recent days;
- a data vendor's own copy, of which customers receive standard files.

Every engine that reads the tier needs the codec, and its version is pinned by the fork
(README, *Packages*). Count every engine before choosing the tier, scheduled jobs and ad hoc
notebooks included.

## At the edge

Every path out of the tier writes standard Parquet. Each tool below writes a copy that any
Parquet reader opens, with ZSTD level 3 (or the codec you choose) in place of PCODEC. The
schema, key-value metadata, row groups, sort order, statistics settings and bloom filters stay
as they were.

CLI, one file:

```sh
tparquet decode trades.parquet out/trades.parquet             # PCODEC -> zstd(3)
tparquet decode --base 'zstd(9)' trades.parquet out/trades.parquet
tparquet verify trades.parquet out/trades.parquet             # same values, floats bit for bit
```

CLI, a directory, keeping its layout (partitions included):

```sh
cd lake && find . -name '*.parquet' | while read -r f; do
  mkdir -p "../export/$(dirname "$f")"
  tparquet decode "$f" "../export/$f" && tparquet verify "$f" "../export/$f" >/dev/null
done
```

Python:

```python
import tparquet as tpq

tpq.transcode("trades.parquet", "out/trades.parquet", target="standard", base="zstd(9)")
```

A query whose result leaves the tier, in DuckDB:

```sql
COPY (SELECT * FROM read_tparquet('trades/*.parquet') WHERE day = '2025-06-01')
TO 'out/trades.parquet' (FORMAT parquet, COMPRESSION zstd);
```

DuckDB's own writer does not keep NaN payloads (DuckDB README, *Writing*); use
`tparquet decode` or `transcode` when the copy must be bit for bit.

The copy is larger than the tparquet file. It is not necessarily the size of the file the data
came from either: `decode` uses the base codec with the writer's default encodings, not
encodings tuned per column. Run `tparquet inspect` on a sample to see the sizes.

## When it does not pay

- **Many engines read the data, or engines you cannot patch read it**: Snowflake, BigQuery,
  Athena, Databricks, BI tools. Every one needs a standard copy, and two copies cost more than
  one ZSTD copy.
- **The data is read much more than it is stored**: tparquet reads about 25% slower than
  Parquet with tuned encodings (925 against 1,225 MB/s on one thread, `bench/RESULTS.md`), and
  writes about 3× slower on wide all-numeric tables.
- **The data is mostly strings or nested lists**: those columns keep ZSTD, and order books
  stored as lists come out slightly larger than ZSTD.
- **Your files already use tuned encodings and storage is a small cost**: against tuned
  ZSTD(9) Parquet the saving is about a third (0.68×), not a half.

## Checklist

1. Run `tparquet encode`, `inspect` and `verify` on a representative file of each dataset,
   and compare the size with your current files, not with defaults.
2. List every reader of the tier. Each one is an engine with the codec, or a consumer of the
   edge's standard copy.
3. Make the edge a step of the pipeline (`decode` and `verify`), not something done by hand.
4. Keep a way back: `tparquet decode` over the whole tier gives standard Parquet again.
