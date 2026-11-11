//! Logical equality of two Parquet files, floats compared bit for bit.

use crate::canonical;
use crate::error::Result;
use arrow_schema::{DataType, Field, Fields, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::ChunkReader;
use std::fmt;
use std::sync::Arc;

/// Where two files first differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// The Arrow schemas (names, types, nullability; not metadata) differ.
    Schema { a: String, b: String },
    /// Row `row` differs (its canonical JSON is shown).
    Row { row: usize, a: String, b: String },
    /// One file has more rows.
    RowCount { a: usize, b: usize },
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema { a, b } => write!(f, "schemas differ:\n  a: {a}\n  b: {b}"),
            Self::Row { row, a, b } => write!(f, "row {row} differs:\n  a: {a}\n  b: {b}"),
            Self::RowCount { a, b } => write!(f, "row counts differ: {a} vs {b}"),
        }
    }
}

/// The schema without metadata, for comparison. The names of list items and map entries
/// are not data (writers differ: `item` and `entries` in arrow-rs, `element` and
/// `key_value` in pyarrow and the Parquet spec), so they are made the same.
fn bare(fields: &Fields) -> Fields {
    fields.iter().map(|f| Arc::new(bare_field(f))).collect()
}

fn bare_field(f: &Field) -> Field {
    let item = |c: &Field| Arc::new(bare_field(c).with_name("item"));
    let t = match f.data_type() {
        DataType::List(c) => DataType::List(item(c)),
        DataType::LargeList(c) => DataType::LargeList(item(c)),
        DataType::FixedSizeList(c, n) => DataType::FixedSizeList(item(c), *n),
        DataType::Map(c, sorted) => DataType::Map(Arc::new(bare_field(c).with_name("entries")), *sorted),
        DataType::Struct(fields) => DataType::Struct(bare(fields)),
        t => t.clone(),
    };
    Field::new(f.name(), t, f.is_nullable())
}

fn lines<R: ChunkReader + 'static>(r: R) -> Result<(Schema, impl Iterator<Item = Result<String>>)> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(r)?;
    let schema = Schema::new(bare(builder.schema().fields()));
    let reader = builder.build()?;
    let lines = reader.flat_map(|batch| match batch {
        Ok(b) => match canonical::batch_lines(&b) {
            Ok(lines) => lines.into_iter().map(Ok).collect::<Vec<_>>(),
            Err(e) => vec![Err(e)],
        },
        Err(e) => vec![Err(e.into())],
    });
    Ok((schema, lines))
}

/// `n` plus the rows left in `lines`; the first error ends the count (the reader repeats it).
fn count(n: usize, mut lines: impl Iterator<Item = Result<String>>) -> Result<usize> {
    lines.try_fold(n, |n, line| line.map(|_| n + 1))
}

/// Compares `a` and `b`: `None` when they hold the same data.
pub fn verify<A: ChunkReader + 'static, B: ChunkReader + 'static>(a: A, b: B) -> Result<Option<Difference>> {
    let (sa, mut la) = lines(a)?;
    let (sb, mut lb) = lines(b)?;
    if sa != sb {
        return Ok(Some(Difference::Schema { a: sa.to_string(), b: sb.to_string() }));
    }
    let mut row = 0;
    loop {
        match (la.next().transpose()?, lb.next().transpose()?) {
            (None, None) => return Ok(None),
            (Some(a), Some(b)) if a == b => row += 1,
            (Some(a), Some(b)) => return Ok(Some(Difference::Row { row, a, b })),
            (Some(_), None) => return Ok(Some(Difference::RowCount { a: count(row + 1, la)?, b: row })),
            (None, Some(_)) => return Ok(Some(Difference::RowCount { a: row, b: count(row + 1, lb)? })),
        }
    }
}
