"""The conformance vectors of testdata/, read through the plugin, against two oracles that do not
use this plugin's reader: the expected rows of testdata/expected/ (canonical JSON, floats bit for
bit), and what Polars' own Parquet reader reads from a standard (non-PCODEC) copy of the file."""

from __future__ import annotations

import collections
import io
import json
import pathlib

import polars as pl
import pytest
import tparquet
from polars.testing import assert_frame_equal

import polars_tparquet as pt
from conftest import TESTDATA, VECTORS, Reads, canonical

MANIFEST = {f["name"]: f for f in json.loads((TESTDATA / "files" / "manifest.json").read_text())["files"]}
by_stem = pytest.mark.parametrize("path", VECTORS, ids=lambda p: p.stem)


def expected(path: pathlib.Path) -> str:
    return (TESTDATA / "expected" / f"{path.stem}.jsonl").read_text()


def standard(path: pathlib.Path, tmp_path: pathlib.Path) -> pl.DataFrame:
    """What Polars' own reader reads from a standard copy of `path`."""
    copy = tmp_path / f"{path.stem}.standard.parquet"
    tparquet.transcode(path, copy, target="standard")
    return pl.read_parquet(copy)


def test_every_vector_is_here() -> None:
    assert len(VECTORS) == 24
    assert sorted(MANIFEST) == sorted(p.stem for p in VECTORS)


@by_stem
def test_vector_equals_polars_reading_a_standard_copy(path: pathlib.Path, tmp_path: pathlib.Path) -> None:
    want = standard(path, tmp_path)
    assert_frame_equal(pt.read_tparquet(path), want, check_exact=True)
    assert_frame_equal(pt.scan_tparquet(path).collect(engine="streaming"), want, check_exact=True)


@by_stem
def test_vector_rows_are_the_expected_rows(path: pathlib.Path) -> None:
    want = expected(path)
    assert canonical(pt.read_tparquet(path)) == want
    assert canonical(pt.scan_tparquet(path).collect(engine="streaming")) == want
    assert canonical(pt.scan_tparquet(path, batch_size=7).collect()) == want
    assert canonical(pt.read_tparquet(io.BytesIO(path.read_bytes()))) == want
    assert "".join(canonical(b) for b in pt.scan_tparquet(path).collect_batches(chunk_size=100)) == want


@by_stem
def test_vector_rows_and_row_groups_are_the_manifests(path: pathlib.Path, reads: Reads) -> None:
    entry = MANIFEST[path.stem]
    lf = pt.scan_tparquet(path)
    assert lf.select(pl.len()).collect().item() == entry["rows"]
    meta = tparquet.ParquetFile(path).metadata
    assert meta.num_row_groups == entry["row_groups"]
    assert meta.pcodec_version == entry["pcodec_version"]
    assert set(lf.collect_schema()) == {c["path"].split(".")[0] for c in entry["columns"]}


@by_stem
@pytest.mark.parametrize("policy", ["auto", "pcodec", "base"])
def test_vector_rewritten_through_the_plugin_keeps_its_rows(
    path: pathlib.Path, tmp_path: pathlib.Path, policy: tparquet.Policy
) -> None:
    want = expected(path)
    written, sunk = tmp_path / "w.parquet", tmp_path / "s.parquet"
    pt.read_tparquet(path).tparquet.write(written, policy=policy, row_group_size=700)  # type: ignore[attr-defined]
    pt.scan_tparquet(path).tparquet.sink(sunk, policy=policy, chunk_size=333)  # type: ignore[attr-defined]
    for out in (written, sunk):
        assert canonical(pt.read_tparquet(out)) == want
        assert canonical(standard(out, tmp_path)) == want


@by_stem
def test_each_column_of_a_vector_alone(path: pathlib.Path, tmp_path: pathlib.Path) -> None:
    want = standard(path, tmp_path)
    lf = pt.scan_tparquet(path)
    for name in want.columns:
        assert_frame_equal(lf.select(name).collect(), want.select(name), check_exact=True)
    backwards = want.columns[::-1]
    assert_frame_equal(pt.read_tparquet(path, columns=backwards), want.select(backwards), check_exact=True)


@by_stem
def test_head_and_tail_of_a_vector(path: pathlib.Path, tmp_path: pathlib.Path) -> None:
    want = standard(path, tmp_path)
    lf = pt.scan_tparquet(path)
    for n in (0, 1, 499, 10_000):
        assert_frame_equal(lf.head(n).collect(), want.head(n), check_exact=True)
        assert_frame_equal(lf.tail(n).collect(), want.tail(n), check_exact=True)
    assert_frame_equal(lf.slice(401, 222).collect(), want.slice(401, 222), check_exact=True)


def test_a_predicate_on_a_vector_skips_row_groups(tmp_path: pathlib.Path, reads: Reads) -> None:
    path = next(p for p in VECTORS if p.stem == "multiple_row_groups")
    want = standard(path, tmp_path)
    cut = want["ts"].sort()[1700]
    out = pt.scan_tparquet(path).filter(pl.col("ts") >= cut).collect()
    assert_frame_equal(out, want.filter(pl.col("ts") >= cut), check_exact=True)
    assert reads.row_groups() == [[4]]
    sym = pt.scan_tparquet(path).filter(pl.col("symbol") == "BTC-USD").collect()
    assert_frame_equal(sym, want.filter(pl.col("symbol") == "BTC-USD"), check_exact=True)


def test_vectors_of_one_schema_scan_together(tmp_path: pathlib.Path) -> None:
    groups: dict[str, list[pathlib.Path]] = collections.defaultdict(list)
    for path in VECTORS:
        groups[str(pt.scan_tparquet(path).collect_schema())].append(path)
    together = [paths for paths in groups.values() if len(paths) > 1]
    assert len(together) == 3
    for paths in together:
        want = pl.concat([standard(p, tmp_path) for p in paths])
        assert_frame_equal(pt.scan_tparquet(paths).collect(), want, check_exact=True)
        assert canonical(pt.scan_tparquet(paths[::-1]).collect()) == "".join(expected(p) for p in paths[::-1])


def test_the_canonical_oracle_tells_float_bits_apart() -> None:
    """canonical() is the oracle of bit-exact floats: it must see what assert_frame_equal does not."""
    a = pl.DataFrame({"f": [0.0, float("nan")]})
    b = pl.DataFrame({"f": pl.Series([0x8000_0000_0000_0000, 0x7FF8_0000_0000_0001]).reinterpret(dtype=pl.Float64)})
    assert_frame_equal(a, b, check_exact=True)
    assert canonical(a) == '{"f":"0x0000000000000000"}\n{"f":"0x7ff8000000000000"}\n'
    assert canonical(b) == '{"f":"0x8000000000000000"}\n{"f":"0x7ff8000000000001"}\n'


def test_polars_alone_refuses_a_pcodec_file() -> None:
    pcodec = next(p for p in VECTORS if p.stem == "primitives_required")
    with pytest.raises(pl.exceptions.ComputeError):
        pl.read_parquet(pcodec)
    control = next(p for p in VECTORS if p.stem == "standard_control")
    assert canonical(pl.read_parquet(control)) == expected(control)
