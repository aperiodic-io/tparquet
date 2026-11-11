# tparquet on real market data

The [acceptance test](../rust/tparquet/tests/acceptance.rs) measures tparquet on synthetic data.
This benchmark measures it on the sample and preview data that market data providers publish
for evaluation, grouped by data type: trades, quotes, order books, bars, metrics and
derivatives.

```sh
make bench            # from the repository root: downloads the samples, writes RESULTS.md
```

## Results

From the last run (31 datasets, 151M rows, 1.2 GB downloaded). Every dataset is in
[`RESULTS.md`](RESULTS.md), and the codec and encoding of every column are in
[`results.json`](results.json).

| Data type | Datasets | Rows | ZSTD(9) | ZSTD(9) tuned | ZSTD(9) ALP | tparquet | tparquet / ZSTD(9) | tparquet / tuned | tparquet / ALP |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Trades | 5 | 138.9M | 737.0 MB | 520.0 MB | 480.6 MB | 362.2 MB | **0.49×** | **0.70×** | **0.75×** |
| Quotes (L1) | 3 | 6.8M | 80.6 MB | 63.3 MB | 63.3 MB | 51.0 MB | **0.63×** | **0.81×** | **0.81×** |
| Order book (L2) | 3 | 5.5M | 126.4 MB | 109.2 MB | 95.7 MB | 58.6 MB | **0.46×** | **0.54×** | **0.61×** |
| Bars (OHLCV) | 1 | 8.9K | 296 KB | 160 KB | 150 KB | 108 KB | **0.36×** | **0.67×** | **0.72×** |
| Metrics (5m bars) | 18 | 161K | 10.5 MB | 6.4 MB | 8.1 MB | 5.8 MB | **0.55×** | **0.91×** | **0.72×** |
| Derivatives | 1 | 119K | 2.2 MB | 1.6 MB | 1.7 MB | 882 KB | **0.40×** | **0.55×** | **0.51×** |
| **All** | 31 | 151.5M | 957.1 MB | 700.7 MB | 649.5 MB | 478.6 MB | **0.50×** | **0.68×** | **0.74×** |

- tparquet wins on every dataset, against all three baselines. The range is 0.17× to 0.70× of
  untuned ZSTD(9), 0.49× to 0.96× of tuned ZSTD(9), and 0.46× to 0.86× of ZSTD(9) ALP.
- **ALP.** Over everything, ALP on the floating-point columns makes the files 7% smaller than
  the tuned baseline (649.5 against 700.7 MB), and tparquet is 0.74× of it. ALP gains on the crypto
  trade, quote and book files, where prices and sizes are short decimals: 0.84× of tuned on the
  Tardis.dev book snapshots, 0.88× on the AVAX book updates, 0.92× on the Binance trades. It loses on 21 of the 31
  datasets, by 2% to 48%: the 5m metrics (ratios, volatilities, float32 aggregates that are not
  short decimals, where the dictionary or BYTE_STREAM_SPLIT was smaller), the DOGE book
  (1.12×) and the NYSE files (1.02× to 1.07×). ALP replaces the tuned encoding on every float
  column; a writer that kept the smaller of the two per column would never lose to tuned.
- The order books gain the most: about half of tuned ZSTD(9), because every price and size
  level is a numeric column.
- The 5m metrics gain the least against tuned ZSTD(9) (0.91×). They are aggregates such as
  ratios, volatilities and percentages, often float32, and leave pcodec little structure to use.
- NYSE quotes gain less than the crypto quotes (0.88× of tuned). One file holds many
  symbols, interleaved in time, so prices jump between rows.
- Strings keep tparquet's base codec, ZSTD(3) with the default encoding. On string trade ids
  (Aperiodic, Tardis.dev) the tuned baseline's DELTA_BYTE_ARRAY is smaller, so tparquet's
  total there would improve with a better base encoding for strings.

Speed, in MB/s of Arrow data (higher is faster; one thread per writer and reader):

| Data type | Arrow data | Write ZSTD(9) | Write tuned | Write ALP | Write tparquet | Read ZSTD(9) | Read tuned | Read ALP | Read tparquet |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Trades | 7.49 GB | 95 | 145 | 175 | 144 | 881 | 1,127 | 1,170 | 914 |
| Quotes (L1) | 482 MB | 84 | 147 | 160 | 58 | 872 | 1,173 | 1,335 | 953 |
| Order book (L2) | 1.46 GB | 118 | 122 | 146 | 47 | 1,196 | 1,136 | 1,656 | 968 |
| Bars (OHLCV) | 436 KB | 33 | 85 | 56 | 10 | 351 | 505 | 658 | 778 |
| Metrics (5m bars) | 10.5 MB | 60 | 92 | 37 | 9 | 293 | 487 | 363 | 553 |
| Derivatives | 12.2 MB | 90 | 87 | 149 | 49 | 953 | 1,142 | 1,393 | 1,045 |
| **All** | 9.46 GB | 97 | 141 | 169 | 102 | 916 | 1,129 | 1,231 | 924 |

- **Writing.** Over everything, tparquet writes at 102 MB/s: about as fast as ZSTD(9) (97),
  0.72× of tuned ZSTD(9) (141) and 0.60× of ZSTD(9) ALP (169), the fastest writer. On the large
  trade files it matches the tuned writer (144 against 145 MB/s). The order books write about
  3× slower than ALP (47 against 146 MB/s), because each of their 80 to 100 numeric columns
  is PCODEC. The small files (bars, metrics: 9K rows each) write at 9 to 10 MB/s. On them the
  cost of the per-column choice dominates: the first batch is compressed both ways.
- **Reading.** tparquet reads at 924 MB/s: about as fast as ZSTD(9) (916), 0.82× of tuned
  ZSTD(9) and 0.75× of ZSTD(9) ALP (1,231 MB/s, the fastest). ALP's lead is largest on the order
  books (1,656 against 968 MB/s), where decoding pcodec costs more per value. The small bar and
  metric files read fastest as tparquet, since there is less to read.
- The ALP file is written by the `parquet` crate (Rust) and the tuned file by pyarrow (C++), so
  the write speeds of these two compare writers as well as encodings. All four files are read by
  the same reader.

## What is measured

Every dataset is read as its provider delivers it, typed as the provider documents it, and
written four times:

| File | Writer | Codecs and encodings |
|---|---|---|
| **ZSTD(9)** | tparquet's writer, `policy="base"` | ZSTD level 9 on every column, writer defaults otherwise (dictionary, then PLAIN): what you get without tuning |
| **ZSTD(9) tuned** | pyarrow | ZSTD level 9, and per column the smallest on the first batch of the writer's default (dictionary, then PLAIN) and the encoding Parquet has for the type: DELTA_BINARY_PACKED for integers and timestamps, BYTE_STREAM_SPLIT for floating point, DELTA_BYTE_ARRAY or DELTA_LENGTH_BYTE_ARRAY for strings. This is what a careful writer does, and how several providers' own files are written |
| **ZSTD(9) ALP** | tparquet's writer, `policy="base"` | ZSTD level 9, the tuned file's encodings, but ALP ([Adaptive Lossless floating-Point](https://github.com/apache/parquet-format/blob/master/Encodings.md), Parquet encoding 10) on every FLOAT and DOUBLE column. pyarrow 26 does not write ALP; the `parquet` crate (60.1) does, so this file comes from tparquet's writer with PCODEC off and per-column `encodings` |
| **tparquet** | tparquet's writer, defaults | PCODEC or ZSTD level 3 per column, whichever is smaller on the first batch |

Each writer gets batches of 2²⁰ rows (one row group). The run checks that the four
files hold the same values (floats bit for bit), then reads each back in full with
tparquet's reader (the same reader for all four, so only the format differs). The *As
delivered* column is the size of the provider's file (CSV, gzip; Parquet with their codec).

Speeds are MB/s of Arrow data: the in-memory size of the batches written, divided by the
seconds the writer spent on them (its `write` calls and the footer), or by the seconds of the
fastest of three full reads. The parsing of the source is not timed, nor is the tuned
baseline's choice of encodings. Every writer and reader runs on one thread. Speeds come from
one run on one machine: compare them with each other, not across machines.

## The data

| Provider | What | Format | Access |
|---|---|---|---|
| [Aperiodic](https://aperiodic.io) | raw trades of the BTC perpetual on Binance and OKX, 2025-06 (80M and 54M rows); the 19 metric datasets (flow, liquidity, impact, funding, basis...) for BTC-USDT on Binance, 5m bars, 2025-05 | Parquet | the free preview, with the public demo key |
| [Tardis.dev](https://tardis.dev) | Binance USDⓈ-M BTCUSDT, 2025-06-01: trades, quotes, 25-level book snapshots, derivative ticker | CSV, gzip | the first day of each month is free, no key |
| [Crypto Lake](https://crypto-lake.com) | Binance spot: BTC-USDT trades and top of book (2022-10-01), DOGE-USDT 20-level book snapshots (2024-09-24), AVAX-USDT book updates (2024-04-01) | Parquet | the public sample bucket (`lakeapi.use_sample_data(anonymous_access=True)`) |
| [NYSE](https://ftp.nyse.com/Historical%20Data%20Samples/) | TAQ NYSE Trades and BBO, 2026-07-01, symbol range 1 (US equities) | CSV, gzip | the public historical data samples |

About 1.2 GB is downloaded into `bench/data/` (git-ignored; `TPQ_BENCH_DATA` or `--data`
moves it). None of it is in the repository: the providers' terms do not allow redistributing
it, so every run downloads it from the provider.

## Running part of it

```sh
cd bench
uv sync
uv run --no-sync tpq-bench list                  # every source, and whether it is cached
uv run --no-sync tpq-bench fetch                 # download only
uv run --no-sync tpq-bench run l2 nyse           # data types, providers or parts of cache keys
uv run --no-sync tpq-bench run --markdown out.md --json out.json
```

## Development

```sh
uv run --no-sync pytest                          # offline: tiny synthetic files, no provider data
uv run --no-sync ruff check src tests && uv run --no-sync ruff format --check src tests
uv run --no-sync mypy
uv run --no-sync python scripts/mutmut_check.py     # fails unless every mutant is killed
```

New sources go in [`catalog.py`](src/tpq_bench/catalog.py), with a loader in
[`loaders.py`](src/tpq_bench/loaders.py) if the format is new.
