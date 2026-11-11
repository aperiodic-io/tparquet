//! C ABI over `tparquet-core`; the contract is `include/tparquet.h`.
#![allow(clippy::missing_safety_doc)] // the header documents the contract

use std::cell::RefCell;
use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};
use tparquet_core::{self as core, Error, Options, PhysicalType};

pub const PCP_OK: i32 = 0;
pub const PCP_ERR_ARGUMENT: i32 = 1;
pub const PCP_ERR_CORRUPT: i32 = 2;
pub const PCP_ERR_SIZE: i32 = 3;
pub const PCP_ERR_PANIC: i32 = 4;

/// A failed call: its return code and message.
type Failure = (i32, String);

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Records the calling thread's message. During thread teardown, after the thread-local is
/// gone (a C pthread key destructor may still call the library), the message is dropped.
fn set_error(msg: String) {
    let _ = LAST_ERROR.try_with(|e| *e.borrow_mut() = msg);
}

/// Runs `f`, records its error message (or clears it), and maps it to a return code.
fn guarded(f: impl FnOnce() -> Result<(), Failure>) -> i32 {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => {
            set_error(String::new());
            PCP_OK
        }
        Ok(Err((code, msg))) => {
            set_error(msg);
            code
        }
        Err(_) => {
            set_error("internal panic in tparquet".into());
            PCP_ERR_PANIC
        }
    }
}

fn code(e: &Error) -> i32 {
    match e {
        Error::SizeMismatch { .. } | Error::NotMultipleOfWidth { .. } => PCP_ERR_SIZE,
        Error::InvalidLevel(_) => PCP_ERR_ARGUMENT,
        _ => PCP_ERR_CORRUPT,
    }
}

fn arg(msg: &str) -> Failure {
    (PCP_ERR_ARGUMENT, msg.to_string())
}

/// Checks a C pointer and length: NULL is only allowed for length 0, and no object is larger
/// than isize::MAX bytes (a negative length cast to size_t, say).
fn check_buffer(p: *const u8, len: usize) -> Result<(), Failure> {
    if p.is_null() && len > 0 {
        return Err(arg("NULL pointer with a non-zero length"));
    }
    if isize::try_from(len).is_err() {
        return Err(arg(&format!("length {len} above isize::MAX")));
    }
    Ok(())
}

/// Refuses an input and an output that share bytes: Rust may not alias them.
fn check_disjoint(src: *const u8, src_len: usize, dst: *const u8, dst_len: usize) -> Result<(), Failure> {
    let (s, d) = (src.addr(), dst.addr());
    if src_len > 0 && dst_len > 0 && s < d.saturating_add(dst_len) && d < s.saturating_add(src_len) {
        return Err(arg("src and dst overlap"));
    }
    Ok(())
}

/// The input and output of a call, from C pointers and lengths: each NULL only with length 0
/// and at most isize::MAX bytes, and the two disjoint.
unsafe fn buffers<'a>(
    src: *const u8,
    src_len: usize,
    dst: *mut u8,
    dst_len: usize,
) -> Result<(&'a [u8], &'a mut [u8]), Failure> {
    check_buffer(src, src_len)?;
    check_buffer(dst, dst_len)?;
    check_disjoint(src, src_len, dst, dst_len)?;
    let src: &[u8] = if src_len == 0 {
        &[]
    } else {
        // SAFETY: non-NULL, at most isize::MAX bytes, and the caller guarantees `src` points at
        // `src_len` readable bytes (header contract)
        unsafe { std::slice::from_raw_parts(src, src_len) }
    };
    let dst: &mut [u8] = if dst_len == 0 {
        &mut []
    } else {
        // SAFETY: as above, writable, and disjoint from `src`
        unsafe { std::slice::from_raw_parts_mut(dst, dst_len) }
    };
    Ok((src, dst))
}

#[unsafe(no_mangle)]
pub extern "C" fn pcp_codec_id() -> i32 {
    core::CODEC_ID
}

#[unsafe(no_mangle)]
pub extern "C" fn pcp_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn pcp_compress_bound(src_len: usize) -> usize {
    src_len.saturating_add(2)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn pcp_compress(
    src: *const u8,
    src_len: usize,
    physical_type: i32,
    level: i32,
    decimal: i32,
    dst: *mut u8,
    dst_cap: usize,
    written: *mut usize,
) -> i32 {
    guarded(|| {
        let (src, dst) = unsafe { buffers(src, src_len, dst, dst_cap)? };
        if written.is_null() {
            return Err(arg("NULL `written`"));
        }
        let bound = pcp_compress_bound(src_len);
        if dst_cap < bound {
            return Err(arg(&format!("dst_cap {dst_cap} below the bound {bound}")));
        }
        let level = match level {
            -1 => core::DEFAULT_LEVEL,
            l => u8::try_from(l).map_err(|_| arg(&format!("pcodec payload: compression level {l} invalid")))?,
        };
        let opts = Options { level, hint: PhysicalType::from_parquet(physical_type), decimal: decimal != 0 };
        let p = core::compress(src, &opts).map_err(|e| (code(&e), e.to_string()))?;
        dst[..p.len()].copy_from_slice(&p);
        // SAFETY: checked non-NULL above
        unsafe { *written = p.len() };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pcp_decompress(src: *const u8, src_len: usize, dst: *mut u8, dst_len: usize) -> i32 {
    guarded(|| {
        let (src, dst) = unsafe { buffers(src, src_len, dst, dst_len)? };
        core::decompress_into(src, dst).map_err(|e| (code(&e), e.to_string()))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pcp_last_error(buf: *mut c_char, cap: usize) -> usize {
    let copy = |msg: &str| {
        if !buf.is_null() && cap > 0 {
            let n = msg.len().min(cap - 1);
            // SAFETY: the caller guarantees `buf` has `cap` writable bytes; n + 1 <= cap
            unsafe {
                std::ptr::copy_nonoverlapping(msg.as_ptr().cast::<c_char>(), buf, n);
                *buf.add(n) = 0;
            }
        }
        msg.len()
    };
    // during thread teardown the message is gone: an empty one
    LAST_ERROR.try_with(|e| copy(&e.borrow())).unwrap_or_else(|_| copy(""))
}
