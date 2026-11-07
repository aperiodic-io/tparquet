//! The `package-extension` binary.

use std::process::Command;
use tparquet_duckdb::metadata::{Api, Metadata, current_platform, signing_hash};

fn run(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_package-extension")).args(args).output().unwrap()
}

#[test]
fn appends_the_footer_for_this_or_a_named_platform() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("lib.so");
    std::fs::write(&lib, b"\x7fELF not really").unwrap();
    let out = dir.path().join("tparquet.duckdb_extension");
    let r = run(&[lib.as_os_str(), out.as_os_str()]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let file = std::fs::read(&out).unwrap();
    assert!(file.starts_with(b"\x7fELF not really"));
    assert_eq!(Metadata::parse(&file), Some(Metadata::for_platform(current_platform().unwrap())));

    let r = run(&[lib.as_os_str(), out.as_os_str(), "osx_arm64".as_ref()]);
    assert!(r.status.success());
    assert_eq!(Metadata::parse(&std::fs::read(&out).unwrap()).unwrap().platform, "osx_arm64");
}

#[test]
fn explains_its_errors() {
    let dir = tempfile::tempdir().unwrap();
    let r = run(&[]);
    assert_eq!(r.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&r.stderr),
        "package-extension: usage: package-extension [--api v1|v2] LIBRARY OUTPUT [PLATFORM]\n"
    );
    let missing = dir.path().join("missing.so");
    let r = run(&[missing.as_os_str(), dir.path().join("x").as_os_str()]);
    assert!(!r.status.success());
    assert!(
        String::from_utf8_lossy(&r.stderr)
            .starts_with(&format!("package-extension: cannot read {}", missing.display()))
    );
    let lib = dir.path().join("lib.so");
    std::fs::write(&lib, b"lib").unwrap();
    let r = run(&[lib.as_os_str(), dir.path().join("no/such/dir/x").as_os_str()]);
    assert!(String::from_utf8_lossy(&r.stderr).starts_with("package-extension: cannot write "));
    let r = run(&[lib.as_os_str(), dir.path().join("x").as_os_str(), "a-platform-name-longer-than-32-bytes".as_ref()]);
    assert!(String::from_utf8_lossy(&r.stderr).contains("must be at most 32 ASCII bytes"));
    let r = run(&["--api".as_ref(), "v3".as_ref(), lib.as_os_str(), dir.path().join("x").as_os_str()]);
    assert_eq!(r.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&r.stderr), "package-extension: unknown API \"v3\": v1 or v2\n");
}

#[test]
fn packages_either_build() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("lib.so");
    std::fs::write(&lib, b"lib").unwrap();
    let out = dir.path().join("tparquet.duckdb_extension");
    for (flag, api) in [("v1", Api::V1), ("v2", Api::V2)] {
        let r = run(&["--api".as_ref(), flag.as_ref(), lib.as_os_str(), out.as_os_str(), "linux_amd64".as_ref()]);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        assert_eq!(Metadata::parse(&std::fs::read(&out).unwrap()), Some(Metadata::for_api("linux_amd64", api)));
    }
    assert_ne!(Metadata::for_api("linux_amd64", Api::V1), Metadata::for_api("linux_amd64", Api::V2));
}

#[test]
fn hashes_and_signs_a_packaged_file() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("lib.so");
    std::fs::write(&lib, b"lib").unwrap();
    let ext = dir.path().join("tparquet.duckdb_extension");
    assert!(run(&[lib.as_os_str(), ext.as_os_str(), "linux_amd64".as_ref()]).status.success());
    let digest = dir.path().join("digest");
    let r = run(&["signing-hash".as_ref(), ext.as_os_str(), digest.as_os_str()]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let packaged = std::fs::read(&ext).unwrap();
    assert_eq!(std::fs::read(&digest).unwrap(), signing_hash(&packaged).unwrap());

    let signature = dir.path().join("sig");
    std::fs::write(&signature, [7u8; 256]).unwrap();
    let r = run(&["set-signature".as_ref(), ext.as_os_str(), signature.as_os_str()]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let signed = std::fs::read(&ext).unwrap();
    assert_eq!(signed.len(), packaged.len());
    assert!(signed.ends_with(&[7u8; 256]));
    // the signature is outside what is hashed
    assert_eq!(signing_hash(&signed).unwrap(), signing_hash(&packaged).unwrap());

    let r = run(&["signing-hash".as_ref(), lib.as_os_str(), digest.as_os_str()]);
    assert_eq!(r.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&r.stderr).contains("no metadata footer"),
        "{}",
        String::from_utf8_lossy(&r.stderr)
    );
    std::fs::write(&signature, [7u8; 10]).unwrap();
    let r = run(&["set-signature".as_ref(), ext.as_os_str(), signature.as_os_str()]);
    assert_eq!(r.status.code(), Some(1));
    assert_eq!(std::fs::read(&ext).unwrap(), signed);
}
