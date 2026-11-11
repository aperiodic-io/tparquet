package tparquet_test

import (
	"bytes"
	"context"
	"fmt"
	"sync"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// parallel runs f(g) on n goroutines at once and reports every error.
func parallel(t *testing.T, n int, f func(g int) error) {
	t.Helper()
	var wg sync.WaitGroup
	errs := make([]error, n)
	start := make(chan struct{})
	for g := 0; g < n; g++ {
		wg.Add(1)
		go func(g int) {
			defer wg.Done()
			<-start
			errs[g] = f(g)
		}(g)
	}
	close(start)
	wg.Wait()
	for g, err := range errs {
		if err != nil {
			t.Errorf("goroutine %d: %v", g, err)
		}
	}
}

func writeBytes(w interface {
	Write(arrow.RecordBatch) error
	Close() error
}, recs ...arrow.RecordBatch) error {
	for _, r := range recs {
		if err := w.Write(r); err != nil {
			return err
		}
	}
	return w.Close()
}

// TestOnePlanManyWriters: a Plan is read-only once made; many goroutines write files with it
// at once, and every file is the same bytes (writing is deterministic).
func TestOnePlanManyWriters(t *testing.T) {
	rec := typedRecord(everyType, 200, 7)
	defer rec.Release()
	plan := newPlan(t, everyType, rec, tparquet.WithWriterProperties(parquet.WithMaxRowGroupLength(80),
		parquet.WithStoreDecimalAsInteger(true)))
	if !plan.UsesPcodec() {
		t.Fatal("the premise: the plan has PCODEC columns")
	}
	outs := make([][]byte, 8)
	parallel(t, len(outs), func(g int) error {
		var buf bytes.Buffer
		w, err := plan.NewFileWriter(&buf)
		if err != nil {
			return err
		}
		if err := writeBytes(w, rec); err != nil {
			return err
		}
		_ = plan.PcodecColumns()
		_, _ = plan.Codec("i64")
		_ = plan.Properties()
		outs[g] = buf.Bytes()
		return nil
	})
	for g := range outs {
		if !bytes.Equal(outs[g], outs[0]) {
			t.Fatalf("file %d differs from file 0", g)
		}
	}
	ref := writeReference(t, []arrow.RecordBatch{rec}, parquet.WithMaxRowGroupLength(80), parquet.WithStoreDecimalAsInteger(true))
	refTbl, err := readTable(t, ref)
	if err != nil {
		t.Fatal(err)
	}
	defer refTbl.Release()
	assertSameTable(t, outs[0], refTbl)
}

// TestConcurrentAutoTrials: FileWriters run their Auto trials at once and all reach the same
// plan and bytes as one alone.
func TestConcurrentAutoTrials(t *testing.T) {
	recs := []arrow.RecordBatch{autoRecord(6000), v2Penalty(800)}
	defer recs[0].Release()
	defer recs[1].Release()
	for _, rec := range recs {
		alone, plan := writeAll(t, rec.Schema(), []arrow.RecordBatch{rec})
		parallel(t, 6, func(g int) error {
			var buf bytes.Buffer
			w, err := tparquet.NewFileWriter(rec.Schema(), &buf)
			if err != nil {
				return err
			}
			if err := writeBytes(w, rec); err != nil {
				return err
			}
			if fmt.Sprint(w.Plan().PcodecColumns()) != fmt.Sprint(plan.PcodecColumns()) {
				return fmt.Errorf("plan %v, alone %v", w.Plan().PcodecColumns(), plan.PcodecColumns())
			}
			if !bytes.Equal(buf.Bytes(), alone) {
				return fmt.Errorf("the file differs from the one written alone")
			}
			return nil
		})
	}
}

// TestManyReadersOfOneFile: goroutines read one PCODEC file at once, each with its own
// reader, serially and with pqarrow's parallel column reads, whole and by row group.
func TestManyReadersOfOneFile(t *testing.T) {
	rec := typedRecord(everyType, 900, 8)
	defer rec.Release()
	data, _ := writeAll(t, everyType, []arrow.RecordBatch{rec}, tparquet.WithDefaultPolicy(tparquet.Pcodec),
		tparquet.WithWriterProperties(parquet.WithMaxRowGroupLength(300), parquet.WithDataPageSize(2048), parquet.WithStoreDecimalAsInteger(true)))
	tbl, err := readTable(t, data)
	if err != nil {
		t.Fatal(err)
	}
	want := jsonlOfTable(t, tbl)
	tbl.Release()
	parallel(t, 8, func(g int) error {
		rdr, err := file.NewParquetReader(bytes.NewReader(data))
		if err != nil {
			return err
		}
		defer rdr.Close()
		fr, err := pqarrow.NewFileReader(rdr, pqarrow.ArrowReadProperties{Parallel: g%2 == 0, BatchSize: int64(64 << g)}, memory.DefaultAllocator)
		if err != nil {
			return err
		}
		var tbl arrow.Table
		if g%4 < 2 {
			tbl, err = fr.ReadTable(context.Background())
		} else {
			tbl, err = fr.ReadRowGroups(context.Background(), allLeaves(rdr.MetaData().Schema.NumColumns()), []int{0, 1, 2})
		}
		if err != nil {
			return err
		}
		defer tbl.Release()
		if !bytes.Equal(jsonlOfTable(t, tbl), want) {
			return fmt.Errorf("read other data")
		}
		return nil
	})
}
