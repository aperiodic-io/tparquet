"""The samples: what each provider publishes for evaluation, under one data type each.

None of the files is in the repository (the providers' terms do not allow redistributing
them); `tpq-bench fetch` downloads them from the providers into the cache. The exception is TSBS,
the one source that is not a provider's: its data is generated into the cache.
"""

from __future__ import annotations

import dataclasses
import re
from typing import TYPE_CHECKING

from tpq_bench.fetch import Fetcher, Http, SignedUrl
from tpq_bench.tsbs import MEASUREMENTS, Tsbs

if TYPE_CHECKING:
    from collections.abc import Sequence


@dataclasses.dataclass(frozen=True)
class Source:
    provider: str
    data: str
    group: str
    # the file in the cache, `<provider slug>/<name>`; an absolute path for a local file
    key: str
    loader: str
    fetch: Fetcher


def slug(provider: str) -> str:
    """The provider's directory in the cache: "Tardis.dev" -> "tardis.dev", "Crypto Lake" -> "cryptolake"."""
    return re.sub(r"[^a-z0-9.]+", "", provider.lower())


# --- Aperiodic: the free preview (https://aperiodic.io/llms.txt, "Preview access"), with the
# public demo key. Raw: the 2025-06 trades of the BTC perpetual on Binance and OKX, the only raw
# preview. Metrics: every metric dataset for BTC-USDT on Binance, 5m bars, 2025-05.
APERIODIC = "https://aperiodic.io/api/v1/data"
DEMO_KEY = (("X-API-KEY", "DEMO-KEY"),)
APERIODIC_METRICS = (
    "ohlcv",
    "vtwap",
    "basis",
    "funding",
    "open_interest",
    "derivative_price",
    "flow",
    "trade_size",
    "impact",
    "range",
    "updownticks",
    "run_structure",
    "returns",
    "slippage",
    "l1_price",
    "l1_imbalance",
    "l1_liquidity",
    "l2_imbalance",
    "l2_liquidity",
)
_PREVIEW = "timestamp=exchange&interval=5m&exchange=binance-futures&symbol=perpetual-BTC-USDT:USDT"

_aperiodic = [
    Source(
        "Aperiodic",
        f"BTC-USDT perpetual trades on {venue}, 2025-06 (raw, Parquet)",
        "trades",
        f"aperiodic/raw_trades_{exchange}_2025-06.parquet",
        "parquet",
        Http(
            f"{APERIODIC}/raw/preview/trades?exchange={exchange}&symbol=perpetual-BTC-USDT:USDT&download=true",
            DEMO_KEY,
        ),
    )
    for exchange, venue in (("binance-futures", "Binance"), ("okx-perps", "OKX"))
] + [
    Source(
        "Aperiodic",
        f"{metric}, BTC-USDT perpetual on Binance, 5m, 2025-05",
        "bars" if metric == "ohlcv" else "metrics",
        f"aperiodic/metrics_{metric}_binance-futures_5m_2025-05.parquet",
        "parquet",
        SignedUrl(f"{APERIODIC}/preview/{metric}?{_PREVIEW}&start_date=2025-05-01&end_date=2025-05-31", DEMO_KEY),
    )
    for metric in APERIODIC_METRICS
]

# --- Tardis.dev: the first day of each month downloads without an API key
# (https://docs.tardis.dev/downloadable-csv-files). Binance USDⓈ-M BTCUSDT, 2025-06-01.
TARDIS = "https://datasets.tardis.dev/v1/binance-futures"
_tardis = [
    Source(
        "Tardis.dev",
        f"{label}, Binance USDⓈ-M BTCUSDT, 2025-06-01 (CSV)",
        group,
        f"tardis.dev/binance-futures_{kind}_2025-06-01_BTCUSDT.csv.gz",
        "tardis",
        Http(f"{TARDIS}/{kind}/2025/06/01/BTCUSDT.csv.gz"),
    )
    for kind, label, group in (
        ("trades", "trades", "trades"),
        ("quotes", "quotes", "quotes"),
        ("book_snapshot_25", "25-level book snapshots", "l2"),
        ("derivative_ticker", "derivative ticker", "derivatives"),
    )
]

# --- Crypto Lake: the public sample bucket its client reads with
# `lakeapi.use_sample_data(anonymous_access=True)`. Binance spot.
LAKE = "https://s3.eu-west-1.amazonaws.com/sample.crypto.lake"
_lake = [
    Source(
        "Crypto Lake",
        f"{label}, Binance {symbol}, {day} (Parquet)",
        group,
        f"cryptolake/{table}_BINANCE_{symbol}_{day}.parquet",
        "parquet",
        Http(f"{LAKE}/{table}/exchange=BINANCE/symbol={symbol}/dt={day}/{name}.snappy.parquet"),
    )
    for table, label, group, symbol, day, name in (
        ("trades", "trades", "trades", "BTC-USDT", "2022-10-01", "1"),
        (
            "level_1",
            "top of book",
            "quotes",
            "BTC-USDT",
            "2022-10-01",
            "converted_77d7cb8bc6244c3ca4d2a1d5c362b9c2",
        ),
        ("book", "20-level book snapshots", "l2", "DOGE-USDT", "2024-09-24", "1"),
        ("book_delta_v2", "book updates", "l2", "AVAX-USDT", "2024-04-01", "1"),
    )
]

# --- NYSE: the public historical data samples (https://ftp.nyse.com/Historical%20Data%20Samples/).
# TAQ NYSE Trades and BBO, 2026-07-01, the files of symbol range 1.
NYSE = "https://ftp.nyse.com/Historical%20Data%20Samples/TAQ%20NYSE%20BBO"
_nyse = [
    Source(
        "NYSE",
        f"TAQ NYSE {label}, symbol range 1, 2026-07-01 (CSV)",
        group,
        f"nyse/EQY_US_TAQ_NYSE_1_{kind}_20260701.gz",
        loader,
        Http(f"{NYSE}/EQY_US_TAQ_NYSE_1_{kind}_20260701.gz"),
    )
    for kind, label, group, loader in (
        ("TRADES", "trades", "trades", "nyse_trades"),
        ("BBO", "BBO quotes", "quotes", "nyse_bbo"),
    )
]

# --- TSBS, the Time Series Benchmark Suite (https://github.com/timescale/tsbs): the DevOps data
# that metric stores are benchmarked with, 100 hosts reporting every 10 s for a day, generated
# (see tpq_bench/tsbs.py). Every measurement in the order the generator writes it, every host's
# report of a tick before the next tick; the cpu measurement, TSBS' cpu-only use case, again
# with each host's series together, as a store keyed by series lays it out.
_ORDER_LABELS = {"time": "in time order", "series": "by host and time"}
_tsbs = [
    Source(
        "TSBS",
        f"devops {measurement}, 100 hosts, 1 day at 10 s, {_ORDER_LABELS[order]} (generated)",
        "observability",
        f"tsbs/devops_{measurement}_{order}.parquet",
        "parquet",
        Tsbs(measurement, order),
    )
    for measurement, order in [*((m, "time") for m in MEASUREMENTS), ("cpu", "series")]
]

SOURCES: list[Source] = [*_aperiodic, *_tardis, *_lake, *_nyse, *_tsbs]


def select(sources: Sequence[Source], words: Sequence[str]) -> list[Source]:
    """The sources a word names: a provider, a data type, or part of a cache key. No words: all."""
    if not words:
        return list(sources)
    chosen: list[Source] = []
    groups = {s.group for s in sources}
    for word in words:
        w = word.lower()
        if w in groups:
            # a data type, even when cache keys contain the word ("l2" in "metrics_l2_imbalance")
            hits = [s for s in sources if s.group == w]
        else:
            hits = [s for s in sources if slug(word) == slug(s.provider) or w in s.key.lower()]
        if not hits:
            raise ValueError(f"{word!r} matches no source")
        chosen += [s for s in hits if s not in chosen]
    return [s for s in sources if s in chosen]
