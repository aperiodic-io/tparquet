"""report: results grouped by data type, as Markdown and JSON."""

from __future__ import annotations

import json

import pytest

from tpq_bench import report
from tpq_bench.measure import Measurement


def m(rows: int, z9: int, tpq: int, codecs: dict[str, str] | None = None) -> Measurement:
    return Measurement(
        rows=rows,
        arrow_bytes=z9 * 10,
        zstd9_bytes=z9,
        tuned_bytes=z9 * 4 // 5,
        tparquet_bytes=tpq,
        write_s_zstd9=2.0,
        write_s_tuned=1.5,
        write_s_tparquet=1.0,
        read_s_zstd9=0.5,
        read_s_tuned=0.5,
        read_s_tparquet=0.25,
        codecs=codecs or {"ts": "PCODEC", "price": "PCODEC", "side": "ZSTD"},
        encodings={"ts": "DELTA_BINARY_PACKED", "price": "dictionary", "side": "dictionary"},
    )


ENTRIES = [
    report.Entry("Tardis.dev", "BTCUSDT quotes", "quotes", 17_800_000, m(1_000_000, 12_000_000, 6_000_000)),
    report.Entry("Tardis.dev", "BTCUSDT trades", "trades", 999, m(1_500, 400_000, 100_000)),
    report.Entry("NYSE", "TAQ trades", "trades", 13_000_000, m(2_500_000, 3_600_000_000, 1_600_000_000)),
]


@pytest.mark.parametrize(
    ("n", "text"),
    [
        (999, "999 B"),
        (1_000, "1 KB"),
        (361_499, "361 KB"),
        (999_000, "999 KB"),
        (1_000_000, "1.0 MB"),
        (12_345_678, "12.3 MB"),
        (100_050_050, "100.1 MB"),
        (1_000_000_000, "1.00 GB"),
        (2_005_000_001, "2.01 GB"),
        (3_600_000_000, "3.60 GB"),
    ],
)
def test_size(n: int, text: str) -> None:
    assert report.size(n) == text


def test_groups_follow_the_order_of_the_data_types() -> None:
    assert [g for g, _ in report.groups(ENTRIES)] == ["trades", "quotes"]
    assert [e.data for e in dict(report.groups(ENTRIES))["trades"]] == ["BTCUSDT trades", "TAQ trades"]


def test_observability_is_the_last_data_type() -> None:
    assert list(report.GROUPS) == ["trades", "quotes", "l2", "bars", "metrics", "derivatives", "observability"]
    assert report.GROUPS["observability"] == "Observability (TSBS DevOps server metrics)"
    entries = [
        report.Entry("TSBS", "devops cpu", "observability", 5, m(10, 100, 50)),
        report.Entry("NYSE", "TAQ trades", "trades", 5, m(10, 100, 50)),
    ]
    assert [g for g, _ in report.groups(entries)] == ["trades", "observability"]
    assert "| Observability (TSBS DevOps server metrics) | 1 | 10 |" in report.markdown(entries)
    assert "\n### Observability (TSBS DevOps server metrics)\n" in report.markdown(entries)


def test_unknown_group_is_refused() -> None:
    with pytest.raises(ValueError, match="group"):
        report.groups([report.Entry("X", "y", "weather", 1, m(1, 1, 1))])


def test_markdown() -> None:
    assert report.markdown(ENTRIES) == (
        "| Data type | Datasets | Rows | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned |\n"
        "|---|---:|---:|---:|---:|---:|---:|---:|\n"
        "| Trades | 2 | 2,501,500 | 3.60 GB | 2.88 GB | 1.60 GB | **0.444×** | **0.556×** |\n"
        "| Quotes (L1, top of book) | 1 | 1,000,000 | 12.0 MB | 9.6 MB | 6.0 MB | **0.500×** | **0.625×** |\n"
        "| **All** | 3 | 3,501,500 | 3.61 GB | 2.89 GB | 1.61 GB | **0.445×** | **0.556×** |\n"
        "\n"
        "Speed, in MB/s of Arrow data (the columns' size in memory):\n"
        "\n"
        "| Data type | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |\n"
        "|---|---:|---:|---:|---:|---:|---:|---:|\n"
        "| Trades | 36.00 GB | 9,001 | 12,001 | 18,002 | 36,004 | 36,004 | 72,008 |\n"
        "| Quotes (L1, top of book) | 120.0 MB | 60 | 80 | 120 | 240 | 240 | 480 |\n"
        "| **All** | 36.12 GB | 6,021 | 8,028 | 12,041 | 24,083 | 24,083 | 48,165 |\n"
        "\n"
        "### Trades\n"
        "\n"
        "| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |\n"
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---|\n"
        "| Tardis.dev | BTCUSDT trades | 1,500 | 999 B | 400 KB | 320 KB | 100 KB | 0.250× | 0.312× | 2 of 3 |\n"
        "| NYSE | TAQ trades | 2,500,000 | 13.0 MB | 3.60 GB | 2.88 GB | 1.60 GB | 0.444× | 0.556× | 2 of 3 |\n"
        "\n"
        "| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |\n"
        "|---|---|---:|---:|---:|---:|---:|---:|---:|\n"
        "| Tardis.dev | BTCUSDT trades | 4.0 MB | 2 | 3 | 4 | 8 | 8 | 16 |\n"
        "| NYSE | TAQ trades | 36.00 GB | 18,000 | 24,000 | 36,000 | 72,000 | 72,000 | 144,000 |\n"
        "\n"
        "### Quotes (L1, top of book)\n"
        "\n"
        "| Provider | Data | Rows | As delivered | ZSTD(9) | ZSTD(9) tuned | tparquet | tparquet / ZSTD(9) | tparquet / tuned | PCODEC columns |\n"
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---|\n"
        "| Tardis.dev | BTCUSDT quotes | 1,000,000 | 17.8 MB | 12.0 MB | 9.6 MB | 6.0 MB | 0.500× | 0.625× | 2 of 3 |\n"
        "\n"
        "| Provider | Data | Arrow data | Write ZSTD(9) | Write tuned | Write tparquet | Read ZSTD(9) | Read tuned | Read tparquet |\n"
        "|---|---|---:|---:|---:|---:|---:|---:|---:|\n"
        "| Tardis.dev | BTCUSDT quotes | 120.0 MB | 60 | 80 | 120 | 240 | 240 | 480 |\n"
    )


@pytest.mark.parametrize(
    ("n", "seconds", "text"),
    [(10**6, 1.0, "1"), (10**9, 0.5, "2,000"), (2_500_000, 1.0, "2"), (2_500_001, 1.0, "3"), (1, 0.0, "–")],
)
def test_speed(n: int, seconds: float, text: str) -> None:
    assert report.speed(n, seconds) == text


def test_json_round_trips_every_field() -> None:
    rows = json.loads(report.to_json(ENTRIES[:1]))
    assert rows == [
        {
            "provider": "Tardis.dev",
            "data": "BTCUSDT quotes",
            "group": "quotes",
            "source_bytes": 17_800_000,
            "rows": 1_000_000,
            "arrow_bytes": 120_000_000,
            "zstd9_bytes": 12_000_000,
            "tuned_bytes": 9_600_000,
            "tparquet_bytes": 6_000_000,
            "ratio": 0.5,
            "ratio_tuned": 0.625,
            "write_s_zstd9": 2.0,
            "write_s_tuned": 1.5,
            "write_s_tparquet": 1.0,
            "read_s_zstd9": 0.5,
            "read_s_tuned": 0.5,
            "read_s_tparquet": 0.25,
            "codecs": {"ts": "PCODEC", "price": "PCODEC", "side": "ZSTD"},
            "tuned_encodings": {"ts": "DELTA_BINARY_PACKED", "price": "dictionary", "side": "dictionary"},
        }
    ]


def test_json_is_indented_and_keeps_unicode() -> None:
    entry = report.Entry("Tardis.dev", "BTCUSDT, Binance USDⓈ-M", "trades", 1, m(10, 10, 5))
    text = report.to_json([entry])
    assert text.startswith('[\n  {\n    "provider": "Tardis.dev",\n')
    assert '"data": "BTCUSDT, Binance USDⓈ-M"' in text
    assert text.endswith("}\n]\n")
