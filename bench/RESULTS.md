# Results

The last run of `make bench` (2026-11-07; tparquet bb8edf1, pyarrow 26.0.1,
Python 3.13.18, on a 4-vCPU cloud VM). How it is measured: [README.md](README.md#what-is-measured).
Size ratios below 1 mean tparquet is smaller; speeds are MB/s of Arrow data, higher is faster.

| Data type | Datasets | Rows | ZSTD(9) | ZSTD(9) tuned | ZSTD(9) ALP | tparquet | tparquet / ZSTD(9) | tparquet / tuned | tparquet / ALP |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Trades | 5 | 138,865,292 | 737.0 MB | 520.0 MB | 480.6 MB | 362.2 MB | **0.491×** | **0.696×** | **0.754×** |
| Quotes (L1, top of book) | 3 | 6,832,005 | 80.6 MB | 63.3 MB | 63.3 MB | 51.0 MB | **0.633×** | **0.806×** | **0.806×** |
| Order book (L2: snapshots and updates) | 3 | 5,513,803 | 126.4 MB | 109.2 MB | 95.7 MB | 58.6 MB | **0.464×** | **0.537×** | **0.613×** |
| Bars (OHLCV) | 1 | 8,928 | 296 KB | 160 KB | 150 KB | 108 KB | **0.364×** | **0.673×** | **0.717×** |
| Metrics (5m bars) | 18 | 160,704 | 10.5 MB | 6.4 MB | 8.1 MB | 5.8 MB | **0.551×** | **0.905×** | **0.718×** |
| Derivatives (funding, mark, index, open interest) | 1 | 118,524 | 2.2 MB | 1.6 MB | 1.7 MB | 882 KB | **0.395×** | **0.553×** | **0.512×** |
| **All** | 31 | 151,499,256 | 957.1 MB | 700.7 MB | 649.5 MB | 478.6 MB | **0.500×** | **0.683×** | **0.737×** |

Speed, in MB/s of Arrow data (the columns' size in memory):

| Data type | Arrow data | Write ZSTD(9) | Write tuned | Write ALP | Write tparquet | Read ZSTD(9) | Read tuned | Read ALP | Read tparquet |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Trades | 7.49 GB | 95 | 145 | 175 | 144 | 881 | 1,127 | 1,170 | 914 |
| Quotes (L1, top of book) | 481.8 MB | 84 | 147 | 160 | 58 | 872 | 1,173 | 1,335 | 953 |
| Order book (L2: snapshots and updates) | 1.46 GB | 118 | 122 | 146 | 47 | 1,196 | 1,136 | 1,656 | 968 |
| Bars (OHLCV) | 436 KB | 33 | 85 | 56 | 10 | 351 | 505 | 658 | 778 |
| Metrics (5m bars) | 10.5 MB | 60 | 92 | 37 | 9 | 293 | 487 | 363 | 553 |
| Derivatives (funding, mark, index, open interest) | 12.2 MB | 90 | 87 | 149 | 49 | 953 | 1,142 | 1,393 | 1,045 |
| **All** | 9.46 GB | 97 | 141 | 169 | 102 | 916 | 1,129 | 1,231 | 924 |

### Trades

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | ZSTD(9) ALP | tparquet | tparquet / ZSTD(9) | tparquet / tuned | tparquet / ALP | ALP columns | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| Aperiodic | BTC-USDT perpetual trades on Binance, 2025-06 (raw, Parquet) | 79,684,576 | 248.6 MB | 399.4 MB | 280.2 MB | 257.4 MB | 193.1 MB | 0.484× | 0.689× | 0.750× | 2 of 6 | 4 of 6 |
| Aperiodic | BTC-USDT perpetual trades on OKX, 2025-06 (raw, Parquet) | 54,282,320 | 189.1 MB | 294.8 MB | 208.5 MB | 193.0 MB | 147.1 MB | 0.499× | 0.706× | 0.762× | 2 of 6 | 4 of 6 |
| Tardis.dev | trades, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 1,959,946 | 17.8 MB | 11.0 MB | 7.5 MB | 7.0 MB | 5.2 MB | 0.474× | 0.691× | 0.740× | 2 of 8 | 4 of 8 |
| Crypto Lake | trades, Binance BTC-USDT, 2022-10-01 (Parquet) | 2,190,208 | 63.4 MB | 22.5 MB | 17.4 MB | 16.3 MB | 11.9 MB | 0.529× | 0.685× | 0.733× | 2 of 6 | 5 of 6 |
| NYSE | TAQ NYSE trades, symbol range 1, 2026-07-01 (CSV) | 748,242 | 12.9 MB | 9.3 MB | 6.4 MB | 6.8 MB | 4.8 MB | 0.513× | 0.748× | 0.700× | 1 of 11 | 6 of 11 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write ALP | Write tparquet | Read ZSTD(9) | Read tuned | Read ALP | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Aperiodic | BTC-USDT perpetual trades on Binance, 2025-06 (raw, Parquet) | 4.26 GB | 94 | 143 | 175 | 157 | 872 | 1,139 | 1,115 | 884 |
| Aperiodic | BTC-USDT perpetual trades on OKX, 2025-06 (raw, Parquet) | 2.90 GB | 97 | 148 | 176 | 150 | 901 | 1,112 | 1,247 | 972 |
| Tardis.dev | trades, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 163.7 MB | 134 | 209 | 232 | 106 | 1,165 | 1,279 | 1,550 | 891 |
| Crypto Lake | trades, Binance BTC-USDT, 2022-10-01 (Parquet) | 104.0 MB | 58 | 119 | 132 | 52 | 675 | 1,101 | 1,282 | 1,054 |
| NYSE | TAQ NYSE trades, symbol range 1, 2026-07-01 (CSV) | 56.2 MB | 72 | 107 | 134 | 28 | 559 | 831 | 908 | 561 |

### Quotes (L1, top of book)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | ZSTD(9) ALP | tparquet | tparquet / ZSTD(9) | tparquet / tuned | tparquet / ALP | ALP columns | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| Tardis.dev | quotes, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 1,210,001 | 16.3 MB | 13.4 MB | 8.9 MB | 8.5 MB | 5.1 MB | 0.380× | 0.573× | 0.597× | 4 of 8 | 6 of 8 |
| Crypto Lake | top of book, Binance BTC-USDT, 2022-10-01 (Parquet) | 713,858 | 11.2 MB | 7.8 MB | 7.4 MB | 6.6 MB | 4.5 MB | 0.574× | 0.610× | 0.685× | 4 of 5 | 5 of 5 |
| NYSE | TAQ NYSE BBO quotes, symbol range 1, 2026-07-01 (CSV) | 4,908,146 | 74.6 MB | 59.4 MB | 47.0 MB | 48.2 MB | 41.4 MB | 0.698× | 0.881× | 0.860× | 2 of 10 | 7 of 10 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write ALP | Write tparquet | Read ZSTD(9) | Read tuned | Read ALP | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Tardis.dev | quotes, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 94.4 MB | 122 | 219 | 266 | 46 | 1,228 | 1,596 | 2,007 | 1,241 |
| Crypto Lake | top of book, Binance BTC-USDT, 2022-10-01 (Parquet) | 28.6 MB | 71 | 78 | 90 | 18 | 684 | 792 | 1,065 | 665 |
| NYSE | TAQ NYSE BBO quotes, symbol range 1, 2026-07-01 (CSV) | 358.9 MB | 79 | 145 | 153 | 77 | 828 | 1,138 | 1,250 | 928 |

### Order book (L2: snapshots and updates)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | ZSTD(9) ALP | tparquet | tparquet / ZSTD(9) | tparquet / tuned | tparquet / ALP | ALP columns | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| Tardis.dev | 25-level book snapshots, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 1,429,796 | 53.8 MB | 81.5 MB | 75.9 MB | 63.7 MB | 38.2 MB | 0.468× | 0.503× | 0.599× | 100 of 104 | 102 of 104 |
| Crypto Lake | 20-level book snapshots, Binance DOGE-USDT, 2024-09-24 (Parquet) | 140,676 | 27.2 MB | 15.2 MB | 11.1 MB | 12.4 MB | 5.7 MB | 0.374× | 0.513× | 0.459× | 80 of 83 | 83 of 83 |
| Crypto Lake | book updates, Binance AVAX-USDT, 2024-04-01 (Parquet) | 3,943,331 | 46.4 MB | 29.7 MB | 22.2 MB | 19.6 MB | 14.8 MB | 0.497× | 0.665× | 0.756× | 2 of 6 | 5 of 6 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write ALP | Write tparquet | Read ZSTD(9) | Read tuned | Read ALP | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Tardis.dev | 25-level book snapshots, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 1.21 GB | 152 | 153 | 161 | 48 | 1,349 | 1,262 | 1,826 | 1,011 |
| Crypto Lake | 20-level book snapshots, Binance DOGE-USDT, 2024-09-24 (Parquet) | 93.4 MB | 86 | 95 | 129 | 33 | 670 | 537 | 1,241 | 714 |
| Crypto Lake | book updates, Binance AVAX-USDT, 2024-04-01 (Parquet) | 158.2 MB | 47 | 50 | 91 | 50 | 853 | 1,024 | 1,093 | 869 |

### Bars (OHLCV)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | ZSTD(9) ALP | tparquet | tparquet / ZSTD(9) | tparquet / tuned | tparquet / ALP | ALP columns | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| Aperiodic | ohlcv, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 159 KB | 296 KB | 160 KB | 150 KB | 108 KB | 0.364× | 0.673× | 0.717× | 6 of 7 | 7 of 7 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write ALP | Write tparquet | Read ZSTD(9) | Read tuned | Read ALP | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Aperiodic | ohlcv, BTC-USDT perpetual on Binance, 5m, 2025-05 | 436 KB | 33 | 85 | 56 | 10 | 351 | 505 | 658 | 778 |

### Metrics (5m bars)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | ZSTD(9) ALP | tparquet | tparquet / ZSTD(9) | tparquet / tuned | tparquet / ALP | ALP columns | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| Aperiodic | vtwap, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 108 KB | 205 KB | 109 KB | 135 KB | 101 KB | 0.494× | 0.928× | 0.750× | 2 of 3 | 3 of 3 |
| Aperiodic | basis, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 223 KB | 390 KB | 224 KB | 276 KB | 204 KB | 0.522× | 0.909× | 0.738× | 8 of 9 | 9 of 9 |
| Aperiodic | funding, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 26 KB | 80 KB | 27 KB | 22 KB | 13 KB | 0.166× | 0.493× | 0.600× | 3 of 4 | 3 of 4 |
| Aperiodic | open_interest, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 272 KB | 391 KB | 266 KB | 294 KB | 227 KB | 0.582× | 0.855× | 0.774× | 5 of 6 | 6 of 6 |
| Aperiodic | derivative_price, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 512 KB | 696 KB | 513 KB | 578 KB | 445 KB | 0.640× | 0.869× | 0.771× | 9 of 10 | 10 of 10 |
| Aperiodic | flow, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 789 KB | 1.3 MB | 786 KB | 1.0 MB | 733 KB | 0.562× | 0.932× | 0.714× | 28 of 37 | 37 of 37 |
| Aperiodic | trade_size, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 556 KB | 960 KB | 554 KB | 739 KB | 520 KB | 0.542× | 0.939× | 0.704× | 20 of 25 | 25 of 25 |
| Aperiodic | impact, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 157 KB | 285 KB | 158 KB | 176 KB | 148 KB | 0.520× | 0.938× | 0.841× | 5 of 6 | 6 of 6 |
| Aperiodic | range, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 302 KB | 509 KB | 304 KB | 376 KB | 271 KB | 0.533× | 0.892× | 0.721× | 10 of 11 | 11 of 11 |
| Aperiodic | updownticks, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 364 KB | 629 KB | 362 KB | 449 KB | 319 KB | 0.507× | 0.881× | 0.710× | 11 of 15 | 15 of 15 |
| Aperiodic | run_structure, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 182 KB | 309 KB | 176 KB | 216 KB | 160 KB | 0.519× | 0.913× | 0.743× | 7 of 8 | 8 of 8 |
| Aperiodic | returns, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 181 KB | 331 KB | 182 KB | 231 KB | 174 KB | 0.528× | 0.960× | 0.756× | 6 of 7 | 7 of 7 |
| Aperiodic | slippage, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 275 KB | 478 KB | 276 KB | 381 KB | 261 KB | 0.547× | 0.947× | 0.686× | 10 of 11 | 11 of 11 |
| Aperiodic | l1_price, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 488 KB | 668 KB | 443 KB | 451 KB | 345 KB | 0.516× | 0.779× | 0.763× | 10 of 13 | 13 of 13 |
| Aperiodic | l1_imbalance, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 307 KB | 518 KB | 308 KB | 385 KB | 275 KB | 0.530× | 0.892× | 0.715× | 10 of 11 | 11 of 11 |
| Aperiodic | l1_liquidity, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 361 KB | 636 KB | 363 KB | 488 KB | 335 KB | 0.526× | 0.921× | 0.686× | 14 of 15 | 15 of 15 |
| Aperiodic | l2_imbalance, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 760 KB | 1.2 MB | 764 KB | 971 KB | 713 KB | 0.594× | 0.934× | 0.735× | 24 of 25 | 25 of 25 |
| Aperiodic | l2_liquidity, BTC-USDT perpetual on Binance, 5m, 2025-05 | 8,928 | 598 KB | 942 KB | 601 KB | 889 KB | 558 KB | 0.592× | 0.928× | 0.627× | 16 of 17 | 17 of 17 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write ALP | Write tparquet | Read ZSTD(9) | Read tuned | Read ALP | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Aperiodic | vtwap, BTC-USDT perpetual on Binance, 5m, 2025-05 | 218 KB | 73 | 302 | 39 | 14 | 343 | 785 | 777 | 896 |
| Aperiodic | basis, BTC-USDT perpetual on Binance, 5m, 2025-05 | 367 KB | 59 | 80 | 38 | 7 | 259 | 484 | 378 | 541 |
| Aperiodic | funding, BTC-USDT perpetual on Binance, 5m, 2025-05 | 183 KB | 36 | 83 | 38 | 10 | 253 | 400 | 447 | 414 |
| Aperiodic | open_interest, BTC-USDT perpetual on Binance, 5m, 2025-05 | 400 KB | 53 | 71 | 41 | 14 | 329 | 575 | 528 | 795 |
| Aperiodic | derivative_price, BTC-USDT perpetual on Binance, 5m, 2025-05 | 725 KB | 61 | 120 | 38 | 15 | 339 | 552 | 519 | 716 |
| Aperiodic | flow, BTC-USDT perpetual on Binance, 5m, 2025-05 | 1.4 MB | 79 | 104 | 39 | 9 | 304 | 471 | 342 | 515 |
| Aperiodic | trade_size, BTC-USDT perpetual on Binance, 5m, 2025-05 | 956 KB | 77 | 102 | 39 | 8 | 292 | 511 | 335 | 501 |
| Aperiodic | impact, BTC-USDT perpetual on Binance, 5m, 2025-05 | 257 KB | 62 | 101 | 85 | 10 | 270 | 482 | 378 | 610 |
| Aperiodic | range, BTC-USDT perpetual on Binance, 5m, 2025-05 | 512 KB | 56 | 98 | 34 | 9 | 232 | 320 | 275 | 358 |
| Aperiodic | updownticks, BTC-USDT perpetual on Binance, 5m, 2025-05 | 588 KB | 63 | 94 | 39 | 8 | 268 | 477 | 337 | 501 |
| Aperiodic | run_structure, BTC-USDT perpetual on Binance, 5m, 2025-05 | 330 KB | 51 | 40 | 42 | 9 | 282 | 510 | 366 | 567 |
| Aperiodic | returns, BTC-USDT perpetual on Binance, 5m, 2025-05 | 294 KB | 64 | 80 | 51 | 9 | 274 | 572 | 328 | 521 |
| Aperiodic | slippage, BTC-USDT perpetual on Binance, 5m, 2025-05 | 441 KB | 68 | 75 | 35 | 9 | 269 | 469 | 269 | 515 |
| Aperiodic | l1_price, BTC-USDT perpetual on Binance, 5m, 2025-05 | 907 KB | 46 | 87 | 44 | 13 | 391 | 596 | 731 | 791 |
| Aperiodic | l1_imbalance, BTC-USDT perpetual on Binance, 5m, 2025-05 | 441 KB | 66 | 84 | 37 | 8 | 250 | 467 | 313 | 557 |
| Aperiodic | l1_liquidity, BTC-USDT perpetual on Binance, 5m, 2025-05 | 588 KB | 70 | 99 | 34 | 8 | 286 | 479 | 306 | 512 |
| Aperiodic | l2_imbalance, BTC-USDT perpetual on Binance, 5m, 2025-05 | 956 KB | 75 | 97 | 29 | 7 | 263 | 397 | 269 | 514 |
| Aperiodic | l2_liquidity, BTC-USDT perpetual on Binance, 5m, 2025-05 | 947 KB | 39 | 101 | 32 | 11 | 331 | 567 | 398 | 616 |

### Derivatives (funding, mark, index, open interest)

| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | ZSTD(9) ALP | tparquet | tparquet / ZSTD(9) | tparquet / tuned | tparquet / ALP | ALP columns | PCODEC columns |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|
| Tardis.dev | derivative ticker, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 118,524 | 1.8 MB | 2.2 MB | 1.6 MB | 1.7 MB | 882 KB | 0.395× | 0.553× | 0.512× | 6 of 11 | 8 of 11 |

| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write ALP | Write tparquet | Read ZSTD(9) | Read tuned | Read ALP | Read tparquet |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Tardis.dev | derivative ticker, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV) | 12.2 MB | 90 | 87 | 149 | 49 | 953 | 1,142 | 1,393 | 1,045 |
