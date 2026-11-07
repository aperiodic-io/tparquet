"""cli: fetch, measure and report, skipping what cannot be fetched here."""

from __future__ import annotations

import json
import pathlib

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

from conftest import Cached
from tpq_bench import catalog, cli, fetch, report, tsbs


def trades(path: pathlib.Path) -> pathlib.Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    n = 5000
    pq.write_table(
        pa.table({"ts": pa.array(range(0, 3 * n, 3), pa.int64()), "price": [100 + (i % 7) / 100 for i in range(n)]}),
        path,
    )
    return path


def test_run_measures_the_cached_sources_and_skips_the_unavailable(
    tmp_path: pathlib.Path, capsys: pytest.CaptureFixture[str]
) -> None:
    data = tmp_path / "data"
    trades(data / "x" / "t.parquet")
    # the unavailable one first: the run goes on after it
    sources = [
        catalog.Source("Y", "ES trades", "trades", "y/t.parquet", "parquet", Cached()),
        catalog.Source("X", "BTC trades", "trades", "x/t.parquet", "parquet", Cached()),
    ]
    entries, skipped = cli.run(sources, data)
    assert [(e.provider, e.data, e.m.rows) for e in entries] == [("X", "BTC trades", 5000)]
    assert entries[0].source_bytes == (data / "x" / "t.parquet").stat().st_size
    assert [s.provider for s, _ in skipped] == ["Y"]
    assert "does not exist" in skipped[0][1]
    # the measurements' files are gone
    assert sorted(p.relative_to(data).as_posix() for p in data.rglob("*")) == ["x", "x/t.parquet"]
    err = capsys.readouterr().err
    assert "X: BTC trades ..." in err
    assert "  5,000 rows, ZSTD(9) " in err
    assert ", tparquet " in err
    assert "×" in err
    assert "skipped: Y: ES trades: " in err


def test_main_writes_markdown_and_json(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    data = tmp_path / "data"
    trades(data / "x" / "t.parquet")
    monkeypatch.setattr(
        catalog, "SOURCES", [catalog.Source("X", "BTC trades", "trades", "x/t.parquet", "parquet", Cached())]
    )
    md, js = tmp_path / "r.md", tmp_path / "r.json"
    assert cli.main(["run", "--data", str(data), "--markdown", str(md), "--json", str(js)]) == 0
    assert "| X | BTC trades | 5,000 |" in md.read_text()
    assert json.loads(js.read_text())[0]["rows"] == 5000


def test_main_fetch_reports_what_it_could_not_fetch(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    trades(tmp_path / "x" / "cached.parquet")
    monkeypatch.setattr(
        catalog,
        "SOURCES",
        [
            catalog.Source("X", "BTC trades", "trades", "x/t.parquet", "parquet", Cached()),
            catalog.Source("X", "ETH trades", "trades", "x/cached.parquet", "parquet", Cached()),
            catalog.Source("X", "BTC quotes", "quotes", "x/q.parquet", "parquet", Cached()),
        ],
    )
    assert cli.main(["fetch", "--data", str(tmp_path)]) == 1
    err = capsys.readouterr().err
    assert "not fetched: X: BTC trades" in err
    assert err.endswith("2 of 3 not fetched: x/t.parquet, x/q.parquet\n")


def test_main_list(tmp_path: pathlib.Path, capsys: pytest.CaptureFixture[str]) -> None:
    assert cli.main(["list", "--data", str(tmp_path), "nyse"]) == 0
    out = capsys.readouterr().out.splitlines()
    assert out == [
        "trades  nyse/EQY_US_TAQ_NYSE_1_TRADES_20260701.gz  missing",
        "quotes  nyse/EQY_US_TAQ_NYSE_1_BBO_20260701.gz  missing",
    ]


def test_run_works_in_the_cache_directory(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    data = tmp_path / "data"
    trades(data / "x" / "t.parquet")
    seen: list[pathlib.Path] = []
    from tpq_bench.measure import measure as real

    def spy(reader: pa.RecordBatchReader, workdir: pathlib.Path) -> object:
        seen.append(workdir)
        return real(reader, workdir)

    monkeypatch.setattr("tpq_bench.cli.measure", spy)
    cli.run([catalog.Source("X", "t", "trades", "x/t.parquet", "parquet", Cached())], data)
    # on the cache's disk, which has room for files of the source's size
    assert seen[0].parent == data
    assert seen[0].name.startswith("work-")


def test_main_writes_markdown_to_standard_output(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    data = tmp_path / "data"
    trades(data / "x" / "t.parquet")
    monkeypatch.setattr(
        catalog, "SOURCES", [catalog.Source("X", "BTC trades", "trades", "x/t.parquet", "parquet", Cached())]
    )
    assert cli.main(["run", "--data", str(data)]) == 0
    assert capsys.readouterr().out.startswith("| Data type |")


def test_main_fetch_downloads_into_a_new_cache(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    source = trades(tmp_path / "remote" / "t.parquet")
    monkeypatch.setattr(
        catalog,
        "SOURCES",
        [catalog.Source("X", "BTC trades", "trades", "x/t.parquet", "parquet", fetch.Http(source.as_uri()))],
    )
    data = tmp_path / "a" / "b"
    assert cli.main(["fetch", "--data", str(data)]) == 0
    assert (data / "x" / "t.parquet").read_bytes() == source.read_bytes()


def test_main_list_shows_what_is_cached(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    trades(tmp_path / "x" / "t.parquet")
    monkeypatch.setattr(
        catalog,
        "SOURCES",
        [
            catalog.Source("X", "BTC trades", "trades", "x/t.parquet", "parquet", Cached()),
            catalog.Source("X", "BTC quotes", "quotes", "x/q.parquet", "parquet", Cached()),
        ],
    )
    # the cache from the environment
    monkeypatch.setenv("TPQ_BENCH_DATA", str(tmp_path))
    assert cli.main(["list"]) == 0
    assert capsys.readouterr().out.splitlines() == ["trades  x/t.parquet  cached", "quotes  x/q.parquet  missing"]


def test_unknown_command(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit):
        cli.main(["publish"])
    assert "invalid choice: 'publish'" in capsys.readouterr().err


def test_main_run_with_nothing_measured(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setattr(
        catalog, "SOURCES", [catalog.Source("X", "BTC trades", "trades", "x/t.parquet", "parquet", Cached())]
    )
    assert cli.main(["run", "--data", str(tmp_path)]) == 1
    assert capsys.readouterr().err.endswith("nothing measured\n")


def test_the_default_cache_is_bench_data(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    assert pathlib.Path(__file__).resolve().parents[1] / "data" == cli.DEFAULT_DATA
    trades(tmp_path / "x" / "t.parquet")
    monkeypatch.setattr(cli, "DEFAULT_DATA", tmp_path)
    monkeypatch.setattr(
        catalog, "SOURCES", [catalog.Source("X", "BTC trades", "trades", "x/t.parquet", "parquet", Cached())]
    )
    monkeypatch.delenv("TPQ_BENCH_DATA")
    assert cli.main(["list"]) == 0
    assert capsys.readouterr().out == "trades  x/t.parquet  cached\n"


def test_run_generates_a_tsbs_source_and_measures_it(tmp_path: pathlib.Path) -> None:
    data = tmp_path / "data"
    sources = [
        catalog.Source(
            "TSBS", f"devops {m}", "observability", f"tsbs/{m}.parquet", "parquet", tsbs.Tsbs(m, hosts=5, ticks=400)
        )
        for m in ("cpu", "mem", "redis")
    ]
    entries, skipped = cli.run(sources, data)
    assert skipped == []
    assert [(e.data, e.group, e.m.rows) for e in entries] == [
        ("devops cpu", "observability", 2000),
        ("devops mem", "observability", 2000),
        ("devops redis", "observability", 2000),
    ]
    # the file as delivered is the generated one, in the cache
    assert all(
        e.source_bytes == (data / "tsbs" / f"{m}.parquet").stat().st_size
        for e, m in zip(entries, ("cpu", "mem", "redis"))
    )
    # the floats of mem are PCODEC-coded or not, but every column has a codec
    assert all(set(e.m.codecs) >= {"time", "hostname"} for e in entries)
    assert "devops mem" in report.markdown(entries)
