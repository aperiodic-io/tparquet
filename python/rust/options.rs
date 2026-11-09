//! Writer options from Python arguments.

use crate::errors::{from_pcodec, invalid};
use pyo3::prelude::*;
use std::collections::BTreeMap;
use tparquet::parquet::basic::Compression;
use tparquet::parquet::file::properties::WriterProperties;
use tparquet::{ColumnCodec, Policy, WriterOptions};

/// The keyword arguments `write_table`, `Writer` and `transcode` share.
#[derive(FromPyObject)]
#[pyo3(from_item_all)]
pub struct Options {
    pub policy: String,
    pub columns: BTreeMap<String, String>,
    pub level: i64,
    pub base: String,
    pub row_group_size: Option<i64>,
    pub encodings: BTreeMap<String, String>,
}

/// Parses a base codec such as `zstd(3)` or `snappy` (`tparquet::parse_base`).
pub fn base_codec(s: &str) -> PyResult<Compression> {
    tparquet::parse_base(s).map_err(from_pcodec)
}

impl Options {
    pub fn writer_options(&self) -> PyResult<WriterOptions> {
        let policy: Policy = self.policy.parse().map_err(from_pcodec)?;
        let mut o = WriterOptions::default().with_policy(policy).with_base(base_codec(&self.base)?);
        for (name, p) in &self.columns {
            o = o.with_column(name.clone(), p.parse().map_err(from_pcodec)?);
        }
        for (name, e) in &self.encodings {
            let encoding = e.parse().map_err(|e: tparquet::parquet::errors::ParquetError| invalid(e.to_string()))?;
            o = o.with_encoding(name.clone(), encoding);
        }
        let bad_level = || invalid(format!("level {} is not in 0..=12", self.level));
        let level = u32::try_from(self.level).map_err(|_| bad_level())?;
        o = o.with_level(level).map_err(|_| bad_level())?;
        if let Some(n) = self.row_group_size {
            let n = usize::try_from(n)
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| invalid(format!("row_group_size must be positive, got {n}")))?;
            o.properties = WriterProperties::builder().set_max_row_group_row_count(Some(n)).build();
        }
        Ok(o)
    }
}

/// The name of a codec without its level, as `inspect` writes it (`PCODEC`, `ZSTD`...).
pub fn codec_name(c: Compression) -> String {
    let s = c.to_string();
    s.split_once('(').map_or(s.clone(), |(name, _)| name.to_string())
}

/// Leaf path to codec name.
pub fn codec_map(codecs: &[ColumnCodec]) -> BTreeMap<String, String> {
    codecs.iter().map(|c| (c.path.clone(), codec_name(c.codec))).collect()
}
