#!/usr/bin/env bash
# Applies each bench/pocs/*.patch (or the ones named) to a clean copy of the library, checks
# that the outputs are unchanged (the recorded-vector suites), and times it with Bench (Java
# only). Results go to bench/results/<patch>.txt; bench/summarize.py turns them into the table
# of bench/RESULTS.md. The baseline (no patch) is timed before each patch, so that drift on a
# shared machine shows up in the comparison instead of in it.
#   bench/run-pocs.sh [patch...]     BENCH_RUNS=9 BENCH_N=1048576 REPEAT=1 by default
# With REPEAT=k, baseline and patch are timed k times each, alternating, in separate JVMs (the
# variance between JVM launches dominates); summarize.py takes each case's median.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
lib=$(dirname "$here")
root=$(cd "$lib/../.." && pwd)
runs=${BENCH_RUNS:-9}
repeat=${REPEAT:-1}
n=${BENCH_N:-1048576}
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$here/results"
patches=("$@")
if [ ${#patches[@]} -eq 0 ]; then
  patches=("$here"/pocs/*.patch)
fi
cp_lib() {
  mkdir -p "$1"
  cp -r "$lib/pom.xml" "$lib/src" "$lib/vectors" "$1/"
}
time_tree() { # tree output
  (cd "$1" && mvn -B -q -Dtparquet.root="$root" test-compile >/dev/null 2>&1 &&
    mvn -B -q -Dtparquet.root="$root" dependency:build-classpath -Dmdep.outputFile=target/cp \
      -Dmdep.includeScope=test >/dev/null 2>&1 &&
    java -Xms2g -Xmx2g -cp "target/test-classes:target/classes:$(cat target/cp)" \
      -Dbench.java-only=true -Dbench.runs="$runs" -Dbench.n="$n" io.aperiodic.pcodec.Bench) \
    | grep '^| [a-z]' >"$2"
}
cp_lib "$work/base"
for p in "${patches[@]}"; do
  p=$(cd "$(dirname "$p")" && pwd)/$(basename "$p")
  name=$(basename "$p" .patch)
  echo "== $name"
  rm -rf -- "$work/poc"
  cp_lib "$work/poc"
  if ! (cd "$work/poc" && patch -s -p3 <"$p"); then
    rm -f "$here/results/$name".*; echo "does not apply" >"$here/results/$name.txt"
    continue
  fi
  if ! (cd "$work/poc" && mvn -B -q -Dtparquet.root="$root" -Dpcodec.oracle=/nonexistent test \
      -Dtest='VectorsTest,RoundTripTest,PayloadVectorsTest,MathVectorsTest' >/dev/null 2>&1); then
    rm -f "$here/results/$name".*; echo "changes the output (the vector suites fail)" >"$here/results/$name.txt"
    continue
  fi
  (cd "$work/base" && mvn -B -q -Dtparquet.root="$root" test-compile >/dev/null 2>&1)
  rm -f "$here/results/$name".*
  for r in $(seq 1 "$repeat"); do
    time_tree "$work/base" "$here/results/$name.base.$r"
    time_tree "$work/poc" "$here/results/$name.txt.$r"
  done
  cat "$here/results/$name".base.* >"$here/results/$name.base"
  cat "$here/results/$name".txt.* >"$here/results/$name.txt"
  rm -f "$here/results/$name".base.* "$here/results/$name".txt.*
done
