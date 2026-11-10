"""Every DuckDB type through COPY (FORMAT tparquet) and read_tparquet, with NULLs at every level.

Each case is a column expression over `i` (3000 rows: more than one DuckDB vector, and more
than one row group where asked). The file must read back with the same values, in the same
order, as the same type (or the documented one), and DuckDB's own Parquet copy of the same
column must read the same through read_tparquet as through read_parquet.
"""

import pyarrow as pa
import pyarrow.parquet as pq
import pytest
from canonical import rows_of
from conftest import DUCKDB_1, copy_to, describe, rows

N = 3000


def null_every(k, expr, offset=0):
    return f"CASE WHEN (i + {offset}) % {k} = 0 THEN NULL ELSE {expr} END"


# name -> (expression, the type read_tparquet gives when it differs from the column's)
CASES = {
    "boolean": (null_every(7, "i % 3 = 0"), None),
    "tinyint": (null_every(7, "CASE i % 4 WHEN 0 THEN -128 WHEN 1 THEN 127 ELSE (i % 200 - 100) END::TINYINT"), None),
    "smallint": (null_every(7, "CASE i % 4 WHEN 0 THEN -32768 WHEN 1 THEN 32767 ELSE i * 7 END::SMALLINT"), None),
    "integer": (null_every(7, "CASE i % 4 WHEN 0 THEN -2147483648 WHEN 1 THEN 2147483647 ELSE i * 7919 END::INTEGER"), None),
    "bigint": (null_every(7, "CASE i % 4 WHEN 0 THEN -9223372036854775808 WHEN 1 THEN 9223372036854775807 ELSE (hash(i) >> 1)::BIGINT * (1 - 2 * (i % 2)) END"), None),
    "utinyint": (null_every(7, "(i % 256)::UTINYINT"), None),
    "usmallint": (null_every(7, "CASE WHEN i % 5 = 1 THEN 65535 ELSE i END::USMALLINT"), None),
    "uinteger": (null_every(7, "CASE WHEN i % 5 = 1 THEN 4294967295 ELSE i * 100003 END::UINTEGER"), None),
    "ubigint": (null_every(7, "CASE WHEN i % 5 = 1 THEN 18446744073709551615 ELSE hash(i) END::UBIGINT"), None),
    "float": (null_every(7, "CASE i % 6 WHEN 0 THEN 'nan'::FLOAT WHEN 1 THEN 'inf'::FLOAT WHEN 2 THEN '-inf'::FLOAT WHEN 3 THEN -0.0::FLOAT ELSE (i * 0.25)::FLOAT END"), None),
    "double": (null_every(7, "CASE i % 6 WHEN 0 THEN 'nan'::DOUBLE WHEN 1 THEN 'inf'::DOUBLE WHEN 2 THEN 5e-324 WHEN 3 THEN -0.0 ELSE 100 + (i % 97) * 0.01 END"), None),
    "varchar": (null_every(7, "CASE i % 4 WHEN 0 THEN '' WHEN 1 THEN 'ünïcode ✓ ' || i WHEN 2 THEN repeat('long', i % 50) ELSE 'q\"''\\' || chr(10) || i END"), None),
    "blob": (null_every(7, "CASE i % 3 WHEN 0 THEN ''::BLOB WHEN 1 THEN '\\x00\\xFF\\x01'::BLOB ELSE encode(repeat('b', i % 40)) END"), None),
    "date": (null_every(7, "CASE i % 5 WHEN 0 THEN DATE '1970-01-01' WHEN 1 THEN DATE '0001-01-01' WHEN 2 THEN DATE '9999-12-31' ELSE DATE '2025-01-01' + i::INTEGER END"), None),
    "date_infinite": (null_every(7, "CASE i % 2 WHEN 0 THEN 'infinity'::DATE ELSE '-infinity'::DATE END"), None),
    "time": (null_every(7, "TIME '00:00:00' + INTERVAL (i * 28.8) SECOND"), None),
    "time_end_of_day": (null_every(7, "CASE i % 2 WHEN 0 THEN TIME '24:00:00' ELSE TIME '23:59:59.999999' END"), None),
    "time_ns": (null_every(7, "(lpad((i % 24)::VARCHAR, 2, '0') || ':00:' || lpad((i % 60)::VARCHAR, 2, '0') || '.' || lpad((i * 1000003 % 1000000000)::VARCHAR, 9, '0'))::TIME_NS"), None),
    "timestamp_s": (null_every(7, "(TIMESTAMP '2025-01-01' + INTERVAL (i * 3601) SECOND)::TIMESTAMP_S"), "TIMESTAMP_MS"),
    "timestamp_ms": (null_every(7, "(TIMESTAMP '1900-01-01' + INTERVAL (i * 7) MILLISECOND)::TIMESTAMP_MS"), None),
    "timestamp": (null_every(7, "TIMESTAMP '2025-01-01' + INTERVAL (i * 7) MICROSECOND"), None),
    "timestamp_ns": (null_every(7, "'2025-01-01 00:00:00.000000001'::TIMESTAMP_NS + INTERVAL (i) SECOND"), None),
    "timestamptz": (null_every(7, "TIMESTAMPTZ '2025-01-01 00:00:00+05' + INTERVAL (i) MINUTE"), None),
    "timestamptz_ns": (null_every(7, "make_timestamp_ns(1735689600123456789 + i * 1000000007)::TIMESTAMPTZ_NS"), None),
    "timestamp_infinite": (null_every(7, "CASE i % 2 WHEN 0 THEN 'infinity'::TIMESTAMP ELSE '-infinity'::TIMESTAMP END"), None),
    "enum": (null_every(7, "(['a', 'b', 'c'])[1 + i % 3]::ENUM('a', 'b', 'c')"), "VARCHAR"),
    "enum_large": (null_every(7, "(i % 300)::VARCHAR::ENUM_300"), "VARCHAR"),
    "list": (null_every(7, f"[{null_every(5, 'i')}, i + 1, NULL]"), None),
    "list_empty": (null_every(7, "[]::VARCHAR[]"), None),
    "list_of_lists": (null_every(11, f"[{null_every(5, '[i, NULL]')}, [], [{null_every(3, 'i')}]]"), None),
    "list_of_varchar": (null_every(7, "['x' || i, NULL, repeat('y', i % 30)]"), None),
    "list_of_decimals": (null_every(7, "[(i * 1.25)::DECIMAL(38, 4), NULL]"), None),
    "list_of_timestamptz": (null_every(7, "[TIMESTAMPTZ '2025-01-01 00:00:00+00' + INTERVAL (i) HOUR, NULL]"), None),
    "list_of_enum": (null_every(7, "[(['a', 'b'])[1 + i % 2]::ENUM('a', 'b'), NULL]"), "VARCHAR[]"),
    "struct": (null_every(7, f"{{'a': {null_every(5, 'i')}, 'b': {null_every(3, 'i::VARCHAR')}}}"), None),
    "nested_struct": (
        null_every(
            11,
            f"{{'x': i * 1.5, 'tags': {null_every(5, '[i::INTEGER, NULL]')}, "
            f"'inner': {null_every(3, "{'y': " + null_every(2, "'v' || i") + ", 'z': [i]}")}}}",
        ),
        None,
    ),
    "struct_list": (null_every(7, f"[{null_every(5, "{'p': i * 0.5, 'q': i}")}, {{'p': NULL, 'q': NULL}}]"), None),
    "map": (null_every(7, f"MAP {{'k' || i: {null_every(5, 'i::INTEGER')}, 'z': 1}}"), None),
    "map_empty": (null_every(7, "MAP {}::MAP(VARCHAR, INTEGER)"), None),
    "map_of_lists": (null_every(7, f"MAP {{i: {null_every(5, '[i, NULL]')}}}"), None),
    "map_of_structs": (null_every(7, f"MAP {{'a' || i: {null_every(5, "{'s': i}")}}}"), None),
    "map_enum_keys": (null_every(7, "MAP {(['a', 'b'])[1 + i % 2]::ENUM('a', 'b'): i}"), "MAP(VARCHAR, BIGINT)"),
    # the CASE of null_every is over lists, cast to arrays after it: DuckDB 1.5 has no CASE over
    # arrays ("Unimplemented type for case expression")
    "array": ("(" + null_every(7, f"[i::INTEGER, {null_every(3, '-i::INTEGER')}]") + ")::INTEGER[2]", None),
    "array_of_varchar": ("(" + null_every(7, f"['a' || i, {null_every(3, "'b'")}, '']") + ")::VARCHAR[3]", None),
    "array_of_structs": ("(" + null_every(7, f"[{{'a': i}}, {null_every(3, "{'a': NULL}")}]") + ")::STRUCT(a BIGINT)[2]", None),
    "array_of_arrays": ("(" + null_every(7, f"[[i, i], {null_every(3, '[NULL, i]')}, [1, 2]]") + ")::BIGINT[2][3]", None),
    "list_of_arrays": (f"[[i, i], {null_every(3, '[1, 2]')}]::BIGINT[2][]", None),
    "struct_of_map_list": (
        null_every(7, f"[{{'m': {null_every(5, "MAP {'k': [i, NULL]}")}, 'n': {null_every(3, 'i')}}}]"),
        None,
    ),
    "deep": (null_every(13, f"[[{{'a': [{{'b': {null_every(5, '[i]')}}}]}}]]"), None),
}
if DUCKDB_1:
    del CASES["timestamptz_ns"]  # DuckDB 1.x has no TIMESTAMP WITH TIME ZONE in nanoseconds


@pytest.fixture(scope="module")
def database(tmp_path_factory):
    from conftest import connect

    con = connect()
    con.execute("CREATE TYPE ENUM_300 AS ENUM (SELECT range::VARCHAR FROM range(300))")
    yield con, tmp_path_factory.mktemp("types")
    con.close()


@pytest.mark.parametrize("options", [{}, {"COMPRESSION": "pcodec", "ROW_GROUP_SIZE": 1000}], ids=["auto", "pcodec-rg1000"])
@pytest.mark.parametrize("name", list(CASES))
def test_a_type_round_trips_in_order(database, name, options):
    con, d = database
    expr, _ = CASES[name]
    con.execute(f"CREATE OR REPLACE TABLE src AS SELECT {expr} AS v FROM range({N}) r(i)")
    path = copy_to(con, "SELECT * FROM src", d / f"{name}.parquet", **options)
    # the type is the one DuckDB's own Parquet round trip gives (read_tparquet is DuckDB's reader)
    standard = d / f"{name}.roundtrip.parquet"
    con.execute(f"COPY src TO '{standard}' (FORMAT parquet)")
    native_type = describe(con, f"SELECT v FROM read_parquet('{standard}')")[0][1]
    back = f"read_tparquet('{path}', file_row_number := true)"
    assert describe(con, f"SELECT v FROM {back}") == [("v", native_type)]
    differ = rows(
        con,
        f"SELECT count(*) FROM (SELECT v, row_number() OVER () - 1 AS n FROM src) a "
        f"JOIN {back} b ON a.n = b.file_row_number WHERE a.v::{native_type} IS DISTINCT FROM b.v",
    )
    assert differ == [(0,)]
    assert rows(con, f"SELECT count(*), count(v) FROM {back}") == rows(con, "SELECT count(*), count(v) FROM src")


@pytest.mark.parametrize("name", list(CASES))
def test_duckdbs_own_parquet_copy_reads_as_read_parquet_reads_it(database, name):
    """read_tparquet reads standard files: DuckDB's Parquet writer's copy of every type."""
    con, d = database
    expr, _ = CASES[name]
    con.execute(f"CREATE OR REPLACE TABLE src AS SELECT {expr} AS v FROM range({N}) r(i)")
    path = d / f"{name}.standard.parquet"
    con.execute(f"COPY src TO '{path}' (FORMAT parquet, ROW_GROUP_SIZE 1024)")
    ours, native = f"read_tparquet('{path}')", f"read_parquet('{path}')"
    native_type = describe(con, f"SELECT * FROM {native}")[0][1]
    assert describe(con, f"SELECT * FROM {ours}") == [("v", native_type)]
    # the values are the same (in read_parquet's type where the types differ)
    assert rows_of(con.sql(f"SELECT v::{native_type} AS v FROM {ours}").to_arrow_table()) == rows_of(
        con.sql(f"SELECT * FROM {native}").to_arrow_table()
    )


def test_floats_keep_every_bit(database):
    con, d = database
    bits64 = [0, 1 << 63, 0x7FF0000000000000, 0xFFF0000000000000, 0x7FF8000000000000, 0x7FF0000000000001,
              0xFFF8DEADBEEF0001, 0x0000000000000001, 0x800FFFFFFFFFFFFF, 0x7FEFFFFFFFFFFFFF, 0x3FB999999999999A]
    bits32 = [0, 1 << 31, 0x7F800000, 0xFF800000, 0x7FC00000, 0x7F800001, 0xFFC0BEEF, 0x00000001, 0x807FFFFF,
              0x7F7FFFFF, 0x3DCCCCCD]
    n = len(bits64)
    t = pa.table(
        {
            "d": pa.array(bits64, pa.uint64()).view(pa.float64()),
            "f": pa.array(bits32, pa.uint32()).view(pa.float32()),
            "ld": pa.array([[x, None] for x in pa.array(bits64, pa.uint64()).view(pa.float64()).to_pylist()],
                           pa.list_(pa.float64())),
        }
    )
    src = d / "bits.parquet"
    pq.write_table(t, src)
    for options in [{"COMPRESSION": "pcodec"}, {"COMPRESSION": "zstd"}, {"COMPRESSION": "pcodec", "COMPRESSION_LEVEL": 0}]:
        path = copy_to(con, f"SELECT * FROM read_tparquet('{src}')", d / "bits.copy.parquet", **options)
        back = con.sql(f"SELECT * FROM read_tparquet('{path}')").to_arrow_table()
        assert back.column("d").combine_chunks().view(pa.uint64()).to_pylist() == bits64
        assert back.column("f").combine_chunks().view(pa.uint32()).to_pylist() == bits32
        lists = back.column("ld").combine_chunks()
        assert lists.values.view(pa.uint64()).to_pylist()[::2] == bits64
        assert len(back) == n


def test_decimals_of_every_width_and_scale(database):
    con, d = database
    import decimal
    import random

    rnd = random.Random(38)
    columns = {}
    for width in range(1, 39):
        for scale in sorted({0, width // 2, width}):
            top = 10**width - 1
            unscaled = [top, -top, 0, 1, -1] + [rnd.randrange(-top, top + 1) for _ in range(495)]
            with decimal.localcontext() as ctx:
                ctx.prec = 80
                values = [None if i % 9 == 4 else decimal.Decimal(u).scaleb(-scale) for i, u in enumerate(unscaled)]
            columns[f"d{width}_{scale}"] = pa.array(values, pa.decimal128(width, scale))
    src = d / "dec.source.parquet"
    pq.write_table(pa.table(columns), src)
    con.execute(f"CREATE OR REPLACE TABLE dec AS SELECT * FROM read_parquet('{src}')")
    assert describe(con, "SELECT d38_19, d4_0 FROM dec") == [("d38_19", "DECIMAL(38,19)"), ("d4_0", "DECIMAL(4,0)")]
    for codec in ["pcodec", "zstd"]:
        path = copy_to(con, "SELECT * FROM dec", d / "dec.parquet", COMPRESSION=codec)
        back = f"read_tparquet('{path}')"
        assert describe(con, f"SELECT * FROM {back}") == describe(con, "SELECT * FROM dec")
        assert rows(con, f"SELECT * FROM {back}") == rows(con, "SELECT * FROM dec")


def test_a_table_of_every_type_in_any_column_order(database):
    con, d = database
    names = list(CASES)
    con.execute(
        "CREATE OR REPLACE TABLE wide AS SELECT "
        + ", ".join(f'{CASES[n][0]} AS "{n}"' for n in names)
        + f" FROM range({N}) r(i)"
    )
    path = copy_to(con, "SELECT * FROM wide", d / "wide.parquet", ROW_GROUP_SIZE=777)
    back = f"read_tparquet('{path}')"
    # every projection order reads the same values as the table
    for order in [names, names[::-1], names[1::2] + names[::2]]:
        cols = ", ".join(f'"{n}"' for n in order)
        diff = rows(con, f"SELECT count(*) FROM (SELECT {cols} FROM wide EXCEPT ALL SELECT {cols} FROM {back})")
        assert diff == [(0,)], order[:3]


# more types, as DuckDB's own Parquet writer stores them (the cases above cover the rest)
STANDARD_ONLY = {
    "interval": (null_every(7, "INTERVAL (i) MONTH + INTERVAL (i * 13) DAY + INTERVAL (i * 7) MILLISECOND"),),
    "hugeint": (null_every(7, "(i * 1000)::HUGEINT"),),
    "uhugeint": (null_every(7, "(i * 1000)::UHUGEINT"),),
    "bit": (null_every(7, "(i % 16)::BIT"),),
    "union": (null_every(7, "union_value(k := i::INTEGER)::UNION(k INTEGER, s VARCHAR)"),),
    "uuid": (null_every(7, "md5(i::VARCHAR)::UUID"),),
    "timetz": (null_every(7, "('12:00:00+0' || (i % 10)::VARCHAR)::TIMETZ"),),
}


@pytest.mark.parametrize("name", list(STANDARD_ONLY))
def test_the_other_types_read_and_write_as_duckdbs_own_parquet(database, name):
    """read_tparquet reads them as read_parquet does, and COPY (FORMAT tparquet) writes them as
    DuckDB's own Parquet writer does (read_tparquet and the COPY are DuckDB's)."""
    con, d = database
    expr = STANDARD_ONLY[name][0]
    con.execute(f"CREATE OR REPLACE TABLE src AS SELECT {expr} AS v FROM range({N}) r(i)")
    path = d / f"{name}.standard.parquet"
    con.execute(f"COPY src TO '{path}' (FORMAT parquet)")
    ours = copy_to(con, "SELECT * FROM src", d / f"{name}.tparquet.parquet")
    native = describe(con, f"SELECT * FROM read_parquet('{path}')")
    want = rows_of(con.sql(f"SELECT * FROM read_parquet('{path}')").to_arrow_table())
    for f in (path, ours):
        assert describe(con, f"SELECT * FROM read_tparquet('{f}')") == native, f
        assert rows_of(con.sql(f"SELECT * FROM read_tparquet('{f}')").to_arrow_table()) == want, f