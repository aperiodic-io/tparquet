#!/usr/bin/env bash
# Checks that what the release workflow (.github/workflows/release.yml) publishes is publishable:
#
#   1. the versions agree: tparquet-core, tparquet, the Python package (tparquet-python's crate
#      version, which maturin uses) and polars-tparquet have one version, and it is TAG's (v1.2.3
#      or 1.2.3) when TAG is given;
#   2. the crates.io crates package and build from their packaged sources alone, in dependency
#      order, each against the others' packages (cargo publish --dry-run);
#   3. the tparquet crate ships the tparquet CLI and nothing that needs this repository
#      (gen-testdata writes into testdata/, the tests read it).
#
#   scripts/check-publish.sh [--allow-dirty] [TAG]
#
# --allow-dirty packages uncommitted changes too (cargo publish refuses them otherwise).
set -euo pipefail

dirty=()
if [ "${1:-}" = "--allow-dirty" ]; then
    dirty=(--allow-dirty)
    shift
fi

# the crates.io crates, in the order they are published (each depends on the ones before it)
crates=(tparquet-core tparquet-parquet tparquet)

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

fail() {
    echo "check-publish: $*" >&2
    exit 1
}

metadata=$(cargo metadata --format-version 1 --no-deps --locked)
version_of() {
    python3 -c 'import json, sys
m = json.load(sys.stdin)
v = [p["version"] for p in m["packages"] if p["name"] == sys.argv[1]]
print(v[0] if v else "")' "$1" <<< "$metadata"
}

version=$(version_of tparquet)
[ -n "$version" ] || fail "no package tparquet in the workspace"
for p in tparquet-core tparquet-python; do
    v=$(version_of "$p")
    [ "$v" = "$version" ] || fail "$p is at version '$v', tparquet at $version: release them together"
done
v=$(python3 -c 'import sys, tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["project"]["version"])' \
    polars-tparquet/pyproject.toml)
[ "$v" = "$version" ] || fail "polars-tparquet is at version '$v', tparquet at $version: release them together"
if [ $# -gt 0 ]; then
    [ "${1#v}" = "$version" ] || fail "tag $1 is not the packages' version $version"
fi
for p in "${crates[@]}"; do
    [ -n "$(version_of "$p")" ] || fail "no package $p in the workspace"
done

args=()
for p in "${crates[@]}"; do args+=(-p "$p"); done
cargo publish --dry-run --locked "${dirty[@]}" "${args[@]}"

# the dry run leaves each package unpacked, as published, in target/package/<name>-<version>
package="target/package/tparquet-${version}"
[ -f "$package/Cargo.toml" ] || fail "$package: no packaged tparquet"
bins=$(python3 -c 'import sys, tomllib; print(" ".join(b["name"] for b in tomllib.load(open(sys.argv[1], "rb")).get("bin", [])))' \
    "$package/Cargo.toml")
[ "$bins" = "tparquet" ] || fail "the tparquet package has the binaries '$bins'; it must have exactly tparquet"
if [ -e "$package/tests" ] || [ -e "$package/src/bin/gen-testdata" ]; then
    fail "the tparquet package contains tests/ or gen-testdata, which need this repository"
fi

echo "check-publish: ${crates[*]} and the Python packages at $version are publishable"
