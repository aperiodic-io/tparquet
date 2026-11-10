# pcodec-java (README.md): JDK 17+ for Maven (the library targets Java 11), cargo for the Rust
# reference oracle, python3 with z3-solver for the proofs. Local equivalents of
# .github/workflows/pcodec-java.yml.
MVN ?= mvn -B
SCALE ?= 1
ORACLE := oracle/target/release/pcodec-oracle

.PHONY: oracle test parity vectors vectors-check proofs mutants check

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

# the SMT proofs (VERIFICATION.md)
proofs:
	python3 -u verification/proofs.py

# PIT on the library (every mutant killed, or excluded with a reason in pom.xml)
mutants:
	$(MVN) test-compile org.pitest:pitest-maven:mutationCoverage

check: parity vectors-check proofs
