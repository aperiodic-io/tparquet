//! The reference oracle for pcodec-java. It reads requests from stdin and writes responses to
//! stdout, each framed as a little-endian u32 length and that many bytes:
//!
//! request  := op:u8 body
//! response := status:u8 body      (status 0 ok, 1 error, 2 panic; body of an error is UTF-8)
//!
//! ops (all integers little-endian):
//!   1 compress     dtype:u8 config values        -> pco::standalone::simple_compress
//!   2 decompress   dtype:u8 bytes                -> n:u32 values, of simple_decompress::<T>
//!   3 meta         dtype:u8 config values        -> Debug of the chunk meta of the first chunk
//!   4 payload_enc  hint:u8 level:u8 decimal:u8 bytes -> tparquet_core::compress
//!   5 payload_dec  expected_len:u64 bytes        -> tparquet_core::decompress
//!   6 math         fn:u8 n:u32 f64s              -> u64s (0 log2, 1 cbrt, 2 10^x, 3 log10 as
//!                  bits; 4..7 `as u64/u32/u16/i64`; 8, 9 round of f64, f32; 10, 11 the bits
//!                  `as f64/f32`; 12 f16::from_f64)
//!   7 f16          fn:u8 n:u32 values            -> values (0 f64 bits -> f16 bits by from_f64,
//!                                                    1 f32 bits -> f16 bits by from_f32)
//!   8 digest       fn:u8 seed:u64 start:u64 count:u64 -> digest:u64 of fn over inputs
//!                  start..start+count (see `digest_value`; the Java side computes the same)
//!   9 all_compress dtype:u8 config len:u32 n_alpha:u32 alphabet start:u64 count:u64
//!                  -> digest:u64 of compress over sequences start..start+count of alphabet^len
//!  11 bench        dtype:u8 config iterations:u32 values -> compress_ns:u64 decompress_ns:u64
//!                  (the fastest of the iterations)
//!  13 trace        dtype:u8 config values -> n_dict:u32 dict:u64* compressed (only with the
//!                  `trace` feature: the dictionary Dict mode chose, in order)
//!  10 all_decode   dtype:u8 n_prefix:u32 prefix suffix_len:u32 start:u64 count:u64
//!                  -> digest:u64 of decompress over prefix ++ suffix, suffixes in order
//!
//! config := level:u32 mode:u8 mode_f64:f64 mode_k:u32 mode_base:u64 delta:u8 delta_order:u64
//!           paging:u8 paging_n:u32 paging_values:u64* enable_8_bit:u8
//! mode 0 Auto 1 Classic 2 TryFloatMult 3 TryFloatQuant 4 TryIntMult 5 TryDict;
//! delta 0 Auto 1 NoOp 2 TryConsecutive 3 TryLookback 4 TryConv1;
//! paging 0 EqualPagesUpTo(paging_values[0]) 1 Exact(paging_values).

use half::f16;
use pco::data_types::Number;
use pco::standalone::{FileCompressor, FileDecompressor, simple_compress, simple_decompress};
use pco::{ChunkConfig, DeltaSpec, ModeSpec, PagingSpec};
use std::io::{self, Read, Write};
use std::panic::{self, AssertUnwindSafe};

struct Cursor<'a> {
    b: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> &'a [u8] {
        let (a, rest) = self.b.split_at(n);
        self.b = rest;
        a
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn f64(&mut self) -> f64 {
        f64::from_bits(self.u64())
    }
    fn rest(&mut self) -> &'a [u8] {
        self.take(self.b.len())
    }
}

fn config(c: &mut Cursor) -> ChunkConfig {
    let level = c.u32() as usize;
    let mode = c.u8();
    let mode_f64 = c.f64();
    let mode_k = c.u32();
    let mode_base = c.u64();
    let delta = c.u8();
    let delta_order = c.u64() as usize;
    let paging = c.u8();
    let paging_n = c.u32() as usize;
    let paging_values: Vec<usize> = (0..paging_n).map(|_| c.u64() as usize).collect();
    let enable_8_bit = c.u8() != 0;
    let mode_spec = match mode {
        0 => ModeSpec::Auto,
        1 => ModeSpec::Classic,
        2 => ModeSpec::TryFloatMult(mode_f64),
        3 => ModeSpec::TryFloatQuant(mode_k),
        4 => ModeSpec::TryIntMult(mode_base),
        _ => ModeSpec::TryDict,
    };
    let delta_spec = match delta {
        0 => DeltaSpec::Auto,
        1 => DeltaSpec::NoOp,
        2 => DeltaSpec::TryConsecutive(delta_order),
        3 => DeltaSpec::TryLookback,
        _ => DeltaSpec::TryConv1(delta_order),
    };
    let paging_spec = match paging {
        0 => PagingSpec::EqualPagesUpTo(paging_values[0]),
        _ => PagingSpec::Exact(paging_values),
    };
    ChunkConfig::default()
        .with_compression_level(level)
        .with_mode_spec(mode_spec)
        .with_delta_spec(delta_spec)
        .with_paging_spec(paging_spec)
        .with_enable_8_bit(enable_8_bit)
}

trait Raw: Number {
    const W: usize;
    fn from_raw(b: &[u8]) -> Self;
    fn put(self, out: &mut Vec<u8>);
}

macro_rules! raw {
    ($t:ty, $w:expr, $from:expr, $to:expr) => {
        impl Raw for $t {
            const W: usize = $w;
            fn from_raw(b: &[u8]) -> Self {
                $from(b.try_into().unwrap())
            }
            fn put(self, out: &mut Vec<u8>) {
                out.extend_from_slice(&$to(self));
            }
        }
    };
}
raw!(u8, 1, u8::from_le_bytes, u8::to_le_bytes);
raw!(i8, 1, i8::from_le_bytes, i8::to_le_bytes);
raw!(u16, 2, u16::from_le_bytes, u16::to_le_bytes);
raw!(i16, 2, i16::from_le_bytes, i16::to_le_bytes);
raw!(u32, 4, u32::from_le_bytes, u32::to_le_bytes);
raw!(i32, 4, i32::from_le_bytes, i32::to_le_bytes);
raw!(u64, 8, u64::from_le_bytes, u64::to_le_bytes);
raw!(i64, 8, i64::from_le_bytes, i64::to_le_bytes);
raw!(f16, 2, |b| f16::from_bits(u16::from_le_bytes(b)), |x: f16| x.to_bits().to_le_bytes());
raw!(f32, 4, |b| f32::from_bits(u32::from_le_bytes(b)), |x: f32| x.to_bits().to_le_bytes());
raw!(f64, 8, |b| f64::from_bits(u64::from_le_bytes(b)), |x: f64| x.to_bits().to_le_bytes());

fn values<T: Raw>(b: &[u8]) -> Vec<T> {
    b.chunks_exact(T::W).map(T::from_raw).collect()
}

type Res = Result<Vec<u8>, String>;

fn compress<T: Raw>(cfg: &ChunkConfig, b: &[u8]) -> Res {
    simple_compress(&values::<T>(b), cfg).map_err(|e| e.to_string())
}

fn meta<T: Raw>(cfg: &ChunkConfig, b: &[u8]) -> Res {
    let v = values::<T>(b);
    let cc = FileCompressor::default().chunk_compressor(&v, cfg).map_err(|e| e.to_string())?;
    Ok(format!("{:?}", cc.meta()).into_bytes())
}

fn decompress<T: Raw>(b: &[u8]) -> Res {
    let v: Vec<T> = simple_decompress(b).map_err(|e| e.to_string())?;
    let mut out = (v.len() as u32).to_le_bytes().to_vec();
    for x in v {
        x.put(&mut out);
    }
    // the decoder's view of the end of the file: bytes after the footer
    let (fd, mut rest) = FileDecompressor::new(b).map_err(|e| e.to_string())?;
    while let pco::standalone::DecompressorItem::Chunk(mut cd) =
        fd.chunk_decompressor::<T, _>(rest).map_err(|e| e.to_string())?
    {
        let mut sink = vec![T::default(); cd.n()];
        cd.read(&mut sink).map_err(|e| e.to_string())?;
        rest = cd.into_src();
    }
    // after the termination byte
    let consumed = b.len() - rest.len() + 1;
    out.extend_from_slice(&(consumed as u32).to_le_bytes());
    Ok(out)
}

macro_rules! by_dtype {
    ($dtype:expr, $f:ident ( $($arg:expr),* )) => {
        match $dtype {
            1 => $f::<u32>($($arg),*),
            2 => $f::<u64>($($arg),*),
            3 => $f::<i32>($($arg),*),
            4 => $f::<i64>($($arg),*),
            5 => $f::<f32>($($arg),*),
            6 => $f::<f64>($($arg),*),
            7 => $f::<u16>($($arg),*),
            8 => $f::<i16>($($arg),*),
            9 => $f::<f16>($($arg),*),
            10 => $f::<u8>($($arg),*),
            11 => $f::<i8>($($arg),*),
            d => Err(format!("unknown dtype {d}")),
        }
    };
}

fn handle(req: &[u8]) -> Res {
    let mut c = Cursor { b: req };
    match c.u8() {
        1 => {
            let dtype = c.u8();
            let cfg = config(&mut c);
            by_dtype!(dtype, compress(&cfg, c.rest()))
        }
        2 => {
            let dtype = c.u8();
            by_dtype!(dtype, decompress(c.rest()))
        }
        3 => {
            let dtype = c.u8();
            let cfg = config(&mut c);
            by_dtype!(dtype, meta(&cfg, c.rest()))
        }
        4 => {
            let hint = c.u8();
            let level = c.u8();
            let decimal = c.u8() != 0;
            let opts = tparquet_core::Options {
                level,
                hint: tparquet_core::PhysicalType::from_parquet(i32::from(hint)),
                decimal,
            };
            tparquet_core::compress(c.rest(), &opts).map_err(|e| e.to_string())
        }
        5 => {
            let len = c.u64() as usize;
            tparquet_core::decompress(c.rest(), len).map_err(|e| e.to_string())
        }
        6 => {
            let f = c.u8();
            let n = c.u32();
            let mut out = Vec::with_capacity(8 * n as usize);
            for _ in 0..n {
                let x = c.f64();
                let r = x.to_bits();
                let y: u64 = match f {
                    0 => x.log2().to_bits(),
                    1 => x.cbrt().to_bits(),
                    2 => 10.0_f64.powf(x).to_bits(),
                    3 => x.log10().to_bits(),
                    4 => x as u64,
                    5 => u64::from(x as u32),
                    6 => u64::from(x as u16),
                    7 => x as i64 as u64,
                    8 => if x.is_nan() { 0 } else { x.round().to_bits() },
                    9 => { let y = x as f32; if y.is_nan() { 0 } else { u64::from(y.round().to_bits()) } }
                    10 => (r as f64).to_bits(),
                    11 => u64::from((r as f32).to_bits()),
                    _ => u64::from(f16::from_f64(x).to_bits()),
                };
                out.extend_from_slice(&y.to_le_bytes());
            }
            Ok(out)
        }
        7 => {
            let f = c.u8();
            let n = c.u32();
            let mut out = Vec::with_capacity(2 * n as usize);
            for _ in 0..n {
                let h = match f {
                    0 => f16::from_f64(c.f64()),
                    _ => f16::from_f32(f32::from_bits(c.u32())),
                };
                out.extend_from_slice(&h.to_bits().to_le_bytes());
            }
            Ok(out)
        }
        8 => {
            let f = c.u8();
            let seed = c.u64();
            let start = c.u64();
            let count = c.u64();
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for i in start..start + count {
                h = (h ^ digest_value(f, seed, i)).wrapping_mul(0x100_0000_01b3);
            }
            Ok(h.to_le_bytes().to_vec())
        }
        9 => {
            let dtype = c.u8();
            let cfg = config(&mut c);
            let len = c.u32() as usize;
            let n_alpha = c.u32() as usize;
            let w = width(dtype);
            let alphabet: Vec<&[u8]> = (0..n_alpha).map(|_| c.take(w)).collect();
            let start = c.u64();
            let count = c.u64();
            let mut h = FNV0;
            let mut seq = vec![0u8; len * w];
            for i in start..start + count {
                let mut k = i;
                for pos in (0..len).rev() {
                    let a = (k % n_alpha as u64) as usize;
                    k /= n_alpha as u64;
                    seq[pos * w..(pos + 1) * w].copy_from_slice(alphabet[a]);
                }
                let out = guarded(|| by_dtype!(dtype, compress(&cfg, &seq)));
                h = fold(h, &out);
            }
            Ok(h.to_le_bytes().to_vec())
        }
        10 => {
            let dtype = c.u8();
            let n_prefix = c.u32() as usize;
            let prefix = c.take(n_prefix).to_vec();
            let suffix_len = c.u32() as usize;
            let start = c.u64();
            let count = c.u64();
            let mut h = FNV0;
            let mut file = prefix.clone();
            file.resize(n_prefix + suffix_len, 0);
            for i in start..start + count {
                for b in 0..suffix_len {
                    file[n_prefix + b] = (i >> (8 * (suffix_len - 1 - b))) as u8;
                }
                let out = guarded(|| by_dtype!(dtype, decompress(&file)));
                h = fold(h, &out);
            }
            Ok(h.to_le_bytes().to_vec())
        }
        11 => {
            let dtype = c.u8();
            let cfg = config(&mut c);
            let iterations = c.u32();
            let b = c.rest();
            by_dtype!(dtype, bench(&cfg, iterations, b))
        }
        #[cfg(feature = "trace")]
        13 => {
            let dtype = c.u8();
            let cfg = config(&mut c);
            pco::TRACE_DICT.with(|t| t.borrow_mut().clear());
            let file = by_dtype!(dtype, compress(&cfg, c.rest()))?;
            let dict = pco::TRACE_DICT.with(|t| t.borrow().clone());
            let mut out = (dict.len() as u32).to_le_bytes().to_vec();
            for x in dict {
                out.extend_from_slice(&x.to_le_bytes());
            }
            out.extend_from_slice(&file);
            Ok(out)
        }
        op => Err(format!("unknown op {op}")),
    }
}

fn main() -> io::Result<()> {
    panic::set_hook(Box::new(|_| {}));
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = io::BufWriter::new(stdout.lock());
    loop {
        let mut len = [0u8; 4];
        if input.read_exact(&mut len).is_err() {
            return Ok(());
        }
        let mut req = vec![0u8; u32::from_le_bytes(len) as usize];
        input.read_exact(&mut req)?;
        let (status, body) = match panic::catch_unwind(AssertUnwindSafe(|| handle(&req))) {
            Ok(Ok(b)) => (0u8, b),
            Ok(Err(e)) => (1u8, e.into_bytes()),
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                (2u8, msg.into_bytes())
            }
        };
        output.write_all(&((body.len() + 1) as u32).to_le_bytes())?;
        output.write_all(&[status])?;
        output.write_all(&body)?;
        output.flush()?;
    }
}

fn splitmix(seed: u64, i: u64) -> u64 {
    let mut z = seed.wrapping_add(i.wrapping_add(1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// a positive double from random bits: any finite positive value, uniform in the bits
fn positive(bits: u64) -> f64 {
    let x = f64::from_bits(bits & 0x7fff_ffff_ffff_ffff);
    if x.is_finite() && x > 0.0 { x } else { 1.0 }
}

/// The function `f` on input number `i` of its domain (`seed` for random domains).
fn digest_value(f: u8, seed: u64, i: u64) -> u64 {
    let r = splitmix(seed, i);
    match f {
        // every f32 bit pattern to f16
        0 => u64::from(f16::from_f32(f32::from_bits(i as u32)).to_bits()),
        // every f16 to f32 and f64
        1 => {
            let h = f16::from_bits(i as u16);
            u64::from(h.to_f32().to_bits()) ^ h.to_f64().to_bits().rotate_left(32)
        }
        // log2 of the integers 1, 2, ...
        2 => ((i + 1) as f64).log2().to_bits(),
        // log2 and cbrt of random positive doubles
        3 => positive(r).log2().to_bits(),
        4 => positive(r).cbrt().to_bits(),
        // log2 and cbrt in (0, 1], where the mode heuristics evaluate them
        5 => {
            let p = ((r >> 11) + 1) as f64 / (1u64 << 53) as f64;
            p.log2().to_bits() ^ p.cbrt().to_bits().rotate_left(17)
        }
        // 10^k for integers k from -400
        6 => 10.0_f64.powf(i as f64 - 400.0).to_bits(),
        // float to int casts and rounding of random doubles
        7 => {
            let x = f64::from_bits(r);
            let y = f32::from_bits(r as u32);
            (x as u64)
                ^ (x as i64 as u64).rotate_left(7)
                ^ u64::from(x as u32).rotate_left(13)
                ^ (if x.is_nan() { 0 } else { x.round().to_bits() }).rotate_left(19)
                ^ u64::from(y as u32).rotate_left(23)
                ^ (if y.is_nan() { 0 } else { u64::from(y.round().to_bits()) }).rotate_left(29)
                ^ (y as u64).rotate_left(31)
                ^ u64::from(x as u16).rotate_left(37)
        }
        // int to float casts of random u64
        8 => (r as f64).to_bits() ^ u64::from((r as f32).to_bits()).rotate_left(32) ^ u64::from(((r >> (r & 63)) as f32).to_bits()),
        // f64 to f16
        9 => u64::from(f16::from_f64(f64::from_bits(r)).to_bits()),
        // log2 within 1024 ulps of every power of two
        10 => {
            let e = (i / 2048) as i64 - 1074;
            let k = (i % 2048) as i64 - 1024;
            let p = if e < -1022 { f64::from_bits(1u64 << (e + 1074)) } else { f64::from_bits(((e + 1023) as u64) << 52) };
            let x = f64::from_bits((p.to_bits() as i64 + k) as u64);
            if x.is_finite() && x > 0.0 { x.log2().to_bits() } else { 0 }
        }
        // log2 of doubles near 1 (glibc's separate path)
        11 => {
            let x = 1.0 + ((r >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.09;
            x.log2().to_bits()
        }
        _ => 0,
    }
}

const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

fn width(dtype: u8) -> usize {
    match dtype {
        10 | 11 => 1,
        7 | 8 | 9 => 2,
        1 | 3 | 5 => 4,
        _ => 8,
    }
}

/// the kind of a pco error (`pco {kind:?} error: ...`), or nothing for any other error
fn error_kind(message: &str) -> Vec<u8> {
    message
        .strip_prefix("pco ")
        .and_then(|rest| rest.split_once(" error: "))
        .map(|(kind, _)| kind.as_bytes().to_vec())
        .unwrap_or_default()
}

/// status and body of a call (the error kind for an error), catching a panic
fn guarded(f: impl FnOnce() -> Res) -> (u8, Vec<u8>) {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(b)) => (0, b),
        Ok(Err(e)) => (1, error_kind(&e)),
        Err(_) => (2, Vec::new()),
    }
}

/// folds one outcome into the digest: FNV-1a of the status and bytes, then mixed in
fn fold(h: u64, out: &(u8, Vec<u8>)) -> u64 {
    let mut x = FNV0;
    x = (x ^ u64::from(out.0)).wrapping_mul(0x100_0000_01b3);
    for &b in &out.1 {
        x = (x ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    (h ^ x).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29)
}

fn bench<T: Raw>(cfg: &ChunkConfig, iterations: u32, b: &[u8]) -> Res {
    use std::time::Instant;
    let v = values::<T>(b);
    let (mut best_c, mut best_d) = (u64::MAX, u64::MAX);
    let mut file = Vec::new();
    for _ in 0..iterations {
        let t = Instant::now();
        file = simple_compress(&v, cfg).map_err(|e| e.to_string())?;
        best_c = best_c.min(t.elapsed().as_nanos() as u64);
        let t = Instant::now();
        let back: Vec<T> = simple_decompress(&file).map_err(|e| e.to_string())?;
        best_d = best_d.min(t.elapsed().as_nanos() as u64);
        std::hint::black_box(back);
    }
    let mut out = best_c.to_le_bytes().to_vec();
    out.extend_from_slice(&best_d.to_le_bytes());
    out.extend_from_slice(&(file.len() as u64).to_le_bytes());
    Ok(out)
}
