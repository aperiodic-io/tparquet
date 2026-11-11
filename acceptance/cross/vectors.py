"""Cross-language acceptance: vectors written by one implementation, read by the others.

A vector directory holds `files/<name>.parquet` and `expected/<name>.jsonl`, the canonical
rows of `testdata/README.md`. Rust (`testdata/`, `gen-testdata`) and Go (`tparquet-go-gen`)
write their own; this script writes the Python, DuckDB and Rust CLI ones and holds the Python
and DuckDB readers. `run.sh` puts them together.

    vectors.py write-python OUT SRC...       every SRC/files/*.parquet transcoded to PCODEC, and
                                             written with the auto and base policies in row
                                             groups of ROW_GROUP rows
    vectors.py write-duckdb EXT OUT SRC...   every SRC vector rewritten with COPY (FORMAT
                                             tparquet): the auto policy, then pcodec (level 3)
                                             and base (snappy) in row groups of ROW_GROUP rows
    vectors.py write-cli CLI OUT SRC...      every SRC vector rewritten by `tparquet encode`
                                             with the auto, pcodec and base policies
    vectors.py check-python DIR              read DIR with tparquet
    vectors.py check-duckdb EXT DIR          read DIR with read_tparquet

A vector written under a policy is named `<name>__<policy>`.

Every check prints `<n> vectors match`, or names the first differing line and exits 1.
"""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterator

import pyarrow as pa
import pyarrow.parquet as pq

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "duckdb" / "tests"))

# DuckDB's built-in Parquet writer, the oracle of the DuckDB vectors, writes every NaN as the
# canonical quiet NaN; these vectors hold other NaN payloads, which the extension keeps
DUCKDB_ORACLE_LOSES_NAN_PAYLOADS = {"float_specials", "go_special_floats"}

# rows per row group of the vectors written under a policy: most vectors get several
ROW_GROUP = 250


class Mismatch(Exception):
    pass


def vectors(src: pathlib.Path) -> Iterator[tuple[str, pathlib.Path, pathlib.Path]]:
    """(name, file, expected) of every vector in `src`; a file without expected rows fails."""
    files = sorted((src / "files").glob("*.parquet"))
    if not files:
        raise Mismatch(f"no files/*.parquet in {src}")
    for f in files:
        expected = src / "expected" / f"{f.stem}.jsonl"
        if not expected.is_file():
            raise Mismatch(f"{f.stem}: no expected/{f.stem}.jsonl")
        yield f.stem, f, expected


def compare(name: str, got: str, want: str) -> None:
    a, b = got.splitlines(), want.splitlines()
    for i in range(max(len(a), len(b))):
        if a[i : i + 1] != b[i : i + 1]:
            raise Mismatch(f"{name}: line {i + 1} differs from expected/{name}.jsonl")


def check(src: pathlib.Path, read: Callable[[pathlib.Path], str | None]) -> int:
    """Compares every vector `read` reads; `read` gives None for a vector its reader cannot
    represent (counted and named as skipped)."""
    n = 0
    skipped = []
    for name, f, expected in vectors(src):
        got = read(f)
        if got is None:
            skipped.append(name)
            continue
        compare(name, got, expected.read_text(encoding="utf-8"))
        n += 1
    if skipped:
        print(f"skipped {len(skipped)}: {', '.join(skipped)}", file=sys.stderr)
    return n


def joined(lines: Iterator[str]) -> str:
    return "".join(line + "\n" for line in lines)


def python_reader() -> Callable[[pathlib.Path], str]:
    import tparquet as ppc
    from tparquet import canonical

    return lambda f: joined(canonical.lines(ppc.read_table(f)))


def duckdb_connect(extension: str):
    import duckdb

    con = duckdb.connect(config={"allow_unsigned_extensions": "true"})
    con.execute(f"LOAD '{extension}'")
    return con


def scan(path: pathlib.Path) -> str:
    return (
        f"SELECT * EXCLUDE (file_row_number) FROM read_tparquet('{path}', file_row_number := true) "
        "ORDER BY file_row_number"
    )


def duckdb_reader(extension: str) -> Callable[[pathlib.Path], str]:
    from canonical import lines  # duckdb/tests/canonical.py

    con = duckdb_connect(extension)
    version = con.execute("SELECT library_version FROM pragma_version()").fetchall()[0][0]

    def read(f: pathlib.Path) -> str | None:
        schema = pq.read_schema(f)
        # DuckDB 1.x has no TIMESTAMP WITH TIME ZONE in nanoseconds: it reads such a column as
        # microseconds, as its own read_parquet does (duckdb/README.md, Limitations)
        if version.startswith("v1.") and any(
            pa.types.is_timestamp(t) and t.unit == "ns" and t.tz is not None for t in leaf_types(schema)
        ):
            return None
        # canonical.lines gives the whole text, one row per line
        return lines(con.sql(scan(f)).to_arrow_table(), schema)

    return read


def leaf_types(schema):
    def leaves(t):
        if pa.types.is_struct(t):
            for i in range(t.num_fields):
                yield from leaves(t.field(i).type)
        elif pa.types.is_map(t):
            yield from leaves(t.key_type)
            yield from leaves(t.item_type)
        elif pa.types.is_list(t) or pa.types.is_large_list(t) or pa.types.is_fixed_size_list(t):
            yield from leaves(t.value_type)
        else:
            yield t

    for field in schema:
        yield from leaves(field.type)


def fresh(out: pathlib.Path) -> None:
    for sub in ("files", "expected"):
        shutil.rmtree(out / sub, ignore_errors=True)
        (out / sub).mkdir(parents=True)


def keep(out: pathlib.Path, name: str, expected: pathlib.Path) -> pathlib.Path:
    """The path of vector `name` in `out`, whose expected rows (those of its source) are copied."""
    shutil.copy(expected, out / "expected" / f"{name}.jsonl")
    return out / "files" / f"{name}.parquet"


def write_python(out: pathlib.Path, sources: list[pathlib.Path]) -> int:
    """Every vector transcoded by the Python package with the Pcodec policy, and read and
    written again with the Auto and Base policies in row groups of ROW_GROUP rows; the rows
    stay those of the source, so its expected rows are copied."""
    import tparquet as ppc

    fresh(out)
    n = 0
    for src in sources:
        for name, f, expected in vectors(src):
            ppc.transcode(f, keep(out, name, expected), target="pcodec", policy="pcodec")
            table = ppc.read_table(f)
            for policy in ("auto", "base"):
                path = keep(out, f"{name}__{policy}", expected)
                ppc.write_table(table, path, policy=policy, row_group_size=ROW_GROUP)
            n += 3
    return n


def write_cli(cli: str, out: pathlib.Path, sources: list[pathlib.Path]) -> int:
    """Every vector rewritten by the Rust CLI (`tparquet encode`) with each policy."""
    fresh(out)
    n = 0
    for src in sources:
        for name, f, expected in vectors(src):
            for policy in ("auto", "pcodec", "base"):
                path = keep(out, f"{name}__{policy}", expected)
                done = subprocess.run([cli, "encode", f, path, "--policy", policy], capture_output=True, text=True)
                if done.returncode != 0:
                    raise Mismatch(f"{name}: tparquet encode --policy {policy}: {done.stderr.strip()}")
                n += 1
    return n


def write_duckdb(extension: str, out: pathlib.Path, sources: list[pathlib.Path]) -> int:
    """Every vector read and written back by the extension. The expected rows are those of
    the same query written by DuckDB's built-in Parquet writer, read by pyarrow: no PCODEC
    code is involved in them."""
    from tparquet import canonical

    con = duckdb_connect(extension)
    fresh(out)
    n = 0
    with tempfile.TemporaryDirectory() as tmp:
        for src in sources:
            for name, f, _ in vectors(src):
                if name in DUCKDB_ORACLE_LOSES_NAN_PAYLOADS:
                    continue
                q = scan(f)
                standard = pathlib.Path(tmp) / f"{name}.parquet"
                con.execute(f"COPY ({q}) TO '{standard}' (FORMAT parquet)")
                rows = joined(canonical.lines(pq.read_table(standard)))
                for suffix, options in [
                    ("", ""),
                    ("__pcodec", f", COMPRESSION pcodec, COMPRESSION_LEVEL 3, ROW_GROUP_SIZE {ROW_GROUP}"),
                    ("__base", f", COMPRESSION snappy, ROW_GROUP_SIZE {ROW_GROUP}"),
                ]:
                    con.execute(
                        f"COPY ({q}) TO '{out / 'files' / f'{name}{suffix}.parquet'}' "
                        f"(FORMAT tparquet{options})"
                    )
                    (out / "expected" / f"{name}{suffix}.jsonl").write_text(rows, encoding="utf-8")
                    n += 1
    return n


def main(argv: list[str]) -> int:
    try:
        match argv:
            case ["write-python", out, *srcs] if srcs:
                print(f"{write_python(pathlib.Path(out), [pathlib.Path(s) for s in srcs])} vectors written")
            case ["write-duckdb", ext, out, *srcs] if srcs:
                n = write_duckdb(ext, pathlib.Path(out), [pathlib.Path(s) for s in srcs])
                print(f"{n} vectors written")
            case ["write-cli", cli, out, *srcs] if srcs:
                print(f"{write_cli(cli, pathlib.Path(out), [pathlib.Path(s) for s in srcs])} vectors written")
            case ["check-python", src]:
                print(f"{check(pathlib.Path(src), python_reader())} vectors match")
            case ["check-duckdb", ext, src]:
                print(f"{check(pathlib.Path(src), duckdb_reader(ext))} vectors match")
            case _:
                print(__doc__, file=sys.stderr)
                return 2
    except Mismatch as e:
        print(f"vectors.py: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
