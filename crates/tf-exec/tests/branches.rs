//! Read-only branch planning, tombstone/revision guards and atomic creation audit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic branch fixtures"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use tf_domain::*;
use tf_protocol::canonical::{ContentDigest, DigestKind};
use tf_store::branches::*;
fn evidence() -> BranchCreationEvidence {
    BranchCreationEvidence {
        source: source(),
        certificate: ContentDigest::from_hex(DigestKind::Compute, &"a".repeat(64)).unwrap(),
        git_ref: None,
    }
}
#[test]
fn planning_is_read_only_and_lazy_creation_is_empty_idempotent_and_audited() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let mut reader =
            tf_store::Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite"))
                .await
                .unwrap();
        let name = "feature/Δ".parse().unwrap();
        let preview = reader
            .preview_output_branch(workspace(), &name)
            .await
            .unwrap();
        assert_eq!(preview.state(), BranchExistence::Absent);
        assert_eq!(h.scalar("SELECT count(*) FROM data_branches").await, 1);
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let id = BranchId::from_bytes([40; 16]);
        assert_eq!(
            store
                .repository()
                .unwrap()
                .create_output_branch(&preview, id, &evidence(), 30)
                .await
                .unwrap(),
            id
        );
        let current = reader
            .preview_output_branch(workspace(), &name)
            .await
            .unwrap();
        assert_eq!(
            current.state(),
            BranchExistence::Present { id, revision: 0 }
        );
        assert_eq!(
            store
                .repository()
                .unwrap()
                .create_output_branch(&current, BranchId::from_bytes([41; 16]), &evidence(), 31)
                .await
                .unwrap(),
            id
        );
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .create_output_branch(&preview, id, &evidence(), 31)
                .await,
            Err(BranchError::Conflict)
        ));
        store.close().await.unwrap();
        reader.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_heads").await, 1);
        assert_eq!(
            h.scalar("SELECT count(*) FROM audit_log WHERE operation='branch.create'")
                .await,
            1
        );
    });
}
#[test]
fn tombstones_and_changed_revisions_cannot_be_silently_recreated_or_adopted() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let mut reader =
            tf_store::Reader::open_existing(&h.root.join(".transflow/runtime/catalog.sqlite"))
                .await
                .unwrap();
        let name = "master".parse().unwrap();
        let preview = reader
            .preview_output_branch(workspace(), &name)
            .await
            .unwrap();
        sqlx::query("UPDATE data_branches SET revision=1")
            .execute(&mut h.db)
            .await
            .unwrap();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .create_output_branch(&preview, BranchId::from_bytes([40; 16]), &evidence(), 30)
                .await,
            Err(BranchError::Conflict)
        ));
        store.close().await.unwrap();
        sqlx::query("UPDATE data_branches SET deleted_at_us=31,revision=2")
            .execute(&mut h.db)
            .await
            .unwrap();
        assert!(matches!(
            reader.preview_output_branch(workspace(), &name).await,
            Err(BranchError::Tombstoned)
        ));
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(matches!(
            store
                .repository()
                .unwrap()
                .create_output_branch(&preview, BranchId::from_bytes([40; 16]), &evidence(), 32)
                .await,
            Err(BranchError::Tombstoned)
        ));
        store.close().await.unwrap();
        reader.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM data_branches").await, 1);
    });
}
#[test]
fn failed_creation_audit_rolls_back_branch_row() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        sqlx::raw_sql(
            "CREATE TRIGGER reject_branch_audit BEFORE INSERT ON audit_log
             BEGIN SELECT RAISE(ABORT,'injected audit failure'); END;",
        )
        .execute(&mut h.db)
        .await
        .unwrap();
        let preview =
            OutputBranchPreview::without_runtime(workspace(), "new branch".parse().unwrap());
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert!(
            store
                .repository()
                .unwrap()
                .create_output_branch(&preview, BranchId::from_bytes([40; 16]), &evidence(), 30,)
                .await
                .is_err()
        );
        store.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM data_branches").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM audit_log").await, 0);
    });
}
