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

/// Runs a callback body: catches panics, reports errors through DuckDB's error slot, each
/// message after `prefix` (`read_tparquet: `, `COPY (FORMAT tparquet): `).
pub fn run(err: *mut s::duckdb_v2_error_info_handle, code: Code, prefix: &str, f: impl FnOnce() -> Result<(), String>) {
    if let Err(e) = crate::ffi::guard(f) {
        unsafe { report(err, code, &format!("{prefix}{e}")) };
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

    /// The name of the type (`INTEGER`, `TIME WITH TIME ZONE`, an alias such as `POINT_2D`).
    pub fn name(&self) -> Result<String, String> {
        let mut v = s::duckdb_v2_str { ptr: std::ptr::null(), len: 0 };
        call!(duckdb_v2_logical_type_get_name(self.0, &mut v))?;
        Ok(unsafe { from_str(&v) })
    }

    /// The parameters of the type, each with its name (empty for a positional one): the width and
    /// scale of a DECIMAL, the child types of a LIST, ARRAY, MAP or STRUCT, the entries of an ENUM.
    pub fn params(&self) -> Result<Vec<(String, Value)>, String> {
        let mut n = 0u64;
        call!(duckdb_v2_logical_type_get_param_count(self.0, &mut n))?;
        (0..n)
            .map(|i| {
                let mut name = s::duckdb_v2_str { ptr: std::ptr::null(), len: 0 };
                let mut v = Value(std::ptr::null_mut());
                call!(duckdb_v2_logical_type_get_param(self.0, i, &mut name, &mut v.0))?;
                Ok((unsafe { from_str(&name) }, v))
            })
            .collect()
    }

    /// The [`SqlType`] of the type: what the extension can write or compare, and the name of
    /// any other type.
    pub fn sql_type(&self) -> Result<SqlType, String> {
        use s::DUCKDB_V2_LOGICAL_TYPE_ID as Id;
        let id = self.id()?;
        if let Some(t) = plain_type(id) {
            return Ok(t);
        }
        let params = self.params()?;
        let int = |i: usize| match params.get(i).map(|p| p.1.val()).transpose()? {
            Some(Val::Int(v)) => Ok(v),
            other => Err(format!("parameter {i} of {} is {other:?}, not an integer", self.name()?)),
        };
        let child = |i: usize| match params.get(i) {
            Some((_, v)) => v.type_value()?.sql_type(),
            None => Err(format!("{} has no parameter {i}", self.name()?)),
        };
        let byte = |i: usize| u8::try_from(int(i)?).map_err(|e| e.to_string());
        Ok(match id {
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_DECIMAL => SqlType::Decimal { width: byte(0)?, scale: byte(1)? },
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_ENUM => {
                SqlType::Enum(params.iter().map(|(_, v)| v.varchar()).collect::<Result<_, _>>()?)
            }
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_LIST => SqlType::List(Box::new(child(0)?)),
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_ARRAY => {
                SqlType::Array(Box::new(child(0)?), usize::try_from(int(1)?).map_err(|e| e.to_string())?)
            }
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_MAP => SqlType::Map(Box::new(child(0)?), Box::new(child(1)?)),
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_STRUCT => SqlType::Struct(
                params
                    .iter()
                    .map(|(n, v)| Ok((n.clone(), v.type_value()?.sql_type()?)))
                    .collect::<Result<_, String>>()?,
            ),
            _ => SqlType::Unsupported(self.name()?),
        })
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

/// The [`SqlType`] of a type id that takes no parameters, if the extension models it.
pub fn plain_type(id: s::DUCKDB_V2_LOGICAL_TYPE_ID) -> Option<SqlType> {
    use s::DUCKDB_V2_LOGICAL_TYPE_ID as Id;
    Some(match id {
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_BOOLEAN => SqlType::Boolean,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TINYINT => SqlType::TinyInt,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_SMALLINT => SqlType::SmallInt,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_INTEGER => SqlType::Integer,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_BIGINT => SqlType::BigInt,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_UTINYINT => SqlType::UTinyInt,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_USMALLINT => SqlType::USmallInt,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_UINTEGER => SqlType::UInteger,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_UBIGINT => SqlType::UBigInt,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_FLOAT => SqlType::Float,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_DOUBLE => SqlType::Double,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_VARCHAR => SqlType::Varchar,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_BLOB => SqlType::Blob,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_DATE => SqlType::Date,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIME => SqlType::Time,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIME_NS => SqlType::TimeNs,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_SEC => SqlType::TimestampS,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_MS => SqlType::TimestampMs,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP => SqlType::Timestamp,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_NS => SqlType::TimestampNs,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_TZ => SqlType::TimestampTz,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_TZ_NS => SqlType::TimestampTzNs,
        Id::DUCKDB_V2_LOGICAL_TYPE_ID_INTERVAL => SqlType::Interval,
        _ => return None,
    })
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

    /// The logical type a TYPE value carries.
    pub fn type_value(&self) -> Result<Type, String> {
        let mut t = Type(std::ptr::null_mut());
        call!(duckdb_v2_value_get_type(self.0, &mut t.0))?;
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

    /// The value as DuckDB renders it (`5.5` for a DECIMAL): `duckdb_v2_value_get_varchar` reads
    /// VARCHAR values only.
    pub fn text(&self) -> Result<String, String> {
        let mut len = 0u64;
        call!(duckdb_v2_value_to_string(self.0, std::ptr::null_mut(), 0, &mut len))?;
        let mut buf = vec![0u8; len as usize + 1];
        call!(duckdb_v2_value_to_string(self.0, buf.as_mut_ptr().cast(), buf.len() as u64, &mut len))?;
        buf.truncate(len as usize);
        Ok(String::from_utf8_lossy(&buf).into_owned())
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
            // as the C API v1 build reads them: any other value by its text (`LEVEL 5.5`)
            _ => Val::Str(self.text()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use s::DUCKDB_V2_LOGICAL_TYPE_ID as Id;

    #[test]
    fn type_ids_without_parameters_are_plain_types() {
        let cases = [
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_BOOLEAN, SqlType::Boolean),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TINYINT, SqlType::TinyInt),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_SMALLINT, SqlType::SmallInt),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_INTEGER, SqlType::Integer),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_BIGINT, SqlType::BigInt),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_UTINYINT, SqlType::UTinyInt),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_USMALLINT, SqlType::USmallInt),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_UINTEGER, SqlType::UInteger),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_UBIGINT, SqlType::UBigInt),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_FLOAT, SqlType::Float),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_DOUBLE, SqlType::Double),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_VARCHAR, SqlType::Varchar),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_BLOB, SqlType::Blob),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_DATE, SqlType::Date),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIME, SqlType::Time),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIME_NS, SqlType::TimeNs),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_SEC, SqlType::TimestampS),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_MS, SqlType::TimestampMs),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP, SqlType::Timestamp),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_NS, SqlType::TimestampNs),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_TZ, SqlType::TimestampTz),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_TZ_NS, SqlType::TimestampTzNs),
            (Id::DUCKDB_V2_LOGICAL_TYPE_ID_INTERVAL, SqlType::Interval),
        ];
        for (id, t) in cases {
            assert_eq!(plain_type(id), Some(t), "{id:?}");
        }
        for id in [
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_DECIMAL,
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_LIST,
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_HUGEINT,
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIME_TZ,
            Id::DUCKDB_V2_LOGICAL_TYPE_ID_SQLNULL,
        ] {
            assert_eq!(plain_type(id), None, "{id:?}");
        }
    }
}
