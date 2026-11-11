mod common;

use arrow_array::{ArrayRef, Float64Array, Int32Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field};
use bytes::Bytes;
use common::write;
use std::sync::Arc;
use tparquet::inspect::inspect;
use tparquet::parquet::basic::{Compression, Encoding};
use tparquet::parquet::file::properties::{WriterProperties, WriterVersion};
use tparquet::verify::{Difference, verify};
use tparquet::{Policy, WriterOptions};

fn file(columns: Vec<(&str, ArrayRef)>) -> Bytes {
    let batch = RecordBatch::try_from_iter(columns).unwrap();
    write(&[batch], WriterOptions::default().with_policy(Policy::Pcodec)).0
}

fn f64s(v: &[f64]) -> ArrayRef {
    Arc::new(Float64Array::from(v.to_vec()))
}

#[test]
fn equal_files() {
    let a = file(vec![("x", f64s(&[1.0, f64::NAN, -0.0]))]);
    let b = file(vec![("x", f64s(&[1.0, f64::NAN, -0.0]))]);
    assert_eq!(verify(a, b).unwrap(), None);
}

#[test]
fn floats_are_compared_bit_for_bit() {
    let a = file(vec![("x", f64s(&[1.0, 0.0]))]);
    let b = file(vec![("x", f64s(&[1.0, -0.0]))]);
    let d = verify(a, b).unwrap().unwrap();
    assert_eq!(
        d,
        Difference::Row {
            row: 1,
            a: "{\"x\":\"0x0000000000000000\"}".into(),
            b: "{\"x\":\"0x8000000000000000\"}".into()
        }
    );
    assert_eq!(
        d.to_string(),
        "row 1 differs:\n  a: {\"x\":\"0x0000000000000000\"}\n  b: {\"x\":\"0x8000000000000000\"}"
    );
    let nan = file(vec![("x", f64s(&[f64::NAN]))]);
    let other_nan = file(vec![("x", f64s(&[f64::from_bits(0x7ff8_0000_0000_0001)]))]);
    assert!(verify(nan, other_nan).unwrap().is_some());
}

#[test]
fn interval_columns_are_compared() {
    use arrow_array::{IntervalDayTimeArray, IntervalYearMonthArray};
    use arrow_buffer::IntervalDayTime;
    let columns = |ms: i32| -> Vec<(&str, ArrayRef)> {
        vec![
            ("ym", Arc::new(IntervalYearMonthArray::from(vec![Some(14), None]))),
            (
                "dt",
                Arc::new(IntervalDayTimeArray::from(vec![IntervalDayTime::new(3, ms), IntervalDayTime::new(-1, 0)])),
            ),
        ]
    };
    assert_eq!(verify(file(columns(1500)), file(columns(1500))).unwrap(), None);
    assert_eq!(
        verify(file(columns(1500)), file(columns(1501))).unwrap(),
        Some(Difference::Row {
            row: 0,
            a: "{\"ym\":{\"months\":14,\"days\":0,\"nanoseconds\":0},\"dt\":{\"months\":0,\"days\":3,\"nanoseconds\":1500000000}}".into(),
            b: "{\"ym\":{\"months\":14,\"days\":0,\"nanoseconds\":0},\"dt\":{\"months\":0,\"days\":3,\"nanoseconds\":1501000000}}".into()
        })
    );
}

#[test]
fn schemas_and_row_counts() {
    let a = file(vec![("x", Arc::new(Int32Array::from(vec![1])) as ArrayRef)]);
    let b = file(vec![("x", Arc::new(Int64Array::from(vec![1])) as ArrayRef)]);
    let d = verify(a.clone(), b).unwrap().unwrap();
    assert!(matches!(d, Difference::Schema { .. }));
    assert!(d.to_string().starts_with("schemas differ:\n  a: Field { \"x\": Int32 }"), "{d}");

    let long = file(vec![("x", Arc::new(Int32Array::from(vec![1, 2, 3])) as ArrayRef)]);
    assert_eq!(verify(long.clone(), a.clone()).unwrap(), Some(Difference::RowCount { a: 3, b: 1 }));
    assert_eq!(verify(a, long).unwrap(), Some(Difference::RowCount { a: 1, b: 3 }));
    assert_eq!(Difference::RowCount { a: 1, b: 3 }.to_string(), "row counts differ: 1 vs 3");
}

/// A list<double> column whose item field is called `item`; and a map<string, int64> whose
/// entries are called `entries`.
fn nested(item: &str, entries: &str, value: f64) -> Bytes {
    use arrow_array::builder::{Float64Builder, Int64Builder, ListBuilder, MapBuilder, MapFieldNames, StringBuilder};
    let mut list = ListBuilder::new(Float64Builder::new()).with_field(Field::new(item, DataType::Float64, true));
    list.values().append_value(value);
    list.append(true);
    let names = MapFieldNames { entry: entries.into(), key: "key".into(), value: "value".into() };
    let mut map = MapBuilder::new(Some(names), StringBuilder::new(), Int64Builder::new());
    map.keys().append_value("k");
    map.values().append_value(1);
    map.append(true).unwrap();
    file(vec![("l", Arc::new(list.finish()) as ArrayRef), ("m", Arc::new(map.finish()) as ArrayRef)])
}

#[test]
fn list_item_and_map_entry_names_are_not_data() {
    // pyarrow writes Parquet's `element` and `key_value`, arrow-rs `item` and `entries`
    assert_eq!(verify(nested("item", "entries", 1.0), nested("element", "key_value", 1.0)).unwrap(), None);
    let d = verify(nested("item", "entries", 1.0), nested("element", "key_value", 2.0)).unwrap();
    assert!(matches!(d, Some(Difference::Row { row: 0, .. })), "{d:?}");
}

#[test]
fn verify_errors_on_bad_files() {
    let a = file(vec![("x", f64s(&[1.0]))]);
    assert!(verify(a.clone(), Bytes::from_static(b"PAR1")).is_err());
    assert!(verify(Bytes::from_static(b"PAR1"), a).is_err());
}

fn v1_dictionary_pcodec() -> Bytes {
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(Compression::PCODEC(Default::default()))
        .set_data_page_row_count_limit(1000)
        .set_write_batch_size(1000)
        .build();
    let values: Vec<i64> = (0..5000).map(|i| (i % 2000) * 3).collect();
    let batch = RecordBatch::try_from_iter([("x", Arc::new(Int64Array::from(values)) as ArrayRef)]).unwrap();
    let mut out = Vec::new();
    let mut w = tparquet::parquet::arrow::ArrowWriter::try_new(&mut out, batch.schema(), Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    Bytes::from(out)
}

#[test]
fn inspect_counts_pages_and_modes() {
    let f = v1_dictionary_pcodec();
    let r = inspect(&f).unwrap();
    assert_eq!((r.rows, r.row_groups), (5000, 1));
    assert_eq!(r.pcodec_version.as_deref(), Some("1"));
    let c = &r.columns[0];
    assert_eq!(
        (c.path.as_str(), c.physical_type.as_str(), c.codecs.clone()),
        ("x", "INT64", vec!["PCODEC".to_string()])
    );
    assert_eq!(c.pages["DICTIONARY_PAGE"], 1);
    assert_eq!(c.pages["DATA_PAGE"], 5);
    // the dictionary is PLAIN i64s: PCO; the data pages (RLE indices) fall back to ZSTD or RAW
    assert_eq!(c.modes.get("PCO"), Some(&1));
    assert_eq!(c.modes.values().sum::<u64>(), 6);
    assert!(c.compressed_bytes > 0 && c.uncompressed_bytes > c.compressed_bytes);

    let json = r.to_json();
    assert_eq!(json["rows"], 5000);
    assert_eq!(json["row_groups"], 1);
    assert_eq!(json["pcodec_version"], "1");
    assert_eq!(json["columns"][0]["path"], "x");
    assert_eq!(json["columns"][0]["physical_type"], "INT64");
    assert_eq!(json["columns"][0]["codecs"][0], "PCODEC");
    assert_eq!(json["columns"][0]["pages"]["DICTIONARY_PAGE"], 1);
    assert_eq!(json["columns"][0]["modes"]["PCO"], 1);
    assert_eq!(json["columns"][0]["compressed_bytes"], c.compressed_bytes);
    assert_eq!(json["columns"][0]["uncompressed_bytes"], c.uncompressed_bytes);

    let text = r.to_text();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("rows: 5000, row groups: 1, parquet.pcodec.version = 1"));
    let line = lines.next().unwrap();
    assert!(
        line.starts_with(&format!(
            "x INT64 PCODEC: {} of {} bytes; pages: DATA_PAGE=5 DICTIONARY_PAGE=1; modes: ",
            c.compressed_bytes, c.uncompressed_bytes
        )),
        "{line}"
    );
    assert!(line.contains("PCO=1"), "{line}");
    assert_eq!(lines.next(), None);
}

#[test]
fn inspect_v2_and_multiple_row_groups() {
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::PCODEC(Default::default()))
        .set_dictionary_enabled(false)
        .set_encoding(Encoding::PLAIN)
        .set_max_row_group_row_count(Some(100))
        .build();
    let x = Float64Array::from_iter((0..250).map(|i| (i % 4 != 0).then_some(i as f64 / 8.0)));
    let batch = RecordBatch::try_from_iter([("x", Arc::new(x) as ArrayRef)]).unwrap();
    let mut out = Vec::new();
    let mut w = tparquet::parquet::arrow::ArrowWriter::try_new(&mut out, batch.schema(), Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let r = inspect(&Bytes::from(out)).unwrap();
    assert_eq!(r.row_groups, 3);
    assert_eq!(r.columns[0].pages["DATA_PAGE_V2"], 3);
    // the last, 50-row group is smaller as plain PCO
    assert_eq!(r.columns[0].modes.values().sum::<u64>(), 3);
    assert_eq!(r.columns[0].modes["DECIMAL"], 2, "{:?}", r.columns[0].modes);
    assert_eq!(r.columns[0].codecs, ["PCODEC"]);

    // a standard file: no modes, no marker
    let (base, _) = write(&[batch], WriterOptions::default().with_policy(Policy::Base));
    let r = inspect(&base).unwrap();
    assert!(r.columns[0].modes.is_empty());
    assert_eq!(r.pcodec_version, None);
    assert_eq!(r.to_text().lines().next(), Some("rows: 250, row groups: 1"));
    assert!(!r.to_text().contains("modes"));
}

#[test]
fn inspect_rejects_corrupt_files() {
    let f = v1_dictionary_pcodec();
    assert!(inspect(&Bytes::from_static(b"nope")).is_err());
    // corrupt every byte of the data region in turn: never a panic, an error or a report
    let data_end = f.len() - 8 - u32::from_le_bytes(f[f.len() - 8..f.len() - 4].try_into().unwrap()) as usize;
    let mut errors = 0;
    for i in (4..data_end).step_by(37) {
        let mut g = f.to_vec();
        g[i] ^= 0xff;
        errors += usize::from(inspect(&Bytes::from(g)).is_err());
    }
    assert!(errors > 0);
}

/// `file` with its footer replaced by one whose row groups are `rgs` copies of its first.
fn repeat_row_group(file: &Bytes, rgs: usize) -> Bytes {
    use tparquet::parquet::file::metadata::{ParquetMetaDataReader, ParquetMetaDataWriter};
    let metadata = ParquetMetaDataReader::new().parse_and_finish(file).unwrap();
    let footer = 8 + u32::from_le_bytes(file[file.len() - 8..file.len() - 4].try_into().unwrap()) as usize;
    let rg = metadata.row_group(0).clone();
    let metadata = metadata.into_builder().set_row_groups(vec![rg; rgs]).build();
    let mut out = file[..file.len() - footer].to_vec();
    ParquetMetaDataWriter::new(&mut out, &metadata).finish().unwrap();
    Bytes::from(out)
}

#[test]
fn inspect_work_is_bounded_by_the_file_size() {
    // a footer whose row groups all point at the same column chunk would make inspect read
    // the chunk once per row group: work quadratic in the file size
    let x = Int64Array::from_iter_values((0..100_000).map(|i| i * i));
    let batch = RecordBatch::try_from_iter([("x", Arc::new(x) as ArrayRef)]).unwrap();
    let f = write(&[batch], WriterOptions::default().with_policy(Policy::Base)).0;
    assert_eq!(inspect(&repeat_row_group(&f, 1)).unwrap().row_groups, 1);
    let err = inspect(&repeat_row_group(&f, 2)).unwrap_err();
    assert_eq!(err.to_string(), "column x: the column chunks add up to more than the file");
}

/// A file of `rows` rows of `x` in row groups of 1000, the first byte of the column chunk of
/// row group `bad` (if any) corrupted.
fn groups(rows: i64, bad: Option<usize>) -> Bytes {
    use tparquet::parquet::file::properties::WriterProperties;
    let x = Int64Array::from_iter_values(0..rows);
    let batch = RecordBatch::try_from_iter([("x", Arc::new(x) as ArrayRef)]).unwrap();
    let props = WriterProperties::builder().set_max_row_group_row_count(Some(1000)).build();
    let f = write(&[batch], WriterOptions::default().with_policy(Policy::Pcodec).with_properties(props)).0;
    let Some(bad) = bad else { return f };
    let metadata = tparquet::parquet::file::metadata::ParquetMetaDataReader::new().parse_and_finish(&f).unwrap();
    let start = metadata.row_group(bad).column(0).byte_range().0 as usize;
    let mut g = f.to_vec();
    g[start..start + 4].copy_from_slice(&[0xff; 4]);
    Bytes::from(g)
}

/// `verify(a, b)`, or `None` if it does not return within a minute.
fn verify_in_time(a: Bytes, b: Bytes) -> Option<tparquet::Result<Option<Difference>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(verify(a, b)));
    rx.recv_timeout(std::time::Duration::from_secs(60)).ok()
}

#[test]
fn verify_reports_errors_after_the_shorter_file_ends() {
    // the longer file cannot be read to its end: that is an error, not a row count (the
    // reader yields the same error again and again, so counting the rest never ended)
    let r = verify_in_time(groups(3000, Some(2)), groups(1000, None)).expect("verify returns");
    assert!(r.is_err(), "{r:?}");
    let r = verify_in_time(groups(1000, None), groups(3000, Some(1))).expect("verify returns");
    assert!(r.is_err(), "{r:?}");
    assert_eq!(
        verify(groups(3000, None), groups(1000, None)).unwrap(),
        Some(Difference::RowCount { a: 3000, b: 1000 })
    );
    assert_eq!(
        verify(groups(1000, None), groups(2000, None)).unwrap(),
        Some(Difference::RowCount { a: 1000, b: 2000 })
    );
    // before the end, a corrupt row group is an error too
    assert!(verify(groups(3000, Some(1)), groups(3000, None)).is_err());
}
