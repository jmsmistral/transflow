//! End-to-end captured comparison against real SQLite and published Parquet history.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic integration assertions"
)]
#[path = "../../tf-exec/tests/support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs};
use tf_catalog::{
    RegistrySnapshot,
    candidate::CandidateIdentity as Id,
    capture::{CaptureLimits, SourceSnapshot},
    compute::{self, Execution},
    validation::{self, ValidationRequest},
    workspace::Workspace,
};
use tf_domain::*;
use tf_plan::freshness::*;
use tf_store::{freshness::Snapshot, publication::*};
use transflow::why::{self, Request, Semantics};
fn key() -> DatasetKey {
    DatasetKey::new(workspace(), DatasetId::from_bytes([10; 16]))
}
fn setup(h: &Harness) {
    fs::create_dir_all(h.root.join("src")).unwrap();
    fs::write(
        h.root.join("workspace.toml"),
        format!("format_version=1\nworkspace_id='{}'\n", workspace()),
    )
    .unwrap();
    fs::write(
        h.root.join(".transflow/catalog.toml"),
        format!(
            "format_version=1\n[[datasets]]\nid='{}'\npath='curated/output'\nkind='transform'\n",
            key().dataset_id()
        ),
    )
    .unwrap();
    fs::write(h.root.join("src/output.py"), "# fixture\n").unwrap();
    fs::write(h.root.join("src/helper.py"), "VALUE=1\n").unwrap();
    fs::write(h.root.join("requirements.in"), "# fixture\n").unwrap();
    fs::write(h.root.join("requirements.lock"), "# fixture\n").unwrap();
}
fn definition() -> Value {
    json!({"module":"output","function":"produce","path":"src/output.py","line":1,"source":false,"engine":"polars","inputs":[],"output":{"ref":{"form":"string","value":"curated/output"},"schema":null,"checks":[]},"parameters":[],"wall_timeout_seconds":null,"cache":"deterministic","refresh":null,"secret_refs":[],"lineage_json":null})
}
fn context<T>(
    h: &Harness,
    definition: Option<Value>,
    run: impl FnOnce(&validation::ValidatedGraph, &ValidationRequest<'_>, &SourceSnapshot) -> T,
) -> T {
    let w = Workspace::load(&h.root, None).unwrap();
    let capture = SourceSnapshot::capture(&w, CaptureLimits::default()).unwrap();
    let registry = RegistrySnapshot::parse(
        workspace(),
        &fs::read_to_string(h.root.join(".transflow/catalog.toml")).unwrap(),
    )
    .unwrap();
    let modules = BTreeMap::from([
        ("output".into(), "src/output.py".into()),
        ("helper".into(), "src/helper.py".into()),
    ]);
    let definitions = definition.into_iter().collect::<Vec<_>>();
    let discovery = json!({"format_version":1,"source_snapshot_id":capture.id().unwrap().to_string(),"catalog_fingerprint":registry.sdk_projection(capture.id().unwrap()).unwrap()["catalog_fingerprint"],"environment_fingerprint":"a".repeat(64),"definitions":definitions,"imported_modules":["helper","output"]});
    let config = fs::read_to_string(h.root.join("workspace.toml")).unwrap();
    let context = ValidationRequest {
        registry: &registry,
        source: capture.id().unwrap(),
        source_digest: capture.digest(),
        config_toml: &config,
        modules: &modules,
        discovery: &discovery,
        environment_fingerprint: &"a".repeat(64),
        sdk_version: validation::SDK_VERSION,
        check_semantics: validation::CHECK_SEMANTICS,
    };
    let graph = validation::validate(&context).unwrap();
    run(&graph, &context, &capture)
}
fn evidence(h: &Harness) -> compute::ComputeKey {
    context(h, Some(definition()), |graph, context, capture| {
        compute::finalize(
            graph,
            context,
            capture,
            Execution {
                output: key(),
                parameters: &json!({}),
                inputs: &[],
                writer: &writer(),
                evaluation_us: None,
                secret_versions: &BTreeMap::new(),
            },
        )
        .unwrap()
    })
}
fn request() -> Request {
    Request {
        python: None,
        branch: "master".parse().unwrap(),
        fallbacks: None,
        semantics: BTreeMap::from([(
            key(),
            Semantics {
                parameters: json!({}),
                writer: writer(),
                evaluation_us: None,
                secret_versions: BTreeMap::new(),
            },
        )]),
        at_us: 100,
    }
}
async fn snapshot(h: &mut Harness) -> Snapshot {
    let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
    let mut reader = owned.repository().unwrap().reader().await.unwrap();
    let s = reader.freshness_snapshot(workspace()).await.unwrap();
    reader.close().await.unwrap();
    owned.close().await.unwrap();
    s
}
fn explain(h: &Harness, s: &Snapshot) -> why::Report {
    let verified = tf_store::artifacts::ArtifactStore::open(&h.root)
        .unwrap()
        .verify(h.digest)
        .unwrap();
    context(h, Some(definition()), |graph, context, capture| {
        why::explain(
            graph,
            context,
            capture,
            s,
            &BTreeMap::from([(verified.digest().hex(), Materialization::Available)]),
            &request(),
        )
        .unwrap()
    })
}
async fn publish(h: &mut Harness, n: u8, generation: u64, with_evidence: bool) {
    let e = evidence(h);
    let mut c = contract();
    c.compute_fingerprint = e.digest().hex();
    c.check_fingerprint = e.check_fingerprint().hex();
    let p = h
        .start_with_evidence(
            n,
            10,
            generation,
            c,
            with_evidence.then(|| serde_json::to_value(e.evidence()).unwrap()),
        )
        .await;
    h.publish(p, n).await.unwrap();
    h.finish(n, "SUCCEEDED").await;
}
#[test]
fn helper_change_and_failed_retry_explain_the_same_usable_last_good_head() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        setup(&h);
        publish(&mut h, 10, 0, true).await;
        let first = snapshot(&mut h).await;
        let initial = explain(&h, &first);
        assert_eq!(
            initial.datasets[&Id::Registered(key())].reasons[0].code,
            "CACHE_MATCH"
        );
        fs::write(h.root.join("src/helper.py"), "VALUE=2\n").unwrap();
        h.seed(20, 10).await;
        sqlx::query(
            "UPDATE attempts SET state='FAILED',started_at_us=50,finished_at_us=60 WHERE id=?",
        )
        .bind(AttemptId::from_bytes([20; 16]).to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
        let current = snapshot(&mut h).await;
        let report = explain(&h, &current);
        let status = &report.datasets[&Id::Registered(key())];
        assert_eq!(status.direct_data, Freshness::Current);
        assert_eq!(status.direct_logic, Freshness::Stale);
        assert_eq!(status.latest_attempt, Some(AttemptState::Failed));
        assert_eq!(status.materialization, Materialization::Available);
        assert_eq!(status.output_quality, Quality::NoChecks);
        let cause = status
            .reasons
            .iter()
            .find(|r| r.subject == "src/helper.py")
            .unwrap();
        assert!(cause.human(&BTreeMap::new()).contains("src/helper.py"));
        assert_ne!(cause.before, cause.after);
        assert_eq!(
            current.heads[&(key().dataset_id(), "master".parse().unwrap())].version,
            VersionId::from_bytes([10; 16])
        );
        assert_eq!(
            explain(&h, &first).datasets[&Id::Registered(key())].latest_attempt,
            Some(AttemptState::Succeeded)
        );
        assert!(
            sqlx::query("UPDATE computation_evidence SET evidence_json='{}'")
                .execute(&mut h.db)
                .await
                .is_err()
        );
    });
}
#[test]
fn old_snapshot_remains_stable_after_publication_and_legacy_evidence_is_unknown() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        setup(&h);
        publish(&mut h, 10, 0, true).await;
        let first = snapshot(&mut h).await;
        fs::write(h.root.join("src/helper.py"), "VALUE=2\n").unwrap();
        publish(&mut h, 20, 1, true).await;
        assert_eq!(
            explain(&h, &first).datasets[&Id::Registered(key())].direct_logic,
            Freshness::Stale
        );
        let second = snapshot(&mut h).await;
        assert_eq!(
            explain(&h, &second).datasets[&Id::Registered(key())].direct_logic,
            Freshness::Current
        );
        publish(&mut h, 30, 2, false).await;
        let legacy = snapshot(&mut h).await;
        assert_eq!(
            explain(&h, &legacy).datasets[&Id::Registered(key())].direct_logic,
            Freshness::Unknown
        );
        context(&h, None, |graph, context, capture| {
            let r = why::explain(
                graph,
                context,
                capture,
                &legacy,
                &BTreeMap::new(),
                &request(),
            )
            .unwrap();
            let s = &r.datasets[&Id::Registered(key())];
            assert_eq!(s.reasons[0].code, "SOURCE_DEFINITION_UNAVAILABLE");
            assert_eq!(s.materialization, Materialization::Unknown);
        });
    });
}
#[test]
fn output_warning_and_later_failed_input_checks_remain_separate() {
    runtime().block_on(async {
        let mut h=Harness::new().await;setup(&h);let output="c".repeat(64);let input="d".repeat(64);
        sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'output',NULL,'check','{\"severity\":\"WARN\"}')").bind(&output).execute(&mut h.db).await.unwrap();
        let mut c=contract();c.checks=vec![CheckRequirement{definition:output.clone(),subject:CheckSubject::Output,required:false}];
        let p=h.start(10,10,0,c).await;
        sqlx::query("INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,outcome,metrics_json,started_at_us,finished_at_us) VALUES(?,?,?,?,'VIOLATION','{}',5,10)").bind(RequestId::from_bytes([50;16]).to_string()).bind(p.intent.attempt().to_string()).bind(json!({"artifact_digest":h.digest.hex()}).to_string()).bind(&output).execute(&mut h.db).await.unwrap();
        h.publish(p,10).await.unwrap();h.finish(10,"SUCCEEDED").await;
        h.seed(20,10).await;
        sqlx::query("INSERT INTO publication_contracts VALUES(?,?)").bind(JobId::from_bytes([20;16]).to_string()).bind(json!({"checks":[{"definition":input,"subject":{"input":"rows"},"required":true}],"inputs":[{"alias":"rows","version":VersionId::from_bytes([10;16]).to_string(),"artifact":h.digest.hex()}]}).to_string()).execute(&mut h.db).await.unwrap();
        sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'input','rows','check','{\"severity\":\"FAIL\"}')").bind(&input).execute(&mut h.db).await.unwrap();
        sqlx::query("INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,outcome,metrics_json,started_at_us,finished_at_us) VALUES(?,?,?,?,'VIOLATION','{}',50,60)").bind(RequestId::from_bytes([51;16]).to_string()).bind(AttemptId::from_bytes([20;16]).to_string()).bind(json!({"alias":"rows","artifact_digest":h.digest.hex(),"version_id":VersionId::from_bytes([10;16]).to_string()}).to_string()).bind(&input).execute(&mut h.db).await.unwrap();
        sqlx::query("UPDATE attempts SET state='FAILED',started_at_us=50,finished_at_us=60 WHERE id=?").bind(AttemptId::from_bytes([20;16]).to_string()).execute(&mut h.db).await.unwrap();
        let frozen=snapshot(&mut h).await;let report=explain(&h,&frozen);let s=&report.datasets[&Id::Registered(key())];
        assert_eq!(s.output_quality,Quality::Warning);assert_eq!(s.input_quality["rows"],InputQuality::Failed);assert_eq!(s.latest_attempt,Some(AttemptState::Failed));
        assert_eq!(frozen.heads[&(key().dataset_id(),"master".parse().unwrap())].checks[0].finished_us,Some(10));
        let mut mismatched=frozen.clone();
        mismatched.attempts.get_mut(&(key().dataset_id(),"master".parse().unwrap())).unwrap().checks[0].subject["version_id"]=json!(VersionId::from_bytes([99;16]).to_string());
        assert_eq!(explain(&h,&mismatched).datasets[&Id::Registered(key())].input_quality["rows"],InputQuality::Unavailable);

    });
}
