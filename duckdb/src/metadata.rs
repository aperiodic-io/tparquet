//! The metadata footer DuckDB reads from the end of a `.duckdb_extension` file.
//!
//! DuckDB (`ExtensionHelper::ParseExtensionMetaData`, `src/main/extension/extension_load.cpp`)
//! reads the last 512 bytes of the file: eight 32-byte fields, then a 256-byte signature. The
//! fields are stored last-first: the field nearest the signature is the magic value `"4"`, then
//! come the platform, the DuckDB (C API) version, the extension version and the ABI type; the
//! other three are unused. Each field is ASCII, zero padded. In front of them DuckDB's own
//! tooling (`extension-ci-tools/scripts/append_extension_metadata.py`) writes the header of a
//! WebAssembly custom section named `duckdb_signature`, so that Wasm builds stay valid; we write
//! the same bytes.
//!
//! An unsigned extension carries 256 zero bytes as its signature; DuckDB then loads it only with
//! `allow_unsigned_extensions`.

/// Width of one metadata field.
pub const FIELD_LEN: usize = 32;
/// Width of the signature.
pub const SIGNATURE_LEN: usize = 256;
/// The bytes DuckDB reads: eight fields and the signature.
pub const FOOTER_LEN: usize = 8 * FIELD_LEN + SIGNATURE_LEN;
/// The magic value of the first field.
pub const MAGIC: &str = "4";

/// Header of the Wasm custom section that holds the footer: section id 0, the section length
/// (531 as LEB128), the name length (16), the name, and the payload length (512 as LEB128).
pub const WASM_SECTION_HEADER: &[u8] = b"\x00\x93\x04\x10duckdb_signature\x80\x04";

/// The ABI of an extension built on DuckDB's C extension API v1 with a stable API version.
pub const ABI_C_STRUCT: &str = "C_STRUCT";
/// The ABI of an extension built on DuckDB's unstable C API (v2 in DuckDB 2.0): it loads only
/// in the DuckDB version named in the footer.
pub const ABI_C_STRUCT_UNSTABLE: &str = "C_STRUCT_UNSTABLE";

/// The C API a build of the extension is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// C API v1.5.6, stable: any DuckDB that serves it.
    V1,
    /// C API v2, unstable in DuckDB 2.0.0: only DuckDB 2.0.0.
    V2,
}

/// What the footer says about the extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// DuckDB platform, e.g. `linux_amd64` (`PRAGMA platform`).
    pub platform: String,
    /// For the `C_STRUCT` ABI: the lowest C API version the extension needs, e.g. `v1.5.6`.
    pub duckdb_version: String,
    /// The extension's own version, e.g. `v0.1.0`.
    pub extension_version: String,
    /// `C_STRUCT`, `C_STRUCT_UNSTABLE` or `CPP`.
    pub abi_type: String,
}

fn field(value: &str, what: &str) -> Result<[u8; FIELD_LEN], String> {
    if !value.is_ascii() || value.len() > FIELD_LEN || value.contains('\0') {
        return Err(format!("{what} {value:?} must be at most {FIELD_LEN} ASCII bytes without NUL"));
    }
    let mut out = [0u8; FIELD_LEN];
    out[..value.len()].copy_from_slice(value.as_bytes());
    Ok(out)
}

impl Metadata {
    /// The bytes to append to the shared library: the Wasm section header, the eight fields
    /// (last first) and an empty signature.
    pub fn footer(&self) -> Result<Vec<u8>, String> {
        let fields = [
            field("", "field 8")?,
            field("", "field 7")?,
            field("", "field 6")?,
            field(&self.abi_type, "ABI type")?,
            field(&self.extension_version, "extension version")?,
            field(&self.duckdb_version, "DuckDB version")?,
            field(&self.platform, "platform")?,
            field(MAGIC, "magic")?,
        ];
        let mut out = Vec::with_capacity(WASM_SECTION_HEADER.len() + FOOTER_LEN);
        out.extend_from_slice(WASM_SECTION_HEADER);
        for f in fields {
            out.extend_from_slice(&f);
        }
        out.resize(out.len() + SIGNATURE_LEN, 0);
        Ok(out)
    }

    /// Reads the metadata back from the end of a file, as DuckDB does; `None` if the magic value
    /// is missing.
    pub fn parse(file: &[u8]) -> Option<Self> {
        let start = file.len().checked_sub(FOOTER_LEN)?;
        let fields: Vec<String> = file[start..start + 8 * FIELD_LEN]
            .chunks(FIELD_LEN)
            .rev()
            .map(|f| String::from_utf8_lossy(f).trim_end_matches('\0').to_string())
            .collect();
        (fields[0] == MAGIC).then(|| Self {
            platform: fields[1].clone(),
            duckdb_version: fields[2].clone(),
            extension_version: fields[3].clone(),
            abi_type: fields[4].clone(),
        })
    }
}

impl Metadata {
    /// The metadata of the C API v1 build of the extension, for `platform`.
    pub fn for_platform(platform: &str) -> Self {
        Self::for_api(platform, Api::V1)
    }

    /// The metadata of the build of the extension for `api`, for `platform`.
    pub fn for_api(platform: &str, api: Api) -> Self {
        let (duckdb_version, abi_type) = match api {
            Api::V1 => (crate::C_API_VERSION, ABI_C_STRUCT),
            Api::V2 => (crate::v2::DUCKDB_VERSION, ABI_C_STRUCT_UNSTABLE),
        };
        Self {
            platform: platform.to_string(),
            duckdb_version: duckdb_version.to_string(),
            extension_version: format!("v{}", env!("CARGO_PKG_VERSION")),
            abi_type: abi_type.to_string(),
        }
    }
}

/// The `.duckdb_extension` file: the shared library with the footer appended.
pub fn package(library: &[u8], metadata: &Metadata) -> Result<Vec<u8>, String> {
    if Metadata::parse(library).is_some() {
        return Err("the library already ends with extension metadata".into());
    }
    let mut out = library.to_vec();
    out.extend(metadata.footer()?);
    Ok(out)
}

/// The DuckDB platform name of the target this crate is compiled for.
pub fn current_platform() -> Option<&'static str> {
    platform_of(std::env::consts::OS, std::env::consts::ARCH)
}

/// DuckDB's name of an OS and architecture (as `std::env::consts` spells them).
pub fn platform_of(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("linux", "x86_64") => "linux_amd64",
        ("linux", "aarch64") => "linux_arm64",
        ("macos", "x86_64") => "osx_amd64",
        ("macos", "aarch64") => "osx_arm64",
        ("windows", "x86_64") => "windows_amd64",
        ("windows", "aarch64") => "windows_arm64",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Metadata {
        Metadata {
            platform: "linux_amd64".into(),
            duckdb_version: "v1.5.6".into(),
            extension_version: "v0.1.0".into(),
            abi_type: ABI_C_STRUCT.into(),
        }
    }

    #[test]
    fn footer_layout_matches_duckdb() {
        let footer = meta().footer().unwrap();
        assert_eq!(footer.len(), 22 + 512);
        assert_eq!(&footer[..22], WASM_SECTION_HEADER);
        let fields = &footer[22..22 + 256];
        let at = |i: usize| &fields[i * 32..(i + 1) * 32];
        // fields 8, 7, 6 are empty
        for i in 0..3 {
            assert_eq!(at(i), &[0u8; 32]);
        }
        let padded = |s: &str| {
            let mut v = s.as_bytes().to_vec();
            v.resize(32, 0);
            v
        };
        assert_eq!(at(3), padded("C_STRUCT"));
        assert_eq!(at(4), padded("v0.1.0"));
        assert_eq!(at(5), padded("v1.5.6"));
        assert_eq!(at(6), padded("linux_amd64"));
        assert_eq!(at(7), padded("4"));
        assert_eq!(&footer[22 + 256..], &[0u8; 256]);
    }

    #[test]
    fn wasm_header_lengths_are_consistent() {
        // section length 0x93 0x04 = 19 + 4 * 128 = 531 = 1 (name length) + 16 + 2 + 512
        assert_eq!(WASM_SECTION_HEADER[1] as usize - 128 + 128 * WASM_SECTION_HEADER[2] as usize, 531);
        assert_eq!(WASM_SECTION_HEADER[3], 16);
        assert_eq!(&WASM_SECTION_HEADER[4..20], b"duckdb_signature");
        assert_eq!(WASM_SECTION_HEADER[20] as usize - 128 + 128 * WASM_SECTION_HEADER[21] as usize, FOOTER_LEN);
        assert_eq!(531, 1 + 16 + 2 + FOOTER_LEN);
    }

    #[test]
    fn parse_round_trips_after_a_library() {
        let mut file = vec![0x7f, b'E', b'L', b'F', 1, 2, 3];
        file.extend(meta().footer().unwrap());
        assert_eq!(Metadata::parse(&file), Some(meta()));
        assert_eq!(Metadata::parse(&file[..file.len() - 1]), None);
        assert_eq!(Metadata::parse(b"short"), None);
        let mut exact = meta().footer().unwrap()[22..].to_vec();
        assert_eq!(Metadata::parse(&exact), Some(meta()));
        exact[255] = b'5';
        assert_eq!(Metadata::parse(&exact), None);
    }

    #[test]
    fn rejects_fields_that_do_not_fit() {
        let mut m = meta();
        m.platform = "x".repeat(32);
        assert!(m.footer().is_ok());
        m.platform = "x".repeat(33);
        assert_eq!(
            m.footer().unwrap_err(),
            format!("platform {:?} must be at most 32 ASCII bytes without NUL", "x".repeat(33))
        );
        m.platform = "linüx".into();
        assert!(m.footer().is_err());
        m.platform = "a\0b".into();
        assert!(m.footer().is_err());
    }

    #[test]
    fn platforms() {
        assert_eq!(platform_of("linux", "x86_64"), Some("linux_amd64"));
        assert_eq!(platform_of("linux", "aarch64"), Some("linux_arm64"));
        assert_eq!(platform_of("macos", "x86_64"), Some("osx_amd64"));
        assert_eq!(platform_of("macos", "aarch64"), Some("osx_arm64"));
        assert_eq!(platform_of("windows", "x86_64"), Some("windows_amd64"));
        assert_eq!(platform_of("windows", "aarch64"), Some("windows_arm64"));
        assert_eq!(platform_of("freebsd", "x86_64"), None);
        assert_eq!(current_platform(), platform_of(std::env::consts::OS, std::env::consts::ARCH));
    }

    #[test]
    fn packages_a_library_once() {
        let m = Metadata::for_platform("linux_arm64");
        assert_eq!(m.platform, "linux_arm64");
        assert_eq!(m.duckdb_version, "v1.5.6");
        assert_eq!(m.extension_version, format!("v{}", env!("CARGO_PKG_VERSION")));
        assert_eq!(m.abi_type, "C_STRUCT");
        let lib = b"\x7fELF library bytes".to_vec();
        let file = package(&lib, &m).unwrap();
        assert_eq!(&file[..lib.len()], &lib[..]);
        assert_eq!(file.len(), lib.len() + 22 + 512);
        assert_eq!(Metadata::parse(&file), Some(m.clone()));
        assert_eq!(package(&file, &m).unwrap_err(), "the library already ends with extension metadata");
        let mut bad = m;
        bad.platform = "x".repeat(40);
        assert!(package(&lib, &bad).is_err());
    }
}
