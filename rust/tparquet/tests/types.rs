//! Every Arrow type the writer can meet, under every policy: the data reads back bit for bit
//! with its Arrow type, and PCODEC is used exactly on the INT32, INT64, FLOAT and DOUBLE
//! leaves.

mod common;

use arrow_array::builder::{BinaryViewBuilder, StringDictionaryBuilder, StringViewBuilder};
use arrow_array::types::*;
use arrow_array::*;
use arrow_buffer::{IntervalDayTime, i256};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use bytes::Bytes;
use common::{batch_lines, codecs, lines, report};
use std::sync::Arc;
use tparquet::parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use tparquet::parquet::basic::{Compression, Type as PhysicalType};
use tparquet::parquet::file::properties::WriterProperties;
use tparquet::{Policy, Writer, WriterOptions};

/// `Some` except every 7th value
fn holes<T>(v: impl IntoIterator<Item = T>) -> Vec<Option<T>> {
    v.into_iter().enumerate().map(|(i, x)| (i % 7 != 3).then_some(x)).collect()
}

/// One column per Arrow type, `rows` long, nulls in every 7th row; with its Parquet physical
/// type.
fn columns(rows: usize) -> Vec<(&'static str, ArrayRef, PhysicalType)> {
    let n = rows as i64;
    let ints = || 0..n;
    let smooth = move |i: i64| 1_700_000_000_000 + i * 37;
    use PhysicalType as P;
    vec![
        ("i8", Arc::new(Int8Array::from(holes(ints().map(|i| (i % 200 - 100) as i8)))) as ArrayRef, P::INT32),
        ("i16", Arc::new(Int16Array::from(holes(ints().map(|i| (i * 13 % 30000) as i16)))), P::INT32),
        ("i32", Arc::new(Int32Array::from(holes(ints().map(|i| (i * 1001) as i32 - 5_000_000)))), P::INT32),
        ("i64", Arc::new(Int64Array::from(holes(ints().map(smooth)))), P::INT64),
        ("u8", Arc::new(UInt8Array::from(holes(ints().map(|i| (i % 256) as u8)))), P::INT32),
        ("u16", Arc::new(UInt16Array::from(holes(ints().map(|i| (i * 7 % 65536) as u16)))), P::INT32),
        // values above i32::MAX are stored as negative INT32s
        ("u32", Arc::new(UInt32Array::from(holes(ints().map(|i| u32::MAX - i as u32)))), P::INT32),
        ("u64", Arc::new(UInt64Array::from(holes(ints().map(|i| u64::MAX - i as u64 * 3)))), P::INT64),
        (
            "f16",
            Arc::new(Float16Array::from(holes(
                ints().map(|i| <Float16Type as ArrowPrimitiveType>::Native::from_f32(i as f32 / 8.0)),
            ))),
            P::FIXED_LEN_BYTE_ARRAY,
        ),
        ("f32", Arc::new(Float32Array::from(holes(ints().map(|i| i as f32 * 0.25 - 3.0)))), P::FLOAT),
        ("f64", Arc::new(Float64Array::from(holes(ints().map(|i| (100_000 + i) as f64 / 100.0)))), P::DOUBLE),
        (
            "f64_bits",
            Arc::new(Float64Array::from(holes(
                ints().map(|i| f64::from_bits((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))),
            ))),
            P::DOUBLE,
        ),
        (
            "dec32",
            Arc::new(Decimal32Array::from(holes(ints().map(|i| i as i32 * 3))).with_precision_and_scale(9, 2).unwrap()),
            P::INT32,
        ),
        (
            "dec64",
            Arc::new(Decimal64Array::from(holes(ints().map(smooth))).with_precision_and_scale(18, 3).unwrap()),
            P::INT64,
        ),
        (
            "dec64_small",
            Arc::new(Decimal64Array::from(holes(ints().map(|i| i - 50))).with_precision_and_scale(5, 1).unwrap()),
            P::INT32,
        ),
        (
            "dec128_int32",
            Arc::new(Decimal128Array::from(holes(ints().map(|i| i as i128))).with_precision_and_scale(8, 2).unwrap()),
            P::INT32,
        ),
        (
            "dec128_int64",
            Arc::new(
                Decimal128Array::from(holes(ints().map(|i| smooth(i) as i128)))
                    .with_precision_and_scale(16, 2)
                    .unwrap(),
            ),
            P::INT64,
        ),
        (
            "dec128_fixed",
            Arc::new(
                Decimal128Array::from(holes(ints().map(|i| i as i128 * 10i128.pow(30))))
                    .with_precision_and_scale(38, 4)
                    .unwrap(),
            ),
            P::FIXED_LEN_BYTE_ARRAY,
        ),
        (
            "dec256_int64",
            Arc::new(
                Decimal256Array::from(holes(ints().map(|i| i256::from_i128(i as i128 - 7))))
                    .with_precision_and_scale(12, 0)
                    .unwrap(),
            ),
            P::INT64,
        ),
        (
            "dec256_fixed",
            Arc::new(
                Decimal256Array::from(holes(
                    ints().map(|i| i256::from_i128(i as i128).wrapping_mul(i256::MAX / i256::from_i128(n as i128 + 1))),
                ))
                .with_precision_and_scale(76, 10)
                .unwrap(),
            ),
            P::FIXED_LEN_BYTE_ARRAY,
        ),
        ("date32", Arc::new(Date32Array::from(holes(ints().map(|i| 19_000 + i as i32)))), P::INT32),
        ("date64", Arc::new(Date64Array::from(holes(ints().map(|i| 1_640_995_200_000 + i * 86_400_000)))), P::INT64),
        ("time32s", Arc::new(Time32SecondArray::from(holes(ints().map(|i| (i % 86_400) as i32)))), P::INT32),
        ("time32ms", Arc::new(Time32MillisecondArray::from(holes(ints().map(|i| (i * 50) as i32)))), P::INT32),
        ("time64us", Arc::new(Time64MicrosecondArray::from(holes(ints().map(|i| i * 1_000_003)))), P::INT64),
        ("time64ns", Arc::new(Time64NanosecondArray::from(holes(ints().map(|i| i * 999_999_937)))), P::INT64),
        ("ts_s", Arc::new(TimestampSecondArray::from(holes(ints().map(|i| 1_700_000_000 + i)))), P::INT64),
        (
            "ts_ms_utc",
            Arc::new(TimestampMillisecondArray::from(holes(ints().map(smooth))).with_timezone("UTC")),
            P::INT64,
        ),
        (
            "ts_us_ny",
            Arc::new(
                TimestampMicrosecondArray::from(holes(ints().map(|i| smooth(i) * 1000)))
                    .with_timezone("America/New_York"),
            ),
            P::INT64,
        ),
        (
            "ts_ns_offset",
            Arc::new(
                TimestampNanosecondArray::from(holes(ints().map(|i| smooth(i) * 1_000_000))).with_timezone("+02:00"),
            ),
            P::INT64,
        ),
        ("ts_ns", Arc::new(TimestampNanosecondArray::from(holes(ints().map(|i| i * 17)))), P::INT64),
        ("dur_s", Arc::new(DurationSecondArray::from(holes(ints().map(|i| i - 3)))), P::INT64),
        ("dur_ms", Arc::new(DurationMillisecondArray::from(holes(ints().map(|i| i * 1000)))), P::INT64),
        ("dur_us", Arc::new(DurationMicrosecondArray::from(holes(ints().map(|i| -i)))), P::INT64),
        ("dur_ns", Arc::new(DurationNanosecondArray::from(holes(ints().map(|i| i << 20)))), P::INT64),
        (
            "interval_ym",
            Arc::new(IntervalYearMonthArray::from(holes(ints().map(|i| i as i32 % 120)))),
            P::FIXED_LEN_BYTE_ARRAY,
        ),
        (
            "interval_dt",
            Arc::new(IntervalDayTimeArray::from(holes(
                ints().map(|i| IntervalDayTime::new(i as i32, (i * 1000) as i32)),
            ))),
            P::FIXED_LEN_BYTE_ARRAY,
        ),
        ("bool", Arc::new(BooleanArray::from(holes(ints().map(|i| i % 3 == 0)))), P::BOOLEAN),
        ("utf8", Arc::new(StringArray::from(holes(ints().map(|i| format!("s{}", i % 50))))), P::BYTE_ARRAY),
        ("large_utf8", Arc::new(LargeStringArray::from(holes(ints().map(|i| format!("é{i}"))))), P::BYTE_ARRAY),
        ("utf8_view", Arc::new(string_view(rows)), P::BYTE_ARRAY),
        ("binary", Arc::new(BinaryArray::from_iter(holes(ints().map(|i| i.to_le_bytes().to_vec())))), P::BYTE_ARRAY),
        (
            "large_binary",
            Arc::new(LargeBinaryArray::from_iter(holes(ints().map(|i| vec![i as u8; (i % 5) as usize])))),
            P::BYTE_ARRAY,
        ),
        ("binary_view", Arc::new(binary_view(rows)), P::BYTE_ARRAY),
        (
            "fixed_binary",
            Arc::new(
                FixedSizeBinaryArray::try_from_sparse_iter_with_size(
                    holes(ints().map(|i| i.to_be_bytes())).into_iter(),
                    8,
                )
                .unwrap(),
            ),
            P::FIXED_LEN_BYTE_ARRAY,
        ),
        ("dict_utf8", Arc::new(string_dictionary(rows)), P::BYTE_ARRAY),
        (
            "dict_i64",
            Arc::new(DictionaryArray::<Int16Type>::new(
                Int16Array::from(holes(ints().map(|i| (i % 4) as i16))),
                Arc::new(Int64Array::from(vec![10, -20, 30_000_000_000, i64::MIN])),
            )),
            P::INT64,
        ),
        (
            "dict_f64",
            Arc::new(DictionaryArray::<UInt8Type>::new(
                UInt8Array::from(holes(ints().map(|i| (i % 3) as u8))),
                Arc::new(Float64Array::from(vec![0.5, -0.0, f64::NAN])),
            )),
            P::DOUBLE,
        ),
        ("null", Arc::new(NullArray::new(rows)), P::INT32),
    ]
}

fn string_view(rows: usize) -> StringViewArray {
    let mut b = StringViewBuilder::new();
    for (i, v) in holes((0..rows).map(|i| format!("a view string that is long enough: {i}"))).into_iter().enumerate() {
        match v {
            Some(v) if i % 2 == 0 => b.append_value(v),
            Some(_) => b.append_value("short"),
            None => b.append_null(),
        }
    }
    b.finish()
}

fn binary_view(rows: usize) -> BinaryViewArray {
    let mut b = BinaryViewBuilder::new();
    for v in holes((0..rows).map(|i| vec![(i % 251) as u8; i % 20])) {
        b.append_option(v);
    }
    b.finish()
}

fn string_dictionary(rows: usize) -> DictionaryArray<Int32Type> {
    let mut b = StringDictionaryBuilder::<Int32Type>::new();
    for v in holes((0..rows).map(|i| format!("k{}", i % 9))) {
        b.append_option(v);
    }
    b.finish()
}

fn eligible(t: PhysicalType) -> bool {
    matches!(t, PhysicalType::INT32 | PhysicalType::INT64 | PhysicalType::FLOAT | PhysicalType::DOUBLE)
}

fn batch_of(cols: Vec<(&'static str, ArrayRef, PhysicalType)>, nullable: bool) -> RecordBatch {
    let fields: Vec<Field> = cols
        .iter()
        .map(|(n, a, _)| {
            Field::new(*n, a.data_type().clone(), nullable || a.null_count() > 0 || a.data_type() == &DataType::Null)
        })
        .collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols.into_iter().map(|(_, a, _)| a).collect()).unwrap()
}

/// Writes `batches` with `options` and checks what every leaf got.
fn write(batches: &[RecordBatch], options: WriterOptions) -> Bytes {
    let mut out = Vec::new();
    let mut w = Writer::try_new(&mut out, batches[0].schema(), options).unwrap();
    for b in batches {
        w.write(b).unwrap();
    }
    let (metadata, chosen) = w.finish().unwrap();
    let file = Bytes::from(out);
    // the codecs the writer reports are the ones in the file
    for rg in metadata.row_groups() {
        for (c, chosen) in rg.columns().iter().zip(&chosen) {
            assert_eq!(c.column_path().string(), chosen.path);
            // the footer has no levels: compare the codecs without them
            let name = |c: Compression| c.to_string().split('(').next().unwrap().to_string();
            assert_eq!(name(c.compression()), name(chosen.codec), "{}", chosen.path);
        }
    }
    file
}

/// The file's batches, read with the Arrow schema it was written with.
fn read(file: &Bytes) -> Vec<RecordBatch> {
    ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap().build().unwrap().map(|b| b.unwrap()).collect()
}

/// `batches` and the file hold the same rows, compared bit for bit as canonical JSON.
fn assert_same(file: &Bytes, batches: &[RecordBatch]) {
    let got = read(file);
    let schema = batches[0].schema();
    let got_rows: usize = got.iter().map(|b| b.num_rows()).sum();
    assert_eq!(got_rows, batches.iter().map(|b| b.num_rows()).sum::<usize>());
    let reader = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
    for (f, g) in schema.fields().iter().zip(reader.schema().fields()) {
        assert_eq!(f.data_type(), g.data_type(), "{}", f.name());
    }
    assert_eq!(batch_lines(&got), batch_lines(batches));
}

const POLICIES: [Policy; 3] = [Policy::Auto, Policy::Pcodec, Policy::Base];

#[test]
fn every_type_on_its_own_under_every_policy() {
    for rows in [1, 2, 1000] {
        for (name, array, physical) in columns(rows) {
            let batch = batch_of(vec![(name, array, physical)], true);
            for policy in POLICIES {
                let file = write(std::slice::from_ref(&batch), WriterOptions::default().with_policy(policy));
                assert_same(&file, std::slice::from_ref(&batch));
                let r = report(&file);
                assert_eq!(r.columns[0].physical_type, physical.to_string(), "{name}");
                let codec = &r.columns[0].codecs[0];
                match policy {
                    Policy::Pcodec => {
                        assert_eq!(codec == "PCODEC", eligible(physical), "{name} {rows}: {codec}")
                    }
                    Policy::Base => assert_eq!(codec, "ZSTD", "{name}"),
                    Policy::Auto => assert!(codec == "ZSTD" || (eligible(physical) && codec == "PCODEC"), "{name}"),
                }
            }
        }
    }
}

#[test]
fn all_types_in_one_file() {
    for nullable in [false, true] {
        let batches: Vec<RecordBatch> =
            [700, 1, 300].into_iter().map(|rows| batch_of(columns(rows), nullable)).collect();
        for policy in POLICIES {
            let file = write(&batches, WriterOptions::default().with_policy(policy));
            assert_same(&file, &batches);
            let r = report(&file);
            assert_eq!(r.rows, 1001);
            for ((name, _, physical), c) in columns(1).into_iter().zip(&r.columns) {
                assert_eq!(c.path, name);
                let pcodec = c.codecs == ["PCODEC"];
                if policy == Policy::Pcodec {
                    assert_eq!(pcodec, eligible(physical), "{name}");
                    // PCODEC chunks are PLAIN without a dictionary page, in V2 pages
                    if pcodec {
                        assert!(!c.pages.contains_key("DICTIONARY_PAGE"), "{name}");
                        assert!(!c.pages.contains_key("DATA_PAGE"), "{name}");
                    }
                } else if policy == Policy::Base {
                    assert!(!pcodec);
                }
            }
            assert_eq!(r.pcodec_version.is_some(), r.columns.iter().any(|c| c.codecs == ["PCODEC"]));
        }
    }
}

#[test]
fn all_null_columns() {
    for rows in [1, 5000] {
        let cols: Vec<(&str, ArrayRef, PhysicalType)> = columns(rows)
            .into_iter()
            .map(|(n, a, p)| (n, arrow_array::new_null_array(a.data_type(), rows), p))
            .collect();
        let batch = batch_of(cols, true);
        for policy in POLICIES {
            let file = write(std::slice::from_ref(&batch), WriterOptions::default().with_policy(policy));
            assert_same(&file, std::slice::from_ref(&batch));
            let r = report(&file);
            if policy == Policy::Pcodec {
                // pages without values: nothing for pcodec to decode
                let c = r.columns.iter().find(|c| c.path == "i64").unwrap();
                assert_eq!(c.codecs, ["PCODEC"]);
                assert!(c.modes.keys().all(|m| m == "EMPTY" || m == "UNCOMPRESSED" || m == "RAW"), "{c:?}");
            }
        }
    }
}

#[test]
fn batches_of_zero_and_one_row() {
    let empty = batch_of(columns(0), true);
    let one = batch_of(columns(1), true);
    for policy in POLICIES {
        // only empty batches: a file without row groups
        let file = write(&[empty.clone(), empty.clone()], WriterOptions::default().with_policy(policy));
        let r = report(&file);
        assert_eq!((r.rows, r.row_groups), (0, 0));
        assert_eq!(r.pcodec_version, None);
        assert!(read(&file).is_empty());
        assert_eq!(
            ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap().schema().fields(),
            empty.schema().fields()
        );

        // empty batches around single rows
        let batches = [empty.clone(), one.clone(), empty.clone(), one.clone(), empty.clone()];
        let file = write(&batches, WriterOptions::default().with_policy(policy));
        assert_same(&file, &batches);
        assert_eq!(report(&file).rows, 2);
    }
}

#[test]
fn auto_on_a_one_row_first_batch_holds_for_the_whole_file() {
    // the choice is made on the first batch with rows, however small
    let first = batch_of(columns(1), true);
    let rest = batch_of(columns(3000), true);
    let mut out = Vec::new();
    let mut w = Writer::try_new(&mut out, first.schema(), WriterOptions::default()).unwrap();
    w.write(&first).unwrap();
    let chosen = w.codecs().unwrap();
    w.write(&rest).unwrap();
    w.flush().unwrap();
    w.write(&rest).unwrap();
    let (_, after) = w.finish().unwrap();
    assert_eq!(chosen, after);
    let file = Bytes::from(out);
    assert_eq!(report(&file).row_groups, 2);
    assert_same(&file, &[first, rest.clone(), rest]);
}

#[test]
fn base_codecs_of_every_kind() {
    use tparquet::parquet::basic::{BrotliLevel, GzipLevel, ZstdLevel};
    let batch = batch_of(columns(500), true);
    for base in [
        Compression::UNCOMPRESSED,
        Compression::SNAPPY,
        Compression::GZIP(GzipLevel::default()),
        Compression::BROTLI(BrotliLevel::default()),
        Compression::LZ4_RAW,
        Compression::ZSTD(ZstdLevel::try_new(1).unwrap()),
    ] {
        for policy in POLICIES {
            let file =
                write(std::slice::from_ref(&batch), WriterOptions::default().with_policy(policy).with_base(base));
            assert_same(&file, std::slice::from_ref(&batch));
            let name = report(&file).columns.iter().find(|c| c.path == "utf8").unwrap().codecs[0].clone();
            assert!(base.to_string().starts_with(&name), "{base} {name}");
        }
    }
}

#[test]
fn every_level() {
    let batch = batch_of(columns(3000), true);
    let mut sizes = vec![];
    for level in 0..=12 {
        let file = write(
            std::slice::from_ref(&batch),
            WriterOptions::default().with_policy(Policy::Pcodec).with_level(level).unwrap(),
        );
        assert_same(&file, std::slice::from_ref(&batch));
        sizes.push(report(&file).columns.iter().map(|c| c.compressed_bytes).sum::<i64>());
    }
    // the level reaches pcodec: not every level gives the same size
    assert!(sizes.iter().any(|s| *s != sizes[0]), "{sizes:?}");
}

#[test]
fn coerced_types_use_the_parquet_names() {
    // coerce_types: Date64 is stored as INT32 dates, list items are `element`
    let mut lb = arrow_array::builder::ListBuilder::new(arrow_array::builder::Int64Builder::new());
    for i in 0..100 {
        lb.append_value([Some(i), None, Some(i * 2)]);
    }
    let date64 = Date64Array::from_iter_values((0..100).map(|i| 1_640_995_200_000 + i * 86_400_000));
    let batch =
        RecordBatch::try_from_iter([("d", Arc::new(date64) as ArrayRef), ("l", Arc::new(lb.finish()) as ArrayRef)])
            .unwrap();
    let props = WriterProperties::builder().set_coerce_types(true).build();
    let options = WriterOptions::default()
        .with_policy(Policy::Base)
        .with_column("l.list.element", Policy::Pcodec)
        .with_column("d", Policy::Pcodec)
        .with_properties(props);
    let file = write(std::slice::from_ref(&batch), options);
    assert_eq!(
        codecs(&file),
        [("d".to_string(), "PCODEC".to_string()), ("l.list.element".to_string(), "PCODEC".to_string())]
    );
    assert_eq!(report(&file).columns[0].physical_type, "INT32");
    assert_eq!(lines(&file), batch_lines(&[batch]));
}

#[test]
fn time_units_and_zones_are_kept() {
    let cols = columns(10);
    let batch = batch_of(cols, true);
    let file = write(std::slice::from_ref(&batch), WriterOptions::default().with_policy(Policy::Pcodec));
    let schema = ParquetRecordBatchReaderBuilder::try_new(file).unwrap().schema().clone();
    let field = |n: &str| schema.field_with_name(n).unwrap().data_type().clone();
    assert_eq!(field("ts_us_ny"), DataType::Timestamp(TimeUnit::Microsecond, Some("America/New_York".into())));
    assert_eq!(field("ts_ns_offset"), DataType::Timestamp(TimeUnit::Nanosecond, Some("+02:00".into())));
    assert_eq!(field("ts_ns"), DataType::Timestamp(TimeUnit::Nanosecond, None));
    assert_eq!(field("dur_ms"), DataType::Duration(TimeUnit::Millisecond));
    assert_eq!(field("dec32"), DataType::Decimal32(9, 2));
    assert_eq!(field("dict_f64"), DataType::Dictionary(Box::new(DataType::UInt8), Box::new(DataType::Float64)));
}
