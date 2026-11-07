"""Row-group skipping and order, on both builds.

The C API v2 build skips row groups whose statistics rule out the query's filters; the C API v1
build cannot (DuckDB's C API v1 hands table functions no filters) and reads every row group.
Skipping is observed with a damaged row group: a query whose filters rule it out reads the file
only if it is skipped. Results never depend on skipping: every query is compared with DuckDB's
own reader.
"""

import pyarrow.parquet as pq
import pytest
from hypothesis import given
from hypothesis import strategies as st

from canonical import rows_of
from conftest import API, connect, encode, rows

GROUP = 2048  # DuckDB writes row groups of whole vectors
N = 10 * GROUP
SKIPS = API == "v2"

TABLE = f"""
SELECT
    i AS id,
    i::INTEGER AS i32,
    i::UBIGINT AS u64,
    CASE WHEN i % 7 = 0 THEN NULL ELSE i * 0.5 END AS dbl,
    (i * 0.25)::FLOAT AS flt,
    'k' || lpad(i::VARCHAR, 6, '0') AS str,
    DATE '2020-01-01' + (i // 10)::INTEGER AS d,
    TIMESTAMP '2020-01-01' + INTERVAL (i) SECOND AS ts,
    (TIMESTAMP '2020-01-01' + INTERVAL (i) SECOND)::TIMESTAMP_MS AS ts_ms,
    (i * 0.01)::DECIMAL(9, 2) AS dec9,
    (i * 0.001)::DECIMAL(30, 3) AS dec30,
    CASE WHEN i // {GROUP} = 5 THEN NULL ELSE i END AS gap,
    i % 2 = 0 AS even
FROM range({N}) r(i)
"""


def _damage(path, row_group):
    """Overwrites the middle of every column chunk of `row_group` with garbage."""
    meta = pq.ParquetFile(path).metadata.row_group(row_group)
    data = bytearray(path.read_bytes())
    for c in range(meta.num_columns):
        col = meta.column(c)
        start = col.dictionary_page_offset or col.data_page_offset
        size = col.total_compressed_size
        for k in range(start + size // 4, start + size - 4):
            data[k] = 0xA5
    path.write_bytes(bytes(data))


@pytest.fixture(scope="module")
def files(tmp_path_factory):
    d = tmp_path_factory.mktemp("pushdown")
    plain = connect()
    plain.execute(f"COPY ({TABLE}) TO '{d}/standard.parquet' (FORMAT parquet, ROW_GROUP_SIZE {GROUP})")
    plain.close()
    good = encode(d / "standard.parquet", d / "pcodec.parquet")
    assert pq.ParquetFile(good).metadata.num_row_groups == 10
    bad = d / "damaged.parquet"
    bad.write_bytes(good.read_bytes())
    _damage(bad, 5)  # ids LO..HI-1
    return d, good, bad


@pytest.fixture(scope="module")
def con(files):
    c = connect()
    c.execute("SET threads = 4")
    c.execute(f"CREATE TABLE native AS SELECT * FROM read_parquet('{files[0]}/standard.parquet')")
    yield c
    c.close()


def test_the_damaged_row_group_is_read_without_filters(con, files):
    _, _, bad = files
    with pytest.raises(Exception, match="damaged.parquet"):
        rows(con, f"SELECT sum(id) FROM read_tparquet('{bad}')")


# the damaged row group holds ids LO..HI-1
LO, HI = 5 * GROUP, 6 * GROUP

# filters that rule the damaged row group out, on every type whose statistics are read
RULE_OUT = [
    f"id < {LO}",
    f"id >= {HI}",
    "id = 4321",
    f"id IN (1, 2, {N - 1})",
    f"id < 100 OR id > {N - 100}",
    f"{LO} > id",
    f"id BETWEEN {HI} AND {HI + 1000}",
    f"i32 < {LO}",
    f"u64 > {HI + 500}",
    f"dbl < {LO // 2}",
    f"flt < {LO // 4}",
    f"str < 'k{LO:06d}'",
    "str = 'k000042'",
    f"d < DATE '2020-01-01' + {LO // 10}",
    f"ts < TIMESTAMP '2020-01-01' + INTERVAL {LO - 1} SECOND",
    # a TIMESTAMP_MS column compared with a TIMESTAMP_MS constant (DuckDB casts the column, not
    # the constant, to compare with a TIMESTAMP: such filters are not read)
    f"ts_ms >= (TIMESTAMP '2020-01-01' + INTERVAL {HI} SECOND)::TIMESTAMP_MS",
    f"dec9 < {LO / 100:.2f}",
    f"dec30 >= {HI / 1000:.3f}",
    f"id < {LO} AND str LIKE 'k%'",
    "gap IS NOT NULL",
]


@pytest.mark.parametrize("where", RULE_OUT)
def test_filters_skip_row_groups_their_statistics_rule_out(con, files, where):
    _, good, bad = files
    expected = rows(con, f"SELECT count(*), sum(id) FROM native WHERE {where}")
    assert rows(con, f"SELECT count(*), sum(id) FROM read_tparquet('{good}') WHERE {where}") == expected
    query = f"SELECT count(*), sum(id) FROM read_tparquet('{bad}') WHERE {where}"
    if SKIPS:
        assert rows(con, query) == expected
    else:
        with pytest.raises(Exception, match="damaged.parquet"):
            rows(con, query)


# filters that cannot rule row group 5 out: it must be read (and fails)
KEEP = [
    f"id > {LO + 500}",
    "id <> 1",
    "gap IS NULL",
    "even",
    f"str LIKE 'k0{LO // 100}%'",
    "id % 2 = 0",
    "dbl > 1 OR even",
    f"id::VARCHAR = '{LO + 1}'",
    f"ts_ms >= TIMESTAMP '2020-01-01' + INTERVAL {HI} SECOND",
]


@pytest.mark.parametrize("where", KEEP)
def test_filters_that_cannot_rule_a_row_group_out_read_it(con, files, where):
    _, _, bad = files
    with pytest.raises(Exception, match="damaged.parquet"):
        rows(con, f"SELECT count(*), sum(id) FROM read_tparquet('{bad}') WHERE {where}")


def test_a_skipped_row_group_keeps_file_row_numbers(con, files):
    _, good, bad = files
    query = (
        "SELECT id, file_row_number FROM read_tparquet('{}', file_row_number := true) "
        f"WHERE id >= {HI + 10} AND id < {HI + 30}"
    )
    got = rows(con, query.format(good))
    assert got == [(i, i) for i in range(HI + 10, HI + 30)]
    if SKIPS:
        assert rows(con, query.format(bad)) == got


def test_filters_of_a_prepared_statement(con, files):
    _, good, bad = files
    con.execute(f"PREPARE q AS SELECT count(*) FROM read_tparquet('{good}') WHERE id < $1")
    assert rows(con, "EXECUTE q(10)") == [(10,)]
    assert rows(con, "EXECUTE q(7000)") == [(7000,)]
    con.execute("DEALLOCATE q")


def test_a_column_a_file_lacks_is_null_for_filters(con, files, tmp_path):
    d, good, _ = files
    other = tmp_path / "other.parquet"
    plain = connect()
    plain.execute(f"COPY (SELECT i AS id, i AS extra FROM range(5) r(i)) TO '{tmp_path}/o.parquet' (FORMAT parquet)")
    plain.close()
    encode(tmp_path / "o.parquet", other)
    union = f"read_tparquet(['{good}', '{other}'], union_by_name := true)"
    assert rows(con, f"SELECT count(*) FROM {union} WHERE extra = 3") == [(1,)]
    assert rows(con, f"SELECT count(*) FROM {union} WHERE extra IS NULL") == [(N,)]
    assert rows(con, f"SELECT count(*) FROM {union} WHERE str IS NULL") == [(5,)]


# generated queries: any filters, any values (NaN, infinities, negative zero, NULL), compared with
# DuckDB's own reader of the same rows

FLOATS = st.sampled_from(["'NaN'::DOUBLE", "'inf'::DOUBLE", "'-inf'::DOUBLE", "-0.0", "0.0", "1.5", "-1.5", "NULL"])


@pytest.fixture(scope="module")
def odd(tmp_path_factory):
    d = tmp_path_factory.mktemp("odd")
    values = ["'NaN'::DOUBLE", "'inf'::DOUBLE", "'-inf'::DOUBLE", "-0.0", "0.0", "1.5", "-1.5", "NULL"]
    groups = []
    for g in range(len(values)):
        picked = [values[(g + k) % len(values)] for k in range(1 + g % 3)]
        groups += [f"SELECT {g} AS g, {v} AS x, CASE WHEN {g} % 2 = 0 THEN NULL ELSE 's' || {g} END AS s" for v in picked]
    plain = connect()
    plain.execute(f"CREATE TABLE t AS {' UNION ALL '.join(groups)}")
    plain.execute(f"COPY (SELECT * FROM t ORDER BY g) TO '{d}/odd.parquet' (FORMAT parquet, ROW_GROUP_SIZE 2)")
    plain.close()
    return encode(d / "odd.parquet", d / "odd-pcodec.parquet"), d / "odd.parquet"


@given(
    op=st.sampled_from(["=", "<>", "<", "<=", ">", ">="]),
    value=FLOATS,
    flip=st.booleans(),
    extra=st.sampled_from(["", " OR s IS NULL", " AND s IS NOT NULL", " OR x IS NULL"]),
)
def test_float_filters_with_nan_and_signed_zero_match_duckdb(con, odd, op, value, flip, extra):
    ours, standard = odd
    cond = f"{value} {op} x" if flip else f"x {op} {value}"
    where = f"({cond}){extra}"
    query = "SELECT g, x, s FROM {} WHERE " + where
    a = sorted(rows_of(con.sql(query.format(f"read_tparquet('{ours}')")).to_arrow_table()))
    b = sorted(rows_of(con.sql(query.format(f"read_parquet('{standard}')")).to_arrow_table()))
    assert a == b, where


def test_threads_keep_the_file_order(files, tmp_path):
    _, good, _ = files
    paths = []
    for k in range(3):
        p = tmp_path / f"p{k}.parquet"
        p.write_bytes(good.read_bytes())
        paths.append(str(p))
    c = connect()
    c.execute("SET threads = 8")
    got = [r[0] for r in rows(c, f"SELECT id FROM read_tparquet('{tmp_path}/p*.parquet')")]
    assert got == list(range(N)) * 3
    got = [r[0] for r in rows(c, f"SELECT id FROM read_tparquet('{tmp_path}/p*.parquet') WHERE id % 3 = 0 LIMIT 20000")]
    assert got == ([i for i in range(N) if i % 3 == 0] * 3)[:20000]
    c.execute(f"CREATE TABLE t AS SELECT id FROM read_tparquet('{tmp_path}/p*.parquet')")
    assert [r[0] for r in rows(c, "SELECT id FROM t")] == list(range(N)) * 3
    c.close()
