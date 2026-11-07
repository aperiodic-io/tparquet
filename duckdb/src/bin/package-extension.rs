//! `package-extension [--api v1|v2] LIBRARY OUTPUT [PLATFORM]`: appends DuckDB's extension
//! metadata to the built shared library, giving the loadable `tparquet.duckdb_extension`.
//!
//! `package-extension signing-hash FILE DIGEST` writes the 32-byte digest DuckDB checks the
//! signature of a packaged extension against; `package-extension set-signature FILE SIGNATURE`
//! puts a 256-byte signature of that digest in the file (`sign.sh` signs with `openssl`).
//!
//! `--api` picks the build (`v1`, the default: C API v1.5.6, any DuckDB that serves it; `v2`:
//! the unstable C API v2, DuckDB 2.0.0 only). PLATFORM defaults to the DuckDB name of the
//! platform this tool runs on (`linux_amd64`...).

use std::process::ExitCode;
use tparquet_duckdb::metadata::{Api, Metadata, current_platform, package, signing_hash, with_signature};

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {path}: {e}"))
}

fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, file, digest] if cmd == "signing-hash" => return write(digest, &signing_hash(&read(file)?)?),
        [cmd, file, signature] if cmd == "set-signature" => {
            return write(file, &with_signature(&read(file)?, &read(signature)?)?);
        }
        _ => {}
    }
    let (api, args) = match args {
        [flag, api, rest @ ..] if flag == "--api" => match api.as_str() {
            "v1" => (Api::V1, rest),
            "v2" => (Api::V2, rest),
            other => return Err(format!("unknown API {other:?}: v1 or v2")),
        },
        _ => (Api::V1, args),
    };
    let (library, output, platform) = match args {
        [l, o] => (l, o, current_platform().ok_or("unknown platform: pass it as the third argument")?.to_string()),
        [l, o, p] => (l, o, p.clone()),
        _ => return Err("usage: package-extension [--api v1|v2] LIBRARY OUTPUT [PLATFORM]".into()),
    };
    let bytes = std::fs::read(library).map_err(|e| format!("cannot read {library}: {e}"))?;
    let file = package(&bytes, &Metadata::for_api(&platform, api))?;
    std::fs::write(output, file).map_err(|e| format!("cannot write {output}: {e}"))?;
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("package-extension: {e}");
            ExitCode::FAILURE
        }
    }
}
