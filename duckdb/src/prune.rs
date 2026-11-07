//! Row-group skipping: predicates of a query, checked against the statistics of a row group.
//!
//! The C API v2 build hands `read_tparquet` the filters DuckDB applies to its rows
//! ([`crate::v2`]). The extension does not take them over (DuckDB keeps applying every filter
//! to the rows), it only uses them to skip row groups that cannot hold a matching row: a
//! row group is skipped when [`may_match`] says no row of it can satisfy the predicates. That
//! must hold for every row group, whatever its values, so every rule here errs towards reading.
//!
//! The comparisons follow DuckDB's semantics: NULL satisfies no comparison, floats order NaN
//! above every number (and NaN equals NaN), strings compare by bytes.

use crate::types::SqlType;
use parquet::basic::{
    ColumnOrder, DecimalType, IntType, LogicalType, SortOrder, TimeUnit, TimestampType, Type as PhysicalType,
};
use parquet::file::metadata::ParquetMetaData;
use parquet::file::statistics::Statistics;
use std::cmp::Ordering;

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    /// The operator with its operands swapped: `c < x` is `x > c`.
    pub fn flip(self) -> Self {
        match self {
            Op::Lt => Op::Gt,
            Op::Le => Op::Ge,
            Op::Gt => Op::Lt,
            Op::Ge => Op::Le,
            op => op,
        }
    }
}

/// A value of a column, as statistics and constants hold it.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    /// Integers, dates, timestamps and unscaled decimals.
    Int(i128),
    Float(f64),
    /// Strings, compared byte by byte.
    Bytes(Vec<u8>),
}

impl Scalar {
    /// The order of two values of the same kind (`None` for different kinds or NaN).
    fn cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Scalar::Int(a), Scalar::Int(b)) => Some(a.cmp(b)),
            (Scalar::Float(a), Scalar::Float(b)) => a.partial_cmp(b),
            (Scalar::Bytes(a), Scalar::Bytes(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }

    fn is_nan(&self) -> bool {
        matches!(self, Scalar::Float(f) if f.is_nan())
    }
}

/// A predicate over the result columns of `read_tparquet` (by result column index).
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    Compare { column: usize, op: Op, value: Scalar },
    IsNull(usize),
    IsNotNull(usize),
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
}

impl Predicate {
    /// The result columns the predicate reads.
    pub fn columns(&self, out: &mut Vec<usize>) {
        match self {
            Predicate::Compare { column, .. } | Predicate::IsNull(column) | Predicate::IsNotNull(column) => {
                out.push(*column)
            }
            Predicate::And(ps) | Predicate::Or(ps) => ps.iter().for_each(|p| p.columns(out)),
        }
    }
}

/// What the statistics of a row group say about one column.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stats {
    /// Rows of the row group.
    pub rows: u64,
    /// NULLs among them, if known.
    pub nulls: Option<u64>,
    /// Bounds of the values that are not NULL (nor NaN), if known.
    pub min: Option<Scalar>,
    pub max: Option<Scalar>,
    /// Whether some values may be NaN, which the bounds leave out.
    pub nan: bool,
}

impl Stats {
    /// A column the file does not have: every row is NULL.
    pub fn all_null(rows: u64) -> Self {
        Stats { rows, nulls: Some(rows), ..Default::default() }
    }

    fn only_nulls(&self) -> bool {
        self.nulls == Some(self.rows)
    }
}

fn compare_may_match(stats: &Stats, op: Op, value: &Scalar) -> bool {
    if stats.only_nulls() {
        return false;
    }
    // without both bounds, or for a NaN constant, nothing is known
    let (Some(min), Some(max)) = (&stats.min, &stats.max) else { return true };
    if value.is_nan() {
        return true;
    }
    let (Some(lo), Some(hi)) = (min.cmp(value), max.cmp(value)) else { return true };
    match op {
        Op::Eq => lo != Ordering::Greater && hi != Ordering::Less,
        // NaN is greater than any number: it satisfies > and >=
        Op::Gt => stats.nan || hi == Ordering::Greater,
        Op::Ge => stats.nan || hi != Ordering::Less,
        Op::Lt => lo == Ordering::Less,
        Op::Le => lo != Ordering::Greater,
        Op::Ne => stats.nan || !(lo == Ordering::Equal && hi == Ordering::Equal),
    }
}

/// Whether some row of a row group may satisfy `predicate`, given the statistics of its
/// columns (`None`: nothing is known about that column).
pub fn may_match(predicate: &Predicate, stats: &dyn Fn(usize) -> Option<Stats>) -> bool {
    match predicate {
        Predicate::Compare { column, op, value } => stats(*column).is_none_or(|s| compare_may_match(&s, *op, value)),
        Predicate::IsNull(column) => stats(*column).is_none_or(|s| s.nulls != Some(0)),
        Predicate::IsNotNull(column) => stats(*column).is_none_or(|s| !s.only_nulls()),
        Predicate::And(ps) => ps.iter().all(|p| may_match(p, stats)),
        Predicate::Or(ps) => ps.iter().any(|p| may_match(p, stats)),
    }
}

/// How the statistics of a Parquet column chunk read as [`Scalar`]s of a DuckDB column type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// Signed integers (`INT32`, `INT64`), also dates, timestamps and decimals stored so.
    Signed,
    /// Unsigned integers stored in `INT32`/`INT64`.
    Unsigned,
    /// Big-endian two's complement decimals (`FIXED_LEN_BYTE_ARRAY`, `BYTE_ARRAY`).
    DecimalBytes,
    Float,
    Bytes,
}

fn unit_of(t: &SqlType) -> Option<TimeUnit> {
    match t {
        SqlType::TimestampMs => Some(TimeUnit::MILLIS),
        SqlType::Timestamp | SqlType::TimestampTz => Some(TimeUnit::MICROS),
        SqlType::TimestampNs => Some(TimeUnit::NANOS),
        _ => None,
    }
}

/// How to read the statistics of the leaf column `leaf` of a file for a result column of type
/// `sql`; `None` where they cannot be compared with DuckDB values of that type (other types,
/// other units, or an order the statistics may not have been computed in).
pub fn reading(metadata: &ParquetMetaData, leaf: usize, sql: &SqlType) -> Option<Reading> {
    let file = metadata.file_metadata();
    let column = file.schema_descr().column(leaf);
    let physical = column.physical_type();
    let logical = column.logical_type_ref();
    let order = file.column_order(leaf);
    let int = matches!(physical, PhysicalType::INT32 | PhysicalType::INT64);
    let type_defined = |sort| order == ColumnOrder::TYPE_DEFINED_ORDER(sort);
    match sql {
        SqlType::TinyInt | SqlType::SmallInt | SqlType::Integer | SqlType::BigInt => {
            let signed = matches!(logical, None | Some(LogicalType::Integer(IntType { is_signed: true, .. })));
            (int && signed).then_some(Reading::Signed)
        }
        SqlType::UTinyInt | SqlType::USmallInt | SqlType::UInteger | SqlType::UBigInt => {
            let unsigned = matches!(logical, Some(LogicalType::Integer(IntType { is_signed: false, .. })));
            (int && unsigned && type_defined(SortOrder::UNSIGNED)).then_some(Reading::Unsigned)
        }
        SqlType::Float | SqlType::Double => {
            let float = matches!(physical, PhysicalType::FLOAT | PhysicalType::DOUBLE) && logical.is_none();
            // both orders leave NaN out of the bounds (a NaN bound means only NaNs, see
            // stats_of); IEEE 754 total order puts -0 below +0, which DuckDB takes as equal, so its
            // bounds bound DuckDB's order as well
            let ordered = matches!(
                order,
                ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::SIGNED)
                    | ColumnOrder::UNDEFINED
                    | ColumnOrder::IEEE_754_TOTAL_ORDER
            );
            (float && ordered).then_some(Reading::Float)
        }
        SqlType::Varchar => {
            let string = physical == PhysicalType::BYTE_ARRAY && matches!(logical, Some(LogicalType::String));
            (string && type_defined(SortOrder::UNSIGNED)).then_some(Reading::Bytes)
        }
        SqlType::Date => {
            (physical == PhysicalType::INT32 && matches!(logical, Some(LogicalType::Date))).then_some(Reading::Signed)
        }
        SqlType::TimestampMs | SqlType::Timestamp | SqlType::TimestampNs | SqlType::TimestampTz => match logical {
            Some(LogicalType::Timestamp(TimestampType { unit, is_adjusted_to_u_t_c }))
                if physical == PhysicalType::INT64
                    && Some(*unit) == unit_of(sql)
                    && *is_adjusted_to_u_t_c == (*sql == SqlType::TimestampTz) =>
            {
                Some(Reading::Signed)
            }
            _ => None,
        },
        SqlType::Decimal { scale, .. } => match logical {
            Some(LogicalType::Decimal(DecimalType { scale: s, .. })) if *s == i32::from(*scale) => match physical {
                PhysicalType::INT32 | PhysicalType::INT64 => Some(Reading::Signed),
                PhysicalType::FIXED_LEN_BYTE_ARRAY | PhysicalType::BYTE_ARRAY => {
                    type_defined(SortOrder::SIGNED).then_some(Reading::DecimalBytes)
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// A big-endian two's complement integer of at most 16 bytes.
fn be_int(bytes: &[u8]) -> Option<i128> {
    if bytes.is_empty() || bytes.len() > 16 {
        return None;
    }
    let fill = if bytes[0] & 0x80 != 0 { 0xff } else { 0 };
    let mut b = [fill; 16];
    b[16 - bytes.len()..].copy_from_slice(bytes);
    Some(i128::from_be_bytes(b))
}

/// The statistics of a column chunk as [`Stats`], read as `reading` says; `rows` is the row
/// count of the row group.
pub fn stats_of(statistics: Option<&Statistics>, reading: Reading, rows: u64) -> Stats {
    let Some(s) = statistics else { return Stats { rows, ..Default::default() } };
    let mut out = Stats { rows, nulls: s.null_count_opt(), ..Default::default() };
    // deprecated min/max were computed in signed order, whatever the type
    let bounds_ok = !s.is_min_max_deprecated() || matches!(reading, Reading::Signed | Reading::Float);
    if !bounds_ok {
        return out;
    }
    let (min, max) = match (reading, s) {
        (Reading::Signed, Statistics::Int32(v)) => {
            (v.min_opt().map(|m| Scalar::Int((*m).into())), v.max_opt().map(|m| Scalar::Int((*m).into())))
        }
        (Reading::Signed, Statistics::Int64(v)) => {
            (v.min_opt().map(|m| Scalar::Int((*m).into())), v.max_opt().map(|m| Scalar::Int((*m).into())))
        }
        (Reading::Unsigned, Statistics::Int32(v)) => {
            (v.min_opt().map(|m| Scalar::Int((*m as u32).into())), v.max_opt().map(|m| Scalar::Int((*m as u32).into())))
        }
        (Reading::Unsigned, Statistics::Int64(v)) => {
            (v.min_opt().map(|m| Scalar::Int((*m as u64).into())), v.max_opt().map(|m| Scalar::Int((*m as u64).into())))
        }
        (Reading::Float, Statistics::Float(v)) => {
            out.nan = s.nan_count_opt() != Some(0);
            (v.min_opt().map(|m| Scalar::Float((*m).into())), v.max_opt().map(|m| Scalar::Float((*m).into())))
        }
        (Reading::Float, Statistics::Double(v)) => {
            out.nan = s.nan_count_opt() != Some(0);
            (v.min_opt().map(|m| Scalar::Float(*m)), v.max_opt().map(|m| Scalar::Float(*m)))
        }
        (Reading::DecimalBytes, Statistics::FixedLenByteArray(_) | Statistics::ByteArray(_)) => {
            (s.min_bytes_opt().and_then(be_int).map(Scalar::Int), s.max_bytes_opt().and_then(be_int).map(Scalar::Int))
        }
        (Reading::Bytes, Statistics::ByteArray(_)) => {
            (s.min_bytes_opt().map(|b| Scalar::Bytes(b.to_vec())), s.max_bytes_opt().map(|b| Scalar::Bytes(b.to_vec())))
        }
        _ => (None, None),
    };
    // a NaN bound (a writer that did not leave NaNs out) bounds nothing
    if min.as_ref().is_some_and(Scalar::is_nan) || max.as_ref().is_some_and(Scalar::is_nan) {
        out.nan = true;
        return out;
    }
    out.min = min;
    out.max = max;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i128) -> Scalar {
        Scalar::Int(v)
    }

    fn cmp(column: usize, op: Op, value: Scalar) -> Predicate {
        Predicate::Compare { column, op, value }
    }

    fn ints(min: i128, max: i128, nulls: u64) -> Stats {
        Stats { rows: 10, nulls: Some(nulls), min: Some(int(min)), max: Some(int(max)), nan: false }
    }

    fn one(s: Stats) -> impl Fn(usize) -> Option<Stats> {
        move |c| (c == 0).then(|| s.clone())
    }

    #[test]
    fn comparisons_against_integer_bounds() {
        let s = one(ints(10, 20, 0));
        let m = |op, v| may_match(&cmp(0, op, int(v)), &s);
        assert!(!m(Op::Eq, 9) && m(Op::Eq, 10) && m(Op::Eq, 15) && m(Op::Eq, 20) && !m(Op::Eq, 21));
        assert!(!m(Op::Lt, 10) && m(Op::Lt, 11));
        assert!(!m(Op::Le, 9) && m(Op::Le, 10));
        assert!(!m(Op::Gt, 20) && m(Op::Gt, 19));
        assert!(!m(Op::Ge, 21) && m(Op::Ge, 20));
        assert!(m(Op::Ne, 10) && m(Op::Ne, 0));
        let same = one(ints(7, 7, 0));
        assert!(!may_match(&cmp(0, Op::Ne, int(7)), &same));
        assert!(may_match(&cmp(0, Op::Ne, int(8)), &same));
    }

    #[test]
    fn nulls_satisfy_no_comparison() {
        let all_null = one(Stats::all_null(10));
        for op in [Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge] {
            assert!(!may_match(&cmp(0, op, int(1)), &all_null), "{op:?}");
        }
        assert!(may_match(&Predicate::IsNull(0), &all_null));
        assert!(!may_match(&Predicate::IsNotNull(0), &all_null));
        let none_null = one(ints(1, 2, 0));
        assert!(!may_match(&Predicate::IsNull(0), &none_null));
        assert!(may_match(&Predicate::IsNotNull(0), &none_null));
        let unknown = one(Stats { rows: 10, ..Default::default() });
        assert!(may_match(&Predicate::IsNull(0), &unknown));
        assert!(may_match(&Predicate::IsNotNull(0), &unknown));
        assert!(may_match(&cmp(0, Op::Eq, int(1)), &unknown));
    }

    #[test]
    fn unknown_columns_and_kinds_are_read() {
        let s = one(ints(10, 20, 0));
        assert!(may_match(&cmp(1, Op::Eq, int(99)), &s));
        assert!(may_match(&cmp(0, Op::Eq, Scalar::Bytes(b"x".to_vec())), &s));
        let half = one(Stats { rows: 10, nulls: Some(0), min: Some(int(10)), max: None, nan: false });
        assert!(may_match(&cmp(0, Op::Lt, int(0)), &half));
    }

    #[test]
    fn nan_is_greater_than_every_number() {
        let floats = |nan| {
            one(Stats { rows: 5, nulls: Some(0), min: Some(Scalar::Float(-1.0)), max: Some(Scalar::Float(1.0)), nan })
        };
        let m = |op, v: f64, nan| may_match(&cmp(0, op, Scalar::Float(v)), &floats(nan));
        // NaN rows satisfy > and >= and <>
        assert!(m(Op::Gt, 5.0, true) && !m(Op::Gt, 5.0, false));
        assert!(m(Op::Ge, 5.0, true) && !m(Op::Ge, 5.0, false));
        assert!(!m(Op::Lt, -1.0, true) && !m(Op::Eq, 5.0, true));
        let same = one(Stats {
            rows: 5,
            nulls: Some(0),
            min: Some(Scalar::Float(0.0)),
            max: Some(Scalar::Float(0.0)),
            nan: true,
        });
        assert!(may_match(&cmp(0, Op::Ne, Scalar::Float(0.0)), &same));
        // a NaN constant is never used to skip
        for op in [Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge] {
            assert!(may_match(&cmp(0, op, Scalar::Float(f64::NAN)), &floats(false)), "{op:?}");
        }
        // -0.0 and 0.0 are equal
        let zero = one(Stats {
            rows: 5,
            nulls: Some(0),
            min: Some(Scalar::Float(-0.0)),
            max: Some(Scalar::Float(-0.0)),
            nan: false,
        });
        assert!(may_match(&cmp(0, Op::Eq, Scalar::Float(0.0)), &zero));
        assert!(!may_match(&cmp(0, Op::Lt, Scalar::Float(0.0)), &zero));
    }

    #[test]
    fn conjunctions_and_disjunctions() {
        let s = |c: usize| match c {
            0 => Some(ints(0, 10, 0)),
            1 => Some(ints(100, 200, 0)),
            _ => None,
        };
        let no = cmp(0, Op::Gt, int(10));
        let yes = cmp(1, Op::Eq, int(150));
        assert!(!may_match(&Predicate::And(vec![yes.clone(), no.clone()]), &s));
        assert!(may_match(&Predicate::Or(vec![no.clone(), yes.clone()]), &s));
        assert!(!may_match(&Predicate::Or(vec![no.clone(), no.clone()]), &s));
        assert!(may_match(&Predicate::And(vec![]), &s));
        assert!(!may_match(&Predicate::Or(vec![]), &s));
        let mut cols = vec![];
        Predicate::Or(vec![no, Predicate::And(vec![yes, Predicate::IsNull(3)])]).columns(&mut cols);
        assert_eq!(cols, vec![0, 1, 3]);
        assert_eq!(Op::Lt.flip(), Op::Gt);
        assert_eq!(Op::Ge.flip(), Op::Le);
        assert_eq!(Op::Eq.flip(), Op::Eq);
        assert_eq!(Op::Ne.flip(), Op::Ne);
        assert_eq!(Op::Le.flip(), Op::Ge);
        assert_eq!(Op::Gt.flip(), Op::Lt);
    }

    #[test]
    fn decimal_bytes_are_big_endian_twos_complement() {
        assert_eq!(be_int(&[0x01, 0x00]), Some(256));
        assert_eq!(be_int(&[0xff, 0xfe]), Some(-2));
        assert_eq!(be_int(&[0x80]), Some(-128));
        assert_eq!(be_int(&[0x7f]), Some(127));
        assert_eq!(be_int(&[]), None);
        assert_eq!(be_int(&[0; 17]), None);
        assert_eq!(be_int(&[0xff; 16]), Some(-1));
    }

    /// DuckDB's comparison of a value with a constant (NULL: no match; NaN greatest).
    fn duck(value: &Option<Scalar>, op: Op, c: &Scalar) -> bool {
        let Some(v) = value else { return false };
        let ord = match (v, c) {
            (Scalar::Float(a), Scalar::Float(b)) => match (a.is_nan(), b.is_nan()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                _ => a.partial_cmp(b).unwrap(),
            },
            _ => v.cmp(c).unwrap(),
        };
        match op {
            Op::Eq => ord == Ordering::Equal,
            Op::Ne => ord != Ordering::Equal,
            Op::Lt => ord == Ordering::Less,
            Op::Le => ord != Ordering::Greater,
            Op::Gt => ord == Ordering::Greater,
            Op::Ge => ord != Ordering::Less,
        }
    }

    fn eval(p: &Predicate, row: &[Option<Scalar>]) -> bool {
        match p {
            Predicate::Compare { column, op, value } => duck(&row[*column], *op, value),
            Predicate::IsNull(c) => row[*c].is_none(),
            Predicate::IsNotNull(c) => row[*c].is_some(),
            Predicate::And(ps) => ps.iter().all(|p| eval(p, row)),
            Predicate::Or(ps) => ps.iter().any(|p| eval(p, row)),
        }
    }

    /// Stats as a writer computes them: bounds of the values that are neither NULL nor NaN.
    fn stats_of_values(values: &[Option<Scalar>]) -> Stats {
        let mut s = Stats {
            rows: values.len() as u64,
            nulls: Some(values.iter().filter(|v| v.is_none()).count() as u64),
            ..Default::default()
        };
        for v in values.iter().flatten() {
            if v.is_nan() {
                s.nan = true;
                continue;
            }
            if s.min.as_ref().is_none_or(|m| v.cmp(m) == Some(Ordering::Less)) {
                s.min = Some(v.clone());
            }
            if s.max.as_ref().is_none_or(|m| v.cmp(m) == Some(Ordering::Greater)) {
                s.max = Some(v.clone());
            }
        }
        s
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % n
        }
    }

    fn value(rng: &mut Rng, kind: u64) -> Option<Scalar> {
        if rng.next(5) == 0 {
            return None;
        }
        Some(match kind {
            0 => Scalar::Int(rng.next(9) as i128 - 4),
            1 => Scalar::Float(
                [f64::NAN, -0.0, 0.0, -1.5, 1.5, 3.0, f64::INFINITY, f64::NEG_INFINITY][rng.next(8) as usize],
            ),
            _ => Scalar::Bytes([&b""[..], b"a", b"ab", b"b", b"\xff"][rng.next(5) as usize].to_vec()),
        })
    }

    fn predicate(rng: &mut Rng, kinds: &[u64], depth: u32) -> Predicate {
        let column = rng.next(kinds.len() as u64) as usize;
        match rng.next(if depth > 0 { 5 } else { 3 }) {
            0 => Predicate::IsNull(column),
            1 => Predicate::IsNotNull(column),
            2 => {
                let ops = [Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge];
                let value = loop {
                    if let Some(v) = value(rng, kinds[column]) {
                        break v;
                    }
                };
                cmp(column, ops[rng.next(6) as usize], value)
            }
            3 => Predicate::And((0..rng.next(3)).map(|_| predicate(rng, kinds, depth - 1)).collect()),
            _ => Predicate::Or((0..rng.next(3)).map(|_| predicate(rng, kinds, depth - 1)).collect()),
        }
    }

    /// The property every rule must keep: a row group with a matching row is never skipped.
    #[test]
    fn a_row_group_with_a_matching_row_is_never_skipped() {
        let mut rng = Rng(42);
        let kinds = [0, 1, 2];
        let mut skipped = 0;
        for _ in 0..20_000 {
            let n = 1 + rng.next(4) as usize;
            let rows: Vec<Vec<Option<Scalar>>> =
                (0..n).map(|_| kinds.iter().map(|&k| value(&mut rng, k)).collect()).collect();
            let columns: Vec<Stats> = (0..kinds.len())
                .map(|c| stats_of_values(&rows.iter().map(|r| r[c].clone()).collect::<Vec<_>>()))
                .collect();
            let p = predicate(&mut rng, &kinds, 2);
            let matches = rows.iter().any(|r| eval(&p, r));
            let may = may_match(&p, &|c| columns.get(c).cloned());
            assert!(may || !matches, "skipped a row group that matches {p:?}: {rows:?}");
            skipped += usize::from(!may);
        }
        // and the rules do skip
        assert!(skipped > 2000, "{skipped}");
    }

    /// The statistics of a file the tparquet writer wrote, column by column, as the scan reads
    /// them for the DuckDB type of each column.
    #[test]
    fn reads_the_statistics_of_each_type_a_file_holds() {
        use arrow_array::{
            ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array, Int8Array,
            Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray, TimestampMillisecondArray,
            TimestampNanosecondArray, TimestampSecondArray, UInt32Array, UInt64Array,
        };
        use arrow_schema::{Field, Schema};
        use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions};
        use std::sync::Arc;
        let columns: Vec<(&str, ArrayRef)> = vec![
            ("i8", Arc::new(Int8Array::from(vec![Some(-3), None, Some(7)]))),
            ("i64", Arc::new(Int64Array::from(vec![i64::MIN, 0, i64::MAX]))),
            ("u32", Arc::new(UInt32Array::from(vec![1, u32::MAX, 5]))),
            ("u64", Arc::new(UInt64Array::from(vec![u64::MAX, 2, 3]))),
            ("f32", Arc::new(Float32Array::from(vec![1.5, f32::NAN, -2.0]))),
            ("f64", Arc::new(Float64Array::from(vec![Some(0.25), None, Some(8.0)]))),
            ("s", Arc::new(StringArray::from(vec!["b", "a\u{e9}", "z"]))),
            ("d", Arc::new(Date32Array::from(vec![10, -5, 20]))),
            ("ms", Arc::new(TimestampMillisecondArray::from(vec![3, 1, 2]))),
            ("us_utc", Arc::new(TimestampMicrosecondArray::from(vec![9, 7, 8]).with_timezone("UTC"))),
            ("sec", Arc::new(TimestampSecondArray::from(vec![1, 2, 3]))),
            ("us", Arc::new(TimestampMicrosecondArray::from(vec![6, 4, 5]))),
            ("ns", Arc::new(TimestampNanosecondArray::from(vec![i64::MAX, -1, 0]))),
            ("dec18", Arc::new(Decimal128Array::from(vec![-150, 99, 3]).with_precision_and_scale(10, 2).unwrap())),
            (
                "dec30",
                Arc::new(
                    Decimal128Array::from(vec![-(1i128 << 70), 5, 1i128 << 80])
                        .with_precision_and_scale(30, 3)
                        .unwrap(),
                ),
            ),
            ("bin", Arc::new(BinaryArray::from(vec![&b"x"[..], b"y", b"z"]))),
            ("bool", Arc::new(BooleanArray::from(vec![true, false, true]))),
        ];
        let batch = RecordBatch::try_from_iter(columns).unwrap();
        let mut buf = vec![];
        let mut w = tparquet::Writer::try_new(&mut buf, batch.schema(), tparquet::WriterOptions::default()).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();
        let meta = ArrowReaderMetadata::load(&bytes::Bytes::from(buf), ArrowReaderOptions::new()).unwrap();
        let schema: Arc<Schema> = meta.schema().clone();
        let parquet = meta.metadata();
        type Got = Option<(Option<Scalar>, Option<Scalar>, Option<u64>, bool)>;
        let got = |name: &str| -> Got {
            let i = schema.index_of(name).unwrap();
            let field: &Field = schema.field(i);
            let sql = SqlType::from_arrow(field).unwrap();
            let r = reading(parquet, i, &sql)?;
            let s = stats_of(parquet.row_group(0).column(i).statistics(), r, 3);
            assert_eq!(s.rows, 3);
            Some((s.min, s.max, s.nulls, s.nan))
        };
        let f = Scalar::Float;
        assert_eq!(got("i8"), Some((Some(int(-3)), Some(int(7)), Some(1), false)));
        assert_eq!(got("i64"), Some((Some(int(i64::MIN.into())), Some(int(i64::MAX.into())), Some(0), false)));
        assert_eq!(got("u32"), Some((Some(int(1)), Some(int(u32::MAX.into())), Some(0), false)));
        assert_eq!(got("u64"), Some((Some(int(2)), Some(int(u64::MAX.into())), Some(0), false)));
        // the writer leaves NaN out of the bounds and counts NaNs
        assert_eq!(got("f32"), Some((Some(f(-2.0)), Some(f(1.5)), Some(0), true)));
        assert_eq!(got("f64"), Some((Some(f(0.25)), Some(f(8.0)), Some(1), false)));
        assert_eq!(
            got("s"),
            Some((Some(Scalar::Bytes("a\u{e9}".into())), Some(Scalar::Bytes(b"z".to_vec())), Some(0), false))
        );
        assert_eq!(got("d"), Some((Some(int(-5)), Some(int(20)), Some(0), false)));
        assert_eq!(got("ms"), Some((Some(int(1)), Some(int(3)), Some(0), false)));
        assert_eq!(got("us_utc"), Some((Some(int(7)), Some(int(9)), Some(0), false)));
        assert_eq!(got("us"), Some((Some(int(4)), Some(int(6)), Some(0), false)));
        assert_eq!(got("ns"), Some((Some(int(-1)), Some(int(i64::MAX.into())), Some(0), false)));
        assert_eq!(got("dec18"), Some((Some(int(-150)), Some(int(99)), Some(0), false)));
        assert_eq!(got("dec30"), Some((Some(int(-(1i128 << 70))), Some(int(1i128 << 80)), Some(0), false)));
        // seconds are stored as milliseconds: their statistics are not in the unit of a
        // TIMESTAMP_S value; blobs and booleans are not compared
        assert_eq!(got("sec"), None);
        assert_eq!(got("bin"), None);
        assert_eq!(got("bool"), None);
        // a type of another column does not read its statistics
        let i = schema.index_of("i64").unwrap();
        for other in [
            SqlType::UBigInt,
            SqlType::Double,
            SqlType::Varchar,
            SqlType::Date,
            SqlType::Timestamp,
            SqlType::Decimal { width: 18, scale: 0 },
        ] {
            assert_eq!(reading(parquet, i, &other), None, "{other}");
        }
        let us = schema.index_of("us_utc").unwrap();
        assert_eq!(reading(parquet, us, &SqlType::Timestamp), None);
        assert_eq!(reading(parquet, us, &SqlType::TimestampTz), Some(Reading::Signed));
        let ms = schema.index_of("ms").unwrap();
        assert_eq!(reading(parquet, ms, &SqlType::Timestamp), None);
        // each unit reads only as its own type
        let ns = schema.index_of("ns").unwrap();
        assert_eq!(reading(parquet, ns, &SqlType::TimestampNs), Some(Reading::Signed));
        for other in [SqlType::TimestampMs, SqlType::Timestamp, SqlType::TimestampTz] {
            assert_eq!(reading(parquet, ns, &other), None, "{other}");
        }
        let d = schema.index_of("dec18").unwrap();
        assert_eq!(reading(parquet, d, &SqlType::Decimal { width: 10, scale: 3 }), None);
        // without statistics nothing is known but the rows
        assert_eq!(stats_of(None, Reading::Signed, 4), Stats { rows: 4, ..Default::default() });
        // statistics of another physical type give no bounds
        let other = stats_of(parquet.row_group(0).column(i).statistics(), Reading::Bytes, 3);
        assert_eq!((other.min, other.max, other.nulls), (None, None, Some(0)));
    }
}
