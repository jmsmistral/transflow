//! Database winner, session fences and real owned workers on client disconnect.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Synthetic integration fixtures.
#[path = "support/publication.rs"]
mod fixture;
#[path = "support/supervision.rs"]
mod peer;
use fixture::*;
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use tf_domain::{
    execution::{CancelRequest, Fence},
    *,
};
use tf_exec::{
    cancellation::{BuildControl, ClientEvent, Disposition, Error},
    discovery::Scratch,
    ownership::{CoordinatorMode, RuntimeOwner},
    publication,
    supervisor::Failure,
};
use tf_store::{artifacts::ArtifactStore, publication::PublicationError};

async fn ready(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "worker never became ready");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
fn control(h: &Harness, n: u8) -> BuildControl {
    BuildControl::new(h.owner.as_ref().unwrap(), BuildId::from_bytes([n; 16]), 10).unwrap()
}

#[test]
fn client_disconnect_and_ctrl_c_are_build_scoped_and_wait_for_real_cleanup() {
    runtime().block_on(async {
        for mode in [CoordinatorMode::Persistent, CoordinatorMode::Temporary] {
            let mut h = Harness::with_mode(mode).await;
            let request = h.start(10, 10, 0, contract()).await;
            let unrelated = h.start(11, 11, 0, contract()).await;
            let mut target = control(&h, 10);
            let mut other = control(&h, 11);
            let worker = target.register(h.owner.as_mut().unwrap(), request.intent.attempt(), request.intent.fence()).await.unwrap();
            let canary = other.register(h.owner.as_mut().unwrap(), unrelated.intent.attempt(), unrelated.intent.fence()).await.unwrap();
            let root = Scratch::create().unwrap();
            let other_root = Scratch::create().unwrap();
            let mut launch = peer::launch(root.path(), "cancel");
            launch.request["attempt_id"] = request.intent.attempt().to_string().into();
            let mut other_launch = peer::launch(other_root.path(), "budget");
            other_launch.request["attempt_id"] = unrelated.intent.attempt().to_string().into();
            let task = thread::spawn(move || worker.run(launch));
            let other_task = thread::spawn(move || canary.run(other_launch));
            ready(&root.path().join("descendant.pid")).await;
            ready(&other_root.path().join("ready")).await;
            let result = target.client_event(h.owner.as_mut().unwrap(), ClientEvent::Disconnected).await.unwrap();
            if mode == CoordinatorMode::Persistent {
                assert_eq!(result, Disposition::Continuing);
                assert_eq!(h.scalar("SELECT cancel_requested FROM builds WHERE id=(SELECT build_id FROM jobs WHERE dataset_id=(SELECT id FROM datasets ORDER BY id LIMIT 1))").await, 0);
                assert!(!task.is_finished());
                assert_eq!(target.client_event(h.owner.as_mut().unwrap(), ClientEvent::Cancel).await.unwrap(), Disposition::Cancellation(CancelRequest::Requested));
            } else {
                assert_eq!(result, Disposition::Cancellation(CancelRequest::Requested));
            }
            assert_eq!(target.request(h.owner.as_mut().unwrap()).await.unwrap(), CancelRequest::AlreadyRequested);
            assert_eq!(h.scalar("SELECT sum(cancel_requested) FROM builds").await, 1);
            assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await, 2);
            assert!(!other_task.is_finished(), "another build must keep running");
            let report = task.join().unwrap();
            assert_eq!(report.outcome, Err(Failure::Canceled), "{report:?}");
            assert!(report.process_start.is_some());
            assert!(report.logs_complete);
            assert!(peer::stopped(report.pid.unwrap()));
            let descendant = fs::read_to_string(root.path().join("descendant.pid")).unwrap().parse().unwrap();
            assert!(peer::stopped(descendant));
            fs::write(other_root.path().join("release"), b"").unwrap();
            let other_report = other_task.join().unwrap();
            assert_eq!(other_report.outcome, Ok(()), "{other_report:?}");
            // Cancellation persistence and cleanup do not fabricate terminal DB evidence.
            assert_eq!(h.scalar("SELECT count(*) FROM attempts WHERE state='VALIDATING_OUTPUTS'").await, 2);
        }
    });
}

#[test]
fn cancellation_after_install_preserves_old_head_and_refuses_prebound_worker_spawn() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let old = h.start(10, 10, 0, contract()).await;
        h.publish(old, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let request = h.start(11, 10, 1, contract()).await;
        let mut target = control(&h, 11);
        let worker = target
            .register(
                h.owner.as_mut().unwrap(),
                request.intent.attempt(),
                request.intent.fence(),
            )
            .await
            .unwrap();
        let artifacts = ArtifactStore::open(&h.root).unwrap();
        let files = h.files(11);
        let candidate = artifacts
            .prepare(
                &files.root,
                &files.files,
                files.writer,
                files.staging,
                |_| Ok(()),
            )
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        store
            .repository()
            .unwrap()
            .prepare_publication(&request, &candidate)
            .await
            .unwrap();
        store.close().await.unwrap();
        let installed = candidate.install(|_| Ok(())).unwrap();
        assert_eq!(
            target.request(h.owner.as_mut().unwrap()).await.unwrap(),
            CancelRequest::Requested
        );
        let root = Scratch::create().unwrap();
        let mut launch = peer::launch(root.path(), "cancel");
        launch.request["attempt_id"] = request.intent.attempt().to_string().into();
        let helper = worker.clone();
        let report = worker.run(launch);
        assert_eq!(report.outcome, Err(Failure::Canceled));
        assert!(report.pid.is_none());
        let helper_root = Scratch::create().unwrap();
        let mut helper_launch = peer::launch(helper_root.path(), "budget");
        helper_launch.request["attempt_id"] = request.intent.attempt().to_string().into();
        let helper_report = helper.run(helper_launch);
        assert_eq!(helper_report.outcome, Err(Failure::Canceled));
        assert!(helper_report.pid.is_none());
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .commit_publication(&request, &installed)
                .await,
            Err(PublicationError::Canceled)
        ));
        store.close().await.unwrap();
        assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM events").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await, 1);
    });
}

#[test]
fn publication_wins_then_cancel_reports_too_late_without_rewriting_success() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let request = h.start(10, 10, 0, contract()).await;
        let mut target = control(&h, 10);
        let receipt = h.publish(request, 10).await.unwrap();
        assert_eq!(receipt.generation, 1);
        for _ in 0..2 {
            assert_eq!(
                target.request(h.owner.as_mut().unwrap()).await.unwrap(),
                CancelRequest::TooLate
            );
        }
        assert_eq!(h.scalar("SELECT cancel_requested FROM builds").await, 0);
        assert_eq!(
            h.scalar("SELECT count(*) FROM attempts WHERE state='SUCCEEDED'")
                .await,
            1
        );
        assert_eq!(
            h.scalar("SELECT count(*) FROM jobs WHERE state='SUCCEEDED'")
                .await,
            1
        );
        assert_eq!(h.scalar("SELECT count(*) FROM events").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM outbox_deliveries").await, 1);
    });
}

#[test]
fn stale_session_cannot_cancel_or_register_or_publish_after_lock_reacquisition() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let request = h.start(10, 10, 0, contract()).await;
        let mut old = control(&h, 10);
        let old_session = h.session();
        h.owner.take();
        let mut owner =
            RuntimeOwner::acquire(&h.root, workspace(), CoordinatorMode::Temporary).unwrap();
        publication::recover(&mut owner, 40).await.unwrap();
        assert!(matches!(
            old.request(&mut owner).await,
            Err(Error::Publication(PublicationError::Fence))
        ));
        assert!(
            old.register(&mut owner, request.intent.attempt(), request.intent.fence())
                .await
                .is_err()
        );
        let mut store = owner.open_store().await.unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .cancel_publications(workspace(), old_session, request.intent.build())
                .await,
            Err(PublicationError::Fence)
        ));
        store.close().await.unwrap();
        h.owner = Some(owner);
        assert!(matches!(
            h.publish(request, 10).await,
            Err(publication::Error::Publication(PublicationError::Fence))
        ));
        assert_eq!(h.scalar("SELECT cancel_requested FROM builds").await, 0);
        assert_eq!(
            h.scalar("SELECT count(*) FROM attempts WHERE state='INTERRUPTED'")
                .await,
            1
        );
        assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await, 0);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 0);
    });
}

#[test]
fn registration_rejects_wrong_build_attempt_fence_duplicate_capacity_and_canceled_build() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let request = h.start(10, 10, 0, contract()).await;
        let other = h.start(11, 11, 0, contract()).await;
        let mut target = control(&h, 10);
        let attempt = request.intent.attempt();
        let fence = request.intent.fence();
        assert!(
            target
                .register(h.owner.as_mut().unwrap(), other.intent.attempt(), fence)
                .await
                .is_err()
        );
        assert!(
            target
                .register(
                    h.owner.as_mut().unwrap(),
                    attempt,
                    Fence {
                        generation: 2,
                        ..fence
                    }
                )
                .await
                .is_err()
        );
        let worker = target
            .register(h.owner.as_mut().unwrap(), attempt, fence)
            .await
            .unwrap();
        assert!(matches!(
            target
                .register(h.owner.as_mut().unwrap(), attempt, fence)
                .await,
            Err(Error::Registration)
        ));
        let root = Scratch::create().unwrap();
        // Capability cannot be used for another attempt (no process launched).
        let report = worker.run(peer::launch(root.path(), "cancel"));
        assert_eq!(report.outcome, Err(Failure::Configuration));
        assert!(report.pid.is_none());
        assert!(BuildControl::new(h.owner.as_ref().unwrap(), request.intent.build(), 0).is_err());
        assert!(
            BuildControl::new(h.owner.as_ref().unwrap(), request.intent.build(), 10_001).is_err()
        );
        target.request(h.owner.as_mut().unwrap()).await.unwrap();
        let mut resumed = control(&h, 10);
        assert!(matches!(
            resumed
                .register(h.owner.as_mut().unwrap(), attempt, fence)
                .await,
            Err(Error::Publication(PublicationError::Canceled))
        ));
    });
}

#[test]
fn database_refusal_does_not_signal_worker_and_lost_control_still_cleans_up() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let request = h.start(10, 10, 0, contract()).await;
        let mut target = control(&h, 10);
        let worker = target
            .register(
                h.owner.as_mut().unwrap(),
                request.intent.attempt(),
                request.intent.fence(),
            )
            .await
            .unwrap();
        let root = Scratch::create().unwrap();
        let mut launch = peer::launch(root.path(), "budget");
        launch.request["attempt_id"] = request.intent.attempt().to_string().into();
        let task = thread::spawn(move || worker.run(launch));
        ready(&root.path().join("ready")).await;
        // Synthetic DB authority loss; the real owner guard remains alive.
        sqlx::query("UPDATE workspaces SET runtime_owner=?")
            .bind(CoordinatorSessionId::from_bytes([99; 16]).to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        assert!(matches!(
            target.request(h.owner.as_mut().unwrap()).await,
            Err(Error::Publication(PublicationError::Fence))
        ));
        assert_eq!(h.scalar("SELECT cancel_requested FROM builds").await, 0);
        assert!(!task.is_finished());
        let pid = fs::read_to_string(root.path().join("leader.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(!peer::stopped(pid));
        // Coordinator control loss is actual cleanup, not a fabricated DB receipt.
        drop(target);
        let report = task.join().unwrap();
        assert_eq!(report.outcome, Err(Failure::Canceled));
        assert!(peer::stopped(pid));
        assert_eq!(h.scalar("SELECT cancel_requested FROM builds").await, 0);
    });
}

#[test]
fn simultaneous_sqlite_writers_choose_one_visibility_winner() {
    runtime().block_on(async {
        for _ in 0..4 {
            let mut h = Harness::new().await;
            let request = h.start(10, 10, 0, contract()).await;
            let files = h.files(10);
            let artifacts = ArtifactStore::open(&h.root).unwrap();
            let candidate = artifacts
                .prepare(
                    &files.root,
                    &files.files,
                    files.writer,
                    files.staging,
                    |_| Ok(()),
                )
                .unwrap();
            let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
            owned
                .repository()
                .unwrap()
                .prepare_publication(&request, &candidate)
                .await
                .unwrap();
            owned.close().await.unwrap();
            let object = candidate.install(|_| Ok(())).unwrap();
            // Two synthetic repository callers race under the same still-held OS
            // owner. Production dispatch serializes store access; this probes SQL's
            // own commit arbitration without replacing it with a mock state machine.
            let path = h.root.join(".transflow/runtime/catalog.sqlite");
            let mut publishing = tf_store::Store::open(&path).await.unwrap();
            let mut canceling = tf_store::Store::open(&path).await.unwrap();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let cancel_barrier = barrier.clone();
            let build = request.intent.build();
            let session = h.session();
            let publication = thread::spawn(move || {
                runtime().block_on(async {
                    barrier.wait();
                    let result = publishing.commit_publication(&request, &object).await;
                    publishing.close().await.unwrap();
                    result
                })
            });
            let cancellation = thread::spawn(move || {
                runtime().block_on(async {
                    cancel_barrier.wait();
                    let result = canceling
                        .cancel_publications(workspace(), session, build)
                        .await
                        .unwrap();
                    canceling.close().await.unwrap();
                    result
                })
            });
            let published = publication.join().unwrap();
            let canceled = cancellation.join().unwrap();
            let count = match canceled {
                CancelRequest::Requested => {
                    assert!(matches!(published, Err(PublicationError::Canceled)));
                    0
                }
                CancelRequest::TooLate => {
                    assert!(published.is_ok());
                    1
                }
                CancelRequest::AlreadyRequested => unreachable!("fresh fixture"),
            };
            for sql in [
                "SELECT count(*) FROM dataset_versions",
                "SELECT count(*) FROM dataset_heads",
                "SELECT count(*) FROM events",
                "SELECT count(*) FROM outbox_deliveries",
                "SELECT count(*) FROM attempts WHERE state='SUCCEEDED'",
            ] {
                assert_eq!(h.scalar(sql).await, count);
            }
            assert_eq!(
                h.scalar("SELECT cancel_requested FROM builds").await,
                1 - count
            );
        }
    });
}

#[test]
fn canceling_pending_sibling_keeps_an_already_published_job_successful() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let first = h.start(10, 10, 0, contract()).await;
        let second = h.start(11, 11, 0, contract()).await;
        // Compose one two-output accepted build from the synthetic seeded records.
        let build = first.intent.build();
        for sql in [
            "UPDATE jobs SET build_id=? WHERE build_id=?",
            "UPDATE write_reservations SET build_id=? WHERE build_id=?",
        ] {
            sqlx::query(sql)
                .bind(build.to_string())
                .bind(second.intent.build().to_string())
                .execute(&mut h.db)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM builds WHERE id=?")
            .bind(second.intent.build().to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut target = BuildControl::new(h.owner.as_ref().unwrap(), build, 1).unwrap();
        let _worker = target
            .register(
                h.owner.as_mut().unwrap(),
                first.intent.attempt(),
                first.intent.fence(),
            )
            .await
            .unwrap();
        assert!(matches!(
            target
                .register(
                    h.owner.as_mut().unwrap(),
                    second.intent.attempt(),
                    second.intent.fence()
                )
                .await,
            Err(Error::Registration)
        ));
        h.publish(first, 10).await.unwrap();
        assert_eq!(
            target.request(h.owner.as_mut().unwrap()).await.unwrap(),
            CancelRequest::Requested
        );
        assert_eq!(
            h.scalar("SELECT count(*) FROM jobs WHERE state='SUCCEEDED'")
                .await,
            1
        );
        assert_eq!(
            h.scalar("SELECT count(*) FROM jobs WHERE state='VALIDATING_OUTPUTS'")
                .await,
            1
        );
        assert_eq!(
            h.scalar("SELECT count(*) FROM attempts WHERE state='SUCCEEDED'")
                .await,
            1
        );
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 1);
        assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM events").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await, 2);
    });
}
