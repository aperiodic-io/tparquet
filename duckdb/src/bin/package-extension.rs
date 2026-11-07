//! `package-extension [--api v1|v2] LIBRARY OUTPUT [PLATFORM]`: appends DuckDB's extension
//! metadata to the built shared library, giving the loadable `tparquet.duckdb_extension`.
//!
//! `--api` picks the build (`v1`, the default: C API v1.5.6, any DuckDB that serves it; `v2`:
//! the unstable C API v2, DuckDB 2.0.0 only). PLATFORM defaults to the DuckDB name of the
//! platform this tool runs on (`linux_amd64`...).

use std::process::ExitCode;
use tparquet_duckdb::metadata::{Api, Metadata, current_platform, package};

fn run(args: &[String]) -> Result<(), String> {
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
