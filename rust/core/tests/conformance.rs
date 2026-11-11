//! The shared conformance vectors (`testdata/payloads`, see `testdata/README.md`): every valid
//! payload decodes to its `.out` bytes and is what the encoder writes for them today; every
//! invalid one fails with the message the manifest records.

use std::collections::BTreeMap;
use std::path::PathBuf;
use tparquet_core::{Mode, Options, PhysicalType, compress, decompress, decompress_into, mode_of};

/// The JSON subset the manifest uses: objects, arrays, strings without escapes beyond `\"` and
/// `\\`, integers, booleans and null.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    fn parse(s: &str) -> Json {
        let mut p = Parser { s: s.as_bytes(), i: 0 };
        let v = p.value();
        p.ws();
        assert_eq!(p.i, s.len(), "trailing data in the manifest");
        v
    }

    fn get(&self, k: &str) -> &Json {
        match self {
            Json::Obj(m) => m.get(k).unwrap_or_else(|| panic!("no key {k}")),
            _ => panic!("not an object"),
        }
    }

    fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            other => panic!("not a string: {other:?}"),
        }
    }

    fn num(&self) -> u64 {
        match self {
            Json::Num(n) => *n,
            other => panic!("not a number: {other:?}"),
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        let hit = self.s.get(self.i) == Some(&c);
        self.i += usize::from(hit);
        hit
    }

    fn lit(&mut self, word: &str) {
        assert_eq!(&self.s[self.i..self.i + word.len()], word.as_bytes());
        self.i += word.len();
    }

    fn string(&mut self) -> String {
        assert!(self.eat(b'"'));
        let mut out = Vec::new();
        loop {
            match self.s[self.i] {
                b'"' => break,
                b'\\' => {
                    self.i += 1;
                    out.push(self.s[self.i]);
                }
                c => out.push(c),
            }
            self.i += 1;
        }
        self.i += 1;
        String::from_utf8(out).unwrap()
    }

    fn value(&mut self) -> Json {
        self.ws();
        match self.s[self.i] {
            b'{' => {
                self.i += 1;
                let mut m = BTreeMap::new();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        let k = self.string();
                        assert!(self.eat(b':'));
                        m.insert(k, self.value());
                        if !self.eat(b',') {
                            assert!(self.eat(b'}'));
                            break;
                        }
                    }
                }
                Json::Obj(m)
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                if !self.eat(b']') {
                    loop {
                        v.push(self.value());
                        if !self.eat(b',') {
                            assert!(self.eat(b']'));
                            break;
                        }
                    }
                }
                Json::Arr(v)
            }
            b'"' => Json::Str(self.string()),
            b't' => {
                self.lit("true");
                Json::Bool(true)
            }
            b'f' => {
                self.lit("false");
                Json::Bool(false)
            }
            b'n' => {
                self.lit("null");
                Json::Null
            }
            _ => {
                let start = self.i;
                while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
                Json::Num(std::str::from_utf8(&self.s[start..self.i]).unwrap().parse().unwrap())
            }
        }
    }
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/payloads")
}

fn vectors() -> Vec<Json> {
    let m = Json::parse(&std::fs::read_to_string(dir().join("manifest.json")).unwrap());
    assert_eq!(m.get("format_version").num(), 1);
    match m.get("vectors") {
        Json::Arr(v) => v.clone(),
        _ => panic!("vectors is not an array"),
    }
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn mode_named(s: &str) -> Mode {
    match s {
        "RAW" => Mode::Raw,
        "PCO" => Mode::Pco,
        "DECIMAL" => Mode::Decimal,
        "ZSTD" => Mode::Zstd,
        other => panic!("unknown mode {other}"),
    }
}

fn hint_named(j: &Json) -> Option<PhysicalType> {
    match j {
        Json::Null => None,
        Json::Str(s) => Some(match s.as_str() {
            "INT32" => PhysicalType::Int32,
            "INT64" => PhysicalType::Int64,
            "FLOAT" => PhysicalType::Float,
            "DOUBLE" => PhysicalType::Double,
            other => panic!("unknown hint {other}"),
        }),
        other => panic!("bad hint {other:?}"),
    }
}

#[test]
fn json_reader_reads_what_the_manifest_uses() {
    let j = Json::parse(r#" { "a": [1, 22, {"b": null}], "c": "x\"y\\", "d": true, "e": false, "f": [], "g": {} } "#);
    assert_eq!(j.get("a"), &Json::Arr(vec![Json::Num(1), Json::Num(22), Json::Obj([("b".into(), Json::Null)].into())]));
    assert_eq!(j.get("c").str(), "x\"y\\");
    assert_eq!(j.get("d"), &Json::Bool(true));
    assert_eq!(j.get("e"), &Json::Bool(false));
    assert_eq!(j.get("f"), &Json::Arr(vec![]));
    assert_eq!(j.get("g"), &Json::Obj(BTreeMap::new()));
}

#[test]
fn every_valid_vector_decodes_to_its_bytes() {
    let mut seen = 0;
    for v in vectors().iter().filter(|v| v.get("valid") == &Json::Bool(true)) {
        let name = v.get("name").str();
        let payload = read(v.get("payload").str());
        let want = read(v.get("decoded").str());
        let n = v.get("expected_len").num() as usize;
        assert_eq!(want.len(), n, "{name}");
        assert_eq!(decompress(&payload, n).as_deref(), Ok(&want[..]), "{name}");
        let mut dst = vec![0x5A; n];
        assert_eq!(decompress_into(&payload, &mut dst), Ok(()), "{name}");
        assert_eq!(dst, want, "{name}");
        assert_eq!(mode_of(&payload), Ok(mode_named(v.get("mode").str())), "{name}");
        seen += 1;
    }
    assert!(seen >= 18, "only {seen} valid vectors");
}

/// Whether the reference encoder writes this valid vector (`reference_encoding`), or it was
/// made by hand (a non-minimal varint, a PCO payload without numbers...).
fn reference_encoding(v: &Json) -> bool {
    match v.get("reference_encoding") {
        Json::Bool(b) => *b,
        other => panic!("{}: reference_encoding is not a boolean: {other:?}", v.get("name").str()),
    }
}

#[test]
fn hand_made_valid_vectors_are_listed_and_marked() {
    // decoders accept these although no encoder writes them (spec section 4)
    let valid: Vec<Json> = vectors().into_iter().filter(|v| v.get("valid") == &Json::Bool(true)).collect();
    let made: Vec<&str> = valid.iter().filter(|v| !reference_encoding(v)).map(|v| v.get("name").str()).collect();
    assert_eq!(made, ["pco_no_values", "decimal_no_values", "zstd_empty_frame", "varint_overlong"]);
    for name in made {
        let v = valid.iter().find(|v| v.get("name").str() == name).unwrap();
        let input = read(v.get("decoded").str());
        let opts = Options { level: v.get("level").num() as u8, hint: hint_named(v.get("hint")), decimal: true };
        assert_ne!(compress(&input, &opts).unwrap(), read(v.get("payload").str()), "{name}");
    }
}

#[test]
fn the_encoder_still_writes_every_valid_vector() {
    // the vectors were written by this encoder (gen-testdata uses compress with decimal on); a
    // change here is an encoder change, which must be deliberate and regenerate the vectors
    for v in vectors().iter().filter(|v| v.get("valid") == &Json::Bool(true) && reference_encoding(v)) {
        let name = v.get("name").str();
        let opts = Options { level: v.get("level").num() as u8, hint: hint_named(v.get("hint")), decimal: true };
        let input = read(v.get("decoded").str());
        assert_eq!(compress(&input, &opts).unwrap(), read(v.get("payload").str()), "{name}");
    }
}

#[test]
fn every_invalid_vector_fails_with_the_recorded_message() {
    let mut seen = 0;
    for v in vectors().iter().filter(|v| v.get("valid") == &Json::Bool(false)) {
        let name = v.get("name").str();
        let payload = read(v.get("payload").str());
        let n = v.get("expected_len").num() as usize;
        let err = decompress(&payload, n).expect_err(name);
        assert_eq!(err.to_string(), v.get("error").str(), "{name}");
        let mut dst = vec![0; n];
        assert_eq!(decompress_into(&payload, &mut dst), Err(err), "{name}");
        seen += 1;
    }
    assert!(seen >= 26, "only {seen} invalid vectors");
}

#[test]
fn the_manifest_lists_every_file() {
    let mut listed: Vec<String> = vec!["manifest.json".into()];
    for v in vectors() {
        listed.push(v.get("payload").str().into());
        if v.get("valid") == &Json::Bool(true) {
            listed.push(v.get("decoded").str().into());
        }
    }
    listed.sort();
    let mut on_disk: Vec<String> =
        std::fs::read_dir(dir()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    on_disk.sort();
    assert_eq!(listed, on_disk);
}
