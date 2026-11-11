"""The conformance vectors in testdata/: every file read to canonical JSON equals its
expected .jsonl byte for byte, its footer matches the manifest, and every payload vector
decodes (or fails) as listed."""

from __future__ import annotations

import io
import json
from typing import Any

import pyarrow.parquet as pq
import pytest

import tparquet as ppc
from conftest import TESTDATA, manifest, vector
from tparquet import canonical, codec

FILES = manifest()
PAYLOADS: list[dict[str, Any]] = json.loads((TESTDATA / "payloads" / "manifest.json").read_text())["vectors"]


def test_every_vector_is_covered() -> None:
    names = {f["name"] for f in FILES}
    assert names == {p.stem for p in (TESTDATA / "files").glob("*.parquet")}
    assert "standard_control" in names
    assert len(PAYLOADS) == len(list((TESTDATA / "payloads").glob("*.bin")))


@pytest.mark.parametrize("entry", FILES, ids=[f["name"] for f in FILES])
def test_file_reads_to_the_expected_rows(entry: dict[str, Any]) -> None:
    expected = (TESTDATA / entry["expected"]).read_bytes()
    table = ppc.read_table(TESTDATA / entry["file"])
    assert canonical.dumps(table).encode() == expected


@pytest.mark.parametrize("entry", FILES, ids=[f["name"] for f in FILES])
def test_file_streams_to_the_expected_rows(entry: dict[str, Any]) -> None:
    # small batches, through the C stream: batch boundaries do not change the rows
    expected = (TESTDATA / entry["expected"]).read_bytes()
    out = "".join(canonical.dumps(b) for b in ppc.ParquetFile(TESTDATA / entry["file"]).iter_batches(batch_size=97))
    assert out.encode() == expected
    assert canonical.dumps(ppc.read_batches(TESTDATA / entry["file"], batch_size=1000)).encode() == expected


@pytest.mark.parametrize("entry", FILES, ids=[f["name"] for f in FILES])
def test_footer_matches_the_manifest(entry: dict[str, Any]) -> None:
    path = TESTDATA / entry["file"]
    md = ppc.ParquetFile(path).metadata
    assert md.num_rows == entry["rows"]
    assert md.num_row_groups == entry["row_groups"]
    assert md.pcodec_version == entry["pcodec_version"]
    kv = {k: v for k, v in md.key_value_metadata.items() if k != "ARROW:schema"}
    assert kv == entry["key_value_metadata"]
    # pyarrow reads the footer of every vector, PCODEC or not
    footer = pq.read_metadata(path).metadata or {}
    assert {k.decode(): v.decode() for k, v in footer.items() if k != b"ARROW:schema"} == {
        k: v or "" for k, v in entry["key_value_metadata"].items()
    }
    assert md.codecs == {c["path"]: c["codecs"] for c in entry["columns"]}
    assert md.physical_types == {c["path"]: c["physical_type"] for c in entry["columns"]}
    report = ppc.inspect(path)
    assert report["rows"] == entry["rows"]
    assert report["pcodec_version"] == entry["pcodec_version"]
    for got, want in zip(report["columns"], entry["columns"], strict=True):
        assert {k: got[k] for k in want} == want


def test_standard_control_through_pyarrow() -> None:
    # the canonical writer agrees with the expected rows on a file pyarrow reads itself
    expected = (TESTDATA / "expected" / "standard_control.jsonl").read_bytes()
    assert canonical.dumps(pq.read_table(vector("standard_control"))).encode() == expected


VALID = [p for p in PAYLOADS if p["valid"]]
INVALID = [p for p in PAYLOADS if not p["valid"]]


@pytest.mark.parametrize("v", VALID, ids=[p["name"] for p in VALID])
def test_valid_payload(v: dict[str, Any]) -> None:
    payload = (TESTDATA / "payloads" / v["payload"]).read_bytes()
    decoded = (TESTDATA / "payloads" / v["decoded"]).read_bytes()
    assert len(decoded) == v["expected_len"]
    assert codec.decompress(payload, v["expected_len"]) == decoded
    assert codec.mode(payload) == v["mode"]
    # the reference encoder (which this package wraps) writes the vector exactly, unless the
    # payload was made by hand (a non-minimal varint, a PCO payload without numbers...)
    encoded = codec.compress(decoded, hint=v["hint"], level=v["level"])
    assert (encoded == payload) is v["reference_encoding"]


def test_hand_made_payloads() -> None:
    # valid payloads no encoder writes, which decoders accept (spec section 4)
    made = [p["name"] for p in VALID if not p["reference_encoding"]]
    assert made == ["pco_no_values", "decimal_no_values", "zstd_empty_frame", "varint_overlong"]


@pytest.mark.parametrize("v", INVALID, ids=[p["name"] for p in INVALID])
def test_invalid_payload(v: dict[str, Any]) -> None:
    payload = (TESTDATA / "payloads" / v["payload"]).read_bytes()
    with pytest.raises(ppc.PcodecError) as e:
        codec.decompress(payload, v["expected_len"])
    assert str(e.value) == v["error"]


def expected_lines(entry: dict[str, Any]) -> list[str]:
    text: str = (TESTDATA / entry["expected"]).read_text(encoding="utf-8")
    return text.splitlines()


def project(line: str, keys: list[str]) -> str:
    """A canonical line with only `keys` (canonical JSON is what json.dumps writes compactly)."""
    row = json.loads(line)
    return json.dumps({k: row[k] for k in keys}, separators=(",", ":"), ensure_ascii=False)


@pytest.mark.parametrize("entry", FILES, ids=[f["name"] for f in FILES])
def test_each_row_group_reads_its_rows(entry: dict[str, Any]) -> None:
    lines = expected_lines(entry)
    f = ppc.ParquetFile(TESTDATA / entry["file"])
    got: list[str] = []
    for rg, n in enumerate(f.metadata.row_group_rows):
        part = list(canonical.lines(f.read(row_groups=[rg])))
        assert len(part) == n
        got += part
    assert got == lines
    # backwards, as asked
    backwards = [line for rg in reversed(range(f.num_row_groups)) for line in canonical.lines(f.read(row_groups=[rg]))]
    assert sorted(backwards) == sorted(lines)


@pytest.mark.parametrize("entry", FILES, ids=[f["name"] for f in FILES])
def test_each_column_reads_alone_and_reordered(entry: dict[str, Any]) -> None:
    lines = expected_lines(entry)
    f = ppc.ParquetFile(TESTDATA / entry["file"])
    names = f.schema.names
    for name in names:
        assert list(canonical.lines(f.read(columns=[name]))) == [project(line, [name]) for line in lines]
    backwards = names[::-1]
    assert list(canonical.lines(f.read(columns=backwards))) == [project(line, backwards) for line in lines]


@pytest.mark.parametrize("entry", FILES, ids=[f["name"] for f in FILES])
def test_limits_and_memory_reads(entry: dict[str, Any]) -> None:
    lines = expected_lines(entry)
    path = TESTDATA / entry["file"]
    f = ppc.ParquetFile(path)
    for limit in [0, 1, len(lines) // 2, len(lines) + 1]:
        assert list(canonical.lines(f.stream(limit=limit))) == lines[:limit]
    in_memory = ppc.ParquetFile(io.BytesIO(path.read_bytes()))
    assert canonical.dumps(in_memory.read()) == canonical.dumps(f.read())
    assert in_memory.metadata == f.metadata


@pytest.mark.parametrize("entry", FILES, ids=[f["name"] for f in FILES])
def test_rewritten_by_this_package(entry: dict[str, Any]) -> None:
    # what this package writes from a vector reads to the same rows, under every policy
    expected = (TESTDATA / entry["expected"]).read_bytes()
    table = ppc.read_table(TESTDATA / entry["file"])
    for policy in ["auto", "pcodec", "base"]:
        buf = io.BytesIO()
        ppc.write_table(table, buf, policy=policy)  # type: ignore[arg-type]
        assert canonical.dumps(ppc.read_table(io.BytesIO(buf.getvalue()))).encode() == expected


@pytest.mark.parametrize("v", VALID, ids=[p["name"] for p in VALID])
def test_valid_payload_at_other_levels(v: dict[str, Any]) -> None:
    decoded = (TESTDATA / "payloads" / v["decoded"]).read_bytes()
    for level in [0, 4, 12]:
        payload = codec.compress(decoded, hint=v["hint"], level=level)
        assert codec.decompress(payload, len(decoded)) == decoded
    with pytest.raises(ppc.PcodecError):
        codec.decompress((TESTDATA / "payloads" / v["payload"]).read_bytes(), len(decoded) + 1)
