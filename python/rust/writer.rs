//! Writing: the streaming [`Writer`] and [`transcode`].

use crate::errors::{Pending, detached, from_pcodec, invalid, locked};
use crate::ffi::{import_batch, import_schema, import_stream};
use crate::file::File;
use crate::options::{Options, base_codec, codec_map, codec_name};
use crate::sink::Sink;
use arrow_array::{RecordBatch, RecordBatchOptions};
use arrow_schema::{DataType, SchemaRef};
use pyo3::prelude::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tparquet::Target;
use tparquet::parquet::file::metadata::ParquetMetaData;

type Codecs = BTreeMap<String, String>;

enum State {
    Open(Box<tparquet::Writer<Sink>>),
    /// Closed (with the codecs, if they were chosen) or aborted.
    Closed(Option<Codecs>),
}

/// Writes record batches to one Parquet file.
#[pyclass(frozen, module = "tparquet._lib")]
pub struct Writer {
    schema: SchemaRef,
    state: Mutex<State>,
    pending: Pending,
    /// The file the writer created, removed when the writer is aborted or fails to finish.
    path: Option<PathBuf>,
}

/// Removes the file a failed write created, if any (it holds no footer).
fn discard(path: Option<&PathBuf>) {
    if let Some(path) = path {
        // best effort: the file may be gone already, and the error that got here matters more
        let _ = std::fs::remove_file(path);
    }
}

/// The dotted path of a union in `fields` (under `prefix`), if any.
fn union_in(fields: &arrow_schema::Fields, prefix: &str) -> Option<String> {
    fields.iter().find_map(|f| {
        let path = if prefix.is_empty() { f.name().clone() } else { format!("{prefix}.{}", f.name()) };
        match f.data_type() {
            DataType::Union(..) => Some(path),
            DataType::Struct(children) => union_in(children, &path),
            DataType::List(c)
            | DataType::LargeList(c)
            | DataType::ListView(c)
            | DataType::LargeListView(c)
            | DataType::FixedSizeList(c, _)
            | DataType::Map(c, _) => union_in(&arrow_schema::Fields::from(vec![c.clone()]), &path),
            _ => None,
        }
    })
}

/// Refuses a schema with a union, which the parquet crate does not write (it panics with
/// "not implemented").
fn refuse_unions(schema: &arrow_schema::Schema) -> PyResult<()> {
    match union_in(schema.fields(), "") {
        Some(path) => Err(invalid(format!("the column {path} is a union, which Parquet cannot store"))),
        None => Ok(()),
    }
}

impl Writer {
    fn create(py: Python<'_>, sink: &Bound<'_, PyAny>, schema: SchemaRef, options: &Options) -> PyResult<Self> {
        let options = options.writer_options()?;
        refuse_unions(&schema)?;
        // what the writer refuses (column names, the schema) is refused before the file is
        // created, or an existing one truncated
        let (s, o) = (schema.clone(), options.clone());
        detached(py, move || tparquet::Writer::try_new(std::io::sink(), s, o).map(drop).map_err(from_pcodec))?;
        let pending = Pending::default();
        let sink = Sink::new(sink, &pending)?;
        let path = sink.path();
        let s = schema.clone();
        let w =
            detached(py, move || tparquet::Writer::try_new(sink, s, options).map_err(from_pcodec)).map_err(|e| {
                discard(path.as_ref());
                pending.or(e)
            })?;
        Ok(Self { schema, state: Mutex::new(State::Open(Box::new(w))), pending, path })
    }

    /// Runs `f` on the open writer with the GIL released; an exception raised by a Python
    /// sink is re-raised as it was.
    fn with_open<T: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&mut tparquet::Writer<Sink>) -> PyResult<T> + Send,
    ) -> PyResult<T> {
        locked(py, &self.state, |state| match state {
            State::Open(w) => f(w),
            State::Closed(_) => Err(closed()),
        })
        .map_err(|e| self.pending.or(e))
    }

    fn write_batches(
        &self,
        py: Python<'_>,
        batches: impl Iterator<Item = PyResult<RecordBatch>> + Send,
    ) -> PyResult<()> {
        let schema = self.schema.clone();
        self.with_open(py, move |w| {
            let mismatch = |e: String| invalid(format!("the data does not match the writer's schema ({schema}): {e}"));
            for batch in batches {
                let batch = batch?;
                // the types are checked below; columns of the same types in another order or
                // under other names would be written as the schema's columns
                let names = |s: &arrow_schema::Schema| s.fields().iter().map(|f| f.name().clone()).collect::<Vec<_>>();
                let given = names(&batch.schema());
                if given != names(&schema) {
                    return Err(mismatch(format!("the data has the columns {given:?}")));
                }
                // with its row count, which a batch without columns has only there
                let rows = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
                let batch = RecordBatch::try_new_with_options(schema.clone(), batch.columns().to_vec(), &rows)
                    .map_err(|e| mismatch(e.to_string()))?;
                w.write(&batch).map_err(from_pcodec)?;
            }
            Ok(())
        })
    }
}

#[pymethods]
impl Writer {
    /// A writer of `schema` (an `arrow_schema` capsule) to `sink` (a `str` path or a binary
    /// file-like object).
    #[new]
    fn new(py: Python<'_>, sink: &Bound<'_, PyAny>, schema: &Bound<'_, PyAny>, options: Options) -> PyResult<Self> {
        Self::create(py, sink, Arc::new(import_schema(schema)?), &options)
    }

    /// Writes every batch of an `arrow_array_stream` capsule.
    fn write_stream(&self, py: Python<'_>, stream: &Bound<'_, PyAny>) -> PyResult<()> {
        let reader = import_stream(stream)?;
        self.write_batches(py, reader.map(|b| b.map_err(crate::errors::from_arrow)))
    }

    /// Writes the record batch of an `arrow_schema` and an `arrow_array` capsule.
    fn write_batch(&self, py: Python<'_>, schema: &Bound<'_, PyAny>, array: &Bound<'_, PyAny>) -> PyResult<()> {
        let batch = import_batch(schema, array)?;
        self.write_batches(py, std::iter::once(Ok(batch)))
    }

    /// Ends the current row group.
    fn flush(&self, py: Python<'_>) -> PyResult<()> {
        self.with_open(py, |w| w.flush().map_err(from_pcodec))
    }

    /// The codec of every leaf column, once chosen.
    #[getter]
    fn codecs(&self, py: Python<'_>) -> PyResult<Option<Codecs>> {
        locked(py, &self.state, |state| {
            Ok(match state {
                State::Open(w) => w.codecs().map(|c| codec_map(&c)),
                State::Closed(c) => c.clone(),
            })
        })
    }

    /// Writes the footer; returns the codec of every leaf column. A file the writer created
    /// is removed when that fails.
    fn close(&self, py: Python<'_>) -> PyResult<Codecs> {
        locked(py, &self.state, |state| {
            let State::Open(w) = std::mem::replace(state, State::Closed(None)) else { return Err(closed()) };
            let (_, codecs) = w.finish().map_err(|e| {
                discard(self.path.as_ref());
                from_pcodec(e)
            })?;
            *state = State::Closed(Some(codec_map(&codecs)));
            Ok(codec_map(&codecs))
        })
        .map_err(|e| self.pending.or(e))
    }

    /// Drops an open writer without writing the footer, removing the file it created; does
    /// nothing once the writer is closed or aborted.
    fn abort(&self, py: Python<'_>) -> PyResult<()> {
        locked(py, &self.state, |state| {
            if let State::Open(_) = state {
                // dropping the writer closes the file before it is removed
                *state = State::Closed(None);
                discard(self.path.as_ref());
            }
            Ok(())
        })
    }

    #[getter]
    fn closed(&self, py: Python<'_>) -> PyResult<bool> {
        locked(py, &self.state, |state| Ok(matches!(state, State::Closed(_))))
    }
}

fn closed() -> PyErr {
    invalid("the writer is closed")
}

/// Writes every batch of an `arrow_array_stream` capsule to `sink` (a `str` path or a binary
/// file-like object); returns the codec of every leaf column.
#[pyfunction]
pub fn write_stream(
    py: Python<'_>,
    sink: &Bound<'_, PyAny>,
    stream: &Bound<'_, PyAny>,
    options: Options,
) -> PyResult<Codecs> {
    let reader = import_stream(stream)?;
    let w = Writer::create(py, sink, arrow_array::RecordBatchReader::schema(&reader), &options)?;
    if let Err(e) = w.write_batches(py, reader.map(|b| b.map_err(crate::errors::from_arrow))) {
        w.abort(py)?;
        return Err(e);
    }
    w.close(py)
}

/// Leaf path to codec name, from the first row group (empty without row groups).
fn first_row_group_codecs(metadata: &ParquetMetaData) -> Codecs {
    metadata.row_groups().first().map_or_else(Codecs::new, |rg| {
        rg.columns().iter().map(|c| (c.column_path().string(), codec_name(c.compression()))).collect()
    })
}

/// Rewrites `input` to `sink`, with PCODEC (`target="pcodec"`) or as standard Parquet
/// (`target="standard"`, where PCODEC chunks get the base codec).
#[pyfunction]
pub fn transcode(
    py: Python<'_>,
    input: &File,
    sink: &Bound<'_, PyAny>,
    target: &str,
    options: Options,
) -> PyResult<Codecs> {
    let target = match target {
        "pcodec" => {
            let options = options.writer_options()?;
            // what the writer refuses is refused before the output is created or truncated
            let (schema, o) = (input.schema_ref(), options.clone());
            detached(py, move || tparquet::Writer::try_new(std::io::sink(), schema, o).map(drop).map_err(from_pcodec))?;
            Target::Pcodec(options)
        }
        "standard" => Target::Standard { base: base_codec(&options.base)? },
        t => return Err(invalid(format!("unknown target {t:?}: expected \"pcodec\" or \"standard\""))),
    };
    let pending = Pending::default();
    let mut sink = Sink::new(sink, &pending)?;
    let path = sink.path();
    let reader = input.reader();
    detached(py, move || {
        let metadata = tparquet::transcode(reader, &mut sink, &target).map_err(from_pcodec)?;
        std::io::Write::flush(&mut sink).map_err(|e| invalid(e.to_string()))?;
        Ok(first_row_group_codecs(&metadata))
    })
    .map_err(|e| {
        discard(path.as_ref());
        pending.or(e)
    })
}
