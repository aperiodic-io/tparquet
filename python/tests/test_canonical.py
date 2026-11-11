"""tparquet.canonical: canonical JSON as testdata/README.md defines it."""

from __future__ import annotations

import decimal
import struct
from typing import Any

import pyarrow as pa
import pytest

from tparquet import canonical


def line(arr: pa.Array[Any], name: str = "x") -> list[str]:
    return list(canonical.lines(pa.table({name: arr})))


def test_integers_of_every_width() -> None:
    assert line(pa.array([-12, None, 0], pa.int8())) == ['{"x":-12}', '{"x":null}', '{"x":0}']
    for t in [pa.int16(), pa.int32(), pa.int64(), pa.uint8(), pa.uint16(), pa.uint32()]:
        assert line(pa.array([7], t)) == ['{"x":7}']
    assert line(pa.array([18446744073709551615], pa.uint64())) == ['{"x":18446744073709551615}']
    assert line(pa.array([-(2**63)], pa.int64())) == ['{"x":-9223372036854775808}']


def test_floats_are_their_bits() -> None:
    assert line(pa.array([1.0, -0.0, None], pa.float64())) == [
        '{"x":"0x3ff0000000000000"}',
        '{"x":"0x8000000000000000"}',
        '{"x":null}',
    ]
    assert line(pa.array([1.0, 2.0], pa.float32())) == ['{"x":"0x3f800000"}', '{"x":"0x40000000"}']
    # a signalling NaN with a payload survives (no conversion through a Python float)
    arr = pa.Array.from_buffers(pa.float32(), 1, [None, pa.py_buffer(struct.pack("<I", 0x7F800001))])
    assert line(arr) == ['{"x":"0x7f800001"}']
    nan64 = pa.Array.from_buffers(pa.float64(), 1, [None, pa.py_buffer(struct.pack("<Q", 0xFFF0000000000123))])
    assert line(nan64) == ['{"x":"0xfff0000000000123"}']
    half = pa.Array.from_buffers(pa.float16(), 2, [None, pa.py_buffer(struct.pack("<HH", 0x3C00, 0x0001))])
    assert line(half) == ['{"x":"0x3c00"}', '{"x":"0x0001"}']


def test_temporal_values_are_the_stored_integers() -> None:
    ts = pa.array([1735689600000, None], pa.timestamp("ms", tz="Europe/Paris"))
    assert line(ts) == ['{"x":1735689600000}', '{"x":null}']
    assert line(pa.array([5], pa.timestamp("ns"))) == ['{"x":5}']
    assert line(pa.array([-3], pa.date32())) == ['{"x":-3}']
    assert line(pa.array([86_400_000], pa.date64())) == ['{"x":86400000}']
    assert line(pa.array([12], pa.time32("s"))) == ['{"x":12}']
    assert line(pa.array([13], pa.time32("ms"))) == ['{"x":13}']
    assert line(pa.array([14], pa.time64("us"))) == ['{"x":14}']
    assert line(pa.array([15], pa.time64("ns"))) == ['{"x":15}']
    assert line(pa.array([-16], pa.duration("us"))) == ['{"x":-16}']


@pytest.mark.parametrize(
    "t", [pa.decimal32(7, 2), pa.decimal64(15, 2), pa.decimal128(30, 2), pa.decimal256(70, 2)], ids=str
)
def test_decimals_are_unscaled(t: pa.DataType) -> None:
    arr = pa.array([decimal.Decimal("-1.25"), None, decimal.Decimal("3.00")], t)
    assert line(arr) == ['{"x":-125}', '{"x":null}', '{"x":300}']
    # a slice starts at its own offset
    assert line(arr.slice(2)) == ['{"x":300}']


def test_big_decimal256() -> None:
    v = decimal.Decimal("-" + "9" * 70)
    assert line(pa.array([v], pa.decimal256(76, 0))) == ['{"x":-' + "9" * 70 + "}"]


def test_booleans_and_nulls() -> None:
    assert line(pa.array([True, False, None])) == ['{"x":true}', '{"x":false}', '{"x":null}']
    assert line(pa.array([None, None], pa.null())) == ['{"x":null}', '{"x":null}']


def test_string_escapes() -> None:
    s = '"\\\b\t\n\f\r\x00\x1f\x7f/é😀'
    want = '{"x":"\\"\\\\\\b\\t\\n\\f\\r\\u0000\\u001f\x7f/é😀"}'
    for t in [pa.string(), pa.large_string(), pa.string_view()]:
        assert line(pa.array([s], t)) == [want]
    assert line(pa.array(["", None])) == ['{"x":""}', '{"x":null}']
    # keys are escaped the same way
    assert line(pa.array([1]), name='a"b') == ['{"a\\"b":1}']


def test_binary_is_base64() -> None:
    for t in [pa.binary(), pa.large_binary(), pa.binary_view()]:
        assert line(pa.array([b"\x00\xffab", b""], t)) == ['{"x":"AP9hYg=="}', '{"x":""}']
    assert line(pa.array([b"abc"], pa.binary(3))) == ['{"x":"YWJj"}']


def test_lists() -> None:
    arr = pa.array([[1, None, 3], None, []], pa.list_(pa.int32()))
    assert line(arr) == ['{"x":[1,null,3]}', '{"x":null}', '{"x":[]}']
    assert line(arr.slice(1)) == ['{"x":null}', '{"x":[]}']
    large = pa.array([[1.0]], pa.large_list(pa.float64()))
    assert line(large) == ['{"x":["0x3ff0000000000000"]}']
    fixed = pa.array([[1, 2], None, [5, 6]], pa.list_(pa.int64(), 2))
    assert line(fixed) == ['{"x":[1,2]}', '{"x":null}', '{"x":[5,6]}']
    assert line(fixed.slice(2)) == ['{"x":[5,6]}']
    nested = pa.array([[[1], [2, 3]]], pa.list_(pa.list_(pa.int8())))
    assert line(nested) == ['{"x":[[1],[2,3]]}']


def test_structs_in_schema_order() -> None:
    t = pa.struct([("b", pa.int32()), ("a", pa.string())])
    arr = pa.array([{"b": 1, "a": "x"}, None, {"b": None, "a": "y"}], t)
    assert line(arr) == ['{"x":{"b":1,"a":"x"}}', '{"x":null}', '{"x":{"b":null,"a":"y"}}']
    assert line(arr.slice(2)) == ['{"x":{"b":null,"a":"y"}}']


def test_maps_are_entry_arrays() -> None:
    arr = pa.array([[("k", 1), ("j", None)], None, []], pa.map_(pa.string(), pa.int64()))
    assert line(arr) == ['{"x":[{"key":"k","value":1},{"key":"j","value":null}]}', '{"x":null}', '{"x":[]}']
    assert line(arr.slice(1, 1)) == ['{"x":null}']


def test_dictionaries_are_decoded() -> None:
    arr = pa.array(["a", None, "b", "a"]).dictionary_encode()
    assert line(arr) == ['{"x":"a"}', '{"x":null}', '{"x":"b"}', '{"x":"a"}']


def test_extension_types_use_their_storage() -> None:
    arr = pa.array([b"\x00" * 16], pa.uuid())
    assert line(arr) == ['{"x":"AAAAAAAAAAAAAAAAAAAAAA=="}']


def test_rows_tables_batches_and_streams() -> None:
    table = pa.table({"a": [1, 2], "b": ["x", "y"]})
    want = ['{"a":1,"b":"x"}', '{"a":2,"b":"y"}']
    assert list(canonical.lines(table)) == want
    assert list(canonical.lines(table.to_batches()[0])) == want
    assert canonical.dumps(table) == '{"a":1,"b":"x"}\n{"a":2,"b":"y"}\n'
    two_chunks = pa.concat_tables([table.slice(0, 1), table.slice(1)])
    assert canonical.dumps(two_chunks) == canonical.dumps(table)
    assert canonical.dumps(pa.RecordBatchReader.from_batches(table.schema, table.to_batches())) == canonical.dumps(
        table
    )
    assert canonical.dumps(table.slice(0, 0)) == ""


def test_c_array_objects() -> None:
    batch = pa.record_batch({"a": [1, 2]})

    class OnlyArray:
        def __arrow_c_array__(self, requested_schema: object | None = None) -> object:
            return batch.__arrow_c_array__(requested_schema)  # type: ignore[no-untyped-call]

    assert list(canonical.lines(OnlyArray())) == ['{"a":1}', '{"a":2}']


@pytest.mark.parametrize("t", [pa.list_view(pa.int32()), pa.large_list_view(pa.int32())], ids=str)
def test_list_views_are_lists(t: pa.DataType) -> None:
    # testdata/README.md: lists of any kind are JSON arrays
    arr = pa.array([[1, None, 3], None, [], [4]], t)
    assert line(arr) == ['{"x":[1,null,3]}', '{"x":null}', '{"x":[]}', '{"x":[4]}']
    assert line(arr.slice(2)) == ['{"x":[]}', '{"x":[4]}']
    # views may overlap and go in any order
    shared = pa.ListViewArray.from_arrays(  # type: ignore[call-overload]
        pa.array([2, 0, 1], pa.int32()), pa.array([1, 3, 2], pa.int32()), pa.array([7, 8, 9])
    )
    assert line(shared) == ['{"x":[9]}', '{"x":[7,8,9]}', '{"x":[8,9]}']


def test_run_end_encoded_values_are_decoded() -> None:
    arr = pa.RunEndEncodedArray.from_arrays([2, 3, 5], pa.array([1, None, 2], pa.int64()))  # type: ignore[call-overload]
    assert line(arr) == ['{"x":1}', '{"x":1}', '{"x":null}', '{"x":2}', '{"x":2}']
    assert line(arr.slice(1, 3)) == ['{"x":1}', '{"x":null}', '{"x":2}']


def test_unsupported_types() -> None:
    union = pa.UnionArray.from_sparse(pa.array([0], pa.int8()), [pa.array([1])])
    with pytest.raises(TypeError, match=r"^canonical JSON does not cover sparse_union"):
        line(union)
    with pytest.raises(TypeError, match=r"^expected Arrow data \(a table, a record batch or a stream\), got int$"):
        list(canonical.lines(42))


def test_intervals_are_months_days_and_nanoseconds() -> None:
    mdn = pa.array([(1, -2, 3), None, (2**31 - 1, -(2**31), -(2**63))], pa.month_day_nano_interval())
    assert line(mdn) == [
        '{"x":{"months":1,"days":-2,"nanoseconds":3}}',
        '{"x":null}',
        '{"x":{"months":2147483647,"days":-2147483648,"nanoseconds":-9223372036854775808}}',
    ]
    assert line(mdn.slice(1)) == line(mdn)[1:]
