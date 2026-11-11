//! Helpers shared by the integration tests: PLAIN bytes, deterministic data, payload builders.
#![allow(dead_code)] // every test binary uses a different subset

use pco::ChunkConfig;
use pco::standalone::simple_compress;
use tparquet_core::PhysicalType;

pub const TYPES: [PhysicalType; 4] =
    [PhysicalType::Int32, PhysicalType::Int64, PhysicalType::Float, PhysicalType::Double];

pub fn le<T: Copy, const W: usize>(vals: &[T], f: fn(T) -> [u8; W]) -> Vec<u8> {
    vals.iter().flat_map(|v| f(*v)).collect()
}

pub fn i32s(v: &[i32]) -> Vec<u8> {
    le(v, i32::to_le_bytes)
}

pub fn i64s(v: &[i64]) -> Vec<u8> {
    le(v, i64::to_le_bytes)
}

pub fn f32s(v: &[f32]) -> Vec<u8> {
    le(v, f32::to_le_bytes)
}

pub fn f64s(v: &[f64]) -> Vec<u8> {
    le(v, f64::to_le_bytes)
}

pub fn bits32(b: &[u8]) -> Vec<u32> {
    b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
}

pub fn bits64(b: &[u8]) -> Vec<u64> {
    b.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect()
}

/// A deterministic xorshift64 stream, so tests need no RNG crate.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// `n` values of type `t` as PLAIN bytes, smooth enough that pcodec beats RAW.
pub fn smooth(t: PhysicalType, n: usize, seed: u64) -> Vec<u8> {
    let mut r = Rng(seed | 1);
    match t {
        PhysicalType::Int32 => i32s(&(0..n).map(|i| 1_000 + i as i32 * 3 + r.below(5) as i32).collect::<Vec<_>>()),
        PhysicalType::Int64 => {
            i64s(&(0..n).map(|i| 1_700_000_000_000 + i as i64 * 250 + r.below(100) as i64).collect::<Vec<_>>())
        }
        PhysicalType::Float => f32s(&(0..n).map(|i| (i as f32 * 0.37).sin() * 100.0).collect::<Vec<_>>()),
        PhysicalType::Double => f64s(&(0..n).map(|i| (i as f64 * 0.37).sin() * 100.0).collect::<Vec<_>>()),
    }
}

/// the pcodec standalone file of `v`, at the default level
pub fn pco<T: pco::data_types::Number>(v: &[T]) -> Vec<u8> {
    simple_compress(v, &ChunkConfig::default()).unwrap()
}

/// the PCO payload (spec 4.2) of PLAIN bytes `src` of type `t`, built without the encoder
pub fn pco_payload(t: PhysicalType, src: &[u8]) -> Vec<u8> {
    let mut p = vec![1, 1, t as u8];
    p.extend(match t {
        PhysicalType::Int32 => pco(&src.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c)).collect::<Vec<_>>()),
        PhysicalType::Int64 => pco(&src.as_chunks::<8>().0.iter().map(|c| i64::from_le_bytes(*c)).collect::<Vec<_>>()),
        PhysicalType::Float => pco(&src.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect::<Vec<_>>()),
        PhysicalType::Double => pco(&src.as_chunks::<8>().0.iter().map(|c| f64::from_le_bytes(*c)).collect::<Vec<_>>()),
    });
    p
}

pub fn varint(mut v: u64, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A DECIMAL payload (spec 4.3) built field by field: exceptions as (position, bits) in
/// increasing position order, `q` the pcodec integers.
pub fn decimal_payload(dtype: u8, exponent: u8, exceptions: &[(u64, u64)], q: &[i64]) -> Vec<u8> {
    let width = if dtype == 4 { 4 } else { 8 };
    let mut p = vec![1, 2, dtype, exponent];
    varint(exceptions.len() as u64, &mut p);
    let mut next = 0;
    for &(pos, _) in exceptions {
        varint(pos - next, &mut p);
        next = pos + 1;
    }
    for &(_, bits) in exceptions {
        p.extend_from_slice(&bits.to_le_bytes()[..width]);
    }
    p.extend(pco(q));
    p
}

/// The spec's DECIMAL value (section 4.3) of `q` at exponent `e`, written out independently of
/// the crate: `f64(q) / P[e]`, rounded to f32 for FLOAT.
pub fn spec_double(q: i64, e: u8) -> f64 {
    let p: f64 = format!("1e{e}").parse().unwrap();
    q as f64 / p
}

pub fn spec_float(q: i64, e: u8) -> f32 {
    spec_double(q, e) as f32
}
