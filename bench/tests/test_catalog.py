"""catalog: the provider samples, each under one data type."""

from __future__ import annotations

import pytest

from tpq_bench import catalog, fetch, report, tsbs


def test_every_source_has_a_known_group_and_loader() -> None:
    for s in catalog.SOURCES:
        assert s.group in report.GROUPS, s.key
        assert s.loader in {"parquet", "tardis", "nyse_trades", "nyse_bbo"}, s.key


def test_keys_are_unique_and_live_under_their_provider() -> None:
    keys = [s.key for s in catalog.SOURCES]
    assert len(keys) == len(set(keys))
    for s in catalog.SOURCES:
        assert s.key.split("/")[0] == catalog.slug(s.provider)


def test_the_providers() -> None:
    assert {s.provider for s in catalog.SOURCES} == {"Aperiodic", "Tardis.dev", "Crypto Lake", "NYSE", "TSBS"}


def test_aperiodic_raw_and_metrics() -> None:
    ap = [s for s in catalog.SOURCES if s.provider == "Aperiodic"]
    assert {s.group for s in ap} == {"trades", "metrics", "bars"}
    raw = [s for s in ap if s.group == "trades"]
    assert len(raw) == 2
    assert all(isinstance(s.fetch, fetch.Http) and "download=true" in s.fetch.url for s in raw)
    metrics = [s for s in ap if s.group != "trades"]
    assert len(metrics) == len(catalog.APERIODIC_METRICS) == 19
    assert all(isinstance(s.fetch, fetch.SignedUrl) for s in metrics)
    assert all(dict(s.fetch.headers) == {"X-API-KEY": "DEMO-KEY"} for s in ap)  # type: ignore[attr-defined]


def test_tsbs_is_generated_not_downloaded() -> None:
    ts = [s for s in catalog.SOURCES if s.provider == "TSBS"]
    assert {s.group for s in ts} == {"observability"}
    assert {s.loader for s in ts} == {"parquet"}
    assert all(isinstance(s.fetch, tsbs.Tsbs) for s in ts)
    # every measurement of TSBS devops as the generator writes it; the cpu one again by series
    assert [(s.fetch.measurement, s.fetch.order) for s in ts] == [  # type: ignore[attr-defined]
        *((m, "time") for m in tsbs.MEASUREMENTS),
        ("cpu", "series"),
    ]
    assert {(s.fetch.hosts, s.fetch.ticks, s.fetch.seed) for s in ts} == {(100, 8640, 123)}  # type: ignore[attr-defined]
    assert [s.key for s in ts] == [
        *(f"tsbs/devops_{m}_time.parquet" for m in tsbs.MEASUREMENTS),
        "tsbs/devops_cpu_series.parquet",
    ]
    assert len({s.data for s in ts}) == len(ts)
    cpu, cpu_series = ts[0], ts[-1]
    assert cpu.data == "devops cpu, 100 hosts, 1 day at 10 s, in time order (generated)"
    assert cpu_series.data == "devops cpu, 100 hosts, 1 day at 10 s, by host and time (generated)"
    assert ts[8].data == "devops redis, 100 hosts, 1 day at 10 s, in time order (generated)"


@pytest.mark.parametrize(
    ("words", "providers"),
    [
        (["tardis"], {"Tardis.dev"}),
        (["tsbs"], {"TSBS"}),
        (["observability"], {"TSBS"}),
        (["Crypto Lake", "tsbs"], {"Crypto Lake", "TSBS"}),
        (["Crypto Lake", "nyse"], {"Crypto Lake", "NYSE"}),
        (["l2"], {"Tardis.dev", "Crypto Lake"}),
    ],
)
def test_select(words: list[str], providers: set[str]) -> None:
    assert {s.provider for s in catalog.select(catalog.SOURCES, words)} == providers


def test_select_by_key() -> None:
    [s] = catalog.select(catalog.SOURCES, ["aperiodic/metrics_flow_"])
    assert s.data.startswith("flow")


def test_select_nothing_is_everything() -> None:
    assert catalog.select(catalog.SOURCES, []) == catalog.SOURCES


def test_select_unknown() -> None:
    with pytest.raises(ValueError, match="matches no source"):
        catalog.select(catalog.SOURCES, ["bloomberg"])
