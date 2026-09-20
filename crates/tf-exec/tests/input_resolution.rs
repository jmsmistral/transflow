//! Real publication/head/lease state: fallback and repeated aliases retain exact identities.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic publication fixtures"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{input::*, *};
use tf_store::{input_resolution::*, retention::LeaseKind};
fn binding(alias: &str, selector: BranchSelector, strict: bool, role: InputRole) -> InputBinding {
    let policies = BranchPolicySnapshot::new(
        vec!["missing".parse().unwrap(), "master".parse().unwrap()],
        BTreeMap::new(),
    )
    .unwrap();
    InputBinding {
        key: InputBindingKey::new(
            DatasetKey::new(workspace(), DatasetId::from_bytes([20; 16])),
            alias.into(),
        )
        .unwrap(),
        dataset: DatasetKey::new(workspace(), DatasetId::from_bytes([10; 16])),
        role,
        policy: policies
            .local(
                &"feature".parse().unwrap(),
                selector,
                if strict {
                    FallbackPermission::Prohibited
                } else {
                    FallbackPermission::Allowed
                },
                None,
            )
            .unwrap(),
    }
}
fn request(n: u8) -> ReadRequest {
    ReadRequest {
        lease: RequestId::from_bytes([n; 16]),
        operation: RequestId::from_bytes([90; 16]),
        kind: LeaseKind::Plan,
        now_us: 100,
        ttl_us: 100,
    }
}
async fn publish(h: &mut Harness, n: u8, g: u64) {
    let r = h.start(n, 10, g, contract()).await;
    h.publish(r, n).await.unwrap();
    h.finish(n, "SUCCEEDED").await;
}
#[test]
fn absent_candidates_are_reported_and_exact_pin_survives_new_head() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0).await;
        sqlx::query(
            "INSERT INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,'feature',0)",
        )
        .bind(BranchId::from_bytes([30; 16]).to_string())
        .bind(workspace().to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let read = store
            .repository()
            .unwrap()
            .resolve_input(
                binding("orders", BranchSelector::Omitted, false, InputRole::Data),
                &BTreeSet::new(),
                request(40),
            )
            .await
            .unwrap();
        assert_eq!(read.fallback_index(), 2);
        assert_eq!(
            read.missing_candidates()[0].reason,
            AbsenceReason::HeadAbsent
        );
        assert_eq!(
            read.missing_candidates()[1].reason,
            AbsenceReason::BranchAbsent
        );
        assert_eq!(read.resolved_branch().as_str(), "master");
        assert_eq!(read.version(), VersionId::from_bytes([10; 16]));
        store.close().await.unwrap();
        publish(&mut h, 11, 1).await;
        let completion = tf_exec::input_read::verify(h.owner.take().unwrap(), read)
            .await
            .unwrap();
        h.owner = Some(completion.owner);
        let artifact = completion.result.unwrap();
        let read = completion.read;
        assert_eq!(read.version(), VersionId::from_bytes([10; 16]));
        assert_eq!(artifact.digest(), h.digest);
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        store
            .repository()
            .unwrap()
            .release_read(read.lease())
            .await
            .unwrap();
        store.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM data_branches").await, 2);
    });
}
#[test]
fn strict_missing_and_planned_producer_do_not_use_available_fallback() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0).await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let b = binding("x", BranchSelector::Current, true, InputRole::Data);
        let result = store
            .repository()
            .unwrap()
            .resolve_input(b.clone(), &BTreeSet::new(), request(40))
            .await;
        assert!(matches!(result, Err(ResolutionError::Missing(attempts)) if attempts.len() == 1));
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .resolve_input(b.clone(), &BTreeSet::from([b.dataset]), request(41))
                .await,
            Err(ResolutionError::PlannedProducer)
        ));
        store.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM read_leases").await, 0);
    });
}
#[test]
fn failed_latest_attempt_preserves_good_head_and_named_override_is_not_fallback() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0).await;
        let _failed = h.start(11, 10, 1, contract()).await;
        h.finish(11, "FAILED").await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let read = store
            .repository()
            .unwrap()
            .resolve_input(
                binding(
                    "x",
                    BranchSelector::Named("master".parse().unwrap()),
                    false,
                    InputRole::Data,
                ),
                &BTreeSet::new(),
                request(40),
            )
            .await
            .unwrap();
        assert_eq!(read.fallback_index(), 0);
        assert_eq!(read.version(), VersionId::from_bytes([10; 16]));
        assert_eq!(read.provenance().resolution["output_branch"], "feature");
        store.close().await.unwrap();
    });
}
#[test]
fn corrupt_selected_head_fails_instead_of_falling_back() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0).await;
        let old_digest = h.digest;
        sqlx::query(
            "INSERT INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,'feature',0)",
        )
        .bind(BranchId::from_bytes([30; 16]).to_string())
        .bind(workspace().to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO dataset_heads SELECT ?,dataset_id,version_id,generation
             FROM dataset_heads WHERE branch_id=?",
        )
        .bind(BranchId::from_bytes([30; 16]).to_string())
        .bind(branch().to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
        // Give master a different, valid object, so fallback could conceal the corrupted feature head.
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/import/empty.parquet"),
            h.root.join("source/one"),
        )
        .unwrap();
        h.digest = tf_store::artifacts::ArtifactStore::open(&h.root)
            .unwrap()
            .prepare(
                &std::fs::File::open(h.root.join("source")).unwrap(),
                &["one".into()],
                writer(),
                RequestId::from_bytes([249; 16]),
                |_| Ok(()),
            )
            .unwrap()
            .digest()
            .unwrap();
        assert_ne!(h.digest, old_digest);
        publish(&mut h, 11, 1).await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let read = store
            .repository()
            .unwrap()
            .resolve_input(
                binding("x", BranchSelector::Omitted, false, InputRole::Data),
                &BTreeSet::new(),
                request(40),
            )
            .await
            .unwrap();
        assert_eq!(read.version(), VersionId::from_bytes([10; 16]));
        store.close().await.unwrap();
        let object = h
            .root
            .join(".transflow/runtime/objects")
            .join(&old_digest.hex()[..2])
            .join(old_digest.hex());
        writable(&object);
        std::fs::remove_file(object.join("manifest.json")).unwrap();
        let completion = tf_exec::input_read::verify(h.owner.take().unwrap(), read)
            .await
            .unwrap();
        h.owner = Some(completion.owner);
        assert!(matches!(
            completion.result,
            Err(tf_exec::input_read::Error::Artifact(_))
        ));
        assert_eq!(completion.read.fallback_index(), 0);
        // The healthy master head remains available; verification did not acquire another lease.
        tf_store::artifacts::ArtifactStore::open(&h.root)
            .unwrap()
            .verify(h.digest)
            .unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_heads").await, 2);
        assert_eq!(h.scalar("SELECT count(*) FROM read_leases").await, 1);
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        store
            .repository()
            .unwrap()
            .release_read(completion.read.lease())
            .await
            .unwrap();
        store.close().await.unwrap();
        assert_eq!(
            h.scalar("SELECT count(*) FROM read_leases WHERE released=0")
                .await,
            0
        );
    });
}
#[test]
fn repeated_dataset_aliases_keep_versions_roles_and_persisted_provenance() {
    use serde_json::json;
    use tf_protocol::canonical::{DigestKind, semantic_fingerprint};
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0).await;
        sqlx::query(
            "INSERT INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,'develop',0)",
        )
        .bind(BranchId::from_bytes([30; 16]).to_string())
        .bind(workspace().to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO dataset_heads SELECT ?,dataset_id,version_id,generation
             FROM dataset_heads WHERE branch_id=?",
        )
        .bind(BranchId::from_bytes([30; 16]).to_string())
        .bind(branch().to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
        publish(&mut h, 11, 1).await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        let left = repo
            .resolve_input(
                binding(
                    "current",
                    BranchSelector::Named("master".parse().unwrap()),
                    false,
                    InputRole::Data,
                ),
                &BTreeSet::new(),
                request(40),
            )
            .await
            .unwrap();
        let right = repo
            .resolve_input(
                binding(
                    "baseline",
                    BranchSelector::Named("develop".parse().unwrap()),
                    false,
                    InputRole::Validation,
                ),
                &BTreeSet::new(),
                request(41),
            )
            .await
            .unwrap();
        assert_ne!(left.binding().key, right.binding().key);
        assert_ne!(left.version(), right.version());
        assert_ne!(left.semantic_value(), right.semantic_value());
        let semantic = right.semantic_value();
        assert_eq!(semantic["role"], "validation");
        let fingerprint = |value: &serde_json::Value| {
            semantic_fingerprint(
                DigestKind::Compute,
                &json!({
                    "semantic": value, "presentation": {}
                }),
            )
            .unwrap()
        };
        for field in ["alias", "role", "version", "resolved_branch"] {
            let mut changed = semantic.clone();
            changed[field] = "changed".into();
            assert_ne!(fingerprint(&semantic), fingerprint(&changed));
        }
        let inputs = vec![left.provenance(), right.provenance()];
        store.close().await.unwrap();
        let mut c = contract();
        c.inputs = inputs;
        let r = h.start(20, 20, 0, c).await;
        h.publish(r, 20).await.unwrap();
        h.finish(20, "SUCCEEDED").await;
        assert_eq!(h.scalar("SELECT count(*) FROM version_inputs").await, 2);
        assert_eq!(
            h.scalar("SELECT count(*) FROM version_inputs WHERE role='validation'")
                .await,
            1
        );
        let retained: String =
            sqlx::query_scalar("SELECT version_id FROM dataset_heads WHERE branch_id=?")
                .bind(BranchId::from_bytes([30; 16]).to_string())
                .fetch_one(&mut h.db)
                .await
                .unwrap();
        assert_eq!(retained, VersionId::from_bytes([10; 16]).to_string());
    });
}

#[test]
fn rejected_input_check_keeps_exact_binding_and_last_good_output() {
    use serde_json::json;
    use tf_store::publication::{CheckRequirement, CheckSubject};
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0).await;
        let first = h.start(20, 20, 0, contract()).await;
        h.publish(first, 20).await.unwrap();
        h.finish(20, "SUCCEEDED").await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let read = store.repository().unwrap().resolve_input(
            binding("x", BranchSelector::Omitted, false, InputRole::Data),
            &BTreeSet::new(), request(40),
        ).await.unwrap();
        store.close().await.unwrap();
        let fingerprint = "c".repeat(64);
        sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'input','x','check',?)")
            .bind(&fingerprint).bind(json!({"severity":"FAIL"}).to_string()).execute(&mut h.db).await.unwrap();
        let mut c = contract();
        c.inputs.push(read.provenance());
        c.checks.push(CheckRequirement { definition: fingerprint.clone(), subject: CheckSubject::Input("x".into()), required: true });
        let next = h.start(21, 20, 1, c).await;
        sqlx::query(
            "INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,
             outcome,metrics_json,started_at_us,finished_at_us) VALUES(?,?,?,?,'VIOLATION','{}',5,10)",
        ).bind(RequestId::from_bytes([42; 16]).to_string()).bind(next.intent.attempt().to_string())
            .bind(json!({"alias":"x","artifact_digest":read.lease().artifact().hex(),"version_id":read.version().to_string()}).to_string())
            .bind(fingerprint).execute(&mut h.db).await.unwrap();
        assert!(h.publish(next, 21).await.is_err());
        h.finish(21, "FAILED").await;
        assert_eq!(read.version(), VersionId::from_bytes([10; 16]));
        assert_eq!(h.scalar("SELECT count(*) FROM read_leases").await, 1);
        let output: String = sqlx::query_scalar("SELECT version_id FROM dataset_heads WHERE dataset_id=?")
            .bind(DatasetId::from_bytes([20; 16]).to_string()).fetch_one(&mut h.db).await.unwrap();
        assert_eq!(output, VersionId::from_bytes([20; 16]).to_string());
    });
}

#[test]
fn collection_claim_on_existing_head_is_an_error_not_missing_data() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0).await;
        // Simulate inconsistent metadata; a normal root-checked collector refuses this claim.
        sqlx::query("INSERT INTO artifact_gc_claims VALUES(?,?,100)")
            .bind(h.digest.hex())
            .bind(RequestId::from_bytes([50; 16]).to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let result = store
            .repository()
            .unwrap()
            .resolve_input(
                binding(
                    "x",
                    BranchSelector::Named("master".parse().unwrap()),
                    false,
                    InputRole::Data,
                ),
                &BTreeSet::new(),
                request(40),
            )
            .await;
        assert!(matches!(result, Err(ResolutionError::Retention(_))));
        store.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM read_leases").await, 0);
    });
}
