//! `COPY ... TO ... (FORMAT tparquet, ...)`: its options and the file it writes.
//!
//! DuckDB's C API v1 (v1.5.6) hands a COPY function the column *types* but not the column
//! *names*, so there the names come from the `COLUMN_NAMES` option (see `duckdb/README.md`); the
//! C API v2 hands over both, and `COLUMN_NAMES` only renames.

use crate::ffi::Val;
use crate::types::SqlType;
use arrow_array::RecordBatch;
use arrow_schema::{Field, Schema, SchemaRef};
use parquet::basic::{BrotliLevel, Compression, GzipLevel, ZstdLevel};
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::sync::Arc;
use tparquet::policy::parse_column_policy;
use tparquet::{Writer, WriterOptions};

/// The name of the COPY format.
pub const FORMAT: &str = "tparquet";

/// Rows the writer gathers before its first write, so that `POLICY 'auto'` compares the codecs
/// on more than one DuckDB vector.
pub const SAMPLE_ROWS: usize = 128 * 1024;

/// The most memory the held-back rows may take: long strings end the sample earlier (and stay
/// below the 2 GiB that one Arrow string array can hold).
pub const SAMPLE_BYTES: usize = 64 << 20;

/// The highest pcodec level.
pub const MAX_LEVEL: u32 = 12;

fn value_text(v: &Val) -> String {
    match v {
        Val::Int(i) => i.to_string(),
        Val::Str(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// The options a COPY accepts, with what they take.
pub const OPTIONS: &[(&str, &str)] = &[
    ("COLUMN_NAMES", "a list of the column names, in order"),
    ("POLICY", "'auto', 'pcodec' or 'base'"),
    ("LEVEL", "the pcodec level, 0 to 12"),
    ("COLUMN_POLICY", "a list of 'column=policy'"),
    ("COMPRESSION", "the base codec: 'zstd', 'snappy', 'gzip', 'brotli', 'lz4' or 'uncompressed'"),
    ("COMPRESSION_LEVEL", "the level of the base codec"),
    ("ROW_GROUP_ROWS", "rows per row group (DuckDB keeps ROW_GROUP_SIZE for itself)"),
];

/// What a COPY writes.
#[derive(Debug, Clone)]
pub struct CopyOptions {
    pub schema: SchemaRef,
    pub column_types: Vec<SqlType>,
    pub writer: WriterOptions,
}

fn string(name: &str, v: &Val) -> Result<String, String> {
    match v {
        Val::Str(s) => Ok(s.clone()),
        other => Err(format!("{name} must be a string, not {other:?}")),
    }
}

fn strings(name: &str, v: &Val) -> Result<Vec<String>, String> {
    match v {
        Val::List(items) => items.iter().map(|i| string(name, i)).collect(),
        Val::Str(s) => Ok(vec![s.clone()]),
        other => Err(format!("{name} must be a list of strings, not {other:?}")),
    }
}

fn integer(name: &str, v: &Val) -> Result<i64, String> {
    match v {
        Val::Int(i) => Ok(*i),
        Val::Str(s) => s.trim().parse().map_err(|_| format!("{name} must be an integer, not {s:?}")),
        other => Err(format!("{name} must be an integer, not {other:?}")),
    }
}

fn base_codec(name: &str, level: Option<i64>) -> Result<Compression, String> {
    let level_of = |default: i64| level.unwrap_or(default);
    let fits = |l: i64| u32::try_from(l).map_err(|_| format!("invalid COMPRESSION_LEVEL {l}"));
    let no_level = |c: Compression| match level {
        Some(_) => Err(format!("COMPRESSION '{name}' takes no COMPRESSION_LEVEL")),
        None => Ok(c),
    };
    match name.to_ascii_lowercase().as_str() {
        "zstd" => {
            let l = i32::try_from(level_of(3)).map_err(|_| "invalid COMPRESSION_LEVEL".to_string())?;
            Ok(Compression::ZSTD(ZstdLevel::try_new(l).map_err(|e| e.to_string())?))
        }
        "gzip" => Ok(Compression::GZIP(GzipLevel::try_new(fits(level_of(6))?).map_err(|e| e.to_string())?)),
        "brotli" => Ok(Compression::BROTLI(BrotliLevel::try_new(fits(level_of(1))?).map_err(|e| e.to_string())?)),
        "snappy" => no_level(Compression::SNAPPY),
        "lz4" | "lz4_raw" => no_level(Compression::LZ4_RAW),
        "uncompressed" | "none" => no_level(Compression::UNCOMPRESSED),
        other => {
            Err(format!("unknown COMPRESSION {other:?}: expected zstd, snappy, gzip, brotli, lz4 or uncompressed"))
        }
    }
}

impl CopyOptions {
    /// Parses the options of a COPY of columns of `types`. DuckDB passes option names upper-cased.
    /// `query_names` are the column names DuckDB gave, if any (the C API v2 does, v1 does not):
    /// they name the columns unless `COLUMN_NAMES` does.
    pub fn parse(
        types: Vec<SqlType>,
        query_names: Option<Vec<String>>,
        options: &[(String, Val)],
    ) -> Result<Self, String> {
        let mut names: Option<Vec<String>> = None;
        let mut writer = WriterOptions::default();
        let mut compression: Option<String> = None;
        let mut compression_level: Option<i64> = None;
        let mut properties = WriterProperties::builder();
        for (name, value) in options {
            let name = name.to_ascii_uppercase();
            match name.as_str() {
                "COLUMN_NAMES" => names = Some(strings(&name, value)?),
                "POLICY" => {
                    writer = writer.with_policy(
                        string(&name, value)?
                            .to_ascii_lowercase()
                            .parse()
                            .map_err(|e: tparquet::Error| e.to_string())?,
                    )
                }
                "LEVEL" => {
                    let level = integer(&name, value)?;
                    let level = u32::try_from(level).ok().filter(|&l| l <= MAX_LEVEL);
                    let level = level
                        .ok_or_else(|| format!("invalid LEVEL {}: expected 0 to {MAX_LEVEL}", value_text(value)))?;
                    writer = writer.with_level(level).map_err(|e| e.to_string())?;
                }
                "COLUMN_POLICY" => {
                    for spec in strings(&name, value)? {
                        let (column, policy) = parse_column_policy(&spec.to_ascii_lowercase_policy())
                            .map_err(|e| format!("COLUMN_POLICY: {e}"))?;
                        writer = writer.with_column(column, policy);
                    }
                }
                "COMPRESSION" => compression = Some(string(&name, value)?),
                "COMPRESSION_LEVEL" => compression_level = Some(integer(&name, value)?),
                "ROW_GROUP_ROWS" => {
                    let rows = integer(&name, value)?;
                    let rows = usize::try_from(rows).ok().filter(|&r| r > 0);
                    let rows = rows.ok_or_else(|| "ROW_GROUP_ROWS must be a positive number of rows".to_string())?;
                    properties = properties.set_max_row_group_row_count(Some(rows));
                }
                // DuckDB may pass the format along with the options
                "FORMAT" => {}
                _ => {
                    let known: Vec<&str> = OPTIONS.iter().map(|o| o.0).collect();
                    return Err(format!(
                        "unknown option {name} for FORMAT {FORMAT}; the options are {}",
                        known.join(", ")
                    ));
                }
            }
        }
        if compression.is_some() || compression_level.is_some() {
            writer = writer.with_base(base_codec(compression.as_deref().unwrap_or("zstd"), compression_level)?);
        }
        writer = writer.with_properties(properties.build());
        let (label, names) = match (names, query_names) {
            (Some(names), _) => ("COLUMN_NAMES", names),
            (None, Some(names)) => ("the query's column names", names),
            (None, None) => {
                return Err(format!(
                    "FORMAT {FORMAT} needs COLUMN_NAMES, e.g. COLUMN_NAMES ['a', 'b']: DuckDB's C API does not pass \
                     column names to COPY functions"
                ));
            }
        };
        if names.len() != types.len() {
            return Err(format!("{label} has {} names but the query has {} columns", names.len(), types.len()));
        }
        for (i, n) in names.iter().enumerate() {
            if n.is_empty() {
                return Err(format!("{label}: name {} is empty", i + 1));
            }
            if names[..i].contains(n) {
                return Err(format!("{label}: {n:?} appears twice"));
            }
        }
        let fields = names
            .iter()
            .zip(&types)
            .map(|(n, t)| Ok(Field::new(n, t.to_arrow().map_err(|e| format!("column \"{n}\": {e}"))?, true)))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { schema: Arc::new(Schema::new(fields)), column_types: types, writer })
    }
}

trait PolicySpec {
    fn to_ascii_lowercase_policy(&self) -> String;
}

impl PolicySpec for String {
    /// `NAME=POLICY` with the policy lower-cased (column names keep their case).
    fn to_ascii_lowercase_policy(&self) -> String {
        match self.rsplit_once('=') {
            Some((n, p)) => format!("{n}={}", p.to_ascii_lowercase()),
            None => self.clone(),
        }
    }
}

/// A file being written by a COPY.
pub struct CopyWriter {
    writer: Option<Writer<File>>,
    /// Batches held back until [`SAMPLE_ROWS`] rows or [`SAMPLE_BYTES`] have arrived (the first
    /// write decides the codecs).
    pending: Vec<RecordBatch>,
    pending_rows: usize,
    pending_bytes: usize,
    sampled: bool,
    schema: SchemaRef,
    pub rows: u64,
}

impl CopyWriter {
    /// Creates the file at `path`.
    pub fn create(path: &str, options: &CopyOptions) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| format!("cannot create {path}: {e}"))?;
        let writer =
            Writer::try_new(file, options.schema.clone(), options.writer.clone()).map_err(|e| e.to_string())?;
        Ok(Self {
            writer: Some(writer),
            pending: vec![],
            pending_rows: 0,
            pending_bytes: 0,
            sampled: false,
            schema: options.schema.clone(),
            rows: 0,
        })
    }

    fn writer(&mut self) -> Result<&mut Writer<File>, String> {
        self.writer.as_mut().ok_or_else(|| "the file is already closed".to_string())
    }

    fn write_pending(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let batch = arrow_select::concat::concat_batches(&self.schema, &self.pending).map_err(|e| e.to_string())?;
        self.pending.clear();
        self.pending_rows = 0;
        self.pending_bytes = 0;
        self.writer()?.write(&batch).map_err(|e| e.to_string())
    }

    /// Adds rows.
    pub fn write(&mut self, batch: RecordBatch) -> Result<(), String> {
        let n = batch.num_rows();
        if n == 0 {
            return Ok(());
        }
        self.rows += n as u64;
        if self.sampled {
            return self.writer()?.write(&batch).map_err(|e| e.to_string());
        }
        self.pending_rows += n;
        self.pending_bytes += batch.get_array_memory_size();
        self.pending.push(batch);
        if self.pending_rows >= SAMPLE_ROWS || self.pending_bytes >= SAMPLE_BYTES {
            self.sampled = true;
            self.write_pending()?;
        }
        Ok(())
    }

    /// Writes what is left and the footer.
    pub fn finish(&mut self) -> Result<(), String> {
        self.write_pending()?;
        let writer = self.writer.take().ok_or_else(|| "the file is already closed".to_string())?;
        // the Parquet writer buffers internally and flushes the file when it writes the footer
        writer.close().map(|_| ()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, Float64Array, Int64Array};
    use arrow_schema::DataType;
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use tparquet::Policy;

    fn opts(pairs: &[(&str, Val)]) -> Vec<(String, Val)> {
        pairs.iter().map(|(n, v)| (n.to_string(), v.clone())).collect()
    }

    fn s(v: &str) -> Val {
        Val::Str(v.into())
    }

    fn names(v: &[&str]) -> Val {
        Val::List(v.iter().map(|n| s(n)).collect())
    }

    fn two() -> Vec<SqlType> {
        vec![SqlType::BigInt, SqlType::Double]
    }

    #[test]
    fn column_names_name_the_columns() {
        let o = CopyOptions::parse(two(), None, &opts(&[("COLUMN_NAMES", names(&["id", "price"]))])).unwrap();
        assert_eq!(o.schema.field(0).name(), "id");
        assert_eq!(o.schema.field(1).data_type(), &DataType::Float64);
        assert!(o.schema.field(0).is_nullable());
        assert_eq!(o.column_types, two());
        assert_eq!(o.writer.policy, Policy::Auto);
        // one column may be named by a plain string
        let o = CopyOptions::parse(vec![SqlType::BigInt], None, &opts(&[("column_names", s("x"))])).unwrap();
        assert_eq!(o.schema.field(0).name(), "x");
    }

    #[test]
    fn column_names_are_required_and_checked() {
        let e = CopyOptions::parse(two(), None, &[]).unwrap_err();
        assert!(e.starts_with("FORMAT tparquet needs COLUMN_NAMES"), "{e}");
        let e = CopyOptions::parse(two(), None, &opts(&[("COLUMN_NAMES", names(&["a"]))])).unwrap_err();
        assert_eq!(e, "COLUMN_NAMES has 1 names but the query has 2 columns");
        let e = CopyOptions::parse(two(), None, &opts(&[("COLUMN_NAMES", names(&["a", "a"]))])).unwrap_err();
        assert_eq!(e, "COLUMN_NAMES: \"a\" appears twice");
        let e = CopyOptions::parse(two(), None, &opts(&[("COLUMN_NAMES", names(&["a", ""]))])).unwrap_err();
        assert_eq!(e, "COLUMN_NAMES: name 2 is empty");
        let e = CopyOptions::parse(two(), None, &opts(&[("COLUMN_NAMES", Val::Int(3))])).unwrap_err();
        assert_eq!(e, "COLUMN_NAMES must be a list of strings, not Int(3)");
        let e = CopyOptions::parse(two(), None, &opts(&[("COLUMN_NAMES", Val::List(vec![Val::Int(1), s("b")]))]));
        assert_eq!(e.unwrap_err(), "COLUMN_NAMES must be a string, not Int(1)");
        let e = CopyOptions::parse(vec![SqlType::Unsupported("UUID".into())], None, &opts(&[("COLUMN_NAMES", s("u"))]));
        assert_eq!(e.unwrap_err(), "column \"u\": columns of type UUID cannot be written to Parquet by this extension");
    }

    #[test]
    fn the_query_names_name_the_columns_unless_column_names_does() {
        let query = || Some(vec!["id".to_string(), "Price".to_string()]);
        let o = CopyOptions::parse(two(), query(), &[]).unwrap();
        assert_eq!(o.schema.field(0).name(), "id");
        assert_eq!(o.schema.field(1).name(), "Price");
        let o = CopyOptions::parse(two(), query(), &opts(&[("COLUMN_NAMES", names(&["a", "b"]))])).unwrap();
        assert_eq!(o.schema.field(1).name(), "b");
        let e = CopyOptions::parse(two(), query(), &opts(&[("COLUMN_NAMES", names(&["a"]))])).unwrap_err();
        assert_eq!(e, "COLUMN_NAMES has 1 names but the query has 2 columns");
        // a column policy names a column as the query does
        let o = CopyOptions::parse(two(), query(), &opts(&[("COLUMN_POLICY", s("Price=pcodec"))])).unwrap();
        assert_eq!(o.writer.columns, vec![("Price".to_string(), Policy::Pcodec)]);
        // names that do not fit (DuckDB makes a query's names unique and non-empty) are refused
        // without blaming COLUMN_NAMES
        let e = CopyOptions::parse(two(), Some(vec!["a".into(), "a".into()]), &[]).unwrap_err();
        assert_eq!(e, "the query's column names: \"a\" appears twice");
        let e = CopyOptions::parse(two(), Some(vec!["a".into()]), &[]).unwrap_err();
        assert_eq!(e, "the query's column names has 1 names but the query has 2 columns");
        let e = CopyOptions::parse(two(), Some(vec!["a".into(), "".into()]), &[]).unwrap_err();
        assert_eq!(e, "the query's column names: name 2 is empty");
    }

    fn with(pairs: &[(&str, Val)]) -> Result<CopyOptions, String> {
        let mut all = opts(&[("COLUMN_NAMES", names(&["id", "price"]))]);
        all.extend(opts(pairs));
        CopyOptions::parse(two(), None, &all)
    }

    #[test]
    fn policy_level_and_column_policies() {
        assert_eq!(with(&[("POLICY", s("PCODEC"))]).unwrap().writer.policy, Policy::Pcodec);
        assert_eq!(with(&[("POLICY", s("base"))]).unwrap().writer.policy, Policy::Base);
        assert_eq!(
            with(&[("POLICY", s("fast"))]).unwrap_err(),
            "unknown policy \"fast\": expected auto, pcodec or base"
        );
        assert_eq!(with(&[("POLICY", Val::Int(1))]).unwrap_err(), "POLICY must be a string, not Int(1)");
        assert_eq!(with(&[("LEVEL", Val::Int(12))]).unwrap().writer.level.compression_level(), 12);
        assert_eq!(with(&[("LEVEL", s("0"))]).unwrap().writer.level.compression_level(), 0);
        assert_eq!(with(&[("LEVEL", Val::Int(13))]).unwrap_err(), "invalid LEVEL 13: expected 0 to 12");
        assert_eq!(with(&[("LEVEL", Val::Int(-1))]).unwrap_err(), "invalid LEVEL -1: expected 0 to 12");
        assert_eq!(with(&[("LEVEL", s("x"))]).unwrap_err(), "LEVEL must be an integer, not \"x\"");
        assert_eq!(with(&[("LEVEL", Val::Bool(true))]).unwrap_err(), "LEVEL must be an integer, not Bool(true)");
        let o = with(&[("COLUMN_POLICY", names(&["Price=PCODEC", "id=base"]))]).unwrap();
        assert_eq!(o.writer.columns, vec![("Price".to_string(), Policy::Pcodec), ("id".to_string(), Policy::Base)]);
        assert!(with(&[("COLUMN_POLICY", s("price"))]).unwrap_err().starts_with("COLUMN_POLICY: expected NAME=policy"));
    }

    #[test]
    fn base_codec_and_row_groups() {
        assert_eq!(with(&[]).unwrap().writer.base, tparquet::writer::default_base());
        let base = |c: &str, l: Option<i64>| {
            let mut o = vec![("COMPRESSION", s(c))];
            if let Some(l) = l {
                o.push(("COMPRESSION_LEVEL", Val::Int(l)));
            }
            with(&o).map(|o| o.writer.base)
        };
        assert_eq!(base("zstd", Some(9)).unwrap(), Compression::ZSTD(ZstdLevel::try_new(9).unwrap()));
        assert_eq!(base("ZSTD", None).unwrap(), Compression::ZSTD(ZstdLevel::try_new(3).unwrap()));
        assert_eq!(base("gzip", None).unwrap(), Compression::GZIP(GzipLevel::try_new(6).unwrap()));
        assert_eq!(base("gzip", Some(9)).unwrap(), Compression::GZIP(GzipLevel::try_new(9).unwrap()));
        assert_eq!(base("brotli", None).unwrap(), Compression::BROTLI(BrotliLevel::try_new(1).unwrap()));
        assert_eq!(base("brotli", Some(11)).unwrap(), Compression::BROTLI(BrotliLevel::try_new(11).unwrap()));
        assert_eq!(base("snappy", None).unwrap(), Compression::SNAPPY);
        assert_eq!(base("lz4", None).unwrap(), Compression::LZ4_RAW);
        assert_eq!(base("lz4_raw", None).unwrap(), Compression::LZ4_RAW);
        assert_eq!(base("uncompressed", None).unwrap(), Compression::UNCOMPRESSED);
        assert_eq!(base("none", None).unwrap(), Compression::UNCOMPRESSED);
        assert_eq!(base("snappy", Some(1)).unwrap_err(), "COMPRESSION 'snappy' takes no COMPRESSION_LEVEL");
        assert!(base("zstd", Some(99)).is_err());
        assert!(base("zstd", Some(i64::MAX)).is_err());
        assert_eq!(base("gzip", Some(-1)).unwrap_err(), "invalid COMPRESSION_LEVEL -1");
        assert!(base("lzo", None).unwrap_err().starts_with("unknown COMPRESSION \"lzo\""));
        // a level alone applies to zstd
        let o = with(&[("COMPRESSION_LEVEL", Val::Int(9))]).unwrap();
        assert_eq!(o.writer.base, Compression::ZSTD(ZstdLevel::try_new(9).unwrap()));
        let o = with(&[("ROW_GROUP_ROWS", Val::Int(1000))]).unwrap();
        assert_eq!(o.writer.properties.max_row_group_row_count(), Some(1000));
        assert!(with(&[("ROW_GROUP_ROWS", Val::Int(0))]).is_err());
        assert_eq!(
            with(&[("ROW_GROUP_ROWS", Val::Int(-5))]).unwrap_err(),
            "ROW_GROUP_ROWS must be a positive number of rows"
        );
    }

    #[test]
    fn unknown_options_list_the_known_ones() {
        assert!(with(&[("FORMAT", s("tparquet"))]).is_ok());
        let e = with(&[("COMPRESION", s("zstd"))]).unwrap_err();
        assert_eq!(
            e,
            "unknown option COMPRESION for FORMAT tparquet; the options are COLUMN_NAMES, POLICY, LEVEL, \
             COLUMN_POLICY, COMPRESSION, COMPRESSION_LEVEL, ROW_GROUP_ROWS"
        );
    }

    fn batch(schema: &SchemaRef, from: i64, n: i64) -> RecordBatch {
        let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(from..from + n));
        let prices: ArrayRef = Arc::new(Float64Array::from_iter_values((from..from + n).map(|i| i as f64 * 0.01)));
        RecordBatch::try_new(schema.clone(), vec![ids, prices]).unwrap()
    }

    fn codecs(path: &std::path::Path) -> Vec<Compression> {
        let r = SerializedFileReader::new(File::open(path).unwrap()).unwrap();
        r.metadata().row_group(0).columns().iter().map(|c| c.compression()).collect()
    }

    #[test]
    fn writes_what_it_receives_after_a_sample() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.parquet");
        let o = with(&[("ROW_GROUP_ROWS", Val::Int(100_000))]).unwrap();
        let mut w = CopyWriter::create(path.to_str().unwrap(), &o).unwrap();
        let mut from = 0;
        // vectors of 2048 rows; the codecs are chosen once SAMPLE_ROWS have arrived
        while from < 300_000 {
            w.write(batch(&o.schema, from, 2048)).unwrap();
            from += 2048;
            assert_eq!(w.sampled, from as usize >= SAMPLE_ROWS);
            // 64 vectors of 2048 rows make the sample
            assert_eq!(w.sampled, from >= 64 * 2048, "{from}");
        }
        w.write(batch(&o.schema, from, 0)).unwrap();
        assert_eq!(w.rows, from as u64);
        w.finish().unwrap();
        assert_eq!(w.finish().unwrap_err(), "the file is already closed");
        assert_eq!(w.write(batch(&o.schema, 0, 1)).unwrap_err(), "the file is already closed");
        let read = tparquet::read::open(&path).unwrap().build().unwrap();
        let mut next = 0i64;
        for b in read {
            let b = b.unwrap();
            let ids = b.column(0).as_any().downcast_ref::<Int64Array>().unwrap();
            for id in ids.values() {
                assert_eq!(*id, next);
                next += 1;
            }
        }
        assert_eq!(next, from);
        // Auto picks PCODEC for both columns of this sequence data
        assert!(codecs(&path).iter().all(|c| matches!(c, Compression::PCODEC(_))), "{:?}", codecs(&path));
    }

    #[test]
    fn a_sample_of_long_strings_ends_at_sample_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.parquet");
        let o = CopyOptions::parse(vec![SqlType::Varchar], None, &opts(&[("COLUMN_NAMES", s("doc"))])).unwrap();
        let mut w = CopyWriter::create(path.to_str().unwrap(), &o).unwrap();
        // vectors of 2048 strings of 4 KiB: 8 MiB each, far fewer rows than SAMPLE_ROWS
        let doc = "x".repeat(4096);
        let vector = || {
            let docs: ArrayRef = Arc::new(arrow_array::StringArray::from_iter_values((0..2048).map(|_| doc.as_str())));
            RecordBatch::try_new(o.schema.clone(), vec![docs]).unwrap()
        };
        let mut held = 0;
        while !w.sampled {
            w.write(vector()).unwrap();
            held += 2048 * 4096;
            assert!(held < 2 * SAMPLE_BYTES, "the sample holds {held} bytes");
        }
        assert!(w.rows < SAMPLE_ROWS as u64);
        assert!(held >= SAMPLE_BYTES);
        assert!(w.pending.is_empty());
        w.write(vector()).unwrap();
        w.finish().unwrap();
        let rows: usize = tparquet::read::open(&path).unwrap().build().unwrap().map(|b| b.unwrap().num_rows()).sum();
        assert_eq!(rows as u64, w.rows);
    }

    #[test]
    fn small_and_empty_copies() {
        let dir = tempfile::tempdir().unwrap();
        let o = with(&[("POLICY", s("base"))]).unwrap();
        let small = dir.path().join("small.parquet");
        let mut w = CopyWriter::create(small.to_str().unwrap(), &o).unwrap();
        w.write(batch(&o.schema, 0, 10)).unwrap();
        w.write(batch(&o.schema, 10, 5)).unwrap();
        assert!(!w.sampled);
        w.finish().unwrap();
        let rows: usize = tparquet::read::open(&small).unwrap().build().unwrap().map(|b| b.unwrap().num_rows()).sum();
        assert_eq!(rows, 15);
        assert!(codecs(&small).iter().all(|c| matches!(c, Compression::ZSTD(_))));

        let empty = dir.path().join("empty.parquet");
        let mut w = CopyWriter::create(empty.to_str().unwrap(), &o).unwrap();
        w.finish().unwrap();
        let r = tparquet::read::open(&empty).unwrap();
        assert_eq!(r.schema().fields().len(), 2);
        assert_eq!(r.metadata().file_metadata().num_rows(), 0);

        let e = CopyWriter::create(dir.path().join("no/such/dir.parquet").to_str().unwrap(), &o).err().unwrap();
        assert!(e.starts_with("cannot create "), "{e}");
    }
}
