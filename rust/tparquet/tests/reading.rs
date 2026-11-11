//! Reading PCODEC files every way the `parquet` crate offers: projection, row selection with
//! page indexes, row filters, offsets and limits, statistics, bloom filters, the push decoder
//! and the async reader, dictionary pages, V1 and V2 pages.

mod common;

use arrow_array::builder::{Float64Builder, ListBuilder};
use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, Int64Type};
use arrow_array::{ArrayRef, BooleanArray, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray};
use bytes::Bytes;
use common::{batch_lines, report};
use std::ops::Range;
use std::sync::Arc;
use tparquet::parquet::DecodeResult;
use tparquet::parquet::arrow::ArrowWriter;
use tparquet::parquet::arrow::ProjectionMask;
use tparquet::parquet::arrow::arrow_reader::statistics::StatisticsConverter;
use tparquet::parquet::arrow::arrow_reader::{
    ArrowPredicateFn, ArrowReaderOptions, ParquetRecordBatchReaderBuilder, RowFilter, RowSelection, RowSelector,
};
use tparquet::parquet::arrow::push_decoder::ParquetPushDecoderBuilder;
use tparquet::parquet::basic::{Compression, Encoding};
use tparquet::parquet::file::metadata::{PageIndexPolicy, ParquetMetaDataReader};
use tparquet::parquet::file::properties::{EnabledStatistics, ReaderProperties, WriterProperties, WriterVersion};
use tparquet::parquet::file::reader::{FileReader, SerializedFileReader};
use tparquet::parquet::file::serialized_reader::ReadOptionsBuilder;
use tparquet::parquet::record::RowAccessor;
use tparquet::{Policy, WriterOptions};

const ROWS: usize = 10_000;

/// A time series: sorted timestamps, decimal prices, sizes, a symbol, an optional double with
/// nulls, and a list of levels.
fn series(rows: usize) -> RecordBatch {
    let ts = Int64Array::from_iter_values((0..rows as i64).map(|i| 1_700_000_000_000 + i * 250));
    let px = Float64Array::from_iter_values((0..rows).map(|i| (650_000 + (i * 7919) % 400) as f64 / 100.0));
    let qty = Int32Array::from_iter_values((0..rows).map(|i| ((i * 31) % 97) as i32 + 1));
    let sym = StringArray::from_iter_values((0..rows).map(|i| ["AAPL", "MSFT", "GOOG"][i % 3]));
    let opt = Float64Array::from_iter((0..rows).map(|i| (i % 5 != 0).then_some(i as f64 * 0.5)));
    let mut levels = ListBuilder::new(Float64Builder::new());
    for i in 0..rows {
        if i % 11 == 0 {
            levels.append_null();
        } else {
            levels.append_value((0..i % 4).map(|k| (k % 3 != 2).then_some((i + k) as f64 / 4.0)));
        }
    }
    RecordBatch::try_from_iter([
        ("ts", Arc::new(ts) as ArrayRef),
        ("px", Arc::new(px) as ArrayRef),
        ("qty", Arc::new(qty) as ArrayRef),
        ("sym", Arc::new(sym) as ArrayRef),
        ("opt", Arc::new(opt) as ArrayRef),
        ("levels", Arc::new(levels.finish()) as ArrayRef),
    ])
    .unwrap()
}

/// Small pages and row groups, page statistics, bloom filters on `ts` and `sym`.
fn properties() -> WriterProperties {
    WriterProperties::builder()
        .set_max_row_group_row_count(Some(3000))
        .set_data_page_row_count_limit(400)
        .set_write_batch_size(100)
        .set_statistics_enabled(EnabledStatistics::Page)
        .set_column_bloom_filter_enabled("ts".into(), true)
        .set_column_bloom_filter_enabled("sym".into(), true)
        .build()
}

/// The series written with every numeric column PCODEC.
fn file() -> (Bytes, RecordBatch) {
    let batch = series(ROWS);
    let options = WriterOptions::default().with_policy(Policy::Pcodec).with_properties(properties());
    let (file, _) = common::write(std::slice::from_ref(&batch), options);
    let r = report(&file);
    assert_eq!(r.row_groups, 4);
    for c in r.columns.iter().filter(|c| c.path != "sym") {
        assert_eq!(c.codecs, ["PCODEC"], "{}", c.path);
        assert!(c.pages["DATA_PAGE_V2"] >= 4 * 6, "{c:?}");
    }
    (file, batch)
}

fn collect(reader: impl Iterator<Item = Result<RecordBatch, arrow_schema::ArrowError>>) -> Vec<RecordBatch> {
    reader.map(|b| b.unwrap()).collect()
}

/// The rows of `batch` at `ranges`.
fn rows_at(batch: &RecordBatch, ranges: &[Range<usize>]) -> Vec<RecordBatch> {
    ranges.iter().map(|r| batch.slice(r.start, r.len())).collect()
}

fn selection(ranges: &[Range<usize>], total: usize) -> RowSelection {
    let mut selectors = vec![];
    let mut at = 0;
    for r in ranges {
        selectors.push(RowSelector::skip(r.start - at));
        selectors.push(RowSelector::select(r.len()));
        at = r.end;
    }
    selectors.push(RowSelector::skip(total - at));
    RowSelection::from(selectors)
}

/// Row ranges within pages, across page and row group boundaries, single rows.
const RANGES: [Range<usize>; 5] = [0..1, 399..401, 2990..3010, 5000..5001, 9000..10_000];

#[test]
fn full_read_and_batch_sizes() {
    let (file, batch) = file();
    for batch_size in [1, 7, 1024, 20_000] {
        let reader = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap().with_batch_size(batch_size);
        assert_eq!(batch_lines(&collect(reader.build().unwrap())), batch_lines(std::slice::from_ref(&batch)));
    }
}

#[test]
fn projections() {
    let (file, batch) = file();
    let builder = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
    for roots in [vec![0], vec![1, 4], vec![5], vec![0, 2, 3, 5]] {
        let builder = ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap();
        let mask = ProjectionMask::roots(builder.parquet_schema(), roots.clone());
        let got = collect(builder.with_projection(mask).build().unwrap());
        assert_eq!(batch_lines(&got), batch_lines(&[batch.project(&roots).unwrap()]), "{roots:?}");
    }
    // the list's leaf alone
    let mask = ProjectionMask::leaves(builder.parquet_schema(), [5]);
    let got = collect(builder.with_projection(mask).build().unwrap());
    assert_eq!(batch_lines(&got), batch_lines(&[batch.project(&[5]).unwrap()]));
}

#[test]
fn row_selection_with_and_without_page_index() {
    let (file, batch) = file();
    let want = batch_lines(&rows_at(&batch, &RANGES));
    for policy in [PageIndexPolicy::Skip, PageIndexPolicy::Optional, PageIndexPolicy::Required] {
        let options = ArrowReaderOptions::new().with_page_index_policy(policy);
        let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(file.clone(), options).unwrap();
        if policy != PageIndexPolicy::Skip {
            // PCODEC columns have both page indexes
            let index = builder.metadata().page_index().expect("page indexes");
            assert!(index.is_complete());
            for rg in 0..4 {
                for col in 0..6 {
                    assert!(index.column_index(rg, col).is_some(), "{rg} {col}");
                    assert!(index.num_data_pages(rg, col).unwrap() >= 2, "{rg} {col}");
                }
            }
        }
        let got = collect(builder.with_row_selection(selection(&RANGES, ROWS)).build().unwrap());
        assert_eq!(batch_lines(&got), want, "{policy:?}");
    }
}

#[test]
fn row_filters_offsets_and_limits() {
    let (file, batch) = file();
    // ts in a window, px above a threshold: both PCODEC columns
    let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(
        file.clone(),
        ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Required),
    )
    .unwrap();
    let schema = builder.parquet_schema();
    let window = ArrowPredicateFn::new(ProjectionMask::roots(schema, [0]), |b: RecordBatch| {
        let ts = b.column(0).as_primitive::<Int64Type>();
        Ok(BooleanArray::from_iter(ts.iter().map(|t| t.map(|t| (1_700_000_500_000..1_700_001_500_000).contains(&t)))))
    });
    let expensive = ArrowPredicateFn::new(ProjectionMask::roots(schema, [1]), |b: RecordBatch| {
        let px = b.column(0).as_primitive::<Float64Type>();
        Ok(BooleanArray::from_iter(px.iter().map(|p| p.map(|p| p > 6501.5))))
    });
    let filter = RowFilter::new(vec![Box::new(window), Box::new(expensive)]);
    let got = collect(builder.with_row_filter(filter).build().unwrap());
    let rows: Vec<usize> = (0..ROWS)
        .filter(|&i| (2000..6000).contains(&i) && (650_000 + (i * 7919) % 400) as f64 / 100.0 > 6501.5)
        .collect();
    assert!(!rows.is_empty() && rows.len() < 4000);
    let want: Vec<String> = rows.iter().map(|&i| batch_lines(&[batch.slice(i, 1)]).remove(0)).collect();
    assert_eq!(batch_lines(&got), want);

    let builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
    let got = collect(builder.with_offset(2995).with_limit(17).build().unwrap());
    assert_eq!(batch_lines(&got), batch_lines(&[batch.slice(2995, 17)]));
}

#[test]
fn statistics_and_page_indexes_of_pcodec_columns() {
    let (file, batch) = file();
    let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(
        file,
        ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Required),
    )
    .unwrap();
    let m = builder.metadata();
    let ts = batch.column(0).as_primitive::<Int64Type>();
    let conv = StatisticsConverter::try_new("ts", builder.schema(), builder.parquet_schema()).unwrap();
    let mins = conv.row_group_mins(m.row_groups().iter()).unwrap();
    let maxes = conv.row_group_maxes(m.row_groups().iter()).unwrap();
    let starts = [0, 3000, 6000, 9000];
    let ends = [2999, 5999, 8999, 9999];
    assert_eq!(mins.as_primitive::<Int64Type>().values(), &starts.map(|i| ts.value(i)));
    assert_eq!(maxes.as_primitive::<Int64Type>().values(), &ends.map(|i| ts.value(i)));
    let nulls = StatisticsConverter::try_new("opt", builder.schema(), builder.parquet_schema()).unwrap();
    let counts = nulls.row_group_null_counts(m.row_groups().iter()).unwrap();
    assert_eq!(counts.values(), &[600, 600, 600, 200]);

    // page statistics: every page of `ts` is a sorted run of the series
    let pages = conv.data_page_mins(m.page_index().unwrap().as_ref(), &[0usize, 1, 2, 3]).unwrap();
    let pages = pages.as_primitive::<Int64Type>();
    assert!(pages.len() >= 4 * 6);
    assert!(pages.values().windows(2).all(|w| w[0] < w[1]));
    assert_eq!(pages.value(0), ts.value(0));

    // px: the row group maximum is the true maximum
    let conv = StatisticsConverter::try_new("px", builder.schema(), builder.parquet_schema()).unwrap();
    let maxes = conv.row_group_maxes(m.row_groups().iter()).unwrap();
    let px = batch.column(1).as_primitive::<Float64Type>();
    let max = px.values()[..3000].iter().cloned().fold(f64::MIN, f64::max);
    assert_eq!(maxes.as_primitive::<Float64Type>().value(0), max);
}

#[test]
fn bloom_filters_on_pcodec_columns() {
    let (file, batch) = file();
    let reader = SerializedFileReader::new_with_options(
        file,
        ReadOptionsBuilder::new()
            .with_reader_properties(ReaderProperties::builder().set_read_bloom_filter(true).build())
            .build(),
    )
    .unwrap();
    let ts = batch.column(0).as_primitive::<Int64Type>();
    for rg in 0..4 {
        let rg_reader = reader.get_row_group(rg).unwrap();
        let bloom = rg_reader.get_column_bloom_filter(0).expect("a bloom filter on ts");
        assert!(bloom.check(&ts.value(rg * 3000)));
        assert!(!bloom.check(&-1i64));
        assert!(rg_reader.get_column_bloom_filter(1).is_none());
        let sym = rg_reader.get_column_bloom_filter(3).expect("a bloom filter on sym");
        assert!(sym.check(&"MSFT"));
        assert!(!sym.check(&"ORCL"));
    }
}

#[test]
fn row_iterator_and_column_readers() {
    let (file, batch) = file();
    let reader = SerializedFileReader::new(file).unwrap();
    let rows: Vec<_> = reader.get_row_iter(None).unwrap().map(|r| r.unwrap()).collect();
    assert_eq!(rows.len(), ROWS);
    let ts = batch.column(0).as_primitive::<Int64Type>();
    assert_eq!(rows[4321].get_long(0).unwrap(), ts.value(4321));
    assert_eq!(rows[4321].get_double(1).unwrap(), batch.column(1).as_primitive::<Float64Type>().value(4321));
}

fn push_decode(file: &Bytes, selection: Option<RowSelection>) -> Vec<RecordBatch> {
    let metadata =
        ParquetMetaDataReader::new().with_page_index_policy(PageIndexPolicy::Optional).parse_and_finish(file).unwrap();
    let mut builder = ParquetPushDecoderBuilder::try_new_decoder(Arc::new(metadata)).unwrap().with_batch_size(999);
    if let Some(s) = selection {
        builder = builder.with_row_selection(s);
    }
    let mut decoder = builder.build().unwrap();
    let mut batches = vec![];
    loop {
        match decoder.try_decode().unwrap() {
            DecodeResult::NeedsData(ranges) => {
                let data = ranges.iter().map(|r| file.slice(r.start as usize..r.end as usize)).collect();
                decoder.push_ranges(ranges, data).unwrap();
            }
            DecodeResult::Data(b) => batches.push(b),
            DecodeResult::Finished => return batches,
        }
    }
}

#[test]
fn push_decoder() {
    let (file, batch) = file();
    assert_eq!(batch_lines(&push_decode(&file, None)), batch_lines(std::slice::from_ref(&batch)));
    let got = push_decode(&file, Some(selection(&RANGES, ROWS)));
    assert_eq!(batch_lines(&got), batch_lines(&rows_at(&batch, &RANGES)));
}

#[test]
fn async_reader() {
    use futures::TryStreamExt;
    use tparquet::parquet::arrow::ParquetRecordBatchStreamBuilder;
    let (file, batch) = file();
    let read = |selection: Option<RowSelection>, roots: Option<Vec<usize>>| {
        let file = std::io::Cursor::new(file.to_vec());
        futures::executor::block_on(async move {
            let options = ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Optional);
            let mut builder = ParquetRecordBatchStreamBuilder::new_with_options(file, options).await.unwrap();
            if let Some(s) = selection {
                builder = builder.with_row_selection(s);
            }
            if let Some(roots) = roots {
                let mask = ProjectionMask::roots(builder.parquet_schema(), roots);
                builder = builder.with_projection(mask);
            }
            builder.with_batch_size(500).build().unwrap().try_collect::<Vec<_>>().await.unwrap()
        })
    };
    assert_eq!(batch_lines(&read(None, None)), batch_lines(std::slice::from_ref(&batch)));
    assert_eq!(batch_lines(&read(Some(selection(&RANGES, ROWS)), None)), batch_lines(&rows_at(&batch, &RANGES)));
    let projected = batch.project(&[1, 5]).unwrap();
    assert_eq!(batch_lines(&read(None, Some(vec![1, 5]))), batch_lines(&[projected]));
}

#[tokio::test]
async fn async_reader_on_a_tokio_runtime() {
    use futures::TryStreamExt;
    use tparquet::parquet::arrow::ParquetRecordBatchStreamBuilder;
    let (file, batch) = file();
    let builder = ParquetRecordBatchStreamBuilder::new(std::io::Cursor::new(file.to_vec())).await.unwrap();
    let got: Vec<RecordBatch> = builder.build().unwrap().try_collect().await.unwrap();
    assert_eq!(batch_lines(&got), batch_lines(&[batch]));
}

/// `batch` written by `ArrowWriter` with PCODEC everywhere and `props`.
fn arrow_write(batch: &RecordBatch, props: tparquet::parquet::file::properties::WriterPropertiesBuilder) -> Bytes {
    let props = props.set_compression(Compression::PCODEC(Default::default())).build();
    let mut out = Vec::new();
    let mut w = ArrowWriter::try_new(&mut out, batch.schema(), Some(props)).unwrap();
    w.write(batch).unwrap();
    w.close().unwrap();
    Bytes::from(out)
}

#[test]
fn every_page_kind_on_every_read_path() {
    // V1 and V2 pages, with and without dictionaries, PLAIN and the other encodings, nested
    // columns with levels: PCODEC gets the expected size for each
    let batch = series(5000);
    let base = || {
        WriterProperties::builder()
            .set_data_page_row_count_limit(300)
            .set_write_batch_size(100)
            .set_max_row_group_row_count(Some(2000))
    };
    let variants = [
        base().set_writer_version(WriterVersion::PARQUET_1_0),
        base().set_writer_version(WriterVersion::PARQUET_2_0),
        base().set_writer_version(WriterVersion::PARQUET_1_0).set_dictionary_enabled(false),
        base().set_writer_version(WriterVersion::PARQUET_2_0).set_dictionary_enabled(false),
        base()
            .set_writer_version(WriterVersion::PARQUET_2_0)
            .set_dictionary_enabled(false)
            .set_encoding(Encoding::PLAIN),
        base()
            .set_dictionary_enabled(false)
            .set_encoding(Encoding::BYTE_STREAM_SPLIT)
            .set_column_encoding("sym".into(), Encoding::PLAIN)
            .set_column_encoding("levels.list.item".into(), Encoding::PLAIN),
        // V2 pages with values stored uncompressed whenever pcodec does not halve them
        base().set_writer_version(WriterVersion::PARQUET_2_0).set_data_page_v2_compression_ratio_threshold(0.5),
        // the dictionary falls back to plain part way through a chunk
        base().set_dictionary_page_size_limit(64),
    ];
    for (i, props) in variants.into_iter().enumerate() {
        let file = arrow_write(&batch, props);
        let want = batch_lines(std::slice::from_ref(&batch));
        let sync = collect(ParquetRecordBatchReaderBuilder::try_new(file.clone()).unwrap().build().unwrap());
        assert_eq!(batch_lines(&sync), want, "variant {i}");
        assert_eq!(batch_lines(&push_decode(&file, None)), want, "variant {i}");
        let ranges = [0..1, 299..301, 1999..2001, 4999..5000];
        let sel = collect(
            ParquetRecordBatchReaderBuilder::try_new_with_options(
                file.clone(),
                ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Required),
            )
            .unwrap()
            .with_row_selection(selection(&ranges, 5000))
            .build()
            .unwrap(),
        );
        assert_eq!(batch_lines(&sel), batch_lines(&rows_at(&batch, &ranges)), "variant {i}");
        let r = report(&file);
        assert!(r.columns.iter().all(|c| c.codecs == ["PCODEC"]));
        assert_eq!(r.pcodec_version.as_deref(), Some("1"));
    }
}

#[test]
fn byte_array_dictionaries_with_pcodec() {
    // the metadata reader's dictionary API decodes PCODEC dictionary pages too
    let batch = series(3000);
    let file = arrow_write(&batch, WriterProperties::builder());
    let metadata = ParquetMetaDataReader::new().parse_and_finish(&file).unwrap();
    let dict = ParquetMetaDataReader::read_column_dictionary(&file, &metadata, 0, 3).unwrap().expect("a dictionary");
    let mut values: Vec<&[u8]> = dict.as_binary::<i32>().iter().map(|v| v.unwrap()).collect();
    values.sort();
    assert_eq!(values, [&b"AAPL"[..], b"GOOG", b"MSFT"]);
    assert!(report(&file).columns[3].pages.contains_key("DICTIONARY_PAGE"));
}

#[test]
fn row_group_and_page_size_limits() {
    let batch = series(20_000);
    for (props, groups) in [
        (WriterProperties::builder().set_max_row_group_row_count(Some(1)).build(), 20_000),
        (WriterProperties::builder().set_max_row_group_row_count(Some(7777)).build(), 3),
        (
            WriterProperties::builder()
                .set_max_row_group_row_count(None)
                .set_max_row_group_bytes(Some(64 * 1024))
                .build(),
            0,
        ),
        (WriterProperties::builder().set_data_page_size_limit(64).set_write_batch_size(10).build(), 1),
    ] {
        for policy in [Policy::Auto, Policy::Pcodec, Policy::Base] {
            let options = WriterOptions::default().with_policy(policy).with_properties(props.clone());
            let small = if groups == 20_000 { batch.slice(0, 50) } else { batch.clone() };
            // the byte limit splits between batches only
            let parts: Vec<RecordBatch> =
                (0..small.num_rows()).step_by(1000).map(|i| small.slice(i, 1000.min(small.num_rows() - i))).collect();
            let (file, _) = common::write(&parts, options);
            let r = report(&file);
            match groups {
                20_000 => assert_eq!(r.row_groups, 50),
                0 => assert!(r.row_groups > 1, "{policy:?}: {}", r.row_groups),
                g => assert_eq!(r.row_groups, g),
            }
            if groups == 1 {
                assert!(r.columns[0].pages.values().sum::<u64>() > 100, "{:?}", r.columns[0].pages);
            }
            assert_eq!(common::lines(&file), batch_lines(&[small]));
        }
    }
}
