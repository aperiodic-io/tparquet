//! The conformance vectors in `testdata/`: up to date, and read correctly.

use assert_cmd::Command;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tparquet::canonical;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

fn manifest(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Relative paths of every file under `dir`, but the README.
fn tree(dir: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for sub in ["files", "expected", "payloads"] {
        for e in std::fs::read_dir(dir.join(sub)).unwrap() {
            out.insert(Path::new(sub).join(e.unwrap().file_name()));
        }
    }
    out
}

#[test]
fn generator_reproduces_testdata_byte_for_byte() {
    let _lock = TESTDATA.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("gen-testdata").unwrap().arg("--out").arg(dir.path()).assert().success();
    let (ours, committed) = (tree(dir.path()), tree(&testdata()));
    assert_eq!(ours, committed);
    for f in &ours {
        let a = std::fs::read(dir.path().join(f)).unwrap();
        let b = std::fs::read(testdata().join(f)).unwrap();
        assert!(
            a == b,
            "{} differs from the committed vector: run `cargo run -p tparquet --bin gen-testdata`",
            f.display()
        );
    }
    // a second run over the first replaces everything with the same bytes, and drops stale files
    for sub in ["files", "expected", "payloads"] {
        std::fs::write(dir.path().join(sub).join("stale"), b"x").unwrap();
    }
    Command::cargo_bin("gen-testdata").unwrap().arg("--out").arg(dir.path()).assert().success();
    assert_eq!(tree(dir.path()), committed);
}

#[test]
fn generator_usage_errors() {
    for args in [&["--frob"][..], &["--frob", "dir"], &["--out", "a", "b"], &["--check"], &["--check", "a", "b"]] {
        Command::cargo_bin("gen-testdata")
            .unwrap()
            .args(args)
            .assert()
            .code(1)
            .stderr("gen-testdata: usage: gen-testdata [--out DIR | --check DIR]\n");
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    Command::cargo_bin("gen-testdata")
        .unwrap()
        .arg("--out")
        .arg(file.path())
        .assert()
        .code(1)
        .stderr(predicates::str::starts_with("gen-testdata: "));
}

/// Tests that read `testdata/` hold this lock against the one that rewrites it.
static TESTDATA: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn read_all() -> Vec<(Vec<u8>, PathBuf)> {
    tree(&testdata()).into_iter().map(|f| (std::fs::read(testdata().join(&f)).unwrap(), f)).collect()
}

/// Puts `testdata/` back as it was when dropped, even after a failed assertion: a generator
/// that writes other vectors (a mutant, under cargo mutants) must not leave them behind for
/// the tests that run after it in the same tree.
struct Restore(Vec<(Vec<u8>, PathBuf)>);

impl Drop for Restore {
    fn drop(&mut self) {
        let keep: BTreeSet<&PathBuf> = self.0.iter().map(|(_, f)| f).collect();
        for f in tree(&testdata()) {
            if !keep.contains(&f) {
                let _ = std::fs::remove_file(testdata().join(&f));
            }
        }
        for (bytes, f) in &self.0 {
            let _ = std::fs::write(testdata().join(f), bytes);
        }
    }
}

#[test]
fn generator_rewrites_testdata_in_place_unchanged() {
    let _lock = TESTDATA.lock().unwrap_or_else(|e| e.into_inner());
    let before = Restore(read_all());
    // without arguments it writes the repository's testdata/
    Command::cargo_bin("gen-testdata").unwrap().assert().success();
    assert!(read_all() == before.0, "regenerating changed testdata/");
}

#[test]
fn files_read_as_expected() {
    let _lock = TESTDATA.lock().unwrap_or_else(|e| e.into_inner());
    let m = manifest(&testdata().join("files/manifest.json"));
    let files = m["files"].as_array().unwrap();
    assert_eq!(files.len(), 24);
    let mut modes = BTreeSet::new();
    let mut pcodec = 0;
    for f in files {
        let path = testdata().join(f["file"].as_str().unwrap());
        let builder = tparquet::read::open(&path).unwrap();
        let mut footer = serde_json::Map::new();
        for kv in builder.metadata().file_metadata().key_value_metadata().into_iter().flatten() {
            if kv.key != "ARROW:schema" {
                footer.insert(kv.key.clone(), kv.value.clone().into());
            }
        }
        assert_eq!(Value::Object(footer), f["key_value_metadata"], "{}", f["name"]);
        let reader = builder.build().unwrap();
        let mut actual = String::new();
        for b in reader {
            canonical::write_batch(&b.unwrap(), &mut actual).unwrap();
        }
        let expected = std::fs::read_to_string(testdata().join(f["expected"].as_str().unwrap())).unwrap();
        assert!(actual == expected, "{}", f["name"]);
        assert_eq!(expected.lines().count() as i64, f["rows"].as_i64().unwrap());
        for c in f["columns"].as_array().unwrap() {
            modes.extend(c["modes"].as_object().unwrap().keys().cloned());
        }
        pcodec += usize::from(f["pcodec_version"] == "1");
    }
    // the four payload modes, and the two labels of pages without a payload to decode
    for m in ["RAW", "PCO", "DECIMAL", "ZSTD", "EMPTY", "UNCOMPRESSED"] {
        assert!(modes.contains(m), "{m}");
    }
    assert_eq!(pcodec, 23);
}

#[test]
fn the_large_page_holds_two_pcodec_chunks() {
    // pcodec's chunks hold at most 2^18 numbers: the one payload of large_page has two
    let file = std::fs::read(testdata().join("files/large_page.parquet")).unwrap();
    let report = tparquet::inspect::inspect(&bytes::Bytes::from(file.clone())).unwrap();
    assert_eq!(report.columns[0].pages, [("DATA_PAGE_V2".to_string(), 1)].into());
    let magic = file.windows(4).position(|w| w == b"pco!").unwrap();
    assert_eq!(file[magic - 3..magic], [1, 1, 1], "version 1, PCO, INT32");
    assert!(file[magic + 1..].windows(4).all(|w| w != b"pco!"), "one pcodec file");
    let (fd, mut src) = pco::standalone::FileDecompressor::new(&file[magic..]).unwrap();
    let mut sizes = vec![];
    while let pco::standalone::DecompressorItem::Chunk(mut chunk) = fd.chunk_decompressor::<i32, _>(src).unwrap() {
        sizes.push(chunk.n());
        let mut out = vec![0; chunk.n()];
        assert!(chunk.read(&mut out).unwrap().finished);
        src = chunk.into_src();
    }
    assert_eq!(sizes, [135_000, 135_000]);
    assert!(sizes.iter().sum::<usize>() > 1 << 18);
}

#[test]
fn read_open_reports_missing_files() {
    let err = tparquet::read::open(testdata().join("files/missing.parquet")).err().unwrap();
    assert!(err.to_string().starts_with("io: "), "{err}");
}

#[test]
fn payload_vectors_decode_or_fail() {
    let _lock = TESTDATA.lock().unwrap_or_else(|e| e.into_inner());
    let dir = testdata().join("payloads");
    let m = manifest(&dir.join("manifest.json"));
    let (mut valid, mut invalid) = (0, 0);
    for v in m["vectors"].as_array().unwrap() {
        let payload = std::fs::read(dir.join(v["payload"].as_str().unwrap())).unwrap();
        let len = v["expected_len"].as_u64().unwrap() as usize;
        let got = tparquet_core::decompress(&payload, len);
        if v["valid"] == true {
            let decoded = std::fs::read(dir.join(v["decoded"].as_str().unwrap())).unwrap();
            assert_eq!(got.unwrap(), decoded, "{}", v["name"]);
            let mode = tparquet_core::mode_of(&payload).unwrap();
            assert_eq!(format!("{mode:?}").to_uppercase(), v["mode"].as_str().unwrap(), "{}", v["name"]);
            valid += 1;
        } else {
            assert_eq!(got.unwrap_err().to_string(), v["error"].as_str().unwrap(), "{}", v["name"]);
            invalid += 1;
        }
    }
    assert_eq!((valid, invalid), (18, 26));
}

/// Copies `files/` and `expected/` of `from` into `to`.
fn copy_vectors(from: &Path, to: &Path) {
    for sub in ["files", "expected"] {
        std::fs::create_dir_all(to.join(sub)).unwrap();
        for e in std::fs::read_dir(from.join(sub)).unwrap() {
            let e = e.unwrap();
            std::fs::copy(e.path(), to.join(sub).join(e.file_name())).unwrap();
        }
    }
}

fn check(dir: &Path) -> assert_cmd::assert::Assert {
    Command::cargo_bin("gen-testdata").unwrap().arg("--check").arg(dir).assert()
}

#[test]
fn check_reads_every_vector_of_a_directory() {
    let _lock = TESTDATA.lock().unwrap_or_else(|e| e.into_inner());
    check(&testdata()).success().stdout("24 vectors match\n").stderr("");

    // the vectors of another writer: files/ and expected/ only (no manifests needed)
    let dir = tempfile::tempdir().unwrap();
    copy_vectors(&testdata(), dir.path());
    std::fs::remove_file(dir.path().join("files/manifest.json")).unwrap();
    check(dir.path()).success().stdout("24 vectors match\n");

    // a changed row is reported by vector and line
    let path = dir.path().join("expected/temporal.jsonl");
    let mut rows: Vec<String> = std::fs::read_to_string(&path).unwrap().lines().map(String::from).collect();
    rows[2] = rows[2].replacen('1', "2", 1);
    std::fs::write(&path, rows.join("\n") + "\n").unwrap();
    check(dir.path())
        .code(1)
        .stdout("")
        .stderr("gen-testdata: temporal: line 3 differs from expected/temporal.jsonl\n");

    // missing or extra rows are differences too
    std::fs::write(&path, "").unwrap();
    check(dir.path()).code(1).stderr("gen-testdata: temporal: line 1 differs from expected/temporal.jsonl\n");
    let rows = std::fs::read_to_string(testdata().join("expected/temporal.jsonl")).unwrap();
    std::fs::write(&path, rows.clone() + "{}\n").unwrap();
    let n = rows.lines().count();
    check(dir.path())
        .code(1)
        .stderr(format!("gen-testdata: temporal: line {} differs from expected/temporal.jsonl\n", n + 1));

    // every file needs its expected rows
    std::fs::remove_file(&path).unwrap();
    check(dir.path()).code(1).stderr(predicates::str::starts_with("gen-testdata: temporal: "));
}

#[test]
fn check_refuses_a_directory_without_vectors_or_with_a_corrupt_file() {
    let dir = tempfile::tempdir().unwrap();
    check(dir.path()).code(1).stderr(predicates::str::starts_with("gen-testdata: "));
    std::fs::create_dir_all(dir.path().join("files")).unwrap();
    std::fs::create_dir_all(dir.path().join("expected")).unwrap();
    std::fs::write(dir.path().join("files/notes.txt"), b"not a vector").unwrap();
    check(dir.path()).code(1).stderr(format!("gen-testdata: no files/*.parquet in {}\n", dir.path().display()));
    std::fs::write(dir.path().join("files/bad.parquet"), b"PAR1 not parquet").unwrap();
    std::fs::write(dir.path().join("expected/bad.jsonl"), b"").unwrap();
    check(dir.path()).code(1).stderr(predicates::str::starts_with("gen-testdata: bad: "));
}

#[test]
fn column_chunk_offsets_are_in_order() {
    // every reader can find the pages: a dictionary page comes before the data pages, and a
    // chunk without data pages has no dictionary page either (arrow-rs then writes
    // data_page_offset 0, which arrow-go's GetDictionaryPage cannot take after a dictionary)
    use tparquet::parquet::file::metadata::ParquetMetaDataReader;
    let _lock = TESTDATA.lock().unwrap_or_else(|e| e.into_inner());
    for e in std::fs::read_dir(testdata().join("files")).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_none_or(|x| x != "parquet") {
            continue;
        }
        let file = bytes::Bytes::from(std::fs::read(&path).unwrap());
        let metadata = ParquetMetaDataReader::new().parse_and_finish(&file).unwrap();
        for rg in metadata.row_groups() {
            for c in rg.columns() {
                let name = format!("{} {}", path.display(), c.column_path());
                if let Some(dictionary) = c.dictionary_page_offset() {
                    assert!(dictionary < c.data_page_offset(), "{name}");
                }
                if rg.num_rows() == 0 {
                    assert_eq!((c.dictionary_page_offset(), c.compressed_size()), (None, 0), "{name}");
                }
            }
        }
    }
    // the file with a row group without rows has no pages at all
    let report =
        tparquet::inspect::inspect(&std::fs::File::open(testdata().join("files/empty.parquet")).unwrap()).unwrap();
    assert!(report.columns.iter().all(|c| c.pages.is_empty() && c.modes.is_empty()), "{report:?}");
}
