"""COPY ... TO ... (FORMAT tparquet): DuckDB's Parquet writer, with PCODEC for the INT32, INT64,
FLOAT and DOUBLE columns (the default) and ZSTD for the others."""

import json

import pyarrow as pa
import pyarrow.compute as pc
import pyarrow.parquet as pq
import pytest
from canonical import lines
from conftest import FILES, codecs, connect, copy_to, rows, run_cli

import duckdb

TYPES = """
SELECT
    (i % 100)::TINYINT AS i8, i::SMALLINT AS i16, i::INTEGER AS i32, i::BIGINT AS i64,
    (i % 200)::UTINYINT AS u8, i::USMALLINT AS u16, i::UINTEGER AS u32, i::UBIGINT AS u64,
    i * 0.5::FLOAT AS f32, i * 0.25 AS f64_decimal, (i * 0.25)::DOUBLE AS f64,
    (i * 0.01)::DECIMAL(9, 2) AS dec9, (i * 0.01)::DECIMAL(18, 2) AS dec18, (i * 0.001)::DECIMAL(30, 3) AS dec30,
    DATE '2020-01-01' + i::INTEGER AS d, TIMESTAMP '2020-01-01' + INTERVAL (i) SECOND AS ts,
    TIME '00:00:00' + INTERVAL (i) SECOND AS t, i % 2 = 0 AS b, 'v' || i AS s,
    CASE WHEN i % 5 = 0 THEN NULL ELSE i END AS nullable,
    [i, i + 1] AS list, {'a': i, 'b': 'x' || i} AS struct
FROM range(5000) r(i)
"""

NUMERIC = {"i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "dec9", "dec18", "d", "ts", "t", "nullable"}


@pytest.fixture(scope="module")
def written(tmp_path_factory):
    d = tmp_path_factory.mktemp("write")
    c = connect()
    c.execute(f"COPY ({TYPES}) TO '{d}/t.parquet' (FORMAT tparquet)")
    c.execute(f"COPY ({TYPES}) TO '{d}/std.parquet' (FORMAT parquet)")
    yield d, c
    c.close()


def test_numeric_columns_get_pcodec_and_the_others_zstd(written):
    d, con = written
    got = codecs(con, d / "t.parquet")
    for column, codec in got.items():
        top = column.split(",")[0]
        if top in NUMERIC or column in ("list, list, element", "struct, a") or top == "f64_decimal":
            physical = rows(
                con, f"SELECT DISTINCT type FROM tparquet_metadata('{d}/t.parquet') WHERE path_in_schema = '{column}'"
            )[0][0]
            assert codec == ({"PCODEC"} if physical in ("INT32", "INT64", "FLOAT", "DOUBLE") else {"ZSTD"}), column
        else:
            assert codec == {"ZSTD"}, column
    assert got["i64"] == {"PCODEC"} and got["f64"] == {"PCODEC"} and got["s"] == {"ZSTD"} and got["dec30"] == {"ZSTD"}


def test_pcodec_columns_are_plain_without_a_dictionary(written):
    d, con = written
    for column, encodings, dictionary in rows(
        con,
        f"SELECT path_in_schema, encodings, dictionary_page_offset FROM tparquet_metadata('{d}/t.parquet') "
        "WHERE compression = 'PCODEC'",
    ):
        assert encodings == "PLAIN", column
        assert dictionary is None, column
    # the same columns of DuckDB's own writer use dictionaries
    assert any(
        "DICTIONARY" in e for (e,) in rows(con, f"SELECT encodings FROM parquet_metadata('{d}/std.parquet')")
    )


def test_the_footer_names_the_pcodec_version(written):
    d, con = written
    kv = rows(con, f"SELECT decode(key), decode(value) FROM tparquet_kv_metadata('{d}/t.parquet')")
    assert ("parquet.pcodec.version", "1") in kv
    assert ("parquet.pcodec.version", "1") not in rows(
        con, f"SELECT decode(key), decode(value) FROM parquet_kv_metadata('{d}/std.parquet')"
    )


def test_reads_back_what_it_wrote(written):
    d, con = written
    assert rows(con, f"SELECT * FROM read_tparquet('{d}/t.parquet') ORDER BY i64") == rows(con, f"{TYPES} ORDER BY i64")


def test_the_reference_decoder_reads_what_duckdb_wrote(written):
    # the Rust implementation (tparquet CLI) decodes the file to a standard one, value for value
    d, con = written
    out = run_cli("decode", d / "t.parquet", d / "decoded.parquet")
    assert out.returncode == 0, out.stderr
    decoded, standard = pq.read_table(d / "decoded.parquet"), pq.read_table(d / "std.parquet")
    assert lines(decoded, decoded.schema) == lines(standard, standard.schema)
    out = run_cli("verify", d / "std.parquet", d / "t.parquet")
    assert out.returncode == 0, out.stdout + out.stderr


def test_pcodec_is_smaller_than_zstd_on_numbers(tmp_path, con):
    query = "SELECT i AS id, 1000000 + i * 37 AS ts, sin(i / 100.0) AS x FROM range(200000) r(i)"
    con.execute(f"COPY ({query}) TO '{tmp_path}/p.parquet' (FORMAT tparquet)")
    con.execute(f"COPY ({query}) TO '{tmp_path}/z.parquet' (FORMAT parquet, COMPRESSION zstd)")
    size = {
        name: rows(con, f"SELECT sum(total_compressed_size) FROM parquet_metadata('{tmp_path}/{name}.parquet')")[0][0]
        if name == "z"
        else rows(con, f"SELECT sum(total_compressed_size) FROM tparquet_metadata('{tmp_path}/{name}.parquet')")[0][0]
        for name in ("p", "z")
    }
    assert size["p"] < size["z"], size
    # the same rows, bit for bit
    assert rows(con, f"SELECT count(*) FROM (({query}) EXCEPT ALL (FROM read_tparquet('{tmp_path}/p.parquet')))") == [(0,)]
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{tmp_path}/p.parquet')") == [(200000,)]


@pytest.mark.parametrize("level", [0, 4, 12])
def test_pcodec_levels(tmp_path, con, level):
    con.execute(
        f"COPY (SELECT range AS x FROM range(1000)) TO '{tmp_path}/l.parquet' (FORMAT tparquet, COMPRESSION_LEVEL {level})"
    )
    assert rows(con, f"SELECT sum(x) FROM read_tparquet('{tmp_path}/l.parquet')") == [(499500,)]


@pytest.mark.parametrize("level", [-1, 13, 22])
def test_pcodec_levels_outside_0_to_12_are_refused(tmp_path, con, level):
    with pytest.raises(duckdb.Error, match="between 0 and 12 for the PCODEC"):
        con.execute(f"COPY (SELECT 1 AS x) TO '{tmp_path}/l.parquet' (FORMAT tparquet, COMPRESSION_LEVEL {level})")


@pytest.mark.parametrize("codec", ["zstd", "snappy", "gzip", "brotli", "lz4", "uncompressed"])
def test_the_other_codecs_write_standard_files(tmp_path, con, codec):
    path = tmp_path / f"{codec}.parquet"
    con.execute(f"COPY (SELECT range AS x, 'a' || range AS s FROM range(100)) TO '{path}' (FORMAT tparquet, COMPRESSION {codec})")
    plain = duckdb.connect()
    assert rows(plain, f"SELECT sum(x), count(DISTINCT s) FROM read_parquet('{path}')") == [(4950, 100)]
    assert ("parquet.pcodec.version", "1") not in rows(
        plain, f"SELECT decode(key), decode(value) FROM parquet_kv_metadata('{path}')"
    )


def test_zstd_takes_its_own_levels(tmp_path, con):
    con.execute(f"COPY (SELECT 1 AS x) TO '{tmp_path}/z.parquet' (FORMAT tparquet, COMPRESSION zstd, COMPRESSION_LEVEL 19)")
    with pytest.raises(duckdb.Error, match="ZSTD and PCODEC"):
        con.execute(f"COPY (SELECT 1 AS x) TO '{tmp_path}/s.parquet' (FORMAT tparquet, COMPRESSION snappy, COMPRESSION_LEVEL 3)")


def test_an_unknown_codec_names_pcodec(tmp_path, con):
    with pytest.raises(duckdb.Error, match="zstd or pcodec"):
        con.execute(f"COPY (SELECT 1 AS x) TO '{tmp_path}/x.parquet' (FORMAT tparquet, COMPRESSION lzo)")


def test_explicit_pcodec_and_parquet_version_v2(tmp_path, con):
    con.execute(
        f"COPY (SELECT range AS x, range::VARCHAR AS s FROM range(3000)) TO '{tmp_path}/v2.parquet' "
        "(FORMAT tparquet, COMPRESSION pcodec, PARQUET_VERSION v2)"
    )
    assert codecs(con, tmp_path / "v2.parquet") == {"x": {"PCODEC"}, "s": {"ZSTD"}}
    assert rows(con, f"SELECT sum(x), count(DISTINCT s) FROM read_tparquet('{tmp_path}/v2.parquet')") == [(4498500, 3000)]


def test_partitioned_and_copy_from(tmp_path, con):
    con.execute(
        f"COPY (SELECT range % 3 AS p, range AS x FROM range(300)) TO '{tmp_path}/parts' (FORMAT tparquet, PARTITION_BY p)"
    )
    files = sorted(p.name for p in (tmp_path / "parts").rglob("*.parquet"))
    assert len(files) == 3
    assert rows(con, f"SELECT p, sum(x) FROM read_tparquet('{tmp_path}/parts/**/*.parquet', hive_partitioning := true) GROUP BY p ORDER BY p") == [
        (0, sum(range(0, 300, 3))), (1, sum(range(1, 300, 3))), (2, sum(range(2, 300, 3)))]
    con.execute("CREATE TABLE t (x BIGINT)")
    con.execute(f"COPY (SELECT range AS x FROM range(10)) TO '{tmp_path}/f.parquet' (FORMAT tparquet)")
    con.execute(f"COPY t FROM '{tmp_path}/f.parquet' (FORMAT tparquet)")
    assert rows(con, "SELECT sum(x) FROM t") == [(45,)]


def test_empty_and_all_null_columns(tmp_path, con):
    con.execute(f"COPY (SELECT range AS x FROM range(0)) TO '{tmp_path}/empty.parquet' (FORMAT tparquet)")
    assert rows(con, f"SELECT count(*) FROM read_tparquet('{tmp_path}/empty.parquet')") == [(0,)]
    con.execute(f"COPY (SELECT NULL::BIGINT AS x, NULL::DOUBLE AS y FROM range(100)) TO '{tmp_path}/null.parquet' (FORMAT tparquet)")
    assert rows(con, f"SELECT count(*), count(x), count(y) FROM read_tparquet('{tmp_path}/null.parquet')") == [(100, 0, 0)]
    assert codecs(con, tmp_path / "null.parquet") == {"x": {"PCODEC"}, "y": {"PCODEC"}}


def test_special_floats_round_trip_bit_for_bit(tmp_path, con):
    values = ["'NaN'::DOUBLE", "'inf'::DOUBLE", "'-inf'::DOUBLE", "-0.0::DOUBLE", "0.0::DOUBLE", "1e-310::DOUBLE"]
    query = " UNION ALL ".join(f"SELECT {i} AS k, {v} AS x" for i, v in enumerate(values))
    con.execute(f"COPY ({query}) TO '{tmp_path}/f.parquet' (FORMAT tparquet)")
    got = rows(con, f"SELECT k, x::VARCHAR FROM read_tparquet('{tmp_path}/f.parquet') ORDER BY k")
    assert got == rows(con, f"SELECT k, x::VARCHAR FROM ({query}) ORDER BY k")
    # pyarrow cannot read PCODEC: the decoded file shows the bits
    out = run_cli("decode", tmp_path / "f.parquet", tmp_path / "fd.parquet")
    assert out.returncode == 0, out.stderr
    bits = [json.dumps(r) for r in pq.read_table(tmp_path / "fd.parquet").column("x").to_pylist()]
    assert bits[1] == "Infinity" and bits[2] == "-Infinity"


def float_bits(table: pa.Table) -> dict[str, list]:
    def bits(a):
        a = a.combine_chunks()
        return pc.cast(a.view(pa.uint64() if pa.types.is_float64(a.type) else pa.uint32()), pa.uint64()).to_pylist()

    return {n: bits(table[n]) for n in table.column_names if pa.types.is_floating(table[n].type)}


def in_order(path):
    return f"SELECT * EXCLUDE (file_row_number) FROM read_tparquet('{path}', file_row_number := true) ORDER BY file_row_number"


def test_nan_payloads_survive_a_copy(tmp_path, con):
    # the PCODEC columns are PLAIN without a dictionary; DuckDB's dictionary, which FORMAT parquet
    # uses, keeps one NaN (DUCKDB_ORACLE_LOSES_NAN_PAYLOADS in acceptance/cross/vectors.py)
    src = FILES / "float_specials.parquet"
    want = float_bits(con.sql(in_order(src)).to_arrow_table())
    quiet = (0x7FF8000000000000, 0xFFF8000000000000)
    assert any(v not in quiet and (v >> 52) & 0x7FF == 0x7FF and v & ((1 << 52) - 1) for v in want["f64"])
    copy_to(con, in_order(src), tmp_path / "f.parquet")
    assert float_bits(con.sql(in_order(tmp_path / "f.parquet")).to_arrow_table()) == want


def inspect(path) -> dict[str, dict]:
    """`tparquet inspect --json`: codecs, sizes and PCODEC payload modes of each column."""
    out = run_cli("inspect", "--json", path)
    assert out.returncode == 0, out.stderr
    return {c["path"]: c for c in json.loads(out.stdout)["columns"]}


NUMBERS = """
SELECT
    i AS id,
    (hash(i) % 1000000)::BIGINT AS noise,
    round(25000 + (hash(i) % 2000) * 0.01, 2)::DOUBLE AS price,
    sqrt(i::DOUBLE) AS root,
    (hash(i) % 100000)::INTEGER AS i32,
    sqrt(i::DOUBLE)::FLOAT AS f32,
    'k' || (hash(i) % 50) AS s
FROM range(50000) r(i)
"""


def test_the_payloads_get_the_type_hint_and_decimal_mode(tmp_path, con):
    con.execute(f"COPY ({NUMBERS}) TO '{tmp_path}/n.parquet' (FORMAT tparquet)")
    cols = inspect(tmp_path / "n.parquet")
    # pcodec on the numbers of the column's physical type (spec section 3: the type hint)
    for name in ("id", "noise", "root", "i32", "f32"):
        assert cols[name]["codecs"] == ["PCODEC"] and set(cols[name]["modes"]) == {"PCO"}, (name, cols[name])
    # DECIMAL mode for decimal-valued floats
    assert set(cols["price"]["modes"]) == {"DECIMAL"}, cols["price"]
    assert cols["s"]["codecs"] == ["ZSTD"]


def test_the_level_is_pcodecs_and_defaults_to_8(tmp_path, con):
    sizes = {}
    for level in ("", ", COMPRESSION_LEVEL 0", ", COMPRESSION_LEVEL 8", ", COMPRESSION_LEVEL 12"):
        path = tmp_path / f"l{level[-2:].strip()}.parquet"
        con.execute(f"COPY ({NUMBERS}) TO '{path}' (FORMAT tparquet{level})")
        sizes[level] = {k: v["compressed_bytes"] for k, v in inspect(path).items()}
    default, l0, l8, l12 = sizes.values()
    assert default == l8
    assert l0["noise"] != l12["noise"] and l0["root"] != l12["root"]


def test_the_other_columns_get_duckdbs_zstd(tmp_path, con):
    # a string column of a PCODEC file is DuckDB's ZSTD column at its default level, byte for
    # byte, whatever the (PCODEC) level
    con.execute(f"COPY ({NUMBERS}) TO '{tmp_path}/z.parquet' (FORMAT parquet, COMPRESSION zstd)")
    want = rows(con, f"SELECT total_compressed_size FROM parquet_metadata('{tmp_path}/z.parquet') WHERE path_in_schema = 's'")
    for level in ("", ", COMPRESSION_LEVEL 0", ", COMPRESSION_LEVEL 12"):
        con.execute(f"COPY ({NUMBERS}) TO '{tmp_path}/p.parquet' (FORMAT tparquet{level})")
        got = rows(con, f"SELECT total_compressed_size FROM tparquet_metadata('{tmp_path}/p.parquet') WHERE path_in_schema = 's'")
        assert got == want, level


def _varint(data, at):
    shift = value = 0
    while True:
        byte = data[at]
        at += 1
        value |= (byte & 0x7F) << shift
        shift += 7
        if byte < 0x80:
            return value, at


def compressed_page_size(data, offset):
    """The compressed size of the page whose header starts at `offset`: the header is a Thrift
    compact struct whose first three fields are i32s (type, uncompressed and compressed size)."""
    at, fields = offset, []
    for _ in range(3):
        assert data[at] & 0x0F == 5, data[at]  # an i32 field, the next one
        value, at = _varint(data, at + 1)
        fields.append((value >> 1) ^ -(value & 1))
    return fields[2]


def test_a_damaged_pcodec_payload_is_an_error(tmp_path, con):
    path = tmp_path / "d.parquet"
    con.execute(f"COPY (SELECT (hash(i) % 1000000)::BIGINT AS noise FROM range(50000) r(i)) TO '{path}' (FORMAT tparquet)")
    ((offset, size),) = rows(con, f"SELECT data_page_offset, total_compressed_size FROM tparquet_metadata('{path}')")
    data = bytearray(path.read_bytes())
    # one page: its payload is the last bytes of the column chunk
    at = offset + size - compressed_page_size(data, offset)
    assert data[at] == 1  # the payload's version
    data[at] = 9
    path.write_bytes(bytes(data))
    with pytest.raises(duckdb.Error, match=r'Failed to read file ".*d\.parquet": PCODEC decompression failure: .*version'):
        rows(con, f"SELECT sum(noise) FROM read_tparquet('{path}')")
