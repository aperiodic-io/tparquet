# pcodec-java (README.md): JDK 17+ for Maven (the library targets Java 11), cargo for the Rust
# reference oracle, python3 with z3-solver for the proofs. Local equivalents of
# .github/workflows/pcodec-java.yml.
MVN ?= mvn -B
SCALE ?= 1
ORACLE := oracle/target/release/pcodec-oracle

.PHONY: oracle test parity vectors vectors-check proofs mutants mutants-oracle bench check

oracle:
	cargo build --release --manifest-path oracle/Cargo.toml
	oracle-traced/prepare.sh
	cargo build --release --manifest-path oracle-traced/Cargo.toml

# without the oracle: unit tests, round trips, the shared testdata/payloads vectors and the
# recorded golden vectors (the oracle-backed suites skip)
test:
	$(MVN) test -Dpcodec.oracle=/nonexistent

# every suite against the reference, which must be present (SCALE multiplies the random cases)
parity: oracle
	$(MVN) test -Dpcodec.oracle.required=true -Dpcodec.parity.scale=$(SCALE)

# re-records vectors/ from the oracle
vectors: oracle
	$(MVN) -q test-compile
	$(MVN) -q dependency:build-classpath -Dmdep.outputFile=target/test.classpath -Dmdep.includeScope=test
	java -cp "target/test-classes:target/classes:$$(cat target/test.classpath)" \
	  -Dpcodec.oracle=$(ORACLE) io.aperiodic.pcodec.Vectors vectors/pco-1.0.4.bin.gz

# fails if re-recording changes a byte
vectors-check: vectors
	git diff --exit-code -- vectors
	test -z "$$(git status --porcelain -- vectors)"

# Java against the Rust reference on the same numbers (BENCHMARK.md); BENCH_ARGS passes
# -Dbench.n=..., -Dbench.runs=..., -Dbench.only=...
bench: oracle
	$(MVN) -q test-compile
	$(MVN) -q dependency:build-classpath -Dmdep.outputFile=target/test.classpath -Dmdep.includeScope=test
	java -Xms2g -Xmx2g -cp "target/test-classes:target/classes:$$(cat target/test.classpath)" \
	  -Dpcodec.oracle=$(ORACLE) $(BENCH_ARGS) io.aperiodic.pcodec.Bench

# the SMT proofs (VERIFICATION.md)
proofs:
	python3 -u verification/proofs.py

# PIT on the library, judged by the suites that need no oracle (threshold in pom.xml)
mutants:
	$(MVN) test-compile org.pitest:pitest-maven:mutationCoverage

# the survivors of `mutants` judged again with the oracle-backed suites added (hours); the
# survivors of both are in target/pit-reports/mutations.csv
mutants-oracle: oracle
	$(MVN) -q -f verification/pit-only-listed/pom.xml install
	$(MVN) test-compile org.pitest:pitest-maven:mutationCoverage -Dpit.threshold=0
	awk -F, '$$6=="SURVIVED"||$$6=="NO_COVERAGE"{print $$2"|"$$4"|"$$5"|"$$3}' \
	  target/pit-reports/mutations.csv | sort -u > target/pit-survivors.txt
	cp target/pit-reports/mutations.csv target/pit-mutations-oracle-free.csv
	PIT_ONLY_LISTED=$(CURDIR)/target/pit-survivors.txt \
	  $(MVN) -P oracle-mutants test-compile org.pitest:pitest-maven:mutationCoverage

check: parity vectors-check proofs
