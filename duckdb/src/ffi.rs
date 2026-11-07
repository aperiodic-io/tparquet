//! Small safe-ish wrappers over the DuckDB C API that the callbacks share.
//!
//! Everything here calls into DuckDB, so it only runs inside a loaded extension; the SQL tests
//! (`duckdb/tests`) exercise it.

use crate::types::SqlType;
use duckdb::ffi;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A logical type this code owns and destroys.
pub struct OwnedType(pub ffi::duckdb_logical_type);

impl Drop for OwnedType {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::duckdb_destroy_logical_type(&mut self.0) };
        }
    }
}

fn simple(id: ffi::DUCKDB_TYPE) -> OwnedType {
    OwnedType(unsafe { ffi::duckdb_create_logical_type(id) })
}

/// A C string for DuckDB; interior NULs (impossible in names DuckDB gave us) become `?`.
pub fn c_string(s: &str) -> CString {
    CString::new(s.replace('\0', "?")).expect("no NUL left")
}

/// Takes a string DuckDB allocated, and frees it.
///
/// # Safety
/// `p` is null or a NUL-terminated string from `duckdb_malloc`.
pub unsafe fn take_string(p: *mut c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { ffi::duckdb_free(p as *mut c_void) };
    s
}

/// The message of an error DuckDB reported, and frees it.
///
/// # Safety
/// `e` is null or error data from DuckDB, owned by the caller.
pub unsafe fn take_error(mut e: ffi::duckdb_error_data) -> String {
    if e.is_null() {
        return "unknown error".into();
    }
    let p = unsafe { ffi::duckdb_error_data_message(e) };
    let message = if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() };
    unsafe { ffi::duckdb_destroy_error_data(&mut e) };
    if message.is_empty() { "unknown error".into() } else { message }
}

/// The DuckDB logical type of a [`SqlType`].
pub fn logical_type(t: &SqlType) -> Result<OwnedType, String> {
    use ffi::*;
    Ok(match t {
        SqlType::Boolean => simple(DUCKDB_TYPE_DUCKDB_TYPE_BOOLEAN),
        SqlType::TinyInt => simple(DUCKDB_TYPE_DUCKDB_TYPE_TINYINT),
        SqlType::SmallInt => simple(DUCKDB_TYPE_DUCKDB_TYPE_SMALLINT),
        SqlType::Integer => simple(DUCKDB_TYPE_DUCKDB_TYPE_INTEGER),
        SqlType::BigInt => simple(DUCKDB_TYPE_DUCKDB_TYPE_BIGINT),
        SqlType::UTinyInt => simple(DUCKDB_TYPE_DUCKDB_TYPE_UTINYINT),
        SqlType::USmallInt => simple(DUCKDB_TYPE_DUCKDB_TYPE_USMALLINT),
        SqlType::UInteger => simple(DUCKDB_TYPE_DUCKDB_TYPE_UINTEGER),
        SqlType::UBigInt => simple(DUCKDB_TYPE_DUCKDB_TYPE_UBIGINT),
        SqlType::Float => simple(DUCKDB_TYPE_DUCKDB_TYPE_FLOAT),
        SqlType::Double => simple(DUCKDB_TYPE_DUCKDB_TYPE_DOUBLE),
        SqlType::Decimal { width, scale } => OwnedType(unsafe { duckdb_create_decimal_type(*width, *scale) }),
        SqlType::Varchar => simple(DUCKDB_TYPE_DUCKDB_TYPE_VARCHAR),
        SqlType::Blob => simple(DUCKDB_TYPE_DUCKDB_TYPE_BLOB),
        SqlType::Date => simple(DUCKDB_TYPE_DUCKDB_TYPE_DATE),
        SqlType::Time => simple(DUCKDB_TYPE_DUCKDB_TYPE_TIME),
        SqlType::TimeNs => simple(DUCKDB_TYPE_DUCKDB_TYPE_TIME_NS),
        SqlType::TimestampS => simple(DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_S),
        SqlType::TimestampMs => simple(DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_MS),
        SqlType::Timestamp => simple(DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP),
        SqlType::TimestampNs => simple(DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_NS),
        SqlType::TimestampTz => simple(DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_TZ),
        SqlType::TimestampTzNs => simple(DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_TZ_NS),
        SqlType::Interval => simple(DUCKDB_TYPE_DUCKDB_TYPE_INTERVAL),
        SqlType::Enum(values) => {
            let names: Vec<CString> = values.iter().map(|v| c_string(v)).collect();
            let mut ptrs: Vec<*const c_char> = names.iter().map(|n| n.as_ptr()).collect();
            OwnedType(unsafe { duckdb_create_enum_type(ptrs.as_mut_ptr(), ptrs.len() as idx_t) })
        }
        SqlType::List(child) => {
            let child = logical_type(child)?;
            OwnedType(unsafe { duckdb_create_list_type(child.0) })
        }
        SqlType::Array(child, n) => {
            let child = logical_type(child)?;
            OwnedType(unsafe { duckdb_create_array_type(child.0, *n as idx_t) })
        }
        SqlType::Map(k, v) => {
            let (k, v) = (logical_type(k)?, logical_type(v)?);
            OwnedType(unsafe { duckdb_create_map_type(k.0, v.0) })
        }
        SqlType::Struct(fields) => {
            let types = fields.iter().map(|(_, t)| logical_type(t)).collect::<Result<Vec<_>, _>>()?;
            let names: Vec<CString> = fields.iter().map(|(n, _)| c_string(n)).collect();
            let mut type_ptrs: Vec<duckdb_logical_type> = types.iter().map(|t| t.0).collect();
            let mut name_ptrs: Vec<*const c_char> = names.iter().map(|n| n.as_ptr()).collect();
            OwnedType(unsafe {
                duckdb_create_struct_type(type_ptrs.as_mut_ptr(), name_ptrs.as_mut_ptr(), fields.len() as idx_t)
            })
        }
        SqlType::Unsupported(name) => return Err(format!("cannot create the DuckDB type {name}")),
    })
}

/// The [`SqlType`] of a DuckDB logical type (borrowed, not destroyed).
///
/// # Safety
/// `t` is a valid logical type.
pub unsafe fn sql_type(t: ffi::duckdb_logical_type) -> SqlType {
    use ffi::*;
    let id = unsafe { duckdb_get_type_id(t) };
    match id {
        DUCKDB_TYPE_DUCKDB_TYPE_BOOLEAN => SqlType::Boolean,
        DUCKDB_TYPE_DUCKDB_TYPE_TINYINT => SqlType::TinyInt,
        DUCKDB_TYPE_DUCKDB_TYPE_SMALLINT => SqlType::SmallInt,
        DUCKDB_TYPE_DUCKDB_TYPE_INTEGER => SqlType::Integer,
        DUCKDB_TYPE_DUCKDB_TYPE_BIGINT => SqlType::BigInt,
        DUCKDB_TYPE_DUCKDB_TYPE_UTINYINT => SqlType::UTinyInt,
        DUCKDB_TYPE_DUCKDB_TYPE_USMALLINT => SqlType::USmallInt,
        DUCKDB_TYPE_DUCKDB_TYPE_UINTEGER => SqlType::UInteger,
        DUCKDB_TYPE_DUCKDB_TYPE_UBIGINT => SqlType::UBigInt,
        DUCKDB_TYPE_DUCKDB_TYPE_FLOAT => SqlType::Float,
        DUCKDB_TYPE_DUCKDB_TYPE_DOUBLE => SqlType::Double,
        DUCKDB_TYPE_DUCKDB_TYPE_DECIMAL => {
            SqlType::Decimal { width: unsafe { duckdb_decimal_width(t) }, scale: unsafe { duckdb_decimal_scale(t) } }
        }
        DUCKDB_TYPE_DUCKDB_TYPE_VARCHAR => SqlType::Varchar,
        DUCKDB_TYPE_DUCKDB_TYPE_BLOB => SqlType::Blob,
        DUCKDB_TYPE_DUCKDB_TYPE_DATE => SqlType::Date,
        DUCKDB_TYPE_DUCKDB_TYPE_TIME => SqlType::Time,
        DUCKDB_TYPE_DUCKDB_TYPE_TIME_NS => SqlType::TimeNs,
        DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_S => SqlType::TimestampS,
        DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_MS => SqlType::TimestampMs,
        DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP => SqlType::Timestamp,
        DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_NS => SqlType::TimestampNs,
        DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_TZ => SqlType::TimestampTz,
        DUCKDB_TYPE_DUCKDB_TYPE_TIMESTAMP_TZ_NS => SqlType::TimestampTzNs,
        DUCKDB_TYPE_DUCKDB_TYPE_INTERVAL => SqlType::Interval,
        DUCKDB_TYPE_DUCKDB_TYPE_ENUM => {
            let n = unsafe { duckdb_enum_dictionary_size(t) };
            SqlType::Enum((0..n as idx_t).map(|i| unsafe { take_string(duckdb_enum_dictionary_value(t, i)) }).collect())
        }
        DUCKDB_TYPE_DUCKDB_TYPE_LIST => {
            let child = OwnedType(unsafe { duckdb_list_type_child_type(t) });
            SqlType::List(Box::new(unsafe { sql_type(child.0) }))
        }
        DUCKDB_TYPE_DUCKDB_TYPE_ARRAY => {
            let child = OwnedType(unsafe { duckdb_array_type_child_type(t) });
            let n = unsafe { duckdb_array_type_array_size(t) } as usize;
            SqlType::Array(Box::new(unsafe { sql_type(child.0) }), n)
        }
        DUCKDB_TYPE_DUCKDB_TYPE_MAP => {
            let k = OwnedType(unsafe { duckdb_map_type_key_type(t) });
            let v = OwnedType(unsafe { duckdb_map_type_value_type(t) });
            SqlType::Map(Box::new(unsafe { sql_type(k.0) }), Box::new(unsafe { sql_type(v.0) }))
        }
        DUCKDB_TYPE_DUCKDB_TYPE_STRUCT => {
            let n = unsafe { duckdb_struct_type_child_count(t) };
            SqlType::Struct(
                (0..n)
                    .map(|i| {
                        let name = unsafe { take_string(duckdb_struct_type_child_name(t, i)) };
                        let child = OwnedType(unsafe { duckdb_struct_type_child_type(t, i) });
                        (name, unsafe { sql_type(child.0) })
                    })
                    .collect(),
            )
        }
        other => SqlType::Unsupported(type_name(other).to_string()),
    }
}

/// The SQL name of a type DuckDB has but [`SqlType`] does not model.
fn type_name(id: ffi::DUCKDB_TYPE) -> &'static str {
    use ffi::*;
    match id {
        DUCKDB_TYPE_DUCKDB_TYPE_HUGEINT => "HUGEINT",
        DUCKDB_TYPE_DUCKDB_TYPE_UHUGEINT => "UHUGEINT",
        DUCKDB_TYPE_DUCKDB_TYPE_UUID => "UUID",
        DUCKDB_TYPE_DUCKDB_TYPE_UNION => "UNION",
        DUCKDB_TYPE_DUCKDB_TYPE_BIT => "BIT",
        DUCKDB_TYPE_DUCKDB_TYPE_TIME_TZ => "TIME WITH TIME ZONE",
        DUCKDB_TYPE_DUCKDB_TYPE_BIGNUM => "BIGNUM",
        DUCKDB_TYPE_DUCKDB_TYPE_VARIANT => "VARIANT",
        DUCKDB_TYPE_DUCKDB_TYPE_GEOMETRY => "GEOMETRY",
        DUCKDB_TYPE_DUCKDB_TYPE_SQLNULL => "NULL",
        _ => "an unknown type",
    }
}

/// An option or parameter value, as plain Rust data.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<Val>),
}

/// Reads a DuckDB value (borrowed).
///
/// # Safety
/// `v` is null or a valid value.
pub unsafe fn val(v: ffi::duckdb_value) -> Val {
    use ffi::*;
    if v.is_null() || unsafe { duckdb_is_null_value(v) } {
        return Val::Null;
    }
    let t = unsafe { duckdb_get_value_type(v) };
    match unsafe { duckdb_get_type_id(t) } {
        DUCKDB_TYPE_DUCKDB_TYPE_BOOLEAN => Val::Bool(unsafe { duckdb_get_bool(v) }),
        DUCKDB_TYPE_DUCKDB_TYPE_TINYINT
        | DUCKDB_TYPE_DUCKDB_TYPE_SMALLINT
        | DUCKDB_TYPE_DUCKDB_TYPE_INTEGER
        | DUCKDB_TYPE_DUCKDB_TYPE_BIGINT
        | DUCKDB_TYPE_DUCKDB_TYPE_UTINYINT
        | DUCKDB_TYPE_DUCKDB_TYPE_USMALLINT
        | DUCKDB_TYPE_DUCKDB_TYPE_UINTEGER
        | DUCKDB_TYPE_DUCKDB_TYPE_INTEGER_LITERAL => Val::Int(unsafe { duckdb_get_int64(v) }),
        DUCKDB_TYPE_DUCKDB_TYPE_LIST | DUCKDB_TYPE_DUCKDB_TYPE_ARRAY => {
            let n = unsafe { duckdb_get_list_size(v) };
            Val::List((0..n).map(|i| unsafe { owned_val(duckdb_get_list_child(v, i)) }).collect())
        }
        DUCKDB_TYPE_DUCKDB_TYPE_STRUCT => {
            let n = unsafe { duckdb_struct_type_child_count(t) };
            Val::List((0..n).map(|i| unsafe { owned_val(duckdb_get_struct_child(v, i)) }).collect())
        }
        _ => Val::Str(unsafe { take_string(duckdb_get_varchar(v)) }),
    }
}

/// Reads and destroys a value DuckDB handed over.
///
/// # Safety
/// `v` is null or a valid value the caller owns.
pub unsafe fn owned_val(mut v: ffi::duckdb_value) -> Val {
    let out = unsafe { val(v) };
    if !v.is_null() {
        unsafe { ffi::duckdb_destroy_value(&mut v) };
    }
    out
}

/// The named members of a STRUCT value (COPY options), destroying it.
///
/// # Safety
/// `v` is null or a valid value the caller owns.
pub unsafe fn owned_struct(mut v: ffi::duckdb_value) -> Vec<(String, Val)> {
    use ffi::*;
    if v.is_null() {
        return vec![];
    }
    let mut out = vec![];
    unsafe {
        if !duckdb_is_null_value(v) {
            let t = duckdb_get_value_type(v);
            if duckdb_get_type_id(t) == DUCKDB_TYPE_DUCKDB_TYPE_STRUCT {
                for i in 0..duckdb_struct_type_child_count(t) {
                    let name = take_string(duckdb_struct_type_child_name(t, i));
                    out.push((name, owned_val(duckdb_get_struct_child(v, i))));
                }
            }
        }
        duckdb_destroy_value(&mut v);
    }
    out
}

/// Runs a callback body, turning errors and panics into a message.
pub fn guard<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => Err(match (p.downcast_ref::<&str>(), p.downcast_ref::<String>()) {
            (Some(s), _) => format!("internal error: {s}"),
            (_, Some(s)) => format!("internal error: {s}"),
            _ => "internal error".into(),
        }),
    }
}

/// Moves `value` to the heap for DuckDB to keep; [`drop_box`] frees it.
pub fn into_raw<T>(value: T) -> *mut c_void {
    Box::into_raw(Box::new(value)) as *mut c_void
}

/// The destructor DuckDB calls for what [`into_raw`] made.
///
/// # Safety
/// `p` came from `into_raw::<T>`.
pub unsafe extern "C" fn drop_box<T>(p: *mut c_void) {
    if !p.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(p as *mut T) })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_turns_panics_into_messages() {
        assert_eq!(guard(|| Ok::<_, String>(5)), Ok(5));
        assert_eq!(guard(|| Err::<(), _>("no".to_string())), Err("no".to_string()));
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        assert_eq!(guard::<()>(|| panic!("static text")), Err("internal error: static text".to_string()));
        let n = 3;
        assert_eq!(guard::<()>(|| panic!("formatted {n}")), Err("internal error: formatted 3".to_string()));
        assert_eq!(guard::<()>(|| std::panic::panic_any(7u8)), Err("internal error".to_string()));
        std::panic::set_hook(hook);
    }

    #[test]
    fn boxes_round_trip_and_strings_lose_nuls() {
        let p = into_raw(String::from("kept"));
        assert!(!p.is_null());
        assert_eq!(unsafe { &*(p as *const String) }, "kept");
        unsafe { drop_box::<String>(p) };
        unsafe { drop_box::<String>(std::ptr::null_mut()) };
        assert_eq!(c_string("a\0b").as_bytes(), b"a?b");
        assert_eq!(c_string("name").as_bytes(), b"name");
        assert_eq!(unsafe { take_string(std::ptr::null_mut()) }, "");
    }
}
