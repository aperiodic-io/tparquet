//! Nested data: lists, large lists, fixed-size lists, maps and structs, nested in each other
//! with nulls at every level, and per-column policies on their leaves.

mod common;

use arrow_array::builder::{
    FixedSizeListBuilder, Float32Builder, Float64Builder, Int32Builder, Int64Builder, LargeListBuilder, ListBuilder,
    MapBuilder, StringBuilder, StructBuilder,
};
use arrow_array::{Array, ArrayRef, Int64Array, RecordBatch, StructArray};
use arrow_buffer::NullBuffer;
use arrow_schema::{DataType, Field, Fields};
use common::{Rng, batch_lines, codecs, lines, report, write};
use std::sync::Arc;
use tparquet::{Policy, WriterOptions};

/// Rows with nulls at every level: a null row, an empty list, null elements, null struct
/// fields, null map values.
fn nested(rows: usize, seed: u64) -> RecordBatch {
    let mut rng = Rng(seed);
    let mut chance = move |n: u64| rng.below(n) == 0;

    // list<list<double>>
    let mut ll = ListBuilder::new(ListBuilder::new(Float64Builder::new()));
    // large_list<int32>
    let mut large = LargeListBuilder::new(Int32Builder::new());
    // fixed_size_list<float, 3>
    let mut fixed = FixedSizeListBuilder::new(Float32Builder::new(), 3);
    // map<string, int64>
    let mut map = MapBuilder::new(None, StringBuilder::new(), Int64Builder::new());
    for i in 0..rows {
        if chance(9) {
            ll.append_null();
        } else {
            for j in 0..i % 3 {
                if chance(7) {
                    ll.values().append_null();
                } else {
                    for k in 0..(i + j) % 4 {
                        let v = (k % 5 != 4).then_some((i * 10 + j + k) as f64 / 100.0);
                        ll.values().values().append_option(v);
                    }
                    ll.values().append(true);
                }
            }
            ll.append(true);
        }
        if chance(8) {
            large.append_null();
        } else {
            large.append_value((0..i % 5).map(|k| (k != 2).then_some((i * 3 + k) as i32)));
        }
        for k in 0..3 {
            fixed.values().append_option((k != 1 || i % 2 == 0).then_some(i as f32 + k as f32 / 4.0));
        }
        fixed.append(!chance(10));
        if chance(6) {
            map.append(false).unwrap();
        } else {
            for k in 0..i % 3 {
                map.keys().append_value(format!("k{k}"));
                map.values().append_option((k != 1).then_some((i * 1000 + k) as i64));
            }
            map.append(true).unwrap();
        }
    }

    // struct<a: int64, inner: struct<b: double, c: string>> with nulls at both levels
    let inner_fields =
        Fields::from(vec![Field::new("b", DataType::Float64, true), Field::new("c", DataType::Utf8, true)]);
    let mut s = StructBuilder::new(
        vec![Field::new("a", DataType::Int64, true), Field::new("inner", DataType::Struct(inner_fields.clone()), true)],
        vec![
            Box::new(Int64Builder::new()),
            Box::new(StructBuilder::new(
                inner_fields,
                vec![Box::new(Float64Builder::new()), Box::new(StringBuilder::new())],
            )),
        ],
    );
    for i in 0..rows {
        let valid = i % 11 != 5;
        s.field_builder::<Int64Builder>(0).unwrap().append_option((i % 4 != 0).then_some(1_700_000_000 + i as i64));
        let inner = s.field_builder::<StructBuilder>(1).unwrap();
        inner.field_builder::<Float64Builder>(0).unwrap().append_option((i % 3 != 0).then_some(i as f64 * 0.01));
        inner.field_builder::<StringBuilder>(1).unwrap().append_option((i % 2 == 0).then(|| format!("c{}", i % 7)));
        inner.append(i % 13 != 7);
        s.append(valid);
    }

    // list<struct<p: double, q: int64>> (an order book)
    let level = Fields::from(vec![Field::new("p", DataType::Float64, false), Field::new("q", DataType::Int64, true)]);
    let mut book = ListBuilder::new(StructBuilder::from_fields(level, 0));
    for i in 0..rows {
        for k in 0..i % 6 {
            let st = book.values();
            st.field_builder::<Float64Builder>(0).unwrap().append_value((650_000 + i + k) as f64 / 100.0);
            st.field_builder::<Int64Builder>(1).unwrap().append_option((k != 3).then_some((k * 100) as i64));
            st.append(true);
        }
        book.append(i % 17 != 3);
    }

    RecordBatch::try_from_iter([
        ("ll", Arc::new(ll.finish()) as ArrayRef),
        ("large", Arc::new(large.finish()) as ArrayRef),
        ("fixed", Arc::new(fixed.finish()) as ArrayRef),
        ("map", Arc::new(map.finish()) as ArrayRef),
        ("s", Arc::new(s.finish()) as ArrayRef),
        ("book", Arc::new(book.finish()) as ArrayRef),
    ])
    .unwrap()
}

const LEAVES: [&str; 9] = [
    "ll.list.item.list.item",
    "large.list.item",
    "fixed.list.item",
    "map.entries.key",
    "map.entries.value",
    "s.a",
    "s.inner.b",
    "s.inner.c",
    "book.list.item.p",
];

#[test]
fn nested_round_trips_under_every_policy() {
    let batches = [nested(1500, 1), nested(1, 2), nested(0, 3), nested(700, 4)];
    for policy in [Policy::Auto, Policy::Pcodec, Policy::Base] {
        let (file, _) = write(&batches, WriterOptions::default().with_policy(policy));
        assert_eq!(lines(&file), batch_lines(&batches), "{policy:?}");
        let got = codecs(&file);
        let paths: Vec<&str> = got.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(&paths[..9], LEAVES);
        assert_eq!(paths[9], "book.list.item.q");
        if policy == Policy::Pcodec {
            for (path, codec) in &got {
                let numeric = !path.ends_with(".key") && path != "s.inner.c";
                assert_eq!(codec == "PCODEC", numeric, "{path}");
            }
            // V2 pages carry the levels uncompressed next to PCODEC values
            assert!(report(&file).columns.iter().all(|c| c.pages.contains_key("DATA_PAGE_V2")));
        }
    }
}

#[test]
fn policies_on_nested_paths() {
    let batch = nested(2000, 5);
    let pcodec = |path: &str, of: &[(String, String)]| of.iter().find(|(p, _)| p == path).unwrap().1 == "PCODEC";
    // a whole subtree, one leaf in it, a map's values, the list item of a list of structs
    let options = WriterOptions::default()
        .with_policy(Policy::Base)
        .with_column("s", Policy::Pcodec)
        .with_column("s.inner.b", Policy::Base)
        .with_column("map.entries.value", Policy::Pcodec)
        .with_column("book.list.item.q", Policy::Pcodec)
        .with_column("ll", Policy::Pcodec);
    let (file, _) = write(std::slice::from_ref(&batch), options);
    let got = codecs(&file);
    assert!(pcodec("s.a", &got));
    assert!(!pcodec("s.inner.b", &got));
    assert!(pcodec("map.entries.value", &got));
    assert!(pcodec("book.list.item.q", &got));
    assert!(!pcodec("book.list.item.p", &got));
    assert!(pcodec("ll.list.item.list.item", &got));
    assert!(!pcodec("large.list.item", &got));
    assert_eq!(lines(&file), batch_lines(&[batch]));
}

#[test]
fn all_null_and_empty_nested_columns() {
    let batch = nested(500, 6);
    let nulls: Vec<ArrayRef> =
        batch.columns().iter().map(|c| arrow_array::new_null_array(c.data_type(), c.len())).collect();
    let all_null = RecordBatch::try_new(batch.schema(), nulls).unwrap();
    // every row an empty list or map: levels but no values
    let mut empty_lists = ListBuilder::new(ListBuilder::new(Float64Builder::new()));
    for _ in 0..500 {
        empty_lists.append(true);
    }
    let empty: RecordBatch = RecordBatch::try_from_iter([("ll", Arc::new(empty_lists.finish()) as ArrayRef)]).unwrap();
    for policy in [Policy::Auto, Policy::Pcodec] {
        let (file, _) = write(std::slice::from_ref(&all_null), WriterOptions::default().with_policy(policy));
        assert_eq!(lines(&file), batch_lines(std::slice::from_ref(&all_null)));
        let (file, _) = write(std::slice::from_ref(&empty), WriterOptions::default().with_policy(policy));
        assert_eq!(lines(&file), batch_lines(std::slice::from_ref(&empty)));
    }
}

#[test]
fn null_structs_hide_their_children() {
    // a struct whose null rows have non-null child values underneath (as Arrow allows):
    // they read back as null structs
    let a = Int64Array::from_iter_values(0..1000);
    let nulls = NullBuffer::from_iter((0..1000).map(|i| i % 3 != 0));
    let s =
        StructArray::new(Fields::from(vec![Field::new("a", DataType::Int64, false)]), vec![Arc::new(a)], Some(nulls));
    let batch = RecordBatch::try_from_iter([("s", Arc::new(s) as ArrayRef)]).unwrap();
    for policy in [Policy::Auto, Policy::Pcodec] {
        let (file, _) = write(std::slice::from_ref(&batch), WriterOptions::default().with_policy(policy));
        let got = lines(&file);
        assert_eq!(got, batch_lines(std::slice::from_ref(&batch)));
        assert_eq!(got[0], "{\"s\":null}");
        assert_eq!(got[1], "{\"s\":{\"a\":1}}");
        assert_eq!(batch.column(0).null_count(), 334);
    }
}

#[test]
fn deep_nesting() {
    // list<struct<x: list<list<int64>>>>: 4 repetition levels, 7 definition levels
    let mut b = ListBuilder::new(StructBuilder::from_fields(
        vec![Field::new("x", DataType::new_list(DataType::new_list(DataType::Int64, true), true), true)],
        0,
    ));
    for i in 0..3000i64 {
        if i % 10 == 9 {
            b.append_null();
            continue;
        }
        for j in 0..i % 3 {
            let st = b.values();
            let x = st.field_builder::<ListBuilder<Box<dyn arrow_array::builder::ArrayBuilder>>>(0).unwrap();
            for k in 0..(i + j) % 3 {
                let inner = x
                    .values()
                    .as_any_mut()
                    .downcast_mut::<ListBuilder<Box<dyn arrow_array::builder::ArrayBuilder>>>()
                    .unwrap();
                let ints = inner.values().as_any_mut().downcast_mut::<Int64Builder>().unwrap();
                for m in 0..k {
                    ints.append_option((m != 1).then_some(i * 100 + j * 10 + m));
                }
                inner.append(k != 2 || i % 2 == 0);
            }
            x.append(j != 1 || i % 4 != 0);
            st.append(true);
        }
        b.append(true);
    }
    let batch = RecordBatch::try_from_iter([("deep", Arc::new(b.finish()) as ArrayRef)]).unwrap();
    for policy in [Policy::Auto, Policy::Pcodec, Policy::Base] {
        let (file, _) = write(std::slice::from_ref(&batch), WriterOptions::default().with_policy(policy));
        assert_eq!(lines(&file), batch_lines(std::slice::from_ref(&batch)), "{policy:?}");
        if policy == Policy::Pcodec {
            assert_eq!(codecs(&file), [("deep.list.item.x.list.item.list.item".to_string(), "PCODEC".to_string())]);
        }
    }
}
