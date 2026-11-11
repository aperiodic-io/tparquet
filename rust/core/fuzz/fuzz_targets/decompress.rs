//! Any payload and any expected size: the decoder returns Ok or Err, never panics or overreads,
//! and decoding into a buffer of the expected size agrees with appending to a `Vec`.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    // first two bytes: expected size (bounded, as a page header bounds it)
    let expected = usize::from(u16::from_le_bytes([data[0], data[1]]));
    let appended = tparquet_core::decompress(&data[2..], expected);
    let mut dst = vec![0; expected];
    let into = tparquet_core::decompress_into(&data[2..], &mut dst).map(|()| dst);
    assert_eq!(appended.ok(), into.ok());
});
