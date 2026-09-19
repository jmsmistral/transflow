//! Fixed cross-language vectors and semantic boundary regressions.
#![allow(clippy::unwrap_used, reason = "Test fixture failures must fail loudly")]
use serde_json::{Value, json};
use std::io::{self, Cursor, Read};
use tf_domain::VersionId;
use tf_protocol::canonical::*;

fn fixtures() -> Value {
    serde_json::from_str(include_str!("../../../schemas/fixtures/canonical-v1.json")).unwrap()
}
const KINDS: [(&str, DigestKind); 5] = [
    ("artifact", DigestKind::Artifact),
    ("source", DigestKind::Source),
    ("compute", DigestKind::Compute),
    ("catalog", DigestKind::Catalog),
    ("schema", DigestKind::Schema),
];
#[test]
fn every_fixed_encoding_and_digest_matches() {
    let f = fixtures();
    let cases = f["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 21);
    for case in cases {
        if case["name"] == "lossless-values" {
            for value in case["value"].as_array().unwrap() {
                tf_protocol::validate_document("WireValue", value).unwrap();
            }
        }
        assert_eq!(
            canonical_json(&case["value"]).unwrap(),
            case["canonical"].as_str().unwrap().as_bytes(),
            "{}",
            case["name"]
        );
        for (name, kind) in KINDS {
            let digest = content_digest(kind, &case["value"]).unwrap();
            assert_eq!(digest.kind(), kind);
            assert_eq!(
                digest.hex(),
                case["sha256"][name].as_str().unwrap(),
                "{} {name}",
                case["name"]
            );
        }
    }
    for case in f["invalid"].as_array().unwrap() {
        assert!(canonical_json(&case["value"]).is_err(), "{}", case["name"]);
    }
}
#[test]
fn limits_and_streaming_raw_sha256() {
    let mut nested = Value::Null;
    for _ in 0..64 {
        nested = json!([nested]);
    }
    assert!(canonical_json(&nested).is_ok());
    assert!(canonical_json(&json!([nested])).is_err());
    let exact = Value::String("a".repeat(MAX_CANONICAL_BYTES - 2));
    assert_eq!(canonical_json(&exact).unwrap().len(), MAX_CANONICAL_BYTES);
    assert!(canonical_json(&Value::String("é".repeat(MAX_CANONICAL_BYTES / 2))).is_err());
    for case in fixtures()["files"].as_array().unwrap() {
        let digest = file_digest(&mut Cursor::new(case["text"].as_str().unwrap())).unwrap();
        assert_eq!(digest.kind(), DigestKind::File);
        assert_eq!(digest.hex(), case["sha256"].as_str().unwrap());
    }
    assert_eq!(
        file_digest(&mut Cursor::new(vec![b'a'; 1_000_000]))
            .unwrap()
            .hex(),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("injected read failure"))
        }
    }
    assert!(matches!(
        file_digest(&mut Broken),
        Err(CanonicalError::Io(_))
    ));
    assert!(content_digest(DigestKind::File, &Value::Null).is_err());
}
#[test]
fn projections_preserve_semantics_and_exclude_only_explicit_presentation() {
    let f = fixtures();
    for (section, kind) in [
        ("semantic", DigestKind::Compute),
        ("source", DigestKind::Source),
    ] {
        let mut doc = f[section]["document"].clone();
        let original = semantic_fingerprint(kind, &doc).unwrap();
        assert_eq!(original.hex(), f[section]["sha256"].as_str().unwrap());
        doc["presentation"] = json!({"description":"updated label", "position":[0,0]});
        assert_eq!(original, semantic_fingerprint(kind, &doc).unwrap());
        doc["semantic"]["description"] = json!("semantic user data");
        assert_ne!(original, semantic_fingerprint(kind, &doc).unwrap());
        doc["unclassified"] = json!(true);
        assert!(semantic_fingerprint(kind, &doc).is_err());
    }
    let original = f["semantic"]["document"].clone();
    for field in ["version_id", "alias", "artifact_digest"] {
        let mut changed = original.clone();
        changed["semantic"]["inputs"][0][field] = json!("changed");
        assert_ne!(
            semantic_fingerprint(DigestKind::Compute, &original).unwrap(),
            semantic_fingerprint(DigestKind::Compute, &changed).unwrap()
        );
    }
    let mut changed = original.clone();
    changed["semantic"]["checks"][0]["severity"] = json!("WARN");
    assert_ne!(
        semantic_fingerprint(DigestKind::Compute, &original).unwrap(),
        semantic_fingerprint(DigestKind::Compute, &changed).unwrap()
    );
    assert!(semantic_fingerprint(DigestKind::Artifact, &original).is_err());
}
#[test]
fn manifest_integrity_and_publication_identity_are_separate() {
    let f = fixtures();
    let manifest = f["artifacts"]["manifest"].clone();
    let digest = artifact_digest(&manifest).unwrap();
    assert_eq!(digest.hex(), f["artifacts"]["sha256"].as_str().unwrap());
    let mut reordered = manifest.clone();
    reordered["files"].as_array_mut().unwrap().reverse();
    assert_ne!(digest, artifact_digest(&reordered).unwrap());
    let mut changed = manifest.clone();
    changed["schema_fingerprint"] = json!("0".repeat(64));
    assert!(artifact_digest(&changed).is_err());
    changed = manifest.clone();
    changed["description"] = json!("not a physical manifest field");
    assert!(artifact_digest(&changed).is_err());
    let first: VersionId = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let second: VersionId = "00000000-0000-4000-8000-000000000002".parse().unwrap();
    let publications = [
        (first, digest),
        (second, artifact_digest(&manifest).unwrap()),
    ];
    assert_ne!(publications[0].0, publications[1].0);
    assert_eq!(publications[0].1, publications[1].1);
    let mut catalog = f["catalog"]["snapshot"].clone();
    verify_catalog_fingerprint(&catalog).unwrap();
    let fingerprint = catalog_fingerprint(&catalog).unwrap();
    catalog["source_snapshot_id"] = json!("00000000-0000-4000-8000-000000000002");
    assert_eq!(fingerprint, catalog_fingerprint(&catalog).unwrap());
    catalog["entries"][0]["path"] = json!("raw/renamed");
    assert_ne!(fingerprint, catalog_fingerprint(&catalog).unwrap());
    assert!(verify_catalog_fingerprint(&catalog).is_err());
}

#[test]
fn retained_digest_parsing_is_canonical_and_keeps_its_role() {
    let digest = file_digest(&mut &b"registry bytes"[..]).unwrap();
    assert_eq!(
        ContentDigest::from_hex(DigestKind::File, &digest.hex()).unwrap(),
        digest
    );
    assert_ne!(
        ContentDigest::from_hex(DigestKind::Source, &digest.hex()).unwrap(),
        digest
    );
    for invalid in [
        digest.hex().to_uppercase(),
        "a".repeat(63),
        "0".repeat(65),
        "g".repeat(64),
        "é".repeat(32),
    ] {
        assert!(ContentDigest::from_hex(DigestKind::File, &invalid).is_err());
    }
}
