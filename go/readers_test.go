package tparquet_test

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"
	"github.com/apache/arrow-go/v18/parquet/schema"

	"github.com/aperiodic-io/tparquet/go/internal/canonical"
)

// jsonlOfTable is the canonical JSON of tbl.
func jsonlOfTable(t testing.TB, tbl arrow.Table) []byte {
	t.Helper()
	var buf bytes.Buffer
	if err := canonical.WriteTable(&buf, tbl); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func jsonlOfReader(t testing.TB, rr pqarrow.RecordReader) []byte {
	t.Helper()
	var buf bytes.Buffer
	for rr.Next() {
		if err := canonical.WriteRecord(&buf, rr.RecordBatch()); err != nil {
			t.Fatal(err)
		}
	}
	if err := rr.Err(); err != nil && err.Error() != "EOF" {
		t.Fatal(err)
	}
	return buf.Bytes()
}

// sameJSONL compares two canonical JSON documents: byte for byte first (the canonical
// spelling is unique, and Equal is slow), value by value for the difference.
func sameJSONL(t testing.TB, what string, got, want []byte) {
	t.Helper()
	if bytes.Equal(got, want) {
		return
	}
	if ok, diff := canonical.Equal(got, want); !ok {
		t.Fatalf("%s: %s", what, diff)
	}
}

// projector parses a canonical JSON document once and returns, for a field name, the
// document with only that field on every line.
func projector(t testing.TB, doc []byte) func(name string) []byte {
	t.Helper()
	var rows []map[string]json.RawMessage
	for _, line := range bytes.Split(doc, []byte("\n")) {
		if len(bytes.TrimSpace(line)) == 0 {
			continue
		}
		var row map[string]json.RawMessage
		if err := json.Unmarshal(line, &row); err != nil {
			t.Fatal(err)
		}
		rows = append(rows, row)
	}
	return func(name string) []byte {
		var out bytes.Buffer
		key, _ := json.Marshal(name)
		for _, row := range rows {
			fmt.Fprintf(&out, "{%s:%s}\n", key, row[name])
		}
		return out.Bytes()
	}
}

// rowGroupLines splits the canonical JSON of a file into the lines of each row group.
func rowGroupLines(doc []byte, md interface {
	NumRowGroups() int
}, rows func(int) int64) [][]byte {
	lines := bytes.SplitAfter(doc, []byte("\n"))
	var out [][]byte
	at := 0
	for g := 0; g < md.NumRowGroups(); g++ {
		n := int(rows(g))
		out = append(out, bytes.Join(lines[at:at+n], nil))
		at += n
	}
	return out
}

// checkReaders reads data in every way arrow-go offers and compares each with want, the
// canonical JSON of the whole file: pqarrow whole-table reads (serial and parallel, small
// and large batches), one row group at a time, one top-level column at a time, record
// readers, and the low-level typed column readers of file.Reader.
func checkReaders(t *testing.T, data, want []byte) {
	t.Helper()
	rdr, err := file.NewParquetReader(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	defer rdr.Close()
	ctx := context.Background()
	project := projector(t, want)
	for i, props := range []pqarrow.ArrowReadProperties{{BatchSize: 100}, {BatchSize: 1 << 16, Parallel: true}} {
		fr, err := pqarrow.NewFileReader(rdr, props, memory.DefaultAllocator)
		if err != nil {
			t.Fatal(err)
		}
		tbl, err := fr.ReadTable(ctx)
		if err != nil {
			t.Fatal(err)
		}
		sameJSONL(t, fmt.Sprintf("ReadTable %+v", props), jsonlOfTable(t, tbl), want)
		tbl.Release()

		md := rdr.MetaData()
		groups := rowGroupLines(want, md, func(g int) int64 { return md.RowGroup(g).NumRows() })
		for g := range groups {
			tbl, err := fr.ReadRowGroups(ctx, allLeaves(md.Schema.NumColumns()), []int{g})
			if err != nil {
				t.Fatal(err)
			}
			sameJSONL(t, fmt.Sprintf("row group %d", g), jsonlOfTable(t, tbl), groups[g])
			tbl.Release()
		}
		// Each top-level field alone, from its leaves (serially only: it is the slow part).
		sc := md.Schema
		for f := 0; f < sc.Root().NumFields() && i == 0; f++ {
			field := sc.Root().Field(f)
			var leaves []int
			for c := 0; c < sc.NumColumns(); c++ {
				if sc.ColumnRoot(c) == field {
					leaves = append(leaves, c)
				}
			}
			tbl, err := fr.ReadRowGroups(ctx, leaves, allLeaves(md.NumRowGroups()))
			if err != nil {
				t.Fatal(err)
			}
			sameJSONL(t, "column "+field.Name(), jsonlOfTable(t, tbl), project(field.Name()))
			tbl.Release()
		}
		// A record reader over the row groups in reverse order.
		var rev []int
		var revWant []byte
		for g := len(groups) - 1; g >= 0; g-- {
			rev = append(rev, g)
			revWant = append(revWant, groups[g]...)
		}
		if len(rev) > 0 {
			rr, err := fr.GetRecordReader(ctx, allLeaves(md.Schema.NumColumns()), rev)
			if err != nil {
				t.Fatal(err)
			}
			sameJSONL(t, "record reader, row groups reversed", jsonlOfReader(t, rr), revWant)
			rr.Release()
		}
	}
	checkLowLevel(t, rdr, project)
}

// allLeaves is 0..n-1: pqarrow's ReadRowGroups reads no column (or no row group) for nil.
func allLeaves(n int) []int {
	out := make([]int, n)
	for i := range out {
		out[i] = i
	}
	return out
}

// checkLowLevel reads every leaf of every row group with the typed column readers and checks
// the level and value counts, and the values of flat numeric top-level columns.
func checkLowLevel(t *testing.T, rdr *file.Reader, project func(string) []byte) {
	t.Helper()
	md := rdr.MetaData()
	sc := md.Schema
	for c := 0; c < sc.NumColumns(); c++ {
		col := sc.Column(c)
		// The values of flat numeric columns are compared, but for unsigned integers, which
		// the physical type holds as signed.
		flat := col.MaxRepetitionLevel() == 0 && sc.ColumnRoot(c).Type() == schema.Primitive && !unsigned(col)
		var got []string // canonical JSON values of a flat column
		for g := 0; g < md.NumRowGroups(); g++ {
			cc, err := md.RowGroup(g).ColumnChunk(c)
			if err != nil {
				t.Fatal(err)
			}
			if cc.NumValues() == 0 {
				continue // see TestRustFilesMatchTheManifest: chunks without pages
			}
			cr, err := rdr.RowGroup(g).Column(c)
			if err != nil {
				t.Fatal(err)
			}
			n := int(cc.NumValues())
			def := make([]int16, n)
			rep := make([]int16, n)
			var levels, values int
			var vals []string
			switch r := cr.(type) {
			case *file.Int32ColumnChunkReader:
				buf := make([]int32, n)
				levels, values = readAll(t, func(off int) (int64, int, error) {
					return r.ReadBatch(int64(n-off), buf[values:], def[off:], rep[off:])
				}, &values)
				for _, v := range buf[:values] {
					vals = append(vals, strconv.FormatInt(int64(v), 10))
				}
			case *file.Int64ColumnChunkReader:
				buf := make([]int64, n)
				levels, values = readAll(t, func(off int) (int64, int, error) {
					return r.ReadBatch(int64(n-off), buf[values:], def[off:], rep[off:])
				}, &values)
				for _, v := range buf[:values] {
					vals = append(vals, strconv.FormatInt(v, 10))
				}
			case *file.Float32ColumnChunkReader:
				buf := make([]float32, n)
				levels, values = readAll(t, func(off int) (int64, int, error) {
					return r.ReadBatch(int64(n-off), buf[values:], def[off:], rep[off:])
				}, &values)
				for _, v := range buf[:values] {
					vals = append(vals, fmt.Sprintf(`"0x%08x"`, math.Float32bits(v)))
				}
			case *file.Float64ColumnChunkReader:
				buf := make([]float64, n)
				levels, values = readAll(t, func(off int) (int64, int, error) {
					return r.ReadBatch(int64(n-off), buf[values:], def[off:], rep[off:])
				}, &values)
				for _, v := range buf[:values] {
					vals = append(vals, fmt.Sprintf(`"0x%016x"`, math.Float64bits(v)))
				}
			default:
				skipAll(t, cr, n)
				flat = false
				continue
			}
			if levels != n {
				t.Fatalf("%s, row group %d: %d levels, metadata says %d values", col.Path(), g, levels, n)
			}
			nonNull := 0
			for _, d := range def[:levels] {
				if d == col.MaxDefinitionLevel() {
					nonNull++
				}
			}
			if values != nonNull {
				t.Fatalf("%s, row group %d: %d values, %d defined levels", col.Path(), g, values, nonNull)
			}
			k := 0
			for _, d := range def[:levels] {
				if d == col.MaxDefinitionLevel() {
					got = append(got, vals[k])
					k++
				} else {
					got = append(got, "null")
				}
			}
		}
		if !flat || md.NumRows == 0 {
			continue
		}
		var doc bytes.Buffer
		for _, v := range got {
			fmt.Fprintf(&doc, "{%q:%s}\n", col.Name(), v)
		}
		sameJSONL(t, "low-level "+col.Path(), doc.Bytes(), project(col.Name()))
	}
}

func unsigned(col *schema.Column) bool {
	it, ok := col.LogicalType().(interface{ IsSigned() bool })
	return ok && !it.IsSigned()
}

// readAll calls read until it returns no more levels; *values counts the values read so far.
func readAll(t *testing.T, read func(off int) (int64, int, error), values *int) (int, int) {
	t.Helper()
	levels := 0
	for {
		total, v, err := read(levels)
		if err != nil {
			t.Fatal(err)
		}
		if total == 0 {
			return levels, *values
		}
		levels += int(total)
		*values += v
	}
}

// skipAll skips every value of a column of another type, which decodes its pages.
func skipAll(t *testing.T, cr file.ColumnChunkReader, n int) {
	t.Helper()
	skipper, ok := cr.(interface{ Skip(int64) (int64, error) })
	if !ok {
		t.Fatalf("%T cannot skip", cr)
	}
	if _, err := skipper.Skip(int64(n)); err != nil {
		t.Fatal(err)
	}
	if cr.HasNext() {
		t.Fatal("values left after skipping all of them")
	}
}

// TestReadersOnRustFiles runs checkReaders on every conformance file.
func TestReadersOnRustFiles(t *testing.T) {
	dir := testdataDir()
	paths, _ := filepath.Glob(filepath.Join(dir, "files", "*.parquet"))
	if len(paths) == 0 {
		t.Skipf("no conformance files in %s", dir)
	}
	for _, p := range paths {
		name := strings.TrimSuffix(filepath.Base(p), ".parquet")
		t.Run(name, func(t *testing.T) {
			data, err := os.ReadFile(p)
			if err != nil {
				t.Fatal(err)
			}
			want, err := os.ReadFile(filepath.Join(dir, "expected", name+".jsonl"))
			if err != nil {
				t.Fatal(err)
			}
			checkReaders(t, data, want)
		})
	}
}
