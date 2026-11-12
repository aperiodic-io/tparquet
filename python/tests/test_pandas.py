"""tparquet.pandas: DataFrames round-trip with their index and dtypes, as with pyarrow."""

from __future__ import annotations

import decimal
import io
import pathlib

import numpy as np
import pandas as pd
import pyarrow as pa
import pyarrow.parquet as pq
import pytest

import tparquet as ppc
import tparquet.pandas as ppd


def frame() -> pd.DataFrame:
    n = 500
    idx = pd.date_range("2025-01-02 09:30", periods=n, freq="250ms", tz="America/New_York", name="ts")
    df: pd.DataFrame = pd.DataFrame(
        {
            "price": np.round(100 + np.cumsum(np.where(np.arange(n) % 3 == 0, 0.01, -0.005)), 2),
            "size": (np.arange(n) % 17 + 1) * 100,
            "side": pd.Categorical(["buy", "sell"] * (n // 2), categories=["buy", "sell"]),
            "venue": pd.Categorical(["X", "Y", "Z", "X", "Y"] * (n // 5), categories=["Z", "Y", "X"], ordered=True),
            "f32": np.arange(n, dtype=np.float32) / 4,
            "i8": (np.arange(n) % 100).astype(np.int8),
            "u64": np.arange(n, dtype=np.uint64) * 2**40,
            "flag": np.arange(n) % 2 == 0,
            "name": [f"n{i % 7}" for i in range(n)],
            "maybe": pd.array([None if i % 5 == 0 else i for i in range(n)], dtype="Int64"),
            "utc": pd.date_range("2025-01-01", periods=n, freq="s", tz="UTC"),
            "naive": pd.date_range("2025-01-01", periods=n, freq="min"),
            "delta": pd.to_timedelta(np.arange(n), unit="ms"),
        },
        index=idx,
    )
    return df


def assert_same_as_pyarrow(df: pd.DataFrame, tmp_path: pathlib.Path, **kwargs: object) -> pd.DataFrame:
    """Writes `df` with this package and with pyarrow; both read back equal."""
    ours = tmp_path / "ours.parquet"
    theirs = tmp_path / "theirs.parquet"
    index = kwargs.get("index")
    assert index is None or isinstance(index, bool)
    ppd.to_parquet(df, ours, **kwargs)  # type: ignore[arg-type]
    df.to_parquet(theirs, engine="pyarrow", index=index)
    back = ppd.read_parquet(ours)
    pd.testing.assert_frame_equal(back, pd.read_parquet(theirs, engine="pyarrow"))
    return back


def test_round_trip_keeps_index_and_dtypes(tmp_path: pathlib.Path) -> None:
    df = frame()
    back = assert_same_as_pyarrow(df, tmp_path)
    # no engine stores an index's freq
    pd.testing.assert_frame_equal(back, df, check_freq=False)
    assert isinstance(back.index, pd.DatetimeIndex)
    assert str(back.index.tz) == "America/New_York"
    assert back["venue"].cat.ordered
    assert list(back["venue"].cat.categories) == ["Z", "Y", "X"]
    assert back["maybe"].dtype == "Int64"
    assert back["f32"].dtype == np.float32


def test_categoricals_keep_order_and_unused_categories(tmp_path: pathlib.Path) -> None:
    # the Parquet writer re-encodes dictionaries in order of appearance; the categories are
    # restored on read, as pyarrow's own files keep them
    df = pd.DataFrame(
        {
            "level": pd.Categorical(["mid", "low", None, "mid"], categories=["high", "mid", "low"], ordered=True),
            "code": pd.Categorical([3, 1, 3, 2], categories=[3, 2, 1, 0]),
            "when": pd.Categorical(pd.to_datetime(["2025-01-02", "2025-01-01"] * 2)),
        }
    )
    # pyarrow reads the int and datetime categoricals back as plain columns: compare with df
    path = tmp_path / "c.parquet"
    ppd.to_parquet(df, path)
    back = ppd.read_parquet(path)
    pd.testing.assert_frame_equal(back, df)
    pd.testing.assert_frame_equal(back[["level"]], pd.read_parquet(_pyarrow_copy(df[["level"]], tmp_path)))
    assert list(back["level"].cat.categories) == ["high", "mid", "low"]
    assert list(back["code"].cat.categories) == [3, 2, 1, 0]
    # in several row groups too
    path = tmp_path / "rg.parquet"
    ppd.to_parquet(df, path, row_group_size=1)
    pd.testing.assert_frame_equal(ppd.read_parquet(path), df)
    pd.testing.assert_frame_equal(ppd.read_parquet(path, columns=["code"]), df[["code"]])


def test_empty_frame_with_categories(tmp_path: pathlib.Path) -> None:
    df = pd.DataFrame({"c": pd.Categorical([], categories=["b", "a"], ordered=True), "x": pd.Series([], dtype="int64")})
    path = tmp_path / "empty.parquet"
    ppd.to_parquet(df, path)
    back = ppd.read_parquet(path)
    pd.testing.assert_frame_equal(back, df)
    assert list(back["c"].cat.categories) == ["b", "a"]


def test_files_without_categories_metadata(tmp_path: pathlib.Path) -> None:
    # a file written by pyarrow (no restored categories needed) and plain Arrow dictionaries
    df = pd.DataFrame({"c": pd.Categorical(["b", "a"], categories=["b", "a", "z"])})
    pd.testing.assert_frame_equal(ppd.read_parquet(_pyarrow_copy(df, tmp_path)), df)
    path = tmp_path / "plain.parquet"
    ppc.write_table(pa.table({"c": pa.array(["b", "a"]).dictionary_encode()}), path)
    assert list(ppd.read_parquet(path)["c"].cat.categories) == ["b", "a"]


def test_codecs_and_options(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "o.parquet"
    choice = ppd.to_parquet(frame(), path, policy="pcodec", row_group_size=200)
    assert choice["price"] == "PCODEC"
    assert choice["ts"] == "PCODEC"
    assert choice["name"] == "ZSTD"
    assert ppc.ParquetFile(path).metadata.row_group_rows == [200, 200, 100]


def test_index_false_and_range_index(tmp_path: pathlib.Path) -> None:
    df = frame().reset_index(drop=True)
    back = assert_same_as_pyarrow(df, tmp_path)
    assert isinstance(back.index, pd.RangeIndex)
    pd.testing.assert_frame_equal(back, df)
    dropped = assert_same_as_pyarrow(frame(), tmp_path, index=False)
    assert "ts" not in dropped.columns
    assert isinstance(dropped.index, pd.RangeIndex)
    stored = assert_same_as_pyarrow(df, tmp_path, index=True)
    assert "__index_level_0__" not in stored.columns
    pd.testing.assert_frame_equal(stored, df)


def test_multi_index(tmp_path: pathlib.Path) -> None:
    df = frame().reset_index().set_index(["name", "ts"])
    back = assert_same_as_pyarrow(df, tmp_path)
    pd.testing.assert_frame_equal(back, df, check_freq=False)


def test_column_selection_keeps_the_index(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "c.parquet"
    df = frame()
    ppd.to_parquet(df, path)
    back = ppd.read_parquet(path, columns=["size", "price"])
    pd.testing.assert_frame_equal(back, df[["size", "price"]], check_freq=False)
    pd.testing.assert_frame_equal(back, pd.read_parquet(_pyarrow_copy(df, tmp_path), columns=["size", "price"]))
    # an index column asked for explicitly is not read twice
    multi = df.reset_index().set_index(["name", "ts"])
    ppd.to_parquet(multi, path)
    pd.testing.assert_frame_equal(
        ppd.read_parquet(path, columns=["name", "price"]),
        pd.read_parquet(_pyarrow_copy(multi, tmp_path), columns=["name", "price"]),
    )


def _pyarrow_copy(df: pd.DataFrame, tmp_path: pathlib.Path) -> pathlib.Path:
    p = tmp_path / "pa.parquet"
    df.to_parquet(p, engine="pyarrow")
    return p


def test_column_selection_without_pandas_metadata(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "plain.parquet"
    ppc.write_table(pa.table({"a": [1, 2], "b": [3.0, 4.0]}), path)
    pd.testing.assert_frame_equal(ppd.read_parquet(path, columns=["b"]), pd.DataFrame({"b": [3.0, 4.0]}))


def test_to_pandas_kwargs(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "k.parquet"
    ppd.to_parquet(frame(), path)
    back = ppd.read_parquet(path, columns=["side"], categories=[], strings_to_categorical=True)
    assert back["side"].dtype == "category"
    arrow_backed = ppd.read_parquet(path, columns=["size"], types_mapper=pd.ArrowDtype)
    assert isinstance(arrow_backed["size"].dtype, pd.ArrowDtype)


def test_file_objects(tmp_path: pathlib.Path) -> None:
    buf = io.BytesIO()
    df = frame()
    ppd.to_parquet(df, buf)
    pd.testing.assert_frame_equal(ppd.read_parquet(io.BytesIO(buf.getvalue())), df, check_freq=False)


def test_pandas_metadata_is_what_pyarrow_writes(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "md.parquet"
    df = frame()
    ppd.to_parquet(df, path)
    ours = ppc.read_table(path).schema.pandas_metadata
    theirs = pq.read_table(_pyarrow_copy(df, tmp_path)).schema.pandas_metadata
    assert ours is not None
    assert ours["index_columns"] == theirs["index_columns"] == ["ts"]
    assert ours["columns"] == theirs["columns"]


def test_to_parquet_passes_every_option() -> None:
    import random

    r = random.Random(3)
    df = pd.DataFrame({"x": [int(r.gauss(0, 1e6)) for _ in range(3000)], "y": [r.random() for _ in range(3000)]})
    options: dict[str, object] = {
        "policy": "base",
        "columns": {"x": "pcodec"},
        "level": 0,
        "base": "snappy",
        "row_group_size": 1000,
    }
    ours = io.BytesIO()
    want = io.BytesIO()
    assert ppd.to_parquet(df, ours, **options) == {"x": "PCODEC", "y": "SNAPPY"}  # type: ignore[arg-type]
    ppc.write_table(pa.Table.from_pandas(df), want, **options)  # type: ignore[arg-type]
    assert ours.getvalue() == want.getvalue()
    other = io.BytesIO()
    ppc.write_table(pa.Table.from_pandas(df), other, **{**options, "level": 8})  # type: ignore[arg-type]
    assert other.getvalue() != want.getvalue()


def test_rejects_non_frames(tmp_path: pathlib.Path) -> None:
    with pytest.raises(TypeError, match="expected a pandas DataFrame, got dict"):
        ppd.to_parquet({"a": [1]}, tmp_path / "x.parquet")  # type: ignore[arg-type]


# ---------------------------------------------------------------- every kind of index and dtype


N = 12


def indexes() -> dict[str, pd.Index]:
    return {
        "range": pd.RangeIndex(N),
        "range_step": pd.RangeIndex(5, 5 + 3 * N, 3),
        "range_named": pd.RangeIndex(N, name="r"),
        "int": pd.Index(np.arange(N) * 7, name="k"),
        "unnamed_int": pd.Index(np.arange(N)[::-1]),
        "string": pd.Index([f"i{i}" for i in range(N)], name="s"),
        "datetime_tz": pd.date_range("2025-03-30", periods=N, freq="h", tz="Europe/Berlin", name="t"),
        "datetime": pd.date_range("2025-01-01", periods=N, freq="D"),
        "categorical": pd.CategoricalIndex(list("abc") * (N // 3), categories=list("cba"), name="c"),
        "multi": pd.MultiIndex.from_arrays([list("ab") * (N // 2), np.arange(N)], names=["l", "n"]),
        "multi_unnamed": pd.MultiIndex.from_arrays([np.arange(N) % 3, np.arange(N)]),
        "float": pd.Index(np.arange(N) / 2, name="f"),
        "duplicate_names": pd.MultiIndex.from_arrays([np.arange(N), np.arange(N)], names=["x", "x"]),
    }


@pytest.mark.parametrize("index", [None, True, False])
@pytest.mark.parametrize("kind", list(indexes()))
def test_every_index(tmp_path: pathlib.Path, kind: str, index: bool | None) -> None:
    df = pd.DataFrame({"v": np.arange(N, dtype=np.int64), "w": np.arange(N) / 3}, index=indexes()[kind])
    assert_same_as_pyarrow(df, tmp_path, index=index)


DTYPES: dict[str, object] = {
    "int8": np.arange(N, dtype=np.int8),
    "uint32": np.arange(N, dtype=np.uint32) * 1_000_000,
    "float16": np.arange(N, dtype=np.float16),
    "Int8": pd.array([None, *range(N - 1)], dtype="Int8"),
    "UInt64": pd.array([None, *range(N - 1)], dtype="UInt64"),
    "Float64": pd.array([None, *np.arange(N - 1) / 2], dtype="Float64"),
    "boolean": pd.array([None, True, False] * (N // 3), dtype="boolean"),
    "string": pd.array([None, *map(str, range(N - 1))], dtype="string"),
    "string_pyarrow": pd.array([None, *map(str, range(N - 1))], dtype="string[pyarrow]"),
    "int64_pyarrow": pd.array([None, *range(N - 1)], dtype="int64[pyarrow]"),
    "decimal_pyarrow": pd.arrays.ArrowExtensionArray(
        pa.array([None, *(decimal.Decimal(i).scaleb(-2) for i in range(N - 1))], pa.decimal128(10, 2))
    ),
    "list_pyarrow": pd.arrays.ArrowExtensionArray(pa.array([None, *([i] for i in range(N - 1))], pa.list_(pa.int64()))),
    "timestamp_pyarrow": pd.arrays.ArrowExtensionArray(
        pa.array([None, *range(N - 1)], pa.timestamp("ns", "Asia/Tokyo"))
    ),
    "datetime_tz": pd.date_range("2025-11-02", periods=N, freq="30min", tz="America/Chicago"),
    "datetime_ns": pd.date_range("1677-09-22", periods=N, freq="D"),
    "datetime_s": pd.Series(pd.date_range("2025", periods=N, freq="s")).astype("datetime64[s]").array,
    "timedelta": pd.to_timedelta(np.arange(N) - 5, unit="h"),
    "period": pd.period_range("2025-01", periods=N, freq="M"),
    "interval": pd.interval_range(0, N),
    "category_str": pd.Categorical(list("xyz") * (N // 3), categories=list("zyxw")),
    "category_int": pd.Categorical(np.arange(N) % 3, categories=[2, 1, 0, 9], ordered=True),
    "category_with_nan": pd.Categorical([None, "a", "b"] * (N // 3)),
    "object_str": np.array([f"o{i}" for i in range(N)], dtype=object),
    "object_bytes": np.array([bytes([i]) for i in range(N)], dtype=object),
    "date_object": np.array([pd.Timestamp("2025-01-01").date()] * N, dtype=object),
    "sparse": pd.arrays.SparseArray([0, 0, 1] * (N // 3)),
}


@pytest.mark.parametrize("name", list(DTYPES))
def test_every_dtype(tmp_path: pathlib.Path, name: str) -> None:
    df = pd.DataFrame({name: DTYPES[name], "other": np.arange(N)})
    if name == "sparse":
        # pyarrow refuses sparse columns; so does this package, the same way
        with pytest.raises((TypeError, pa.ArrowException)):
            ppd.to_parquet(df, tmp_path / "x.parquet")
        return
    if name == "list_pyarrow":
        # pandas cannot read its own metadata of a pyarrow list dtype back; neither can this
        df.to_parquet(tmp_path / "theirs.parquet", engine="pyarrow")
        with pytest.raises(TypeError, match="not understood"):
            pd.read_parquet(tmp_path / "theirs.parquet", engine="pyarrow")
        ppd.to_parquet(df, tmp_path / "ours.parquet")
        with pytest.raises(TypeError, match="not understood"):
            ppd.read_parquet(tmp_path / "ours.parquet")
        assert ppd.read_parquet(tmp_path / "ours.parquet", ignore_metadata=True)[name].tolist()[1:] == [
            [i] for i in range(N - 1)
        ]
        return
    if name in KEPT_BETTER:
        # pyarrow reads these back as something else; this package keeps them
        path = tmp_path / "ours.parquet"
        ppd.to_parquet(df, path)
        pd.testing.assert_frame_equal(ppd.read_parquet(path), df)
        theirs = tmp_path / "theirs.parquet"
        df.to_parquet(theirs, engine="pyarrow")
        assert pd.read_parquet(theirs)[name].dtype != df[name].dtype
        return
    back = assert_same_as_pyarrow(df, tmp_path)
    if name.startswith("category"):
        assert back[name].dtype == df[name].dtype


#: pyarrow stores seconds as milliseconds and reads integer categories as plain integers
KEPT_BETTER = {"datetime_s", "category_int"}


def test_a_frame_without_rows_or_columns(tmp_path: pathlib.Path) -> None:
    for df in [pd.DataFrame(), pd.DataFrame({"a": []}, dtype="int64"), pd.DataFrame(index=pd.RangeIndex(5))]:
        assert_same_as_pyarrow(df, tmp_path)


def test_attrs_and_column_names(tmp_path: pathlib.Path) -> None:
    df = pd.DataFrame({"a b": [1], "é": [2.0], "1": [3]})
    df.attrs = {"source": "test"}
    back = assert_same_as_pyarrow(df, tmp_path)
    assert back.attrs == {"source": "test"}
    # column names that are not strings are stored as strings, as pyarrow does
    assert_same_as_pyarrow(pd.DataFrame({0: [1], 1: [2]}), tmp_path)


def test_read_parquet_keyword_arguments(tmp_path: pathlib.Path) -> None:
    df = pd.DataFrame({"a": pd.array([1, None], dtype="Int64"), "s": ["x", None]})
    path = tmp_path / "k.parquet"
    ppd.to_parquet(df, path)
    arrow = ppd.read_parquet(path, types_mapper=pd.ArrowDtype)
    assert isinstance(arrow["a"].dtype, pd.ArrowDtype)
    assert ppd.read_parquet(path, columns=["s"]).columns.tolist() == ["s"]
    with pytest.raises(TypeError):
        ppd.read_parquet(path, not_an_argument=True)


def test_to_parquet_passes_the_policy() -> None:
    # a column the auto policy gives to PCODEC, so that only the policy passed changes the codec
    df = pd.DataFrame({"z": [1_700_000_000_000 + 1000 * i for i in range(2000)]})
    assert ppd.to_parquet(df, io.BytesIO()) == {"z": "PCODEC"}
    assert ppd.to_parquet(df, io.BytesIO(), policy="auto") == {"z": "PCODEC"}
    assert ppd.to_parquet(df, io.BytesIO(), policy="base") == {"z": "ZSTD"}
