//! Core payload vectors: valid payloads with their decoded bytes, and payloads a decoder must
//! reject.

use crate::rng::Rng;
use serde_json::{Value, json};
use tparquet_core::{Mode, Options, PhysicalType, compress, decompress, mode_of};

/// A payload vector: the payload, the expected size given to the decoder, and either the
/// decoded bytes or the error.
pub struct Vector {
    pub name: String,
    pub payload: Vec<u8>,
    pub expected_len: usize,
    pub decoded: Option<Vec<u8>>,
    pub meta: Value,
}

fn le<T: Copy, const W: usize>(v: &[T], f: fn(T) -> [u8; W]) -> Vec<u8> {
    v.iter().flat_map(|x| f(*x)).collect()
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Raw => "RAW",
        Mode::Pco => "PCO",
        Mode::Decimal => "DECIMAL",
        Mode::Zstd => "ZSTD",
        _ => unreachable!("the core has four modes"),
    }
}

fn hint_name(t: Option<PhysicalType>) -> Value {
    match t {
        Some(PhysicalType::Int32) => "INT32".into(),
        Some(PhysicalType::Int64) => "INT64".into(),
        Some(PhysicalType::Float) => "FLOAT".into(),
        Some(PhysicalType::Double) => "DOUBLE".into(),
        None => Value::Null,
    }
}

fn valid(name: &str, input: Vec<u8>, hint: Option<PhysicalType>, level: u8, mode: Mode) -> Vector {
    let payload = compress(&input, &Options { level, hint, decimal: true }).expect("compress");
    assert_eq!(mode_of(&payload), Ok(mode), "{name}");
    assert_eq!(decompress(&payload, input.len()).as_ref(), Ok(&input), "{name}");
    Vector {
        name: name.to_string(),
        expected_len: input.len(),
        meta: json!({ "mode": mode_name(mode), "hint": hint_name(hint), "level": level, "reference_encoding": true }),
        payload,
        decoded: Some(input),
    }
}

/// A valid payload made by hand, which the reference encoder does not write for its input
/// (with `hint` and level 8 it writes another payload; checkers re-encode with those).
fn made(name: &str, payload: Vec<u8>, decoded: Vec<u8>, hint: Option<PhysicalType>, mode: Mode) -> Vector {
    assert_eq!(mode_of(&payload), Ok(mode), "{name}");
    assert_eq!(decompress(&payload, decoded.len()).as_ref(), Ok(&decoded), "{name}");
    let level = tparquet_core::DEFAULT_LEVEL;
    let reference = compress(&decoded, &Options { level, hint, decimal: true }).expect("compress");
    assert_ne!(reference, payload, "{name}: the encoder writes this payload");
    Vector {
        name: name.to_string(),
        expected_len: decoded.len(),
        meta: json!({ "mode": mode_name(mode), "hint": hint_name(hint), "level": level, "reference_encoding": false }),
        payload,
        decoded: Some(decoded),
    }
}

fn invalid(name: &str, payload: Vec<u8>, expected_len: usize) -> Vector {
    let err = decompress(&payload, expected_len).expect_err(name);
    Vector { name: name.to_string(), payload, expected_len, decoded: None, meta: json!({ "error": err.to_string() }) }
}

fn varint(mut v: u64, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// `v` as a minimal varint
fn minimal(v: u64) -> Vec<u8> {
    let mut out = vec![];
    varint(v, &mut out);
    out
}

/// `v` as a varint of exactly `len` bytes: the minimal form padded with continuation bytes
/// that carry zero bits (non-minimal when `len` is more than the minimal length).
fn varint_of_len(v: u64, len: usize) -> Vec<u8> {
    let mut out = minimal(v);
    assert!(out.len() <= len, "{len} bytes are too few for {v}");
    while out.len() < len {
        // the last byte is below 0x80: this sets its continuation bit
        *out.last_mut().expect("a varint has a byte") += 0x80;
        out.push(0);
    }
    out
}

/// A DECIMAL payload whose exceptions are at `positions`, with its exception count and
/// position deltas rewritten by `count` and `position` (index, delta), which return the
/// varint bytes.
fn rewrite_decimal(
    dec: &[u8],
    positions: &[usize],
    count: impl Fn(u64) -> Vec<u8>,
    position: impl Fn(usize, u64) -> Vec<u8>,
) -> Vec<u8> {
    let deltas: Vec<u64> =
        (0..positions.len()).map(|j| (positions[j] - if j == 0 { 0 } else { positions[j - 1] + 1 }) as u64).collect();
    let mut minimal_form = minimal(positions.len() as u64);
    deltas.iter().for_each(|&d| minimal_form.extend(minimal(d)));
    let body = 4 + minimal_form.len();
    assert_eq!(dec[4..body], minimal_form, "the exceptions of the payload");
    let mut out = dec[..4].to_vec();
    out.extend(count(positions.len() as u64));
    for (i, &d) in deltas.iter().enumerate() {
        out.extend(position(i, d));
    }
    out.extend_from_slice(&dec[body..]);
    out
}

pub fn all() -> Vec<Vector> {
    let mut rng = Rng(0x5043_0002);
    let n = 2000;
    let smooth_i32: Vec<i32> = (0..n).map(|i| 1000 + i * 3 + (rng.below(5) as i32)).collect();
    let smooth_i64: Vec<i64> = (0..n as i64).map(|i| 1_735_689_600_000 + i * 250 + rng.below(100) as i64).collect();
    let floats: Vec<f32> = (0..n).map(|i| (i as f32).sin() * 1000.0).collect();
    let doubles: Vec<f64> = (0..n).map(|i| (i as f64).sqrt() * std::f64::consts::E).collect();
    let prices: Vec<f64> = (0..n).map(|_| (6_000_000 + rng.below(20_000)) as f64 / 100.0).collect();
    let fprices: Vec<f32> = (0..n).map(|_| (100_000 + rng.below(40_000)) as f32 / 100.0).collect();
    let mut with_exc = prices.clone();
    let exceptions: Vec<usize> = (3..with_exc.len()).step_by(200).collect();
    for &i in &exceptions {
        with_exc[i] = f64::from_bits(0x7ff8_0000_0000_0000 + i as u64);
    }
    let random: Vec<u8> = (0..4096).map(|_| rng.next() as u8).collect();
    let text: Vec<u8> = (0..300).flat_map(|i| format!("row {i}: hello pcodec\n").into_bytes()).collect();

    // pcodec files without chunks, and a Zstandard frame without content: no numbers, no bytes
    let no_numbers = pco::standalone::simple_compress::<i64>(&[], &pco::ChunkConfig::default()).expect("pco");
    let no_values = [&[1, Mode::Pco as u8, PhysicalType::Int64 as u8][..], &no_numbers].concat();
    // DOUBLE, exponent 2, no exceptions
    let no_decimals = [&[1, Mode::Decimal as u8, PhysicalType::Double as u8, 2, 0][..], &no_numbers].concat();
    let empty_frame = [&[1, Mode::Zstd as u8][..], &zstd::bulk::compress(&[], 3).expect("zstd")].concat();
    let mut v = vec![
        valid("raw_empty", vec![], Some(PhysicalType::Int64), 8, Mode::Raw),
        valid("raw_random", random.clone(), Some(PhysicalType::Int64), 8, Mode::Raw),
        valid("raw_random_no_hint", random[..100].to_vec(), None, 8, Mode::Raw),
        valid("pco_int32", le(&smooth_i32, i32::to_le_bytes), Some(PhysicalType::Int32), 8, Mode::Pco),
        valid("pco_int64", le(&smooth_i64, i64::to_le_bytes), Some(PhysicalType::Int64), 8, Mode::Pco),
        valid("pco_int64_level0", le(&smooth_i64, i64::to_le_bytes), Some(PhysicalType::Int64), 0, Mode::Pco),
        valid("pco_int64_level12", le(&smooth_i64, i64::to_le_bytes), Some(PhysicalType::Int64), 12, Mode::Pco),
        valid("pco_float", le(&floats, f32::to_le_bytes), Some(PhysicalType::Float), 8, Mode::Pco),
        valid("pco_double", le(&doubles, f64::to_le_bytes), Some(PhysicalType::Double), 8, Mode::Pco),
        valid("decimal_double", le(&prices, f64::to_le_bytes), Some(PhysicalType::Double), 8, Mode::Decimal),
        valid("decimal_float", le(&fprices, f32::to_le_bytes), Some(PhysicalType::Float), 8, Mode::Decimal),
        valid("decimal_exceptions", le(&with_exc, f64::to_le_bytes), Some(PhysicalType::Double), 8, Mode::Decimal),
        valid("zstd_no_hint", text.clone(), None, 8, Mode::Zstd),
        valid("zstd_width_mismatch", text[..text.len() / 8 * 8 - 5].to_vec(), Some(PhysicalType::Int64), 8, Mode::Zstd),
    ];
    v.extend([
        made("pco_no_values", no_values, vec![], Some(PhysicalType::Int64), Mode::Pco),
        made("decimal_no_values", no_decimals, vec![], Some(PhysicalType::Double), Mode::Decimal),
        made("zstd_empty_frame", empty_frame, vec![], None, Mode::Zstd),
    ]);
    let get = |v: &[Vector], name: &str| v.iter().find(|x| x.name == name).expect("vector").payload.clone();
    let pco = get(&v, "pco_int64");
    let pco_len = smooth_i64.len() * 8;
    let dec = get(&v, "decimal_exceptions");
    let dec_len = with_exc.len() * 8;
    let zstd = get(&v, "zstd_no_hint");
    let dec_out = v.iter().find(|x| x.name == "decimal_exceptions").expect("vector").decoded.clone().expect("valid");

    let with_byte = |p: &[u8], i: usize, b: u8| {
        let mut p = p.to_vec();
        p[i] = b;
        p
    };
    // the first exception position of `decimal_exceptions`, moved past the end
    // (header: version, mode, dtype, exponent, then k and the first position, one byte each)
    assert!(dec[4] < 0x80 && dec[5] < 0x80, "one-byte varints");
    let first = 5;
    let mut past_end = dec[..first].to_vec();
    varint(with_exc.len() as u64, &mut past_end);
    past_end.extend_from_slice(&dec[first + 1..]);
    let mut many = vec![1, 2, 5, 2];
    varint(1000, &mut many);
    let mut short_exc = vec![1, 2, 5, 2, 1, 0];
    short_exc.extend([0; 3]);
    let trailing = |p: &[u8]| [p, &[0, 0, 0][..]].concat();
    // every varint of `decimal_exceptions` in a longer form than needed: the count in the
    // longest allowed (10 bytes), each position 1 to 8 bytes longer than it needs, up to 10
    let overlong = rewrite_decimal(
        &dec,
        &exceptions,
        |k| varint_of_len(k, 10),
        |i, d| varint_of_len(d, (minimal(d).len() + 1 + i % 8).min(10)),
    );
    // the count in 11 bytes, and in 10 bytes whose last one carries more than the top bit
    let too_long = rewrite_decimal(
        &dec,
        &exceptions,
        |k| {
            let mut b = varint_of_len(k, 10);
            b[9] |= 0x80;
            b.push(0);
            b
        },
        |_, d| minimal(d),
    );
    let overflow = rewrite_decimal(
        &dec,
        &exceptions,
        |k| {
            let mut b = varint_of_len(k, 10);
            b[9] = 0x02;
            b
        },
        |_, d| minimal(d),
    );

    v.extend([
        invalid("empty_payload", vec![], 0),
        invalid("truncated_header", vec![1], 0),
        invalid("bad_version", vec![2, 0, 1, 2, 3, 4], 4),
        invalid("bad_mode", vec![1, 4, 1, 2, 3, 4], 4),
        invalid("raw_too_short", vec![1, 0, 1, 2, 3], 4),
        invalid("raw_too_long", vec![1, 0, 1, 2, 3, 4, 5], 4),
        invalid("pco_bad_dtype", with_byte(&pco, 2, 3), pco_len),
        invalid("pco_missing_dtype", vec![1, 1], 8),
        invalid("pco_fewer_values", pco.clone(), pco_len + 8),
        invalid("pco_more_values", pco.clone(), pco_len - 8),
        invalid("pco_not_whole_values", pco.clone(), pco_len - 3),
        invalid("pco_corrupt", pco[..pco.len() / 2].to_vec(), pco_len),
        invalid("decimal_bad_exponent", with_byte(&dec, 3, 19), dec_len),
        invalid("decimal_int_dtype", with_byte(&dec, 2, 2), dec_len),
        invalid("decimal_position_past_end", past_end, dec_len),
        invalid("decimal_too_many_exceptions", many, 16),
        invalid("decimal_truncated_exceptions", short_exc, 16),
        invalid("decimal_truncated_header", vec![1, 2, 5], 16),
        invalid("zstd_too_short", zstd.clone(), text.len() + 1),
        invalid("zstd_too_long", zstd.clone(), text.len() - 1),
        invalid("zstd_corrupt", with_byte(&zstd, 2, zstd[2] ^ 0xff), text.len()),
        invalid("zstd_empty_body", vec![1, 3], 0),
        invalid("pco_trailing_bytes", trailing(&pco), pco_len),
        invalid("decimal_trailing_bytes", trailing(&dec), dec_len),
        invalid("varint_too_long", too_long, dec_len),
        invalid("varint_overflow", overflow, dec_len),
    ]);
    v.push(made("varint_overlong", overlong, dec_out, Some(PhysicalType::Double), Mode::Decimal));
    v
}
