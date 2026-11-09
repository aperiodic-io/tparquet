"""pandas DataFrames to and from PCODEC Parquet files (needs pandas and pyarrow).

The pandas metadata pyarrow writes (index, column dtypes, categoricals, time zones) is kept,
so a DataFrame round-trips as it does with `DataFrame.to_parquet(engine="pyarrow")` and
`pandas.read_parquet`.
"""

from __future__ import annotations

import base64
import json
from typing import TYPE_CHECKING, Any

import pandas as pd
import pyarrow as pa
import pyarrow.compute as pc

from tparquet._core import ParquetFile, Policy, Sink, Source, _names, write_table

if TYPE_CHECKING:
    from collections.abc import Mapping, Sequence

__all__ = ["read_parquet", "to_parquet"]


#: Schema metadata key: per dictionary column, its categories (base64 of an Arrow IPC stream
#: of one array), as JSON. The Parquet writer re-encodes dictionaries in order of first
#: appearance, which loses the category order and the unused categories; they are restored on
#: read.
CATEGORIES_KEY = "tparquet.pandas.categories"


def _ipc(categories: pa.Array[Any]) -> str:
    sink = pa.BufferOutputStream()
    batch = pa.RecordBatch.from_arrays([categories], ["categories"])
    with pa.ipc.new_stream(sink, batch.schema) as w:
        w.write_batch(batch)
    return base64.b64encode(sink.getvalue().to_pybytes()).decode()


def _from_ipc(data: str) -> pa.Array[Any]:
    stream = pa.ipc.open_stream(base64.b64decode(data)).read_all()
    categories: pa.Array[Any] = stream.column("categories").combine_chunks()
    return categories


def _encode_categories(table: pa.Table) -> pa.Table:
    """`table` with the categories of its dictionary columns in the schema metadata."""
    # Table.from_pandas gives every column one chunk, holding the categories
    stored = {
        f.name: _ipc(table.column(i).chunk(0).dictionary)  # type: ignore[attr-defined]
        for i, f in enumerate(table.schema)
        if pa.types.is_dictionary(f.type)
    }
    if not stored:
        return table
    metadata = dict(table.schema.metadata or {})
    metadata[CATEGORIES_KEY.encode()] = json.dumps(stored).encode()
    return table.replace_schema_metadata(metadata)


def _decode_categories(table: pa.Table) -> pa.Table:
    """`table` with the categories its metadata records put back, in their order."""
    stored = (table.schema.metadata or {}).get(CATEGORIES_KEY.encode())
    if stored is None:
        return table
    for name, data in json.loads(stored).items():
        i = table.schema.get_field_index(name)
        if i < 0:
            continue
        categories = _from_ipc(data)
        field = table.schema.field(i)
        t = field.type
        # a table without rows may have no chunks: one empty chunk then carries the categories
        stored_chunks: list[Any] = table.column(i).chunks or [pa.array([], t)]
        chunks = [
            pa.DictionaryArray.from_arrays(
                pc.index_in(chunk.dictionary, value_set=categories).cast(t.index_type).take(chunk.indices),
                categories,
                ordered=t.ordered,
            )
            for chunk in stored_chunks
        ]
        table = table.set_column(i, field, pa.chunked_array(chunks))
    return table


def _index_columns(schema: pa.Schema) -> list[str]:
    """The stored index columns the pandas metadata names (a RangeIndex is not stored)."""
    metadata = schema.pandas_metadata or {}
    return [c for c in metadata.get("index_columns", []) if isinstance(c, str)]


def read_parquet(source: Source, columns: Sequence[str] | None = None, **to_pandas_kwargs: Any) -> pd.DataFrame:
    """Reads a Parquet file (PCODEC or not) as a DataFrame, restoring the index and dtypes
    from the pandas metadata.

    `columns` selects data columns; the index columns are read too, as `pandas.read_parquet`
    does. The keyword arguments go to `pyarrow.Table.to_pandas` (`types_mapper`,
    `categories`...).
    """
    f = ParquetFile(source)
    wanted = _names(columns)
    if wanted is not None:
        columns = wanted + [c for c in _index_columns(f.schema) if c not in wanted]
    return _decode_categories(f.read(columns=columns)).to_pandas(**to_pandas_kwargs)


def to_parquet(
    df: pd.DataFrame,
    sink: Sink,
    index: bool | None = None,
    *,
    policy: Policy = "auto",
    columns: Mapping[str, Policy] | None = None,
    level: int = 8,
    # "ZSTD(3)" (a mutant) names the same codec as "zstd(3)": no test can tell them apart
    base: str = "zstd(3)",  # pragma: no mutate
    row_group_size: int | None = None,
    encodings: Mapping[str, str] | None = None,
) -> dict[str, str]:
    """Writes a DataFrame as a Parquet file with PCODEC where the policy selects it.

    `index` is as in `DataFrame.to_parquet`: `None` stores a RangeIndex as metadata only and
    any other index as columns, `True` always stores it, `False` drops it. The other options
    are those of `tparquet.write_table`; returns the codec of every leaf column.
    """
    if not isinstance(df, pd.DataFrame):
        raise TypeError(f"expected a pandas DataFrame, got {type(df).__name__}")
    table = _encode_categories(pa.Table.from_pandas(df, preserve_index=index))
    return write_table(
        table,
        sink,
        policy=policy,
        columns=columns,
        level=level,
        base=base,
        row_group_size=row_group_size,
        encodings=encodings,
    )
