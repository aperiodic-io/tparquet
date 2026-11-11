mod common;

use arrow_array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::Schema;
use bytes::Bytes;
use common::{codecs, lines, report};
use std::collections::HashMap;
use std::sync::Arc;
use tparquet::parquet::arrow::ArrowWriter;
use tparquet::parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use tparquet::parquet::basic::{Compression, GzipLevel};
use tparquet::parquet::file::metadata::{KeyValue, SortingColumn};
use tparquet::parquet::file::properties::WriterProperties;
use tparquet::verify::verify;
use tparquet::{Policy, Target, WriterOptions, transcode};

fn batch(offset: i64, rows: i64) -> RecordBatch {
    let ids = Int64Array::from_iter_values((offset..offset + rows).map(|i| i * 3));
    let px = Float64Array::from_iter((offset..offset + rows).map(|i| (i % 9 != 0).then_some(i as f64 / 4.0)));
    let names = StringArray::from_iter_values((offset..offset + rows).map(|i| format!("s{}", i % 7)));
    let b = RecordBatch::try_from_iter([
        ("id", Arc::new(ids) as ArrayRef),
        ("px", Arc::new(px) as ArrayRef),
        ("name", Arc::new(names) as ArrayRef),
    ])
    .unwrap();
    let metadata = HashMap::from([("origin".to_string(), "test".to_string())]);
    let schema = Arc::new(Schema::new_with_metadata(b.schema().fields().clone(), metadata));
    b.with_schema(schema).unwrap()
}

/// A standard file: SNAPPY, row groups of 1000, 1000 and 500 rows, a custom key-value pair.
fn standard() -> Bytes {
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![KeyValue::new("custom".to_string(), "kept".to_string())]))
        .build();
    let mut out = Vec::new();
    let mut w = ArrowWriter::try_new(&mut out, batch(0, 1).schema(), Some(props)).unwrap();
    for (offset, rows) in [(0, 1000), (1000, 1000), (2000, 500)] {
        w.write(&batch(offset, rows)).unwrap();
        w.flush().unwrap();
    }
    w.close().unwrap();
    Bytes::from(out)
}

fn run(input: Bytes, target: &Target) -> Bytes {
    let mut out = Vec::new();
    transcode(input, &mut out, target).unwrap();
    Bytes::from(out)
}

fn row_groups(file: &Bytes) -> Vec<i64> {
    let b = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
    b.metadata().row_groups().iter().map(|rg| rg.num_rows()).collect()
}

fn kv(file: &Bytes) -> Vec<(String, Option<String>)> {
    let b = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
    let kv = b.metadata().file_metadata().key_value_metadata().cloned().unwrap_or_default();
    kv.into_iter().filter(|kv| kv.key != "ARROW:schema").map(|kv| (kv.key, kv.value)).collect()
}

/// The Arrow schema as written in `ARROW:schema` (the reader merges the other key-value
/// pairs into its metadata)
fn arrow_schema(file: &Bytes) -> Schema {
    let b = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
    let file = b.metadata().file_metadata();
    let kv = file.key_value_metadata().cloned().unwrap_or_default();
    let arrow: Vec<KeyValue> = kv.into_iter().filter(|kv| kv.key == "ARROW:schema").collect();
    tparquet::parquet::arrow::parquet_to_arrow_schema(file.schema_descr(), Some(&arrow)).unwrap()
}

#[test]
fn standard_to_pcodec_and_back() {
    let input = standard();
    let pcodec = run(input.clone(), &Target::Pcodec(WriterOptions::default().with_policy(Policy::Pcodec)));
    assert_eq!(codecs(&pcodec).iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["PCODEC", "PCODEC", "ZSTD"]);
    assert_eq!(row_groups(&pcodec), [1000, 1000, 500]);
    assert_eq!(
        kv(&pcodec),
        [
            ("custom".to_string(), Some("kept".to_string())),
            ("parquet.pcodec.version".to_string(), Some("1".to_string()))
        ]
    );
    assert_eq!(arrow_schema(&pcodec), arrow_schema(&input));
    assert_eq!(arrow_schema(&pcodec).metadata()["origin"], "test");
    assert_eq!(lines(&pcodec), lines(&input));
    assert_eq!(verify(input.clone(), pcodec.clone()).unwrap(), None);

    let back = run(pcodec, &Target::standard());
    assert_eq!(codecs(&back).iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["ZSTD", "ZSTD", "ZSTD"]);
    assert_eq!(row_groups(&back), [1000, 1000, 500]);
    assert_eq!(kv(&back), [("custom".to_string(), Some("kept".to_string()))]);
    assert_eq!(arrow_schema(&back), arrow_schema(&input));
    assert_eq!(report(&back).pcodec_version, None);
    assert_eq!(verify(input, back).unwrap(), None);
}

#[test]
fn standard_keeps_the_codecs_of_other_columns() {
    let input = standard();
    let options = WriterOptions::default().with_policy(Policy::Base).with_column("id", Policy::Pcodec);
    let pcodec = run(input.clone(), &Target::Pcodec(options.with_base(Compression::SNAPPY)));
    assert_eq!(codecs(&pcodec).iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["PCODEC", "SNAPPY", "SNAPPY"]);
    let gzip = Compression::GZIP(GzipLevel::default());
    let back = run(pcodec, &Target::Standard { base: gzip });
    assert_eq!(codecs(&back).iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["GZIP", "SNAPPY", "SNAPPY"]);
    assert_eq!(verify(input, back).unwrap(), None);
}

#[test]
fn row_groups_survive_small_batches() {
    // the reader's batches (1024 rows by default) straddle the 1000-row row groups
    let input = standard();
    let out = run(input.clone(), &Target::Pcodec(WriterOptions::default()));
    assert_eq!(row_groups(&out), [1000, 1000, 500]);
    assert_eq!(lines(&out), lines(&input));
}

#[test]
fn files_without_rows_or_arrow_schema() {
    // no row groups at all
    let mut out = Vec::new();
    ArrowWriter::try_new(&mut out, batch(0, 1).schema(), None).unwrap().close().unwrap();
    let empty = run(Bytes::from(out), &Target::Pcodec(WriterOptions::default()));
    assert_eq!(row_groups(&empty), Vec::<i64>::new());
    assert_eq!(kv(&empty), []);
    assert_eq!(arrow_schema(&empty).fields().len(), 3);

    // no ARROW:schema: the schema comes from the Parquet schema
    let props = WriterProperties::builder().build();
    let mut out = Vec::new();
    let opts = tparquet::parquet::arrow::arrow_writer::ArrowWriterOptions::new()
        .with_properties(props)
        .with_skip_arrow_metadata(true);
    let mut w = ArrowWriter::try_new_with_options(&mut out, batch(0, 1).schema(), opts).unwrap();
    w.write(&batch(0, 10)).unwrap();
    w.close().unwrap();
    let input = Bytes::from(out);
    let pcodec = run(input.clone(), &Target::Pcodec(WriterOptions::default()));
    assert!(arrow_schema(&pcodec).metadata().is_empty());
    assert_eq!(lines(&pcodec), lines(&input));
}

#[test]
fn bad_input_is_an_error() {
    let mut out = Vec::new();
    let err = transcode(Bytes::from_static(b"not parquet"), &mut out, &Target::standard()).unwrap_err();
    assert!(err.to_string().starts_with("parquet: "), "{err}");
}

#[test]
fn row_groups_without_rows_are_dropped() {
    // testdata's `empty` file: one row group without rows
    let input = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/files/empty.parquet")).unwrap();
    let out = run(Bytes::from(input), &Target::standard());
    assert_eq!(row_groups(&out), Vec::<i64>::new());
    // a zero-row group between others
    let mut buf = Vec::new();
    let mut w = ArrowWriter::try_new(&mut buf, batch(0, 1).schema(), None).unwrap();
    w.write(&batch(0, 10)).unwrap();
    w.flush().unwrap();
    w.close().unwrap();
    let out = run(Bytes::from(buf), &Target::Pcodec(WriterOptions::default()));
    assert_eq!(row_groups(&out), [10]);
}

/// The Parquet schema of a file, as text.
fn parquet_schema(file: &Bytes) -> String {
    let b = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
    let mut s = Vec::new();
    tparquet::parquet::schema::printer::print_schema(&mut s, b.parquet_schema().root_schema());
    String::from_utf8(s).unwrap()
}

fn sorting(file: &Bytes) -> Vec<Option<Vec<SortingColumn>>> {
    let b = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
    b.metadata().row_groups().iter().map(|rg| rg.sorting_columns().cloned()).collect()
}

/// A standard file whose row groups say they are sorted by `id`, then `px` descending.
fn sorted() -> Bytes {
    let sort = vec![
        SortingColumn { column_idx: 0, descending: false, nulls_first: false },
        SortingColumn { column_idx: 1, descending: true, nulls_first: true },
    ];
    let props = WriterProperties::builder().set_sorting_columns(Some(sort)).build();
    let mut out = Vec::new();
    let mut w = ArrowWriter::try_new(&mut out, batch(0, 1).schema(), Some(props)).unwrap();
    for (offset, rows) in [(0, 700), (700, 300)] {
        w.write(&batch(offset, rows)).unwrap();
        w.flush().unwrap();
    }
    w.close().unwrap();
    Bytes::from(out)
}

#[test]
fn sorting_columns_are_kept() {
    let input = sorted();
    assert!(sorting(&input).iter().all(|s| s.as_ref().is_some_and(|s| s.len() == 2)));
    let pcodec = run(input.clone(), &Target::Pcodec(WriterOptions::default()));
    assert_eq!(sorting(&pcodec), sorting(&input));
    let back = run(pcodec, &Target::standard());
    assert_eq!(sorting(&back), sorting(&input));
    // row groups that disagree say nothing about the output's
    let mixed = with_row_group_sorting(&input, 1, None);
    let out = run(mixed.clone(), &Target::Pcodec(WriterOptions::default()));
    assert_eq!(sorting(&out), [None, None]);
    assert_eq!(lines(&out), lines(&input));
    let first_only = with_row_group_sorting(&input, 0, Some(vec![]));
    assert_eq!(sorting(&run(first_only, &Target::standard())), [None, None]);
    let unsorted = run(standard(), &Target::Pcodec(WriterOptions::default()));
    assert!(sorting(&unsorted).iter().all(|s| s.is_none()));
}

/// `file` with the sorting columns of row group `rg` replaced by `sort` in the footer.
fn with_row_group_sorting(file: &Bytes, rg: usize, sort: Option<Vec<SortingColumn>>) -> Bytes {
    use tparquet::parquet::file::metadata::{ParquetMetaDataReader, ParquetMetaDataWriter};
    let metadata = ParquetMetaDataReader::new().parse_and_finish(file).unwrap();
    let footer = 8 + u32::from_le_bytes(file[file.len() - 8..file.len() - 4].try_into().unwrap()) as usize;
    let mut groups = metadata.row_groups().to_vec();
    groups[rg] = groups[rg].clone().into_builder().set_sorting_columns(sort).build().unwrap();
    let metadata = metadata.into_builder().set_row_groups(groups).build();
    let mut out = file[..file.len() - footer].to_vec();
    ParquetMetaDataWriter::new(&mut out, &metadata).finish().unwrap();
    Bytes::from(out)
}

/// A file with field ids, logical types, nesting and nulls, in three row groups.
fn rich() -> Bytes {
    use arrow_array::builder::{Int64Builder, ListBuilder, MapBuilder, StringBuilder};
    use arrow_array::{Array, Decimal128Array, Int32Array, StructArray, TimestampMicrosecondArray};
    use arrow_schema::{DataType, Field};
    let id = |f: Field, id: i32| f.with_metadata(HashMap::from([("PARQUET:field_id".to_string(), id.to_string())]));
    let rows = 2500;
    let ts = TimestampMicrosecondArray::from_iter_values((0..rows).map(|i| 1_700_000_000_000_000 + i * 1000))
        .with_timezone("Europe/Paris");
    let dec = Decimal128Array::from_iter((0..rows).map(|i| (i % 5 != 0).then_some(i as i128 * 25)))
        .with_precision_and_scale(10, 2)
        .unwrap();
    let mut list = ListBuilder::new(Int64Builder::new());
    let mut map = MapBuilder::new(None, StringBuilder::new(), Int64Builder::new());
    for i in 0..rows {
        list.append_value((0..i % 3).map(|k| Some(i * 10 + k)));
        map.keys().append_value(format!("k{}", i % 2));
        map.values().append_value(i);
        map.append(i % 7 != 0).unwrap();
    }
    let st = StructArray::from(vec![(
        Arc::new(id(Field::new("n", DataType::Int32, false), 11)),
        Arc::new(Int32Array::from_iter_values((0..rows).map(|i| i as i32))) as ArrayRef,
    )]);
    let fields = vec![
        id(Field::new("ts", ts.data_type().clone(), false), 1),
        id(Field::new("dec", dec.data_type().clone(), true), 2),
        id(Field::new("list", list.finish().data_type().clone(), false), 3),
        id(Field::new("map", map.finish().data_type().clone(), true), 4),
        id(Field::new("st", st.data_type().clone(), false), 5),
    ];
    let mut list = ListBuilder::new(Int64Builder::new());
    let mut map = MapBuilder::new(None, StringBuilder::new(), Int64Builder::new());
    for i in 0..rows {
        list.append_value((0..i % 3).map(|k| Some(i * 10 + k)));
        map.keys().append_value(format!("k{}", i % 2));
        map.values().append_value(i);
        map.append(i % 7 != 0).unwrap();
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(ts), Arc::new(dec), Arc::new(list.finish()), Arc::new(map.finish()), Arc::new(st)],
    )
    .unwrap();
    let mut out = Vec::new();
    let mut w = ArrowWriter::try_new(&mut out, schema, None).unwrap();
    for (start, len) in [(0, 1000), (1000, 1), (1001, 1499)] {
        w.write(&batch.slice(start, len)).unwrap();
        w.flush().unwrap();
    }
    w.close().unwrap();
    Bytes::from(out)
}

#[test]
fn schema_field_ids_and_row_groups_are_kept_both_ways() {
    let input = rich();
    for target in [
        Target::Pcodec(WriterOptions::default()),
        Target::Pcodec(WriterOptions::default().with_policy(Policy::Pcodec)),
        Target::Pcodec(WriterOptions::default().with_policy(Policy::Base)),
    ] {
        let out = run(input.clone(), &target);
        assert_eq!(parquet_schema(&out), parquet_schema(&input));
        assert!(parquet_schema(&out).contains("(TIMESTAMP(MICROS,true))"), "{}", parquet_schema(&out));
        assert!(parquet_schema(&out).contains("[11]"), "field ids are kept");
        assert_eq!(arrow_schema(&out), arrow_schema(&input));
        assert_eq!(row_groups(&out), [1000, 1, 1499]);
        assert_eq!(verify(input.clone(), out.clone()).unwrap(), None);
        let back = run(out.clone(), &Target::standard());
        assert_eq!(parquet_schema(&back), parquet_schema(&input));
        assert_eq!(row_groups(&back), [1000, 1, 1499]);
        assert!(codecs(&back).iter().all(|c| c.1 != "PCODEC"));
        assert_eq!(verify(back, input.clone()).unwrap(), None);
    }
}

#[test]
fn transcoding_is_idempotent() {
    for input in [standard(), rich()] {
        for options in [WriterOptions::default(), WriterOptions::default().with_policy(Policy::Pcodec)] {
            let target = Target::Pcodec(options);
            let once = run(input.clone(), &target);
            let twice = run(once.clone(), &target);
            assert_eq!(twice, once);
            let back = run(once.clone(), &Target::standard());
            assert_eq!(run(back.clone(), &Target::standard()), back);
            // standard -> PCODEC -> standard -> PCODEC gives the same file again
            assert_eq!(run(back, &target), once);
        }
    }
}

#[test]
fn the_target_options_are_used() {
    // level, per-column policies and the base codec of Target::Pcodec reach the output
    let input = standard();
    let options = WriterOptions::default()
        .with_policy(Policy::Pcodec)
        .with_level(0)
        .unwrap()
        .with_column("px", Policy::Base)
        .with_base(Compression::SNAPPY);
    let out = run(input.clone(), &Target::Pcodec(options));
    assert_eq!(codecs(&out).iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["PCODEC", "SNAPPY", "SNAPPY"]);
    let level8 = run(input.clone(), &Target::Pcodec(WriterOptions::default().with_policy(Policy::Pcodec)));
    assert_ne!(report(&out).columns[0].compressed_bytes, report(&level8).columns[0].compressed_bytes);
    let err = transcode(input, Vec::new(), &Target::Pcodec(WriterOptions::default().with_column("nope", Policy::Base)));
    assert_eq!(err.unwrap_err().to_string(), "no column named \"nope\"");
}

/// What the footer records about how each column chunk was written, per row group and leaf:
/// (path, chunk statistics, distinct count, column index, offset index, bloom filter: its
/// header and bitset as written)
type Footer = Vec<Vec<(String, bool, bool, bool, bool, Option<Vec<u8>>)>>;

fn footer(file: &Bytes) -> Footer {
    use tparquet::parquet::bloom_filter::Sbbf;
    let b = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
    b.metadata()
        .row_groups()
        .iter()
        .map(|rg| {
            rg.columns()
                .iter()
                .map(|c| {
                    let bloom = Sbbf::read_from_column_chunk(c, file).unwrap().map(|f| {
                        let mut v = Vec::new();
                        f.write(&mut v).unwrap();
                        v
                    });
                    (
                        c.column_path().string(),
                        c.statistics().is_some(),
                        c.statistics().is_some_and(|s| s.distinct_count_opt().is_some()),
                        c.column_index_offset().is_some(),
                        c.offset_index_offset().is_some(),
                        bloom,
                    )
                })
                .collect()
        })
        .collect()
}

/// `batch(0, rows)` written with `props`, in one row group
fn written_with(props: WriterProperties, rows: i64) -> Bytes {
    let mut out = Vec::new();
    let mut w = ArrowWriter::try_new(&mut out, batch(0, 1).schema(), Some(props)).unwrap();
    w.write(&batch(0, rows)).unwrap();
    w.close().unwrap();
    Bytes::from(out)
}

#[test]
fn statistics_settings_and_bloom_filters_are_kept_both_ways() {
    use tparquet::parquet::file::properties::EnabledStatistics;
    use tparquet::parquet::schema::types::ColumnPath;
    let col = |s: &str| ColumnPath::from(s);
    let page_stats = WriterProperties::builder()
        .set_column_statistics_enabled(col("id"), EnabledStatistics::None)
        .set_column_statistics_enabled(col("px"), EnabledStatistics::Chunk)
        .set_column_bloom_filter_fpp(col("id"), 0.001)
        .set_column_bloom_filter_enabled(col("name"), true)
        .set_write_row_group_number_distinct_values(true)
        .build();
    let no_page_index = WriterProperties::builder()
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_column_statistics_enabled(col("name"), EnabledStatistics::None)
        .set_offset_index_disabled(true)
        .set_column_bloom_filter_fpp(col("px"), 0.2)
        // 64 bytes for 3000 values: a full filter
        .set_column_bloom_filter_max_ndv(col("px"), 64)
        .build();
    for props in [page_stats, no_page_index] {
        let input = written_with(props, 3000);
        let want = footer(&input);
        for options in [WriterOptions::default(), WriterOptions::default().with_policy(Policy::Pcodec)] {
            let out = run(input.clone(), &Target::Pcodec(options));
            assert!(codecs(&out).iter().any(|c| c.1 == "PCODEC"));
            assert_eq!(footer(&out), want);
            assert_eq!(footer(&run(out, &Target::standard())), want);
        }
    }
}

#[test]
fn bloom_filters_are_kept_in_every_row_group() {
    // row groups of different sizes: each gets a filter, at least as large as the input's
    // largest needed (the writer sizes them per column, not per row group)
    let props = WriterProperties::builder().set_bloom_filter_enabled(true).set_bloom_filter_fpp(0.01).build();
    let mut out = Vec::new();
    let mut w = ArrowWriter::try_new(&mut out, batch(0, 1).schema(), Some(props)).unwrap();
    for (offset, rows) in [(0, 5000), (5000, 10), (5010, 700)] {
        w.write(&batch(offset, rows)).unwrap();
        w.flush().unwrap();
    }
    w.close().unwrap();
    let input = Bytes::from(out);
    let bloom =
        |f: &Footer| f.iter().map(|rg| rg.iter().map(|c| c.5.is_some()).collect::<Vec<_>>()).collect::<Vec<_>>();
    let out = run(input.clone(), &Target::Pcodec(WriterOptions::default()));
    assert_eq!(bloom(&footer(&out)), vec![vec![true; 3]; 3]);
    assert_eq!(bloom(&footer(&run(out.clone(), &Target::standard()))), vec![vec![true; 3]; 3]);
    // every id of every row group passes its row group's filter
    let b = ParquetRecordBatchReaderBuilder::try_new(out.clone()).unwrap();
    for (rg, (offset, rows)) in b.metadata().row_groups().iter().zip([(0i64, 5000i64), (5000, 10), (5010, 700)]) {
        let f = tparquet::parquet::bloom_filter::Sbbf::read_from_column_chunk(rg.column(0), &out).unwrap().unwrap();
        assert!((offset..offset + rows).all(|i| f.check(&(i * 3))), "row group at {offset}");
    }
}
