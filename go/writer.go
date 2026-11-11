package tparquet

import (
	"io"

	"github.com/apache/arrow-go/v18/arrow"
	"github.com/apache/arrow-go/v18/parquet/pqarrow"
)

// FileWriter writes Arrow record batches to a Parquet file, choosing each column's codec on
// the first batch with rows (see NewPlan) and keeping it for the whole file. Each Write makes
// at least one row group, as pqarrow.FileWriter.Write does.
type FileWriter struct {
	schema  *arrow.Schema
	sink    io.Writer
	opts    []Option
	plan    *Plan
	fw      *pqarrow.FileWriter
	pending []arrow.RecordBatch // empty batches written before the plan
}

// NewFileWriter returns a writer of sc to w. It checks the options at once; the plan, and
// so the Parquet writer, is made by the first Write of a batch with rows (or by Close).
func NewFileWriter(sc *arrow.Schema, w io.Writer, opts ...Option) (*FileWriter, error) {
	if _, err := NewPlan(sc, nil, opts...); err != nil {
		return nil, err
	}
	return &FileWriter{schema: sc, sink: w, opts: opts}, nil
}

func (w *FileWriter) start(sample arrow.RecordBatch) error {
	plan, err := NewPlan(w.schema, sample, w.opts...)
	if err != nil {
		return err
	}
	fw, err := plan.NewFileWriter(w.sink)
	if err != nil {
		return err
	}
	w.plan, w.fw = plan, fw
	for len(w.pending) > 0 {
		rec := w.pending[0]
		w.pending = w.pending[1:]
		err := fw.Write(rec)
		rec.Release()
		if err != nil {
			return err
		}
	}
	return nil
}

// Write writes rec as one or more row groups. The first call with rows runs the Auto trial
// on rec; empty batches before it are kept, and written first.
func (w *FileWriter) Write(rec arrow.RecordBatch) error {
	if w.fw == nil {
		if rec.NumRows() == 0 {
			if !rec.Schema().Equal(w.schema) {
				return errSampleSchema
			}
			rec.Retain()
			w.pending = append(w.pending, rec)
			return nil
		}
		if err := w.start(rec); err != nil {
			return err
		}
	}
	return w.fw.Write(rec)
}

// Plan returns the codec plan, or nil before the first Write of a batch with rows.
func (w *FileWriter) Plan() *Plan { return w.plan }

// Close writes the footer. It may be called more than once.
func (w *FileWriter) Close() error {
	if w.fw == nil {
		if err := w.start(nil); err != nil {
			return err
		}
	}
	return w.fw.Close()
}
