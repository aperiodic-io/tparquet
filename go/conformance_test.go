package tparquet_test

import (
	"errors"
	"os"
	"testing"

	"github.com/aperiodic-io/tparquet/go/internal/vectors"
)

// TestConformanceVectors reads every testdata/files/*.parquet of the repository (or of
// $TPARQUET_TESTDATA) and compares it with testdata/expected/<name>.jsonl.
func TestConformanceVectors(t *testing.T) {
	dir := os.Getenv("TPARQUET_TESTDATA")
	if dir == "" {
		dir = "../testdata"
	}
	n, err := vectors.Check(dir)
	if errors.Is(err, vectors.ErrNoVectors) {
		t.Skipf("no conformance vectors in %s (testdata/files/*.parquet); set TPARQUET_TESTDATA to check others", dir)
	}
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("%d vectors match", n)
}

// TestPayloadVectors decodes every payload of testdata/payloads/manifest.json (or of
// $TPARQUET_TESTDATA) through the registered codec.
func TestPayloadVectors(t *testing.T) {
	dir := os.Getenv("TPARQUET_TESTDATA")
	if dir == "" {
		dir = "../testdata"
	}
	n, err := vectors.CheckPayloads(dir)
	if errors.Is(err, vectors.ErrNoPayloads) {
		t.Skipf("no payload vectors in %s", dir)
	}
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("%d payloads match", n)
}
