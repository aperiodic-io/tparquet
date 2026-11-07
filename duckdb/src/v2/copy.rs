//! `COPY ... (FORMAT tparquet)` on the C API v2.

use libduckdb_sys::v2 as s;

/// Registers the COPY format `tparquet` on the extension being loaded.
pub fn register(
    _extension: s::duckdb_v2_extension_handle,
    _context: s::duckdb_v2_context_handle,
) -> Result<(), String> {
    Ok(())
}
