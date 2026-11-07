"""The TSBS DevOps data: what metric stores are benchmarked with, generated here.

TSBS, the Time Series Benchmark Suite (https://github.com/timescale/tsbs, used by TimescaleDB,
InfluxDB, VictoriaMetrics, QuestDB, ClickHouse and others), simulates a fleet of servers that
report system metrics every 10 seconds. This is a port of its `devops` use case: the nine
measurements (cpu, diskio, disk, kernel, mem, net, nginx, postgresl, redis; 101 fields), the ten
tags of a host, and the random walks that make the values, after pkg/data/usecases/devops of
the Go generator. The port draws from Python's random number generator, so the numbers are not
the ones `tsbs_generate_data` writes for the same seed; the model, the schema and the shape of
the data are.

Nothing is downloaded: the data is generated into the cache, and is a function of the seed.
Each host and measurement has its own random stream, so a host's series do not depend on how
many hosts there are, and the two orders hold the same rows.
"""

from __future__ import annotations

import dataclasses
import math
import random
from collections.abc import Callable
from typing import TYPE_CHECKING, Any

import pyarrow as pa
import pyarrow.parquet as pq

if TYPE_CHECKING:
    import pathlib

# TSBS' own examples: `--seed=123`, 10 second intervals, from 2016-01-01T00:00:00Z
SEED = 123
INTERVAL_S = 10
START_S = 1_451_606_400
HOSTS = 100
# one day
TICKS = 24 * 3600 // INTERVAL_S

ORDERS = ("time", "series")

TERABYTE = 1 << 40
INODE_SIZE = 4096
SIXTEEN_GB = 16 << 30

REGIONS = {
    "us-east-1": ("us-east-1a", "us-east-1b", "us-east-1c", "us-east-1e"),
    "us-west-1": ("us-west-1a", "us-west-1b"),
    "us-west-2": ("us-west-2a", "us-west-2b", "us-west-2c"),
    "eu-west-1": ("eu-west-1a", "eu-west-1b", "eu-west-1c"),
    "eu-central-1": ("eu-central-1a", "eu-central-1b"),
    "ap-southeast-1": ("ap-southeast-1a", "ap-southeast-1b"),
    "ap-southeast-2": ("ap-southeast-2a", "ap-southeast-2b"),
    "ap-northeast-1": ("ap-northeast-1a", "ap-northeast-1c"),
    "sa-east-1": ("sa-east-1a", "sa-east-1b", "sa-east-1c"),
}
TEAMS = ("SF", "NYC", "LON", "CHI")
OSES = ("Ubuntu16.10", "Ubuntu16.04LTS", "Ubuntu15.10")
ARCHS = ("x64", "x86")
ENVIRONMENTS = ("production", "staging", "test")


@dataclasses.dataclass(frozen=True)
class Walk:
    """A random walk: each value is the one before plus a step drawn from N(mean, std).

    - `lo` and `hi` clamp the value after each step (TSBS' ClampedRandomWalk);
    - `monotonic` adds the size of the step, so the value never decreases (a counter);
    - `start` is the first value, drawn uniformly from [0, hi] when it is None.

    The first value is the start: the walk steps from the second value on.
    """

    mean: float
    std: float
    lo: float = -math.inf
    hi: float = math.inf
    start: float | None = 0.0
    monotonic: bool = False

    def values(self, rng: random.Random, ticks: int) -> list[float]:
        state = rng.random() * self.hi if self.start is None else self.start
        out = [state]
        for _ in range(ticks - 1):
            step = rng.gauss(self.mean, self.std)
            state += abs(step) if self.monotonic else step
            state = min(max(state, self.lo), self.hi)
            out.append(state)
        return out


def _counter(mean: float) -> Walk:
    return Walk(mean, 1.0, monotonic=True)


def _gauge(mean: float, hi: float, start: float | None = 0.0) -> Walk:
    return Walk(mean, 1.0, 0.0, hi, start)


Column = list[int] | list[float]
# the tags of a measurement besides the ten of the host, and its fields: both in column order
Series = tuple[dict[str, str], dict[str, Column]]
Maker = Callable[[random.Random, int], Series]


def _ints(values: list[float]) -> list[int]:
    # TSBS writes int64(value): truncated towards zero
    return [int(v) for v in values]


def _walks(rng: random.Random, ticks: int, walks: dict[str, Walk]) -> dict[str, Column]:
    return {name: _ints(w.values(rng, ticks)) for name, w in walks.items()}


def _cpu(rng: random.Random, ticks: int) -> Series:
    names = (
        "usage_user",
        "usage_system",
        "usage_idle",
        "usage_nice",
        "usage_iowait",
        "usage_irq",
        "usage_softirq",
        "usage_steal",
        "usage_guest",
        "usage_guest_nice",
    )
    return {}, _walks(rng, ticks, {n: Walk(0.0, 1.0, 0.0, 100.0, None) for n in names})


def _diskio(rng: random.Random, ticks: int) -> Series:
    serial = f"{rng.randrange(1000):03d}-{rng.randrange(1000):03d}-{rng.randrange(1000):03d}"
    walks = {
        "reads": _counter(50.0),
        "writes": _counter(50.0),
        "read_bytes": _counter(100.0),
        "write_bytes": _counter(100.0),
        "read_time": _counter(5.0),
        "write_time": _counter(5.0),
        "io_time": _counter(5.0),
    }
    return {"serial": serial}, _walks(rng, ticks, walks)


def _disk(rng: random.Random, ticks: int) -> Series:
    path = f"/dev/sda{rng.randrange(10)}"
    fstype = rng.choice(("ext3", "ext4", "btrfs"))
    free = _ints(Walk(50.0, 1.0, 0.0, TERABYTE, TERABYTE / 2).values(rng, ticks))
    used = [TERABYTE - f for f in free]
    inodes_total = TERABYTE // INODE_SIZE
    inodes_free = [f // INODE_SIZE for f in free]
    fields: dict[str, Column] = {
        "total": [TERABYTE] * ticks,
        "free": free,
        "used": used,
        "used_percent": [int(100.0 * (u / TERABYTE)) for u in used],
        "inodes_total": [inodes_total] * ticks,
        "inodes_free": inodes_free,
        "inodes_used": [inodes_total - f for f in inodes_free],
    }
    return {"path": path, "fstype": fstype}, fields


def _kernel(rng: random.Random, ticks: int) -> Series:
    boot_time = rng.randrange(240)
    walks = {
        n: _counter(5.0)
        for n in ("interrupts", "context_switches", "processes_forked", "disk_pages_in", "disk_pages_out")
    }
    fields: dict[str, Column] = {"boot_time": [boot_time] * ticks}
    return {}, fields | _walks(rng, ticks, walks)


def _mem(rng: random.Random, ticks: int) -> Series:
    total = rng.choice((8 << 30, 12 << 30, 16 << 30))
    walk = Walk(0.0, total / 64, 0.0, float(total), None)
    used = _ints(walk.values(rng, ticks))
    cached = _ints(walk.values(rng, ticks))
    buffered = _ints(walk.values(rng, ticks))
    available = [total - u for u in used]
    fields: dict[str, Column] = {
        "total": [total] * ticks,
        "available": available,
        "used": used,
        # sic: TSBS writes the available bytes as the free bytes
        "free": list(available),
        "cached": cached,
        "buffered": buffered,
        "used_percent": [100.0 * (u / total) for u in used],
        "available_percent": [100.0 * (a / total) for a in available],
        "buffered_percent": [100.0 * b / total for b in buffered],
    }
    return {}, fields


def _net(rng: random.Random, ticks: int) -> Series:
    interface = f"eth{rng.randrange(4)}"
    walks = {
        "bytes_sent": _counter(50.0),
        "bytes_recv": _counter(50.0),
        "packets_sent": _counter(50.0),
        "packets_recv": _counter(50.0),
        "err_in": _counter(5.0),
        "err_out": _counter(5.0),
        "drop_in": _counter(5.0),
        "drop_out": _counter(5.0),
    }
    return {"interface": interface}, _walks(rng, ticks, walks)


def _nginx(rng: random.Random, ticks: int) -> Series:
    server = f"nginx_{rng.randrange(100000)}"
    port = str(rng.randrange(20000) + 1024)
    walks = {
        "accepts": _counter(5.0),
        "active": _gauge(5.0, 100.0),
        "handled": _counter(5.0),
        "reading": _gauge(5.0, 100.0),
        "requests": _counter(5.0),
        "waiting": _gauge(5.0, 100.0),
        "writing": _gauge(5.0, 100.0),
    }
    return {"port": port, "server": server}, _walks(rng, ticks, walks)


def _postgresl(rng: random.Random, ticks: int) -> Series:
    # TSBS' spelling of the measurement
    names = (
        "numbackends",
        "xact_commit",
        "xact_rollback",
        "blks_read",
        "blks_hit",
        "tup_returned",
        "tup_fetched",
        "tup_inserted",
        "tup_updated",
        "tup_deleted",
        "conflicts",
        "temp_files",
        "temp_bytes",
        "deadlocks",
        "blk_read_time",
        "blk_write_time",
    )
    walks = {n: _gauge(1024.0, float(1 << 30)) if n == "temp_bytes" else _gauge(5.0, 1000.0) for n in names}
    return {}, _walks(rng, ticks, walks)


def _redis(rng: random.Random, ticks: int) -> Series:
    server = f"redis_{rng.randrange(100000)}"
    port = str(rng.randrange(20000) + 1024)
    low, high, half = 5.0, 50.0, SIXTEEN_GB / 2
    walks = {
        "total_connections_received": _counter(low),
        "expired_keys": _counter(high),
        "evicted_keys": _counter(high),
        "keyspace_hits": _counter(high),
        "keyspace_misses": _counter(high),
        "instantaneous_ops_per_sec": Walk(1.0, 1.0),
        "instantaneous_input_kbps": Walk(1.0, 1.0),
        "instantaneous_output_kbps": Walk(1.0, 1.0),
        "connected_clients": _gauge(high, 10000.0),
        "used_memory": _gauge(high, float(SIXTEEN_GB), half),
        "used_memory_rss": _gauge(high, float(SIXTEEN_GB), half),
        "used_memory_peak": _gauge(high, float(SIXTEEN_GB), half),
        "used_memory_lua": _gauge(high, float(SIXTEEN_GB), half),
        "rdb_changes_since_last_save": _gauge(high, 10000.0),
        "sync_full": _gauge(low, 1000.0),
        "sync_partial_ok": _gauge(low, 1000.0),
        "sync_partial_err": _gauge(low, 1000.0),
        "pubsub_channels": _gauge(low, 1000.0),
        "pubsub_patterns": _gauge(low, 1000.0),
        "latest_fork_usec": _gauge(low, 1000.0),
        "connected_slaves": _gauge(low, 1000.0),
        "master_repl_offset": _gauge(low, 1000.0),
        "repl_backlog_active": _gauge(low, 1000.0),
        "repl_backlog_size": _gauge(low, 1000.0),
        "repl_backlog_histlen": _gauge(low, 1000.0),
        "mem_fragmentation_ratio": _gauge(low, 100.0),
        "used_cpu_sys": _gauge(low, 1000.0),
        "used_cpu_user": _gauge(low, 1000.0),
        "used_cpu_sys_children": _gauge(low, 1000.0),
        "used_cpu_user_children": _gauge(low, 1000.0),
    }
    fields: dict[str, Column] = {"uptime_in_seconds": [INTERVAL_S * k for k in range(ticks)]}
    return {"port": port, "server": server}, fields | _walks(rng, ticks, walks)


# the measurements of TSBS devops, in its order
MEASUREMENTS: dict[str, Maker] = {
    "cpu": _cpu,
    "diskio": _diskio,
    "disk": _disk,
    "kernel": _kernel,
    "mem": _mem,
    "net": _net,
    "nginx": _nginx,
    "postgresl": _postgresl,
    "redis": _redis,
}


def _host_tags(rng: random.Random, host: int) -> dict[str, str]:
    region = rng.choice(list(REGIONS))
    return {
        "hostname": f"host_{host}",
        "region": region,
        "datacenter": rng.choice(REGIONS[region]),
        "rack": str(rng.randrange(100)),
        "os": rng.choice(OSES),
        "arch": rng.choice(ARCHS),
        "team": rng.choice(TEAMS),
        "service": str(rng.randrange(20)),
        "service_version": str(rng.randrange(2)),
        "service_environment": rng.choice(ENVIRONMENTS),
    }


def generate(
    measurement: str,
    *,
    hosts: int = HOSTS,
    ticks: int = TICKS,
    seed: int = SEED,
    order: str = "time",
) -> pa.Table:
    """The rows of one measurement for `hosts` hosts reporting `ticks` times, 10 seconds apart.

    The columns are `time` (ns, UTC), the host's ten tags, the measurement's own tags and its
    fields. Order "time" is how the generator writes it, every host's report of a tick before
    the next tick; order "series" is a host's whole series at a time, in time order, as a
    store keyed by series lays it out.
    """
    if measurement not in MEASUREMENTS:
        raise ValueError(f"unknown measurement {measurement!r}")
    if order not in ORDERS:
        raise ValueError(f"unknown order {order!r}: one of {', '.join(ORDERS)}")
    if hosts < 1 or ticks < 1:
        raise ValueError("hosts and ticks must be at least 1")
    make = MEASUREMENTS[measurement]
    times = pa.array([(START_S + INTERVAL_S * k) * 10**9 for k in range(ticks)], pa.int64()).cast(
        pa.timestamp("ns", tz="UTC")
    )
    tables = []
    for host in range(hosts):
        tags = _host_tags(random.Random(f"{seed}/host/{host}"), host)
        own, fields = make(random.Random(f"{seed}/{measurement}/{host}"), ticks)
        columns: dict[str, Any] = {"time": times}
        columns |= {name: pa.repeat(value, ticks) for name, value in (tags | own).items()}
        columns |= {name: pa.array(values) for name, values in fields.items()}
        tables.append(pa.table(columns))
    table = pa.concat_tables(tables).combine_chunks()
    if order == "series":
        return table
    return table.take(pa.array([h * ticks + t for t in range(ticks) for h in range(hosts)], pa.int64()))


@dataclasses.dataclass(frozen=True)
class Tsbs:
    """A fetcher that generates the file instead of downloading it: Parquet, written by
    pyarrow with its defaults (Snappy, dictionaries), through `dest.part`."""

    measurement: str
    order: str = "time"
    hosts: int = HOSTS
    ticks: int = TICKS
    seed: int = SEED

    def __call__(self, dest: pathlib.Path, opener: Any = None) -> None:
        table = generate(self.measurement, hosts=self.hosts, ticks=self.ticks, seed=self.seed, order=self.order)
        dest.parent.mkdir(parents=True, exist_ok=True)
        part = dest.with_name(dest.name + ".part")  # pragma: no mutate (any temporary name)
        try:
            pq.write_table(table, part)
            part.replace(dest)
        finally:
            part.unlink(missing_ok=True)
