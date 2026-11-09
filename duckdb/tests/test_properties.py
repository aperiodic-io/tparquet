"""Property tests over generated tables (Hypothesis): every type pyarrow writes, nested, with
NULLs at every level, in random row groups and page versions.

For each table, written as a standard Parquet file by pyarrow:

- read_tparquet reads it as read_parquet does (bit for bit, in read_parquet's type where the
  two differ);
- COPY (FORMAT tparquet), with random options, writes what read_tparquet reads back unchanged;
- the Rust CLI's PCODEC transcode of it reads the same.
"""

import datetime
import decimal

import pyarrow as pa
import pyarrow.parquet as pq
from canonical import rows_of
from conftest import DUCKDB_1, connect, copy_to, describe, encode
from hypothesis import given
from hypothesis import strategies as st

ints = {
    pa.int8(): (-(2**7), 2**7 - 1),
    pa.int16(): (-(2**15), 2**15 - 1),
    pa.int32(): (-(2**31), 2**31 - 1),
    pa.int64(): (-(2**63), 2**63 - 1),
    pa.uint8(): (0, 2**8 - 1),
    pa.uint16(): (0, 2**16 - 1),
    pa.uint32(): (0, 2**32 - 1),
    pa.uint64(): (0, 2**64 - 1),
}

leaf_types = st.one_of(
    st.sampled_from(list(ints)),
    st.sampled_from(
        [
            pa.float32(),
            pa.float64(),
            pa.bool_(),
            pa.string(),
            pa.large_string(),
            pa.binary(),
            pa.date32(),
            pa.time64("us"),
            pa.time64("ns"),
            pa.timestamp("ms"),
            pa.timestamp("us"),
            pa.timestamp("ns"),
            pa.timestamp("us", tz="UTC"),
            pa.timestamp("ms", tz="America/New_York"),
        ]
    ),
    st.integers(1, 38).flatmap(lambda p: st.integers(0, p).map(lambda s: pa.decimal128(p, s))),
)


def nested(children):
    names = st.text("abcxyz", min_size=1, max_size=3)
    return st.one_of(
        children.map(pa.list_),
        st.lists(st.tuples(names, children), min_size=1, max_size=3, unique_by=lambda f: f[0].lower()).map(
            lambda fs: pa.struct(fs)
        ),
        st.tuples(st.sampled_from([pa.string(), pa.int32()]), children).map(lambda kv: pa.map_(*kv)),
    )


types = st.recursive(leaf_types, nested, max_leaves=4)


def values(t: pa.DataType):
    """Python values of type `t`, or None."""
    if pa.types.is_integer(t):
        lo, hi = ints[t]
        v = st.one_of(st.integers(lo, hi), st.sampled_from([lo, hi, 0]))
    elif pa.types.is_floating(t):
        v = st.floats(width=t.bit_width, allow_nan=True, allow_infinity=True, allow_subnormal=True)
    elif pa.types.is_boolean(t):
        v = st.booleans()
    elif pa.types.is_string(t) or pa.types.is_large_string(t):
        v = st.text(max_size=20)
    elif pa.types.is_binary(t):
        v = st.binary(max_size=20)
    elif pa.types.is_date32(t):
        v = st.dates(datetime.date(1, 1, 1), datetime.date(9999, 12, 31))
    elif pa.types.is_time(t):
        v = st.integers(0, 86_400 * {"us": 10**6, "ns": 10**9}[t.unit] - 1)
    elif pa.types.is_timestamp(t):
        per_s = {"ms": 10**3, "us": 10**6, "ns": 10**9}[t.unit]
        bound = (2**63 - 1) // 1000 if t.unit == "ns" else 250_000 * 365 * 86_400 * per_s // 1000
        v = st.integers(-bound, bound)
    elif pa.types.is_decimal(t):
        top = 10**t.precision - 1
        v = st.integers(-top, top).map(lambda u: scaled(u, t.scale))
    elif pa.types.is_list(t):
        v = st.lists(values(t.value_type), max_size=4)
    elif pa.types.is_struct(t):
        v = st.fixed_dictionaries({t.field(i).name: values(t.field(i).type) for i in range(t.num_fields)})
    elif pa.types.is_map(t):
        # map keys are unique (DuckDB refuses a map that repeats one, as read_parquet does)
        v = st.lists(
            st.tuples(values(t.key_type).filter(lambda k: k is not None), values(t.item_type)),
            max_size=3,
            unique_by=lambda kv: kv[0],
        )
    else:
        raise AssertionError(t)
    return st.one_of(st.none(), v)


def scaled(u, scale):
    with decimal.localcontext() as ctx:
        ctx.prec = 80
        return decimal.Decimal(u).scaleb(-scale)


@st.composite
def tables(draw):
    columns = draw(st.lists(types, min_size=1, max_size=4))
    n = draw(st.integers(0, 120))
    data = {f"c{i}": pa.array(draw(st.lists(values(t), min_size=n, max_size=n)), t) for i, t in enumerate(columns)}
    return pa.table(data)


write_options = st.fixed_dictionaries(
    {
        "row_group_size": st.sampled_from([1, 7, 50, 10_000]),
        "data_page_version": st.sampled_from(["1.0", "2.0"]),
        "use_dictionary": st.booleans(),
        "compression": st.sampled_from(["zstd", "snappy", "none"]),
    }
)

copy_options = st.fixed_dictionaries(
    {
        "COMPRESSION": st.sampled_from(["pcodec", "zstd", "snappy", "uncompressed", "gzip", "lz4", "brotli"]),
        "ROW_GROUP_SIZE": st.sampled_from([1, 3, 64, 1_000_000]),
    }
)


def flba_in_list(t: pa.DataType, in_list=False) -> bool:
    """Whether a decimal (pyarrow writes FIXED_LEN_BYTE_ARRAY) is inside a list or map."""
    if pa.types.is_decimal(t):
        return in_list
    if pa.types.is_list(t):
        return flba_in_list(t.value_type, True)
    if pa.types.is_map(t):
        return flba_in_list(t.key_type, True) or flba_in_list(t.item_type, True)
    if pa.types.is_struct(t):
        return any(flba_in_list(t.field(i).type, in_list) for i in range(t.num_fields))
    return False


def write(table, path, options):
    """pyarrow's writer, but V1 pages where the Parquet reader cannot read its V2 pages
    (test_pyarrow_v2_pages_of_fixed_len_columns_under_null_lists)."""
    if any(flba_in_list(f.type) for f in table.schema):
        options = {**options, "data_page_version": "1.0"}
    pq.write_table(table, path, **options)


def read(con, sql):
    return rows_of(con.sql(sql).to_arrow_table())


def in_order(path, fn="read_tparquet"):
    return f"SELECT * EXCLUDE (file_row_number) FROM {fn}('{path}', file_row_number := true) ORDER BY file_row_number"


@given(table=tables(), options=write_options)
def test_read_tparquet_reads_standard_files_as_read_parquet(tmp_path_factory, table, options):
    path = tmp_path_factory.mktemp("prop") / "standard.parquet"
    write(table, path, options)
    con = connect()
    try:
        assert describe(con, in_order(path)) == describe(con, in_order(path, "read_parquet"))
        assert read(con, in_order(path)) == read(con, in_order(path, "read_parquet"))
    finally:
        con.close()


@given(table=tables(), write_opts=write_options, options=copy_options)
def test_copy_writes_what_read_tparquet_reads(tmp_path_factory, table, write_opts, options):
    d = tmp_path_factory.mktemp("prop")
    write(table, d / "standard.parquet", write_opts)
    con = connect()
    try:
        source = in_order(d / "standard.parquet")
        copy_to(con, source, d / "copy.parquet", **options)
        assert describe(con, f"SELECT * FROM read_tparquet('{d}/copy.parquet')") == describe(con, source)
        if DUCKDB_1:
            # DuckDB 1.x's COPY does not always keep the order of an ordered query (its own
            # FORMAT parquet neither): the rows are DuckDB's own COPY's, in its order
            standard = {**options, "COMPRESSION": "zstd" if options["COMPRESSION"] == "pcodec" else options["COMPRESSION"]}
            opts = "".join(f", {k} {v}" for k, v in standard.items())
            con.execute(f"COPY ({source}) TO '{d}/own.parquet' (FORMAT parquet{opts})")
            expected = read(con, in_order(d / "own.parquet", "read_parquet"))
        else:
            expected = read(con, source)
        assert read(con, in_order(d / "copy.parquet")) == expected
    finally:
        con.close()


@given(table=tables(), write_opts=write_options, policy=st.sampled_from(["pcodec", "auto"]))
def test_pcodec_transcodes_read_as_the_original(tmp_path_factory, table, write_opts, policy):
    d = tmp_path_factory.mktemp("prop")
    write(table, d / "standard.parquet", write_opts)
    encode(d / "standard.parquet", d / "pcodec.parquet", policy)
    con = connect()
    try:
        assert read(con, in_order(d / "pcodec.parquet")) == read(con, in_order(d / "standard.parquet"))
    finally:
        con.close()


# DuckDB's reader takes the V2 pages pyarrow writes for FIXED_LEN_BYTE_ARRAY columns under null lists
# (arrow-rs' parquet 60.1.0 refused them: it checks the values against num_values - num_nulls)
def test_pyarrow_v2_pages_of_fixed_len_columns_under_null_lists(con, tmp_path):
    t = pa.table({"a": pa.array([None], pa.list_(pa.list_(pa.struct([("d", pa.decimal128(1, 0))]))))})
    pq.write_table(t, tmp_path / "f.parquet", data_page_version="2.0", use_dictionary=False)
    assert con.execute(f"SELECT * FROM read_tparquet('{tmp_path}/f.parquet')").fetchall() == [(None,)]
