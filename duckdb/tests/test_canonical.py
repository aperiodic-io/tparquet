"""The canonical JSON converter the conformance tests rely on."""

import pyarrow as pa
import pytest
from canonical import lines, values


def test_floats_keep_their_bits():
    nan = pa.array([1.5, -0.0, None]).cast(pa.float64())
    assert values(nan, pa.float64()) == ['"0x3ff8000000000000"', '"0x8000000000000000"', "null"]
    payload = pa.array([0x7FF8DEADBEEF0001], pa.uint64()).view(pa.float64())
    assert values(payload, pa.float64()) == ['"0x7ff8deadbeef0001"']
    assert values(pa.array([1.0], pa.float32()), pa.float32()) == ['"0x3f800000"']
    with pytest.raises(AssertionError):
        values(pa.array([1.0], pa.float32()), pa.float64())


def test_timestamps_go_back_to_the_file_unit():
    us = pa.array([1_735_689_600_033_000, None], pa.timestamp("us", tz="UTC"))
    assert values(us, pa.timestamp("ms", tz="UTC")) == ["1735689600033", "null"]
    with pytest.raises(AssertionError):
        values(pa.array([1_001], pa.timestamp("us")), pa.timestamp("ms"))
    assert values(pa.array([5], pa.time64("us")), pa.time64("us")) == ["5"]
    assert values(pa.array([20089], pa.date32()), pa.date32()) == ["20089"]


def test_strings_decimals_binary_and_nesting():
    s = pa.array(['a"\\\n\x01ü/', None])
    assert values(s, pa.string()) == ['"a\\"\\\\\\n\\u0001ü/"', "null"]
    d = pa.array([pa.scalar(-12345, pa.int64())]).cast(pa.decimal128(38, 0))
    assert values(pa.array([None], pa.decimal128(9, 2)), pa.decimal128(9, 2)) == ["null"]
    import decimal

    big = pa.array([decimal.Decimal("-1234567890123456789012345678.9012345678")], pa.decimal128(38, 10))
    assert values(big, pa.decimal128(38, 10)) == ["-12345678901234567890123456789012345678"]
    assert values(d, pa.decimal128(38, 0)) == ["-12345"]
    assert values(pa.array([b"\x00\xff"]), pa.binary()) == ['"AP8="']
    lst = pa.array([[1, None], None, []], pa.list_(pa.int64()))
    assert values(lst, pa.list_(pa.int64())) == ["[1,null]", "null", "[]"]
    st = pa.array([{"x": 1, "y": True}, None], pa.struct([("x", pa.int32()), ("y", pa.bool_())]))
    assert values(st, st.type) == ['{"x":1,"y":true}', "null"]
    m = pa.array([[("k", 1)]], pa.map_(pa.string(), pa.int64()))
    assert values(m, m.type) == ['[{"key":"k","value":1}]']
    d = pa.array(["u", None, "u"]).dictionary_encode()
    assert values(d, d.type) == ['"u"', "null", '"u"']
    t = pa.table({"a": [1, 2], "b": ["x", None]})
    assert lines(t, t.schema) == '{"a":1,"b":"x"}\n{"a":2,"b":null}\n'


def test_intervals_are_months_days_and_nanoseconds():
    # DuckDB hands its INTERVAL (months, days, microseconds) to Arrow as month-day-nano
    mdn = pa.array([(1, -2, 3_000), None, (2**31 - 1, -(2**31), -(2**63))], pa.month_day_nano_interval())
    assert values(mdn, pa.month_day_nano_interval()) == [
        '{"months":1,"days":-2,"nanoseconds":3000}',
        "null",
        '{"months":2147483647,"days":-2147483648,"nanoseconds":-9223372036854775808}',
    ]
