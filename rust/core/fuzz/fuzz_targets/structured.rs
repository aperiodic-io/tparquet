//! Payloads built field by field (spec section 4) from the fuzzer's choices, decoded against a
//! model of the spec written independently of the decoder: a well-formed DECIMAL or PCO payload
//! decodes to exactly the model's bytes; then one of its bytes is changed, cut or appended, and
//! the decoder must still return Ok (of the right size) or Err, never panic.
#![no_main]
use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use pco::ChunkConfig;
use pco::standalone::simple_compress;
use tparquet_core::decompress;

fn varint(mut v: u64, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// (payload, the bytes the spec says it decodes to)
fn build(u: &mut Unstructured) -> Result<(Vec<u8>, Vec<u8>)> {
    let n = u.int_in_range(0..=600usize)?;
    if u.arbitrary::<bool>()? {
        // DECIMAL: dtype 4 or 5, e <= 18, k exceptions at increasing positions
        let double = u.arbitrary::<bool>()?;
        let width = if double { 8 } else { 4 };
        let e: u8 = u.int_in_range(0..=18)?;
        let q: Vec<i64> = (0..n).map(|_| u.arbitrary()).collect::<Result<_>>()?;
        let mut positions = vec![];
        for i in 0..n as u64 {
            if u.ratio(1, 16)? {
                positions.push(i);
            }
        }
        let p10: f64 = format!("1e{e}").parse().unwrap();
        let mut want: Vec<u8> = q
            .iter()
            .flat_map(|q| {
                let x = *q as f64 / p10;
                if double { x.to_le_bytes().to_vec() } else { (x as f32).to_le_bytes().to_vec() }
            })
            .collect();
        let mut p = vec![1, 2, if double { 5 } else { 4 }, e];
        varint(positions.len() as u64, &mut p);
        let mut next = 0;
        for &pos in &positions {
            varint(pos - next, &mut p);
            next = pos + 1;
        }
        for &pos in &positions {
            let bits: [u8; 8] = u.arbitrary()?;
            p.extend_from_slice(&bits[..width]);
            let at = pos as usize * width;
            want[at..at + width].copy_from_slice(&bits[..width]);
        }
        p.extend(simple_compress(&q, &ChunkConfig::default()).unwrap());
        Ok((p, want))
    } else {
        // PCO: any bits of any dtype
        let dtype = *u.choose(&[1u8, 2, 4, 5])?;
        let width = if dtype == 1 || dtype == 4 { 4 } else { 8 };
        let raw: Vec<u8> = (0..n * width).map(|_| u.arbitrary()).collect::<Result<_>>()?;
        let level = u.int_in_range(0..=12usize)?;
        let cfg = ChunkConfig::default().with_compression_level(level);
        let file = match dtype {
            1 => simple_compress(
                &raw.chunks(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect::<Vec<_>>(),
                &cfg,
            ),
            2 => simple_compress(
                &raw.chunks(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect::<Vec<_>>(),
                &cfg,
            ),
            4 => simple_compress(
                &raw.chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect::<Vec<_>>(),
                &cfg,
            ),
            _ => simple_compress(
                &raw.chunks(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect::<Vec<_>>(),
                &cfg,
            ),
        }
        .unwrap();
        let mut p = vec![1, 1, dtype];
        p.extend(file);
        Ok((p, raw))
    }
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok((p, want)) = build(&mut u) else { return };
    assert_eq!(decompress(&p, want.len()).expect("a well-formed payload"), want);
    // a wrong size is refused
    assert!(decompress(&p, want.len() + 1).is_err());
    // then damage it
    let mut q = p.clone();
    match u.int_in_range(0..=3u8).unwrap_or(0) {
        0 => {
            let i = u.choose_index(q.len()).unwrap_or(0);
            q[i] ^= u.arbitrary::<u8>().unwrap_or(1).max(1);
        }
        1 => q.truncate(u.choose_index(q.len()).unwrap_or(0)),
        2 => q.push(u.arbitrary().unwrap_or(0)),
        _ => {
            let i = u.choose_index(q.len()).unwrap_or(0);
            q.insert(i, u.arbitrary().unwrap_or(0));
        }
    }
    if let Ok(out) = decompress(&q, want.len()) {
        assert_eq!(out.len(), want.len());
    }
});
