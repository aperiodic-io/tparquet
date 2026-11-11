"""Canonical JSON (testdata/README.md) of a DuckDB result, fetched as Arrow.

Floats are written from their IEEE bits (`Array.view` reinterprets the buffer, so NaN payloads
and signed zeros survive), decimals as their unscaled integer, temporal values as the integer
they are stored as. DuckDB keeps time-zoned timestamps in microseconds (or nanoseconds), so a
file column stored in another unit is converted back to it: `file_schema` gives the units.
"""

import base64
import decimal
import json

import pyarrow as pa

_UNIT = {"s": 1, "ms": 1_000, "us": 1_000_000, "ns": 1_000_000_000}
_TIME_UNIT = {"s": 1, "ms": 1_000, "us": 1_000_000, "ns": 1_000_000_000}


def _string(s: str) -> str:
    # json.dumps with ensure_ascii=False escapes exactly ", \, \b \t \n \f \r and the other
    # characters below U+0020 as lower-case \u00xx, and nothing else
    return json.dumps(s, ensure_ascii=False)


def _rescale(values, have: str, want: str):
    """Integers in unit `have` to unit `want`; the conversion must be exact."""
    num, den = _UNIT[want], _UNIT[have]
    out = []
    for v in values:
        if v is None:
            out.append(None)
            continue
        scaled = v * num
        if scaled % den:
            raise AssertionError(f"{v} {have} is not a whole number of {want}")
        out.append(scaled // den)
    return out


def _base(t: pa.DataType) -> pa.DataType:
    return t.value_type if pa.types.is_dictionary(t) else t


def values(arr: pa.Array, file_type: pa.DataType) -> list:
    """The canonical JSON text of every element of `arr` (None for null)."""
    if pa.types.is_dictionary(arr.type):
        arr = arr.dictionary_decode()
    if isinstance(arr.type, pa.ExtensionType):
        arr = arr.storage
    t = arr.type
    want = _base(file_type)
    valid = [v.is_valid for v in arr] if arr.null_count else [True] * len(arr)

    def wrap(texts):
        return [x if ok else "null" for x, ok in zip(texts, valid)]

    if pa.types.is_list(t) or pa.types.is_large_list(t):
        offsets = arr.offsets.to_pylist()
        child = values(arr.values, _base(want).value_type)
        return wrap(["[" + ",".join(child[offsets[i] : offsets[i + 1]]) + "]" for i in range(len(arr))])
    if pa.types.is_fixed_size_list(t):
        n = t.list_size
        child = values(arr.values, _base(want).value_type)
        start = arr.offset * n
        return wrap(["[" + ",".join(child[start + i * n : start + (i + 1) * n]) + "]" for i in range(len(arr))])
    if pa.types.is_map(t):
        offsets = arr.offsets.to_pylist()
        keys = values(arr.keys, want.key_type)
        items = values(arr.items, want.item_type)
        rows = []
        for i in range(len(arr)):
            entries = [f'{{"key":{keys[j]},"value":{items[j]}}}' for j in range(offsets[i], offsets[i + 1])]
            rows.append("[" + ",".join(entries) + "]")
        return wrap(rows)
    if pa.types.is_struct(t):
        children = [values(arr.field(i), want.field(i).type) for i in range(t.num_fields)]
        names = [_string(t.field(i).name) for i in range(t.num_fields)]
        rows = []
        for r in range(len(arr)):
            rows.append("{" + ",".join(f"{n}:{c[r]}" for n, c in zip(names, children)) + "}")
        return wrap(rows)
    if pa.types.is_floating(t):
        width = {16: pa.uint16(), 32: pa.uint32(), 64: pa.uint64()}[t.bit_width]
        digits = t.bit_width // 4
        if _base(want).bit_width != t.bit_width:
            raise AssertionError(f"float width {t} vs file {want}")
        bits = arr.view(width).to_pylist()
        return wrap([f'"0x{b:0{digits}x}"' if b is not None else "null" for b in bits])
    if pa.types.is_decimal(t):
        with decimal.localcontext() as ctx:
            ctx.prec = 80
            return wrap(["null" if d is None else str(int(d.scaleb(t.scale))) for d in arr.to_pylist()])
    if pa.types.is_timestamp(t):
        raw = arr.view(pa.int64()).to_pylist()
        return wrap(["null" if v is None else str(v) for v in _rescale(raw, t.unit, want.unit)])
    if pa.types.is_time(t):
        raw = arr.view(pa.int64() if t.bit_width == 64 else pa.int32()).to_pylist()
        return wrap(["null" if v is None else str(v) for v in _rescale(raw, t.unit, want.unit)])
    if pa.types.is_date32(t):
        return wrap(["null" if v is None else str(v) for v in arr.view(pa.int32()).to_pylist()])
    if pa.types.is_interval(t):
        # DuckDB's INTERVAL arrives as month-day-nano, the canonical form's fields
        return wrap(
            ["null" if v is None else f'{{"months":{v[0]},"days":{v[1]},"nanoseconds":{v[2]}}}' for v in arr.to_pylist()]
        )
    if pa.types.is_boolean(t):
        return wrap(["null" if v is None else ("true" if v else "false") for v in arr.to_pylist()])
    if pa.types.is_integer(t):
        return wrap(["null" if v is None else str(v) for v in arr.to_pylist()])
    if pa.types.is_string(t) or pa.types.is_large_string(t) or pa.types.is_string_view(t):
        return wrap(["null" if v is None else _string(v) for v in arr.to_pylist()])
    if pa.types.is_binary(t) or pa.types.is_large_binary(t) or pa.types.is_fixed_size_binary(t) or pa.types.is_binary_view(t):
        return wrap(["null" if v is None else _string(base64.b64encode(v).decode()) for v in arr.to_pylist()])
    if pa.types.is_interval(t):
        # not in the testdata format (no vector has one): months, days and nanoseconds
        return wrap(["null" if v is None else f"[{v.months},{v.days},{v.nanoseconds}]" for v in arr.to_pylist()])
    raise AssertionError(f"no canonical form for {t}")


def lines(table: pa.Table, file_schema: pa.Schema) -> str:
    """The canonical JSONL of a table whose columns are those of `file_schema`, in order."""
    assert table.column_names == file_schema.names, (table.column_names, file_schema.names)
    columns = []
    for name, col in zip(table.column_names, table.columns):
        arr = col.combine_chunks() if col.num_chunks != 1 else col.chunk(0)
        columns.append((_string(name), values(arr, file_schema.field(name).type)))
    out = []
    for r in range(table.num_rows):
        out.append("{" + ",".join(f"{n}:{c[r]}" for n, c in columns) + "}\n")
    return "".join(out)


def rows_of(table: pa.Table) -> list:
    """The canonical text of every row of a DuckDB result, bit for bit (floats by their bits),
    in order; sort it to compare rows in any order."""
    columns = []
    for col in table.columns:
        arr = col.combine_chunks() if col.num_chunks != 1 else col.chunk(0)
        if isinstance(arr, pa.ChunkedArray):
            arr = pa.concat_arrays(arr.chunks) if arr.num_chunks else pa.array([], arr.type)
        columns.append(values(arr, arr.type.storage_type if isinstance(arr.type, pa.ExtensionType) else arr.type))
    return ["[" + ",".join(c[r] for c in columns) + "]" for r in range(table.num_rows)]
