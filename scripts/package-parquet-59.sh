#!/usr/bin/env bash
# Writes into DIR the crates.io package tparquet-parquet 59.3.2: the parquet 59.3.2 fork of
# datafusion/parquet (FORK.md there) under the name tparquet-parquet, as rust/parquet publishes
# 60.1.0. The fork itself keeps the name `parquet`: the datafusion/ workspace patches DataFusion's
# own `parquet` dependency with it, and Cargo's [patch] only takes a package of the same name.
# So the rename is made here, on a copy, and nowhere else: the copy differs from the fork only in
# its package name, description, repository, and tparquet-core taken from crates.io.
#
#   scripts/package-parquet-59.sh DIR
#
# DIR must not exist, and must be outside this repository (cargo would take the root workspace
# for the package's). check-publish.sh builds the copy, publish-crates.sh publishes it.
set -euo pipefail

[ $# -eq 1 ] || { echo "usage: $0 DIR" >&2; exit 2; }
out=$1
root=$(cd "$(dirname "$0")/.." && pwd)
[ ! -e "$out" ] || { echo "package-parquet-59: $out exists" >&2; exit 1; }

# the tparquet-core requirement of the 60.1.0 fork, so that both forks need the same core
core=$(python3 -c 'import sys, tomllib
print(tomllib.load(open(sys.argv[1], "rb"))["dependencies"]["tparquet-core"]["version"])' \
    "$root/rust/parquet/Cargo.toml")

mkdir -p "$out"
(cd "$root/datafusion/parquet" && git ls-files -z . ':!FORK.md' ':!FORK.patch' | xargs -0 cp --parents -t "$out")

python3 - "$out/Cargo.toml" "$core" <<'EOF'
import sys
path, core = sys.argv[1], sys.argv[2]
text = open(path).read()
edits = [
    ('[package]\nedition = "2024"\nrust-version = "1.85"\nname = "parquet"\nversion = "59.3.2"\n',
     '[package]\nedition = "2024"\nrust-version = "1.85"\nname = "tparquet-parquet"\nversion = "59.3.2"\n'),
    ('description = "Apache Parquet implementation in Rust"\n',
     'description = "The arrow-rs parquet crate 59.3.2 with the PCODEC compression codec, for tparquet"\n'),
    ('repository = "https://github.com/apache/arrow-rs"\n',
     'repository = "https://github.com/aperiodic-io/tparquet"\n'),
    ('[dependencies.tparquet-core]\npath = "../../rust/core"\n',
     f'[dependencies.tparquet-core]\nversion = "{core}"\n'),
    ('[dev-dependencies.tparquet-core]\npath = "../../rust/core"\n',
     f'[dev-dependencies.tparquet-core]\nversion = "{core}"\n'),
]
for old, new in edits:
    if text.count(old) != 1:
        sys.exit(f"package-parquet-59: {path} does not hold exactly one {old!r}")
    text = text.replace(old, new)
open(path, "w").write(text)
EOF
echo "package-parquet-59: tparquet-parquet 59.3.2 in $out"
