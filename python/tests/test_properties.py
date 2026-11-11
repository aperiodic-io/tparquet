"""Property tests: random pyarrow tables round-trip bit for bit under every policy."""

from __future__ import annotations

import datetime
import decimal
import io
import itertools
from typing import Any

import pyarrow as pa
import pyarrow.compute as pc
from hypothesis import given
from hypothesis import strategies as st

import tparquet as ppc
from tparquet import canonical


def ints(bits: int, signed: bool = True) -> st.SearchStrategy[int]:
    if signed:
        return st.integers(-(2 ** (bits - 1)), 2 ** (bits - 1) - 1)
    return st.integers(0, 2**bits - 1)


@st.composite
def numbers(draw: st.DrawFn, n: int, bits: int, signed: bool = True) -> list[int]:
    """Integers: arbitrary, or smooth (a random walk), or a few repeated values."""
    kind = draw(st.sampled_from(["any", "walk", "few"]))
    lo, hi = (-(2 ** (bits - 1)), 2 ** (bits - 1) - 1) if signed else (0, 2**bits - 1)
    if kind == "any":
        return draw(st.lists(ints(bits, signed), min_size=n, max_size=n))
    if kind == "few":
        pool = draw(st.lists(ints(bits, signed), min_size=1, max_size=4))
        return [pool[i % len(pool)] for i in draw(st.lists(st.integers(0, 3), min_size=n, max_size=n))]
    start = draw(ints(bits, signed))
    steps = draw(st.lists(st.integers(-5, 5), min_size=n, max_size=n))
    out, v = [], start
    for s in steps:
        v = min(hi, max(lo, v + s))
        out.append(v)
    return out


@st.composite
def float_bits(draw: st.DrawFn, n: int, bits: int) -> pa.Array[Any]:
    """Floats from arbitrary bit patterns (NaN payloads, -0.0, subnormals) or decimal prices."""
    if draw(st.booleans()):
        raw = draw(st.lists(ints(bits, signed=False), min_size=n, max_size=n))
        unsigned = pa.uint64() if bits == 64 else pa.uint32()
        return pa.array(raw, unsigned).view(pa.float64() if bits == 64 else pa.float32())
    cents = draw(numbers(n, 32))
    return pa.array([c / 100 for c in cents], pa.float64() if bits == 64 else pa.float32())


def nulled(draw: st.DrawFn, arr: pa.Array[Any]) -> pa.Array[Any]:
    """`arr`, or `arr` with random elements made null."""
    if len(arr) == 0 or not draw(st.booleans()):
        return arr
    mask = draw(st.lists(st.booleans(), min_size=len(arr), max_size=len(arr)))
    out: pa.Array[Any] = pc.if_else(pa.array(mask), pa.nulls(len(arr), arr.type), arr)
    return out


COLUMN_KINDS = [
    "i8",
    "i16",
    "i32",
    "i64",
    "u32",
    "u64",
    "f32",
    "f64",
    "bool",
    "str",
    "ts_ms_tz",
    "date32",
    "dec64",
    "list_f64",
    "list_i32",
    "struct",
]


@st.composite
def column(draw: st.DrawFn, kind: str, n: int) -> pa.Array[Any]:
    simple = {
        "i8": (8, True, pa.int8()),
        "i16": (16, True, pa.int16()),
        "i32": (32, True, pa.int32()),
        "i64": (64, True, pa.int64()),
        "u32": (32, False, pa.uint32()),
        "u64": (64, False, pa.uint64()),
    }
    if kind in simple:
        bits, signed, t = simple[kind]
        return nulled(draw, pa.array(draw(numbers(n, bits, signed)), t))
    if kind in ("f32", "f64"):
        return nulled(draw, draw(float_bits(n, 64 if kind == "f64" else 32)))
    if kind == "bool":
        return nulled(draw, pa.array(draw(st.lists(st.booleans(), min_size=n, max_size=n))))
    if kind == "str":
        return nulled(draw, pa.array(draw(st.lists(st.text(max_size=5), min_size=n, max_size=n)), pa.string()))
    if kind == "ts_ms_tz":
        v = [1_735_689_600_000 + x for x in draw(numbers(n, 32))]
        return nulled(draw, pa.array(v, pa.timestamp("ms", tz="Asia/Tokyo")))
    if kind == "date32":
        return nulled(draw, pa.array([abs(x) % 100_000 for x in draw(numbers(n, 32))], pa.date32()))
    if kind == "dec64":
        decimals = [decimal.Decimal(x // 1000).scaleb(-4) for x in draw(numbers(n, 64))]
        return nulled(draw, pa.array(decimals, pa.decimal128(18, 4)))
    if kind in ("list_f64", "list_i32"):
        lengths = draw(st.lists(st.integers(0, 4), min_size=n, max_size=n))
        total = sum(lengths)
        values: pa.Array[Any] = (
            draw(float_bits(total, 64)) if kind == "list_f64" else pa.array(draw(numbers(total, 32)), pa.int32())
        )
        offsets = [0]
        for length in lengths:
            offsets.append(offsets[-1] + length)
        return nulled(draw, pa.ListArray.from_arrays(pa.array(offsets, pa.int32()), values))
    a = pa.array(draw(numbers(n, 64)), pa.int64())
    b = draw(float_bits(n, 64))
    return nulled(draw, pa.StructArray.from_arrays([a, b], ["a", "b"]))


@st.composite
def tables(draw: st.DrawFn) -> pa.Table:
    n = draw(st.integers(0, 120))
    kinds = draw(st.lists(st.sampled_from(COLUMN_KINDS), min_size=1, max_size=4))
    return pa.table({f"{k}_{i}": draw(column(k, n)) for i, k in enumerate(kinds)})


@given(table=tables(), policy=st.sampled_from(["auto", "pcodec", "base"]), rows=st.integers(1, 64))
def test_tables_round_trip_bit_for_bit(table: pa.Table, policy: ppc.Policy, rows: int) -> None:
    buf = io.BytesIO()
    ppc.write_table(table, buf, policy=policy, row_group_size=rows)
    back = ppc.read_table(io.BytesIO(buf.getvalue()))
    assert back.schema == table.schema
    assert canonical.dumps(back) == canonical.dumps(table)


@given(table=tables(), level=st.integers(0, 12), chunk=st.integers(1, 50))
def test_streamed_batches_round_trip(table: pa.Table, level: int, chunk: int) -> None:
    buf = io.BytesIO()
    with ppc.Writer(buf, table.schema, policy="pcodec", level=level) as w:
        for b in table.to_batches(max_chunksize=chunk):
            w.write(b)
    back = ppc.read_table(io.BytesIO(buf.getvalue()), batch_size=chunk)
    assert canonical.dumps(back) == canonical.dumps(table)


@given(data=st.binary(max_size=2000), hint=st.sampled_from([None, "INT32", "INT64", "FLOAT", "DOUBLE"]))
def test_payloads_round_trip(data: bytes, hint: Any) -> None:
    from tparquet import codec

    assert codec.decompress(codec.compress(data, hint=hint), len(data)) == data


@given(payload=st.binary(max_size=300), expected=st.integers(0, 5000))
def test_garbage_payloads_raise_cleanly(payload: bytes, expected: int) -> None:
    from tparquet import codec

    try:
        out = codec.decompress(payload, expected)
    except ppc.PcodecError:
        return
    assert len(out) == expected


# ---------------------------------------------------------------- generated schemas


LEAVES: list[pa.DataType] = [
    pa.int8(), pa.int16(), pa.int32(), pa.int64(), pa.uint8(), pa.uint16(), pa.uint32(), pa.uint64(),
    pa.float32(), pa.float64(), pa.bool_(), pa.string(), pa.large_string(), pa.string_view(), pa.binary(),
    pa.large_binary(), pa.binary_view(), pa.binary(3), pa.decimal32(7, 2), pa.decimal64(16, 3),
    pa.decimal128(30, 4), pa.decimal256(60, 10), pa.date32(), pa.date64(), pa.time32("s"), pa.time32("ms"),
    pa.time64("us"), pa.time64("ns"), pa.timestamp("s"), pa.timestamp("ms", "UTC"), pa.timestamp("us", "Asia/Tokyo"),
    pa.timestamp("ns", "-07:00"), pa.duration("s"), pa.duration("ns"), pa.null(),
]  # fmt: skip
KEYS: list[pa.DataType] = [pa.int32(), pa.int64(), pa.string(), pa.date32()]


def types(depth: int = 2) -> st.SearchStrategy[pa.DataType]:
    """Arrow types: the leaves above, and lists of every kind, structs and maps of them."""
    leaf = st.sampled_from(LEAVES)
    if depth == 0:
        return leaf
    inner = types(depth - 1)
    return st.one_of(
        leaf,
        leaf,
        inner.map(pa.list_),
        inner.map(pa.large_list),
        inner.map(pa.list_view),
        st.tuples(inner, st.integers(1, 3)).map(lambda t: pa.list_(t[0], t[1])),
        st.lists(st.tuples(st.sampled_from("abc"), inner), min_size=1, max_size=3, unique_by=lambda f: f[0]).map(
            lambda fields: pa.struct([pa.field(n, t, nullable=n != "c" or pa.types.is_null(t)) for n, t in fields])
        ),
        st.tuples(st.sampled_from(KEYS), inner).map(lambda kv: pa.map_(kv[0], kv[1])),
    )


DAY = datetime.timedelta(days=1)


def values_of(t: pa.DataType, nullable: bool = True) -> st.SearchStrategy[Any]:
    """Python values of type `t` (None among them when `nullable`)."""
    v: st.SearchStrategy[Any]
    if pa.types.is_null(t):
        return st.none()
    if pa.types.is_integer(t):
        bits = t.bit_width
        signed = pa.types.is_signed_integer(t)
        v = st.one_of(ints(bits, signed), st.integers(0, 9))
    elif pa.types.is_floating(t):
        width: Any = t.bit_width
        v = st.one_of(st.floats(width=width), st.sampled_from([0.0, -0.0, 0.5]))
    elif pa.types.is_boolean(t):
        v = st.booleans()
    elif pa.types.is_string(t) or pa.types.is_large_string(t) or pa.types.is_string_view(t):
        v = st.text(max_size=20)
    elif pa.types.is_fixed_size_binary(t):
        v = st.binary(min_size=t.byte_width, max_size=t.byte_width)
    elif pa.types.is_binary(t) or pa.types.is_large_binary(t) or pa.types.is_binary_view(t):
        v = st.binary(max_size=20)
    elif pa.types.is_decimal(t):
        v = st.integers(-(10**t.precision) + 1, 10**t.precision - 1).map(
            lambda i: decimal.Decimal(i).scaleb(-t.scale, decimal.Context(prec=80))
        )
    elif pa.types.is_date32(t):
        v = st.dates()
    elif pa.types.is_date64(t):
        v = st.dates(min_value=datetime.date(1, 1, 1))
    elif pa.types.is_time32(t) or pa.types.is_time64(t):
        v = st.times()
        if t.unit in ("s", "ms"):
            v = v.map(lambda x: x.replace(microsecond=x.microsecond // 1000 * 1000 if t.unit == "ms" else 0))
    elif pa.types.is_timestamp(t):
        # seconds Python's datetime holds (years 1 to 9999), in the type's unit, a day inside
        # them: as_py gives the time in the type's zone, which is up to a day off UTC
        per_second = {"s": 1, "ms": 10**3, "us": 10**6, "ns": 10**9}[t.unit]
        # (and two days inside the 64-bit range, which zone conversions of the values must not leave)
        edge = (2**63 - 1) // per_second - 2 * 86_400
        lo, hi = max(-edge, -62_135_596_800 + 86_400), min(edge, 253_402_300_799 - 86_400)
        v = st.integers(lo, hi).map(lambda x: pa.scalar(x * per_second, pa.timestamp(t.unit)).cast(t))
    elif pa.types.is_duration(t):
        # what Python's timedelta holds (999999999 days), in any unit
        v = st.integers(-(2**46), 2**46).map(lambda x: pa.scalar(x, t))
    elif pa.types.is_fixed_size_list(t):
        v = st.lists(values_of(t.value_type), min_size=t.list_size, max_size=t.list_size)
    elif pa.types.is_list(t) or pa.types.is_large_list(t) or pa.types.is_list_view(t):
        v = st.lists(values_of(t.value_type), max_size=4)
    elif pa.types.is_map(t):
        v = st.lists(st.tuples(values_of(t.key_type, nullable=False), values_of(t.item_type)), max_size=3)
    else:
        assert pa.types.is_struct(t), t
        fields = [t.field(i) for i in range(t.num_fields)]
        v = st.fixed_dictionaries({f.name: values_of(f.type, f.nullable) for f in fields})
    return st.one_of(st.none(), v) if nullable else v


@st.composite
def nested_tables(draw: st.DrawFn) -> pa.Table:
    n = draw(st.integers(0, 40))
    columns = {}
    for i, t in enumerate(draw(st.lists(types(), min_size=1, max_size=3))):
        values = draw(st.lists(values_of(t), min_size=n, max_size=n))
        arr = pa.array([v.as_py() if isinstance(v, pa.Scalar) else v for v in values], t)
        # (not of booleans: the parquet crate cannot read a dictionary of booleans back, an
        # upstream limitation: "entered unreachable code: Invalid physical type")
        dictionary = pa.types.is_primitive(t) and not pa.types.is_null(t) and not pa.types.is_boolean(t)
        if draw(st.integers(0, 4)) == 0 and dictionary:
            arr = arr.dictionary_encode()
        columns[f"c{i}"] = arr
    return pa.table(columns)


OPTIONS = st.fixed_dictionaries(
    {
        "policy": st.sampled_from(["auto", "pcodec", "base"]),
        "level": st.integers(0, 12),
        "base": st.sampled_from(["zstd(1)", "snappy", "uncompressed", "lz4_raw", "gzip(1)", "brotli(1)"]),
        "row_group_size": st.one_of(st.none(), st.integers(1, 50)),
    }
)


@given(table=nested_tables(), options=OPTIONS)
def test_generated_schemas_round_trip(table: pa.Table, options: dict[str, Any]) -> None:
    buf = io.BytesIO()
    codecs = ppc.write_table(table, buf, **options)
    f = ppc.ParquetFile(io.BytesIO(buf.getvalue()))
    back = f.read()
    assert back.schema == table.schema
    assert canonical.dumps(back) == canonical.dumps(table)
    if f.num_row_groups:
        assert codecs == {path: c[0] for path, c in f.metadata.codecs.items()}
    if options["policy"] == "base":
        assert "PCODEC" not in codecs.values()


@given(table=nested_tables(), options=OPTIONS, cuts=st.lists(st.integers(0, 40), max_size=5), batch=st.integers(1, 50))
def test_generated_schemas_through_a_writer(
    table: pa.Table, options: dict[str, Any], cuts: list[int], batch: int
) -> None:
    buf = io.BytesIO()
    bounds = sorted({0, table.num_rows, *(c for c in cuts if c <= table.num_rows)})
    with ppc.Writer(buf, table.schema, **options) as w:
        for start, end in itertools.pairwise(bounds):
            w.write(table.slice(start, end - start))
            w.write(table.slice(start, 0))
        if cuts and cuts[0] % 2:
            w.flush()
    back = ppc.read_table(io.BytesIO(buf.getvalue()), batch_size=batch)
    assert back.schema == table.schema
    assert canonical.dumps(back) == canonical.dumps(table)


@given(table=nested_tables(), policy=st.sampled_from(["auto", "pcodec", "base"]), rows=st.integers(1, 30))
def test_transcode_round_trips_and_is_idempotent(table: pa.Table, policy: ppc.Policy, rows: int) -> None:
    standard = io.BytesIO()
    ppc.write_table(table, standard, policy="base", row_group_size=rows)
    encoded = io.BytesIO()
    ppc.transcode(io.BytesIO(standard.getvalue()), encoded, policy=policy)
    again = io.BytesIO()
    ppc.transcode(io.BytesIO(encoded.getvalue()), again, policy=policy)
    decoded = io.BytesIO()
    ppc.transcode(io.BytesIO(encoded.getvalue()), decoded, target="standard")
    for out in [encoded, again, decoded]:
        assert ppc.verify(io.BytesIO(standard.getvalue()), io.BytesIO(out.getvalue())) is None
        back = ppc.read_table(io.BytesIO(out.getvalue()))
        assert back.schema == table.schema
        assert canonical.dumps(back) == canonical.dumps(table)
    # transcoding a transcoded file changes nothing
    assert again.getvalue() == encoded.getvalue()
    md = ppc.ParquetFile(io.BytesIO(decoded.getvalue())).metadata
    assert md.pcodec_version is None
    assert md.row_group_rows == ppc.ParquetFile(io.BytesIO(standard.getvalue())).metadata.row_group_rows
