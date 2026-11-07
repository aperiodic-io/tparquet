//! The C API callbacks of the COPY function `tparquet` (`duckdb_register_copy_function`,
//! stable since C API v1.5.6):
//!
//! - bind: the column types (`duckdb_copy_function_bind_get_column_type`) and the options
//!   (`duckdb_copy_function_bind_get_options`, one STRUCT value) become [`CopyOptions`];
//! - global init: creates the file DuckDB names (`duckdb_copy_function_global_init_get_file_path`);
//! - sink: converts each chunk to an Arrow batch ([`crate::chunk`]) and writes it. DuckDB calls
//!   the sink of a C API COPY function from one thread at a time, in order (the C API sets no
//!   parallel execution mode), so the batches keep the query's order;
//! - finalize: writes the rest and the footer.
//!
//! The same COPY function reads with `read_tparquet` for `COPY t FROM`.

use crate::chunk::{VectorView, convert};
use crate::copy::{CopyOptions, CopyWriter, FORMAT};
use crate::ffi::{OwnedType, c_string, drop_box, guard, into_raw, owned_struct, sql_type};
use arrow_array::RecordBatch;
use duckdb::ffi;
use std::ffi::CStr;
use std::sync::Mutex;

/// A flat DuckDB vector, read through the C API.
pub struct Vector(pub ffi::duckdb_vector);

impl VectorView for Vector {
    fn data(&self) -> *const u8 {
        unsafe { ffi::duckdb_vector_get_data(self.0) as *const u8 }
    }
    fn validity(&self) -> *const u64 {
        unsafe { ffi::duckdb_vector_get_validity(self.0) as *const u64 }
    }
    fn list_child(&self) -> (Self, usize) {
        unsafe {
            (Vector(ffi::duckdb_list_vector_get_child(self.0)), ffi::duckdb_list_vector_get_size(self.0) as usize)
        }
    }
    fn struct_child(&self, i: usize) -> Self {
        Vector(unsafe { ffi::duckdb_struct_vector_get_child(self.0, i as u64) })
    }
    fn array_child(&self) -> Self {
        Vector(unsafe { ffi::duckdb_array_vector_get_child(self.0) })
    }
}

unsafe fn bind_impl(info: ffi::duckdb_copy_function_bind_info) -> Result<(), String> {
    let n = unsafe { ffi::duckdb_copy_function_bind_get_column_count(info) };
    let types = (0..n)
        .map(|i| {
            let t = OwnedType(unsafe { ffi::duckdb_copy_function_bind_get_column_type(info, i) });
            unsafe { sql_type(t.0) }
        })
        .collect();
    let options = unsafe { owned_struct(ffi::duckdb_copy_function_bind_get_options(info)) };
    let options = CopyOptions::parse(types, None, &options)?;
    unsafe { ffi::duckdb_copy_function_bind_set_bind_data(info, into_raw(options), Some(drop_box::<CopyOptions>)) };
    Ok(())
}

unsafe extern "C" fn bind(info: ffi::duckdb_copy_function_bind_info) {
    if let Err(e) = guard(|| unsafe { bind_impl(info) }) {
        let e = c_string(&format!("COPY (FORMAT {FORMAT}): {e}"));
        unsafe { ffi::duckdb_copy_function_bind_set_error(info, e.as_ptr()) };
    }
}

unsafe fn global_init_impl(info: ffi::duckdb_copy_function_global_init_info) -> Result<(), String> {
    let options = unsafe { &*(ffi::duckdb_copy_function_global_init_get_bind_data(info) as *const CopyOptions) };
    let path = unsafe { CStr::from_ptr(ffi::duckdb_copy_function_global_init_get_file_path(info)) };
    let writer = CopyWriter::create(&path.to_string_lossy(), options)?;
    let state = into_raw(Mutex::new(writer));
    unsafe { ffi::duckdb_copy_function_global_init_set_global_state(info, state, Some(drop_box::<Mutex<CopyWriter>>)) };
    Ok(())
}

unsafe extern "C" fn global_init(info: ffi::duckdb_copy_function_global_init_info) {
    if let Err(e) = guard(|| unsafe { global_init_impl(info) }) {
        let e = c_string(&format!("COPY (FORMAT {FORMAT}): {e}"));
        unsafe { ffi::duckdb_copy_function_global_init_set_error(info, e.as_ptr()) };
    }
}

fn lock(state: *mut std::ffi::c_void) -> Result<std::sync::MutexGuard<'static, CopyWriter>, String> {
    let state = unsafe { &*(state as *const Mutex<CopyWriter>) };
    state.lock().map_err(|_| "an earlier write panicked".to_string())
}

unsafe fn sink_impl(info: ffi::duckdb_copy_function_sink_info, chunk: ffi::duckdb_data_chunk) -> Result<(), String> {
    let options = unsafe { &*(ffi::duckdb_copy_function_sink_get_bind_data(info) as *const CopyOptions) };
    let rows = unsafe { ffi::duckdb_data_chunk_get_size(chunk) } as usize;
    let sel: Vec<Option<usize>> = (0..rows).map(Some).collect();
    let columns = options
        .column_types
        .iter()
        .enumerate()
        .map(|(i, t)| convert(&Vector(unsafe { ffi::duckdb_data_chunk_get_vector(chunk, i as u64) }), t, &sel))
        .collect::<Result<Vec<_>, String>>()?;
    let batch = RecordBatch::try_new(options.schema.clone(), columns).map_err(|e| e.to_string())?;
    lock(unsafe { ffi::duckdb_copy_function_sink_get_global_state(info) })?.write(batch)
}

unsafe extern "C" fn sink(info: ffi::duckdb_copy_function_sink_info, chunk: ffi::duckdb_data_chunk) {
    if let Err(e) = guard(|| unsafe { sink_impl(info, chunk) }) {
        let e = c_string(&format!("COPY (FORMAT {FORMAT}): {e}"));
        unsafe { ffi::duckdb_copy_function_sink_set_error(info, e.as_ptr()) };
    }
}

unsafe extern "C" fn finalize(info: ffi::duckdb_copy_function_finalize_info) {
    let r = guard(|| lock(unsafe { ffi::duckdb_copy_function_finalize_get_global_state(info) })?.finish());
    if let Err(e) = r {
        let e = c_string(&format!("COPY (FORMAT {FORMAT}): {e}"));
        unsafe { ffi::duckdb_copy_function_finalize_set_error(info, e.as_ptr()) };
    }
}

/// Registers the COPY function `tparquet` on `connection`.
///
/// # Safety
/// `connection` is an open connection.
pub unsafe fn register(connection: ffi::duckdb_connection) -> Result<(), String> {
    unsafe {
        let mut f = ffi::duckdb_create_copy_function();
        let name = c_string(FORMAT);
        ffi::duckdb_copy_function_set_name(f, name.as_ptr());
        ffi::duckdb_copy_function_set_bind(f, Some(bind));
        ffi::duckdb_copy_function_set_global_init(f, Some(global_init));
        ffi::duckdb_copy_function_set_sink(f, Some(sink));
        ffi::duckdb_copy_function_set_finalize(f, Some(finalize));
        let mut from = crate::read::table_function()?;
        ffi::duckdb_copy_function_set_copy_from_function(f, from);
        let state = ffi::duckdb_register_copy_function(connection, f);
        ffi::duckdb_destroy_table_function(&mut from);
        ffi::duckdb_destroy_copy_function(&mut f);
        if state != ffi::duckdb_state_DuckDBSuccess {
            return Err(format!("cannot register the COPY function {FORMAT}"));
        }
    }
    Ok(())
}
