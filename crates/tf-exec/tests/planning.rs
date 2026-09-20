//! Real SQLite/Parquet draft immutability, guard failures and atomic reservation qualification.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Synthetic retained publication fixtures"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::json;
use tf_domain::*;
use tf_store::planning::*;
async fn draft(h: &mut Harness, n: u8) -> DraftPlan {
    let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
    let store = owned.repository().unwrap();
    for d in [20, 21] {
        store
            .register_dataset(workspace(), DatasetId::from_bytes([d; 16]), 1)
            .await
            .unwrap();
    }
    let output = store.plan_guard(workspace(), "master", None).await.unwrap();
    let mut guards = vec![];
    for d in [20, 21] {
        guards.push(
            store
                .plan_guard(workspace(), "master", Some(DatasetId::from_bytes([d; 16])))
                .await
                .unwrap(),
        );
    }
    let plan = DraftPlan {
        format_version: 1,
        id: RequestId::from_bytes([n; 16]).to_string(),
        workspace: workspace().to_string(),
        source: SourceSnapshotId::from_bytes([90; 16]).to_string(),
        source_digest: "a".repeat(64),
        registry: "format_version=1\n".into(),
        replacement: "format_version=1\n".into(),
        discovery: json!({}),
        environment: json!({"fingerprint":"b".repeat(64)}),
        source_evidence: json!({"selector":{"kind":"working_tree"},"git":null,"files":[]}),
        output,
        guards,
        writes: [20, 21]
            .into_iter()
            .map(|d| PlannedWrite {
                dataset: DatasetId::from_bytes([d; 16]).to_string(),
                job: JobId::from_bytes([d; 16]).to_string(),
                generation: 0,
                bindings: json!([]),
            })
            .collect(),
        reads: vec![],
        context: json!({"parameters":{},"branch_policies":{"": ["master"]}}),
        created_us: 100,
        expires_us: 1000,
    };
    store.save_draft(&plan).await.unwrap();
    owned.close().await.unwrap();
    plan
}
async fn count(h: &mut Harness, table: &str) -> i64 {
    sqlx::query_scalar(match table {
        "jobs" => "SELECT count(*) FROM jobs",
        "builds" => "SELECT count(*) FROM builds",
        "write_reservations" => "SELECT count(*) FROM write_reservations",
        _ => panic!("unknown fixture table"),
    })
    .fetch_one(&mut h.db)
    .await
    .unwrap()
}
#[test]
fn exact_saved_content_expiry_and_head_conflicts_leave_no_jobs_or_reservations() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let plan = draft(&mut h, 80).await;
        let session = h.owner.as_ref().unwrap().registration().session().unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store = owned.repository().unwrap();
        assert_eq!(
            store
                .load_draft(plan.id.parse().unwrap())
                .await
                .unwrap()
                .digest()
                .unwrap(),
            plan.digest().unwrap()
        );
        let mut changed = plan.clone();
        changed.context["force"] = json!(true);
        assert!(store.save_draft(&changed).await.is_err());
        for now in [99, 1000] {
            assert!(
                store
                    .accept_draft(&plan, BuildId::from_bytes([80; 16]), branch(), session, now)
                    .await
                    .is_err()
            );
        }
        owned.close().await.unwrap();
        sqlx::query("UPDATE data_branches SET revision=revision+1 WHERE id=?")
            .bind(branch().to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(
            owned
                .repository()
                .unwrap()
                .accept_draft(&plan, BuildId::from_bytes([80; 16]), branch(), session, 101)
                .await
                .is_err()
        );
        owned.close().await.unwrap();
        assert_eq!(count(&mut h, "jobs").await, 0);
        assert_eq!(count(&mut h, "write_reservations").await, 0);
    });
}
#[test]
fn whole_write_set_rolls_back_when_later_dataset_is_unavailable_and_acceptance_is_single_use() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let plan = draft(&mut h, 80).await;
        let session = h.owner.as_ref().unwrap().registration().session().unwrap();
        sqlx::query("UPDATE datasets SET tombstoned_at_us=1 WHERE id=?")
            .bind(DatasetId::from_bytes([21; 16]).to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(
            owned
                .repository()
                .unwrap()
                .accept_draft(&plan, BuildId::from_bytes([80; 16]), branch(), session, 101)
                .await
                .is_err()
        );
        owned.close().await.unwrap();
        assert_eq!(count(&mut h, "jobs").await, 0);
        assert_eq!(count(&mut h, "write_reservations").await, 0);
        assert_eq!(count(&mut h, "builds").await, 0);
        sqlx::query("UPDATE datasets SET tombstoned_at_us=NULL")
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store = owned.repository().unwrap();
        store
            .accept_draft(&plan, BuildId::from_bytes([80; 16]), branch(), session, 102)
            .await
            .unwrap();
        assert!(
            store
                .accept_draft(&plan, BuildId::from_bytes([81; 16]), branch(), session, 103)
                .await
                .is_err()
        );
        owned.close().await.unwrap();
        assert_eq!(count(&mut h, "jobs").await, 2);
        assert_eq!(count(&mut h, "write_reservations").await, 2);
    });
}
#[test]
fn pinned_boundary_lease_is_promoted_and_earlier_fallback_arrival_invalidates_draft() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let publication = h.start(10, 10, 0, contract()).await;
        h.publish(publication, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let mut plan = draft(&mut h, 80).await;
        // Save a distinct draft containing a real immutable leased version and both candidate guards.
        plan.id = RequestId::from_bytes([81; 16]).to_string();
        let session = h.owner.as_ref().unwrap().registration().session().unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store = owned.repository().unwrap();
        let lease = RequestId::from_bytes([82; 16]);
        let ticket = store
            .acquire_read(
                lease,
                plan.id.parse().unwrap(),
                tf_store::retention::LeaseKind::Plan,
                tf_store::retention::ReadTarget::Version(VersionId::from_bytes([10; 16])),
                100,
                900,
            )
            .await
            .unwrap();
        plan.reads.push(PlannedRead {
            consumer: plan.writes[0].dataset.clone(),
            alias: "rows".into(),
            dataset: DatasetId::from_bytes([10; 16]).to_string(),
            version: VersionId::from_bytes([10; 16]).to_string(),
            artifact: ticket.artifact().hex(),
            lease: lease.to_string(),
            semantic: json!({}),
            provenance: json!({"alias":"rows","workspace":workspace().to_string(),"dataset":DatasetId::from_bytes([10;16]).to_string(),"version":VersionId::from_bytes([10;16]).to_string(),"artifact":ticket.artifact().hex(),"declared_branch":{"kind":"omitted","name":null},"starting_branch":"master","resolved_branch":"master","role":"data","resolution":{"kind":"exact_pin"}}),
        });
        plan.guards.push(
            store
                .plan_guard(
                    workspace(),
                    "earlier",
                    Some(DatasetId::from_bytes([10; 16])),
                )
                .await
                .unwrap(),
        );
        store.save_draft(&plan).await.unwrap();
        owned.close().await.unwrap();
        sqlx::query(
            "INSERT INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,'earlier',0)",
        )
        .bind(BranchId::from_bytes([83; 16]).to_string())
        .bind(workspace().to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(
            owned
                .repository()
                .unwrap()
                .accept_draft(&plan, BuildId::from_bytes([80; 16]), branch(), session, 101)
                .await
                .is_err()
        );
        owned.close().await.unwrap();
        sqlx::query("DELETE FROM data_branches WHERE name='earlier'")
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        owned
            .repository()
            .unwrap()
            .accept_draft(&plan, BuildId::from_bytes([80; 16]), branch(), session, 102)
            .await
            .unwrap();
        owned.close().await.unwrap();
        let (kind, expiry): (String, i64) =
            sqlx::query_as("SELECT kind,expires_at_us FROM read_leases WHERE id=?")
                .bind(lease.to_string())
                .fetch_one(&mut h.db)
                .await
                .unwrap();
        assert_eq!(kind, "build");
        assert_eq!(expiry, 3_600_000_102);
    });
}

#[test]
fn competing_writer_on_second_output_never_partially_reserves_first() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let first = draft(&mut h, 80).await;
        let mut second = first.clone();
        second.id = RequestId::from_bytes([81; 16]).to_string();
        second.writes[0].dataset = DatasetId::from_bytes([22; 16]).to_string();
        for (w, n) in second.writes.iter_mut().zip([30, 31]) {
            w.job = JobId::from_bytes([n; 16]).to_string();
        }
        let session = h.owner.as_ref().unwrap().registration().session().unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store = owned.repository().unwrap();
        store
            .register_dataset(workspace(), DatasetId::from_bytes([22; 16]), 1)
            .await
            .unwrap();
        second.guards.push(
            store
                .plan_guard(workspace(), "master", Some(DatasetId::from_bytes([22; 16])))
                .await
                .unwrap(),
        );
        store.save_draft(&second).await.unwrap();
        store
            .accept_draft(
                &first,
                BuildId::from_bytes([80; 16]),
                branch(),
                session,
                101,
            )
            .await
            .unwrap();
        assert!(
            store
                .accept_draft(
                    &second,
                    BuildId::from_bytes([81; 16]),
                    branch(),
                    session,
                    102
                )
                .await
                .is_err()
        );
        owned.close().await.unwrap();
        assert_eq!(count(&mut h, "jobs").await, 2);
        assert_eq!(count(&mut h, "write_reservations").await, 2);
        assert_eq!(count(&mut h, "builds").await, 1);
        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM write_reservations WHERE dataset_id=?")
                .bind(DatasetId::from_bytes([22; 16]).to_string())
                .fetch_one(&mut h.db)
                .await
                .unwrap();
        assert_eq!(rows, 0);
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let replay = owned
            .repository()
            .unwrap()
            .lease_replay(
                BuildId::from_bytes([80; 16]),
                "replay".parse().unwrap(),
                RequestId::from_bytes([90; 16]),
                &[],
                103,
                100,
            )
            .await
            .unwrap();
        assert_eq!(replay.manifest.scope().unwrap().len(), 2);
        assert_eq!(replay.manifest.parameters(), &json!({}));
        owned.close().await.unwrap();
    });
}
