package tparquet_test

import (
	"bytes"
	"context"
	"fmt"
	"math/rand/v2"
	"strings"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

// modeFile writes one required 8-byte column whose pages use the given payload mode, with
// a page index, and returns the file and the payload of its first page as [start, end).
func modeFile(t testing.TB, mode byte) ([]byte, arrow.RecordBatch, int, int) {
	data, rec, start, end, _ := modeFileSize(t, mode)
	return data, rec, start, end
}

// modeFileSize is modeFile, and also returns the decoded size of the first page's payload.
func modeFileSize(t testing.TB, mode byte) ([]byte, arrow.RecordBatch, int, int, int) {
	t.Helper()
	typ, pt := arrow.DataType(arrow.PrimitiveTypes.Int64), parquet.Types.Int64
	if mode == 2 {
		typ, pt = arrow.PrimitiveTypes.Float64, parquet.Types.Double
	}
	sc := arrow.NewSchema([]arrow.Field{{Name: "x", Type: typ}}, nil)
	b := array.NewRecordBuilder(memory.DefaultAllocator, sc)
	defer b.Release()
	r := rand.New(rand.NewPCG(uint64(mode), 0))
	for i := 0; i < 3000; i++ {
		switch mode {
		case 0: // random bits: nothing compresses
			b.Field(0).(*array.Int64Builder).Append(int64(r.Uint64()))
		case 1:
			b.Field(0).(*array.Int64Builder).Append(1_000_000 + int64(i)*7 + int64(r.IntN(3)))
		case 2:
			b.Field(0).(*array.Float64Builder).Append(float64(r.IntN(100000)) / 100)
		case 3: // repeats, no type hint: the ZSTD fallback
			b.Field(0).(*array.Int64Builder).Append(int64(i % 3))
		}
	}
	rec := b.NewRecordBatch()
	props := []parquet.WriterProperty{parquet.WithPageIndexEnabled(true), parquet.WithDataPageVersion(parquet.DataPageV2),
		parquet.WithCompressionFor("x", tparquet.Compression), parquet.WithDictionaryDefault(false),
		parquet.WithCompressionLevelFor("x", tparquet.Level(pt, -1))}
	if mode == 3 {
		props = append(props, parquet.WithCompressionLevelFor("x", compress.DefaultCompressionLevel))
	}
	data := writeWithProps(t, []arrow.RecordBatch{rec}, props...)
	rdr, err := file.NewParquetReader(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Close()
	rgi, err := rdr.GetPageIndexReader().RowGroup(0)
	if err != nil {
		t.Fatal(err)
	}
	oi, err := rgi.GetOffsetIndex(0)
	if err != nil {
		t.Fatal(err)
	}
	locs := oi.GetPageLocations()
	start, end := int(locs[0].Offset), int(locs[0].Offset)+int(locs[0].CompressedPageSize)
	rows := 3000
	if len(locs) > 1 {
		rows = int(locs[1].FirstRowIndex)
	}
	// The payload is the end of the page: find where it starts by decoding.
	c, _ := compress.GetCodec(tparquet.Compression)
	for i := start; i < end-1; i++ {
		if data[i] != 1 || data[i+1] != mode {
			continue
		}
		if _, err := c.(compress.ErrorCodec).DecodeWithError(make([]byte, rows*8), data[i:end]); err == nil {
			return data, rec, i, end, rows * 8
		}
	}
	t.Fatalf("no mode %d payload in the first page", mode)
	return nil, nil, 0, 0, 0
}

// readErr reads a whole file with pqarrow and with the low-level page reader, and returns
// both errors; a panic fails the test.
func readErr(t testing.TB, data []byte) (error, error) {
	t.Helper()
	defer func() {
		if r := recover(); r != nil {
			t.Fatalf("reading a corrupt file panicked: %v", r)
		}
	}()
	tbl, err := readTable(t, data)
	if err == nil {
		tbl.Release()
	}
	rdr, lerr := file.NewParquetReader(bytes.NewReader(data))
	if lerr != nil {
		return err, lerr
	}
	defer rdr.Close()
	for g := 0; g < rdr.NumRowGroups() && lerr == nil; g++ {
		for c := 0; c < rdr.MetaData().Schema.NumColumns() && lerr == nil; c++ {
			pr, perr := rdr.RowGroup(g).GetColumnPageReader(c)
			if perr != nil {
				return err, perr
			}
			for pr.Next() {
			}
			lerr = pr.Err()
		}
	}
	return err, lerr
}

// TestCorruptPayloadsInFiles corrupts the header of a real page payload of each mode: every
// reader fails with the codec's error, wrapped by arrow-go, and never panics.
func TestCorruptPayloadsInFiles(t *testing.T) {
	type edit struct {
		at  int
		to  byte
		msg string
	}
	cases := map[byte][]edit{
		0: {{0, 2, "unsupported version 2"}, {1, 4, "unknown mode 4"}, {1, 3, "pcodec payload: "}},
		1: {{0, 0, "unsupported version 0"}, {1, 255, "unknown mode 255"}, {2, 3, "unknown dtype 3"}, {2, 7, "unknown dtype 7"},
			{1, 0, "pcodec payload: decodes to"}, {2, 1, "pcodec payload: "}},
		2: {{2, 2, "unknown dtype 2"}, {3, 19, "exponent 19 above 18"}, {3, 255, "exponent 255 above 18"}, {1, 1, "pcodec payload: "}},
		3: {{1, 77, "unknown mode 77"}, {2, 0, "pcodec payload: zstd"}, {1, 0, "pcodec payload: decodes to"}},
	}
	for mode, edits := range cases {
		data, rec, start, end := modeFile(t, mode)
		assertSameData(t, data, rec)
		rec.Release()
		for _, e := range edits {
			t.Run(fmt.Sprintf("mode %d byte %d = %d", mode, e.at, e.to), func(t *testing.T) {
				bad := bytes.Clone(data)
				bad[start+e.at] = e.to
				for i, err := range []error{first(readErr(t, bad)), second(readErr(t, bad))} {
					if err == nil || !strings.Contains(err.Error(), e.msg) || !strings.Contains(err.Error(), "parquet: could not decompress") {
						t.Errorf("reader %d: err = %v, want the codec's %q", i, err, e.msg)
					}
				}
			})
		}
		// The body zeroed: an error or other values, never a panic (pco has no checksum).
		bad := bytes.Clone(data)
		clear(bad[start+4 : end])
		readErr(t, bad)
	}
}

func first(a, _ error) error  { return a }
func second(_, b error) error { return b }

// TestTruncatedFiles: every prefix of a PCODEC file is a read error, not a panic.
func TestTruncatedFiles(t *testing.T) {
	data, rec := corruptFile(t)
	defer rec.Release()
	for n := 0; n < len(data); n += 1 + len(data)/150 {
		if a, b := readErr(t, data[:n]); a == nil || b == nil {
			t.Fatalf("a file cut at %d of %d bytes read: %v %v", n, len(data), a, b)
		}
	}
}

// TestDecodeIntoAWrongSizeIsAnError: whatever the page size arrow-go is told, the codec
// checks it; a payload is never decoded into fewer or more bytes than it holds.
func TestDecodeIntoAWrongSizeIsAnError(t *testing.T) {
	c := codec(t).(compress.ErrorCodec)
	for mode := byte(0); mode < 4; mode++ {
		data, rec, start, end, size := modeFileSize(t, mode)
		rec.Release()
		payload := data[start:end]
		for _, d := range []int{-size, -8, -1, 1, 8, 1 << 10} {
			dst := make([]byte, size+d)
			if _, err := c.DecodeWithError(dst, payload); err == nil || !strings.Contains(err.Error(), "pcodec payload: ") {
				t.Errorf("mode %d: %d bytes into %d: err = %v", mode, size, size+d, err)
			}
		}
	}
}

// TestCorruptFilesInParallel reads corrupt and good files at once: each goroutine gets its
// own error (the library keeps its last error per OS thread).
func TestCorruptFilesInParallel(t *testing.T) {
	data, rec, start, _ := modeFile(t, 1)
	defer rec.Release()
	bad := bytes.Clone(data)
	bad[start+2] = 3
	done := make(chan error)
	for g := 0; g < 8; g++ {
		go func(g int) {
			var err error
			for i := 0; i < 10 && err == nil; i++ {
				rdr, ferr := file.NewParquetReader(bytes.NewReader(map[bool][]byte{true: bad, false: data}[g%2 == 0]))
				if ferr != nil {
					err = ferr
					break
				}
				fr, _ := pqarrow.NewFileReader(rdr, pqarrow.ArrowReadProperties{Parallel: true}, memory.DefaultAllocator)
				tbl, rerr := fr.ReadTable(context.Background())
				switch {
				case g%2 == 0 && (rerr == nil || !strings.Contains(rerr.Error(), "unknown dtype 3")):
					err = fmt.Errorf("corrupt file: %v", rerr)
				case g%2 == 1 && rerr != nil:
					err = fmt.Errorf("good file: %v", rerr)
				}
				if rerr == nil {
					tbl.Release()
				}
				rdr.Close()
			}
			done <- err
		}(g)
	}
	for g := 0; g < 8; g++ {
		if err := <-done; err != nil {
			t.Error(err)
		}
	}
}
