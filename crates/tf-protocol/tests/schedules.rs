//! Schedule semantic normalization and source independence.
#![allow(clippy::unwrap_used, reason = "Synthetic schedule fixtures")]
use serde_json::{Value, json};
use tf_protocol::schedule::Definition;
fn definition() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "schedule-definition-fixed")
        .unwrap()["value"]
        .clone()
}
#[test]
fn explicit_sources_ordered_fallbacks_and_identity_validation() {
    let mut value = definition();
    assert!(Definition::decode(&value).is_ok());
    for reference in ["refs/heads/master", "refs/tags/stable", &"a".repeat(40)] {
        value["build"]["source"] = json!({"kind":"git_ref","ref":reference});
        assert!(Definition::decode(&value).is_ok());
    }
    for reference in [
        "HEAD",
        "master",
        "refs/heads/../master",
        "refs/heads/main^",
        "refs/heads/.hidden",
        "refs/heads/master.lock",
        "refs/heads/x//y",
    ] {
        value["build"]["source"] = json!({"kind":"git_ref","ref":reference});
        assert!(Definition::decode(&value).is_err(), "{reference}");
    }
    value = definition();
    for tail in [json!(["master"]), json!(["main", "main"])] {
        value["build"]["fallback_branches"] = tail;
        assert!(Definition::decode(&value).is_err());
    }
    value = definition();
    value["build"]["boundaries"] = value["build"]["targets"].clone();
    assert!(Definition::decode(&value).is_err());
    value = definition();
    value["policies"]["token_window_seconds"] = Value::Null;
    assert!(Definition::decode(&value).is_err());
    value["policies"]["acknowledge_no_expiry"] = json!(true);
    assert!(Definition::decode(&value).is_ok());
}
#[test]
fn trigger_bounds_leaf_identity_and_conflicting_payload_modes() {
    let mut v = definition();
    let leaf = json!({"kind":"dataset_head_changed","id":"a","dataset":v["build"]["targets"][0],"branch":"master","payload_mode":"pin","include_resets":false});
    let mut other = leaf.clone();
    other["id"] = json!("b");
    v["trigger"] = json!({"kind":"and","children":[leaf,other]});
    assert!(Definition::decode(&v).is_ok());
    v["trigger"]["children"][1]["payload_mode"] = json!("signal_only");
    assert!(Definition::decode(&v).is_err());
    v["trigger"] = json!({"kind":"or","children":[{"kind":"manual"},leaf]});
    assert!(Definition::decode(&v).is_err());
    let leaves: Vec<Value> = (0..64)
        .map(|n| {
            let mut l = leaf.clone();
            l["id"] = json!(format!("leaf_{n}"));
            l
        })
        .collect();
    v["trigger"] = json!({"kind":"and","children":leaves});
    assert!(Definition::decode(&v).is_ok());
    v["trigger"]["children"]
        .as_array_mut()
        .unwrap()
        .push(leaf.clone());
    assert!(Definition::decode(&v).is_err());
    let mut tree = leaf.clone();
    for n in 0..8 {
        let mut l = leaf.clone();
        l["id"] = json!(format!("depth_{n}"));
        tree = json!({"kind":"and","children":[l,tree]});
    }
    v["trigger"] = tree;
    assert!(Definition::decode(&v).is_err());
}
