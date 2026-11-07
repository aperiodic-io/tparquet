//! Helpers over DuckDB's C API v2 (`libduckdb_sys::v2`): calls that report errors through an
//! error info handle, strings, values, logical types and opaque pointers.

use crate::ffi::Val;
use crate::types::SqlType;
use libduckdb_sys::v2 as s;
use std::ffi::c_void;

pub use s::DUCKDB_V2_ERROR as Code;

/// A failed call of the C API: its error code and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: Code,
    pub message: String,
}

/// Reads and destroys an error info handle.
fn take_text(mut err: s::duckdb_v2_error_info_handle) -> String {
    if err.is_null() {
        return String::new();
    }
    let mut text = s::duckdb_v2_str { ptr: std::ptr::null(), len: 0 };
    let read = unsafe { s::duckdb_v2_error_info_get_text(err, &mut text) };
    let message = if read == Code::DUCKDB_V2_ERROR_NONE { unsafe { from_str(&text) } } else { String::new() };
    unsafe { s::duckdb_v2_error_info_destroy(&mut err) };
    message
}

/// The result of a C API call that returned `code` and maybe filled `err`.
pub fn check(code: Code, err: s::duckdb_v2_error_info_handle, what: &str) -> Result<(), String> {
    let message = take_text(err);
    if code == Code::DUCKDB_V2_ERROR_NONE {
        return Ok(());
    }
    Err(if message.is_empty() { format!("{what} failed ({code:?})") } else { format!("{what}: {message}") })
}

/// Reports `message` through the error slot of a callback (never NULL, never pointing at
/// NULL, owned by DuckDB).
///
/// # Safety
/// `slot` is the error slot DuckDB passed to the running callback.
pub unsafe fn report(slot: *mut s::duckdb_v2_error_info_handle, code: Code, message: &str) {
    if slot.is_null() || unsafe { (*slot).is_null() } {
        return;
    }
    let text = as_str(message);
    unsafe {
        s::duckdb_v2_error_info_set_code(*slot, code);
        s::duckdb_v2_error_info_set_text(*slot, &text);
    }
}

/// A borrowed view of `text` (valid while `text` lives).
pub fn as_str(text: &str) -> s::duckdb_v2_str {
    s::duckdb_v2_str { ptr: text.as_ptr().cast(), len: text.len() as u64 }
}

/// The text of a view DuckDB gave.
///
/// # Safety
/// `v` points at `len` readable bytes (or `len` is 0).
pub unsafe fn from_str(v: &s::duckdb_v2_str) -> String {
    if v.len == 0 || v.ptr.is_null() {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(v.ptr.cast::<u8>(), v.len as usize) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// An opaque pointer that owns a boxed `T`, dropped by DuckDB through `destroy`.
pub fn opaque<T>(value: T) -> s::duckdb_v2_opaque {
    s::duckdb_v2_opaque { ptr: crate::ffi::into_raw(value), destroy: Some(drop_box::<T>), equals: None }
}

unsafe extern "C" fn drop_box<T>(p: *mut c_void) {
    unsafe { crate::ffi::drop_box::<T>(p) }
}

/// A value DuckDB gave, destroyed on drop.
pub struct Value(pub s::duckdb_v2_value_handle);

impl Drop for Value {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { s::duckdb_v2_value_destroy(&mut self.0) };
        }
    }
}

/// A logical type DuckDB gave or the extension made, destroyed on drop.
pub struct Type(pub s::duckdb_v2_logical_type_handle);

impl Drop for Type {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { s::duckdb_v2_logical_type_destroy(&mut self.0) };
        }
    }
}

impl Type {
    pub fn id(&self) -> Result<s::DUCKDB_V2_LOGICAL_TYPE_ID, String> {
        let mut id = s::DUCKDB_V2_LOGICAL_TYPE_ID::DUCKDB_V2_LOGICAL_TYPE_ID_INVALID;
        call!(duckdb_v2_logical_type_get_id(self.0, &mut id))?;
        Ok(id)
    }

    /// The type as DuckDB writes it (`DECIMAL(18,3)`, `STRUCT(a INTEGER)`...).
    pub fn text(&self) -> Result<String, String> {
        let mut len = 0u64;
        call!(duckdb_v2_logical_type_to_text(self.0, std::ptr::null_mut(), 0, &mut len))?;
        let mut buf = vec![0u8; len as usize + 1];
        call!(duckdb_v2_logical_type_to_text(self.0, buf.as_mut_ptr().cast(), buf.len() as u64, &mut len))?;
        buf.truncate(len as usize);
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

/// The factory of a callback's context.
pub fn factory(context: s::duckdb_v2_context_handle) -> Result<s::duckdb_v2_factory_handle, String> {
    let mut f: s::duckdb_v2_factory_handle = std::ptr::null_mut();
    call!(duckdb_v2_context_get_factory(context, &mut f))?;
    Ok(f)
}

/// A logical type parsed from DuckDB's text for it.
pub fn type_from_text(factory: s::duckdb_v2_factory_handle, text: &str) -> Result<Type, String> {
    let mut t = Type(std::ptr::null_mut());
    call!(duckdb_v2_factory_create_type_from_text(factory, &as_str(text), &mut t.0))
        .map_err(|e| format!("type {text}: {e}"))?;
    Ok(t)
}

/// A logical type of a type id without parameters (`ANY`, `BOOLEAN`).
pub fn type_from_id(factory: s::duckdb_v2_factory_handle, id: s::DUCKDB_V2_LOGICAL_TYPE_ID) -> Result<Type, String> {
    let mut t = Type(std::ptr::null_mut());
    call!(duckdb_v2_factory_create_type_from_id(factory, id, std::ptr::null(), std::ptr::null(), 0, &mut t.0))?;
    Ok(t)
}

/// The DuckDB logical type of a [`SqlType`], through DuckDB's own parser of type names.
pub fn logical_type(factory: s::duckdb_v2_factory_handle, t: &SqlType) -> Result<Type, String> {
    type_from_text(factory, &t.to_string())
}

impl Value {
    pub fn logical_type(&self) -> Result<Type, String> {
        let mut t = Type(std::ptr::null_mut());
        call!(duckdb_v2_value_get_logical_type(self.0, &mut t.0))?;
        Ok(t)
    }

    pub fn is_null(&self) -> Result<bool, String> {
        let mut null = false;
        call!(duckdb_v2_value_is_null(self.0, &mut null))?;
        Ok(null)
    }

    pub fn varchar(&self) -> Result<String, String> {
        let mut v = s::duckdb_v2_str { ptr: std::ptr::null(), len: 0 };
        call!(duckdb_v2_value_get_varchar(self.0, &mut v))?;
        Ok(unsafe { from_str(&v) })
    }

    pub fn blob(&self) -> Result<Vec<u8>, String> {
        let mut v = s::duckdb_v2_str { ptr: std::ptr::null(), len: 0 };
        call!(duckdb_v2_value_get_blob(self.0, &mut v))?;
        if v.len == 0 {
            return Ok(vec![]);
        }
        Ok(unsafe { std::slice::from_raw_parts(v.ptr.cast::<u8>(), v.len as usize) }.to_vec())
    }

    pub fn children(&self) -> Result<Vec<Value>, String> {
        let mut n = 0u64;
        call!(duckdb_v2_value_get_child_count(self.0, &mut n))?;
        (0..n)
            .map(|i| {
                let mut c = Value(std::ptr::null_mut());
                call!(duckdb_v2_value_get_child(self.0, i, &mut c.0))?;
                Ok(c)
            })
            .collect()
    }

    /// The value as the extension's [`Val`] (the kinds its parameters and options take).
    pub fn val(&self) -> Result<Val, String> {
        use s::DUCKDB_V2_LOGICAL_TYPE_ID as Id;
        if self.is_null()? {
            return Ok(Val::Null);
        }
        let t = self.logical_type()?;
        Ok(match t.id()? {
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_SQLNULL => Val::Null,
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_BOOLEAN => {
                let mut b = false;
                call!(duckdb_v2_value_get_bool(self.0, &mut b))?;
                Val::Bool(b)
            }
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_TINYINT
            | Id::DUCKDB_V2_LOGICAL_TYPE_ID_SMALLINT
            | Id::DUCKDB_V2_LOGICAL_TYPE_ID_INTEGER
            | Id::DUCKDB_V2_LOGICAL_TYPE_ID_BIGINT
            | Id::DUCKDB_V2_LOGICAL_TYPE_ID_UTINYINT
            | Id::DUCKDB_V2_LOGICAL_TYPE_ID_USMALLINT
            | Id::DUCKDB_V2_LOGICAL_TYPE_ID_UINTEGER => {
                let mut v = 0i64;
                call!(duckdb_v2_value_get_bigint(self.0, &mut v))?;
                Val::Int(v)
            }
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_VARCHAR => Val::Str(self.varchar()?),
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_LIST
            | Id::DUCKDB_V2_LOGICAL_TYPE_ID_ARRAY
            | Id::DUCKDB_V2_LOGICAL_TYPE_ID_STRUCT => {
                Val::List(self.children()?.iter().map(Value::val).collect::<Result<_, _>>()?)
            }
            // as the C API v1 build reads them: any other value by its text
            _ => Val::Str(self.varchar()?),
        })
    }
}
