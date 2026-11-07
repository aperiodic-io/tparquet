//! `read_tparquet` on the C API v2: the same scan as the C API v1 build ([`crate::read`],
//! logic in [`crate::scan`]), plus what only the C API v2 offers:
//!
//! - filter pushdown: the filters of the query skip row groups by their statistics
//!   ([`super::filter`], [`crate::prune`]); DuckDB still applies every filter to the rows;
//! - batch indexes: each row group is a batch, so DuckDB keeps the file order of rows read in
//!   parallel (query results, INSERT, COPY) instead of running such scans on one thread;
//! - DuckDB's glob, for globs over remote file systems ([`crate::vfs2`]).
//!
//! Batches reach DuckDB through its own Arrow importer, as zero-copy vectors.

use crate::ffi::Val;
use crate::prune::Predicate;
use crate::scan::{self, Bound, Local, ReadOptions, Scan, ScanBatch, Slot};
use crate::v2::filter::{self, Columns};
use crate::v2::sys::{self, Code, as_str, factory, logical_type, opaque, report, type_from_id};
use crate::vfs2::{DuckFs, with_context};
use arrow_array::{Array, ArrayRef, Int64Array, StringArray, StructArray};
use arrow_schema::{DataType, Field, Fields};
use libduckdb_sys::v2 as s;
use std::sync::{Arc, Mutex};

/// DuckDB's vector size (`STANDARD_VECTOR_SIZE`): the rows of one batch at most.
pub const VECTOR_SIZE: usize = 2048;

struct BindData {
    bound: Arc<Bound>,
    /// What the filters DuckDB offered say, for skipping row groups.
    predicates: Mutex<Vec<Predicate>>,
}

struct GlobalState {
    scan: Scan,
    slots: Vec<Slot>,
}

#[derive(Default)]
struct LocalState {
    local: Local,
    importer: Option<Importer>,
    /// The batch index of the rows last produced.
    batch: u64,
}

struct Importer(s::duckdb_v2_arrow_importer_handle);

impl Drop for Importer {
    fn drop(&mut self) {
        unsafe { s::duckdb_v2_arrow_importer_destroy(&mut self.0) };
    }
}

fn paths(v: Val) -> Result<Vec<String>, String> {
    match v {
        Val::Str(s) => Ok(vec![s]),
        Val::List(items) => items
            .into_iter()
            .map(|i| match i {
                Val::Str(s) => Ok(s),
                other => Err(format!("the list of files holds {other:?}, not a path")),
            })
            .collect(),
        Val::Null => Err("the path is NULL".into()),
        other => Err(format!("expected a file path, a glob or a list of them, got {other:?}")),
    }
}

fn arg(info: s::duckdb_v2_function_bind_info_handle, index: u64) -> Result<Val, String> {
    let mut v = sys::Value(std::ptr::null_mut());
    call!(duckdb_v2_function_bind_get_arg_value(info, index, &mut v.0))?;
    v.val()
}

fn flag(info: s::duckdb_v2_function_bind_info_handle, name: &str) -> Result<bool, String> {
    let (mut index, mut found) = (0u64, false);
    call!(duckdb_v2_function_bind_get_arg_index(info, &as_str(name), &mut index, &mut found))?;
    if !found {
        return Ok(false);
    }
    match arg(info, index)? {
        Val::Null => Ok(false),
        Val::Bool(b) => Ok(b),
        other => Err(format!("{name} must be a boolean, got {other:?}")),
    }
}

fn bind_impl(
    info: s::duckdb_v2_function_bind_info_handle,
    result: s::duckdb_v2_table_function_bind_info_handle,
    context: s::duckdb_v2_context_handle,
) -> Result<(), String> {
    let patterns = paths(arg(info, 0)?)?;
    let options = ReadOptions {
        filename: flag(info, scan::FILENAME)?,
        file_row_number: flag(info, scan::FILE_ROW_NUMBER)?,
        union_by_name: flag(info, "union_by_name")?,
    };
    let fs: Arc<dyn crate::fs::FileSystem> = Arc::new(DuckFs);
    let bound = Bound::bind(fs.clone(), scan::expand(&patterns, fs.as_ref())?, options)?;
    let f = factory(context)?;
    for (name, t) in bound.result_columns() {
        let lt = logical_type(f, &t)?;
        call!(duckdb_v2_table_function_bind_add_result_column(result, &as_str(&name), lt.0))?;
    }
    call!(duckdb_v2_table_function_bind_set_cardinality(result, bound.estimated_rows, bound.exact_rows))?;
    let mut data = opaque(BindData { bound: Arc::new(bound), predicates: Mutex::new(vec![]) });
    call!(duckdb_v2_function_bind_set_bind_data(info, &mut data))?;
    Ok(())
}

/// Runs a callback body: catches panics, reports errors through DuckDB's error slot.
fn run(err: *mut s::duckdb_v2_error_info_handle, code: Code, f: impl FnOnce() -> Result<(), String>) {
    if let Err(e) = crate::ffi::guard(f) {
        unsafe { report(err, code, &format!("{}: {e}", scan::FUNCTION)) };
    }
}

unsafe extern "C" fn bind(
    info: s::duckdb_v2_function_bind_info_handle,
    result: s::duckdb_v2_table_function_bind_info_handle,
    context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_QUERY_BINDER, || with_context(context, || bind_impl(info, result, context)));
}

fn filter_impl(info: s::duckdb_v2_table_function_filter_pushdown_info_handle) -> Result<(), String> {
    let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
    call!(duckdb_v2_table_function_filter_pushdown_get_bind_data(info, &mut data))?;
    let bind = unsafe { &*(data as *const BindData) };
    let mut n = 0u64;
    call!(duckdb_v2_table_function_filter_pushdown_get_column_count(info, &mut n))?;
    let scan_columns = (0..n)
        .map(|i| {
            let mut c = 0u64;
            call!(duckdb_v2_table_function_filter_pushdown_get_column_index(info, i, &mut c))?;
            Ok(c)
        })
        .collect::<Result<Vec<u64>, String>>()?;
    let types: Vec<_> = bind.bound.columns.iter().map(|c| c.sql.clone()).collect();
    let columns = Columns { scan: &scan_columns, types: &types };
    call!(duckdb_v2_table_function_filter_pushdown_get_filter_count(info, &mut n))?;
    let mut found = vec![];
    for i in 0..n {
        let mut e: s::duckdb_v2_expression_handle = std::ptr::null_mut();
        call!(duckdb_v2_table_function_filter_pushdown_get_filter(info, i, &mut e))?;
        // never accepted: DuckDB keeps filtering the rows, the predicates only skip row groups
        found.extend(filter::predicate(e, &columns));
    }
    bind.predicates.lock().map_err(|_| "poisoned")?.extend(found);
    Ok(())
}

unsafe extern "C" fn filter_pushdown(
    info: s::duckdb_v2_table_function_filter_pushdown_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_QUERY_OPTIMIZER, || filter_impl(info));
}

fn init_global_impl(info: s::duckdb_v2_table_function_init_global_info_handle) -> Result<(), String> {
    let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
    call!(duckdb_v2_table_function_init_global_get_bind_data(info, &mut data))?;
    let bind = unsafe { &*(data as *const BindData) };
    let mut n = 0u64;
    call!(duckdb_v2_table_function_init_global_get_column_count(info, &mut n))?;
    let indices = (0..n)
        .map(|i| {
            let mut c = 0u64;
            call!(duckdb_v2_table_function_init_global_get_column_index(info, i, &mut c))?;
            Ok(c)
        })
        .collect::<Result<Vec<u64>, String>>()?;
    let slots = bind.bound.slots(&indices);
    let predicates = bind.predicates.lock().map_err(|_| "poisoned")?.clone();
    let scan = Scan::new(bind.bound.clone(), &slots, VECTOR_SIZE).with_predicates(predicates);
    call!(duckdb_v2_table_function_init_global_set_max_threads(info, scan.max_threads()))?;
    let mut state = opaque(GlobalState { scan, slots });
    call!(duckdb_v2_table_function_init_global_set_global_state(info, &mut state))?;
    Ok(())
}

unsafe extern "C" fn init_global(
    info: s::duckdb_v2_table_function_init_global_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || init_global_impl(info));
}

unsafe extern "C" fn init_local(
    info: s::duckdb_v2_table_function_init_local_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || {
        let mut state = opaque(LocalState::default());
        call!(duckdb_v2_table_function_init_local_set_local_state(info, &mut state))
    });
}

/// The arrays of a batch, one per column DuckDB asked for.
fn arrays(batch: &ScanBatch, slots: &[Slot]) -> Vec<ArrayRef> {
    let n = batch.num_rows;
    slots
        .iter()
        .map(|slot| -> ArrayRef {
            match slot {
                Slot::File { projected, .. } => batch.columns[*projected].clone(),
                Slot::Filename => {
                    Arc::new(StringArray::from_iter_values(std::iter::repeat_n(batch.file.path.as_str(), n)))
                }
                Slot::FileRowNumber => {
                    Arc::new(Int64Array::from_iter_values((0..n as u64).map(|r| (batch.first_row + r) as i64)))
                }
                Slot::Other => Arc::new(arrow_array::new_null_array(&DataType::Boolean, n)),
            }
        })
        .collect()
}

/// Strips field metadata (Arrow extension types DuckDB would map to other types: the bound
/// types come from [`crate::types::SqlType::from_arrow_type`]).
fn plain(t: &DataType) -> DataType {
    let field = |f: &Arc<Field>| Arc::new(Field::new(f.name(), plain(f.data_type()), f.is_nullable()));
    match t {
        DataType::List(f) => DataType::List(field(f)),
        DataType::LargeList(f) => DataType::LargeList(field(f)),
        DataType::FixedSizeList(f, n) => DataType::FixedSizeList(field(f), *n),
        DataType::Map(f, sorted) => DataType::Map(field(f), *sorted),
        DataType::Struct(fs) => DataType::Struct(fs.iter().map(field).collect()),
        other => other.clone(),
    }
}

fn exec_impl(
    info: s::duckdb_v2_table_function_exec_info_handle,
    context: s::duckdb_v2_context_handle,
) -> Result<(), String> {
    let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
    call!(duckdb_v2_table_function_exec_get_global_state(info, &mut p))?;
    let global = unsafe { &*(p as *const GlobalState) };
    call!(duckdb_v2_table_function_exec_get_local_state(info, &mut p))?;
    let local = unsafe { &mut *(p as *mut LocalState) };
    let mut output: s::duckdb_v2_data_chunk_handle = std::ptr::null_mut();
    call!(duckdb_v2_table_function_exec_get_output_chunk(info, &mut output))?;
    let mut first: s::duckdb_v2_vector_handle = std::ptr::null_mut();
    call!(duckdb_v2_data_chunk_get_vector(output, 0, &mut first))?;
    let Some(batch) = global.scan.next(&mut local.local)? else {
        return call!(duckdb_v2_vector_set_size(first, 0));
    };
    local.batch = batch.batch;
    let columns = arrays(&batch, &global.slots);
    if !columns.is_empty() {
        let of_file = |e: String| format!("cannot read {}: {e}", batch.file.path);
        let fields: Fields =
            columns.iter().enumerate().map(|(i, c)| Field::new(format!("c{i}"), plain(c.data_type()), true)).collect();
        let array = StructArray::try_new(fields.clone(), columns, None).map_err(|e| of_file(e.to_string()))?;
        if local.importer.is_none() {
            let schema =
                arrow_schema::ffi::FFI_ArrowSchema::try_from(&DataType::Struct(fields)).map_err(|e| e.to_string())?;
            let mut importer = Importer(std::ptr::null_mut());
            let schema_ptr = &schema as *const _ as *mut libduckdb_sys::ArrowSchema;
            call!(duckdb_v2_arrow_importer_create(context, schema_ptr, 0, &mut importer.0))?;
            local.importer = Some(importer);
        }
        let importer = local.importer.as_ref().expect("created above").0;
        let mut ffi_array = arrow_data::ffi::FFI_ArrowArray::new(&array.to_data());
        let array_ptr = &mut ffi_array as *mut _ as *mut libduckdb_sys::ArrowArray;
        // consumed: DuckDB takes the buffers over (zero-copy) and clears `release`
        call!(duckdb_v2_arrow_importer_append(importer, array_ptr, true, true)).map_err(of_file)?;
        let mut chunk: s::duckdb_v2_data_chunk_handle = std::ptr::null_mut();
        call!(duckdb_v2_arrow_importer_next_chunk(importer, &mut chunk)).map_err(of_file)?;
        let chunk = Chunk(chunk);
        for i in 0..global.slots.len() as u64 {
            let (mut target, mut source): (s::duckdb_v2_vector_handle, s::duckdb_v2_vector_handle) =
                (std::ptr::null_mut(), std::ptr::null_mut());
            call!(duckdb_v2_data_chunk_get_vector(output, i, &mut target))?;
            call!(duckdb_v2_data_chunk_get_vector(chunk.0, i, &mut source))?;
            call!(duckdb_v2_vector_reference(target, source)).map_err(of_file)?;
        }
    }
    call!(duckdb_v2_vector_set_size(first, batch.num_rows as u64))
}

struct Chunk(s::duckdb_v2_data_chunk_handle);

impl Drop for Chunk {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { s::duckdb_v2_data_chunk_destroy(&mut self.0) };
        }
    }
}

unsafe extern "C" fn exec(
    info: s::duckdb_v2_table_function_exec_info_handle,
    context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || with_context(context, || exec_impl(info, context)));
}

unsafe extern "C" fn partition_data(
    info: s::duckdb_v2_table_function_partition_data_info_handle,
    _context: s::duckdb_v2_context_handle,
    err: *mut s::duckdb_v2_error_info_handle,
) {
    run(err, Code::DUCKDB_V2_ERROR_INPUT_INVALID, || {
        let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
        call!(duckdb_v2_table_function_partition_data_get_local_state(info, &mut p))?;
        let local = unsafe { &*(p as *const LocalState) };
        call!(duckdb_v2_table_function_partition_data_set_batch_index(info, local.batch))
    });
}

struct Function(s::duckdb_v2_table_function_handle);

impl Drop for Function {
    fn drop(&mut self) {
        unsafe { s::duckdb_v2_table_function_destroy(&mut self.0) };
    }
}

/// Registers `read_tparquet` on the extension being loaded.
pub fn register(extension: s::duckdb_v2_extension_handle, context: s::duckdb_v2_context_handle) -> Result<(), String> {
    use s::DUCKDB_V2_FUNCTION_PARAMETER_KIND as Kind;
    use s::DUCKDB_V2_LOGICAL_TYPE_ID as Id;
    let f = factory(context)?;
    let mut function = Function(std::ptr::null_mut());
    call!(duckdb_v2_table_function_create(f, &mut function.0))?;
    let tf = function.0;
    call!(duckdb_v2_table_function_set_name(tf, &as_str(scan::FUNCTION)))?;
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
    let boolean = type_from_id(f, Id::DUCKDB_V2_LOGICAL_TYPE_ID_BOOLEAN)?;
    let mut no = sys::Value(std::ptr::null_mut());
    call!(duckdb_v2_value_create_bool(f, false, &mut no.0))?;
    for name in [scan::FILENAME, scan::FILE_ROW_NUMBER, "union_by_name"] {
        call!(duckdb_v2_function_signature_add_parameter(
            sig,
            &as_str(name),
            boolean.0,
            no.0,
            Kind::DUCKDB_V2_FUNCTION_PARAMETER_KIND_NAMED_ONLY
        ))?;
    }
    call!(duckdb_v2_table_function_set_bind_callback(tf, Some(bind)))?;
    call!(duckdb_v2_table_function_set_init_global_callback(tf, Some(init_global)))?;
    call!(duckdb_v2_table_function_set_init_local_callback(tf, Some(init_local)))?;
    call!(duckdb_v2_table_function_set_exec_callback(tf, Some(exec)))?;
    call!(duckdb_v2_table_function_set_projection_pushdown(tf, true))?;
    call!(duckdb_v2_table_function_set_filter_pushdown_callback(tf, Some(filter_pushdown)))?;
    call!(duckdb_v2_table_function_set_partition_data_callback(tf, Some(partition_data)))?;
    call!(duckdb_v2_extension_register_table_function(extension, tf))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_types_lose_field_metadata_at_any_depth() {
        let meta = |f: Field| {
            f.with_metadata(std::collections::HashMap::from([(
                "ARROW:extension:name".to_string(),
                "arrow.uuid".to_string(),
            )]))
        };
        let inner = Arc::new(meta(Field::new("element", DataType::FixedSizeBinary(16), true)));
        let t = DataType::Struct(
            vec![
                Arc::new(meta(Field::new("l", DataType::List(inner.clone()), true))),
                Arc::new(Field::new("ll", DataType::LargeList(inner.clone()), false)),
                Arc::new(Field::new("f", DataType::FixedSizeList(inner.clone(), 2), true)),
            ]
            .into(),
        );
        let DataType::Struct(fs) = plain(&t) else { panic!() };
        assert!(fs.iter().all(|f| f.metadata().is_empty()));
        assert!(!fs[1].is_nullable());
        let DataType::List(e) = fs[0].data_type() else { panic!() };
        assert!(e.metadata().is_empty());
        assert_eq!(e.data_type(), &DataType::FixedSizeBinary(16));
        assert_eq!(plain(&DataType::Int8), DataType::Int8);
    }

    #[test]
    fn paths_are_strings_or_lists_of_strings() {
        assert_eq!(paths(Val::Str("a".into())).unwrap(), vec!["a"]);
        assert_eq!(paths(Val::List(vec![Val::Str("a".into()), Val::Str("b".into())])).unwrap(), vec!["a", "b"]);
        assert!(paths(Val::List(vec![Val::Int(1)])).unwrap_err().contains("not a path"));
        assert_eq!(paths(Val::Null).unwrap_err(), "the path is NULL");
        assert!(paths(Val::Int(1)).unwrap_err().starts_with("expected a file path"));
    }
}
