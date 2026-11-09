#!/usr/bin/env python3
"""Signs built tparquet extensions with an RSA 2048 key, as DuckDB signs its own.

    duckdb/sign.py PRIVATE_KEY.pem EXTENSION...

DuckDB checks the signature against the public keys of the extension repository the
extension is installed from (CREATE EXTENSION REPOSITORY ... USING PUBLIC KEY; duckdb/README.md,
"Signing"), so a signed extension loads without allow_unsigned_extensions.

The signed digest is DuckDB's (ExtensionHelper::CheckExtensionSignature): the file without its
last 256 bytes (the signature) cut in chunks of 1 MiB, the SHA-256 of each chunk, and the
SHA-256 of their concatenation. The signature is an RSA PKCS#1 v1.5 signature of that digest as
a SHA-256 digest (`openssl pkeyutl -sign -pkeyopt digest:sha256`), written over the last 256
bytes.
"""

from __future__ import annotations

import hashlib
import pathlib
import subprocess
import sys
import tempfile

FIELD_LEN = 32
SIGNATURE_LEN = 256
FOOTER_LEN = 8 * FIELD_LEN + SIGNATURE_LEN
MAGIC = "4"
CHUNK = 1 << 20


def metadata(data: bytes) -> dict[str, str] | None:
    """The metadata footer DuckDB reads at the end of an extension (`None` without its magic)."""
    if len(data) < FOOTER_LEN:
        return None
    start = len(data) - FOOTER_LEN
    raw = [data[start + i * FIELD_LEN : start + (i + 1) * FIELD_LEN] for i in range(8)][::-1]
    fields = [f.rstrip(b"\0").decode("utf-8", "replace") for f in raw]
    if fields[0] != MAGIC:
        return None
    return {"platform": fields[1], "duckdb_version": fields[2], "extension_version": fields[3], "abi_type": fields[4]}


def signing_hash(data: bytes) -> bytes:
    if metadata(data) is None:
        raise ValueError("not a packaged extension: no metadata footer")
    content = data[: len(data) - SIGNATURE_LEN]
    chunks = hashlib.sha256()
    for i in range(0, len(content), CHUNK):
        chunks.update(hashlib.sha256(content[i : i + CHUNK]).digest())
    return chunks.digest()


def with_signature(data: bytes, signature: bytes) -> bytes:
    if len(signature) != SIGNATURE_LEN:
        raise ValueError(f"a signature is {SIGNATURE_LEN} bytes (RSA 2048), not {len(signature)}")
    if metadata(data) is None:
        raise ValueError("not a packaged extension: no metadata footer")
    return data[: len(data) - SIGNATURE_LEN] + signature


def sign(key: pathlib.Path, extension: pathlib.Path) -> None:
    data = extension.read_bytes()
    digest = signing_hash(data)
    with tempfile.TemporaryDirectory() as tmp:
        d = pathlib.Path(tmp) / "digest"
        d.write_bytes(digest)
        out = subprocess.run(
            ["openssl", "pkeyutl", "-sign", "-inkey", str(key), "-pkeyopt", "digest:sha256", "-in", str(d)],
            capture_output=True,
            check=False,
        )
        if out.returncode != 0:
            raise ValueError(f"openssl: {out.stderr.decode().strip()}")
    extension.write_bytes(with_signature(data, out.stdout))


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: duckdb/sign.py PRIVATE_KEY.pem EXTENSION...", file=sys.stderr)
        return 2
    key, *extensions = (pathlib.Path(a) for a in argv)
    for ext in extensions:
        try:
            sign(key, ext)
        except (OSError, ValueError) as e:
            print(f"sign: {ext}: {e}", file=sys.stderr)
            return 1
        print(f"signed {ext}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
