# The tparquet DuckDB extension: DuckDB's own parquet extension with the PCODEC codec, a patch
# over each DuckDB release (FORK.md).
#
#   make build       build/vVERSION/tparquet.duckdb_extension for each of VERSIONS
#   make test        the tests (tests/) of each build, with that version's Python duckdb
#   make thorough    the tests with many more generated tables and damaged files
#   make mutation    the scripted mutants of the patch (mutation/), for MUTATION_VERSION
#   make sign KEY=private.pem   sign the builds (sign.py)
#   make check-fork  the patches apply to the verified releases (scripts/check-duckdb-fork.sh)
#
# The tests need the Python environment (`uv sync` here) and the tparquet CLI, which `test`
# builds.

VERSIONS ?= 1.5.6 2.0.0
MUTATION_VERSION ?= 2.0.0
UV ?= uv
JOBS ?= $(shell nproc)
ROOT := $(abspath ..)
CLI := $(ROOT)/target/release/tparquet
PYTEST = TPARQUET_CLI=$(CLI) TPARQUET_DUCKDB_EXTENSION=$(abspath build/v$(1)/tparquet.duckdb_extension) \
	$(UV) run --locked --with duckdb==$(1) python -m pytest -q -p no:cacheprovider tests

.PHONY: build test thorough mutation sign check-fork cli clean

build:
	@for v in $(VERSIONS); do JOBS=$(JOBS) ./build.sh $$v || exit 1; done

cli:
	cargo build --release -q --manifest-path $(ROOT)/Cargo.toml -p tparquet --bin tparquet

test: build cli
	$(foreach v,$(VERSIONS),$(call PYTEST,$(v)) &&) true

thorough: build cli
	$(foreach v,$(VERSIONS),HYPOTHESIS_PROFILE=thorough TPARQUET_DUCKDB_FUZZ_CASES=5000 $(call PYTEST,$(v)) &&) true

mutation: cli
	JOBS=$(JOBS) ./build.sh $(MUTATION_VERSION)
	TPARQUET_CLI=$(CLI) python3 mutation/run.py --version $(MUTATION_VERSION) --jobs $(JOBS) -- \
		$(UV) run --locked --with duckdb==$(MUTATION_VERSION) python

sign:
	@test -n "$(KEY)" || { echo "usage: make sign KEY=private.pem"; exit 2; }
	python3 sign.py $(KEY) $(foreach v,$(VERSIONS),build/v$(v)/tparquet.duckdb_extension)

check-fork:
	$(ROOT)/scripts/check-duckdb-fork.sh

clean:
	rm -rf build
