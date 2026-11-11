//! The acceptance hook `tparquet-datafusion-vectors`: `check DIR` and `write OUT SRC...`, with
//! the contract of `acceptance/cross/vectors.py` and `gen-testdata --check`.

use crate::common::inspect::inspect_file;
use crate::common::*;
use std::path::Path;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tparquet-datafusion-vectors")).args(args).output().unwrap()
}

fn check(dir: &Path) -> Output {
    run(&["check", dir.to_str().unwrap()])
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
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

#[test]
fn check_reads_every_vector_of_a_directory() {
    let out = check(&testdata());
    assert_eq!(
        (out.status.code(), text(&out.stdout), text(&out.stderr)),
        (Some(0), "24 vectors match\n".into(), "".into())
    );

    // the vectors of another writer: files/ and expected/ only (no manifests needed)
    let dir = tempfile::tempdir().unwrap();
    copy_vectors(&testdata(), dir.path());
    std::fs::remove_file(dir.path().join("files/manifest.json")).unwrap();
    assert_eq!(text(&check(dir.path()).stdout), "24 vectors match\n");

    // a changed row is reported by vector and line (as acceptance/cross/run.sh tampers)
    let path = dir.path().join("expected/temporal.jsonl");
    let mut rows: Vec<String> = std::fs::read_to_string(&path).unwrap().lines().map(String::from).collect();
    rows[1] = rows[1].replacen('1', "2", 1);
    std::fs::write(&path, rows.join("\n") + "\n").unwrap();
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stdout), "");
    assert_eq!(
        text(&out.stderr),
        "tparquet-datafusion-vectors: temporal: line 2 differs from expected/temporal.jsonl\n"
    );

    // missing or extra rows are differences too
    std::fs::write(&path, "").unwrap();
    assert_eq!(
        text(&check(dir.path()).stderr),
        "tparquet-datafusion-vectors: temporal: line 1 differs from expected/temporal.jsonl\n"
    );
    let rows = expected_rows("temporal");
    std::fs::write(&path, rows.clone() + "{}\n").unwrap();
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        format!(
            "tparquet-datafusion-vectors: temporal: line {} differs from expected/temporal.jsonl\n",
            rows.lines().count() + 1
        )
    );

    // every file needs its expected rows
    std::fs::remove_file(&path).unwrap();
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stderr), "tparquet-datafusion-vectors: temporal: no expected/temporal.jsonl\n");
}

#[test]
fn check_refuses_a_directory_without_vectors_or_with_a_corrupt_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with(&format!("tparquet-datafusion-vectors: {}/files: ", dir.path().display())));
    std::fs::create_dir_all(dir.path().join("files")).unwrap();
    std::fs::create_dir_all(dir.path().join("expected")).unwrap();
    std::fs::write(dir.path().join("files/notes.txt"), b"not a vector").unwrap();
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        format!("tparquet-datafusion-vectors: no files/*.parquet in {}\n", dir.path().display())
    );
    std::fs::write(dir.path().join("files/bad.parquet"), b"PAR1 not parquet").unwrap();
    std::fs::write(dir.path().join("expected/bad.jsonl"), b"").unwrap();
    let out = check(dir.path());
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with("tparquet-datafusion-vectors: bad: "), "{}", text(&out.stderr));
}

#[test]
fn write_rewrites_every_vector_with_copy_stored_as_tparquet() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("datafusion");
    // stale files are removed
    std::fs::create_dir_all(out.join("files")).unwrap();
    std::fs::write(out.join("files/stale.parquet"), b"x").unwrap();
    let o = run(&["write", out.to_str().unwrap(), testdata().to_str().unwrap()]);
    assert_eq!(
        (o.status.code(), text(&o.stdout), text(&o.stderr)),
        (Some(0), "24 vectors written\n".into(), "".into())
    );
    let mut names: Vec<String> = std::fs::read_dir(out.join("files"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut want: Vec<String> = vector_names().iter().map(|n| format!("{n}.parquet")).collect();
    want.sort();
    assert_eq!(names, want);
    for name in vector_names() {
        assert_eq!(std::fs::read_to_string(out.join(format!("expected/{name}.jsonl"))).unwrap(), expected_rows(&name));
        let r = inspect_file(out.join(format!("files/{name}.parquet")));
        for c in &r.columns {
            if r.rows > 0 && ["INT32", "INT64", "FLOAT", "DOUBLE"].contains(&c.physical_type.as_str()) {
                assert_eq!(c.codecs, ["PCODEC"], "{name} {c:?}");
            }
        }
    }
    let o = check(&out);
    assert_eq!((o.status.code(), text(&o.stdout)), (Some(0), "24 vectors match\n".into()));

    // several sources: a name met twice keeps the last
    let second = dir.path().join("second");
    copy_vectors(&testdata(), &second);
    for e in std::fs::read_dir(second.join("files")).unwrap() {
        let p = e.unwrap().path();
        if p.file_stem().unwrap() != "mixed" {
            std::fs::remove_file(p).unwrap();
        }
    }
    let o = run(&["write", out.to_str().unwrap(), testdata().to_str().unwrap(), second.to_str().unwrap()]);
    assert_eq!(text(&o.stdout), "25 vectors written\n");
    assert_eq!(text(&check(&out).stdout), "24 vectors match\n");
}

#[test]
fn write_fails_on_a_bad_source() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("files")).unwrap();
    std::fs::create_dir_all(src.join("expected")).unwrap();
    std::fs::write(src.join("files/bad.parquet"), b"PAR1 not parquet").unwrap();
    std::fs::write(src.join("expected/bad.jsonl"), b"").unwrap();
    let o = run(&["write", dir.path().join("out").to_str().unwrap(), src.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).starts_with("tparquet-datafusion-vectors: bad: "), "{}", text(&o.stderr));
    let o = run(&["write", dir.path().join("out").to_str().unwrap(), dir.path().join("missing").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn usage_errors() {
    for args in [&[][..], &["check"], &["check", "a", "b"], &["write", "out"], &["frob", "x"]] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&o.stderr), "usage: tparquet-datafusion-vectors check DIR | write OUT SRC...\n");
        assert_eq!(text(&o.stdout), "");
    }
}
