//! A PCODEC page whose header claims far more than its payload holds (spec section 7, AGENTS.md:
//! no unbounded allocation), read through the parquet fork's page reader.
//!
//! `uncompressed_page_size` is untrusted: a page of a few bytes can claim almost 2 GiB. The
//! reader (upstream arrow-rs, `decode_page`) reserves that capacity for every codec before it
//! calls the codec; the reservation is address space that nothing writes. The PCODEC codec
//! must not write it either, nor allocate more, before the payload shows it can produce that
//! many bytes.
//!
//! Allocations through the Rust global allocator are counted on this thread; what is written
//! is the process's peak resident size (`VmHWM`), so this binary holds a single test.
#![cfg(target_os = "linux")]

use bytes::Bytes;
use parquet::basic::{Compression, Encoding, PageType, Type as PhysicalType};
use parquet::column::page::{CompressedPage, Page, PageReader, PageWriter};
use parquet::file::metadata::ColumnChunkMetaData;
use parquet::file::serialized_reader::SerializedPageReader;
use parquet::file::writer::{SerializedPageWriter, TrackedWrite};
use parquet::schema::types::{ColumnDescriptor, ColumnPath, Type};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

struct Counting;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn grow(n: usize) {
    let _ = LIVE.try_with(|l| {
        let v = l.get() + n as isize;
        l.set(v);
        let _ = PEAK.try_with(|p| p.set(p.get().max(v)));
    });
}

fn shrink(n: usize) {
    let _ = LIVE.try_with(|l| l.set(l.get() - n as isize));
}

// SAFETY: forwards every call to the system allocator unchanged; only counts sizes
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        grow(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        grow(l.size());
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        shrink(l.size());
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        grow(n);
        shrink(l.size());
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// the most `f` had allocated at once on this thread, beyond what was live before
fn peak_of<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let start = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(start));
    let r = f();
    (r, (PEAK.with(Cell::get) - start) as usize)
}

/// the process's peak resident size, in bytes
fn vm_hwm() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmHWM:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse::<usize>().unwrap() * 1024
}

/// the most `uncompressed_page_size`, an i32, can claim
const CLAIM: usize = i32::MAX as usize;

/// a column chunk of one V1 data page with the PCODEC payload `payload` and a header claiming
/// `CLAIM` bytes, and the chunk's metadata
fn chunk(payload: &'static [u8]) -> (Bytes, ColumnChunkMetaData) {
    let leaf = Type::primitive_type_builder("v", PhysicalType::INT64).build().unwrap();
    let descr = ColumnDescriptor::new(Arc::new(leaf), 0, 0, ColumnPath::from("v"));
    let page = Page::DataPage {
        buf: Bytes::from_static(payload),
        num_values: 1,
        encoding: Encoding::PLAIN,
        def_level_encoding: Encoding::RLE,
        rep_level_encoding: Encoding::RLE,
        statistics: None,
    };
    let mut sink = TrackedWrite::new(Vec::new());
    let mut writer = SerializedPageWriter::new(&mut sink);
    let spec = writer.write_page(CompressedPage::new(page, CLAIM)).unwrap();
    assert_eq!(spec.page_type, PageType::DATA_PAGE);
    assert_eq!(spec.uncompressed_size, CLAIM + spec.compressed_size - payload.len());
    writer.close().unwrap();
    drop(writer);
    let bytes = Bytes::from(sink.into_inner().unwrap());
    let meta = ColumnChunkMetaData::builder(Arc::new(descr))
        .set_compression(Compression::PCODEC(Default::default()))
        .set_num_values(1)
        .set_data_page_offset(0)
        .set_total_compressed_size(bytes.len() as i64)
        .build()
        .unwrap();
    (bytes, meta)
}

#[test]
fn a_small_page_claiming_2_gib_writes_none_of_it() {
    let hostile: [(&str, &'static [u8]); 4] = [
        ("raw", &[1, 0, 7]),
        ("pco", &[1, 1, 2, 0xff, 0xff]),
        ("decimal", &[1, 2, 5, 2, 0]),
        // a single-segment frame with a content size of 2^31 - 1 and one RLE block of 128 KiB
        ("zstd", &[1, 3, 0x28, 0xB5, 0x2F, 0xFD, 0xA0, 0xFF, 0xFF, 0xFF, 0x7F, 0x03, 0x00, 0x10, 0x00]),
    ];
    let before = vm_hwm();
    for (name, payload) in hostile {
        let (bytes, meta) = chunk(payload);
        let mut reader = SerializedPageReader::new(Arc::new(bytes), &meta, 1, None).unwrap();
        let (page, peak) = peak_of(|| reader.get_next_page());
        let err = page.expect_err(name).to_string();
        assert!(err.contains("pcodec payload"), "{name}: {err}");
        // upstream's reservation of the claim, and nothing else of that size
        assert!(peak < CLAIM + (1 << 20), "{name}: {peak} bytes");
        let written = vm_hwm().saturating_sub(before);
        eprintln!("{name}: {} byte payload: peak allocation {peak}, resident growth {written} bytes", payload.len());
        assert!(written < 64 << 20, "{name}: {written} bytes resident for a {} byte page", payload.len());
    }
}
