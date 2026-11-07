//! DuckDB's virtual file system through the C API v1 (`duckdb_client_context_get_file_system`,
//! stable since v1.5.6): local files, `httpfs` (HTTP, S3, ...) and any file system registered
//! with DuckDB, with DuckDB's settings and secrets.
//!
//! The C API v1 has no glob: local globs go through the `glob` crate, and a glob over a remote
//! path is refused (the C API v2 build has DuckDB's glob, [`crate::vfs2`]).

use crate::ffi::{c_string, take_error};
use crate::fs::{FileSystem, Input, RandomAccess, is_remote, local_glob};
use duckdb::ffi;
use std::sync::{Arc, Mutex};

/// The message of a glob over a remote path in the v1 build.
pub const REMOTE_GLOB: &str = "globs over remote files need the C API v2 build of the extension \
     (DuckDB's C API v1 has no glob); list the files instead";

/// The file system of a client context (a connection): it lives as long as the connection, and
/// so as long as anything a query of it binds.
pub struct DuckFs {
    fs: Mutex<ffi::duckdb_file_system>,
}

// SAFETY: DuckDB's file systems are thread-safe; the wrapper's error slot is guarded by the mutex.
unsafe impl Send for DuckFs {}
unsafe impl Sync for DuckFs {}

impl DuckFs {
    /// The file system of the connection binding a table function.
    ///
    /// # Safety
    /// `info` is the bind info of a running bind callback.
    pub unsafe fn of_bind(info: ffi::duckdb_bind_info) -> Result<Self, String> {
        let mut context: ffi::duckdb_client_context = std::ptr::null_mut();
        unsafe { ffi::duckdb_table_function_get_client_context(info, &mut context) };
        if context.is_null() {
            return Err("DuckDB gave no client context".into());
        }
        let fs = unsafe { ffi::duckdb_client_context_get_file_system(context) };
        unsafe { ffi::duckdb_destroy_client_context(&mut context) };
        if fs.is_null() {
            return Err("DuckDB gave no file system".into());
        }
        Ok(Self { fs: Mutex::new(fs) })
    }
}

impl Drop for DuckFs {
    fn drop(&mut self) {
        if let Ok(fs) = self.fs.get_mut() {
            unsafe { ffi::duckdb_destroy_file_system(fs) };
        }
    }
}

struct Options(ffi::duckdb_file_open_options);

impl Drop for Options {
    fn drop(&mut self) {
        unsafe { ffi::duckdb_destroy_file_open_options(&mut self.0) };
    }
}

impl FileSystem for DuckFs {
    fn open(&self, path: &str) -> Result<Input, String> {
        let fs = self.fs.lock().map_err(|_| "a reader thread panicked")?;
        let options = Options(unsafe { ffi::duckdb_create_file_open_options() });
        unsafe { ffi::duckdb_file_open_options_set_flag(options.0, ffi::duckdb_file_flag_DUCKDB_FILE_FLAG_READ, true) };
        let c_path = c_string(path);
        let mut handle: ffi::duckdb_file_handle = std::ptr::null_mut();
        let state = unsafe { ffi::duckdb_file_system_open(*fs, c_path.as_ptr(), options.0, &mut handle) };
        if state != ffi::duckdb_state_DuckDBSuccess || handle.is_null() {
            let e = unsafe { take_error(ffi::duckdb_file_system_error_data(*fs)) };
            return Err(format!("cannot open {path}: {e}"));
        }
        let mut file = DuckFile { handle: Mutex::new(handle), size: 0 };
        let size = unsafe { ffi::duckdb_file_handle_size(handle) };
        if size < 0 {
            return Err(format!("cannot open {path}: {}", file.error()));
        }
        file.size = size as u64;
        Ok(Input::new(path, Arc::new(file)))
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, String> {
        if is_remote(pattern) {
            return Err(format!("{REMOTE_GLOB}: {pattern:?}"));
        }
        local_glob(pattern)
    }
}

/// An open file; reads seek and read under its lock.
struct DuckFile {
    handle: Mutex<ffi::duckdb_file_handle>,
    size: u64,
}

// SAFETY: the handle is only used under its mutex.
unsafe impl Send for DuckFile {}
unsafe impl Sync for DuckFile {}

impl DuckFile {
    fn error(&self) -> String {
        match self.handle.lock() {
            Ok(h) => unsafe { take_error(ffi::duckdb_file_handle_error_data(*h)) },
            Err(_) => "a reader thread panicked".into(),
        }
    }
}

impl Drop for DuckFile {
    fn drop(&mut self) {
        if let Ok(h) = self.handle.get_mut() {
            unsafe { ffi::duckdb_destroy_file_handle(h) };
        }
    }
}

impl RandomAccess for DuckFile {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        let h = self.handle.lock().map_err(|_| "a reader thread panicked")?;
        let failed = || unsafe { take_error(ffi::duckdb_file_handle_error_data(*h)) };
        if unsafe { ffi::duckdb_file_handle_seek(*h, offset as i64) } != ffi::duckdb_state_DuckDBSuccess {
            return Err(failed());
        }
        let mut done = 0;
        while done < buf.len() {
            let rest = &mut buf[done..];
            let n = unsafe { ffi::duckdb_file_handle_read(*h, rest.as_mut_ptr().cast(), rest.len() as i64) };
            match n {
                n if n < 0 => return Err(failed()),
                0 => return Err(format!("the file ends {} bytes early", rest.len())),
                n => done += n as usize,
            }
        }
        Ok(())
    }
}
