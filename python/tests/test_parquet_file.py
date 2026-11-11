"""ParquetFile and the readers: metadata, row groups, streams with columns, row groups,
batch sizes and limits, one-shot streams, and many files and streams opened and dropped."""

from __future__ import annotations

import gc
import io
import os
import pathlib
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import tparquet as ppc

TABLE = pa.table(
    {
        "id": pa.array(range(1000), pa.int64()),
        "x": pa.array([i / 3 for i in range(1000)], pa.float32()),
        "s": pa.array([f"v{i % 11}" for i in range(1000)]),
        "nested": pa.array([{"a": i, "b": [i, i]} for i in range(1000)]),
    },
    metadata={"origin": "tests"},
)


@pytest.fixture
def path(tmp_path: pathlib.Path) -> pathlib.Path:
    p = tmp_path / "t.parquet"
    ppc.write_table(TABLE, p, policy="pcodec", row_group_size=300)
    return p


def rows(stream: Any) -> pa.Table:
    return pa.RecordBatchReader.from_stream(stream).read_all()


def test_metadata(path: pathlib.Path) -> None:
    f = ppc.ParquetFile(path)
    md = f.metadata
    assert md.num_rows == 1000
    assert md.row_group_rows == [300, 300, 300, 100]
    assert md.num_row_groups == f.num_row_groups == 4
    assert md.pcodec_version == "1"
    assert md.key_value_metadata["parquet.pcodec.version"] == "1"
    assert "ARROW:schema" in md.key_value_metadata
    assert md.created_by is not None
    assert md.physical_types == {
        "id": "INT64",
        "x": "FLOAT",
        "s": "BYTE_ARRAY",
        "nested.a": "INT64",
        "nested.b.list.item": "INT64",
    }
    assert md.codecs["id"] == ["PCODEC"]
    assert md.codecs["s"] == ["ZSTD"]
    assert f.schema == TABLE.schema
    assert f.schema.metadata is not None
    assert f.schema.metadata[b"origin"] == b"tests"
    assert f.schema_capsule.names == ["id", "x", "s", "nested"]  # type: ignore[attr-defined]
    assert pa.schema(f.schema_capsule) == TABLE.schema  # type: ignore[arg-type]
    assert (
        repr(f) == "<ParquetFile: 1000 rows, 4 row groups, columns ['id', 'x', 's', 'nested.a', 'nested.b.list.item']>"
    )


def test_metadata_is_frozen(path: pathlib.Path) -> None:
    md = ppc.ParquetFile(path).metadata
    with pytest.raises(AttributeError):
        md.num_rows = 3  # type: ignore[misc]


def test_codecs_per_row_group_in_file_order(tmp_path: pathlib.Path) -> None:
    # a file whose row groups use different codecs (written by pyarrow, then appended to)
    path = tmp_path / "mixed.parquet"
    with pq.ParquetWriter(path, TABLE.select(["id"]).schema, compression={"id": "snappy"}) as w:
        w.write_table(TABLE.select(["id"]).slice(0, 10))
    assert ppc.ParquetFile(path).metadata.codecs == {"id": ["SNAPPY"]}
    out = tmp_path / "out.parquet"
    ppc.transcode(path, out, policy="pcodec")
    assert ppc.ParquetFile(out).metadata.codecs == {"id": ["PCODEC"]}


@pytest.mark.parametrize("columns", [["s"], ["nested", "id"], ["x", "s", "id", "nested"], []])
def test_columns_in_the_order_asked(path: pathlib.Path, columns: list[str]) -> None:
    f = ppc.ParquetFile(path)
    assert f.read(columns=columns).equals(TABLE.select(columns))
    assert rows(f.stream(columns=columns)).equals(TABLE.select(columns))
    assert pa.schema(f.empty(columns)) == TABLE.select(columns).schema  # type: ignore[arg-type]
    assert ppc.read_table(path, columns=columns).num_rows == 1000


def test_columns_are_top_level_names(path: pathlib.Path) -> None:
    with pytest.raises(ppc.PcodecError, match=r"^no column named 'nested.a'$"):
        ppc.read_table(path, columns=["nested.a"])
    with pytest.raises(ppc.PcodecError, match=r"^column 'id' is requested more than once$"):
        ppc.read_table(path, columns=["id", "s", "id"])


@pytest.mark.parametrize("groups", [[0], [3], [2, 0], [1, 1], [3, 2, 1, 0], []])
def test_row_groups_in_the_order_asked(path: pathlib.Path, groups: list[int]) -> None:
    starts = [0, 300, 600, 900]
    lengths = [300, 300, 300, 100]
    parts = [TABLE.slice(starts[g], lengths[g]) for g in groups]
    want = pa.concat_tables(parts) if parts else TABLE.slice(0, 0)
    assert ppc.read_table(path, row_groups=groups).equals(want)
    assert pa.Table.from_batches(list(ppc.ParquetFile(path).iter_batches(row_groups=groups)), TABLE.schema).equals(want)


@pytest.mark.parametrize("bad", [4, 5, -1, 2**40])
def test_row_groups_out_of_range(path: pathlib.Path, bad: int) -> None:
    with pytest.raises(ppc.PcodecError, match=rf"^row group {bad} out of range \(the file has 4\)$"):
        ppc.read_table(path, row_groups=[0, bad])


@pytest.mark.parametrize("batch_size", [1, 7, 299, 300, 301, 1000, 10**6])
def test_batch_sizes(path: pathlib.Path, batch_size: int) -> None:
    batches = list(ppc.ParquetFile(path).iter_batches(batch_size=batch_size))
    assert all(b.num_rows <= batch_size for b in batches)
    assert sum(b.num_rows for b in batches) == 1000
    assert pa.Table.from_batches(batches).equals(TABLE)
    assert len(batches) == -(-1000 // batch_size)


@pytest.mark.parametrize("bad", [0, -1])
def test_bad_batch_sizes(path: pathlib.Path, bad: int) -> None:
    with pytest.raises(ppc.PcodecError, match=rf"^batch_size must be positive, got {bad}$"):
        ppc.read_table(path, batch_size=bad)


@pytest.mark.parametrize("limit", [0, 1, 299, 300, 301, 999, 1000, 5000])
def test_limits(path: pathlib.Path, limit: int) -> None:
    f = ppc.ParquetFile(path)
    assert rows(f.stream(limit=limit)).equals(TABLE.slice(0, limit))
    assert (
        rows(f.stream(limit=limit, row_groups=[3, 0], columns=["id"])).column("id").to_pylist()
        == (list(range(900, 1000)) + list(range(300)))[:limit]
    )


def test_streams_are_one_shot(path: pathlib.Path) -> None:
    f = ppc.ParquetFile(path)
    stream = f.stream()
    assert sum(b.num_rows for b in stream) == 1000
    assert list(stream) == []
    exported = f.stream()
    exported.__arrow_c_stream__()
    with pytest.raises(ppc.PcodecError, match="already consumed"):
        next(iter(exported))
    with pytest.raises(ppc.PcodecError, match="already consumed"):
        exported.__arrow_c_stream__()
    # the schema stays available
    assert pa.schema(exported) == TABLE.schema  # type: ignore[arg-type]


def test_batches_expose_the_c_array_interface(path: pathlib.Path) -> None:
    batch = next(iter(ppc.ParquetFile(path).stream(batch_size=10)))
    assert batch.num_rows == 10
    assert pa.record_batch(batch).equals(TABLE.slice(0, 10).to_batches()[0])
    # the schema carries the file's metadata
    assert pa.record_batch(batch).schema.metadata[b"origin"] == b"tests"


def test_a_file_is_read_by_many_streams(path: pathlib.Path) -> None:
    f = ppc.ParquetFile(path)
    streams = [iter(f.stream(batch_size=50)) for _ in range(10)]
    firsts = [next(s) for s in streams]
    assert all(pa.record_batch(b).equals(pa.record_batch(firsts[0])) for b in firsts)
    assert all(sum(b.num_rows for b in s) == 950 for s in streams)


def test_read_matches_pyarrow(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "std.parquet"
    pq.write_table(TABLE, path, row_group_size=128)
    assert ppc.read_table(path).equals(pq.read_table(path))
    assert ppc.ParquetFile(path).metadata.row_group_rows == [
        pq.ParquetFile(path).metadata.row_group(i).num_rows for i in range(8)
    ]


def fds() -> int:
    return len(os.listdir("/proc/self/fd"))


@pytest.mark.skipif(not os.path.isdir("/proc/self/fd"), reason="needs /proc")
def test_files_are_closed_when_dropped(path: pathlib.Path) -> None:
    gc.collect()
    before = fds()
    files = [ppc.ParquetFile(path) for _ in range(50)]
    streams = [f.stream() for f in files]
    next(iter(streams[0]))
    assert fds() >= before + 50
    del files, streams
    gc.collect()
    assert fds() == before


def rss() -> int:
    with open("/proc/self/statm") as f:
        return int(f.read().split()[1]) * os.sysconf("SC_PAGE_SIZE")


@pytest.mark.slow
@pytest.mark.skipif(not os.path.isdir("/proc/self/fd"), reason="needs /proc")
def test_no_leaks_when_opening_and_dropping(path: pathlib.Path) -> None:
    data = path.read_bytes()

    def once() -> None:
        f = ppc.ParquetFile(path)
        next(iter(f.stream(batch_size=100)))  # a stream left half-read
        ppc.ParquetFile(io.BytesIO(data)).stream().__arrow_c_stream__()  # a capsule never imported
        pa.RecordBatchReader.from_stream(f.stream()).read_next_batch()
        w = ppc.Writer(io.BytesIO(), TABLE.schema)
        w.write(TABLE)  # a writer never closed

    for _ in range(20):
        once()
    gc.collect()
    memory, files = rss(), fds()
    for _ in range(300):
        once()
    gc.collect()
    assert fds() == files
    assert rss() - memory < 64 << 20
