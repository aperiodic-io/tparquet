//! Unsigned LEB128, at most 10 bytes, below 2^64 (spec section 4).

use crate::Error;

pub(crate) fn put(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Reads one varint from the front of `src`, advancing it.
pub(crate) fn get(src: &mut &[u8]) -> Result<u64, Error> {
    let mut v: u64 = 0;
    for i in 0..10 {
        let (&b, rest) = src.split_first().ok_or(Error::Truncated)?;
        *src = rest;
        let part = u64::from(b & 0x7F);
        // the tenth byte may only carry the top bit of a u64
        if i == 9 && part > 1 {
            return Err(Error::Truncated);
        }
        v |= part << (7 * i);
        if b < 0x80 {
            return Ok(v);
        }
    }
    Err(Error::Truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(v: u64) -> Vec<u8> {
        let mut o = vec![];
        put(&mut o, v);
        o
    }

    #[test]
    fn known_encodings() {
        assert_eq!(enc(0), [0]);
        assert_eq!(enc(1), [1]);
        assert_eq!(enc(127), [0x7F]);
        assert_eq!(enc(128), [0x80, 0x01]);
        assert_eq!(enc(300), [0xAC, 0x02]);
        assert_eq!(enc(u64::MAX), [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
    }

    #[test]
    fn round_trip_and_advance() {
        for v in [0, 1, 127, 128, 255, 16_383, 16_384, 1 << 35, u64::MAX - 1, u64::MAX] {
            let mut b = enc(v);
            b.push(0xEE);
            let mut s = &b[..];
            assert_eq!(get(&mut s).unwrap(), v);
            assert_eq!(s, [0xEE]);
        }
    }

    #[test]
    fn rejects_overlong_and_truncated() {
        let mut s: &[u8] = &[];
        assert_eq!(get(&mut s), Err(Error::Truncated));
        let mut s: &[u8] = &[0x80];
        assert_eq!(get(&mut s), Err(Error::Truncated));
        // 2^64 does not fit
        let mut s: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02];
        assert_eq!(get(&mut s), Err(Error::Truncated));
        // eleven bytes
        let mut s: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x81, 0x00];
        assert_eq!(get(&mut s), Err(Error::Truncated));
    }

    #[test]
    fn lengths_at_every_7_bit_boundary() {
        for bits in 0..64u32 {
            for v in [(1u64 << bits) - 1, 1u64 << bits] {
                let b = enc(v);
                let want = (64 - v.leading_zeros()).div_ceil(7).max(1) as usize;
                assert_eq!(b.len(), want, "{v}");
                assert!(b[..b.len() - 1].iter().all(|x| x & 0x80 != 0), "{v}: continuation bits");
                assert!(b[b.len() - 1] < 0x80, "{v}: last byte");
                let mut s = &b[..];
                assert_eq!(get(&mut s), Ok(v));
                assert!(s.is_empty());
            }
        }
    }

    #[test]
    fn overlong_forms_up_to_ten_bytes_are_read() {
        // the spec bounds the length (10 bytes) and the value (< 2^64), not the form
        for len in 2..=10 {
            let mut b = vec![0x80; len - 1];
            b.push(0);
            let mut s = &b[..];
            assert_eq!(get(&mut s), Ok(0), "{len} bytes");
            assert!(s.is_empty());
            b[0] = 0x85;
            let mut s = &b[..];
            assert_eq!(get(&mut s), Ok(5), "{len} bytes");
        }
        // the tenth byte may be 0 or 1, never more, and never continues
        for (last, want) in
            [(0x00, Ok(0)), (0x01, Ok(1 << 63)), (0x02, Err(Error::Truncated)), (0x7F, Err(Error::Truncated))]
        {
            let mut b = vec![0x80; 9];
            b.push(last);
            let mut s = &b[..];
            assert_eq!(get(&mut s), want, "tenth byte {last:#x}");
        }
        for last in [0x80, 0x81] {
            let mut b = vec![0x80; 9];
            b.extend([last, 0x00]);
            let mut s = &b[..];
            assert_eq!(get(&mut s), Err(Error::Truncated), "tenth byte {last:#x} continues");
        }
    }

    #[test]
    fn a_varint_cut_anywhere_is_truncated() {
        let b = enc(u64::MAX);
        for cut in 0..b.len() {
            let mut s = &b[..cut];
            assert_eq!(get(&mut s), Err(Error::Truncated), "cut at {cut}");
        }
    }
}
