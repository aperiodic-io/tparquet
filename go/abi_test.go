package tparquet_test

import (
	"os"
	"strings"
	"testing"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// TestAnABIMismatchIsAReadError: a file with PCODEC columns, read by a binary linked against
// a library of another ABI version, fails with an error that says so.
func TestAnABIMismatchIsAReadError(t *testing.T) {
	data, err := os.ReadFile("../testdata/files/mixed.parquet")
	if err != nil {
		t.Fatal(err)
	}
	if tbl, err := readTable(t, data); err != nil {
		t.Fatal(err)
	} else {
		tbl.Release()
	}
	restore := tparquet.SimulateABIMismatch()
	defer restore()
	tbl, err := readTable(t, data)
	if err == nil {
		tbl.Release()
		t.Fatal("read a PCODEC file despite the ABI mismatch")
	}
	if !strings.Contains(err.Error(), "the linked libtparquet_capi has ABI version 2, this package needs 1") {
		t.Fatalf("err = %v", err)
	}
}
