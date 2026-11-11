//! The PCODEC Parquet compression codec: the page payload of `spec/FORMAT.md`.
//!
//! [`compress`] turns the bytes Parquet would compress into a payload; [`decompress`] turns a
//! payload back into exactly those bytes. Decoding never needs the column type: the payload
//! says how it was written. The type hint given to [`compress`] only affects the ratio.

mod decimal;
mod varint;

use decimal::Float;
use pco::ChunkConfig;
use pco::FULL_BATCH_N;
use pco::data_types::Number;
use pco::standalone::{DecompressorItem, FileDecompressor, simple_compress};
use std::fmt;

/// `CompressionCodec` value of PCODEC in the Parquet footer.
pub const CODEC_ID: i32 = 20547;
/// Payload format version this crate writes and reads.
pub const FORMAT_VERSION: u8 = 1;
/// Footer key-value pair writers add to files with PCODEC columns.
pub const KV_KEY: &str = "parquet.pcodec.version";
/// Value of [`KV_KEY`].
pub const KV_VALUE: &str = "1";
/// pcodec compression level used when none is given.
pub const DEFAULT_LEVEL: u8 = 8;
/// Highest pcodec compression level.
pub const MAX_LEVEL: u8 = 12;
/// ZSTD level of the fallback mode.
const ZSTD_LEVEL: i32 = 3;

/// The Parquet physical types PCODEC models, with their Parquet `Type` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PhysicalType {
    Int32 = 1,
    Int64 = 2,
    Float = 4,
    Double = 5,
}

impl PhysicalType {
    /// From a Parquet `Type` value; `None` for types PCODEC does not model.
    pub fn from_parquet(t: i32) -> Option<Self> {
        match t {
            1 => Some(Self::Int32),
            2 => Some(Self::Int64),
            4 => Some(Self::Float),
            5 => Some(Self::Double),
            _ => None,
        }
    }

    /// Bytes per value in Parquet `PLAIN` encoding.
    pub fn width(self) -> usize {
        match self {
            Self::Int32 | Self::Float => 4,
            Self::Int64 | Self::Double => 8,
        }
    }

    fn from_dtype(b: u8) -> Result<Self, Error> {
        Self::from_parquet(i32::from(b)).ok_or(Error::UnknownDtype(b))
    }
}

/// How a payload was written (spec section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mode {
    Raw = 0,
    Pco = 1,
    Decimal = 2,
    Zstd = 3,
}

impl Mode {
    fn from_byte(b: u8) -> Result<Self, Error> {
        match b {
            0 => Ok(Self::Raw),
            1 => Ok(Self::Pco),
            2 => Ok(Self::Decimal),
            3 => Ok(Self::Zstd),
            _ => Err(Error::UnknownMode(b)),
        }
    }
}

/// Encoder settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// pcodec compression level, 0..=12.
    pub level: u8,
    /// The column's physical type; without it the encoder can only use ZSTD or RAW.
    pub hint: Option<PhysicalType>,
    /// Try DECIMAL for float columns.
    pub decimal: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { level: DEFAULT_LEVEL, hint: None, decimal: true }
    }
}

impl Options {
    /// Default options for a column of type `t`.
    pub fn with_hint(t: PhysicalType) -> Self {
        Self { hint: Some(t), ..Self::default() }
    }
}

/// Why a payload could not be decoded (or an option was refused).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    Truncated,
    UnsupportedVersion(u8),
    UnknownMode(u8),
    UnknownDtype(u8),
    BadExponent(u8),
    BadException,
    SizeMismatch {
        expected: usize,
        actual: usize,
    },
    NotMultipleOfWidth {
        len: usize,
        width: usize,
    },
    InvalidLevel(u8),
    /// The expected size given to [`decompress`] cannot be allocated.
    AllocationFailed(usize),
    /// Bytes after the pcodec file, which must run to the end of the payload.
    TrailingBytes(usize),
    Pco(String),
    Zstd(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("pcodec payload: ")?;
        match self {
            Self::Truncated => f.write_str("truncated"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported version {v}"),
            Self::UnknownMode(m) => write!(f, "unknown mode {m}"),
            Self::UnknownDtype(d) => write!(f, "unknown dtype {d}"),
            Self::BadExponent(e) => write!(f, "exponent {e} above 18"),
            Self::BadException => f.write_str("exception count or position out of range"),
            Self::SizeMismatch { expected, actual } => write!(f, "decodes to {actual} bytes, expected {expected}"),
            Self::NotMultipleOfWidth { len, width } => {
                write!(f, "expected size {len} is not a multiple of the value width {width}")
            }
            Self::InvalidLevel(l) => write!(f, "compression level {l} above {MAX_LEVEL}"),
            Self::AllocationFailed(n) => write!(f, "cannot allocate the expected size of {n} bytes"),
            Self::TrailingBytes(n) => write!(f, "{n} bytes after the pcodec file"),
            Self::Pco(e) => write!(f, "pcodec: {e}"),
            Self::Zstd(e) => write!(f, "zstd: {e}"),
        }
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------- encode

/// Encodes `src` as a payload.
pub fn compress(src: &[u8], opts: &Options) -> Result<Vec<u8>, Error> {
    if opts.level > MAX_LEVEL {
        return Err(Error::InvalidLevel(opts.level));
    }
    let raw = || [&[FORMAT_VERSION, Mode::Raw as u8][..], src].concat();
    if src.is_empty() {
        return Ok(raw());
    }
    let cfg = ChunkConfig::default().with_compression_level(usize::from(opts.level));
    let best = match opts.hint.filter(|t| src.len().is_multiple_of(t.width())) {
        Some(PhysicalType::Int32) => pco_payload(PhysicalType::Int32, &values(src, i32::from_le_bytes), &cfg)?,
        Some(PhysicalType::Int64) => pco_payload(PhysicalType::Int64, &values(src, i64::from_le_bytes), &cfg)?,
        Some(PhysicalType::Float) => float_payload(PhysicalType::Float, &values(src, f32::from_le_bytes), opts, &cfg)?,
        Some(PhysicalType::Double) => {
            float_payload(PhysicalType::Double, &values(src, f64::from_le_bytes), opts, &cfg)?
        }
        None => {
            let mut p = vec![FORMAT_VERSION, Mode::Zstd as u8];
            p.extend(zstd::bulk::compress(src, ZSTD_LEVEL).map_err(|e| Error::Zstd(e.to_string()))?);
            p
        }
    };
    Ok(if best.len() < src.len() + 2 { best } else { raw() })
}

fn values<T, const W: usize>(src: &[u8], f: fn([u8; W]) -> T) -> Vec<T> {
    src.as_chunks::<W>().0.iter().map(|c| f(*c)).collect()
}

fn pco_bytes<T: Number>(v: &[T], cfg: &ChunkConfig) -> Result<Vec<u8>, Error> {
    simple_compress(v, cfg).map_err(|e| Error::Pco(e.to_string()))
}

fn pco_payload<T: Number>(t: PhysicalType, v: &[T], cfg: &ChunkConfig) -> Result<Vec<u8>, Error> {
    let mut p = vec![FORMAT_VERSION, Mode::Pco as u8, t as u8];
    p.extend(pco_bytes(v, cfg)?);
    Ok(p)
}

fn float_payload<F: Float + Plain>(
    t: PhysicalType,
    v: &[F],
    opts: &Options,
    cfg: &ChunkConfig,
) -> Result<Vec<u8>, Error> {
    let plain = pco_payload(t, v, cfg)?;
    if !opts.decimal {
        return Ok(plain);
    }
    let Some(e) = decimal::exponent(v) else { return Ok(plain) };
    let Some((qs, exc)) = decimal::split(v, e) else { return Ok(plain) };
    let mut p = vec![FORMAT_VERSION, Mode::Decimal as u8, t as u8, e];
    varint::put(&mut p, exc.len() as u64);
    let mut next = 0;
    for &i in &exc {
        varint::put(&mut p, (i - next) as u64);
        next = i + 1;
    }
    for &i in &exc {
        let at = p.len();
        p.resize(at + size_of::<F>(), 0);
        v[i].put(&mut p[at..]);
    }
    p.extend(pco_bytes(&qs, cfg)?);
    Ok(if p.len() < plain.len() { p } else { plain })
}

/// pcodec numbers this crate writes as Parquet `PLAIN` bytes.
trait Plain: Number {
    /// writes the value's little-endian bytes into `out`, which is exactly its width
    fn put(self, out: &mut [u8]);
    /// appends the value's little-endian bytes to `out`
    fn append(self, out: &mut Vec<u8>);
}

macro_rules! plain {
    ($($t:ty),*) => {$(
        impl Plain for $t {
            fn put(self, out: &mut [u8]) {
                out.copy_from_slice(&self.to_le_bytes());
            }
            fn append(self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
        }
    )*};
}
plain!(i32, i64, f32, f64);

// ---------------------------------------------------------------- decode

/// The mode of a payload, after checking its version.
pub fn mode_of(payload: &[u8]) -> Result<Mode, Error> {
    match payload {
        [] | [_] => Err(Error::Truncated),
        [v, ..] if *v != FORMAT_VERSION => Err(Error::UnsupportedVersion(*v)),
        [_, m, ..] => Mode::from_byte(*m),
    }
}

/// Decodes a payload that must produce exactly `expected_len` bytes.
///
/// `expected_len` is untrusted too (a page header): see [`decompress_append`].
pub fn decompress(payload: &[u8], expected_len: usize) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    decompress_append(payload, &mut out, expected_len)?;
    Ok(out)
}

/// Decodes a payload that must produce exactly `expected_len` bytes, appending them to `out`.
/// On an error `out` is left as it was (its capacity may have grown).
///
/// `expected_len` is untrusted too (a page header): nothing proportional to it is allocated or
/// written before the payload shows it can produce that many bytes. A RAW body must have that
/// length, a ZSTD body must be long enough to hold that many bytes (each Zstandard block costs
/// at least 4 bytes and decodes to at most 128 KiB), and PCO and DECIMAL output grows as the
/// numbers are decoded. Memory that cannot be allocated is [`Error::AllocationFailed`].
pub fn decompress_append(payload: &[u8], out: &mut Vec<u8>, expected_len: usize) -> Result<(), Error> {
    let start = out.len();
    let r = append(payload, &mut Tail { out, start, expected_len }, expected_len);
    if r.is_err() {
        out.truncate(start);
    }
    r
}

/// Decodes a payload into `dst`, which it must fill exactly.
pub fn decompress_into(payload: &[u8], dst: &mut [u8]) -> Result<(), Error> {
    let mode = mode_of(payload)?;
    let body = &payload[2..];
    match mode {
        Mode::Raw => {
            check(dst.len(), body.len())?;
            dst.copy_from_slice(body);
            Ok(())
        }
        Mode::Zstd => zstd_into(body, dst),
        Mode::Pco | Mode::Decimal => numbers(mode, body, dst, dst.len()),
    }
}

/// [`decompress_append`] into `out`, which [`decompress_append`] restores on an error
fn append(payload: &[u8], out: &mut Tail<'_>, len: usize) -> Result<(), Error> {
    let mode = mode_of(payload)?;
    let body = &payload[2..];
    match mode {
        Mode::Raw => {
            check(len, body.len())?;
            out.reserve(len)?;
            out.out.extend_from_slice(body);
            Ok(())
        }
        Mode::Zstd => {
            // RFC 8878 3.1.1.2: a block that decodes to anything is a 3 byte header and at
            // least one byte, and decodes to at most 128 KiB
            if len > (body.len() / 4).saturating_mul(ZSTD_BLOCK_MAX) {
                return Err(Error::Truncated);
            }
            out.reserve(len)?;
            let at = out.out.len();
            out.out.resize(at + len, 0);
            zstd_into(body, &mut out.out[at..])
        }
        Mode::Pco | Mode::Decimal => numbers(mode, body, out, len),
    }
}

/// the most one Zstandard block decodes to (RFC 8878 3.1.1.2, `Block_Maximum_Size`)
const ZSTD_BLOCK_MAX: usize = 128 << 10;

/// The PCO and DECIMAL bodies (spec 4.2, 4.3), decoded into `len` bytes of `dst`.
fn numbers<S: Sink + ?Sized>(mode: Mode, body: &[u8], dst: &mut S, len: usize) -> Result<(), Error> {
    if mode == Mode::Pco {
        let (&dtype, body) = body.split_first().ok_or(Error::Truncated)?;
        return match PhysicalType::from_dtype(dtype)? {
            PhysicalType::Int32 => pco_into::<i32, S>(body, dst, len),
            PhysicalType::Int64 => pco_into::<i64, S>(body, dst, len),
            PhysicalType::Float => pco_into::<f32, S>(body, dst, len),
            PhysicalType::Double => pco_into::<f64, S>(body, dst, len),
        };
    }
    let [dtype, e, ..] = *body else { return Err(Error::Truncated) };
    let t = PhysicalType::from_dtype(dtype)?;
    if e > decimal::MAX_EXPONENT {
        return Err(Error::BadExponent(e));
    }
    let body = &body[2..];
    match t {
        PhysicalType::Float => decimal_into::<f32, S>(body, e, dst, len),
        PhysicalType::Double => decimal_into::<f64, S>(body, e, dst, len),
        PhysicalType::Int32 | PhysicalType::Int64 => Err(Error::UnknownDtype(dtype)),
    }
}

/// Where decoded numbers go: the caller's slice ([`decompress_into`]), or the end of a `Vec`
/// that grows as the numbers are decoded ([`decompress_append`]).
trait Sink {
    /// writes numbers `i..i + nums.len()`; they arrive in order, from 0, and never beyond the
    /// expected size
    fn put<T: Plain>(&mut self, i: usize, nums: impl ExactSizeIterator<Item = T>) -> Result<(), Error>;
    /// the bytes written so far
    fn written(&mut self) -> &mut [u8];
}

impl Sink for [u8] {
    fn put<T: Plain>(&mut self, i: usize, nums: impl ExactSizeIterator<Item = T>) -> Result<(), Error> {
        let width = size_of::<T>();
        for (out, v) in self[i * width..].chunks_exact_mut(width).zip(nums) {
            v.put(out);
        }
        Ok(())
    }
    fn written(&mut self) -> &mut [u8] {
        self
    }
}

/// the end of a `Vec`, from its length `start` when decoding started
struct Tail<'a> {
    out: &'a mut Vec<u8>,
    start: usize,
    expected_len: usize,
}

impl Tail<'_> {
    fn reserve(&mut self, additional: usize) -> Result<(), Error> {
        self.out.try_reserve(additional).map_err(|_| Error::AllocationFailed(self.expected_len))
    }
}

impl Sink for Tail<'_> {
    fn put<T: Plain>(&mut self, _: usize, nums: impl ExactSizeIterator<Item = T>) -> Result<(), Error> {
        self.reserve(nums.len() * size_of::<T>())?;
        for v in nums {
            v.append(self.out);
        }
        Ok(())
    }
    fn written(&mut self) -> &mut [u8] {
        &mut self.out[self.start..]
    }
}

/// Zstandard frames into exactly `dst`, decoded in one pass straight into it: the frames'
/// declared window sizes cost no memory, and output past `dst.len()` is never produced
fn zstd_into(body: &[u8], dst: &mut [u8]) -> Result<(), Error> {
    use zstd::zstd_safe::{self, zstd_sys::ZSTD_ErrorCode};
    if body.is_empty() {
        // no frame at all (spec 4.4: one frame or several)
        return Err(Error::Truncated);
    }
    // zstd's error codes are the negated ZSTD_ErrorCode values (zstd_errors.h)
    let too_small = (ZSTD_ErrorCode::ZSTD_error_dstSize_tooSmall as usize).wrapping_neg();
    match zstd_safe::decompress(dst, body) {
        Ok(n) => check(dst.len(), n),
        // the frames hold more than the page: at least one byte more
        Err(code) if code == too_small => Err(Error::SizeMismatch { expected: dst.len(), actual: dst.len() + 1 }),
        Err(code) => Err(Error::Zstd(zstd_safe::get_error_name(code).to_string())),
    }
}

fn check(expected: usize, actual: usize) -> Result<(), Error> {
    if expected == actual { Ok(()) } else { Err(Error::SizeMismatch { expected, actual }) }
}

/// `len / width`, refusing a length that is not whole values
fn count(len: usize, width: usize) -> Result<usize, Error> {
    if len.is_multiple_of(width) { Ok(len / width) } else { Err(Error::NotMultipleOfWidth { len, width }) }
}

fn pco_error(e: pco::errors::PcoError) -> Error {
    Error::Pco(e.to_string())
}

/// Decodes the pcodec file `src`, which must hold exactly `n` numbers and run to the end of
/// `src`, one batch at a time:
/// `put(i, batch)` receives numbers `i..i + batch.len()`. Nothing proportional to `n` or to
/// what the chunk headers claim is allocated; size errors count `width` bytes per number.
fn pco_batches<T: Number>(
    src: &[u8],
    n: usize,
    width: usize,
    mut put: impl FnMut(usize, &[T]) -> Result<(), Error>,
) -> Result<(), Error> {
    let (file, mut src) = FileDecompressor::new(src).map_err(pco_error)?;
    let mut batch = [T::default(); FULL_BATCH_N];
    let mut done = 0;
    loop {
        let mut chunk = match file.chunk_decompressor::<T, _>(src).map_err(pco_error)? {
            DecompressorItem::Chunk(chunk) => chunk,
            DecompressorItem::EndOfData([]) => break,
            DecompressorItem::EndOfData(rest) => return Err(Error::TrailingBytes(rest.len())),
        };
        let end = done + chunk.n();
        if end > n {
            // more numbers than the page has room for, refused before they are decoded
            return Err(Error::SizeMismatch { expected: n * width, actual: end * width });
        }
        loop {
            let progress = chunk.read(&mut batch).map_err(pco_error)?;
            put(done, &batch[..progress.n_processed])?;
            done += progress.n_processed;
            if progress.finished {
                break;
            }
        }
        src = chunk.into_src();
    }
    check(n * width, done * width)
}

fn pco_into<T: Plain, S: Sink + ?Sized>(src: &[u8], dst: &mut S, len: usize) -> Result<(), Error> {
    let width = size_of::<T>();
    let n = count(len, width)?;
    pco_batches::<T>(src, n, width, |i, nums| dst.put(i, nums.iter().copied()))
}

fn decimal_into<F: Float + Plain, S: Sink + ?Sized>(
    mut body: &[u8],
    e: u8,
    dst: &mut S,
    len: usize,
) -> Result<(), Error> {
    let width = size_of::<F>();
    let n = count(len, width)?;
    let k = varint::get(&mut body)?;
    if k > n as u64 {
        return Err(Error::BadException);
    }
    // the positions are checked here and read again once the values are written, so that an
    // exception count the payload does not back costs no allocation
    let deltas = body;
    let mut next: u64 = 0;
    for _ in 0..k {
        let p = next.checked_add(varint::get(&mut body)?).ok_or(Error::BadException)?;
        if p >= n as u64 {
            return Err(Error::BadException);
        }
        next = p + 1;
    }
    // k <= n, so this is at most len
    let (exc, pco) = body.split_at_checked(k as usize * width).ok_or(Error::Truncated)?;
    pco_batches::<i64>(pco, n, width, |i, qs| dst.put(i, qs.iter().map(|q| F::decode(*q, e))))?;
    let dst = dst.written();
    let (mut deltas, mut next) = (deltas, 0);
    for bits in exc.chunks_exact(width) {
        // checked above: each delta is a valid varint and every position is below n
        let p = next + varint::get(&mut deltas)? as usize;
        dst[p * width..(p + 1) * width].copy_from_slice(bits);
        next = p + 1;
    }
    Ok(())
}
