//! Parquet with the PCODEC compression codec (`spec/FORMAT.md`).
//!
//! - [`Writer`] writes Arrow data with PCODEC for the numeric columns its [`Policy`] selects:
//!   `Auto` compares PCODEC with the base codec (ZSTD level 3 by default) on the first batch.
//! - [`read`] reads any Parquet file, PCODEC or not, as Arrow.
//! - [`transcode`] converts standard Parquet to PCODEC and back.
//! - [`inspect`] and [`verify`] report on files; the `tparquet` CLI wraps all of them.
//!
//! The Parquet implementation is the `parquet` crate fork in `rust/parquet`, re-exported as
//! [`parquet`].

pub mod canonical;
mod error;
pub mod inspect;
pub mod policy;
pub mod transcode;
pub mod verify;
pub mod writer;

pub use error::{Error, Result};
pub use parquet;
pub use policy::Policy;
pub use transcode::{Target, transcode};
pub use writer::{ColumnCodec, Writer, WriterOptions, parse_base};

/// Reading Parquet files (PCODEC or not) as Arrow record batches.
pub mod read {
    pub use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};

    use crate::Result;
    use std::fs::File;
    use std::path::Path;

    /// Opens a file for reading.
    pub fn open(path: impl AsRef<Path>) -> Result<ParquetRecordBatchReaderBuilder<File>> {
        Ok(ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?)
    }
}
