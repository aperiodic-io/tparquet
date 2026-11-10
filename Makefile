# Local equivalents of .github/workflows/rust.yml. `make check` runs them all but the slow
# ones (fuzz-smoke, mutants, fork-mutants, fork-upstream-tests).
OWN := -p tparquet-core -p tparquet-capi -p tparquet-cread -p tparquet-jni -p tparquet
JOBS ?= 2
CARGO := cargo
UPSTREAM := acceptance/rust-upstream/Cargo.toml
FUZZ_TIME ?= 60

.PHONY: check fmt fmt-check clippy test c-test check-fork testdata testdata-check fuzz-smoke \
	mutants fork-mutants fork-upstream-tests python-test python-mutants polars-test polars-mutants acceptance \
	java-test cpp-test trino-test trino-mutants datafusion-test \
	bench-test bench-mutants bench

check: fmt-check clippy test c-test check-fork testdata-check

fmt:
	$(CARGO) fmt $(OWN)
	$(CARGO) fmt --manifest-path $(UPSTREAM)

fmt-check:
	$(CARGO) fmt $(OWN) --check
	$(CARGO) fmt --manifest-path $(UPSTREAM) --check

# --no-deps: the vendored fork keeps upstream's lint configuration
clippy:
	$(CARGO) clippy -j $(JOBS) --no-deps $(OWN) --all-targets -- -D warnings
	$(CARGO) clippy -j $(JOBS) --no-deps --manifest-path $(UPSTREAM) --all-targets -- -D warnings

test:
	$(CARGO) test -j $(JOBS) $(OWN)
	$(CARGO) test -j $(JOBS) -p parquet --lib -- pcodec
	$(CARGO) test -j $(JOBS) -p parquet --test arrow_writer
	$(CARGO) test -j $(JOBS) --manifest-path $(UPSTREAM)

c-test:
	$(MAKE) -C rust/capi c-test
	$(MAKE) -C rust/cread c-test

check-fork:
	scripts/check-fork.sh
	scripts/check-fork-java.sh
	scripts/check-cpp-fork.sh
	scripts/check-duckdb-fork.sh

testdata:
	$(CARGO) run -j $(JOBS) -p tparquet --bin gen-testdata

# regenerates the vectors and fails if any byte (or file) changed
testdata-check: testdata
	git diff --exit-code -- testdata
	test -z "$$(git status --porcelain -- testdata)"

# --target: cargo-fuzz defaults to the platform it was built for, and prebuilt cargo-fuzz
# binaries (as CI installs) are musl builds, for which nightly ships no sanitizer std
fuzz-smoke:
	cd rust/core && host=$$(rustc +nightly -vV | sed -n 's/^host: //p') && \
	for t in $$($(CARGO) +nightly fuzz list); do \
		$(CARGO) +nightly fuzz run --target $$host $$t -- -max_total_time=$(FUZZ_TIME) || exit 1; done

# cargo mutants exits 3 when some mutants only made the tests hang (a timeout). A hang is a
# detected mutant, so exit 3 is accepted, but only when no mutant was missed: cargo-mutants
# also exits 3 when there are both timeouts and missed mutants, so missed.txt is checked.
MUTANTS_OK = || { [ $$? -eq 3 ] && [ ! -s mutants.out/missed.txt ]; }

# python/rust is judged by the Python suite (see python-mutants), which needs Python (the
# DuckDB extension, a C++ fork, has its own run: `make -C duckdb mutation`). rust/jni is judged by
# the Java suite (`make -C java jni-mutants`): without Java its test passes without testing anything
mutants:
	$(CARGO) mutants --workspace -j $(JOBS) --exclude 'python/**' --exclude 'rust/jni/**' $(MUTANTS_OK)

PYTHON_VENV := $(CURDIR)/python/.venv/bin/python

# the Python package: `uv sync` (Python 3.14) and `maturin develop`, then pytest
python-test:
	cd python && uv sync --python 3.14 && uv run --no-sync maturin develop --uv && uv run --no-sync pytest

# mutmut on the Python modules; cargo mutants on the pyo3 crate, with pytest as its test
python-mutants:
	cd python && HYPOTHESIS_PROFILE=mutation TPARQUET_TESTDATA=$(CURDIR)/testdata \
	  uv run --no-sync python scripts/mutmut_check.py
	HYPOTHESIS_PROFILE=mutation TPARQUET_PYTHON=$(PYTHON_VENV) PYO3_PYTHON=$(PYTHON_VENV) \
	  $(CARGO) mutants -p tparquet-python -j $(JOBS) $(MUTANTS_OK)

# the Polars plugin (polars-tparquet/): uv builds tparquet from python/ into its own venv,
# rebuilt every time (uv does not notice changes to a path dependency by itself)
polars-test:
	cd polars-tparquet && uv sync --python 3.14 --reinstall-package tparquet && uv run --no-sync pytest

# the benchmark harness (bench/): its offline tests, then the benchmark itself, which downloads
# the providers' samples (about 1.2 GB, cached in bench/data) and rewrites bench/RESULTS.md
bench-test:
	cd bench && uv sync --python 3.14 && uv run --no-sync pytest

bench-mutants:
	cd bench && uv run --no-sync python scripts/mutmut_check.py

bench:
	cd bench && uv sync --python 3.14 && \
	  uv run --no-sync tpq-bench run --markdown RESULTS.md --json results.json

polars-mutants:
	cd polars-tparquet && HYPOTHESIS_PROFILE=mutation TPARQUET_TESTDATA=$(CURDIR)/testdata \
	  uv run --no-sync python scripts/mutmut_check.py

# the fork's changed functions only (rust/parquet/FORK.md); the fork is rebuilt for every
# mutant, so it is built without optimization here
fork-mutants:
	CARGO_PROFILE_DEV_PACKAGE_PARQUET_OPT_LEVEL=0 $(CARGO) mutants --no-config -p parquet -j $(JOBS) \
	  -f rust/parquet/src/basic.rs -f rust/parquet/src/compression.rs \
	  -f rust/parquet/src/column/writer/mod.rs -f rust/parquet/src/file/writer.rs \
	  --re 'PcodecLevel|PcodecCodec|with_pcodec_version|CodecOptionsBuilder::set_physical_type|CodecOptionsBuilder::build|create_codec|<impl From<CompressionCodec> for Compression>::from|<impl From<Compression> for CompressionCodec>::from|<impl FromStr for Compression>::from_str|write_metadata' \
	  --cargo-arg=--lib -- -- basic:: compression:: column::writer:: file::writer:: \
	  --skip test_byte_stream_split_extended_roundtrip --skip test_int96_interop \
	  --skip test_rewrite_missing_column_index --skip test_rewrite_no_page_indexes \
	  $(MUTANTS_OK)

# the upstream suite of the fork; without parquet-testing/arrow-testing, 340 tests fail
# for want of that data, before and after the fork (see rust/parquet/FORK.md)
fork-upstream-tests:
	$(CARGO) test -j $(JOBS) -p parquet --all-features --no-fail-fast -- --test-threads $(JOBS)

# cross-language acceptance (acceptance/cross/run.sh): Rust, Go, Python and DuckDB each read
# the vectors all four wrote; needs python/.venv with the package and `make -C duckdb build`
acceptance:
	acceptance/cross/run.sh

# the JVM packages (java/README.md): the JNI binding, the parquet-java fork, Spark
java-test:
	$(MAKE) -C java test

# the C++ library and the Arrow C++ fork (cpp/README.md)
cpp-test:
	$(MAKE) -C cpp test

# Trino (trino/README.md): JDK 25 for its reactor, JDK 21 for the java/ modules it uses
trino-test:
	$(MAKE) -C trino test

trino-mutants:
	$(MAKE) -C trino mutants

# DataFusion (datafusion/README.md): its own Cargo workspace, on a fork of parquet 59.3.2
datafusion-test:
	$(MAKE) -C datafusion check
