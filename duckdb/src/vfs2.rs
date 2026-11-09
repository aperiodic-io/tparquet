//! DuckDB's virtual file system through the C API v2: local files, `httpfs` (HTTP, S3, ...) and
//! any file system registered with DuckDB, with DuckDB's settings and secrets, and DuckDB's glob.
//!
//! The C API v2 lends a callback the file system of its context for the duration of the
//! callback only, so the file system cannot be kept: each callback that may open files runs
//! inside [`with_context`], which makes its context's file system the current one of the thread,
//! and [`DuckFs`] opens through the current one. An open file keeps its own handle and is read
//! with positional reads from any thread.

// DuckDB's opaque handles, checked by DuckDB on every call (see crate::v2)
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::fs::{FileSystem, Input, RandomAccess};
use crate::v2::sys::{as_str, from_str};
use libduckdb_sys::v2 as s;
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::Arc;

thread_local! {
    static CURRENT: Cell<s::duckdb_v2_file_system_handle> = const { Cell::new(std::ptr::null_mut()) };
}

/// Runs `f` with the file system of `context` as the current one of this thread.
pub fn with_context<T>(context: s::duckdb_v2_context_handle, f: impl FnOnce() -> T) -> T {
    let mut fs: s::duckdb_v2_file_system_handle = std::ptr::null_mut();
    if call!(duckdb_v2_context_get_file_system(context, &mut fs)).is_err() {
        fs = std::ptr::null_mut();
    }
    with_file_system(fs, f)
}

/// Runs `f` with `fs` as the current file system of this thread; the previous one is back when
/// `f` returns or panics (a panic is caught later by the callback's guard, and the borrowed
/// file system must not outlive the callback).
fn with_file_system<T>(fs: s::duckdb_v2_file_system_handle, f: impl FnOnce() -> T) -> T {
    struct Restore(s::duckdb_v2_file_system_handle);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.replace(fs)));
    f()
}

fn current() -> Result<s::duckdb_v2_file_system_handle, String> {
    let fs = CURRENT.with(Cell::get);
    if fs.is_null() { Err("no DuckDB file system in this callback".into()) } else { Ok(fs) }
}

/// The file system of the running callback's context ([`with_context`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct DuckFs;

struct Options(s::duckdb_v2_file_open_options_handle);

impl Drop for Options {
    fn drop(&mut self) {
        unsafe { s::duckdb_v2_file_open_options_destroy(&mut self.0) };
    }
}

unsafe extern "C" fn collect(
    path: *const s::duckdb_v2_str,
    user_data: *mut c_void,
    _err: *mut s::duckdb_v2_error_info_handle,
) {
    let out = unsafe { &mut *(user_data as *mut Vec<String>) };
    if let Some(p) = unsafe { path.as_ref() } {
        out.push(unsafe { from_str(p) });
    }
}

impl FileSystem for DuckFs {
    fn open(&self, path: &str) -> Result<Input, String> {
        let fs = current()?;
        let mut options = Options(std::ptr::null_mut());
        call!(duckdb_v2_file_open_options_create(fs, &mut options.0))
            .map_err(|e| format!("cannot open {path}: {e}"))?;
        for flag in [
            s::DUCKDB_V2_FILE_FLAG::DUCKDB_V2_FILE_FLAG_READ,
            s::DUCKDB_V2_FILE_FLAG::DUCKDB_V2_FILE_FLAG_PARALLEL_ACCESS,
        ] {
            call!(duckdb_v2_file_open_options_set_flag(options.0, flag))
                .map_err(|e| format!("cannot open {path}: {e}"))?;
        }
        let mut handle: s::duckdb_v2_file_handle = std::ptr::null_mut();
        call!(duckdb_v2_file_system_open(fs, &as_str(path), options.0, &mut handle))
            .map_err(|e| format!("cannot open {path}: {}", e.trim_start_matches("duckdb_v2_file_system_open: ")))?;
        let mut file = DuckFile { handle, size: 0 };
        call!(duckdb_v2_file_size(handle, &mut file.size)).map_err(|e| format!("cannot open {path}: {e}"))?;
        Ok(Input::new(path, Arc::new(file)))
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, String> {
        let fs = current()?;
        let mut out: Vec<String> = vec![];
        call!(duckdb_v2_file_system_path_glob(
            fs,
            &as_str(pattern),
            Some(collect),
            (&mut out as *mut Vec<String>).cast()
        ))
        .map_err(|e| {
            format!("cannot glob {pattern:?}: {}", e.trim_start_matches("duckdb_v2_file_system_path_glob: "))
        })?;
        let mut out: Vec<String> = out.into_iter().map(|p| as_written(pattern, p)).collect();
        out.sort();
        Ok(out)
    }
}

/// `path`, a match of `pattern`, with the scheme spelled as in `pattern`: DuckDB's glob over an
/// fsspec file system answers `memory://data/*` with `memory:///data/...`, while a path without
/// a glob is read and named as it is written.
pub fn as_written(pattern: &str, path: String) -> String {
    if let Some((scheme, rest)) = pattern.split_once("://")
        && !rest.starts_with('/')
        && let Some(tail) = path.strip_prefix(&format!("{scheme}:///"))
    {
        return format!("{scheme}://{tail}");
    }
    path
}

struct DuckFile {
    handle: s::duckdb_v2_file_handle,
    size: u64,
}

// SAFETY: the file is opened with FILE_FLAG_PARALLEL_ACCESS and read only with positional reads,
// which DuckDB documents as safe from several threads at once.
unsafe impl Send for DuckFile {}
unsafe impl Sync for DuckFile {}

impl Drop for DuckFile {
    fn drop(&mut self) {
        unsafe { s::duckdb_v2_file_destroy(&mut self.handle) };
    }
}

impl RandomAccess for DuckFile {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        if buf.is_empty() {
            return Ok(());
        }
        call!(duckdb_v2_file_read_at(self.handle, buf.as_mut_ptr().cast(), buf.len() as u64, offset))
            .map_err(|e| e.trim_start_matches("duckdb_v2_file_read_at: ").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_system_of_a_callback_ends_with_it_even_when_it_panics() {
        let fs = 0x10 as s::duckdb_v2_file_system_handle;
        assert!(current().is_err());
        let r = std::panic::catch_unwind(|| {
            with_file_system(fs, || {
                assert_eq!(current(), Ok(fs));
                panic!("in a callback")
            })
        });
        assert!(r.is_err());
        assert!(current().is_err(), "the borrowed file system outlived its callback");
        // nested scopes restore the outer one
        with_file_system(fs, || {
            with_file_system(std::ptr::null_mut(), || assert!(current().is_err()));
            assert_eq!(current(), Ok(fs));
        });
        assert!(current().is_err());
    }

    #[test]
    fn glob_results_keep_the_spelling_of_the_pattern() {
        // fsspec file systems answer a glob of memory://data/* with memory:///data/...
        let p = |s: &str| s.to_string();
        assert_eq!(as_written("memory://data/*.parquet", p("memory:///data/a.parquet")), "memory://data/a.parquet");
        assert_eq!(as_written("memory:///data/*.parquet", p("memory:///data/a.parquet")), "memory:///data/a.parquet");
        assert_eq!(as_written("s3://b/*.parquet", p("s3://b/a.parquet")), "s3://b/a.parquet");
        assert_eq!(as_written("/tmp/*.parquet", p("/tmp/a.parquet")), "/tmp/a.parquet");
        assert_eq!(as_written("memory://data/*", p("other:///x")), "other:///x");
    }
}
