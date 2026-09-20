//! Exact lease bindings, virtual-clock expiry and real SQLite collection races.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic retention fixtures"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::json;
use tf_domain::*;
use tf_exec::publication;
use tf_store::publication::{InputProvenance, InputRole};
use tf_store::retention::*;
fn id(n: u8) -> RequestId {
    RequestId::from_bytes([n; 16])
}
fn policy() -> RetentionPolicy {
    RetentionPolicy {
        latest: 1,
        age_us: 0,
    }
}
async fn publish(h: &mut Harness, n: u8, dataset: u8, generation: u64) {
    let r = h.start(n, dataset, generation, contract()).await;
    h.publish(r, n).await.unwrap();
    h.finish(n, "SUCCEEDED").await;
}
#[test]
fn head_read_is_pinned_and_renewals_do_not_follow_new_heads() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 10, 0).await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let lease = store
            .repository()
            .unwrap()
            .acquire_read(
                id(40),
                id(41),
                LeaseKind::Query,
                ReadTarget::Head(branch(), DatasetId::from_bytes([10; 16])),
                100,
                100,
            )
            .await
            .unwrap();
        store.close().await.unwrap();
        publish(&mut h, 11, 10, 1).await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        let renewed = repo.renew_read(&lease, 150, 100).await.unwrap();
        assert_eq!(renewed.version(), Some(VersionId::from_bytes([10; 16])));
        assert_eq!(renewed.artifact(), h.digest);
        assert_eq!(renewed.expires_at_us(), 250);
        assert!(repo.renew_read(&lease, 151, 100).await.is_err());
        assert!(repo.release_read(&lease).await.is_err());
        let mut reader =
            tf_store::Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite"))
                .await
                .unwrap();
        assert_eq!(
            reader.catalog_lifecycle_blockers(151).await.unwrap(),
            ["active read leases: 1"]
        );
        repo.release_read(&renewed).await.unwrap();
        assert!(
            reader
                .catalog_lifecycle_blockers(151)
                .await
                .unwrap()
                .is_empty()
        );
        reader.close().await.unwrap();
        assert!(repo.renew_read(&renewed, 152, 100).await.is_err());
        store.close().await.unwrap();
    });
}
#[test]
fn expiring_provider_copy_lease_survives_coordinator_restart_and_fences_collection() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 10, 0).await;
        // An unreferenced metadata-only object is a valid lease/GC fixture: no filesystem deletion.
        let d = tf_protocol::canonical::ContentDigest::from_hex(
            tf_protocol::canonical::DigestKind::Artifact,
            &"d".repeat(64),
        )
        .unwrap();
        sqlx::query("INSERT INTO artifacts VALUES(?,'{}','{}',1,0,0,'MANIFEST')")
            .bind(d.hex())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let lease = store
            .repository()
            .unwrap()
            .acquire_read(
                id(40),
                id(41),
                LeaseKind::Copy,
                ReadTarget::Artifact(d),
                100,
                20,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .claim_collection(id(42), d, 110, policy())
                .await,
            Err(RetentionError::Rooted)
        ));
        store.close().await.unwrap();
        h.owner.take();
        let mut owner = tf_exec::ownership::RuntimeOwner::acquire(
            &h.root,
            workspace(),
            tf_exec::ownership::CoordinatorMode::MetadataOnly,
        )
        .unwrap();
        let mut store = owner.open_store().await.unwrap();
        let repo = store.repository().unwrap();
        assert!(matches!(
            repo.claim_collection(id(42), d, 119, policy()).await,
            Err(RetentionError::Rooted)
        ));
        let claim = repo
            .claim_collection(id(42), d, 120, policy())
            .await
            .unwrap();
        assert!(repo.renew_read(&lease, 120, 20).await.is_err());
        assert!(
            repo.acquire_read(
                id(43),
                id(41),
                LeaseKind::Copy,
                ReadTarget::Artifact(d),
                120,
                20
            )
            .await
            .is_err()
        );
        assert!(
            repo.pin_retained(
                id(44),
                id(41),
                PinKind::Replica,
                PinTarget::Artifact(d),
                120,
                None
            )
            .await
            .is_err()
        );
        repo.abandon_collection(claim).await.unwrap();
        assert!(
            repo.acquire_read(
                id(43),
                id(41),
                LeaseKind::Copy,
                ReadTarget::Artifact(d),
                121,
                20
            )
            .await
            .is_ok()
        );
        store.close().await.unwrap();
        h.owner = Some(owner);
    });
}
#[test]
fn retained_lineage_explicit_roots_and_plan_expiry_are_independent() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 10, 0).await;
        publish(&mut h, 11, 10, 1).await;
        let mut c = contract();
        c.inputs.push(InputProvenance {
            alias: "old".into(),
            dataset: DatasetKey::new(workspace(), DatasetId::from_bytes([10; 16])),
            version: VersionId::from_bytes([10; 16]),
            artifact: h.digest,
            declared_branch: json!(null),
            starting_branch: "master".parse().unwrap(),
            resolved_branch: "master".parse().unwrap(),
            role: InputRole::Data,
            resolution: json!({}),
        });
        let r = h.start(12, 12, 0, c).await;
        h.publish(r, 12).await.unwrap();
        h.finish(12, "SUCCEEDED").await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        let roots = repo.retention_roots(100, policy()).await.unwrap();
        assert!(roots.versions.contains(&VersionId::from_bytes([10; 16])));
        assert!(roots.sources.contains(&source()));
        repo.pin_retained(
            id(40),
            id(41),
            PinKind::Plan,
            PinTarget::Source(source()),
            100,
            Some(120),
        )
        .await
        .unwrap();
        let lease = repo
            .acquire_read(
                id(42),
                id(41),
                LeaseKind::Plan,
                ReadTarget::Version(VersionId::from_bytes([10; 16])),
                100,
                50,
            )
            .await
            .unwrap();
        repo.unpin_retained(id(40), id(41)).await.unwrap();
        assert!(repo.renew_read(&lease, 130, 50).await.is_ok());
        assert!(repo.retention_roots(129, policy()).await.is_err());
        store.close().await.unwrap();
    });
}
#[test]
fn active_candidate_and_input_contract_are_roots_until_build_finishes() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        let files = h.files(10);
        let completion = publication::publish(h.owner.take().unwrap(), r, files, |point| {
            if point == publication::Boundary::BeforeCommit {
                Err(std::io::Error::from_raw_os_error(28))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap();
        h.owner = Some(completion.owner);
        assert!(completion.result.is_err());
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .claim_collection(id(40), h.digest, 100, policy())
                .await,
            Err(RetentionError::Rooted)
        ));
        store.close().await.unwrap();
        h.finish(10, "FAILED").await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let claim = store
            .repository()
            .unwrap()
            .claim_collection(id(40), h.digest, 101, policy())
            .await
            .unwrap();
        assert_eq!(claim.artifact(), h.digest);
        store
            .repository()
            .unwrap()
            .abandon_collection(claim)
            .await
            .unwrap();
        store.close().await.unwrap();
    });
}
#[test]
fn collection_claim_blocks_publication_even_when_the_bytes_already_exist() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        let files = h.files(10);
        let completion = publication::publish(h.owner.take().unwrap(), r, files, |point| {
            if point == publication::Boundary::BeforeCommit {
                Err(std::io::Error::from_raw_os_error(28))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap();
        h.owner = Some(completion.owner);
        h.finish(10, "FAILED").await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let _claim = store
            .repository()
            .unwrap()
            .claim_collection(id(40), h.digest, 100, policy())
            .await
            .unwrap();
        store.close().await.unwrap();
        let next = h.start(11, 10, 0, contract()).await;
        assert!(h.publish(next, 11).await.is_err());
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 0);
    });
}

#[test]
fn refused_expiry_observation_cannot_resurrect_a_lease_after_clock_rollback() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 10, 0).await;
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        let lease = repo
            .acquire_read(
                id(40),
                id(41),
                LeaseKind::Query,
                ReadTarget::Version(VersionId::from_bytes([10; 16])),
                100,
                80,
            )
            .await
            .unwrap();
        assert!(repo.renew_read(&lease, 200, 50).await.is_err());
        assert!(matches!(
            repo.renew_read(&lease, 150, 50).await,
            Err(RetentionError::ClockOrLimit)
        ));
        store.close().await.unwrap();
    });
}
