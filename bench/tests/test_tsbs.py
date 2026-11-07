"""tsbs: the TSBS DevOps data (https://github.com/timescale/tsbs), generated here.

The expected columns, ranges and formulas are those of the Go generator
(pkg/data/usecases/devops), which is the specification of these tests.
"""

from __future__ import annotations

import datetime as dt
import math
import pathlib
import random
import re
from itertools import pairwise
from typing import Any

import pyarrow as pa
import pyarrow.parquet as pq
import pytest

from tpq_bench import fetch, tsbs
from tpq_bench.catalog import Source

HOST_TAGS = [
    "hostname",
    "region",
    "datacenter",
    "rack",
    "os",
    "arch",
    "team",
    "service",
    "service_version",
    "service_environment",
]

# measurement -> (its own tags, its fields): the names and the order of the Go generator
COLUMNS: dict[str, tuple[list[str], list[str]]] = {
    "cpu": (
        [],
        [
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
        ],
    ),
    "diskio": (
        ["serial"],
        ["reads", "writes", "read_bytes", "write_bytes", "read_time", "write_time", "io_time"],
    ),
    "disk": (
        ["path", "fstype"],
        ["total", "free", "used", "used_percent", "inodes_total", "inodes_free", "inodes_used"],
    ),
    "kernel": (
        [],
        ["boot_time", "interrupts", "context_switches", "processes_forked", "disk_pages_in", "disk_pages_out"],
    ),
    "mem": (
        [],
        [
            "total",
            "available",
            "used",
            "free",
            "cached",
            "buffered",
            "used_percent",
            "available_percent",
            "buffered_percent",
        ],
    ),
    "net": (
        ["interface"],
        ["bytes_sent", "bytes_recv", "packets_sent", "packets_recv", "err_in", "err_out", "drop_in", "drop_out"],
    ),
    "nginx": (
        ["port", "server"],
        ["accepts", "active", "handled", "reading", "requests", "waiting", "writing"],
    ),
    "postgresl": (
        [],
        [
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
        ],
    ),
    "redis": (
        ["port", "server"],
        [
            "uptime_in_seconds",
            "total_connections_received",
            "expired_keys",
            "evicted_keys",
            "keyspace_hits",
            "keyspace_misses",
            "instantaneous_ops_per_sec",
            "instantaneous_input_kbps",
            "instantaneous_output_kbps",
            "connected_clients",
            "used_memory",
            "used_memory_rss",
            "used_memory_peak",
            "used_memory_lua",
            "rdb_changes_since_last_save",
            "sync_full",
            "sync_partial_ok",
            "sync_partial_err",
            "pubsub_channels",
            "pubsub_patterns",
            "latest_fork_usec",
            "connected_slaves",
            "master_repl_offset",
            "repl_backlog_active",
            "repl_backlog_size",
            "repl_backlog_histlen",
            "mem_fragmentation_ratio",
            "used_cpu_sys",
            "used_cpu_user",
            "used_cpu_sys_children",
            "used_cpu_user_children",
        ],
    ),
}
FLOATS = {("mem", "used_percent"), ("mem", "available_percent"), ("mem", "buffered_percent")}


def after(mean: float, ticks: int) -> Any:
    """What a walk of N(mean, 1) steps has reached after `ticks` values: `ticks - 1` steps, whose
    sum has a standard deviation of sqrt(ticks - 1); within five of them."""
    return pytest.approx(mean * (ticks - 1), abs=5 * math.sqrt(ticks - 1))


def rows(table: pa.Table) -> list[dict[str, Any]]:
    return table.to_pylist()


def column(table: pa.Table, name: str) -> list[Any]:
    return table.column(name).to_pylist()


def per_host(table: pa.Table, name: str, hosts: int) -> list[list[Any]]:
    """The values of a column of a table in series order, one list per host."""
    values = column(table, name)
    n = len(values) // hosts
    return [values[h * n : (h + 1) * n] for h in range(hosts)]


# --- the walks


def test_a_walk_starts_at_its_start_and_steps_from_the_second_value() -> None:
    w = tsbs.Walk(mean=2.0, std=0.0, start=10.0)
    assert w.values(random.Random(1), 4) == [10.0, 12.0, 14.0, 16.0]


def test_a_walk_has_one_value_per_tick() -> None:
    assert len(tsbs.Walk(mean=0.0, std=1.0, start=0.0).values(random.Random(1), 7)) == 7
    assert tsbs.Walk(mean=0.0, std=1.0, start=3.0).values(random.Random(1), 1) == [3.0]


def test_a_clamped_walk_stays_between_its_bounds() -> None:
    up = tsbs.Walk(mean=30.0, std=0.0, lo=0.0, hi=100.0, start=50.0).values(random.Random(1), 5)
    assert up == [50.0, 80.0, 100.0, 100.0, 100.0]
    down = tsbs.Walk(mean=-30.0, std=0.0, lo=0.0, hi=100.0, start=50.0).values(random.Random(1), 5)
    assert down == [50.0, 20.0, 0.0, 0.0, 0.0]
    wild = tsbs.Walk(mean=0.0, std=1000.0, lo=-5.0, hi=5.0, start=0.0).values(random.Random(1), 500)
    assert min(wild) == -5.0
    assert max(wild) == 5.0


def test_a_monotonic_walk_adds_the_size_of_each_step() -> None:
    w = tsbs.Walk(mean=-5.0, std=0.0, monotonic=True)
    assert w.values(random.Random(1), 4) == [0.0, 5.0, 10.0, 15.0]
    noisy = tsbs.Walk(mean=0.0, std=3.0, monotonic=True).values(random.Random(1), 200)
    assert all(a <= b for a, b in pairwise(noisy))
    assert noisy[-1] > 0


def test_steps_are_normal_with_the_given_mean_and_deviation() -> None:
    values = tsbs.Walk(mean=5.0, std=2.0, start=0.0).values(random.Random(7), 20_001)
    steps = [b - a for a, b in pairwise(values)]
    mean = sum(steps) / len(steps)
    var = sum((s - mean) ** 2 for s in steps) / len(steps)
    assert mean == pytest.approx(5.0, abs=0.1)
    assert var**0.5 == pytest.approx(2.0, abs=0.1)


def test_a_walk_without_a_start_starts_uniformly_between_zero_and_its_upper_bound() -> None:
    starts = [tsbs.Walk(mean=0.0, std=0.0, hi=100.0, start=None).values(random.Random(i), 1)[0] for i in range(300)]
    assert all(0.0 <= s <= 100.0 for s in starts)
    assert min(starts) < 10
    assert max(starts) > 90
    assert len(set(starts)) == 300


# --- the tables


def test_the_nine_measurements_have_the_fields_of_tsbs_devops() -> None:
    assert list(tsbs.MEASUREMENTS) == list(COLUMNS)
    # "101 metrics", as TSBS counts them
    assert sum(len(fields) for _, fields in COLUMNS.values()) == 101


@pytest.mark.parametrize("name", list(COLUMNS))
def test_columns_and_types(name: str) -> None:
    table = tsbs.generate(name, hosts=3, ticks=4)
    tags, fields = COLUMNS[name]
    assert table.schema.names == ["time", *HOST_TAGS, *tags, *fields]
    assert table.schema.field("time").type == pa.timestamp("ns", tz="UTC")
    for t in [*HOST_TAGS, *tags]:
        assert table.schema.field(t).type == pa.string(), t
    for f in fields:
        assert table.schema.field(f).type == (pa.float64() if (name, f) in FLOATS else pa.int64()), f
    assert table.num_rows == 12
    for c in table.schema.names:
        assert table.column(c).null_count == 0, c


def test_an_unknown_measurement() -> None:
    with pytest.raises(ValueError, match="unknown measurement 'cpu-only'"):
        tsbs.generate("cpu-only", hosts=1, ticks=1)


def test_an_unknown_order() -> None:
    with pytest.raises(ValueError, match="unknown order 'random'"):
        tsbs.generate("cpu", hosts=1, ticks=1, order="random")


def test_nothing_to_generate() -> None:
    with pytest.raises(ValueError, match="hosts and ticks"):
        tsbs.generate("cpu", hosts=0, ticks=1)
    with pytest.raises(ValueError, match="hosts and ticks"):
        tsbs.generate("cpu", hosts=1, ticks=0)


def test_time_order_is_a_tick_of_every_host_at_a_time() -> None:
    t = tsbs.generate("cpu", hosts=3, ticks=4, order="time")
    assert column(t, "hostname") == ["host_0", "host_1", "host_2"] * 4
    start = dt.datetime(2016, 1, 1, tzinfo=dt.timezone.utc)
    times = column(t, "time")
    assert times[:3] == [start] * 3
    assert times == [start + dt.timedelta(seconds=10 * k) for k in range(4) for _ in range(3)]


def test_series_order_is_every_tick_of_a_host_at_a_time() -> None:
    t = tsbs.generate("cpu", hosts=3, ticks=4, order="series")
    assert column(t, "hostname") == ["host_0"] * 4 + ["host_1"] * 4 + ["host_2"] * 4
    start = dt.datetime(2016, 1, 1, tzinfo=dt.timezone.utc)
    assert column(t, "time") == [start + dt.timedelta(seconds=10 * k) for _ in range(3) for k in range(4)]


def test_the_two_orders_hold_the_same_rows() -> None:
    by_time = tsbs.generate("mem", hosts=4, ticks=6, order="time")
    by_series = tsbs.generate("mem", hosts=4, ticks=6, order="series")
    assert by_time != by_series
    assert by_time.sort_by([("hostname", "ascending"), ("time", "ascending")]) == by_series


def test_time_order_is_the_default() -> None:
    assert tsbs.generate("cpu", hosts=2, ticks=3) == tsbs.generate("cpu", hosts=2, ticks=3, order="time")


def test_the_data_is_a_function_of_the_seed() -> None:
    a = tsbs.generate("cpu", hosts=3, ticks=50, seed=1)
    assert tsbs.generate("cpu", hosts=3, ticks=50, seed=1) == a
    assert tsbs.generate("cpu", hosts=3, ticks=50, seed=2) != a
    # the default seed is the one of TSBS' own examples
    assert tsbs.SEED == 123
    assert tsbs.generate("cpu", hosts=3, ticks=50) == tsbs.generate("cpu", hosts=3, ticks=50, seed=123)


def test_a_host_is_the_same_whatever_the_other_hosts_are() -> None:
    few = tsbs.generate("cpu", hosts=2, ticks=20, order="series")
    many = tsbs.generate("cpu", hosts=5, ticks=20, order="series")
    assert few == many.slice(0, 40)


def test_host_tags_come_from_the_choices_of_tsbs_and_do_not_change() -> None:
    hosts, ticks = 100, 3
    t = tsbs.generate("cpu", hosts=hosts, ticks=ticks, order="series")
    regions = {
        "us-east-1": {"us-east-1a", "us-east-1b", "us-east-1c", "us-east-1e"},
        "us-west-1": {"us-west-1a", "us-west-1b"},
        "us-west-2": {"us-west-2a", "us-west-2b", "us-west-2c"},
        "eu-west-1": {"eu-west-1a", "eu-west-1b", "eu-west-1c"},
        "eu-central-1": {"eu-central-1a", "eu-central-1b"},
        "ap-southeast-1": {"ap-southeast-1a", "ap-southeast-1b"},
        "ap-southeast-2": {"ap-southeast-2a", "ap-southeast-2b"},
        "ap-northeast-1": {"ap-northeast-1a", "ap-northeast-1c"},
        "sa-east-1": {"sa-east-1a", "sa-east-1b", "sa-east-1c"},
    }
    for name in HOST_TAGS:
        for host in per_host(t, name, hosts):
            assert len(set(host)) == 1, name
    first = {name: [h[0] for h in per_host(t, name, hosts)] for name in HOST_TAGS}
    assert first["hostname"] == [f"host_{i}" for i in range(hosts)]
    assert set(first["region"]) == set(regions)  # every region turns up in 100 hosts
    for region, datacenter in zip(first["region"], first["datacenter"]):
        assert datacenter in regions[region]
    assert {int(r) for r in first["rack"]} <= set(range(100))
    assert len(set(first["rack"])) > 30
    assert set(first["os"]) == {"Ubuntu16.10", "Ubuntu16.04LTS", "Ubuntu15.10"}
    assert set(first["arch"]) == {"x64", "x86"}
    assert set(first["team"]) == {"SF", "NYC", "LON", "CHI"}
    assert {int(s) for s in first["service"]} <= set(range(20))
    assert len(set(first["service"])) > 10
    assert set(first["service_version"]) == {"0", "1"}
    assert set(first["service_environment"]) == {"production", "staging", "test"}


def test_a_host_has_the_same_tags_in_every_measurement() -> None:
    cpu = tsbs.generate("cpu", hosts=8, ticks=2, order="series")
    for name in ("mem", "disk", "redis"):
        other = tsbs.generate(name, hosts=8, ticks=2, order="series")
        assert other.select(HOST_TAGS) == cpu.select(HOST_TAGS), name


# --- the values of each measurement


def test_cpu_walks_stay_in_zero_to_a_hundred_in_small_steps() -> None:
    hosts, ticks = 3, 2000
    t = tsbs.generate("cpu", hosts=hosts, ticks=ticks, order="series")
    for field in COLUMNS["cpu"][1]:
        for series in per_host(t, field, hosts):
            assert min(series) >= 0
            assert max(series) <= 100
            # steps are N(0, 1); the values are truncated to integers
            assert max(abs(b - a) for a, b in pairwise(series)) <= 8
            # a walk: it moves
            assert len(set(series)) > 5
    # the start is uniform: the fields of a host do not all start alike
    assert len({column(t, f)[0] for f in COLUMNS["cpu"][1]}) > 3


def test_the_ten_cpu_fields_are_independent_walks() -> None:
    t = tsbs.generate("cpu", hosts=1, ticks=300)
    assert column(t, "usage_user") != column(t, "usage_system")
    assert column(t, "usage_idle") != column(t, "usage_nice")


def test_kernel_counters_never_decrease_and_boot_time_is_fixed() -> None:
    hosts, ticks = 4, 300
    t = tsbs.generate("kernel", hosts=hosts, ticks=ticks, order="series")
    for field in ("interrupts", "context_switches", "processes_forked", "disk_pages_in", "disk_pages_out"):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert all(a <= b for a, b in pairwise(series))
            # a step is |N(5, 1)|: about 5 a tick
            assert series[-1] == after(5, ticks)
    boots = per_host(t, "boot_time", hosts)
    assert all(len(set(b)) == 1 for b in boots)
    assert {b[0] for b in boots} <= set(range(240))


def test_kernel_boot_times_differ_between_hosts() -> None:
    t = tsbs.generate("kernel", hosts=40, ticks=1)
    assert len(set(column(t, "boot_time"))) > 10


def test_disk_follows_the_arithmetic_of_tsbs() -> None:
    hosts, ticks = 3, 200
    t = tsbs.generate("disk", hosts=hosts, ticks=ticks, order="series")
    terabyte = 1 << 40
    assert set(column(t, "total")) == {terabyte}
    assert set(column(t, "inodes_total")) == {terabyte // 4096}
    for r in rows(t):
        assert r["free"] + r["used"] == terabyte
        assert r["used_percent"] == int(100.0 * (r["used"] / terabyte))
        assert r["inodes_free"] == r["free"] // 4096
        assert r["inodes_used"] == r["inodes_total"] - r["inodes_free"]
        assert 0 <= r["free"] <= terabyte
    # `free` starts at half a terabyte and moves by about 50 bytes a tick
    assert {s[0] for s in per_host(t, "free", hosts)} == {terabyte // 2}
    assert all(s[-1] > terabyte // 2 for s in per_host(t, "free", hosts))
    # exactly half used to begin with; a little less than half after, which truncates to 49
    assert {p[0] for p in per_host(t, "used_percent", hosts)} == {50}
    assert {p[-1] for p in per_host(t, "used_percent", hosts)} == {49}


def test_disk_tags() -> None:
    t = tsbs.generate("disk", hosts=60, ticks=2, order="series")
    first = rows(t)[::2]
    assert {r["path"] for r in first} <= {f"/dev/sda{i}" for i in range(10)}
    assert len({r["path"] for r in first}) > 5
    assert {r["fstype"] for r in first} == {"ext3", "ext4", "btrfs"}
    for host in range(60):
        assert rows(t)[2 * host]["path"] == rows(t)[2 * host + 1]["path"]
        assert rows(t)[2 * host]["fstype"] == rows(t)[2 * host + 1]["fstype"]


def test_mem_follows_the_arithmetic_of_tsbs() -> None:
    hosts, ticks = 3, 400
    t = tsbs.generate("mem", hosts=hosts, ticks=ticks, order="series")
    totals = {8 << 30, 12 << 30, 16 << 30}
    for host in per_host(t, "total", hosts):
        assert len(set(host)) == 1
        assert host[0] in totals
    for r in rows(t):
        total = r["total"]
        assert r["available"] == total - r["used"]
        # sic: TSBS writes `available` as `free`
        assert r["free"] == r["available"]
        assert 0 <= r["used"] <= total
        assert 0 <= r["cached"] <= total
        assert 0 <= r["buffered"] <= total
        assert r["used_percent"] == pytest.approx(100.0 * r["used"] / total, rel=1e-12)
        assert r["available_percent"] == pytest.approx(100.0 * r["available"] / total, rel=1e-12)
        assert r["buffered_percent"] == pytest.approx(100.0 * r["buffered"] / total, rel=1e-12)
    # used, cached and buffered start uniformly in [0, total], and walk in steps of total/64
    for field in ("used", "cached", "buffered"):
        for total, series in zip(per_host(t, "total", hosts), per_host(t, field, hosts)):
            assert 0 < series[0] < total[0]
            assert max(abs(b - a) for a, b in pairwise(series)) < total[0] / 64 * 6
            assert len(set(series)) > 100


def test_mem_totals_are_all_three_sizes() -> None:
    t = tsbs.generate("mem", hosts=40, ticks=1)
    assert set(column(t, "total")) == {8 << 30, 12 << 30, 16 << 30}


def test_diskio_serials_and_counters() -> None:
    hosts, ticks = 40, 100
    t = tsbs.generate("diskio", hosts=hosts, ticks=ticks, order="series")
    serials = [s[0] for s in per_host(t, "serial", hosts)]
    assert all(re.fullmatch(r"\d{3}-\d{3}-\d{3}", s) for s in serials)
    assert len(set(serials)) == hosts
    assert all(len(set(s)) == 1 for s in per_host(t, "serial", hosts))
    for field, per_tick in (
        ("reads", 50),
        ("writes", 50),
        ("read_bytes", 100),
        ("write_bytes", 100),
        ("read_time", 5),
        ("write_time", 5),
        ("io_time", 5),
    ):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert all(a <= b for a, b in pairwise(series))
            assert series[-1] == after(per_tick, ticks)


def test_net_interfaces_and_counters() -> None:
    hosts, ticks = 40, 100
    t = tsbs.generate("net", hosts=hosts, ticks=ticks, order="series")
    interfaces = [s[0] for s in per_host(t, "interface", hosts)]
    assert set(interfaces) == {"eth0", "eth1", "eth2", "eth3"}
    for field, per_tick in (
        ("bytes_sent", 50),
        ("bytes_recv", 50),
        ("packets_sent", 50),
        ("packets_recv", 50),
        ("err_in", 5),
        ("err_out", 5),
        ("drop_in", 5),
        ("drop_out", 5),
    ):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert all(a <= b for a, b in pairwise(series))
            assert series[-1] == after(per_tick, ticks)


def test_nginx_tags_gauges_and_counters() -> None:
    hosts, ticks = 30, 300
    t = tsbs.generate("nginx", hosts=hosts, ticks=ticks, order="series")
    ports = [s[0] for s in per_host(t, "port", hosts)]
    servers = [s[0] for s in per_host(t, "server", hosts)]
    assert all(1024 <= int(p) < 1024 + 20000 for p in ports)
    assert all(re.fullmatch(r"nginx_\d+", s) and int(s[6:]) < 100000 for s in servers)
    assert len(set(servers)) > 20
    assert all(len(set(s)) == 1 for s in per_host(t, "port", hosts) + per_host(t, "server", hosts))
    for field in ("accepts", "handled", "requests"):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert all(a <= b for a, b in pairwise(series))
    for field in ("active", "reading", "waiting", "writing"):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert min(series) >= 0
            assert max(series) <= 100
    # a gauge clamped at 0 and 100 with steps N(5, 1) climbs to its ceiling
    assert all(series[-1] == 100 for series in per_host(t, "active", hosts))


def test_postgresl_gauges_are_clamped() -> None:
    hosts, ticks = 3, 150
    t = tsbs.generate("postgresl", hosts=hosts, ticks=ticks, order="series")
    for field in COLUMNS["postgresl"][1]:
        ceiling = 1 << 30 if field == "temp_bytes" else 1000
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert min(series) >= 0
            assert max(series) <= ceiling
    # N(5, 1) steps: 5 a tick, 745 after 150 ticks, short of the ceiling of 1000
    assert all(series[-1] == after(5, ticks) for series in per_host(t, "xact_commit", hosts))
    # temp_bytes steps are N(1024, 1)
    assert all(series[-1] == after(1024, ticks) for series in per_host(t, "temp_bytes", hosts))


def test_redis_tags_uptime_and_fields() -> None:
    hosts, ticks = 30, 150
    t = tsbs.generate("redis", hosts=hosts, ticks=ticks, order="series")
    assert all(1024 <= int(s[0]) < 1024 + 20000 for s in per_host(t, "port", hosts))
    servers = [s[0] for s in per_host(t, "server", hosts)]
    assert all(re.fullmatch(r"redis_\d+", s) and int(s[6:]) < 100000 for s in servers)
    for series in per_host(t, "uptime_in_seconds", hosts):
        assert series == [10 * k for k in range(ticks)]
    for field in ("total_connections_received", "expired_keys", "evicted_keys", "keyspace_hits", "keyspace_misses"):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert all(a <= b for a, b in pairwise(series))
    sixteen_gb = 16 << 30
    for field in ("used_memory", "used_memory_rss", "used_memory_peak", "used_memory_lua"):
        for series in per_host(t, field, hosts):
            assert series[0] == sixteen_gb // 2
            assert min(series) >= 0
            assert max(series) <= sixteen_gb
    for field, ceiling in (("connected_clients", 10000), ("rdb_changes_since_last_save", 10000)):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert max(series) <= ceiling
            assert series[-1] == after(50, ticks)
    for field in (
        "sync_full",
        "sync_partial_ok",
        "sync_partial_err",
        "pubsub_channels",
        "pubsub_patterns",
        "latest_fork_usec",
        "connected_slaves",
        "master_repl_offset",
        "repl_backlog_active",
        "repl_backlog_size",
        "repl_backlog_histlen",
        "used_cpu_sys",
        "used_cpu_user",
        "used_cpu_sys_children",
        "used_cpu_user_children",
    ):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert max(series) <= 1000
            assert series[-1] == after(5, ticks)
    for series in per_host(t, "mem_fragmentation_ratio", hosts):
        assert max(series) <= 100
        assert series[-1] == 100  # N(5, 1) steps reach the ceiling of 100 within 150 ticks
    # unclamped walks with N(1, 1) steps
    for field in ("instantaneous_ops_per_sec", "instantaneous_input_kbps", "instantaneous_output_kbps"):
        for series in per_host(t, field, hosts):
            assert series[0] == 0
            assert series[-1] == after(1, ticks)


# --- the file in the cache


def test_the_fetcher_writes_the_table_as_parquet(tmp_path: pathlib.Path) -> None:
    dest = tmp_path / "tsbs" / "cpu.parquet"
    tsbs.Tsbs("cpu", order="series", hosts=4, ticks=30, seed=9)(dest)
    assert pq.read_table(dest) == tsbs.generate("cpu", hosts=4, ticks=30, seed=9, order="series")
    assert list(dest.parent.iterdir()) == [dest]


def test_the_fetcher_ignores_the_opener(tmp_path: pathlib.Path) -> None:
    def opener(*args: object, **kwargs: object) -> None:
        raise AssertionError("nothing is downloaded")

    tsbs.Tsbs("mem", hosts=2, ticks=3)(tmp_path / "m.parquet", opener)
    assert pq.read_table(tmp_path / "m.parquet").num_rows == 6


def test_the_fetcher_defaults_are_the_benchmarks() -> None:
    f = tsbs.Tsbs("cpu")
    assert (f.order, f.hosts, f.ticks, f.seed) == ("time", 100, 8640, 123)
    assert (tsbs.HOSTS, tsbs.TICKS, tsbs.INTERVAL_S) == (100, 8640, 10)


def test_a_failed_generation_leaves_nothing(tmp_path: pathlib.Path) -> None:
    dest = tmp_path / "x.parquet"
    with pytest.raises(ValueError, match="unknown measurement"):
        tsbs.Tsbs("gpu", hosts=1, ticks=1)(dest)
    assert list(tmp_path.iterdir()) == []


def test_ensure_generates_once(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[str] = []
    real = tsbs.generate

    def counting(name: str, **kwargs: Any) -> pa.Table:
        calls.append(name)
        return real(name, **kwargs)

    monkeypatch.setattr(tsbs, "generate", counting)
    s = Source("TSBS", "cpu", "observability", "tsbs/cpu.parquet", "parquet", tsbs.Tsbs("cpu", hosts=2, ticks=3))
    path = fetch.ensure(s, tmp_path)
    assert path == tmp_path / "tsbs" / "cpu.parquet"
    assert pq.read_table(path).num_rows == 6
    assert fetch.ensure(s, tmp_path) == path
    assert calls == ["cpu"]
