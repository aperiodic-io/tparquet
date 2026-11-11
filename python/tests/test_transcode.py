"""transcode: to standard Parquet and back, between policies, idempotent, keeping the schema,
the metadata, the row groups and their sorting; and its errors."""

from __future__ import annotations

import io
import pathlib
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import tparquet as ppc
from conftest import TPARQUET_FILES, vector
from tparquet import canonical

TABLE = pa.table(
    {
        "t": pa.array(range(1_700_000_000_000, 1_700_000_000_000 + 250 * 4000, 250), pa.timestamp("ms", "UTC")),
        "px": pa.array([100 + (i % 37) / 100 for i in range(4000)]),
        "qty": pa.array([None if i % 9 == 0 else i % 50 for i in range(4000)], pa.int32()),
        "sym": pa.array([f"S{i % 3}" for i in range(4000)]).dictionary_encode(),
    },
    metadata={"desk": "rates"},
)


def standard(tmp_path: pathlib.Path, **kwargs: Any) -> pathlib.Path:
    path = tmp_path / "std.parquet"
    pq.write_table(TABLE, path, row_group_size=1500, **kwargs)
    return path


def test_to_pcodec_and_back(tmp_path: pathlib.Path) -> None:
    src = standard(tmp_path)
    enc, dec = tmp_path / "enc.parquet", tmp_path / "dec.parquet"
    assert ppc.transcode(src, enc, policy="pcodec") == {"t": "PCODEC", "px": "PCODEC", "qty": "PCODEC", "sym": "ZSTD"}
    assert ppc.transcode(enc, dec, target="standard") == {"t": "ZSTD", "px": "ZSTD", "qty": "ZSTD", "sym": "ZSTD"}
    for p in [enc, dec]:
        assert ppc.verify(src, p) is None
        assert ppc.read_table(p).equals(TABLE)
        assert ppc.ParquetFile(p).metadata.row_group_rows == [1500, 1500, 1000]
    assert pq.read_table(dec).equals(TABLE)
    assert ppc.ParquetFile(enc).metadata.pcodec_version == "1"
    assert ppc.ParquetFile(dec).metadata.pcodec_version is None


@pytest.mark.parametrize("policy", ["auto", "pcodec", "base"])
def test_between_policies(tmp_path: pathlib.Path, policy: ppc.Policy) -> None:
    src = standard(tmp_path)
    first, second = tmp_path / "a.parquet", tmp_path / "b.parquet"
    ppc.transcode(src, first, policy="pcodec")
    codecs = ppc.transcode(first, second, policy=policy)
    if policy == "base":
        assert "PCODEC" not in codecs.values()
        assert pq.read_table(second).equals(TABLE)
    if policy == "pcodec":
        assert codecs["px"] == "PCODEC"
    assert ppc.verify(src, second) is None


@pytest.mark.parametrize("policy", ["auto", "pcodec", "base"])
def test_idempotent(tmp_path: pathlib.Path, policy: ppc.Policy) -> None:
    src = standard(tmp_path)
    once, twice = io.BytesIO(), io.BytesIO()
    ppc.transcode(src, once, policy=policy, level=3)
    ppc.transcode(io.BytesIO(once.getvalue()), twice, policy=policy, level=3)
    assert once.getvalue() == twice.getvalue()
    back, again = io.BytesIO(), io.BytesIO()
    ppc.transcode(io.BytesIO(once.getvalue()), back, target="standard")
    ppc.transcode(io.BytesIO(back.getvalue()), again, target="standard")
    assert back.getvalue() == again.getvalue()


def test_keeps_schema_metadata_and_sorting(tmp_path: pathlib.Path) -> None:
    sorting = [pq.SortingColumn(0)]
    src = standard(tmp_path, sorting_columns=sorting)
    out = tmp_path / "out.parquet"
    ppc.transcode(src, out)
    f = ppc.ParquetFile(out)
    assert f.schema == TABLE.schema
    assert f.schema.metadata is not None
    assert f.schema.metadata[b"desk"] == b"rates"
    meta = pq.ParquetFile(io.BytesIO(ppc_standard(out))).metadata
    assert all(tuple(meta.row_group(i).sorting_columns) == tuple(sorting) for i in range(meta.num_row_groups))
    # the dictionary column stays one
    assert pa.types.is_dictionary(f.schema.field("sym").type)


def ppc_standard(path: pathlib.Path) -> bytes:
    buf = io.BytesIO()
    ppc.transcode(path, buf, target="standard")
    return buf.getvalue()


def test_options_reach_the_output(tmp_path: pathlib.Path) -> None:
    src = standard(tmp_path)
    out = tmp_path / "o.parquet"
    codecs = ppc.transcode(src, out, policy="base", columns={"px": "pcodec", "t": "auto"}, base="snappy", level=0)
    assert codecs["px"] == "PCODEC"
    assert codecs["qty"] == "SNAPPY"
    assert codecs["sym"] == "SNAPPY"
    level0, level12 = io.BytesIO(), io.BytesIO()
    ppc.transcode(src, level0, policy="pcodec", level=0)
    ppc.transcode(src, level12, policy="pcodec", level=12)
    assert level0.getvalue() != level12.getvalue()
    # to standard: PCODEC chunks get the base codec, the others keep theirs
    enc = tmp_path / "enc.parquet"
    ppc.transcode(src, enc, policy="pcodec", base="gzip(5)")
    assert ppc.transcode(enc, tmp_path / "s.parquet", target="standard", base="lz4_raw") == {
        "t": "LZ4_RAW",
        "px": "LZ4_RAW",
        "qty": "LZ4_RAW",
        "sym": "GZIP",
    }


def test_row_groups_without_rows_are_dropped(tmp_path: pathlib.Path) -> None:
    out = tmp_path / "o.parquet"
    assert ppc.transcode(vector("empty"), out) == {}
    md = ppc.ParquetFile(out).metadata
    assert md.num_rows == 0
    assert md.num_row_groups == 0
    assert ppc.read_table(out).schema == ppc.read_table(vector("empty")).schema


@pytest.mark.parametrize("name", TPARQUET_FILES)
def test_every_vector_to_standard_and_back(tmp_path: pathlib.Path, name: str) -> None:
    src = vector(name)
    std, enc = tmp_path / "std.parquet", tmp_path / "enc.parquet"
    ppc.transcode(src, std, target="standard")
    ppc.transcode(std, enc, policy="pcodec")
    assert ppc.verify(src, std) is None
    assert ppc.verify(src, enc) is None
    assert ppc.ParquetFile(std).metadata.pcodec_version is None
    assert canonical.dumps(pq.read_table(std)) == canonical.dumps(ppc.read_table(src))


def test_errors(tmp_path: pathlib.Path) -> None:
    src = standard(tmp_path)
    out = tmp_path / "o.parquet"
    with pytest.raises(ppc.PcodecError, match=r'^unknown target "nope": expected "pcodec" or "standard"$'):
        ppc.transcode(src, out, target="nope")  # type: ignore[arg-type]
    with pytest.raises(ppc.PcodecError, match=r'^no column named "nope"$'):
        ppc.transcode(src, out, columns={"nope": "pcodec"})
    with pytest.raises(ppc.PcodecError, match=r"^the base codec cannot be PCODEC$"):
        ppc.transcode(src, out, target="standard", base="pcodec")
    with pytest.raises(ppc.PcodecError, match="parquet"):
        ppc.transcode(io.BytesIO(b"PAR1 not really PAR1"), out)
    assert not out.exists()
