"""The core API: read_table, read_batches, ParquetFile, write_table, Writer, transcode,
inspect, verify, with paths and file-like objects."""

from __future__ import annotations

import io
import os
import re
from typing import TYPE_CHECKING

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import tparquet as ppc
from conftest import vector

if TYPE_CHECKING:
    import pathlib


def codecs_of(path: str | os.PathLike[str]) -> dict[str, list[str]]:
    return {c["path"]: c["codecs"] for c in ppc.inspect(path)["columns"]}


# ---------------------------------------------------------------- write_table / read_table


def test_round_trip_auto(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    choice = ppc.write_table(numbers, path)
    assert ppc.read_table(path).equals(numbers)
    # Auto picks PCODEC for the smooth numbers and the base codec for strings
    assert choice["i64"] == "PCODEC"
    assert choice["s"] == "ZSTD"
    assert set(choice) == {"i32", "i64", "f32", "f64", "s"}
    assert codecs_of(path)["i64"] == ["PCODEC"]


def test_policy_pcodec_and_base(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    p = tmp_path / "p.parquet"
    b = tmp_path / "b.parquet"
    assert ppc.write_table(numbers, p, policy="pcodec") == {
        "i32": "PCODEC",
        "i64": "PCODEC",
        "f32": "PCODEC",
        "f64": "PCODEC",
        "s": "ZSTD",
    }
    assert set(ppc.write_table(numbers, b, policy="base").values()) == {"ZSTD"}
    assert ppc.read_table(p).equals(numbers)
    assert ppc.read_table(b).equals(numbers)
    # a standard file: pyarrow reads it
    assert pq.read_table(b).equals(numbers)


def test_column_policies(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "c.parquet"
    choice = ppc.write_table(numbers, path, policy="base", columns={"f64": "pcodec"})
    assert choice["f64"] == "PCODEC"
    assert choice["i64"] == "ZSTD"
    assert codecs_of(path)["f64"] == ["PCODEC"]


def test_base_codec_and_level(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "s.parquet"
    choice = ppc.write_table(numbers, path, policy="pcodec", base="snappy", level=0)
    assert choice["s"] == "SNAPPY"
    assert ppc.read_table(path).equals(numbers)
    lvl12 = tmp_path / "l12.parquet"
    ppc.write_table(numbers, lvl12, policy="pcodec", level=12)
    assert ppc.read_table(lvl12).equals(numbers)


def test_row_group_size(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "rg.parquet"
    ppc.write_table(numbers, path, row_group_size=500)
    md = ppc.ParquetFile(path).metadata
    assert md.num_row_groups == 4
    assert md.row_group_rows == [500, 500, 500, 500]
    assert md.num_rows == 2000


def test_column_encodings(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "alp.parquet"
    encodings = {"f64": "ALP", "f32": "alp", "i64": "DELTA_BINARY_PACKED"}
    codecs = ppc.write_table(numbers, path, policy="base", base="zstd(9)", encodings=encodings)
    assert set(codecs.values()) == {"ZSTD"}
    md = pq.ParquetFile(path).metadata.row_group(0)
    # pyarrow does not know ALP (encoding 10) and names it UNKNOWN
    found = {md.column(i).path_in_schema: set(md.column(i).encodings) for i in range(md.num_columns)}
    assert "DELTA_BINARY_PACKED" in found["i64"]
    assert "RLE_DICTIONARY" in found["i32"]
    assert "RLE_DICTIONARY" not in found["f64"] | found["f32"]
    assert ppc.read_table(path).equals(numbers)
    bss = {"f64": "BYTE_STREAM_SPLIT"}
    with ppc.Writer(tmp_path / "w.parquet", numbers.schema, policy="base", encodings=bss) as w:
        w.write(numbers)
    # only strings in the asserts: pytest prints the operands of a failing one, and pyarrow's repr
    # of a column's metadata aborts the process on a codec it does not know (PCODEC)
    encodings = tuple(pq.ParquetFile(tmp_path / "w.parquet").metadata.row_group(0).column(3).encodings)
    assert "BYTE_STREAM_SPLIT" in encodings
    assert ppc.read_table(tmp_path / "w.parquet").equals(numbers)
    import polars as pl

    import tparquet.pandas as ppd
    import tparquet.polars as ppl

    pandas_out, polars_out = tmp_path / "pandas.parquet", tmp_path / "polars.parquet"
    ppd.to_parquet(numbers.to_pandas(), pandas_out, policy="base", encodings=bss)
    ppl.write_parquet(pl.DataFrame(numbers), polars_out, policy="base", encodings=bss)
    for out in (pandas_out, polars_out):
        encodings = tuple(pq.ParquetFile(out).metadata.row_group(0).column(3).encodings)
        assert "BYTE_STREAM_SPLIT" in encodings, str(out)


@pytest.mark.parametrize(
    ("kwargs", "message"),
    [
        ({"policy": "fast"}, "unknown policy"),
        ({"columns": {"nope": "pcodec"}}, "no column named"),
        ({"columns": {"i64": "fast"}}, "unknown policy"),
        ({"level": 13}, "level"),
        ({"level": -1}, "level"),
        ({"base": "pcodec(8)"}, "base codec cannot be PCODEC"),
        ({"base": "zstd(99)"}, "zstd"),
        ({"base": "nope"}, "nope"),
        ({"row_group_size": 0}, "row_group_size"),
        ({"encodings": {"nope": "PLAIN"}}, "no column named"),
        ({"encodings": {"f64": "FAST"}}, "FAST"),
        ({"encodings": {"f64": "RLE_DICTIONARY"}}, "dictionary encoding"),
    ],
)
def test_bad_write_options(tmp_path: pathlib.Path, numbers: pa.Table, kwargs: dict[str, object], message: str) -> None:
    with pytest.raises(ppc.PcodecError, match=message) as e:
        ppc.write_table(numbers, tmp_path / "x.parquet", **kwargs)  # type: ignore[arg-type]
    assert isinstance(e.value, ValueError)


def test_write_rejects_non_arrow(tmp_path: pathlib.Path) -> None:
    with pytest.raises(TypeError, match="__arrow_c_stream__"):
        ppc.write_table([1, 2, 3], tmp_path / "x.parquet")  # type: ignore[arg-type]


def test_write_record_batch_via_c_array(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    batch = numbers.to_batches()[0]

    class OnlyArray:
        def __arrow_c_array__(self, requested_schema: object | None = None) -> tuple[object, object]:
            return batch.__arrow_c_array__(requested_schema)  # type: ignore[no-any-return,no-untyped-call]

    path = tmp_path / "a.parquet"
    ppc.write_table(OnlyArray(), path)
    assert ppc.read_table(path).equals(pa.Table.from_batches([batch]))


def test_write_non_struct_array_fails(tmp_path: pathlib.Path) -> None:
    with pytest.raises(ppc.PcodecError, match="struct"):
        ppc.write_table(pa.array([1, 2, 3]), tmp_path / "x.parquet")


def test_empty_table(tmp_path: pathlib.Path) -> None:
    empty = pa.table({"x": pa.array([], pa.int64())})
    path = tmp_path / "e.parquet"
    # Auto columns that never see a row get the base codec
    assert ppc.write_table(empty, path) == {"x": "ZSTD"}
    assert ppc.read_table(path).equals(empty)


# ---------------------------------------------------------------- reading


def test_read_columns_in_requested_order(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    t = ppc.read_table(path, columns=["s", "i32"])
    assert t.column_names == ["s", "i32"]
    assert t.equals(numbers.select(["s", "i32"]))


def test_read_unknown_or_duplicate_column(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    with pytest.raises(ppc.PcodecError, match="no column named 'zz'"):
        ppc.read_table(path, columns=["zz"])
    with pytest.raises(ppc.PcodecError, match="more than once"):
        ppc.read_table(path, columns=["s", "s"])


def test_read_row_groups(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "rg.parquet"
    ppc.write_table(numbers, path, row_group_size=500)
    t = ppc.read_table(path, row_groups=[3, 1])
    assert t.equals(pa.concat_tables([numbers.slice(1500, 500), numbers.slice(500, 500)]))
    assert ppc.read_table(path, row_groups=[]).num_rows == 0
    with pytest.raises(ppc.PcodecError, match="row group 4 out of range"):
        ppc.read_table(path, row_groups=[4])
    with pytest.raises(ppc.PcodecError, match="row group -1 out of range"):
        ppc.read_table(path, row_groups=[-1])


def test_batch_size(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    batches = list(ppc.ParquetFile(path).iter_batches(batch_size=300))
    assert [b.num_rows for b in batches] == [300] * 6 + [200]
    assert ppc.read_table(path, batch_size=7).to_batches()[0].num_rows == 7
    with pytest.raises(ppc.PcodecError, match="batch_size"):
        ppc.read_table(path, batch_size=0)


def test_read_batches_is_an_arrow_stream(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    stream = ppc.read_batches(path, columns=["i64"], batch_size=1000)
    reader = pa.RecordBatchReader.from_stream(stream)
    assert reader.schema == pa.schema([pa.field("i64", pa.int64())])
    assert reader.read_all().equals(numbers.select(["i64"]))
    # one shot, like any C stream
    with pytest.raises(ppc.PcodecError, match="already consumed"):
        pa.RecordBatchReader.from_stream(stream)


def test_stream_schema_capsule(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    stream = ppc.read_batches(path)
    assert pa.schema(stream) == numbers.schema  # type: ignore[arg-type]
    assert pa.schema(ppc.read_batches(path, columns=["s"])) == numbers.select(["s"]).schema  # type: ignore[arg-type]


def test_stream_requested_schema(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    same = pa.RecordBatchReader.from_stream(ppc.read_batches(path), schema=numbers.schema)
    assert same.read_all().equals(numbers)
    other = pa.schema([pa.field("i32", pa.int64())])
    with pytest.raises(ppc.PcodecError, match="cast"):
        pa.RecordBatchReader.from_stream(ppc.read_batches(path, columns=["i32"]), schema=other)


def test_parquet_file(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path, policy="pcodec", row_group_size=1500)
    f = ppc.ParquetFile(path)
    assert f.schema == numbers.schema
    md = f.metadata
    assert md.num_rows == 2000
    assert md.num_row_groups == 2
    assert md.row_group_rows == [1500, 500]
    assert md.pcodec_version == "1"
    assert md.created_by is not None
    assert md.created_by.startswith("parquet-rs")
    assert md.codecs == {"i32": ["PCODEC"], "i64": ["PCODEC"], "f32": ["PCODEC"], "f64": ["PCODEC"], "s": ["ZSTD"]}
    assert md.physical_types == {"i32": "INT32", "i64": "INT64", "f32": "FLOAT", "f64": "DOUBLE", "s": "BYTE_ARRAY"}
    assert f.read().equals(numbers)
    assert f.read(columns=["f64"], row_groups=[1]).equals(numbers.select(["f64"]).slice(1500))
    assert pa.Table.from_batches(list(f.iter_batches(columns=["s"]))).equals(numbers.select(["s"]))
    assert f.num_row_groups == 2
    assert "ParquetFile" in repr(f)


def test_metadata_of_a_standard_file() -> None:
    md = ppc.ParquetFile(vector("standard_control")).metadata
    assert md.pcodec_version is None
    assert "PCODEC" not in {c for cs in md.codecs.values() for c in cs}


# ---------------------------------------------------------------- paths and file objects


def test_str_and_pathlike(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    ppc.write_table(numbers, str(tmp_path / "s.parquet"))

    class P:
        def __fspath__(self) -> str:
            return str(tmp_path / "s.parquet")

    assert ppc.read_table(P()).equals(numbers)
    assert ppc.read_table(os.fsencode(tmp_path / "s.parquet")).equals(numbers)


def test_file_objects(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    buf = io.BytesIO()
    ppc.write_table(numbers, buf)
    assert not buf.closed
    data = buf.getvalue()
    assert data[:4] == b"PAR1"
    assert data[-4:] == b"PAR1"
    assert ppc.read_table(io.BytesIO(data)).equals(numbers)
    with open(tmp_path / "f.parquet", "wb") as f:
        ppc.write_table(numbers, f)
    with open(tmp_path / "f.parquet", "rb") as f:
        assert ppc.read_table(f).equals(numbers)
        f.seek(0)
        assert ppc.ParquetFile(f).metadata.num_rows == 2000


def test_file_object_errors_propagate(numbers: pa.Table) -> None:
    class Broken(io.RawIOBase):
        def writable(self) -> bool:
            return True

        def write(self, b: object) -> int:
            raise RuntimeError("disk on fire")

    with pytest.raises(RuntimeError, match="disk on fire"):
        ppc.write_table(numbers, Broken())


def test_buffered_file_objects_are_flushed(numbers: pa.Table) -> None:
    raw = io.BytesIO()
    buffered = io.BufferedWriter(raw, buffer_size=1 << 20)
    ppc.write_table(numbers, buffered)
    # write_table flushes what it wrote: the raw stream has the whole file
    assert ppc.read_table(io.BytesIO(raw.getvalue())).equals(numbers)
    raw2 = io.BytesIO()
    buffered2 = io.BufferedWriter(raw2, buffer_size=1 << 20)
    std = io.BytesIO()
    ppc.write_table(numbers, std, policy="base")
    ppc.transcode(io.BytesIO(std.getvalue()), buffered2)
    assert ppc.read_table(io.BytesIO(raw2.getvalue())).equals(numbers)


def test_short_writes_are_retried(numbers: pa.Table) -> None:
    out = io.BytesIO()

    class Trickle:
        def write(self, b: bytes) -> int:
            return out.write(bytes(b)[:3])

    ppc.write_table(numbers, Trickle())
    assert ppc.read_table(io.BytesIO(out.getvalue())).equals(numbers)


def test_write_returning_none_means_all_written(numbers: pa.Table) -> None:
    out = io.BytesIO()

    class NoCount:
        def write(self, b: bytes) -> None:
            out.write(b)

    ppc.write_table(numbers, NoCount())
    assert ppc.read_table(io.BytesIO(out.getvalue())).equals(numbers)


def test_bad_path_types(numbers: pa.Table) -> None:
    with pytest.raises(TypeError, match=r"^expected a path or a binary file object, got int$"):
        ppc.read_table(42)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match=r"^expected a path or a binary file object, got int$"):
        ppc.write_table(numbers, 42)  # type: ignore[arg-type]

    class Text:
        def read(self) -> str:
            return "PAR1"

    with pytest.raises(TypeError, match=r"^the file object must return bytes from read\(\), got str$"):
        ppc.read_table(Text())  # type: ignore[arg-type]


def test_missing_file_is_os_error(tmp_path: pathlib.Path) -> None:
    with pytest.raises(FileNotFoundError) as e:
        ppc.read_table(tmp_path / "missing.parquet")
    assert e.value.filename == str(tmp_path / "missing.parquet")
    with pytest.raises(FileNotFoundError):
        ppc.write_table(pa.table({"a": [1]}), tmp_path / "no" / "dir.parquet")


def test_not_parquet_is_pcodec_error(tmp_path: pathlib.Path) -> None:
    junk = tmp_path / "junk.parquet"
    junk.write_bytes(b"this is not parquet at all")
    with pytest.raises(ppc.PcodecError, match="parquet"):
        ppc.read_table(junk)
    with pytest.raises(ppc.PcodecError):
        ppc.read_table(io.BytesIO(b""))
    with pytest.raises(ppc.PcodecError):
        ppc.inspect(junk)


def test_corrupt_page_is_pcodec_error(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    good = tmp_path / "good.parquet"
    ppc.write_table(numbers.select(["i64"]), good, policy="pcodec")
    data = bytearray(good.read_bytes())
    # the first page's payload starts after its header; flip bytes in the middle of the chunk
    for i in range(60, 120):
        data[i] ^= 0x5A
    with pytest.raises(ppc.PcodecError):
        ppc.read_table(io.BytesIO(bytes(data)))


# ---------------------------------------------------------------- Writer


def test_writer_streams_batches(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "w.parquet"
    with ppc.Writer(path, numbers.schema, row_group_size=10_000) as w:
        assert w.codecs is None
        for b in numbers.to_batches(max_chunksize=300):
            w.write(b)
        assert w.codecs is not None
        assert w.codecs["i64"] == "PCODEC"
        w.flush()
        w.write(numbers.slice(0, 100))
    assert w.closed
    assert w.codecs is not None
    assert w.codecs["s"] == "ZSTD"
    md = ppc.ParquetFile(path).metadata
    assert md.row_group_rows == [2000, 100]
    assert ppc.read_table(path).equals(pa.concat_tables([numbers, numbers.slice(0, 100)]))


def test_writer_schema_from_capsule_and_mismatch(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "w.parquet"
    w = ppc.Writer(path, numbers.schema)
    with pytest.raises(ppc.PcodecError, match="schema"):
        w.write(pa.table({"other": [1]}))
    w.write(numbers)
    codecs = w.close()
    assert codecs == w.codecs
    assert ppc.read_table(path).equals(numbers)
    with pytest.raises(ppc.PcodecError, match="closed"):
        w.write(numbers)
    with pytest.raises(ppc.PcodecError, match="closed"):
        w.close()


def write_then_fail(sink: ppc.Sink, table: pa.Table) -> ppc.Writer:
    """Writes `table` in a `with` block that raises `KeyError`; returns the writer."""
    w = ppc.Writer(sink, table.schema)

    def body() -> None:
        with w:
            w.write(table)
            raise KeyError("stop")

    with pytest.raises(KeyError, match="stop"):
        body()
    return w


def test_writer_abort_removes_path(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "w.parquet"
    w = write_then_fail(path, numbers)
    assert not path.exists()
    assert w.closed


def test_writer_abort_keeps_file_objects(numbers: pa.Table) -> None:
    buf = io.BytesIO()
    w = write_then_fail(buf, numbers)
    assert not buf.closed
    assert w.closed


def test_writer_takes_batches_through_c_array(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    batch = numbers.to_batches()[0]

    class OnlyArray:
        def __arrow_c_array__(self, requested_schema: object | None = None) -> tuple[object, object]:
            return batch.__arrow_c_array__(requested_schema)  # type: ignore[no-any-return,no-untyped-call]

    path = tmp_path / "w.parquet"
    with ppc.Writer(path, numbers.schema) as w:
        w.write(OnlyArray())
        w.write(OnlyArray())
        with pytest.raises(TypeError, match="__arrow_c_stream__ or __arrow_c_array__, got list"):
            w.write([1, 2])  # type: ignore[arg-type]
    assert ppc.read_table(path).equals(pa.concat_tables([numbers, numbers]))


def test_writer_column_policies(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "w.parquet"
    with ppc.Writer(path, numbers.schema, policy="base", columns={"i64": "pcodec"}) as w:
        w.write(numbers)
    assert w.codecs is not None
    assert w.codecs["i64"] == "PCODEC"
    assert w.codecs["i32"] == "ZSTD"


def test_writer_needs_a_schema(tmp_path: pathlib.Path) -> None:
    with pytest.raises(TypeError, match="__arrow_c_schema__, got str"):
        ppc.Writer(tmp_path / "x.parquet", "nope")  # type: ignore[arg-type]


# ---------------------------------------------------------------- transcode, inspect, verify


def test_transcode_both_ways(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    std = tmp_path / "std.parquet"
    pq.write_table(numbers, std, row_group_size=1000)
    enc = tmp_path / "enc.parquet"
    assert ppc.transcode(std, enc, policy="pcodec") == {
        "i32": "PCODEC",
        "i64": "PCODEC",
        "f32": "PCODEC",
        "f64": "PCODEC",
        "s": "ZSTD",
    }
    assert ppc.ParquetFile(enc).metadata.row_group_rows == [1000, 1000]
    assert ppc.verify(std, enc) is None
    dec = tmp_path / "dec.parquet"
    assert set(ppc.transcode(enc, dec, target="standard", base="snappy").values()) == {"SNAPPY", "ZSTD"}
    assert pq.read_table(dec).equals(numbers)
    assert ppc.verify(dec, std) is None


def test_transcode_options(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    std = tmp_path / "std.parquet"
    pq.write_table(numbers, std)
    out = tmp_path / "o.parquet"
    choice = ppc.transcode(std, out, policy="base", columns={"f64": "pcodec"}, level=3, base="snappy")
    assert choice["f64"] == "PCODEC"
    assert choice["i64"] == "SNAPPY"
    with pytest.raises(ppc.PcodecError, match="target"):
        ppc.transcode(std, out, target="nope")  # type: ignore[arg-type]
    with pytest.raises(ppc.PcodecError, match="base codec cannot be PCODEC"):
        ppc.transcode(std, out, target="standard", base="pcodec(1)")
    buf = io.BytesIO()
    ppc.transcode(io.BytesIO(std.read_bytes()), buf)
    assert ppc.verify(std, io.BytesIO(buf.getvalue())) is None


def test_transcode_empty_file_reports_nothing(tmp_path: pathlib.Path) -> None:
    out = tmp_path / "o.parquet"
    assert ppc.transcode(vector("empty"), out) == {}


def test_inspect_matches_manifest() -> None:
    report = ppc.inspect(vector("primitives_required"))
    assert report["rows"] == 3000
    assert report["row_groups"] == 1
    assert report["pcodec_version"] == "1"
    f64 = report["columns"][3]
    assert f64["path"] == "f64"
    assert f64["modes"] == {"DECIMAL": 1}
    assert f64["pages"] == {"DATA_PAGE_V2": 1}
    assert f64["compressed_bytes"] > 0


def test_verify_reports_differences(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    a = tmp_path / "a.parquet"
    b = tmp_path / "b.parquet"
    c = tmp_path / "c.parquet"
    ppc.write_table(numbers, a)
    ppc.write_table(numbers.slice(0, 10), b)
    ppc.write_table(numbers.select(["s"]), c)
    assert ppc.verify(a, a) is None
    diff = ppc.verify(a, b)
    assert diff is not None
    assert "row counts differ: 2000 vs 10" in diff
    schema_diff = ppc.verify(a, c)
    assert schema_diff is not None
    assert "schemas differ" in schema_diff


def test_empty_stream(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    f = ppc.ParquetFile(path)
    assert pa.RecordBatchReader.from_stream(f.empty()).read_all().equals(numbers.slice(0, 0))
    assert pa.schema(f.empty(["s", "i32"])) == numbers.select(["s", "i32"]).schema  # type: ignore[arg-type]
    with pytest.raises(ppc.PcodecError, match="no column named 'x'"):
        f.empty(["x"])


@pytest.fixture(scope="module")
def big_exp() -> pa.Table:
    """70 000 rows on which pcodec levels 8 and 9 differ, more than one default batch."""
    import random

    r = random.Random(1)
    return pa.table({"x": pa.array([int(r.expovariate(1e-3)) for _ in range(70_000)], pa.int64())})


def test_default_level_and_batch_size_behave(big_exp: pa.Table, tmp_path: pathlib.Path) -> None:
    import pandas as pd
    import polars as pl

    import tparquet.duckdb as pdd
    import tparquet.pandas as ppd
    import tparquet.polars as ppl

    def written(f: object, data: object, **kw: object) -> bytes:
        buf = io.BytesIO()
        f(data, buf, policy="pcodec", **kw)  # type: ignore[operator]
        return buf.getvalue()

    level8 = written(ppc.write_table, big_exp, level=8)
    assert level8 != written(ppc.write_table, big_exp, level=9)
    assert written(ppc.write_table, big_exp) == level8
    w = io.BytesIO()
    with ppc.Writer(w, big_exp.schema, policy="pcodec") as writer:
        writer.write(big_exp)
    assert w.getvalue() == level8
    assert written(ppl.write_parquet, pl.from_arrow(big_exp)) == level8
    df = pd.DataFrame({"x": big_exp["x"].to_numpy()})
    assert written(ppd.to_parquet, df) == written(ppd.to_parquet, df, level=8)
    assert written(ppd.to_parquet, df) != written(ppd.to_parquet, df, level=9)
    std = io.BytesIO()
    ppc.write_table(big_exp, std, policy="base")
    t = io.BytesIO()
    ppc.transcode(io.BytesIO(std.getvalue()), t, policy="pcodec")
    t8 = io.BytesIO()
    ppc.transcode(io.BytesIO(std.getvalue()), t8, policy="pcodec", level=8)
    assert t.getvalue() == t8.getvalue()
    # batches of 65536 rows by default
    path = tmp_path / "big.parquet"
    path.write_bytes(level8)
    f = ppc.ParquetFile(path)
    first = [
        ppc.read_table(path).to_batches()[0].num_rows,
        f.read().to_batches()[0].num_rows,
        next(f.iter_batches()).num_rows,
        next(iter(ppc.read_batches(path))).num_rows,
        next(iter(f.stream())).num_rows,
        pa.RecordBatchReader.from_stream(pdd.ArrowSource(path)).read_next_batch().num_rows,
    ]
    assert first == [65536] * 6


def test_public_defaults() -> None:
    import inspect

    import tparquet.duckdb as pdd
    import tparquet.pandas as ppd
    import tparquet.polars as ppl

    def defaults(f: object) -> dict[str, object]:
        return {
            k: v.default
            for k, v in inspect.signature(f).parameters.items()  # type: ignore[arg-type]
            if v.default is not inspect.Parameter.empty
        }

    write = {
        "policy": "auto",
        "columns": None,
        "level": 8,
        "base": "zstd(3)",
        "row_group_size": None,
        "encodings": None,
    }
    assert defaults(ppc.write_table) == write
    assert defaults(ppc.Writer) == write
    assert defaults(ppd.to_parquet) == {"index": None, **write}
    assert defaults(ppl.write_parquet) == write
    assert defaults(ppc.transcode) == {
        "target": "pcodec",
        "policy": "auto",
        "columns": None,
        "level": 8,
        "base": "zstd(3)",
    }
    read = {"columns": None, "row_groups": None, "batch_size": 65536}
    assert defaults(ppc.read_table) == read
    assert defaults(ppc.read_batches) == read
    assert defaults(ppc.ParquetFile.read) == read
    assert defaults(ppc.ParquetFile.iter_batches) == {"batch_size": 65536, "columns": None, "row_groups": None}
    assert defaults(ppc.ParquetFile.stream) == {**read, "limit": None}
    assert defaults(ppl.scan_parquet) == {"batch_size": None}
    assert defaults(ppl.read_parquet) == {"columns": None}
    assert defaults(ppd.read_parquet) == {"columns": None}
    assert defaults(pdd.ArrowSource) == {"columns": None, "batch_size": 65536}
    assert defaults(pdd.register) == {"columns": None}
    assert defaults(pdd.sql_view) == {"columns": None}
    assert defaults(pdd.read_parquet) == {"connection": None, "columns": None}


def test_rust_panics_become_pcodec_errors() -> None:
    from tparquet import _lib

    with pytest.raises(ppc.PcodecError, match=r"^internal error: boom$"):
        _lib._panic("boom")
    with pytest.raises(ppc.PcodecError, match=r"^internal error: a static message$"):
        _lib._panic()
    with pytest.raises(ppc.PcodecError, match=r"^internal error: $"):
        _lib._panic("")
    # in a stream: through __next__, and through the C stream (where unwinding would abort)
    with pytest.raises(ppc.PcodecError, match="internal error: the reader panicked"):
        next(iter(_lib.PanickingStream().stream()))
    reader = pa.RecordBatchReader.from_stream(_lib.PanickingStream().stream())
    with pytest.raises(pa.ArrowException, match="internal error: the reader panicked"):
        reader.read_all()


def test_functions_returning_pyarrow_say_what_to_do(monkeypatch: pytest.MonkeyPatch) -> None:
    import sys

    from tparquet import _core

    monkeypatch.setitem(sys.modules, "pyarrow", None)
    with pytest.raises(ImportError) as e:
        _core._pyarrow()
    assert str(e.value) == (
        "this function returns pyarrow objects: install pyarrow, or use read_batches(), whose result "
        "any Arrow library reads through __arrow_c_stream__"
    )


def test_stream_options_are_passed_on(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path, row_group_size=500)
    f = ppc.ParquetFile(path)
    assert pa.RecordBatchReader.from_stream(f.stream(limit=5)).read_all().equals(numbers.slice(0, 5))
    assert (
        pa.RecordBatchReader.from_stream(f.stream(row_groups=[1], limit=600)).read_all().equals(numbers.slice(500, 500))
    )
    batches = list(f.iter_batches(batch_size=200, row_groups=[3]))
    assert [b.num_rows for b in batches] == [200, 200, 100]
    assert pa.Table.from_batches(batches).equals(numbers.slice(1500))
    stream = ppc.read_batches(path, row_groups=[2], batch_size=300)
    assert [b.num_rows for b in pa.RecordBatchReader.from_stream(stream)] == [300, 200]
    with pytest.raises(ppc.PcodecError, match="limit must not be negative, got -1"):
        f.stream(limit=-1)


def test_version() -> None:
    assert ppc.__version__ == "1.0.0"


def test_text_mode_files_are_refused_with_a_type_error(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    with (
        open(path) as text,
        pytest.raises(
            TypeError,
            match=r"^expected a binary file object, got a text file \(TextIOWrapper\), which holds str, not bytes: open it in binary mode \('rb'\)$",
        ),
    ):
        ppc.read_table(text)  # type: ignore[arg-type]
    with pytest.raises(
        TypeError,
        match=r"^expected a binary file object, got a text file \(StringIO\), which holds str, not bytes: open it in binary mode \('wb'\)$",
    ):
        ppc.write_table(numbers, io.StringIO())  # type: ignore[arg-type]
    with open(tmp_path / "w.parquet", "w") as text, pytest.raises(TypeError, match=r"\('wb'\)$"):
        ppc.Writer(text, numbers.schema)  # type: ignore[arg-type]


def test_paths_that_are_not_utf_8(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    raw = os.fsencode(tmp_path) + b"/caf\xe9.parquet"
    for sink in (raw, os.fsdecode(raw)):
        assert ppc.write_table(numbers, sink)["i64"] == "PCODEC"
        assert ppc.read_table(raw).equals(numbers)
        os.remove(raw)
    with ppc.Writer(raw, numbers.schema) as w:
        w.write(numbers)
    assert ppc.read_table(os.fsdecode(raw)).equals(numbers)
    out = os.fsencode(tmp_path) + b"/out\xff.parquet"
    ppc.transcode(raw, out, target="standard")
    with open(out, "rb") as f:
        assert pq.read_table(f).equals(numbers)
    # an aborted writer removes the file it created
    w = ppc.Writer(os.fsdecode(out), numbers.schema)
    w.abort()
    assert not os.path.exists(out)


def test_a_file_replaced_after_it_was_opened_reads_as_opened(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path, row_group_size=500)
    f = ppc.ParquetFile(path)
    stream = f.stream(columns=["i64"])
    # replaced (a new file renamed over it) and removed: what was opened is still read
    other = tmp_path / "other.parquet"
    ppc.write_table(numbers.slice(0, 10).select(["s"]), other)
    os.replace(other, path)
    assert f.read().equals(numbers)
    assert pa.RecordBatchReader.from_stream(stream).read_all().equals(numbers.select(["i64"]))
    assert ppc.inspect(path)["rows"] == 10
    os.remove(path)
    assert f.read(row_groups=[3]).equals(numbers.slice(1500))
    assert f.metadata.num_rows == 2000


def test_page_errors_name_the_file(tmp_path: pathlib.Path) -> None:
    good = tmp_path / "good.parquet"
    ppc.write_table(pa.table({"a": pa.array(range(100_000), pa.int64())}), good)
    data = bytearray(good.read_bytes())
    for i in range(100, 400):  # in the first page, past its header
        data[i] ^= 0x5A
    bad = tmp_path / "bad.parquet"
    bad.write_bytes(bytes(data))
    with pytest.raises(ppc.PcodecError, match=rf"^{re.escape(str(bad))}: "):
        ppc.read_table(bad)
    with pytest.raises(ppc.PcodecError, match=rf"^{re.escape(str(bad))}: "):
        next(iter(ppc.read_batches(bad)))
    with pytest.raises(pa.ArrowException, match=re.escape(str(bad))):
        pa.RecordBatchReader.from_stream(ppc.read_batches(bad)).read_all()
    # a file read into memory has no name
    with pytest.raises(ppc.PcodecError) as e:
        ppc.read_table(io.BytesIO(bytes(data)))
    assert str(bad) not in str(e.value)


@pytest.mark.parametrize("base", ["zstd(", "zstd(3", "zstd(3x", "gzip(é", "zstd)(", "("])
def test_malformed_base_codecs_are_pcodec_errors(tmp_path: pathlib.Path, numbers: pa.Table, base: str) -> None:
    # the parquet crate's parser panics on some of these (a PanicException is not an Exception)
    message = re.escape(f"base codec {base!r}: expected NAME or NAME(LEVEL)".replace("'", '"'))
    with pytest.raises(ppc.PcodecError, match=f"^{message}$"):
        ppc.write_table(numbers, tmp_path / "x.parquet", base=base)
    with pytest.raises(ppc.PcodecError, match=f"^{message}$"):
        ppc.transcode(vector("standard_control"), tmp_path / "y.parquet", target="standard", base=base)
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize(
    "base",
    [
        "snappy",
        "gzip(1)",
        "gzip(9)",
        "brotli(0)",
        "brotli(11)",
        "lz4",
        "lz4_raw",
        "zstd(1)",
        "zstd(22)",
        "uncompressed",
        "SNAPPY",
        "ZSTD(3)",
    ],
)
def test_every_base_codec_is_written(numbers: pa.Table, base: str) -> None:
    buf = io.BytesIO()
    choice = ppc.write_table(numbers, buf, policy="base", base=base)
    assert set(choice.values()) == {base.split("(")[0].upper()}
    assert ppc.read_table(io.BytesIO(buf.getvalue())).equals(numbers)
    assert pq.read_table(io.BytesIO(buf.getvalue())).equals(numbers)


def test_lzo_is_refused(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    with pytest.raises(ppc.PcodecError, match=r"^the base codec cannot be LZO: no Parquet writer implements it$"):
        ppc.write_table(numbers, tmp_path / "x.parquet", base="lzo")
    assert list(tmp_path.iterdir()) == []


def test_integer_bounds_leave_out_paths_that_name_two_leaves(tmp_path: pathlib.Path) -> None:
    inner = pa.StructArray.from_arrays([pa.array(range(1000, 1010), pa.int64())], ["b"])
    path = tmp_path / "d.parquet"
    ppc.write_table(pa.table({"a.b": pa.array(range(10), pa.int64()), "a": inner, "c": pa.array(range(10))}), path)
    # "a.b" is the top-level column and the leaf b of the struct a: whose bounds would it be?
    assert ppc.ParquetFile(path).integer_bounds() == [{"c": (0, 9)}]
    ppc.write_table(pa.table({"a": inner, "x": pa.array(range(10), pa.int32())}), path, row_group_size=5)
    assert ppc.ParquetFile(path).integer_bounds() == [
        {"a.b": (1000, 1004), "x": (0, 4)},
        {"a.b": (1005, 1009), "x": (5, 9)},
    ]
