//! Real Parquet removal, durable interrupted intents and leased/pinned root protection.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic GC assertions"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::json;
use std::{
    fs::{self, File},
    path::Path,
};
use tf_domain::*;
use tf_store::{
    artifacts::ArtifactStore,
    gc::{GRACE_US, LOGS_US},
    gc_files::{Boundary, Directory},
    retention::*,
};
fn id(n: u8) -> RequestId {
    RequestId::from_bytes([n; 16])
}
fn policy() -> RetentionPolicy {
    RetentionPolicy {
        latest: 1,
        age_us: 0,
    }
}
fn orphan(h: &Harness) -> tf_store::artifacts::VerifiedArtifact {
    ArtifactStore::open(&h.root).unwrap().prepare(&File::open(h.root.join("source")).unwrap(),&["one".into()],json!({"engine":"import","version":"gc-fixture","compression":"uncompressed","row_group_size":"8192"}),id(200),|_|Ok(())).unwrap().install(|_|Ok(())).unwrap()
}
#[test]
fn orphan_grace_reader_backup_and_crash_resume_are_real_physical_fences() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let object = orphan(&h);
        let digest = object.digest();
        let key = digest.hex();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store = owned.repository().unwrap();
        store.observe_orphan(&object, 100).await.unwrap();
        assert!(matches!(
            store
                .begin_gc("artifact", &key, id(100), 100 + GRACE_US - 1, policy())
                .await,
            Err(RetentionError::Rooted)
        ));
        let now = 100 + GRACE_US;
        store
            .pin_retained(
                id(110),
                id(111),
                PinKind::Backup,
                PinTarget::Artifact(digest),
                now,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .begin_gc("artifact", &key, id(100), now, policy())
                .await,
            Err(RetentionError::Rooted)
        ));
        store.unpin_retained(id(110), id(111)).await.unwrap();
        let lease = store
            .acquire_read(
                id(112),
                id(111),
                LeaseKind::Query,
                ReadTarget::Artifact(digest),
                now,
                100,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .begin_gc("artifact", &key, id(100), now, policy())
                .await,
            Err(RetentionError::Rooted)
        ));
        store.release_read(&lease).await.unwrap();
        let root = h.digest.hex();
        assert!(matches!(
            store
                .begin_gc("artifact", &root, id(101), now, policy())
                .await,
            Err(RetentionError::Rooted)
        ));
        let dir = Directory::open(&h.root, &Path::new("objects").join(&key[..2])).unwrap();
        let (identity, bytes) = dir.inspect(&key).unwrap();
        assert!(bytes > 0);
        store
            .begin_gc("artifact", &key, id(100), now, policy())
            .await
            .unwrap();
        assert!(
            store
                .acquire_read(
                    id(113),
                    id(111),
                    LeaseKind::Query,
                    ReadTarget::Artifact(digest),
                    now,
                    100
                )
                .await
                .is_err()
        );
        assert!(
            dir.remove(&key, id(100), Some(identity), |boundary| {
                if boundary == Boundary::Renamed {
                    Err(std::io::Error::other("crash after durable rename"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert!(dir.retired(id(100)).unwrap());
        assert!(
            ArtifactStore::open(&h.root)
                .unwrap()
                .verify(digest)
                .is_err()
        );
        owned.close().await.unwrap();
        h.owner.take();
        let mut owner = tf_exec::ownership::RuntimeOwner::acquire(
            &h.root,
            workspace(),
            tf_exec::ownership::CoordinatorMode::MetadataOnly,
        )
        .unwrap();
        let mut owned = owner.open_store().await.unwrap();
        let store = owned.repository().unwrap();
        let snapshot = store.gc_inventory(now, policy()).await.unwrap();
        assert_eq!(
            snapshot.items[&("artifact".into(), key.clone())]["state"],
            "CLAIMED"
        );
        dir.remove(&key, id(100), None, |_| Ok(())).unwrap();
        store
            .finish_gc("artifact", &key, id(100), now)
            .await
            .unwrap();
        assert!(!dir.retired(id(100)).unwrap());
        assert!(
            !h.root
                .join(".transflow/runtime/objects")
                .join(&key[..2])
                .join(&key)
                .exists()
        );
        assert!(
            store
                .acquire_read(
                    id(114),
                    id(111),
                    LeaseKind::Query,
                    ReadTarget::Artifact(digest),
                    now,
                    100
                )
                .await
                .is_err()
        );
        assert!(
            ArtifactStore::open(&h.root)
                .unwrap()
                .verify(h.digest)
                .is_ok()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM gc_deletions")
            .fetch_one(&mut h.db)
            .await
            .unwrap();
        assert_eq!(count, 1);
        owned.close().await.unwrap();
    });
}
#[test]
fn read_only_inventory_preserves_clock_and_refuses_unknown_roots() {
    runtime().block_on(async{
        let mut h=Harness::new().await;
        let before:i64=sqlx::query_scalar("SELECT now_us FROM retention_clock").fetch_one(&mut h.db).await.unwrap();
        let mut reader=tf_store::Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite")).await.unwrap();
        let result=reader.gc_inventory(10_000,policy()).await.unwrap();assert!(result.roots.artifacts.is_empty());reader.close().await.unwrap();
        assert_eq!(h.scalar("SELECT now_us FROM retention_clock").await,before);
        assert_eq!(h.scalar("SELECT count(*) FROM gc_items").await,0);
        // Existing unsupported export-root state must not silently authorize deletion.
        sqlx::query("INSERT INTO events(id,type,payload_json,wall_time_us) VALUES(?,'fixture','{}',1)").bind(id(120).to_string()).execute(&mut h.db).await.unwrap();
        sqlx::query("INSERT INTO outbox_deliveries(event_id,target,payload_json,schema_version,attempts,next_attempt_at_us) VALUES(?,'local-fixture','{}',1,0,0)").bind(id(120).to_string()).execute(&mut h.db).await.unwrap();
        let mut reader=tf_store::Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite")).await.unwrap();
        assert!(reader.gc_inventory(10_000,policy()).await.is_err());reader.close().await.unwrap();
    });
}
#[test]
fn physical_deletion_refuses_symlinks_substitution_and_resumes_empty_tree() {
    runtime().block_on(async {
        let h = Harness::new().await;
        let parent = h.root.join(".transflow/runtime/attempts");
        fs::create_dir(&parent).unwrap();
        let key = id(130).to_string();
        let tree = parent.join(&key);
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("stdout.log"), b"synthetic").unwrap();
        let outside = h.root.join("outside");
        fs::write(&outside, b"preserve").unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("escape")).unwrap();
        let dir = Directory::open(&h.root, Path::new("attempts")).unwrap();
        assert!(dir.inspect(&key).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"preserve");
        fs::remove_file(tree.join("escape")).unwrap();
        let (identity, _) = dir.inspect(&key).unwrap();
        fs::rename(&tree, parent.join("replaced")).unwrap();
        fs::create_dir(&tree).unwrap();
        assert!(
            dir.remove(&key, id(131), Some(identity), |_| Ok(()))
                .is_err()
        );
        let (identity, _) = dir.inspect(&key).unwrap();
        assert!(
            dir.remove(&key, id(131), Some(identity), |b| {
                if b == Boundary::FilesRemoved {
                    Err(std::io::Error::other("crash before unlink"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert!(dir.retired(id(131)).unwrap());
        dir.remove(&key, id(131), None, |_| Ok(())).unwrap();
        assert_eq!(fs::read(&outside).unwrap(), b"preserve");
    });
}
#[test]
fn source_backup_roots_and_completed_log_age_are_independent() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        h.seed(20, 20).await;
        sqlx::query("UPDATE attempts SET state='SUCCEEDED',finished_at_us=50")
            .execute(&mut h.db)
            .await
            .unwrap();
        sqlx::query("UPDATE builds SET state='SUCCEEDED',finished_at_us=50")
            .execute(&mut h.db)
            .await
            .unwrap();
        let source = source().to_string();
        let now = GRACE_US + 100;
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store = owned.repository().unwrap();
        store.observe_source(&source, 100).await.unwrap();
        store
            .pin_retained(
                id(140),
                id(141),
                PinKind::Backup,
                PinTarget::Source(source.parse().unwrap()),
                now,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .begin_gc("source", &source, id(142), now, policy())
                .await,
            Err(RetentionError::Rooted)
        ));
        store.unpin_retained(id(140), id(141)).await.unwrap();
        store
            .begin_gc("source", &source, id(142), now, policy())
            .await
            .unwrap();
        assert!(
            store
                .pin_retained(
                    id(143),
                    id(141),
                    PinKind::History,
                    PinTarget::Source(source.parse().unwrap()),
                    now,
                    None
                )
                .await
                .is_err()
        );
        store
            .finish_gc("source", &source, id(142), now)
            .await
            .unwrap();
        owned.close().await.unwrap();
        // Log retention derives from immutable terminal metadata, never directory mtime.
        let mut reader =
            tf_store::Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite"))
                .await
                .unwrap();
        assert!(
            reader
                .gc_inventory(LOGS_US + 49, policy())
                .await
                .unwrap()
                .logs
                .is_empty()
        );
        assert_eq!(
            reader
                .gc_inventory(LOGS_US + 50, policy())
                .await
                .unwrap()
                .logs
                .len(),
            1
        );
        reader.close().await.unwrap();
    });
}
#[test]
fn expired_version_metadata_and_verified_republication_remain_distinct() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let old = h.digest;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/import/empty.parquet"),
            h.root.join("source/one"),
        )
        .unwrap();
        h.digest = ArtifactStore::open(&h.root)
            .unwrap()
            .prepare(
                &File::open(h.root.join("source")).unwrap(),
                &["one".into()],
                writer(),
                id(201),
                |_| Ok(()),
            )
            .unwrap()
            .digest()
            .unwrap();
        let r = h.start(11, 10, 1, contract()).await;
        h.publish(r, 11).await.unwrap();
        h.finish(11, "SUCCEEDED").await;
        let key = old.hex();
        let directory = Directory::open(&h.root, &Path::new("objects").join(&key[..2])).unwrap();
        let (identity, _) = directory.inspect(&key).unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store = owned.repository().unwrap();
        let lease = store
            .acquire_read(
                id(150),
                id(151),
                LeaseKind::Query,
                ReadTarget::Version(VersionId::from_bytes([10; 16])),
                100,
                100,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .begin_gc("artifact", &key, id(152), 100, policy())
                .await,
            Err(RetentionError::Rooted)
        ));
        store.release_read(&lease).await.unwrap();
        store
            .begin_gc("artifact", &key, id(152), 100, policy())
            .await
            .unwrap();
        directory
            .remove(&key, id(152), Some(identity), |_| Ok(()))
            .unwrap();
        store
            .finish_gc("artifact", &key, id(152), 100)
            .await
            .unwrap();
        owned.close().await.unwrap();
        let mut reader =
            tf_store::Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite"))
                .await
                .unwrap();
        let versions = reader
            .api_versions(workspace(), DatasetId::from_bytes([10; 16]), "", 100, false)
            .await
            .unwrap();
        assert_eq!(versions[0]["availability"], "expired");
        assert_eq!(versions[1]["availability"], "not_verified");
        reader.close().await.unwrap();
        assert!(
            ArtifactStore::open(&h.root)
                .unwrap()
                .verify(h.digest)
                .is_ok()
        );
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/import/two-rows.parquet"),
            h.root.join("source/one"),
        )
        .unwrap();
        h.digest = old;
        let r = h.start(12, 10, 2, contract()).await;
        h.publish(r, 12).await.unwrap();
        h.finish(12, "SUCCEEDED").await;
        assert!(ArtifactStore::open(&h.root).unwrap().verify(old).is_ok());
        let mut reader =
            tf_store::Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite"))
                .await
                .unwrap();
        let versions = reader
            .api_versions(workspace(), DatasetId::from_bytes([10; 16]), "", 100, false)
            .await
            .unwrap();
        assert_eq!(versions.len(), 3);
        assert!(versions.iter().all(|v| v["availability"] == "not_verified"));
        reader.close().await.unwrap();
    });
}
#[test]
fn default_twenty_and_thirty_day_retention_is_a_union_with_live_leases() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        for n in 10..31 {
            let r = h.start(n, 10, u64::from(n - 10), contract()).await;
            h.publish(r, n).await.unwrap();
            h.finish(n, "SUCCEEDED").await;
        }
        let window = RetentionPolicy::default().age_us;
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store = owned.repository().unwrap();
        assert_eq!(
            store
                .gc_inventory(window + 19, RetentionPolicy::default())
                .await
                .unwrap()
                .roots
                .versions
                .len(),
            21
        );
        assert_eq!(
            store
                .gc_inventory(window + 21, RetentionPolicy::default())
                .await
                .unwrap()
                .roots
                .versions
                .len(),
            20
        );
        store
            .acquire_read(
                id(160),
                id(161),
                LeaseKind::Query,
                ReadTarget::Version(VersionId::from_bytes([10; 16])),
                window + 21,
                100,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .gc_inventory(window + 21, RetentionPolicy::default())
                .await
                .unwrap()
                .roots
                .versions
                .len(),
            21
        );
        owned.close().await.unwrap();
    });
}
