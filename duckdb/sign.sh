#!/bin/sh
# Signs packaged tparquet extensions (C API v1 and v2 builds alike) with an RSA 2048 key:
#
#   ./sign.sh private.pem build/tparquet.duckdb_extension build/v2/tparquet.duckdb_extension
#
# DuckDB checks the signature against the public keys of the extension repository it is
# installed from (CREATE EXTENSION REPOSITORY ... USING PUBLIC KEY; see README "Signing"), so
# a signed extension loads without allow_unsigned_extensions. $PACKAGER is the
# package-extension binary (default: ../target/release/package-extension).
set -eu
[ $# -ge 2 ] || { echo "usage: $0 PRIVATE_KEY.pem EXTENSION..." >&2; exit 2; }
key=$1
shift
packager=${PACKAGER:-$(dirname "$0")/../target/release/package-extension}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for ext in "$@"; do
    "$packager" signing-hash "$ext" "$tmp/digest"
    # PKCS#1 v1.5 over the digest as a SHA-256 digest (not hashed again)
    openssl pkeyutl -sign -inkey "$key" -pkeyopt digest:sha256 -in "$tmp/digest" -out "$tmp/signature"
    "$packager" set-signature "$ext" "$tmp/signature"
    echo "signed $ext"
done
