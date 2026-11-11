"""The Writer's life (close, abort, use after either) and what a failed write leaves behind."""

from __future__ import annotations

import io
import pathlib
from collections.abc import Callable, Iterator
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import tparquet as ppc

TABLE = pa.table({"a": pa.array(range(100), pa.int64()), "b": pa.array([i / 4 for i in range(100)])})


def union_table() -> pa.Table:
    """A table whose union column no Parquet writer takes."""
    u = pa.UnionArray.from_sparse(pa.array([0, 1], pa.int8()), [pa.array([1, 2]), pa.array(["a", "b"])])
    return pa.table({"a": pa.array([1, 2], pa.int64()), "u": u})


# ---------------------------------------------------------------- close and abort


def test_abort_after_close_keeps_the_file(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "w.parquet"
    w = ppc.Writer(path, TABLE.schema)
    w.write(TABLE)
    codecs = w.close()
    w.abort()
    assert ppc.read_table(path).equals(TABLE)
    assert w.closed
    assert w.codecs == codecs


def test_close_then_an_exception_keeps_the_file(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "w.parquet"

    def body() -> None:
        with ppc.Writer(path, TABLE.schema) as w:
            w.write(TABLE)
            w.close()
            raise KeyError("after close")

    with pytest.raises(KeyError):
        body()
    assert ppc.read_table(path).equals(TABLE)


def test_abort_twice(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "w.parquet"
    w = ppc.Writer(path, TABLE.schema)
    w.write(TABLE)
    w.abort()
    assert not path.exists()
    w.abort()
    assert w.closed
    assert w.codecs is None


@pytest.mark.parametrize("end", ["close", "abort"])
def test_a_finished_writer_refuses_everything_but_abort(tmp_path: pathlib.Path, end: str) -> None:
    w = ppc.Writer(tmp_path / "w.parquet", TABLE.schema)
    w.write(TABLE)
    (w.close if end == "close" else w.abort)()
    calls: list[Callable[[], object]] = [
        lambda: w.write(TABLE),
        w.flush,
        w.close,
        lambda: w.write(TABLE.to_batches()[0]),
    ]
    for call in calls:
        with pytest.raises(ppc.PcodecError, match=r"^the writer is closed$"):
            call()
    w.abort()


def test_abort_of_a_file_object_writer_leaves_the_object() -> None:
    buf = io.BytesIO()
    w = ppc.Writer(buf, TABLE.schema, policy="pcodec")
    w.write(TABLE)
    w.abort()
    w.abort()
    assert not buf.closed


def test_abort_of_a_file_removed_meanwhile(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "w.parquet"
    w = ppc.Writer(path, TABLE.schema)
    path.unlink()
    w.abort()
    assert not path.exists()


# ---------------------------------------------------------------- what a failure leaves


def failing_stream(after: int) -> pa.RecordBatchReader:
    def batches() -> Iterator[pa.RecordBatch]:
        yield from TABLE.to_batches(max_chunksize=10)[:after]
        raise RuntimeError("the source broke")

    return pa.RecordBatchReader.from_batches(TABLE.schema, batches())


@pytest.mark.parametrize("after", [0, 1, 3])
def test_a_stream_that_fails_leaves_no_file(tmp_path: pathlib.Path, after: int) -> None:
    path = tmp_path / "w.parquet"
    with pytest.raises(ppc.PcodecError, match="the source broke"):
        ppc.write_table(failing_stream(after), path)
    assert list(tmp_path.iterdir()) == []


def test_a_stream_that_fails_in_a_writer_leaves_it_usable(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "w.parquet"
    with ppc.Writer(path, TABLE.schema) as w:
        with pytest.raises(ppc.PcodecError, match="the source broke"):
            w.write(failing_stream(0))
        w.write(TABLE)
    assert ppc.read_table(path).equals(TABLE)


def existing(tmp_path: pathlib.Path) -> tuple[pathlib.Path, bytes]:
    path = tmp_path / "old.parquet"
    pq.write_table(TABLE, path)
    return path, path.read_bytes()


@pytest.mark.parametrize(
    ("kwargs", "message"),
    [
        ({"columns": {"zz": "pcodec"}}, 'no column named "zz"'),
        ({"columns": {"a.b": "pcodec"}}, 'no column named "a.b"'),
        ({"policy": "nope"}, "unknown policy"),
        ({"level": 13}, "level 13"),
        ({"base": "nope"}, "base codec"),
        ({"row_group_size": -1}, "row_group_size"),
    ],
)
def test_refused_options_leave_an_existing_file_alone(
    tmp_path: pathlib.Path, kwargs: dict[str, Any], message: str
) -> None:
    path, before = existing(tmp_path)
    with pytest.raises(ppc.PcodecError, match=message):
        ppc.write_table(TABLE, path, **kwargs)
    with pytest.raises(ppc.PcodecError, match=message):
        ppc.write_table(TABLE.to_batches()[0], path, **kwargs)
    with pytest.raises(ppc.PcodecError, match=message):
        ppc.Writer(path, TABLE.schema, **kwargs)
    if "row_group_size" not in kwargs:  # transcode keeps the row groups
        source = tmp_path / "source.parquet"
        pq.write_table(TABLE, source)
        with pytest.raises(ppc.PcodecError, match=message):
            ppc.transcode(source, path, **kwargs)
    assert path.read_bytes() == before


def test_a_schema_no_writer_takes_leaves_an_existing_file_alone(tmp_path: pathlib.Path) -> None:
    path, before = existing(tmp_path)
    table = union_table()
    with pytest.raises(ppc.PcodecError):
        ppc.write_table(table, path)
    with pytest.raises(ppc.PcodecError):
        ppc.write_table(table.to_batches()[0], path)
    with pytest.raises(ppc.PcodecError):
        ppc.Writer(path, table.schema)
    assert path.read_bytes() == before


def test_a_transcode_that_fails_leaves_no_file(tmp_path: pathlib.Path) -> None:
    good = tmp_path / "good.parquet"
    ppc.write_table(pa.table({"a": pa.array(range(100_000), pa.int64())}), good)
    data = bytearray(good.read_bytes())
    for i in range(100, 400):  # in the first page, past its header
        data[i] ^= 0x5A
    bad = tmp_path / "bad.parquet"
    bad.write_bytes(bytes(data))
    out = tmp_path / "out.parquet"
    for target in ["pcodec", "standard"]:
        with pytest.raises(ppc.PcodecError):
            ppc.transcode(bad, out, target=target)  # type: ignore[arg-type]
        assert not out.exists()


# ---------------------------------------------------------------- the data must match the schema


SAME_TYPES = pa.table({"a": pa.array(range(5), pa.int64()), "b": pa.array(range(100, 105), pa.int64())})


@pytest.mark.parametrize(
    "data",
    [
        SAME_TYPES.select(["b", "a"]),
        SAME_TYPES.rename_columns(["x", "y"]),
        SAME_TYPES.rename_columns(["a", "B"]),
        SAME_TYPES.select(["a"]),
        SAME_TYPES.append_column("c", pa.array(range(5), pa.int64())),
        pa.table({"a": pa.array(range(5), pa.int32()), "b": pa.array(range(100, 105), pa.int64())}),
    ],
    ids=["swapped", "renamed", "case", "fewer", "more", "type"],
)
def test_data_that_does_not_match_the_schema_is_refused(tmp_path: pathlib.Path, data: pa.Table) -> None:
    path = tmp_path / "w.parquet"
    with ppc.Writer(path, SAME_TYPES.schema) as w:
        both: list[ppc.ArrowData] = [data, data.to_batches()[0]]
        for d in both:
            with pytest.raises(ppc.PcodecError, match=r"^the data does not match the writer's schema"):
                w.write(d)
        w.write(SAME_TYPES)
    assert ppc.read_table(path).equals(SAME_TYPES)


def test_metadata_and_nullability_need_not_match(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "w.parquet"
    required = pa.schema([pa.field("a", pa.int64(), nullable=False), pa.field("b", pa.int64())], metadata={"k": "v"})
    with ppc.Writer(path, required) as w:
        w.write(SAME_TYPES)
        w.write(SAME_TYPES.replace_schema_metadata({"other": "x"}))
    back = ppc.read_table(path)
    assert back.schema == required
    assert back.schema.metadata[b"k"] == b"v"
    with ppc.Writer(path, required) as w, pytest.raises(ppc.PcodecError, match="null"):
        w.write(pa.table({"a": pa.array([None], pa.int64()), "b": pa.array([1], pa.int64())}))


def test_unions_are_refused_by_name(tmp_path: pathlib.Path) -> None:
    table = union_table()
    u = table["u"].combine_chunks()
    nested = pa.table({"s": pa.StructArray.from_arrays([u], ["inner"])})
    listed = pa.table({"i": pa.array([1]), "l": pa.ListArray.from_arrays(pa.array([0, 2], pa.int32()), u)})
    cases: list[tuple[Any, str]] = [(table, "u"), (nested, "s.inner"), (listed, "l.item"), (table.to_batches()[0], "u")]
    for data, column in cases:
        with pytest.raises(ppc.PcodecError, match=rf"^the column {column} is a union, which Parquet cannot store$"):
            ppc.write_table(data, tmp_path / "x.parquet")
    with pytest.raises(ppc.PcodecError, match=r"^the column u is a union"):
        ppc.Writer(tmp_path / "x.parquet", table.schema)
    assert list(tmp_path.iterdir()) == []


# ---------------------------------------------------------------- policies and options


def file_codecs(buf: io.BytesIO) -> dict[str, str]:
    """Leaf path to the codec of its (one) chunk codec, as the footer says."""
    codecs = ppc.ParquetFile(io.BytesIO(buf.getvalue())).metadata.codecs
    assert all(len(c) == 1 for c in codecs.values())
    return {path: c[0] for path, c in codecs.items()}


def written(table: pa.Table, **kwargs: Any) -> tuple[dict[str, str], dict[str, str]]:
    """(the codecs write_table reports, the codecs the file has)."""
    buf = io.BytesIO()
    reported = ppc.write_table(table, buf, **kwargs)
    assert ppc.read_table(io.BytesIO(buf.getvalue())).equals(table)
    return reported, file_codecs(buf)


NESTED = pa.table(
    {
        "a.b": pa.array(range(500), pa.int64()),
        "a": pa.array([{"b": i, "c": i / 2} for i in range(500)]),
        "ab": pa.array(range(500), pa.int64()),
        "l": pa.array([[i, i + 1] for i in range(500)], pa.list_(pa.int64())),
        "m": pa.array([[("k", float(i))] for i in range(500)], pa.map_(pa.string(), pa.float64())),
    }
)


@pytest.mark.parametrize(
    ("columns", "pcodec"),
    [
        ({"a": "pcodec"}, {"a.b", "a.c"}),
        ({"a.c": "pcodec"}, {"a.c"}),
        # a dotted name names the top-level field "a.b" and the leaf b of "a": both
        ({"a.b": "pcodec"}, {"a.b"}),
        ({"ab": "pcodec"}, {"ab"}),
        ({"l": "pcodec"}, {"l.list.item"}),
        ({"l.list.item": "pcodec"}, {"l.list.item"}),
        ({"m.entries.value": "pcodec"}, {"m.entries.value"}),
        # inner groups of the Parquet schema are whole names too
        ({"l.list": "pcodec"}, {"l.list.item"}),
        ({"m.entries": "pcodec"}, {"m.entries.value"}),
        # the longest name wins
        ({"a": "pcodec", "a.c": "base"}, {"a.b"}),
    ],
    ids=str,
)
def test_column_policies_on_nested_and_dotted_names(columns: dict[str, ppc.Policy], pcodec: set[str]) -> None:
    reported, actual = written(NESTED, policy="base", columns=columns)
    assert reported == actual
    # "a.b" is both the top-level column and a's leaf: one key holds both
    assert {path for path, c in reported.items() if c == "PCODEC"} == pcodec
    assert ppc.ParquetFile(io.BytesIO(_bytes(NESTED, policy="base", columns=columns))).metadata.codecs.keys() == {
        "a.b",
        "a.c",
        "ab",
        "l.list.item",
        "m.entries.key",
        "m.entries.value",
    }


def _bytes(table: pa.Table, **kwargs: Any) -> bytes:
    buf = io.BytesIO()
    ppc.write_table(table, buf, **kwargs)
    return buf.getvalue()


def test_a_name_must_be_a_whole_field(tmp_path: pathlib.Path) -> None:
    for name in ["a.", "a.b.c", "l.lis", "A", ""]:
        with pytest.raises(ppc.PcodecError, match=rf"^no column named {name!r}$".replace("'", '"')):
            ppc.write_table(NESTED, io.BytesIO(), columns={name: "pcodec"})


def test_each_policy_and_the_reports_match_the_file() -> None:
    smooth = pa.table(
        {
            "t": pa.array(range(0, 3_000_000, 1000), pa.int64()),
            "noise": pa.array([(i * 2654435761) % 2**31 for i in range(3000)], pa.int32()),
            "s": pa.array([str(i % 5) for i in range(3000)]),
        }
    )
    for policy in ["auto", "pcodec", "base"]:
        reported, actual = written(smooth, policy=policy)
        assert reported == actual, policy
        assert reported["s"] == "ZSTD"
    assert written(smooth, policy="pcodec")[0] == {"t": "PCODEC", "noise": "PCODEC", "s": "ZSTD"}
    assert written(smooth, policy="base")[0] == {"t": "ZSTD", "noise": "ZSTD", "s": "ZSTD"}
    assert written(smooth)[0]["t"] == "PCODEC"


def test_auto_keeps_the_base_codec_when_it_is_smaller_in_total() -> None:
    # spec section 6: a file with PCODEC has V2 pages, whose levels are not compressed; when
    # that costs the other columns more than PCODEC saves, every column keeps the base codec
    import random

    r = random.Random(12)
    mid, bids, asks = 6_500_000, [], []
    for _ in range(4000):
        mid += r.randrange(5) - 2
        bids.append([(mid - k) / 100 for k in range(20)])
        asks.append([(mid + 1 + k) / 100 for k in range(20)])
    table = pa.table({"t": pa.array(range(4000), pa.int64()), "bids": bids, "asks": asks})
    auto, base = _bytes(table), _bytes(table, policy="base")
    pcodec_t = _bytes(table, policy="base", columns={"t": "pcodec"})
    t_size = {c["path"]: c["compressed_bytes"] for c in ppc.inspect(io.BytesIO(pcodec_t))["columns"]}["t"]
    assert t_size < {c["path"]: c["compressed_bytes"] for c in ppc.inspect(io.BytesIO(base))["columns"]}["t"]
    assert len(pcodec_t) > len(base)
    assert written(table)[0] == {"t": "ZSTD", "bids.list.item": "ZSTD", "asks.list.item": "ZSTD"}
    assert len(auto) == len(base)


def test_the_choice_waits_for_rows() -> None:
    table = TABLE
    w = ppc.Writer(io.BytesIO(), table.schema)
    assert w.codecs is None
    w.write(table.slice(0, 0))
    w.flush()
    assert w.codecs is None
    w.write(table.slice(0, 50))
    chosen = w.codecs
    assert chosen is not None
    assert w.close() == chosen
    # forced policies are known at once
    assert ppc.Writer(io.BytesIO(), table.schema, policy="pcodec").codecs == {"a": "PCODEC", "b": "PCODEC"}
    assert ppc.Writer(io.BytesIO(), table.schema, policy="base", base="snappy").codecs == {"a": "SNAPPY", "b": "SNAPPY"}
    # without rows, Auto columns get the base codec
    empty = ppc.Writer(io.BytesIO(), table.schema)
    assert empty.close() == {"a": "ZSTD", "b": "ZSTD"}


@pytest.mark.parametrize("level", range(13))
def test_every_level(level: int) -> None:
    buf = io.BytesIO()
    ppc.write_table(TABLE, buf, policy="pcodec", level=level)
    assert ppc.read_table(io.BytesIO(buf.getvalue())).equals(TABLE)


def test_levels_change_the_payload() -> None:
    import random

    r = random.Random(5)
    table = pa.table({"x": pa.array([int(r.expovariate(1e-4)) for _ in range(50_000)], pa.int64())})
    sizes = {len(_bytes(table, policy="pcodec", level=level)) for level in [0, 4, 8, 12]}
    assert len(sizes) > 1


@pytest.mark.parametrize("rows", [1, 2, 99, 100, 101, 10**9])
def test_row_group_sizes(rows: int) -> None:
    buf = io.BytesIO()
    ppc.write_table(TABLE, buf, row_group_size=rows)
    md = ppc.ParquetFile(io.BytesIO(buf.getvalue())).metadata
    assert md.row_group_rows == [min(rows, 100 - i) for i in range(0, 100, rows)]


def test_row_group_size_with_a_writer_and_flush() -> None:
    buf = io.BytesIO()
    with ppc.Writer(buf, TABLE.schema, row_group_size=30) as w:
        w.write(TABLE.slice(0, 40))
        w.flush()
        w.write(TABLE.slice(40, 60))
    assert ppc.ParquetFile(io.BytesIO(buf.getvalue())).metadata.row_group_rows == [30, 10, 30, 30]


def test_an_imported_stream_is_released() -> None:
    import gc
    import weakref

    class Batches:
        """An iterator the stream holds until it is released (a stream that ends drops it
        anyway, so this one fails part way)."""

        def __init__(self) -> None:
            self.sent = False

        def __iter__(self) -> Batches:
            return self

        def __next__(self) -> pa.RecordBatch:
            if self.sent:
                raise RuntimeError("the source broke")
            self.sent = True
            return TABLE.to_batches()[0]

    writes: list[Callable[[pa.RecordBatchReader], object]] = [
        lambda r: ppc.write_table(r, io.BytesIO()),
        lambda r: ppc.Writer(io.BytesIO(), TABLE.schema).write(r),
    ]
    for write in writes:
        it = Batches()
        alive = weakref.ref(it)
        with pytest.raises(ppc.PcodecError, match="the source broke"):
            write(pa.RecordBatchReader.from_batches(TABLE.schema, it))
        del it
        gc.collect()
        assert alive() is None
