//! Random Arrow batches round-trip bit for bit through every policy.

mod common;

use arrow_array::builder::{Float64Builder, ListBuilder};
use arrow_array::{
    Array, ArrayRef, Decimal128Array, Float32Array, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray,
    StructArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema};
use common::{batch_lines, lines, write};
use proptest::prelude::*;
use std::sync::Arc;
use tparquet::{Policy, WriterOptions};

/// Values that are often smooth or decimal (so PCO and DECIMAL are exercised), sometimes
/// arbitrary bits (NaN payloads, infinities, signed zeros).
fn f64_value() -> impl Strategy<Value = f64> {
    prop_oneof![
        (-100_000i64..100_000).prop_map(|v| v as f64 / 100.0),
        any::<u64>().prop_map(f64::from_bits),
        Just(-0.0),
        Just(f64::NAN),
        Just(f64::INFINITY),
    ]
}

fn f32_value() -> impl Strategy<Value = f32> {
    prop_oneof![(-10_000i32..10_000).prop_map(|v| v as f32 / 4.0), any::<u32>().prop_map(f32::from_bits)]
}

fn column(rows: usize) -> impl Strategy<Value = ArrayRef> {
    let n = rows;
    prop_oneof![
        prop::collection::vec(prop::option::weighted(0.9, any::<i32>()), n)
            .prop_map(|v| Arc::new(Int32Array::from(v)) as ArrayRef),
        prop::collection::vec(0i32..1000, n).prop_map(|v| Arc::new(Int32Array::from(v)) as ArrayRef),
        prop::collection::vec(prop::option::weighted(0.9, any::<i64>()), n)
            .prop_map(|v| Arc::new(Int64Array::from(v)) as ArrayRef),
        prop::collection::vec(prop::option::weighted(0.8, f32_value()), n)
            .prop_map(|v| Arc::new(Float32Array::from(v)) as ArrayRef),
        prop::collection::vec(prop::option::weighted(0.8, f64_value()), n)
            .prop_map(|v| Arc::new(Float64Array::from(v)) as ArrayRef),
        prop::collection::vec(1_700_000_000_000_000i64..1_700_000_100_000_000, n)
            .prop_map(|v| Arc::new(TimestampMicrosecondArray::from(v).with_timezone("UTC")) as ArrayRef),
        prop::collection::vec(-10_000_000i128..10_000_000, n)
            .prop_map(|v| Arc::new(Decimal128Array::from(v).with_precision_and_scale(18, 4).unwrap()) as ArrayRef),
        prop::collection::vec(prop::option::weighted(0.9, "[a-c]{0,3}"), n)
            .prop_map(|v| Arc::new(StringArray::from(v)) as ArrayRef),
        prop::collection::vec(
            prop::option::weighted(0.9, prop::collection::vec(prop::option::weighted(0.9, f64_value()), 0..4)),
            n
        )
        .prop_map(|v| {
            let mut b = ListBuilder::new(Float64Builder::new());
            for row in v {
                match row {
                    Some(items) => b.append_value(items),
                    None => b.append_null(),
                }
            }
            Arc::new(b.finish()) as ArrayRef
        }),
        (prop::collection::vec(any::<i64>(), n), prop::collection::vec(f64_value(), n)).prop_map(|(a, b)| {
            Arc::new(StructArray::from(vec![
                (Arc::new(Field::new("a", DataType::Int64, false)), Arc::new(Int64Array::from(a)) as ArrayRef),
                (Arc::new(Field::new("b", DataType::Float64, false)), Arc::new(Float64Array::from(b)) as ArrayRef),
            ])) as ArrayRef
        }),
    ]
}

fn batch() -> impl Strategy<Value = RecordBatch> {
    (0usize..300, 1usize..5).prop_flat_map(|(rows, cols)| prop::collection::vec(column(rows), cols)).prop_map(
        |columns| {
            let fields: Vec<Field> = columns
                .iter()
                .enumerate()
                .map(|(i, c)| Field::new(format!("c{i}"), c.data_type().clone(), c.null_count() > 0 || i % 2 == 0))
                .collect();
            RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
        },
    )
}

fn policy() -> impl Strategy<Value = Policy> {
    prop_oneof![Just(Policy::Auto), Just(Policy::Pcodec), Just(Policy::Base)]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn roundtrip_bit_exact(batch in batch(), policy in policy(), level in 0u32..=12, split in 0usize..300) {
        let options = WriterOptions::default().with_policy(policy).with_level(level).unwrap();
        let split = split.min(batch.num_rows());
        let parts = [batch.slice(0, split), batch.slice(split, batch.num_rows() - split)];
        let (file, _) = write(&parts, options);
        prop_assert_eq!(lines(&file), batch_lines(&[batch]));
    }
}

/// Batches with nested columns too: maps, lists of structs, nullable structs.
fn nested_column(rows: usize) -> impl Strategy<Value = ArrayRef> {
    use arrow_array::builder::{Int64Builder, MapBuilder, StringBuilder};
    prop_oneof![
        column(rows),
        prop::collection::vec(prop::option::weighted(0.8, prop::collection::vec((0u8..4, any::<i64>()), 0..3)), rows)
            .prop_map(|rows| {
                let mut m = MapBuilder::new(None, StringBuilder::new(), Int64Builder::new());
                for row in rows {
                    match row {
                        Some(entries) => {
                            for (k, v) in entries {
                                m.keys().append_value(format!("k{k}"));
                                m.values().append_option((k != 3).then_some(v));
                            }
                            m.append(true).unwrap();
                        }
                        None => m.append(false).unwrap(),
                    }
                }
                Arc::new(m.finish()) as ArrayRef
            }),
        (
            prop::collection::vec(prop::option::weighted(0.8, f64_value()), rows),
            prop::collection::vec(any::<bool>(), rows)
        )
            .prop_map(|(v, valid)| {
                let child = Arc::new(Float64Array::from(v)) as ArrayRef;
                let nulls = arrow_buffer::NullBuffer::from(valid);
                Arc::new(StructArray::new(
                    vec![Field::new("v", DataType::Float64, true)].into(),
                    vec![child],
                    Some(nulls),
                )) as ArrayRef
            }),
    ]
}

fn nested_batch() -> impl Strategy<Value = RecordBatch> {
    (0usize..200, 1usize..4).prop_flat_map(|(rows, cols)| prop::collection::vec(nested_column(rows), cols)).prop_map(
        |columns| {
            let fields: Vec<Field> = columns
                .iter()
                .enumerate()
                .map(|(i, c)| Field::new(format!("c{i}"), c.data_type().clone(), true))
                .collect();
            RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn nested_roundtrip_bit_exact(batch in nested_batch(), policy in policy(), v2 in any::<bool>()) {
        use tparquet::parquet::file::properties::{WriterProperties, WriterVersion};
        let version = if v2 { WriterVersion::PARQUET_2_0 } else { WriterVersion::PARQUET_1_0 };
        let props = WriterProperties::builder().set_writer_version(version).set_data_page_row_count_limit(50).set_write_batch_size(16).build();
        let options = WriterOptions::default().with_policy(policy).with_properties(props);
        let (file, _) = write(std::slice::from_ref(&batch), options);
        prop_assert_eq!(lines(&file), batch_lines(&[batch]));
    }

    #[test]
    fn transcode_round_trips_and_verify_agrees(batch in nested_batch(), policy in policy(), groups in 1usize..4) {
        use tparquet::{Target, transcode, verify::verify};
        use tparquet::parquet::arrow::ArrowWriter;
        // a standard file with `groups` row groups
        let mut input = Vec::new();
        let mut w = ArrowWriter::try_new(&mut input, batch.schema(), None).unwrap();
        let step = batch.num_rows().div_ceil(groups).max(1);
        for start in (0..batch.num_rows()).step_by(step) {
            w.write(&batch.slice(start, step.min(batch.num_rows() - start))).unwrap();
            w.flush().unwrap();
        }
        w.close().unwrap();
        let input = bytes::Bytes::from(input);
        let mut enc = Vec::new();
        transcode(input.clone(), &mut enc, &Target::Pcodec(WriterOptions::default().with_policy(policy))).unwrap();
        let enc = bytes::Bytes::from(enc);
        let mut dec = Vec::new();
        transcode(enc.clone(), &mut dec, &Target::standard()).unwrap();
        let dec = bytes::Bytes::from(dec);
        prop_assert_eq!(lines(&enc), batch_lines(std::slice::from_ref(&batch)));
        prop_assert_eq!(verify(input.clone(), enc.clone()).unwrap(), None);
        prop_assert_eq!(verify(enc.clone(), dec.clone()).unwrap(), None);
        let rgs = |f: &bytes::Bytes| {
            let b = tparquet::parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(f.clone()).unwrap();
            b.metadata().row_groups().iter().map(|rg| rg.num_rows()).collect::<Vec<_>>()
        };
        prop_assert_eq!(rgs(&enc), rgs(&input));
        prop_assert_eq!(rgs(&dec), rgs(&input));
        // transcoding PCODEC to PCODEC again changes nothing
        let mut again = Vec::new();
        transcode(enc.clone(), &mut again, &Target::Pcodec(WriterOptions::default().with_policy(policy))).unwrap();
        prop_assert_eq!(bytes::Bytes::from(again), enc);
    }

    #[test]
    fn verify_finds_any_changed_row(batch in batch(), row in any::<prop::sample::Index>()) {
        use tparquet::verify::{Difference, verify};
        prop_assume!(batch.num_rows() > 0);
        let row = row.index(batch.num_rows());
        // the same rows with row `row` replaced by the next one (or the previous one)
        let other = if row + 1 < batch.num_rows() { row + 1 } else { row.saturating_sub(1) };
        let indices = arrow_array::UInt64Array::from_iter_values((0..batch.num_rows() as u64).map(|i| if i == row as u64 { other as u64 } else { i }));
        let changed = arrow_select::take::take_record_batch(&batch, &indices).unwrap();
        let (a, _) = write(std::slice::from_ref(&batch), WriterOptions::default());
        let (b, _) = write(std::slice::from_ref(&changed), WriterOptions::default().with_policy(Policy::Pcodec));
        let same = batch_lines(&[batch.slice(row, 1)]) == batch_lines(&[batch.slice(other, 1)]);
        match verify(a, b).unwrap() {
            None => prop_assert!(same),
            Some(Difference::Row { row: r, .. }) => { prop_assert!(!same); prop_assert_eq!(r, row) }
            Some(d) => prop_assert!(false, "{d}"),
        }
    }
}
