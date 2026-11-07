# tparquet on real market data

The [acceptance test](../rust/tparquet/tests/acceptance.rs) measures tparquet on synthetic data.
This benchmark measures it on the sample and preview data that market data providers publish
for evaluation, grouped by data type: trades, quotes, order books, bars, metrics and
derivatives. One group is not market data: the DevOps server metrics of
[TSBS](https://github.com/timescale/tsbs), the data that metric stores are benchmarked with
(see [Observability data](#observability-data-tsbs)).

```sh
make bench            # from the repository root: downloads the samples, writes RESULTS.md
```

## Results

From the last run (31 datasets, 151M rows, 1.2 GB downloaded). Every dataset is in
[`RESULTS.md`](RESULTS.md), and the codec and encoding of every column are in
[`results.json`](results.json).

| Data type | Datasets | Rows | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned |
|---|---:|---:|---:|---:|---:|---:|---:|
| Trades | 5 | 138.9M | 737.0 MB | 520.0 MB | 362.2 MB | **0.49×** | **0.70×** |
| Quotes (L1) | 3 | 6.8M | 80.6 MB | 63.3 MB | 51.0 MB | **0.63×** | **0.81×** |
| Order book (L2) | 3 | 5.5M | 126.4 MB | 109.2 MB | 58.6 MB | **0.46×** | **0.54×** |
| Bars (OHLCV) | 1 | 8.9K | 296 KB | 160 KB | 108 KB | **0.36×** | **0.67×** |
| Metrics (5m bars) | 18 | 161K | 10.5 MB | 6.4 MB | 5.8 MB | **0.55×** | **0.91×** |
| Derivatives | 1 | 119K | 2.2 MB | 1.6 MB | 882 KB | **0.40×** | **0.55×** |
| **All** | 31 | 151.5M | 957.1 MB | 700.7 MB | 478.6 MB | **0.50×** | **0.68×** |

- tparquet wins on every dataset, against both baselines. The range is 0.17× to 0.70× of
  untuned ZSTD(9), and 0.49× to 0.96× of tuned ZSTD(9).
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

| Data type | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---:|---:|---:|---:|---:|---:|---:|
| Trades | 7.49 GB | 100 | 159 | 152 | 988 | 1,191 | 930 |
| Quotes (L1) | 482 MB | 91 | 160 | 68 | 1,053 | 1,533 | 1,002 |
| Order book (L2) | 1.46 GB | 141 | 136 | 51 | 1,346 | 1,340 | 883 |
| Bars (OHLCV) | 436 KB | 32 | 87 | 11 | 383 | 542 | 749 |
| Metrics (5m bars) | 10.5 MB | 55 | 84 | 8 | 302 | 502 | 587 |
| Derivatives | 12.2 MB | 105 | 93 | 38 | 1,038 | 1,440 | 1,387 |
| **All** | 9.46 GB | 104 | 155 | 109 | 1,031 | 1,225 | 925 |

- **Writing.** Over everything, tparquet writes at 109 MB/s: about as fast as ZSTD(9) (104)
  and 0.70× of tuned ZSTD(9) (155). On the large trade files it matches the tuned writer
  (152 against 159 MB/s) and is 1.5× as fast as ZSTD(9). The order books write about 3×
  slower (51 MB/s), because each of their 80 to 100 numeric columns is PCODEC. The small
  files (bars, metrics: 9K rows each) write at 8 to 11 MB/s. On them the cost of the
  per-column choice dominates: the first batch is compressed both ways.
- **Reading.** tparquet reads at 925 MB/s: 0.90× of ZSTD(9) and 0.76× of tuned ZSTD(9). Trades
  and quotes read at 0.94× and 0.95× of ZSTD(9). The order books read at 0.66×, because
  decoding pcodec costs more per value than ZSTD on these columns. The small bar and metric
  files read fastest as tparquet, since there is less to read.

## What is measured

Every dataset is read as its provider delivers it, typed as the provider documents it, and
written three times:

| File | Writer | Codecs and encodings |
|---|---|---|
| **ZSTD(9)** | tparquet's writer, `policy="base"` | ZSTD level 9 on every column, writer defaults otherwise (dictionary, then PLAIN): what you get without tuning |
| **ZSTD(9) tuned** | pyarrow | ZSTD level 9, and per column the smallest on the first batch of the writer's default (dictionary, then PLAIN) and the encoding Parquet has for the type: DELTA_BINARY_PACKED for integers and timestamps, BYTE_STREAM_SPLIT for floating point, DELTA_BYTE_ARRAY or DELTA_LENGTH_BYTE_ARRAY for strings. This is what a careful writer does, and how several providers' own files are written |
| **tparquet** | tparquet's writer, defaults | PCODEC or ZSTD level 3 per column, whichever is smaller on the first batch |

Each writer gets batches of 2²⁰ rows (one row group). The run checks that the three
files hold the same values (floats bit for bit), then reads each back in full with
tparquet's reader (the same reader for all three, so only the format differs). The *As
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
it, so every run downloads it from the provider. The TSBS data is not downloaded but generated
into the same cache.

### Observability data (TSBS)

The market data above is what the providers sell. Metric stores are benchmarked on something
else: [TSBS](https://github.com/timescale/tsbs), the Time Series Benchmark Suite that
TimescaleDB, InfluxDB, VictoriaMetrics, QuestDB and ClickHouse publish their numbers on. Its
`devops` use case simulates a fleet of servers that each report system metrics every 10
seconds. [`tsbs.py`](src/tpq_bench/tsbs.py) generates it, a port of the Go generator
(`pkg/data/usecases/devops`): 100 hosts, one day (2016-01-01) at 10 s, seed 123, so 864,000
rows per measurement.

| Measurement | Fields | Columns beyond the host's ten tags |
|---|---:|---|
| `cpu` (TSBS' cpu-only use case) | 10 | `usage_user`, `usage_system`, ... as int64 percentages |
| `diskio`, `net` | 7, 8 | int64 counters; the tag `serial` or `interface` |
| `disk`, `mem` | 7, 9 | int64 sizes and percentages; `mem` has three float64 percentages |
| `kernel`, `nginx`, `postgresl`, `redis` | 6, 7, 16, 31 | int64 counters and gauges; `nginx` and `redis` have `port` and `server` tags |

Nine measurements, 101 fields, as TSBS counts them. Every column is as TSBS types it: the
host's tags (`hostname`, `region`, `datacenter`, `rack`, `os`, `arch`, `team`, `service`,
`service_version`, `service_environment`) are strings, the values are int64 (TSBS truncates
every walk to an integer) except `mem`'s percentages, and `time` is a UTC timestamp in
nanoseconds. Rows are in the order the generator writes them: every host's report of a tick
before the next tick. The `cpu` measurement is also measured with each host's series together
(by host, then time), as a store keyed by series lays it out.

Read these results with three things in mind:

- **The data is synthetic.** The values are random walks (clamped, monotonic or free), as in
  TSBS, not telemetry from real servers. They show how tparquet does on the data metric stores
  are benchmarked on, not on what a fleet reports; real counters and gauges are less regular
  (or more) than a random walk.
- **It is a port, not the generator's output.** The model, the schema, the tags and the ranges
  are TSBS', but the numbers come from Python's random number generator, so they are not the
  ones `tsbs_generate_data --seed=123` writes. Compare sizes with each other, not with files
  made by the Go tool.
- **"As delivered" is the generated file**: Parquet written by pyarrow with its defaults
  (Snappy, dictionaries), not TSBS' text formats.

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
[`loaders.py`](src/tpq_bench/loaders.py) if the format is new. More TSBS data (more hosts, a
longer span, another seed) is a `Tsbs(...)` fetcher with other arguments.
