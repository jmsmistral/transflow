//! Real SQLite/Parquet export leases, crash boundaries and replica visibility.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic foreign publication fixtures"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::PermissionsExt,
};
use tf_domain::{input::*, *};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
use tf_store::{
    artifacts::{ArtifactStore, Boundary},
    input_resolution::ReadRequest,
    retention::*,
};
fn id(n: u8) -> RequestId {
    RequestId::from_bytes([n; 16])
}
fn request(n: u8, kind: LeaseKind) -> ReadRequest {
    ReadRequest {
        lease: id(n),
        operation: id(90),
        kind,
        now_us: 100,
        ttl_us: 100,
    }
}
fn binding() -> InputBinding {
    InputBinding {
        key: InputBindingKey::new(
            DatasetKey::new(
                WorkspaceId::from_bytes([22; 16]),
                DatasetId::from_bytes([23; 16]),
            ),
            "rows".into(),
        )
        .unwrap(),
        dataset: DatasetKey::new(workspace(), DatasetId::from_bytes([10; 16])),
        role: InputRole::Data,
        policy: BranchPolicySnapshot::new(vec![], BTreeMap::new())
            .unwrap()
            .external(
                &"feature".parse().unwrap(),
                &"master".parse().unwrap(),
                BranchSelector::Omitted,
                FallbackPermission::Allowed,
                None,
            )
            .unwrap(),
    }
}
#[test]
fn provider_resolution_is_owner_qualified_and_fenced_across_metadata_restarts() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let s = owned.repository().unwrap();
        assert!(
            s.resolve_input(binding(), &BTreeSet::new(), request(40, LeaseKind::Plan))
                .await
                .is_err()
        );
        assert!(
            s.resolve_export(
                WorkspaceId::from_bytes([99; 16]),
                binding(),
                None,
                request(41, LeaseKind::Copy)
            )
            .await
            .is_err()
        );
        assert!(
            s.resolve_export(
                workspace(),
                binding(),
                Some(VersionId::from_bytes([99; 16])),
                request(41, LeaseKind::Copy)
            )
            .await
            .is_err()
        );
        let read = s
            .resolve_export(workspace(), binding(), None, request(42, LeaseKind::Copy))
            .await
            .unwrap();
        assert_eq!(
            read.binding().key.consumer().workspace_id(),
            WorkspaceId::from_bytes([22; 16])
        );
        assert_eq!(read.resolved_branch().as_str(), "master");
        let ticket = s.copy_lease(id(42), id(90), 0).await.unwrap();
        assert!(s.copy_lease(id(42), id(91), 0).await.is_err());
        let renewed = s.renew_read(&ticket, 110, 100).await.unwrap();
        assert!(s.release_read(&ticket).await.is_err());
        assert!(s.copy_lease(id(42), id(90), 0).await.is_err());
        assert!(
            s.retention_roots(111, RetentionPolicy::default())
                .await
                .unwrap()
                .artifacts
                .contains(&h.digest.hex())
        );
        owned.close().await.unwrap();
        h.owner.take();
        let mut owner =
            RuntimeOwner::acquire(&h.root, workspace(), CoordinatorMode::MetadataOnly).unwrap();
        let mut owned = owner.open_store().await.unwrap();
        let s = owned.repository().unwrap();
        let recovered = s.copy_lease(id(42), id(90), 1).await.unwrap();
        assert_eq!(recovered.expires_at_us(), renewed.expires_at_us());
        assert!(s.renew_read(&recovered, 210, 100).await.is_err());
        s.release_read(&recovered).await.unwrap();
        owned.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM attempts").await, 1);
    });
}
#[test]
fn every_copy_boundary_is_invisible_until_durable_replica_and_local_pin() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let s = owned.repository().unwrap();
        let read = s
            .resolve_export(workspace(), binding(), None, request(42, LeaseKind::Copy))
            .await
            .unwrap();
        let metadata = s
            .export_metadata(binding().dataset, read.version())
            .await
            .unwrap();
        owned.close().await.unwrap();
        for (index, boundary) in [
            Boundary::FileSynced(0),
            Boundary::CandidateSynced,
            Boundary::BeforeInstall,
            Boundary::Installed,
            Boundary::Durable,
        ]
        .into_iter()
        .enumerate()
        {
            let root = h.root.join(format!("consumer-{index}"));
            std::fs::create_dir_all(root.join(".transflow/runtime")).unwrap();
            std::fs::set_permissions(
                root.join(".transflow/runtime"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            let mut owner = RuntimeOwner::acquire(
                &root,
                WorkspaceId::from_bytes([22; 16]),
                CoordinatorMode::Temporary,
            )
            .unwrap();
            let provider = ArtifactStore::open(&h.root).unwrap();
            let local = ArtifactStore::open(&root).unwrap();
            let mut owned = owner.open_store().await.unwrap();
            let s = owned.repository().unwrap();
            s.register_workspace(WorkspaceId::from_bytes([22; 16]), "consumer", 1)
                .await
                .unwrap();
            s.begin_replica(binding().dataset, read.version(), &metadata)
                .await
                .unwrap();
            assert!(
                local
                    .copy_from_observed(&provider, h.digest, id(50), |point| if point == boundary {
                        Err(std::io::Error::other("injected copy interruption"))
                    } else {
                        Ok(())
                    })
                    .is_err()
            );
            assert!(
                s.replica_metadata(binding().dataset, read.version())
                    .await
                    .unwrap()
                    .is_none()
            );
            // Retry safely verifies an installed orphan or reconstructs a cleaned staging copy.
            let artifact = local.copy_from(&provider, h.digest, id(51)).unwrap();
            let lease = s
                .finish_replica(
                    binding().dataset,
                    read.version(),
                    &artifact,
                    request(52, LeaseKind::Plan),
                )
                .await
                .unwrap();
            assert_eq!(
                s.replica_metadata(binding().dataset, read.version())
                    .await
                    .unwrap(),
                Some(metadata.clone())
            );
            assert!(
                s.claim_collection(id(53), h.digest, 110, RetentionPolicy::default())
                    .await
                    .is_err()
            );
            let mut renamed = metadata.clone();
            renamed["origin_branch"]["name"] = serde_json::json!("renamed");
            s.begin_replica(binding().dataset, read.version(), &renamed)
                .await
                .unwrap();
            assert_eq!(
                s.replica_metadata(binding().dataset, read.version())
                    .await
                    .unwrap(),
                Some(metadata.clone())
            );
            let mut conflict = metadata.clone();
            conflict["source"]["digest"] = serde_json::json!("f".repeat(64));
            assert!(
                s.begin_replica(binding().dataset, read.version(), &conflict)
                    .await
                    .is_err()
            );
            assert_eq!(
                s.replica_metadata(binding().dataset, read.version())
                    .await
                    .unwrap(),
                Some(metadata.clone())
            );
            s.release_read(&lease).await.unwrap();
            let claim = s
                .claim_collection(id(53), h.digest, 111, RetentionPolicy::default())
                .await
                .unwrap();
            assert!(
                s.replica_metadata(binding().dataset, read.version())
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                s.finish_replica(
                    binding().dataset,
                    read.version(),
                    &artifact,
                    ReadRequest {
                        now_us: 111,
                        ..request(54, LeaseKind::Plan)
                    }
                )
                .await
                .is_err()
            );
            s.abandon_collection(claim).await.unwrap();
            owned.close().await.unwrap();
        }
    });
}

#[test]
fn schema_eight_job_bindings_upgrade_without_losing_local_foreign_keys() {
    runtime().block_on(async {
        let mut h=Harness::new().await;
        let r=h.start(10,10,0,contract()).await;
        h.publish(r,10).await.unwrap();h.finish(10,"SUCCEEDED").await;
        sqlx::query("INSERT INTO job_inputs(job_id,alias,version_id,binding_json) SELECT id,'legacy',?,'{}' FROM jobs")
            .bind(VersionId::from_bytes([10;16]).to_string()).execute(&mut h.db).await.unwrap();
        // Reconstruct the previously shipped schema-8 table with its real local FKs.
        sqlx::raw_sql("CREATE TABLE legacy_job_inputs (job_id TEXT NOT NULL REFERENCES jobs(id),alias TEXT NOT NULL,version_id TEXT REFERENCES dataset_versions(id),parent_job_id TEXT REFERENCES jobs(id),binding_json TEXT NOT NULL CHECK(json_valid(binding_json)),CHECK((version_id IS NULL)!=(parent_job_id IS NULL)),PRIMARY KEY(job_id,alias)) STRICT; INSERT INTO legacy_job_inputs SELECT job_id,alias,version_id,parent_job_id,binding_json FROM job_inputs; DROP TABLE job_inputs; ALTER TABLE legacy_job_inputs RENAME TO job_inputs; DELETE FROM schema_migrations WHERE version=9; PRAGMA user_version=8;").execute(&mut h.db).await.unwrap();
        let owned=h.owner.as_mut().unwrap().open_store().await.unwrap();owned.close().await.unwrap();
        assert_eq!(h.scalar("PRAGMA user_version").await,9);
        assert_eq!(h.scalar("SELECT count(*) FROM job_inputs WHERE alias='legacy' AND version_id IS NOT NULL AND foreign_version_id IS NULL").await,1);
        assert!(sqlx::query("UPDATE job_inputs SET version_id=?").bind(VersionId::from_bytes([99;16]).to_string()).execute(&mut h.db).await.is_err());
    });
}

#[test]
fn leased_old_provider_version_cannot_be_collected_until_release() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let old = h.digest;
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let read = owned
            .repository()
            .unwrap()
            .resolve_export(workspace(), binding(), None, request(42, LeaseKind::Copy))
            .await
            .unwrap();
        owned.close().await.unwrap();
        let mut files = h.files(11);
        files.writer["version"] = serde_json::json!("replacement");
        h.digest = ArtifactStore::open(&h.root)
            .unwrap()
            .prepare(
                &files.root,
                &files.files,
                files.writer.clone(),
                id(50),
                |_| Ok(()),
            )
            .unwrap()
            .digest()
            .unwrap();
        assert_ne!(old, h.digest);
        let r = h.start(11, 10, 1, contract()).await;
        let completion =
            tf_exec::publication::publish(h.owner.take().unwrap(), r, files, |_| Ok(()))
                .await
                .unwrap();
        h.owner = Some(completion.owner);
        completion.result.unwrap();
        h.finish(11, "SUCCEEDED").await;
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let s = owned.repository().unwrap();
        let policy = RetentionPolicy {
            latest: 1,
            age_us: 0,
        };
        assert!(matches!(
            s.claim_collection(id(43), old, 110, policy).await,
            Err(RetentionError::Rooted)
        ));
        s.release_read(read.lease()).await.unwrap();
        let claim = s.claim_collection(id(43), old, 111, policy).await.unwrap();
        assert_eq!(claim.artifact(), old);
        assert!(
            s.resolve_export(
                workspace(),
                binding(),
                Some(read.version()),
                ReadRequest {
                    now_us: 111,
                    ..request(44, LeaseKind::Copy)
                }
            )
            .await
            .is_err()
        );
        s.abandon_collection(claim).await.unwrap();
        owned.close().await.unwrap();
    });
}
