package tparquet_test

import (
	"bytes"
	"context"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/metadata"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	"github.com/aperiodic-io/tparquet/go/internal/canonical"
)

// readTable reads a whole Parquet file with pqarrow.
func readTable(t testing.TB, data []byte) (arrow.Table, error) {
	t.Helper()
	rdr, err := file.NewParquetReader(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	defer rdr.Close()
	fr, err := pqarrow.NewFileReader(rdr, pqarrow.ArrowReadProperties{BatchSize: 1000}, memory.DefaultAllocator)
	if err != nil {
		return nil, err
	}
	return fr.ReadTable(context.Background())
}

func fileMetadata(t testing.TB, data []byte) *metadata.FileMetaData {
	t.Helper()
	rdr, err := file.NewParquetReader(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Close()
	return rdr.MetaData()
}

func jsonlOfRecords(t testing.TB, recs ...arrow.RecordBatch) []byte {
	t.Helper()
	var buf bytes.Buffer
	for _, r := range recs {
		if err := canonical.WriteRecord(&buf, r); err != nil {
			t.Fatal(err)
		}
	}
	return buf.Bytes()
}

// assertSameData checks that the file holds exactly recs, bit for bit (canonical JSON keeps
// NaN payloads and signed zeros).
func assertSameData(t testing.TB, data []byte, recs ...arrow.RecordBatch) {
	t.Helper()
	tbl, err := readTable(t, data)
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	defer tbl.Release()
	var got bytes.Buffer
	if err := canonical.WriteTable(&got, tbl); err != nil {
		t.Fatal(err)
	}
	sameJSONL(t, "data after the round trip", got.Bytes(), jsonlOfRecords(t, recs...))
}

// assertSameTable checks that the file holds exactly the rows of want.
func assertSameTable(t testing.TB, data []byte, want arrow.Table) {
	t.Helper()
	var w bytes.Buffer
	if err := canonical.WriteTable(&w, want); err != nil {
		t.Fatal(err)
	}
	tbl, err := readTable(t, data)
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	defer tbl.Release()
	var got bytes.Buffer
	if err := canonical.WriteTable(&got, tbl); err != nil {
		t.Fatal(err)
	}
	sameJSONL(t, "data", got.Bytes(), w.Bytes())
}

// columnCodecs returns the codec of every column chunk, by path, checking that all row
// groups agree.
func columnCodecs(t testing.TB, md *metadata.FileMetaData) map[string]string {
	t.Helper()
	out := map[string]string{}
	for i := 0; i < md.NumRowGroups(); i++ {
		rg := md.RowGroup(i)
		for j := 0; j < rg.NumColumns(); j++ {
			cc, err := rg.ColumnChunk(j)
			if err != nil {
				t.Fatal(err)
			}
			c := "PCODEC"
			if cc.Compression() != 20547 {
				c = cc.Compression().String()
			}
			p := cc.PathInSchema().String()
			if prev, ok := out[p]; ok && prev != c {
				t.Fatalf("%s: row groups disagree: %s and %s", p, prev, c)
			}
			out[p] = c
		}
	}
	return out
}

func footerValue(md *metadata.FileMetaData, key string) (string, bool) {
	v := md.KeyValueMetadata().FindValue(key)
	if v == nil {
		return "", false
	}
	return *v, true
}

// writeWithProps writes recs with pqarrow, one row group per record (or more, by the
// properties), storing the Arrow schema so every type reads back as written.
func writeWithProps(t testing.TB, recs []arrow.RecordBatch, props ...parquet.WriterProperty) []byte {
	t.Helper()
	var buf bytes.Buffer
	w, err := pqarrow.NewFileWriter(recs[0].Schema(), &buf, parquet.NewWriterProperties(props...),
		pqarrow.NewArrowWriterProperties(pqarrow.WithStoreSchema()))
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range recs {
		if err := w.Write(r); err != nil {
			t.Fatal(err)
		}
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}
