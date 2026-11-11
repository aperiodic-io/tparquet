"""Canonical JSON of Arrow data, as `testdata/README.md` defines it (needs pyarrow).

One compact JSON object per row: integers (and everything stored as an integer: timestamps,
dates, times, durations, unscaled decimals) as plain numbers, floats as the hex of their IEEE
bits, intervals as months, days and nanoseconds, binary as base64, lists as arrays, structs as
objects, maps as arrays of entry objects, dictionaries and run-end encoded arrays decoded. Two
rows are equal exactly when their lines are equal byte for byte.
"""

from __future__ import annotations

import base64
import json
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Iterator

    import pyarrow as pa


def _string(s: str) -> str:
    # mutmut's `ensure_ascii=None` is equivalent (falsy like False): no test can tell
    return json.dumps(s, ensure_ascii=False)  # pragma: no mutate


def _is_float(t: Any, pa: Any) -> int:
    """The bit width of a float type, else 0."""
    if pa.types.is_float16(t):
        return 16
    if pa.types.is_float32(t):
        return 32
    if pa.types.is_float64(t):
        return 64
    return 0


def _stored_integer(t: Any, pa: Any) -> Any:
    """The integer type a temporal type is stored as, else `None`."""
    if pa.types.is_date32(t) or pa.types.is_time32(t):
        return pa.int32()
    if pa.types.is_timestamp(t) or pa.types.is_date64(t) or pa.types.is_time64(t) or pa.types.is_duration(t):
        return pa.int64()
    return None


def _interval(months: int, days: int, nanoseconds: int) -> str:
    return f'{{"months":{months},"days":{days},"nanoseconds":{nanoseconds}}}'


def _decimals(arr: Any) -> list[str]:
    width = arr.type.byte_width
    data = memoryview(arr.buffers()[1])
    start = arr.offset * width
    return [
        str(int.from_bytes(data[start + i * width : start + (i + 1) * width], "little", signed=True))
        for i in range(len(arr))
    ]


def _join(children: list[str], starts: list[int], ends: list[int]) -> list[str]:
    return ["[" + ",".join(children[s:e]) + "]" for s, e in zip(starts, ends)]


def _values(arr: Any, pa: Any) -> list[str]:
    """The canonical JSON of every element of `arr`."""
    t = arr.type
    if isinstance(arr, pa.ExtensionArray):
        return _values(arr.storage, pa)
    if pa.types.is_dictionary(t):
        return _values(arr.dictionary_decode(), pa)
    if pa.types.is_run_end_encoded(t):
        import pyarrow.compute as pc

        return _values(pc.run_end_decode(arr), pa)  # type: ignore[attr-defined]  # not in pyarrow-stubs
    if pa.types.is_null(t):
        return ["null"] * len(arr)
    out = _non_null_values(arr, t, pa)
    if arr.null_count:
        out = [v if valid else "null" for v, valid in zip(out, arr.is_valid().to_pylist())]
    return out


def _non_null_values(arr: Any, t: Any, pa: Any) -> list[str]:
    """`_values` where null elements may hold anything."""
    if pa.types.is_boolean(t):
        return ["true" if v else "false" for v in arr.to_pylist()]
    if pa.types.is_integer(t):
        return [str(v) for v in arr.to_pylist()]
    width = _is_float(t, pa)
    if width:
        unsigned = {16: pa.uint16(), 32: pa.uint32(), 64: pa.uint64()}[width]
        digits = width // 4
        return [f'"0x{v or 0:0{digits}x}"' for v in arr.view(unsigned).to_pylist()]
    stored = _stored_integer(t, pa)
    if stored is not None:
        return [str(v) for v in arr.view(stored).to_pylist()]
    if pa.types.is_decimal(t):
        return _decimals(arr)
    if pa.types.is_interval(t):
        # month-day-nano: pyarrow has no array class for year-month and day-time intervals.
        # `_values` replaces what a null gets here, so mutants of the (0, 0, 0) are equivalent
        return [_interval(*(v or (0, 0, 0))) for v in arr.to_pylist()]  # pragma: no mutate
    if pa.types.is_string(t) or pa.types.is_large_string(t) or pa.types.is_string_view(t):
        return [_string(v or "") for v in arr.to_pylist()]
    if (
        pa.types.is_binary(t)
        or pa.types.is_large_binary(t)
        or pa.types.is_binary_view(t)
        or pa.types.is_fixed_size_binary(t)
    ):
        return ['"' + base64.b64encode(v or b"").decode() + '"' for v in arr.to_pylist()]
    if pa.types.is_list(t) or pa.types.is_large_list(t) or pa.types.is_map(t):
        offsets = arr.offsets.to_pylist()
        return _join(_values(arr.values, pa), offsets[:-1], offsets[1:])
    if pa.types.is_list_view(t) or pa.types.is_large_list_view(t):
        starts = arr.offsets.to_pylist()
        return _join(_values(arr.values, pa), starts, [s + n for s, n in zip(starts, arr.sizes.to_pylist())])
    if pa.types.is_fixed_size_list(t):
        size = t.list_size
        starts = [(arr.offset + i) * size for i in range(len(arr))]
        return _join(_values(arr.values, pa), starts, [s + size for s in starts])
    if pa.types.is_struct(t):
        names = [_string(t.field(i).name) + ":" for i in range(t.num_fields)]
        fields = [_values(child, pa) for child in arr.flatten()]
        return ["{" + ",".join(n + f[i] for n, f in zip(names, fields)) + "}" for i in range(len(arr))]
    raise TypeError(f"canonical JSON does not cover {t}")


def _batches(data: object, pa: Any) -> Iterator[pa.RecordBatch]:
    if isinstance(data, pa.RecordBatch):
        yield data
    elif isinstance(data, pa.Table):
        yield from data.to_batches()
    elif hasattr(data, "__arrow_c_stream__"):
        yield from pa.RecordBatchReader.from_stream(data)
    elif hasattr(data, "__arrow_c_array__"):
        yield pa.record_batch(data)
    else:
        raise TypeError(f"expected Arrow data (a table, a record batch or a stream), got {type(data).__name__}")


def lines(data: object) -> Iterator[str]:
    """The canonical JSON line of every row (without `\\n`) of a `pyarrow` table or record
    batch, or of any object with `__arrow_c_stream__` or `__arrow_c_array__`."""
    import pyarrow as pa

    for batch in _batches(data, pa):
        keys = [_string(name) + ":" for name in batch.schema.names]
        columns = [_values(c, pa) for c in batch.columns]
        for row in range(batch.num_rows):
            yield "{" + ",".join(k + c[row] for k, c in zip(keys, columns)) + "}"


def dumps(data: object) -> str:
    """The canonical JSON Lines text of `data`: every line followed by `\\n`."""
    return "".join(line + "\n" for line in lines(data))
