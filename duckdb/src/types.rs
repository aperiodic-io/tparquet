//! DuckDB column types and their Arrow counterparts, in both directions.
//!
//! - Reading: every Arrow type the Parquet reader produces maps to the DuckDB type that keeps its
//!   values exactly ([`SqlType::from_arrow`]); [`normalize`] then reshapes the few arrays whose
//!   layout the `duckdb` crate's Arrow-to-vector writer would not copy exactly.
//! - Writing (`COPY ... TO`): every DuckDB type a COPY can hand over maps to the Arrow type the
//!   Parquet writer stores ([`SqlType::to_arrow`]).
//!
//! This module has no DuckDB calls, so all of it is unit tested without a database.

use arrow_array::{
    Array, ArrayRef, Decimal128Array, DurationMicrosecondArray, FixedSizeListArray, LargeListArray, ListArray,
    MapArray, StructArray,
};
use arrow_cast::cast;
use arrow_schema::{DataType, Field, FieldRef, Fields, TimeUnit};
use std::fmt;
use std::sync::Arc;

/// The time zone written for `TIMESTAMPTZ` columns: DuckDB stores instants in UTC, and `UTC`
/// is what the Parquet reader gives a timestamp with `isAdjustedToUTC` (so the file reads the
/// same with and without its embedded Arrow schema).
pub const UTC: &str = "UTC";
/// The name Parquet gives a list element (`LogicalTypes.md`).
pub const LIST_ELEMENT: &str = "element";
/// The name Parquet gives the entries of a map.
pub const MAP_ENTRIES: &str = "key_value";

/// A DuckDB column type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlType {
    Boolean,
    TinyInt,
    SmallInt,
    Integer,
    BigInt,
    UTinyInt,
    USmallInt,
    UInteger,
    UBigInt,
    Float,
    Double,
    /// `DECIMAL(width, scale)`, width 1..=38.
    Decimal {
        width: u8,
        scale: u8,
    },
    Varchar,
    Blob,
    Date,
    /// `TIME`: microseconds since midnight.
    Time,
    /// `TIME_NS`: nanoseconds since midnight.
    TimeNs,
    TimestampS,
    TimestampMs,
    /// `TIMESTAMP`: microseconds.
    Timestamp,
    TimestampNs,
    /// `TIMESTAMPTZ`: microseconds, UTC.
    TimestampTz,
    /// `TIMESTAMPTZ_NS`: nanoseconds, UTC.
    TimestampTzNs,
    Interval,
    /// An `ENUM` with these values; read from DuckDB only.
    Enum(Vec<String>),
    List(Box<SqlType>),
    /// A fixed-size `ARRAY`.
    Array(Box<SqlType>, usize),
    Struct(Vec<(String, SqlType)>),
    Map(Box<SqlType>, Box<SqlType>),
    /// A DuckDB type this extension cannot write, by name.
    Unsupported(String),
}

impl fmt::Display for SqlType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Boolean => "BOOLEAN",
            Self::TinyInt => "TINYINT",
            Self::SmallInt => "SMALLINT",
            Self::Integer => "INTEGER",
            Self::BigInt => "BIGINT",
            Self::UTinyInt => "UTINYINT",
            Self::USmallInt => "USMALLINT",
            Self::UInteger => "UINTEGER",
            Self::UBigInt => "UBIGINT",
            Self::Float => "FLOAT",
            Self::Double => "DOUBLE",
            Self::Decimal { width, scale } => return write!(f, "DECIMAL({width},{scale})"),
            Self::Varchar => "VARCHAR",
            Self::Blob => "BLOB",
            Self::Date => "DATE",
            Self::Time => "TIME",
            Self::TimeNs => "TIME_NS",
            Self::TimestampS => "TIMESTAMP_S",
            Self::TimestampMs => "TIMESTAMP_MS",
            Self::Timestamp => "TIMESTAMP",
            Self::TimestampNs => "TIMESTAMP_NS",
            Self::TimestampTz => "TIMESTAMP WITH TIME ZONE",
            Self::TimestampTzNs => "TIMESTAMPTZ_NS",
            Self::Interval => "INTERVAL",
            Self::Enum(values) => {
                f.write_str("ENUM(")?;
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "'{}'", v.replace('\'', "''"))?;
                }
                return f.write_str(")");
            }
            Self::List(t) => return write!(f, "{t}[]"),
            Self::Array(t, n) => return write!(f, "{t}[{n}]"),
            Self::Struct(fields) => {
                f.write_str("STRUCT(")?;
                for (i, (name, t)) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "\"{}\" {t}", name.replace('"', "\"\""))?;
                }
                return f.write_str(")");
            }
            Self::Map(k, v) => return write!(f, "MAP({k}, {v})"),
            Self::Unsupported(name) => name,
        };
        f.write_str(name)
    }
}

/// The names DuckDB gives columns (or struct fields) named `names`: names are case-insensitive
/// there, so a repeated name gets `_1`, `_2`, ... (the first suffix that is free), as
/// `QueryResult::DeduplicateColumns` does for `read_parquet`.
pub fn deduplicate(names: Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let total = names.len();
    let mut out = Vec::with_capacity(total);
    for name in names {
        let low = name.to_ascii_lowercase();
        let Some(&count) = seen.get(&low) else {
            seen.insert(low, 1);
            out.push(name);
            continue;
        };
        // the first free suffix from `count` on: one of the next `total` is free, as each name
        // before this one took one name
        let free = |n: &usize| !seen.contains_key(&format!("{name}_{n}").to_ascii_lowercase());
        let suffix = (count..).take(total).find(free).unwrap_or(count);
        let renamed = format!("{name}_{suffix}");
        seen.insert(low, suffix);
        seen.insert(renamed.to_ascii_lowercase(), 1);
        out.push(renamed);
    }
    out
}

/// Largest DuckDB decimal width.
pub const MAX_DECIMAL_WIDTH: u8 = 38;

fn decimal(width: u8, scale: i8) -> Result<SqlType, String> {
    if width == 0 || width > MAX_DECIMAL_WIDTH {
        return Err(format!("DuckDB decimals have 1 to {MAX_DECIMAL_WIDTH} digits, not {width}"));
    }
    if scale < 0 || scale as u8 > width {
        return Err(format!("DuckDB cannot represent a decimal of width {width} with scale {scale}"));
    }
    Ok(SqlType::Decimal { width, scale: scale as u8 })
}

impl SqlType {
    /// The DuckDB type of a column the Parquet reader returns as `field`.
    pub fn from_arrow(field: &Field) -> Result<Self, String> {
        Self::from_arrow_type(field.data_type()).map_err(|e| format!("column \"{}\": {e}", field.name()))
    }

    /// The DuckDB type of an Arrow type, see [`SqlType::from_arrow`].
    pub fn from_arrow_type(data_type: &DataType) -> Result<Self, String> {
        Ok(match data_type {
            DataType::Boolean => Self::Boolean,
            DataType::Int8 => Self::TinyInt,
            DataType::Int16 => Self::SmallInt,
            DataType::Int32 => Self::Integer,
            DataType::Int64 => Self::BigInt,
            DataType::UInt8 => Self::UTinyInt,
            DataType::UInt16 => Self::USmallInt,
            DataType::UInt32 => Self::UInteger,
            DataType::UInt64 => Self::UBigInt,
            // DuckDB has no half float; FLOAT holds every value exactly
            DataType::Float16 | DataType::Float32 => Self::Float,
            DataType::Float64 => Self::Double,
            DataType::Decimal32(w, s)
            | DataType::Decimal64(w, s)
            | DataType::Decimal128(w, s)
            | DataType::Decimal256(w, s) => decimal(*w, *s)?,
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => Self::Varchar,
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView | DataType::FixedSizeBinary(_) => {
                Self::Blob
            }
            DataType::Date32 | DataType::Date64 => Self::Date,
            DataType::Time32(_) | DataType::Time64(TimeUnit::Microsecond) => Self::Time,
            DataType::Time64(_) => Self::TimeNs,
            DataType::Timestamp(unit, None) => match unit {
                TimeUnit::Second => Self::TimestampS,
                TimeUnit::Millisecond => Self::TimestampMs,
                TimeUnit::Microsecond => Self::Timestamp,
                TimeUnit::Nanosecond => Self::TimestampNs,
            },
            // DuckDB keeps instants in UTC at microsecond or nanosecond precision; the time zone
            // of the column is a display setting there (`SET TimeZone`)
            DataType::Timestamp(TimeUnit::Nanosecond, Some(_)) => Self::TimestampTzNs,
            DataType::Timestamp(_, Some(_)) => Self::TimestampTz,
            DataType::Duration(_) | DataType::Interval(_) => Self::Interval,
            DataType::List(f) | DataType::LargeList(f) => Self::List(Box::new(Self::from_arrow_type(f.data_type())?)),
            DataType::FixedSizeList(f, n) => {
                let n = usize::try_from(*n).map_err(|_| format!("invalid fixed-size list length {n}"))?;
                Self::Array(Box::new(Self::from_arrow_type(f.data_type())?), n)
            }
            DataType::Struct(fields) => {
                let names = deduplicate(fields.iter().map(|f| f.name().clone()).collect());
                let types =
                    fields.iter().map(|f| Self::from_arrow_type(f.data_type())).collect::<Result<Vec<_>, _>>()?;
                Self::Struct(names.into_iter().zip(types).collect())
            }
            DataType::Map(entries, _) => match entries.data_type() {
                DataType::Struct(kv) if kv.len() == 2 => Self::Map(
                    Box::new(Self::from_arrow_type(kv[0].data_type())?),
                    Box::new(Self::from_arrow_type(kv[1].data_type())?),
                ),
                other => return Err(format!("invalid map entries {other}")),
            },
            DataType::Dictionary(_, values) => Self::from_arrow_type(values)?,
            other => return Err(format!("the Arrow type {other} has no DuckDB equivalent")),
        })
    }

    /// The Arrow type a `COPY ... TO` writes for a column of this type.
    pub fn to_arrow(&self) -> Result<DataType, String> {
        Ok(match self {
            Self::Boolean => DataType::Boolean,
            Self::TinyInt => DataType::Int8,
            Self::SmallInt => DataType::Int16,
            Self::Integer => DataType::Int32,
            Self::BigInt => DataType::Int64,
            Self::UTinyInt => DataType::UInt8,
            Self::USmallInt => DataType::UInt16,
            Self::UInteger => DataType::UInt32,
            Self::UBigInt => DataType::UInt64,
            Self::Float => DataType::Float32,
            Self::Double => DataType::Float64,
            // the Parquet writer stores widths up to 9 as INT32 and up to 18 as INT64
            Self::Decimal { width, scale } => DataType::Decimal128(*width, *scale as i8),
            Self::Varchar | Self::Enum(_) => DataType::Utf8,
            Self::Blob => DataType::Binary,
            Self::Date => DataType::Date32,
            Self::Time => DataType::Time64(TimeUnit::Microsecond),
            Self::TimeNs => DataType::Time64(TimeUnit::Nanosecond),
            // Parquet has no seconds unit: seconds are written as milliseconds
            Self::TimestampS | Self::TimestampMs => DataType::Timestamp(TimeUnit::Millisecond, None),
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
            Self::TimestampNs => DataType::Timestamp(TimeUnit::Nanosecond, None),
            Self::TimestampTz => DataType::Timestamp(TimeUnit::Microsecond, Some(UTC.into())),
            Self::TimestampTzNs => DataType::Timestamp(TimeUnit::Nanosecond, Some(UTC.into())),
            Self::List(t) => DataType::List(Arc::new(Field::new(LIST_ELEMENT, t.to_arrow()?, true))),
            Self::Array(t, n) => {
                let n = i32::try_from(*n).map_err(|_| format!("ARRAY length {n} is too large"))?;
                DataType::FixedSizeList(Arc::new(Field::new(LIST_ELEMENT, t.to_arrow()?, true)), n)
            }
            Self::Struct(fields) => DataType::Struct(
                fields
                    .iter()
                    .map(|(name, t)| Ok(Field::new(name, t.to_arrow()?, true)))
                    .collect::<Result<Fields, String>>()?,
            ),
            Self::Map(k, v) => DataType::Map(
                Arc::new(Field::new(
                    MAP_ENTRIES,
                    DataType::Struct(Fields::from(vec![
                        Field::new("key", k.to_arrow()?, false),
                        Field::new("value", v.to_arrow()?, true),
                    ])),
                    false,
                )),
                false,
            ),
            Self::Interval | Self::Unsupported(_) => {
                return Err(format!("columns of type {self} cannot be written to Parquet by this extension"));
            }
        })
    }
}

/// DuckDB's `interval_t`, the layout of an `INTERVAL` vector's values.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Interval {
    pub months: i32,
    pub days: i32,
    pub micros: i64,
}

/// The DuckDB intervals of an Arrow interval or duration array, one per row (NULL rows give
/// zero). Durations become microseconds (nanoseconds are truncated toward zero, as DuckDB's own
/// Arrow import does); one that does not fit in 64-bit microseconds is an error.
///
/// The `duckdb` crate writes intervals through nanoseconds and without their NULLs (it rebuilds
/// the array from its values), so `crate::read` writes them again with these.
pub fn intervals(array: &dyn Array) -> Result<Vec<Interval>, String> {
    use arrow_array::cast::AsArray;
    use arrow_array::types::*;
    use arrow_schema::IntervalUnit;
    fn each<T: arrow_array::ArrowPrimitiveType>(
        array: &dyn Array,
        f: impl Fn(T::Native) -> Result<Interval, String>,
    ) -> Result<Vec<Interval>, String> {
        let a = array.as_primitive::<T>();
        (0..a.len()).map(|i| if a.is_valid(i) { f(a.value(i)) } else { Ok(Interval::default()) }).collect()
    }
    let micros = |v: i64, per_unit: i64, unit: &str| {
        let micros =
            v.checked_mul(per_unit).ok_or_else(|| format!("the duration {v} {unit} does not fit in an INTERVAL"))?;
        Ok(Interval { micros, ..Default::default() })
    };
    match array.data_type() {
        DataType::Interval(IntervalUnit::YearMonth) => {
            each::<IntervalYearMonthType>(array, |months| Ok(Interval { months, ..Default::default() }))
        }
        DataType::Interval(IntervalUnit::DayTime) => each::<IntervalDayTimeType>(array, |v| {
            Ok(Interval { months: 0, days: v.days, micros: v.milliseconds as i64 * 1000 })
        }),
        DataType::Interval(IntervalUnit::MonthDayNano) => each::<IntervalMonthDayNanoType>(array, |v| {
            Ok(Interval { months: v.months, days: v.days, micros: v.nanoseconds / 1000 })
        }),
        DataType::Duration(TimeUnit::Second) => each::<DurationSecondType>(array, |v| micros(v, 1_000_000, "s")),
        DataType::Duration(TimeUnit::Millisecond) => each::<DurationMillisecondType>(array, |v| micros(v, 1000, "ms")),
        DataType::Duration(TimeUnit::Microsecond) => each::<DurationMicrosecondType>(array, |v| micros(v, 1, "us")),
        DataType::Duration(TimeUnit::Nanosecond) => {
            each::<DurationNanosecondType>(array, |v| Ok(Interval { micros: v / 1000, ..Default::default() }))
        }
        other => Err(format!("{other} is not an interval")),
    }
}

/// The type to read a column of `data_type` as: every interval becomes `Interval(MonthDayNano)`.
///
/// Parquet stores an `INTERVAL` as months, days and milliseconds, and the Parquet reader takes it
/// for `Interval(DayTime)`, without the months, unless it is asked for `MonthDayNano` (or the
/// file's own Arrow schema says `YearMonth`, which drops the days). The other types are kept.
pub fn whole_intervals(data_type: &DataType) -> DataType {
    let field = |f: &FieldRef| with_type(f, whole_intervals(f.data_type()));
    match data_type {
        DataType::Interval(_) => DataType::Interval(arrow_schema::IntervalUnit::MonthDayNano),
        DataType::List(f) => DataType::List(field(f)),
        DataType::LargeList(f) => DataType::LargeList(field(f)),
        DataType::FixedSizeList(f, n) => DataType::FixedSizeList(field(f), *n),
        DataType::Map(f, sorted) => DataType::Map(field(f), *sorted),
        DataType::Struct(fields) => DataType::Struct(fields.iter().map(field).collect()),
        other => other.clone(),
    }
}

/// What takes the arrays [`normalize`] reshapes to DuckDB vectors.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Target {
    /// The `duckdb` crate's Arrow-to-vector writer (the C API v1 build, [`crate::read`]).
    #[default]
    DuckdbCrate,
    /// DuckDB's own Arrow importer (the C API v2 build, [`crate::v2::read`]).
    ArrowImporter,
}

/// Whether [`normalize`] changes (or checks) arrays of this type for `target`.
fn needs_normalize(data_type: &DataType, target: Target) -> bool {
    let crate_writer = target == Target::DuckdbCrate;
    match data_type {
        DataType::Dictionary(_, _) | DataType::Float16 | DataType::Decimal256(_, _) => true,
        DataType::Time64(TimeUnit::Nanosecond) | DataType::Timestamp(TimeUnit::Nanosecond, Some(_)) => crate_writer,
        DataType::Decimal128(_, _) | DataType::Duration(TimeUnit::Second | TimeUnit::Millisecond) => !crate_writer,
        DataType::List(f) | DataType::LargeList(f) | DataType::FixedSizeList(f, _) | DataType::Map(f, _) => {
            needs_normalize(f.data_type(), target)
        }
        DataType::Struct(fields) => fields.iter().any(|f| needs_normalize(f.data_type(), target)),
        _ => false,
    }
}

/// Changes the data type of `array` without touching its buffers.
fn reinterpret(array: &ArrayRef, data_type: DataType) -> Result<ArrayRef, String> {
    let data = array.to_data().into_builder().data_type(data_type).build().map_err(|e| e.to_string())?;
    Ok(arrow_array::make_array(data))
}

fn with_type(field: &FieldRef, data_type: DataType) -> FieldRef {
    Arc::new(field.as_ref().clone().with_data_type(data_type))
}

/// Reshapes an array the Parquet reader returned so that the `duckdb` crate's writer copies it
/// exactly into a vector of [`SqlType::from_arrow_type`]`(array.data_type())`:
///
/// - dictionaries are decoded, half floats widened to `FLOAT`, 256-bit decimals (of at most 38
///   digits) narrowed to 128 bits;
/// - for the `duckdb` crate ([`Target::DuckdbCrate`]), nanosecond times and time-zoned
///   timestamps keep their 64-bit values but lose the Arrow type that would make the crate round
///   them to microseconds (DuckDB stores them in `TIME_NS` and `TIMESTAMPTZ_NS`);
/// - for DuckDB's Arrow importer ([`Target::ArrowImporter`]), durations in seconds and
///   milliseconds become microseconds, with the error of [`intervals`] for one that does not fit
///   (the importer's own error does not name the value), and 128-bit decimals are checked
///   against their precision (the importer would take a wider value; the crate refuses it);
/// - lists, maps and structs are rebuilt around their normalized children.
pub fn normalize(array: &ArrayRef, target: Target) -> Result<ArrayRef, String> {
    if !needs_normalize(array.data_type(), target) {
        return Ok(array.clone());
    }
    let cast_to = |t: &DataType| cast(array, t).map_err(|e| e.to_string());
    let normalize = |a: &ArrayRef| normalize(a, target);
    match array.data_type() {
        DataType::Dictionary(_, values) => normalize(&cast_to(values)?),
        DataType::Float16 => cast_to(&DataType::Float32),
        DataType::Decimal256(w, s) => normalize(&cast_to(&DataType::Decimal128(*w, *s))?),
        DataType::Time64(TimeUnit::Nanosecond) => reinterpret(array, DataType::Int64),
        DataType::Timestamp(TimeUnit::Nanosecond, Some(_)) => {
            reinterpret(array, DataType::Timestamp(TimeUnit::Nanosecond, None))
        }
        DataType::Decimal128(width, _) => {
            let a = array.as_any().downcast_ref::<Decimal128Array>().ok_or("not a decimal array")?;
            let digits = |v: i128| v.unsigned_abs().checked_ilog10().map_or(1, |l| l + 1);
            match a.iter().flatten().map(digits).find(|&d| d > *width as u32) {
                // the message of the `duckdb` crate, which the C API v1 build reads with
                Some(d) => Err(format!("decimal value has {d} digits, exceeding width {width}")),
                None => Ok(array.clone()),
            }
        }
        DataType::Duration(_) => {
            let micros = intervals(array.as_ref())?.into_iter().map(|i| i.micros).collect::<Vec<_>>();
            Ok(Arc::new(DurationMicrosecondArray::new(micros.into(), array.nulls().cloned())))
        }
        DataType::List(f) => {
            let a = array.as_any().downcast_ref::<ListArray>().ok_or("not a list array")?;
            let values = normalize(a.values())?;
            let field = with_type(f, values.data_type().clone());
            Ok(Arc::new(
                ListArray::try_new(field, a.offsets().clone(), values, a.nulls().cloned())
                    .map_err(|e| e.to_string())?,
            ))
        }
        DataType::LargeList(f) => {
            let a = array.as_any().downcast_ref::<LargeListArray>().ok_or("not a large list array")?;
            let values = normalize(a.values())?;
            let field = with_type(f, values.data_type().clone());
            Ok(Arc::new(
                LargeListArray::try_new(field, a.offsets().clone(), values, a.nulls().cloned())
                    .map_err(|e| e.to_string())?,
            ))
        }
        DataType::FixedSizeList(f, n) => {
            let a = array.as_any().downcast_ref::<FixedSizeListArray>().ok_or("not a fixed-size list array")?;
            let values = normalize(a.values())?;
            let field = with_type(f, values.data_type().clone());
            Ok(Arc::new(FixedSizeListArray::try_new(field, *n, values, a.nulls().cloned()).map_err(|e| e.to_string())?))
        }
        DataType::Map(f, sorted) => {
            let a = array.as_any().downcast_ref::<MapArray>().ok_or("not a map array")?;
            let entries: ArrayRef = Arc::new(a.entries().clone());
            let entries = normalize(&entries)?;
            let entries = entries.as_any().downcast_ref::<StructArray>().ok_or("map entries are not a struct")?;
            let field = with_type(f, entries.data_type().clone());
            Ok(Arc::new(
                MapArray::try_new(field, a.offsets().clone(), entries.clone(), a.nulls().cloned(), *sorted)
                    .map_err(|e| e.to_string())?,
            ))
        }
        DataType::Struct(fields) => {
            let a = array.as_any().downcast_ref::<StructArray>().ok_or("not a struct array")?;
            let columns = a.columns().iter().map(normalize).collect::<Result<Vec<_>, _>>()?;
            let fields: Fields =
                fields.iter().zip(&columns).map(|(f, c)| with_type(f, c.data_type().clone())).collect();
            Ok(Arc::new(StructArray::try_new(fields, columns, a.nulls().cloned()).map_err(|e| e.to_string())?))
        }
        other => Err(format!("cannot normalize {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::builder::{Int32Builder, ListBuilder, MapBuilder, StringBuilder, StringDictionaryBuilder};
    use arrow_array::types::Int8Type;
    use arrow_array::*;
    use arrow_buffer::i256;
    use arrow_schema::IntervalUnit;

    fn ty(t: DataType) -> SqlType {
        SqlType::from_arrow_type(&t).unwrap()
    }

    fn item(t: DataType) -> FieldRef {
        Arc::new(Field::new("item", t, true))
    }

    /// The list element as Parquet names it.
    fn element(t: DataType) -> FieldRef {
        Arc::new(Field::new("element", t, true))
    }

    #[test]
    fn integers_of_every_width_and_sign() {
        assert_eq!(ty(DataType::Boolean), SqlType::Boolean);
        assert_eq!(ty(DataType::Int8), SqlType::TinyInt);
        assert_eq!(ty(DataType::Int16), SqlType::SmallInt);
        assert_eq!(ty(DataType::Int32), SqlType::Integer);
        assert_eq!(ty(DataType::Int64), SqlType::BigInt);
        assert_eq!(ty(DataType::UInt8), SqlType::UTinyInt);
        assert_eq!(ty(DataType::UInt16), SqlType::USmallInt);
        assert_eq!(ty(DataType::UInt32), SqlType::UInteger);
        assert_eq!(ty(DataType::UInt64), SqlType::UBigInt);
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn repeated_names_get_the_suffixes_duckdb_gives_them() {
        assert_eq!(deduplicate(names(&["a", "b"])), names(&["a", "b"]));
        assert_eq!(deduplicate(names(&["A", "a"])), names(&["A", "a_1"]));
        assert_eq!(deduplicate(names(&["x", "x", "x"])), names(&["x", "x_1", "x_2"]));
        // a suffix the file already uses is skipped, case-insensitively
        assert_eq!(deduplicate(names(&["x", "x", "x_1", "X"])), names(&["x", "x_1", "x_1_1", "X_2"]));
        assert_eq!(deduplicate(names(&["id", "ID", "Id", "iD"])), names(&["id", "ID_1", "Id_2", "iD_3"]));
        assert_eq!(deduplicate(names(&["x_1", "x", "x", "X_1"])), names(&["x_1", "x", "x_2", "X_1_1"]));
        assert_eq!(deduplicate(vec![]), Vec::<String>::new());
        let t = DataType::Struct(Fields::from(vec![
            Field::new("a", DataType::Int8, true),
            Field::new("A", DataType::Utf8, true),
        ]));
        assert_eq!(ty(t), SqlType::Struct(vec![("a".into(), SqlType::TinyInt), ("A_1".into(), SqlType::Varchar)]));
    }

    #[test]
    fn intervals_and_durations_in_duckdb_units() {
        use arrow_buffer::{IntervalDayTime, IntervalMonthDayNano};
        let iv = |months, days, micros| Interval { months, days, micros };
        let zero = Interval::default();
        let a = IntervalYearMonthArray::from(vec![Some(14), None, Some(-1)]);
        assert_eq!(intervals(&a).unwrap(), vec![iv(14, 0, 0), zero, iv(-1, 0, 0)]);
        let a = IntervalDayTimeArray::from(vec![Some(IntervalDayTime::new(3, 14_706_789)), None]);
        assert_eq!(intervals(&a).unwrap(), vec![iv(0, 3, 14_706_789_000), zero]);
        let a = IntervalMonthDayNanoArray::from(vec![
            Some(IntervalMonthDayNano::new(1, 2, 3_999)),
            Some(IntervalMonthDayNano::new(-1, -2, -3_999)),
            None,
        ]);
        assert_eq!(intervals(&a).unwrap(), vec![iv(1, 2, 3), iv(-1, -2, -3), zero]);
        let us = |micros| iv(0, 0, micros);
        assert_eq!(intervals(&DurationSecondArray::from(vec![Some(-2), None])).unwrap(), vec![us(-2_000_000), zero]);
        assert_eq!(intervals(&DurationMillisecondArray::from(vec![7, -7])).unwrap(), vec![us(7000), us(-7000)]);
        assert_eq!(intervals(&DurationMicrosecondArray::from(vec![i64::MAX])).unwrap(), vec![us(i64::MAX)]);
        assert_eq!(intervals(&DurationNanosecondArray::from(vec![1999, -1999])).unwrap(), vec![us(1), us(-1)]);
        // the largest durations that fit, and the first that do not
        let s = i64::MAX / 1_000_000;
        assert_eq!(
            intervals(&DurationSecondArray::from(vec![s, -s])).unwrap(),
            vec![us(s * 1_000_000), us(-s * 1_000_000)]
        );
        assert_eq!(
            intervals(&DurationSecondArray::from(vec![s + 1])).unwrap_err(),
            format!("the duration {} s does not fit in an INTERVAL", s + 1)
        );
        let ms = i64::MAX / 1000;
        assert_eq!(intervals(&DurationMillisecondArray::from(vec![ms])).unwrap(), vec![us(ms * 1000)]);
        assert_eq!(
            intervals(&DurationMillisecondArray::from(vec![-ms - 1])).unwrap_err(),
            format!("the duration {} ms does not fit in an INTERVAL", -ms - 1)
        );
        // a NULL row is not converted
        let masked = DurationSecondArray::new(vec![i64::MAX, 1].into(), Some(vec![false, true].into()));
        assert_eq!(intervals(&masked).unwrap(), vec![zero, us(1_000_000)]);
        assert_eq!(intervals(&Int64Array::from(vec![1])).unwrap_err(), "Int64 is not an interval");
        assert_eq!(std::mem::size_of::<Interval>(), 16);
    }

    #[test]
    fn intervals_are_read_whole_at_any_depth() {
        use arrow_schema::IntervalUnit::*;
        let whole = DataType::Interval(MonthDayNano);
        for unit in [DayTime, YearMonth, MonthDayNano] {
            assert_eq!(whole_intervals(&DataType::Interval(unit)), whole);
        }
        let kv = |v: DataType| {
            Arc::new(Field::new(
                "key_value",
                DataType::Struct(Fields::from(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", v, true),
                ])),
                false,
            ))
        };
        let day_time = DataType::Interval(DayTime);
        let cases = [
            (DataType::List(element(day_time.clone())), DataType::List(element(whole.clone()))),
            (DataType::LargeList(item(day_time.clone())), DataType::LargeList(item(whole.clone()))),
            (DataType::FixedSizeList(item(day_time.clone()), 3), DataType::FixedSizeList(item(whole.clone()), 3)),
            (DataType::Map(kv(day_time.clone()), true), DataType::Map(kv(whole.clone()), true)),
            (
                DataType::Struct(Fields::from(vec![
                    Field::new("a", DataType::Int8, false),
                    Field::new("b", day_time.clone(), true),
                ])),
                DataType::Struct(Fields::from(vec![
                    Field::new("a", DataType::Int8, false),
                    Field::new("b", whole.clone(), true),
                ])),
            ),
        ];
        for (t, want) in cases {
            assert_eq!(whole_intervals(&t), want, "{t}");
        }
        for t in [DataType::Int64, DataType::Duration(TimeUnit::Second), DataType::List(item(DataType::Utf8))] {
            assert_eq!(whole_intervals(&t), t);
        }
    }

    #[test]
    fn floats_and_decimals() {
        assert_eq!(ty(DataType::Float16), SqlType::Float);
        assert_eq!(ty(DataType::Float32), SqlType::Float);
        assert_eq!(ty(DataType::Float64), SqlType::Double);
        for t in [
            DataType::Decimal32(9, 2),
            DataType::Decimal64(9, 2),
            DataType::Decimal128(9, 2),
            DataType::Decimal256(9, 2),
        ] {
            assert_eq!(ty(t), SqlType::Decimal { width: 9, scale: 2 });
        }
        assert_eq!(ty(DataType::Decimal128(38, 38)), SqlType::Decimal { width: 38, scale: 38 });
        assert_eq!(ty(DataType::Decimal128(1, 0)), SqlType::Decimal { width: 1, scale: 0 });
        assert_eq!(
            SqlType::from_arrow_type(&DataType::Decimal256(39, 2)).unwrap_err(),
            "DuckDB decimals have 1 to 38 digits, not 39"
        );
        assert!(SqlType::from_arrow_type(&DataType::Decimal128(0, 0)).is_err());
        assert_eq!(
            SqlType::from_arrow_type(&DataType::Decimal128(10, -2)).unwrap_err(),
            "DuckDB cannot represent a decimal of width 10 with scale -2"
        );
        assert!(SqlType::from_arrow_type(&DataType::Decimal128(5, 6)).is_err());
    }

    #[test]
    fn strings_and_binaries() {
        for t in [DataType::Utf8, DataType::LargeUtf8, DataType::Utf8View] {
            assert_eq!(ty(t), SqlType::Varchar);
        }
        for t in [DataType::Binary, DataType::LargeBinary, DataType::BinaryView, DataType::FixedSizeBinary(16)] {
            assert_eq!(ty(t), SqlType::Blob);
        }
        assert_eq!(ty(DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))), SqlType::Varchar);
    }

    #[test]
    fn temporal_types_keep_their_unit() {
        use TimeUnit::*;
        assert_eq!(ty(DataType::Date32), SqlType::Date);
        assert_eq!(ty(DataType::Date64), SqlType::Date);
        assert_eq!(ty(DataType::Time32(Second)), SqlType::Time);
        assert_eq!(ty(DataType::Time32(Millisecond)), SqlType::Time);
        assert_eq!(ty(DataType::Time64(Microsecond)), SqlType::Time);
        assert_eq!(ty(DataType::Time64(Nanosecond)), SqlType::TimeNs);
        assert_eq!(ty(DataType::Timestamp(Second, None)), SqlType::TimestampS);
        assert_eq!(ty(DataType::Timestamp(Millisecond, None)), SqlType::TimestampMs);
        assert_eq!(ty(DataType::Timestamp(Microsecond, None)), SqlType::Timestamp);
        assert_eq!(ty(DataType::Timestamp(Nanosecond, None)), SqlType::TimestampNs);
        for unit in [Second, Millisecond, Microsecond] {
            assert_eq!(ty(DataType::Timestamp(unit, Some("America/New_York".into()))), SqlType::TimestampTz);
        }
        assert_eq!(ty(DataType::Timestamp(Nanosecond, Some("UTC".into()))), SqlType::TimestampTzNs);
        assert_eq!(ty(DataType::Duration(Millisecond)), SqlType::Interval);
        assert_eq!(ty(DataType::Interval(IntervalUnit::DayTime)), SqlType::Interval);
    }

    #[test]
    fn nested_types() {
        assert_eq!(ty(DataType::List(item(DataType::Int64))), SqlType::List(Box::new(SqlType::BigInt)));
        assert_eq!(ty(DataType::LargeList(item(DataType::Utf8))), SqlType::List(Box::new(SqlType::Varchar)));
        assert_eq!(
            ty(DataType::FixedSizeList(item(DataType::Float32), 3)),
            SqlType::Array(Box::new(SqlType::Float), 3)
        );
        let s = DataType::Struct(Fields::from(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("tags", DataType::List(item(DataType::Int32)), true),
        ]));
        assert_eq!(
            ty(s),
            SqlType::Struct(vec![
                ("id".into(), SqlType::Integer),
                ("tags".into(), SqlType::List(Box::new(SqlType::Integer)))
            ])
        );
        let map = MapBuilder::new(None, StringBuilder::new(), Int32Builder::new()).finish();
        assert_eq!(ty(map.data_type().clone()), SqlType::Map(Box::new(SqlType::Varchar), Box::new(SqlType::Integer)));
        let bad_map = DataType::Map(Arc::new(Field::new("entries", DataType::Int32, false)), false);
        assert_eq!(SqlType::from_arrow_type(&bad_map).unwrap_err(), "invalid map entries Int32");
        let one_field = DataType::Map(
            Arc::new(Field::new(
                "entries",
                DataType::Struct(Fields::from(vec![Field::new("k", DataType::Int32, false)])),
                false,
            )),
            false,
        );
        assert!(SqlType::from_arrow_type(&one_field).is_err());
        assert!(SqlType::from_arrow_type(&DataType::List(item(DataType::Null))).is_err());
        assert!(SqlType::from_arrow_type(&DataType::FixedSizeList(item(DataType::Int8), -1)).is_err());
    }

    #[test]
    fn unsupported_types_name_the_column() {
        let e = SqlType::from_arrow(&Field::new("n", DataType::Null, true)).unwrap_err();
        assert_eq!(e, "column \"n\": the Arrow type Null has no DuckDB equivalent");
        assert_eq!(SqlType::from_arrow(&Field::new("x", DataType::Int8, true)), Ok(SqlType::TinyInt));
    }

    #[test]
    fn writes_every_supported_type() {
        use TimeUnit::*;
        let cases = [
            (SqlType::Boolean, DataType::Boolean),
            (SqlType::TinyInt, DataType::Int8),
            (SqlType::SmallInt, DataType::Int16),
            (SqlType::Integer, DataType::Int32),
            (SqlType::BigInt, DataType::Int64),
            (SqlType::UTinyInt, DataType::UInt8),
            (SqlType::USmallInt, DataType::UInt16),
            (SqlType::UInteger, DataType::UInt32),
            (SqlType::UBigInt, DataType::UInt64),
            (SqlType::Float, DataType::Float32),
            (SqlType::Double, DataType::Float64),
            (SqlType::Decimal { width: 18, scale: 4 }, DataType::Decimal128(18, 4)),
            (SqlType::Varchar, DataType::Utf8),
            (SqlType::Enum(vec!["a".into()]), DataType::Utf8),
            (SqlType::Blob, DataType::Binary),
            (SqlType::Date, DataType::Date32),
            (SqlType::Time, DataType::Time64(Microsecond)),
            (SqlType::TimeNs, DataType::Time64(Nanosecond)),
            (SqlType::TimestampS, DataType::Timestamp(Millisecond, None)),
            (SqlType::TimestampMs, DataType::Timestamp(Millisecond, None)),
            (SqlType::Timestamp, DataType::Timestamp(Microsecond, None)),
            (SqlType::TimestampNs, DataType::Timestamp(Nanosecond, None)),
            (SqlType::TimestampTz, DataType::Timestamp(Microsecond, Some("UTC".into()))),
            (SqlType::TimestampTzNs, DataType::Timestamp(Nanosecond, Some("UTC".into()))),
            (SqlType::List(Box::new(SqlType::Double)), DataType::List(element(DataType::Float64))),
            (SqlType::Array(Box::new(SqlType::Integer), 2), DataType::FixedSizeList(element(DataType::Int32), 2)),
        ];
        for (sql, arrow) in cases {
            assert_eq!(sql.to_arrow().unwrap(), arrow, "{sql}");
        }
        let s = SqlType::Struct(vec![("a".into(), SqlType::Integer), ("b".into(), SqlType::Varchar)]);
        assert_eq!(
            s.to_arrow().unwrap(),
            DataType::Struct(Fields::from(vec![
                Field::new("a", DataType::Int32, true),
                Field::new("b", DataType::Utf8, true)
            ]))
        );
        let m = SqlType::Map(Box::new(SqlType::Varchar), Box::new(SqlType::BigInt)).to_arrow().unwrap();
        let DataType::Map(entries, false) = &m else { panic!("{m}") };
        assert_eq!(entries.name(), "key_value");
        assert!(!entries.is_nullable());
        let DataType::Struct(kv) = entries.data_type() else { panic!() };
        assert_eq!((kv[0].name().as_str(), kv[0].data_type(), kv[0].is_nullable()), ("key", &DataType::Utf8, false));
        assert_eq!((kv[1].name().as_str(), kv[1].data_type(), kv[1].is_nullable()), ("value", &DataType::Int64, true));
        // the arrow types round-trip through the read mapping, but for seconds (stored as ms)
        // and enums (stored as strings)
        for t in [SqlType::BigInt, SqlType::TimestampTzNs, SqlType::Decimal { width: 9, scale: 2 }] {
            assert_eq!(SqlType::from_arrow_type(&t.to_arrow().unwrap()).unwrap(), t);
        }
    }

    #[test]
    fn refuses_types_it_cannot_write() {
        assert_eq!(
            SqlType::Unsupported("UUID".into()).to_arrow().unwrap_err(),
            "columns of type UUID cannot be written to Parquet by this extension"
        );
        assert!(SqlType::Interval.to_arrow().is_err());
        assert!(SqlType::List(Box::new(SqlType::Interval)).to_arrow().is_err());
        assert!(SqlType::Struct(vec![("i".into(), SqlType::Interval)]).to_arrow().is_err());
        assert!(SqlType::Map(Box::new(SqlType::Interval), Box::new(SqlType::Integer)).to_arrow().is_err());
        assert!(SqlType::Map(Box::new(SqlType::Integer), Box::new(SqlType::Interval)).to_arrow().is_err());
        assert!(SqlType::Array(Box::new(SqlType::Interval), 1).to_arrow().is_err());
        assert!(SqlType::Array(Box::new(SqlType::Integer), usize::MAX).to_arrow().is_err());
    }

    #[test]
    fn displays_duckdb_names() {
        let t = SqlType::Struct(vec![
            ("a\"b".into(), SqlType::List(Box::new(SqlType::Decimal { width: 9, scale: 2 }))),
            (
                "m".into(),
                SqlType::Map(Box::new(SqlType::Varchar), Box::new(SqlType::Array(Box::new(SqlType::Float), 3))),
            ),
            ("e".into(), SqlType::Enum(vec!["x".into(), "it's".into()])),
        ]);
        assert_eq!(
            t.to_string(),
            "STRUCT(\"a\"\"b\" DECIMAL(9,2)[], \"m\" MAP(VARCHAR, FLOAT[3]), \"e\" ENUM('x', 'it''s'))"
        );
        let names = [
            (SqlType::Boolean, "BOOLEAN"),
            (SqlType::TinyInt, "TINYINT"),
            (SqlType::SmallInt, "SMALLINT"),
            (SqlType::Integer, "INTEGER"),
            (SqlType::BigInt, "BIGINT"),
            (SqlType::UTinyInt, "UTINYINT"),
            (SqlType::USmallInt, "USMALLINT"),
            (SqlType::UInteger, "UINTEGER"),
            (SqlType::UBigInt, "UBIGINT"),
            (SqlType::Float, "FLOAT"),
            (SqlType::Double, "DOUBLE"),
            (SqlType::Varchar, "VARCHAR"),
            (SqlType::Blob, "BLOB"),
            (SqlType::Date, "DATE"),
            (SqlType::Time, "TIME"),
            (SqlType::TimeNs, "TIME_NS"),
            (SqlType::TimestampS, "TIMESTAMP_S"),
            (SqlType::TimestampMs, "TIMESTAMP_MS"),
            (SqlType::Timestamp, "TIMESTAMP"),
            (SqlType::TimestampNs, "TIMESTAMP_NS"),
            (SqlType::TimestampTz, "TIMESTAMP WITH TIME ZONE"),
            (SqlType::TimestampTzNs, "TIMESTAMPTZ_NS"),
            (SqlType::Interval, "INTERVAL"),
            (SqlType::Unsupported("BIT".into()), "BIT"),
        ];
        for (t, name) in names {
            assert_eq!(t.to_string(), name);
        }
    }

    fn norm(a: impl Array + 'static) -> ArrayRef {
        normalize(&(Arc::new(a) as ArrayRef), Target::DuckdbCrate).unwrap()
    }

    #[test]
    fn plain_arrays_are_kept() {
        let a: ArrayRef = Arc::new(Int64Array::from(vec![1, 2]));
        assert!(Arc::ptr_eq(&normalize(&a, Target::DuckdbCrate).unwrap(), &a));
        let l: ArrayRef =
            Arc::new(ListArray::from_iter_primitive::<arrow_array::types::Int32Type, _, _>(vec![Some(vec![Some(1)])]));
        assert!(Arc::ptr_eq(&normalize(&l, Target::DuckdbCrate).unwrap(), &l));
    }

    #[test]
    fn decodes_dictionaries_and_widens() {
        let mut d = StringDictionaryBuilder::<Int8Type>::new();
        d.append_value("a");
        d.append_null();
        d.append_value("a");
        let n = norm(d.finish());
        assert_eq!(n.as_ref(), &StringArray::from(vec![Some("a"), None, Some("a")]) as &dyn Array);
        let h = norm(Float16Array::from(vec![half::f16::from_f32(1.5)]));
        assert_eq!(h.as_ref(), &Float32Array::from(vec![1.5]) as &dyn Array);
        let big =
            Decimal256Array::from(vec![Some(i256::from_i128(-12345)), None]).with_precision_and_scale(20, 2).unwrap();
        let n = norm(big);
        assert_eq!(
            n.as_ref(),
            &Decimal128Array::from(vec![Some(-12345), None]).with_precision_and_scale(20, 2).unwrap() as &dyn Array
        );
    }

    #[test]
    fn keeps_nanoseconds_as_raw_values() {
        let t = norm(Time64NanosecondArray::from(vec![Some(86_399_999_999_999), None]));
        assert_eq!(t.as_ref(), &Int64Array::from(vec![Some(86_399_999_999_999), None]) as &dyn Array);
        let ts = norm(TimestampNanosecondArray::from(vec![1_735_689_600_007_000_001]).with_timezone("UTC"));
        assert_eq!(ts.as_ref(), &TimestampNanosecondArray::from(vec![1_735_689_600_007_000_001]) as &dyn Array);
        // microsecond and naive nanosecond timestamps are already copied exactly
        let us: ArrayRef = Arc::new(TimestampMicrosecondArray::from(vec![1]).with_timezone("UTC"));
        assert!(Arc::ptr_eq(&normalize(&us, Target::DuckdbCrate).unwrap(), &us));
        let ns: ArrayRef = Arc::new(TimestampNanosecondArray::from(vec![1]));
        assert!(Arc::ptr_eq(&normalize(&ns, Target::DuckdbCrate).unwrap(), &ns));
        let t_us: ArrayRef = Arc::new(Time64MicrosecondArray::from(vec![1]));
        assert!(Arc::ptr_eq(&normalize(&t_us, Target::DuckdbCrate).unwrap(), &t_us));
    }

    fn dict_strings(values: &[Option<&str>]) -> ArrayRef {
        let mut d = StringDictionaryBuilder::<Int8Type>::new();
        for v in values {
            d.append_option(*v);
        }
        Arc::new(d.finish())
    }

    #[test]
    fn rebuilds_nested_arrays_around_normalized_children() {
        let values = dict_strings(&[Some("x"), Some("y"), None]);
        let offsets = arrow_buffer::OffsetBuffer::new(vec![0, 2, 2, 3].into());
        let nulls = arrow_buffer::NullBuffer::from(vec![true, false, true]);
        let list =
            ListArray::new(item(values.data_type().clone()), offsets.clone(), values.clone(), Some(nulls.clone()));
        let n = norm(list);
        let n = n.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(n.value_type(), DataType::Utf8);
        assert_eq!(n.offsets(), &offsets);
        assert_eq!(n.nulls(), Some(&nulls));
        assert_eq!(n.values().as_ref(), &StringArray::from(vec![Some("x"), Some("y"), None]) as &dyn Array);

        let large = LargeListArray::new(
            item(values.data_type().clone()),
            arrow_buffer::OffsetBuffer::new(vec![0i64, 1, 3].into()),
            values.clone(),
            None,
        );
        let n = norm(large);
        let n = n.as_any().downcast_ref::<LargeListArray>().unwrap();
        assert_eq!(n.value_type(), DataType::Utf8);
        assert_eq!(n.value_offsets(), &[0, 1, 3]);

        let fixed = FixedSizeListArray::new(item(values.data_type().clone()), 3, values.clone(), None);
        let n = norm(fixed);
        let n = n.as_any().downcast_ref::<FixedSizeListArray>().unwrap();
        assert_eq!((n.value_type(), n.value_length()), (DataType::Utf8, 3));

        let s = StructArray::new(
            Fields::from(vec![
                Field::new("s", values.data_type().clone(), true),
                Field::new("i", DataType::Int32, true),
            ]),
            vec![values.clone(), Arc::new(Int32Array::from(vec![1, 2, 3]))],
            Some(nulls.clone()),
        );
        let n = norm(s);
        let n = n.as_any().downcast_ref::<StructArray>().unwrap();
        assert_eq!(n.fields()[0].data_type(), &DataType::Utf8);
        assert_eq!(n.fields()[0].name(), "s");
        assert_eq!(n.fields()[1].data_type(), &DataType::Int32);
        assert_eq!(n.nulls(), Some(&nulls));
        assert_eq!(n.column(0).as_ref(), &StringArray::from(vec![Some("x"), Some("y"), None]) as &dyn Array);
    }

    #[test]
    fn rebuilds_maps() {
        let mut b = MapBuilder::new(None, StringBuilder::new(), ListBuilder::new(Int32Builder::new()));
        b.keys().append_value("k");
        b.values().append_value([Some(1)]);
        b.append(true).unwrap();
        b.append(false).unwrap();
        let map = b.finish();
        // a map whose values hold nanosecond timestamps
        let entries = map.entries();
        let ts: ArrayRef = Arc::new(TimestampNanosecondArray::from(vec![5]).with_timezone("UTC"));
        let fields = Fields::from(vec![
            Field::new("keys", DataType::Utf8, false),
            Field::new("values", ts.data_type().clone(), true),
        ]);
        let entries = StructArray::new(fields.clone(), vec![entries.column(0).clone(), ts], None);
        let field = Arc::new(Field::new("entries", DataType::Struct(fields), false));
        let map = MapArray::new(field, map.offsets().clone(), entries, map.nulls().cloned(), false);
        let n = norm(map);
        let n = n.as_any().downcast_ref::<MapArray>().unwrap();
        assert_eq!(n.entries().column(1).data_type(), &DataType::Timestamp(TimeUnit::Nanosecond, None));
        assert_eq!(n.value_offsets(), &[0, 1, 1]);
        assert!(n.is_null(1));
        let DataType::Map(f, false) = n.data_type() else { panic!() };
        assert_eq!(f.name(), "entries");
    }

    fn import(a: impl Array + 'static) -> Result<ArrayRef, String> {
        normalize(&(Arc::new(a) as ArrayRef), Target::ArrowImporter)
    }

    fn kept(a: ArrayRef, target: Target) {
        assert!(Arc::ptr_eq(&normalize(&a, target).unwrap(), &a), "{:?}", a.data_type());
    }

    #[test]
    fn the_arrow_importer_takes_nanoseconds_as_they_are() {
        kept(Arc::new(Time64NanosecondArray::from(vec![Some(1), None])), Target::ArrowImporter);
        kept(Arc::new(TimestampNanosecondArray::from(vec![1]).with_timezone("UTC")), Target::ArrowImporter);
        // dictionaries are still decoded
        let n = normalize(&dict_strings(&[Some("a"), None]), Target::ArrowImporter).unwrap();
        assert_eq!(n.as_ref(), &StringArray::from(vec![Some("a"), None]) as &dyn Array);
    }

    #[test]
    fn the_arrow_importer_takes_durations_in_microseconds() {
        let s = import(DurationSecondArray::from(vec![Some(2), None])).unwrap();
        assert_eq!(s.as_ref(), &DurationMicrosecondArray::from(vec![Some(2_000_000), None]) as &dyn Array);
        let ms = import(DurationMillisecondArray::from(vec![-3])).unwrap();
        assert_eq!(ms.as_ref(), &DurationMicrosecondArray::from(vec![-3000]) as &dyn Array);
        let e = import(DurationSecondArray::from(vec![i64::MAX / 2])).unwrap_err();
        assert_eq!(e, "the duration 4611686018427387903 s does not fit in an INTERVAL");
        let e = import(DurationMillisecondArray::from(vec![i64::MIN / 10])).unwrap_err();
        assert_eq!(e, "the duration -922337203685477580 ms does not fit in an INTERVAL");
        // microseconds and nanoseconds as they are (the importer truncates nanoseconds, as the
        // C API v1 build does), and intervals
        kept(Arc::new(DurationMicrosecondArray::from(vec![i64::MAX])), Target::ArrowImporter);
        kept(Arc::new(DurationNanosecondArray::from(vec![i64::MIN])), Target::ArrowImporter);
        kept(Arc::new(IntervalYearMonthArray::from(vec![1])), Target::ArrowImporter);
        // the C API v1 build writes durations itself
        kept(Arc::new(DurationSecondArray::from(vec![i64::MAX])), Target::DuckdbCrate);
        // inside a list
        let values: ArrayRef = Arc::new(DurationSecondArray::from(vec![1, 2]));
        let offsets = arrow_buffer::OffsetBuffer::new(vec![0, 2].into());
        let list = import(ListArray::new(item(values.data_type().clone()), offsets, values, None)).unwrap();
        let list = list.as_any().downcast_ref::<ListArray>().unwrap();
        assert_eq!(list.values().as_ref(), &DurationMicrosecondArray::from(vec![1_000_000, 2_000_000]) as &dyn Array);
    }

    #[test]
    fn the_arrow_importer_gets_decimals_within_their_precision() {
        let decimal =
            |v: i128, p: u8| Decimal128Array::from(vec![Some(v), None]).with_precision_and_scale(p, 0).unwrap();
        kept(Arc::new(decimal(999, 3)), Target::ArrowImporter);
        kept(Arc::new(decimal(-999, 3)), Target::ArrowImporter);
        let e = import(decimal(1000, 3)).unwrap_err();
        assert_eq!(e, "decimal value has 4 digits, exceeding width 3");
        assert_eq!(import(decimal(-1000, 3)).unwrap_err(), "decimal value has 4 digits, exceeding width 3");
        assert_eq!(import(decimal(i128::MIN, 38)).unwrap_err(), "decimal value has 39 digits, exceeding width 38");
        // a struct of one
        let s = StructArray::new(
            Fields::from(vec![Field::new("d", DataType::Decimal128(18, 0), true)]),
            vec![Arc::new(decimal(10_i128.pow(18), 18))],
            None,
        );
        assert!(import(s).is_err());
        // the duckdb crate checks decimals itself
        kept(Arc::new(decimal(1000, 3)), Target::DuckdbCrate);
    }
}
