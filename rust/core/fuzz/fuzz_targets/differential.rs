//! tparquet-core's PCO mode against pcodec itself: whatever bytes follow a PCO header, if
//! tparquet decodes them, pcodec decodes the same numbers (tparquet is stricter: it also
//! refuses bytes after the file and counts that differ from the page); and pcodec's own files of
//! any configuration decode in tparquet exactly as in pcodec (or fail in both).
#![no_main]
use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use pco::data_types::Number;
use pco::standalone::{FileDecompressor, simple_compress};
use pco::{ChunkConfig, DeltaSpec, ModeSpec, PagingSpec};
use tparquet_core::decompress;

fn plain<T: Number, const W: usize>(v: &[T], f: fn(T) -> [u8; W]) -> Vec<u8> {
    v.iter().flat_map(|x| f(*x)).collect()
}

fn config(u: &mut Unstructured) -> Result<ChunkConfig> {
    let delta = match u.int_in_range(0..=4u8)? {
        0 => DeltaSpec::Auto,
        1 => DeltaSpec::NoOp,
        2 => DeltaSpec::TryConsecutive(u.int_in_range(1..=7)?),
        3 => DeltaSpec::TryLookback,
        _ => DeltaSpec::TryConv1(u.int_in_range(1..=32)?),
    };
    let mode = match u.int_in_range(0..=3u8)? {
        0 => ModeSpec::Auto,
        1 => ModeSpec::Classic,
        2 => ModeSpec::TryDict,
        _ => ModeSpec::TryIntMult(u.int_in_range(2..=1000)?),
    };
    Ok(ChunkConfig::default()
        .with_compression_level(u.int_in_range(0..=12)?)
        .with_delta_spec(delta)
        .with_mode_spec(mode)
        .with_paging_spec(PagingSpec::EqualPagesUpTo(u.int_in_range(1..=1000)?)))
}

fn check<T: Number + PartialEq, const W: usize>(u: &mut Unstructured, dtype: u8, f: fn(T) -> [u8; W]) -> Result<()>
where
    T: for<'a> libfuzzer_sys::arbitrary::Arbitrary<'a>,
{
    let v: Vec<T> = u.arbitrary()?;
    let cfg = config(u)?;
    if let Ok(file) = simple_compress(&v, &cfg) {
        let mut p = vec![1, 1, dtype];
        p.extend(&file);
        // pcodec 1.0.4 can write files it cannot read itself (TryDict with TryLookback), so the
        // two decoders are compared rather than either trusted
        let ours = decompress(&p, v.len() * W).ok();
        let theirs = FileDecompressor::new(&file[..])
            .and_then(|(fd, rest)| fd.with_max_prealloc(1 << 16).simple_decompress::<T>(rest))
            .ok()
            .map(|t| plain(&t, f));
        assert_eq!(ours, theirs, "tparquet and pcodec disagree on pcodec's own file");
        if let Some(ours) = ours {
            assert_eq!(ours, plain(&v, f));
        }
    }
    // arbitrary bytes after the header: tparquet Ok implies pcodec agrees
    let body: Vec<u8> = u.arbitrary()?;
    let n = u.int_in_range(0..=4096usize)?;
    let mut p = vec![1, 1, dtype];
    p.extend(&body);
    if let Ok(out) = decompress(&p, n * W) {
        // capped: pcodec's own simple_decompress reserves up to 4 GB from the file's count hint
        let (file, rest) = FileDecompressor::new(&body[..]).expect("pcodec reads the header tparquet read");
        let theirs: Vec<T> =
            file.with_max_prealloc(1 << 16).simple_decompress(rest).expect("pcodec reads what tparquet read");
        assert_eq!(plain(&theirs, f), out);
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let _ = match u.int_in_range(0..=3u8) {
        Ok(0) => check::<i32, 4>(&mut u, 1, i32::to_le_bytes),
        Ok(1) => check::<i64, 8>(&mut u, 2, i64::to_le_bytes),
        Ok(2) => check::<f32, 4>(&mut u, 4, f32::to_le_bytes),
        _ => check::<f64, 8>(&mut u, 5, f64::to_le_bytes),
    };
});
