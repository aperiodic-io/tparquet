"""Arguments of the wrong type or out of range: a TypeError or a PcodecError that names the
argument, never an extraction error of the extension or a column per letter."""

from __future__ import annotations

import errno
import io
import os
import pathlib
from collections.abc import Callable
from typing import Any

import pyarrow as pa
import pytest

import tparquet as ppc
from conftest import vector
from tparquet import codec

TABLE = pa.table({"ab": pa.array(range(10), pa.int64()), "a": pa.array(range(10), pa.int64())})
I64_MAX = 2**63 - 1


@pytest.fixture
def path(tmp_path: pathlib.Path) -> pathlib.Path:
    p = tmp_path / "t.parquet"
    ppc.write_table(TABLE, p, row_group_size=5)
    return p


def write(**kwargs: Any) -> Callable[[], dict[str, str]]:
    return lambda: ppc.write_table(TABLE, io.BytesIO(), **kwargs)


def readers(path: pathlib.Path, **kwargs: Any) -> list[Callable[[], object]]:
    f = ppc.ParquetFile(path)
    return [
        lambda: ppc.read_table(path, **kwargs),
        lambda: ppc.read_batches(path, **kwargs),
        lambda: f.read(**kwargs),
        lambda: f.stream(**kwargs),
        lambda: list(f.iter_batches(**kwargs)),
    ]


@pytest.mark.parametrize("name", ["level", "row_group_size"])
@pytest.mark.parametrize("value", [True, 8.0, "8", None])
def test_write_options_must_be_ints(name: str, value: object) -> None:
    if value is None and name == "row_group_size":
        assert write(**{name: value})()["a"] == "PCODEC"
        return
    with pytest.raises(TypeError, match=rf"^{name} must be an int, got {type(value).__name__}$"):
        write(**{name: value})()
    options: dict[str, Any] = {name: value}
    with pytest.raises(TypeError, match=rf"^{name} must be an int"):
        ppc.Writer(io.BytesIO(), TABLE.schema, **options)


@pytest.mark.parametrize("value", [I64_MAX + 1, -(2**63) - 1, 2**200])
def test_write_options_beyond_64_bits(value: int) -> None:
    for name in ["level", "row_group_size"]:
        with pytest.raises(ppc.PcodecError, match=rf"^{name} {value} is out of range$"):
            write(**{name: value})()
    with pytest.raises(ppc.PcodecError, match=rf"^level {value} is out of range$"):
        ppc.transcode(vector("standard_control"), io.BytesIO(), level=value)


def test_write_options_at_the_64_bit_limits() -> None:
    # the extension's own checks take over
    with pytest.raises(ppc.PcodecError, match=rf"^level {I64_MAX} is not in 0\.\.=12$"):
        write(level=I64_MAX)()
    with pytest.raises(ppc.PcodecError, match=rf"^level {-(2**63)} is not in 0\.\.=12$"):
        write(level=-(2**63))()
    with pytest.raises(ppc.PcodecError, match=rf"^row_group_size must be positive, got {-(2**63)}$"):
        write(row_group_size=-(2**63))()
    assert write(row_group_size=I64_MAX)()["a"] == "PCODEC"


def test_read_options_must_be_ints(path: pathlib.Path) -> None:
    for name, value in [("batch_size", True), ("batch_size", 10.0), ("row_groups", [True]), ("row_groups", ["0"])]:
        for read in readers(path, **{name: value}):
            with pytest.raises(
                TypeError,
                match=rf"^{name} must be an int, got {type(True if value is True else value[0] if isinstance(value, list) else value).__name__}$",
            ):
                read()
    f = ppc.ParquetFile(path)
    with pytest.raises(TypeError, match=r"^limit must be an int, got float$"):
        f.stream(limit=1.0)  # type: ignore[arg-type]


@pytest.mark.parametrize("value", [I64_MAX + 1, 2**100])
def test_read_options_beyond_64_bits(path: pathlib.Path, value: int) -> None:
    f = ppc.ParquetFile(path)
    for read in readers(path, batch_size=value):
        with pytest.raises(ppc.PcodecError, match=rf"^batch_size {value} is out of range$"):
            read()
    for read in readers(path, row_groups=[0, value]):
        with pytest.raises(ppc.PcodecError, match=rf"^row_groups {value} is out of range$"):
            read()
    with pytest.raises(ppc.PcodecError, match=rf"^limit {value} is out of range$"):
        f.stream(limit=value)
    # at the limit: the extension's checks
    assert ppc.read_table(path, batch_size=I64_MAX).num_rows == 10
    assert pa.RecordBatchReader.from_stream(f.stream(limit=I64_MAX)).read_all().num_rows == 10
    with pytest.raises(ppc.PcodecError, match=rf"^row group {I64_MAX} out of range"):
        ppc.read_table(path, row_groups=[I64_MAX])


def test_columns_are_a_sequence_of_names_not_a_string(path: pathlib.Path) -> None:
    f = ppc.ParquetFile(path)
    calls: list[Callable[[], object]] = [*readers(path, columns="ab"), lambda: f.empty("ab")]
    for read in calls:
        with pytest.raises(TypeError, match=r"^columns must be a sequence of names, not a str$"):
            read()
    assert ppc.read_table(path, columns=("ab",)).column_names == ["ab"]
    with pytest.raises(TypeError, match=r"^columns must be a sequence of names, not a bytes$"):
        ppc.read_table(path, columns=b"ab")  # type: ignore[arg-type]


def test_columns_of_the_integrations(path: pathlib.Path) -> None:
    import duckdb

    import tparquet.duckdb as pdd
    import tparquet.pandas as ppd
    import tparquet.polars as ppl

    calls: list[Callable[[], object]] = [
        lambda: ppd.read_parquet(path, columns="ab"),
        lambda: ppl.read_parquet(path, columns="ab"),
        lambda: pdd.ArrowSource(path, columns="ab"),
        lambda: pdd.read_parquet(path, columns="ab", connection=duckdb.connect()),
    ]
    for read in calls:
        with pytest.raises(TypeError, match=r"^columns must be a sequence of names, not a str$"):
            read()


@pytest.mark.parametrize("value", [True, 8.0])
def test_codec_arguments_must_be_ints(value: object) -> None:
    with pytest.raises(TypeError, match=rf"^level must be an int, got {type(value).__name__}$"):
        codec.compress(b"", level=value)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match=rf"^expected_len must be an int, got {type(value).__name__}$"):
        codec.decompress(b"", value)  # type: ignore[arg-type]


def test_codec_arguments_beyond_64_bits() -> None:
    with pytest.raises(ppc.PcodecError, match=rf"^level {2**64} is out of range$"):
        codec.compress(b"", level=2**64)
    with pytest.raises(ppc.PcodecError, match=rf"^expected_len {2**64} is out of range$"):
        codec.decompress(b"", 2**64)
    with pytest.raises(ppc.PcodecError, match=rf"^compression level {I64_MAX} is not in 0\.\.=12$"):
        codec.compress(b"", level=I64_MAX)


def test_a_directory_is_not_a_file(tmp_path: pathlib.Path) -> None:
    with pytest.raises(IsADirectoryError) as e:
        ppc.read_table(tmp_path)
    assert e.value.filename == str(tmp_path)
    assert e.value.errno == errno.EISDIR
    assert str(e.value) == f"[Errno {errno.EISDIR}] {os.strerror(errno.EISDIR)}: {str(tmp_path)!r}"
    with pytest.raises(IsADirectoryError):
        ppc.ParquetFile(str(tmp_path))
    with pytest.raises(IsADirectoryError) as e:
        ppc.write_table(TABLE, tmp_path)
    assert e.value.filename == str(tmp_path)
