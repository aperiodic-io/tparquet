package tparquet_test

import (
	"bytes"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// footerValues returns every value of key in the footer, in order.
func footerValues(t *testing.T, data []byte, key string) []string {
	t.Helper()
	kv := fileMetadata(t, data).KeyValueMetadata()
	var out []string
	for i, k := range kv.Keys() {
		if k == key {
			out = append(out, kv.Values()[i])
		}
	}
	return out
}

// TestFooterKeyFromSchemaMetadata: pqarrow copies the Arrow schema metadata into the footer;
// a FooterKey there must neither be duplicated nor mark a file without PCODEC columns.
func TestFooterKeyFromSchemaMetadata(t *testing.T) {
	rec := autoRecord(5000)
	defer rec.Release()
	for _, value := range []string{"1", "7"} {
		md := arrow.NewMetadata([]string{tparquet.FooterKey, "x", tparquet.FooterKey}, []string{value, "y", value})
		sc := arrow.NewSchema(rec.Schema().Fields(), &md)
		withMeta := array.NewRecordBatch(sc, rec.Columns(), rec.NumRows())
		for _, store := range []bool{false, true} {
			arrowProps := pqarrow.DefaultWriterProps()
			if store {
				arrowProps = pqarrow.NewArrowWriterProperties(pqarrow.WithStoreSchema())
			}
			for _, pol := range []tparquet.Policy{tparquet.Auto, tparquet.Base} {
				var buf bytes.Buffer
				w, err := tparquet.NewFileWriter(sc, &buf, tparquet.WithDefaultPolicy(pol),
					tparquet.WithArrowWriterProperties(arrowProps))
				if err != nil {
					t.Fatal(err)
				}
				if err := w.Write(withMeta); err != nil {
					t.Fatal(err)
				}
				if err := w.Close(); err != nil {
					t.Fatal(err)
				}
				got := footerValues(t, buf.Bytes(), tparquet.FooterKey)
				want := "[]"
				if w.Plan().UsesPcodec() {
					want = "[1]"
				}
				if pol == tparquet.Auto && !w.Plan().UsesPcodec() {
					t.Fatal("the premise: Auto picks PCODEC for ts")
				}
				if s := fmtStrings(got); s != want {
					t.Errorf("value %q, store %v, policy %d: footer values %s, want %s", value, store, pol, s, want)
				}
				if x := footerValues(t, buf.Bytes(), "x"); len(x) != 1 || x[0] != "y" {
					t.Errorf("the other schema metadata is lost: %v", x)
				}
				assertSameData(t, buf.Bytes(), rec)
			}
		}
		withMeta.Release()
	}
}

func fmtStrings(s []string) string {
	var b bytes.Buffer
	b.WriteByte('[')
	for i, v := range s {
		if i > 0 {
			b.WriteByte(' ')
		}
		b.WriteString(v)
	}
	b.WriteByte(']')
	return b.String()
}
