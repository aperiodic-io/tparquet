package tparquet_test

import (
	"bytes"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// TestLargePages: one 4 MiB PCODEC page of int64s, one of doubles (DECIMAL), and
// multi-megabyte binary values forced to PCODEC (the ZSTD fallback), above arrow-go's 1 MiB
// streaming threshold, which a codec that cannot stream must not reach.
func TestLargePages(t *testing.T) {
	const n = 1 << 19
	sc := arrow.NewSchema([]arrow.Field{
		{Name: "ts", Type: arrow.PrimitiveTypes.Int64},
		{Name: "px", Type: arrow.PrimitiveTypes.Float64},
	}, nil)
	b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
	defer b.Release()
	ts := b.Field(0).(*array.Int64Builder)
	px := b.Field(1).(*array.Float64Builder)
	ts.Reserve(n)
	px.Reserve(n)
	for i := 0; i < n; i++ {
		ts.UnsafeAppend(1_700_000_000_000_000 + int64(i)*1000 + int64(i%7))
		px.UnsafeAppend(float64(i%100000) / 100)
	}
	rec := b.NewRecordBatch()
	defer rec.Release()
	data, plan := writeAll(t, sc, []arrow.RecordBatch{rec}, tparquet.WithDefaultPolicy(tparquet.Pcodec),
		tparquet.WithWriterProperties(parquet.WithDataPageSize(64<<20), parquet.WithMaxRowGroupLength(n), parquet.WithBatchSize(n)))
	if len(plan.PcodecColumns()) != 2 {
		t.Fatal("plan")
	}
	if len(data) > n { // 8 MiB of data
		t.Errorf("%d bytes: PCODEC should shrink this a lot", len(data))
	}
	assertSameData(t, data, rec)

	bsc := arrow.NewSchema([]arrow.Field{{Name: "blob", Type: arrow.BinaryTypes.LargeBinary}}, nil)
	bb := array.NewRecordBuilder(memory.DefaultAllocator, bsc)
	defer bb.Release()
	for i := 0; i < 3; i++ {
		bb.Field(0).(*array.BinaryBuilder).Append(bytes.Repeat([]byte{byte(i), 1, 2, 3, 4, 5, 6, 7, 8}, 400_000+i))
	}
	blobs := bb.NewRecordBatch()
	defer blobs.Release()
	data, plan = writeAll(t, bsc, []arrow.RecordBatch{blobs}, tparquet.WithPolicy("blob", tparquet.Pcodec))
	if c, _ := plan.Codec("blob"); c != tparquet.Compression {
		t.Fatal("blob is forced to PCODEC")
	}
	if len(data) > 100_000 {
		t.Errorf("%d bytes: the ZSTD fallback should compress repeats", len(data))
	}
	assertSameData(t, data, blobs)
}
