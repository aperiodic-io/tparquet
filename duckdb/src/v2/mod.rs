//! The C API v2 build of the extension: the entrypoint `tparquet_init_c_api_v2`.
//!
//! The same library holds both builds; the metadata footer decides which entrypoint DuckDB calls
//! ([`crate::metadata`]): `C_STRUCT` with C API `v1.5.6` calls `tparquet_init_c_api` (any DuckDB
//! that serves C API v1.5.6), `C_STRUCT_UNSTABLE` with DuckDB `v2.0.0` calls
//! `tparquet_init_c_api_v2` (only DuckDB 2.0.0: every function of the C API v2 is unstable in
//! DuckDB 2.0.0, and an extension built on them loads only in the version it was built for).

// The functions here take DuckDB's opaque handles (pointers DuckDB gave, checked by DuckDB on
// every call), not pointers they dereference themselves.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod copy;
pub mod filter;
pub mod read;
pub mod sys;

use libduckdb_sys::v2 as s;
use sys::{Code, report};

/// The DuckDB version the C API v2 build is for: the only one that loads it.
pub const DUCKDB_VERSION: &str = "v2.0.0";

unsafe fn init(input: &s::duckdb_v2_extension_input) -> Result<bool, String> {
    if !unsafe { s::duckdb_rs_extension_api_v2_init(input, DUCKDB_VERSION) }? {
        // DuckDB refused and has recorded why
        return Ok(false);
    }
    read::register(input.extension, input.context)?;
    copy::register(input.extension, input.context)?;
    Ok(true)
}

/// The entrypoint DuckDB calls on `LOAD` of the C API v2 build.
///
/// # Safety
/// Called by DuckDB with its extension input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tparquet_init_c_api_v2(input: *mut s::duckdb_v2_extension_input) {
    let Some(input) = (unsafe { input.as_ref() }) else { return };
    if let Err(e) = crate::ffi::guard(|| unsafe { init(input) }) {
        unsafe { report(input.err, Code::DUCKDB_V2_ERROR_API, &e) };
    }
}
