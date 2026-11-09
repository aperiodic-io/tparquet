"""The same record batches written as ZSTD(9) Parquet (untuned, tuned, ALP) and as tparquet, then read back."""

from __future__ import annotations

import dataclasses
import time
from typing import TYPE_CHECKING, Any, cast

import pyarrow as pa
import pyarrow.parquet as pq
import tparquet as tpq

if TYPE_CHECKING:
    import pathlib
    from collections.abc import Callable, Iterable, Iterator

# rows per batch handed to the writers: the size of a row group, so that tparquet's per-column
# choice (made on the first batch) sees as much data as a writer of a whole table would
BATCH_ROWS = 1 << 20


@dataclasses.dataclass(frozen=True)
class Measurement:
    rows: int
    # in-memory size of the Arrow batches written: the denominator of the speeds
    arrow_bytes: int
    zstd9_bytes: int
    tuned_bytes: int
    alp_bytes: int
    tparquet_bytes: int
    write_s_zstd9: float
    write_s_tuned: float
    write_s_alp: float
    write_s_tparquet: float
    read_s_zstd9: float
    read_s_tuned: float
    read_s_alp: float
    read_s_tparquet: float
    # dotted leaf path -> codec of the tparquet file
    codecs: dict[str, str]
    # top-level column -> encoding of the tuned file
    encodings: dict[str, str]
    # top-level column -> encoding of the ALP file
    alp_encodings: dict[str, str]

    @property
    def ratio(self) -> float:
        return self.tparquet_bytes / self.zstd9_bytes

    @property
    def ratio_tuned(self) -> float:
        return self.tparquet_bytes / self.tuned_bytes

    @property
    def ratio_alp(self) -> float:
        return self.tparquet_bytes / self.alp_bytes

    @property
    def alp_columns(self) -> int:
        return sum(e == ALP for e in self.alp_encodings.values())

    @property
    def pcodec_columns(self) -> int:
        return sum(c == "PCODEC" for c in self.codecs.values())


DICTIONARY = "dictionary"


def candidates(t: pa.DataType) -> list[str]:
    """The encodings the tuned baseline tries on a column of type `t`: the writer's default
    (dictionary, falling back to PLAIN), and the one Parquet has for the type."""
    if pa.types.is_integer(t) or pa.types.is_timestamp(t):
        return [DICTIONARY, "DELTA_BINARY_PACKED"]
    if pa.types.is_floating(t):
        return [DICTIONARY, "BYTE_STREAM_SPLIT"]
    if pa.types.is_string(t) or pa.types.is_large_string(t):
        return [DICTIONARY, "DELTA_BYTE_ARRAY", "DELTA_LENGTH_BYTE_ARRAY"]
    return [DICTIONARY]


def _write_tuned(where: pathlib.Path | pa.NativeFile, schema: pa.Schema, encodings: dict[str, str]) -> pq.ParquetWriter:
    column_encoding = {n: e for n, e in encodings.items() if e != DICTIONARY}
    return pq.ParquetWriter(
        where,
        schema,
        compression="zstd",  # pragma: no mutate (pyarrow ignores the case)
        compression_level=9,
        # a list of columns, as documented (the stubs only know bool)
        use_dictionary=[f.name for f in schema if f.name not in column_encoding],  # type: ignore[arg-type]
        column_encoding=column_encoding or None,
    )


def encoding_sizes(column: pa.Array[Any]) -> dict[str, int]:
    """The bytes of `column` written alone, ZSTD(9), with each of its candidate encodings."""
    table = pa.table({"c": column})
    sizes = {}
    for encoding in candidates(column.type):
        sink = pa.BufferOutputStream()
        with _write_tuned(sink, table.schema, {"c": encoding}) as w:
            w.write_table(table)
        sizes[encoding] = sink.getvalue().size
    return sizes


def choose_encodings(batch: pa.RecordBatch) -> dict[str, str]:
    """Per column, the candidate encoding that writes `batch` smallest: tparquet chooses its
    codecs on the first batch, the tuned baseline its encodings."""
    out = {}
    for name, column in zip(batch.schema.names, batch.columns):
        sizes = encoding_sizes(column)
        out[name] = min(sizes, key=sizes.__getitem__)
    return out


ALP = "ALP"


def alp_encodings(schema: pa.Schema, tuned: dict[str, str]) -> dict[str, str]:
    """The encodings of the ALP baseline: ALP on every FLOAT and DOUBLE column, the tuned
    encoding on the others."""
    return {
        f.name: ALP if pa.types.is_float32(f.type) or pa.types.is_float64(f.type) else tuned[f.name] for f in schema
    }


def _write_alp(path: pathlib.Path, schema: pa.Schema, encodings: dict[str, str]) -> tpq.Writer:
    """pyarrow does not write ALP: tparquet's writer does (the `parquet` crate's encoder),
    without PCODEC, ZSTD level 9, a dictionary only where the encoding says so."""
    return tpq.Writer(
        path,
        schema,
        policy="base",
        base="zstd(9)",  # pragma: no mutate (tparquet ignores the case of the codec name)
        encodings={n: e for n, e in encodings.items() if e != DICTIONARY},
    )


def rebatch(batches: Iterable[pa.RecordBatch], rows: int) -> Iterator[pa.RecordBatch]:
    """`batches` as batches of exactly `rows` rows, but the last."""
    pending: list[pa.RecordBatch] = []
    have = 0
    for batch in batches:
        while batch.num_rows:
            take = min(rows - have, batch.num_rows)
            pending.append(batch.slice(0, take))
            batch = batch.slice(take)
            have += take
            if have == rows:
                yield pa.Table.from_batches(pending).combine_chunks().to_batches()[0]
                pending, have = [], 0
    if have:
        yield pa.Table.from_batches(pending).combine_chunks().to_batches()[0]


READS = 3


def _read(path: pathlib.Path) -> float:
    """Seconds of a full read with tparquet's reader, the best of `READS` (reads are quick, and
    a small file's single read is mostly noise)."""
    best = float("inf")  # pragma: no mutate (float() ignores the case)
    for _ in range(READS):
        start = time.perf_counter()
        for _ in tpq.read_batches(path):
            pass
        best = min(best, time.perf_counter() - start)
    return best


def measure(
    reader: pa.RecordBatchReader, workdir: pathlib.Path, batch_rows: int = BATCH_ROWS, keep: bool = False
) -> Measurement:
    """Writes `reader` to `workdir` four times, checks that the files hold the same data and
    reads each back in full with tparquet's reader:

    - ZSTD(9): tparquet's writer with ZSTD level 9 on every column, its defaults otherwise
      (dictionary, then PLAIN): what a writer gets without tuning;
    - ZSTD(9) tuned: pyarrow, ZSTD level 9, each column with the encoding of `candidates`
      that is smallest on the first batch;
    - ZSTD(9) ALP: the tuned encodings with ALP on every FLOAT and DOUBLE column, ZSTD level
      9, written by tparquet's writer without PCODEC (pyarrow does not write ALP);
    - tparquet: tparquet's writer with its defaults (policy auto: PCODEC or ZSTD level 3 per
      column, chosen on the first batch).

    Only the writers and the readers are timed, not the parsing of the source; the choice
    of the tuned encodings is not timed either.
    """
    paths = {name: workdir / f"{name}.parquet" for name in ("zstd9", "tuned", "alp", "tparquet")}
    z9 = tpq.Writer(
        paths["zstd9"],
        reader.schema,
        policy="base",
        base="zstd(9)",  # pragma: no mutate (tparquet ignores the case of the codec name)
    )
    auto = tpq.Writer(paths["tparquet"], reader.schema)
    tuned: pq.ParquetWriter | None = None
    alp: tpq.Writer | None = None
    encodings: dict[str, str]  # chosen on the first batch
    alp_enc: dict[str, str]
    rows = arrow_bytes = 0
    write = dict.fromkeys(paths, 0.0)

    def timed(name: str, f: Callable[[], object]) -> object:
        start = time.perf_counter()
        result = f()
        write[name] += time.perf_counter() - start
        return result

    try:
        for batch in rebatch(reader, batch_rows):
            if tuned is None:
                encodings = choose_encodings(batch)
                tuned = _write_tuned(paths["tuned"], reader.schema, encodings)
                alp_enc = alp_encodings(reader.schema, encodings)
                alp = _write_alp(paths["alp"], reader.schema, alp_enc)
            rows += batch.num_rows
            arrow_bytes += batch.nbytes
            timed("zstd9", lambda: z9.write(batch))  # noqa: B023 (called at once)
            timed("tuned", lambda: tuned.write_batch(batch))  # noqa: B023
            # opened with `tuned` on the first batch
            timed("alp", lambda: cast("tpq.Writer", alp).write(batch))  # noqa: B023
            timed("tparquet", lambda: auto.write(batch))  # noqa: B023
        if tuned is None or alp is None:
            raise ValueError("the source has no rows")
        timed("zstd9", z9.close)
        timed("tuned", tuned.close)
        timed("alp", alp.close)
        codecs = cast("dict[str, str]", timed("tparquet", auto.close))  # pragma: no mutate (typing only)
        for name in ("tuned", "alp", "tparquet"):
            difference = tpq.verify(paths["zstd9"], paths[name])
            if difference is not None:
                raise AssertionError(f"the {name} file differs from the ZSTD(9) file: {difference}")
        read = {name: _read(path) for name, path in paths.items()}
        return Measurement(
            rows=rows,
            arrow_bytes=arrow_bytes,
            zstd9_bytes=paths["zstd9"].stat().st_size,
            tuned_bytes=paths["tuned"].stat().st_size,
            alp_bytes=paths["alp"].stat().st_size,
            tparquet_bytes=paths["tparquet"].stat().st_size,
            write_s_zstd9=write["zstd9"],
            write_s_tuned=write["tuned"],
            write_s_alp=write["alp"],
            write_s_tparquet=write["tparquet"],
            read_s_zstd9=read["zstd9"],
            read_s_tuned=read["tuned"],
            read_s_alp=read["alp"],
            read_s_tparquet=read["tparquet"],
            codecs=codecs,
            encodings=encodings,
            alp_encodings=alp_enc,
        )
    finally:
        # abort() removes the file of a writer still open and does nothing once it is closed
        for w in (z9, auto, alp):
            if w is not None:
                w.abort()
        if tuned is not None and tuned.is_open:
            tuned.close()
        if not keep:
            for path in paths.values():
                path.unlink(missing_ok=True)
