//! Compatibility: hand-checked payload bytes. Decoding them must never change (they may exist in
//! files); encoding them is pinned too, so an encoder change (or a pcodec upgrade) is noticed.

mod common;
use common::*;
use tparquet_core::{Mode, Options, PhysicalType, compress, decompress, mode_of};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// (name, payload hex, decoded bytes, mode)
fn vectors() -> Vec<(&'static str, Vec<u8>, Vec<u8>, Mode)> {
    let mut dec = (0..200).map(|i| i as f64 / 4.0).collect::<Vec<_>>();
    dec[7] = f64::from_bits(0x7FF8_0000_0000_0042);
    dec[150] = -0.0;
    vec![
        ("empty", unhex("0100"), vec![], Mode::Raw),
        ("raw", unhex("0100c0ffee"), vec![0xC0, 0xFF, 0xEE], Mode::Raw),
        (
            "pco_i32",
            unhex(concat!(
                "01010170636f21030004040401030f0000410100000010003033333318420000010000002001000000",
                "000a40226d646bffaa0a00"
            )),
            i32s(&(0..16).map(|i| i * 10).collect::<Vec<_>>()),
            Mode::Pco,
        ),
        (
            "pco_i64_extremes",
            unhex(concat!(
                "01010270636f210300050a0401042700001051020019000000000000000860feffffffffffffff0100",
                "0000000000000075f6038439abd451550500"
            )),
            i64s(&[i64::MIN, -1, 0, 1, i64::MAX].iter().cycle().take(40).copied().collect::<Vec<_>>()),
            Mode::Pco,
        ),
        (
            // dtype 5, e = 2, k = 2, d0 = 7, d1 = 0x8e 0x01 = 142 = 150 - 7 - 1, then the NaN
            // payload 0x7ff8000000000042 and -0.0, then the pcodec file ("pco!" ... 0x00)
            "decimal_f64_exceptions",
            unhex(concat!(
                "0102050202078e01420000000000f87f000000000000008070636f2103000732040104c7000091010000",
                "0000000010710380000000000000000020801f000000000000000800080000000000000002200080000000",
                "000000000000b81e85eb51b81e05f2d5140723f8601800"
            )),
            f64s(&dec),
            Mode::Decimal,
        ),
        (
            // dtype 4, e = 1, k = 0: 3.0, 3.1, ... 3.9 repeated
            "decimal_f32",
            unhex(concat!(
                "010204010070636f21030006190401046300001051020071ffffffffffffff07e0010000000000008000",
                "1e00000000000080e5170a23c58413440100"
            )),
            f32s(&(0..100).map(|i| (i % 10) as f32 / 10.0 + 3.0).collect::<Vec<_>>()),
            Mode::Decimal,
        ),
        (
            "zstd",
            unhex("010328b52ffd205a8500004874706172717565742001004eb41c01"),
            b"tparquet ".iter().cycle().take(90).copied().collect(),
            Mode::Zstd,
        ),
    ]
}

#[test]
fn pinned_payloads_decode_forever() {
    for (name, payload, want, mode) in vectors() {
        assert_eq!(mode_of(&payload), Ok(mode), "{name}");
        assert_eq!(decompress(&payload, want.len()).as_deref(), Ok(&want[..]), "{name}");
    }
}

#[test]
fn pinned_payloads_are_what_the_encoder_writes() {
    let hint = |t| Options::with_hint(t);
    for (name, payload, input, _) in vectors() {
        let opts = match name {
            "pco_i32" => hint(PhysicalType::Int32),
            "pco_i64_extremes" => hint(PhysicalType::Int64),
            "decimal_f64_exceptions" => hint(PhysicalType::Double),
            "decimal_f32" => hint(PhysicalType::Float),
            "raw" | "empty" | "zstd" => Options::default(),
            other => unreachable!("{other}"),
        };
        assert_eq!(compress(&input, &opts).unwrap(), payload, "{name}");
    }
}

#[test]
fn pinned_decimal_fields() {
    // the header of the DECIMAL vector, field by field (spec 4.3)
    let (_, p, _, _) = vectors().into_iter().find(|v| v.0 == "decimal_f64_exceptions").unwrap();
    assert_eq!(&p[..4], [1, 2, 5, 2], "version, mode, dtype, exponent");
    assert_eq!(p[4], 2, "k");
    assert_eq!(&p[5..8], [7, 0x8E, 0x01], "position deltas");
    assert_eq!(u64::from_le_bytes(p[8..16].try_into().unwrap()), 0x7FF8_0000_0000_0042);
    assert_eq!(u64::from_le_bytes(p[16..24].try_into().unwrap()), (-0.0f64).to_bits());
    assert_eq!(&p[24..28], b"pco!");
    assert_eq!(p.last(), Some(&0), "pcodec termination byte");
}
