//! Actual SQLite snapshot replacement, edit conflicts, retention and migration safety.
#![allow(clippy::unwrap_used, reason = "Synthetic schedule fixtures")]
#[allow(dead_code, reason = "Shared fixture utilities")]
#[path = "../../../tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use sqlx::Connection;
use support::filesystem::ScratchDirectory;
use tf_domain::{
    DatasetId, RequestId, ScheduleId, ScheduleOccurrenceId, SourceSnapshotId, WorkspaceId,
};
use tf_store::{
    Store,
    retention::RetentionPolicy,
    schedules::{Error, Freeze, Save},
};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;
fn ids() -> (WorkspaceId, DatasetId, ScheduleId) {
    (
        "00000001-0000-4000-8000-000000000001".parse().unwrap(),
        "00000002-0000-4000-8000-000000000002".parse().unwrap(),
        ScheduleId::from_bytes([4; 16]),
    )
}
fn definition() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "schedule-definition-fixed")
        .unwrap()["value"]
        .clone()
}
fn save<'a>(v: &'a Value, guard: Option<&'a str>) -> Save<'a> {
    Save {
        id: ids().2,
        workspace: ids().0,
        definition: v,
        expected_etag: guard,
        paused: true,
        actor: "fixture",
        at_us: 100,
        receipt: None,
    }
}
async fn fixture(path: &std::path::Path) -> std::result::Result<Store, Box<dyn std::error::Error>> {
    let mut store = Store::open(path).await?;
    store
        .register_workspace(ids().0, "synthetic-root", 1)
        .await?;
    store.register_dataset(ids().0, ids().1, 2).await?;
    let mut db = support::database::connect(path).await?;
    sqlx::query("INSERT INTO source_snapshots VALUES(?,?,?,'{}',NULL,'{}','{}')")
        .bind(
            definition()["build"]["source"]["snapshot_id"]
                .as_str()
                .unwrap(),
        )
        .bind(ids().0.to_string())
        .bind("a".repeat(64))
        .execute(&mut db)
        .await?;
    db.close().await?;
    Ok(store)
}
#[tokio::test]
async fn static_cycles_are_rejected_atomically_and_branch_roles_remain_independent() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("cycles.sqlite");
    let mut store = fixture(&path).await?;
    let mut first = definition();
    let second_id = ScheduleId::from_bytes([12; 16]);
    first["trigger"] = json!({"kind":"schedule_succeeded","id":"other","schedule_id":second_id.to_string(),"require_materialization":false});
    let saved = store.save_schedule(save(&first, None)).await?;
    let mut second = definition();
    second["name"] = json!("second");
    second["trigger"] = json!({"kind":"schedule_succeeded","id":"first","schedule_id":ids().2.to_string(),"require_materialization":false});
    let mut request = save(&second, None);
    request.id = second_id;
    assert!(
        matches!(store.save_schedule(request).await, Err(Error::Cycle { path }) if path.len()==3)
    );
    let mut reader = store.reader().await?;
    assert!(reader.schedule(ids().0, second_id).await?.is_none());
    assert_eq!(
        reader.schedule(ids().0, ids().2).await?.unwrap()["etag"],
        saved["etag"]
    );
    reader.close().await?;
    let mut self_trigger = definition();
    self_trigger["trigger"] = json!({"kind":"dataset_published","id":"self","dataset":{"workspace_id":ids().0.to_string(),"dataset_id":ids().1.to_string()},"branch":"master","payload_mode":"signal_only","include_resets":false});
    assert!(matches!(
        store
            .save_schedule(save(&self_trigger, saved["etag"].as_str()))
            .await,
        Err(Error::Cycle { .. })
    ));
    // Target and trigger identity may legitimately coincide on different data branches.
    self_trigger["trigger"]["branch"] = json!("feature");
    let current = store
        .save_schedule(save(&self_trigger, saved["etag"].as_str()))
        .await?;
    assert_eq!(current["definition"]["trigger"]["branch"], "feature");
    store.close().await?;
    Ok(())
}
#[tokio::test]
async fn guarded_replacement_keeps_one_snapshot_and_accepted_execution() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let mut store = fixture(&path).await?;
    let v = definition();
    let old = store.save_schedule(save(&v, None)).await?;
    tf_protocol::validate_document("ScheduleRecordV1", &old)?;
    let epoch = old["trigger_epoch"].as_str().unwrap();
    let occurrence = ScheduleOccurrenceId::from_bytes([5; 16]);
    let digest = "b".repeat(64);
    let payload = json!({"event":"synthetic"});
    let freeze = || Freeze {
        workspace: ids().0,
        id: ids().2,
        epoch,
        occurrence,
        evidence: &digest,
        payload: &payload,
        at_us: 110,
    };
    assert!(store.freeze_schedule_occurrence(freeze()).await?);
    assert!(!store.freeze_schedule_occurrence(freeze()).await?);
    let mut db = support::database::connect(&path).await?;
    for (token, consumed) in [
        ("unconsumed", None),
        ("consumed", Some(occurrence.to_string())),
    ] {
        sqlx::query("INSERT INTO trigger_tokens VALUES(?,?,?,'leaf',NULL,'tick','{}',100,NULL,?)")
            .bind(token)
            .bind(ids().2.to_string())
            .bind(epoch)
            .bind(consumed)
            .execute(&mut db)
            .await?;
    }
    let mut newer = v.clone();
    newer["build"]["data_branch"] = json!("feature");
    newer["build"]["fallback_branches"] = json!(["master"]);
    newer["build"]["source"] = json!({"kind":"git_ref","ref":"refs/heads/stable"});
    let receipt = RequestId::from_bytes([6; 16]);
    let receipt_digest = "c".repeat(64);
    let mut request = save(&newer, old["etag"].as_str());
    request.receipt = Some((receipt, &receipt_digest));
    request.paused = false;
    let current = store.save_schedule(request).await?;
    assert_eq!(current["paused"], true);
    assert_ne!(current["etag"], old["etag"]);
    assert_ne!(current["trigger_epoch"], old["trigger_epoch"]);
    assert!(
        matches!(store.save_schedule(save(&v,old["etag"].as_str())).await,Err(Error::Conflict{current_etag:Some(e)}) if e==current["etag"].as_str().unwrap())
    );
    assert!(store.freeze_schedule_occurrence(freeze()).await.is_err());
    let retained: String = sqlx::query_scalar("SELECT execution_json FROM schedule_occurrences")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&retained)?["build"],
        v["build"]
    );
    assert!(
        sqlx::query("UPDATE schedule_occurrences SET execution_json='{}'")
            .execute(&mut db)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schedules")
            .fetch_one(&mut db)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM trigger_tokens")
            .fetch_one(&mut db)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_log WHERE operation='schedule_saved'"
        )
        .fetch_one(&mut db)
        .await?,
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE name='schedule_revisions'"
        )
        .fetch_one(&mut db)
        .await?,
        0
    );
    let roots = store
        .retention_roots(120, RetentionPolicy::default())
        .await?;
    assert!(
        roots.sources.contains(
            &v["build"]["source"]["snapshot_id"]
                .as_str()
                .unwrap()
                .parse::<SourceSnapshotId>()?
        )
    );
    sqlx::query("UPDATE schedule_occurrences SET disposition='SUCCEEDED'")
        .execute(&mut db)
        .await?;
    assert!(
        store
            .retention_roots(121, RetentionPolicy::default())
            .await?
            .sources
            .is_empty()
    );
    let mut rd = store.reader().await?;
    assert!(
        matches!(rd.api_receipt(receipt,&receipt_digest).await?,tf_store::api::Receipt::Complete(r) if r["data"]==current)
    );
    assert_eq!(rd.schedule(ids().0, ids().2).await?, Some(current.clone()));
    assert_eq!(rd.schedules(ids().0, "").await?["schedules"][0], current);
    rd.close().await?;
    db.close().await?;
    store.close().await?;
    let store = Store::open(&path).await?;
    let mut rd = store.reader().await?;
    assert_eq!(rd.schedule(ids().0, ids().2).await?, Some(current));
    rd.close().await?;
    store.close().await?;
    Ok(())
}
#[tokio::test]
async fn invalid_references_and_failed_audit_or_receipt_rollback() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let mut store = fixture(&path).await?;
    let mut v = definition();
    v["build"]["source"]["snapshot_id"] = json!(SourceSnapshotId::from_bytes([9; 16]).to_string());
    assert!(matches!(
        store.save_schedule(save(&v, None)).await,
        Err(Error::Missing)
    ));
    v = definition();
    v["build"]["targets"][0]["workspace_id"] = json!(WorkspaceId::from_bytes([9; 16]).to_string());
    assert!(store.save_schedule(save(&v, None)).await.is_err());
    let mut db = support::database::connect(&path).await?;
    sqlx::query("CREATE TRIGGER fail_schedule_audit BEFORE INSERT ON audit_log WHEN NEW.operation='schedule_saved' BEGIN SELECT RAISE(ABORT,'injected'); END").execute(&mut db).await?;
    v = definition();
    assert!(store.save_schedule(save(&v, None)).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schedules")
            .fetch_one(&mut db)
            .await?,
        0
    );
    sqlx::query("DROP TRIGGER fail_schedule_audit")
        .execute(&mut db)
        .await?;
    let mut request = save(&v, None);
    request.receipt = Some((RequestId::from_bytes([10; 16]), "bad"));
    assert!(store.save_schedule(request).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schedules")
            .fetch_one(&mut db)
            .await?,
        0
    );
    let old = store.save_schedule(save(&v, None)).await?;
    assert!(
        store
            .retention_roots(110, RetentionPolicy::default())
            .await?
            .sources
            .contains(
                &v["build"]["source"]["snapshot_id"]
                    .as_str()
                    .unwrap()
                    .parse()?
            )
    );
    sqlx::query("UPDATE schedules SET deleted_at_us=100")
        .execute(&mut db)
        .await?;
    assert!(matches!(
        store.save_schedule(save(&v, None)).await,
        Err(Error::Conflict { .. })
    ));
    assert!(
        store
            .save_schedule(save(&v, old["etag"].as_str()))
            .await
            .is_err()
    );
    db.close().await?;
    store.close().await?;
    Ok(())
}
#[tokio::test]
async fn legacy_upgrade_removes_definition_history_but_preserves_consumed_evidence() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let mut db = support::database::connect(&path).await?;
    sqlx::query(
        "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,checksum TEXT NOT NULL) STRICT",
    )
    .execute(&mut db)
    .await?;
    let mut paths: Vec<_> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))?
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    for (n, path) in paths.iter().take(12).enumerate() {
        let bytes = std::fs::read_to_string(path)?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(bytes.as_str()))
            .execute(&mut db)
            .await?;
        sqlx::query("INSERT INTO schema_migrations VALUES(?,?)")
            .bind((n + 1) as i64)
            .bind(tf_protocol::canonical::file_digest(&mut bytes.as_bytes())?.hex())
            .execute(&mut db)
            .await?;
    }
    sqlx::raw_sql("PRAGMA application_id=1414678092; PRAGMA user_version=12;")
        .execute(&mut db)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,root_identity,created_at_us) VALUES(?,'synthetic',1)")
        .bind(ids().0.to_string())
        .execute(&mut db)
        .await?;
    sqlx::query("INSERT INTO schedules VALUES(?,'legacy',NULL,1,NULL)")
        .bind(ids().2.to_string())
        .execute(&mut db)
        .await?;
    for n in 1..=2 {
        sqlx::query("INSERT INTO schedule_revisions VALUES(?,?,'{}',?,'{}','fixture',1)")
            .bind(ids().2.to_string())
            .bind(n)
            .bind(
                json!({"source":{"kind":"git_ref","ref":format!("refs/heads/revision_{n}")}})
                    .to_string(),
            )
            .execute(&mut db)
            .await?;
    }
    sqlx::query("UPDATE schedules SET active_revision=2")
        .execute(&mut db)
        .await?;
    sqlx::query(
        "INSERT INTO schedule_occurrences VALUES('occurrence',?,1,'evidence',1,'{}','QUEUED',NULL)",
    )
    .bind(ids().2.to_string())
    .execute(&mut db)
    .await?;
    sqlx::query("INSERT INTO trigger_tokens VALUES('token',?,1,'leaf',NULL,'tick','{}',1,NULL,'occurrence')").bind(ids().2.to_string()).execute(&mut db).await?;
    db.close().await?;
    Store::open(&path).await?.close().await?;
    let backups: Vec<_> = std::fs::read_dir(dir.path())?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("schedule-schema-12-backup-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    let mut backup = support::database::connect(&backups[0].path()).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schedule_revisions")
            .fetch_one(&mut backup)
            .await?,
        2
    );
    backup.close().await?;
    let mut db = support::database::connect(&path).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pragma_foreign_key_check")
            .fetch_one(&mut db)
            .await?,
        0
    );
    let encoded: String = sqlx::query_scalar("SELECT definition_json FROM schedules")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&encoded)?["build"]["source"]["ref"],
        "refs/heads/revision_2"
    );
    let encoded: String = sqlx::query_scalar("SELECT execution_json FROM schedule_occurrences")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&encoded)?["build"]["source"]["ref"],
        "refs/heads/revision_1"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT needs_review FROM schedules")
            .fetch_one(&mut db)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by='occurrence'"
        )
        .fetch_one(&mut db)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE name='schedule_revisions'"
        )
        .fetch_one(&mut db)
        .await?,
        0
    );
    db.close().await?;
    Ok(())
}

#[tokio::test]
async fn large_definitions_page_below_http_budget_with_continuation() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let mut store = fixture(&path).await?;
    for n in 20..27 {
        let mut v = definition();
        v["name"] = json!(format!("Synthetic {n}"));
        v["build"]["parameters"] = json!({"synthetic": "x".repeat(200_000)});
        let mut request = save(&v, None);
        request.id = ScheduleId::from_bytes([n; 16]);
        store.save_schedule(request).await?;
    }
    let mut rd = store.reader().await?;
    let first = rd.schedules(ids().0, "").await?;
    assert!(serde_json::to_vec(&first)?.len() < 2 * 1024 * 1024);
    assert_eq!(first["schedules"].as_array().unwrap().len(), 5);
    assert!(first["next_cursor"].is_string());
    let next = rd
        .schedules(ids().0, first["next_cursor"].as_str().unwrap())
        .await?;
    assert_eq!(next["schedules"].as_array().unwrap().len(), 2);
    assert!(next["next_cursor"].is_null());
    assert!(first["schedules"].as_array().unwrap().iter().all(|a| {
        next["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| a["id"] != b["id"])
    }));
    tf_protocol::validate_document("ApiSchedulesV1", &first)?;
    rd.close().await?;
    store.close().await?;
    Ok(())
}
