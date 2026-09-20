//! Real retained versions, strict object reads, replay immutability and all-or-none leases.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic retained publication fixtures"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{input::*, *};
use tf_store::{input_resolution::*, replay::ReplayBoundary, retention::LeaseKind};
fn key(n: u8) -> DatasetKey {
    DatasetKey::new(workspace(), DatasetId::from_bytes([n; 16]))
}
fn binding(alias: &str, dataset: u8) -> InputBinding {
    InputBinding {
        key: InputBindingKey::new(key(20), alias.into()).unwrap(),
        dataset: key(dataset),
        role: InputRole::Data,
        policy: BranchPolicySnapshot::new(vec!["master".parse().unwrap()], BTreeMap::new())
            .unwrap()
            .local(
                &"feature".parse().unwrap(),
                BranchSelector::Named("absent".parse().unwrap()),
                FallbackPermission::Prohibited,
                None,
            )
            .unwrap(),
    }
}
fn request(n: u8) -> ReadRequest {
    ReadRequest {
        lease: RequestId::from_bytes([n; 16]),
        operation: RequestId::from_bytes([99; 16]),
        kind: LeaseKind::Plan,
        now_us: 100,
        ttl_us: 100,
    }
}
async fn publish(h: &mut Harness, n: u8, dataset: u8, generation: u64) {
    let r = h.start(n, dataset, generation, contract()).await;
    h.publish(r, n).await.unwrap();
    h.finish(n, "SUCCEEDED").await;
}
fn empty_source(h: &mut Harness, n: u8) {
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
            RequestId::from_bytes([n; 16]),
            |_| Ok(()),
        )
        .unwrap()
        .digest()
        .unwrap();
}
#[test]
fn exact_pin_overrides_strict_selector_and_reads_old_bytes_after_new_head_and_tombstone() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 10, 0).await;
        let original = h.digest;
        empty_source(&mut h, 249);
        publish(&mut h, 11, 10, 1).await;
        assert_ne!(original, h.digest);
        sqlx::query("UPDATE data_branches SET deleted_at_us=50 WHERE id=?")
            .bind(branch().to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let read = store
            .repository()
            .unwrap()
            .resolve_pin(
                binding("orders", 10),
                VersionId::from_bytes([10; 16]),
                &BTreeSet::new(),
                request(40),
            )
            .await
            .unwrap();
        assert!(read.is_exact_pin());
        assert!(read.missing_candidates().is_empty());
        assert_eq!(read.provenance().resolution["kind"], "exact_pin");
        assert_eq!(read.provenance().resolution["selector_override"], true);
        assert!(read.provenance().resolution["fallback_index"].is_null());
        assert_eq!(read.provenance().starting_branch.as_str(), "absent");
        assert_eq!(read.lease().artifact(), original);
        store.close().await.unwrap();
        let c = tf_exec::input_read::verify(h.owner.take().unwrap(), read)
            .await
            .unwrap();
        h.owner = Some(c.owner);
        assert_eq!(c.result.unwrap().digest(), original);
        assert_eq!(c.read.version(), VersionId::from_bytes([10; 16]));
        assert_eq!(h.scalar("SELECT count(*) FROM read_leases").await, 1);
    });
}
#[test]
fn wrong_dataset_missing_version_write_conflict_and_collection_never_fall_back() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 10, 0).await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        for (b, v) in [(binding("wrong", 11), 10), (binding("missing", 10), 99)] {
            assert!(
                repo.resolve_pin(
                    b,
                    VersionId::from_bytes([v; 16]),
                    &BTreeSet::new(),
                    request(v)
                )
                .await
                .is_err()
            );
        }
        let mut b = binding("conflict", 10);
        b.policy = BranchPolicySnapshot::new(vec![], BTreeMap::new())
            .unwrap()
            .local(
                &"master".parse().unwrap(),
                BranchSelector::Current,
                FallbackPermission::Allowed,
                None,
            )
            .unwrap();
        assert!(matches!(
            repo.resolve_pin(
                b,
                VersionId::from_bytes([10; 16]),
                &BTreeSet::from([key(10)]),
                request(45)
            )
            .await,
            Err(ResolutionError::PlannedProducer)
        ));
        store.close().await.unwrap();
        sqlx::query("INSERT INTO artifact_gc_claims VALUES(?, ?, 100)")
            .bind(h.digest.hex())
            .bind(RequestId::from_bytes([90; 16]).to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .resolve_pin(
                    binding("collecting", 10),
                    VersionId::from_bytes([10; 16]),
                    &BTreeSet::new(),
                    request(46)
                )
                .await,
            Err(ResolutionError::Retention(_))
        ));
        store.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM read_leases").await, 0);
    });
}
async fn retain(h: &mut Harness, two: bool) -> tf_store::replay::ReplayManifest {
    publish(h, 10, 10, 0).await;
    if two {
        empty_source(h, 249);
        publish(h, 11, 11, 0).await;
    }
    h.seed(20, 20).await;
    let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
    let repo = store.repository().unwrap();
    let mut boundaries = vec![];
    for n in if two { vec![10, 11] } else { vec![10] } {
        let read = repo
            .resolve_pin(
                binding(&format!("alias_{n}"), n),
                VersionId::from_bytes([n; 16]),
                &BTreeSet::new(),
                request(n + 30),
            )
            .await
            .unwrap();
        boundaries.push(ReplayBoundary {
            consumer: key(20),
            input: read.provenance(),
        });
        repo.release_read(read.lease()).await.unwrap();
    }
    let mut wrong = boundaries.clone();
    wrong[0].input.dataset = key(11);
    assert!(
        repo.retain_replay(BuildId::from_bytes([20; 16]), json!({}), &[key(20)], &wrong)
            .await
            .is_err()
    );
    let m = repo
        .retain_replay(
            BuildId::from_bytes([20; 16]),
            json!({"threshold":{"type":"int64","value":"9007199254740993"}}),
            &[key(20)],
            &boundaries,
        )
        .await
        .unwrap();
    assert_eq!(
        repo.retain_replay(
            BuildId::from_bytes([20; 16]),
            m.parameters().clone(),
            &[key(20)],
            &boundaries
        )
        .await
        .unwrap()
        .value()
        .unwrap(),
        m.value().unwrap()
    );
    assert!(
        repo.retain_replay(
            BuildId::from_bytes([20; 16]),
            json!({}),
            &[key(20)],
            &boundaries
        )
        .await
        .is_err()
    );
    store.close().await.unwrap();
    m
}
#[test]
fn replay_retains_original_context_and_verifies_original_bytes_on_explicit_destination() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let m = retain(&mut h, false).await;
        let original = h.digest;
        empty_source(&mut h, 249);
        publish(&mut h, 12, 10, 1).await;
        assert!(
            sqlx::query("UPDATE replay_manifests SET manifest_json='{}'")
                .execute(&mut h.db)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM replay_manifests")
                .execute(&mut h.db)
                .await
                .is_err()
        );
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let read = store
            .repository()
            .unwrap()
            .lease_replay(
                BuildId::from_bytes([20; 16]),
                "recovery".parse().unwrap(),
                RequestId::from_bytes([80; 16]),
                &[RequestId::from_bytes([81; 16])],
                100,
                100,
            )
            .await
            .unwrap();
        assert_eq!(read.destination.as_str(), "recovery");
        assert_eq!(read.manifest.source().unwrap(), source());
        assert_eq!(read.manifest.scope().unwrap(), vec![key(20)]);
        assert_eq!(read.manifest.value().unwrap(), m.value().unwrap());
        assert_eq!(read.leases[0].artifact(), original);
        store.close().await.unwrap();
        let c = tf_exec::input_read::verify_replay(h.owner.take().unwrap(), read)
            .await
            .unwrap();
        h.owner = Some(c.owner);
        assert_eq!(c.result.unwrap()[0].digest(), original);
        assert_eq!(h.scalar("SELECT count(*) FROM data_branches").await, 1);
    });
}
#[test]
fn replay_collection_failure_rolls_back_every_new_lease_and_missing_manifest_fails() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        retain(&mut h, true).await;
        sqlx::query("INSERT INTO artifact_gc_claims VALUES(?, ?, 100)")
            .bind(h.digest.hex())
            .bind(RequestId::from_bytes([90; 16]).to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        let result = repo
            .lease_replay(
                BuildId::from_bytes([20; 16]),
                "recovery".parse().unwrap(),
                RequestId::from_bytes([80; 16]),
                &[
                    RequestId::from_bytes([81; 16]),
                    RequestId::from_bytes([82; 16]),
                ],
                100,
                100,
            )
            .await;
        assert!(matches!(result, Err(tf_store::replay::Error::Retention(_))));
        assert!(
            repo.lease_replay(
                BuildId::from_bytes([99; 16]),
                "recovery".parse().unwrap(),
                RequestId::from_bytes([80; 16]),
                &[],
                100,
                100
            )
            .await
            .is_err()
        );
        store.close().await.unwrap();
        assert_eq!(
            h.scalar("SELECT count(*) FROM read_leases WHERE released=0")
                .await,
            0
        );
        assert_eq!(h.scalar("SELECT count(*) FROM read_leases").await, 2);
    });
}
#[test]
fn corrupt_retained_replay_bytes_fail_even_with_a_healthy_current_head() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        retain(&mut h, false).await;
        let original = h.digest;
        empty_source(&mut h, 249);
        publish(&mut h, 12, 10, 1).await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let read = store
            .repository()
            .unwrap()
            .lease_replay(
                BuildId::from_bytes([20; 16]),
                "recovery".parse().unwrap(),
                RequestId::from_bytes([80; 16]),
                &[RequestId::from_bytes([81; 16])],
                100,
                100,
            )
            .await
            .unwrap();
        store.close().await.unwrap();
        let object = h
            .root
            .join(".transflow/runtime/objects")
            .join(&original.hex()[..2])
            .join(original.hex());
        writable(&object);
        std::fs::remove_file(object.join("manifest.json")).unwrap();
        let c = tf_exec::input_read::verify_replay(h.owner.take().unwrap(), read)
            .await
            .unwrap();
        h.owner = Some(c.owner);
        assert!(matches!(
            c.result,
            Err(tf_exec::input_read::Error::Artifact(_))
        ));
        assert_eq!(c.read.leases[0].artifact(), original);
        tf_store::artifacts::ArtifactStore::open(&h.root)
            .unwrap()
            .verify(h.digest)
            .unwrap();
    });
}
