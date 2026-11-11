//! The conformance files: what each holds and how it is written.

use crate::rng::Rng;
use arrow_array::builder::{
    ArrayBuilder, Float64Builder, Int32Builder, Int64Builder, ListBuilder, MapBuilder, StringBuilder,
    StringDictionaryBuilder, StructBuilder,
};
use arrow_array::types::Int32Type;
use arrow_array::{
    Array, ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array, Int8Array, Int16Array,
    Int32Array, Int64Array, RecordBatch, StringArray, StructArray, Time64MicrosecondArray, TimestampMicrosecondArray,
    TimestampMillisecondArray, TimestampNanosecondArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Fields, Schema, SchemaRef};
use std::sync::Arc;
use tparquet::parquet::arrow::{ArrowSchemaConverter, ArrowWriter, add_encoded_arrow_schema_to_metadata};
use tparquet::parquet::basic::{Compression, Encoding, PcodecLevel, ZstdLevel};
use tparquet::parquet::file::metadata::{KeyValue, SortingColumn};
use tparquet::parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use tparquet::parquet::file::writer::SerializedFileWriter;
use tparquet::parquet::schema::types::ColumnPath;
use tparquet::{Policy, Result, Writer, WriterOptions};

/// One conformance file.
pub struct Case {
    pub name: &'static str,
    pub description: &'static str,
    /// The batches written, one row group each.
    pub batches: Vec<RecordBatch>,
    pub how: How,
}

/// How a case is written.
pub enum How {
    /// The high-level writer with these options.
    Writer(WriterOptions),
    /// `ArrowWriter` with these properties.
    Arrow(WriterProperties),
    /// One row group without rows and PCODEC column chunks without pages.
    EmptyRowGroup,
}

fn pcodec() -> Compression {
    Compression::PCODEC(PcodecLevel::default())
}

fn zstd(level: i32) -> Compression {
    Compression::ZSTD(ZstdLevel::try_new(level).expect("valid zstd level"))
}

fn batch(columns: Vec<(&str, ArrayRef, bool)>) -> RecordBatch {
    let fields: Vec<Field> =
        columns.iter().map(|(n, a, nullable)| Field::new(*n, a.data_type().clone(), *nullable)).collect();
    let arrays = columns.into_iter().map(|(_, a, _)| a).collect();
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).expect("valid batch")
}

fn arc(a: impl Array + 'static) -> ArrayRef {
    Arc::new(a)
}

/// `Some(v)` except every `every`-th row
fn holes<T>(v: Vec<T>, every: usize) -> Vec<Option<T>> {
    v.into_iter().enumerate().map(|(i, x)| (i % every != every - 1).then_some(x)).collect()
}

/// INT32, INT64, FLOAT, DOUBLE columns: smooth, random and decimal-like.
fn primitives(rng: &mut Rng, rows: usize) -> (Vec<i32>, Vec<i64>, Vec<f32>, Vec<f64>) {
    let mut acc = 0i64;
    let i32s = (0..rows).map(|i| (i as i32 * 37) % 1000 - 500).collect();
    let i64s = (0..rows)
        .map(|_| {
            acc += rng.below(1000) as i64;
            1_000_000_000_000 + acc
        })
        .collect();
    let f32s = (0..rows).map(|i| (i % 400) as f32 * 0.25 - 50.0).collect();
    let f64s = (0..rows).map(|_| rng.below(1_000_000) as f64 / 1000.0).collect();
    (i32s, i64s, f32s, f64s)
}

fn primitives_case(rng: &mut Rng, optional: bool) -> RecordBatch {
    let (a, b, c, d) = primitives(rng, 3000);
    if optional {
        batch(vec![
            ("i32", arc(Int32Array::from(holes(a, 7))), true),
            ("i64", arc(Int64Array::from(holes(b, 5))), true),
            ("f32", arc(Float32Array::from(holes(c, 3))), true),
            ("f64", arc(Float64Array::from(holes(d, 2))), true),
        ])
    } else {
        batch(vec![
            ("i32", arc(Int32Array::from(a)), false),
            ("i64", arc(Int64Array::from(b)), false),
            ("f32", arc(Float32Array::from(c)), false),
            ("f64", arc(Float64Array::from(d)), false),
        ])
    }
}

fn lists(rng: &mut Rng, rows: usize) -> RecordBatch {
    let mut ints = ListBuilder::new(Int64Builder::new());
    let mut levels = ListBuilder::new(Float64Builder::new());
    for i in 0..rows {
        match i % 9 {
            0 => ints.append_null(),
            1 => ints.append_value([]),
            _ => ints.append_value((0..rng.below(6)).map(|j| (j % 4 != 3).then_some((i as i64) * 10 + j as i64))),
        }
        if i % 11 == 0 {
            levels.append_null();
        } else {
            let mid = 25_000 + (i % 200) as i64;
            levels.append_value((0..10).map(|j| Some((mid - j) as f64 * 0.01)));
        }
    }
    batch(vec![("ints", arc(ints.finish()), true), ("levels", arc(levels.finish()), true)])
}

fn nested_struct(rng: &mut Rng, rows: usize) -> RecordBatch {
    let ids = Int32Array::from_iter_values(0..rows as i32);
    let bid: Vec<f64> = (0..rows).map(|i| 1000.0 + ((i * 3) % 500) as f64 * 0.5).collect();
    let ask: Vec<f64> = bid.iter().map(|b| b + 0.5).collect();
    let size = Int64Array::from(holes((0..rows).map(|_| rng.below(100) as i64 * 100).collect(), 6));
    let quote = StructArray::from(vec![
        (Arc::new(Field::new("bid", DataType::Float64, false)), arc(Float64Array::from(bid))),
        (Arc::new(Field::new("ask", DataType::Float64, false)), arc(Float64Array::from(ask))),
        (Arc::new(Field::new("size", DataType::Int64, true)), arc(size)),
    ]);
    let mut tags = ListBuilder::new(Int32Builder::new());
    for i in 0..rows {
        tags.append_value((0..i % 3).map(|j| Some(j as i32)));
    }
    let tags = tags.finish();
    let book = StructArray::from(vec![
        (Arc::new(Field::new("id", DataType::Int32, false)), arc(ids)),
        (Arc::new(Field::new("quote", quote.data_type().clone(), false)), arc(quote)),
        (Arc::new(Field::new("tags", tags.data_type().clone(), false)), arc(tags)),
    ]);
    batch(vec![("book", arc(book), false)])
}

fn temporal(rows: usize) -> RecordBatch {
    let base_ms = 1_735_689_600_000i64; // 2025-01-01T00:00:00Z
    let ms = TimestampMillisecondArray::from_iter_values((0..rows as i64).map(|i| base_ms + i * 1250));
    let us = TimestampMicrosecondArray::from_iter_values((0..rows as i64).map(|i| base_ms * 1000 + i * 999_983));
    let ns = TimestampNanosecondArray::from_iter_values((0..rows as i64).map(|i| base_ms * 1_000_000 + i * 7_000_001));
    let local = TimestampMillisecondArray::from_iter_values((0..rows as i64).map(|i| base_ms + i * 60_000));
    batch(vec![
        ("ts_ms_utc", arc(ms.with_timezone("UTC")), false),
        ("ts_us", arc(us), false),
        ("ts_ns_utc", arc(ns.with_timezone("UTC")), false),
        ("ts_ms_ny", arc(local.with_timezone("America/New_York")), false),
        ("date32", arc(Date32Array::from_iter_values((0..rows as i32).map(|i| 20_089 + i / 24))), false),
        (
            "time64_us",
            arc(Time64MicrosecondArray::from_iter_values((0..rows as i64).map(|i| i * 1_000_003 % 86_400_000_000))),
            false,
        ),
        (
            "dec18_4",
            arc(Decimal128Array::from_iter_values((0..rows as i128).map(|i| 1_234_500 + i * 25))
                .with_precision_and_scale(18, 4)
                .expect("valid decimal")),
            false,
        ),
        (
            "dec9_2",
            arc(Decimal128Array::from(holes((0..rows as i128).map(|i| -50_000 + i * 3).collect(), 10))
                .with_precision_and_scale(9, 2)
                .expect("valid decimal")),
            true,
        ),
    ])
}

fn decimal_floats(rng: &mut Rng, rows: usize) -> RecordBatch {
    let mut p = 6_500_000i64;
    let cents: Vec<f64> = (0..rows)
        .map(|_| {
            p += rng.below(21) as i64 - 10;
            p as f64 / 100.0
        })
        .collect();
    let btc: Vec<f64> = (0..rows).map(|_| (rng.below(500_000_000) + 1) as f64 / 1e8).collect();
    let halves: Vec<f32> = (0..rows).map(|i| ((i * 13) % 2000) as f32 * 0.5).collect();
    let tenths: Vec<f32> = (0..rows).map(|i| (i % 1000) as f32 / 10.0).collect();
    // mostly on the grid, a few values off it (exceptions)
    let mut exceptions = cents.clone();
    for i in (0..rows).step_by(250) {
        exceptions[i] = std::f64::consts::PI * i as f64;
    }
    batch(vec![
        ("price_cents", arc(Float64Array::from(cents)), false),
        ("amount_1e8", arc(Float64Array::from(btc)), false),
        ("f32_halves", arc(Float32Array::from(halves)), false),
        ("f32_tenths", arc(Float32Array::from(tenths)), false),
        ("with_exceptions", arc(Float64Array::from(exceptions)), false),
    ])
}

fn float_specials(rows: usize) -> RecordBatch {
    let d = [
        0.0,
        -0.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        f64::from_bits(0xfff8_0000_0000_0000),
        f64::from_bits(0x7ff0_0000_0000_0001),
        f64::from_bits(0x7ff8_dead_beef_0001),
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        f64::MAX,
        f64::MIN,
        1.5,
        -2.25,
    ];
    let f = [
        0.0,
        -0.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        f32::from_bits(0xffc0_0000),
        f32::from_bits(0x7f80_0001),
        f32::from_bits(0x7fc0_beef),
        f32::MIN_POSITIVE,
        f32::from_bits(1),
        f32::MAX,
        f32::MIN,
        1.5,
    ];
    batch(vec![
        ("f64", arc(Float64Array::from_iter_values((0..rows).map(|i| d[i % d.len()]))), false),
        ("f32", arc(Float32Array::from_iter_values((0..rows).map(|i| f[i % f.len()]))), false),
        ("f64_opt", arc(Float64Array::from(holes((0..rows).map(|i| d[(i * 5) % d.len()]).collect(), 4))), true),
    ])
}

/// A trades-like table: PCODEC numbers next to dictionary-encoded ZSTD strings.
fn mixed(rng: &mut Rng, rows: usize, offset: usize) -> RecordBatch {
    let mut t = 1_735_689_600_000i64 + offset as i64 * 37;
    let ts = TimestampMillisecondArray::from_iter_values((0..rows).map(|_| {
        t += rng.below(40) as i64;
        t
    }));
    let mut p = 9_500_000i64;
    let price = Float64Array::from_iter_values((0..rows).map(|_| {
        p += rng.below(11) as i64 - 5;
        p as f64 / 100.0
    }));
    let qty = Float64Array::from_iter_values((0..rows).map(|_| (rng.below(5000) + 1) as f64 / 1000.0));
    let mut sym = StringDictionaryBuilder::<Int32Type>::new();
    let mut venue = Vec::with_capacity(rows);
    let symbols = ["BTC-USD", "ETH-USD", "SOL-USD", "ADA-USD"];
    for _ in 0..rows {
        sym.append_value(symbols[rng.below(4) as usize]);
        venue.push(["XNAS", "XNYS", "ARCX"][rng.below(3) as usize]);
    }
    let side = BooleanArray::from_iter((0..rows).map(|_| Some(rng.below(2) == 0)));
    let note = StringArray::from(holes((0..rows).map(|i| format!("trade #{i} ünïcode \"quoted\"\n")).collect(), 3));
    batch(vec![
        ("ts", arc(ts.with_timezone("UTC")), false),
        ("price", arc(price), false),
        ("qty", arc(qty), false),
        ("symbol", arc(sym.finish()), false),
        ("venue", arc(StringArray::from(venue)), false),
        ("is_buy", arc(side), false),
        ("note", arc(note), true),
    ])
}

fn v1_props() -> WriterProperties {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(pcodec())
        .set_data_page_row_count_limit(1000)
        .set_write_batch_size(500)
        .build()
}

fn v1_pages(rng: &mut Rng) -> RecordBatch {
    let rows = 2500;
    let (a, b, _, d) = primitives(rng, rows);
    let random: Vec<i64> = (0..rows).map(|_| rng.next() as i64).collect();
    batch(vec![
        // required: PLAIN or dictionary pages of plain values (PCO, DECIMAL)
        ("i32", arc(Int32Array::from(a)), false),
        ("f64", arc(Float64Array::from(d)), false),
        // optional: levels in front of the values, the width no longer divides (ZSTD)
        ("i64_opt", arc(Int64Array::from(holes(b, 4))), true),
        // incompressible (RAW)
        ("random", arc(Int64Array::from(random)), false),
        // dictionary-encoded strings
        ("tag", arc(StringArray::from_iter_values((0..rows).map(|i| format!("t{}", i % 9)))), false),
    ])
}

fn v2_props() -> WriterProperties {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(pcodec())
        .set_dictionary_enabled(false)
        .set_encoding(Encoding::PLAIN)
        .set_column_encoding("tag".into(), Encoding::DELTA_LENGTH_BYTE_ARRAY)
        .set_data_page_row_count_limit(1000)
        .set_write_batch_size(500)
        .build()
}

/// map<int32, double> and map<string, int64>: null maps, empty maps, null values.
fn maps(rng: &mut Rng, rows: usize) -> RecordBatch {
    let mut by_id = MapBuilder::new(None, Int32Builder::new(), Float64Builder::new());
    let mut by_tag = MapBuilder::new(None, StringBuilder::new(), Int64Builder::new());
    for i in 0..rows {
        match i % 7 {
            0 => by_id.append(false).expect("map"),
            1 => by_id.append(true).expect("map"),
            _ => {
                for j in 0..rng.below(5) {
                    by_id.keys().append_value((i * 10 + j as usize) as i32);
                    if j == 2 {
                        by_id.values().append_null();
                    } else {
                        by_id.values().append_value((i as f64 + j as f64) * 0.25);
                    }
                }
                by_id.append(true).expect("map");
            }
        }
        if i % 5 == 0 {
            by_tag.append(i % 10 == 0).expect("map");
        } else {
            for j in 0..1 + i % 3 {
                by_tag.keys().append_value(["bid", "ask", "last"][j]);
                by_tag.values().append_value(1_000_000 + (i * 3 + j) as i64 * 17);
            }
            by_tag.append(true).expect("map");
        }
    }
    batch(vec![("by_id", arc(by_id.finish()), true), ("by_tag", arc(by_tag.finish()), true)])
}

/// list<struct<id int64, pts list<struct<x double, y int32>>>>, with a null and an empty value
/// at every level that has one.
fn deep_nesting(rng: &mut Rng, rows: usize) -> RecordBatch {
    let point = Fields::from(vec![Field::new("x", DataType::Float64, true), Field::new("y", DataType::Int32, true)]);
    let pts = Field::new_list("pts", Field::new_list_field(DataType::Struct(point.clone()), true), true);
    let leg = Fields::from(vec![Field::new("id", DataType::Int64, true), pts]);
    let points = ListBuilder::new(StructBuilder::from_fields(point.clone(), 0))
        .with_field(Field::new_list_field(DataType::Struct(point), true));
    let legs =
        StructBuilder::new(leg.clone(), vec![Box::new(Int64Builder::new()) as Box<dyn ArrayBuilder>, Box::new(points)]);
    let mut b = ListBuilder::new(legs).with_field(Field::new_list_field(DataType::Struct(leg), true));
    for i in 0..rows {
        match i % 13 {
            0 => {
                b.append_null();
                continue;
            }
            1 => {
                b.append(true);
                continue;
            }
            _ => {}
        }
        for j in 0..1 + rng.below(3) as usize {
            let legs = b.values();
            let k = (i + j) % 11;
            if k == 0 {
                // a null struct: its children get placeholders
                legs.field_builder::<Int64Builder>(0).expect("id").append_null();
                legs.field_builder::<ListBuilder<StructBuilder>>(1).expect("pts").append_null();
                legs.append(false);
                continue;
            }
            let id = legs.field_builder::<Int64Builder>(0).expect("id");
            if k == 1 {
                id.append_null()
            } else {
                id.append_value((i * 100 + j) as i64)
            }
            let pts = legs.field_builder::<ListBuilder<StructBuilder>>(1).expect("pts");
            match k {
                2 => pts.append_null(),
                3 => pts.append(true),
                _ => {
                    for m in 0..1 + rng.below(4) as usize {
                        let p = pts.values();
                        if (i + m) % 9 == 0 {
                            p.field_builder::<Float64Builder>(0).expect("x").append_null();
                            p.field_builder::<Int32Builder>(1).expect("y").append_null();
                            p.append(false);
                            continue;
                        }
                        let x = p.field_builder::<Float64Builder>(0).expect("x");
                        if m == 1 {
                            x.append_null()
                        } else {
                            x.append_value(i as f64 * 0.5 + m as f64)
                        }
                        let y = p.field_builder::<Int32Builder>(1).expect("y");
                        if m == 2 {
                            y.append_null()
                        } else {
                            y.append_value((i * 7 + m) as i32 - 500)
                        }
                        p.append(true);
                    }
                    pts.append(true);
                }
            }
            legs.append(true);
        }
        b.append(true);
    }
    batch(vec![("legs", arc(b.finish()), true)])
}

/// Low-cardinality values, then distinct ones: the dictionary outgrows its page size limit and
/// the chunk goes on with PLAIN pages.
fn dictionary_fallback(rng: &mut Rng, rows: usize) -> RecordBatch {
    let low = rows / 3;
    let ids = Int64Array::from_iter_values(
        (0..rows).map(|i| if i < low { 7_000_000_000 + rng.below(12) as i64 } else { 7_000_000_000 + i as i64 * 31 }),
    );
    let px = Float64Array::from(holes(
        (0..rows).map(|i| if i < low { 100.0 + rng.below(8) as f64 * 0.5 } else { 100.0 + i as f64 * 0.01 }).collect(),
        9,
    ));
    let small = Int32Array::from_iter_values((0..rows).map(|i| (i % 5) as i32));
    batch(vec![("id", arc(ids), false), ("px", arc(px), true), ("small", arc(small), false)])
}

fn dictionary_fallback_props() -> WriterProperties {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(pcodec())
        .set_dictionary_enabled(true)
        // the encoding after the fallback (by default DELTA_BINARY_PACKED for integers in V2)
        .set_encoding(Encoding::PLAIN)
        .set_dictionary_page_size_limit(1024)
        .set_data_page_row_count_limit(400)
        .set_write_batch_size(200)
        .build()
}

/// The same kinds of values with every pcodec level, one column per level.
fn levels(rng: &mut Rng, rows: usize) -> (RecordBatch, WriterProperties) {
    let mut b = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_dictionary_enabled(false)
        .set_encoding(Encoding::PLAIN);
    let mut columns = vec![];
    let (a, i, f, d) = primitives(rng, rows);
    for level in 0..=12u32 {
        let name = format!("level{level}");
        let array: ArrayRef = match level % 4 {
            0 => arc(Int64Array::from(i.clone())),
            1 => arc(Float64Array::from(d.clone())),
            2 => arc(Int32Array::from(a.clone())),
            _ => arc(Float32Array::from(f.clone())),
        };
        b = b.set_column_compression(
            ColumnPath::from(name.as_str()),
            Compression::PCODEC(PcodecLevel::try_new(level).expect("level")),
        );
        columns.push((name, array));
    }
    let fields: Vec<Field> = columns.iter().map(|(n, a)| Field::new(n, a.data_type().clone(), false)).collect();
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns.into_iter().map(|(_, a)| a).collect())
        .expect("valid batch");
    (batch, b.build())
}

/// One column per payload mode, with DATA_PAGE (V1) pages, which keep RAW payloads (a V2 page
/// would be stored uncompressed instead).
fn payload_modes(rng: &mut Rng, rows: usize) -> RecordBatch {
    batch(vec![
        ("raw", arc(Int64Array::from_iter_values((0..rows).map(|_| rng.next() as i64))), false),
        ("pco", arc(Int32Array::from_iter_values((0..rows as i32).map(|i| 40_000 + i * 3))), false),
        (
            "decimal",
            arc(Float64Array::from_iter_values((0..rows).map(|i| (25_000 + (i * 7) % 300) as f64 / 100.0))),
            false,
        ),
        ("zstd", arc(StringArray::from_iter_values((0..rows).map(|i| format!("order-{}", i % 50)))), false),
        // levels in front of the values: the width no longer divides
        ("zstd_levels", arc(Int64Array::from(holes((0..rows as i64).map(|i| i * 1000).collect(), 3))), true),
    ])
}

fn payload_modes_props() -> WriterProperties {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(pcodec())
        .set_dictionary_enabled(false)
        .set_encoding(Encoding::PLAIN)
        .build()
}

/// Every integer logical type Arrow writes in INT32 and INT64, at its extremes, and decimals at
/// the precision bounds of each physical type.
fn integer_types(rng: &mut Rng, rows: usize) -> RecordBatch {
    fn pick<T: Copy>(rng: &mut Rng, ends: &[T], i: usize, mid: impl Fn(u64) -> T) -> T {
        if i < ends.len() { ends[i] } else { mid(rng.next()) }
    }
    let n = rows;
    let i8s = Int8Array::from_iter_values((0..n).map(|i| pick(rng, &[i8::MIN, i8::MAX, 0, -1], i, |r| r as i8)));
    let i16s = Int16Array::from(holes(
        (0..n).map(|i| pick(rng, &[i16::MIN, i16::MAX, 0, -1], i, |r| (r % 2000) as i16 - 1000)).collect(),
        6,
    ));
    let i32s = Int32Array::from_iter_values((0..n).map(|i| pick(rng, &[i32::MIN, i32::MAX, 0, -1], i, |r| r as i32)));
    let u8s = UInt8Array::from_iter_values((0..n).map(|i| pick(rng, &[0, u8::MAX, 1, 128], i, |r| r as u8)));
    let u16s =
        UInt16Array::from_iter_values((0..n).map(|i| pick(rng, &[0, u16::MAX, 1, 32768], i, |r| (r % 300) as u16)));
    let u32s = UInt32Array::from(holes(
        (0..n).map(|i| pick(rng, &[0, u32::MAX, 1, 1 << 31], i, |r| u32::MAX - (r % 1000) as u32)).collect(),
        5,
    ));
    let u64s = UInt64Array::from_iter_values(
        (0..n).map(|i| pick(rng, &[0, u64::MAX, 1, 1 << 63], i, |r| u64::MAX - r % 100_000)),
    );
    let i64s = Int64Array::from_iter_values((0..n).map(|i| pick(rng, &[i64::MIN, i64::MAX, 0, -1], i, |r| r as i64)));
    let dec = |p: u8, s: i8, max: i128, rng: &mut Rng| {
        let v: Vec<Option<i128>> = (0..n)
            .map(|i| match i {
                0 => Some(-max),
                1 => Some(max),
                2 => Some(0),
                _ if i % 8 == 7 => None,
                _ => Some((rng.next() % (2 * max as u64 + 1)) as i128 - max),
            })
            .collect();
        arc(Decimal128Array::from(v).with_precision_and_scale(p, s).expect("valid decimal"))
    };
    batch(vec![
        ("i8", arc(i8s), false),
        ("i16", arc(i16s), true),
        ("i32", arc(i32s), false),
        ("u8", arc(u8s), false),
        ("u16", arc(u16s), false),
        ("u32", arc(u32s), true),
        ("u64", arc(u64s), false),
        ("i64", arc(i64s), false),
        ("dec1_0", dec(1, 0, 9, rng), true),
        ("dec9_9", dec(9, 9, 999_999_999, rng), true),
        ("dec10_2", dec(10, 2, 9_999_999_999, rng), true),
        ("dec18_0", dec(18, 0, 999_999_999_999_999_999, rng), true),
    ])
}

/// Auto: smooth timestamps (PCODEC wins) next to a column of a few large values in long runs
/// (the base codec's dictionary wins).
fn auto_choice(rng: &mut Rng, rows: usize) -> RecordBatch {
    let mut t = 1_735_689_600_000_000i64;
    let ts = TimestampMicrosecondArray::from_iter_values((0..rows).map(|_| {
        t += 1_000 + rng.below(50) as i64;
        t
    }));
    let codes = [-4_611_686_018_427_387_904i64, 9_007_199_254_740_993, 1_311_768_467_294_899_695];
    let regime = Int64Array::from_iter_values((0..rows).map(|i| codes[(i / 250) % 3]));
    let level = Float64Array::from_iter_values((0..rows).map(|i| (10_000 + (i % 200) as i64) as f64 / 100.0));
    batch(vec![
        ("ts", arc(ts.with_timezone("UTC")), false),
        ("regime", arc(regime), false),
        ("level", arc(level), false),
    ])
}

/// More than 2^18 values in one page: two pcodec chunks in one payload (pcodec's chunks hold
/// at most 2^18 numbers).
fn large_page(rows: usize, per_row: usize) -> RecordBatch {
    let mut b = ListBuilder::new(Int32Builder::new()).with_field(Field::new_list_field(DataType::Int32, false));
    for r in 0..rows {
        b.append_value((0..per_row).map(|i| Some(((r * per_row + i) / 4096 % 10) as i32)));
    }
    batch(vec![("digits", arc(b.finish()), false)])
}

fn large_page_props() -> WriterProperties {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(pcodec())
        .set_dictionary_enabled(false)
        .set_encoding(Encoding::PLAIN)
        .set_data_page_size_limit(8 << 20)
        .set_data_page_row_count_limit(usize::MAX)
        .build()
}

fn statistics_props() -> WriterProperties {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(pcodec())
        .set_dictionary_enabled(false)
        .set_encoding(Encoding::PLAIN)
        .set_statistics_enabled(EnabledStatistics::Page)
        .set_write_page_header_statistics(true)
        .set_bloom_filter_enabled(true)
        .set_data_page_row_count_limit(300)
        .set_write_batch_size(100)
        .set_sorting_columns(Some(vec![SortingColumn { column_idx: 0, descending: false, nulls_first: false }]))
        .build()
}

/// Pages whose values are all null: V2 values sections of expected size 0 (`EMPTY`), and a
/// column without any value, whose dictionary page is empty too.
fn all_null_pages(rows: usize) -> RecordBatch {
    let sparse = Int64Array::from_iter((0..rows).map(|i| (i >= 600 && i % 4 == 0).then_some(i as i64 * 11)));
    let tail = Float64Array::from_iter((0..rows).map(|i| (i < 300).then_some(i as f64 * 0.125)));
    let none = Int32Array::from(vec![None; rows]);
    batch(vec![("sparse", arc(sparse), true), ("tail", arc(tail), true), ("none", arc(none), true)])
}

fn all_null_pages_props() -> WriterProperties {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(pcodec())
        .set_dictionary_enabled(false)
        .set_column_dictionary_enabled("none".into(), true)
        .set_encoding(Encoding::PLAIN)
        .set_data_page_row_count_limit(200)
        .set_write_batch_size(100)
        .build()
}

/// Key-value pairs of the caller in the footer: the writer keeps them, and replaces a stale
/// `parquet.pcodec.version` with its own.
fn key_value_options() -> WriterOptions {
    let kv = |k: &str, v: Option<&str>| KeyValue { key: k.into(), value: v.map(String::from) };
    let props = WriterProperties::builder()
        .set_key_value_metadata(Some(vec![
            kv("parquet.pcodec.version", Some("0")),
            kv("origin", Some("synthetic test data")),
            kv("schema.note", Some("prix en €, ünïcode \"quoted\"\n")),
            kv("empty", Some("")),
            kv("no.value", None),
        ]))
        .build();
    WriterOptions::default().with_policy(Policy::Pcodec).with_properties(props)
}

/// Every case, in a fixed order.
pub fn all() -> Vec<Case> {
    let mut rng = Rng(0x5043_0001);
    let rng = &mut rng;
    let pcodec_policy = || WriterOptions::default().with_policy(Policy::Pcodec);
    vec![
        Case {
            name: "primitives_required",
            description: "INT32, INT64, FLOAT, DOUBLE, required, PCODEC",
            batches: vec![primitives_case(rng, false)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "primitives_optional",
            description: "INT32, INT64, FLOAT, DOUBLE with nulls, PCODEC",
            batches: vec![primitives_case(rng, true)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "lists",
            description: "list<int64> and list<double> with null lists, empty lists and null items, PCODEC",
            batches: vec![lists(rng, 1200)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "nested_struct",
            description: "struct<id int32, quote struct<bid, ask double, size int64>, tags list<int32>>, PCODEC",
            batches: vec![nested_struct(rng, 2000)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "temporal",
            description: "timestamps (ms, us, ns; UTC, none, America/New_York), date32, time64, decimals in INT64 and INT32, PCODEC",
            batches: vec![temporal(1200)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "decimal_floats",
            description: "decimal-valued doubles and floats (prices, 1e-8 amounts), some off the grid; DECIMAL payloads",
            batches: vec![decimal_floats(rng, 1500)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "float_specials",
            description: "NaN payloads (quiet, signalling, negative), +-0, +-inf, subnormals, extremes",
            batches: vec![float_specials(1000)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "empty",
            description: "a row group with 0 rows and PCODEC column chunks without pages",
            batches: vec![primitives_case(rng, true).slice(0, 0)],
            how: How::EmptyRowGroup,
        },
        Case {
            name: "multiple_row_groups",
            description: "5 row groups of 400 rows, Auto policy",
            batches: (0..5).map(|i| mixed(rng, 400, i * 400)).collect(),
            how: How::Writer(WriterOptions::default()),
        },
        Case {
            name: "pages_v1",
            description: "PCODEC with DATA_PAGE (V1) pages and dictionary pages; RAW, PCO, DECIMAL and ZSTD payloads",
            batches: vec![v1_pages(rng)],
            how: How::Arrow(v1_props()),
        },
        Case {
            name: "pages_v2",
            description: "PCODEC with DATA_PAGE_V2 pages, PLAIN, several pages per chunk, a V2 page stored uncompressed",
            batches: vec![v1_pages(rng)],
            how: How::Arrow(v2_props()),
        },
        Case {
            name: "mixed",
            description: "trades: PCODEC numeric columns next to dictionary-encoded ZSTD strings and booleans (Auto)",
            batches: vec![mixed(rng, 1500, 0)],
            how: How::Writer(WriterOptions::default()),
        },
        Case {
            name: "standard_control",
            description: "control: the mixed table as standard Parquet (ZSTD level 3, no PCODEC)",
            batches: vec![mixed(rng, 1500, 0)],
            how: How::Arrow(WriterProperties::builder().set_compression(zstd(3)).build()),
        },
        Case {
            name: "maps",
            description: "map<int32, double> and map<string, int64> with null maps, empty maps and null values, PCODEC",
            batches: vec![maps(rng, 1000)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "deep_nesting",
            description: "list<struct<id int64, pts list<struct<x double, y int32>>>> with nulls and empty lists at every level, PCODEC",
            batches: vec![deep_nesting(rng, 600)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "dictionary_fallback",
            description: "dictionary-encoded PCODEC columns whose dictionary outgrows its limit: RLE_DICTIONARY then PLAIN pages in one chunk",
            batches: vec![dictionary_fallback(rng, 2400)],
            how: How::Arrow(dictionary_fallback_props()),
        },
        {
            let (batch, props) = levels(rng, 800);
            Case {
                name: "pcodec_levels",
                description: "one PCODEC column per pcodec level 0 to 12 (INT64, DOUBLE, INT32, FLOAT in turn)",
                batches: vec![batch],
                how: How::Arrow(props),
            }
        },
        Case {
            name: "payload_modes",
            description: "one column per payload mode side by side, DATA_PAGE (V1) pages: RAW, PCO, DECIMAL, ZSTD (strings; levels before INT64 values)",
            batches: vec![payload_modes(rng, 1000)],
            how: How::Arrow(payload_modes_props()),
        },
        Case {
            name: "integer_types",
            description: "int8, int16, int32, uint8, uint16, uint32 in INT32; uint64, int64 in INT64, at their extremes; decimals at the precision bounds of INT32 and INT64, PCODEC",
            batches: vec![integer_types(rng, 400)],
            how: How::Writer(pcodec_policy()),
        },
        Case {
            name: "auto_choice",
            description: "Auto: PCODEC for the timestamps and the decimal doubles, the base codec (ZSTD) for an INT64 column of three values in long runs",
            batches: vec![auto_choice(rng, 3000)],
            how: How::Writer(WriterOptions::default()),
        },
        Case {
            name: "large_page",
            description: "one DATA_PAGE_V2 page of 270000 INT32 values in 3 rows of a list: a payload of two pcodec chunks",
            batches: vec![large_page(3, 90_000)],
            how: How::Arrow(large_page_props()),
        },
        Case {
            name: "statistics",
            description: "PCODEC with page statistics in the column index and the page headers, offset index, bloom filters and sorting columns",
            batches: vec![primitives_case(rng, true).slice(0, 1500)],
            how: How::Arrow(statistics_props()),
        },
        Case {
            name: "all_null_pages",
            description: "PCODEC pages whose values are all null (V2 values sections of expected size 0, EMPTY), and a column without values with an empty dictionary page",
            batches: vec![all_null_pages(1000)],
            how: How::Arrow(all_null_pages_props()),
        },
        Case {
            name: "key_value_metadata",
            description: "key-value pairs of the writer's caller in the footer, next to parquet.pcodec.version (a stale value given by the caller replaced)",
            batches: vec![primitives_case(rng, false).slice(0, 500)],
            how: How::Writer(key_value_options()),
        },
    ]
}

/// Writes a case to Parquet bytes.
pub fn write(case: &Case) -> Result<Vec<u8>> {
    let schema: SchemaRef = case.batches[0].schema();
    let mut out = Vec::new();
    match &case.how {
        How::Writer(options) => {
            let mut w = Writer::try_new(&mut out, schema, options.clone())?;
            for b in &case.batches {
                w.write(b)?;
                w.flush()?;
            }
            w.close()?;
        }
        How::Arrow(props) => {
            let mut w = ArrowWriter::try_new(&mut out, schema, Some(props.clone()))?;
            for b in &case.batches {
                w.write(b)?;
                w.flush()?;
            }
            w.close()?;
        }
        How::EmptyRowGroup => {
            let descr = ArrowSchemaConverter::new().convert(&schema)?;
            // no dictionary: a chunk without values would still get an empty dictionary page,
            // and arrow-rs writes data_page_offset 0 for a chunk without data pages, which
            // then lies before the dictionary page offset
            let mut props = WriterProperties::builder().set_compression(pcodec()).set_dictionary_enabled(false).build();
            add_encoded_arrow_schema_to_metadata(&schema, &mut props);
            let mut w = SerializedFileWriter::new(&mut out, descr.root_schema_ptr(), Arc::new(props))?;
            let mut rg = w.next_row_group()?;
            while let Some(c) = rg.next_column()? {
                c.close()?;
            }
            rg.close()?;
            w.close()?;
        }
    }
    Ok(out)
}
