"""measure: the same batches written as ZSTD(9) Parquet and as tparquet, sizes and times."""

from __future__ import annotations

import pathlib
import random
import time

import pyarrow as pa
import pyarrow.parquet as pq
import pytest
import tparquet as tpq

from tpq_bench import measure


def reader(*batches: pa.RecordBatch) -> pa.RecordBatchReader:
    return pa.RecordBatchReader.from_batches(batches[0].schema, batches)


def walk(n: int) -> list[float]:
    """Prices on a 0.01 tick grid, a deterministic random walk."""
    tick, out = 10_000, []
    for i in range(n):
        tick += (i * 7919 + i * i) % 9 - 4
        out.append(tick / 100)
    return out


WALK = walk(60_000)


def ticks(n: int, start: int = 0) -> pa.RecordBatch:
    return pa.record_batch(
        {
            "ts": pa.array([1_700_000_000_000 + 7 * i for i in range(start, start + n)], pa.int64()),
            "price": pa.array(WALK[start : start + n], pa.float64()),
            "side": pa.array(["buy" if i % 3 else "sell" for i in range(start, start + n)]),
        }
    )


def test_rebatch_makes_batches_of_the_given_rows() -> None:
    out = list(measure.rebatch(reader(ticks(3), ticks(4, 3), ticks(0, 7), ticks(5, 7)), 5))
    assert [b.num_rows for b in out] == [5, 5, 2]
    assert pa.Table.from_batches(out) == pa.Table.from_batches([ticks(12)])


def test_rebatch_of_nothing_is_nothing() -> None:
    assert list(measure.rebatch(reader(ticks(0)), 5)) == []


def test_measure_writes_the_four_files_with_the_same_data(tmp_path: pathlib.Path) -> None:
    m = measure.measure(reader(ticks(20_000), ticks(30_000, 20_000)), tmp_path, batch_rows=16_384, keep=True)
    assert m.rows == 50_000
    # the in-memory size of the batches the writers got
    assert m.arrow_bytes == sum(b.nbytes for b in measure.rebatch(reader(ticks(20_000), ticks(30_000, 20_000)), 16_384))
    z9, tuned, auto = tmp_path / "zstd9.parquet", tmp_path / "tuned.parquet", tmp_path / "tparquet.parquet"
    assert m.zstd9_bytes == z9.stat().st_size
    assert m.tuned_bytes == tuned.stat().st_size
    assert m.tparquet_bytes == auto.stat().st_size
    alp = tmp_path / "alp.parquet"
    assert m.alp_bytes == alp.stat().st_size
    assert tpq.verify(z9, alp) is None
    # the ALP baseline: the tuned encodings, ALP on the floating-point columns, ZSTD(9)
    assert m.alp_encodings == {**m.encodings, "price": "ALP"}
    md = pq.ParquetFile(alp).metadata
    alp_columns = {md.row_group(0).column(i).path_in_schema: md.row_group(0).column(i) for i in range(3)}
    assert {c.compression for c in alp_columns.values()} == {"ZSTD"}
    # pyarrow does not know ALP (encoding 10), and names it UNKNOWN
    assert set(alp_columns["price"].encodings) == {"RLE", "UNKNOWN"}
    assert "DELTA_BINARY_PACKED" in alp_columns["ts"].encodings
    assert m.ratio_alp == pytest.approx(m.tparquet_bytes / m.alp_bytes)
    # the baseline is standard Parquet, ZSTD level 9 on every column
    md = pq.ParquetFile(z9).metadata
    assert {md.row_group(0).column(i).compression for i in range(3)} == {"ZSTD"}
    assert pq.read_table(z9) == pa.Table.from_batches([ticks(50_000)])
    assert tpq.verify(z9, auto) is None
    assert tpq.verify(z9, tuned) is None
    # the tuned baseline: ZSTD(9), each column with the encoding that was smallest on the first batch
    md = pq.ParquetFile(tuned).metadata
    columns = {md.row_group(0).column(i).path_in_schema: md.row_group(0).column(i) for i in range(3)}
    assert {c.compression for c in columns.values()} == {"ZSTD"}
    assert m.encodings == measure.choose_encodings(ticks(16_384))
    assert m.encodings["ts"] == "DELTA_BINARY_PACKED"
    for name, encoding in m.encodings.items():
        assert (encoding if encoding != "dictionary" else "RLE_DICTIONARY") in columns[name].encodings
    assert m.tuned_bytes < m.zstd9_bytes
    assert m.codecs == {"ts": "PCODEC", "price": "PCODEC", "side": "ZSTD"}
    assert m.pcodec_columns == 2
    assert m.tparquet_bytes < m.zstd9_bytes
    assert m.ratio == pytest.approx(m.tparquet_bytes / m.zstd9_bytes)
    assert m.ratio_tuned == pytest.approx(m.tparquet_bytes / m.tuned_bytes)
    writes = (m.write_s_zstd9, m.write_s_tuned, m.write_s_alp, m.write_s_tparquet)
    reads = (m.read_s_zstd9, m.read_s_tuned, m.read_s_alp, m.read_s_tparquet)
    for t in writes + reads:
        assert t > 0


def test_measure_removes_its_files(tmp_path: pathlib.Path) -> None:
    measure.measure(reader(ticks(100)), tmp_path)
    assert list(tmp_path.iterdir()) == []


def test_measure_of_an_empty_source_fails(tmp_path: pathlib.Path) -> None:
    with pytest.raises(ValueError, match=r"^the source has no rows$"):
        measure.measure(reader(ticks(0)), tmp_path)


def test_candidate_encodings() -> None:
    assert measure.candidates(pa.int32()) == ["dictionary", "DELTA_BINARY_PACKED"]
    assert measure.candidates(pa.uint64()) == ["dictionary", "DELTA_BINARY_PACKED"]
    assert measure.candidates(pa.timestamp("ns", tz="UTC")) == ["dictionary", "DELTA_BINARY_PACKED"]
    assert measure.candidates(pa.float32()) == ["dictionary", "BYTE_STREAM_SPLIT"]
    assert measure.candidates(pa.float64()) == ["dictionary", "BYTE_STREAM_SPLIT"]
    assert measure.candidates(pa.string()) == ["dictionary", "DELTA_BYTE_ARRAY", "DELTA_LENGTH_BYTE_ARRAY"]
    assert measure.candidates(pa.large_string()) == ["dictionary", "DELTA_BYTE_ARRAY", "DELTA_LENGTH_BYTE_ARRAY"]
    assert measure.candidates(pa.bool_()) == ["dictionary"]
    assert measure.candidates(pa.list_(pa.float64())) == ["dictionary"]


def test_the_smallest_encoding_is_chosen() -> None:
    n = 50_000
    rng = random.Random(7)
    batch = pa.record_batch(
        {
            # few distinct values in random order: the dictionary wins
            "lots": pa.array([rng.choice([0.001, 1.5, 250.25, 1e6]) for _ in range(n)]),
            # a counter: the deltas win
            "id": pa.array(range(10**12, 10**12 + n), pa.int64()),
            # increasing ids as text, sharing prefixes: DELTA_BYTE_ARRAY wins
            "text_id": pa.array([str(10**12 + i) for i in range(n)]),
            "flag": pa.array([i % 2 == 0 for i in range(n)]),
        }
    )
    sizes = {name: measure.encoding_sizes(batch.column(name)) for name in batch.schema.names}
    chosen = measure.choose_encodings(batch)
    assert chosen == {name: min(s, key=s.__getitem__) for name, s in sizes.items()}
    assert chosen == {
        "lots": "dictionary",
        "id": "DELTA_BINARY_PACKED",
        "text_id": "DELTA_BYTE_ARRAY",
        "flag": "dictionary",
    }
    assert set(sizes["text_id"]) == {"dictionary", "DELTA_BYTE_ARRAY", "DELTA_LENGTH_BYTE_ARRAY"}


def test_alp_encodings() -> None:
    types: dict[str, pa.DataType] = {
        "f64": pa.float64(),
        "f32": pa.float32(),
        "f16": pa.float16(),
        "ts": pa.int64(),
        "lots": pa.float64(),
        "side": pa.string(),
        "flag": pa.bool_(),
    }
    schema = pa.schema(types)
    tuned = {
        "f64": "BYTE_STREAM_SPLIT",
        "f32": "dictionary",
        "f16": "dictionary",
        "ts": "DELTA_BINARY_PACKED",
        "lots": "dictionary",
        "side": "DELTA_BYTE_ARRAY",
        "flag": "dictionary",
    }
    # ALP is for FLOAT and DOUBLE only; float16 is a FIXED_LEN_BYTE_ARRAY
    assert measure.alp_encodings(schema, tuned) == {
        "f64": "ALP",
        "f32": "ALP",
        "f16": "dictionary",
        "ts": "DELTA_BINARY_PACKED",
        "lots": "ALP",
        "side": "DELTA_BYTE_ARRAY",
        "flag": "dictionary",
    }


def test_times_are_the_writers_and_readers_own(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # a clock that ticks once per reading: each timed call takes exactly one tick
    ticks_ = iter(range(10**6))
    monkeypatch.setattr(time, "perf_counter", lambda: float(next(ticks_)))
    m = measure.measure(reader(ticks(5_000)), tmp_path, batch_rows=2_000)
    # three batches and the footer, for each writer; one full read of each file
    assert (m.write_s_zstd9, m.write_s_tuned, m.write_s_alp, m.write_s_tparquet) == (4.0, 4.0, 4.0, 4.0)
    assert (m.read_s_zstd9, m.read_s_tuned, m.read_s_alp, m.read_s_tparquet) == (1.0, 1.0, 1.0, 1.0)


def test_the_tuned_file_is_zstd_level_9() -> None:
    rng, n = random.Random(3), 20_000
    # noisy enough data for ZSTD levels 9 and 10 to differ
    table = pa.table(
        {
            "ts": pa.array([1_700_000_000_000 + rng.randrange(1000) * i for i in range(n)], pa.int64()),
            "price": [round(100 + rng.gauss(0, 1), 2) for _ in range(n)],
            "side": [rng.choice(["buy", "sell", "x"]) for _ in range(n)],
        }
    )
    encodings = {"ts": "DELTA_BINARY_PACKED", "price": "BYTE_STREAM_SPLIT", "side": "dictionary"}

    def written(level: int | None = None) -> bytes:
        sink = pa.BufferOutputStream()
        with pq.ParquetWriter(
            sink,
            table.schema,
            compression="zstd",
            use_dictionary=["side"],  # type: ignore[arg-type]  # a list of columns, as documented
            column_encoding={"ts": "DELTA_BINARY_PACKED", "price": "BYTE_STREAM_SPLIT"},
            compression_level=level,
        ) as w:
            w.write_table(table)
        return bytes(sink.getvalue())

    sink = pa.BufferOutputStream()
    with measure._write_tuned(sink, table.schema, encodings) as w:
        w.write_table(table)
    tuned = bytes(sink.getvalue())
    assert tuned == written(9)
    assert tuned != written(10)
    assert tuned != written()


@pytest.mark.parametrize("name", ["tuned", "alp", "tparquet"])
def test_files_that_differ_are_an_error(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch, name: str) -> None:
    monkeypatch.setattr(tpq, "verify", lambda a, b: "row 3, column price" if f"/{name}." in str(b) else None)
    with pytest.raises(
        AssertionError, match=rf"^the {name} file differs from the ZSTD\(9\) file: row 3, column price$"
    ):
        measure.measure(reader(ticks(100)), tmp_path)
    assert list(tmp_path.iterdir()) == []


def test_a_read_is_timed_as_the_best_of_three(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    path = tmp_path / "t.parquet"
    pq.write_table(pa.Table.from_batches([ticks(100)]), path)
    clock = iter([0.0, 5.0, 10.0, 11.0, 20.0, 22.0])
    monkeypatch.setattr(time, "perf_counter", lambda: next(clock))
    assert measure._read(path) == 1.0
