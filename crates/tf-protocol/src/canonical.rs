//! Canonical JSON and domain-separated SHA-256, profile v1. See schemas/canonical-v1.md.
use crate::validate_document;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    fmt,
    io::{self, Read},
};

/// Maximum serialized metadata size. File bytes are hashed separately in fixed chunks.
pub const MAX_CANONICAL_BYTES: usize = 16 * 1024 * 1024;
/// Largest interoperable bare integer. Larger values use lossless string carriers.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Recoverable canonicalization, contract or file-reading failure.
#[derive(Debug)]
pub enum CanonicalError {
    /// Unsupported number, document, or depth/byte limit.
    Invalid,
    /// Closed contract or fingerprint validation failed.
    Contract,
    /// Raw file hashing failed. A partial digest is never returned.
    Io(io::Error),
}
impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "Content cannot be encoded with canonical JSON v1",
            Self::Contract => "Content does not match its fingerprint contract",
            Self::Io(_) => "Could not read all content for its digest",
        })
    }
}
impl std::error::Error for CanonicalError {}

/// Hash purposes are retained alongside bytes to prevent accidental role equality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DigestKind {
    /// Canonical physical artifact manifest.
    Artifact,
    /// Captured source content descriptor (selection belongs to the capture service).
    Source,
    /// Fully selected computation semantics (selection belongs to the planner).
    Compute,
    /// Stable catalogue identity/path manifest.
    Catalog,
    /// Logical schema, including field order.
    Schema,
    /// Unprefixed SHA-256 of raw file bytes, for manifest file entries.
    File,
}
impl DigestKind {
    /// Exact ASCII domain/version prefix; raw file SHA-256 has no prefix.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Artifact => "transflow.artifact.v1",
            Self::Source => "transflow.source.v1",
            Self::Compute => "transflow.compute.v1",
            Self::Catalog => "transflow.catalog.v1",
            Self::Schema => "transflow.schema.v1",
            Self::File => "",
        }
    }
}
/// Content identity with an explicit role, never a publication/version UUID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentDigest {
    kind: DigestKind,
    bytes: [u8; 32],
}
impl ContentDigest {
    /// Parse a canonical lowercase SHA-256, with the role supplied by its enclosing contract.
    pub fn from_hex(kind: DigestKind, text: &str) -> Result<Self, CanonicalError> {
        if text.len() != 64
            || !text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(CanonicalError::Invalid);
        }
        let mut bytes = [0; 32];
        for (out, pair) in bytes.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
            let pair = std::str::from_utf8(pair).map_err(|_| CanonicalError::Invalid)?;
            *out = u8::from_str_radix(pair, 16).map_err(|_| CanonicalError::Invalid)?;
        }
        Ok(Self { kind, bytes })
    }

    /// Purpose used to compute this digest.
    pub fn kind(&self) -> DigestKind {
        self.kind
    }
    /// Lowercase hexadecimal wire representation. Role comes from the enclosing field.
    pub fn hex(&self) -> String {
        self.bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
struct Encoder(Vec<u8>);
impl Encoder {
    fn append(&mut self, bytes: &[u8]) -> Result<(), CanonicalError> {
        if bytes.len() > MAX_CANONICAL_BYTES.saturating_sub(self.0.len()) {
            return Err(CanonicalError::Invalid);
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn string(&mut self, s: &str) -> Result<(), CanonicalError> {
        self.append(b"\"")?;
        for ch in s.chars() {
            match ch {
                '"' => self.append(b"\\\"")?,
                '\\' => self.append(b"\\\\")?,
                '\u{8}' => self.append(b"\\b")?,
                '\t' => self.append(b"\\t")?,
                '\n' => self.append(b"\\n")?,
                '\u{c}' => self.append(b"\\f")?,
                '\r' => self.append(b"\\r")?,
                '\u{0}'..='\u{1f}' => self.append(format!("\\u{:04x}", ch as u32).as_bytes())?,
                _ => self.append(ch.encode_utf8(&mut [0; 4]).as_bytes())?,
            }
        }
        self.append(b"\"")
    }
    fn value(&mut self, value: &Value, depth: usize) -> Result<(), CanonicalError> {
        if depth > 64 {
            return Err(CanonicalError::Invalid);
        }
        match value {
            Value::Null => self.append(b"null"),
            Value::Bool(true) => self.append(b"true"),
            Value::Bool(false) => self.append(b"false"),
            Value::String(s) => self.string(s),
            Value::Number(n) => {
                let number = n.as_f64().ok_or(CanonicalError::Invalid)?;
                if !number.is_finite()
                    || number.fract() != 0.0
                    || number.abs() > MAX_SAFE_INTEGER as f64
                {
                    return Err(CanonicalError::Invalid);
                }
                self.append((number as i64).to_string().as_bytes())
            }
            Value::Array(values) => {
                self.append(b"[")?;
                for (index, item) in values.iter().enumerate() {
                    if index != 0 {
                        self.append(b",")?;
                    }
                    self.value(item, depth + 1)?;
                }
                self.append(b"]")
            }
            Value::Object(values) => {
                self.append(b"{")?;
                // Explicit sort remains correct if serde_json's preserve_order feature changes.
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        self.append(b",")?;
                    }
                    self.string(key)?;
                    self.append(b":")?;
                    self.value(values.get(key).ok_or(CanonicalError::Invalid)?, depth + 1)?;
                }
                self.append(b"}")
            }
        }
    }
}
/// Serialize validated decoded JSON. Reject duplicate keys at the raw decode boundary.
/// Strings are not normalized; wire numeric string spellings are preserved exactly.
pub fn canonical_json(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut encoder = Encoder(Vec::new());
    encoder.value(value, 0)?;
    Ok(encoder.0)
}
/// Hash `ASCII prefix || NUL || canonical JSON`. File digests require `file_digest`.
/// This low-level routine does not select semantic fields or validate a domain schema.
pub fn content_digest(kind: DigestKind, value: &Value) -> Result<ContentDigest, CanonicalError> {
    if kind == DigestKind::File {
        return Err(CanonicalError::Invalid);
    }
    let bytes = canonical_json(value)?;
    let mut hash = Sha256::new();
    hash.update(kind.prefix().as_bytes());
    hash.update([0]);
    hash.update(bytes);
    Ok(ContentDigest {
        kind,
        bytes: hash.finalize().into(),
    })
}
/// SHA-256 over every raw byte, in bounded chunks. Call outside async/write transactions.
pub fn file_digest<R: Read>(reader: &mut R) -> Result<ContentDigest, CanonicalError> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(size) => hash.update(buffer.get(..size).ok_or(CanonicalError::Invalid)?),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(CanonicalError::Io(error)),
        }
    }
    Ok(ContentDigest {
        kind: DigestKind::File,
        bytes: hash.finalize().into(),
    })
}
/// Validate and hash an ordered logical schema.
pub fn schema_digest(value: &Value) -> Result<ContentDigest, CanonicalError> {
    validate_document("LogicalSchemaV1", value).map_err(|_| CanonicalError::Contract)?;
    content_digest(DigestKind::Schema, value)
}
/// Validate a physical manifest and its schema fingerprint before hashing all fields.
/// File digests/lengths are asserted metadata here; storage verifies actual files later.
pub fn artifact_digest(value: &Value) -> Result<ContentDigest, CanonicalError> {
    validate_document("ArtifactManifestV1", value).map_err(|_| CanonicalError::Contract)?;
    if value["schema_fingerprint"].as_str()
        != Some(schema_digest(&value["logical_schema"])?.hex().as_str())
    {
        return Err(CanonicalError::Contract);
    }
    content_digest(DigestKind::Artifact, value)
}
/// Select catalogue content, excluding its self-fingerprint and independent source snapshot ID.
/// Entry array order is significant; producers must supply a deterministic order.
pub fn catalog_fingerprint(snapshot: &Value) -> Result<ContentDigest, CanonicalError> {
    validate_document("CatalogSnapshotV1", snapshot).map_err(|_| CanonicalError::Contract)?;
    content_digest(
        DigestKind::Catalog,
        &serde_json::json!({
            "format_version": snapshot["format_version"],
            "workspace_id": snapshot["workspace_id"],
            "entries": snapshot["entries"],
        }),
    )
}
/// Reject a catalogue snapshot whose recorded fingerprint does not describe its content.
pub fn verify_catalog_fingerprint(snapshot: &Value) -> Result<(), CanonicalError> {
    if snapshot["catalog_fingerprint"].as_str()
        != Some(catalog_fingerprint(snapshot)?.hex().as_str())
    {
        return Err(CanonicalError::Contract);
    }
    Ok(())
}
/// Hash an explicitly selected semantic document, never recursively stripping user keys.
/// The closed envelope has exactly `semantic` and `presentation`; presentation is excluded.
/// Only source/compute use this envelope. Complete source/compute field selection is later work.
pub fn semantic_fingerprint(
    kind: DigestKind,
    document: &Value,
) -> Result<ContentDigest, CanonicalError> {
    if !matches!(kind, DigestKind::Source | DigestKind::Compute) {
        return Err(CanonicalError::Contract);
    }
    let members = document.as_object().ok_or(CanonicalError::Contract)?;
    if members.len() != 2 || !members.contains_key("presentation") {
        return Err(CanonicalError::Contract);
    }
    content_digest(
        kind,
        members.get("semantic").ok_or(CanonicalError::Contract)?,
    )
}
