"""Signed extensions load without allow_unsigned_extensions, from a repository that trusts the key.

The build under test is signed with a new RSA key (`sign.py`), put in a local extension
repository (`<prefix>/<DuckDB version>/<platform>/tparquet.duckdb_extension`), and installed and
loaded from that repository, whose public key DuckDB then trusts. A tampered file, a file signed
with another key and an unsigned file are refused.
"""

import pathlib
import shutil
import subprocess
import sys

import pytest
from conftest import DUCKDB_1, FILES, extension_path, rows
from sign import signing_hash

import duckdb

HERE = pathlib.Path(__file__).resolve().parent


def keypair(d: pathlib.Path, name: str):
    private = d / f"{name}.pem"
    subprocess.run(["openssl", "genrsa", "-out", private, "2048"], check=True, capture_output=True)
    public = subprocess.run(["openssl", "rsa", "-in", private, "-pubout"], check=True, capture_output=True, text=True)
    return private, public.stdout


def sign_with(key, ext):
    return subprocess.run([sys.executable, HERE.parent / "sign.py", key, ext], capture_output=True, text=True)


def sign(key, ext):
    out = sign_with(key, ext)
    assert out.returncode == 0, out.stderr
    return ext


def strict(tmp_path) -> duckdb.DuckDBPyConnection:
    """A database that loads only signed extensions."""
    config = {"allow_unsigned_extensions": "false", "extension_directory": str(tmp_path / "installed")}
    if not DUCKDB_1:
        config["allow_extension_repositories"] = "allowed"
    return duckdb.connect(config=config)


def trust(con, repo, key, tmp_path):
    """Creates a repository named after `tmp_path` (DuckDB keeps repositories beyond the
    database: the names must not clash between tests or runs); returns its name."""
    name = "tparquet_test_" + format(abs(hash(str(tmp_path))), "x")
    con.execute(f"CREATE OR REPLACE EXTENSION REPOSITORY {name} WITH PREFIX '{repo}' USING PUBLIC KEY '{key}'")
    return name


def forget(con, name):
    con.execute(f"DROP EXTENSION REPOSITORY IF EXISTS {name}")


def repository(tmp_path, con, ext, name="repo"):
    """A local repository holding `ext` as tparquet for this DuckDB."""
    version = rows(con, "SELECT library_version FROM pragma_version()")[0][0]
    platform = rows(con, "PRAGMA platform")[0][0]
    target = tmp_path / name / version / platform / "tparquet.duckdb_extension"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy(ext, target)
    return tmp_path / name


@pytest.fixture
def signed(tmp_path):
    private, public = keypair(tmp_path, "key")
    ext = tmp_path / "tparquet.duckdb_extension"
    shutil.copy(extension_path(), ext)
    return sign(private, ext), public


# DuckDB 1.x trusts only DuckDB's own keys: it has no extension repository with a key of its own
REPOSITORIES = pytest.mark.skipif(DUCKDB_1, reason="DuckDB 1.x: no extension repository with a public key")


def test_the_signature_verifies_with_the_public_key(tmp_path, signed):
    ext, public = signed
    data = ext.read_bytes()
    (tmp_path / "public.pem").write_text(public)
    (tmp_path / "digest").write_bytes(signing_hash(data))
    (tmp_path / "signature").write_bytes(data[-256:])
    out = subprocess.run(
        ["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", tmp_path / "public.pem", "-pkeyopt", "digest:sha256",
         "-in", tmp_path / "digest", "-sigfile", tmp_path / "signature"],
        capture_output=True, text=True,
    )
    assert out.returncode == 0, out.stdout + out.stderr
    # the unsigned build carries no signature (DuckDB's 256 zero bytes)
    assert pathlib.Path(extension_path()).read_bytes()[-256:] == bytes(256)


@REPOSITORIES
def test_a_signed_extension_loads_from_a_repository_that_trusts_its_key(tmp_path, signed):
    ext, public = signed
    con = strict(tmp_path)
    repo = repository(tmp_path, con, ext)
    name = trust(con, repo, public, tmp_path)
    try:
        con.execute(f"INSTALL tparquet FROM {name}")
        con.execute(f"LOAD tparquet FROM {name}")
        assert rows(con, f"SELECT count(*) FROM read_tparquet('{FILES}/standard_control.parquet')")[0][0] > 0
    finally:
        forget(con, name)


@REPOSITORIES
def test_the_same_file_unsigned_or_tampered_or_under_another_key_is_refused(tmp_path, signed):
    ext, public = signed
    _, other_public = keypair(tmp_path, "other")
    tampered = tmp_path / "tampered" / "tparquet.duckdb_extension"
    tampered.parent.mkdir()
    data = bytearray(ext.read_bytes())
    data[len(data) // 2] ^= 0x01
    tampered.write_bytes(bytes(data))
    unsigned = tmp_path / "unsigned" / "tparquet.duckdb_extension"
    unsigned.parent.mkdir()
    shutil.copy(extension_path(), unsigned)
    for name, file, key in [("t", tampered, public), ("o", ext, other_public), ("u", unsigned, public)]:
        con = strict(tmp_path / name)
        repo = repository(tmp_path / name, con, file)
        repo_name = trust(con, repo, key, tmp_path / name)
        try:
            with pytest.raises(duckdb.Error, match="signature|signed|unsigned"):
                con.execute(f"INSTALL tparquet FROM {repo_name}")
                con.execute(f"LOAD tparquet FROM {repo_name}")
        finally:
            forget(con, repo_name)
            con.close()


def test_a_bare_load_of_a_signed_file_still_needs_a_trusted_key(tmp_path, signed):
    ext, _ = signed
    con = strict(tmp_path)
    with pytest.raises(duckdb.Error, match="signature|signed"):
        con.execute(f"LOAD '{ext}'")


def test_signing_refuses_a_file_that_is_not_a_packaged_extension(tmp_path):
    private, _ = keypair(tmp_path, "key")
    bogus = tmp_path / "bogus.duckdb_extension"
    bogus.write_bytes(b"not an extension")
    out = sign_with(private, bogus)
    assert out.returncode != 0
    assert "no metadata footer" in out.stderr
    assert bogus.read_bytes() == b"not an extension"
