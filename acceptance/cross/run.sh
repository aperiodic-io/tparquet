#!/usr/bin/env bash
# Cross-language acceptance: every implementation reads the vectors every other one wrote.
#
#   writers: rust (testdata/), go (tparquet-go-gen), python (transcode, write_table with each
#            policy), duckdb (COPY with PCODEC and with snappy), rust-cli (tparquet encode with each
#            policy, of the rust and go vectors), java (TparquetWriter on the parquet-java fork),
#            cpp (tparquet::FileWriter on the Arrow C++ fork), datafusion (COPY ... STORED AS
#            TPARQUET on the parquet 59 fork)
#   readers: rust (gen-testdata --check), go (tparquet-go-gen -check), python, duckdb, java, cpp,
#            datafusion
#
# Needs: the Rust workspace, Go, python/.venv with the package installed (`make python-test`
# or `maturin develop`), the built DuckDB extension (`make -C duckdb build`, for the DuckDB of that
# environment), and the Java
# packages installed (`make -C java install`, JDK 21 and Maven), and the C++ library (`make -C
# cpp build`; TPARQUET_CPP_VECTORS names a built tparquet-cpp-vectors instead).
# Usage: acceptance/cross/run.sh [OUT]   (default: target/cross)
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
# cargo builds into $CARGO_TARGET_DIR when it is set
target=${CARGO_TARGET_DIR:-$root/target}
out=${1:-$root/target/cross}
py=${PYTHON:-$root/python/.venv/bin/python}
# the build of the extension for the DuckDB of that Python environment
ext=${TPARQUET_DUCKDB_EXTENSION:-$root/duckdb/build/v$("$py" -c 'import duckdb; print(duckdb.__version__)')/tparquet.duckdb_extension}
vectors="$root/acceptance/cross/vectors.py"
mkdir -p "$out"

cargo build -q --manifest-path "$root/Cargo.toml" -p tparquet --bin gen-testdata --bin tparquet
cargo build -q --release --manifest-path "$root/Cargo.toml" -p tparquet-capi
rust_check() { "$target/debug/gen-testdata" --check "$1"; }
# the Go package links ../target/release/libtparquet_capi.a (go/link.go), or with the
# tparquet_external tag the library of TPARQUET_LIB_DIR (go/Makefile)
lib_dir=${TPARQUET_LIB_DIR:-}
if [ -z "$lib_dir" ] && [ "$target" != "$root/target" ]; then lib_dir=$target/release; fi
if [ -n "$lib_dir" ]; then
  (cd "$root/go" && CGO_LDFLAGS="$lib_dir/libtparquet_capi.a" \
    go build -tags tparquet_external -o "$out/tparquet-go-gen" ./cmd/tparquet-go-gen)
else
  (cd "$root/go" && go build -o "$out/tparquet-go-gen" ./cmd/tparquet-go-gen)
fi
go_check() { "$out/tparquet-go-gen" -check "$1"; }
python_check() { "$py" "$vectors" check-python "$1"; }
duckdb_check() { "$py" "$vectors" check-duckdb "$ext" "$1"; }
# the Java CLI (java/tparquet, io.aperiodic.tparquet.Vectors) on the parquet-java fork
make -s -C "$root/java" jni
java_cli() { make -s -C "$root/java" vectors ARGS="$*"; }
java_check() { java_cli check "$1"; }
# the C++ CLI (cpp/, tparquet-cpp-vectors) on the Arrow C++ fork
cpp_cli=${TPARQUET_CPP_VECTORS:-$root/target/cpp/build-release/tparquet-cpp-vectors}
[ -n "${TPARQUET_CPP_VECTORS:-}" ] || make -s -C "$root/cpp" build > /dev/null
cpp_check() { "$cpp_cli" check "$1"; }
# the DataFusion CLI (datafusion/, its own Cargo workspace)
cargo build -q --manifest-path "$root/datafusion/Cargo.toml" --bin tparquet-datafusion-vectors
df_cli="$root/datafusion/target/debug/tparquet-datafusion-vectors"
datafusion_check() { "$df_cli" check "$1"; }

echo "== writers"
"$out/tparquet-go-gen" "$out/go" > /dev/null
"$py" "$vectors" write-python "$out/python" "$root/testdata" "$out/go"
"$py" "$vectors" write-duckdb "$ext" "$out/duckdb" "$root/testdata" "$out/go"
"$py" "$vectors" write-cli "$target/debug/tparquet" "$out/rust-cli" "$root/testdata" "$out/go"
rm -rf "$out/java" && java_cli write "$out/java" "$root/testdata" "$out/go"
rm -rf "$out/cpp" && "$cpp_cli" write "$out/cpp" "$root/testdata" "$out/go" > /dev/null
rm -rf "$out/datafusion" && "$df_cli" write "$out/datafusion" "$root/testdata" "$out/go" > /dev/null

declare -A dirs=([rust]="$root/testdata" [go]="$out/go" [python]="$out/python" [duckdb]="$out/duckdb"
  [rust-cli]="$out/rust-cli" [java]="$out/java" [cpp]="$out/cpp" [datafusion]="$out/datafusion")
for writer in rust go python duckdb rust-cli java cpp datafusion; do
  for reader in rust go python duckdb java cpp datafusion; do
    printf '%-10s -> %-10s ' "$writer" "$reader"
    "${reader}_check" "${dirs[$writer]}"
  done
done

# every checker must also notice a difference: one changed row in a copy of the Rust vectors
echo "== the checkers fail on a changed row"
bad="$out/tampered"
rm -rf "$bad" && mkdir -p "$bad"
cp -r "$root/testdata/files" "$root/testdata/expected" "$bad/"
sed -i '2s/1/2/' "$bad/expected/temporal.jsonl"
for reader in rust go python duckdb java cpp datafusion; do
  if "${reader}_check" "$bad" > /dev/null 2>&1; then
    echo "$reader accepted a changed row" >&2
    exit 1
  fi
  echo "$reader: refused"
done
echo "cross-language acceptance: ok"
