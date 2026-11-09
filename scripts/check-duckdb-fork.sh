#!/usr/bin/env bash
# Checks the fork of DuckDB's parquet extension in duckdb/ (duckdb/FORK.md): for each DuckDB
# version, downloads its source distribution (cached in target/duckdb/dl), verifies its sha256,
# checks that FORK.md and prepare.sh record it, that duckdb/VERSION.patch applies cleanly to its
# DuckDB tree, and that it only touches the files FORK.md lists.
#
#   scripts/check-duckdb-fork.sh                         check every version
#   scripts/check-duckdb-fork.sh --update VERSION [SRC]  rewrite duckdb/VERSION.patch from the
#                                                        edited tree SRC (default target/duckdb/VERSION/src)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
fork="$root/duckdb"
dl="$root/target/duckdb/dl"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

sha256_of() {
    case "$1" in
        1.5.6) echo 166a91dbfacfc0c9f08cc76c0243cb6d3d4296bfab5bad72a3cfb63140a5b7c8 ;;
        2.0.0) echo 09de511ef0dc76a5213e8d3d87a8b89bb4d3e8a6fe73a9f60dc001269946b44f ;;
        *) echo "check-duckdb-fork: no fork of DuckDB $1" >&2; exit 2 ;;
    esac
}

# the verified DuckDB tree of VERSION in $work/upstream-VERSION
upstream() {
    local version=$1 sha256 tarball url
    sha256=$(sha256_of "$version")
    tarball="$dl/duckdb-${version}.tar.gz"
    url="https://files.pythonhosted.org/packages/source/d/duckdb/duckdb-${version}.tar.gz"
    grep -q "$sha256" "$fork/FORK.md" || { echo "check-duckdb-fork: FORK.md does not record sha256 $sha256 ($version)" >&2; exit 1; }
    grep -q "$sha256" "$fork/prepare.sh" || { echo "check-duckdb-fork: prepare.sh does not verify sha256 $sha256 ($version)" >&2; exit 1; }
    verified() { echo "$sha256  $tarball" | sha256sum --check --quiet --status - 2>/dev/null; }
    if ! verified; then
        mkdir -p "$dl"
        curl -sSfL --retry 3 -o "$tarball.part" "$url"
        mv "$tarball.part" "$tarball"
        verified || { echo "check-duckdb-fork: sha256 mismatch for $url" >&2; rm -f "$tarball"; exit 1; }
    fi
    mkdir "$work/upstream-$version"
    tar -xzf "$tarball" -C "$work/upstream-$version" --strip-components=3 "duckdb-${version}/external/duckdb"
}

if [ "${1:-}" = "--update" ]; then
    version=${2:?usage: scripts/check-duckdb-fork.sh --update VERSION [SRC]}
    src=${3:-$root/target/duckdb/$version/src}
    upstream "$version"
    mkdir "$work/fork"
    # only the files that differ: the tree is large
    (cd "$src" && find . -type f ! -name .tparquet-fork ! -path './.git/*' -print0) |
        while IFS= read -r -d '' f; do
            if ! cmp -s "$src/$f" "$work/upstream-$version/$f"; then
                mkdir -p "$work/fork/$(dirname "$f")" "$work/base/$(dirname "$f")"
                cp "$src/$f" "$work/fork/$f"
                [ -f "$work/upstream-$version/$f" ] && cp "$work/upstream-$version/$f" "$work/base/$f"
            fi
        done
    cd "$work"
    status=0
    git -c core.quotepath=off diff --no-index --no-color --no-ext-diff --no-renames \
        --diff-algorithm=myers --no-prefix base fork > fork.diff || status=$?
    [ "$status" -le 1 ] || { echo "check-duckdb-fork: git diff failed" >&2; exit "$status"; }
    cp fork.diff "$fork/$version.patch"
    sha256sum "$fork/$version.patch" | cut -d' ' -f1 > "$src/.tparquet-fork"
    echo "check-duckdb-fork: wrote duckdb/$version.patch ($(grep -c '^diff --git' fork.diff) files, $(wc -l < fork.diff) lines)"
    exit 0
fi

# the files of FORK.md's table, but the rows of another version only ("1.5.6 only")
listed() { grep -E '^\| `(third_party|extension|scripts)/' "$fork/FORK.md" | grep -Ev "\(($(echo 1.5.6 2.0.0 | tr ' ' '\n' | grep -vx "$1" | tr '\n' '|' | sed 's/|$//')) only\)" | sed -n 's/^| `\([^`]*\)` |.*/\1/p' | sort -u; }
for version in 1.5.6 2.0.0; do
    listed=$(listed "$version")
    upstream "$version"
    patchfile="$fork/$version.patch"
    (cd "$work/upstream-$version" && patch -p1 --batch --forward --dry-run --quiet < "$patchfile") ||
        { echo "check-duckdb-fork: $version.patch does not apply cleanly to DuckDB $version" >&2; exit 1; }
    files=$(sed -n 's|^diff --git [^ ]* [^/]*/\(.*\)$|\1|p' "$patchfile" | sort -u)
    if [ "$files" != "$listed" ]; then
        echo "check-duckdb-fork: the files $version.patch changes differ from FORK.md's table:" >&2
        diff <(echo "$files") <(echo "$listed") >&2 || true
        exit 1
    fi
    echo "check-duckdb-fork: $version.patch applies to DuckDB $version and changes the $(echo "$files" | wc -l) files FORK.md lists"
done
