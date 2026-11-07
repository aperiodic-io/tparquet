"""Runs the sqllogictest-style files in tests/sql/*.test (the subset of DuckDB's format below).

    # comment
    require tparquet      (the extension is always loaded; the line is documentation)
    onlyif v1 | onlyif v2 (the next record runs only on that build of the extension)
    statement ok | statement error
    <sql, up to a blank line>
    ----                        (statement error only: a substring of the message follows)
    query <one letter per column>
    <sql>
    ----
    <rows: values separated by tabs; NULL for null>

`__TESTDATA__` is replaced by the repository's testdata/files directory, `__TEST_DIR__`
by a fresh temporary directory and `__EXTENSION__` by the extension file.
"""

import pathlib

import pytest

from conftest import API, FILES, connect, extension_path

SQL_DIR = pathlib.Path(__file__).parent / "sql"


def records(text: str):
    lines = text.splitlines()
    i = 0
    only = None
    while i < len(lines):
        line = lines[i].strip()
        i += 1
        if not line or line.startswith("#") or line.startswith("require "):
            continue
        if line.startswith("onlyif "):
            only = line.split()[1]
            if only not in ("v1", "v2"):
                raise ValueError(f"unknown build {only!r}")
            continue
        kind = line.split()
        if kind[0] not in ("statement", "query"):
            raise ValueError(f"unknown record {line!r}")
        sql, expected = [], []
        while i < len(lines) and lines[i].strip() and lines[i].strip() != "----":
            sql.append(lines[i])
            i += 1
        if i < len(lines) and lines[i].strip() == "----":
            i += 1
            while i < len(lines) and lines[i].strip():
                expected.append(lines[i])
                i += 1
        if only in (None, API):
            yield kind, "\n".join(sql), expected
        only = None


def show(v) -> str:
    if v is None:
        return "NULL"
    if isinstance(v, bool):
        return "true" if v else "false"
    return str(v)


@pytest.mark.parametrize("path", sorted(SQL_DIR.glob("*.test")), ids=lambda p: p.name)
def test_sql_file(path, tmp_path):
    con = connect()
    text = path.read_text().replace("__TESTDATA__", str(FILES)).replace("__TEST_DIR__", str(tmp_path))
    text = text.replace("__EXTENSION__", extension_path())
    count = 0
    for kind, sql, expected in records(text):
        count += 1
        where = f"{path.name}: {sql}"
        if kind == ["statement", "ok"]:
            con.execute(sql)
        elif kind == ["statement", "error"]:
            with pytest.raises(Exception) as e:
                con.execute(sql).fetchall()
            assert "\n".join(expected) in str(e.value), where
        elif kind[0] == "query":
            got = ["\t".join(show(v) for v in row) for row in con.execute(sql).fetchall()]
            assert all(len(r.split("\t")) == len(kind[1]) for r in got) or not got, where
            assert got == expected, where
        else:
            raise ValueError(f"{path.name}: unknown record {kind}")
    assert count > 0
    con.close()
