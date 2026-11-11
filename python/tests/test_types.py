"""Every pyarrow type through write_table, Writer and read_table, under every policy: the
schema comes back as written and the values bit for bit (canonical JSON), with nulls at every
level. Types Parquet cannot store are refused with a PcodecError and leave no file."""

from __future__ import annotations

import datetime
import decimal
import io
import pathlib
from typing import Any, Literal

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import tparquet as ppc
from tparquet import canonical

POLICIES: list[ppc.Policy] = ["auto", "pcodec", "base"]
TIME_UNITS: list[Literal["s", "ms", "us", "ns"]] = ["s", "ms", "us", "ns"]
N = 40


def nulls(values: list[Any], every: int = 5) -> list[Any]:
    return [None if i % every == 2 else v for i, v in enumerate(values)]


def ints(lo: int, hi: int) -> list[int]:
    """`N` integers from `lo` to `hi`, both included."""
    return [lo + (hi - lo) * i // (N - 1) for i in range(N)]


DAY_MS = 86_400_000

COLUMNS: dict[str, pa.Array[Any]] = {
    # integers of every width, at their limits
    "int8": pa.array(nulls(ints(-(2**7), 2**7 - 1)), pa.int8()),
    "int16": pa.array(nulls(ints(-(2**15), 2**15 - 1)), pa.int16()),
    "int32": pa.array(nulls(ints(-(2**31), 2**31 - 1)), pa.int32()),
    "int64": pa.array(nulls(ints(-(2**63), 2**63 - 1)), pa.int64()),
    "uint8": pa.array(nulls(ints(0, 2**8 - 1)), pa.uint8()),
    "uint16": pa.array(nulls(ints(0, 2**16 - 1)), pa.uint16()),
    "uint32": pa.array(nulls(ints(0, 2**32 - 1)), pa.uint32()),
    "uint64": pa.array(nulls(ints(0, 2**64 - 1)), pa.uint64()),
    # floats, with the values that are easy to lose
    "float16": pa.array(
        np.array(nulls([0.0, -0.0, 1.5, 65504.0, -np.inf, np.nan, 6e-8] * 6)[:N], dtype=object), pa.float16()
    ),
    "float32": pa.array(nulls([0.0, -0.0, 1.5, 3.4e38, -np.inf, np.nan, 1e-45, 0.1] * 5), pa.float32()),
    "float64": pa.array(nulls([0.0, -0.0, 1.5, 1.7e308, np.inf, np.nan, 5e-324, 0.1] * 5), pa.float64()),
    "float64_nan_payload": pa.array([0x7FF0000000000001 + i for i in range(N)], pa.uint64()).view(pa.float64()),
    "decimal32": pa.array(nulls([decimal.Decimal(i * 37 - 700).scaleb(-2) for i in range(N)]), pa.decimal32(9, 2)),
    "decimal64": pa.array(nulls([decimal.Decimal(i * 10**12 - 7).scaleb(-3) for i in range(N)]), pa.decimal64(18, 3)),
    "decimal128": pa.array(nulls([decimal.Decimal(i * 10**30 + 1).scaleb(-5) for i in range(N)]), pa.decimal128(38, 5)),
    "decimal256": pa.array(
        nulls([decimal.Decimal(-i * 10**60 - 1).scaleb(-10) for i in range(N)]), pa.decimal256(76, 10)
    ),
    # time
    "date32": pa.array(nulls(ints(-719_162, 2_932_896)), pa.date32()),
    "date64": pa.array(nulls([d * DAY_MS for d in ints(-719_162, 2_932_896)]), pa.date64()),
    "time32_s": pa.array(nulls(ints(0, 86_399)), pa.time32("s")),
    "time32_ms": pa.array(nulls(ints(0, 86_399_999)), pa.time32("ms")),
    "time64_us": pa.array(nulls(ints(0, 86_399_999_999)), pa.time64("us")),
    "time64_ns": pa.array(nulls(ints(0, 86_399_999_999_999)), pa.time64("ns")),
    **{
        f"timestamp_{unit}_{tz or 'naive'}".replace("/", "_"): pa.array(
            nulls(ints(-(2**62), 2**62)), pa.timestamp(unit, tz)
        )
        for unit in TIME_UNITS
        for tz in [None, "UTC", "Asia/Kolkata", "+05:45"]
    },
    **{f"duration_{unit}": pa.array(nulls(ints(-(2**63), 2**63 - 1)), pa.duration(unit)) for unit in TIME_UNITS},
    # the rest
    "bool": pa.array(nulls([i % 3 == 0 for i in range(N)])),
    "string": pa.array(nulls([f"s{i}é\x00\n" * (i % 4) for i in range(N)]), pa.string()),
    "large_string": pa.array(nulls([str(i) * i for i in range(N)]), pa.large_string()),
    "string_view": pa.array(nulls(["short", "a string longer than twelve bytes"] * (N // 2)), pa.string_view()),
    "binary": pa.array(nulls([bytes([i]) * i for i in range(N)]), pa.binary()),
    "large_binary": pa.array(nulls([b"\xff" * i for i in range(N)]), pa.large_binary()),
    "fixed_binary": pa.array(nulls([bytes([i, 255 - i, 0]) for i in range(N)]), pa.binary(3)),
    "binary_view": pa.array(nulls([b"\x00" * (i % 20) for i in range(N)]), pa.binary_view()),
    "null": pa.nulls(N),
    "dictionary_string": pa.array(nulls([f"k{i % 4}" for i in range(N)])).dictionary_encode(),
    "dictionary_int64": pa.array(nulls([i % 3 * 1000 for i in range(N)]), pa.int64()).dictionary_encode(),
    "dictionary_float64": pa.array(nulls([i % 3 / 7 for i in range(N)])).dictionary_encode(),
    "dictionary_uint8_index": pa.DictionaryArray.from_arrays(
        pa.array(nulls([i % 2 for i in range(N)]), pa.uint8()), pa.array([10, 20], pa.int32())
    ),
    "dictionary_date32": pa.array(nulls([i % 3 for i in range(N)]), pa.date32()).dictionary_encode(),
    # nested, with nulls at every level
    "list_int64": pa.array(nulls([[i, None, -i][: i % 4] for i in range(N)]), pa.list_(pa.int64())),
    "large_list_float32": pa.array(nulls([[i / 2, None][: i % 3] for i in range(N)]), pa.large_list(pa.float32())),
    "fixed_size_list_int32": pa.array(nulls([[i, None] for i in range(N)]), pa.list_(pa.int32(), 2)),
    "list_view_int64": pa.array(nulls([[i] * (i % 3) for i in range(N)]), pa.list_view(pa.int64())),
    "large_list_view_float64": pa.array(nulls([[i / 3] * (i % 3) for i in range(N)]), pa.large_list_view(pa.float64())),
    "list_of_lists": pa.array(nulls([[[i], None, []][: i % 4] for i in range(N)]), pa.list_(pa.list_(pa.int16()))),
    "struct": pa.array(
        nulls([{"a": i, "b": None if i % 3 else i / 4, "c": {"d": f"x{i}"}} for i in range(N)]),
        pa.struct([("a", pa.int32()), ("b", pa.float64()), ("c", pa.struct([("d", pa.string())]))]),
    ),
    "struct_of_lists": pa.array(
        nulls([{"l": [i, i + 1][: i % 3], "t": [i * 1000]} for i in range(N)]),
        pa.struct([("l", pa.list_(pa.int64())), ("t", pa.list_(pa.timestamp("ms", "UTC")))]),
    ),
    "list_of_structs": pa.array(
        nulls([[{"x": i, "y": None}, None][: i % 3] for i in range(N)]),
        pa.list_(pa.struct([("x", pa.int64()), ("y", pa.float64())])),
    ),
    "map": pa.array(
        nulls([[(f"k{j}", None if j == 1 else i * j) for j in range(i % 3)] for i in range(N)]),
        pa.map_(pa.string(), pa.int64()),
    ),
    "map_int_keys": pa.array(
        nulls([[(j, j / 2) for j in range(i % 4)] for i in range(N)]), pa.map_(pa.int64(), pa.float64())
    ),
    "map_of_lists": pa.array(nulls([[("k", [i, None])] for i in range(N)]), pa.map_(pa.string(), pa.list_(pa.int32()))),
    # extension types keep their type
    "uuid": pa.array(nulls([bytes([i]) * 16 for i in range(N)]), pa.uuid()),
    "json": pa.array(nulls([f'{{"a": {i}}}' for i in range(N)]), pa.json_()),
    "bool8": pa.array(nulls([i % 2 for i in range(N)]), pa.int8()).cast(pa.bool8()),
    "fixed_shape_tensor": pa.FixedShapeTensorArray.from_numpy_ndarray(
        np.arange(N * 4, dtype=np.int64).reshape(N, 2, 2)
    ),
}


def table_of(*names: str) -> pa.Table:
    return pa.table({n: COLUMNS[n] for n in names})


def round_trip(table: pa.Table, **kwargs: Any) -> tuple[pa.Table, dict[str, str]]:
    buf = io.BytesIO()
    codecs = ppc.write_table(table, buf, **kwargs)
    return ppc.read_table(io.BytesIO(buf.getvalue())), codecs


def assert_same(back: pa.Table, table: pa.Table) -> None:
    assert back.schema == table.schema
    for name in table.column_names:
        assert back.schema.field(name).type == table.schema.field(name).type, name
    assert canonical.dumps(back) == canonical.dumps(table)


@pytest.mark.parametrize("policy", POLICIES)
@pytest.mark.parametrize("name", list(COLUMNS))
def test_each_type_round_trips(name: str, policy: ppc.Policy) -> None:
    table = table_of(name)
    back, _ = round_trip(table, policy=policy)
    assert_same(back, table)
    # sliced, and in several row groups
    sliced = table.slice(3, N - 7)
    back, _ = round_trip(sliced, policy=policy, row_group_size=6)
    assert_same(back, sliced)


@pytest.mark.parametrize("policy", POLICIES)
def test_every_type_in_one_table(policy: ppc.Policy, tmp_path: pathlib.Path) -> None:
    table = pa.table(COLUMNS)
    path = tmp_path / "all.parquet"
    codecs = ppc.write_table(table, path, policy=policy, row_group_size=16)
    assert_same(ppc.read_table(path), table)
    md = ppc.ParquetFile(path).metadata
    assert md.num_row_groups == 3
    # every leaf is reported, with the codec the file has
    assert codecs == {path: codecs[0] for path, codecs in md.codecs.items()}
    if policy == "base":
        assert "PCODEC" not in codecs.values()
        assert md.pcodec_version is None
        # a standard file: pyarrow reads it (it has no float16 bit-exact view, so compare
        # canonically). Only the compared columns are read: pyarrow 25 (the newest on Python
        # 3.10) cannot read a fixed-size list with null entries, not even from its own files.
        some = ["int64", "string", "struct"]
        assert canonical.dumps(pq.read_table(path, columns=some)) == canonical.dumps(table.select(some))
    else:
        assert md.pcodec_version == "1"


def test_writer_takes_every_type_in_batches(tmp_path: pathlib.Path) -> None:
    table = pa.table(COLUMNS)
    path = tmp_path / "w.parquet"
    with ppc.Writer(path, table.schema, policy="auto", row_group_size=25) as w:
        w.write(table.slice(0, 0))
        for batch in table.to_batches(max_chunksize=7):
            w.write(batch)
        w.write(table.slice(0, 5))
    assert_same(ppc.read_table(path), pa.concat_tables([table, table.slice(0, 5)]))


# Which leaves PCODEC applies to: INT32, INT64, FLOAT and DOUBLE (spec section 3)
ELIGIBLE = [
    "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "float32", "float64",
    "date32", "date64", "time32_s", "time32_ms", "time64_us", "time64_ns", "timestamp_ms_UTC",
    "timestamp_s_naive", "duration_ns", "decimal32", "decimal64", "dictionary_int64", "dictionary_float64",
    "dictionary_uint8_index", "dictionary_date32", "null", "bool8",
]  # fmt: skip
NOT_ELIGIBLE = [
    "float16", "decimal128", "decimal256", "bool", "string", "large_string", "string_view", "binary",
    "large_binary", "fixed_binary", "binary_view", "dictionary_string", "uuid", "json",
]  # fmt: skip


@pytest.mark.parametrize("name", ELIGIBLE)
def test_pcodec_applies_to_numeric_leaves(name: str) -> None:
    assert set(round_trip(table_of(name), policy="pcodec")[1].values()) == {"PCODEC"}


@pytest.mark.parametrize("name", NOT_ELIGIBLE)
def test_other_leaves_keep_the_base_codec(name: str) -> None:
    assert set(round_trip(table_of(name), policy="pcodec", base="snappy")[1].values()) == {"SNAPPY"}


def test_nested_leaves_are_reported_by_dotted_path() -> None:
    _, codecs = round_trip(table_of("struct", "list_of_structs", "map", "fixed_size_list_int32"), policy="pcodec")
    assert codecs == {
        "struct.a": "PCODEC",
        "struct.b": "PCODEC",
        "struct.c.d": "ZSTD",
        "list_of_structs.list.item.x": "PCODEC",
        "list_of_structs.list.item.y": "PCODEC",
        "map.entries.key": "ZSTD",
        "map.entries.value": "PCODEC",
        "fixed_size_list_int32.list.item": "PCODEC",
    }


def test_nulls_at_every_level() -> None:
    t = pa.list_(pa.struct([("m", pa.map_(pa.string(), pa.list_(pa.int64())))]))
    values: list[Any] = [
        None,
        [],
        [None],
        [{"m": None}],
        [{"m": []}],
        [{"m": [("k", None)]}],
        [{"m": [("k", [])]}],
        [{"m": [("k", [None])]}],
        [{"m": [("k", [1, None, 2]), ("j", None)]}, None, {"m": []}],
    ]
    table = pa.table({"deep": pa.array(values * 5, t)})
    for policy in POLICIES:
        back, _ = round_trip(table, policy=policy, row_group_size=4)
        assert_same(back, table)
        assert back.column("deep").to_pylist() == values * 5


def test_required_fields_keep_their_nullability() -> None:
    schema = pa.schema(
        [
            pa.field("a", pa.int64(), nullable=False),
            pa.field("s", pa.struct([pa.field("x", pa.float64(), nullable=False)]), nullable=True),
            pa.field("l", pa.list_(pa.field("item", pa.int32(), nullable=False)), nullable=False),
        ]
    )
    table = pa.table(
        {
            "a": list(range(N)),
            "s": [None if i % 3 else {"x": i / 2} for i in range(N)],
            "l": [[i] * (i % 3) for i in range(N)],
        },
        schema=schema,
    )
    back, _ = round_trip(table, policy="pcodec")
    assert back.schema == schema
    assert back.equals(table)


def test_metadata_of_schema_and_fields_round_trips() -> None:
    fields: list[pa.Field[Any]] = [
        pa.field("a", pa.int64(), metadata={"unit": "kg"}),
        pa.field("b", pa.string(), metadata={"x": "1"}),
    ]
    schema = pa.schema(
        fields,
        metadata={"origin": "test", "empty": ""},
    )
    table = pa.table({"a": [1, 2], "b": ["x", "y"]}, schema=schema)
    back, _ = round_trip(table, policy="pcodec")
    assert back.schema.field("a").metadata == {b"unit": b"kg"}
    assert back.schema.field("b").metadata == {b"x": b"1"}
    assert back.schema.metadata is not None
    assert back.schema.metadata[b"origin"] == b"test"
    assert back.schema.metadata[b"empty"] == b""


class Money(pa.ExtensionType):
    """A user extension type: int64 cents with a currency."""

    def __init__(self, currency: str) -> None:
        self.currency = currency
        super().__init__(pa.int64(), "test.money")

    def __arrow_ext_serialize__(self) -> bytes:
        return self.currency.encode()

    @classmethod
    def __arrow_ext_deserialize__(cls, storage_type: pa.DataType, serialized: bytes) -> Money:
        return Money(serialized.decode())


def test_user_extension_types() -> None:
    money = Money("EUR")
    arr = pa.ExtensionArray.from_storage(money, pa.array(nulls(list(range(N))), pa.int64()))
    table = pa.table({"m": arr})
    pa.register_extension_type(money)  # type: ignore[arg-type]  # pyarrow-stubs want a PyExtensionType
    try:
        back, codecs = round_trip(table, policy="pcodec")
        assert codecs == {"m": "PCODEC"}
        assert back.schema.field("m").type == money
        assert back.column("m").type.currency == "EUR"
        assert_same(back, table)
    finally:
        pa.unregister_extension_type("test.money")
    # unregistered, pyarrow reads the storage with the extension metadata
    back, _ = round_trip(table)
    assert back.schema.field("m").type == pa.int64()
    assert back.schema.field("m").metadata == {
        b"ARROW:extension:name": b"test.money",
        b"ARROW:extension:metadata": b"EUR",
    }


def test_chunked_columns_and_zero_rows() -> None:
    table = pa.concat_tables([table_of("int64", "string", "struct").slice(i * 10, 10) for i in range(4)])
    assert table.column("int64").num_chunks == 4
    back, _ = round_trip(table, policy="pcodec")
    assert_same(back, table)
    empty = table.slice(0, 0)
    for policy in POLICIES:
        back, codecs = round_trip(empty, policy=policy)
        assert back.num_rows == 0
        assert back.schema == empty.schema
        assert codecs["int64"] == ("PCODEC" if policy == "pcodec" else "ZSTD")
    assert round_trip(pa.table({"x": pa.array([], pa.int32())}).select([]))[0].num_columns == 0


def test_a_table_without_columns() -> None:
    table = pa.table({"x": [1, 2, 3]}).select([])
    both: list[Any] = [table, table.to_batches()[0]]
    for data in both:
        back, codecs = round_trip(data)
        assert codecs == {}
        assert back.num_columns == 0
        # as with pyarrow: the rows of a row group are counted by its column chunks
        assert back.num_rows == 0
        buf = io.BytesIO()
        pq.write_table(table, buf)
        assert pq.read_table(buf).num_rows == 0


def test_run_end_encoded_columns_come_back_decoded() -> None:
    # the parquet crate writes the values and reads them back as their value type
    ree = pa.RunEndEncodedArray.from_arrays([10, 25, N], pa.array([1, None, 3], pa.int64()))  # type: ignore[call-overload]
    for policy in POLICIES:
        back, _ = round_trip(pa.table({"r": ree}), policy=policy)
        assert back.schema.field("r").type == pa.int64()
        assert back.column("r").to_pylist() == ree.to_pylist()
        assert canonical.dumps(back) == canonical.dumps(pa.table({"r": ree}))


@pytest.mark.parametrize(
    ("array", "message"),
    [
        (pa.array([(1, 2, 3)], pa.month_day_nano_interval()), "MonthDayNano"),
        (pa.UnionArray.from_sparse(pa.array([0], pa.int8()), [pa.array([1])]), "union"),
        (pa.UnionArray.from_dense(pa.array([0], pa.int8()), pa.array([0], pa.int32()), [pa.array([1])]), "union"),
    ],
    ids=["interval", "sparse_union", "dense_union"],
)
def test_types_parquet_cannot_store(tmp_path: pathlib.Path, array: pa.Array[Any], message: str) -> None:
    path = tmp_path / "x.parquet"
    with pytest.raises(ppc.PcodecError, match=message):
        ppc.write_table(pa.table({"x": array}), path)
    assert not path.exists()


def test_timestamps_keep_unit_and_zone_through_python_values() -> None:
    tz = datetime.timezone(datetime.timedelta(hours=-3, minutes=-30))
    when = datetime.datetime(2025, 3, 30, 1, 59, 59, 999999, tzinfo=tz)
    table = pa.table({"t": pa.array([when, None], pa.timestamp("us", "-03:30"))})
    back, _ = round_trip(table, policy="pcodec")
    assert back.column("t").to_pylist()[0] == when
    assert back.schema.field("t").type.tz == "-03:30"
