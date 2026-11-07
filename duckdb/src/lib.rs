//! `tparquet`: a DuckDB extension that reads and writes Parquet files whose column chunks
//! use the PCODEC codec (`spec/FORMAT.md`), built on DuckDB's C extension API (v1).
//!
//! - `read_tparquet(path, filename := , file_row_number := , union_by_name := )`, a table
//!   function ([`read`], logic in [`scan`]);
//! - `COPY ... TO 'f.parquet' (FORMAT tparquet, COLUMN_NAMES [...], POLICY 'auto', ...)`
//!   and `COPY t FROM 'f.parquet' (FORMAT tparquet)`, a COPY function ([`write`], logic
//!   in [`copy`] and [`chunk`]).

pub mod chunk;
pub mod copy;
pub mod ffi;
pub mod fs;
pub mod metadata;
pub mod prune;
pub mod read;
pub mod scan;
pub mod types;
pub mod vfs1;
pub mod write;

use duckdb::ffi as sys;
use std::ffi::c_char;

/// The C API version the extension asks DuckDB for: the first that has COPY functions in its
/// stable API. DuckDB 2.0.0 serves C API v1.5.6.
pub const C_API_VERSION: &str = "v1.5.6";
/// The extension name: the file is `tparquet.duckdb_extension` and the entrypoint
/// `tparquet_init_c_api`.
pub const EXTENSION_NAME: &str = "tparquet";

unsafe fn init(info: sys::duckdb_extension_info, access: *const sys::duckdb_extension_access) -> Result<bool, String> {
    if !unsafe { sys::duckdb_rs_extension_api_init(info, access, C_API_VERSION) }? {
        // DuckDB refused the version and has recorded why
        return Ok(false);
    }
    let get_database = unsafe { (*access).get_database }.ok_or("DuckDB passed no get_database")?;
    let db = unsafe { get_database(info) };
    if db.is_null() {
        return Ok(false);
    }
    let mut connection: sys::duckdb_connection = std::ptr::null_mut();
    if unsafe { sys::duckdb_connect(*db, &mut connection) } != sys::duckdb_state_DuckDBSuccess {
        return Err("cannot connect to the database".into());
    }
    let registered = unsafe { read::register(connection) }.and_then(|()| unsafe { write::register(connection) });
    unsafe { sys::duckdb_disconnect(&mut connection) };
    registered.map(|()| true)
}

/// The entrypoint DuckDB calls on `LOAD`.
///
/// # Safety
/// Called by DuckDB with its extension info and access table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tparquet_init_c_api(
    info: sys::duckdb_extension_info,
    access: *const sys::duckdb_extension_access,
) -> bool {
    match ffi::guard(|| unsafe { init(info, access) }) {
        Ok(loaded) => loaded,
        Err(e) => {
            if let Some(set_error) = unsafe { access.as_ref() }.and_then(|a| a.set_error) {
                let e = ffi::c_string(&e);
                unsafe { set_error(info, e.as_ptr() as *const c_char) };
            }
            false
        }
    }
}
