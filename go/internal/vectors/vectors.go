// Package vectors writes the Go-produced conformance vectors and checks a directory of
// vectors (testdata layout: files/<name>.parquet and expected/<name>.jsonl, the canonical
// JSON of internal/canonical) against this reader.
package vectors

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/arrow/memory"
	"github.com/apache/arrow-go/v18/parquet"
	"github.com/apache/arrow-go/v18/parquet/file"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"

	tparquet "github.com/aperiodic-io/tparquet/go"
	"github.com/aperiodic-io/tparquet/go/internal/canonical"
)

// ErrNoVectors means the directory has no files/*.parquet.
var ErrNoVectors = errors.New("vectors: no files/*.parquet")

// Check reads every files/*.parquet under dir and compares its canonical JSON with
// expected/<name>.jsonl. It returns the number of files read and every difference.
func Check(dir string) (int, error) {
	paths, _ := filepath.Glob(filepath.Join(dir, "files", "*.parquet")) // the pattern is valid
	if len(paths) == 0 {
		return 0, fmt.Errorf("%w in %s", ErrNoVectors, dir)
	}
	sort.Strings(paths)
	var errs []error
	for _, p := range paths {
		name := strings.TrimSuffix(filepath.Base(p), ".parquet")
		if err := checkOne(p, filepath.Join(dir, "expected", name+".jsonl")); err != nil {
			errs = append(errs, fmt.Errorf("%s: %w", name, err))
		}
	}
	return len(paths), errors.Join(errs...)
}

func checkOne(parquetPath, jsonlPath string) error {
	want, err := os.ReadFile(jsonlPath)
	if err != nil {
		return err
	}
	got, err := ReadJSONL(parquetPath)
	if err != nil {
		return err
	}
	if ok, diff := canonical.Equal(got, want); !ok {
		return errors.New(diff)
	}
	return nil
}

// ReadJSONL reads a Parquet file with pqarrow and returns its canonical JSON lines.
func ReadJSONL(path string) ([]byte, error) {
	rdr, err := file.OpenParquetFile(path, false)
	if err != nil {
		return nil, err
	}
	defer rdr.Close()
	fr, err := pqarrow.NewFileReader(rdr, pqarrow.ArrowReadProperties{BatchSize: 4096}, memory.DefaultAllocator)
	if err != nil {
		return nil, err
	}
	tbl, err := fr.ReadTable(context.Background())
	if err != nil {
		return nil, err
	}
	defer tbl.Release()
	var sb strings.Builder
	if err := canonical.WriteTable(&sb, tbl); err != nil {
		return nil, err
	}
	return []byte(sb.String()), nil
}

// vector is one Go-produced file.
type vector struct {
	name  string
	recs  func() []arrow.RecordBatch
	props []parquet.WriterProperty
	auto  bool // write with tparquet.FileWriter (Auto) instead of props
	// opts: write with tparquet.FileWriter and these options (props are its writer properties)
	opts []tparquet.Option
}

// Generate writes every Go-produced vector into dir/files and dir/expected and returns
// their names. The output is deterministic.
func Generate(dir string) ([]string, error) {
	for _, sub := range []string{"files", "expected"} {
		if err := os.MkdirAll(filepath.Join(dir, sub), 0o755); err != nil {
			return nil, err
		}
	}
	var names []string
	for _, v := range all() {
		if err := v.write(dir); err != nil {
			return nil, fmt.Errorf("%s: %w", v.name, err)
		}
		names = append(names, v.name)
	}
	return names, nil
}

func (v vector) write(dir string) error {
	recs := v.recs()
	defer func() {
		for _, r := range recs {
			r.Release()
		}
	}()
	var jsonl strings.Builder
	for _, r := range recs {
		if err := canonical.WriteRecord(&jsonl, r); err != nil {
			return err
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "expected", v.name+".jsonl"), []byte(jsonl.String()), 0o644); err != nil {
		return err
	}
	f, err := os.Create(filepath.Join(dir, "files", v.name+".parquet"))
	if err != nil {
		return err
	}
	defer f.Close()
	sc := recs[0].Schema()
	type writer interface {
		Write(arrow.RecordBatch) error
		Close() error
	}
	var w writer
	if v.auto || v.opts != nil {
		w, err = tparquet.NewFileWriter(sc, f, append(v.opts, tparquet.WithWriterProperties(v.props...))...)
	} else {
		var fw *pqarrow.FileWriter
		fw, err = pqarrow.NewFileWriter(sc, f, parquet.NewWriterProperties(v.props...), pqarrow.DefaultWriterProps())
		if err == nil {
			err = fw.AppendKeyValueMetadata(tparquet.FooterKey, tparquet.FooterValue)
		}
		w = fw
	}
	if err != nil {
		return err
	}
	for _, r := range recs {
		if r.NumRows() == 0 {
			continue // an empty file: the footer only
		}
		if err := w.Write(r); err != nil {
			return err
		}
	}
	return w.Close()
}

// pcodecAll returns properties writing the given leaves with PCODEC (type hint, PLAIN, no
// dictionary), plus base.
func pcodecAll(cols map[string]parquet.Type, base ...parquet.WriterProperty) []parquet.WriterProperty {
	props := append([]parquet.WriterProperty{}, base...)
	paths := make([]string, 0, len(cols))
	for p := range cols {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	for _, p := range paths {
		props = append(props, tparquet.ColumnProperties(p, cols[p], -1)...)
	}
	return props
}
