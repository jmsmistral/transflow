//! Immutable evidence, diagnostic privacy and exact input-certificate integration.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic evidence fixtures"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tf_domain::{execution::*, *};
use tf_protocol::canonical::{DigestKind, content_digest};
use tf_store::{artifacts::ArtifactStore, check_evidence::*, publication::*};

fn hash(value: &Value) -> String {
    content_digest(DigestKind::Compute, value).unwrap().hex()
}
fn declaration(severity: &str, samples: &str) -> Value {
    json!({"id":"nonnull","name":"Non-null IDs","expectation":{"kind":"non_null","columns":["id"]},"on_error":severity,"null_policy":"fail","sample_rows":samples,"description":null})
}
fn policy() -> Value {
    json!({"allowed_columns":["id"],"sensitive_columns":[],"max_rows":20})
}
fn job(r: &PublicationRequest) -> Job {
    let i = &r.intent;
    Job::new(
        i.job(),
        i.build(),
        i.binding(),
        i.target(),
        i.fence(),
        RetryPolicy::default(),
        EventTime(0),
    )
}
fn envelope(
    r: &PublicationRequest,
    d: &Value,
    status: &str,
    input: bool,
    sample: Option<&str>,
) -> Value {
    json!({"format_version":1,"request_id":r.intent.attempt().to_string(),"attempt_id":r.intent.attempt().to_string(),"artifact_digest":r.intent.artifact().hex(),"subject_version":if input {json!(VersionId::from_bytes([9;16]).to_string())} else {Value::Null},"consumer_definition":"c".repeat(64),"binding":if input {json!("items")} else {Value::Null},"phase":if input {"input"} else {"output"},"evaluator":"duckdb-1.5.5:core-v1","semantics":"strict-null-key-v1","started_us":"5","finished_us":"10","duration_us":"5","sample_policy":policy(),"checks":[{"id":d["id"],"name":d["name"],"definition_digest":hash(d),"status":status,"severity":d["on_error"],"exact":status!="ERROR","failed_rows":if status=="ERROR" {Value::Null} else {json!(if status=="PASS" {"0"} else {"1"})},"error":if status=="ERROR" {json!("evaluator_failure")} else {Value::Null},"metrics":[],"sample":sample}]})
}
async fn setup(h: &mut Harness, d: &Value, input: bool) -> PublicationContract {
    let mut c = contract();
    if input {
        let p = h.start(9, 9, 0, contract()).await;
        h.publish(p, 9).await.unwrap();
        h.finish(9, "SUCCEEDED").await;
        c.inputs.push(InputProvenance {
            alias: "items".into(),
            dataset: DatasetKey::new(workspace(), DatasetId::from_bytes([9; 16])),
            version: VersionId::from_bytes([9; 16]),
            artifact: h.digest,
            declared_branch: json!({"kind":"current","name":null}),
            starting_branch: "master".parse().unwrap(),
            resolved_branch: "master".parse().unwrap(),
            role: InputRole::Data,
            resolution: json!({}),
        });
    }
    let session = h.session();
    let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
    c.checks = store
        .repository()
        .unwrap()
        .register_check_definitions(
            workspace(),
            session,
            if input { Some("items") } else { None },
            std::slice::from_ref(d),
        )
        .await
        .unwrap();
    store.close().await.unwrap();
    c
}
#[test]
fn immutable_evidence_survives_reopen_and_samples_require_explicit_inspection() {
    runtime().block_on(async {
    let mut h=Harness::new().await;let d=declaration("FAIL","1");let c=setup(&mut h,&d,false).await;
    let r=h.start(20,20,0,c).await;let job=job(&r);let consumer="c".repeat(64);
    let ctx=Context{job:&job,attempt:r.intent.attempt(),contract:&r.contract,at_us:20,consumer_definition:&consumer,sample_policy:&policy()};
    let artifacts=ArtifactStore::open(&h.root).unwrap();let f=h.files(51);
    let object=artifacts.prepare(&f.root,&f.files,f.writer,f.staging,|_|Ok(())).unwrap().install(|_|Ok(())).unwrap();
    let sample=json!({"columns":object.manifest()["logical_schema"]["fields"],"rows":[[{"type":"i64","value":"1"}]],"limit":1,"truncated":false,"reason":null});
    let digest=hash(&sample);let samples=BTreeMap::from([(digest.clone(),sample.clone())]);let e=envelope(&r,&d,"VIOLATION",false,Some(&digest));
    let mut store=h.owner.as_mut().unwrap().open_store().await.unwrap();let repo=store.repository().unwrap();
    let ids=repo.record_check_evaluation(&ctx,&e,object.manifest(),&samples).await.unwrap();
    assert_eq!(repo.record_check_evaluation(&ctx,&e,object.manifest(),&samples).await.unwrap(),ids);
    repo.record_failed_check_candidate(&ctx,&object).await.unwrap();
    assert!(repo.retention_roots(100,Default::default()).await.unwrap().artifacts.contains(&h.digest.hex()));
    assert!(repo.claim_collection(RequestId::from_bytes([70;16]),h.digest,100,Default::default()).await.is_err());
    store.close().await.unwrap();
    let mut store=h.owner.as_mut().unwrap().open_store().await.unwrap();let repo=store.repository().unwrap();
    let safe=repo.check_evidence(ids[0]).await.unwrap();assert_eq!(safe["evaluation"],e);assert!(!safe.to_string().contains("\"rows\""));
    let explicit=repo.inspect_check_sample(ids[0],Inspection::ExplicitDiagnostic).await.unwrap();assert_eq!(explicit["sample"],sample);assert!(explicit["warning"].is_string());
    let failed=repo.inspect_failed_check_candidate(r.intent.attempt(),Inspection::ExplicitDiagnostic).await.unwrap();assert_eq!(failed["artifact_digest"],h.digest.hex());assert!(failed["warning"].as_str().unwrap().contains("unpublished"));store.close().await.unwrap();
    assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await,0);assert_eq!(h.scalar("SELECT count(*) FROM check_certificates").await,0);
    for statement in ["UPDATE check_results SET outcome='PASS'","DELETE FROM check_results","UPDATE check_samples SET payload_json='{}'","DELETE FROM failed_check_candidates","UPDATE check_definitions SET policy_json='{}'"] {assert!(sqlx::query(statement).execute(&mut h.db).await.is_err());}
 });
}
#[test]
fn input_certificate_reuse_preserves_original_times_and_rejects_changed_identity_or_force() {
    runtime().block_on(async {
        for status in ["PASS", "VIOLATION"] {
            let mut h = Harness::new().await;
            let d = declaration("WARN", "0");
            let c = setup(&mut h, &d, true).await;
            let r = h.start(20, 20, 0, c.clone()).await;
            let original_job = job(&r);
            let consumer = "c".repeat(64);
            let ctx = Context {
                job: &original_job,
                attempt: r.intent.attempt(),
                contract: &c,
                at_us: 20,
                consumer_definition: &consumer,
                sample_policy: &policy(),
            };
            let artifacts = ArtifactStore::open(&h.root).unwrap();
            let object = artifacts.verify(h.digest).unwrap();
            let session = h.session();
            let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
            let original = store
                .repository()
                .unwrap()
                .record_check_evaluation(
                    &ctx,
                    &envelope(&r, &d, status, true, None),
                    object.manifest(),
                    &BTreeMap::new(),
                )
                .await
                .unwrap()[0];
            store.close().await.unwrap();
            let destination = h.start(21, 21, 0, c.clone()).await;
            let destination_job = job(&destination);
            let ctx = Context {
                job: &destination_job,
                attempt: destination.intent.attempt(),
                contract: &c,
                at_us: 100,
                consumer_definition: &consumer,
                sample_policy: &policy(),
            };
            let p = policy();
            let q = InputCertificateQuery {
                input: &c.inputs[0],
                check: &d,
                consumer_definition: &consumer,
                sample_policy: &policy(),
                evaluator: "duckdb-1.5.5:core-v1",
                semantics: "strict-null-key-v1",
                normalization: NORMALIZATION,
            };
            let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
            let repo = store.repository().unwrap();
            for field in [
                "consumer",
                "evaluator",
                "semantics",
                "normalization",
                "policy",
                "declaration",
                "severity",
                "ast",
                "sample_rows",
                "alias",
            ] {
                let mut changed = q.input.clone();
                changed.alias = "other".into();
                let mut cd = d.clone();
                match field {
                    "severity" => cd["on_error"] = json!("FAIL"),
                    "ast" => cd["expectation"]["kind"] = json!("primary_key"),
                    "sample_rows" => cd["sample_rows"] = json!("1"),
                    _ => cd["null_policy"] = json!("ignore"),
                }
                let mut cp = p.clone();
                cp["max_rows"] = json!(1);
                let other = "d".repeat(64);
                let altered = InputCertificateQuery {
                    input: if field == "alias" { &changed } else { q.input },
                    check: if matches!(field, "declaration" | "severity" | "ast" | "sample_rows") {
                        &cd
                    } else {
                        q.check
                    },
                    consumer_definition: if field == "consumer" {
                        &other
                    } else {
                        q.consumer_definition
                    },
                    evaluator: if field == "evaluator" {
                        "other"
                    } else {
                        q.evaluator
                    },
                    semantics: if field == "semantics" {
                        "other"
                    } else {
                        q.semantics
                    },
                    normalization: if field == "normalization" {
                        "other"
                    } else {
                        q.normalization
                    },
                    sample_policy: if field == "policy" {
                        &cp
                    } else {
                        q.sample_policy
                    },
                };
                assert!(
                    repo.lookup_input_certificate(workspace(), session, &altered, &object)
                        .await
                        .unwrap()
                        .is_none(),
                    "{field}"
                );
            }
            let cert = repo
                .lookup_input_certificate(workspace(), session, &q, &object)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(cert.original_result(), original);
            let wrong = "d".repeat(64);
            let wrong_ctx = Context {
                consumer_definition: &wrong,
                ..ctx
            };
            assert!(
                repo.reuse_input_certificate(&wrong_ctx, cert)
                    .await
                    .is_err()
            );
            sqlx::query("UPDATE build_plans SET context_json='{\"force\":true}' WHERE id=?")
                .bind(destination.intent.binding().plan.to_string())
                .execute(&mut h.db)
                .await
                .unwrap();
            let cert = repo
                .lookup_input_certificate(workspace(), session, &q, &object)
                .await
                .unwrap()
                .unwrap();
            assert!(repo.reuse_input_certificate(&ctx, cert).await.is_err());
            sqlx::query("UPDATE build_plans SET context_json='{}' WHERE id=?")
                .bind(destination.intent.binding().plan.to_string())
                .execute(&mut h.db)
                .await
                .unwrap();
            let cert = repo
                .lookup_input_certificate(workspace(), session, &q, &object)
                .await
                .unwrap()
                .unwrap();
            sqlx::query("UPDATE builds SET cancel_requested=1 WHERE id=?")
                .bind(destination.intent.build().to_string())
                .execute(&mut h.db)
                .await
                .unwrap();
            assert!(repo.reuse_input_certificate(&ctx, cert).await.is_err());
            sqlx::query("UPDATE builds SET cancel_requested=0 WHERE id=?")
                .bind(destination.intent.build().to_string())
                .execute(&mut h.db)
                .await
                .unwrap();
            let cert = repo
                .lookup_input_certificate(workspace(), session, &q, &object)
                .await
                .unwrap()
                .unwrap();
            let reused = repo.reuse_input_certificate(&ctx, cert).await.unwrap();
            let view = repo.check_evidence(reused).await.unwrap();
            assert_eq!(view["reused_at_us"], "100");
            assert_eq!(view["original_result_id"], original.to_string());
            assert_eq!(view["evaluation"]["finished_us"], "10");
            assert_eq!(view["evaluation"]["checks"][0]["status"], status);
            store.close().await.unwrap();
            assert_eq!(
                h.scalar(
                    "SELECT count(*) FROM check_results WHERE started_at_us=5 AND finished_at_us=10"
                )
                .await,
                2
            );
            assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 1);
        }
    });
}
#[test]
fn policy_and_failed_evidence_are_checked_atomically() {
    runtime().block_on(async {
  for failure in ["default","requested-cap","sensitive","error"] {
    let mut h=Harness::new().await;let d=declaration("WARN",if failure=="default" {"0"} else {"1"});let c=setup(&mut h,&d,false).await;let r=h.start(20,20,0,c).await;let j=job(&r);let consumer="c".repeat(64);let ctx=Context{job:&j,attempt:r.intent.attempt(),contract:&r.contract,at_us:20,consumer_definition:&consumer,sample_policy:&policy()};
    let artifacts=ArtifactStore::open(&h.root).unwrap();let f=h.files(51);let object=artifacts.prepare(&f.root,&f.files,f.writer,f.staging,|_|Ok(())).unwrap().install(|_|Ok(())).unwrap();
    let sample=json!({"columns":object.manifest()["logical_schema"]["fields"],"rows":[[{"type":"i64","value":"1"}]],"limit":if failure=="requested-cap" {2} else {1},"truncated":false,"reason":null});let digest=hash(&sample);let samples=BTreeMap::from([(digest.clone(),sample)]);let mut e=envelope(&r,&d,"VIOLATION",false,Some(&digest));if failure=="sensitive" {e["sample_policy"]["sensitive_columns"]=json!(["id"]);}
    let mut store=h.owner.as_mut().unwrap().open_store().await.unwrap();let repo=store.repository().unwrap();
    if failure=="error" {let e=envelope(&r,&d,"ERROR",false,None);repo.record_check_evaluation(&ctx,&e,object.manifest(),&BTreeMap::new()).await.unwrap();} else {let ctx=Context{sample_policy:&e["sample_policy"],..ctx};assert!(repo.record_check_evaluation(&ctx,&e,object.manifest(),&samples).await.is_err());}
    store.close().await.unwrap();assert_eq!(h.scalar("SELECT count(*) FROM check_certificates").await,0);assert_eq!(h.scalar("SELECT count(*) FROM check_samples").await,0);assert_eq!(h.scalar("SELECT count(*) FROM check_evaluations").await,i64::from(failure=="error"));
  }
 });
}
