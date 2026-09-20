//! Actual bundled SQLite migrations, constraints and transaction failure tests.
#[allow(
    dead_code,
    reason = "Shared T007 harness supplies more fixtures than this suite needs"
)]
#[path = "../../../tests/support/mod.rs"]
mod support;
use sqlx::{Connection, SqliteConnection};
use std::{
    error::Error,
    time::{Duration, Instant},
};
use support::filesystem::ScratchDirectory;
use tf_domain::{DatasetId, RequestId, WorkspaceId};
use tf_store::{Store, StoreError};
type Result = std::result::Result<(), Box<dyn Error>>;
fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}
fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
fn dataset() -> DatasetId {
    DatasetId::from_bytes([2; 16])
}
async fn raw(path: &std::path::Path) -> std::result::Result<SqliteConnection, sqlx::Error> {
    support::database::connect(path).await
}

#[test]
fn fresh_reopen_pragmas_and_durable_repositories() -> Result {
    runtime()?.block_on(async {
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("runtime.sqlite");
        let mut store = Store::open(&path).await?;
        assert_eq!(store.info().sqlite_version, "3.51.3");
        assert_eq!(store.info().schema_version, 3);
        assert!(store.info().foreign_keys);
        assert_eq!(store.info().synchronous, 2);
        assert_eq!(store.info().busy_timeout_ms, 250);
        store
            .register_workspace(workspace(), "synthetic-root", 100)
            .await?;
        store
            .register_workspace(workspace(), "synthetic-root", 200)
            .await?;
        assert!(
            store
                .register_workspace(workspace(), "other-root", 300)
                .await
                .is_err()
        );
        store.register_dataset(workspace(), dataset(), 100).await?;
        store.register_dataset(workspace(), dataset(), 200).await?;
        let one = store
            .append_event(
                RequestId::from_bytes([3; 16]),
                "test",
                &serde_json::json!({"value":1}),
                100,
            )
            .await?;
        let two = store
            .append_event(
                RequestId::from_bytes([4; 16]),
                "test",
                &serde_json::json!({"value":2}),
                50,
            )
            .await?;
        assert!(two > one);
        let mut reader = store.reader().await?;
        assert!(reader.contains_dataset(workspace(), dataset()).await?);
        assert!(reader.events_after(0, 101).await.is_err());
        assert_eq!(reader.events_after(one, 100).await?.len(), 1);
        reader.close().await?;
        store.close().await?;
        let store = Store::open(&path).await?;
        let mut db = raw(&path).await?;
        assert_eq!(
            sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
                .fetch_one(&mut db)
                .await?,
            "wal"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"
            )
            .fetch_one(&mut db)
            .await?,
            43
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT created_at_us FROM datasets")
                .fetch_one(&mut db)
                .await?,
            100
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log")
                .fetch_one(&mut db)
                .await?,
            2
        );
        db.close().await?;
        store.close().await?;
        Ok(())
    })
}
async fn v1(path: &std::path::Path) -> std::result::Result<SqliteConnection, Box<dyn Error>> {
    let mut db = raw(path).await?;
    sqlx::raw_sql(include_str!("../migrations/001_core.sql"))
        .execute(&mut db)
        .await?;
    sqlx::raw_sql("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, checksum TEXT NOT NULL) STRICT; PRAGMA application_id=1414678092; PRAGMA user_version=1;").execute(&mut db).await?;
    let checksum = tf_protocol::canonical::file_digest(
        &mut include_bytes!("../migrations/001_core.sql").as_slice(),
    )?
    .hex();
    sqlx::query("INSERT INTO schema_migrations VALUES(1,?)")
        .bind(checksum)
        .execute(&mut db)
        .await?;
    Ok(db)
}
#[test]
fn upgrade_preserves_existing_rows_and_is_idempotent() -> Result {
    runtime()?.block_on(async {
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("runtime.sqlite");
        let mut db = v1(&path).await?;
        sqlx::query(
            "INSERT INTO workspaces(id,root_identity,created_at_us) VALUES('old-id','old-root',10)",
        )
        .execute(&mut db)
        .await?;
        db.close().await?;
        Store::open(&path).await?.close().await?;
        Store::open(&path).await?.close().await?;
        let mut db = raw(&path).await?;
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT id FROM workspaces")
                .fetch_one(&mut db)
                .await?,
            "old-id"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schema_migrations")
                .fetch_one(&mut db)
                .await?,
            3
        );
        db.close().await?;
        Ok(())
    })
}
#[test]
fn incompatible_and_tampered_migration_evidence_is_refused() -> Result {
    runtime()?.block_on(async {
        for statement in [
            "PRAGMA user_version=99",
            "UPDATE schema_migrations SET checksum=printf('%064d',0) WHERE version=1",
            "DELETE FROM schema_migrations WHERE version=1",
            "PRAGMA application_id=12",
        ] {
            let dir = ScratchDirectory::new()?;
            let path = dir.path().join("runtime.sqlite");
            Store::open(&path).await?.close().await?;
            let mut db = raw(&path).await?;
            sqlx::raw_sql(statement).execute(&mut db).await?;
            db.close().await?;
            assert!(matches!(
                Store::open(&path).await,
                Err(StoreError::NewerSchema | StoreError::Migration)
            ));
        }
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("unknown.sqlite");
        let mut db = raw(&path).await?;
        sqlx::query("CREATE TABLE unrelated(value TEXT)")
            .execute(&mut db)
            .await?;
        db.close().await?;
        assert!(matches!(
            Store::open(&path).await,
            Err(StoreError::Migration)
        ));
        Ok(())
    })
}
#[test]
fn migration_failure_rolls_back_all_ddl_and_version_markers() -> Result {
    runtime()?.block_on(async {
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("runtime.sqlite");
        let mut db = v1(&path).await?;
        sqlx::query("CREATE TABLE replicas(foreign_obstacle TEXT)")
            .execute(&mut db)
            .await?;
        db.close().await?;
        assert!(Store::open(&path).await.is_err());
        let mut db = raw(&path).await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA user_version")
                .fetch_one(&mut db)
                .await?,
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sqlite_master WHERE name='catalog_mutations'"
            )
            .fetch_one(&mut db)
            .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schema_migrations")
                .fetch_one(&mut db)
                .await?,
            1
        );
        db.close().await?;
        Ok(())
    })
}
#[test]
fn foreign_keys_unique_keys_and_immutable_evidence_are_enforced() -> Result {
    runtime()?.block_on(async{
 let dir=ScratchDirectory::new()?;let path=dir.path().join("runtime.sqlite");let mut store=Store::open(&path).await?;
 assert!(store.register_dataset(workspace(),dataset(),0).await.is_err());store.register_workspace(workspace(),"root",0).await?;
 store.append_event(RequestId::from_bytes([1;16]),"test",&serde_json::json!({}),0).await?;
 let mut db=raw(&path).await?;
 for statement in ["DELETE FROM events", "UPDATE events SET type='changed'", "INSERT INTO attempts(id,job_id,attempt_no,session_id,fence,state,started_at_us) VALUES('a','missing',1,'s',1,'RUNNING',0)", "INSERT INTO foreign_versions VALUES('w','d','v','{}','bad','available')"]{
 assert!(sqlx::raw_sql(statement).execute(&mut db).await.is_err());
 }
 sqlx::raw_sql("INSERT INTO foreign_versions VALUES('w','d','v','{}',printf('%064d',0),'available'); INSERT INTO foreign_versions VALUES('other','d','v','{}',printf('%064d',0),'available');").execute(&mut db).await?;
 assert!(sqlx::query("INSERT INTO foreign_versions SELECT * FROM foreign_versions LIMIT 1").execute(&mut db).await.is_err());
 let violations=sqlx::query("PRAGMA foreign_key_check").fetch_all(&mut db).await?;assert!(violations.is_empty());
 db.close().await?;store.close().await?;Ok(())
})
}
#[test]
fn failed_event_audit_transaction_rolls_back_and_releases_writer() -> Result {
    runtime()?.block_on(async{
 let dir=ScratchDirectory::new()?;let path=dir.path().join("runtime.sqlite");let mut store=Store::open(&path).await?;let mut db=raw(&path).await?;
 sqlx::raw_sql("CREATE TRIGGER fail_audit BEFORE INSERT ON audit_log BEGIN SELECT RAISE(ABORT,'injected'); END;").execute(&mut db).await?;
 assert!(store.append_event(RequestId::from_bytes([1;16]),"test",&serde_json::json!({}),0).await.is_err());
 let mut reader=store.reader().await?;assert!(reader.events_after(0,10).await?.is_empty());
 sqlx::query("DROP TRIGGER fail_audit").execute(&mut db).await?;
 store.append_event(RequestId::from_bytes([1;16]),"test",&serde_json::json!({}),0).await?;
 assert_eq!(reader.events_after(0,10).await?.len(),1);
 reader.close().await?;db.close().await?;store.close().await?;Ok(())
})
}
#[test]
fn busy_wait_is_bounded_and_reader_snapshot_does_not_block_writer() -> Result {
    runtime()?.block_on(async {
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("runtime.sqlite");
        let mut store = Store::open(&path).await?;
        let mut db = raw(&path).await?;
        let tx = db.begin_with("BEGIN IMMEDIATE").await?;
        let start = Instant::now();
        assert!(
            store
                .register_workspace(workspace(), "root", 0)
                .await
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        tx.rollback().await?;
        store.register_workspace(workspace(), "root", 0).await?;
        let mut snapshot = db.begin().await?;
        sqlx::query("SELECT * FROM events")
            .fetch_all(&mut *snapshot)
            .await?;
        store
            .append_event(
                RequestId::from_bytes([1; 16]),
                "test",
                &serde_json::json!({}),
                0,
            )
            .await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events")
                .fetch_one(&mut *snapshot)
                .await?,
            0
        );
        snapshot.commit().await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events")
                .fetch_one(&mut db)
                .await?,
            1
        );
        db.close().await?;
        store.close().await?;
        Ok(())
    })
}
#[test]
fn database_symlink_is_rejected_before_open() -> Result {
    runtime()?.block_on(async {
        let dir = ScratchDirectory::new()?;
        let actual = dir.path().join("actual.sqlite");
        let link = dir.path().join("link.sqlite");
        std::fs::write(&actual, b"keep")?;
        std::os::unix::fs::symlink(&actual, &link)?;
        assert!(matches!(
            Store::open(&link).await,
            Err(StoreError::Filesystem)
        ));
        assert_eq!(std::fs::read(&actual)?, b"keep");
        Ok(())
    })
}

#[test]
fn branch_heads_require_matching_versions_and_increasing_generations() -> Result {
    runtime()?.block_on(async {
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("runtime.sqlite");
        let store = Store::open(&path).await?;
        let mut db = raw(&path).await?;
        sqlx::raw_sql(
            "INSERT INTO workspaces(id,root_identity,created_at_us) VALUES('w','root',0);
 INSERT INTO datasets VALUES('d1','w',0,NULL),('d2','w',0,NULL);
 INSERT INTO source_snapshots VALUES('s','w','digest','{}',NULL,'{}','{}');
 INSERT INTO data_branches VALUES('b','w','master',NULL,0,NULL);
 INSERT INTO artifacts VALUES(printf('%064d',0),'{}','{}',1,1,1,'VERIFIED');
 INSERT INTO dataset_versions VALUES('v1','d1',printf('%064d',0),NULL,'i1','s',0,'c','k');
 INSERT INTO dataset_versions VALUES('v2','d2',printf('%064d',0),NULL,'i2','s',0,'c','k');
 INSERT INTO dataset_heads VALUES('b','d1','v1',1);",
        )
        .execute(&mut db)
        .await?;
        assert!(
            sqlx::query("UPDATE dataset_heads SET version_id='v2',generation=2")
                .execute(&mut db)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE dataset_heads SET generation=1")
                .execute(&mut db)
                .await
                .is_err()
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT version_id FROM dataset_heads")
                .fetch_one(&mut db)
                .await?,
            "v1"
        );
        sqlx::query("UPDATE dataset_heads SET generation=2")
            .execute(&mut db)
            .await?;
        assert!(
            sqlx::query("UPDATE dataset_versions SET compute_fingerprint='changed'")
                .execute(&mut db)
                .await
                .is_err()
        );
        db.close().await?;
        store.close().await?;
        Ok(())
    })
}

fn mutation() -> std::result::Result<tf_store::catalog_mutations::CatalogMutation, Box<dyn Error>> {
    use tf_protocol::canonical::file_digest;
    Ok(tf_store::catalog_mutations::CatalogMutation {
        id: RequestId::from_bytes([12; 16]),
        workspace: workspace(),
        source: tf_domain::SourceSnapshotId::from_bytes([13; 16]),
        old_digest: file_digest(&mut &b"old"[..])?,
        new_digest: file_digest(&mut &b"new"[..])?,
        assignments: vec![
            ("raw/a".parse()?, dataset()),
            ("raw/b".parse()?, DatasetId::from_bytes([3; 16])),
        ],
        index_ids: vec![dataset(), DatasetId::from_bytes([3; 16])],
    })
}
#[test]
fn registry_journal_roundtrip_single_pending_and_atomic_index_failure() -> Result {
    use tf_store::catalog_mutations::MutationState;
    runtime()?.block_on(async {
        let dir=ScratchDirectory::new()?;let path=dir.path().join("runtime.sqlite");let mut store=Store::open(&path).await?;
        store.register_workspace(workspace(),"synthetic-root",100).await?;
        let m=mutation()?;store.prepare_catalog_mutation(&m).await?;let mut another=m.clone();another.id=RequestId::from_bytes([14;16]);
        assert!(store.prepare_catalog_mutation(&another).await.is_err());store.close().await?;
        let mut store=Store::open(&path).await?;assert_eq!(store.pending_catalog_mutations().await?,vec![m.clone()]);
        let mut db=raw(&path).await?;
        sqlx::query("CREATE TRIGGER fail_second BEFORE INSERT ON datasets WHEN NEW.id='03030303-0303-0303-0303-030303030303' BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END").execute(&mut db).await?;
        assert!(store.index_catalog_mutation(m.id,200).await.is_err());
        let mut reader=store.reader().await?;assert!(!reader.contains_dataset(workspace(),dataset()).await?);reader.close().await?;
        assert_eq!(store.catalog_mutation_state(m.id).await?,Some(MutationState::Prepared));
        sqlx::query("DROP TRIGGER fail_second").execute(&mut db).await?;
        store.catalog_replaced(m.id).await?;store.index_catalog_mutation(m.id,200).await?;
        assert_eq!(store.catalog_mutation_state(m.id).await?,Some(MutationState::Indexed));assert!(store.pending_catalog_mutations().await?.is_empty());
        let mut reader=store.reader().await?;assert!(reader.contains_dataset(workspace(),dataset()).await?);assert_eq!(reader.dataset_version_count(workspace(),dataset()).await?,0);reader.close().await?;
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM dataset_versions").fetch_one(&mut db).await?,0);
        db.close().await?;store.close().await?;Ok(())
    })
}
#[test]
fn invalid_or_corrupt_registry_intent_fails_without_partial_indexing() -> Result {
    runtime()?.block_on(async {
        let dir=ScratchDirectory::new()?;let path=dir.path().join("runtime.sqlite");let mut store=Store::open(&path).await?;store.register_workspace(workspace(),"synthetic-root",100).await?;
        let mut invalid=mutation()?;invalid.index_ids.clear();assert!(store.prepare_catalog_mutation(&invalid).await.is_err());
        let mut invalid=mutation()?;invalid.assignments.push(invalid.assignments[0].clone());assert!(store.prepare_catalog_mutation(&invalid).await.is_err());
        let m=mutation()?;store.prepare_catalog_mutation(&m).await?;
        let mut db=raw(&path).await?;sqlx::query("UPDATE catalog_mutations SET proposed_ids_json='[{\"path\":\"raw/a\",\"id\":\"malformed\"}]'").execute(&mut db).await?;
        assert!(store.pending_catalog_mutations().await.is_err());assert!(store.index_catalog_mutation(m.id,200).await.is_err());
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM datasets").fetch_one(&mut db).await?,0);
        db.close().await?;store.close().await?;Ok(())
    })
}
