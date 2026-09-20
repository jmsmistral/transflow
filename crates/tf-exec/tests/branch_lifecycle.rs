//! Real branch revision, reference, audit and source-indexed policy persistence.
#![allow(clippy::unwrap_used, reason = "Synthetic lifecycle fixtures")]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use std::collections::BTreeMap;
use tf_domain::{input::BranchPolicySnapshot, *};
use tf_store::{
    Reader,
    branch_lifecycle::{BranchChange, Error},
};
#[test]
fn rename_preserves_heads_and_delete_retains_history_with_audit() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let mut reader = Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite"))
            .await
            .unwrap();
        let old = reader
            .branch(workspace(), &"master".parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        repo.change_branch(
            workspace(),
            &old,
            BranchChange::Rename("renamed".parse().unwrap()),
            100,
        )
        .await
        .unwrap();
        assert!(matches!(
            repo.change_branch(workspace(), &old, BranchChange::Delete, 101)
                .await,
            Err(Error::Conflict)
        ));
        let new = reader
            .branch(workspace(), &"renamed".parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(new.id, old.id);
        assert_eq!(new.heads, 1);
        repo.change_branch(workspace(), &new, BranchChange::Delete, 102)
            .await
            .unwrap();
        store.close().await.unwrap();
        reader.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_heads").await, 1);
        assert_eq!(
            h.scalar("SELECT count(*) FROM audit_log WHERE operation LIKE 'branch.%'")
                .await,
            2
        );
    });
}
#[test]
fn live_jobs_views_schedules_and_leases_are_checked_again_at_mutation() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let mut reader = Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite"))
            .await
            .unwrap();
        let row = reader
            .branch(workspace(), &"master".parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .change_branch(workspace(), &row, BranchChange::Delete, 100)
                .await,
            Err(Error::Referenced(_))
        ));
        store.close().await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        sqlx::query("INSERT INTO graph_views VALUES('view',0,1,'{}',?,1)")
            .bind(serde_json::json!({"branch_id":row.id.to_string()}).to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .change_branch(workspace(), &row, BranchChange::Delete, 100)
                .await,
            Err(Error::Referenced(_))
        ));
        store.close().await.unwrap();
        sqlx::query("DELETE FROM graph_views")
            .execute(&mut h.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO schedules VALUES('schedule','daily',NULL,1,NULL)")
            .execute(&mut h.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO schedule_revisions VALUES('schedule',1,'{}','{}',?,'fixture',1)")
            .bind(serde_json::json!({"fallbacks":["master"]}).to_string())
            .execute(&mut h.db)
            .await
            .unwrap();
        sqlx::query("UPDATE schedules SET active_revision=1")
            .execute(&mut h.db)
            .await
            .unwrap();
        assert!(
            reader
                .branch_references(&row, 100)
                .await
                .unwrap()
                .iter()
                .any(|s| s.contains("schedule"))
        );
        sqlx::query("UPDATE schedules SET deleted_at_us=99")
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let lease = store
            .repository()
            .unwrap()
            .acquire_read(
                RequestId::from_bytes([40; 16]),
                RequestId::from_bytes([41; 16]),
                tf_store::retention::LeaseKind::Query,
                tf_store::retention::ReadTarget::Head(branch(), DatasetId::from_bytes([10; 16])),
                100,
                100,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .change_branch(workspace(), &row, BranchChange::Delete, 101)
                .await,
            Err(Error::Referenced(_))
        ));
        store
            .repository()
            .unwrap()
            .release_read(&lease)
            .await
            .unwrap();
        store
            .repository()
            .unwrap()
            .change_branch(workspace(), &row, BranchChange::Delete, 102)
            .await
            .unwrap();
        store.close().await.unwrap();
        reader.close().await.unwrap();
    });
}
#[test]
fn audit_failure_rolls_back_and_source_policy_cannot_be_replaced() {
    runtime().block_on(async {
    let mut h=Harness::new().await;let mut reader=Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite")).await.unwrap();let row=reader.branch(workspace(),&"master".parse().unwrap()).await.unwrap().unwrap();
    sqlx::raw_sql("CREATE TRIGGER reject_audit BEFORE INSERT ON audit_log BEGIN SELECT RAISE(ABORT,'injected failure'); END;").execute(&mut h.db).await.unwrap();
    let mut store=h.owner.as_mut().unwrap().open_store().await.unwrap();assert!(store.repository().unwrap().change_branch(workspace(),&row,BranchChange::Rename("new".parse().unwrap()),100).await.is_err());assert!(reader.branch(workspace(),&"master".parse().unwrap()).await.unwrap().is_some());
    let first=BranchPolicySnapshot::new(vec!["master".parse().unwrap()],BTreeMap::from([("master".parse().unwrap(),vec![])])).unwrap();let changed=BranchPolicySnapshot::new(vec!["develop".parse().unwrap()],BTreeMap::new()).unwrap();
    store.repository().unwrap().snapshot_branch_policies(source(),&first).await.unwrap();store.repository().unwrap().snapshot_branch_policies(source(),&first).await.unwrap();assert!(store.repository().unwrap().snapshot_branch_policies(source(),&changed).await.is_err());
    let retained=reader.branch_policy_snapshot(source()).await.unwrap();assert_eq!(retained.defaults(),first.defaults());assert!(retained.rules().get(&"master".parse().unwrap()).unwrap().is_empty());store.close().await.unwrap();reader.close().await.unwrap();
 });
}
