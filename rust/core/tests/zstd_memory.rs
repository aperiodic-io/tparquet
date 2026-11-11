//! A ZSTD payload's frame header declares a window size of its choosing (RFC 8878 3.1.1.1.2);
//! the decoder must not size its memory by it, only by the expected size (spec section 7).
//!
//! zstd allocates with C malloc, which a Rust allocator cannot see, so this binary caps its own
//! address space (RLIMIT_AS) around one decode. It holds a single test so that nothing else runs
//! under the cap.
#![cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]

use tparquet_core::decompress;

const RLIMIT_AS: i32 = 9;

unsafe extern "C" {
    fn getrlimit(resource: i32, rlim: *mut [u64; 2]) -> i32;
    fn setrlimit(resource: i32, rlim: *const [u64; 2]) -> i32;
}

fn vm_size() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmSize:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() * 1024
}

/// a ZSTD payload: one frame without a content size, declaring a window of 2^`window_log`
/// bytes, holding one raw block of `data`
pub fn frame(window_log: u8, data: &[u8]) -> Vec<u8> {
    let mut p = vec![1, 3, 0x28, 0xB5, 0x2F, 0xFD, 0x00, (window_log - 10) << 3];
    let block = 1 | (data.len() as u32) << 3; // last block, raw
    p.extend(&block.to_le_bytes()[..3]);
    p.extend(data);
    p
}

#[test]
fn a_declared_window_costs_no_memory() {
    // warm up this thread's malloc arena and zstd outside the cap
    assert_eq!(decompress(&frame(10, b"warm"), 4).unwrap(), b"warm");
    let mut old = [0u64; 2];
    assert_eq!(unsafe { getrlimit(RLIMIT_AS, &mut old) }, 0);
    // 48 MB to spare: far less than the 128 MB window zstd's streaming decoder would reserve
    let cap = [vm_size() + (48 << 20), old[1]];
    assert_eq!(unsafe { setrlimit(RLIMIT_AS, &cap) }, 0);
    let got = decompress(&frame(27, b"abcdefgh"), 8);
    assert_eq!(unsafe { setrlimit(RLIMIT_AS, &old) }, 0);
    assert_eq!(got.as_deref(), Ok(&b"abcdefgh"[..]));
}
