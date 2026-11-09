#!/usr/bin/env bash
# Builds the tparquet DuckDB extension of one DuckDB version (duckdb/FORK.md): DuckDB's own
# parquet extension with the PCODEC codec, as a loadable extension with DuckDB linked in
# statically (EXTENSION_STATIC_BUILD, as DuckDB builds the extensions it distributes: every
# symbol but the entry point tparquet_duckdb_cpp_init stays inside the extension, so its parquet
# code does not mix with the host's), and the codec's C ABI (rust/capi, libtparquet_capi.a)
# linked in. DISABLE_BUILTIN_EXTENSIONS keeps the other extensions out of the build (DuckDB 2.0
# reads it as STATICALLY_LINK_EXTENSIONS=core_functions).
#
#   duckdb/build.sh VERSION      VERSION: 1.5.6 or 2.0.0
#
# Writes duckdb/build/vVERSION/tparquet.duckdb_extension. The build tree is
# target/duckdb/VERSION/build (incremental: rerunning rebuilds what changed).
set -euo pipefail

version=${1:?usage: duckdb/build.sh VERSION}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
src="$root/target/duckdb/$version/src"
build="$root/target/duckdb/$version/build"
out="$here/build/v$version"
jobs=${JOBS:-$(nproc)}

"$here/prepare.sh" "$version" "$src"
cargo build --release --quiet --manifest-path "$root/Cargo.toml" -p tparquet-capi

if [ ! -f "$build/build.ninja" ]; then
    cmake -G Ninja -S "$src" -B "$build" \
        -DCMAKE_BUILD_TYPE=Release \
        -DOVERRIDE_GIT_DESCRIBE="v${version}" \
        -DEXTENSION_STATIC_BUILD=1 \
        -DBUILD_EXTENSIONS=parquet \
        -DDISABLE_BUILTIN_EXTENSIONS=1 \
        -DSKIP_EXTENSIONS=jemalloc \
        -DBUILD_SHELL=0 -DBUILD_UNITTESTS=0 -DENABLE_UNITTEST_CPP_TESTS=0 \
        -DTPARQUET_CAPI_INCLUDE_DIR="$root/rust/capi/include" \
        -DTPARQUET_CAPI_LIBRARY="$root/target/release/libtparquet_capi.a"
fi
ninja -C "$build" -j "$jobs" tparquet_loadable_extension
mkdir -p "$out"
cp "$build/extension/parquet/tparquet.duckdb_extension" "$out/tparquet.duckdb_extension"
echo "build: $out/tparquet.duckdb_extension"
