//! `read_tparquet`: which files, which columns, and the batches they yield.
//!
//! The DuckDB callbacks in [`crate::read`] only move values between DuckDB and this module, so
//! everything here is tested without a database:
//!
//! - [`expand`] turns the path argument (a file, a glob, or a list of them) into files of a
//!   [`FileSystem`] (DuckDB's, in the extension);
//! - [`Bound::bind`] reads the schema (of the first file, or of every file with
//!   `union_by_name`) and gives the result columns and their DuckDB types;
//! - [`Scan`] hands out row groups, one at a time, to the threads DuckDB runs, and each thread
//!   reads its row group in batches of at most one DuckDB vector.

use crate::fs::{FileSystem, Input, in_home, is_remote};
use crate::prune::{Predicate, Reading, Stats, may_match, reading, stats_of};
use crate::types::{SqlType, deduplicate, normalize, whole_intervals};
use arrow_array::{ArrayRef, new_null_array};
use arrow_schema::{DataType, Fields, Schema, SchemaRef};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tparquet::read::ParquetRecordBatchReader;

/// The name of the table function.
pub const FUNCTION: &str = "read_tparquet";
/// Name of the column `filename := true` adds.
pub const FILENAME: &str = "filename";
/// Name of the column `file_row_number := true` adds.
pub const FILE_ROW_NUMBER: &str = "file_row_number";

/// Checks that the bound columns (`have`) are the columns of the table a `COPY ... FROM` fills
/// (`table`), by position and type.
pub fn check_copy_from(have: &[SqlType], table: &[SqlType]) -> Result<(), String> {
    if have == table {
        return Ok(());
    }
    let show = |ts: &[SqlType]| ts.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ");
    Err(format!(
        "the file columns ({}) do not match the table columns ({}) by position and type; \
         use INSERT INTO ... SELECT ... FROM {FUNCTION}(...) to map or cast them",
        show(have),
        show(table),
    ))
}

/// The named parameters of `read_tparquet`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadOptions {
    /// Add a `filename` column.
    pub filename: bool,
    /// Add a `file_row_number` column: the row's position in its file, from 0.
    pub file_row_number: bool,
    /// Combine files by column name; columns a file lacks are NULL.
    pub union_by_name: bool,
}

fn is_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

/// The files that `patterns` name, in order: a pattern with `*`, `?` or `[` is a glob whose
/// matches (in `fs`) are sorted; any other pattern is a file. A local file must exist (a remote
/// one is checked when it is opened, which costs a request). `~/` is the home directory.
pub fn expand(patterns: &[String], fs: &dyn FileSystem) -> Result<Vec<String>, String> {
    if patterns.is_empty() {
        return Err("no files given".into());
    }
    let mut files = vec![];
    for given in patterns {
        let pattern = &in_home(given, std::env::home_dir());
        if is_glob(pattern) {
            let mut matched = fs.glob(pattern)?;
            if matched.is_empty() {
                return Err(format!("no files match {given:?}"));
            }
            matched.sort();
            files.extend(matched);
        } else {
            if !is_remote(pattern) && !Path::new(pattern).is_file() {
                return Err(format!("no such file: {given:?}"));
            }
            files.push(pattern.clone());
        }
    }
    Ok(files)
}

/// A result column that comes from the files.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    pub sql: SqlType,
    /// The Arrow type of the column in the first file that has it (for NULL fill).
    pub arrow: DataType,
}

/// Where a projected result column comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The file column [`Bound::columns`]`[i]`, the `i`-th projected file column.
    File {
        column: usize,
        projected: usize,
    },
    Filename,
    FileRowNumber,
    /// A column DuckDB asks for that is none of ours (the row id of `count(*)`); left empty.
    Other,
}

/// The footer of a file, with its intervals read whole ([`whole_intervals`]).
fn open_metadata(fs: &dyn FileSystem, path: &str) -> Result<ArrowReaderMetadata, String> {
    let file = fs.open(path)?;
    let unreadable = |e: parquet::errors::ParquetError| format!("cannot read {path} as Parquet: {e}");
    let metadata = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new()).map_err(unreadable)?;
    let schema = metadata.schema();
    let fields: Fields = schema
        .fields()
        .iter()
        .map(|f| Arc::new(f.as_ref().clone().with_data_type(whole_intervals(f.data_type()))))
        .collect();
    if &fields == schema.fields() {
        return Ok(metadata);
    }
    let hint = Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));
    ArrowReaderMetadata::try_new(metadata.metadata().clone(), ArrowReaderOptions::new().with_schema(hint))
        .map_err(unreadable)
}

/// The columns of a file, named as DuckDB names them ([`deduplicate`]).
fn columns_of(path: &str, schema: &SchemaRef) -> Result<Vec<Column>, String> {
    let names = deduplicate(schema.fields().iter().map(|f| f.name().clone()).collect());
    schema
        .fields()
        .iter()
        .zip(names)
        .map(|(f, name)| {
            Ok(Column {
                name,
                sql: SqlType::from_arrow(f).map_err(|e| format!("{path}: {e}"))?,
                arrow: f.data_type().clone(),
            })
        })
        .collect()
}

/// Whether two column names are the same name in DuckDB (which ignores case).
fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The result of binding: the files, the result columns and the options.
pub struct Bound {
    /// Where the files are read from.
    pub fs: Arc<dyn FileSystem>,
    pub files: Vec<String>,
    pub columns: Vec<Column>,
    pub options: ReadOptions,
    /// File metadata already read during bind, by file index.
    metadata: Vec<Option<ArrowReaderMetadata>>,
    /// Rows of the files read during bind, extrapolated to all files.
    pub estimated_rows: u64,
    /// Whether [`Bound::estimated_rows`] is exact (every file was read).
    pub exact_rows: bool,
}

impl fmt::Debug for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bound")
            .field("files", &self.files)
            .field("columns", &self.columns)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl Bound {
    /// Reads the schema: of the first file, or of all of them with `union_by_name`.
    pub fn bind(fs: Arc<dyn FileSystem>, files: Vec<String>, options: ReadOptions) -> Result<Self, String> {
        let first = files.first().ok_or("no files given")?;
        let mut metadata = vec![None; files.len()];
        let meta = open_metadata(fs.as_ref(), first)?;
        let mut columns = columns_of(first, meta.schema())?;
        let mut rows = meta.metadata().file_metadata().num_rows().max(0) as u64;
        metadata[0] = Some(meta);
        if options.union_by_name {
            for (i, path) in files.iter().enumerate().skip(1) {
                let meta = open_metadata(fs.as_ref(), path)?;
                for c in columns_of(path, meta.schema())? {
                    match columns.iter().find(|have| same_name(&have.name, &c.name)) {
                        None => columns.push(c),
                        Some(have) if have.sql != c.sql => {
                            return Err(format!(
                                "column \"{}\" is {} in {first} but {} in {path}",
                                c.name, have.sql, c.sql
                            ));
                        }
                        Some(_) => {}
                    }
                }
                rows += meta.metadata().file_metadata().num_rows().max(0) as u64;
                metadata[i] = Some(meta);
            }
        } else {
            rows *= files.len() as u64;
        }
        for extra in [(options.filename, FILENAME), (options.file_row_number, FILE_ROW_NUMBER)] {
            if extra.0 && columns.iter().any(|c| same_name(&c.name, extra.1)) {
                return Err(format!("the files already have a column \"{}\"", extra.1));
            }
        }
        if columns.is_empty() && !options.filename && !options.file_row_number {
            // DuckDB takes a table function without result columns for an internal error
            return Err(format!("{first} has no columns"));
        }
        let exact_rows = options.union_by_name || files.len() == 1;
        Ok(Self { fs, files, columns, options, metadata, estimated_rows: rows, exact_rows })
    }

    /// The names and types of the result columns: the file columns, then `filename` and
    /// `file_row_number` if asked for.
    pub fn result_columns(&self) -> Vec<(String, SqlType)> {
        let mut out: Vec<(String, SqlType)> = self.columns.iter().map(|c| (c.name.clone(), c.sql.clone())).collect();
        if self.options.filename {
            out.push((FILENAME.into(), SqlType::Varchar));
        }
        if self.options.file_row_number {
            out.push((FILE_ROW_NUMBER.into(), SqlType::BigInt));
        }
        out
    }

    /// What each column DuckDB asks for (by result column index) holds.
    pub fn slots(&self, column_indices: &[u64]) -> Vec<Slot> {
        let n = self.columns.len() as u64;
        let filename = self.options.filename.then_some(n);
        let row_number = self.options.file_row_number.then(|| n + u64::from(self.options.filename));
        let mut projected = 0;
        column_indices
            .iter()
            .map(|&i| {
                if i < n {
                    projected += 1;
                    Slot::File { column: i as usize, projected: projected - 1 }
                } else if Some(i) == filename {
                    Slot::Filename
                } else if Some(i) == row_number {
                    Slot::FileRowNumber
                } else {
                    Slot::Other
                }
            })
            .collect()
    }
}

/// A file being scanned.
#[derive(Debug)]
pub struct OpenFile {
    /// The path, as the `filename` column shows it.
    pub path: String,
    metadata: ArrowReaderMetadata,
    /// For each projected file column, its top-level field index in this file.
    fields: Vec<Option<usize>>,
    /// The file row number of the first row of each row group.
    row_group_starts: Vec<u64>,
    /// For each column of [`Bound::columns`]: whether this file has it, and the leaf column
    /// and [`Reading`] of its statistics (for predicates; `None` without predicates).
    statistics: Option<Vec<FileColumn>>,
}

/// What a file holds of a bound column, for row-group skipping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileColumn {
    Absent,
    Stats { leaf: usize, reading: Reading },
    Unknown,
}

/// The leaf column of the top-level field `root`, if that field is a primitive column.
fn leaf_of_root(metadata: &ArrowReaderMetadata, root: usize) -> Option<usize> {
    let schema = metadata.parquet_schema();
    if !schema.root_schema().get_fields().get(root)?.is_primitive() {
        return None;
    }
    (0..schema.num_columns()).find(|&leaf| schema.get_column_root_idx(leaf) == root)
}

impl OpenFile {
    fn new(
        path: String,
        metadata: ArrowReaderMetadata,
        bound: &Bound,
        projected: &[usize],
        with_statistics: bool,
    ) -> Result<Self, String> {
        let file_columns = columns_of(&path, metadata.schema())?;
        // where each bound column is in this file (a top-level field index)
        let position = |want: &Column| -> Option<usize> {
            if bound.options.union_by_name {
                file_columns.iter().position(|f| same_name(&f.name, &want.name) && f.sql == want.sql)
            } else {
                bound.columns.iter().position(|c| std::ptr::eq(c, want))
            }
        };
        let reference = &bound.files[0];
        let fields = if bound.options.union_by_name {
            projected
                .iter()
                .map(|&c| {
                    let want = &bound.columns[c];
                    match file_columns.iter().position(|f| same_name(&f.name, &want.name)) {
                        Some(i) if file_columns[i].sql != want.sql => Err(format!(
                            "column \"{}\" is {} in {path}, but {} was bound",
                            want.name, file_columns[i].sql, want.sql
                        )),
                        found => Ok(found),
                    }
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            let same = file_columns.len() == bound.columns.len()
                && file_columns.iter().zip(&bound.columns).all(|(a, b)| same_name(&a.name, &b.name) && a.sql == b.sql);
            if !same {
                let describe =
                    |cs: &[Column]| cs.iter().map(|c| format!("{} {}", c.name, c.sql)).collect::<Vec<_>>().join(", ");
                return Err(format!(
                    "{path} has the columns ({}) but {reference} has ({}); the files must have the same schema, \
                     or pass union_by_name := true",
                    describe(&file_columns),
                    describe(&bound.columns)
                ));
            }
            projected.iter().map(|&c| Some(c)).collect()
        };
        let mut row_group_starts = vec![];
        let mut start = 0u64;
        for rg in metadata.metadata().row_groups() {
            row_group_starts.push(start);
            start += rg.num_rows().max(0) as u64;
        }
        let statistics = with_statistics.then(|| {
            bound
                .columns
                .iter()
                .map(|c| match position(c) {
                    None => FileColumn::Absent,
                    Some(root) => leaf_of_root(&metadata, root)
                        .and_then(|leaf| {
                            reading(metadata.metadata(), leaf, &c.sql)
                                .map(|reading| FileColumn::Stats { leaf, reading })
                        })
                        .unwrap_or(FileColumn::Unknown),
                })
                .collect()
        });
        Ok(Self { path, metadata, fields, row_group_starts, statistics })
    }

    fn row_groups(&self) -> usize {
        self.row_group_starts.len()
    }

    /// Whether a row of `row_group` may satisfy every predicate.
    fn may_match(&self, row_group: usize, predicates: &[Predicate]) -> bool {
        let Some(columns) = &self.statistics else { return true };
        let rg = self.metadata.metadata().row_group(row_group);
        let rows = rg.num_rows().max(0) as u64;
        let stats = |c: usize| -> Option<Stats> {
            match columns.get(c)? {
                FileColumn::Absent => Some(Stats::all_null(rows)),
                FileColumn::Stats { leaf, reading } => Some(stats_of(rg.column(*leaf).statistics(), *reading, rows)),
                FileColumn::Unknown => None,
            }
        };
        predicates.iter().all(|p| may_match(p, &stats))
    }

    /// A reader of one row group, with the projected columns.
    fn reader(
        &self,
        fs: &dyn FileSystem,
        row_group: usize,
        batch_size: usize,
    ) -> Result<(ParquetRecordBatchReader, Vec<Option<usize>>), String> {
        let file: Input = fs.open(&self.path)?;
        let mut roots: Vec<usize> = self.fields.iter().flatten().copied().collect();
        roots.sort_unstable();
        roots.dedup();
        // the reader returns the selected roots in file order
        let positions = self.fields.iter().map(|f| f.map(|f| roots.binary_search(&f).unwrap_or(0))).collect();
        let mask = ProjectionMask::roots(self.metadata.parquet_schema(), roots);
        let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(file, self.metadata.clone())
            .with_row_groups(vec![row_group])
            .with_projection(mask)
            .with_batch_size(batch_size)
            .build()
            .map_err(|e| format!("cannot read {}: {e}", self.path))?;
        Ok((reader, positions))
    }
}

/// Rows of one file, ready for DuckDB.
#[derive(Debug)]
pub struct ScanBatch {
    pub file: Arc<OpenFile>,
    /// The position of the batch's row group among the row groups of all the files, in file
    /// order: DuckDB's batch index, by which it restores the order of rows read in parallel.
    pub batch: u64,
    /// The file row number of the first row.
    pub first_row: u64,
    pub num_rows: usize,
    /// One array per projected file column, normalized ([`crate::types::normalize`]).
    pub columns: Vec<ArrayRef>,
}

#[derive(Debug, Default)]
struct Queue {
    next_file: usize,
    current: Option<(Arc<OpenFile>, usize)>,
    /// The batch index of the next row group.
    next_batch: u64,
    /// Bind's metadata, taken when the file is opened.
    metadata: Vec<Option<ArrowReaderMetadata>>,
}

/// The state of a scan shared by DuckDB's threads.
#[derive(Debug)]
pub struct Scan {
    bound: Arc<Bound>,
    /// The projected file columns, as indices into [`Bound::columns`].
    projected: Vec<usize>,
    batch_size: usize,
    /// Row groups where no row can satisfy these are skipped.
    predicates: Vec<Predicate>,
    queue: Mutex<Queue>,
}

/// One thread's position in a scan.
#[derive(Default)]
pub struct Local {
    reader: Option<Reader>,
}

struct Reader {
    file: Arc<OpenFile>,
    reader: ParquetRecordBatchReader,
    positions: Vec<Option<usize>>,
    next_row: u64,
    batch: u64,
}

impl Scan {
    /// A scan of `bound` that yields `slots` in batches of at most `batch_size` rows.
    pub fn new(bound: Arc<Bound>, slots: &[Slot], batch_size: usize) -> Self {
        let projected = slots
            .iter()
            .filter_map(|s| match s {
                Slot::File { column, .. } => Some(*column),
                _ => None,
            })
            .collect();
        let metadata = bound.metadata.clone();
        Self {
            bound,
            projected,
            batch_size: batch_size.max(1),
            predicates: vec![],
            queue: Mutex::new(Queue { metadata, ..Default::default() }),
        }
    }

    /// The scan, skipping row groups where no row can satisfy all of `predicates`.
    pub fn with_predicates(mut self, predicates: Vec<Predicate>) -> Self {
        self.predicates = predicates;
        self
    }

    /// How many threads could work at once: one per row group, as far as bind knows.
    pub fn max_threads(&self) -> u64 {
        let first = self.bound.metadata[0].as_ref().map_or(1, |m| m.metadata().num_row_groups().max(1));
        (first as u64).saturating_mul(self.bound.files.len() as u64)
    }

    /// The next row group to read, opening files as needed.
    fn next_unit(&self) -> Result<Option<(Arc<OpenFile>, usize, u64)>, String> {
        let mut q = self.queue.lock().map_err(|_| "a reader thread panicked")?;
        loop {
            if let Some((file, rg)) = q.current.take()
                && rg < file.row_groups()
            {
                q.current = Some((file.clone(), rg + 1));
                let batch = q.next_batch;
                q.next_batch += 1;
                if !file.may_match(rg, &self.predicates) {
                    continue;
                }
                return Ok(Some((file, rg, batch)));
            }
            let i = q.next_file;
            let Some(path) = self.bound.files.get(i) else { return Ok(None) };
            q.next_file += 1;
            let metadata = match q.metadata[i].take() {
                Some(m) => m,
                None => open_metadata(self.bound.fs.as_ref(), path)?,
            };
            let file =
                OpenFile::new(path.clone(), metadata, &self.bound, &self.projected, !self.predicates.is_empty())?;
            q.current = Some((Arc::new(file), 0));
        }
    }

    /// The next batch for this thread; `None` when the scan is done.
    pub fn next(&self, local: &mut Local) -> Result<Option<ScanBatch>, String> {
        loop {
            if let Some(r) = &mut local.reader {
                // the Parquet reader panics on some damaged pages (its level decoder): say which file
                let next = crate::ffi::guard(|| Ok(r.reader.next()))
                    .map_err(|e| format!("cannot read {}: {e}", r.file.path))?;
                match next {
                    Some(Ok(batch)) if batch.num_rows() > 0 => {
                        let num_rows = batch.num_rows();
                        let columns = r
                            .positions
                            .iter()
                            .zip(&self.projected)
                            .map(|(p, &c)| match p {
                                Some(p) => normalize(batch.column(*p)),
                                None => normalize(&new_null_array(&self.bound.columns[c].arrow, num_rows)),
                            })
                            .collect::<Result<Vec<_>, String>>()
                            .map_err(|e| format!("{}: {e}", r.file.path))?;
                        let first_row = r.next_row;
                        r.next_row += num_rows as u64;
                        let batch = r.batch;
                        return Ok(Some(ScanBatch { file: r.file.clone(), batch, first_row, num_rows, columns }));
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Err(format!("cannot read {}: {e}", r.file.path)),
                    None => local.reader = None,
                }
            }
            let Some((file, rg, batch)) = self.next_unit()? else { return Ok(None) };
            let (reader, positions) = file.reader(self.bound.fs.as_ref(), rg, self.batch_size)?;
            let next_row = file.row_group_starts[rg];
            local.reader = Some(Reader { file, reader, positions, next_row, batch });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::LocalFs;
    use arrow_array::{Array, Int32Array, Int64Array, RecordBatch, StringArray};
    use arrow_schema::Field;
    use parquet::file::properties::WriterProperties;
    use std::fs::File;
    use std::path::PathBuf;
    use tparquet::{Policy, Writer, WriterOptions};

    /// Writes `batch` with PCODEC to `dir/name`, `rows_per_group` rows per row group.
    fn write(dir: &Path, name: &str, batch: &RecordBatch, rows_per_group: usize) -> String {
        let path = dir.join(name).to_string_lossy().into_owned();
        let props = WriterProperties::builder().set_max_row_group_row_count(Some(rows_per_group)).build();
        let options = WriterOptions::default().with_policy(Policy::Pcodec).with_properties(props);
        let mut w = Writer::try_new(File::create(&path).unwrap(), batch.schema(), options).unwrap();
        w.write(batch).unwrap();
        w.close().unwrap();
        path
    }

    fn ints(n: i64, offset: i64) -> RecordBatch {
        let ids = Int64Array::from_iter_values(offset..offset + n);
        let names = StringArray::from_iter_values((offset..offset + n).map(|i| format!("n{i}")));
        RecordBatch::try_from_iter([("id", Arc::new(ids) as ArrayRef), ("name", Arc::new(names) as ArrayRef)]).unwrap()
    }

    /// Binds local files.
    fn bind(files: Vec<String>, options: ReadOptions) -> Result<Bound, String> {
        Bound::bind(Arc::new(LocalFs), files, options)
    }

    fn local(p: PathBuf) -> String {
        p.to_string_lossy().into_owned()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// More batches than any test file has: a scan that goes past it does not end. Without the
    /// bound, a mutant that makes the scan endless fills memory (about 70 MB/s here) before
    /// cargo-mutants' timeout, which shut down the CI runners.
    const ENDLESS: usize = 10_000;

    /// Every batch the scan gives `local`; panics if the scan does not end.
    fn drain(scan: &Scan, local: &mut Local) -> Vec<ScanBatch> {
        let mut out = vec![];
        while let Some(b) = scan.next(local).unwrap() {
            out.push(b);
            assert!(out.len() < ENDLESS, "the scan does not end");
        }
        out
    }

    fn all_rows(scan: &Scan) -> Vec<ScanBatch> {
        drain(scan, &mut Local::default())
    }

    fn ids(batches: &[ScanBatch], col: usize) -> Vec<Option<i64>> {
        batches
            .iter()
            .flat_map(|b| {
                let a = b.columns[col].as_any().downcast_ref::<Int64Array>().unwrap();
                a.iter().collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn expands_files_and_sorted_globs() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["b.parquet", "a.parquet", "c.txt"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        std::fs::create_dir(local(dir.path().join("d.parquet"))).unwrap();
        let d = dir.path().display();
        let files = expand(&[format!("{d}/*.parquet")], &LocalFs).unwrap();
        assert_eq!(files, vec![local(dir.path().join("a.parquet")), local(dir.path().join("b.parquet"))]);
        let files = expand(&[format!("{d}/c.txt"), format!("{d}/?.parquet")], &LocalFs).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0], local(dir.path().join("c.txt")));
        assert_eq!(expand(&[format!("{d}/[ab].parquet")], &LocalFs).unwrap().len(), 2);
        assert_eq!(expand(&[format!("{d}/*.csv")], &LocalFs).unwrap_err(), format!("no files match \"{d}/*.csv\""));
        assert_eq!(
            expand(&[format!("{d}/x.parquet")], &LocalFs).unwrap_err(),
            format!("no such file: \"{d}/x.parquet\"")
        );
        // a directory is not a file
        assert!(expand(&[format!("{d}/d.parquet")], &LocalFs).is_err());
        assert!(expand(&[format!("{d}/[.parquet")], &LocalFs).unwrap_err().starts_with("invalid glob"));
        assert_eq!(expand(&[], &LocalFs).unwrap_err(), "no files given");
    }

    #[test]
    fn a_leading_tilde_is_the_home_directory() {
        let home = || Some(PathBuf::from("/home/me"));
        assert_eq!(in_home("~/a/b.parquet", home()), "/home/me/a/b.parquet");
        assert_eq!(in_home("~/*.parquet", home()), "/home/me/*.parquet");
        for kept in ["~a.parquet", "a/~/b", "/x/~/y", "~", "./~/z", "a.parquet"] {
            assert_eq!(in_home(kept, home()), kept);
        }
        assert_eq!(in_home("~/a", None), "~/a");
        // expand finds files there, and names the pattern as given
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(local(dir.path().join("f.parquet")), b"x").unwrap();
        let home_dir = std::env::home_dir().unwrap();
        let relative = dir.path().strip_prefix(&home_dir);
        if let Ok(rest) = relative {
            let p = format!("~/{}/f.parquet", rest.display());
            assert_eq!(expand(std::slice::from_ref(&p), &LocalFs).unwrap(), vec![local(dir.path().join("f.parquet"))]);
        }
        assert_eq!(expand(&["~/no/such.parquet".into()], &LocalFs).unwrap_err(), "no such file: \"~/no/such.parquet\"");
        assert_eq!(
            expand(&["~/no/such/*.parquet".into()], &LocalFs).unwrap_err(),
            "no files match \"~/no/such/*.parquet\""
        );
    }

    #[test]
    fn binds_the_columns_of_the_first_file() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(10, 0), 4);
        let b = write(dir.path(), "b.parquet", &ints(5, 10), 4);
        let bound = bind(vec![a.clone()], ReadOptions::default()).unwrap();
        assert_eq!(bound.result_columns(), vec![("id".into(), SqlType::BigInt), ("name".into(), SqlType::Varchar)]);
        assert_eq!(bound.columns[0].arrow, DataType::Int64);
        assert_eq!((bound.estimated_rows, bound.exact_rows), (10, true));
        let bound = bind(vec![a.clone(), b.clone()], ReadOptions::default()).unwrap();
        assert_eq!((bound.estimated_rows, bound.exact_rows), (20, false));
        let opts = ReadOptions { filename: true, file_row_number: true, union_by_name: false };
        let bound = bind(vec![a.clone()], opts).unwrap();
        let names: Vec<String> = bound.result_columns().into_iter().map(|c| c.0).collect();
        assert_eq!(names, strings(&["id", "name", "filename", "file_row_number"]));
        assert_eq!(bound.result_columns()[2].1, SqlType::Varchar);
        assert_eq!(bound.result_columns()[3].1, SqlType::BigInt);
        let opts = ReadOptions { file_row_number: true, ..Default::default() };
        assert_eq!(bound_names(&bind(vec![a.clone()], opts).unwrap()), strings(&["id", "name", "file_row_number"]));
        assert!(bind(vec![], ReadOptions::default()).is_err());
    }

    fn bound_names(b: &Bound) -> Vec<String> {
        b.result_columns().into_iter().map(|c| c.0).collect()
    }

    #[test]
    fn refuses_extra_columns_the_file_has() {
        let dir = tempfile::tempdir().unwrap();
        let batch = RecordBatch::try_from_iter([
            ("filename", Arc::new(Int32Array::from(vec![1])) as ArrayRef),
            ("file_row_number", Arc::new(Int32Array::from(vec![1])) as ArrayRef),
        ])
        .unwrap();
        let f = write(dir.path(), "f.parquet", &batch, 10);
        assert!(bind(vec![f.clone()], ReadOptions::default()).is_ok());
        let e = bind(vec![f.clone()], ReadOptions { filename: true, ..Default::default() }).unwrap_err();
        assert_eq!(e, "the files already have a column \"filename\"");
        let e = bind(vec![f], ReadOptions { file_row_number: true, ..Default::default() }).unwrap_err();
        assert_eq!(e, "the files already have a column \"file_row_number\"");
    }

    #[test]
    fn a_file_without_columns_needs_an_extra_column() {
        let dir = tempfile::tempdir().unwrap();
        let empty = RecordBatch::new_empty(Arc::new(Schema::empty()));
        let f = write(dir.path(), "f.parquet", &empty, 10);
        let e = bind(vec![f.clone()], ReadOptions::default()).unwrap_err();
        assert_eq!(e, format!("{} has no columns", f));
        let e = bind(vec![f.clone(), f.clone()], ReadOptions { union_by_name: true, ..Default::default() });
        assert!(e.is_err());
        for opts in [
            ReadOptions { filename: true, ..Default::default() },
            ReadOptions { file_row_number: true, ..Default::default() },
        ] {
            assert_eq!(bind(vec![f.clone()], opts).unwrap().result_columns().len(), 1);
        }
    }

    #[test]
    fn explains_files_that_are_not_parquet() {
        let dir = tempfile::tempdir().unwrap();
        let p = local(dir.path().join("x.parquet"));
        std::fs::write(&p, b"not parquet at all").unwrap();
        let e = bind(vec![p.clone()], ReadOptions::default()).unwrap_err();
        assert!(e.starts_with(&format!("cannot read {} as Parquet: ", p)), "{e}");
        let e = bind(vec![local(dir.path().join("missing"))], ReadOptions::default()).unwrap_err();
        assert!(e.starts_with("cannot open "), "{e}");
    }

    #[test]
    fn maps_projected_columns_to_slots() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(3, 0), 4);
        let opts = ReadOptions { filename: true, file_row_number: true, union_by_name: false };
        let bound = bind(vec![a.clone()], opts).unwrap();
        assert_eq!(
            bound.slots(&[3, 1, 2, 0, 4, u64::MAX]),
            vec![
                Slot::FileRowNumber,
                Slot::File { column: 1, projected: 0 },
                Slot::Filename,
                Slot::File { column: 0, projected: 1 },
                Slot::Other,
                Slot::Other
            ]
        );
        let bound = bind(vec![a.clone()], ReadOptions { file_row_number: true, ..Default::default() }).unwrap();
        assert_eq!(bound.slots(&[2]), vec![Slot::FileRowNumber]);
        let bound = bind(vec![a], ReadOptions::default()).unwrap();
        assert_eq!(bound.slots(&[2, 3]), vec![Slot::Other, Slot::Other]);
    }

    #[test]
    fn scans_every_row_group_of_every_file_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(10, 0), 4);
        let b = write(dir.path(), "b.parquet", &ints(5, 10), 4);
        let bound = Arc::new(bind(vec![a.clone(), b.clone()], ReadOptions::default()).unwrap());
        let slots = bound.slots(&[0, 1]);
        let scan = Scan::new(bound.clone(), &slots, 3);
        assert_eq!(scan.max_threads(), 6);
        let batches = all_rows(&scan);
        assert_eq!(ids(&batches, 0), (0..15).map(Some).collect::<Vec<_>>());
        assert!(batches.iter().all(|b| b.num_rows <= 3 && b.num_rows == b.columns[0].len()));
        // row groups of 4, 4, 2 and 4, 1, read 3 rows at a time
        let shape: Vec<(String, u64, usize)> = batches
            .iter()
            .map(|b| (b.file.path.rsplit('/').next().unwrap().to_string(), b.first_row, b.num_rows))
            .collect();
        let expect =
            [("a", 0, 3), ("a", 3, 1), ("a", 4, 3), ("a", 7, 1), ("a", 8, 2), ("b", 0, 3), ("b", 3, 1), ("b", 4, 1)];
        let expect: Vec<(String, u64, usize)> =
            expect.iter().map(|(f, r, n)| (format!("{f}.parquet"), *r, *n)).collect();
        assert_eq!(shape, expect);
        assert_eq!(batches[0].file.path, a);
        let names = batches[5].columns[1].as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(names.value(0), "n10");
    }

    #[test]
    fn reads_only_projected_columns() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(5, 0), 100);
        let bound = Arc::new(bind(vec![a], ReadOptions { filename: true, ..Default::default() }).unwrap());
        // name, then id: the reader returns them in file order, the scan in projection order
        let scan = Scan::new(bound.clone(), &bound.slots(&[1, 2, 0]), 2048);
        let batches = all_rows(&scan);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].columns.len(), 2);
        assert_eq!(batches[0].columns[0].data_type(), &DataType::Utf8);
        assert_eq!(ids(&batches, 1), (0..5).map(Some).collect::<Vec<_>>());
        // no file column at all (count(*), or only the filename): rows without columns
        let scan = Scan::new(bound.clone(), &bound.slots(&[2]), 2);
        let batches = all_rows(&scan);
        assert_eq!(batches.iter().map(|b| b.num_rows).collect::<Vec<_>>(), vec![2, 2, 1]);
        assert!(batches.iter().all(|b| b.columns.is_empty()));
    }

    #[test]
    fn threads_share_the_row_groups() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(100, 0), 10);
        let bound = Arc::new(bind(vec![a], ReadOptions::default()).unwrap());
        let scan = Arc::new(Scan::new(bound.clone(), &bound.slots(&[0]), 2048));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let scan = scan.clone();
                std::thread::spawn(move || ids(&drain(&scan, &mut Local::default()), 0))
            })
            .collect();
        let mut all: Vec<Option<i64>> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        all.sort();
        assert_eq!(all, (0..100).map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn files_must_share_the_schema_unless_union_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(3, 0), 10);
        let other = RecordBatch::try_from_iter([
            ("name", Arc::new(StringArray::from(vec!["x"])) as ArrayRef),
            ("extra", Arc::new(Int32Array::from(vec![7])) as ArrayRef),
        ])
        .unwrap();
        let b = write(dir.path(), "b.parquet", &other, 10);
        let bound = Arc::new(bind(vec![a.clone(), b.clone()], ReadOptions::default()).unwrap());
        let scan = Scan::new(bound.clone(), &bound.slots(&[0]), 2048);
        let mut local = Local::default();
        assert!(scan.next(&mut local).unwrap().is_some());
        let e = scan.next(&mut local).unwrap_err();
        assert_eq!(
            e,
            format!(
                "{} has the columns (name VARCHAR, extra INTEGER) but {} has (id BIGINT, name VARCHAR); \
                 the files must have the same schema, or pass union_by_name := true",
                b, a
            )
        );
        // the same names with another type also differ
        let retyped = RecordBatch::try_from_iter([
            ("id", Arc::new(Int32Array::from(vec![1])) as ArrayRef),
            ("name", Arc::new(StringArray::from(vec!["x"])) as ArrayRef),
        ])
        .unwrap();
        let c = write(dir.path(), "c.parquet", &retyped, 10);
        let bound = Arc::new(bind(vec![a.clone(), c.clone()], ReadOptions::default()).unwrap());
        let scan = Scan::new(bound.clone(), &bound.slots(&[1]), 2048);
        let mut local = Local::default();
        scan.next(&mut local).unwrap();
        assert!(scan.next(&mut local).is_err());

        let opts = ReadOptions { union_by_name: true, ..Default::default() };
        let bound = Arc::new(bind(vec![a.clone(), b.clone()], opts).unwrap());
        assert_eq!(bound_names(&bound), strings(&["id", "name", "extra"]));
        assert_eq!((bound.estimated_rows, bound.exact_rows), (4, true));
        let scan = Scan::new(bound.clone(), &bound.slots(&[2, 0]), 2048);
        let batches = all_rows(&scan);
        assert_eq!(ids(&batches, 1), vec![Some(0), Some(1), Some(2), None]);
        let extra: Vec<Option<i32>> = batches
            .iter()
            .flat_map(|b| b.columns[0].as_any().downcast_ref::<Int32Array>().unwrap().iter().collect::<Vec<_>>())
            .collect();
        assert_eq!(extra, vec![None, None, None, Some(7)]);

        let e = bind(vec![a.clone(), c.clone()], opts).unwrap_err();
        assert_eq!(e, format!("column \"id\" is BIGINT in {} but INTEGER in {}", a, c));
    }

    fn one(name: &str, values: Vec<i64>) -> RecordBatch {
        RecordBatch::try_from_iter([(name, Arc::new(Int64Array::from(values)) as ArrayRef)]).unwrap()
    }

    #[test]
    fn names_are_deduplicated_and_matched_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        let dup = RecordBatch::try_from_iter([
            ("x", Arc::new(Int64Array::from(vec![1])) as ArrayRef),
            ("X", Arc::new(Int64Array::from(vec![2])) as ArrayRef),
        ])
        .unwrap();
        let d = write(dir.path(), "dup.parquet", &dup, 10);
        assert_eq!(bound_names(&bind(vec![d.clone()], ReadOptions::default()).unwrap()), strings(&["x", "X_1"]));

        let a = write(dir.path(), "a.parquet", &one("Price", vec![1]), 10);
        let b = write(dir.path(), "b.parquet", &one("price", vec![2]), 10);
        for union_by_name in [false, true] {
            let opts = ReadOptions { union_by_name, ..Default::default() };
            let bound = Arc::new(bind(vec![a.clone(), b.clone()], opts).unwrap());
            assert_eq!(bound_names(&bound), strings(&["Price"]));
            let scan = Scan::new(bound.clone(), &bound.slots(&[0]), 2048);
            assert_eq!(ids(&all_rows(&scan), 0), vec![Some(1), Some(2)], "{union_by_name}");
        }
        // another name is still another column
        let c = write(dir.path(), "c.parquet", &one("cost", vec![3]), 10);
        let opts = ReadOptions { union_by_name: true, ..Default::default() };
        let bound = bind(vec![a.clone(), c.clone()], opts).unwrap();
        assert_eq!(bound_names(&bound), strings(&["Price", "cost"]));
        let bound = Arc::new(bind(vec![a.clone(), c.clone()], ReadOptions::default()).unwrap());
        let scan = Scan::new(bound.clone(), &bound.slots(&[0]), 2048);
        let mut local = Local::default();
        scan.next(&mut local).unwrap();
        assert!(scan.next(&mut local).unwrap_err().contains("the files must have the same schema"));

        let f = write(dir.path(), "f.parquet", &one("FileName", vec![1]), 10);
        let e = bind(vec![f], ReadOptions { filename: true, ..Default::default() }).unwrap_err();
        assert_eq!(e, "the files already have a column \"filename\"");
    }

    #[test]
    fn union_by_name_checks_files_opened_after_bind() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(3, 0), 10);
        let opts = ReadOptions { union_by_name: true, ..Default::default() };
        let mut bound = bind(vec![a.clone(), a.clone()], opts).unwrap();
        // as if the second file changed between bind and scan
        bound.metadata[1] = None;
        bound.columns[0].sql = SqlType::Integer;
        let bound = Arc::new(bound);
        let scan = Scan::new(bound.clone(), &bound.slots(&[0]), 2048);
        let mut local = Local::default();
        let e = scan.next(&mut local).unwrap_err();
        assert_eq!(e, format!("column \"id\" is BIGINT in {}, but INTEGER was bound", a));
    }

    #[test]
    fn skips_files_without_row_groups_and_reports_damaged_files() {
        let dir = tempfile::tempdir().unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let empty = local(dir.path().join("empty.parquet"));
        let w = Writer::try_new(File::create(&empty).unwrap(), schema, WriterOptions::default()).unwrap();
        w.close().unwrap();
        let ids_only =
            RecordBatch::try_from_iter([("id", Arc::new(Int64Array::from(vec![4, 5])) as ArrayRef)]).unwrap();
        let a = write(dir.path(), "a.parquet", &ids_only, 10);
        let bound = Arc::new(bind(vec![empty.clone(), a.clone(), empty.clone()], ReadOptions::default()).unwrap());
        let scan = Scan::new(bound.clone(), &bound.slots(&[0]), 2048);
        assert_eq!(scan.max_threads(), 3);
        assert_eq!(ids(&all_rows(&scan), 0), vec![Some(4), Some(5)]);

        // a file replaced by garbage after bind
        let bound = Arc::new(bind(vec![a.clone(), a.clone()], ReadOptions::default()).unwrap());
        let bad = local(dir.path().join("a.parquet"));
        let scan = Scan::new(bound.clone(), &bound.slots(&[0]), 2048);
        let mut local = Local::default();
        scan.next(&mut local).unwrap();
        std::fs::write(&bad, b"garbage").unwrap();
        let e = scan.next(&mut local).unwrap_err();
        assert!(e.starts_with("cannot read "), "{e}");
    }

    #[test]
    fn scans_files_of_any_file_system() {
        let dir = tempfile::tempdir().unwrap();
        let fs = crate::fs::tests::MemFs::default();
        for (name, offset) in [("b.parquet", 10), ("a.parquet", 0)] {
            let path = write(dir.path(), name, &ints(10, offset), 4);
            fs.put(&format!("mem://d/{name}"), std::fs::read(path).unwrap());
        }
        // globs go to the file system; a remote file is not checked before it is opened
        let files = expand(&["mem://d/*.parquet".into(), "mem://d/missing.parquet".into()], &fs).unwrap();
        assert_eq!(files, strings(&["mem://d/a.parquet", "mem://d/b.parquet", "mem://d/missing.parquet"]));
        assert_eq!(expand(&["mem://d/*.csv".into()], &fs).unwrap_err(), "no files match \"mem://d/*.csv\"");
        let fs: Arc<dyn FileSystem> = Arc::new(fs);
        let bound = Arc::new(Bound::bind(fs.clone(), files[..2].to_vec(), ReadOptions::default()).unwrap());
        let scan = Scan::new(bound, &[Slot::File { column: 0, projected: 0 }], 3);
        let batches = all_rows(&scan);
        assert_eq!(ids(&batches, 0), (0..20).map(Some).collect::<Vec<_>>());
        assert_eq!(batches[0].file.path, "mem://d/a.parquet");
        let e = Bound::bind(fs, vec!["mem://d/missing.parquet".into()], ReadOptions::default()).unwrap_err();
        assert_eq!(e, "no such file: mem://d/missing.parquet");
    }

    fn id_cmp(op: crate::prune::Op, v: i128) -> crate::prune::Predicate {
        crate::prune::Predicate::Compare { column: 0, op, value: crate::prune::Scalar::Int(v) }
    }

    #[test]
    fn skips_row_groups_the_predicates_rule_out() {
        use crate::prune::{Op, Predicate};
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(100, 0), 10);
        let b = write(dir.path(), "b.parquet", &ints(100, 100), 10);
        let bound = Arc::new(bind(vec![a.clone(), b], ReadOptions::default()).unwrap());
        let slots = [Slot::File { column: 0, projected: 0 }];
        let firsts = |preds: Vec<Predicate>| {
            let scan = Scan::new(bound.clone(), &slots, 100).with_predicates(preds);
            all_rows(&scan).iter().map(|b| (b.file.path.ends_with("a.parquet"), b.first_row)).collect::<Vec<_>>()
        };
        assert_eq!(firsts(vec![id_cmp(Op::Ge, 185)]), vec![(false, 80), (false, 90)]);
        assert_eq!(firsts(vec![id_cmp(Op::Lt, 10)]), vec![(true, 0)]);
        assert_eq!(firsts(vec![id_cmp(Op::Ge, 45), id_cmp(Op::Le, 54)]), vec![(true, 40), (true, 50)]);
        assert_eq!(
            firsts(vec![Predicate::Or(vec![id_cmp(Op::Eq, 5), id_cmp(Op::Eq, 195)])]),
            vec![(true, 0), (false, 90)]
        );
        assert!(firsts(vec![id_cmp(Op::Gt, 1000)]).is_empty());
        assert!(firsts(vec![Predicate::IsNull(0)]).is_empty());
        assert_eq!(firsts(vec![]).len(), 20);
        // predicates on the filename or on columns without statistics skip nothing
        let strings = crate::prune::Predicate::Compare { column: 1, op: Op::Eq, value: crate::prune::Scalar::Int(1) };
        assert_eq!(firsts(vec![strings]).len(), 20);
        let bound_f = Arc::new(bind(vec![a], ReadOptions { filename: true, ..Default::default() }).unwrap());
        let scan = Scan::new(bound_f, &slots, 100).with_predicates(vec![Predicate::IsNull(2)]);
        assert_eq!(all_rows(&scan).len(), 10);
    }

    #[test]
    fn a_column_a_file_lacks_is_null_for_its_predicates() {
        use crate::prune::{Op, Predicate};
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(10, 0), 5);
        let c = write(dir.path(), "c.parquet", &one("other", vec![1, 2, 3]), 5);
        let opts = ReadOptions { union_by_name: true, ..Default::default() };
        let bound = Arc::new(bind(vec![a, c], opts).unwrap());
        let other = bound.columns.iter().position(|c| c.name == "other").unwrap();
        let slots = [Slot::File { column: other, projected: 0 }];
        let files = |p: Predicate| {
            let scan = Scan::new(bound.clone(), &slots, 100).with_predicates(vec![p]);
            all_rows(&scan).iter().map(|b| b.file.path.rsplit('/').next().unwrap().to_string()).collect::<Vec<_>>()
        };
        let eq = |v| Predicate::Compare { column: other, op: Op::Eq, value: crate::prune::Scalar::Int(v) };
        assert_eq!(files(eq(2)), strings(&["c.parquet"]));
        assert_eq!(files(Predicate::IsNull(other)), strings(&["a.parquet", "a.parquet"]));
        assert_eq!(files(Predicate::IsNotNull(other)), strings(&["c.parquet"]));
        // c.parquet has no id: every id there is NULL
        assert_eq!(files(id_cmp(Op::Lt, 3)), strings(&["a.parquet"]));
    }

    #[test]
    fn batches_carry_the_index_of_their_row_group_in_scan_order() {
        use crate::prune::Op;
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.parquet", &ints(25, 0), 10);
        let b = write(dir.path(), "b.parquet", &ints(25, 25), 10);
        let bound = Arc::new(bind(vec![a, b], ReadOptions::default()).unwrap());
        let slots = [Slot::File { column: 0, projected: 0 }];
        let scan = Scan::new(bound.clone(), &slots, 4);
        let batches = all_rows(&scan);
        let index: Vec<(u64, u64)> = batches.iter().map(|b| (b.batch, b.first_row)).collect();
        assert_eq!(
            index,
            vec![
                (0, 0),
                (0, 4),
                (0, 8),
                (1, 10),
                (1, 14),
                (1, 18),
                (2, 20),
                (2, 24),
                (3, 0),
                (3, 4),
                (3, 8),
                (4, 10),
                (4, 14),
                (4, 18),
                (5, 20),
                (5, 24)
            ]
        );
        // skipped row groups keep their index, so indices stay those of the file order
        let scan = Scan::new(bound, &slots, 100).with_predicates(vec![id_cmp(Op::Ge, 35)]);
        assert_eq!(all_rows(&scan).iter().map(|b| b.batch).collect::<Vec<_>>(), vec![4, 5]);
    }

    #[test]
    fn copy_from_needs_the_table_columns_by_position_and_type() {
        let two = [SqlType::BigInt, SqlType::Varchar];
        assert_eq!(check_copy_from(&two, &two), Ok(()));
        assert_eq!(
            check_copy_from(&two, &[SqlType::Varchar, SqlType::BigInt]).unwrap_err(),
            "the file columns (BIGINT, VARCHAR) do not match the table columns (VARCHAR, BIGINT) by position and \
             type; use INSERT INTO ... SELECT ... FROM read_tparquet(...) to map or cast them"
        );
        assert!(check_copy_from(&two, &two[..1]).is_err());
    }
}
