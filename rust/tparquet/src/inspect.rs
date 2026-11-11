//! What a file holds per column: codecs, sizes, pages and PCODEC payload modes.

use crate::error::{Result, invalid};
use parquet::basic::Compression;
use parquet::file::metadata::{ColumnChunkMetaData, ParquetMetaDataReader};
use parquet::file::reader::ChunkReader;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use tparquet_core::{Mode, mode_of};

/// The report of [`inspect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    /// Rows in the file.
    pub rows: i64,
    /// Number of row groups.
    pub row_groups: usize,
    /// Value of the `parquet.pcodec.version` footer key, if present.
    pub pcodec_version: Option<String>,
    /// One entry per leaf column, in schema order.
    pub columns: Vec<ColumnReport>,
}

/// One leaf column, over all row groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnReport {
    /// Dotted leaf path.
    pub path: String,
    /// Parquet physical type.
    pub physical_type: String,
    /// Codecs of its column chunks, each once, in the order met.
    pub codecs: Vec<String>,
    /// Compressed bytes (pages with their headers), as the metadata says.
    pub compressed_bytes: i64,
    /// Uncompressed bytes, as the metadata says.
    pub uncompressed_bytes: i64,
    /// Pages per page type.
    pub pages: BTreeMap<String, u64>,
    /// PCODEC pages per payload mode (`RAW`, `PCO`, `DECIMAL`, `ZSTD`), plus `UNCOMPRESSED`
    /// for V2 pages stored without compression and `EMPTY` for pages with nothing to decode.
    pub modes: BTreeMap<String, u64>,
}

/// Name of a codec without its level.
fn codec_name(c: Compression) -> String {
    let s = c.to_string();
    match s.split_once('(') {
        Some((name, _)) => name.to_string(),
        None => s,
    }
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Raw => "RAW",
        Mode::Pco => "PCO",
        Mode::Decimal => "DECIMAL",
        Mode::Zstd => "ZSTD",
        _ => "UNKNOWN",
    }
}

fn page_type_name(t: i32) -> String {
    match t {
        0 => "DATA_PAGE".into(),
        1 => "INDEX_PAGE".into(),
        2 => "DICTIONARY_PAGE".into(),
        3 => "DATA_PAGE_V2".into(),
        t => format!("PAGE_TYPE_{t}"),
    }
}

/// Reads the footer and every page header of `reader`.
pub fn inspect<R: ChunkReader>(reader: &R) -> Result<FileReport> {
    let metadata = ParquetMetaDataReader::new().parse_and_finish(reader)?;
    let file = metadata.file_metadata();
    let schema = file.schema_descr();
    let mut columns: Vec<ColumnReport> = schema
        .columns()
        .iter()
        .map(|c| ColumnReport {
            path: c.path().string(),
            physical_type: c.physical_type().to_string(),
            codecs: vec![],
            compressed_bytes: 0,
            uncompressed_bytes: 0,
            pages: BTreeMap::new(),
            modes: BTreeMap::new(),
        })
        .collect();
    // the column chunks of a valid file are disjoint: reading more bytes than the file
    // holds means the footer points several chunks at the same bytes (bounded work)
    let mut budget = reader.len();
    for rg in metadata.row_groups() {
        for (report, chunk) in columns.iter_mut().zip(rg.columns()) {
            let codec = codec_name(chunk.compression());
            if !report.codecs.contains(&codec) {
                report.codecs.push(codec);
            }
            report.compressed_bytes += chunk.compressed_size();
            report.uncompressed_bytes += chunk.uncompressed_size();
            scan_pages(reader, chunk, report, &mut budget)?;
        }
    }
    let pcodec_version = file
        .key_value_metadata()
        .and_then(|kv| kv.iter().find(|kv| kv.key == tparquet_core::KV_KEY))
        .map(|kv| kv.value.clone().unwrap_or_default());
    Ok(FileReport { rows: file.num_rows(), row_groups: metadata.num_row_groups(), pcodec_version, columns })
}

/// Counts the pages of `chunk` into `report`; `budget` is the number of bytes left to read.
fn scan_pages<R: ChunkReader>(
    reader: &R,
    chunk: &ColumnChunkMetaData,
    report: &mut ColumnReport,
    budget: &mut u64,
) -> Result<()> {
    let bad = || invalid(format!("column {}: bad column chunk offsets", report.path));
    let start = chunk.dictionary_page_offset().unwrap_or(chunk.data_page_offset());
    let start = u64::try_from(start).map_err(|_| bad())?;
    let len = usize::try_from(chunk.compressed_size()).map_err(|_| bad())?;
    if start.checked_add(len as u64).is_none_or(|end| end > reader.len()) {
        return Err(bad());
    }
    *budget = budget
        .checked_sub(len as u64)
        .ok_or_else(|| invalid(format!("column {}: the column chunks add up to more than the file", report.path)))?;
    let bytes = reader.get_bytes(start, len)?;
    let pcodec = matches!(chunk.compression(), Compression::PCODEC(_));
    let mut pos = 0;
    while pos < bytes.len() {
        let mut p = Compact::new(&bytes[pos..]);
        let h = p.page_header().map_err(|e| invalid(format!("column {}: page header: {e}", report.path)))?;
        let body_start = pos + p.consumed();
        let body_end = usize::try_from(h.compressed_size)
            .ok()
            .and_then(|n| body_start.checked_add(n))
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid(format!("column {}: page runs past the column chunk", report.path)))?;
        *report.pages.entry(page_type_name(h.page_type)).or_default() += 1;
        if pcodec {
            let mode = page_mode(&h, &bytes[body_start..body_end]);
            *report.modes.entry(mode.to_string()).or_default() += 1;
        }
        pos = body_end;
    }
    Ok(())
}

/// The payload mode of one PCODEC page.
fn page_mode(h: &PageHeader, body: &[u8]) -> &'static str {
    let (levels, compressed) = h.v2.unwrap_or((0, true));
    if !compressed {
        return "UNCOMPRESSED";
    }
    if i64::from(h.uncompressed_size) <= i64::from(levels) {
        return "EMPTY";
    }
    let payload = usize::try_from(levels).ok().and_then(|l| body.get(l..)).unwrap_or_default();
    mode_of(payload).map_or("INVALID", mode_name)
}

/// The fields of a Thrift `PageHeader` this module needs.
#[derive(Debug, PartialEq, Eq)]
struct PageHeader {
    page_type: i32,
    uncompressed_size: i32,
    compressed_size: i32,
    /// V2 pages: bytes of levels before the values, and `is_compressed`.
    v2: Option<(i32, bool)>,
}

/// A reader of the Thrift compact protocol, just enough for page headers.
struct Compact<'a> {
    /// The bytes not read yet.
    buf: &'a [u8],
    /// Length of the whole input.
    len: usize,
    depth: usize,
}

const MAX_DEPTH: usize = 32;
type Parse<T> = std::result::Result<T, String>;

impl<'a> Compact<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, len: buf.len(), depth: 0 }
    }

    /// Bytes read so far.
    fn consumed(&self) -> usize {
        self.len - self.buf.len()
    }

    fn byte(&mut self) -> Parse<u8> {
        let (&b, rest) = self.buf.split_first().ok_or("truncated")?;
        self.buf = rest;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Parse<()> {
        self.buf = self.buf.get(n..).ok_or("truncated")?;
        Ok(())
    }

    fn varint(&mut self) -> Parse<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("varint too long".into())
    }

    fn zigzag(&mut self) -> Parse<i64> {
        let v = self.varint()?;
        Ok((v >> 1) as i64 ^ -((v & 1) as i64))
    }

    fn i32(&mut self) -> Parse<i32> {
        i32::try_from(self.zigzag()?).map_err(|_| "i32 out of range".into())
    }

    /// Calls `field(self, id, type)` for each field of a struct; it returns whether it read
    /// the value, else the value is skipped.
    fn read_struct(&mut self, mut field: impl FnMut(&mut Self, i16, u8) -> Parse<bool>) -> Parse<()> {
        self.nested(|p| {
            let mut last: i16 = 0;
            loop {
                let b = p.byte()?;
                if b == 0 {
                    return Ok(());
                }
                let t = b & 0x0f;
                let delta = i16::from(b >> 4);
                let id = if delta == 0 {
                    i16::try_from(p.zigzag()?).map_err(|_| "field id out of range")?
                } else {
                    last.checked_add(delta).ok_or("field id out of range")?
                };
                last = id;
                if !field(p, id, t)? {
                    p.skip(t)?;
                }
            }
        })
    }

    /// Reads a struct, list, set or map with `f`, one level deeper: containers nest at most
    /// `MAX_DEPTH` deep, so the recursion is bounded.
    fn nested(&mut self, f: impl FnOnce(&mut Self) -> Parse<()>) -> Parse<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("nested too deep".into());
        }
        f(self)?;
        self.depth -= 1;
        Ok(())
    }

    fn skip(&mut self, t: u8) -> Parse<()> {
        match t {
            1 | 2 => Ok(()),
            3 => self.take(1),
            4..=6 => self.varint().map(drop),
            7 => self.take(8),
            8 => {
                let n = usize::try_from(self.varint()?).map_err(|_| "length out of range")?;
                self.take(n)
            }
            9 | 10 => self.nested(|p| {
                let h = p.byte()?;
                let n = match h >> 4 {
                    15 => p.varint()?,
                    n => u64::from(n),
                };
                p.elements(h & 0x0f, n)
            }),
            11 => self.nested(|p| {
                let n = p.varint()?;
                if n == 0 {
                    return Ok(());
                }
                let kv = p.byte()?;
                for _ in 0..n {
                    p.element(kv >> 4)?;
                    p.element(kv & 0x0f)?;
                }
                Ok(())
            }),
            12 => self.read_struct(|_, _, _| Ok(false)),
            t => Err(format!("unknown field type {t}")),
        }
    }

    fn elements(&mut self, t: u8, n: u64) -> Parse<()> {
        for _ in 0..n {
            self.element(t)?;
        }
        Ok(())
    }

    /// A list, set or map element: booleans take one byte there.
    fn element(&mut self, t: u8) -> Parse<()> {
        match t {
            1 | 2 => self.take(1),
            t => self.skip(t),
        }
    }

    fn page_header(&mut self) -> Parse<PageHeader> {
        let mut h = PageHeader { page_type: -1, uncompressed_size: -1, compressed_size: -1, v2: None };
        self.read_struct(|p, id, t| match (id, t) {
            (1, 5) => p.i32().map(|v| h.page_type = v).map(|()| true),
            (2, 5) => p.i32().map(|v| h.uncompressed_size = v).map(|()| true),
            (3, 5) => p.i32().map(|v| h.compressed_size = v).map(|()| true),
            (8, 12) => {
                let (mut def, mut rep, mut compressed) = (0, 0, true);
                p.read_struct(|p, id, t| match (id, t) {
                    (5, 5) => p.i32().map(|v| def = v).map(|()| true),
                    (6, 5) => p.i32().map(|v| rep = v).map(|()| true),
                    (7, 1 | 2) => {
                        compressed = t == 1;
                        Ok(true)
                    }
                    _ => Ok(false),
                })?;
                let levels = def.checked_add(rep).filter(|l| *l >= 0).ok_or("bad level lengths")?;
                h.v2 = Some((levels, compressed));
                Ok(true)
            }
            _ => Ok(false),
        })?;
        if h.page_type < 0 || h.uncompressed_size < 0 || h.compressed_size < 0 {
            return Err("missing or negative required field".into());
        }
        Ok(h)
    }
}

impl FileReport {
    /// The report as JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "rows": self.rows,
            "row_groups": self.row_groups,
            "pcodec_version": self.pcodec_version,
            "columns": self.columns.iter().map(|c| serde_json::json!({
                "path": c.path,
                "physical_type": c.physical_type,
                "codecs": c.codecs,
                "compressed_bytes": c.compressed_bytes,
                "uncompressed_bytes": c.uncompressed_bytes,
                "pages": c.pages,
                "modes": c.modes,
            })).collect::<Vec<_>>(),
        })
    }

    /// The report as text, one line per column.
    pub fn to_text(&self) -> String {
        let mut s = format!("rows: {}, row groups: {}", self.rows, self.row_groups);
        if let Some(v) = &self.pcodec_version {
            write!(s, ", {} = {v}", tparquet_core::KV_KEY).expect("write to String");
        }
        s.push('\n');
        let counts =
            |m: &BTreeMap<String, u64>| m.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ");
        for c in &self.columns {
            writeln!(
                s,
                "{} {} {}: {} of {} bytes; pages: {}{}",
                c.path,
                c.physical_type,
                c.codecs.join(","),
                c.compressed_bytes,
                c.uncompressed_bytes,
                counts(&c.pages),
                if c.modes.is_empty() { String::new() } else { format!("; modes: {}", counts(&c.modes)) },
            )
            .expect("write to String");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// compact-protocol bytes of a page header
    fn header(fields: &[u8]) -> Vec<u8> {
        let mut v = fields.to_vec();
        v.push(0);
        v
    }

    #[test]
    fn parses_v1_header() {
        // type=0 (zigzag 0), uncompressed=10 (20), compressed=7 (14)
        let b = header(&[0x15, 0x00, 0x15, 20, 0x15, 14]);
        let mut p = Compact::new(&b);
        assert_eq!(
            p.page_header().unwrap(),
            PageHeader { page_type: 0, uncompressed_size: 10, compressed_size: 7, v2: None }
        );
        assert_eq!(p.consumed(), b.len());
    }

    #[test]
    fn parses_v2_header_and_skips_the_rest() {
        let mut b = vec![0x15, 6, 0x15, 200, 1, 0x15, 100];
        // field 4 (crc, i32), skipped
        b.extend([0x15, 9]);
        // field 5: a struct with a binary, a list of bools, a map, a double, a nested struct
        b.extend([0x1c, 0x18, 2, b'a', b'b', 0x19, 0x21, 1, 2, 0x1b, 1, 0x55, 2, 4, 0x17]);
        b.extend([0u8; 8]);
        b.extend([0x1c, 0x13, 7, 0x16, 3, 0x14, 5, 0x1a, 0x00, 0]);
        b.push(0);
        // field 8: data page v2 header: def=3 (6), rep=2 (4), is_compressed=false
        b.extend([0x3c, 0x15, 2, 0x15, 4, 0x15, 6, 0x15, 4, 0x15, 6, 0x15, 4, 0x12, 0]);
        b.push(0);
        let h = Compact::new(&b).page_header().unwrap();
        assert_eq!(h, PageHeader { page_type: 3, uncompressed_size: 100, compressed_size: 50, v2: Some((5, false)) });
    }

    #[test]
    fn long_field_ids_and_lists() {
        // field id given in full (delta 0, i16 zigzag 2 = 1), then a list of 16 i32 (size in a varint)
        let mut b = vec![0x05, 2, 0, 0x15, 2, 0x15, 2, 0x19, 0xf5, 16];
        b.extend([0u8; 16]);
        b.push(0);
        let h = Compact::new(&b).page_header().unwrap();
        assert_eq!(h, PageHeader { page_type: 0, uncompressed_size: 1, compressed_size: 1, v2: None });
    }

    #[test]
    fn rejects_bad_headers() {
        for (bytes, err) in [
            (&[][..], "truncated"),
            (&[0x15][..], "truncated"),
            (&[0x15, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80][..], "varint too long"),
            (&[0x15, 0xfe, 0xff, 0xff, 0xff, 0x7f][..], "i32 out of range"),
            (&[0x1d][..], "unknown field type 13"),
            (&[0x15, 1, 0][..], "missing or negative required field"),
            // each required field missing on its own
            (&[0x25, 0, 0x15, 0, 0][..], "missing or negative required field"),
            (&[0x15, 0, 0x25, 0, 0][..], "missing or negative required field"),
            (&[0x15, 0, 0x15, 0, 0][..], "missing or negative required field"),
            (&[0x18, 5, 0][..], "truncated"),
            (&[0x17, 0, 0][..], "truncated"),
            (&[0x13][..], "truncated"),
            (&[0x19, 0x35][..], "truncated"),
            (&[0x1b, 1, 0x55][..], "truncated"),
            (&[0x05, 0xfe, 0xff, 0x7f][..], "field id out of range"),
            (&[0x8c, 0x55, 1, 0x15, 0, 0, 0][..], "bad level lengths"),
            // def = -2 (zigzag 3), rep = 1 (zigzag 2)
            (&[0x8c, 0x55, 3, 0x15, 2, 0, 0][..], "bad level lengths"),
            (&[0x8c, 0x55, 0xfe, 0xff, 0xff, 0xff, 0x0f, 0x15, 2, 0, 0][..], "bad level lengths"),
        ] {
            assert_eq!(Compact::new(bytes).page_header().unwrap_err(), err, "{bytes:?}");
        }
        let mut deep = vec![0x1c; 40];
        deep.extend([0; 41]);
        assert_eq!(Compact::new(&deep).page_header().unwrap_err(), "nested too deep");
        // depth is per nesting, not per struct: 40 sibling structs are fine
        let mut siblings = vec![0x15, 0, 0x15, 0, 0x15, 0];
        for _ in 0..40 {
            siblings.extend([0x1c, 0]);
        }
        siblings.push(0);
        assert!(Compact::new(&siblings).page_header().is_ok());
        let mut ok = vec![0x15, 0, 0x15, 0, 0x15, 0];
        ok.extend([0x1c; 31]);
        ok.extend([0; 32]);
        assert!(Compact::new(&ok).page_header().is_ok());
    }

    #[test]
    fn nested_containers_are_bounded() {
        // a list of lists of lists ... (a few bytes per level): the depth limit applies to
        // lists, sets and maps as to structs, and the parser never overflows the stack
        // field 4 holds `levels + 1` nested containers: lists or sets of one element, the
        // innermost empty; or maps of one entry whose key is the next map and whose value an
        // i32 (0), the innermost empty
        let header = |levels: usize, container: u8| {
            let mut b = vec![0x15, 0, 0x15, 0, 0x15, 0, 0x40 | container];
            if container == 11 {
                b.extend([1, 0xb5].repeat(levels));
                b.push(0);
                b.extend(vec![0; levels]);
            } else {
                b.extend(vec![0x10 | container; levels]);
                b.push(0x05);
            }
            b.push(0);
            b
        };
        for t in [9, 10, 11] {
            let deep = header(1_000_000, t);
            assert_eq!(Compact::new(&deep).page_header().unwrap_err(), "nested too deep", "{t}");
            let ok = header(MAX_DEPTH - 2, t);
            assert!(Compact::new(&ok).page_header().is_ok(), "{t}");
        }
        // a list of structs counts both
        let mut b = vec![0x15, 0, 0x15, 0, 0x15, 0, 0x19];
        for _ in 0..MAX_DEPTH {
            b.extend([0x1c, 0x19]);
        }
        assert_eq!(Compact::new(&b).page_header().unwrap_err(), "nested too deep");
    }

    #[test]
    fn empty_map_and_field_id_overflow() {
        // an empty map, then field ids that overflow i16 by delta
        let b = [0x15, 0, 0x15, 0, 0x15, 0, 0x1b, 0, 0];
        assert!(Compact::new(&b).page_header().is_ok());
        let mut b = vec![0x05, 0xfc, 0xff, 0x03, 0];
        b.extend([0xf5, 0]);
        assert_eq!(Compact::new(&b).page_header().unwrap_err(), "field id out of range");
    }

    #[test]
    fn page_modes() {
        let h = |uncompressed, v2| PageHeader { page_type: 0, uncompressed_size: uncompressed, compressed_size: 0, v2 };
        assert_eq!(page_mode(&h(4, None), &[1, 0, 1, 2, 3, 4]), "RAW");
        assert_eq!(page_mode(&h(4, None), &[1, 1, 2]), "PCO");
        assert_eq!(page_mode(&h(4, None), &[1, 2]), "DECIMAL");
        assert_eq!(page_mode(&h(4, None), &[1, 3]), "ZSTD");
        assert_eq!(page_mode(&h(4, None), &[2, 0]), "INVALID");
        assert_eq!(page_mode(&h(0, None), &[]), "EMPTY");
        assert_eq!(page_mode(&h(6, Some((2, true))), &[9, 9, 1, 1]), "PCO");
        assert_eq!(page_mode(&h(2, Some((2, true))), &[9, 9]), "EMPTY");
        assert_eq!(page_mode(&h(3, Some((2, true))), &[9, 9]), "INVALID");
        assert_eq!(page_mode(&h(3, Some((5, true))), &[9, 9]), "EMPTY");
        assert_eq!(page_mode(&h(9, Some((5, true))), &[9, 9]), "INVALID");
        assert_eq!(page_mode(&h(6, Some((2, false))), &[9, 9, 1, 1]), "UNCOMPRESSED");
    }

    fn report(path: &str) -> ColumnReport {
        ColumnReport {
            path: path.into(),
            physical_type: "INT64".into(),
            codecs: vec![],
            compressed_bytes: 0,
            uncompressed_bytes: 0,
            pages: BTreeMap::new(),
            modes: BTreeMap::new(),
        }
    }

    /// a PCODEC column chunk at `offset` of `len` bytes
    fn chunk(offset: i64, len: i64) -> ColumnChunkMetaData {
        use parquet::schema::types::{ColumnDescriptor, ColumnPath, Type as SchemaType};
        let t = SchemaType::primitive_type_builder("x", parquet::basic::Type::INT64).build().unwrap();
        let descr = std::sync::Arc::new(ColumnDescriptor::new(std::sync::Arc::new(t), 0, 0, ColumnPath::from("x")));
        ColumnChunkMetaData::builder(descr)
            .set_compression(Compression::PCODEC(Default::default()))
            .set_data_page_offset(offset)
            .set_total_compressed_size(len)
            .build()
            .unwrap()
    }

    #[test]
    fn column_chunks_must_lie_in_the_file() {
        // one data page: type 0, uncompressed 4, compressed 6 (a RAW payload of 4 bytes)
        let page = bytes::Bytes::from_static(&[0x15, 0, 0x15, 8, 0x15, 12, 0, 1, 0, 9, 9, 9, 9]);
        let n = page.len() as i64;
        let mut r = report("x");
        // a chunk that ends exactly at the end of the input is fine
        let mut budget = n as u64;
        scan_pages(&page, &chunk(0, n), &mut r, &mut budget).unwrap();
        assert_eq!(budget, 0);
        // a second chunk over the same bytes is over budget
        let err = scan_pages(&page, &chunk(0, 1), &mut r, &mut budget).unwrap_err();
        assert_eq!(err.to_string(), "column x: the column chunks add up to more than the file");
        assert_eq!(r.pages["DATA_PAGE"], 1);
        assert_eq!(r.modes["RAW"], 1);
        for (offset, len) in [(0, n + 1), (1, n), (-1, 2), (0, -1), (i64::MAX, 1)] {
            let err = scan_pages(&page, &chunk(offset, len), &mut report("x"), &mut { u64::MAX }).unwrap_err();
            assert_eq!(err.to_string(), "column x: bad column chunk offsets", "{offset} {len}");
        }
        // a page that runs past its chunk, a broken header
        let err = scan_pages(&page, &chunk(0, n - 1), &mut report("x"), &mut { u64::MAX }).unwrap_err();
        assert_eq!(err.to_string(), "column x: page runs past the column chunk");
        let err = scan_pages(&page, &chunk(1, n - 1), &mut report("x"), &mut { u64::MAX }).unwrap_err();
        assert!(err.to_string().starts_with("column x: page header: "), "{err}");
    }

    #[test]
    fn names() {
        assert_eq!(codec_name(Compression::ZSTD(Default::default())), "ZSTD");
        assert_eq!(codec_name(Compression::SNAPPY), "SNAPPY");
        assert_eq!(page_type_name(0), "DATA_PAGE");
        assert_eq!(page_type_name(1), "INDEX_PAGE");
        assert_eq!(page_type_name(2), "DICTIONARY_PAGE");
        assert_eq!(page_type_name(3), "DATA_PAGE_V2");
        assert_eq!(page_type_name(9), "PAGE_TYPE_9");
        assert_eq!(mode_name(Mode::Raw), "RAW");
        assert_eq!(mode_name(Mode::Pco), "PCO");
        assert_eq!(mode_name(Mode::Decimal), "DECIMAL");
        assert_eq!(mode_name(Mode::Zstd), "ZSTD");
    }
}
