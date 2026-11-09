"""The core API over the extension module: no Arrow library is needed to call it, except
where a function returns pyarrow objects."""

from __future__ import annotations

import dataclasses
import errno
import io
import json
import os
from typing import TYPE_CHECKING, Any, Literal, Protocol, Union

from tparquet import _lib

if TYPE_CHECKING:
    from collections.abc import Iterator, Mapping, Sequence
    from types import TracebackType

    import pyarrow as pa

PcodecError = _lib.PcodecError

#: A codec policy for INT32, INT64, FLOAT and DOUBLE columns.
Policy = Literal["auto", "pcodec", "base"]


class Readable(Protocol):
    """A binary file object to read from: its `read()` returns the rest of the file."""

    def read(self) -> bytes: ...


class Writable(Protocol):
    """A binary file object to write to."""

    def write(self, data: bytes, /) -> object: ...


#: A path: `str`, `bytes` or `os.PathLike`.
Path = Union[str, bytes, "os.PathLike[str]", "os.PathLike[bytes]"]
#: Where a file is read from: a path, or a binary file object.
Source = Union[Path, Readable]
#: Where a file is written: a path, or a binary file object.
Sink = Union[Path, Writable]


class ArrowStreamExportable(Protocol):
    """An object with `__arrow_c_stream__` (a table, a record batch reader, a DataFrame...)."""

    def __arrow_c_stream__(self, requested_schema: object | None = None) -> object: ...


class ArrowArrayExportable(Protocol):
    """An object with `__arrow_c_array__` (a record batch, a struct array)."""

    def __arrow_c_array__(self, requested_schema: object | None = None) -> tuple[object, object]: ...


class ArrowSchemaExportable(Protocol):
    """An object with `__arrow_c_schema__` (a schema)."""

    def __arrow_c_schema__(self) -> object: ...


ArrowData = Union[ArrowStreamExportable, ArrowArrayExportable]


def _path(source: object) -> str | None:
    """The path `source` names, or `None` when it is not a path."""
    if isinstance(source, (str, bytes)) or hasattr(source, "__fspath__"):
        return os.fsdecode(source)
    return None


def _binary(obj: object, mode: str) -> None:
    """Refuses a text-mode file object, which cannot hold Parquet bytes."""
    if isinstance(obj, io.TextIOBase):
        raise TypeError(
            f"expected a binary file object, got a text file ({type(obj).__name__}), which holds str, not bytes: "
            f"open it in binary mode ('{mode}')"
        )


def _open(source: Source) -> _lib.File:
    path = _path(source)
    if path is not None:
        if os.path.isdir(path):
            raise IsADirectoryError(errno.EISDIR, os.strerror(errno.EISDIR), path)
        return _lib.open_path(path)
    _binary(source, "rb")
    read = getattr(source, "read", None)
    if read is None:
        raise TypeError(f"expected a path or a binary file object, got {type(source).__name__}")
    data = read()
    if not isinstance(data, bytes):
        raise TypeError(f"the file object must return bytes from read(), got {type(data).__name__}")
    return _lib.open_bytes(data)


def _sink(target: Sink) -> object:
    """A `str` path, or the file object itself."""
    path = _path(target)
    if path is not None:
        return path
    _binary(target, "wb")
    if not hasattr(target, "write"):
        raise TypeError(f"expected a path or a binary file object, got {type(target).__name__}")
    return target


#: Integer arguments are 64-bit signed integers in the extension.
_I64 = 2**63


def _int(name: str, value: object) -> int:
    """`value`, which must be an `int` (not a `bool`) of 64 bits; the extension checks the
    range that makes sense."""
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"{name} must be an int, got {type(value).__name__}")
    if not -_I64 <= value < _I64:
        raise PcodecError(f"{name} {value} is out of range")
    return value


def _optional_int(name: str, value: object) -> int | None:
    return None if value is None else _int(name, value)


def _names(columns: Sequence[str] | None) -> list[str] | None:
    """`columns` as a list; a string is not a sequence of names."""
    if isinstance(columns, (str, bytes)):
        raise TypeError(f"columns must be a sequence of names, not a {type(columns).__name__}")
    return None if columns is None else list(columns)


def _options(
    policy: Policy,
    columns: Mapping[str, Policy] | None,
    level: int,
    base: str,
    row_group_size: int | None,
    encodings: Mapping[str, str] | None = None,
) -> dict[str, Any]:
    return {
        "encodings": dict(encodings or {}),
        "policy": policy,
        "columns": dict(columns or {}),
        "level": _int("level", level),
        "base": base,
        "row_group_size": _optional_int("row_group_size", row_group_size),
    }


def _pyarrow() -> Any:
    try:
        import pyarrow
    except ImportError as e:
        raise ImportError(
            "this function returns pyarrow objects: install pyarrow, or use read_batches(), whose result "
            "any Arrow library reads through __arrow_c_stream__"
        ) from e
    return pyarrow


@dataclasses.dataclass(frozen=True)
class FileMetadata:
    """What the footer of a file says."""

    #: Rows in the file.
    num_rows: int
    #: Rows per row group.
    row_group_rows: list[int]
    #: The writer, as the file names it.
    created_by: str | None
    #: The footer's key-value pairs.
    key_value_metadata: dict[str, str | None]
    #: Dotted leaf column path to its Parquet physical type.
    physical_types: dict[str, str]
    #: Dotted leaf column path to the codecs of its column chunks (each once, in file order).
    codecs: dict[str, list[str]]

    @property
    def num_row_groups(self) -> int:
        """Number of row groups."""
        return len(self.row_group_rows)

    @property
    def pcodec_version(self) -> str | None:
        """The `parquet.pcodec.version` footer value: `"1"` in files with PCODEC chunks."""
        return self.key_value_metadata.get("parquet.pcodec.version")


class ParquetFile:
    """A Parquet file (PCODEC or not) whose footer has been read.

    `source` is a path or a binary file object; a file object is read into memory once.
    """

    def __init__(self, source: Source) -> None:
        self._file = _open(source)
        d = self._file.metadata()
        self._metadata = FileMetadata(
            num_rows=d["num_rows"],
            row_group_rows=d["row_group_rows"],
            created_by=d["created_by"],
            key_value_metadata=dict(d["key_value_metadata"]),
            physical_types={path: t for path, t, _ in d["columns"]},
            codecs={path: codecs for path, _, codecs in d["columns"]},
        )

    @property
    def metadata(self) -> FileMetadata:
        """Rows, row groups and per-column codecs."""
        return self._metadata

    @property
    def num_row_groups(self) -> int:
        """Number of row groups."""
        return self._metadata.num_row_groups

    @property
    def schema(self) -> pa.Schema:
        """The Arrow schema, as a `pyarrow.Schema`."""
        pyarrow = _pyarrow()
        schema: pa.Schema = pyarrow.schema(self._file.schema())
        return schema

    @property
    def schema_capsule(self) -> ArrowSchemaExportable:
        """The Arrow schema as an object with `__arrow_c_schema__`, for any Arrow library."""
        return self._file.schema()

    def empty(self, columns: Sequence[str] | None = None) -> _lib.RecordBatchStream:
        """A stream with the schema `stream(columns)` has and no rows."""
        return self._file.empty(_names(columns))

    def integer_bounds(self) -> list[dict[str, tuple[int, int]]]:
        """Per row group, `{leaf path: (min, max)}` from the statistics of its `INT32` and
        `INT64` leaves, as stored (signed; timestamps in their unit, dates in days). Leaves
        without both bounds are left out, and so are dotted paths that name two leaves (a
        top-level column `a.b` and the field `b` of a struct `a`)."""
        return self._file.integer_bounds()

    def _column_bounds(self) -> tuple[dict[str, str], list[dict[str, tuple[int, int]]]]:
        """How each top-level column of signed integers, dates or timestamps is stored
        (`int`, `date32`, `date64`, `timestamp[<unit>]`), and per row group `{name: (min,
        max)}` from its statistics: what `tparquet.polars` prunes row groups with."""
        return self._file.column_bounds()

    def stream(
        self,
        columns: Sequence[str] | None = None,
        row_groups: Sequence[int] | None = None,
        batch_size: int = 65536,
        limit: int | None = None,
    ) -> _lib.RecordBatchStream:
        """A one-shot stream of record batches, exposing `__arrow_c_stream__`.

        `columns` are top-level names, returned in that order; `row_groups` are read in the
        order given; `limit` caps the number of rows.
        """
        return self._file.stream(
            _names(columns),
            None if row_groups is None else [_int("row_groups", rg) for rg in row_groups],
            _int("batch_size", batch_size),
            _optional_int("limit", limit),
        )

    def iter_batches(
        self,
        batch_size: int = 65536,
        columns: Sequence[str] | None = None,
        row_groups: Sequence[int] | None = None,
    ) -> Iterator[pa.RecordBatch]:
        """Yields `pyarrow.RecordBatch` objects of at most `batch_size` rows."""
        pyarrow = _pyarrow()
        for batch in self.stream(columns, row_groups, batch_size):
            yield pyarrow.record_batch(batch)

    def read(
        self, columns: Sequence[str] | None = None, row_groups: Sequence[int] | None = None, batch_size: int = 65536
    ) -> pa.Table:
        """Reads the file (or the given columns and row groups) as a `pyarrow.Table`."""
        pyarrow = _pyarrow()
        stream = self.stream(columns, row_groups, batch_size)
        schema = pyarrow.schema(stream)
        table: pa.Table = pyarrow.Table.from_batches([pyarrow.record_batch(b) for b in stream], schema=schema)
        return table

    def __repr__(self) -> str:
        md = self._metadata
        return f"<ParquetFile: {md.num_rows} rows, {md.num_row_groups} row groups, columns {list(md.codecs)}>"


def read_batches(
    source: Source,
    columns: Sequence[str] | None = None,
    row_groups: Sequence[int] | None = None,
    batch_size: int = 65536,
) -> _lib.RecordBatchStream:
    """Reads a Parquet file (PCODEC or not) as a one-shot Arrow stream.

    The result exposes `__arrow_c_stream__` and `__arrow_c_schema__`, so pyarrow
    (`pa.RecordBatchReader.from_stream`), Polars (`pl.DataFrame`), DuckDB and nanoarrow
    read it without a copy. Batches are decoded as they are pulled, with the GIL released.
    """
    return ParquetFile(source).stream(columns, row_groups, batch_size)


def read_table(
    source: Source,
    columns: Sequence[str] | None = None,
    row_groups: Sequence[int] | None = None,
    batch_size: int = 65536,
) -> pa.Table:
    """Reads a Parquet file (PCODEC or not) as a `pyarrow.Table` (pyarrow must be installed).

    `columns` are top-level names, in the order wanted; `row_groups` are indices, read in the
    order given.
    """
    return ParquetFile(source).read(columns, row_groups, batch_size)


def _schema_capsule(schema: ArrowSchemaExportable) -> object:
    export = getattr(schema, "__arrow_c_schema__", None)
    if export is None:
        raise TypeError(f"expected a schema with __arrow_c_schema__, got {type(schema).__name__}")
    return export()


class Writer:
    """Writes record batches to one Parquet file, choosing PCODEC per column.

    Use it as a context manager: the footer is written on a normal exit; on an exception the
    file is abandoned (and removed when the writer created it from a path).

    With `policy="auto"` each INT32/INT64/FLOAT/DOUBLE column gets whichever of PCODEC and the
    base codec is smaller on the first batch with rows; `"pcodec"` and `"base"` force one.
    `columns` maps a top-level name or a dotted leaf path to its own policy. `level` is the
    pcodec level (0 to 12), `base` the codec of the other columns (`"zstd(3)"`, `"snappy"`,
    `"uncompressed"`...), and `row_group_size` the most rows per row group. `encodings` maps a
    column, named as in `columns`, to the Parquet encoding it is written with when it gets the
    base codec (`"ALP"`, `"BYTE_STREAM_SPLIT"`, `"DELTA_BINARY_PACKED"`...), without a
    dictionary; PCODEC columns are always PLAIN.
    """

    def __init__(
        self,
        sink: Sink,
        schema: ArrowSchemaExportable,
        policy: Policy = "auto",
        columns: Mapping[str, Policy] | None = None,
        level: int = 8,
        # "ZSTD(3)" (a mutant) names the same codec as "zstd(3)": no test can tell them apart
        base: str = "zstd(3)",  # pragma: no mutate
        row_group_size: int | None = None,
        encodings: Mapping[str, str] | None = None,
    ) -> None:
        target = _sink(sink)
        capsule = _schema_capsule(schema)
        self._writer = _lib.Writer(target, capsule, _options(policy, columns, level, base, row_group_size, encodings))

    def write(self, data: ArrowData) -> None:
        """Writes a table, a record batch, or any object with `__arrow_c_stream__` or
        `__arrow_c_array__` with the writer's schema."""
        first, second = _capsules(data)
        if second is None:
            self._writer.write_stream(first)
        else:
            self._writer.write_batch(first, second)

    def flush(self) -> None:
        """Ends the current row group."""
        self._writer.flush()

    def close(self) -> dict[str, str]:
        """Writes the footer; returns the codec of every leaf column."""
        return self._writer.close()

    def abort(self) -> None:
        """Abandons the file without a footer, removing it when the writer created it; does
        nothing once the writer is closed or aborted."""
        self._writer.abort()

    @property
    def codecs(self) -> dict[str, str] | None:
        """Dotted leaf path to codec (`"PCODEC"`, `"ZSTD"`...), once chosen."""
        return self._writer.codecs

    @property
    def closed(self) -> bool:
        """Whether the writer was closed or aborted."""
        return self._writer.closed

    def __enter__(self) -> Writer:
        return self

    def __exit__(
        self, exc_type: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None
    ) -> None:
        if exc_type is None:
            self.close()
        else:
            self.abort()


def _capsules(data: ArrowData) -> tuple[object, object]:
    """`data` as `(stream capsule, None)`, or as the `(schema, array)` capsules of one batch."""
    stream = getattr(data, "__arrow_c_stream__", None)
    if stream is not None:
        return stream(), None
    array = getattr(data, "__arrow_c_array__", None)
    if array is None:
        raise TypeError(f"expected Arrow data with __arrow_c_stream__ or __arrow_c_array__, got {type(data).__name__}")
    return array()  # type: ignore[no-any-return]


def write_table(
    data: ArrowData,
    sink: Sink,
    policy: Policy = "auto",
    columns: Mapping[str, Policy] | None = None,
    level: int = 8,
    # "ZSTD(3)" (a mutant) names the same codec as "zstd(3)": no test can tell them apart
    base: str = "zstd(3)",  # pragma: no mutate
    row_group_size: int | None = None,
    encodings: Mapping[str, str] | None = None,
) -> dict[str, str]:
    """Writes Arrow data to a Parquet file with PCODEC where the policy selects it.

    `data` is anything with `__arrow_c_stream__` (a pyarrow or Polars table, a record batch
    reader, a DuckDB result...) or `__arrow_c_array__` (a record batch). The options are
    those of `Writer`. Returns the codec chosen for every leaf column, by dotted path.
    """
    first, second = _capsules(data)
    target = _sink(sink)
    options = _options(policy, columns, level, base, row_group_size, encodings)
    if second is None:
        return _lib.write_stream(target, first, options)
    writer = _lib.Writer(target, first, options)
    try:
        writer.write_batch(first, second)
    except BaseException:
        writer.abort()
        raise
    return writer.close()


def transcode(
    source: Source,
    sink: Sink,
    target: Literal["pcodec", "standard"] = "pcodec",
    policy: Policy = "auto",
    columns: Mapping[str, Policy] | None = None,
    level: int = 8,
    # "ZSTD(3)" (a mutant) names the same codec as "zstd(3)": no test can tell them apart
    base: str = "zstd(3)",  # pragma: no mutate
) -> dict[str, str]:
    """Rewrites a Parquet file keeping its schema, metadata and row groups.

    `target="pcodec"` applies PCODEC where the policy selects it; `target="standard"` writes
    standard Parquet, with `base` in place of PCODEC. Returns the codec of every leaf column
    in the first row group (empty when the file has no rows).
    """
    return _lib.transcode(_open(source), _sink(sink), target, _options(policy, columns, level, base, None))


def inspect(source: Source) -> dict[str, Any]:
    """Codecs, sizes, page counts and PCODEC payload modes per column, as
    `tparquet inspect --json` reports them."""
    report: dict[str, Any] = json.loads(_open(source).inspect())
    return report


def verify(a: Source, b: Source) -> str | None:
    """Compares the data of two Parquet files (floats bit for bit): `None` when they are the
    same, else where they first differ."""
    return _lib.verify(_open(a), _open(b))
