#!/usr/bin/env bash
# Publishes the crates.io crates (scripts/check-publish.sh) whose version crates.io does not have
# yet, in dependency order. tparquet-parquet keeps upstream's version (rust/parquet/FORK.md), so a
# release of tparquet often has no new tparquet-parquet; a version already published is skipped
# rather than refused, which also makes a failed release safe to run again.
#
#   CARGO_REGISTRY_TOKEN=... scripts/publish-crates.sh [--dry-run]
set -euo pipefail

crates=(tparquet-core tparquet-parquet tparquet)

dry_run=()
if [ "${1:-}" = "--dry-run" ]; then
    dry_run=(--dry-run)
fi

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

metadata=$(cargo metadata --format-version 1 --no-deps --locked)

# published NAME VERSION: whether crates.io's index lists NAME at VERSION (yanked or not)
published() {
    python3 - "$1" "$2" <<'EOF'
import json, sys, urllib.error, urllib.request
name, version = sys.argv[1], sys.argv[2]
n = name.lower()
prefix = {1: "1", 2: "2", 3: f"3/{n[0]}"}.get(len(n), f"{n[:2]}/{n[2:4]}")
request = urllib.request.Request(f"https://index.crates.io/{prefix}/{n}", headers={"User-Agent": "tparquet-release"})
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        lines = response.read().decode().splitlines()
except urllib.error.HTTPError as e:
    if e.code == 404:  # no version of the crate yet
        sys.exit(1)
    raise
sys.exit(0 if any(json.loads(line)["vers"] == version for line in lines if line) else 1)
EOF
}

args=()
for p in "${crates[@]}"; do
    v=$(python3 -c 'import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == sys.argv[1]))' "$p" <<< "$metadata")
    if published "$p" "$v"; then
        echo "publish-crates: $p $v is on crates.io already, skipped"
    else
        echo "publish-crates: $p $v"
        args+=(-p "$p")
    fi
done

if [ "${#args[@]}" -eq 0 ]; then
    echo "publish-crates: nothing to publish"
    exit 0
fi
cargo publish --locked "${dry_run[@]}" "${args[@]}"
