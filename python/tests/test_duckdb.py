"""tparquet.duckdb: PCODEC files queried from DuckDB-Python through the Arrow stream."""

from __future__ import annotations

import decimal
import io
import pathlib
import subprocess
import sys
import textwrap
from collections.abc import Callable, Iterator

import duckdb
import pyarrow as pa
import pytest

import tparquet as ppc
import tparquet.duckdb as pdd
from conftest import TPARQUET_FILES, vector


@pytest.fixture
def con() -> Iterator[duckdb.DuckDBPyConnection]:
    c = duckdb.connect()
    yield c
    c.close()


@pytest.fixture
def trades(tmp_path: pathlib.Path) -> pathlib.Path:
    n = 1000
    t = pa.table(
        {
            "ts": pa.array([1_735_830_000_000 + i * 250 for i in range(n)], pa.timestamp("ms", tz="UTC")),
            "sym": [f"S{i % 3}" for i in range(n)],
            "price": [round(100 + (i % 41) * 0.01, 2) for i in range(n)],
            "size": pa.array([(i % 13 + 1) * 100 for i in range(n)], pa.int32()),
        }
    )
    path = tmp_path / "trades.parquet"
    ppc.write_table(t, path, policy="pcodec", row_group_size=300)
    return path


def test_register_and_query_repeatedly(con: duckdb.DuckDBPyConnection, trades: pathlib.Path) -> None:
    assert pdd.register(con, "trades", trades) is con
    count, total = con.sql("select count(*), sum(size) from trades").fetchall()[0]
    assert count == 1000
    assert total == sum((i % 13 + 1) * 100 for i in range(1000))
    # a registered file can be scanned any number of times
    for _ in range(3):
        assert con.sql("select count(*) from trades where sym = 'S1'").fetchall()[0] == (333,)
    by_sym = con.sql("select sym, max(price) from trades group by sym order by sym").fetchall()
    assert [s for s, _ in by_sym] == ["S0", "S1", "S2"]


def test_same_rows_as_duckdb_reading_a_standard_copy(
    con: duckdb.DuckDBPyConnection, trades: pathlib.Path, tmp_path: pathlib.Path
) -> None:
    std = tmp_path / "std.parquet"
    ppc.transcode(trades, std, target="standard")
    pdd.register(con, "t", trades)
    ours = con.sql("select * from t order by ts").fetchall()
    theirs = con.sql(f"select * from read_parquet('{std}') order by ts").fetchall()
    assert ours == theirs


@pytest.mark.parametrize("name", TPARQUET_FILES)
def test_conformance_files(con: duckdb.DuckDBPyConnection, name: str, tmp_path: pathlib.Path) -> None:
    # every vector reads in DuckDB as DuckDB reads its standard transcoding
    std = tmp_path / "std.parquet"
    ppc.transcode(vector(name), std, target="standard")
    pdd.register(con, "v", vector(name))
    ours = con.sql("select * from v").fetchall()
    theirs = con.sql(f"select * from read_parquet('{std}')").fetchall()
    assert repr(ours) == repr(theirs)


def test_register_columns_and_join(con: duckdb.DuckDBPyConnection, trades: pathlib.Path) -> None:
    pdd.register(con, "a", trades, columns=["sym", "size"])
    pdd.register(con, "b", trades, columns=["price"])
    assert [d[0] for d in con.sql("select * from a").description] == ["sym", "size"]
    assert con.sql("select count(*) from a, (select max(price) m from b) where size > 1000").fetchall()[0] == (
        sum(1 for i in range(1000) if (i % 13 + 1) * 100 > 1000),
    )


def test_sql_view(con: duckdb.DuckDBPyConnection, trades: pathlib.Path) -> None:
    pdd.sql_view(con, "trades_v", trades)
    assert con.sql("select view_name from duckdb_views() where view_name = 'trades_v'").fetchall()[0] == ("trades_v",)
    con.sql("create temp view big as select * from trades_v where size >= 1200")
    assert con.sql("select count(*) from big").fetchall()[0] == (sum(1 for i in range(1000) if i % 13 + 1 >= 12),)
    # replacing the view points it at another file
    pdd.sql_view(con, "trades_v", trades, columns=["sym"])
    assert [d[0] for d in con.sql("select * from trades_v").description] == ["sym"]


def test_sql_view_names_are_quoted(con: duckdb.DuckDBPyConnection, trades: pathlib.Path) -> None:
    pdd.sql_view(con, 'odd "name"', trades, columns=["size"])
    assert con.sql('select count(*) from "odd ""name"""').fetchall() == [(1000,)]


def test_read_parquet_uses_the_connection_and_columns(trades: pathlib.Path) -> None:
    con = duckdb.connect()
    rel = pdd.read_parquet(trades, connection=con, columns=["size", "sym"])
    assert rel.columns == ["size", "sym"]
    con.close()
    with pytest.raises(duckdb.ConnectionException):
        rel.fetchall()


def test_read_parquet_relation(trades: pathlib.Path, con: duckdb.DuckDBPyConnection) -> None:
    rel = pdd.read_parquet(trades, connection=con)
    assert rel.columns == ["ts", "sym", "price", "size"]
    assert rel.filter("sym = 'S2'").count("*").fetchall()[0] == (333,)
    assert rel.aggregate("count(*)").fetchall()[0] == (1000,)
    default = pdd.read_parquet(trades)
    assert default.aggregate("min(size)").fetchall()[0] == (100,)


def test_file_objects(con: duckdb.DuckDBPyConnection, trades: pathlib.Path) -> None:
    pdd.register(con, "f", io.BytesIO(trades.read_bytes()))
    assert con.sql("select count(*) from f").fetchall()[0] == (1000,)


def test_source_object(trades: pathlib.Path) -> None:
    src = pdd.ArrowSource(trades, columns=["size"], batch_size=128)
    assert pa.schema(src) == pa.schema([pa.field("size", pa.int32())])  # type: ignore[arg-type]
    first = pa.RecordBatchReader.from_stream(src).read_all()
    second = pa.RecordBatchReader.from_stream(src).read_all()
    assert first.equals(second)
    assert first.num_rows == 1000
    assert max(b.num_rows for b in first.to_batches()) == 128
    assert "ArrowSource" in repr(src)
    # the requested schema reaches the stream
    same = pa.RecordBatchReader.from_stream(src, schema=pa.schema([pa.field("size", pa.int32())]))
    assert same.read_all().num_rows == 1000
    with pytest.raises(ppc.PcodecError, match="cast"):
        pa.RecordBatchReader.from_stream(src, schema=pa.schema([pa.field("size", pa.int64())]))


def test_works_without_pyarrow(trades: pathlib.Path) -> None:
    code = textwrap.dedent(
        f"""
        import sys
        sys.modules["pyarrow"] = None  # importing pyarrow now fails
        import duckdb
        import tparquet.duckdb as pdd
        con = duckdb.connect()
        pdd.register(con, "t", {str(trades)!r})
        print(con.sql("select count(*), min(size) from t").fetchall()[0])
        con.close()
        """
    )
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, check=True)
    assert out.stdout.strip() == "(1000, 100)"


def test_types_as_duckdb_reads_them(con: duckdb.DuckDBPyConnection, tmp_path: pathlib.Path) -> None:
    # nested data, zones, decimals, nulls at every level: as DuckDB reads the standard copy
    t = pa.table(
        {
            "i8": pa.array([1, None, -3], pa.int8()),
            "u64": pa.array([2**64 - 1, 0, None], pa.uint64()),
            "f": pa.array([float("nan"), -0.0, None]),
            "d": pa.array([0, None, 20_000], pa.date32()),
            "t": pa.array([0, None, 2**60], pa.timestamp("ns", "Asia/Tokyo")),
            "dec": pa.array([None, decimal.Decimal("1.5"), decimal.Decimal("-2.125")], pa.decimal128(18, 3)),
            "l": pa.array([[1, None], None, []], pa.list_(pa.int64())),
            "s": pa.array([{"a": 1, "b": "x"}, None, {"a": None, "b": None}]),
            "m": pa.array([[("k", 1.5)], [], None], pa.map_(pa.string(), pa.float64())),
            "str": pa.array(["é", None, ""]),
            "bin": pa.array([b"\x00", None, b""]),
        }
    )
    ours, std = tmp_path / "ours.parquet", tmp_path / "std.parquet"
    ppc.write_table(t, ours, policy="pcodec")
    ppc.transcode(ours, std, target="standard")
    pdd.register(con, "ours", ours)
    assert repr(con.sql("select * from ours").fetchall()) == repr(
        con.sql(f"select * from read_parquet('{std}')").fetchall()
    )
    assert con.sql("select count(*) from ours where l is not null and len(l) = 2").fetchall() == [(1,)]


def test_queries_push_nothing_down_but_are_right(con: duckdb.DuckDBPyConnection, trades: pathlib.Path) -> None:
    pdd.register(con, "t", trades)
    assert con.sql("select sym, count(*), round(sum(price), 2) from t group by sym order by sym").fetchall() == [
        ("S0", 334, round(sum(round(100 + (i % 41) * 0.01, 2) for i in range(0, 1000, 3)), 2)),
        ("S1", 333, round(sum(round(100 + (i % 41) * 0.01, 2) for i in range(1, 1000, 3)), 2)),
        ("S2", 333, round(sum(round(100 + (i % 41) * 0.01, 2) for i in range(2, 1000, 3)), 2)),
    ]
    assert con.sql("select size from t limit 3").fetchall() == [(100,), (200,), (300,)]
    assert con.sql("select count(*) from t where ts > (select min(ts) from t)").fetchall() == [(999,)]


def test_many_connections_and_threads(trades: pathlib.Path) -> None:
    from concurrent.futures import ThreadPoolExecutor

    source = pdd.ArrowSource(trades)

    def query(i: int) -> int:
        con = duckdb.connect()
        try:
            con.register("t", source)
            ((count,),) = con.sql(f"select count(*) from t where size > {i * 100}").fetchall()
            return int(count)
        finally:
            con.close()

    with ThreadPoolExecutor(8) as pool:
        counts = list(pool.map(query, range(14)))
    assert counts == [sum(1 for j in range(1000) if (j % 13 + 1) * 100 > i * 100) for i in range(14)]


def test_errors_reach_duckdb(con: duckdb.DuckDBPyConnection, tmp_path: pathlib.Path, trades: pathlib.Path) -> None:
    data = bytearray(trades.read_bytes())
    for i in range(100, 300):
        data[i] ^= 0x5A
    bad = tmp_path / "bad.parquet"
    bad.write_bytes(bytes(data))
    pdd.register(con, "bad", bad)
    with pytest.raises(duckdb.Error):
        con.sql("select * from bad").fetchall()
    with pytest.raises(FileNotFoundError):
        pdd.register(con, "missing", tmp_path / "missing.parquet")
    # columns are checked when the source is made, not when DuckDB first reads it
    makers: list[Callable[[], object]] = [
        lambda: pdd.register(con, "x", trades, columns=["nope"]),
        lambda: pdd.sql_view(con, "x", trades, columns=["nope"]),
        lambda: pdd.read_parquet(trades, connection=con, columns=["nope"]),
        lambda: pdd.ArrowSource(trades, columns=["nope"]),
    ]
    for make in makers:
        with pytest.raises(ppc.PcodecError, match=r"^no column named 'nope'$"):
            make()
