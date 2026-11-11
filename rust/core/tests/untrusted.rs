//! Payloads and sizes as an attacker chooses them (spec section 7): the decoder returns an
//! error, never panics, and never accepts what the spec does not allow.

mod common;
use common::*;
use tparquet_core::{Error, PhysicalType, decompress};

#[test]
fn an_unallocatable_expected_size_is_an_error_not_a_panic() {
    // a page header can claim any size; a payload that cannot produce it is refused before the
    // size is allocated (memory.rs: an allocation that fails is Error::AllocationFailed)
    let pco = pco_payload(PhysicalType::Int64, &i64s(&[1, 2, 3]));
    for n in [usize::MAX, usize::MAX - 1, isize::MAX as usize + 1, isize::MAX as usize, 1 << 62] {
        assert_eq!(decompress(&[1, 0], n), Err(Error::SizeMismatch { expected: n, actual: 0 }), "{n}");
        assert_eq!(decompress(&[1, 0, 7], n), Err(Error::SizeMismatch { expected: n, actual: 1 }), "{n}");
        let pco_err = if n % 8 == 0 {
            Error::SizeMismatch { expected: n, actual: 24 }
        } else {
            Error::NotMultipleOfWidth { len: n, width: 8 }
        };
        assert_eq!(decompress(&pco, n), Err(pco_err), "{n}");
        assert_eq!(decompress(&[1, 3, 0x28, 0xB5, 0x2F, 0xFD], n), Err(Error::Truncated), "{n}");
    }
    assert_eq!(Error::AllocationFailed(7).to_string(), "pcodec payload: cannot allocate the expected size of 7 bytes");
    // a malformed header is reported before anything else
    assert_eq!(decompress(&[], usize::MAX), Err(Error::Truncated));
    assert_eq!(decompress(&[3, 0], usize::MAX), Err(Error::UnsupportedVersion(3)));
    assert_eq!(decompress(&[1, 9], usize::MAX), Err(Error::UnknownMode(9)));
}

/// a ZSTD payload: one frame without a content size, declaring a window of 2^`window_log`
/// bytes, holding one raw block of `data`
fn zstd_frame(window_log: u8, data: &[u8]) -> Vec<u8> {
    let mut p = vec![1, 3, 0x28, 0xB5, 0x2F, 0xFD, 0x00, (window_log - 10) << 3];
    let block = 1 | (data.len() as u32) << 3; // last block, raw
    p.extend(&block.to_le_bytes()[..3]);
    p.extend(data);
    p
}

#[test]
fn zstd_frames_decode_whatever_window_they_declare() {
    // RFC 8878 allows windows up to 2^41; one frame decoded straight into the page needs none
    for log in [10, 20, 27, 28, 30, 31] {
        assert_eq!(decompress(&zstd_frame(log, b"page"), 4).as_deref(), Ok(&b"page"[..]), "window 2^{log}");
    }
    // and the expected size still bounds the output
    let p = zstd_frame(31, b"page");
    assert_eq!(decompress(&p, 3), Err(Error::SizeMismatch { expected: 3, actual: 4 }));
    assert_eq!(decompress(&p, 5), Err(Error::SizeMismatch { expected: 5, actual: 4 }));
}

#[test]
fn a_zstd_body_needs_a_frame() {
    // spec 4.4: one frame, or several; zero frames is not a body, even for an empty page
    assert_eq!(decompress(&[1, 3], 0), Err(Error::Truncated));
    assert_eq!(decompress(&[1, 3], 5), Err(Error::Truncated));
    // a skippable frame is a frame (RFC 8878 3.1.2): an empty page
    let skippable = [1, 3, 0x50, 0x2A, 0x4D, 0x18, 2, 0, 0, 0, 0xAA, 0xBB];
    assert_eq!(decompress(&skippable, 0), Ok(vec![]));
    assert_eq!(decompress(&skippable, 1), Err(Error::SizeMismatch { expected: 1, actual: 0 }));
}

#[test]
fn nothing_may_follow_the_pcodec_file() {
    // spec 4: `pco` is one pcodec standalone file that runs to the end of the payload
    let src = smooth(PhysicalType::Double, 1_000, 5);
    let plain = pco_payload(PhysicalType::Double, &src);
    let dec = decimal_payload(5, 2, &[(3, 7)], &(0..1_000).collect::<Vec<_>>());
    for (p, n) in [(plain, src.len()), (dec, 8_000)] {
        assert!(decompress(&p, n).is_ok());
        for tail in [&[0u8][..], &[0xFF], &[0, 0, 0, 0], b"pco!"] {
            let mut q = p.clone();
            q.extend_from_slice(tail);
            assert_eq!(decompress(&q, n), Err(Error::TrailingBytes(tail.len())), "{tail:?}");
        }
        // a second pcodec file is trailing bytes too
        let mut twice = p.clone();
        let second = pco(&[1i64]);
        twice.extend(&second);
        assert_eq!(decompress(&twice, n), Err(Error::TrailingBytes(second.len())));
    }
    assert_eq!(Error::TrailingBytes(3).to_string(), "pcodec payload: 3 bytes after the pcodec file");
}

#[test]
fn varints_in_the_payload() {
    // the exception count k, in its shortest and overlong forms: 0 exceptions for 2 values
    let q = pco(&[15i64, 25]);
    for form in [&[0u8][..], &[0x80, 0x00], &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00]] {
        let mut p = vec![1, 2, 5, 1];
        p.extend_from_slice(form);
        p.extend(&q);
        assert_eq!(decompress(&p, 16), Ok(f64s(&[1.5, 2.5])), "{form:?}");
    }
    // eleven bytes: refused
    let mut p = vec![1, 2, 5, 1];
    p.extend([0x80; 10]);
    p.push(0);
    p.extend(&q);
    assert_eq!(decompress(&p, 16), Err(Error::Truncated));
    // 2^63 is a valid varint and too many exceptions; 2^64 is not a varint
    let mut p = vec![1, 2, 5, 1];
    p.extend([0x80; 9]);
    p.push(0x01);
    assert_eq!(decompress(&p, 16), Err(Error::BadException));
    let mut p = vec![1, 2, 5, 1];
    p.extend([0x80; 9]);
    p.push(0x02);
    assert_eq!(decompress(&p, 16), Err(Error::Truncated));
    // a position delta in an overlong form
    let mut p = vec![1, 2, 5, 1, 1, 0x81, 0x80, 0x00];
    p.extend(1.25f64.to_le_bytes());
    p.extend(&q);
    assert_eq!(decompress(&p, 16), Ok(f64s(&[1.5, 1.25])));
}

/// one valid payload per mode and dtype, with its decoded length
fn samples() -> Vec<(&'static str, Vec<u8>, usize)> {
    use tparquet_core::{Options, compress};
    let mut v = vec![("raw", vec![1, 0, 1, 2, 3, 4, 5], 5)];
    for t in TYPES {
        let src = smooth(t, 300, 11);
        let p = compress(&src, &Options { decimal: false, ..Options::with_hint(t) }).unwrap();
        v.push(("pco", p, src.len()));
    }
    let mut d: Vec<f64> = (0..300).map(|i| f64::from(i) / 20.0).collect();
    d[3] = f64::NAN;
    v.push((
        "decimal f64",
        decimal_payload(5, 2, &[(3, f64::NAN.to_bits())], &(0..300).map(|i| i * 5).collect::<Vec<_>>()),
        2_400,
    ));
    v.push((
        "decimal f32",
        decimal_payload(4, 1, &[(0, 0x7FC0_0001), (299, 0x8000_0000)], &(0..300).collect::<Vec<_>>()),
        1_200,
    ));
    let text: Vec<u8> = b"open,high,low,close;".iter().cycle().take(500).copied().collect();
    let mut z = vec![1, 3];
    z.extend(zstd::bulk::compress(&text, 3).unwrap());
    v.push(("zstd", z, 500));
    for (name, p, n) in &v {
        assert_eq!(decompress(p, *n).map(|o| o.len()), Ok(*n), "{name}");
    }
    v
}

#[test]
fn every_truncation_is_an_error() {
    for (name, p, n) in samples() {
        for cut in 0..p.len() {
            let r = decompress(&p[..cut], n);
            assert!(r.is_err(), "{name}: payload cut to {cut} of {} bytes decoded", p.len());
        }
    }
}

#[test]
fn every_corrupted_byte_is_survived() {
    // without a checksum a corrupt pcodec stream may decode to other numbers; it must never
    // panic, and anything it decodes has exactly the expected size
    for (name, p, n) in samples() {
        for i in 0..p.len() {
            for x in [0x01u8, 0x10, 0x80, 0xFF] {
                let mut q = p.clone();
                q[i] ^= x;
                for len in [n, n.saturating_sub(1), n + 1, 0] {
                    if let Ok(out) = decompress(&q, len) {
                        assert_eq!(out.len(), len, "{name}: byte {i} ^ {x:#x}");
                    }
                }
            }
            for b in [0x00, 0xFF] {
                let mut q = p.clone();
                q[i] = b;
                let _ = decompress(&q, n);
            }
        }
    }
}

#[test]
fn header_bytes_are_checked_whatever_follows() {
    for (name, p, n) in samples() {
        for v in [0u8, 2, 3, 0x7F, 0xFF] {
            let mut q = p.clone();
            q[0] = v;
            assert_eq!(decompress(&q, n), Err(Error::UnsupportedVersion(v)), "{name}");
        }
        for m in 4..=255u8 {
            let mut q = p.clone();
            q[1] = m;
            assert_eq!(decompress(&q, n), Err(Error::UnknownMode(m)), "{name}");
        }
    }
}

#[test]
fn a_zstd_frame_cannot_lie_about_its_size() {
    // frame content size field (8 bytes, FHD 0xE0 = FCS flag 3, single segment) claiming 2^40
    // bytes, then a raw block of 4: refused, and the claim costs nothing
    let mut p = vec![1, 3, 0x28, 0xB5, 0x2F, 0xFD, 0xE0];
    p.extend((1u64 << 40).to_le_bytes());
    p.extend([0x21, 0, 0]);
    p.extend(b"page");
    assert!(decompress(&p, 4).is_err());
    // a content size smaller than the blocks it holds
    let mut p = vec![1, 3, 0x28, 0xB5, 0x2F, 0xFD, 0x20, 2, 0x21, 0, 0];
    p.extend(b"page");
    assert!(decompress(&p, 4).is_err());
    assert!(decompress(&p, 2).is_err());
    // and an honest one
    let mut p = vec![1, 3, 0x28, 0xB5, 0x2F, 0xFD, 0x20, 4, 0x21, 0, 0];
    p.extend(b"page");
    assert_eq!(decompress(&p, 4).as_deref(), Ok(&b"page"[..]));
}

#[test]
fn a_pcodec_size_hint_is_not_trusted() {
    // pcodec's standalone header: "pco!", version 3, no uniform type (0), then the count hint as
    // 6 bits of (power - 1) and `power` bits, padded to a byte. Claim 2^64 - 1 values.
    let p = pco_payload(PhysicalType::Int64, &i64s(&[4, 5, 6]));
    assert_eq!(&p[3..9], [b'p', b'c', b'o', b'!', 3, 0]);
    let power = usize::from(p[9] & 0x3F) + 1;
    assert_eq!((power, p[9] >> 6), (2, 3), "the hint is 3");
    let mut q = p[..9].to_vec();
    q.extend([0xFF; 8]);
    q.push(0x3F);
    q.extend(&p[9 + (6 + power).div_ceil(8)..]);
    assert_eq!(decompress(&q, 24), Ok(i64s(&[4, 5, 6])));
    assert_eq!(decompress(&q, 32), Err(Error::SizeMismatch { expected: 32, actual: 24 }));
}

#[test]
fn a_zstd_body_is_refused_only_when_too_short_for_the_expected_size() {
    // a 10 byte body: a frame header (magic, descriptor, window 2^17) and one RLE block of
    // 128 KiB; 10 bytes hold at most 2 blocks (RFC 8878 3.1.1.2), 256 KiB
    let p = [1, 3, 0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x38, 0x03, 0x00, 0x10, 0x61];
    assert_eq!(decompress(&p, 128 << 10).as_deref(), Ok(&[0x61; 128 << 10][..]));
    assert_eq!(decompress(&p, 256 << 10), Err(Error::SizeMismatch { expected: 256 << 10, actual: 128 << 10 }));
    assert_eq!(decompress(&p, (256 << 10) + 1), Err(Error::Truncated));
}
