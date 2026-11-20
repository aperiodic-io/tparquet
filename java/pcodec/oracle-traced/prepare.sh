#!/usr/bin/env bash
# Builds target/pco-1.0.4-traced: pco 1.0.4 exactly as published, plus one statement that
# records the dictionary Dict mode chose (its order among equal counts comes from a HashMap, so
# it differs between processes) in a thread-local, so that the parity tests can give the Java
# encoder the same order. Nothing else changes; the diff is checked below.
set -euo pipefail
cd "$(dirname "$0")"
src=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/pco-1.0.4 2>/dev/null | head -1)
if [ -z "$src" ]; then
  cargo fetch --manifest-path ../oracle/Cargo.toml >/dev/null
  src=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/pco-1.0.4 | head -1)
fi
dst=target/pco-1.0.4-traced
rm -rf "$dst"
mkdir -p target
cp -r "$src" "$dst"
chmod -R u+w "$dst"
python3 - "$dst" <<'PY'
import sys
d = sys.argv[1]
p = d + "/src/mode/dict.rs"
s = open(p).read()
anchor = "    .collect::<Vec<_>>();\n\n  // Here we reuse the hashmap"
assert s.count(anchor) == 1
s = s.replace(anchor, "    .collect::<Vec<_>>();\n  crate::TRACE_DICT.with(|t| *t.borrow_mut() = dict.iter().map(|x| crate::data_types::latent_priv::LatentPriv::to_u64(*x)).collect());\n\n  // Here we reuse the hashmap")
open(p, "w").write(s)
p = d + "/src/lib.rs"
s = open(p).read()
s += "\nthread_local! {\n  /// the dictionary Dict mode chose last on this thread (tracing only)\n  pub static TRACE_DICT: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };\n}\n"
open(p, "w").write(s)
PY
# the only differences from the published crate are the two additions
n=$(diff -r "$src/src" "$dst/src" | grep -c '^[<>]' || true)
[ "$n" = "6" ] || { echo "unexpected diff ($n lines)"; diff -r "$src/src" "$dst/src"; exit 1; }
