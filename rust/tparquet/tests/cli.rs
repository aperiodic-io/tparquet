mod common;

use arrow_array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn cli() -> Command {
    Command::cargo_bin("tparquet").unwrap()
}

fn batch(rows: i64, shift: f64) -> RecordBatch {
    let ts = Int64Array::from_iter_values((0..rows).map(|i| 1_700_000_000_000 + i * 50));
    let px = Float64Array::from_iter_values((0..rows).map(|i| ((i * 7) % 300) as f64 / 100.0 + shift));
    let s = StringArray::from_iter_values((0..rows).map(|i| format!("v{}", i % 5)));
    RecordBatch::try_from_iter([
        ("ts", Arc::new(ts) as ArrayRef),
        ("px", Arc::new(px) as ArrayRef),
        ("s", Arc::new(s) as ArrayRef),
    ])
    .unwrap()
}

/// A standard Parquet file in `dir`.
fn standard(dir: &Path, name: &str, shift: f64) -> PathBuf {
    let path = dir.join(name);
    let b = batch(3000, shift);
    let mut w = tparquet::parquet::arrow::ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), b.schema(), None)
        .unwrap();
    w.write(&b).unwrap();
    w.close().unwrap();
    path
}

/// No `.*.tmp-tparquet` file is left in `dir`.
fn assert_no_temporary_files(dir: &Path) {
    for e in std::fs::read_dir(dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        assert!(!name.ends_with(".tmp-tparquet") || !name.starts_with('.'), "{name}");
    }
}

fn inspect_json(path: &Path) -> serde_json::Value {
    let out = cli().arg("inspect").arg(path).arg("--json").output().unwrap();
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

fn codecs(path: &Path) -> Vec<String> {
    inspect_json(path)["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["codecs"][0].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn encode_inspect_decode_verify() {
    let dir = tempfile::tempdir().unwrap();
    let input = standard(dir.path(), "in.parquet", 0.0);
    let enc = dir.path().join("enc.parquet");
    let dec = dir.path().join("dec.parquet");

    cli().arg("encode").arg(&input).arg(&enc).assert().success().stdout("").stderr("");
    assert_eq!(codecs(&enc), ["PCODEC", "PCODEC", "ZSTD"]);
    assert_eq!(inspect_json(&enc)["pcodec_version"], "1");
    cli()
        .arg("inspect")
        .arg(&enc)
        .assert()
        .success()
        .stdout(predicate::str::starts_with("rows: 3000, row groups: 1, parquet.pcodec.version = 1\nts INT64 PCODEC: "))
        .stdout(predicate::str::contains("modes: PCO=1"));

    cli().arg("verify").arg(&input).arg(&enc).assert().code(0).stdout("identical\n");
    cli().arg("decode").arg(&enc).arg(&dec).assert().success();
    assert_eq!(codecs(&dec), ["ZSTD", "ZSTD", "ZSTD"]);
    assert_eq!(inspect_json(&dec)["pcodec_version"], serde_json::Value::Null);
    cli().arg("verify").arg(&input).arg(&dec).assert().code(0);
    assert_no_temporary_files(dir.path());
}

#[test]
fn encode_options() {
    let dir = tempfile::tempdir().unwrap();
    let input = standard(dir.path(), "in.parquet", 0.0);
    let out = dir.path().join("out.parquet");
    let run = |args: &[&str]| {
        cli().arg("encode").arg(&input).arg(&out).args(args).assert().success();
        codecs(&out)
    };
    assert_eq!(run(&["--policy", "base"]), ["ZSTD", "ZSTD", "ZSTD"]);
    assert_eq!(run(&["--policy", "pcodec", "--column", "px=base"]), ["PCODEC", "ZSTD", "ZSTD"]);
    assert_eq!(run(&["--policy", "base", "--column", "px=pcodec", "--base", "snappy"]), ["SNAPPY", "PCODEC", "SNAPPY"]);
    assert_eq!(run(&["--base", "zstd(9)", "--level", "0", "--column", "ts=auto"]), ["PCODEC", "PCODEC", "ZSTD"]);
    let l0 = inspect_json(&out)["columns"][1]["compressed_bytes"].as_i64().unwrap();
    run(&["--level", "12"]);
    let l12 = inspect_json(&out)["columns"][1]["compressed_bytes"].as_i64().unwrap();
    assert_ne!(l0, l12);
    cli().arg("decode").arg(&out).arg(dir.path().join("d.parquet")).args(["--base", "gzip(6)"]).assert().success();
    assert_eq!(codecs(&dir.path().join("d.parquet")), ["GZIP", "GZIP", "ZSTD"]);
}

#[test]
fn verify_reports_differences_with_exit_code_1() {
    let dir = tempfile::tempdir().unwrap();
    let a = standard(dir.path(), "a.parquet", 0.0);
    let b = standard(dir.path(), "b.parquet", 0.5);
    cli().arg("verify").arg(&a).arg(&b).assert().code(1).stdout(predicate::str::starts_with(
        "row 0 differs:\n  a: {\"ts\":1700000000000,\"px\":\"0x0000000000000000\"",
    ));
}

#[test]
fn bad_arguments_and_inputs_are_clean_errors() {
    let dir = tempfile::tempdir().unwrap();
    let input = standard(dir.path(), "in.parquet", 0.0);
    let out = dir.path().join("out.parquet");
    let garbage = dir.path().join("garbage.parquet");
    std::fs::write(&garbage, b"PAR1 this is not a parquet file PAR1").unwrap();
    let missing = dir.path().join("missing.parquet");

    let fails = |args: Vec<&std::ffi::OsStr>, code: i32, msg: &str| {
        cli()
            .args(args)
            .assert()
            .code(code)
            .stdout("")
            .stderr(predicate::str::contains(msg))
            .stderr(predicate::str::contains("panicked").not());
    };
    let p = |s: &'static str| std::ffi::OsStr::new(s);
    let (i, o, g, m) = (input.as_os_str(), out.as_os_str(), garbage.as_os_str(), missing.as_os_str());
    fails(
        vec![p("encode"), i, o, p("--policy"), p("fast")],
        2,
        "unknown policy \"fast\": expected auto, pcodec or base",
    );
    fails(vec![p("encode"), i, o, p("--column"), p("px")], 2, "expected NAME=policy, got \"px\"");
    fails(vec![p("encode"), i, o, p("--column"), p("nope=base")], 2, "tparquet: no column named \"nope\"");
    fails(
        vec![p("encode"), i, o, p("--level"), p("13")],
        2,
        "tparquet: parquet: Parquet error: valid compression range 0..=12 exceeded.",
    );
    fails(vec![p("encode"), i, o, p("--base"), p("pcodec")], 2, "the base codec cannot be PCODEC");
    fails(vec![p("encode"), i, o, p("--base"), p("lzma")], 2, "unsupported compression lzma");
    // the parquet crate's parser panics on these, or reads "zstd(3x" as zstd(3)
    for bad in ["zstd(", "zstd(3", "zstd(3x", "gzip(é", "zstd)("] {
        fails(vec![p("encode"), i, o, p("--base"), p(bad)], 2, "expected NAME or NAME(LEVEL)");
    }
    // no Parquet writer implements LZO
    fails(vec![p("encode"), i, o, p("--base"), p("lzo")], 2, "the base codec cannot be LZO");
    fails(vec![p("encode"), m, o], 2, &format!("tparquet: {}: No such file", missing.display()));
    fails(vec![p("encode"), g, o], 2, "tparquet: parquet: ");
    fails(vec![p("decode"), g, o], 2, "tparquet: parquet: ");
    fails(vec![p("inspect"), g], 2, "tparquet: parquet: ");
    fails(vec![p("inspect"), m], 2, "No such file");
    fails(vec![p("verify"), i, g], 2, "tparquet: parquet: ");
    fails(vec![p("verify"), m, i], 2, "No such file");
    fails(vec![p("frobnicate")], 2, "unrecognized subcommand");
    assert!(!out.exists());
    assert_no_temporary_files(dir.path());

    let unwritable = dir.path().join("no-such-dir").join("out.parquet");
    fails(vec![p("encode"), i, unwritable.as_os_str()], 2, ".tmp-tparquet: No such file");
    fails(vec![p("encode"), i, p("/")], 2, "tparquet: /: not a file name");
}

#[test]
fn corrupt_files_never_panic() {
    let dir = tempfile::tempdir().unwrap();
    let input = standard(dir.path(), "in.parquet", 0.0);
    let enc = dir.path().join("enc.parquet");
    cli().arg("encode").arg(&input).arg(&enc).args(["--policy", "pcodec"]).assert().success();
    let bytes = std::fs::read(&enc).unwrap();
    let bad = dir.path().join("bad.parquet");
    for i in (0..bytes.len()).step_by(bytes.len() / 60) {
        let mut b = bytes.clone();
        b[i] ^= 0x5a;
        std::fs::write(&bad, &b).unwrap();
        for cmd in ["inspect", "decode"] {
            let mut c = cli();
            c.arg(cmd).arg(&bad);
            if cmd == "decode" {
                c.arg(dir.path().join("dec.parquet"));
            }
            let out = c.output().unwrap();
            let code = out.status.code();
            assert!(code == Some(0) || code == Some(2), "{cmd} byte {i}: {code:?}");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!stderr.contains("panicked"), "{cmd} byte {i}: {stderr}");
            assert!(code == Some(0) || stderr.starts_with("tparquet: "), "{cmd} byte {i}: {stderr}");
        }
    }
}

#[test]
fn reader_panics_are_reported_as_errors() {
    // a one-bit corruption that makes the reader panic (an index out of bounds in the
    // upstream parquet code): the CLI reports it like any other error
    let dir = tempfile::tempdir().unwrap();
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("../parquet/tests/arrow_reader/bigdelta.parquet");
    let enc = dir.path().join("enc.parquet");
    cli().arg("encode").arg(&input).arg(&enc).args(["--policy", "pcodec"]).assert().success();
    let mut b = std::fs::read(&enc).unwrap();
    b[31] ^= 1;
    let bad = dir.path().join("bad.parquet");
    std::fs::write(&bad, &b).unwrap();
    cli()
        .arg("decode")
        .arg(&bad)
        .arg(dir.path().join("dec.parquet"))
        .assert()
        .code(2)
        .stderr("tparquet: internal error: offset + len out of bounds\n");
}

#[test]
fn the_temporary_file_never_replaces_the_input() {
    // the output is written next to itself first; that file must not be the input
    let dir = tempfile::tempdir().unwrap();
    let input = standard(dir.path(), "x.tmp-tparquet", 0.0);
    let before = std::fs::read(&input).unwrap();
    let out = dir.path().join("x.parquet");
    cli().arg("encode").arg(&input).arg(&out).assert().success();
    assert_eq!(std::fs::read(&input).unwrap(), before);
    cli().arg("verify").arg(&input).arg(&out).assert().code(0);
    // nor an unrelated file of the same name, which stays as it was
    let other = dir.path().join("y.tmp-tparquet");
    std::fs::write(&other, b"keep me").unwrap();
    cli().arg("decode").arg(&out).arg(dir.path().join("y.parquet")).assert().success();
    assert_eq!(std::fs::read(&other).unwrap(), b"keep me");
    // encoding a file in place replaces it
    let copy = dir.path().join("copy.parquet");
    std::fs::copy(&input, &copy).unwrap();
    cli().arg("encode").arg(&copy).arg(&copy).assert().success();
    assert_eq!(codecs(&copy), ["PCODEC", "PCODEC", "ZSTD"]);
    cli().arg("verify").arg(&input).arg(&copy).assert().code(0);
    let names: Vec<String> =
        std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(names.len(), 5, "{names:?}");
}

#[test]
fn help_version_and_usage() {
    cli()
        .arg("--help")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("encode"))
        .stdout(predicate::str::contains("verify"));
    cli().arg("--version").assert().code(0).stdout(predicate::str::starts_with("tparquet "));
    for cmd in ["encode", "decode", "inspect", "verify"] {
        cli().arg(cmd).arg("--help").assert().code(0);
        // a missing argument is a usage error
        cli().arg(cmd).assert().code(2).stdout("").stderr(predicate::str::contains("Usage"));
    }
    cli().assert().code(2);
    cli().args(["encode", "a", "b", "--level", "-1"]).assert().code(2);
    cli().args(["encode", "a", "b", "--nope"]).assert().code(2).stderr(predicate::str::contains("unexpected argument"));
    cli().args(["verify", "a"]).assert().code(2);
}

#[test]
fn verify_exit_codes_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let a = standard(dir.path(), "a.parquet", 0.0);
    let same = standard(dir.path(), "same.parquet", 0.0);
    cli().arg("verify").arg(&a).arg(&same).assert().code(0).stdout("identical\n").stderr("");
    // fewer rows
    let short = dir.path().join("short.parquet");
    let b = batch(10, 0.0);
    let mut w =
        tparquet::parquet::arrow::ArrowWriter::try_new(std::fs::File::create(&short).unwrap(), b.schema(), None)
            .unwrap();
    w.write(&b).unwrap();
    w.close().unwrap();
    cli().arg("verify").arg(&a).arg(&short).assert().code(1).stdout("row counts differ: 3000 vs 10\n");
    cli().arg("verify").arg(&short).arg(&a).assert().code(1).stdout("row counts differ: 10 vs 3000\n");
    // another schema
    let other = dir.path().join("other.parquet");
    let b = RecordBatch::try_from_iter([("ts", Arc::new(Float64Array::from(vec![1.0])) as ArrayRef)]).unwrap();
    let mut w =
        tparquet::parquet::arrow::ArrowWriter::try_new(std::fs::File::create(&other).unwrap(), b.schema(), None)
            .unwrap();
    w.write(&b).unwrap();
    w.close().unwrap();
    cli()
        .arg("verify")
        .arg(&a)
        .arg(&other)
        .assert()
        .code(1)
        .stdout(predicate::str::starts_with("schemas differ:\n  a: "));
    // a directory is an error, not a difference
    cli().arg("verify").arg(&a).arg(dir.path()).assert().code(2).stdout("");
    cli().arg("inspect").arg(dir.path()).assert().code(2);
}

#[test]
fn every_vector_encodes_decodes_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let files = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/files");
    let mut n = 0;
    for e in std::fs::read_dir(&files).unwrap() {
        let path = e.unwrap().path();
        if path.extension().is_none_or(|x| x != "parquet") {
            continue;
        }
        n += 1;
        let dec = dir.path().join("dec.parquet");
        let enc = dir.path().join("enc.parquet");
        cli().arg("decode").arg(&path).arg(&dec).assert().success();
        let all_codecs = |p: &Path| -> Vec<String> {
            let r = inspect_json(p);
            let cols = r["columns"].as_array().unwrap().clone();
            cols.iter()
                .flat_map(|c| c["codecs"].as_array().unwrap().clone())
                .map(|c| c.as_str().unwrap().to_string())
                .collect()
        };
        assert!(all_codecs(&dec).iter().all(|c| c != "PCODEC"), "{}", path.display());
        assert_eq!(inspect_json(&dec)["pcodec_version"], serde_json::Value::Null);
        cli().arg("verify").arg(&path).arg(&dec).assert().code(0);
        cli().arg("encode").arg(&dec).arg(&enc).args(["--policy", "pcodec"]).assert().success();
        cli().arg("verify").arg(&path).arg(&enc).assert().code(0);
        cli().arg("verify").arg(&enc).arg(&dec).assert().code(0);
        let report = inspect_json(&enc);
        let original = inspect_json(&path);
        assert_eq!(report["rows"], original["rows"]);
        cli().arg("inspect").arg(&path).assert().success();
    }
    assert_eq!(n, 24);
    assert_no_temporary_files(dir.path());
}

#[test]
fn encode_column_options_on_nested_paths() {
    let dir = tempfile::tempdir().unwrap();
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/files/nested_struct.parquet");
    let out = dir.path().join("out.parquet");
    let paths: Vec<String> = inspect_json(&input)["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect();
    let leaf = paths.iter().find(|p| p.contains('.')).unwrap().clone();
    let top = leaf.split('.').next().unwrap().to_string();
    // the subtree to base, then one leaf back to pcodec (the longer name wins, in any order)
    for args in [
        vec!["--policy", "pcodec", "--column", &format!("{top}=base"), "--column", &format!("{leaf}=pcodec")],
        vec!["--policy", "pcodec", "--column", &format!("{leaf}=pcodec"), "--column", &format!("{top}=base")],
    ] {
        cli().arg("encode").arg(&input).arg(&out).args(&args).assert().success();
        let report = inspect_json(&out);
        for c in report["columns"].as_array().unwrap() {
            let path = c["path"].as_str().unwrap();
            if path == leaf {
                assert_eq!(c["codecs"][0], "PCODEC");
            } else if path.starts_with(&format!("{top}.")) {
                assert_ne!(c["codecs"][0], "PCODEC", "{path}");
            }
        }
        cli().arg("verify").arg(&input).arg(&out).assert().code(0);
    }
    // the last of two policies for the same name wins
    cli()
        .arg("encode")
        .arg(&input)
        .arg(&out)
        .args(["--policy", "base", "--column", &format!("{leaf}=pcodec"), "--column", &format!("{leaf}=base")])
        .assert()
        .success();
    assert!(codecs(&out).iter().all(|c| c != "PCODEC"));
}
