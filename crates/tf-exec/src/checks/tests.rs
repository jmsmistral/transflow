#![allow(clippy::unwrap_used, reason = "Synthetic compiler/result assertions")]
use super::{compiler, results};
use serde_json::{Value, json};
fn schema() -> Value {
    json!({"format_version":1,"fields":[{"name":"x","logical_type":{"type":"i64"},"nullable":true}]})
}
fn check(ast: Value) -> Value {
    let mut ast = ast;
    ast["ast_version"] = json!(1);
    json!({"expectation":ast})
}
#[test]
fn typed_compiler_batches_rows_and_rolls_back_rejected_expressions() {
    let row = check(
        json!({"kind":"row_compare","op":"gte","left":{"kind":"column","name":"x"},"right":{"kind":"literal","value":{"type":"i64","value":"0"}}}),
    );
    let invalid = check(
        json!({"kind":"dataset_all","children":[{"kind":"every","child":{"kind":"non_null","columns":["x"]}},{"kind":"metric_compare","op":"equals","left":{"kind":"row_count","input":null},"right":{"kind":"row_count","input":{"kind":"input","alias":"hidden"}}}]}),
    );
    let mut checks = vec![invalid];
    checks.extend(vec![row; 33]);
    let p = compiler::compile(&checks, &schema());
    assert!(p.nodes[0].is_err());
    assert_eq!(p.queries.len(), 2);
    assert_eq!(p.queries[0]["width"], 65);
    assert_eq!(p.queries[0]["parameters"].as_array().unwrap().len(), 32);
    assert_eq!(p.queries[1]["width"], 3);
}
#[test]
fn parent_outcome_does_not_hide_child_query_errors_or_invent_row_attribution() {
    let n = compiler::Node::Boolean(
        tf_domain::expectation::BooleanOp::Any,
        vec![
            compiler::Node::Fixed(true),
            compiler::Node::Row(0, 0, vec![]),
        ],
    );
    assert!(
        results::outcome(
            &n,
            &[json!({"counts":[],"error":"query_error"})],
            false,
            "$",
            &mut vec![]
        )
        .is_err()
    );
    assert_eq!(
        results::outcome(&compiler::Node::Fixed(false), &[], false, "$", &mut vec![]).unwrap(),
        (false, None)
    );
    assert!(
        results::outcome(
            &compiler::Node::Row(0, 0, vec![]),
            &[json!({"counts":["1","1","1"],"error":null})],
            false,
            "$",
            &mut vec![]
        )
        .is_err()
    );
}
#[test]
fn exactness_preflight_rejects_lossy_schemas_without_reading_rows() {
    let mut s = schema();
    s["fields"][0]["logical_type"] = json!({"type":"timestamp","unit":"ns","timezone":"UTC"});
    assert!(compiler::schema_supported(&s).is_err());
    s["fields"][0]["logical_type"] = json!({"type":"timestamp","unit":"ns","timezone":null});
    assert!(compiler::schema_supported(&s).is_ok());
    s["fields"][0]["logical_type"] = json!({"type":"decimal","precision":40,"scale":2});
    assert!(compiler::schema_supported(&s).is_err());
}
