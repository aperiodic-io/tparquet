#!/usr/bin/env bash
# Prepares the patched DuckDB source tree of one DuckDB version (duckdb/FORK.md): downloads the
# DuckDB source distribution from PyPI (cached in target/duckdb/dl), verifies its sha256,
# extracts its DuckDB tree (external/duckdb) and applies duckdb/<VERSION>.patch.
#
#   duckdb/prepare.sh VERSION [DEST]     VERSION: 1.5.6 or 2.0.0; default DEST: target/duckdb/VERSION/src
#
# DEST is left alone when it already holds this patch (DEST/.tparquet-fork records the patch's
# sha256). To change the fork, edit DEST and run scripts/check-duckdb-fork.sh --update VERSION.
set -euo pipefail

version=${1:?usage: duckdb/prepare.sh VERSION [DEST]}
case "$version" in
    1.5.6) sha256=166a91dbfacfc0c9f08cc76c0243cb6d3d4296bfab5bad72a3cfb63140a5b7c8 ;;
    2.0.0) sha256=09de511ef0dc76a5213e8d3d87a8b89bb4d3e8a6fe73a9f60dc001269946b44f ;;
    *) echo "prepare: no fork of DuckDB $version (1.5.6, 2.0.0)" >&2; exit 2 ;;
esac
url="https://files.pythonhosted.org/packages/source/d/duckdb/duckdb-${version}.tar.gz"

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
dest=${2:-$root/target/duckdb/$version/src}
dl="$root/target/duckdb/dl"
tarball="$dl/duckdb-${version}.tar.gz"
patchfile="$here/$version.patch"

grep -q "$sha256" "$here/FORK.md" || { echo "prepare: FORK.md does not record sha256 $sha256" >&2; exit 1; }

patch_sum=$(sha256sum "$patchfile" | cut -d' ' -f1)
if [ -f "$dest/.tparquet-fork" ] && [ "$(cat "$dest/.tparquet-fork")" = "$patch_sum" ]; then
    echo "prepare: $dest is DuckDB ${version} + ${version}.patch"
    exit 0
fi

verified() { echo "$sha256  $tarball" | sha256sum --check --quiet --status - 2>/dev/null; }
if ! verified; then
    mkdir -p "$dl"
    curl -sSfL --retry 3 -o "$tarball.part" "$url"
    mv "$tarball.part" "$tarball"
    verified || { echo "prepare: sha256 mismatch for $url" >&2; rm -f "$tarball"; exit 1; }
fi

rm -rf "$dest" "$dest.tmp"
mkdir -p "$dest.tmp"
tar -xzf "$tarball" -C "$dest.tmp" --strip-components=3 "duckdb-${version}/external/duckdb"
(cd "$dest.tmp" && patch -p1 --batch --forward --quiet < "$patchfile")
# the build appends the extension's footer with scripts/append_metadata.cmake and
# scripts/null.txt (one NUL byte), which the 1.5.6 source distribution leaves out: the patch adds
# the script (DuckDB 2.0.0's), the byte a text patch cannot hold is written here
[ -f "$dest.tmp/scripts/null.txt" ] || printf '\0' > "$dest.tmp/scripts/null.txt"
echo "$patch_sum" > "$dest.tmp/.tparquet-fork"
mv "$dest.tmp" "$dest"
echo "prepare: $dest is DuckDB ${version} (sha256 ${sha256:0:16}...) + ${version}.patch"
