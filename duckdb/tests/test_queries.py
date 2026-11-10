"""Queries over read_tparquet give what the same queries over read_parquet give.

The same rows are written twice: by COPY (FORMAT tparquet) with PCODEC and many row groups, and
by DuckDB's own Parquet writer. Each query runs over both (read_tparquet of the PCODEC file,
read_parquet of the standard one) with one thread and with four; projections, filters, limits,
aggregates and joins must not change any result (rows in any order unless the query orders
them).
"""

import pytest
from canonical import rows_of
from conftest import connect, copy_to, rows
from hypothesis import given
from hypothesis import strategies as st

import duckdb

N = 50_000

TABLE = f"""
SELECT
    i AS id,
    CASE WHEN i % 11 = 0 THEN NULL ELSE (i * 7919) % 1000 END AS k,
    CASE WHEN i % 13 = 0 THEN NULL ELSE 100 + (i % 977) * 0.01 END AS price,
    CASE WHEN i % 17 = 0 THEN NULL ELSE (i % 50 + 1) * 0.001 END::FLOAT AS qty,
    CASE WHEN i % 19 = 0 THEN NULL ELSE 'sym' || (i % 23) END AS sym,
    CASE WHEN i % 29 = 0 THEN NULL ELSE i % 2 = 0 END AS buy,
    TIMESTAMP '2025-01-01' + INTERVAL (i * 37) MILLISECOND AS ts,
    CASE WHEN i % 31 = 0 THEN NULL ELSE DATE '2025-01-01' + (i % 400)::INTEGER END AS d,
    CASE WHEN i % 7 = 0 THEN NULL ELSE [i % 5, CASE WHEN i % 3 = 0 THEN NULL ELSE i END] END AS li,
    CASE WHEN i % 37 = 0 THEN NULL ELSE {{'a': i % 9, 'b': 'x' || (i % 4)}} END AS st,
    (i * 0.25)::DECIMAL(18, 2) AS amount
FROM range({N}) r(i)
"""

COLUMNS = ["id", "k", "price", "qty", "sym", "buy", "ts", "d", "li", "st", "amount"]


@pytest.fixture(scope="module")
def files(tmp_path_factory):
    d = tmp_path_factory.mktemp("queries")
    con = connect()
    con.execute(f"CREATE TABLE t AS {TABLE}")
    pcodec = copy_to(con, "SELECT * FROM t", d / "pcodec.parquet", COMPRESSION="pcodec", ROW_GROUP_SIZE=4999)
    standard = d / "standard.parquet"
    con.execute(f"COPY t TO '{standard}' (FORMAT parquet, ROW_GROUP_SIZE 10000)")
    # the same rows split over three files, for globs and lists
    for part, (lo, hi) in enumerate([(0, 20_000), (20_000, 20_001), (20_001, N)]):
        copy_to(con, f"SELECT * FROM t WHERE id >= {lo} AND id < {hi}", d / f"part-{part}.parquet", ROW_GROUP_SIZE=3000)
    con.close()
    return d, pcodec, standard


@pytest.fixture(scope="module", params=[1, 4], ids=["1-thread", "4-threads"])
def con(request, files):
    c = connect()
    c.execute(f"SET threads = {request.param}")
    # The rows read_parquet reads from the standard file, in a table: the queries run over the
    # table, not over read_parquet itself, because DuckDB 2.0.0's read_parquet fails some of them
    # with a FATAL error that invalidates the database (a filter on a struct field, ordered and
    # limited: "Vector::Reference used on vector of different type").
    c.execute(f"CREATE TABLE native AS SELECT * FROM read_parquet('{files[2]}')")
    yield c
    c.close()


def same(con, files, query, ordered=False, sources=None):
    """`query` with `{t}` read through read_tparquet and from read_parquet's rows."""
    d, pcodec, standard = files
    ours = query.format(t=sources or f"read_tparquet('{pcodec}')")
    native = query.format(t="native")
    a = rows_of(con.sql(ours).to_arrow_table())
    b = rows_of(con.sql(native).to_arrow_table())
    if not ordered:
        a, b = sorted(a), sorted(b)
    assert a == b, query
    return a


QUERIES = [
    "SELECT * FROM {t}",
    "SELECT count(*) FROM {t}",
    "SELECT count(*), count(k), count(price), count(li), count(st) FROM {t}",
    "SELECT sym, buy, price FROM {t}",
    "SELECT st, id FROM {t}",
    "SELECT li FROM {t}",
    "SELECT price, price, id FROM {t}",
    "SELECT id + 1 AS x, upper(sym) AS s FROM {t}",
    "SELECT * FROM {t} WHERE k = 7",
    "SELECT id FROM {t} WHERE k IS NULL",
    "SELECT id, price FROM {t} WHERE price BETWEEN 101 AND 102.5",
    "SELECT id FROM {t} WHERE qty > 0.025",
    "SELECT * FROM {t} WHERE sym LIKE 'sym1%' AND buy",
    "SELECT id FROM {t} WHERE sym IN ('sym3', 'sym22') OR price IS NULL",
    "SELECT id FROM {t} WHERE ts >= TIMESTAMP '2025-01-01 00:10:00' AND ts < TIMESTAMP '2025-01-01 00:20:00'",
    "SELECT id FROM {t} WHERE d = DATE '2025-02-01'",
    "SELECT id FROM {t} WHERE list_contains(li, 3)",
    "SELECT id FROM {t} WHERE st.a = 4 AND st.b = 'x0'",
    "SELECT id FROM {t} WHERE amount > 12000.00",
    "SELECT id FROM {t} WHERE id % 1000 = 999",
    "SELECT id FROM {t} WHERE false",
    "SELECT sym, count(*), sum(price), min(qty), max(ts), avg(amount) FROM {t} GROUP BY sym",
    "SELECT k, count(DISTINCT sym) FROM {t} GROUP BY ALL HAVING count(*) > 40",
    "SELECT DISTINCT sym FROM {t}",
    "SELECT buy, list_sort(list(DISTINCT st.b)) FROM {t} GROUP BY buy",
    # (DuckDB 1.5 does not bind an unnest grouped by its alias)
    "SELECT x, count(*) FROM (SELECT unnest(li) AS x FROM {t}) GROUP BY x",
    "SELECT a.id, b.id FROM {t} a JOIN {t} b ON a.k = b.k AND a.id + 1 = b.id",
    "SELECT id, sum(price) OVER (PARTITION BY sym ORDER BY id ROWS 3 PRECEDING) AS w FROM {t}",
    "SELECT count(*) FROM (SELECT DISTINCT * FROM {t})",
]

ORDERED = [
    "SELECT * FROM {t} ORDER BY id LIMIT 10",
    "SELECT * FROM {t} ORDER BY id LIMIT 10 OFFSET 49995",
    "SELECT id, sym FROM {t} ORDER BY id DESC LIMIT 3000 OFFSET 2047",
    "SELECT price, id FROM {t} ORDER BY price NULLS FIRST, id LIMIT 100",
    "SELECT * FROM {t} WHERE sym = 'sym5' ORDER BY ts LIMIT 7 OFFSET 3",
    "SELECT id FROM {t} ORDER BY id LIMIT 0",
]


@pytest.mark.parametrize("query", QUERIES)
def test_queries_match_read_parquet(con, files, query):
    same(con, files, query)


@pytest.mark.parametrize("query", ORDERED)
def test_ordered_queries_match_read_parquet(con, files, query):
    same(con, files, query, ordered=True)


@pytest.mark.parametrize("limit", [0, 1, 2047, 2048, 2049, 4999, 5000, 12345, N, N + 1])
def test_a_limit_without_order_gives_that_many_rows_of_the_file(con, files, limit):
    d, pcodec, _ = files
    got = rows(con, f"SELECT id FROM read_tparquet('{pcodec}') LIMIT {limit}")
    assert len(got) == min(limit, N)
    assert len(set(got)) == len(got)
    assert all(0 <= r[0] < N for r in got)


def test_count_star_comes_from_the_footer(con, files, tmp_path):
    d, pcodec, _ = files
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{pcodec}')") == [(N,)]
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{pcodec}', filename := true)") == [(N,)]
    # the column data is not read: a file whose pages are destroyed still counts
    damaged = tmp_path / "damaged.parquet"
    data = bytearray(pcodec.read_bytes())
    footer = int.from_bytes(data[-8:-4], "little")
    data[4 : len(data) - 8 - footer] = bytes(len(data) - 12 - footer)
    damaged.write_bytes(bytes(data))
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{damaged}')") == [(N,)]
    with pytest.raises(duckdb.Error):
        rows(con, f"SELECT sum(id) FROM read_tparquet('{damaged}')")


def test_projection_pushdown_is_in_the_plan(con, files):
    d, pcodec, _ = files
    plan = rows(con, f"EXPLAIN SELECT sym, id FROM read_tparquet('{pcodec}') WHERE price > 1")[0][1]
    scan = plan.split("Read Tparquet")[-1]
    assert "Projections:" in scan and "price" in scan and "sym" in scan and "amount" not in scan


def test_globs_and_lists_hold_the_rows_of_the_single_file(con, files):
    d, pcodec, _ = files
    for sources in [
        f"read_tparquet('{d}/part-*.parquet')",
        f"read_tparquet(['{d}/part-0.parquet', '{d}/part-[12].parquet'])",
        f"read_tparquet(['{d}/part-2.parquet', '{d}/part-1.parquet', '{d}/part-0.parquet'])",
        f"read_tparquet('{d}/part-?.parquet', union_by_name := true)",
    ]:
        same(con, files, "SELECT * FROM {t}", sources=sources)
        same(con, files, "SELECT sym, sum(price), count(*) FROM {t} GROUP BY sym", sources=sources)


def test_file_row_numbers_and_filenames_across_threads(con, files):
    d, _, _ = files
    got = rows(
        con,
        f"SELECT filename, file_row_number, id FROM read_tparquet('{d}/part-*.parquet', filename := true, "
        "file_row_number := true) ORDER BY id",
    )
    starts = {f"{d}/part-0.parquet": 0, f"{d}/part-1.parquet": 20_000, f"{d}/part-2.parquet": 20_001}
    assert len(got) == N
    assert all(starts[f] + n == i for f, n, i in got)
    # the extra columns filter and project like the others
    assert rows(
        con,
        f"SELECT count(*), min(id) FROM read_tparquet('{d}/part-*.parquet', filename := true) "
        f"WHERE filename LIKE '%part-2%'",
    ) == [(N - 20_001, 20_001)]


def test_a_scan_of_several_threads_gives_every_row_once(files):
    d, pcodec, _ = files
    for threads in [2, 3, 8]:
        c = connect()
        c.execute(f"SET threads = {threads}")
        got = [r[0] for r in rows(c, f"SELECT id FROM read_tparquet('{d}/part-*.parquet')")]
        c.close()
        assert sorted(got) == list(range(N))


def test_one_thread_keeps_the_file_order(files):
    d, pcodec, _ = files
    c = connect()
    c.execute("SET threads = 1")
    assert [r[0] for r in rows(c, f"SELECT id FROM read_tparquet('{d}/part-*.parquet')")] == list(range(N))
    c.close()


# generated queries: a projection, a conjunction of predicates and maybe an ordered limit


def predicate(column):
    return {
        "id": st.integers(-1, N).map(lambda v: f"id {'<' if v % 2 else '>='} {v}"),
        "k": st.sampled_from(["k IS NULL", "k IS NOT NULL", "k = 7", "k < 300", "k <> 0"]),
        "price": st.floats(99, 111, allow_nan=False).map(lambda v: f"price > {v!r}"),
        "qty": st.sampled_from(["qty < 0.01", "qty IS NULL", "qty >= 0.025"]),
        "sym": st.sampled_from(["sym = 'sym3'", "sym LIKE '%1%'", "sym IS NULL", "sym > 'sym15'"]),
        "buy": st.sampled_from(["buy", "NOT buy", "buy IS NULL"]),
        "ts": st.integers(0, N * 37).map(lambda ms: f"ts < TIMESTAMP '2025-01-01' + INTERVAL ({ms}) MILLISECOND"),
        "d": st.integers(0, 400).map(lambda n: f"d >= DATE '2025-01-01' + {n}"),
        "li": st.sampled_from(["len(li) = 2", "li IS NULL", "li[2] IS NULL", "list_contains(li, 0)"]),
        "st": st.sampled_from(["st.a > 4", "st IS NULL", "st.b = 'x1'"]),
        "amount": st.integers(0, N // 4).map(lambda v: f"amount <= {v}"),
    }[column]


@st.composite
def queries(draw):
    projection = draw(st.lists(st.sampled_from(COLUMNS), min_size=0, max_size=6))
    filtered = draw(st.lists(st.sampled_from(COLUMNS), max_size=3, unique=True))
    where = [draw(predicate(c)) for c in filtered]
    select = ", ".join(projection) if projection else "count(*)"
    sql = f"SELECT {select} FROM {{t}}" + (f" WHERE {' AND '.join(where)}" if where else "")
    ordered = bool(projection) and draw(st.booleans())
    if ordered:
        sql += f" ORDER BY id LIMIT {draw(st.integers(0, 3000))} OFFSET {draw(st.integers(0, N))}"
        sql = sql.replace(f"SELECT {select}", f"SELECT id, {select}")
    return sql, ordered


@given(q=queries())
def test_generated_queries_match_read_parquet(con, files, q):
    sql, ordered = q
    same(con, files, sql, ordered=ordered)
