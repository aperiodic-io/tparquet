//! `tparquet`: encode, decode, inspect and verify Parquet files with the PCODEC codec.

use clap::{Parser, Subcommand};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tparquet::parquet::basic::Compression;
use tparquet::policy::parse_column_policy;
use tparquet::writer::default_base;
use tparquet::{Error, Policy, Target, WriterOptions, inspect, transcode, verify};

#[derive(Parser)]
#[command(name = "tparquet", version, about = "Parquet files with the PCODEC compression codec")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Rewrite a Parquet file with PCODEC for the numeric columns the policy selects
    Encode {
        input: PathBuf,
        output: PathBuf,
        /// Policy of the INT32/INT64/FLOAT/DOUBLE columns
        #[arg(long, default_value = "auto", value_parser = parse_policy)]
        policy: Policy,
        /// Policy of one column (a top-level name or a dotted leaf path); repeatable
        #[arg(long = "column", value_name = "NAME=POLICY", value_parser = parse_column)]
        columns: Vec<(String, Policy)>,
        /// pcodec compression level, 0..=12
        #[arg(long, default_value_t = 8)]
        level: u32,
        /// Codec of the other columns, such as zstd(3), snappy or uncompressed
        #[arg(long, value_parser = parse_codec)]
        base: Option<Compression>,
    },
    /// Rewrite a Parquet file as standard Parquet (PCODEC columns get the base codec)
    Decode {
        input: PathBuf,
        output: PathBuf,
        /// Codec that replaces PCODEC
        #[arg(long, value_parser = parse_codec)]
        base: Option<Compression>,
    },
    /// Show codecs, sizes, pages and PCODEC payload modes per column
    Inspect {
        file: PathBuf,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Compare two files' data (floats bit for bit); exit code 1 if they differ
    Verify { a: PathBuf, b: PathBuf },
}

fn parse_policy(s: &str) -> Result<Policy, String> {
    s.parse().map_err(|e: Error| e.to_string())
}

fn parse_column(s: &str) -> Result<(String, Policy), String> {
    parse_column_policy(s).map_err(|e| e.to_string())
}

fn parse_codec(s: &str) -> Result<Compression, String> {
    tparquet::parse_base(s).map_err(|e| e.to_string())
}

fn open(path: &Path) -> Result<File, Error> {
    File::open(path).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
}

/// Writes to a new temporary file next to `path` (`.NAME.PID.tmp-tparquet`), renamed into
/// place once complete. The temporary file is created only if no file has its name, so it can
/// never truncate the input or any other file.
fn write_atomically(path: &Path, f: impl FnOnce(&mut BufWriter<File>) -> Result<(), Error>) -> Result<(), Error> {
    let name = path.file_name().ok_or_else(|| Error::Invalid(format!("{}: not a file name", path.display())))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.tmp-tparquet", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| Error::Invalid(format!("{}: {e}", tmp.display())))?;
    let mut w = BufWriter::new(file);
    let done = f(&mut w).and_then(|()| Ok(w.flush()?));
    match done {
        Ok(()) => Ok(std::fs::rename(&tmp, path)?),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, Error> {
    match cli.command {
        Command::Encode { input, output, policy, columns, level, base } => {
            let mut options = WriterOptions::default().with_policy(policy).with_level(level)?;
            options.columns = columns;
            options.base = base.unwrap_or_else(default_base);
            let input = open(&input)?;
            write_atomically(&output, |w| transcode(input, w, &Target::Pcodec(options)).map(drop))?;
        }
        Command::Decode { input, output, base } => {
            let input = open(&input)?;
            let target = Target::Standard { base: base.unwrap_or_else(default_base) };
            write_atomically(&output, |w| transcode(input, w, &target).map(drop))?;
        }
        Command::Inspect { file, json } => {
            let report = inspect::inspect(&open(&file)?)?;
            if json {
                println!("{}", report.to_json());
            } else {
                print!("{}", report.to_text());
            }
        }
        Command::Verify { a, b } => match verify::verify(open(&a)?, open(&b)?)? {
            None => println!("identical"),
            Some(d) => {
                println!("{d}");
                return Ok(ExitCode::from(1));
            }
        },
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // a panic on a malformed file is reported like any other error
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(|| run(cli)).unwrap_or_else(|panic| {
        let msg = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        Err(Error::Invalid(format!("internal error: {msg}")))
    });
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tparquet: {e}");
            ExitCode::from(2)
        }
    }
}
