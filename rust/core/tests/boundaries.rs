//! Boundary values of every type survive every mode bit for bit (spec 4.2: "including NaN
//! payloads and signed zeros").

mod common;
use common::*;
use tparquet_core::{Mode, Options, PhysicalType, compress, decompress, mode_of};

fn every_way(src: &[u8], t: PhysicalType) -> Vec<Mode> {
    let mut modes = vec![];
    for hint in [None, Some(t)] {
        for decimal in [false, true] {
            for level in [0, 4, 8, 12] {
                let p = compress(src, &Options { level, hint, decimal }).unwrap();
                assert_eq!(decompress(&p, src.len()).as_deref(), Ok(src), "{hint:?} {decimal} {level}");
                modes.push(mode_of(&p).unwrap());
            }
        }
    }
    modes
}

fn patterns<T: Copy>(specials: &[T], filler: T) -> Vec<Vec<T>> {
    let n = 1_000;
    vec![
        specials.to_vec(),
        specials.iter().copied().cycle().take(n).collect(),
        (0..n).map(|i| if i % 97 == 0 { specials[i / 97 % specials.len()] } else { filler }).collect(),
        specials.iter().flat_map(|s| std::iter::repeat_n(*s, 300)).collect(),
    ]
}

#[test]
fn integer_extremes() {
    let i32s_ = [i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX];
    for v in patterns(&i32s_, 12_345) {
        every_way(&i32s(&v), PhysicalType::Int32);
    }
    let i64s_ =
        [i64::MIN, i64::MIN + 1, i64::from(i32::MIN) - 1, -1, 0, 1, i64::from(u32::MAX) + 1, i64::MAX - 1, i64::MAX];
    for v in patterns(&i64s_, 1_700_000_000_000) {
        every_way(&i64s(&v), PhysicalType::Int64);
    }
    // sequences that wrap around in delta form
    let wrap: Vec<i64> = (0..1_000).map(|i| i64::MAX.wrapping_add(i * 1_000_003)).collect();
    assert!(every_way(&i64s(&wrap), PhysicalType::Int64).contains(&Mode::Pco));
    let wrap: Vec<i32> = (0..1_000).map(|i| i32::MAX.wrapping_add(i * 7_919)).collect();
    assert!(every_way(&i32s(&wrap), PhysicalType::Int32).contains(&Mode::Pco));
}

fn double_specials() -> Vec<f64> {
    let mut v = vec![
        0.0,
        -0.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        f64::EPSILON,
        f64::from_bits(1),
        f64::from_bits(0x000F_FFFF_FFFF_FFFF),
        -f64::from_bits(1),
    ];
    // every NaN class: quiet and signalling, both signs, smallest, largest and odd payloads
    for sign in [0u64, 1 << 63] {
        for payload in
            [1u64, 0x42, 0x0007_FFFF_FFFF_FFFF, 0x0008_0000_0000_0000, 0x000F_FFFF_FFFF_FFFF, 0x000A_AAAA_5555_5555]
        {
            v.push(f64::from_bits(sign | 0x7FF0_0000_0000_0000 | payload));
        }
    }
    v
}

fn float_specials() -> Vec<f32> {
    let mut v = vec![
        0.0,
        -0.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        f32::MIN,
        f32::MIN_POSITIVE,
        f32::EPSILON,
        f32::from_bits(1),
        f32::from_bits(0x007F_FFFF),
        -f32::from_bits(1),
    ];
    for sign in [0u32, 1 << 31] {
        for payload in [1u32, 0x42, 0x003F_FFFF, 0x0040_0000, 0x007F_FFFF, 0x0055_AAAA] {
            v.push(f32::from_bits(sign | 0x7F80_0000 | payload));
        }
    }
    v
}

#[test]
fn double_extremes_and_every_nan_class() {
    let s = double_specials();
    for v in patterns(&s, 101.25) {
        every_way(&f64s(&v), PhysicalType::Double);
    }
    // as rare exceptions among decimals: DECIMAL keeps them exact
    let mut v: Vec<f64> = (0..4_000).map(|i| f64::from(i % 1_000) / 100.0).collect();
    for (j, x) in s.iter().enumerate().filter(|(_, x)| x.to_bits() != 0) {
        v[j * 100 + 1] = *x;
    }
    assert!(every_way(&f64s(&v), PhysicalType::Double).contains(&Mode::Decimal));
}

#[test]
fn float_extremes_and_every_nan_class() {
    let s = float_specials();
    for v in patterns(&s, 101.25) {
        every_way(&f32s(&v), PhysicalType::Float);
    }
    let mut v: Vec<f32> = (0..4_000).map(|i| (i % 1_000) as f32 / 10.0).collect();
    for (j, x) in s.iter().enumerate().filter(|(_, x)| x.to_bits() != 0) {
        v[j * 100 + 1] = *x;
    }
    assert!(every_way(&f32s(&v), PhysicalType::Float).contains(&Mode::Decimal));
}

#[test]
fn any_bits_as_floats() {
    // integers reinterpreted as floats are NaNs, subnormals and huge values all at once
    let mut r = Rng(17);
    let bits: Vec<u64> = (0..3_000).map(|_| r.next() >> r.below(64)).collect();
    let src: Vec<u8> = bits.iter().flat_map(|b| b.to_le_bytes()).collect();
    for t in TYPES {
        every_way(&src, t);
    }
}
