//! Parquet modular encryption (the `parquet` crate's `encryption` feature) with PCODEC
//! columns: pages are compressed, then encrypted; decrypted, then decompressed.

mod common;

use arrow_array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use bytes::Bytes;
use common::batch_lines;
use std::sync::Arc;
use tparquet::parquet::arrow::arrow_reader::{
    ArrowReaderOptions, ParquetRecordBatchReaderBuilder, RowSelection, RowSelector,
};
use tparquet::parquet::encryption::decrypt::FileDecryptionProperties;
use tparquet::parquet::encryption::encrypt::FileEncryptionProperties;
use tparquet::parquet::file::metadata::PageIndexPolicy;
use tparquet::parquet::file::properties::WriterProperties;
use tparquet::{Policy, WriterOptions};

const FOOTER_KEY: &[u8; 16] = b"0123456789012345";
const TS_KEY: &[u8; 16] = b"1234567890123450";
const PX_KEY: &[u8; 16] = b"1234567890123451";

fn batch(rows: usize) -> RecordBatch {
    RecordBatch::try_from_iter([
        (
            "ts",
            Arc::new(Int64Array::from_iter_values((0..rows as i64).map(|i| 1_700_000_000_000 + i * 50))) as ArrayRef,
        ),
        ("px", Arc::new(Float64Array::from_iter((0..rows).map(|i| (i % 9 != 0).then_some(i as f64 / 100.0))))),
        ("s", Arc::new(StringArray::from_iter_values((0..rows).map(|i| format!("k{}", i % 4))))),
    ])
    .unwrap()
}

/// `batch` written by the tparquet writer with `policy`, encrypted with a footer key, and
/// column keys for `ts` and `px` when `column_keys`.
fn write(batch: &RecordBatch, policy: Policy, plaintext_footer: bool, column_keys: bool) -> Bytes {
    let mut e = FileEncryptionProperties::builder(FOOTER_KEY.to_vec()).with_plaintext_footer(plaintext_footer);
    if column_keys {
        e = e.with_column_key("ts", TS_KEY.to_vec()).with_column_key("px", PX_KEY.to_vec());
    }
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(1000))
        .set_data_page_row_count_limit(200)
        .set_write_batch_size(100)
        .with_file_encryption_properties(e.build().unwrap())
        .build();
    let options = WriterOptions::default().with_policy(policy).with_properties(props);
    common::write(std::slice::from_ref(batch), options).0
}

fn decryption(column_keys: bool) -> Arc<FileDecryptionProperties> {
    let mut d = FileDecryptionProperties::builder(FOOTER_KEY.to_vec());
    if column_keys {
        d = d.with_column_key("ts", TS_KEY.to_vec()).with_column_key("px", PX_KEY.to_vec());
    }
    d.build().unwrap()
}

fn read(file: &Bytes, column_keys: bool, selection: Option<RowSelection>) -> Vec<RecordBatch> {
    let options = ArrowReaderOptions::new()
        .with_file_decryption_properties(decryption(column_keys))
        .with_page_index_policy(PageIndexPolicy::Optional);
    let mut builder = ParquetRecordBatchReaderBuilder::try_new_with_options(file.clone(), options).unwrap();
    let codecs: Vec<String> =
        builder.metadata().row_group(0).columns().iter().map(|c| c.compression().to_string()).collect();
    assert!(codecs[0].starts_with("PCODEC") && codecs[1].starts_with("PCODEC"), "{codecs:?}");
    let footer = builder.metadata().file_metadata().key_value_metadata().cloned().unwrap_or_default();
    assert!(footer.iter().any(|kv| kv.key == "parquet.pcodec.version" && kv.value.as_deref() == Some("1")));
    if let Some(s) = selection {
        builder = builder.with_row_selection(s);
    }
    builder.build().unwrap().map(|b| b.unwrap()).collect()
}

#[test]
fn encrypted_pcodec_files_round_trip() {
    let batch = batch(3500);
    for plaintext_footer in [false, true] {
        for column_keys in [false, true] {
            for policy in [Policy::Pcodec, Policy::Auto] {
                let file = write(&batch, policy, plaintext_footer, column_keys);
                // Auto compares encrypted scratch chunks; it picks PCODEC for both (`read` checks)
                let got = read(&file, column_keys, None);
                assert_eq!(batch_lines(&got), batch_lines(std::slice::from_ref(&batch)));
                // page skipping through the offset index of encrypted PCODEC chunks
                let s = RowSelection::from(vec![
                    RowSelector::skip(1150),
                    RowSelector::select(500),
                    RowSelector::skip(1849),
                    RowSelector::select(1),
                ]);
                let got = read(&file, column_keys, Some(s));
                assert_eq!(batch_lines(&got), batch_lines(&[batch.slice(1150, 500), batch.slice(3499, 1)]));
            }
        }
    }
}

#[test]
fn encrypted_files_need_their_keys() {
    let batch = batch(500);
    let file = write(&batch, Policy::Pcodec, false, true);
    assert!(ParquetRecordBatchReaderBuilder::try_new(file.clone()).is_err());
    // the footer key alone does not decrypt the PCODEC columns' keys
    let options = ArrowReaderOptions::new().with_file_decryption_properties(decryption(false));
    let err = ParquetRecordBatchReaderBuilder::try_new_with_options(file.clone(), options)
        .and_then(|b| b.build()?.collect::<Result<Vec<_>, _>>().map_err(Into::into));
    assert!(err.is_err());
    // inspect and verify fail cleanly on an encrypted footer
    assert!(tparquet::inspect::inspect(&file).is_err());
    assert!(tparquet::verify::verify(file.clone(), file).is_err());
}

#[test]
fn inspect_on_a_plaintext_footer_with_encrypted_columns_does_not_panic() {
    let batch = batch(500);
    for column_keys in [false, true] {
        let file = write(&batch, Policy::Pcodec, true, column_keys);
        // the page headers are encrypted: an error, or a report of what could be read
        let _ = tparquet::inspect::inspect(&file);
    }
}
