package tparquet_test

import (
	"bytes"
	"context"
	"testing"

	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
	"github.com/aperiodic-io/tparquet/go/internal/canonical"
)

const (
	footerKey = "0123456789012345"
	columnKey = "abcdefghijklmnop"
)

// TestAutoWithEncryption: arrow-go marks FileEncryptionProperties as used by the first file
// written with them and wipes their keys when it closes; the Auto trial's scratch files must
// not be that file.
func TestAutoWithEncryption(t *testing.T) {
	rec := autoRecord(20000)
	defer rec.Release()
	for _, footer := range []bool{true, false} {
		opts := []parquet.EncryptOption{parquet.WithEncryptedColumns(parquet.ColumnPathToEncryptionPropsMap{
			"ts": parquet.NewColumnEncryptionProperties("ts", parquet.WithKey(columnKey)),
		})}
		if !footer {
			opts = append(opts, parquet.WithPlaintextFooter())
		}
		enc := parquet.NewFileEncryptionProperties(footerKey, opts...)
		var buf bytes.Buffer
		w, err := tparquet.NewFileWriter(rec.Schema(), &buf, tparquet.WithWriterProperties(parquet.WithEncryptionProperties(enc)))
		if err != nil {
			t.Fatal(err)
		}
		if err := w.Write(rec); err != nil {
			t.Fatal(err)
		}
		if err := w.Close(); err != nil {
			t.Fatal(err)
		}
		if got := w.Plan().PcodecColumns(); len(got) != 1 || got[0] != "ts" {
			t.Fatalf("plan %v", got)
		}
		dec := parquet.NewFileDecryptionProperties(parquet.WithFooterKey(footerKey),
			parquet.WithColumnKeys(parquet.ColumnPathToDecryptionPropsMap{
				"ts": parquet.NewColumnDecryptionProperties("ts", parquet.WithDecryptKey(columnKey)),
			}))
		props := parquet.NewReaderProperties(memory.DefaultAllocator)
		props.FileDecryptProps = dec
		rdr, err := file.NewParquetReader(bytes.NewReader(buf.Bytes()), file.WithReadProps(props))
		if err != nil {
			t.Fatal(err)
		}
		fr, err := pqarrow.NewFileReader(rdr, pqarrow.ArrowReadProperties{}, memory.DefaultAllocator)
		if err != nil {
			t.Fatal(err)
		}
		tbl, err := fr.ReadTable(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		var got bytes.Buffer
		if err := canonical.WriteTable(&got, tbl); err != nil {
			t.Fatal(err)
		}
		tbl.Release()
		rdr.Close()
		if ok, diff := canonical.Equal(got.Bytes(), jsonlOfRecords(t, rec)); !ok {
			t.Fatal(diff)
		}
		if _, err := readTable(t, buf.Bytes()); err == nil {
			t.Fatal("the file reads without the keys: it is not encrypted")
		}
	}
}
