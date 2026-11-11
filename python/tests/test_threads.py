"""Many threads at once, and the GIL released while Rust works; the core without pyarrow."""

from __future__ import annotations

import io
import pathlib
import subprocess
import sys
import textwrap
import threading
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor

import pyarrow as pa
import pytest

import tparquet as ppc
from tparquet import canonical, codec


@pytest.fixture(scope="module")
def big() -> pa.Table:
    n = 2_000_000
    return pa.table(
        {
            "t": pa.array(range(1_700_000_000_000, 1_700_000_000_000 + 3 * n, 3), pa.int64()),
            "p": pa.array([(i * 7919 % 10007) / 100 for i in range(n)], pa.float64()),
        }
    )


def test_many_threads_read_one_file(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path, policy="pcodec", row_group_size=250)
    shared = ppc.ParquetFile(path)
    in_memory = ppc.ParquetFile(io.BytesIO(path.read_bytes()))
    want = canonical.dumps(numbers)

    def work(i: int) -> str:
        source = [shared, in_memory][i % 2]
        if i % 3 == 0:
            return canonical.dumps(ppc.read_table(path, batch_size=97 + i))
        return canonical.dumps(source.read(row_groups=list(range(8))))

    with ThreadPoolExecutor(max_workers=16) as pool:
        results = list(pool.map(work, range(64)))
    assert all(r == want for r in results)


def test_many_threads_write(numbers: pa.Table) -> None:
    def work(policy: ppc.Policy) -> bytes:
        buf = io.BytesIO()
        ppc.write_table(numbers, buf, policy=policy)
        return buf.getvalue()

    policies: list[ppc.Policy] = ["auto", "pcodec", "base"] * 8
    with ThreadPoolExecutor(max_workers=12) as pool:
        files = list(pool.map(work, policies))
    for policy, data in zip(policies, files, strict=True):
        assert data == files[policies.index(policy)]  # deterministic
        assert ppc.read_table(io.BytesIO(data)).equals(numbers)


def test_one_stream_shared_by_threads(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path)
    stream = ppc.read_batches(path, batch_size=10)
    rows: list[int] = []

    def drain() -> None:
        for b in stream:
            rows.append(b.num_rows)

    threads = [threading.Thread(target=drain) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert sum(rows) == numbers.num_rows


def ticks_while(work: Callable[[], object]) -> int:
    """How often the main thread runs while `work` runs in another thread."""
    done = threading.Event()

    def run() -> None:
        try:
            work()
        finally:
            done.set()

    t = threading.Thread(target=run)
    n = 0
    t.start()
    while not done.is_set():
        n += 1
    t.join()
    return n


def test_the_gil_is_released(big: pa.Table) -> None:
    buf = io.BytesIO()
    # encoding the one batch: the Python file object is only called between pages
    assert ticks_while(lambda: ppc.write_table(big, buf, policy="pcodec")) > 1000
    f = ppc.ParquetFile(io.BytesIO(buf.getvalue()))
    stream = f.stream(batch_size=big.num_rows)
    # decoding the whole file in one batch
    assert ticks_while(lambda: next(iter(stream))) > 1000
    buffer = pa.array(range(4_000_000), pa.int64()).buffers()[1]
    assert buffer is not None
    data = buffer.to_pybytes()
    payload = codec.compress(data, hint="INT64")
    assert ticks_while(lambda: codec.compress(data, hint="INT64")) > 1000
    assert ticks_while(lambda: codec.decompress(payload, len(data))) > 100
    assert ticks_while(lambda: ppc.ParquetFile(io.BytesIO(buf.getvalue())).read()) > 1000


def test_core_without_pyarrow(tmp_path: pathlib.Path) -> None:
    # the core and the Polars integration need no pyarrow; read_table says what to use
    code = textwrap.dedent(
        f"""
        import sys
        sys.modules["pyarrow"] = None
        import polars as pl
        import tparquet as ppc
        import tparquet.polars as ppl
        df = pl.DataFrame({{"a": [1, 2, 3], "b": [0.5, None, 1.5], "s": ["x", "y", None]}})
        path = {str(tmp_path / "np.parquet")!r}
        print(sorted(ppc.write_table(df, path).items()))
        print(pl.DataFrame(ppc.read_batches(path)).equals(df))
        print(ppl.scan_parquet(path).filter(pl.col("a") > 1).collect()["a"].to_list())
        print(ppc.ParquetFile(path).metadata.num_rows, ppc.ParquetFile(path).schema_capsule.names)
        try:
            ppc.read_table(path)
        except ImportError as e:
            print("ImportError:", e)
        """
    )
    out = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, check=True).stdout.splitlines()
    assert out[0] == "[('a', 'PCODEC'), ('b', 'PCODEC'), ('s', 'ZSTD')]"
    assert out[1] == "True"
    assert out[2] == "[2, 3]"
    assert out[3] == "3 ['a', 'b', 's']"
    assert out[4].startswith("ImportError: this function returns pyarrow objects")
    assert "read_batches()" in out[4]


def test_one_writer_shared_by_threads(numbers: pa.Table) -> None:
    buf = io.BytesIO()
    batches = numbers.to_batches(max_chunksize=100)
    with ppc.Writer(buf, numbers.schema, row_group_size=500) as w, ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(w.write, batches * 4))
    back = ppc.read_table(io.BytesIO(buf.getvalue()))
    assert back.num_rows == numbers.num_rows * 4
    # every batch was written whole (writes do not interleave)
    assert sorted(canonical.lines(back)) == sorted(list(canonical.lines(numbers)) * 4)


def test_close_while_threads_write(numbers: pa.Table) -> None:
    buf = io.BytesIO()
    w = ppc.Writer(buf, numbers.schema)
    written: list[int] = []
    refused: list[str] = []
    start = threading.Barrier(5)

    def write() -> None:
        start.wait()
        for b in numbers.to_batches(max_chunksize=50):
            try:
                w.write(b)
                written.append(b.num_rows)
            except ppc.PcodecError as e:
                refused.append(str(e))

    threads = [threading.Thread(target=write) for _ in range(4)]
    for t in threads:
        t.start()
    start.wait()
    w.close()
    for t in threads:
        t.join()
    assert set(refused) <= {"the writer is closed"}
    # what was written before the close is in the file, the rest was refused
    assert ppc.read_table(io.BytesIO(buf.getvalue())).num_rows == sum(written)


def test_the_gil_is_released_while_a_path_is_read(tmp_path: pathlib.Path, big: pa.Table) -> None:
    path = tmp_path / "big.parquet"
    ppc.write_table(big, path, policy="pcodec")
    assert ticks_while(lambda: ppc.ParquetFile(path).read()) > 1000
    # (inspect reads page headers only: too quick to measure)
    assert ticks_while(lambda: ppc.verify(path, path)) > 1000
    out = tmp_path / "out.parquet"
    assert ticks_while(lambda: ppc.transcode(path, out, target="standard")) > 1000


def test_streams_of_one_file_in_many_threads(tmp_path: pathlib.Path, numbers: pa.Table) -> None:
    path = tmp_path / "t.parquet"
    ppc.write_table(numbers, path, row_group_size=200)
    f = ppc.ParquetFile(path)
    want = canonical.dumps(numbers.select(["i64", "s"]))

    def read(i: int) -> str:
        # through the C stream from a thread that does not hold the GIL while pyarrow pulls
        reader = pa.RecordBatchReader.from_stream(f.stream(columns=["i64", "s"], batch_size=37 + i))
        return canonical.dumps(reader.read_all())

    with ThreadPoolExecutor(max_workers=8) as pool:
        assert set(pool.map(read, range(32))) == {want}
