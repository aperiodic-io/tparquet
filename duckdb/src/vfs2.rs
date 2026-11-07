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
    let before = CURRENT.with(|c| c.replace(fs));
    let out = f();
    CURRENT.with(|c| c.set(before));
    out
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
        out.sort();
        Ok(out)
    }
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
