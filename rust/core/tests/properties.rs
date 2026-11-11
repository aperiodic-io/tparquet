//! Property tests: lossless on any bytes with any hint and level; the decoder never panics.

use proptest::prelude::*;
use tparquet_core::{Options, PhysicalType, compress, decompress};

fn any_hint() -> impl Strategy<Value = Option<PhysicalType>> {
    prop_oneof![
        Just(None),
        Just(Some(PhysicalType::Int32)),
        Just(Some(PhysicalType::Int64)),
        Just(Some(PhysicalType::Float)),
        Just(Some(PhysicalType::Double)),
    ]
}

/// decimal-looking doubles with occasional arbitrary bit patterns: the DECIMAL path's inputs
fn decimalish() -> impl Strategy<Value = Vec<u8>> {
    (0u8..6, prop::collection::vec((any::<i32>(), any::<u64>(), 0u8..100), 0..3_000)).prop_map(|(e, v)| {
        v.into_iter()
            .flat_map(|(q, bits, roll)| {
                let x = if roll == 0 { f64::from_bits(bits) } else { q as f64 / 10f64.powi(e as i32) };
                x.to_le_bytes()
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn lossless_on_any_bytes(src in prop::collection::vec(any::<u8>(), 0..4_096), hint in any_hint(), level in 0u8..=12, decimal: bool) {
        let p = compress(&src, &Options { level, hint, decimal }).unwrap();
        prop_assert_eq!(decompress(&p, src.len()).unwrap(), src);
    }

    #[test]
    fn lossless_on_decimal_doubles(src in decimalish(), level in 0u8..=12) {
        let p = compress(&src, &Options { level, hint: Some(PhysicalType::Double), decimal: true }).unwrap();
        prop_assert_eq!(decompress(&p, src.len()).unwrap(), src.clone());
        // the same values read as f32 pairs are also lossless
        let p = compress(&src, &Options { level, hint: Some(PhysicalType::Float), decimal: true }).unwrap();
        prop_assert_eq!(decompress(&p, src.len()).unwrap(), src);
    }

    #[test]
    fn never_bigger_than_raw(src in prop::collection::vec(any::<u8>(), 0..2_048), hint in any_hint()) {
        let p = compress(&src, &Options { hint, ..Options::default() }).unwrap();
        prop_assert!(p.len() <= src.len() + 2);
    }

    #[test]
    fn decoder_never_panics(p in prop::collection::vec(any::<u8>(), 0..512), expected in 0usize..65_536) {
        let _ = decompress(&p, expected);
    }

    #[test]
    fn decoder_never_panics_on_valid_prefix(mode in 0u8..4, dtype in prop_oneof![Just(1u8), Just(2), Just(4), Just(5)],
                                            rest in prop::collection::vec(any::<u8>(), 0..512), expected in 0usize..65_536) {
        let mut p = vec![1, mode, dtype];
        p.extend(rest);
        let _ = decompress(&p, expected);
    }
}

mod common;
use common::{decimal_payload, spec_double, spec_float};
use tparquet_core::{Error, Mode, mode_of};

fn any_type() -> impl Strategy<Value = PhysicalType> {
    prop_oneof![
        Just(PhysicalType::Int32),
        Just(PhysicalType::Int64),
        Just(PhysicalType::Float),
        Just(PhysicalType::Double)
    ]
}

/// integers as columns hold them: a start, small or large steps, and some extremes
fn column(width: usize) -> impl Strategy<Value = Vec<u8>> {
    (any::<i64>(), 0u32..64, prop::collection::vec((any::<i64>(), 0u8..20), 0..2_000)).prop_map(
        move |(start, shift, steps)| {
            let mut x = start;
            steps
                .into_iter()
                .flat_map(|(s, roll)| {
                    x = match roll {
                        0 => i64::MIN,
                        1 => i64::MAX,
                        2 => s,
                        _ => x.wrapping_add(s >> shift),
                    };
                    x.to_le_bytes()[..width].to_vec()
                })
                .collect()
        },
    )
}

/// floats that are short decimals at exponent `e`, with rare arbitrary bit patterns
fn decimals(width: usize) -> impl Strategy<Value = Vec<u8>> {
    (0u8..=18, prop::collection::vec((-1_000_000i64..1_000_000, any::<u64>(), 0u8..200), 0..2_000)).prop_map(
        move |(e, v)| {
            v.into_iter()
                .flat_map(|(q, bits, roll)| {
                    if width == 4 {
                        let x = if roll == 0 { f32::from_bits(bits as u32) } else { spec_float(q, e) };
                        x.to_le_bytes().to_vec()
                    } else {
                        let x = if roll == 0 { f64::from_bits(bits) } else { spec_double(q, e) };
                        x.to_le_bytes().to_vec()
                    }
                })
                .collect()
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    #[test]
    fn lossless_on_columns_of_every_type(
        (t, src) in any_type().prop_flat_map(|t| (Just(t), column(t.width()))),
        level in 0u8..=12,
        decimal: bool,
    ) {
        let p = compress(&src, &Options { level, hint: Some(t), decimal }).unwrap();
        prop_assert!(p.len() <= src.len() + 2);
        prop_assert_eq!(decompress(&p, src.len()).unwrap(), src);
    }

    #[test]
    fn lossless_on_decimals_at_every_exponent(src in decimals(8), src32 in decimals(4), level in 0u8..=12) {
        for (src, t) in [(src, PhysicalType::Double), (src32, PhysicalType::Float)] {
            let p = compress(&src, &Options { level, hint: Some(t), decimal: true }).unwrap();
            prop_assert!(p.len() <= src.len() + 2);
            prop_assert_eq!(decompress(&p, src.len()).unwrap(), src);
        }
    }

    #[test]
    fn the_mode_follows_the_hint(src in prop::collection::vec(any::<u8>(), 0..3_000), hint in any_hint(), decimal: bool) {
        let p = compress(&src, &Options { hint, decimal, ..Options::default() }).unwrap();
        let whole = hint.is_some_and(|t| src.len().is_multiple_of(t.width()));
        match mode_of(&p).unwrap() {
            Mode::Raw => prop_assert_eq!(p.len(), src.len() + 2),
            Mode::Zstd => prop_assert!(!whole),
            Mode::Pco => prop_assert!(whole && p[2] == hint.unwrap() as u8),
            Mode::Decimal => prop_assert!(whole && decimal && matches!(hint, Some(PhysicalType::Float | PhysicalType::Double))),
            _ => prop_assert!(false, "unknown mode"),
        }
    }

    #[test]
    fn a_wrong_expected_size_always_fails(src in prop::collection::vec(any::<u8>(), 1..2_000), hint in any_hint(), delta in 1usize..20, grow: bool) {
        let p = compress(&src, &Options { hint, ..Options::default() }).unwrap();
        let n = if grow { src.len() + delta } else { src.len().saturating_sub(delta) };
        prop_assert!(decompress(&p, n).is_err());
    }

    #[test]
    fn a_truncated_payload_always_fails(src in prop::collection::vec(any::<u8>(), 1..1_000), hint in any_hint(), cut in any::<prop::sample::Index>()) {
        let p = compress(&src, &Options { hint, ..Options::default() }).unwrap();
        let cut = cut.index(p.len());
        prop_assert!(decompress(&p[..cut], src.len()).is_err());
    }

    #[test]
    fn decimal_payloads_decode_to_the_spec_formula(
        double: bool,
        e in 0u8..=18,
        q in prop::collection::vec(any::<i64>(), 0..600),
        exc in prop::collection::btree_map(0u64..600, any::<u64>(), 0..20),
    ) {
        let n = q.len() as u64;
        let width = if double { 8 } else { 4 };
        let exc: Vec<(u64, u64)> = exc.into_iter().filter(|(p, _)| *p < n).map(|(p, b)| (p, if double { b } else { b & 0xFFFF_FFFF })).collect();
        let p = decimal_payload(if double { 5 } else { 4 }, e, &exc, &q);
        let out = decompress(&p, q.len() * width).unwrap();
        for (i, q) in q.iter().enumerate() {
            let got = &out[i * width..(i + 1) * width];
            let want = match exc.iter().find(|(p, _)| *p == i as u64) {
                Some((_, bits)) => bits.to_le_bytes()[..width].to_vec(),
                None if double => spec_double(*q, e).to_le_bytes().to_vec(),
                None => spec_float(*q, e).to_le_bytes().to_vec(),
            };
            prop_assert_eq!(got, &want[..], "value {}", i);
        }
        // one value more or less is a size error
        let longer = decompress(&p, (q.len() + 1) * width);
        prop_assert!(matches!(longer, Err(Error::SizeMismatch { .. })), "{:?}", longer);
    }
}
