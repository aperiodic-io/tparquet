//! Against pcodec itself: a PCO payload is the dtype byte and pco's own standalone file, and the
//! decoder reads whatever pco writes (every mode, delta encoding and paging), exactly as pco
//! reads it.

mod common;
use common::*;
use pco::data_types::Number;
use pco::standalone::{simple_compress, simple_decompress};
use pco::{ChunkConfig, DeltaSpec, ModeSpec, PagingSpec};
use tparquet_core::{Mode, Options, PhysicalType, compress, decompress, mode_of};

fn configs() -> Vec<ChunkConfig> {
    let mut v = vec![];
    for level in [0, 3, 8, 12] {
        v.push(ChunkConfig::default().with_compression_level(level));
    }
    for d in [
        DeltaSpec::NoOp,
        DeltaSpec::TryConsecutive(1),
        DeltaSpec::TryConsecutive(7),
        DeltaSpec::TryLookback,
        DeltaSpec::TryConv1(8),
    ] {
        v.push(ChunkConfig::default().with_delta_spec(d));
    }
    for m in [
        ModeSpec::Classic,
        ModeSpec::TryDict,
        ModeSpec::TryIntMult(1_000),
        ModeSpec::TryFloatMult(0.25),
        ModeSpec::TryFloatQuant(8),
    ] {
        v.push(ChunkConfig::default().with_mode_spec(m));
    }
    v.push(ChunkConfig::default().with_paging_spec(PagingSpec::EqualPagesUpTo(77)));
    v
}

/// pco's own file for `v` under every config: tparquet decodes it to what pco decodes
fn same_as_pco<T: Number + PartialEq + std::fmt::Debug>(t: PhysicalType, v: &[T], plain: &[u8]) {
    for cfg in configs() {
        let Ok(file) = simple_compress(v, &cfg) else { continue }; // a spec this type cannot use
        let theirs: Vec<T> = simple_decompress(&file).unwrap();
        assert_eq!(theirs.len(), v.len());
        let mut p = vec![1, 1, t as u8];
        p.extend(&file);
        let ours = decompress(&p, plain.len()).unwrap();
        assert!(ours == plain, "{t:?} {cfg:?}");
    }
}

#[test]
fn every_pcodec_configuration_decodes_like_pcodec() {
    let mut r = Rng(3);
    let n = 3_000;
    let i: Vec<i32> = (0..n).map(|k| [7, -3, 1 << 20, 0][k % 4] + (r.below(3) as i32) * 1_000).collect();
    same_as_pco(PhysicalType::Int32, &i, &i32s(&i));
    let l: Vec<i64> = (0..n as i64).map(|k| 1_700_000_000_000 + k * 1_000 + (k % 7) * (k % 13)).collect();
    same_as_pco(PhysicalType::Int64, &l, &i64s(&l));
    let f: Vec<f32> = (0..n).map(|k| (k % 50) as f32 * 0.25 + if k % 9 == 0 { f32::NAN } else { 0.0 }).collect();
    same_as_pco(PhysicalType::Float, &f, &f32s(&f));
    let d: Vec<f64> = (0..n).map(|k| (k as f64 * 0.01).sin() * 1e6 + if k % 11 == 0 { -0.0 } else { 0.5 }).collect();
    same_as_pco(PhysicalType::Double, &d, &f64s(&d));
}

#[test]
fn a_pco_payload_is_pcos_own_file() {
    for t in TYPES {
        let src = match t {
            PhysicalType::Float => f32s(&(0..2_000).map(|i| (i % 64) as f32 * 0.5 - 7.0).collect::<Vec<_>>()),
            PhysicalType::Double => f64s(&(0..2_000).map(|i| f64::from(i % 64) * 0.5 - 7.0).collect::<Vec<_>>()),
            _ => smooth(t, 2_000, 21),
        };
        for level in [0, 5, 8, 12] {
            let p = compress(&src, &Options { level, decimal: false, ..Options::with_hint(t) }).unwrap();
            assert_eq!(mode_of(&p), Ok(Mode::Pco));
            assert_eq!(&p[..3], [1, 1, t as u8]);
            let cfg = ChunkConfig::default().with_compression_level(usize::from(level));
            let file = match t {
                PhysicalType::Int32 => simple_compress(&ints::<i32, 4>(&src), &cfg),
                PhysicalType::Int64 => simple_compress(&ints::<i64, 8>(&src), &cfg),
                PhysicalType::Float => simple_compress(&ints::<f32, 4>(&src), &cfg),
                PhysicalType::Double => simple_compress(&ints::<f64, 8>(&src), &cfg),
            }
            .unwrap();
            assert_eq!(&p[3..], &file[..], "{t:?} level {level}");
        }
    }
}

trait FromLe<const W: usize> {
    fn from_le(b: [u8; W]) -> Self;
}
impl FromLe<4> for i32 {
    fn from_le(b: [u8; 4]) -> Self {
        i32::from_le_bytes(b)
    }
}
impl FromLe<8> for i64 {
    fn from_le(b: [u8; 8]) -> Self {
        i64::from_le_bytes(b)
    }
}
impl FromLe<4> for f32 {
    fn from_le(b: [u8; 4]) -> Self {
        f32::from_le_bytes(b)
    }
}
impl FromLe<8> for f64 {
    fn from_le(b: [u8; 8]) -> Self {
        f64::from_le_bytes(b)
    }
}

fn ints<T: FromLe<W>, const W: usize>(src: &[u8]) -> Vec<T> {
    src.as_chunks::<W>().0.iter().map(|c| T::from_le(*c)).collect()
}

#[test]
fn a_decimal_payloads_integers_are_a_pcodec_i64_file() {
    let d: Vec<f64> = (0..5_000).map(|i| f64::from(10_000 + i % 3_000) / 100.0).collect();
    let p = compress(&f64s(&d), &Options::with_hint(PhysicalType::Double)).unwrap();
    assert_eq!(&p[..5], [1, 2, 5, 2, 0], "DECIMAL, DOUBLE, e = 2, no exceptions");
    let q: Vec<i64> = simple_decompress(&p[5..]).unwrap();
    assert_eq!(q, (0..5_000).map(|i| i64::from(10_000 + i % 3_000)).collect::<Vec<_>>());
    assert_eq!(&p[5..], &simple_compress(&q, &ChunkConfig::default()).unwrap()[..]);
}
