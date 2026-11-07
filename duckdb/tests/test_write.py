"""COPY ... TO ... (FORMAT tparquet) and COPY ... FROM, read back and checked by the Rust CLI."""

import json

import duckdb
import pyarrow.parquet as pq
import pytest

from canonical import lines
from conftest import API, FILES, run_cli


def rows(con, sql):
    return con.execute(sql).fetchall()


def names_of(con, sql):
    return [r[0] for r in con.execute(f"DESCRIBE {sql}").fetchall()]


def copy(con, query, path, **options):
    names = names_of(con, query)
    opts = ", ".join(f"{k} {v}" for k, v in options.items())
    cols = ", ".join(f"'{n}'" for n in names)
    con.execute(f"COPY ({query}) TO '{path}' (FORMAT tparquet, COLUMN_NAMES [{cols}]{', ' + opts if opts else ''})")
    return path


def inspect(path):
    out = run_cli("inspect", "--json", path)
    assert out.returncode == 0, out.stderr
    return json.loads(out.stdout)


def codecs(path):
    """Leaf path -> set of codecs, from `tparquet inspect --json`."""
    info = inspect(path)
    return {c["path"]: set(c["codecs"]) for c in info["columns"]}


ALL_TYPES = """
SELECT
    i::TINYINT % 100 AS i8, (i * 300)::SMALLINT AS i16, (i * 70000)::INTEGER AS i32, i * 10000000000 AS i64,
    (i % 200)::UTINYINT AS u8, (i * 60)::USMALLINT AS u16, (i * 4000000)::UINTEGER AS u32,
    (i::UBIGINT * 1000000000000000) AS u64,
    (i * 0.5)::FLOAT AS f32, i * 0.01 AS f64,
    (i * 0.25)::DECIMAL(4,2) AS dec4, (i * 1.5)::DECIMAL(9,2) AS dec9,
    (i * 12345.6789)::DECIMAL(18,4) AS dec18, (i * 1e20)::DECIMAL(38,2) AS dec38,
    'row ' || i || ' ünïcode' AS s, ('\\x' || lpad(to_hex(i), 2, '0'))::BLOB AS bin,
    i % 3 = 0 AS b,
    DATE '2025-01-01' + i::INTEGER AS d, TIME '12:00:00' + INTERVAL (i) SECOND AS t,
    '12:00:00.123456789'::TIME_NS AS tns,
    (TIMESTAMP '2025-01-01' + INTERVAL (i) SECOND)::TIMESTAMP_S AS ts_s,
    (TIMESTAMP '2025-01-01' + INTERVAL (i * 7) MILLISECOND)::TIMESTAMP_MS AS ts_ms,
    TIMESTAMP '2025-01-01' + INTERVAL (i * 7) MICROSECOND AS ts_us,
    ('2025-01-01 00:00:00.000000001'::TIMESTAMP_NS + INTERVAL (i) SECOND) AS ts_ns,
    TIMESTAMPTZ '2025-01-01 00:00:00+00' + INTERVAL (i) MINUTE AS tstz,
    '2025-01-01 00:00:00.123456789+00'::TIMESTAMPTZ_NS AS tstz_ns,
    (['a', 'b', 'c'])[1 + i % 3]::ENUM('a', 'b', 'c') AS e,
    [i, i + 1, NULL] AS li, [] :: VARCHAR[] AS empty_list,
    {'x': i * 1.5, 'tags': [i::INTEGER], 'inner': {'y': 'v' || i}} AS st,
    MAP {'k' || i: i::INTEGER} AS m,
    [i::INTEGER, -i::INTEGER]::INTEGER[2] AS arr
FROM range(0, 50) r(i)
"""


def with_nulls(query):
    """The same columns, with every column NULL in every fifth row."""
    cols = names_of(duckdb.connect(), query)
    picks = ", ".join(f"CASE WHEN rn % 5 = 4 THEN NULL ELSE {c} END AS {c}" for c in cols)
    return f"SELECT {picks} FROM (SELECT *, row_number() OVER () - 1 AS rn FROM ({query}))"


def test_every_writable_type_round_trips(con, tmp_path):
    con.execute(f"CREATE TABLE src AS {with_nulls(ALL_TYPES)}")
    path = copy(con, "SELECT * FROM src", tmp_path / "all.parquet")
    back = f"read_tparquet('{path}')"
    # seconds come back as milliseconds and enums as strings; every other type is unchanged
    src_types = {r[0]: r[1] for r in con.execute("DESCRIBE src").fetchall()}
    back_types = {r[0]: r[1] for r in con.execute(f"DESCRIBE SELECT * FROM {back}").fetchall()}
    changed = {c: (src_types[c], back_types[c]) for c in src_types if src_types[c] != back_types[c]}
    assert changed == {"ts_s": ("TIMESTAMP_S", "TIMESTAMP_MS"), "e": ("ENUM('a', 'b', 'c')", "VARCHAR")}
    for a, b in [("src", back), (back, "src")]:
        diff = rows(con, f"SELECT count(*) FROM (SELECT * FROM {a} EXCEPT ALL SELECT * FROM {b})")
        assert diff == [(0,)], (a, b)
    assert rows(con, f"SELECT count(*), count(i64) FROM {back}") == [(50, 40)]


def test_floats_are_written_bit_for_bit(con, tmp_path):
    src = FILES / "float_specials.parquet"
    path = copy(con, f"SELECT * FROM read_tparquet('{src}')", tmp_path / "f.parquet")
    schema = pq.read_schema(src)
    a = lines(con.sql(f"SELECT * FROM read_tparquet('{src}')").to_arrow_table(), schema)
    b = lines(con.sql(f"SELECT * FROM read_tparquet('{path}')").to_arrow_table(), schema)
    assert a == b
    assert a == (FILES.parent / "expected" / "float_specials.jsonl").read_text()


def test_auto_picks_pcodec_for_numeric_columns_and_keeps_strings_on_zstd(con, tmp_path):
    q = (
        "SELECT i AS id, 1735689600000 + i * 37 AS ts, (100 + (i % 97) * 0.01)::DOUBLE AS price, "
        "(i * 0.25)::DECIMAL(18, 2) AS amount, 'sym' || (i % 4) AS sym FROM range(300000) r(i)"
    )
    path = copy(con, q, tmp_path / "auto.parquet", POLICY="'auto'", LEVEL=8)
    c = codecs(path)
    assert c == {"id": {"PCODEC"}, "ts": {"PCODEC"}, "price": {"PCODEC"}, "amount": {"PCODEC"}, "sym": {"ZSTD"}}
    assert inspect(path)["pcodec_version"] == "1"
    assert rows(con, f"SELECT count(*), sum(id), max(price) FROM read_tparquet('{path}')") == rows(
        con, f"SELECT count(*), sum(id), max(price) FROM ({q})"
    )


def test_order_is_kept_across_vectors_and_row_groups(con, tmp_path):
    q = "SELECT i AS id FROM range(1000000) r(i)"
    path = copy(con, q, tmp_path / "order.parquet", ROW_GROUP_ROWS=100000)
    assert inspect(path)["row_groups"] == 10
    r = rows(
        con,
        f"SELECT count(*) FILTER (WHERE id <> file_row_number) FROM read_tparquet('{path}', file_row_number := true)",
    )
    assert r == [(0,)]


def test_policies_levels_and_base_codecs(con, tmp_path):
    q = "SELECT i AS a, i * 2 AS b, i::VARCHAR AS c FROM range(10000) r(i)"
    c = codecs(copy(con, q, tmp_path / "base.parquet", POLICY="'base'"))
    assert c == {"a": {"ZSTD"}, "b": {"ZSTD"}, "c": {"ZSTD"}}
    c = codecs(copy(con, q, tmp_path / "pc.parquet", POLICY="'pcodec'", LEVEL=0))
    assert c == {"a": {"PCODEC"}, "b": {"PCODEC"}, "c": {"ZSTD"}}
    c = codecs(copy(con, q, tmp_path / "col.parquet", POLICY="'base'", COLUMN_POLICY="['b=pcodec']"))
    assert c == {"a": {"ZSTD"}, "b": {"PCODEC"}, "c": {"ZSTD"}}
    c = codecs(copy(con, q, tmp_path / "snappy.parquet", POLICY="'base'", COMPRESSION="'snappy'"))
    assert c == {"a": {"SNAPPY"}, "b": {"SNAPPY"}, "c": {"SNAPPY"}}
    c = codecs(copy(con, q, tmp_path / "z9.parquet", POLICY="'pcodec'", COMPRESSION="'zstd'", COMPRESSION_LEVEL=9))
    assert c["c"] == {"ZSTD"}
    for name in ["base", "pc", "col", "snappy", "z9"]:
        assert rows(con, f"SELECT sum(a), sum(b), count(c) FROM read_tparquet('{tmp_path}/{name}.parquet')") == [
            (49995000, 99990000, 10000)
        ]


def test_an_empty_result_writes_an_empty_file(con, tmp_path):
    path = copy(con, "SELECT 1::BIGINT AS a, 'x' AS b WHERE false", tmp_path / "empty.parquet")
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{path}')") == [(0,)]
    assert names_of(con, f"SELECT * FROM read_tparquet('{path}')") == ["a", "b"]


@pytest.mark.parametrize(
    "options, message",
    [
        (", COLUMN_NAMES ['a']", "COLUMN_NAMES has 1 names but the query has 2 columns"),
        (", COLUMN_NAMES ['a', 'a']", 'COLUMN_NAMES: "a" appears twice'),
        (", COLUMN_NAMES ['a', 'b'], POLICY 'fast'", 'unknown policy "fast"'),
        (", COLUMN_NAMES ['a', 'b'], LEVEL 13", "invalid LEVEL 13: expected 0 to 12"),
        (", COLUMN_NAMES ['a', 'b'], COMPRESION 'zstd'", "unknown option COMPRESION for FORMAT tparquet"),
        (", COLUMN_NAMES ['a', 'b'], COMPRESSION 'lzo'", 'unknown COMPRESSION "lzo"'),
    ],
)
def test_bad_options_are_explained(con, tmp_path, options, message):
    with pytest.raises(duckdb.Error, match=message):
        con.execute(f"COPY (SELECT 1 AS a, 2 AS b) TO '{tmp_path}/x.parquet' (FORMAT tparquet{options})")


def test_column_names_are_required_in_v1_and_come_from_the_query_in_v2(con, tmp_path):
    q = 'SELECT 1::BIGINT AS id, \'x\' AS "Sym", 2.5::DOUBLE AS "with space"'
    sql = f"COPY ({q}) TO '{tmp_path}/n.parquet' (FORMAT tparquet)"
    if API == "v1":
        # the C API v1 gives a COPY function the types of the columns but not their names
        with pytest.raises(duckdb.Error, match="FORMAT tparquet needs COLUMN_NAMES"):
            con.execute(sql)
        return
    con.execute(sql)
    assert names_of(con, f"SELECT * FROM read_tparquet('{tmp_path}/n.parquet')") == ["id", "Sym", "with space"]
    assert rows(con, f"SELECT * FROM read_tparquet('{tmp_path}/n.parquet')") == [(1, "x", 2.5)]
    # DuckDB makes the names of a query unique before it hands them over
    con.execute(f"COPY (SELECT 1 AS a, 2 AS a) TO '{tmp_path}/d.parquet' (FORMAT tparquet)")
    assert names_of(con, f"SELECT * FROM read_tparquet('{tmp_path}/d.parquet')") == ["a", "a_1"]
    # COLUMN_NAMES, when given, replaces them and is checked as in v1
    con.execute(f"COPY ({q}) TO '{tmp_path}/r.parquet' (FORMAT tparquet, COLUMN_NAMES ['x', 'y', 'z'])")
    assert names_of(con, f"SELECT * FROM read_tparquet('{tmp_path}/r.parquet')") == ["x", "y", "z"]
    with pytest.raises(duckdb.Error, match="COLUMN_NAMES has 2 names but the query has 3 columns"):
        con.execute(f"COPY ({q}) TO '{tmp_path}/r.parquet' (FORMAT tparquet, COLUMN_NAMES ['x', 'y'])")
    # a column policy names a column as the query does
    con.execute(f"COPY ({q}) TO '{tmp_path}/p.parquet' (FORMAT tparquet, COLUMN_POLICY ['with space=base'])")
    assert codecs(tmp_path / "p.parquet")["with space"] == {"ZSTD"}


@pytest.mark.parametrize(
    "expr, name",
    [
        ("uuid()", "UUID"),
        ("1::HUGEINT", "HUGEINT"),
        ("1::UHUGEINT", "UHUGEINT"),
        ("INTERVAL 1 DAY", "INTERVAL"),
        ("[INTERVAL 1 DAY]", "INTERVAL"),
        ("union_value(k := 1)", "UNION"),
        ("'101'::BIT", "BIT"),
        ("'12:00:00+01'::TIMETZ", "TIME WITH TIME ZONE"),
        ("1::BIGNUM", "BIGNUM"),
        ("1::VARIANT", "VARIANT"),
        ("'POINT(1 2)'::GEOMETRY", "GEOMETRY"),
        ("NULL", "NULL"),
    ],
)
def test_types_it_cannot_write_are_refused(con, tmp_path, expr, name):
    with pytest.raises(duckdb.Error, match=f'column "v": columns of type {name} cannot be written'):
        con.execute(f"COPY (SELECT {expr} AS v) TO '{tmp_path}/x.parquet' (FORMAT tparquet, COLUMN_NAMES ['v'])")


def test_an_unwritable_path(con):
    with pytest.raises(duckdb.Error, match="cannot create /no/such/dir/x.parquet"):
        con.execute("COPY (SELECT 1 AS a) TO '/no/such/dir/x.parquet' (FORMAT tparquet, COLUMN_NAMES ['a'])")


def test_copy_from_into_a_table(con, tmp_path):
    src = FILES / "mixed.parquet"
    con.execute(f"CREATE TABLE t AS SELECT * FROM read_tparquet('{src}') LIMIT 0")
    con.execute(f"COPY t FROM '{src}' (FORMAT tparquet)")
    assert rows(con, "SELECT count(*) FROM t") == [(1500,)]
    con.execute(f"COPY t FROM '{src}' (FORMAT tparquet)")
    assert rows(con, "SELECT count(*) FROM t") == [(3000,)]
    con.execute("CREATE TABLE narrow (ts TIMESTAMPTZ, price DOUBLE)")
    with pytest.raises(duckdb.Error, match="do not match the table columns"):
        con.execute(f"COPY narrow FROM '{src}' (FORMAT tparquet)")


def test_duckdb_write_then_rust_verify(con, tmp_path):
    """A file DuckDB writes with the extension holds what DuckDB's own Parquet writer writes."""
    q = with_nulls(
        "SELECT i AS id, 100 + (i % 977) * 0.01 AS price, ((i % 50) + 1) * 0.001 AS qty, "
        "(TIMESTAMP '2025-01-01' + INTERVAL (i * 13) MILLISECOND)::TIMESTAMP_MS AS ts, "
        "'s' || (i % 7) AS sym, i % 2 = 0 AS buy, [i, i * 2] AS li, {'a': i::INTEGER, 'b': 'x' || i} AS st, "
        "(i * 0.5)::DECIMAL(10, 2) AS dec, DATE '2025-01-01' + (i % 365)::INTEGER AS d "
        "FROM range(200000) r(i)"
    )
    pcodec = copy(con, q, tmp_path / "pcodec.parquet")
    standard = tmp_path / "standard.parquet"
    con.execute(f"COPY ({q}) TO '{standard}' (FORMAT parquet)")
    out = run_cli("verify", standard, pcodec)
    assert out.returncode == 0, out.stdout + out.stderr
    # and verify notices a difference
    other = copy(con, q.replace("i * 2]", "i * 3]"), tmp_path / "other.parquet")
    assert run_cli("verify", standard, other).returncode == 1


@pytest.mark.parametrize("name", sorted(p.stem for p in FILES.glob("*.parquet")))
def test_vectors_rewritten_by_duckdb_verify_against_duckdbs_own_writer(con, tmp_path, name):
    """Every vector, read and written back by the extension, holds what DuckDB's built-in
    Parquet writer writes for the same rows (`tparquet verify`), and reads back to the
    vector's canonical rows."""
    src = FILES / f"{name}.parquet"
    q = f"SELECT * EXCLUDE (file_row_number) FROM read_tparquet('{src}', file_row_number := true) ORDER BY file_row_number"
    out = copy(con, q, tmp_path / f"{name}.parquet")
    standard = tmp_path / f"{name}.standard.parquet"
    con.execute(f"COPY ({q}) TO '{standard}' (FORMAT parquet)")
    r = run_cli("verify", standard, out)
    if name == "float_specials":
        # DuckDB's own writer does not keep NaN payloads: the extension's file has the vector's
        # bits (checked below), DuckDB's has a canonical NaN in their place
        assert r.returncode == 1 and '"f64_opt":"0x7ff0000000000001"' in r.stdout, r.stdout + r.stderr
    else:
        assert r.returncode == 0, r.stdout + r.stderr
    back = con.sql(f"SELECT * FROM read_tparquet('{out}')").to_arrow_table()
    assert lines(back, pq.read_schema(src)) == (FILES.parent / "expected" / f"{name}.jsonl").read_text()


def test_infinite_seconds_timestamps_stay_infinite(con, tmp_path):
    q = (
        "SELECT * FROM (VALUES ('infinity'::TIMESTAMP_S), ('-infinity'::TIMESTAMP_S), (NULL), "
        "('2025-01-01 00:00:01'::TIMESTAMP_S)) t(ts)"
    )
    path = copy(con, q, tmp_path / "inf.parquet")
    assert rows(con, f"SELECT ts::VARCHAR, typeof(ts) FROM read_tparquet('{path}')") == [
        ("infinity", "TIMESTAMP_MS"),
        ("-infinity", "TIMESTAMP_MS"),
        (None, "TIMESTAMP_MS"),
        ("2025-01-01 00:00:01", "TIMESTAMP_MS"),
    ]
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{path}') WHERE ts = 'infinity'::TIMESTAMP_MS") == [(1,)]


# ---------------------------------------------------------------------------------------------
# COPY targets


def test_copy_over_an_existing_file_replaces_it(con, tmp_path):
    p = copy(con, "SELECT i AS a FROM range(10) r(i)", tmp_path / "f.parquet")
    copy(con, "SELECT 'x' || i AS b FROM range(3) r(i)", p)
    assert rows(con, f"SELECT * FROM read_tparquet('{p}')") == [("x0",), ("x1",), ("x2",)]


def test_a_query_that_fails_midway_leaves_no_file_and_keeps_an_old_one(con, tmp_path):
    failing = "SELECT CASE WHEN i < 300000 THEN i ELSE error('boom') END AS a FROM range(400000) r(i)"
    with pytest.raises(duckdb.Error, match="boom"):
        copy(con, failing, tmp_path / "new.parquet")
    assert not (tmp_path / "new.parquet").exists()
    old = copy(con, "SELECT 42::BIGINT AS a", tmp_path / "old.parquet")
    with pytest.raises(duckdb.Error, match="boom"):
        copy(con, failing, old)
    assert rows(con, f"SELECT * FROM read_tparquet('{old}')") == [(42,)]
    assert sorted(p.name for p in tmp_path.iterdir()) == ["old.parquet"]


def test_a_directory_is_not_a_target(con, tmp_path):
    (tmp_path / "d.parquet").mkdir()
    with pytest.raises(duckdb.Error):
        copy(con, "SELECT 1 AS a", tmp_path / "d.parquet")


def test_partition_by_writes_hive_directories_of_the_other_columns(con, tmp_path):
    con.execute("CREATE TABLE t AS SELECT i AS id, i % 3 AS k, 'v' || i AS s FROM range(9000) r(i)")
    # DuckDB hands the COPY function the columns without the partition column
    with pytest.raises(duckdb.Error, match="COLUMN_NAMES has 3 names but the query has 2 columns"):
        con.execute(f"COPY t TO '{tmp_path}/a' (FORMAT tparquet, COLUMN_NAMES ['id', 'k', 's'], PARTITION_BY (k))")
    con.execute(
        f"COPY t TO '{tmp_path}/p' (FORMAT tparquet, COLUMN_NAMES ['id', 's'], PARTITION_BY (k), FILE_EXTENSION 'parquet')"
    )
    files = sorted(p.relative_to(tmp_path / "p").as_posix() for p in (tmp_path / "p").rglob("*.parquet"))
    assert [f.split("/")[0] for f in files] == ["k=0", "k=1", "k=2"]
    got = rows(
        con,
        f"SELECT regexp_extract(filename, 'k=(\\d)', 1)::BIGINT AS k, count(*), sum(id) "
        f"FROM read_tparquet('{tmp_path}/p/*/*.parquet', filename := true) GROUP BY ALL ORDER BY 1",
    )
    assert got == rows(con, "SELECT k, count(*), sum(id) FROM t GROUP BY ALL ORDER BY 1")
    con.execute(
        f"COPY t TO '{tmp_path}/w' (FORMAT tparquet, COLUMN_NAMES ['id', 'k', 's'], PARTITION_BY (k), "
        "WRITE_PARTITION_COLUMNS true, FILE_EXTENSION 'parquet')"
    )
    diff = f"SELECT * FROM t EXCEPT ALL SELECT * FROM read_tparquet('{tmp_path}/w/**/*.parquet')"
    assert rows(con, f"SELECT count(*) FROM ({diff})") == [(0,)]


def test_per_thread_output_needs_a_file_extension(con, tmp_path):
    con.execute("SET threads = 4")
    q = "SELECT i AS id FROM range(100000) r(i)"
    con.execute(f"COPY ({q}) TO '{tmp_path}/t' (FORMAT tparquet, COLUMN_NAMES ['id'], PER_THREAD_OUTPUT true)")
    names = [p.name for p in (tmp_path / "t").iterdir()]
    if API == "v1":
        # the C API v1 has no way to give a COPY function its extension: the files end in "."
        assert names and all(n.endswith(".") for n in names), names
    else:
        # the C API v2 build sets the extension of the format
        assert names and all(n.endswith(".parquet") for n in names), names
        assert rows(con, f"SELECT count(*), sum(id) FROM read_tparquet('{tmp_path}/t/*.parquet')") == [
            (100000, 4999950000)
        ]
    con.execute(
        f"COPY ({q}) TO '{tmp_path}/e' (FORMAT tparquet, COLUMN_NAMES ['id'], PER_THREAD_OUTPUT true, "
        "FILE_EXTENSION 'parquet')"
    )
    assert rows(con, f"SELECT count(*), sum(id) FROM read_tparquet('{tmp_path}/e/*.parquet')") == [
        (100000, 4999950000)
    ]


def test_partitioned_writes_name_files_by_the_format_in_v2(con, tmp_path):
    con.execute("CREATE TABLE t AS SELECT i AS id, i % 3 AS k, 'v' || i AS s FROM range(9000) r(i)")
    names = "" if API == "v2" else ", COLUMN_NAMES ['id', 's']"
    con.execute(f"COPY t TO '{tmp_path}/p' (FORMAT tparquet{names}, PARTITION_BY (k))")
    files = sorted(p.relative_to(tmp_path / "p").as_posix() for p in (tmp_path / "p").rglob("*") if p.is_file())
    assert [f.split("/")[0] for f in files] == ["k=0", "k=1", "k=2"]
    if API == "v1":
        assert all(f.endswith(".") for f in files), files
        return
    assert all(f.endswith(".parquet") for f in files), files
    # the names come from the table, without the partition column
    got = rows(
        con,
        f"SELECT regexp_extract(filename, 'k=(\\d)', 1)::BIGINT AS k, count(*), sum(id), min(s) "
        f"FROM read_tparquet('{tmp_path}/p/*/*.parquet', filename := true) GROUP BY ALL ORDER BY 1",
    )
    assert got == rows(con, "SELECT k, count(*), sum(id), min(s) FROM t GROUP BY ALL ORDER BY 1")
    con.execute(f"COPY t TO '{tmp_path}/w' (FORMAT tparquet, PARTITION_BY (k), WRITE_PARTITION_COLUMNS true)")
    assert names_of(con, f"SELECT * FROM read_tparquet('{tmp_path}/w/*/*.parquet')") == ["id", "k", "s"]
    diff = f"SELECT * FROM t EXCEPT ALL SELECT * FROM read_tparquet('{tmp_path}/w/**/*.parquet')"
    assert rows(con, f"SELECT count(*) FROM ({diff})") == [(0,)]


def test_options_duckdb_does_not_implement_for_the_format(con, tmp_path):
    with pytest.raises(duckdb.Error, match='FILE_SIZE_BYTES not implemented for "tparquet"'):
        con.execute(f"COPY (SELECT 1 AS a) TO '{tmp_path}/x' (FORMAT tparquet, COLUMN_NAMES ['a'], FILE_SIZE_BYTES 1000)")


def test_a_single_row_and_a_single_column(con, tmp_path):
    p = copy(con, "SELECT 1.5::DOUBLE AS x", tmp_path / "one.parquet", POLICY="'pcodec'")
    assert rows(con, f"SELECT * FROM read_tparquet('{p}')") == [(1.5,)]
    assert codecs(p) == {"x": {"PCODEC"}}


@pytest.mark.parametrize("rows_per_group", [1, 2047, 2048, 2049, 4096, 10000, 10001])
def test_row_group_rows_cuts_row_groups_at_any_size(con, tmp_path, rows_per_group):
    n = 10000
    p = copy(con, f"SELECT i AS id, i::VARCHAR AS s FROM range({n}) r(i)", tmp_path / "rg.parquet",
             ROW_GROUP_ROWS=rows_per_group)
    info = inspect(p)
    assert info["row_groups"] == -(-n // rows_per_group)
    meta = pq.ParquetFile(p).metadata
    assert [meta.row_group(i).num_rows for i in range(meta.num_row_groups)][:-1] == [rows_per_group] * (
        meta.num_row_groups - 1
    )
    con.execute("SET threads = 4")
    got = rows(con, f"SELECT count(*), count(DISTINCT id), sum(id) FROM read_tparquet('{p}')")
    assert got == [(n, n, n * (n - 1) // 2)]
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{p}', file_row_number := true) WHERE id <> file_row_number") == [(0,)]


def test_row_groups_larger_than_a_vector_keep_their_order(con, tmp_path):
    con.execute("SET threads = 1")
    p = copy(con, "SELECT i AS id FROM range(300000) r(i)", tmp_path / "big.parquet", ROW_GROUP_ROWS=150001)
    assert inspect(p)["row_groups"] == 2
    got = [r[0] for r in rows(con, f"SELECT id FROM read_tparquet('{p}')")]
    assert got == list(range(300000))


def test_very_long_strings(con, tmp_path):
    q = "SELECT i AS id, repeat(chr(65 + i::INTEGER), 4000000 + i::INTEGER) AS s, repeat('\\x00\\xff', 1000000)::BLOB AS b FROM range(6) r(i)"
    p = copy(con, q, tmp_path / "long.parquet")
    assert rows(con, f"SELECT id, length(s), s[1], octet_length(b) FROM read_tparquet('{p}') ORDER BY id") == [
        (i, 4000000 + i, chr(65 + i), 2000000) for i in range(6)
    ]
    assert rows(con, f"SELECT count(*) FROM (({q}) EXCEPT ALL SELECT * FROM read_tparquet('{p}'))") == [(0,)]


# ---------------------------------------------------------------------------------------------
# options


def test_option_values_may_be_strings_or_numbers(con, tmp_path):
    q = "SELECT i AS a, i * 2 AS b FROM range(5000) r(i)"
    p = copy(con, q, tmp_path / "s.parquet", LEVEL="'3'", ROW_GROUP_ROWS="'1000'", POLICY="'PCODEC'", COLUMN_POLICY="'b=BASE'")
    assert inspect(p)["row_groups"] == 5
    assert codecs(p) == {"a": {"PCODEC"}, "b": {"ZSTD"}}
    # a column policy names the column exactly as COLUMN_NAMES does
    with pytest.raises(duckdb.Error, match='no column named "A"'):
        copy(con, q, tmp_path / "t.parquet", COLUMN_POLICY="['A=pcodec']")


@pytest.mark.parametrize(
    "options, message",
    [
        ("LEVEL 5.5", 'LEVEL must be an integer, not "5.5"'),
        ("LEVEL -1", "invalid LEVEL -1: expected 0 to 12"),
        ("LEVEL 'high'", 'LEVEL must be an integer, not "high"'),
        ("POLICY 1", "POLICY must be a string, not Int\\(1\\)"),
        ("POLICY 'PCO'", 'unknown policy "pco": expected auto, pcodec or base'),
        ("ROW_GROUP_ROWS 0", "ROW_GROUP_ROWS must be a positive number of rows"),
        ("ROW_GROUP_ROWS -3", "ROW_GROUP_ROWS must be a positive number of rows"),
        ("ROW_GROUP_ROWS 'many'", 'ROW_GROUP_ROWS must be an integer, not "many"'),
        ("COLUMN_POLICY ['a']", "COLUMN_POLICY: expected NAME=policy"),
        ("COLUMN_POLICY ['a=fast']", "COLUMN_POLICY: "),
        ("COLUMN_POLICY [1]", "COLUMN_POLICY must be a string"),
        ("COMPRESSION 'zstd', COMPRESSION_LEVEL 99", "COPY \\(FORMAT tparquet\\): "),
        ("COMPRESSION 'gzip', COMPRESSION_LEVEL 10", "COPY \\(FORMAT tparquet\\): "),
        ("COMPRESSION 'brotli', COMPRESSION_LEVEL 12", "COPY \\(FORMAT tparquet\\): "),
        ("COMPRESSION 'lz4', COMPRESSION_LEVEL 1", "COMPRESSION 'lz4' takes no COMPRESSION_LEVEL"),
        ("COMPRESSION 'uncompressed', COMPRESSION_LEVEL 1", "COMPRESSION 'uncompressed' takes no COMPRESSION_LEVEL"),
        ("COMPRESSION_LEVEL 'x'", 'COMPRESSION_LEVEL must be an integer, not "x"'),
        ("COMPRESSION 3", "COMPRESSION must be a string"),
        ("COLUMN_NAMES ['a', '']", "COLUMN_NAMES: name 2 is empty"),
        ("COLUMN_NAMES 'a'", "COLUMN_NAMES has 1 names but the query has 2 columns"),
    ],
)
def test_invalid_option_values_are_explained(con, tmp_path, options, message):
    names = "" if "COLUMN_NAMES" in options else "COLUMN_NAMES ['a', 'b'], "
    with pytest.raises(duckdb.Error, match=message):
        con.execute(f"COPY (SELECT 1 AS a, 2 AS b) TO '{tmp_path}/x.parquet' (FORMAT tparquet, {names}{options})")
    assert not (tmp_path / "x.parquet").exists()
    assert rows(con, "SELECT 1") == [(1,)]


def test_a_column_policy_for_a_column_the_query_does_not_have(con, tmp_path):
    with pytest.raises(duckdb.Error, match='no column named "c"'):
        copy(con, "SELECT 1 AS a, 2 AS b", tmp_path / "x.parquet", COLUMN_POLICY="['c=pcodec']")


def test_a_column_policy_on_a_column_pcodec_does_not_apply_to(con, tmp_path):
    p = copy(con, "SELECT 'x' || i AS s, i AS n FROM range(1000) r(i)", tmp_path / "x.parquet",
             COLUMN_POLICY="['s=pcodec']", POLICY="'base'")
    assert codecs(p) == {"s": {"ZSTD"}, "n": {"ZSTD"}}


@pytest.mark.parametrize("codec, name", [("'zstd'", "ZSTD"), ("'snappy'", "SNAPPY"), ("'gzip'", "GZIP"),
                                         ("'brotli'", "BROTLI"), ("'lz4'", "LZ4_RAW"), ("'uncompressed'", "UNCOMPRESSED")])
def test_every_base_codec_reads_back(con, tmp_path, codec, name):
    q = "SELECT i AS a, 'x' || (i % 10) AS s, i * 0.5 AS f FROM range(20000) r(i)"
    p = copy(con, q, tmp_path / "c.parquet", COMPRESSION=codec, POLICY="'base'")
    assert codecs(p) == {"a": {name}, "s": {name}, "f": {name}}
    assert rows(con, f"SELECT count(*) FROM (({q}) EXCEPT ALL SELECT * FROM read_tparquet('{p}'))") == [(0,)]
    # and DuckDB's own reader reads the standard file
    assert rows(con, f"SELECT count(*) FROM read_parquet('{p}')") == [(20000,)]


@pytest.mark.parametrize("level", [0, 1, 8, 12])
def test_every_pcodec_level_reads_back(con, tmp_path, level):
    q = "SELECT i AS a, (100 + (i % 977) * 0.01)::DOUBLE AS price, (i * 0.5)::FLOAT AS f FROM range(20000) r(i)"
    p = copy(con, q, tmp_path / "l.parquet", LEVEL=level, POLICY="'pcodec'")
    assert codecs(p) == {"a": {"PCODEC"}, "price": {"PCODEC"}, "f": {"PCODEC"}}
    assert rows(con, f"SELECT count(*) FROM (({q}) EXCEPT ALL SELECT * FROM read_tparquet('{p}'))") == [(0,)]


def test_a_copy_with_many_threads_keeps_the_query_order(con, tmp_path):
    con.execute("SET threads = 8")
    con.execute("CREATE TABLE src AS SELECT (i * 7919) % 1000003 AS v FROM range(1000000) r(i)")
    p = copy(con, "SELECT * FROM src", tmp_path / "o.parquet")
    a = rows(con, f"SELECT v FROM read_tparquet('{p}', file_row_number := true) ORDER BY file_row_number")
    assert a == rows(con, "SELECT v FROM src")


def test_a_copy_of_a_parallel_scan_keeps_the_file_order(con, tmp_path):
    src = copy(con, "SELECT (i * 7919) % 1000003 AS v, i AS id FROM range(1000000) r(i)", tmp_path / "src.parquet",
               ROW_GROUP_ROWS=10000)
    assert inspect(src)["row_groups"] == 100
    con.execute("SET threads = 8")
    out = copy(con, f"SELECT v, id FROM read_tparquet('{src}')", tmp_path / "out.parquet")
    got = rows(con, f"SELECT count(*) FROM read_tparquet('{out}', file_row_number := true) WHERE id <> file_row_number")
    assert got == [(0,)]
    assert rows(con, f"SELECT count(*), sum(v) FROM read_tparquet('{out}')") == rows(
        con, f"SELECT count(*), sum(v) FROM read_tparquet('{src}')"
    )


# ---------------------------------------------------------------------------------------------
# COPY FROM


def test_copy_from_with_a_column_list_a_glob_and_errors(con, tmp_path):
    copy(con, "SELECT i AS id, 'v' || i AS s FROM range(100) r(i)", tmp_path / "a.parquet")
    copy(con, "SELECT i + 100 AS id, 'w' AS s FROM range(50) r(i)", tmp_path / "b.parquet")
    con.execute("CREATE TABLE t (id BIGINT, s VARCHAR, extra INTEGER DEFAULT 7)")
    con.execute(f"COPY t (id, s) FROM '{tmp_path}/*.parquet' (FORMAT tparquet)")
    assert rows(con, "SELECT count(*), sum(id), min(extra), max(extra) FROM t") == [(150, 11175, 7, 7)]
    con.execute("CREATE TABLE swapped (s VARCHAR, id BIGINT)")
    with pytest.raises(duckdb.Error, match=r"the file columns \(BIGINT, VARCHAR\) do not match the table columns \(VARCHAR, BIGINT\)"):
        con.execute(f"COPY swapped FROM '{tmp_path}/a.parquet' (FORMAT tparquet)")
    with pytest.raises(duckdb.Error, match="no such file"):
        con.execute(f"COPY t FROM '{tmp_path}/missing.parquet' (FORMAT tparquet)")
    # INSERT ... SELECT maps by name instead
    con.execute(f"INSERT INTO swapped SELECT s, id FROM read_tparquet('{tmp_path}/a.parquet')")
    assert rows(con, "SELECT count(*) FROM swapped") == [(100,)]


def test_copy_from_takes_the_flags_of_read_tparquet(con, tmp_path):
    copy(con, "SELECT 1::BIGINT AS id, 'x' AS s", tmp_path / "a.parquet")
    con.execute("CREATE TABLE f (id BIGINT, s VARCHAR, filename VARCHAR)")
    con.execute(f"COPY f FROM '{tmp_path}/a.parquet' (FORMAT tparquet, FILENAME true)")
    assert rows(con, "SELECT * FROM f") == [(1, "x", f"{tmp_path}/a.parquet")]
    con.execute("CREATE TABLE t (id BIGINT, s VARCHAR)")
    con.execute(f"COPY t FROM '{tmp_path}/a.parquet' (FORMAT tparquet, UNION_BY_NAME true)")
    assert rows(con, "SELECT * FROM t") == [(1, "x")]
    with pytest.raises(duckdb.Error, match="'delim' is not a supported option for copy function"):
        con.execute(f"COPY t FROM '{tmp_path}/a.parquet' (FORMAT tparquet, DELIM ',')")
    with pytest.raises(duckdb.Error, match="read_tparquet: filename must be a boolean"):
        con.execute(f"COPY t FROM '{tmp_path}/a.parquet' (FORMAT tparquet, FILENAME 1)")
