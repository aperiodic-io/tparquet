//! The decoder against hand-built payloads (spec/FORMAT.md section 4), independent of the
//! encoder, and its rejection of every malformed input the spec names.

use pco::ChunkConfig;
use pco::standalone::simple_compress;
use tparquet_core::{Error, Mode, PhysicalType, decompress, decompress_append, decompress_into, mode_of};

fn pco_i64(v: &[i64]) -> Vec<u8> {
    simple_compress(v, &ChunkConfig::default()).unwrap()
}

/// DECIMAL body: dtype, exponent, k, position deltas, exception bits, pco of q
fn decimal(dtype: u8, exp: u8, deltas: &[u8], exceptions: &[u8], q: &[i64]) -> Vec<u8> {
    let mut p = vec![1, 2, dtype, exp, deltas.len() as u8];
    p.extend_from_slice(deltas);
    p.extend_from_slice(exceptions);
    p.extend(pco_i64(q));
    p
}

fn bits64(b: &[u8]) -> Vec<u64> {
    b.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect()
}

#[test]
fn raw_vector() {
    assert_eq!(decompress(&[1, 0, 9, 8, 7], 3).unwrap(), vec![9, 8, 7]);
    assert_eq!(mode_of(&[1, 0]).unwrap(), Mode::Raw);
}

#[test]
fn pco_vector_each_dtype() {
    let i: Vec<i32> = vec![-3, 0, 7, i32::MAX];
    let mut p = vec![1, 1, 1];
    p.extend(simple_compress(&i, &ChunkConfig::default()).unwrap());
    let want: Vec<u8> = i.iter().flat_map(|x| x.to_le_bytes()).collect();
    assert_eq!(decompress(&p, 16).unwrap(), want);
    assert_eq!(mode_of(&p).unwrap(), Mode::Pco);

    let l: Vec<i64> = vec![i64::MIN, 5];
    let mut p = vec![1, 1, 2];
    p.extend(pco_i64(&l));
    assert_eq!(decompress(&p, 16).unwrap(), l.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>());

    let f: Vec<f32> = vec![1.5, -0.0, f32::NAN];
    let mut p = vec![1, 1, 4];
    p.extend(simple_compress(&f, &ChunkConfig::default()).unwrap());
    assert_eq!(decompress(&p, 12).unwrap(), f.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>());

    let d: Vec<f64> = vec![2.25, f64::INFINITY];
    let mut p = vec![1, 1, 5];
    p.extend(simple_compress(&d, &ChunkConfig::default()).unwrap());
    assert_eq!(decompress(&p, 16).unwrap(), d.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>());
}

#[test]
fn decimal_vector_double_with_exceptions() {
    // values [1.23, NaN-payload, -0.05, -0.0]; exceptions at 1 and 3: d0 = 1, d1 = 3 - 1 - 1 = 1
    let nan = f64::from_bits(0x7FF8_0000_0000_0042);
    let mut exc = nan.to_le_bytes().to_vec();
    exc.extend((-0.0f64).to_le_bytes());
    let p = decimal(5, 2, &[1, 1], &exc, &[123, 123, -5, -5]);
    let out = decompress(&p, 32).unwrap();
    assert_eq!(bits64(&out), vec![1.23f64.to_bits(), nan.to_bits(), (-0.05f64).to_bits(), (-0.0f64).to_bits()]);
    assert_eq!(mode_of(&p).unwrap(), Mode::Decimal);
}

#[test]
fn decimal_vector_float_rounds_through_f64() {
    // f32(f64(q) / 10^e): 0.1f32 is not 0.1, but this is the value the spec defines
    let p = decimal(4, 1, &[], &[], &[1, 25, -3]);
    let out = decompress(&p, 12).unwrap();
    let got: Vec<u32> = out.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
    let want: Vec<u32> = [1i64, 25, -3].iter().map(|q| ((*q as f64 / 10.0) as f32).to_bits()).collect();
    assert_eq!(got, want);
}

#[test]
fn decimal_vector_largest_exponent() {
    let p = decimal(5, 18, &[], &[], &[1, 7]);
    let out = decompress(&p, 16).unwrap();
    assert_eq!(bits64(&out), vec![1e-18f64.to_bits(), (7.0 / 1e18f64).to_bits()]);
}

#[test]
fn zstd_vector() {
    let mut p = vec![1, 3];
    p.extend(zstd::bulk::compress(b"hello hello hello", 1).unwrap());
    assert_eq!(decompress(&p, 17).unwrap(), b"hello hello hello");
    // two concatenated frames, as some writers produce
    let mut two = p.clone();
    two.extend(zstd::bulk::compress(b"!", 1).unwrap());
    assert_eq!(decompress(&two, 18).unwrap(), b"hello hello hello!");
}

#[test]
fn rejects_truncated_header() {
    assert!(matches!(decompress(&[], 0), Err(Error::Truncated)));
    assert!(matches!(decompress(&[1], 0), Err(Error::Truncated)));
    assert!(matches!(decompress(&[1, 1], 8), Err(Error::Truncated)));
    assert!(matches!(decompress(&[1, 2, 5], 8), Err(Error::Truncated)));
    assert!(matches!(decompress(&[1, 2, 5, 2], 8), Err(Error::Truncated)));
    assert!(matches!(mode_of(&[1]), Err(Error::Truncated)));
}

#[test]
fn rejects_unknown_version_mode_dtype() {
    assert!(matches!(decompress(&[0, 0], 0), Err(Error::UnsupportedVersion(0))));
    assert!(matches!(decompress(&[2, 0], 0), Err(Error::UnsupportedVersion(2))));
    assert!(matches!(mode_of(&[2, 0]), Err(Error::UnsupportedVersion(2))));
    assert!(matches!(decompress(&[1, 4], 0), Err(Error::UnknownMode(4))));
    assert!(matches!(mode_of(&[1, 255]), Err(Error::UnknownMode(255))));
    for bad in [0u8, 3, 6, 7, 255] {
        assert!(matches!(decompress(&[1, 1, bad, 0], 8), Err(Error::UnknownDtype(b)) if b == bad));
    }
    // DECIMAL is for floats only
    for bad in [1u8, 2] {
        let p = decimal(bad, 2, &[], &[], &[1]);
        assert!(matches!(decompress(&p, 8), Err(Error::UnknownDtype(b)) if b == bad));
    }
}

#[test]
fn rejects_size_mismatch_in_every_mode() {
    // `actual` is exact where the decoder can know it, and expected + one value when the payload
    // holds more than fits (the decoder stops there)
    let mm = |expected, actual| Err(Error::SizeMismatch { expected, actual });
    let width = |len, width| Err(Error::NotMultipleOfWidth { len, width });
    assert_eq!(decompress(&[1, 0, 1, 2], 3), mm(3, 2));
    assert_eq!(decompress(&[1, 0, 1, 2], 1), mm(1, 2));
    let mut p = vec![1, 1, 2];
    p.extend(pco_i64(&[1, 2, 3]));
    assert_eq!(decompress(&p, 16), mm(16, 24), "more values than expected");
    assert_eq!(decompress(&p, 32), mm(32, 24), "fewer values than expected");
    assert_eq!(decompress(&p, 25), width(25, 8));
    let mut p4 = vec![1, 1, 1];
    p4.extend(simple_compress(&[1i32, 2, 3], &ChunkConfig::default()).unwrap());
    assert_eq!(decompress(&p4, 8), mm(8, 12));
    assert_eq!(decompress(&p4, 16), mm(16, 12));
    assert_eq!(decompress(&p4, 14), width(14, 4));
    let d = decimal(5, 0, &[], &[], &[1, 2]);
    assert_eq!(decompress(&d, 8), mm(8, 16));
    assert_eq!(decompress(&d, 24), mm(24, 16));
    assert_eq!(decompress(&d, 12), width(12, 8));
    let d4 = decimal(4, 0, &[], &[], &[1, 2, 3]);
    assert_eq!(decompress(&d4, 8), mm(8, 12));
    assert_eq!(decompress(&d4, 16), mm(16, 12));
    assert_eq!(decompress(&d4, 10), width(10, 4));
    let mut z = vec![1, 3];
    z.extend(zstd::bulk::compress(&[7u8; 100], 1).unwrap());
    assert_eq!(decompress(&z, 99), mm(99, 100), "zstd output larger than expected");
    assert_eq!(decompress(&z, 101), mm(101, 100), "zstd output smaller than expected");
}

#[test]
fn decimal_exceptions_without_a_pco_stream_are_a_pco_error() {
    // one exception and its 8 bytes, then nothing
    let mut p = vec![1, 2, 5, 0, 1, 0];
    p.extend(1.5f64.to_le_bytes());
    assert!(matches!(decompress(&p, 8), Err(Error::Pco(_))));
}

#[test]
fn rejects_bad_decimal_fields() {
    assert!(matches!(decompress(&decimal(5, 19, &[], &[], &[1]), 8), Err(Error::BadExponent(19))));
    // k = 2 exceptions but only one value
    let p = decimal(5, 0, &[0, 0], &[0; 16], &[1]);
    assert!(matches!(decompress(&p, 8), Err(Error::BadException)));
    // position 1 is past the end of a single value
    let p = decimal(5, 0, &[1], &[0; 8], &[1]);
    assert!(matches!(decompress(&p, 8), Err(Error::BadException)));
    // exception bytes cut short
    let mut p = vec![1, 2, 5, 0, 1, 0, 1, 2, 3];
    assert!(matches!(decompress(&p, 8), Err(Error::Truncated)));
    // an endless varint
    p = vec![1, 2, 5, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01];
    assert!(matches!(decompress(&p, 8), Err(Error::Truncated)));
}

#[test]
fn rejects_corrupt_inner_streams() {
    let mut p = vec![1, 1, 2];
    p.extend([0xDE, 0xAD, 0xBE, 0xEF, 0, 1, 2, 3]);
    assert!(matches!(decompress(&p, 8), Err(Error::Pco(_))));
    let z = [1, 3, 0x28, 0xB5, 0x2F, 0xFD, 0xFF, 0xFF];
    assert!(matches!(decompress(&z, 8), Err(Error::Zstd(_))));
}

#[test]
fn decompress_into_uses_the_buffer_length() {
    let mut dst = [0u8; 3];
    decompress_into(&[1, 0, 4, 5, 6], &mut dst).unwrap();
    assert_eq!(dst, [4, 5, 6]);
    let mut short = [0u8; 2];
    assert!(matches!(
        decompress_into(&[1, 0, 4, 5, 6], &mut short),
        Err(Error::SizeMismatch { expected: 2, actual: 3 })
    ));
}

#[test]
fn errors_display_and_types_map() {
    assert_eq!(Error::UnknownMode(9).to_string(), "pcodec payload: unknown mode 9");
    assert_eq!(
        Error::NotMultipleOfWidth { len: 25, width: 8 }.to_string(),
        "pcodec payload: expected size 25 is not a multiple of the value width 8"
    );
    assert_eq!(
        Error::SizeMismatch { expected: 2, actual: 3 }.to_string(),
        "pcodec payload: decodes to 3 bytes, expected 2"
    );
    assert_eq!(PhysicalType::from_parquet(1), Some(PhysicalType::Int32));
    assert_eq!(PhysicalType::from_parquet(2), Some(PhysicalType::Int64));
    assert_eq!(PhysicalType::from_parquet(4), Some(PhysicalType::Float));
    assert_eq!(PhysicalType::from_parquet(5), Some(PhysicalType::Double));
    for other in [-1, 0, 3, 6, 7, 8] {
        assert_eq!(PhysicalType::from_parquet(other), None);
    }
    assert_eq!(
        [PhysicalType::Int32, PhysicalType::Int64, PhysicalType::Float, PhysicalType::Double].map(|t| t.width()),
        [4, 8, 4, 8]
    );
    assert_eq!(
        [PhysicalType::Int32, PhysicalType::Int64, PhysicalType::Float, PhysicalType::Double].map(|t| t as u8),
        [1, 2, 4, 5]
    );
    assert_eq!(tparquet_core::CODEC_ID, 20547);
    assert_eq!(tparquet_core::KV_KEY, "parquet.pcodec.version");
    assert_eq!(tparquet_core::KV_VALUE, "1");
}

#[test]
fn decompress_append_appends_and_leaves_the_output_alone_on_an_error() {
    let mut p = decimal(5, 1, &[1], &f64::NAN.to_le_bytes(), &[15, 0, 25]);
    let want: Vec<u8> = [1.5, f64::NAN, 2.5].iter().flat_map(|x: &f64| x.to_le_bytes()).collect();
    let mut out = vec![7, 7];
    assert_eq!(decompress_append(&p, &mut out, 24), Ok(()));
    assert_eq!(out[..2], [7, 7]);
    assert_eq!(bits64(&out[2..]), bits64(&want));
    for len in [16, 32] {
        assert!(decompress_append(&p, &mut out, len).is_err(), "{len}");
        assert_eq!(out.len(), 26, "{len}");
    }
    p.truncate(p.len() - 1);
    assert!(decompress_append(&p, &mut out, 24).is_err());
    assert_eq!(out.len(), 26);
    assert_eq!(decompress_append(&[1, 0, 9], &mut out, 1), Ok(()));
    assert_eq!(out.len(), 27);
}
