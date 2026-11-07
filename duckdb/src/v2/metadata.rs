//! `tparquet_metadata(path)` on the C API v2 ([`crate::footer`]): the row groups of the files,
//! from their footers only.

use crate::footer::{self, Footers};
use crate::v2::read::{VECTOR_SIZE, import};
use crate::v2::sys::{self, Code, as_str, factory, logical_type, opaque, report, type_from_id};
use crate::vfs2::{DuckFs, with_context};
use libduckdb_sys::v2 as s;
use std::sync::Arc;

struct Files {
    fs: Arc<dyn crate::fs::FileSystem>,
    files: Vec<String>,
}

struct Local {
    importer: Option<crate::v2::read::Importer>,
}

fn run(err: *mut s::duckdb_v2_error_info_handle, code: Code, f: impl FnOnce() -> Result<(), String>) {
    if let Err(e) = crate::ffi::guard(f) {
        unsafe { report(err, code, &format!("{}: {e}", footer::FUNCTION)) };
    }
}

fn bind_impl(
    info: s::duckdb_v2_function_bind_info_handle,
    result: s::duckdb_v2_table_function_bind_info_handle,
    context: s::duckdb_v2_context_handle,
) -> Result<(), String> {
    let mut v = sys::Value(std::ptr::null_mut());
    call!(duckdb_v2_function_bind_get_arg_value(info, 0, &mut v.0))?;
    let patterns = crate::v2::read::paths(v.val()?)?;
    let fs: Arc<dyn crate::fs::FileSystem> = Arc::new(DuckFs);
    let files = crate::scan::expand(&patterns, fs.as_ref())?;
    let f = factory(context)?;
    for (name, t) in footer::columns() {
        let lt = logical_type(f, &t)?;
        call!(duckdb_v2_table_function_bind_add_result_column(result, &as_str(&name), lt.0))?;
    }
    let mut data = opaque(Files { fs, files });
    call!(duckdb_v2_function_bind_set_bind_data(info, &mut data))
}

unsafe extern "C" fn bind(
    info: s::duckdb_v2_function_bind_info_handle,
    result: s::duckdb_v2_table_function_bind_info_handle,
    context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_QUERY_BINDER, || with_context(context, || bind_impl(info, result, context)));
}

unsafe extern "C" fn init_global(
    info: s::duckdb_v2_table_function_init_global_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || {
        let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
        call!(duckdb_v2_table_function_init_global_get_bind_data(info, &mut p))?;
        let bind = unsafe { &*(p as *const Files) };
        let mut state = opaque(Footers::new(bind.fs.clone(), bind.files.clone(), VECTOR_SIZE));
        call!(duckdb_v2_table_function_init_global_set_global_state(info, &mut state))?;
        // one thread: the rows come in file order
        call!(duckdb_v2_table_function_init_global_set_max_threads(info, 1))
    });
}

unsafe extern "C" fn init_local(
    info: s::duckdb_v2_table_function_init_local_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || {
        let mut state = opaque(Local { importer: None });
        call!(duckdb_v2_table_function_init_local_set_local_state(info, &mut state))
    });
}

fn exec_impl(
    info: s::duckdb_v2_table_function_exec_info_handle,
    context: s::duckdb_v2_context_handle,
) -> Result<(), String> {
    let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
    call!(duckdb_v2_table_function_exec_get_global_state(info, &mut p))?;
    let footers = unsafe { &*(p as *const Footers) };
    call!(duckdb_v2_table_function_exec_get_local_state(info, &mut p))?;
    let local = unsafe { &mut *(p as *mut Local) };
    let mut output: s::duckdb_v2_data_chunk_handle = std::ptr::null_mut();
    call!(duckdb_v2_table_function_exec_get_output_chunk(info, &mut output))?;
    let rows = match footers.next()? {
        Some(arrays) => import(context, &mut local.importer, output, arrays)?,
        None => 0,
    };
    let mut first: s::duckdb_v2_vector_handle = std::ptr::null_mut();
    call!(duckdb_v2_data_chunk_get_vector(output, 0, &mut first))?;
    call!(duckdb_v2_vector_set_size(first, rows as u64))
}

unsafe extern "C" fn exec(
    info: s::duckdb_v2_table_function_exec_info_handle,
    context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || with_context(context, || exec_impl(info, context)));
}

/// Registers `tparquet_metadata` on the extension being loaded.
pub fn register(extension: s::duckdb_v2_extension_handle, context: s::duckdb_v2_context_handle) -> Result<(), String> {
    use s::DUCKDB_V2_FUNCTION_PARAMETER_KIND as Kind;
    use s::DUCKDB_V2_LOGICAL_TYPE_ID as Id;
    let f = factory(context)?;
    let function = crate::v2::read::Function::create(f)?;
    let tf = function.0;
    call!(duckdb_v2_table_function_set_name(tf, &as_str(footer::FUNCTION)))?;
    let mut sig: s::duckdb_v2_function_signature_handle = std::ptr::null_mut();
    call!(duckdb_v2_table_function_get_signature(tf, &mut sig))?;
    let any = type_from_id(f, Id::DUCKDB_V2_LOGICAL_TYPE_ID_ANY)?;
    call!(duckdb_v2_function_signature_add_parameter(
        sig,
        &as_str("path"),
        any.0,
        std::ptr::null_mut(),
        Kind::DUCKDB_V2_FUNCTION_PARAMETER_KIND_POSITIONAL_ONLY
    ))?;
    call!(duckdb_v2_table_function_set_bind_callback(tf, Some(bind)))?;
    call!(duckdb_v2_table_function_set_init_global_callback(tf, Some(init_global)))?;
    call!(duckdb_v2_table_function_set_init_local_callback(tf, Some(init_local)))?;
    call!(duckdb_v2_table_function_set_exec_callback(tf, Some(exec)))?;
    call!(duckdb_v2_extension_register_table_function(extension, tf))
}
