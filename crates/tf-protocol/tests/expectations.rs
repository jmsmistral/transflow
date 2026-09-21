//! Shared AST, canonicalization and deferred-schema regression fixtures.
#![allow(clippy::unwrap_used, reason = "Synthetic conformance assertions")]
use serde_json::{Value, json};
use tf_protocol::{
    canonical::{DigestKind, canonical_json, content_digest},
    expectation::{decode, encode, validate_schema},
};

#[test]
fn canonical_ast_and_effective_policy_vectors_match_python() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../schemas/fixtures/expectations-v1.json"
    ))
    .unwrap();
    for case in cases {
        let aliases = case["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let parsed = decode(&case["ast"], &aliases);
        assert_eq!(
            parsed.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {parsed:?}",
            case["name"]
        );
        if let Ok(ast) = parsed {
            let encoded = encode(&ast, &aliases).unwrap();
            assert_eq!(encoded, case["ast"]);
            assert_eq!(
                canonical_json(&encoded).unwrap(),
                case["canonical"].as_str().unwrap().as_bytes()
            );
            let effective = json!({"expectation":encoded,"null_policy":"fail","sample_rows":"0","on_error":"FAIL"});
            assert_eq!(
                content_digest(DigestKind::Compute, &effective)
                    .unwrap()
                    .hex(),
                case["effective_compute_digest"]
            );
        }
    }
}
#[test]
fn known_schema_rejects_types_and_missing_columns_unknown_schema_defers() {
    let schema = json!({"format_version":1,"fields":[{"name":"amount","logical_type":{"type":"string"},"nullable":true}]});
    let ast = decode(&json!({"ast_version":1,"kind":"row_compare","op":"gte","left":{"kind":"column","name":"amount"},"right":{"kind":"literal","value":{"type":"i64","value":"0"}}}),&Default::default()).unwrap();
    assert!(validate_schema(&ast, &Value::Null).is_ok());
    assert!(
        validate_schema(&ast, &schema)
            .unwrap_err()
            .0
            .contains("incompatible")
    );
    let ast = decode(
        &json!({"ast_version":1,"kind":"non_null","columns":["missing"]}),
        &Default::default(),
    )
    .unwrap();
    assert!(
        validate_schema(&ast, &schema)
            .unwrap_err()
            .0
            .contains("absent")
    );
    let ast = decode(
        &json!({"ast_version":1,"kind":"primary_key","columns":["amount"]}),
        &Default::default(),
    )
    .unwrap();
    assert!(validate_schema(&ast, &schema).is_ok());
    let mut float_schema = schema;
    float_schema["fields"][0]["logical_type"]["type"] = "f64".into();
    assert!(
        validate_schema(&ast, &float_schema)
            .unwrap_err()
            .0
            .contains("unsupported")
    );
}
#[test]
fn ast_size_and_depth_are_bounded() {
    let leaf = json!({"kind":"non_null","columns":["id"]});
    let mut nested = leaf.clone();
    for _ in 0..40 {
        nested = json!({"kind":"row_not","child":nested});
    }
    nested["ast_version"] = 1.into();
    assert!(decode(&nested, &Default::default()).is_err());
    let broad = json!({"ast_version":1,"kind":"row_all","children":vec![leaf;1000]});
    assert!(decode(&broad, &Default::default()).is_err());
}
