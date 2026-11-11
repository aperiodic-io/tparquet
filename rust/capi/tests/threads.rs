//! The C ABI from many threads at once: results are independent, and each thread's last error
//! is its own (tparquet.h: thread-safe, last error per thread).

use std::sync::{Arc, Barrier};
use tparquet_capi::*;

fn compress(src: &[u8], t: i32) -> (i32, Vec<u8>) {
    let mut dst = vec![0u8; pcp_compress_bound(src.len())];
    let mut written = 0usize;
    let rc = unsafe { pcp_compress(src.as_ptr(), src.len(), t, -1, 1, dst.as_mut_ptr(), dst.len(), &mut written) };
    dst.truncate(written);
    (rc, dst)
}

fn decompress(p: &[u8], n: usize) -> (i32, Vec<u8>) {
    let mut dst = vec![0u8; n];
    let rc = unsafe { pcp_decompress(p.as_ptr(), p.len(), dst.as_mut_ptr(), n) };
    (rc, dst)
}

fn last_error() -> String {
    let mut buf = vec![0u8; 512];
    let n = unsafe { pcp_last_error(buf.as_mut_ptr().cast(), buf.len()) };
    buf.truncate(n);
    String::from_utf8(buf).unwrap()
}

/// thread `i`'s own data, of type `t`
fn page(i: usize, t: i32) -> Vec<u8> {
    let n = 500 + i * 37;
    match t {
        1 => (0..n).flat_map(|k| (k as i32 * 3 + i as i32).to_le_bytes()).collect(),
        2 => (0..n).flat_map(|k| (1_700_000_000_000 + k as i64 * i as i64).to_le_bytes()).collect(),
        4 => (0..n).flat_map(|k| ((k % 100 + i) as f32 / 10.0).to_le_bytes()).collect(),
        _ => (0..n).flat_map(|k| ((k * 7 + i) as f64 / 100.0).to_le_bytes()).collect(),
    }
}

#[test]
fn many_threads_round_trip_their_own_pages() {
    let threads = 16;
    let start = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|i| {
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                for round in 0..30 {
                    let t = [1, 2, 4, 5, -1][(i + round) % 5];
                    let src = page(i + round, t);
                    let (rc, p) = compress(&src, t);
                    assert_eq!(rc, PCP_OK, "thread {i} round {round}");
                    assert_eq!(decompress(&p, src.len()), (PCP_OK, src.clone()), "thread {i} round {round}");
                    assert_eq!(last_error(), "");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn every_thread_keeps_its_own_last_error() {
    // all threads fail at once, each with an expected size of its own, then check the message
    // only after all have failed; half of them then succeed, which clears only their message
    let threads = 16;
    let failed = Arc::new(Barrier::new(threads));
    let cleared = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|i| {
            let (failed, cleared) = (Arc::clone(&failed), Arc::clone(&cleared));
            std::thread::spawn(move || {
                let want = format!("pcodec payload: decodes to 1 bytes, expected {}", 100 + i);
                for _ in 0..20 {
                    assert_eq!(decompress(&[1, 0, 7], 100 + i).0, PCP_ERR_SIZE);
                    failed.wait();
                    assert_eq!(last_error(), want, "thread {i}");
                    if i % 2 == 0 {
                        assert_eq!(decompress(&[1, 0, 7], 1), (PCP_OK, vec![7]));
                    }
                    cleared.wait();
                    assert_eq!(last_error(), if i % 2 == 0 { "" } else { &want }, "thread {i}");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn errors_of_every_kind_from_many_threads() {
    let handles: Vec<_> = (0..8)
        .map(|i| {
            std::thread::spawn(move || {
                for _ in 0..50 {
                    let (rc, msg) = match i % 4 {
                        0 => (decompress(&[2, 0], 0).0, "pcodec payload: unsupported version 2"),
                        1 => (decompress(&[1, 9], 0).0, "pcodec payload: unknown mode 9"),
                        2 => (decompress(&[1, 1], 16).0, "pcodec payload: truncated"),
                        _ => (
                            unsafe { pcp_decompress(std::ptr::null(), 4, std::ptr::null_mut(), 0) },
                            "NULL pointer with a non-zero length",
                        ),
                    };
                    assert_eq!(rc, if i % 4 == 3 { PCP_ERR_ARGUMENT } else { PCP_ERR_CORRUPT });
                    assert_eq!(last_error(), msg);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}
