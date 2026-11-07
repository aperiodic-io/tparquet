# Results

The last run of `make bench` (2026-11-06; tparquet 0f2845d, pyarrow 26.0.1,
Python 3.14.6, on a 4-vCPU cloud VM). How it is measured: [README.md](README.md#what-is-measured).
Size ratios below 1 mean tparquet is smaller; speeds are MB/s of Arrow data, higher is faster.

The Observability (TSBS) rows come from a separate run, `tpq-bench run observability` on 2026-11-07 (tparquet e550317, pyarrow 26.0.1, Python 3.14.6, 4 vCPUs), merged into the results of the run above; the other rows are unchanged. Compare speeds within a group, and across groups only knowing they are two runs. The TSBS data is generated, not delivered: see [README.md](README.md#observability-data-tsbs).

| Data type | Datasets | Rows | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned |
|---|---:|---:|---:|---:|---:|---:|---:|
| Trades | 5 | 138,865,292 | 737.0 MB | 520.0 MB | 362.2 MB | **0.491×** | **0.696×** |
| Quotes (L1, top of book) | 3 | 6,832,005 | 80.6 MB | 63.3 MB | 51.0 MB | **0.633×** | **0.806×** |
| Order book (L2: snapshots and updates) | 3 | 5,513,803 | 126.4 MB | 109.2 MB | 58.6 MB | **0.464×** | **0.537×** |
| Bars (OHLCV) | 1 | 8,928 | 296 KB | 160 KB | 108 KB | **0.364×** | **0.673×** |
| Metrics (5m bars) | 18 | 160,704 | 10.5 MB | 6.4 MB | 5.8 MB | **0.551×** | **0.905×** |
| Derivatives (funding, mark, index, open interest) | 1 | 118,524 | 2.2 MB | 1.6 MB | 882 KB | **0.395×** | **0.553×** |
| Observability (TSBS DevOps server metrics) | 10 | 8,640,000 | 114.3 MB | 83.6 MB | 77.5 MB | **0.678×** | **0.926×** |
| **All** | 41 | 160,139,256 | 1.07 GB | 784.3 MB | 556.1 MB | **0.519×** | **0.709×** |

Speed, in MB/s of Arrow data (the columns' size in memory):

| Data type | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---:|---:|---:|---:|---:|---:|---:|
| Trades | 7.49 GB | 100 | 159 | 152 | 988 | 1,191 | 930 |
| Quotes (L1, top of book) | 481.8 MB | 91 | 160 | 68 | 1,053 | 1,533 | 1,002 |
| Order book (L2: snapshots and updates) | 1.46 GB | 141 | 136 | 51 | 1,346 | 1,340 | 883 |
| Bars (OHLCV) | 436 KB | 32 | 87 | 11 | 383 | 542 | 749 |
| Metrics (5m bars) | 10.5 MB | 55 | 84 | 8 | 302 | 502 | 587 |
| Derivatives (funding, mark, index, open interest) | 12.2 MB | 105 | 93 | 38 | 1,038 | 1,440 | 1,387 |
| Observability (TSBS DevOps server metrics) | 1.78 GB | 202 | 365 | 90 | 1,758 | 2,282 | 1,634 |
| **All** | 11.23 GB | 113 | 171 | 106 | 1,103 | 1,322 | 993 |

### Trades

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Aperiodic | BTC-USDT perpetual trades on Binance, 2025-06 (raw, Parquet) | 79,684,576 | 248.6 MB | 399.4 MB | 280.2 MB | 193.1 MB | 0.484× | 0.689× | 4 of 6 |
| Aperiodic | BTC-USDT perpetual trades on OKX, 2025-06 (raw, Parquet) | 54,282,320 | 189.1 MB | 294.8 MB | 208.5 MB | 147.1 MB | 0.499× | 0.706× | 4 of 6 |
| Tardis.dev | trades, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 1,959,946 | 17.8 MB | 11.0 MB | 7.5 MB | 5.2 MB | 0.474× | 0.691× | 4 of 8 |
| Crypto Lake | trades, Binance BTC-USDT, 2022-10-01 (Parquet) | 2,190,208 | 63.4 MB | 22.5 MB | 17.4 MB | 11.9 MB | 0.529× | 0.685× | 5 of 6 |
| NYSE | TAQ NYSE trades, symbol range 1, 2026-07-01 (CSV) | 748,242 | 12.9 MB | 9.3 MB | 6.4 MB | 4.8 MB | 0.513× | 0.748× | 6 of 11 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Aperiodic | BTC-USDT perpetual trades on Binance, 2025-06 (raw, Parquet) | 4.26 GB | 100 | 159 | 164 | 1,000 | 1,167 | 942 |
| Aperiodic | BTC-USDT perpetual trades on OKX, 2025-06 (raw, Parquet) | 2.90 GB | 101 | 158 | 155 | 969 | 1,205 | 898 |
| Tardis.dev | trades, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 163.7 MB | 154 | 232 | 108 | 1,274 | 1,689 | 1,321 |
| Crypto Lake | trades, Binance BTC-USDT, 2022-10-01 (Parquet) | 104.0 MB | 63 | 130 | 53 | 795 | 1,271 | 907 |
| NYSE | TAQ NYSE trades, symbol range 1, 2026-07-01 (CSV) | 56.2 MB | 87 | 136 | 46 | 879 | 1,287 | 968 |

### Quotes (L1, top of book)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Tardis.dev | quotes, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 1,210,001 | 16.3 MB | 13.4 MB | 8.9 MB | 5.1 MB | 0.380× | 0.573× | 6 of 8 |
| Crypto Lake | top of book, Binance BTC-USDT, 2022-10-01 (Parquet) | 713,858 | 11.2 MB | 7.8 MB | 7.4 MB | 4.5 MB | 0.574× | 0.610× | 5 of 5 |
| NYSE | TAQ NYSE BBO quotes, symbol range 1, 2026-07-01 (CSV) | 4,908,146 | 74.6 MB | 59.4 MB | 47.0 MB | 41.4 MB | 0.698× | 0.881× | 7 of 10 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Tardis.dev | quotes, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 94.4 MB | 138 | 182 | 60 | 1,442 | 2,004 | 1,315 |
| Crypto Lake | top of book, Binance BTC-USDT, 2022-10-01 (Parquet) | 28.6 MB | 89 | 113 | 23 | 1,122 | 1,303 | 955 |
| NYSE | TAQ NYSE BBO quotes, symbol range 1, 2026-07-01 (CSV) | 358.9 MB | 83 | 161 | 85 | 979 | 1,463 | 946 |

### Order book (L2: snapshots and updates)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Tardis.dev | 25-level book snapshots, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 1,429,796 | 53.8 MB | 81.5 MB | 75.9 MB | 38.2 MB | 0.468× | 0.503× | 102 of 104 |
| Crypto Lake | 20-level book snapshots, Binance DOGE-USDT, 2024-09-24 (Parquet) | 140,676 | 27.2 MB | 15.2 MB | 11.1 MB | 5.7 MB | 0.374× | 0.513× | 83 of 83 |
| Crypto Lake | book updates, Binance AVAX-USDT, 2024-04-01 (Parquet) | 3,943,331 | 46.4 MB | 29.7 MB | 22.2 MB | 14.8 MB | 0.497× | 0.665× | 5 of 6 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Tardis.dev | 25-level book snapshots, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 1.21 GB | 161 | 161 | 52 | 1,466 | 1,404 | 879 |
| Crypto Lake | 20-level book snapshots, Binance DOGE-USDT, 2024-09-24 (Parquet) | 93.4 MB | 102 | 88 | 32 | 910 | 865 | 850 |
| Crypto Lake | book updates, Binance AVAX-USDT, 2024-04-01 (Parquet) | 158.2 MB | 83 | 73 | 62 | 1,000 | 1,307 | 936 |

### Bars (OHLCV)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Aperiodic | ohlcv, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 159 KB | 296 KB | 160 KB | 108 KB | 0.364× | 0.673× | 7 of 7 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Aperiodic | ohlcv, BTC-USDT perpetual on Binance, 5m, 2025-05 | 436 KB | 32 | 87 | 11 | 383 | 542 | 749 |

### Metrics (5m bars)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Aperiodic | vtwap, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 108 KB | 205 KB | 109 KB | 101 KB | 0.494× | 0.928× | 3 of 3 |
| Aperiodic | basis, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 223 KB | 390 KB | 224 KB | 204 KB | 0.522× | 0.909× | 9 of 9 |
| Aperiodic | funding, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 26 KB | 80 KB | 27 KB | 13 KB | 0.166× | 0.493× | 3 of 4 |
| Aperiodic | open_interest, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 272 KB | 391 KB | 266 KB | 227 KB | 0.582× | 0.855× | 6 of 6 |
| Aperiodic | derivative_price, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 512 KB | 696 KB | 513 KB | 445 KB | 0.640× | 0.869× | 10 of 10 |
| Aperiodic | flow, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 789 KB | 1.3 MB | 786 KB | 733 KB | 0.562× | 0.932× | 37 of 37 |
| Aperiodic | trade_size, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 556 KB | 960 KB | 554 KB | 520 KB | 0.542× | 0.939× | 25 of 25 |
| Aperiodic | impact, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 157 KB | 285 KB | 158 KB | 148 KB | 0.520× | 0.938× | 6 of 6 |
| Aperiodic | range, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 302 KB | 509 KB | 304 KB | 271 KB | 0.533× | 0.892× | 11 of 11 |
| Aperiodic | updownticks, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 364 KB | 629 KB | 362 KB | 319 KB | 0.507× | 0.881× | 15 of 15 |
| Aperiodic | run_structure, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 182 KB | 309 KB | 176 KB | 160 KB | 0.519× | 0.913× | 8 of 8 |
| Aperiodic | returns, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 181 KB | 331 KB | 182 KB | 174 KB | 0.528× | 0.960× | 7 of 7 |
| Aperiodic | slippage, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 275 KB | 478 KB | 276 KB | 261 KB | 0.547× | 0.947× | 11 of 11 |
| Aperiodic | l1_price, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 488 KB | 668 KB | 443 KB | 345 KB | 0.516× | 0.779× | 13 of 13 |
| Aperiodic | l1_imbalance, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 307 KB | 518 KB | 308 KB | 275 KB | 0.530× | 0.892× | 11 of 11 |
| Aperiodic | l1_liquidity, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 361 KB | 636 KB | 363 KB | 335 KB | 0.526× | 0.921× | 15 of 15 |
| Aperiodic | l2_imbalance, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 760 KB | 1.2 MB | 764 KB | 713 KB | 0.594× | 0.934× | 25 of 25 |
| Aperiodic | l2_liquidity, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 598 KB | 942 KB | 601 KB | 558 KB | 0.592× | 0.928× | 17 of 17 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Aperiodic | vtwap, BTC-USDT perpetual on Binance, 5m, 2025-05 | 218 KB | 70 | 303 | 14 | 393 | 776 | 709 |
| Aperiodic | basis, BTC-USDT perpetual on Binance, 5m, 2025-05 | 367 KB | 60 | 81 | 8 | 280 | 588 | 623 |
| Aperiodic | funding, BTC-USDT perpetual on Binance, 5m, 2025-05 | 183 KB | 49 | 96 | 17 | 393 | 717 | 697 |
| Aperiodic | open_interest, BTC-USDT perpetual on Binance, 5m, 2025-05 | 400 KB | 35 | 56 | 12 | 302 | 415 | 797 |
| Aperiodic | derivative_price, BTC-USDT perpetual on Binance, 5m, 2025-05 | 725 KB | 52 | 94 | 13 | 369 | 528 | 802 |
| Aperiodic | flow, BTC-USDT perpetual on Binance, 5m, 2025-05 | 1.4 MB | 55 | 74 | 6 | 262 | 405 | 414 |
| Aperiodic | trade_size, BTC-USDT perpetual on Binance, 5m, 2025-05 | 956 KB | 77 | 74 | 7 | 336 | 603 | 591 |
| Aperiodic | impact, BTC-USDT perpetual on Binance, 5m, 2025-05 | 257 KB | 67 | 102 | 11 | 303 | 583 | 689 |
| Aperiodic | range, BTC-USDT perpetual on Binance, 5m, 2025-05 | 512 KB | 58 | 98 | 9 | 331 | 539 | 649 |
| Aperiodic | updownticks, BTC-USDT perpetual on Binance, 5m, 2025-05 | 588 KB | 64 | 97 | 8 | 239 | 483 | 636 |
| Aperiodic | run_structure, BTC-USDT perpetual on Binance, 5m, 2025-05 | 330 KB | 69 | 68 | 10 | 338 | 628 | 691 |
| Aperiodic | returns, BTC-USDT perpetual on Binance, 5m, 2025-05 | 294 KB | 66 | 78 | 9 | 248 | 557 | 649 |
| Aperiodic | slippage, BTC-USDT perpetual on Binance, 5m, 2025-05 | 441 KB | 68 | 74 | 9 | 284 | 567 | 574 |
| Aperiodic | l1_price, BTC-USDT perpetual on Binance, 5m, 2025-05 | 907 KB | 43 | 89 | 12 | 308 | 431 | 585 |
| Aperiodic | l1_imbalance, BTC-USDT perpetual on Binance, 5m, 2025-05 | 441 KB | 42 | 58 | 5 | 242 | 396 | 411 |
| Aperiodic | l1_liquidity, BTC-USDT perpetual on Binance, 5m, 2025-05 | 588 KB | 76 | 105 | 9 | 303 | 542 | 581 |
| Aperiodic | l2_imbalance, BTC-USDT perpetual on Binance, 5m, 2025-05 | 956 KB | 74 | 98 | 7 | 310 | 451 | 624 |
| Aperiodic | l2_liquidity, BTC-USDT perpetual on Binance, 5m, 2025-05 | 947 KB | 40 | 101 | 10 | 359 | 624 | 674 |

### Derivatives (funding, mark, index, open interest)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Tardis.dev | derivative ticker, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 118,524 | 1.8 MB | 2.2 MB | 1.6 MB | 882 KB | 0.395× | 0.553× | 8 of 11 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Tardis.dev | derivative ticker, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 12.2 MB | 105 | 93 | 38 | 1,038 | 1,440 | 1,387 |

### Observability (TSBS DevOps server metrics)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| TSBS | devops cpu, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 8.0 MB | 7.8 MB | 7.7 MB | 7.4 MB | 0.947× | 0.953× | 11 of 21 |
| TSBS | devops diskio, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 17.5 MB | 11.4 MB | 7.0 MB | 6.6 MB | 0.576× | 0.934× | 8 of 19 |
| TSBS | devops disk, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 6.1 MB | 3.7 MB | 2.1 MB | 2.1 MB | 0.560× | 0.964× | 3 of 20 |
| TSBS | devops kernel, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 9.2 MB | 7.6 MB | 5.0 MB | 4.7 MB | 0.617× | 0.930× | 6 of 17 |
| TSBS | devops mem, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 50.1 MB | 37.2 MB | 33.5 MB | 31.2 MB | 0.838× | 0.932× | 9 of 20 |
| TSBS | devops net, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 18.6 MB | 12.9 MB | 8.0 MB | 7.4 MB | 0.575× | 0.926× | 9 of 20 |
| TSBS | devops nginx, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 5.8 MB | 4.7 MB | 3.1 MB | 2.9 MB | 0.624× | 0.936× | 4 of 20 |
| TSBS | devops postgresl, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 4.3 MB | 2.4 MB | 1.5 MB | 1.4 MB | 0.576× | 0.882× | 17 of 27 |
| TSBS | devops redis, 100 hosts, 1 day at 10 s, in time order (generated) | 864,000 | 29.5 MB | 19.6 MB | 12.4 MB | 11.5 MB | 0.586× | 0.927× | 31 of 44 |
| TSBS | devops cpu, 100 hosts, 1 day at 10 s, by host and time (generated) | 864,000 | 9.2 MB | 7.1 MB | 3.2 MB | 2.5 MB | 0.346× | 0.768× | 11 of 21 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| TSBS | devops cpu, 100 hosts, 1 day at 10 s, in time order (generated) | 162.2 MB | 396 | 386 | 94 | 2,381 | 2,419 | 1,632 |
| TSBS | devops diskio, 100 hosts, 1 day at 10 s, in time order (generated) | 154.4 MB | 141 | 328 | 80 | 1,264 | 2,049 | 1,054 |
| TSBS | devops disk, 100 hosts, 1 day at 10 s, in time order (generated) | 159.9 MB | 271 | 362 | 84 | 1,554 | 2,319 | 2,028 |
| TSBS | devops kernel, 100 hosts, 1 day at 10 s, in time order (generated) | 134.5 MB | 367 | 334 | 80 | 1,781 | 2,198 | 1,600 |
| TSBS | devops mem, 100 hosts, 1 day at 10 s, in time order (generated) | 155.3 MB | 86 | 200 | 56 | 1,125 | 1,993 | 1,455 |
| TSBS | devops net, 100 hosts, 1 day at 10 s, in time order (generated) | 155.3 MB | 117 | 346 | 68 | 1,457 | 2,011 | 1,394 |
| TSBS | devops nginx, 100 hosts, 1 day at 10 s, in time order (generated) | 161.7 MB | 488 | 483 | 119 | 2,348 | 2,546 | 1,963 |
| TSBS | devops postgresl, 100 hosts, 1 day at 10 s, in time order (generated) | 203.7 MB | 450 | 540 | 152 | 3,053 | 3,182 | 2,403 |
| TSBS | devops redis, 100 hosts, 1 day at 10 s, in time order (generated) | 327.6 MB | 160 | 368 | 92 | 1,973 | 2,119 | 1,554 |
| TSBS | devops cpu, 100 hosts, 1 day at 10 s, by host and time (generated) | 162.2 MB | 455 | 538 | 140 | 1,694 | 2,364 | 1,962 |
