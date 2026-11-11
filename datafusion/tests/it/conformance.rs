//! The conformance vectors of `testdata/` (`testdata/README.md`) through DataFusion: every
//! file read with SQL gives exactly its expected rows, and every payload vector decodes (or
//! fails) as listed, through the forked crate's codec.

use crate::common::inspect::inspect_file;
use crate::common::*;
use datafusion::arrow::array::{Array, AsArray};
use datafusion::prelude::{ParquetReadOptions, SessionConfig, SessionContext};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, Encoding};
use parquet::column::page::{CompressedPage, Page, PageWriteSpec, PageWriter};
use parquet::column::writer::{ColumnWriter, get_column_writer};
use parquet::data_type::FixedLenByteArray;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::file::writer::{SerializedFileWriter, SerializedPageWriter, TrackedWrite};
use parquet::schema::parser::parse_message_type;
use parquet::schema::types::SchemaDescriptor;
use std::collections::BTreeSet;
use std::sync::Arc;

#[test]
fn canonical_port_reads_every_vector() {
    // the arrow 59 port of rust/tparquet/src/canonical.rs, over the fork's own reader
    let names = vector_names();
    assert_eq!(names.len(), 24);
    for name in &names {
        let file = std::fs::File::open(vector_file(name)).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file).unwrap().build().unwrap();
        let batches: Vec<_> = reader.map(Result::unwrap).collect();
        assert!(canonical_lines(&batches) == expected_rows(name), "{name}");
    }
}

#[test]
fn inspect_port_matches_the_manifest() {
    // tests/common/inspect.rs reports what testdata/files/manifest.json lists
    let mut modes = BTreeSet::new();
    for f in manifest("files")["files"].as_array().unwrap() {
        let report = inspect_file(testdata().join(f["file"].as_str().unwrap()));
        assert_eq!(report.rows, f["rows"].as_i64().unwrap());
        assert_eq!(report.row_groups as u64, f["row_groups"].as_u64().unwrap());
        assert_eq!(report.pcodec_version.as_deref(), f["pcodec_version"].as_str());
        let columns = f["columns"].as_array().unwrap();
        assert_eq!(report.columns.len(), columns.len());
        for (c, want) in report.columns.iter().zip(columns) {
            let json = serde_json::json!({
                "path": c.path,
                "physical_type": c.physical_type,
                "codecs": c.codecs,
                "pages": c.pages,
                "modes": c.modes,
            });
            assert_eq!(&json, want, "{}", f["name"]);
            modes.extend(c.modes.keys().cloned());
        }
    }
    assert!(["RAW", "PCO", "DECIMAL", "ZSTD"].iter().all(|m| modes.contains(*m)), "{modes:?}");
}

/// Every vector through `query(name, path)`, run in `ctx`.
async fn every_vector(ctx: &SessionContext, setup: impl Fn(&str) -> Vec<String>) {
    for name in vector_names() {
        let mut statements = setup(&name);
        let query = statements.pop().unwrap();
        for s in &statements {
            sql(ctx, s).await;
        }
        let rows = canonical_sql(ctx, &query).await;
        assert!(rows == expected_rows(&name), "{name}: {query}");
    }
}

#[tokio::test]
async fn every_vector_through_stored_as_tparquet() {
    let ctx = ctx();
    every_vector(&ctx, |name| {
        vec![
            format!("CREATE EXTERNAL TABLE {name} STORED AS TPARQUET LOCATION {}", lit(&vector_file(name))),
            format!("SELECT * FROM {name}"),
        ]
    })
    .await;
}

#[tokio::test]
async fn every_vector_through_stored_as_parquet_without_registering() {
    // the plain Parquet format reads PCODEC because of the fork alone
    let ctx = SessionContext::new_with_config(SessionConfig::new().with_target_partitions(1));
    every_vector(&ctx, |name| {
        vec![
            format!("CREATE EXTERNAL TABLE {name} STORED AS PARQUET LOCATION {}", lit(&vector_file(name))),
            format!("SELECT * FROM {name}"),
        ]
    })
    .await;
}

#[tokio::test]
async fn every_vector_through_read_parquet_and_register_parquet() {
    let ctx = SessionContext::new_with_config(SessionConfig::new().with_target_partitions(1));
    for name in vector_names() {
        let path = vector_file(&name);
        let df = ctx.read_parquet(path.to_str().unwrap(), ParquetReadOptions::default()).await.unwrap();
        assert!(canonical_lines(&df.collect().await.unwrap()) == expected_rows(&name), "{name}");
        ctx.register_parquet(&name, path.to_str().unwrap(), ParquetReadOptions::default()).await.unwrap();
        assert!(canonical_sql(&ctx, &format!("SELECT * FROM {name}")).await == expected_rows(&name), "{name}");
    }
}

#[tokio::test]
async fn every_vector_through_the_session_extension() {
    use tparquet_datafusion::SessionContextExt;
    let ctx = ctx();
    for name in vector_names() {
        let path = vector_file(&name);
        let df = ctx.read_tparquet(path.to_str().unwrap(), ParquetReadOptions::default()).await.unwrap();
        assert!(canonical_lines(&df.collect().await.unwrap()) == expected_rows(&name), "{name}");
        ctx.register_tparquet(&name, path.to_str().unwrap(), ParquetReadOptions::default()).await.unwrap();
        assert!(canonical_sql(&ctx, &format!("SELECT * FROM {name}")).await == expected_rows(&name), "{name}");
    }
}

#[tokio::test]
async fn every_vector_as_a_url_table() {
    // SELECT * FROM 'file.parquet'
    let ctx = ctx().enable_url_table();
    every_vector(&ctx, |name| vec![format!("SELECT * FROM {}", lit(&vector_file(name)))]).await;
}

#[tokio::test]
async fn every_vector_with_several_partitions() {
    // with 4 target partitions the rows are the same, in some order
    let ctx = parallel_ctx();
    for name in vector_names() {
        sql(&ctx, &format!("CREATE EXTERNAL TABLE {name} STORED AS TPARQUET LOCATION {}", lit(&vector_file(&name))))
            .await;
        let sorted = |s: String| {
            let mut v: Vec<String> = s.lines().map(String::from).collect();
            v.sort();
            v
        };
        let rows = canonical_sql(&ctx, &format!("SELECT * FROM {name}")).await;
        assert_eq!(sorted(rows), sorted(expected_rows(&name)), "{name}");
    }
}

#[tokio::test]
async fn standard_control_file_reads_with_both_formats() {
    let ctx = ctx();
    let path = lit(&vector_file("standard_control"));
    sql(&ctx, &format!("CREATE EXTERNAL TABLE a STORED AS TPARQUET LOCATION {path}")).await;
    sql(&ctx, &format!("CREATE EXTERNAL TABLE b STORED AS PARQUET LOCATION {path}")).await;
    let expected = expected_rows("standard_control");
    assert_eq!(expected.lines().count(), 1500);
    assert!(canonical_sql(&ctx, "SELECT * FROM a").await == expected);
    assert!(canonical_sql(&ctx, "SELECT * FROM b").await == expected);
    assert_eq!(inspect_file(vector_file("standard_control")).pcodec_version, None);
}

/// A page writer that writes every data page with `payload` as its compressed bytes.
struct SwapPayload<'a> {
    inner: SerializedPageWriter<'a, Vec<u8>>,
    payload: Vec<u8>,
}

impl PageWriter for SwapPayload<'_> {
    fn write_page(&mut self, page: CompressedPage) -> parquet::errors::Result<PageWriteSpec> {
        let uncompressed = page.uncompressed_size();
        let page = match page.compressed_page() {
            Page::DataPage { num_values, encoding, def_level_encoding, rep_level_encoding, .. } => Page::DataPage {
                buf: self.payload.clone().into(),
                num_values: *num_values,
                encoding: *encoding,
                def_level_encoding: *def_level_encoding,
                rep_level_encoding: *rep_level_encoding,
                statistics: None,
            },
            other => panic!("unexpected page {:?}", other.page_type()),
        };
        self.inner.write_page(CompressedPage::new(page, uncompressed))
    }

    fn close(&mut self) -> parquet::errors::Result<()> {
        self.inner.close()
    }
}

/// A Parquet file with one `FIXED_LEN_BYTE_ARRAY(1)` column of `expected_len` rows in one
/// PCODEC `DATA_PAGE` whose payload is `payload`: the decoder is handed `payload` with the
/// expected size `expected_len`, and each decoded byte is one value.
fn payload_file(payload: &[u8], expected_len: usize) -> Vec<u8> {
    let schema = Arc::new(parse_message_type("message m { required fixed_len_byte_array(1) v; }").unwrap());
    let props = Arc::new(
        WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_1_0)
            .set_compression(Compression::PCODEC(Default::default()))
            .set_dictionary_enabled(false)
            .set_encoding(Encoding::PLAIN)
            .set_statistics_enabled(EnabledStatistics::None)
            .set_data_page_size_limit(usize::MAX)
            .set_data_page_row_count_limit(usize::MAX)
            .build(),
    );
    let descr = SchemaDescriptor::new(schema.clone());
    let mut chunk = TrackedWrite::new(Vec::new());
    let close = {
        let pages = SwapPayload { inner: SerializedPageWriter::new(&mut chunk), payload: payload.to_vec() };
        let ColumnWriter::FixedLenByteArrayColumnWriter(mut w) =
            get_column_writer(descr.column(0), props.clone(), Box::new(pages))
        else {
            unreachable!()
        };
        let values = vec![FixedLenByteArray::from(vec![0u8]); expected_len];
        w.write_batch(&values, None, None).unwrap();
        w.close().unwrap()
    };
    let chunk = bytes::Bytes::from(chunk.into_inner().unwrap());
    let mut out = vec![];
    let mut file = SerializedFileWriter::new(&mut out, schema, props).unwrap();
    let mut rg = file.next_row_group().unwrap();
    rg.append_column(&chunk, close).unwrap();
    rg.close().unwrap();
    file.close().unwrap();
    out
}

#[tokio::test]
async fn payload_vectors_decode_or_fail_through_datafusion() {
    let dir = testdata().join("payloads");
    let tmp = tempfile::tempdir().unwrap();
    let ctx = ctx();
    let (mut valid, mut invalid, mut empty) = (0, 0, 0);
    for v in manifest("payloads")["vectors"].as_array().unwrap() {
        let name = v["name"].as_str().unwrap();
        let payload = std::fs::read(dir.join(v["payload"].as_str().unwrap())).unwrap();
        let len = v["expected_len"].as_u64().unwrap() as usize;
        let path = tmp.path().join(format!("{name}.parquet"));
        std::fs::write(&path, payload_file(&payload, len)).unwrap();
        let query = format!("SELECT v FROM {}", lit(&path));
        let ctx = ctx.clone().enable_url_table();

        // the reference decoder agrees with the manifest
        let core = tparquet_core::decompress(&payload, len);
        if len == 0 {
            // Parquet never calls the codec for an empty values section (the fork's
            // decode_page skips it), so these read as no rows, valid or not
            assert_eq!(canonical_sql(&ctx, &query).await, "", "{name}");
            empty += 1;
        } else if v["valid"] == true {
            let decoded = std::fs::read(dir.join(v["decoded"].as_str().unwrap())).unwrap();
            let mut got = vec![];
            for b in sql(&ctx, &query).await {
                let col = b.column(0).as_fixed_size_binary();
                assert_eq!(col.null_count(), 0);
                got.extend_from_slice(col.value_data());
            }
            assert_eq!(got, decoded, "{name}");
            let mode = tparquet_core::mode_of(&payload).unwrap();
            assert_eq!(format!("{mode:?}").to_uppercase(), v["mode"].as_str().unwrap(), "{name}");
            valid += 1;
        } else {
            let err = sql_err(&ctx, &query).await;
            assert!(err.contains(v["error"].as_str().unwrap()), "{name}: {err}");
            invalid += 1;
        }
        match core {
            Ok(bytes) => assert_eq!(bytes.len(), len, "{name}"),
            Err(e) => assert_eq!(e.to_string(), v["error"].as_str().unwrap(), "{name}"),
        }
    }
    // raw_empty, empty_payload, truncated_header, pco_no_values, decimal_no_values,
    // zstd_empty_frame and zstd_empty_body have an expected size of 0
    assert_eq!((valid, invalid, empty), (14, 23, 7));
}
