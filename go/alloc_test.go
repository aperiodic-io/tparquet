package tparquet_test

import (
	"bytes"
	"context"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// TestNoLeaks writes (with the Auto trial and pending empty batches) and reads PCODEC files
// with checked allocators: every buffer is given back.
func TestNoLeaks(t *testing.T) {
	mem := memory.NewCheckedAllocator(memory.NewGoAllocator())
	defer mem.AssertSize(t, 0)
	rec := typedRecord(everyType, 300, 9)
	defer rec.Release()
	empty := rec.NewSlice(0, 0)
	defer empty.Release()
	data, _ := writeAll(t, everyType, []arrow.RecordBatch{empty, rec, empty},
		tparquet.WithWriterProperties(parquet.WithAllocator(mem), parquet.WithStoreDecimalAsInteger(true), parquet.WithMaxRowGroupLength(100)),
		tparquet.WithArrowWriterProperties(pqarrow.NewArrowWriterProperties(pqarrow.WithAllocator(mem))))

	rmem := memory.NewCheckedAllocator(memory.NewGoAllocator())
	defer rmem.AssertSize(t, 0)
	props := parquet.NewReaderProperties(rmem)
	rdr, err := file.NewParquetReader(bytes.NewReader(data), file.WithReadProps(props))
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Close()
	fr, err := pqarrow.NewFileReader(rdr, pqarrow.ArrowReadProperties{Parallel: true}, rmem)
	if err != nil {
		t.Fatal(err)
	}
	tbl, err := fr.ReadTable(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if tbl.NumRows() != 300 {
		t.Fatalf("%d rows", tbl.NumRows())
	}
	tbl.Release()
}
