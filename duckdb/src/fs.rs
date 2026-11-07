//! Where `read_tparquet` reads its files from.
//!
//! The scan reads through a [`FileSystem`]: in the extension, DuckDB's own virtual file system
//! (local files, `httpfs`, S3, and any file system registered with DuckDB; [`crate::read`] and
//! [`crate::v2`]), so that DuckDB's settings and secrets apply; in the unit tests, [`LocalFs`].
//!
//! A file is an [`Input`]: random access to its bytes, which the Parquet reader takes as a
//! [`ChunkReader`].

use bytes::Bytes;
use parquet::errors::ParquetError;
use parquet::file::reader::{ChunkReader, Length};
use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

/// Random access to the bytes of one open file.
pub trait RandomAccess: Send + Sync {
    /// The size of the file in bytes.
    fn size(&self) -> u64;
    /// Fills `buf` with the bytes at `offset`; fails if the file ends before `buf` is full.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), String>;
}

/// The file systems the scan reads from.
pub trait FileSystem: Send + Sync {
    /// Opens the file at `path` for reading.
    fn open(&self, path: &str) -> Result<Input, String>;
    /// The files that match the glob `pattern`, sorted by name; an empty list if none does.
    fn glob(&self, pattern: &str) -> Result<Vec<String>, String>;
}

/// An open file the Parquet reader reads from (cheap to clone).
#[derive(Clone)]
pub struct Input {
    file: Arc<dyn RandomAccess>,
    path: Arc<str>,
}

impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Input").field("path", &self.path).finish()
    }
}

impl Input {
    pub fn new(path: &str, file: Arc<dyn RandomAccess>) -> Self {
        Self { file, path: path.into() }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    fn read(&self, start: u64, length: usize) -> Result<Vec<u8>, ParquetError> {
        let end = start.checked_add(length as u64).filter(|&end| end <= self.file.size());
        if end.is_none() {
            return Err(ParquetError::EOF(format!(
                "{}: {length} bytes at {start} are past the end of the file ({} bytes)",
                self.path,
                self.file.size()
            )));
        }
        let mut buf = vec![0; length];
        self.file.read_at(start, &mut buf).map_err(|e| ParquetError::External(format!("{}: {e}", self.path).into()))?;
        Ok(buf)
    }
}

impl Length for Input {
    fn len(&self) -> u64 {
        self.file.size()
    }
}

/// A sequential reader of an [`Input`] from some offset, as [`ChunkReader::get_read`] returns.
pub struct InputReader {
    input: Input,
    position: u64,
}

impl Read for InputReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.input.len().saturating_sub(self.position);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.input.file.read_at(self.position, &mut buf[..n]).map_err(std::io::Error::other)?;
        self.position += n as u64;
        Ok(n)
    }
}

impl ChunkReader for Input {
    type T = InputReader;

    fn get_read(&self, start: u64) -> parquet::errors::Result<InputReader> {
        Ok(InputReader { input: self.clone(), position: start })
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<Bytes> {
        self.read(start, length).map(Bytes::from)
    }
}

/// Whether a path names a file of a file system other than the local one: it starts with a
/// scheme (`s3://`, `https://`, `memory://`...).
pub fn is_remote(path: &str) -> bool {
    match path.find("://") {
        Some(i) => i > 0 && path[..i].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')),
        None => false,
    }
}

/// `pattern` with a leading `~/` replaced by the home directory, as DuckDB's file system (and so
/// `COPY ... TO`) takes it.
pub fn in_home(pattern: &str, home: Option<PathBuf>) -> String {
    match (pattern.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => pattern.to_string(),
    }
}

/// The local files matching `pattern` (with the `glob` crate), sorted.
pub fn local_glob(pattern: &str) -> Result<Vec<String>, String> {
    let paths = glob::glob(pattern).map_err(|e| format!("invalid glob {pattern:?}: {e}"))?;
    let mut matched: Vec<String> =
        paths.filter_map(Result::ok).filter(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned()).collect();
    matched.sort();
    Ok(matched)
}

/// The local file system, through `std::fs` (for the tests: the extension reads through DuckDB).
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalFs;

struct LocalFile(std::fs::File, u64);

impl RandomAccess for LocalFile {
    fn size(&self) -> u64 {
        self.1
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        std::os::unix::fs::FileExt::read_exact_at(&self.0, buf, offset).map_err(|e| e.to_string())
    }
}

impl FileSystem for LocalFs {
    fn open(&self, path: &str) -> Result<Input, String> {
        if is_remote(path) {
            return Err(format!("cannot open {path}: not a local file"));
        }
        let file = std::fs::File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
        let size = file.metadata().map_err(|e| format!("cannot open {path}: {e}"))?.len();
        Ok(Input::new(path, Arc::new(LocalFile(file, size))))
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, String> {
        local_glob(pattern)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// An in-memory file system with a `mem://` scheme that counts the reads it serves.
    #[derive(Default)]
    pub struct MemFs {
        pub files: Mutex<BTreeMap<String, Arc<Vec<u8>>>>,
        pub reads: Arc<Mutex<u64>>,
    }

    struct MemFile(Arc<Vec<u8>>, Arc<Mutex<u64>>);

    impl RandomAccess for MemFile {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), String> {
            *self.1.lock().unwrap() += 1;
            let start = offset as usize;
            let bytes = self.0.get(start..start + buf.len()).ok_or("read past the end")?;
            buf.copy_from_slice(bytes);
            Ok(())
        }
    }

    impl MemFs {
        pub fn put(&self, path: &str, bytes: Vec<u8>) {
            self.files.lock().unwrap().insert(path.to_string(), Arc::new(bytes));
        }
    }

    impl FileSystem for MemFs {
        fn open(&self, path: &str) -> Result<Input, String> {
            let bytes = self.files.lock().unwrap().get(path).cloned().ok_or(format!("no such file: {path}"))?;
            Ok(Input::new(path, Arc::new(MemFile(bytes, self.reads.clone()))))
        }
        fn glob(&self, pattern: &str) -> Result<Vec<String>, String> {
            let p = glob::Pattern::new(pattern).map_err(|e| e.to_string())?;
            Ok(self.files.lock().unwrap().keys().filter(|k| p.matches(k)).cloned().collect())
        }
    }

    #[test]
    fn remote_paths_have_a_scheme() {
        for remote in ["s3://b/k.parquet", "https://h/x", "memory://a", "gcs://b/*.parquet", "az+x.y-z://q"] {
            assert!(is_remote(remote), "{remote}");
        }
        for local in ["/tmp/a.parquet", "a.parquet", "~/x", "://x", "dir/x://y", "C:/x", ""] {
            assert!(!is_remote(local), "{local}");
        }
    }

    #[test]
    fn a_leading_tilde_is_the_home_directory() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(in_home("~/data/*.parquet", home.clone()), "/home/u/data/*.parquet");
        assert_eq!(in_home("~user/x", home.clone()), "~user/x");
        assert_eq!(in_home("a/~/x", home), "a/~/x");
        assert_eq!(in_home("~/x", None), "~/x");
    }

    #[test]
    fn inputs_read_ranges_and_refuse_reads_past_the_end() {
        let fs = MemFs::default();
        fs.put("mem://f", (0u8..100).collect());
        let input = fs.open("mem://f").unwrap();
        assert_eq!(input.len(), 100);
        assert_eq!(input.path(), "mem://f");
        assert_eq!(&input.get_bytes(10, 5).unwrap()[..], &[10, 11, 12, 13, 14]);
        assert_eq!(input.get_bytes(100, 0).unwrap().len(), 0);
        assert_eq!(input.get_bytes(95, 5).unwrap().len(), 5);
        let e = input.get_bytes(96, 5).unwrap_err().to_string();
        assert!(e.contains("mem://f: 5 bytes at 96 are past the end of the file (100 bytes)"), "{e}");
        assert!(input.get_bytes(u64::MAX, 2).is_err());
        let mut rest = vec![];
        input.get_read(97).unwrap().read_to_end(&mut rest).unwrap();
        assert_eq!(rest, vec![97, 98, 99]);
        let mut r = input.get_read(0).unwrap();
        let mut buf = [0u8; 7];
        assert_eq!(r.read(&mut buf).unwrap(), 7);
        assert_eq!(buf, [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(r.read(&mut buf).unwrap(), 7);
        assert_eq!(buf[0], 7);
        assert_eq!(input.get_read(100).unwrap().read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn read_errors_name_the_file() {
        struct Broken;
        impl RandomAccess for Broken {
            fn size(&self) -> u64 {
                10
            }
            fn read_at(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
                Err("connection reset".into())
            }
        }
        let input = Input::new("s3://b/k", Arc::new(Broken));
        let e = input.get_bytes(0, 4).unwrap_err().to_string();
        assert!(e.contains("s3://b/k: connection reset"), "{e}");
        let e = input.get_read(0).unwrap().read(&mut [0u8; 4]).unwrap_err().to_string();
        assert!(e.contains("connection reset"), "{e}");
        assert!(format!("{input:?}").contains("s3://b/k"));
    }

    #[test]
    fn the_local_file_system_opens_and_globs_local_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.parquet", "a.parquet", "c.txt"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        std::fs::create_dir(dir.path().join("d.parquet")).unwrap();
        let d = dir.path().display();
        assert_eq!(
            LocalFs.glob(&format!("{d}/*.parquet")).unwrap(),
            vec![format!("{d}/a.parquet"), format!("{d}/b.parquet")]
        );
        assert!(LocalFs.glob(&format!("{d}/*.csv")).unwrap().is_empty());
        assert!(LocalFs.glob(&format!("{d}/[")).unwrap_err().contains("invalid glob"));
        let f = LocalFs.open(&format!("{d}/c.txt")).unwrap();
        assert_eq!(f.len(), 5);
        assert_eq!(&f.get_bytes(1, 3).unwrap()[..], b".tx");
        assert!(f.get_bytes(3, 3).is_err());
        assert!(LocalFs.open(&format!("{d}/none")).unwrap_err().starts_with(&format!("cannot open {d}/none: ")));
        assert!(LocalFs.open("s3://b/k").unwrap_err().contains("not a local file"));
    }
}
