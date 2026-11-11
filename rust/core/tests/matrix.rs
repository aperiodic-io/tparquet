//! Every dtype × mode × length: the encoder's mode choice (spec section 5) and the decoder's size
//! checks (sections 3 and 4) at 0, 1, the width, around it, around pcodec's batch of 256 and
//! across several pcodec chunks.

mod common;
use common::*;
use pco::standalone::simple_compress;
use pco::{ChunkConfig, PagingSpec};
use tparquet_core::{Error, Mode, Options, PhysicalType, compress, decompress, decompress_into, mode_of};

const HINTS: [Option<PhysicalType>; 5] =
    [None, Some(PhysicalType::Int32), Some(PhysicalType::Int64), Some(PhysicalType::Float), Some(PhysicalType::Double)];

/// byte lengths around every interesting value count for a width
fn lengths(width: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..=2 * width + 1).collect();
    for n in [7, 255, 256, 257, 1_000, 4_096, 4_097] {
        v.extend([n * width - 1, n * width, n * width + 1]);
    }
    v
}

fn check_roundtrip(src: &[u8], opts: &Options) -> Mode {
    let p = compress(src, opts).unwrap();
    assert!(p.len() <= src.len() + 2, "{} > {} + 2", p.len(), src.len());
    assert_eq!(decompress(&p, src.len()).as_deref(), Ok(src), "{opts:?} len {}", src.len());
    let mut dst = vec![0xA5; src.len()];
    assert_eq!(decompress_into(&p, &mut dst), Ok(()));
    assert!(dst == src);
    mode_of(&p).unwrap()
}

#[test]
fn every_hint_and_length_round_trips_with_the_right_mode() {
    let mut seen = Vec::new();
    for hint in HINTS {
        let width = hint.map_or(8, PhysicalType::width);
        for len in lengths(width) {
            // smooth values of the hinted type where whole values fit, followed by spare bytes
            let t = hint.unwrap_or(PhysicalType::Int64);
            let mut src = smooth(t, len / t.width() + 1, len as u64);
            src.truncate(len);
            for decimal in [false, true] {
                for level in [0, 8, 12] {
                    let mode = check_roundtrip(&src, &Options { level, hint, decimal });
                    seen.push((hint, mode));
                    let whole = hint.is_some_and(|t| len.is_multiple_of(t.width()));
                    match mode {
                        Mode::Raw => {}
                        Mode::Zstd => assert!(!whole, "{hint:?} {len}: ZSTD although the width divides"),
                        Mode::Pco => assert!(whole && len > 0, "{hint:?} {len}: PCO"),
                        Mode::Decimal => assert!(
                            whole && decimal && matches!(t, PhysicalType::Float | PhysicalType::Double),
                            "{hint:?} {len}: DECIMAL"
                        ),
                        other => panic!("unknown mode {other:?}"),
                    }
                    if len == 0 {
                        assert_eq!(mode, Mode::Raw);
                    }
                }
            }
        }
    }
    // every type reached PCO, and both hints and no hint reached ZSTD and RAW
    for hint in HINTS {
        assert!(seen.contains(&(hint, Mode::Raw)), "{hint:?} RAW");
        assert!(seen.contains(&(hint, Mode::Zstd)), "{hint:?} ZSTD");
        if hint.is_some() {
            assert!(seen.contains(&(hint, Mode::Pco)), "{hint:?} PCO");
        }
    }
}

#[test]
fn decimal_values_of_every_length_use_decimal() {
    for (t, width) in [(PhysicalType::Float, 4), (PhysicalType::Double, 8)] {
        for n in [100, 255, 256, 257, 1_000, 4_096, 4_097, 10_000] {
            let src = if width == 4 {
                f32s(&(0..n).map(|i| (1_000 + i % 777) as f32 / 10.0).collect::<Vec<_>>())
            } else {
                f64s(&(0..n).map(|i| (100_000 + i % 777) as f64 / 100.0).collect::<Vec<_>>())
            };
            assert_eq!(check_roundtrip(&src, &Options::with_hint(t)), Mode::Decimal, "{t:?} n={n}");
            // and PCO without decimal
            let off = Options { decimal: false, ..Options::with_hint(t) };
            assert_eq!(check_roundtrip(&src, &off), Mode::Pco, "{t:?} n={n}");
        }
    }
}

#[test]
fn integer_hints_never_use_decimal() {
    // the same bytes as decimal doubles, read as integers
    let src = f64s(&(0..2_000).map(|i| i as f64 / 4.0).collect::<Vec<_>>());
    for t in [PhysicalType::Int32, PhysicalType::Int64] {
        assert_eq!(check_roundtrip(&src, &Options::with_hint(t)), Mode::Pco, "{t:?}");
    }
}

#[test]
fn pco_payloads_of_every_count_check_the_size() {
    for t in TYPES {
        let w = t.width();
        for n in [0usize, 1, 2, 3, 255, 256, 257, 511, 512, 513, 1_000] {
            let src = smooth(t, n, n as u64 + 9);
            let p = pco_payload(t, &src);
            assert_eq!(decompress(&p, n * w).as_deref(), Ok(&src[..]), "{t:?} n={n}");
            let mm = |expected, actual| Err(Error::SizeMismatch { expected, actual });
            assert_eq!(decompress(&p, (n + 1) * w), mm((n + 1) * w, n * w), "{t:?} n={n} + 1");
            if n > 0 {
                assert_eq!(decompress(&p, (n - 1) * w), mm((n - 1) * w, n * w), "{t:?} n={n} - 1");
            }
            for off in 1..w {
                let len = n * w + off;
                assert_eq!(decompress(&p, len), Err(Error::NotMultipleOfWidth { len, width: w }));
            }
        }
    }
}

#[test]
fn an_empty_pcodec_file_is_an_empty_page() {
    for t in TYPES {
        let p = pco_payload(t, &[]);
        assert_eq!(decompress(&p, 0), Ok(vec![]), "{t:?}");
        let w = t.width();
        assert_eq!(decompress(&p, w), Err(Error::SizeMismatch { expected: w, actual: 0 }));
    }
    let p = decimal_payload(5, 3, &[], &[]);
    assert_eq!(decompress(&p, 0), Ok(vec![]));
    assert_eq!(decompress(&p, 8), Err(Error::SizeMismatch { expected: 8, actual: 0 }));
}

#[test]
fn many_chunks_decode_and_are_counted() {
    // a pcodec file of ten chunks of 100 values (the encoder splits at 2^18)
    let v: Vec<i64> = (0..1_000).map(|i| i * i).collect();
    let cfg = ChunkConfig::default().with_paging_spec(PagingSpec::EqualPagesUpTo(100));
    let mut p = vec![1, 1, 2];
    p.extend(simple_compress(&v, &cfg).unwrap());
    assert_eq!(decompress(&p, 8_000), Ok(i64s(&v)));
    // the chunk that does not fit is refused: nine fit, the tenth ends at 1000 values
    assert_eq!(decompress(&p, 950 * 8), Err(Error::SizeMismatch { expected: 7_600, actual: 8_000 }));
    assert_eq!(decompress(&p, 900 * 8), Err(Error::SizeMismatch { expected: 7_200, actual: 8_000 }));
    assert_eq!(decompress(&p, 899 * 8), Err(Error::SizeMismatch { expected: 7_192, actual: 7_200 }));
    assert_eq!(decompress(&p, 1_001 * 8), Err(Error::SizeMismatch { expected: 8_008, actual: 8_000 }));
    // uneven chunks, one value each at the edges
    let cfg = ChunkConfig::default().with_paging_spec(PagingSpec::Exact(vec![1, 256, 257, 255, 230, 1]));
    let mut p = vec![1, 1, 2];
    p.extend(simple_compress(&v, &cfg).unwrap());
    assert_eq!(decompress(&p, 8_000), Ok(i64s(&v)));
}

#[test]
fn the_encoder_splits_large_pages_into_chunks_that_decode() {
    // over 2^18 values: pcodec's default paging writes several chunks
    for t in TYPES {
        let n = (1 << 18) + 300;
        let src = smooth(t, n, 77);
        for decimal in [false, true] {
            check_roundtrip(&src, &Options { decimal, ..Options::with_hint(t) });
        }
    }
    let d: Vec<f64> = (0..(1 << 18) + 300).map(|i| (i % 10_000) as f64 / 100.0).collect();
    assert_eq!(check_roundtrip(&f64s(&d), &Options::with_hint(PhysicalType::Double)), Mode::Decimal);
}

#[test]
fn raw_payloads_check_the_size_exactly() {
    for n in [0usize, 1, 2, 100] {
        let mut p = vec![1, 0];
        p.extend(Rng(n as u64 + 1).bytes(n));
        assert_eq!(decompress(&p, n).as_deref(), Ok(&p[2..]));
        assert_eq!(decompress(&p, n + 1), Err(Error::SizeMismatch { expected: n + 1, actual: n }));
        if n > 0 {
            assert_eq!(decompress(&p, n - 1), Err(Error::SizeMismatch { expected: n - 1, actual: n }));
        }
    }
}

#[test]
fn zstd_payloads_check_the_size_exactly() {
    for n in [1usize, 2, 100, 128 * 1024 + 1] {
        let src: Vec<u8> = (0..n).map(|i| (i % 7) as u8).collect();
        for level in [-5, 1, 3, 19] {
            let mut p = vec![1, 3];
            p.extend(zstd::bulk::compress(&src, level).unwrap());
            assert_eq!(decompress(&p, n), Ok(src.clone()), "n={n} level={level}");
            assert_eq!(decompress(&p, n + 1), Err(Error::SizeMismatch { expected: n + 1, actual: n }));
            assert_eq!(decompress(&p, n - 1), Err(Error::SizeMismatch { expected: n - 1, actual: n }));
        }
        // a streaming frame, without a content size, as the Parquet ZSTD codec writes it
        let mut p = vec![1, 3];
        p.extend(zstd::stream::encode_all(&src[..], 3).unwrap());
        assert_eq!(decompress(&p, n), Ok(src.clone()));
        assert_eq!(decompress(&p, n - 1), Err(Error::SizeMismatch { expected: n - 1, actual: n }));
    }
}

#[test]
fn zstd_payloads_may_hold_many_frames() {
    let mut p = vec![1, 3];
    let mut want = vec![];
    for i in 0..20u8 {
        let part = vec![i; usize::from(i) * 3];
        p.extend(zstd::bulk::compress(&part, 1).unwrap());
        want.extend(part);
    }
    assert_eq!(decompress(&p, want.len()), Ok(want.clone()));
    assert_eq!(
        decompress(&p, want.len() - 1),
        Err(Error::SizeMismatch { expected: want.len() - 1, actual: want.len() })
    );
    // an empty frame anywhere is fine
    p.extend(zstd::bulk::compress(&[], 1).unwrap());
    assert_eq!(decompress(&p, want.len()), Ok(want));
}
