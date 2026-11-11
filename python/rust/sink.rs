//! Where a writer writes: a file it creates, or a Python object with `write`.

use crate::errors::{Pending, os_error};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

pub enum Sink {
    File {
        file: BufWriter<File>,
        path: PathBuf,
    },
    /// A binary file-like object; the exception it raises is kept in `pending`.
    Python {
        obj: Py<PyAny>,
        pending: Pending,
    },
}

impl Sink {
    /// A sink for `target`: a `str` path (created or truncated) or a file-like object.
    pub fn new(target: &Bound<'_, PyAny>, pending: &Pending) -> PyResult<Self> {
        if target.is_instance_of::<PyString>() {
            // through the file system encoding: a name that is not UTF-8 is a `str` with
            // surrogate escapes
            let path: PathBuf = target.extract()?;
            let file = File::create(&path).map_err(|e| os_error(&e, &path))?;
            return Ok(Self::File { file: BufWriter::with_capacity(1 << 20, file), path });
        }
        Ok(Self::Python { obj: target.clone().unbind(), pending: pending.clone() })
    }

    /// The file the sink created, if it is one.
    pub fn path(&self) -> Option<PathBuf> {
        match self {
            Self::File { path, .. } => Some(path.clone()),
            Self::Python { .. } => None,
        }
    }
}

fn call<T>(pending: &Pending, f: impl FnOnce(Python<'_>) -> PyResult<T>) -> io::Result<T> {
    Python::attach(f).map_err(|e| {
        let msg = e.to_string();
        pending.set(e);
        io::Error::other(msg)
    })
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::File { file, .. } => file.write(buf),
            Self::Python { obj, pending } => call(pending, |py| {
                let n = obj.call_method1(py, "write", (PyBytes::new(py, buf),))?;
                // `None` from a file object that does not count (`RawIOBase` returns a count)
                Ok(n.extract::<Option<usize>>(py)?.unwrap_or(buf.len()).min(buf.len()))
            }),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::File { file, .. } => file.flush(),
            Self::Python { obj, pending } => call(pending, |py| {
                let obj = obj.bind(py);
                if obj.hasattr("flush")? {
                    obj.call_method0("flush")?;
                }
                Ok(())
            }),
        }
    }
}
