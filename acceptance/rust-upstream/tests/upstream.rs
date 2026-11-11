//! The upstream `parquet` 60.1.0 crate does not know PCODEC: reading a PCODEC file fails with
//! an error naming the unknown codec, and it never returns data. The standard control file
//! still reads.

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};
use std::fs::File;
use std::path::{Path, PathBuf};

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/files").join(format!("{name}.parquet"))
}

const TPARQUET_FILES: [&str; 23] = [
    "primitives_required",
    "primitives_optional",
    "lists",
    "nested_struct",
    "temporal",
    "decimal_floats",
    "float_specials",
    "empty",
    "multiple_row_groups",
    "pages_v1",
    "pages_v2",
    "mixed",
    "maps",
    "deep_nesting",
    "dictionary_fallback",
    "pcodec_levels",
    "payload_modes",
    "integer_types",
    "auto_choice",
    "large_page",
    "statistics",
    "all_null_pages",
    "key_value_metadata",
];

#[test]
fn upstream_rejects_every_pcodec_file() {
    for name in TPARQUET_FILES {
        let err = SerializedFileReader::new(File::open(testdata(name)).unwrap()).err();
        let err = err.unwrap_or_else(|| panic!("{name}: upstream read a PCODEC file"));
        assert!(err.to_string().contains("Unexpected CompressionCodec 20547"), "{name}: {err}");
        let err = ParquetRecordBatchReaderBuilder::try_new(File::open(testdata(name)).unwrap()).err();
        let err = err.unwrap_or_else(|| panic!("{name}: upstream read a PCODEC file"));
        assert!(err.to_string().contains("Unexpected CompressionCodec 20547"), "{name}: {err}");
    }
}

#[test]
fn upstream_reads_the_control_file() {
    let r = SerializedFileReader::new(File::open(testdata("standard_control")).unwrap()).unwrap();
    assert_eq!(r.metadata().file_metadata().num_rows(), 1500);
    let rows: usize = ParquetRecordBatchReaderBuilder::try_new(File::open(testdata("standard_control")).unwrap())
        .unwrap()
        .build()
        .unwrap()
        .map(|b| b.unwrap().num_rows())
        .sum();
    assert_eq!(rows, 1500);
}

/// Every PCODEC vector as bytes.
fn pcodec_files() -> Vec<(&'static str, bytes::Bytes)> {
    TPARQUET_FILES.iter().map(|name| (*name, bytes::Bytes::from(std::fs::read(testdata(name)).unwrap()))).collect()
}

fn assert_unknown_codec(name: &str, err: Option<impl std::fmt::Display>) {
    let err = err.unwrap_or_else(|| panic!("{name}: upstream read a PCODEC file"));
    assert!(err.to_string().contains("Unexpected CompressionCodec 20547"), "{name}: {err}");
}

#[test]
fn upstream_rejects_pcodec_on_every_entry_point() {
    use parquet::arrow::ProjectionMask;
    use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions};
    use parquet::file::metadata::{PageIndexPolicy, ParquetMetaDataReader};
    use parquet::file::serialized_reader::ReadOptionsBuilder;
    for (name, file) in pcodec_files() {
        // the footer alone, with and without page indexes
        assert_unknown_codec(name, ParquetMetaDataReader::new().parse_and_finish(&file).err());
        let reader = ParquetMetaDataReader::new().with_page_index_policy(PageIndexPolicy::Optional);
        assert_unknown_codec(name, reader.parse_and_finish(&file).err());
        assert_unknown_codec(name, ArrowReaderMetadata::load(&file, ArrowReaderOptions::new()).err());
        let options = ReadOptionsBuilder::new().with_page_index().build();
        assert_unknown_codec(name, SerializedFileReader::new_with_options(file.clone(), options).err());
        // even a projection that only needs other columns, or none
        let err = ParquetRecordBatchReaderBuilder::try_new(file.clone())
            .and_then(|b| {
                let mask = ProjectionMask::leaves(b.parquet_schema(), []);
                b.with_projection(mask).build().map(|_| ())
            })
            .err();
        assert_unknown_codec(name, err);
    }
}

#[test]
fn upstream_rejects_pcodec_files_however_they_are_cut() {
    // the footer is where the codec is: every suffix of a PCODEC file that holds its footer
    // fails the same way; a truncated file fails as truncated, never with data
    for (name, file) in pcodec_files() {
        let len = file.len();
        let footer = 8 + u32::from_le_bytes(file[len - 8..len - 4].try_into().unwrap()) as usize;
        for cut in [len - footer, len - footer - 1, 0] {
            let tail = file.slice(cut..);
            assert_unknown_codec(name, SerializedFileReader::new(tail).err());
        }
        assert!(SerializedFileReader::new(file.slice(..len - 1)).is_err(), "{name}");
    }
}

#[test]
fn upstream_does_not_know_the_codec_name() {
    use parquet::basic::Compression;
    for name in ["PCODEC", "pcodec", "PCODEC(8)"] {
        let err = name.parse::<Compression>().unwrap_err();
        assert!(err.to_string().contains("unsupported compression"), "{name}: {err}");
    }
    assert!("ZSTD(3)".parse::<Compression>().is_ok());
}
