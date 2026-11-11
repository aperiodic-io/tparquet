"""Polars plugin for tparquet: Parquet for time series, with the PCODEC codec.

    import polars_tparquet as pt

    lf = pt.scan_tparquet("trades/**/*.parquet")        # lazy; files, lists, globs, directories
    df = pt.read_tparquet("t.parquet", columns=["ts", "price"])
    df.tparquet.write("out.parquet")                     # PCODEC where it is smaller
    lf.filter(...).tparquet.sink("out.parquet")          # streams the query into one file

Reading and writing are tparquet's Rust core; batches reach Polars through the Arrow C
interface. Each file is a Polars IO source, so a query's projection, row limit and predicate
are pushed to the reader, and the predicate also skips row groups by their statistics. Files
that are not PCODEC are read too.
"""

from __future__ import annotations

import contextlib
import errno
import glob
import os
import shutil
import uuid
from collections.abc import Iterator, Mapping, Sequence
from typing import Union

import polars as pl
from tparquet import PcodecError, Policy, Sink, Source, Writer
from tparquet.polars import scan_parquet, write_parquet

__all__ = ["DataFrameNamespace", "LazyFrameNamespace", "read_tparquet", "scan_tparquet"]

#: One file, or several: a list of paths and file objects, a glob, or a directory.
Sources = Union[Source, Sequence[Source]]

_GLOB = frozenset("*?[")


def _is_path(source: object) -> bool:
    return isinstance(source, (str, bytes, os.PathLike))


def _expand(source: Source) -> Sequence[Source]:
    """The files one source names: a glob's matches and a directory's .parquet files, sorted."""
    if not _is_path(source):
        return [source]
    name = os.path.expanduser(os.fsdecode(source))  # type: ignore[arg-type]
    # a path that exists names that file or directory, whatever characters it holds
    if _GLOB & set(name) and not os.path.exists(name):
        # the files it matches: a directory matched (by `**` or `*`) is not one
        matches = sorted(m for m in glob.glob(name, recursive=True) if os.path.isfile(m))
        if not matches:
            raise FileNotFoundError(f"no file matches {name}")
        return matches
    if os.path.isdir(name):
        below = _parquet_files(name)
        if not below:
            raise FileNotFoundError(f"no .parquet file in {name}")
        return below
    return [name]


def _parquet_files(folder: str) -> list[str]:
    """The .parquet files (A.PARQUET too) below `folder`, sorted. Hidden files and directories
    (whose names start with a dot: unfinished files of other writers, macOS `._` files) are
    skipped, as a glob skips them, and symlinks to directories are not followed (symlinks to
    files are)."""
    found: list[str] = []
    for top, folders, names in os.walk(folder):
        folders[:] = [f for f in folders if not f.startswith(".")]
        for n in names:
            path = os.path.join(top, n)
            if not n.startswith(".") and n.lower().endswith(".parquet") and os.path.isfile(path):
                found.append(path)
    return sorted(found)


def _files(source: Sources) -> list[Source]:
    if isinstance(source, (list, tuple)):
        if not source:
            raise ValueError("no files to scan")
        return [f for s in source for f in _expand(s)]
    return list(_expand(source))  # type: ignore[arg-type]


def _name(source: Source) -> str:
    return os.fsdecode(source) if _is_path(source) else repr(source)  # type: ignore[arg-type]


def _writable(schema: pl.Schema) -> pl.Schema:
    """`schema`, if it can be written: Polars exports the values of an Object column (which
    cannot be nested) as the addresses of its Python objects, so those are refused, as Polars'
    own writer refuses them."""
    for name, dtype in schema.items():
        if dtype == pl.Object:
            raise pl.exceptions.ComputeError(
                f"cannot write column {name!r} of dtype Object: its values are Python objects"
            )
    return schema


@contextlib.contextmanager
def _replacing(target: Sink) -> Iterator[Sink]:
    """Where to write `target`. A file object is written in place. A path is written as a hidden
    temporary file beside it (beside the file it links to), which replaces it once complete and
    is removed if writing fails: the target is never seen half written, a failure leaves it as it
    was, and a query can write over a file it reads."""
    if not _is_path(target):
        yield target
        return
    path = os.path.realpath(os.path.expanduser(os.fsdecode(target)))  # type: ignore[arg-type]
    folder, name = os.path.split(path)
    if not os.path.isdir(folder):
        raise FileNotFoundError(errno.ENOENT, "no such directory", folder)
    # replacing needs the right to write the directory only: a file that may not be written is
    # refused, as writing it in place would be
    if os.path.exists(path) and not os.access(path, os.W_OK):
        raise PermissionError(errno.EACCES, "Permission denied", path)
    temporary = os.path.join(folder, f".{name}.{uuid.uuid4().hex}.tmp")
    try:
        yield temporary
        with contextlib.suppress(FileNotFoundError):  # a file replaced keeps its permissions
            shutil.copymode(path, temporary)
        os.replace(temporary, path)
    except BaseException:
        with contextlib.suppress(FileNotFoundError):
            os.remove(temporary)
        raise


def _scan(f: Source, batch_size: int | None) -> pl.LazyFrame:
    """One file as a LazyFrame; an error in its footer names the file."""
    # tparquet opens the file here and keeps it open, so a relative path is the file it named
    # now, whatever chdir follows before the query runs
    try:
        return scan_parquet(f, batch_size=batch_size)
    except PcodecError as e:
        raise PcodecError(f"{_name(f)}: {e}") from e


def scan_tparquet(
    source: Sources, *, batch_size: int | None = None, include_file_paths: str | None = None
) -> pl.LazyFrame:
    """A LazyFrame over Parquet files (PCODEC or not), read in order.

    `source` is a path (a leading `~` is the home directory), a binary file object, a glob
    (`*`, `?`, `[...]`; `**` matches directories too), a directory (every `.parquet` file below
    it, hidden ones skipped), or a list of these. A path that exists is never a glob. Globs and
    directories are read in sorted order, lists in the order given. A file object is read when
    scanned, a relative path resolved. All files must have the same schema.

    Each file is a Polars IO source: the projection, row limit and predicate of the query reach
    the reader, and the predicate skips row groups whose statistics rule it out. Batches have
    `batch_size` rows, else the size the engine asks for. `include_file_paths` adds a String
    column of that name holding each row's file path.
    """
    frames: list[pl.LazyFrame] = []
    first: tuple[str, pl.Schema] | None = None
    # a file object is read (to its end) when scanned: one listed twice is scanned once
    objects: dict[int, pl.LazyFrame] = {}
    for f in _files(source):
        if isinstance(f, str):
            lf = _scan(f, batch_size)
        else:
            if id(f) not in objects:
                objects[id(f)] = _scan(f, batch_size)
            lf = objects[id(f)]
        schema = lf.collect_schema()
        if first is None:
            first = (_name(f), schema)
        elif schema != first[1]:
            raise pl.exceptions.SchemaError(
                f"{_name(f)}: schema differs from {first[0]}: {dict(schema)} != {dict(first[1])}"
            )
        if include_file_paths is not None:
            if not _is_path(f):
                raise ValueError("include_file_paths needs file paths, not file objects")
            if include_file_paths in schema:
                raise pl.exceptions.DuplicateError(f"{_name(f)} already has a column {include_file_paths!r}")
            # a Polars string is UTF-8: a byte of the name that is not becomes one U+FFFD
            # ("UTF-8", a mutant, names the same codec as "utf-8": no test can tell them apart)
            shown = os.fsencode(_name(f)).decode("utf-8", "replace")  # pragma: no mutate
            lf = lf.with_columns(pl.lit(shown).alias(include_file_paths))
        frames.append(lf)
    return pl.concat(frames)


def read_tparquet(
    source: Sources,
    *,
    columns: Sequence[str] | Sequence[int] | None = None,
    n_rows: int | None = None,
    batch_size: int | None = None,
    include_file_paths: str | None = None,
) -> pl.DataFrame:
    """Parquet files (PCODEC or not) as one DataFrame: `scan_tparquet`, with `columns` (names, or
    indices as Polars' `read_parquet` takes them) in the order given and at most `n_rows` rows,
    collected."""
    if n_rows is not None and n_rows < 0:
        raise ValueError(f"n_rows must not be negative, got {n_rows}")
    lf = scan_tparquet(source, batch_size=batch_size, include_file_paths=include_file_paths)
    if columns is not None:
        picked = [columns] if isinstance(columns, str) else columns
        # an int given to select would be a literal, not the column at that index
        lf = lf.select(pl.nth(c) if isinstance(c, int) else pl.col(c) for c in picked)
    if n_rows is not None:
        lf = lf.head(n_rows)
    return lf.collect()


@pl.api.register_dataframe_namespace("tparquet")
class DataFrameNamespace:
    """`df.tparquet`: writes the frame as Parquet with the PCODEC codec."""

    def __init__(self, df: pl.DataFrame) -> None:
        self._df = df

    def write(
        self,
        target: Sink,
        *,
        policy: Policy = "auto",
        columns: Mapping[str, Policy] | None = None,
        level: int = 8,
        # "ZSTD(3)" (a mutant) names the same codec as "zstd(3)": no test can tell them apart
        base: str = "zstd(3)",  # pragma: no mutate
        row_group_size: int | None = None,
    ) -> dict[str, str]:
        """Writes the frame to `target` (a path or a binary file object) and returns the codec of
        every leaf column.

        With `policy="auto"` each INT32/INT64/FLOAT/DOUBLE column gets whichever of PCODEC and the
        base codec is smaller; `"pcodec"` and `"base"` force one. `columns` maps a column name or
        dotted leaf path to its own policy, `level` is the pcodec level (0 to 12), `base` the codec
        of the other columns (`"zstd(3)"`, `"snappy"`, ...) and `row_group_size` the most rows
        per row group.

        A path is written as a hidden temporary file beside it, which replaces it once complete:
        if writing fails, the target is left as it was.
        """
        _writable(self._df.schema)
        with _replacing(target) as sink:
            return write_parquet(
                self._df, sink, policy=policy, columns=columns, level=level, base=base, row_group_size=row_group_size
            )


@pl.api.register_lazyframe_namespace("tparquet")
class LazyFrameNamespace:
    """`lf.tparquet`: streams the query's result into a Parquet file with the PCODEC codec."""

    def __init__(self, lf: pl.LazyFrame) -> None:
        self._lf = lf

    def sink(
        self,
        target: Sink,
        *,
        policy: Policy = "auto",
        columns: Mapping[str, Policy] | None = None,
        level: int = 8,
        # "ZSTD(3)" (a mutant) names the same codec as "zstd(3)": no test can tell them apart
        base: str = "zstd(3)",  # pragma: no mutate
        row_group_size: int | None = None,
        chunk_size: int | None = None,
    ) -> dict[str, str]:
        """Runs the query and writes its result to `target` batch by batch (`chunk_size` rows,
        else the engine's), never holding all of it; returns the codec of every leaf column.

        The options are those of `DataFrame.tparquet.write`; with `policy="auto"` the codecs are
        chosen on the first batch with rows. A path is written as a hidden temporary file beside
        it, which replaces it once complete: if the query fails, the target is left as it was,
        and a query may write over a file it reads.
        """
        if chunk_size is not None and chunk_size <= 0:
            raise ValueError(f"chunk_size must be positive, got {chunk_size}")
        schema = _writable(self._lf.collect_schema())
        # the writer is closed on a normal exit and aborted on an exception
        with (
            _replacing(target) as sink,
            Writer(
                sink, schema, policy=policy, columns=columns, level=level, base=base, row_group_size=row_group_size
            ) as writer,
        ):
            for batch in self._lf.collect_batches(chunk_size=chunk_size):
                writer.write(batch)
        codecs = writer.codecs
        assert codecs is not None  # closed: the codecs are chosen
        return codecs
