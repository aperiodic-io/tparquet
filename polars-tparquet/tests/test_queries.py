"""Queries over scans: each gives what the same query gives over the frame in memory."""

from __future__ import annotations

import pathlib
from collections.abc import Callable
from typing import Literal

import polars as pl
import pytest
import tparquet
from polars.testing import assert_frame_equal

import polars_tparquet as pt
from conftest import VECTORS, Reads, ticks

Write = Callable[..., pathlib.Path]
Query = Callable[[pl.LazyFrame], pl.LazyFrame]

T0 = 1_700_000_000_000

#: Queries that every engine runs over a scan of three files exactly as over the frame.
QUERIES: dict[str, Query] = {
    "all": lambda lf: lf,
    "head": lambda lf: lf.head(1234),
    "head(0)": lambda lf: lf.head(0),
    "tail": lambda lf: lf.tail(77),
    "tail(0)": lambda lf: lf.tail(0),
    "slice": lambda lf: lf.slice(990, 25),
    "slice past the end": lambda lf: lf.slice(2990, 100),
    "negative slice": lambda lf: lf.slice(-1500, 20),
    "first": lambda lf: lf.first(),
    "last": lambda lf: lf.last(),
    "select": lambda lf: lf.select("sym", "ts"),
    "select an expression": lambda lf: lf.select(n=pl.col("qty") * 2, p=pl.col("price").round(1)),
    "len": lambda lf: lf.select(pl.len()),
    "count": lambda lf: lf.select(pl.col("qty").count(), pl.col("qty").null_count().alias("nulls")),
    "a literal": lambda lf: lf.select(pl.lit(1)),
    "is_null": lambda lf: lf.filter(pl.col("qty").is_null()),
    "is_not_null": lambda lf: lf.filter(pl.col("qty").is_not_null()).select("qty"),
    "eq_missing null": lambda lf: lf.filter(pl.col("qty").eq_missing(None)),
    "string eq": lambda lf: lf.filter(pl.col("sym") == "ETH"),
    "string ne": lambda lf: lf.filter(pl.col("sym") != "ETH").select("sym"),
    "string range": lambda lf: lf.filter(pl.col("sym") > "C"),
    "starts_with": lambda lf: lf.filter(pl.col("sym").str.starts_with("S")),
    "contains": lambda lf: lf.filter(pl.col("sym").str.contains("T")),
    "is_in": lambda lf: lf.filter(pl.col("sym").is_in(["BTC", "SOL"])),
    "int range": lambda lf: lf.filter(pl.col("ts").is_between(T0 + 250 * 999, T0 + 250 * 1001)),
    "int lt": lambda lf: lf.filter(pl.col("ts") < T0 + 250 * 5),
    # the literal on the left: the pruner flips the comparison
    "int gt literal first": lambda lf: lf.filter(T0 + 250 * 2995 < pl.col("ts")),  # noqa: SIM300
    "int eq": lambda lf: lf.filter(pl.col("ts") == T0 + 250 * 2000),
    "int ne": lambda lf: lf.filter(pl.col("ts") != T0),
    "int is_in": lambda lf: lf.filter(pl.col("ts").is_in([T0, T0 + 250 * 2999])),
    "float": lambda lf: lf.filter(pl.col("price") > 104.5),
    "or": lambda lf: lf.filter((pl.col("ts") < T0 + 250 * 3) | (pl.col("ts") > T0 + 250 * 2996)),
    "and of mixed": lambda lf: lf.filter((pl.col("ts") > T0 + 250 * 1500) & (pl.col("sym") == "BTC")),
    "not": lambda lf: lf.filter(~(pl.col("ts") > T0 + 250 * 10)),
    "nothing matches": lambda lf: lf.filter(pl.col("ts") < 0),
    "filter then head": lambda lf: lf.filter(pl.col("qty") > 40).head(10),
    "head then filter": lambda lf: lf.head(100).filter(pl.col("qty") > 40),
    "filter then tail": lambda lf: lf.filter(pl.col("qty") > 40).tail(10),
    "filter then slice": lambda lf: lf.filter(pl.col("sym") == "SOL").slice(500, 7),
    "filter on a column not selected": lambda lf: lf.filter(pl.col("qty") > 45).select("ts"),
    "with_row_index": lambda lf: lf.with_row_index(),
    "with_row_index then filter": lambda lf: lf.with_row_index("i", offset=10).filter(pl.col("qty") == 3),
    # sums of floats depend on the order of the additions: these aggregations do not
    "group_by": lambda lf: lf.group_by("sym").agg(pl.col("qty").sum(), pl.col("price").max(), pl.len()).sort("sym"),
    "sort": lambda lf: lf.sort("price", "ts", descending=[True, False]),
    "unique": lambda lf: lf.select("sym").unique(maintain_order=True),
    "with_columns": lambda lf: lf.with_columns(notional=pl.col("price") * pl.col("qty")),
    "rename": lambda lf: lf.rename({"ts": "time"}).filter(pl.col("time") > T0 + 250 * 2990),
    "cast": lambda lf: lf.with_columns(pl.col("ts").cast(pl.Datetime("ms"))).filter(pl.col("ts").dt.second() == 0),
    "window": lambda lf: lf.with_columns(prev=pl.col("price").shift(1).over("sym")),
    "self join": lambda lf: lf.join(lf.select("ts", q2=pl.col("qty")), on="ts", maintain_order="left").filter(
        pl.col("q2") > 40
    ),
    "concat with itself": lambda lf: pl.concat([lf.head(3), lf.tail(3)]),
    "cache": lambda lf: lf.cache().filter(pl.col("qty") > 49),
}


@pytest.fixture(scope="module")
def three_files(tmp_path_factory: pytest.TempPathFactory) -> tuple[list[pathlib.Path], pl.DataFrame]:
    """Three files of 1000 rows each, in row groups of 128, and the frame they hold."""
    folder = tmp_path_factory.mktemp("three")
    df = ticks(3000)
    paths = []
    for k in range(3):
        paths.append(folder / f"{k}.parquet")
        df.slice(1000 * k, 1000).tparquet.write(paths[-1], row_group_size=128)  # type: ignore[attr-defined]
    return paths, df


@pytest.mark.parametrize("engine", ["in-memory", "streaming"])
@pytest.mark.parametrize("query", QUERIES.values(), ids=QUERIES.keys())
def test_a_query_over_a_scan_gives_what_it_gives_in_memory(
    three_files: tuple[list[pathlib.Path], pl.DataFrame], query: Query, engine: Literal["in-memory", "streaming"]
) -> None:
    paths, df = three_files
    got = query(pt.scan_tparquet(paths)).collect(engine=engine)
    want = query(df.lazy()).collect(engine=engine)
    assert_frame_equal(got, want, check_exact=True)


def test_a_join_of_two_scans(write: Write) -> None:
    trades = write("trades.parquet", ticks(500), row_group_size=64)
    quotes = write("quotes.parquet", ticks(800).select("ts", bid=pl.col("price") - 0.01), row_group_size=100)
    lf = pt.scan_tparquet(trades).join(pt.scan_tparquet(quotes), on="ts", how="left", maintain_order="left")
    lf = lf.filter(pl.col("qty") > 10)
    want = ticks(500).join(ticks(800).select("ts", bid=pl.col("price") - 0.01), on="ts", how="left")
    assert_frame_equal(lf.collect(), want.filter(pl.col("qty") > 10))
    asof = pt.scan_tparquet(trades).join_asof(pt.scan_tparquet(quotes), on="ts").collect()
    assert asof.height == 500
    assert asof["bid"].null_count() == 0


def test_collect_twice_reads_twice(write: Write, reads: Reads) -> None:
    lf = pt.scan_tparquet([write("a.parquet", ticks(10)), write("b.parquet", ticks(10))]).filter(pl.col("qty") > 3)
    first, second = lf.collect(), lf.collect()
    assert_frame_equal(first, second)
    assert len(reads.calls) == 4


def test_explain_reads_nothing(write: Write, reads: Reads) -> None:
    lf = pt.scan_tparquet([write("a.parquet", ticks(10)), write("b.parquet", ticks(10))])
    plan = lf.filter(pl.col("ts") > 0).select("ts").explain()
    assert "PYTHON SCAN" in plan
    assert lf.explain(optimized=False)
    assert reads.calls == []


def test_a_count_reads_no_column_it_does_not_need(write: Write, reads: Reads) -> None:
    lf = pt.scan_tparquet([write("a.parquet", ticks(10)), write("b.parquet", ticks(7))])
    assert lf.select(pl.len()).collect().item() == 17
    assert all(len(c) <= 1 for c in reads.columns())


def test_head_reaches_the_reader(write: Write, reads: Reads) -> None:
    pt.scan_tparquet(write("a.parquet", ticks(1000), row_group_size=100)).head(5).collect()
    assert [c["limit"] for c in reads.calls] == [5]


def test_the_path_column_alone(write: Write) -> None:
    a, b = write("a.parquet", ticks(3)), write("b.parquet", ticks(2))
    lf = pt.scan_tparquet([a, b], include_file_paths="file")
    assert lf.select("file").collect()["file"].to_list() == [str(a)] * 3 + [str(b)] * 2
    assert lf.select(pl.col("file").n_unique()).collect().item() == 2
    assert lf.filter(pl.col("file").str.ends_with("b.parquet")).select(pl.len()).collect().item() == 2
    assert lf.group_by("file", maintain_order=True).len().collect().rows() == [(str(a), 3), (str(b), 2)]
    assert lf.select("file").collect(engine="streaming")["file"].to_list() == [str(a)] * 3 + [str(b)] * 2
    assert lf.collect_schema()["file"] == pl.String


def test_collect_batches_gives_every_row(write: Write) -> None:
    paths = [write(f"{k}.parquet", ticks(700, offset=700 * k), row_group_size=100) for k in range(3)]
    lf = pt.scan_tparquet(paths, batch_size=64).filter(pl.col("qty") > 20)
    batches = list(lf.collect_batches())
    assert_frame_equal(pl.concat(batches), lf.collect())
    assert all(b.height <= 64 for b in batches)
    sized = list(lf.collect_batches(chunk_size=1000))
    assert_frame_equal(pl.concat(sized), lf.collect())


def test_empty_files_in_a_list(write: Write, tmp_path: pathlib.Path) -> None:
    empty = write("empty.parquet", ticks(0))
    assert tparquet.ParquetFile(empty).metadata.num_row_groups == 0
    a = write("a.parquet", ticks(3))
    out = pt.scan_tparquet([empty, a, empty], include_file_paths="file").collect()
    assert_frame_equal(out.drop("file"), ticks(3))
    assert out["file"].to_list() == [str(a)] * 3
    alone = pt.scan_tparquet(empty, include_file_paths="file")
    assert alone.collect().height == 0
    assert alone.collect_schema() == pl.Schema([*ticks(0).schema.items(), ("file", pl.String)])
    assert alone.select(pl.len()).collect().item() == 0
    assert alone.filter(pl.col("ts") > 0).collect(engine="streaming").height == 0
    assert alone.head(5).collect().height == 0


def test_a_row_group_without_rows() -> None:
    vector = next(p for p in VECTORS if p.stem == "empty")
    file = tparquet.ParquetFile(vector)
    assert file.metadata.num_rows == 0
    lf = pt.scan_tparquet(vector)
    assert lf.collect().height == 0
    assert lf.select(pl.len()).collect().item() == 0
    assert pt.scan_tparquet([vector, vector]).collect().schema == lf.collect_schema()


def test_required_and_optional_columns_scan_together() -> None:
    """The same Polars schema, one file with required columns, one with optional ones."""
    required, optional = (next(p for p in VECTORS if p.stem == f"primitives_{k}") for k in ("required", "optional"))
    out = pt.scan_tparquet([required, optional]).collect()
    assert_frame_equal(out, pl.concat([pt.read_tparquet(required), pt.read_tparquet(optional)]))
    assert out.null_count().row(0) != (0, 0, 0, 0)


def test_a_dictionary_column_and_a_plain_one_differ(tmp_path: pathlib.Path) -> None:
    """A dictionary-encoded string column reads as Categorical, a plain one as String."""
    mixed = next(p for p in VECTORS if p.stem == "mixed")
    plain = tmp_path / "plain.parquet"
    pt.read_tparquet(mixed).with_columns(pl.col("symbol").cast(pl.String)).tparquet.write(plain)  # type: ignore[attr-defined]
    with pytest.raises(pl.exceptions.SchemaError, match=r"plain\.parquet: schema differs from .*mixed\.parquet"):
        pt.scan_tparquet([mixed, plain])


@pytest.mark.parametrize(
    "dtype",
    [pl.Datetime("ms"), pl.Datetime("us", "UTC"), pl.Datetime("ms", "Europe/Paris"), pl.Float32, pl.Enum(["a"])],
    ids=str,
)
def test_schemas_differing_in_one_dtype_detail(write: Write, dtype: pl.DataType) -> None:
    base = pl.DataFrame({"x": pl.Series([None], dtype=pl.Datetime("ms", "UTC"))})
    a = write("a.parquet", base)
    b = write("b.parquet", base.cast({"x": dtype}))
    with pytest.raises(pl.exceptions.SchemaError, match=r"b\.parquet: schema differs"):
        pt.scan_tparquet([a, b])


def test_a_query_error_names_the_problem(write: Write) -> None:
    lf = pt.scan_tparquet(write("a.parquet", ticks(3)))
    with pytest.raises(pl.exceptions.ColumnNotFoundError, match="nope"):
        lf.select("nope").collect()
    with pytest.raises(pl.exceptions.InvalidOperationError):
        lf.select(pl.col("sym") + 1).collect()


def test_a_corrupt_page_is_named_in_the_error(write: Write, tmp_path: pathlib.Path) -> None:
    good = write("good.parquet", pl.DataFrame({"a": range(100_000)}))
    data = bytearray(good.read_bytes())
    for i in range(100, 400):  # in the first page, past its header
        data[i] ^= 0x5A
    bad = tmp_path / "bad.parquet"
    bad.write_bytes(bytes(data))
    lf = pt.scan_tparquet([good, bad])  # the footer is intact
    with pytest.raises(tparquet.PcodecError, match=r"bad\.parquet"):
        lf.collect()


@pytest.mark.parametrize("engine", ["in-memory", "streaming"])
@pytest.mark.parametrize(
    "query",
    [
        lambda lf: lf.head(3).filter(pl.col("ts") > T0),
        lambda lf: lf.slice(0, 1).filter(pl.col("ts") != T0),
        lambda lf: lf.head(5).filter(pl.col("qty").is_null()),
    ],
    ids=["head(3).filter", "slice(0, 1).filter", "head(5).filter(is_null)"],
)
def test_a_limit_before_a_filter_over_one_file(
    write: Write, query: Query, engine: Literal["in-memory", "streaming"]
) -> None:
    path = write("a.parquet", ticks(10))
    assert_frame_equal(query(pt.scan_tparquet(path)).collect(engine=engine), query(ticks(10).lazy()).collect())


def test_a_limit_before_a_filter_over_several_files(write: Write) -> None:
    """Over a union of files Polars keeps the limit above the sources: these are right."""
    paths = [write(f"{k}.parquet", ticks(10)) for k in range(2)]
    query: Query = lambda lf: lf.head(3).filter(pl.col("ts") > T0)  # noqa: E731
    assert_frame_equal(query(pt.scan_tparquet(paths)).collect(), query(pl.concat([ticks(10)] * 2).lazy()).collect())
