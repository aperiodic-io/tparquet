//! Decimal-looking floats, the inputs DECIMAL mode is for: short decimals at an exponent the
//! fuzzer picks, with arbitrary bit patterns (NaN payloads, -0.0, subnormals, huge values) mixed
//! in at any rate. Always lossless, never over RAW + 2, and DECIMAL only for float hints.
#![no_main]
use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use tparquet_core::{Mode, Options, PhysicalType, compress, decompress, mode_of};

fn input(u: &mut Unstructured) -> Result<(Vec<u8>, bool)> {
    let double = u.arbitrary::<bool>()?;
    let e: i32 = u.int_in_range(0..=18)?;
    let rate: u8 = u.arbitrary()?;
    let range: i64 = 10i64.pow(u.int_in_range(0..=17)?);
    let n = u.int_in_range(0..=5000usize)?;
    let mut src = Vec::with_capacity(n * 8);
    for _ in 0..n {
        let odd = u.arbitrary::<u8>()? < rate;
        let q = u.int_in_range(-range..=range)?;
        let bits: u64 = if odd { u.arbitrary()? } else { 0 };
        if double {
            let x = if odd { f64::from_bits(bits) } else { q as f64 / 10f64.powi(e) };
            src.extend(x.to_le_bytes());
        } else {
            let x = if odd { f32::from_bits(bits as u32) } else { (q as f64 / 10f64.powi(e)) as f32 };
            src.extend(x.to_le_bytes());
        }
    }
    Ok((src, double))
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok((src, double)) = input(&mut u) else { return };
    let level = u.int_in_range(0..=12u8).unwrap_or(8);
    let float = if double { PhysicalType::Double } else { PhysicalType::Float };
    for hint in [Some(float), Some(PhysicalType::Int64), None] {
        let p = compress(&src, &Options { level, hint, decimal: true }).expect("valid options");
        assert!(p.len() <= src.len() + 2);
        assert_eq!(decompress(&p, src.len()).expect("own payload"), src);
        if mode_of(&p) == Ok(Mode::Decimal) {
            assert_eq!(hint, Some(float));
            assert_eq!(p[2], float as u8);
        }
    }
});
