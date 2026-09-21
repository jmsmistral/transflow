//! Actual command stream separation and shared human/JSON diagnostic projections.
#![allow(clippy::unwrap_used, reason = "Test fixture assertions")]
use serde_json::Value;
use std::{ffi::OsString, process::Command};
use tf_domain::diagnostic::*;
use tf_protocol::diagnostic::CliEnvelope;
fn cycle() -> Diagnostic {
    let r = Redactor::new(vec!["fixture-private".into()]).unwrap();
    let cause = Diagnostic::new(
        DiagnosticCode::OperationFailed,
        r.text("Input declaration could not be resolved").unwrap(),
        r.text("fixture-private\x1b[31m\u{202e}").unwrap(),
        r.text("Correct the input reference.").unwrap(),
    );
    Diagnostic::new(
        DiagnosticCode::GraphCycle,
        r.text("Workspace validation failed").unwrap(),
        r.text("A dependency cycle was found in the transform graph.")
            .unwrap(),
        r.text("Remove one dependency. No transform functions were run.")
            .unwrap(),
    )
    .with_sources(vec![
        SourceRange::new(r.text("src/orders.py").unwrap(), (18, 1), (18, 24)).unwrap(),
    ])
    .unwrap()
    .with_affected(vec![
        r.text("curated/orders -> analytics/totals -> curated/orders")
            .unwrap(),
    ])
    .unwrap()
    .with_causes(vec![cause])
    .unwrap()
}
#[test]
fn cycle_opens_with_human_reason_and_retains_structured_details() {
    let d = cycle();
    let r = Redactor::default();
    let ctx = RequestContext {
        workspace: Some(r.text("/synthetic/workspace").unwrap()),
        source: Some(r.text("working-tree").unwrap()),
        request_id: Some(tf_domain::RequestId::from_bytes([1; 16])),
    };
    let human = transflow::render_diagnostic(&d, &ctx, false, false);
    assert!(human.starts_with("Workspace validation failed\n\nA dependency cycle"));
    assert!(!human.contains("TF_GRAPH_CYCLE"));
    assert!(human.contains("src/orders.py:18:1"));
    assert!(!human.contains("fixture-private"));
    assert!(!human.contains('\x1b'));
    assert!(transflow::render_diagnostic(&d, &ctx, true, false).contains("TF_GRAPH_CYCLE"));
    for status in [
        ExitStatus::Failure,
        ExitStatus::Usage,
        ExitStatus::Interrupted,
    ] {
        let envelope = CliEnvelope::failure(
            &r.text("test-version").unwrap(),
            status,
            &ctx,
            std::slice::from_ref(&d),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(envelope.as_bytes()).unwrap();
        tf_protocol::validate_document("CliEnvelopeV1", &value).unwrap();
        assert_eq!(value["diagnostics"][0]["code"], "TF_GRAPH_CYCLE");
        assert_eq!(value["exit_status"], status.code());
        assert_eq!(value["context"]["source"], "working-tree");
        assert!(
            value["diagnostics"][0]["causes"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("REDACTED")
        );
    }
    assert!(
        CliEnvelope::failure(&r.text("version").unwrap(), ExitStatus::Success, &ctx, &[d]).is_err()
    );
}
#[test]
fn executable_json_is_one_envelope_for_help_version_and_usage() {
    for args in [
        vec!["--json"],
        vec!["--json", "--help"],
        vec!["--version", "--json"],
        vec!["--color", "always", "--json", "--unknown"],
        vec!["build", "--json"],
        vec!["trust", "--json"],
        vec![
            "--json",
            "--workspace",
            "/synthetic/consumer",
            "external",
            "add",
            "--workspace",
            "/synthetic/provider",
        ],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_transflow"))
            .args(&args)
            .current_dir(std::env::temp_dir())
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(result.stderr.is_empty(), "{args:?}");
        assert!(!result.stdout.contains(&0x1b));
        assert_eq!(result.stdout.iter().filter(|b| **b == b'\n').count(), 1);
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        tf_protocol::validate_document("CliEnvelopeV1", &value).unwrap();
        assert_eq!(
            value["exit_status"].as_i64(),
            result.status.code().map(i64::from)
        );
        if args.contains(&"external") {
            assert_eq!(value["context"]["workspace"], "/synthetic/consumer");
        }
    }
}
#[test]
fn output_intent_respects_option_values_and_end_of_options() {
    for (args, json) in [
        (
            vec!["transflow", "--workspace", "--json", "--invalid"],
            false,
        ),
        (vec!["transflow", "--", "--json"], false),
        (vec!["transflow", "--json", "--color=invalid"], true),
    ] {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        transflow::run(args.into_iter().map(OsString::from), &mut out, &mut err).unwrap();
        assert_eq!(err.is_empty(), json);
        assert_eq!(out.is_empty(), !json);
    }
    let raw = "--unknown=fixture-private\x1b[2J\r\n";
    let (mut out, mut err) = (Vec::new(), Vec::new());
    transflow::run(
        ["transflow", "--json", raw].map(OsString::from),
        &mut out,
        &mut err,
    )
    .unwrap();
    assert!(!String::from_utf8(out).unwrap().contains("fixture-private"));
    assert!(err.is_empty());
    let huge = "a".repeat(5000);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    transflow::run(
        ["transflow", "--json", "--workspace", &huge].map(OsString::from),
        &mut out,
        &mut err,
    )
    .unwrap();
    assert!(serde_json::from_slice::<Value>(&out).is_ok());
    assert!(err.is_empty());
}

#[test]
fn complete_graph_json_has_no_small_result_cap_and_escapes_visual_controls() {
    let cases: Value =
        serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json")).unwrap();
    let mut graph = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "graph-depth-zero")
        .unwrap()["value"]
        .clone();
    graph["depth"] = Value::Null;
    graph["branch"] = serde_json::json!("review\u{202e}branch");
    graph["nodes"] = serde_json::json!((0..4000).map(|n|serde_json::json!({"identity":format!("pending:data/{n}"),"paths":[format!("data/{n}/{}","x".repeat(200))],"depth":n.to_string(),"external":false,"producer":true})).collect::<Vec<_>>());
    graph["total_nodes"] = serde_json::json!("4000");
    let redactor = Redactor::default();
    let result = CliEnvelope::inspection(
        &redactor.text("test").unwrap(),
        &RequestContext::default(),
        graph,
    )
    .unwrap();
    assert!(result.as_bytes().len() > 1024 * 1024);
    let text = std::str::from_utf8(result.as_bytes()).unwrap();
    assert!(!text.contains('\u{202e}'));
    let value: Value = serde_json::from_str(text).unwrap();
    assert_eq!(value["result"]["nodes"].as_array().unwrap().len(), 4000);
    assert_eq!(value["result"]["branch"], "review\u{202e}branch");
}
