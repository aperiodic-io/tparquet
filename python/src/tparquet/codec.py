"""The PCODEC page payload on its own (`spec/FORMAT.md` section 4).

A payload is what a PCODEC column chunk stores in place of a page's compressed bytes. It
describes itself: decoding needs only the payload and the size it must decode to.
"""

from __future__ import annotations

from typing import Literal

from tparquet import _lib
from tparquet._core import _int

#: `CompressionCodec.PCODEC` in the Parquet footer.
CODEC_ID: int = _lib.CODEC_ID
#: The payload format version this package reads and writes.
FORMAT_VERSION: int = _lib.FORMAT_VERSION
#: The footer key that marks files with PCODEC chunks.
KV_KEY: str = _lib.KV_KEY

#: A Parquet physical type, as the encoder's type hint.
Hint = Literal["INT32", "INT64", "FLOAT", "DOUBLE"]
#: How a payload is written.
Mode = Literal["RAW", "PCO", "DECIMAL", "ZSTD"]


def compress(data: bytes, hint: Hint | None = None, level: int = 8) -> bytes:
    """Encodes `data` (the bytes Parquet would compress) as a payload.

    `hint` is the column's physical type: with it numbers are stored with pcodec (and
    decimal-valued floats as exact integers); without it the payload is ZSTD or RAW.
    `level` is pcodec's compression level, 0 to 12.
    """
    return _lib.compress(data, hint, _int("level", level))


def decompress(payload: bytes, expected_len: int) -> bytes:
    """Decodes a payload, which must produce exactly `expected_len` bytes; raises
    `PcodecError` on any malformed payload."""
    return _lib.decompress(payload, _int("expected_len", expected_len))


def mode(payload: bytes) -> Mode:
    """The mode a payload was written with."""
    m: Mode = _lib.mode(payload)  # type: ignore[assignment]
    return m
