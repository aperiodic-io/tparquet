package tparquet_test

import (
	"bytes"
	"errors"
	"fmt"
	"slices"
	"strings"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

func TestFileWriterAuto(t *testing.T) {
	first, second := autoRecord(20000), autoRecord(5000)
	defer first.Release()
	defer second.Release()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(first.Schema(), &buf)
	if err != nil {
		t.Fatal(err)
	}
	if w.Plan() != nil {
		t.Fatal("the plan is made on the first Write")
	}
	if err := w.Write(first); err != nil {
		t.Fatal(err)
	}
	if got := w.Plan().PcodecColumns(); len(got) != 1 || got[0] != "ts" {
		t.Fatalf("plan %v", got)
	}
	if err := w.Write(second); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	md := fileMetadata(t, buf.Bytes())
	if md.NumRowGroups() != 2 {
		t.Fatalf("%d row groups", md.NumRowGroups())
	}
	codecs := columnCodecs(t, md)
	if codecs["ts"] != "PCODEC" || codecs["cycle"] != "ZSTD" || codecs["name"] != "ZSTD" {
		t.Fatalf("codecs %v", codecs)
	}
	if v, ok := footerValue(md, tparquet.FooterKey); !ok || v != "1" {
		t.Fatalf("footer %q %v", v, ok)
	}
	assertSameData(t, buf.Bytes(), first, second)
}

func TestFileWriterWithoutPcodecHasNoFooterKey(t *testing.T) {
	rec := autoRecord(100)
	defer rec.Release()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(rec.Schema(), &buf, tparquet.WithDefaultPolicy(tparquet.Base))
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Write(rec); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	md := fileMetadata(t, buf.Bytes())
	if _, ok := footerValue(md, tparquet.FooterKey); ok {
		t.Fatal("a file without PCODEC columns should not carry the footer key")
	}
	assertSameData(t, buf.Bytes(), rec)
}

func TestFileWriterCloseWithoutWrite(t *testing.T) {
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(mixedSchema, &buf, tparquet.WithDefaultPolicy(tparquet.Pcodec))
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatalf("a second Close: %v", err)
	}
	tbl, err := readTable(t, buf.Bytes())
	if err != nil {
		t.Fatal(err)
	}
	defer tbl.Release()
	if tbl.NumRows() != 0 || tbl.NumCols() != 6 {
		t.Fatalf("%d rows, %d columns", tbl.NumRows(), tbl.NumCols())
	}
	if v, ok := footerValue(fileMetadata(t, buf.Bytes()), tparquet.FooterKey); !ok || v != "1" {
		t.Fatal("forced PCODEC columns get the footer key even without rows")
	}
	rec := randomRecord(mixedSchema, 2)
	defer rec.Release()
	if err := w.Write(rec); err == nil {
		t.Fatal("Write after Close should fail")
	}
}

func TestFileWriterErrors(t *testing.T) {
	if _, err := tparquet.NewFileWriter(mixedSchema, &bytes.Buffer{}, tparquet.WithPcodecLevel(99)); err == nil {
		t.Fatal("bad options are reported by NewFileWriter")
	}
	w, err := tparquet.NewFileWriter(mixedSchema, &bytes.Buffer{})
	if err != nil {
		t.Fatal(err)
	}
	other := autoRecord(10)
	defer other.Release()
	if err := w.Write(other); err == nil || !strings.Contains(err.Error(), "schema differs") {
		t.Fatalf("err = %v", err)
	}
	if w.Plan() != nil {
		t.Fatal("a failed first Write makes no plan")
	}
}

type failingWriter struct{}

func (failingWriter) Write([]byte) (int, error) { return 0, errors.New("disk full") }

func TestFileWriterSinkErrors(t *testing.T) {
	rec := autoRecord(10)
	defer rec.Release()
	w, err := tparquet.NewFileWriter(rec.Schema(), failingWriter{})
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Write(rec); err == nil {
		t.Fatal("a failing sink should fail the first Write")
	}
	w, err = tparquet.NewFileWriter(rec.Schema(), failingWriter{})
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err == nil {
		t.Fatal("a failing sink should fail Close")
	}
}

func TestFileWriterPassesOptions(t *testing.T) {
	rec := autoRecord(10000)
	defer rec.Release()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(rec.Schema(), &buf,
		tparquet.WithWriterProperties(parquet.WithMaxRowGroupLength(4000)),
		tparquet.WithArrowWriterProperties(pqarrow.NewArrowWriterProperties(pqarrow.WithStoreSchema())))
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Write(rec); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	md := fileMetadata(t, buf.Bytes())
	if md.NumRowGroups() != 3 {
		t.Fatalf("%d row groups, want 3", md.NumRowGroups())
	}
	if _, ok := footerValue(md, "ARROW:schema"); !ok {
		t.Fatal("the arrow writer properties were not used")
	}
}

// TestFileWriterEmptyFirstBatch: an empty first batch has no data to compare codecs on, so
// the trial waits for the first batch with rows; the empty batches still make their (empty)
// row groups, in order.
func TestFileWriterEmptyFirstBatch(t *testing.T) {
	rec := autoRecord(20000)
	defer rec.Release()
	empty := rec.NewSlice(0, 0)
	defer empty.Release()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(rec.Schema(), &buf)
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range []arrow.RecordBatch{empty, empty, rec, empty} {
		if err := w.Write(r); err != nil {
			t.Fatal(err)
		}
		if r == empty && w.Plan() == nil {
			continue
		}
		if got := w.Plan().PcodecColumns(); len(got) != 1 || got[0] != "ts" {
			t.Fatalf("plan %v, want [ts]", got)
		}
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	md := fileMetadata(t, buf.Bytes())
	var rows []int64
	for i := 0; i < md.NumRowGroups(); i++ {
		rows = append(rows, md.RowGroup(i).NumRows())
	}
	if fmt.Sprint(rows) != "[0 0 20000 0]" {
		t.Fatalf("row groups of %v rows", rows)
	}
	if columnCodecs(t, md)["ts"] != "PCODEC" {
		t.Fatal("ts is not PCODEC")
	}
	assertSameData(t, buf.Bytes(), rec)
}

func TestFileWriterOnlyEmptyBatches(t *testing.T) {
	rec := autoRecord(10)
	defer rec.Release()
	empty := rec.NewSlice(0, 0)
	defer empty.Release()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(rec.Schema(), &buf, tparquet.WithPolicy("cycle", tparquet.Pcodec))
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Write(empty); err != nil {
		t.Fatal(err)
	}
	if w.Plan() != nil {
		t.Fatal("an empty batch makes no plan")
	}
	other := randomRecord(mixedSchema, 0)
	defer other.Release()
	if err := w.Write(other); err == nil || !strings.Contains(err.Error(), "schema differs") {
		t.Fatalf("an empty batch of another schema: err = %v", err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	md := fileMetadata(t, buf.Bytes())
	if md.NumRowGroups() != 1 || md.RowGroup(0).NumRows() != 0 {
		t.Fatalf("%d row groups", md.NumRowGroups())
	}
	if got := columnCodecs(t, md); got["cycle"] != "PCODEC" || got["ts"] != "ZSTD" {
		t.Fatalf("codecs %v", got)
	}
}

// shortWriter accepts n bytes, then fails.
type shortWriter struct{ n int }

func (s *shortWriter) Write(p []byte) (int, error) {
	if len(p) > s.n {
		return 0, errors.New("disk full")
	}
	s.n -= len(p)
	return len(p), nil
}

func TestFileWriterPendingBatchSinkError(t *testing.T) {
	rec := autoRecord(100)
	defer rec.Release()
	empty := rec.NewSlice(0, 0)
	defer empty.Release()
	for _, last := range []func(w *tparquet.FileWriter) error{
		func(w *tparquet.FileWriter) error { return w.Write(rec) },
		func(w *tparquet.FileWriter) error { return w.Close() },
	} {
		w, err := tparquet.NewFileWriter(rec.Schema(), &shortWriter{n: 4}, tparquet.WithDefaultPolicy(tparquet.Base))
		if err != nil {
			t.Fatal(err)
		}
		if err := w.Write(empty); err != nil {
			t.Fatal(err)
		}
		if err := last(w); err == nil || !strings.Contains(err.Error(), "disk full") {
			t.Fatalf("err = %v, want the sink's", err)
		}
	}
}

// TestFileWriterSchemaMismatches: the schema is checked on every Write; schema-level
// metadata is not part of it (pqarrow compares fields), field nullability is.
func TestFileWriterSchemaMismatches(t *testing.T) {
	rec := autoRecord(3000)
	defer rec.Release()
	md := arrow.NewMetadata([]string{"k"}, []string{"v"})
	withMeta := array.NewRecordBatch(arrow.NewSchema(rec.Schema().Fields(), &md), rec.Columns(), rec.NumRows())
	defer withMeta.Release()
	fields := slices.Clone(rec.Schema().Fields())
	fields[1].Nullable = true
	nullable := array.NewRecordBatch(arrow.NewSchema(fields, nil), rec.Columns(), rec.NumRows())
	defer nullable.Release()

	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(rec.Schema(), &buf)
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Write(nullable); err == nil || !strings.Contains(err.Error(), "schema differs") {
		t.Fatalf("first Write of another schema: %v", err)
	}
	if err := w.Write(withMeta); err != nil {
		t.Fatalf("schema metadata is not part of the schema check: %v", err)
	}
	if err := w.Write(nullable); err == nil || !strings.Contains(err.Error(), "schema does not match") {
		t.Fatalf("later Write of another schema: %v", err)
	}
	if err := w.Write(rec); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	assertSameData(t, buf.Bytes(), rec, rec)
}

// TestFileWriterEmptyFiles: Close with no Write gives a valid file without row groups for
// every policy; only forced PCODEC columns mark it.
func TestFileWriterEmptyFiles(t *testing.T) {
	for _, tc := range []struct {
		opts   []tparquet.Option
		marked bool
	}{
		{nil, false},
		{[]tparquet.Option{tparquet.WithDefaultPolicy(tparquet.Base)}, false},
		{[]tparquet.Option{tparquet.WithDefaultPolicy(tparquet.Pcodec)}, true},
		{[]tparquet.Option{tparquet.WithPolicy("name", tparquet.Pcodec)}, true},
	} {
		var buf bytes.Buffer
		w, err := tparquet.NewFileWriter(mixedSchema, &buf, tc.opts...)
		if err != nil {
			t.Fatal(err)
		}
		if err := w.Close(); err != nil {
			t.Fatal(err)
		}
		md := fileMetadata(t, buf.Bytes())
		if md.NumRows != 0 || md.NumRowGroups() != 0 {
			t.Fatalf("%d rows, %d row groups", md.NumRows, md.NumRowGroups())
		}
		if _, ok := footerValue(md, tparquet.FooterKey); ok != tc.marked || w.Plan().UsesPcodec() != tc.marked {
			t.Fatalf("footer key %v, plan %v, want %v", ok, w.Plan().PcodecColumns(), tc.marked)
		}
		checkReaders(t, buf.Bytes(), nil)
	}
}
