"""Round trips of every kind of column, under every policy, with pyarrow data."""

from __future__ import annotations

import datetime
import decimal
import io
import pathlib
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import tparquet as ppc
from tparquet import canonical

POLICIES: list[ppc.Policy] = ["auto", "pcodec", "base"]
N = 300


def with_nulls(values: list[Any], t: pa.DataType) -> pa.Array[Any]:
    return pa.array([None if i % 7 == 3 else v for i, v in enumerate(values)], t)


def columns() -> dict[str, pa.Array[Any]]:
    ints = list(range(-N // 2, N // 2))
    ts = [1_735_689_600_000 + i * 17 for i in range(N)]
    return {
        "i8": with_nulls([i % 100 for i in ints], pa.int8()),
        "i16": with_nulls(ints, pa.int16()),
        "i32": with_nulls(ints, pa.int32()),
        "i64": with_nulls([i * 1_000_003 for i in ints], pa.int64()),
        "u8": with_nulls([i % 200 for i in range(N)], pa.uint8()),
        "u16": with_nulls(list(range(N)), pa.uint16()),
        "u32": with_nulls([i * 1000 for i in range(N)], pa.uint32()),
        "u64": with_nulls([2**64 - 1 - i for i in range(N)], pa.uint64()),
        "f16": pa.array([float(i % 10) for i in range(N)], pa.float16()),
        "f32": with_nulls([i / 8 for i in ints], pa.float32()),
        "f64": with_nulls([i * 0.01 for i in ints], pa.float64()),
        "f64_special": pa.array([float("nan"), -0.0, float("inf"), -float("inf"), 5e-324] * (N // 5), pa.float64()),
        "bool": with_nulls([i % 3 == 0 for i in range(N)], pa.bool_()),
        "str": with_nulls([f"s{i % 11}" for i in range(N)], pa.string()),
        "large_str": with_nulls([f"é{i}" for i in range(N)], pa.large_string()),
        "str_view": with_nulls([f"v{i}" * (i % 5) for i in range(N)], pa.string_view()),
        "bin": with_nulls([bytes([i % 256]) * (i % 4) for i in range(N)], pa.binary()),
        "large_bin": with_nulls([b"x" * (i % 3) for i in range(N)], pa.large_binary()),
        "fixed_bin": with_nulls([i.to_bytes(4, "little") for i in range(N)], pa.binary(4)),
        "date32": with_nulls([18000 + i for i in range(N)], pa.date32()),
        "time32": with_nulls([i * 1000 for i in range(N)], pa.time32("ms")),
        "time64": with_nulls([i * 1_000_000 for i in range(N)], pa.time64("us")),
        "time64ns": with_nulls([i * 1_000_000_007 for i in range(N)], pa.time64("ns")),
        "ts_s": with_nulls([t // 1000 for t in ts], pa.timestamp("s")),
        "ts_ms_tz": with_nulls(ts, pa.timestamp("ms", tz="America/New_York")),
        "ts_us_utc": with_nulls([t * 1000 for t in ts], pa.timestamp("us", tz="UTC")),
        "ts_ns_tz": with_nulls([t * 1_000_000 + 1 for t in ts], pa.timestamp("ns", tz="+05:30")),
        "duration": with_nulls(ints, pa.duration("ms")),
        "dec_i32": with_nulls([decimal.Decimal(i) / 100 for i in ints], pa.decimal128(9, 2)),
        "dec_i64": with_nulls([decimal.Decimal(i * 99_991) / 10_000 for i in ints], pa.decimal128(18, 4)),
        "dec_wide": with_nulls([decimal.Decimal(i) * 10**25 for i in ints], pa.decimal128(38, 0)),
        "dec256": with_nulls([decimal.Decimal(i) * 10**50 for i in ints], pa.decimal256(60, 0)),
        "list_i64": with_nulls([list(range(i % 4)) for i in range(N)], pa.list_(pa.int64())),
        "list_f64": with_nulls([[i / 4, None, -i / 4][: i % 4] for i in range(N)], pa.list_(pa.float64())),
        "large_list": with_nulls([[i] * (i % 3) for i in range(N)], pa.large_list(pa.int32())),
        "fixed_list": with_nulls([[i, -i] for i in range(N)], pa.list_(pa.float32(), 2)),
        "list_list": pa.array([[[i], [], None][: i % 4] for i in range(N)], pa.list_(pa.list_(pa.int16()))),
        "struct": with_nulls(
            [{"a": i, "b": f"b{i}", "c": {"d": i / 2}} for i in range(N)],
            pa.struct([("a", pa.int64()), ("b", pa.string()), ("c", pa.struct([("d", pa.float64())]))]),
        ),
        "list_struct": pa.array(
            [[{"p": i * 0.5, "q": i}] * (i % 3) for i in range(N)],
            pa.list_(pa.struct([("p", pa.float64()), ("q", pa.int32())])),
        ),
        "map": with_nulls(
            [[(f"k{j}", i + j) for j in range(i % 3)] for i in range(N)], pa.map_(pa.string(), pa.int64())
        ),
        "dict": pa.array([f"d{i % 4}" if i % 9 else None for i in range(N)]).dictionary_encode(),
        "null": pa.nulls(N),
    }


def table() -> pa.Table:
    return pa.table(columns())


@pytest.mark.parametrize("policy", POLICIES)
def test_every_column_type_round_trips(tmp_path: pathlib.Path, policy: ppc.Policy) -> None:
    t = table()
    path = tmp_path / "all.parquet"
    ppc.write_table(t, path, policy=policy)
    back = ppc.read_table(path)
    assert back.schema == t.schema
    assert canonical.dumps(back) == canonical.dumps(t)
    # pyarrow reads base files (pyarrow < 26 cannot read arrow-rs's fixed-size lists with nulls,
    # PCODEC or not)
    if policy == "base":
        skip = [] if int(pa.__version__.split(".")[0]) >= 26 else ["fixed_list"]
        names = [n for n in t.column_names if n not in skip]
        assert canonical.dumps(pq.read_table(path, columns=names)) == canonical.dumps(t.select(names))


def test_auto_choices_on_mixed_columns(tmp_path: pathlib.Path) -> None:
    choice = ppc.write_table(table(), tmp_path / "a.parquet")
    # only INT32/INT64/FLOAT/DOUBLE leaves can be PCODEC
    assert choice["str"] == "ZSTD"
    assert choice["bool"] == "ZSTD"
    assert choice["fixed_bin"] == "ZSTD"
    assert choice["dec_wide"] == "ZSTD"
    assert choice["list_i64.list.item"] in {"PCODEC", "ZSTD"}
    assert "struct.c.d" in choice
    pcodec = ppc.write_table(table(), tmp_path / "p.parquet", policy="pcodec")
    assert pcodec["ts_ms_tz"] == "PCODEC"
    assert pcodec["dec_i64"] == "PCODEC"
    assert pcodec["date32"] == "PCODEC"
    assert pcodec["struct.c.d"] == "PCODEC"
    assert pcodec["map.entries.value"] == "PCODEC"
    assert pcodec["f16"] == "ZSTD"


def test_nested_column_policy(tmp_path: pathlib.Path) -> None:
    t = table().select(["struct", "list_f64"])
    choice = ppc.write_table(t, tmp_path / "n.parquet", policy="base", columns={"struct.c.d": "pcodec"})
    assert choice == {
        "struct.a": "ZSTD",
        "struct.b": "ZSTD",
        "struct.c.d": "PCODEC",
        "list_f64.list.item": "ZSTD",
    }
    choice = ppc.write_table(t, tmp_path / "s.parquet", policy="base", columns={"struct": "pcodec"})
    assert choice["struct.a"] == "PCODEC"
    assert choice["struct.b"] == "ZSTD"


def test_timestamps_keep_their_time_zone(tmp_path: pathlib.Path) -> None:
    t = pa.table(
        {
            "ny": pa.array(
                [datetime.datetime(2025, 1, 1, 9, 30, tzinfo=datetime.timezone.utc)],
                pa.timestamp("ms", tz="America/New_York"),
            ),
            "naive": pa.array([datetime.datetime(2025, 1, 1, 9, 30)], pa.timestamp("us")),
        }
    )
    path = tmp_path / "tz.parquet"
    ppc.write_table(t, path, policy="pcodec")
    back = ppc.read_table(path)
    assert back.schema.field("ny").type == pa.timestamp("ms", tz="America/New_York")
    assert back.schema.field("naive").type == pa.timestamp("us")
    assert back.equals(t)


def test_multiple_row_groups_and_batches(tmp_path: pathlib.Path) -> None:
    t = table()
    path = tmp_path / "rg.parquet"
    ppc.write_table(t, path, row_group_size=64)
    md = ppc.ParquetFile(path).metadata
    assert md.row_group_rows == [64, 64, 64, 64, 44]
    assert canonical.dumps(ppc.read_table(path, batch_size=50)) == canonical.dumps(t)
    assert canonical.dumps(ppc.read_table(path, row_groups=[4, 0])) == canonical.dumps(
        pa.concat_tables([t.slice(256), t.slice(0, 64)])
    )


def test_schema_metadata_round_trips(tmp_path: pathlib.Path) -> None:
    t = pa.table({"x": [1, 2]}).replace_schema_metadata({"owner": "desk-7", "b": "\x00"})
    field_md = pa.schema([pa.field("x", pa.int64(), metadata={"unit": "lots"})])
    t = t.cast(field_md).replace_schema_metadata({"owner": "desk-7"})
    path = tmp_path / "m.parquet"
    ppc.write_table(t, path)
    back = ppc.read_table(path)
    assert back.schema.metadata is not None
    assert back.schema.metadata[b"owner"] == b"desk-7"
    assert back.schema.field("x").metadata == {b"unit": b"lots"}


def test_writer_streams_into_a_file_object() -> None:
    t = table()
    buf = io.BytesIO()
    with ppc.Writer(buf, t.schema, row_group_size=100) as w:
        for b in t.to_batches(max_chunksize=37):
            w.write(b)
    back = ppc.read_table(io.BytesIO(buf.getvalue()))
    assert canonical.dumps(back) == canonical.dumps(t)
    assert ppc.ParquetFile(io.BytesIO(buf.getvalue())).metadata.row_group_rows == [100, 100, 100]


def test_writer_from_a_record_batch_reader(tmp_path: pathlib.Path) -> None:
    t = table()
    reader = pa.RecordBatchReader.from_batches(t.schema, t.to_batches(max_chunksize=10))
    path = tmp_path / "r.parquet"
    ppc.write_table(reader, path, policy="pcodec")
    assert canonical.dumps(ppc.read_table(path)) == canonical.dumps(t)


def test_read_into_other_arrow_libraries(tmp_path: pathlib.Path) -> None:
    nanoarrow = pytest.importorskip("nanoarrow")
    t = pa.table({"a": pa.array([1, 2, 3], pa.int64()), "b": ["x", None, "z"]})
    path = tmp_path / "n.parquet"
    ppc.write_table(t, path)
    stream = nanoarrow.ArrayStream(ppc.read_batches(path))
    assert [c.name for c in stream.schema.fields] == ["a", "b"]
    arr = stream.read_all()
    assert len(arr) == 3
    # nanoarrow data written back
    out = tmp_path / "n2.parquet"
    ppc.write_table(nanoarrow.ArrayStream(ppc.read_batches(path)), out)
    assert ppc.read_table(out).equals(t)


def test_null_arrays_with_a_buffer_are_imported(tmp_path: pathlib.Path) -> None:
    # Polars exports Null arrays with one (null) buffer where the C data interface has none
    import nanoarrow as na
    import polars as pl

    df = pl.DataFrame(
        {
            "n": pl.Series([None] * 3, dtype=pl.Null),
            "i": [1, 2, 3],
            "l": pl.Series([[None], [None, None], None], dtype=pl.List(pl.Null)),
            "s": pl.Series([{"a": None}] * 3, dtype=pl.Struct({"a": pl.Null})),
        }
    )
    path = tmp_path / "n.parquet"
    assert ppc.write_table(df, path)["i"] == "PCODEC"
    assert pl.DataFrame(ppc.read_batches(path)).equals(df)
    # one batch through __arrow_c_array__, and through a Writer
    batch = next(iter(na.c_array_stream(df)))
    ppc.write_table(batch, path)
    assert pl.DataFrame(ppc.read_batches(path)).equals(df)
    with ppc.Writer(path, df.to_arrow().schema) as w:
        w.write(df)
        w.write(batch)
    assert pl.DataFrame(ppc.read_batches(path)).equals(pl.concat([df, df]))
