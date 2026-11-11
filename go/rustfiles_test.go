package tparquet_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/array"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/compress"
	"github.com/apache/arrow-go/v18/parquet/file"

	tparquet "github.com/aperiodic-io/tparquet/go"
)

func testdataDir() string {
	if dir := os.Getenv("TPARQUET_TESTDATA"); dir != "" {
		return dir
	}
	return "../testdata"
}

// fileManifest is testdata/files/manifest.json (testdata/README.md).
type fileManifest struct {
	Files []struct {
		Name          string  `json:"name"`
		File          string  `json:"file"`
		Rows          int64   `json:"rows"`
		RowGroups     int     `json:"row_groups"`
		PcodecVersion *string `json:"pcodec_version"`
		Columns       []struct {
			Path         string         `json:"path"`
			PhysicalType string         `json:"physical_type"`
			Codecs       []string       `json:"codecs"`
			Pages        map[string]int `json:"pages"`
			Modes        map[string]int `json:"modes"`
		} `json:"columns"`
	} `json:"files"`
}

// payloadSpy records the payload mode of every page arrow-go decodes, as tparquet inspect
// names them: EMPTY for a page of expected size 0 (whatever its payload: arrow-go decodes
// it, readers built on arrow-rs do not), else the mode byte.
type payloadSpy struct {
	compress.ErrorCodec
	modes []string
	empty [][]byte // the payloads of the EMPTY pages
}

func (s *payloadSpy) DecodeWithError(dst, src []byte) ([]byte, error) {
	switch {
	case len(dst) == 0:
		s.modes = append(s.modes, "EMPTY")
		s.empty = append(s.empty, slices.Clone(src))
	case len(src) < 2 || int(src[1]) >= 4:
		s.modes = append(s.modes, "INVALID")
	default:
		s.modes = append(s.modes, []string{"RAW", "PCO", "DECIMAL", "ZSTD"}[src[1]])
	}
	return s.ErrorCodec.DecodeWithError(dst, src)
}

// TestRustFilesMatchTheManifest reads the files the Rust writer made (testdata/files) page by
// page with arrow-go's low-level reader and checks what the manifest says about each: rows,
// row groups, the footer value, and per column the physical type, codec, page counts by
// page type and payload modes.
func TestRustFilesMatchTheManifest(t *testing.T) {
	dir := testdataDir()
	raw, err := os.ReadFile(filepath.Join(dir, "files", "manifest.json"))
	if errors.Is(err, os.ErrNotExist) {
		t.Skipf("no manifest in %s", dir)
	}
	if err != nil {
		t.Fatal(err)
	}
	var m fileManifest
	if err := json.Unmarshal(raw, &m); err != nil {
		t.Fatal(err)
	}
	real, _ := compress.GetCodec(tparquet.Compression)
	spy := &payloadSpy{ErrorCodec: real.(compress.ErrorCodec)}
	compress.RegisterCodec(tparquet.Compression, spy)
	t.Cleanup(func() { compress.RegisterCodec(tparquet.Compression, real) })

	for _, f := range m.Files {
		t.Run(f.Name, func(t *testing.T) {
			rdr, err := file.OpenParquetFile(filepath.Join(dir, f.File), false)
			if err != nil {
				t.Fatal(err)
			}
			defer rdr.Close()
			md := rdr.MetaData()
			if md.NumRows != f.Rows || md.NumRowGroups() != f.RowGroups {
				t.Fatalf("%d rows in %d row groups, manifest %d in %d", md.NumRows, md.NumRowGroups(), f.Rows, f.RowGroups)
			}
			v, ok := footerValue(md, tparquet.FooterKey)
			if (f.PcodecVersion == nil) == ok || (ok && v != *f.PcodecVersion) {
				t.Fatalf("footer value %q (%v), manifest %v", v, ok, f.PcodecVersion)
			}
			if len(f.Columns) != md.Schema.NumColumns() {
				t.Fatalf("%d columns, manifest %d", md.Schema.NumColumns(), len(f.Columns))
			}
			for c, col := range f.Columns {
				if got := md.Schema.Column(c).Path(); got != col.Path {
					t.Fatalf("column %d is %s, manifest %s", c, got, col.Path)
				}
				if got := md.Schema.Column(c).PhysicalType().String(); got != col.PhysicalType {
					t.Errorf("%s: %s, manifest %s", col.Path, got, col.PhysicalType)
				}
				pages, modes := map[string]int{}, map[string]int{}
				codecs := map[string]bool{}
				pageLevel := true
				for g := 0; g < md.NumRowGroups(); g++ {
					rg := rdr.RowGroup(g)
					cc, err := rg.MetaData().ColumnChunk(c)
					if err != nil {
						t.Fatal(err)
					}
					name := cc.Compression().String()
					if cc.Compression() == tparquet.Compression {
						name = "PCODEC"
					}
					codecs[name] = true
					if cc.DataPageOffset() == 0 {
						// The Rust writer gives a chunk with no data pages data_page_offset
						// 0, below its dictionary page; arrow-go cannot read such a chunk's
						// pages (GetDictionaryPage panics): the codec is all to check.
						pageLevel = false
						continue
					}
					pr, err := rg.GetColumnPageReader(c)
					if err != nil {
						t.Fatal(err)
					}
					spy.modes = nil
					for pr.Next() {
						switch p := pr.Page().(type) {
						case *file.DictionaryPage:
							pages["DICTIONARY_PAGE"]++
						case *file.DataPageV1:
							pages["DATA_PAGE"]++
						case *file.DataPageV2:
							pages["DATA_PAGE_V2"]++
							if !p.IsCompressed() && cc.Compression() == tparquet.Compression {
								modes["UNCOMPRESSED"]++
							}
						}
					}
					if err := pr.Err(); err != nil {
						t.Fatal(err)
					}
					if cc.Compression() == tparquet.Compression {
						for _, mode := range spy.modes {
							modes[mode]++
						}
					}
				}
				if len(codecs) != len(col.Codecs) || !codecs[col.Codecs[0]] {
					t.Errorf("%s: codecs %v, manifest %v", col.Path, codecs, col.Codecs)
				}
				if !pageLevel {
					continue
				}
				if fmt.Sprint(pages) != fmt.Sprint(col.Pages) {
					t.Errorf("%s: pages %v, manifest %v", col.Path, pages, col.Pages)
				}
				if fmt.Sprint(modes) != fmt.Sprint(col.Modes) {
					t.Errorf("%s: modes %v, manifest %v", col.Path, modes, col.Modes)
				}
			}
		})
	}
}

// TestPagesWithoutValuesHaveAPayload: a V2 page whose values are all null has an empty
// values section, which the Go writer still compresses, to the payload RAW 01 00 (spec
// section 3: a writer must write a valid payload for expected size 0), and reads back.
func TestPagesWithoutValuesHaveAPayload(t *testing.T) {
	sc := arrow.NewSchema([]arrow.Field{{Name: "x", Type: arrow.PrimitiveTypes.Int64, Nullable: true}}, nil)
	b := array.NewInt64Builder(memory.DefaultAllocator)
	defer b.Release()
	for i := 0; i < 3000; i++ {
		if i >= 2000 {
			b.Append(int64(i))
		} else {
			b.AppendNull()
		}
	}
	col := b.NewArray()
	defer col.Release()
	rec := array.NewRecordBatch(sc, []arrow.Array{col}, 3000)
	defer rec.Release()
	var buf bytes.Buffer
	w, err := tparquet.NewFileWriter(sc, &buf, tparquet.WithDefaultPolicy(tparquet.Pcodec),
		tparquet.WithWriterProperties(parquet.WithMaxRowGroupLength(1000)))
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Write(rec); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}

	real, _ := compress.GetCodec(tparquet.Compression)
	spy := &payloadSpy{ErrorCodec: real.(compress.ErrorCodec)}
	compress.RegisterCodec(tparquet.Compression, spy)
	t.Cleanup(func() { compress.RegisterCodec(tparquet.Compression, real) })
	rdr, err := file.NewParquetReader(bytes.NewReader(buf.Bytes()))
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Close()
	// arrow-go ends a page by the size of its values: the row groups of nulls make the pages
	pages := 0
	for g := 0; g < rdr.NumRowGroups(); g++ {
		pr, err := rdr.RowGroup(g).GetColumnPageReader(0)
		if err != nil {
			t.Fatal(err)
		}
		for pr.Next() {
			if _, ok := pr.Page().(*file.DataPageV2); !ok {
				t.Fatalf("a %T page", pr.Page())
			}
			pages++
		}
		if err := pr.Err(); err != nil {
			t.Fatal(err)
		}
	}
	if len(spy.empty) != 2 || pages != 3 {
		t.Fatalf("%d pages, %d of them without values", pages, len(spy.empty))
	}
	for _, p := range spy.empty {
		if !bytes.Equal(p, []byte{1, 0}) {
			t.Fatalf("the payload of a page without values is % x, want 01 00", p)
		}
	}
	assertSameData(t, buf.Bytes(), rec)
}
