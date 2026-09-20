//! SIGKILL at real service boundaries, followed by OS-lock reacquisition and SQLite recovery.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic integration assertions"
)]
#[path = "support/publication.rs"]
mod fixture;
#[allow(dead_code, reason = "Shared real process/filesystem harness")]
#[path = "../../../tests/support/mod.rs"]
mod support;
use fixture::*;
use sqlx::Connection;
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::PathBuf,
};
use support::{filesystem::ScratchDirectory, process::TestChild};
use tf_domain::*;
use tf_exec::{
    ownership::{CoordinatorMode, RuntimeOwner},
    publication::{self, Boundary},
};
use tf_store::artifacts::{ArtifactStore, Boundary as ArtifactBoundary};
fn label(b: Boundary) -> &'static str {
    match b {
        Boundary::Artifact(ArtifactBoundary::FileSynced(_)) => "file",
        Boundary::Artifact(ArtifactBoundary::CandidateSynced) => "candidate",
        Boundary::IntentPrepared => "intent",
        Boundary::Artifact(ArtifactBoundary::BeforeInstall) => "before_install",
        Boundary::Artifact(ArtifactBoundary::Installed) => "installed",
        Boundary::Artifact(ArtifactBoundary::Durable) => "durable",
        Boundary::BeforeCommit => "before_commit",
        Boundary::Committed => "committed",
        Boundary::BeforeNotification => "notification",
    }
}
struct Tree(ScratchDirectory);
impl Tree {
    fn new() -> Self {
        Self(ScratchDirectory::new().unwrap())
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        writable(self.0.path());
    }
}
async fn next_candidate(h: &mut Harness) {
    fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/import/empty.parquet"),
        h.root.join("source/one"),
    )
    .unwrap();
    let objects = ArtifactStore::open(&h.root).unwrap();
    h.digest = objects
        .prepare(
            &File::open(h.root.join("source")).unwrap(),
            &["one".into()],
            writer(),
            RequestId::from_bytes([250; 16]),
            |_| Ok(()),
        )
        .unwrap()
        .digest()
        .unwrap();
}
async fn baseline(h: &mut Harness) {
    let r = h.start(10, 10, 0, contract()).await;
    let receipt = h.publish(r, 10).await.unwrap();
    h.finish(10, "SUCCEEDED").await;
    let session = h.session();
    let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
    store
        .repository()
        .unwrap()
        .acknowledge_publication(workspace(), session, receipt.event, 30)
        .await
        .unwrap();
    store.close().await.unwrap();
    next_candidate(h).await;
}
#[test]
fn publication_crash_child() {
    let Some(root) = std::env::var_os("TRANSFLOW_TEST_CHILD_ROOT") else {
        return;
    };
    let boundary = std::env::var("TRANSFLOW_TEST_CHILD_BOUNDARY").unwrap();
    runtime().block_on(async {
        let mut h = Harness::at(PathBuf::from(root)).await;
        baseline(&mut h).await;
        let r = h.start(11, 10, 1, contract()).await;
        let files = h.files(11);
        let completion = publication::publish(h.owner.take().unwrap(), r, files, move |b| {
            if label(b) == boundary {
                println!("TRANSFLOW_FIXTURE_READY");
                io::stdout().flush()?;
                let mut command = [0];
                io::stdin().read_exact(&mut command)?;
            }
            Ok(())
        })
        .await
        .unwrap();
        h.owner = Some(completion.owner);
        completion.result.unwrap();
    });
}
#[test]
fn sigkill_at_every_durability_boundary_preserves_valid_heads_and_replay_identity() {
    for boundary in [
        "file",
        "candidate",
        "intent",
        "before_install",
        "installed",
        "durable",
        "before_commit",
        "committed",
        "notification",
    ] {
        let tree = Tree::new();
        let root = tree.0.path().canonicalize().unwrap();
        let mut child =
            TestChild::spawn_named(&root, "publication_crash_child", Some(boundary)).unwrap();
        child.wait_ready().unwrap();
        assert!(RuntimeOwner::acquire(&root, workspace(), CoordinatorMode::Temporary).is_err());
        let committed = matches!(boundary, "committed" | "notification");
        runtime().block_on(async {
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(root.join(".transflow/runtime/catalog.sqlite"))
                .read_only(true);
            let mut db = sqlx::SqliteConnection::connect_with(&options)
                .await
                .unwrap();
            let generation: i64 = sqlx::query_scalar("SELECT generation FROM dataset_heads")
                .fetch_one(&mut db)
                .await
                .unwrap();
            assert_eq!(
                generation,
                if committed { 2 } else { 1 },
                "reader at {boundary}"
            );
            db.close().await.unwrap();
        });
        assert!(!child.kill_and_wait().unwrap().success());
        fs::remove_dir_all(root.join("source")).unwrap(); // Replay must not rematerialize.
        runtime().block_on(async{
 let mut owner=RuntimeOwner::acquire(&root,workspace(),CoordinatorMode::Temporary).unwrap();publication::recover(&mut owner,50).await.unwrap();
 let options=sqlx::sqlite::SqliteConnectOptions::new().filename(root.join(".transflow/runtime/catalog.sqlite")).foreign_keys(true);let mut db=sqlx::SqliteConnection::connect_with(&options).await.unwrap();
 let versions:i64=sqlx::query_scalar("SELECT count(*) FROM dataset_versions").fetch_one(&mut db).await.unwrap();assert_eq!(versions,if committed{2}else{1},"{boundary}");
 let generation:i64=sqlx::query_scalar("SELECT generation FROM dataset_heads").fetch_one(&mut db).await.unwrap();assert_eq!(generation,versions);
 let state:String=sqlx::query_scalar("SELECT state FROM attempts WHERE id=?").bind(AttemptId::from_bytes([11;16]).to_string()).fetch_one(&mut db).await.unwrap();assert_eq!(state,if committed{"SUCCEEDED"}else{"INTERRUPTED"});
 let reservations:i64=sqlx::query_scalar("SELECT count(*) FROM write_reservations").fetch_one(&mut db).await.unwrap();assert_eq!(reservations,0);
 let head_digest:String=sqlx::query_scalar("SELECT v.artifact_digest FROM dataset_heads h JOIN dataset_versions v ON v.id=h.version_id").fetch_one(&mut db).await.unwrap();let objects=ArtifactStore::open(&root).unwrap();let object=objects.verify(tf_protocol::canonical::ContentDigest::from_hex(tf_protocol::canonical::DigestKind::Artifact,&head_digest).unwrap()).unwrap();assert_eq!(object.manifest()["files"][0]["row_count"],if committed{"0"}else{"2"});
 let all:Vec<String>=sqlx::query_scalar("SELECT artifact_digest FROM dataset_versions").fetch_all(&mut db).await.unwrap();for hash in all{objects.verify(tf_protocol::canonical::ContentDigest::from_hex(tf_protocol::canonical::DigestKind::Artifact,&hash).unwrap()).unwrap();}
 let orphans:i64=sqlx::query_scalar("SELECT count(*) FROM artifacts a WHERE NOT EXISTS(SELECT 1 FROM dataset_versions v WHERE v.artifact_digest=a.digest)").fetch_one(&mut db).await.unwrap();assert_eq!(orphans,if !committed&&!matches!(boundary,"file"|"candidate"){1}else{0});
 let stages:Vec<_>=fs::read_dir(root.join(".transflow/runtime/objects")).unwrap().filter_map(|e|{let e=e.unwrap();e.file_name().to_str().filter(|s|s.starts_with(".staging-")).map(str::to_owned)}).collect();
 assert_eq!(stages.len(),usize::from(matches!(boundary,"file"|"candidate"|"intent"|"before_install")));
 let session=owner.registration().session().unwrap();let mut store=owner.open_store().await.unwrap();let first=store.repository().unwrap().pending_publications(100).await.unwrap();let second=store.repository().unwrap().pending_publications(100).await.unwrap();assert_eq!(first,second);assert_eq!(first.len(),usize::from(committed));
 if committed {assert_eq!(first[0].0,RequestId::from_bytes([11;16]));store.repository().unwrap().acknowledge_publication(workspace(),session,first[0].0,60).await.unwrap();assert!(store.repository().unwrap().pending_publications(100).await.unwrap().is_empty());}
 store.close().await.unwrap();let events:i64=sqlx::query_scalar("SELECT count(*) FROM events").fetch_one(&mut db).await.unwrap();assert_eq!(events,versions);assert!(!root.join("source").exists());let after:i64=sqlx::query_scalar("SELECT count(*) FROM dataset_versions").fetch_one(&mut db).await.unwrap();assert_eq!(after,versions);let integrity:String=sqlx::query_scalar("PRAGMA integrity_check").fetch_one(&mut db).await.unwrap();assert_eq!(integrity,"ok");assert!(sqlx::query("PRAGMA foreign_key_check").fetch_all(&mut db).await.unwrap().is_empty());db.close().await.unwrap();
 });
    }
}
#[test]
fn injected_disk_full_and_permission_failures_preserve_last_good_without_repair() {
    runtime().block_on(async {
        for errno in [28, 13] {
            for boundary in [
                "file",
                "candidate",
                "intent",
                "before_install",
                "installed",
                "durable",
                "before_commit",
                "committed",
                "notification",
            ] {
                let mut h = Harness::new().await;
                baseline(&mut h).await;
                let r = h.start(11, 10, 1, contract()).await;
                let files = h.files(11);
                let completion =
                    publication::publish(h.owner.take().unwrap(), r, files, move |b| {
                        if label(b) == boundary {
                            Err(io::Error::from_raw_os_error(errno))
                        } else {
                            Ok(())
                        }
                    })
                    .await
                    .unwrap();
                h.owner = Some(completion.owner);
                assert!(completion.result.is_err());
                let expected = if matches!(boundary, "committed" | "notification") {
                    2
                } else {
                    1
                };
                assert_eq!(
                    h.scalar("SELECT count(*) FROM dataset_versions").await,
                    expected
                );
                assert_eq!(
                    h.scalar("SELECT generation FROM dataset_heads").await,
                    expected
                );
                h.owner.take();
                let mut owner =
                    RuntimeOwner::acquire(&h.root, workspace(), CoordinatorMode::Temporary)
                        .unwrap();
                publication::recover(&mut owner, 50).await.unwrap();
                h.owner = Some(owner);
                assert_eq!(
                    h.scalar("SELECT count(*) FROM dataset_versions").await,
                    expected
                );
            }
        }
    });
}
#[test]
fn corruption_after_durable_install_cannot_become_a_head() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        baseline(&mut h).await;
        let request = h.start(11, 10, 1, contract()).await;
        let files = h.files(11);
        let digest = h.digest.hex();
        let object = h
            .root
            .join(".transflow/runtime/objects")
            .join(&digest[..2])
            .join(&digest);
        let completion =
            publication::publish(h.owner.take().unwrap(), request, files, move |point| {
                if point == Boundary::BeforeCommit {
                    writable(&object);
                    fs::write(
                        object.join("part-00000.parquet"),
                        b"corrupted after installation",
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();
        h.owner = Some(completion.owner);
        assert!(completion.result.is_err());
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 1);
        assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await, 1);
        assert!(
            ArtifactStore::open(&h.root)
                .unwrap()
                .verify(h.digest)
                .is_err()
        );
    });
}
#[test]
fn dropped_async_caller_keeps_real_ownership_until_blocking_work_finishes() {
    let rt = runtime();
    rt.block_on(async {
        let mut h = Harness::new().await;
        let request = h.start(10, 10, 0, contract()).await;
        let files = h.files(10);
        let root = h.root.clone();
        let (ready, seen) = std::sync::mpsc::sync_channel(1);
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let (finished, done) = std::sync::mpsc::sync_channel(1);
        let task = tokio::spawn(publication::publish(
            h.owner.take().unwrap(),
            request,
            files,
            move |point| {
                if point == Boundary::BeforeCommit {
                    ready.send(()).map_err(io::Error::other)?;
                    wait.recv().map_err(io::Error::other)?;
                }
                if point == Boundary::BeforeNotification {
                    finished.send(()).map_err(io::Error::other)?;
                }
                Ok(())
            },
        ));
        tokio::task::spawn_blocking(move || {
            seen.recv_timeout(std::time::Duration::from_secs(20))
                .unwrap()
        })
        .await
        .unwrap();
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert!(RuntimeOwner::acquire(&root, workspace(), CoordinatorMode::Temporary).is_err());
        release.send(()).unwrap();
        tokio::task::spawn_blocking(move || {
            done.recv_timeout(std::time::Duration::from_secs(20))
                .unwrap()
        })
        .await
        .unwrap();
        // The notification signal precedes final DB close. A blocking-pool barrier alone is
        // not proof of owner drop; wait on the observable OS lock with a bounded deadline.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match RuntimeOwner::acquire(&root, workspace(), CoordinatorMode::Temporary) {
                Ok(mut owner) => {
                    publication::recover(&mut owner, 50).await.unwrap();
                    h.owner = Some(owner);
                    break;
                }
                Err(_) => {
                    assert!(std::time::Instant::now() < deadline);
                    tokio::task::yield_now().await;
                }
            }
        }
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 1);
    });
}

#[test]
fn later_job_failure_in_the_same_build_cannot_rollback_an_earlier_publication() {
    use tf_domain::execution::{
        EventTime, ExecutionBinding, Fence, Job, OutputTarget, Phase, RetryPolicy,
    };
    use tf_store::publication::PublicationRequest;
    runtime().block_on(async {
        let mut h = Harness::new().await;
        h.seed(10, 10).await;
        h.seed(11, 11).await;
        let build = BuildId::from_bytes([10; 16]);
        sqlx::query("UPDATE jobs SET build_id=? WHERE id=?")
            .bind(build.to_string())
            .bind(JobId::from_bytes([11; 16]).to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let fence = Fence {
            session: h.session(),
            generation: 1,
        };
        let targets = [10, 11].map(|n| OutputTarget {
            dataset: DatasetKey::new(workspace(), DatasetId::from_bytes([n; 16])),
            branch: branch(),
            expected_generation: 0,
        });
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        store
            .repository()
            .unwrap()
            .reserve_publications(workspace(), build, fence, &targets, 2)
            .await
            .unwrap();
        for n in [10, 11] {
            store
                .repository()
                .unwrap()
                .freeze_publication_contract(
                    workspace(),
                    fence.session,
                    JobId::from_bytes([n; 16]),
                    &contract(),
                )
                .await
                .unwrap();
        }
        store.close().await.unwrap();
        sqlx::query("UPDATE jobs SET state='VALIDATING_OUTPUTS'")
            .execute(&mut h.db)
            .await
            .unwrap();
        sqlx::query("UPDATE attempts SET state='VALIDATING_OUTPUTS'")
            .execute(&mut h.db)
            .await
            .unwrap();
        for (n, target) in [10u8, 11].into_iter().zip(targets) {
            let attempt = AttemptId::from_bytes([n; 16]);
            let mut job = Job::new(
                JobId::from_bytes([n; 16]),
                build,
                ExecutionBinding {
                    plan: PlanId::from_bytes([10; 16]),
                    source: source(),
                },
                target,
                fence,
                RetryPolicy::default(),
                EventTime(0),
            );
            job.queue(fence, EventTime(1)).unwrap();
            job.start_attempt(attempt, fence, EventTime(2)).unwrap();
            for (index, phase) in [
                Phase::ValidatingInputs,
                Phase::Running,
                Phase::Materializing,
                Phase::ValidatingOutputs,
            ]
            .into_iter()
            .enumerate()
            {
                job.advance(attempt, fence, phase, EventTime(index as u64 + 3))
                    .unwrap();
            }
            let intent = job
                .prepare_publication(
                    attempt,
                    fence,
                    VersionId::from_bytes([n; 16]),
                    h.digest.hex().parse().unwrap(),
                    EventTime(7),
                )
                .unwrap();
            let request = PublicationRequest {
                intent,
                contract: contract(),
                at_us: 20,
            };
            let files = h.files(n);
            let completion =
                publication::publish(h.owner.take().unwrap(), request, files, move |point| {
                    if n == 11 && point == Boundary::BeforeCommit {
                        Err(io::Error::from_raw_os_error(28))
                    } else {
                        Ok(())
                    }
                })
                .await
                .unwrap();
            h.owner = Some(completion.owner);
            assert_eq!(completion.result.is_ok(), n == 10);
        }
        sqlx::query(
            "UPDATE attempts SET state='FAILED',finished_at_us=30 WHERE state='COMMITTING'",
        )
        .execute(&mut h.db)
        .await
        .unwrap();
        sqlx::query("UPDATE jobs SET state='FAILED' WHERE state='COMMITTING'")
            .execute(&mut h.db)
            .await
            .unwrap();
        h.finish(10, "FAILED").await;
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_heads").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 1);
        assert_eq!(
            h.scalar("SELECT count(*) FROM attempts WHERE state='SUCCEEDED'")
                .await,
            1
        );
        assert_eq!(
            h.scalar("SELECT count(*) FROM jobs WHERE state='FAILED'")
                .await,
            1
        );
        assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await, 0);
    });
}
