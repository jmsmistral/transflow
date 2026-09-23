//! Exact gate evidence must survive persistence without weakening the frozen contract.
#![allow(
    clippy::unwrap_used,
    reason = "Synthetic SQLite integration assertions"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::{Value, json};
use tf_domain::RequestId;
use tf_store::publication::*;

fn envelope(request: &PublicationRequest, declaration: &str, required: bool) -> Value {
    json!({"format_version":1,"request_id":RequestId::from_bytes([99;16]).to_string(),
    "attempt_id":request.intent.attempt().to_string(),"artifact_digest":request.intent.artifact().hex(),
    "subject_version":null,"consumer_definition":"a".repeat(64),"binding":null,"phase":"output",
    "evaluator":"duckdb-1.5.5:core-v1","semantics":"strict-null-key-v1","started_us":"5","finished_us":"10","duration_us":"5",
    "checks":[{"id":"check","name":"check","definition_digest":declaration,"status":if required{"PASS"}else{"VIOLATION"},
    "severity":if required{"FAIL"}else{"WARN"},"exact":true,"failed_rows":"1","error":null,"metrics":[],"sample":null}]})
}
#[test]
fn gate_rows_are_exact_atomic_and_never_replace_published_warnings() {
    runtime().block_on(async {
        for mode in [
            "pass",
            "warn",
            "error",
            "skipped",
            "missing",
            "duplicate",
            "attempt",
            "artifact",
            "version",
            "alias",
            "definition",
            "late",
            "inexact",
            "severity",
            "phase",
            "canceled",
        ] {
            let mut h = Harness::new().await;
            let required = mode != "warn" && mode != "error";
            let declaration = "c".repeat(64);
            let definition = gate_definition(&declaration, None).unwrap();
            sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'output',NULL,'check',?)")
                .bind(&definition)
                .bind(json!({"severity":if required{"FAIL"}else{"WARN"}}).to_string())
                .execute(&mut h.db)
                .await
                .unwrap();
            let mut contract = contract();
            contract.checks.push(CheckRequirement {
                definition,
                subject: CheckSubject::Output,
                required,
            });
            let request = h.start(10, 10, 0, contract).await;
            let mut result = envelope(&request, &declaration, required);
            match mode {
                "error" => result["checks"][0]["status"] = json!("ERROR"),
                "skipped" => result["checks"][0]["status"] = json!("SKIPPED"),
                "missing" => result["checks"] = json!([]),
                "duplicate" => result["checks"]
                    .as_array_mut()
                    .unwrap()
                    .push(envelope(&request, &declaration, required)["checks"][0].clone()),
                "attempt" => {
                    result["attempt_id"] = json!(RequestId::from_bytes([90; 16]).to_string())
                }
                "artifact" => result["artifact_digest"] = json!("d".repeat(64)),
                "version" => {
                    result["subject_version"] = json!(RequestId::from_bytes([90; 16]).to_string())
                }
                "alias" => result["binding"] = json!("other"),
                "definition" => result["checks"][0]["definition_digest"] = json!("d".repeat(64)),
                "late" => result["finished_us"] = json!("21"),
                "inexact" => result["checks"][0]["exact"] = json!(false),
                "severity" => result["checks"][0]["severity"] = json!("WARN"),
                "phase" => result["phase"] = json!("input"),
                "canceled" => {
                    sqlx::query("UPDATE builds SET cancel_requested=1")
                        .execute(&mut h.db)
                        .await
                        .unwrap();
                }
                _ => {}
            }
            let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
            let recorded = store
                .repository()
                .unwrap()
                .record_gate_results(&request, &[result.clone()])
                .await;
            store.close().await.unwrap();
            let allowed = mode == "pass" || mode == "warn";
            assert_eq!(recorded.is_ok(), allowed, "{mode}: {recorded:?}");
            assert_eq!(
                h.scalar("SELECT count(*) FROM check_results").await,
                i64::from(allowed),
                "{mode}"
            );
            if allowed {
                h.publish(request.clone(), 10).await.unwrap();
                let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
                assert!(
                    store
                        .repository()
                        .unwrap()
                        .record_gate_results(&request, &[result])
                        .await
                        .is_err()
                );
                store.close().await.unwrap();
                assert_eq!(
                    h.scalar("SELECT count(*) FROM version_check_results").await,
                    1
                );
                assert_eq!(
                    h.scalar("SELECT count(*) FROM check_results WHERE outcome='VIOLATION'")
                        .await,
                    i64::from(mode == "warn")
                );
            }
        }
    });
}
#[test]
fn identical_declarations_remain_scoped_to_exact_alias_and_phase() {
    let declaration = "c".repeat(64);
    let keys =
        [None, Some("items"), Some("baseline")].map(|a| gate_definition(&declaration, a).unwrap());
    assert_ne!(keys[0], keys[1]);
    assert_ne!(keys[1], keys[2]);
    assert!(gate_definition("not a digest", None).is_err());
}
