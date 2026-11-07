//! The filters DuckDB offers a C API v2 table function, as [`Predicate`]s for row-group skipping.
//!
//! What cannot be expressed is left out: a conjunct that cannot be read is dropped (the rest
//! still rules row groups out), a disjunction with a part that cannot be read is dropped
//! whole. The filters are never taken over: DuckDB keeps applying them to the rows.

use crate::prune::{Op, Predicate, Scalar};
use crate::types::SqlType;
use crate::v2::sys::{Type, Value};
use libduckdb_sys::v2 as s;
use s::DUCKDB_V2_EXPRESSION_TYPE as E;
use s::DUCKDB_V2_LOGICAL_TYPE_ID as Id;

fn kind(e: s::duckdb_v2_expression_handle) -> Option<E> {
    let mut t = E::DUCKDB_V2_EXPRESSION_TYPE_INVALID;
    call!(duckdb_v2_expression_get_type(e, &mut t)).ok()?;
    Some(t)
}

fn children(e: s::duckdb_v2_expression_handle) -> Option<Vec<s::duckdb_v2_expression_handle>> {
    let mut n = 0u64;
    call!(duckdb_v2_expression_get_child_count(e, &mut n)).ok()?;
    (0..n)
        .map(|i| {
            let mut c: s::duckdb_v2_expression_handle = std::ptr::null_mut();
            call!(duckdb_v2_expression_get_child(e, i, &mut c)).ok()?;
            Some(c)
        })
        .collect()
}

/// The DuckDB type id of the values of a column type whose statistics can be read.
fn id_of(t: &SqlType) -> Option<Id> {
    Some(match t {
        SqlType::TinyInt => Id::DUCKDB_V2_LOGICAL_TYPE_ID_TINYINT,
        SqlType::SmallInt => Id::DUCKDB_V2_LOGICAL_TYPE_ID_SMALLINT,
        SqlType::Integer => Id::DUCKDB_V2_LOGICAL_TYPE_ID_INTEGER,
        SqlType::BigInt => Id::DUCKDB_V2_LOGICAL_TYPE_ID_BIGINT,
        SqlType::UTinyInt => Id::DUCKDB_V2_LOGICAL_TYPE_ID_UTINYINT,
        SqlType::USmallInt => Id::DUCKDB_V2_LOGICAL_TYPE_ID_USMALLINT,
        SqlType::UInteger => Id::DUCKDB_V2_LOGICAL_TYPE_ID_UINTEGER,
        SqlType::UBigInt => Id::DUCKDB_V2_LOGICAL_TYPE_ID_UBIGINT,
        SqlType::Float => Id::DUCKDB_V2_LOGICAL_TYPE_ID_FLOAT,
        SqlType::Double => Id::DUCKDB_V2_LOGICAL_TYPE_ID_DOUBLE,
        SqlType::Varchar => Id::DUCKDB_V2_LOGICAL_TYPE_ID_VARCHAR,
        SqlType::Date => Id::DUCKDB_V2_LOGICAL_TYPE_ID_DATE,
        SqlType::TimestampMs => Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_MS,
        SqlType::Timestamp => Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP,
        SqlType::TimestampNs => Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_NS,
        SqlType::TimestampTz => Id::DUCKDB_V2_LOGICAL_TYPE_ID_TIMESTAMP_TZ,
        SqlType::Decimal { .. } => Id::DUCKDB_V2_LOGICAL_TYPE_ID_DECIMAL,
        _ => return None,
    })
}

macro_rules! get {
    ($f:ident, $v:expr, $t:ty) => {{
        let mut out: $t = Default::default();
        call!($f($v, &mut out)).ok()?;
        out
    }};
}

/// A constant of the type of a column, as a [`Scalar`]; `None` for NULL or another type.
fn scalar(e: s::duckdb_v2_expression_handle, column: &SqlType) -> Option<Scalar> {
    let mut v = Value(std::ptr::null_mut());
    call!(duckdb_v2_expression_constant_get_value(e, &mut v.0)).ok()?;
    if v.is_null().ok()? {
        return None;
    }
    let t: Type = v.logical_type().ok()?;
    if Some(t.id().ok()?) != id_of(column) {
        return None;
    }
    let h = v.0;
    Some(match column {
        SqlType::TinyInt => Scalar::Int(get!(duckdb_v2_value_get_tinyint, h, i8).into()),
        SqlType::SmallInt => Scalar::Int(get!(duckdb_v2_value_get_smallint, h, i16).into()),
        SqlType::Integer => Scalar::Int(get!(duckdb_v2_value_get_int, h, i32).into()),
        SqlType::BigInt => Scalar::Int(get!(duckdb_v2_value_get_bigint, h, i64).into()),
        SqlType::UTinyInt => Scalar::Int(get!(duckdb_v2_value_get_utinyint, h, u8).into()),
        SqlType::USmallInt => Scalar::Int(get!(duckdb_v2_value_get_usmallint, h, u16).into()),
        SqlType::UInteger => Scalar::Int(get!(duckdb_v2_value_get_uint, h, u32).into()),
        SqlType::UBigInt => Scalar::Int(get!(duckdb_v2_value_get_ubigint, h, u64).into()),
        SqlType::Float => Scalar::Float(get!(duckdb_v2_value_get_float, h, f32).into()),
        SqlType::Double => Scalar::Float(get!(duckdb_v2_value_get_double, h, f64)),
        SqlType::Varchar => Scalar::Bytes(v.varchar().ok()?.into_bytes()),
        SqlType::Date => Scalar::Int(get!(duckdb_v2_value_get_date, h, i32).into()),
        SqlType::TimestampMs => Scalar::Int(get!(duckdb_v2_value_get_timestamp_ms, h, i64).into()),
        SqlType::Timestamp => Scalar::Int(get!(duckdb_v2_value_get_timestamp, h, i64).into()),
        SqlType::TimestampNs => Scalar::Int(get!(duckdb_v2_value_get_timestamp_ns, h, i64).into()),
        SqlType::TimestampTz => Scalar::Int(get!(duckdb_v2_value_get_timestamp_tz, h, i64).into()),
        SqlType::Decimal { scale, .. } => {
            let mut x = s::duckdb_v2_hugeint_t { lower: 0, upper: 0 };
            let (mut width, mut got_scale) = (0u8, 0u8);
            call!(duckdb_v2_value_get_decimal(h, &mut x, &mut width, &mut got_scale)).ok()?;
            if got_scale != *scale {
                return None;
            }
            Scalar::Int((i128::from(x.upper) << 64) | i128::from(x.lower))
        }
        _ => return None,
    })
}

/// What a column reference of a filter names: the result column of `read_tparquet`.
pub struct Columns<'a> {
    /// For each column of the scan at optimization time (what a reference's index indexes),
    /// the result column it is.
    pub scan: &'a [u64],
    /// The types of the file columns (the first result columns).
    pub types: &'a [SqlType],
}

impl Columns<'_> {
    fn column(&self, e: s::duckdb_v2_expression_handle) -> Option<usize> {
        if kind(e)? != E::DUCKDB_V2_EXPRESSION_TYPE_BOUND_COLUMN_REF {
            return None;
        }
        let mut i = 0u64;
        call!(duckdb_v2_expression_column_ref_get_index(e, &mut i)).ok()?;
        let column = usize::try_from(*self.scan.get(i as usize)?).ok()?;
        (column < self.types.len()).then_some(column)
    }
}

fn compare_op(k: E) -> Option<Op> {
    Some(match k {
        E::DUCKDB_V2_EXPRESSION_TYPE_COMPARE_EQUAL => Op::Eq,
        E::DUCKDB_V2_EXPRESSION_TYPE_COMPARE_NOTEQUAL => Op::Ne,
        E::DUCKDB_V2_EXPRESSION_TYPE_COMPARE_LESSTHAN => Op::Lt,
        E::DUCKDB_V2_EXPRESSION_TYPE_COMPARE_LESSTHANOREQUALTO => Op::Le,
        E::DUCKDB_V2_EXPRESSION_TYPE_COMPARE_GREATERTHAN => Op::Gt,
        E::DUCKDB_V2_EXPRESSION_TYPE_COMPARE_GREATERTHANOREQUALTO => Op::Ge,
        _ => return None,
    })
}

/// The predicate a filter expression is, as far as it can be expressed.
pub fn predicate(e: s::duckdb_v2_expression_handle, columns: &Columns) -> Option<Predicate> {
    let k = kind(e)?;
    let cs = children(e)?;
    let constant = |c: s::duckdb_v2_expression_handle| kind(c) == Some(E::DUCKDB_V2_EXPRESSION_TYPE_VALUE_CONSTANT);
    match k {
        E::DUCKDB_V2_EXPRESSION_TYPE_CONJUNCTION_AND => {
            let parts: Vec<Predicate> = cs.iter().filter_map(|c| predicate(*c, columns)).collect();
            (!parts.is_empty()).then_some(Predicate::And(parts))
        }
        E::DUCKDB_V2_EXPRESSION_TYPE_CONJUNCTION_OR => {
            cs.iter().map(|c| predicate(*c, columns)).collect::<Option<Vec<_>>>().map(Predicate::Or)
        }
        E::DUCKDB_V2_EXPRESSION_TYPE_OPERATOR_IS_NULL => Some(Predicate::IsNull(columns.column(*cs.first()?)?)),
        E::DUCKDB_V2_EXPRESSION_TYPE_OPERATOR_IS_NOT_NULL => Some(Predicate::IsNotNull(columns.column(*cs.first()?)?)),
        E::DUCKDB_V2_EXPRESSION_TYPE_COMPARE_IN => {
            let (first, rest) = cs.split_first()?;
            let column = columns.column(*first)?;
            let values = rest
                .iter()
                .map(|c| constant(*c).then(|| scalar(*c, &columns.types[column])).flatten())
                .collect::<Option<Vec<_>>>()?;
            Some(Predicate::Or(
                values.into_iter().map(|value| Predicate::Compare { column, op: Op::Eq, value }).collect(),
            ))
        }
        k => {
            let op = compare_op(k)?;
            let [a, b] = cs.as_slice() else { return None };
            let (column, value, op) = match (columns.column(*a), columns.column(*b)) {
                (Some(c), None) if constant(*b) => (c, scalar(*b, &columns.types[c])?, op),
                (None, Some(c)) if constant(*a) => (c, scalar(*a, &columns.types[c])?, op.flip()),
                _ => return None,
            };
            Some(Predicate::Compare { column, op, value })
        }
    }
}
