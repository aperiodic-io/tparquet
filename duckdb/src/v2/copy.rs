//! `COPY ... (FORMAT tparquet)` on the C API v2.
//!
//! `COPY ... TO` (the same options and file as the C API v1 build, [`crate::copy`]):
//!
//! - bind: the column names and types (`duckdb_v2_copy_to_bind_get_column_name/_type`) and the
//!   options become [`CopyOptions`]; the names DuckDB gives name the columns unless
//!   `COLUMN_NAMES` does;
//! - init, once per file DuckDB writes (one, or one per partition or thread): creates the file;
//! - batch: converts the rows of a batch (a column data collection, scanned chunk by chunk) to
//!   Arrow record batches ([`super::vector`], [`crate::chunk`]);
//! - flush: writes the record batches of a batch. DuckDB never flushes two batches of one file at
//!   once, and flushes them in the order of the rows: the C API v2 sets no parallel execution
//!   mode for a COPY function, so outside `PARTITION_BY` and `PER_THREAD_OUTPUT` DuckDB sinks the
//!   rows from one thread, in the query's order, and cuts and flushes the batches as they come;
//! - finalize: writes the rest and the footer.
//!
//! `COPY table FROM` reads with the scan of `read_tparquet` ([`super::read::Reader`]); the file's
//! columns must be the table's, by position and type.

use crate::chunk::convert;
use crate::copy::{CopyOptions, CopyWriter, FORMAT};
use crate::ffi::Val;
use crate::scan::{self, Bound, ReadOptions, Scan, Slot};
use crate::v2::read::{self, Chunk, Reader, VECTOR_SIZE};
use crate::v2::sys::{self, Code, Type, Value, as_str, factory, from_str, opaque};
use crate::v2::vector::Flat;
use crate::vfs2::{DuckFs, with_context};
use arrow_array::RecordBatch;
use libduckdb_sys::v2 as s;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

/// The extension of the files DuckDB names itself (`PARTITION_BY`, `PER_THREAD_OUTPUT`).
pub const FILE_EXTENSION: &str = "parquet";

const PREFIX: &str = "COPY (FORMAT tparquet): ";

struct ToBind {
    options: CopyOptions,
    /// The column types, to scan the batches with.
    types: Vec<Type>,
}

fn to_run(err: *mut s::duckdb_v2_error_info_handle, code: Code, f: impl FnOnce() -> Result<(), String>) {
    sys::run(err, code, PREFIX, f)
}

/// The text of a borrowed string DuckDB fills in.
fn text(fill: impl FnOnce(&mut s::duckdb_v2_str) -> Result<(), String>) -> Result<String, String> {
    let mut v = s::duckdb_v2_str { ptr: std::ptr::null(), len: 0 };
    fill(&mut v)?;
    Ok(unsafe { from_str(&v) })
}

/// The options of a COPY statement, as `(NAME, value)` with the name upper-cased.
fn options(
    count: u64,
    name: impl Fn(u64, &mut s::duckdb_v2_str) -> Result<(), String>,
    value: impl Fn(u64, &mut s::duckdb_v2_value_handle) -> Result<(), String>,
) -> Result<Vec<(String, Val)>, String> {
    (0..count)
        .map(|i| {
            let n = text(|v| name(i, v))?.to_ascii_uppercase();
            let mut v = Value(std::ptr::null_mut());
            value(i, &mut v.0)?;
            Ok((n, v.val()?))
        })
        .collect()
}

fn to_bind_impl(info: s::duckdb_v2_copy_to_bind_info_handle) -> Result<(), String> {
    let mut n = 0u64;
    call!(duckdb_v2_copy_to_bind_get_column_count(info, &mut n))?;
    let (mut types, mut sql, mut names) = (vec![], vec![], vec![]);
    for i in 0..n {
        let mut t = Type(std::ptr::null_mut());
        call!(duckdb_v2_copy_to_bind_get_column_type(info, i, &mut t.0))?;
        sql.push(t.sql_type()?);
        types.push(t);
        names.push(text(|v| call!(duckdb_v2_copy_to_bind_get_column_name(info, i, v)))?);
    }
    let mut count = 0u64;
    call!(duckdb_v2_copy_to_bind_get_option_count(info, &mut count))?;
    let options = options(
        count,
        |i, v| call!(duckdb_v2_copy_to_bind_get_option_name(info, i, v)),
        |i, v| call!(duckdb_v2_copy_to_bind_get_option_value(info, i, v)),
    )?;
    let options = CopyOptions::parse(sql, Some(names), &options)?;
    let mut data = opaque(ToBind { options, types });
    call!(duckdb_v2_copy_to_bind_set_bind_data(info, &mut data))
}

unsafe extern "C" fn to_bind(
    info: s::duckdb_v2_copy_to_bind_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    to_run(err, Code::DUCKDB_V2_ERROR_QUERY_BINDER, || to_bind_impl(info));
}

/// The data DuckDB keeps for the extension: `get` fills in its pointer.
unsafe fn data<'a, T>(get: impl FnOnce(&mut *mut c_void) -> Result<(), String>) -> Result<&'a T, String> {
    let mut p: *mut c_void = std::ptr::null_mut();
    get(&mut p)?;
    unsafe { (p as *const T).as_ref() }.ok_or_else(|| "DuckDB passed no state".to_string())
}

fn to_init_impl(info: s::duckdb_v2_copy_to_init_info_handle) -> Result<(), String> {
    let bind: &ToBind = unsafe { data(|p| call!(duckdb_v2_copy_to_init_get_bind_data(info, p))) }?;
    let path = text(|v| call!(duckdb_v2_copy_to_init_get_file_path(info, v)))?;
    let writer = CopyWriter::create(&path, &bind.options)?;
    let mut state = opaque(Mutex::new(writer));
    call!(duckdb_v2_copy_to_init_set_init_data(info, &mut state))
}

unsafe extern "C" fn to_init(
    info: s::duckdb_v2_copy_to_init_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    to_run(err, Code::DUCKDB_V2_ERROR_IO_GENERAL, || to_init_impl(info));
}

struct Collection(s::duckdb_v2_column_data_collection_handle);

impl Drop for Collection {
    fn drop(&mut self) {
        unsafe { s::duckdb_v2_column_data_collection_destroy(&mut self.0) };
    }
}

struct SharedScan(s::duckdb_v2_column_data_collection_shared_scan_state_handle);

impl Drop for SharedScan {
    fn drop(&mut self) {
        unsafe { s::duckdb_v2_column_data_collection_shared_scan_state_destroy(&mut self.0) };
    }
}

struct WorkerScan(s::duckdb_v2_column_data_collection_worker_scan_state_handle);

impl Drop for WorkerScan {
    fn drop(&mut self) {
        unsafe { s::duckdb_v2_column_data_collection_worker_scan_state_destroy(&mut self.0) };
    }
}

/// The rows of a chunk of `bind`'s columns as a record batch.
fn record_batch(chunk: s::duckdb_v2_data_chunk_handle, bind: &ToBind) -> Result<RecordBatch, String> {
    let mut rows = 0u64;
    call!(duckdb_v2_data_chunk_get_size(chunk, &mut rows))?;
    let sel: Vec<Option<usize>> = (0..rows as usize).map(Some).collect();
    let columns = bind
        .options
        .column_types
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut v: s::duckdb_v2_vector_handle = std::ptr::null_mut();
            call!(duckdb_v2_data_chunk_get_vector(chunk, i as u64, &mut v))?;
            convert(&Flat::of(v, t)?, t, &sel)
        })
        .collect::<Result<Vec<_>, String>>()?;
    RecordBatch::try_new(bind.options.schema.clone(), columns).map_err(|e| e.to_string())
}

fn to_batch_impl(
    info: s::duckdb_v2_copy_to_batch_info_handle,
    context: s::duckdb_v2_context_handle,
) -> Result<(), String> {
    let bind: &ToBind = unsafe { data(|p| call!(duckdb_v2_copy_to_batch_get_bind_data(info, p))) }?;
    let mut rows = Collection(std::ptr::null_mut());
    call!(duckdb_v2_copy_to_batch_take_input(info, &mut rows.0))?;
    let handles: Vec<s::duckdb_v2_logical_type_handle> = bind.types.iter().map(|t| t.0).collect();
    let mut chunk = Chunk(std::ptr::null_mut());
    call!(duckdb_v2_data_chunk_create(factory(context)?, handles.as_ptr(), handles.len() as u64, &mut chunk.0))?;
    let mut shared = SharedScan(std::ptr::null_mut());
    call!(duckdb_v2_column_data_collection_shared_scan_state_create(rows.0, &mut shared.0))?;
    let mut worker = WorkerScan(std::ptr::null_mut());
    call!(duckdb_v2_column_data_collection_worker_scan_state_create(rows.0, &mut worker.0))?;
    let mut batches = vec![];
    loop {
        let mut produced = false;
        call!(duckdb_v2_column_data_collection_scan(rows.0, shared.0, worker.0, chunk.0, &mut produced))?;
        if !produced {
            break;
        }
        // converted (copied) before the next scan, which ends the scanned chunk's data
        batches.push(record_batch(chunk.0, bind)?);
    }
    let mut prepared = opaque(batches);
    call!(duckdb_v2_copy_to_batch_set_batch_data(info, &mut prepared))
}

unsafe extern "C" fn to_batch(
    info: s::duckdb_v2_copy_to_batch_info_handle,
    context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    to_run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || to_batch_impl(info, context));
}

fn lock(writer: &Mutex<CopyWriter>) -> Result<std::sync::MutexGuard<'_, CopyWriter>, String> {
    writer.lock().map_err(|_| "an earlier write panicked".to_string())
}

fn to_flush_impl(info: s::duckdb_v2_copy_to_flush_info_handle) -> Result<(), String> {
    let writer: &Mutex<CopyWriter> = unsafe { data(|p| call!(duckdb_v2_copy_to_flush_get_init_data(info, p))) }?;
    let batches: &Vec<RecordBatch> = unsafe { data(|p| call!(duckdb_v2_copy_to_flush_get_batch_data(info, p))) }?;
    let mut writer = lock(writer)?;
    batches.iter().try_for_each(|b| writer.write(b.clone()))
}

unsafe extern "C" fn to_flush(
    info: s::duckdb_v2_copy_to_flush_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    to_run(err, Code::DUCKDB_V2_ERROR_IO_GENERAL, || to_flush_impl(info));
}

unsafe extern "C" fn to_finalize(
    info: s::duckdb_v2_copy_to_finalize_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    to_run(err, Code::DUCKDB_V2_ERROR_IO_GENERAL, || {
        let writer: &Mutex<CopyWriter> = unsafe { data(|p| call!(duckdb_v2_copy_to_finalize_get_init_data(info, p))) }?;
        lock(writer)?.finish()
    });
}

/// The options `COPY ... FROM` takes: the flags of `read_tparquet`, as the C API v1 build takes
/// them (DuckDB's C API v1 hands a COPY's options to the reading table function as its named
/// parameters).
fn read_options(options: &[(String, Val)]) -> Result<ReadOptions, String> {
    let mut read = ReadOptions::default();
    for (name, value) in options {
        let name = name.to_ascii_lowercase();
        let flag = match name.as_str() {
            scan::FILENAME => &mut read.filename,
            scan::FILE_ROW_NUMBER => &mut read.file_row_number,
            "union_by_name" => &mut read.union_by_name,
            _ => return Err(format!("'{name}' is not a supported option for copy function '{FORMAT}'")),
        };
        *flag = match value {
            Val::Null => false,
            Val::Bool(b) => *b,
            other => return Err(format!("{name} must be a boolean, got {other:?}")),
        };
    }
    Ok(read)
}

struct FromBind {
    bound: Arc<Bound>,
}

struct FromGlobal {
    scan: Scan,
    slots: Vec<Slot>,
}

fn from_bind_impl(info: s::duckdb_v2_copy_from_bind_info_handle) -> Result<(), String> {
    let path = text(|v| call!(duckdb_v2_copy_from_bind_get_file_path(info, v)))?;
    let mut count = 0u64;
    call!(duckdb_v2_copy_from_bind_get_option_count(info, &mut count))?;
    let options = options(
        count,
        |i, v| call!(duckdb_v2_copy_from_bind_get_option_name(info, i, v)),
        |i, v| call!(duckdb_v2_copy_from_bind_get_option_value(info, i, v)),
    )?;
    let fs: Arc<dyn crate::fs::FileSystem> = Arc::new(DuckFs);
    let bound = Bound::bind(fs.clone(), scan::expand(&[path], fs.as_ref())?, read_options(&options)?)?;
    let mut n = 0u64;
    call!(duckdb_v2_copy_from_bind_get_column_count(info, &mut n))?;
    let table = (0..n)
        .map(|i| {
            let mut t = Type(std::ptr::null_mut());
            call!(duckdb_v2_copy_from_bind_get_column_type(info, i, &mut t.0))?;
            t.sql_type()
        })
        .collect::<Result<Vec<_>, String>>()?;
    let have: Vec<_> = bound.result_columns().into_iter().map(|c| c.1).collect();
    scan::check_copy_from(&have, &table)?;
    call!(duckdb_v2_copy_from_bind_set_cardinality(info, bound.estimated_rows, bound.exact_rows))?;
    let mut data = opaque(FromBind { bound: Arc::new(bound) });
    call!(duckdb_v2_copy_from_bind_set_bind_data(info, &mut data))
}

unsafe extern "C" fn from_bind(
    info: s::duckdb_v2_copy_from_bind_info_handle,
    context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    read::run(err, Code::DUCKDB_V2_ERROR_QUERY_BINDER, || with_context(context, || from_bind_impl(info)));
}

fn from_init_global_impl(info: s::duckdb_v2_copy_from_init_global_info_handle) -> Result<(), String> {
    let bind: &FromBind = unsafe { data(|p| call!(duckdb_v2_copy_from_init_global_get_bind_data(info, p))) }?;
    let columns: Vec<u64> = (0..bind.bound.result_columns().len() as u64).collect();
    let slots = bind.bound.slots(&columns);
    let scan = Scan::new(bind.bound.clone(), &slots, VECTOR_SIZE);
    call!(duckdb_v2_copy_from_init_global_set_max_threads(info, scan.max_threads()))?;
    let mut state = opaque(FromGlobal { scan, slots });
    call!(duckdb_v2_copy_from_init_global_set_global_state(info, &mut state))
}

unsafe extern "C" fn from_init_global(
    info: s::duckdb_v2_copy_from_init_global_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    read::run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || from_init_global_impl(info));
}

unsafe extern "C" fn from_init_local(
    info: s::duckdb_v2_copy_from_init_local_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    read::run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || {
        let mut state = opaque(Reader::default());
        call!(duckdb_v2_copy_from_init_local_set_local_state(info, &mut state))
    });
}

fn from_exec_impl(
    info: s::duckdb_v2_copy_from_exec_info_handle,
    context: s::duckdb_v2_context_handle,
) -> Result<(), String> {
    let global: &FromGlobal = unsafe { data(|p| call!(duckdb_v2_copy_from_exec_get_global_state(info, p))) }?;
    let mut p: *mut c_void = std::ptr::null_mut();
    call!(duckdb_v2_copy_from_exec_get_local_state(info, &mut p))?;
    // the state of this thread alone (from_init_local)
    let reader = unsafe { (p as *mut Reader).as_mut() }.ok_or("DuckDB passed no state")?;
    let mut output: s::duckdb_v2_data_chunk_handle = std::ptr::null_mut();
    call!(duckdb_v2_copy_from_exec_get_output_chunk(info, &mut output))?;
    reader.next(&global.scan, &global.slots, output, context).map(|_| ())
}

unsafe extern "C" fn from_exec(
    info: s::duckdb_v2_copy_from_exec_info_handle,
    context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    read::run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || with_context(context, || from_exec_impl(info, context)));
}

struct Function(s::duckdb_v2_copy_function_handle);

impl Drop for Function {
    fn drop(&mut self) {
        unsafe { s::duckdb_v2_copy_function_destroy(&mut self.0) };
    }
}

/// Registers the COPY format `tparquet` on the extension being loaded.
pub fn register(extension: s::duckdb_v2_extension_handle, context: s::duckdb_v2_context_handle) -> Result<(), String> {
    let mut function = Function(std::ptr::null_mut());
    call!(duckdb_v2_copy_function_create(factory(context)?, &mut function.0))?;
    let f = function.0;
    call!(duckdb_v2_copy_function_set_name(f, &as_str(FORMAT)))?;
    call!(duckdb_v2_copy_function_set_file_extension(f, &as_str(FILE_EXTENSION)))?;
    call!(duckdb_v2_copy_to_set_bind_callback(f, Some(to_bind)))?;
    call!(duckdb_v2_copy_to_set_init_callback(f, Some(to_init)))?;
    call!(duckdb_v2_copy_to_set_batch_callback(f, Some(to_batch)))?;
    call!(duckdb_v2_copy_to_set_flush_callback(f, Some(to_flush)))?;
    call!(duckdb_v2_copy_to_set_finalize_callback(f, Some(to_finalize)))?;
    call!(duckdb_v2_copy_from_set_bind_callback(f, Some(from_bind)))?;
    call!(duckdb_v2_copy_from_set_init_global_callback(f, Some(from_init_global)))?;
    call!(duckdb_v2_copy_from_set_init_local_callback(f, Some(from_init_local)))?;
    call!(duckdb_v2_copy_from_set_exec_callback(f, Some(from_exec)))?;
    call!(duckdb_v2_extension_register_copy_function(extension, f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(pairs: &[(&str, Val)]) -> Vec<(String, Val)> {
        pairs.iter().map(|(n, v)| (n.to_string(), v.clone())).collect()
    }

    #[test]
    fn copy_from_takes_the_flags_of_read_tparquet() {
        assert_eq!(read_options(&[]).unwrap(), ReadOptions::default());
        let o = read_options(&opts(&[("FILENAME", Val::Bool(true)), ("union_by_name", Val::Bool(true))])).unwrap();
        assert_eq!(o, ReadOptions { filename: true, file_row_number: false, union_by_name: true });
        let o = read_options(&opts(&[("FILE_ROW_NUMBER", Val::Bool(true)), ("FILENAME", Val::Null)])).unwrap();
        assert_eq!(o, ReadOptions { filename: false, file_row_number: true, union_by_name: false });
        let o = read_options(&opts(&[("UNION_BY_NAME", Val::Bool(true)), ("UNION_BY_NAME", Val::Bool(false))]));
        assert_eq!(o.unwrap(), ReadOptions::default());
        assert_eq!(
            read_options(&opts(&[("DELIM", Val::Str(",".into()))])).unwrap_err(),
            "'delim' is not a supported option for copy function 'tparquet'"
        );
        assert_eq!(
            read_options(&opts(&[("FILENAME", Val::Int(1))])).unwrap_err(),
            "filename must be a boolean, got Int(1)"
        );
    }
}
