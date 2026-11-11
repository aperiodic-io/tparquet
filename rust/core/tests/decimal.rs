//! DECIMAL mode (spec section 4.3, encoder section 5): exponents, exceptions and the 1% bound,
//! exception positions, the decode formula bit for bit, and the values that must stay exceptions.

mod common;
use common::*;
use pco::standalone::simple_decompress;
use tparquet_core::{Error, Mode, Options, PhysicalType, compress, decompress, mode_of};

/// A DECIMAL payload taken apart: (dtype, exponent, positions, exception bits, q)
struct Parts {
    dtype: u8,
    exponent: u8,
    positions: Vec<u64>,
    exceptions: Vec<u64>,
    q: Vec<i64>,
}

fn varint_at(p: &[u8], at: &mut usize) -> u64 {
    let mut v = 0;
    for i in 0.. {
        let b = p[*at];
        *at += 1;
        v |= u64::from(b & 0x7F) << (7 * i);
        if b < 0x80 {
            break;
        }
    }
    v
}

fn split_payload(p: &[u8]) -> Parts {
    assert_eq!(&p[..2], [1, 2], "not a DECIMAL payload");
    let (dtype, exponent) = (p[2], p[3]);
    let width = if dtype == 4 { 4 } else { 8 };
    let mut at = 4;
    let k = varint_at(p, &mut at);
    let mut positions = vec![];
    let mut next = 0;
    for _ in 0..k {
        let pos = next + varint_at(p, &mut at);
        positions.push(pos);
        next = pos + 1;
    }
    let exceptions = (0..k as usize)
        .map(|j| {
            let mut b = [0u8; 8];
            b[..width].copy_from_slice(&p[at + j * width..at + (j + 1) * width]);
            u64::from_le_bytes(b)
        })
        .collect();
    let q = simple_decompress::<i64>(&p[at + k as usize * width..]).unwrap();
    Parts { dtype, exponent, positions, exceptions, q }
}

fn encode(src: &[u8], t: PhysicalType) -> Vec<u8> {
    let p = compress(src, &Options::with_hint(t)).unwrap();
    assert_eq!(decompress(&p, src.len()).as_deref(), Ok(src), "lossless");
    p
}

/// `n` decimals with exactly `e` digits after the point (at least one in ten has a last digit
/// other than 0, so `e - 1` digits are not enough)
fn decimals(n: usize, seed: u64) -> Vec<i64> {
    let mut r = Rng(seed);
    (0..n).map(|i| r.below(1_000_000) as i64 * 10 + if i % 10 == 0 { 1 + r.below(9) as i64 } else { 0 }).collect()
}

#[test]
fn the_encoder_picks_the_smallest_exponent_for_doubles() {
    for e in 0..=18u8 {
        let q = decimals(2_000, u64::from(e) + 1);
        let v: Vec<f64> = q.iter().map(|q| spec_double(*q, e)).collect();
        let p = encode(&f64s(&v), PhysicalType::Double);
        assert_eq!(mode_of(&p), Ok(Mode::Decimal), "e={e}");
        let parts = split_payload(&p);
        assert_eq!((parts.dtype, parts.exponent), (5, e), "e={e}");
        assert!(parts.positions.is_empty(), "e={e}: {} exceptions", parts.positions.len());
        assert_eq!(parts.q, q, "e={e}: the integers are the decimals' digits");
    }
}

#[test]
fn the_encoder_picks_the_smallest_exponent_for_floats() {
    // f32 holds about 7 digits: q below 10^6 is exact at every exponent
    for e in 0..=18u8 {
        let mut r = Rng(u64::from(e) + 40);
        let q: Vec<i64> = (0..2_000).map(|i| r.below(99_999) as i64 * 10 + i64::from(i % 10 == 0)).collect();
        let v: Vec<f32> = q.iter().map(|q| spec_float(*q, e)).collect();
        let p = encode(&f32s(&v), PhysicalType::Float);
        assert_eq!(mode_of(&p), Ok(Mode::Decimal), "e={e}");
        let parts = split_payload(&p);
        assert_eq!((parts.dtype, parts.exponent), (4, e), "e={e}");
        for (i, (q, v)) in parts.q.iter().zip(&v).enumerate() {
            if !parts.positions.contains(&(i as u64)) {
                assert_eq!(spec_float(*q, e).to_bits(), v.to_bits(), "e={e} i={i}");
            }
        }
    }
}

/// `n` doubles with two decimals, with NaN payloads (exact at no exponent) at `at`
fn with_exceptions(n: usize, at: &[usize]) -> Vec<f64> {
    let mut v: Vec<f64> = (0..n).map(|i| (10_000 + (i * 37) % 9_000) as f64 / 100.0).collect();
    for (j, &i) in at.iter().enumerate() {
        v[i] = f64::from_bits(0x7FF8_0000_0000_0000 | j as u64);
    }
    v
}

#[test]
fn up_to_one_percent_of_exceptions_and_no_more() {
    let n = 1_000;
    for k in [0usize, 1, 2, 9, 10, 11, 12, 50] {
        let at: Vec<usize> = (0..k).map(|j| j * 19 + 3).collect();
        let p = encode(&f64s(&with_exceptions(n, &at)), PhysicalType::Double);
        if k <= n / 100 {
            assert_eq!(mode_of(&p), Ok(Mode::Decimal), "k={k}");
            let parts = split_payload(&p);
            assert_eq!(parts.positions, at.iter().map(|i| *i as u64).collect::<Vec<_>>(), "k={k}");
        } else {
            assert_eq!(mode_of(&p), Ok(Mode::Pco), "k={k}: over 1%");
        }
    }
    // under 100 values, a single exception is over 1%
    let p = encode(&f64s(&with_exceptions(99, &[5])), PhysicalType::Double);
    assert_eq!(mode_of(&p), Ok(Mode::Pco));
}

#[test]
fn the_probe_needs_99_percent_of_its_first_4096_values() {
    // all exceptions inside the probe of 4096: 40 leave 99.02%, 41 leave 98.99%
    let n = 20_000;
    for (k, want) in [(40usize, Mode::Decimal), (41, Mode::Pco)] {
        let at: Vec<usize> = (0..k).map(|j| j * 100).collect();
        let p = encode(&f64s(&with_exceptions(n, &at)), PhysicalType::Double);
        assert_eq!(mode_of(&p), Ok(want), "k={k}");
    }
    // past the probe only the overall 1% counts: 200 exceptions in 20000, then 201
    for (k, want) in [(200usize, Mode::Decimal), (201, Mode::Pco)] {
        let at: Vec<usize> = (0..k).map(|j| 4_096 + j * 70).collect();
        let p = encode(&f64s(&with_exceptions(n, &at)), PhysicalType::Double);
        assert_eq!(mode_of(&p), Ok(want), "k={k}");
    }
}

#[test]
fn exception_positions_at_the_edges_and_far_apart() {
    let n = 40_000;
    // first, last, consecutive, and gaps that need 2 and 3 varint bytes
    let at = [0usize, 1, 2, 3, 130, 131, 20_000, 39_998, 39_999];
    let p = encode(&f64s(&with_exceptions(n, &at)), PhysicalType::Double);
    let parts = split_payload(&p);
    assert_eq!(parts.positions, at.iter().map(|i| *i as u64).collect::<Vec<_>>());
    // the deltas on the wire: d_j = p_j - p_{j-1} - 1
    assert_eq!(&p[4..5], [9]);
    assert_eq!(&p[5..9], [0, 0, 0, 0]);
    assert_eq!(&p[9..11], [126, 0], "130 - 3 - 1");
    let mut want = vec![];
    varint(20_000 - 131 - 1, &mut want);
    assert_eq!(want.len(), 3);
    assert_eq!(&p[11..14], &want[..]);
}

#[test]
fn an_exception_repeats_the_previous_regular_q() {
    // spec 4.3, reference encoder: q at an exception is the previous regular q, 0 before the first
    let at = [0usize, 1, 500, 501, 999];
    let v = with_exceptions(1_000, &at);
    let parts = split_payload(&encode(&f64s(&v), PhysicalType::Double));
    assert_eq!(parts.exponent, 2);
    assert_eq!((parts.q[0], parts.q[1]), (0, 0));
    assert_eq!(parts.q[500], parts.q[499]);
    assert_eq!(parts.q[501], parts.q[499]);
    assert_eq!(parts.q[999], parts.q[998]);
    assert_eq!(parts.q[2], (v[2] * 100.0).round() as i64);
    for (j, &i) in at.iter().enumerate() {
        assert_eq!(parts.exceptions[j], v[i].to_bits());
    }
}

#[test]
fn signed_zero_nan_infinity_and_subnormals_are_exceptions() {
    let specials = [
        -0.0,
        f64::NAN,
        -f64::NAN,
        f64::from_bits(0x7FF0_0000_0000_0001), // signalling
        f64::from_bits(0xFFF0_0000_0000_0001), // negative signalling
        f64::from_bits(0x7FFF_FFFF_FFFF_FFFF), // all payload bits
        f64::from_bits(0x7FF8_0000_DEAD_BEEF),
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::from_bits(1), // smallest subnormal
        -f64::from_bits(1),
        f64::from_bits(0x000F_FFFF_FFFF_FFFF), // largest subnormal
        f64::MIN_POSITIVE,
    ];
    let mut v: Vec<f64> = (0..2_000).map(|i| f64::from(i) / 8.0).collect();
    let at: Vec<usize> = (0..specials.len()).map(|j| j * 150 + 7).collect();
    for (j, &i) in at.iter().enumerate() {
        v[i] = specials[j];
    }
    assert_eq!(v[0].to_bits(), 0, "+0.0 is a regular value");
    let parts = split_payload(&encode(&f64s(&v), PhysicalType::Double));
    assert_eq!(parts.positions, at.iter().map(|i| *i as u64).collect::<Vec<_>>());
    assert_eq!(parts.exceptions, specials.iter().map(|x| x.to_bits()).collect::<Vec<_>>());

    // the same for floats
    let fspecials: Vec<f32> = [
        -0.0,
        f32::NAN,
        -f32::NAN,
        f32::from_bits(0x7F80_0001),
        f32::from_bits(0xFF80_0001),
        f32::from_bits(0x7FFF_FFFF),
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::from_bits(1),
        f32::from_bits(0x007F_FFFF),
        f32::MIN_POSITIVE,
    ]
    .to_vec();
    let mut w: Vec<f32> = (0..2_000).map(|i| (i % 300) as f32 / 10.0).collect();
    let at: Vec<usize> = (0..fspecials.len()).map(|j| j * 150 + 7).collect();
    for (j, &i) in at.iter().enumerate() {
        w[i] = fspecials[j];
    }
    let parts = split_payload(&encode(&f32s(&w), PhysicalType::Float));
    assert_eq!(parts.dtype, 4);
    assert_eq!(parts.positions, at.iter().map(|i| *i as u64).collect::<Vec<_>>());
    assert_eq!(parts.exceptions, fspecials.iter().map(|x| u64::from(x.to_bits())).collect::<Vec<_>>());
}

#[test]
fn only_exceptions_is_not_decimal() {
    let v = vec![f64::NAN; 1_000];
    assert_eq!(mode_of(&encode(&f64s(&v), PhysicalType::Double)), Ok(Mode::Pco));
    let v = vec![-0.0f32; 1_000];
    assert_eq!(mode_of(&encode(&f32s(&v), PhysicalType::Float)), Ok(Mode::Pco));
}

// ------------------------------------------------------------ the decoder against the formula

#[test]
fn the_decoder_computes_the_spec_formula_at_every_exponent() {
    let mut q: Vec<i64> = vec![0, 1, -1, 7, 10, 99, 123_456_789, -987_654_321, 1 << 53, (1 << 53) + 1, -(1 << 53) - 3];
    q.extend([i64::MAX, i64::MIN, i64::MAX - 1, i64::MIN + 1, 1 << 62, -(1 << 62)]);
    let mut r = Rng(5);
    q.extend((0..200).map(|_| r.next() as i64 >> (r.below(60) + 1)));
    for e in 0..=18 {
        let out = decompress(&decimal_payload(5, e, &[], &q), q.len() * 8).unwrap();
        let want: Vec<u64> = q.iter().map(|q| spec_double(*q, e).to_bits()).collect();
        assert_eq!(bits64(&out), want, "DOUBLE e={e}");
        let out = decompress(&decimal_payload(4, e, &[], &q), q.len() * 4).unwrap();
        let want: Vec<u32> = q.iter().map(|q| spec_float(*q, e).to_bits()).collect();
        assert_eq!(bits32(&out), want, "FLOAT e={e}");
    }
}

#[test]
fn integers_round_to_the_nearest_double_with_ties_to_even() {
    // f64(q) for q beyond 2^53: 2^53 + 1 is a tie between 2^53 and 2^53 + 2: even wins
    let q = [(1i64 << 53) + 1, (1 << 53) + 3, i64::MAX, -(1 << 53) - 1];
    let out = bits64(&decompress(&decimal_payload(5, 0, &[], &q), 32).unwrap());
    assert_eq!(out[0], 9_007_199_254_740_992f64.to_bits());
    assert_eq!(out[1], 9_007_199_254_740_996f64.to_bits());
    assert_eq!(out[2], 9_223_372_036_854_775_808f64.to_bits());
    assert_eq!(out[3], (-9_007_199_254_740_992f64).to_bits());
}

#[test]
fn floats_round_once_from_the_double_quotient() {
    // f32(f64(q) / P[e]) is not f32(q) / f32(10^e): find values where the two differ, and check
    // the decoder follows the spec
    let mut differ = 0;
    for e in 1..=10u8 {
        let q: Vec<i64> = (1..2_000).map(|i| i * 7_919 + 1_000_003).collect();
        let out = bits32(&decompress(&decimal_payload(4, e, &[], &q), q.len() * 4).unwrap());
        for (q, got) in q.iter().zip(out) {
            assert_eq!(got, spec_float(*q, e).to_bits(), "q={q} e={e}");
            let naive = *q as f32 / 10f32.powi(i32::from(e));
            differ += usize::from(naive.to_bits() != got);
        }
    }
    assert!(differ > 10, "only {differ} values tell the two formulas apart");
}

#[test]
fn exceptions_overwrite_whatever_q_says() {
    // q at an exception is unused: any value there decodes to the exception's bits
    let nan = 0x7FF4_0000_0000_0001u64;
    let p = decimal_payload(5, 3, &[(0, nan), (2, 0x8000_0000_0000_0000)], &[i64::MIN, 1_500, i64::MAX]);
    assert_eq!(bits64(&decompress(&p, 24).unwrap()), [nan, 1.5f64.to_bits(), 0x8000_0000_0000_0000]);
    let p = decimal_payload(4, 1, &[(1, 0xFFC0_0001)], &[15, 0, -15]);
    assert_eq!(bits32(&decompress(&p, 12).unwrap()), [1.5f32.to_bits(), 0xFFC0_0001, (-1.5f32).to_bits()]);
}

#[test]
fn every_value_may_be_an_exception() {
    let n = 300u64;
    let exc: Vec<(u64, u64)> = (0..n).map(|i| (i, f64::from_bits(i).to_bits())).collect();
    let p = decimal_payload(5, 0, &exc, &vec![0; n as usize]);
    assert_eq!(bits64(&decompress(&p, n as usize * 8).unwrap()), (0..n).collect::<Vec<_>>());
    // k = n + 1 is one too many
    let mut q = p[..4].to_vec();
    varint(n + 1, &mut q);
    assert_eq!(decompress(&q, n as usize * 8), Err(Error::BadException));
}

#[test]
fn positions_must_be_inside_the_page() {
    let q = vec![0i64; 10];
    // last position exactly n - 1 is fine, n is not
    assert!(decompress(&decimal_payload(5, 0, &[(9, 1)], &q), 80).is_ok());
    assert_eq!(decompress(&decimal_payload(5, 0, &[(10, 1)], &q), 80), Err(Error::BadException));
    assert_eq!(decompress(&decimal_payload(5, 0, &[(3, 1), (10, 1)], &q), 80), Err(Error::BadException));
    // a delta that would wrap a u64 position
    let mut p = vec![1, 2, 5, 0, 2, 0];
    varint(u64::MAX, &mut p);
    assert_eq!(decompress(&p, 80), Err(Error::BadException));
    let mut p = vec![1, 2, 5, 0, 1];
    varint(u64::MAX, &mut p);
    assert_eq!(decompress(&p, 80), Err(Error::BadException));
    // the position check comes before the exception bytes are needed
    let mut p = vec![1, 2, 5, 0, 1];
    varint(10, &mut p);
    assert_eq!(decompress(&p, 80), Err(Error::BadException));
}

#[test]
fn the_pcodec_stream_holds_exactly_n_integers() {
    let p = decimal_payload(5, 2, &[(1, 0)], &[1, 2, 3]);
    assert_eq!(decompress(&p, 24).map(|v| v.len()), Ok(24));
    assert_eq!(decompress(&p, 16), Err(Error::SizeMismatch { expected: 16, actual: 24 }));
    assert_eq!(decompress(&p, 32), Err(Error::SizeMismatch { expected: 32, actual: 24 }));
    let p4 = decimal_payload(4, 2, &[], &[1, 2, 3]);
    assert_eq!(decompress(&p4, 8), Err(Error::SizeMismatch { expected: 8, actual: 12 }));
    assert_eq!(decompress(&p4, 16), Err(Error::SizeMismatch { expected: 16, actual: 12 }));
    // the stream is i64 whatever the dtype: an f64 stream is refused
    let mut p = vec![1, 2, 5, 0, 0];
    p.extend(pco(&[1.5f64]));
    assert!(matches!(decompress(&p, 8), Err(Error::Pco(_))), "{:?}", decompress(&p, 8));
}

#[test]
fn every_exponent_up_to_18_and_none_above() {
    for e in 0..=18 {
        assert!(decompress(&decimal_payload(5, e, &[], &[1]), 8).is_ok(), "e={e}");
        assert!(decompress(&decimal_payload(4, e, &[], &[1]), 4).is_ok(), "e={e}");
    }
    for e in [19u8, 20, 100, 255] {
        assert_eq!(decompress(&decimal_payload(5, e, &[], &[1]), 8), Err(Error::BadExponent(e)));
        assert_eq!(decompress(&decimal_payload(4, e, &[], &[1]), 4), Err(Error::BadExponent(e)));
    }
    assert_eq!(Error::BadExponent(19).to_string(), "pcodec payload: exponent 19 above 18");
}
