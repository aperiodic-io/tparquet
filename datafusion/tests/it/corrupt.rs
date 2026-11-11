//! Every PCODEC payload is untrusted: a corrupt page gives a DataFusion error, never a panic
//! or an abort (spec section 7).

use crate::common::inspect::page_headers;
use crate::common::*;
use datafusion::prelude::SessionContext;
use parquet::basic::Compression;
use parquet::file::metadata::ParquetMetaDataReader;
use proptest::prelude::*;
use std::path::Path;

/// `(start, end)` in the file of every PCODEC payload (the compressed values section of a
/// page) of the file `bytes`.
fn payloads(bytes: &[u8]) -> Vec<(usize, usize)> {
    let b = bytes::Bytes::copy_from_slice(bytes);
    let metadata = ParquetMetaDataReader::new().parse_and_finish(&b).unwrap();
    let mut out = vec![];
    for rg in metadata.row_groups() {
        for c in rg.columns() {
            if !matches!(c.compression(), Compression::PCODEC(_)) {
                continue;
            }
            // a chunk without one non-null value has nothing to decode: the reader need not
            // decompress its pages (all_null_pages' dictionary column)
            if c.statistics().and_then(|s| s.null_count_opt()) == Some(c.num_values() as u64) {
                continue;
            }
            for (body, h) in page_headers(bytes, c) {
                let (levels, compressed) = h.v2.unwrap_or((0, true));
                let start = body + levels as usize;
                let end = body + h.compressed_size as usize;
                // a values section that decodes to 0 bytes never reaches the codec (spec
                // section 3: a reader may skip it), so corrupting its payload changes nothing
                let values = h.uncompressed_size - levels;
                if compressed && start < end && values > 0 {
                    out.push((start, end));
                }
            }
        }
    }
    out
}

/// Reads the file at `path` with `SELECT *`; `Err` is a DataFusion error (a panic fails the
/// test).
async fn read(ctx: &SessionContext, path: &Path) -> Result<String, String> {
    let df = ctx.sql(&format!("SELECT * FROM {}", lit(path))).await.map_err(|e| e.to_string())?;
    let batches = df.collect().await.map_err(|e| e.to_string())?;
    Ok(canonical_lines(&batches))
}

fn url_ctx() -> SessionContext {
    ctx().enable_url_table()
}

#[tokio::test]
async fn corrupt_payloads_of_every_vector_are_errors() {
    let ctx = url_ctx();
    let dir = tempfile::tempdir().unwrap();
    let mut corrupted = 0;
    for name in vector_names() {
        let original = std::fs::read(vector_file(&name)).unwrap();
        for (i, (start, end)) in payloads(&original).into_iter().enumerate() {
            // version byte, mode byte, a truncated body, a zeroed body
            let mut cases: Vec<(&str, Vec<u8>)> = vec![];
            let mut v = original.clone();
            v[start] = 2;
            cases.push(("version 2", v));
            let mut v = original.clone();
            v[start + 1] = 9;
            cases.push(("mode 9", v));
            // (a RAW body is the values themselves: any bytes decode)
            if end - start > 3 && original[start + 1] != 0 {
                let mut v = original.clone();
                v[start + 2..end].fill(0xff);
                cases.push(("body 0xff", v));
            }
            for (what, bytes) in cases {
                let path = dir.path().join(format!("{name}-{i}.parquet"));
                std::fs::write(&path, &bytes).unwrap();
                let err = read(&ctx, &path).await.expect_err(&format!("{name} page {i} {what}: read"));
                assert!(err.contains("pcodec payload") || err.contains("Parquet error"), "{name} {what}: {err}");
                corrupted += 1;
            }
        }
    }
    assert!(corrupted > 100, "{corrupted}");
}

#[tokio::test]
async fn the_error_names_the_payload_problem() {
    let ctx = url_ctx();
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = std::fs::read(vector_file("primitives_required")).unwrap();
    let (start, _) = payloads(&bytes)[0];
    bytes[start] = 7;
    let path = dir.path().join("bad.parquet");
    std::fs::write(&path, &bytes).unwrap();
    let err = read(&ctx, &path).await.unwrap_err();
    assert!(err.contains("pcodec payload: unsupported version 7"), "{err}");
    // the same through COPY: the error stops the query, nothing is half-written
    let out = dir.path().join("out.parquet");
    let err = sql_err(&ctx, &format!("COPY (SELECT * FROM {}) TO {} STORED AS TPARQUET", lit(&path), lit(&out))).await;
    assert!(err.contains("pcodec payload: unsupported version 7"), "{err}");
}

/// Every PCODEC vector, with its payload ranges.
/// A vector: its name, its bytes and its payload ranges.
type Target = (String, Vec<u8>, Vec<(usize, usize)>);

fn targets() -> Vec<Target> {
    vector_names()
        .into_iter()
        .map(|n| {
            let bytes = std::fs::read(vector_file(&n)).unwrap();
            let p = payloads(&bytes);
            (n, bytes, p)
        })
        .filter(|(_, _, p)| !p.is_empty())
        .collect()
}

proptest! {
    // no regression files: the failures cargo mutants provokes are not regressions
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(64) })]

    /// Random bytes written over a random part of a random payload: the read succeeds or is
    /// a DataFusion error.
    #[test]
    fn random_bytes_in_a_page_never_panic(
        file in any::<prop::sample::Index>(),
        page in any::<prop::sample::Index>(),
        at in any::<prop::sample::Index>(),
        junk in prop::collection::vec(any::<u8>(), 1..64),
    ) {
        let targets = targets();
        let (name, mut bytes, pages) = targets[file.index(targets.len())].clone();
        let (start, end) = pages[page.index(pages.len())];
        let at = start + at.index(end - start);
        let n = junk.len().min(end - at);
        bytes[at..at + n].copy_from_slice(&junk[..n]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{name}.parquet"));
        std::fs::write(&path, &bytes).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result = rt.block_on(read(&url_ctx(), &path));
        if let Err(e) = result {
            prop_assert!(!e.is_empty());
        }
    }

    /// A truncated payload: the page header still says the old size, so the reader sees the
    /// next page's header (or the footer) as payload bytes.
    #[test]
    fn shifted_pages_never_panic(file in any::<prop::sample::Index>(), page in any::<prop::sample::Index>(), cut in 1usize..16) {
        let targets = targets();
        let (name, bytes, pages) = targets[file.index(targets.len())].clone();
        let (start, end) = pages[page.index(pages.len())];
        let cut = cut.min(end - start);
        let mut shifted = bytes[..end - cut].to_vec();
        shifted.extend_from_slice(&bytes[end..]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{name}.parquet"));
        std::fs::write(&path, &shifted).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _ = rt.block_on(read(&url_ctx(), &path));
    }
}
