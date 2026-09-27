//! Real import visibility transactions share durable artifacts and head/event publication.
#![allow(clippy::unwrap_used, reason = "Synthetic publication assertions")]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::json;
use tf_domain::*;
use tf_store::{
    artifacts::{ArtifactStore, VerifiedArtifact},
    import_publication::ImportPublication,
};

async fn request(h: &mut Harness, n: u8) -> ImportPublication {
    let session = h.session();
    let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
    let s = owned.repository().unwrap();
    s.register_dataset(workspace(), DatasetId::from_bytes([10; 16]), 1)
        .await
        .unwrap();
    let head = s
        .plan_guard(workspace(), "master", Some(DatasetId::from_bytes([10; 16])))
        .await
        .unwrap();
    owned.close().await.unwrap();
    ImportPublication {
        dataset: DatasetKey::new(workspace(), DatasetId::from_bytes([10; 16])),
        import: RequestId::from_bytes([n; 16]),
        version: VersionId::from_bytes([n; 16]),
        session,
        head,
        new_branch: BranchId::from_bytes([n; 16]),
        source: SourceSnapshotId::from_bytes([n; 16]),
        source_digest: "a".repeat(64),
        manifest: json!({"files":[],"import_files":["one"]}),
        git: json!(null),
        environment: json!({}),
        at_us: 20,
    }
}
fn artifact(h: &Harness) -> VerifiedArtifact {
    let f = h.files(200);
    let store = ArtifactStore::open(&h.root).unwrap();
    store
        .prepare(&f.root, &f.files, f.writer, f.staging, |_| Ok(()))
        .unwrap()
        .install(|_| Ok(()))
        .unwrap()
}
async fn commit(h: &mut Harness, r: &ImportPublication, a: &VerifiedArtifact) -> bool {
    let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
    let result = owned.repository().unwrap().commit_import(r, a).await;
    owned.close().await.unwrap();
    result.is_ok()
}
#[test]
fn imports_publish_origin_head_and_event_without_fake_attempts_or_checks() {
    runtime().block_on(async {
        let mut h=Harness::new().await;
        let a=artifact(&h);
        let first=request(&mut h,80).await;
        assert!(commit(&mut h,&first,&a).await);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions WHERE import_id IS NOT NULL AND attempt_id IS NULL").await,1);
        assert_eq!(h.scalar("SELECT count(*) FROM attempts").await,0);
        assert_eq!(h.scalar("SELECT count(*) FROM check_results").await,0);
        assert_eq!(h.scalar("SELECT count(*) FROM events WHERE type='dataset.published'").await,1);
        assert_eq!(h.scalar("SELECT count(*) FROM outbox_deliveries").await,1);
        let stale=request(&mut h,81).await;
        let next=request(&mut h,82).await;
        assert!(commit(&mut h,&next,&a).await);
        assert!(!commit(&mut h,&stale,&a).await);
        assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await,2);
        assert_eq!(h.scalar("SELECT count(*) FROM artifacts").await,1);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await,2);
    });
}
#[test]
fn import_fence_reservation_and_late_sql_failure_preserve_last_good() {
    runtime().block_on(async {
        for failure in ["fence","reservation","outbox"] {
            let mut h=Harness::new().await;
            let a=artifact(&h);
            let first=request(&mut h,80).await;
            assert!(commit(&mut h,&first,&a).await);
            let mut r=request(&mut h,81).await;
            match failure {
                "fence" => r.session=CoordinatorSessionId::from_bytes([99;16]),
                "reservation" => {h.start(15,10,1,contract()).await;},
                _ => {sqlx::raw_sql("CREATE TRIGGER import_failure BEFORE INSERT ON outbox_deliveries BEGIN SELECT RAISE(ABORT,'injected outbox failure'); END;").execute(&mut h.db).await.unwrap();},
            }
            assert!(!commit(&mut h,&r,&a).await,"{failure}");
            assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await,1);
            assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await,1);
            assert_eq!(h.scalar("SELECT count(*) FROM events WHERE type IN ('dataset.published','dataset.head_changed')").await,1);
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM source_snapshots WHERE id=?").bind(r.source.to_string()).fetch_one(&mut h.db).await.unwrap(),0);
        }
    });
}
