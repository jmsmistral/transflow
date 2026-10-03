//! Real SQLite lifecycle fencing, frozen policies, manual receipts and bounded replay.
#![allow(clippy::unwrap_used, reason = "Synthetic scheduling fixtures")]
#[allow(dead_code, reason = "Shared fixture utilities")]
#[path = "../../../tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection};
use support::filesystem::ScratchDirectory;
use tf_domain::{DatasetId, RequestId, ScheduleId, ScheduleOccurrenceId, VersionId, WorkspaceId};
use tf_store::{
    Store,
    retention::RetentionPolicy,
    schedule_lifecycle::{Action, Control, Replay},
    schedules::{Error, Freeze, Save},
};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;
fn workspace() -> WorkspaceId {
    "00000001-0000-4000-8000-000000000001".parse().unwrap()
}
fn dataset() -> DatasetId {
    "00000002-0000-4000-8000-000000000002".parse().unwrap()
}
fn sid(n: u8) -> ScheduleId {
    ScheduleId::from_bytes([n; 16])
}
fn branch() -> String {
    "00000003-0000-4000-8000-000000000003".into()
}
fn version(n: u8) -> VersionId {
    VersionId::from_bytes([n; 16])
}
fn definition(n: u8, trigger: Value) -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json")).unwrap();
    let mut v = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "schedule-definition-fixed")
        .unwrap()["value"]
        .clone();
    v["name"] = json!(format!("schedule-{n}"));
    v["trigger"] = trigger;
    v
}
fn leaf(kind: &str, id: &str, mode: &str, resets: bool) -> Value {
    json!({"kind":kind,"id":id,"dataset":{"workspace_id":workspace().to_string(),"dataset_id":dataset().to_string()},"branch":"master","payload_mode":mode,"include_resets":resets})
}
async fn fixture(
    path: &std::path::Path,
) -> std::result::Result<(Store, SqliteConnection), Box<dyn std::error::Error>> {
    let mut s = Store::open(path).await?;
    s.register_workspace(workspace(), "synthetic-root", 1)
        .await?;
    s.register_dataset(workspace(), dataset(), 2).await?;
    let mut db = support::database::connect(path).await?;
    let source = definition(5, json!({"kind":"manual"}))["build"]["source"]["snapshot_id"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query("INSERT INTO source_snapshots VALUES(?,?,?,'{}',NULL,'{}','{}')")
        .bind(source)
        .bind(workspace().to_string())
        .bind("a".repeat(64))
        .execute(&mut db)
        .await?;
    sqlx::query("INSERT INTO data_branches VALUES(?,?,'master',NULL,0,NULL)")
        .bind(branch())
        .bind(workspace().to_string())
        .execute(&mut db)
        .await?;
    Ok((s, db))
}
async fn save(
    s: &mut Store,
    n: u8,
    v: &Value,
    paused: bool,
) -> std::result::Result<Value, Box<dyn std::error::Error>> {
    Ok(s.save_schedule(Save {
        id: sid(n),
        workspace: workspace(),
        definition: v,
        expected_etag: None,
        paused,
        actor: "fixture",
        at_us: 10,
        receipt: None,
    })
    .await?)
}
async fn publish(
    db: &mut SqliteConnection,
    n: u8,
    generation: i64,
    cause: &str,
    at: i64,
) -> Result {
    let digest = format!("{n:064x}");
    let source = definition(5, json!({"kind":"manual"}))["build"]["source"]["snapshot_id"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query(
        "INSERT INTO artifacts VALUES(?,'{}','[]',0,0,0,'VERIFIED') ON CONFLICT DO NOTHING",
    )
    .bind(&digest)
    .execute(&mut *db)
    .await?;
    sqlx::query("INSERT INTO dataset_versions VALUES(?,?,?,NULL,?,?,?,'compute','check') ON CONFLICT DO NOTHING").bind(version(n).to_string()).bind(dataset().to_string()).bind(&digest).bind(format!("import-{n}")).bind(source).bind(at).execute(&mut *db).await?;
    let event = tf_domain::RequestId::from_bytes([n + 50; 16]).to_string();
    let mut tx = db.begin().await?;
    sqlx::query("INSERT INTO events(id,type,payload_json,causation_id,correlation_id,wall_time_us) VALUES(?,?,?,'cause','correlation',?)").bind(&event).bind(if cause=="publication" {"dataset.published"} else {"dataset.head_changed"}).bind(json!({"version_id":version(n).to_string(),"branch":"master"}).to_string()).bind(at).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO head_changes VALUES(?,?,?,NULL,?,?,?,?)")
        .bind(event)
        .bind(branch())
        .bind(dataset().to_string())
        .bind(version(n).to_string())
        .bind(generation)
        .bind(cause)
        .bind(at)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO dataset_heads VALUES(?,?,?,?) ON CONFLICT(branch_id,dataset_id) DO UPDATE SET version_id=excluded.version_id,generation=excluded.generation").bind(branch()).bind(dataset().to_string()).bind(version(n).to_string()).bind(generation).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
async fn scan(s: &mut Store, epoch: &str, now: i64) -> Result {
    s.scan_schedule_events(workspace(), sid(5), epoch, 100, now)
        .await?;
    Ok(())
}
async fn count(
    db: &mut SqliteConnection,
    query: &'static str,
) -> std::result::Result<i64, sqlx::Error> {
    sqlx::query_scalar(query).fetch_one(db).await
}
async fn control(
    s: &mut Store,
    current: &Value,
    key: u8,
    action: Action,
    at: i64,
) -> std::result::Result<Value, Error> {
    let digest = format!("{key:064x}");
    Ok(s.control_schedule(Control {
        workspace: workspace(),
        id: sid(5),
        etag: current["etag"].as_str().unwrap(),
        key: RequestId::from_bytes([key; 16]),
        digest: &digest,
        actor: "fixture",
        at_us: at,
        action,
    })
    .await?["data"]
        .clone())
}
async fn freeze(s: &mut Store, current: &Value, n: u8, at: i64, payload: &Value) -> Result {
    s.freeze_schedule_occurrence(Freeze {
        workspace: workspace(),
        id: sid(5),
        epoch: current["trigger_epoch"].as_str().unwrap(),
        occurrence: ScheduleOccurrenceId::from_bytes([n; 16]),
        evidence: &format!("{n:064x}"),
        payload,
        at_us: at,
    })
    .await?;
    Ok(())
}
#[tokio::test]
async fn manual_while_paused_freezes_settings_without_consuming_tokens_and_retries_after_restart()
-> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("db");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(5, leaf("dataset_published", "leaf", "pin", false));
    let original = save(&mut s, 5, &d, false).await?;
    publish(&mut db, 1, 1, "publication", 20).await?;
    scan(&mut s, original["trigger_epoch"].as_str().unwrap(), 20).await?;
    freeze(&mut s, &original, 30, 21, &json!({})).await?;
    freeze(&mut s, &original, 31, 22, &json!({})).await?;
    sqlx::query("UPDATE schedule_occurrences SET disposition='RUNNING' WHERE id=?")
        .bind(ScheduleOccurrenceId::from_bytes([31; 16]).to_string())
        .execute(&mut db)
        .await?;
    let paused = control(&mut s, &original, 40, Action::Pause, 25).await?;
    assert_eq!(paused["paused"], true);
    assert_ne!(paused["etag"], original["etag"]);
    assert_eq!(paused["trigger_epoch"], original["trigger_epoch"]);
    let manual = control(&mut s, &paused, 41, Action::Run, 26).await?;
    tf_protocol::validate_document("ScheduleRunV1", &manual)?;
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM schedule_occurrences WHERE disposition='HELD'"
        )
        .await?,
        1
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM schedule_occurrences WHERE disposition='RUNNING'"
        )
        .await?,
        1
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NULL"
        )
        .await?,
        1
    );
    assert_eq!(count(&mut db, "SELECT count(*) FROM builds").await?, 0);
    let frozen: String =
        sqlx::query_scalar("SELECT execution_json FROM schedule_occurrences WHERE id=?")
            .bind(manual["id"].as_str())
            .fetch_one(&mut db)
            .await?;
    assert_eq!(serde_json::from_str::<Value>(&frozen)?["build"], d["build"]);
    let mut newer = d.clone();
    newer["build"]["data_branch"] = json!("feature");
    newer["build"]["source"] = json!({"kind":"git_ref","ref":"refs/heads/stable"});
    s.save_schedule(Save {
        id: sid(5),
        workspace: workspace(),
        definition: &newer,
        expected_etag: paused["etag"].as_str(),
        paused: false,
        actor: "fixture",
        at_us: 30,
        receipt: None,
    })
    .await?;
    assert!(
        s.retention_roots(30, RetentionPolicy::default())
            .await?
            .sources
            .contains(
                &d["build"]["source"]["snapshot_id"]
                    .as_str()
                    .unwrap()
                    .parse()?
            )
    );
    s.close().await?;
    let mut s = Store::open(&path).await?;
    assert_eq!(
        control(&mut s, &paused, 41, Action::Run, 999).await?,
        manual
    );
    assert!(matches!(
        control(&mut s, &paused, 42, Action::Run, 31).await,
        Err(Error::Conflict { .. })
    ));
    let digest = "a".repeat(64);
    assert!(matches!(
        s.control_schedule(Control {
            workspace: workspace(),
            id: sid(5),
            etag: paused["etag"].as_str().unwrap(),
            key: RequestId::from_bytes([41; 16]),
            digest: &digest,
            actor: "fixture",
            at_us: 31,
            action: Action::Run
        })
        .await,
        Err(Error::Conflict { .. })
    ));
    assert_eq!(count(&mut db,"SELECT count(*) FROM schedule_occurrences WHERE json_extract(payload_json,'$.manual')=1").await?,1);
    let mut rd = s.reader().await?;
    assert_eq!(
        rd.schedule(workspace(), sid(5)).await?.unwrap()["paused"],
        true
    );
    rd.close().await?;
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn delayed_scan_distinguishes_pre_pause_paused_and_post_resume_events() -> Result {
    let dir = ScratchDirectory::new()?;
    let (mut s, mut db) = fixture(&dir.path().join("db")).await?;
    let d = definition(5, leaf("dataset_published", "leaf", "pin", false));
    let original = save(&mut s, 5, &d, false).await?;
    publish(&mut db, 1, 1, "publication", 20).await?;
    let paused = control(&mut s, &original, 40, Action::Pause, 30).await?;
    publish(&mut db, 2, 2, "publication", 40).await?;
    let resumed = control(&mut s, &paused, 41, Action::Resume(None), 50).await?;
    publish(&mut db, 3, 3, "publication", 60).await?;
    s.close().await?;
    let mut s = Store::open(&dir.path().join("db")).await?;
    let scan = s
        .scan_schedule_events(
            workspace(),
            sid(5),
            resumed["trigger_epoch"].as_str().unwrap(),
            100,
            60,
        )
        .await?;
    assert_eq!((scan.scanned, scan.tokens, scan.ignored), (3, 2, 1));
    assert_eq!(count(&mut db,"SELECT count(*) FROM trigger_tokens WHERE json_extract(payload_json,'$.dataset.version_id') IS NOT NULL").await?,2);
    let ignored:i64=sqlx::query_scalar("SELECT count(*) FROM trigger_tokens WHERE json_extract(payload_json,'$.dataset.version_id')=?").bind(version(2).to_string()).fetch_one(&mut db).await?;
    assert_eq!(ignored, 0);
    assert!(
        s.queue_schedule_occurrence(
            workspace(),
            sid(5),
            resumed["trigger_epoch"].as_str().unwrap(),
            60
        )
        .await?
        .is_some()
    );
    assert!(
        s.queue_schedule_occurrence(
            workspace(),
            sid(5),
            resumed["trigger_epoch"].as_str().unwrap(),
            60
        )
        .await?
        .is_none()
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn resume_uses_frozen_queue_skip_and_coalesce_policies_across_definition_edits() -> Result {
    for (policy, queued, coalesced, skipped) in [
        ("queue", 2, 0, 0),
        ("coalesce_latest", 1, 1, 0),
        ("skip", 0, 0, 2),
    ] {
        let dir = ScratchDirectory::new()?;
        let (mut s, mut db) = fixture(&dir.path().join("db")).await?;
        let mut d = definition(5, json!({"kind":"manual"}));
        d["policies"]["overlap_policy"] = json!(policy);
        let original = save(&mut s, 5, &d, false).await?;
        freeze(&mut s, &original, 30, 20, &json!({})).await?;
        freeze(&mut s, &original, 31, 21, &json!({})).await?;
        let paused = control(&mut s, &original, 40, Action::Pause, 25).await?;
        let mut newer = d.clone();
        newer["policies"]["overlap_policy"] = json!("skip");
        newer["build"]["data_branch"] = json!("feature");
        let edited = s
            .save_schedule(Save {
                id: sid(5),
                workspace: workspace(),
                definition: &newer,
                expected_etag: paused["etag"].as_str(),
                paused: false,
                actor: "fixture",
                at_us: 26,
                receipt: None,
            })
            .await?;
        let resumed = control(&mut s, &edited, 41, Action::Resume(None), 30).await?;
        assert_eq!(resumed["paused"], false);
        assert_eq!(resumed["trigger_epoch"], edited["trigger_epoch"]);
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM schedule_occurrences WHERE disposition='QUEUED'"
            )
            .await?,
            queued
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM schedule_occurrences WHERE disposition='COALESCED'"
            )
            .await?,
            coalesced
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM schedule_occurrences WHERE disposition='SKIPPED'"
            )
            .await?,
            skipped
        );
        let executions: Vec<String> =
            sqlx::query_scalar("SELECT execution_json FROM schedule_occurrences")
                .fetch_all(&mut db)
                .await?;
        for execution in executions {
            assert_eq!(
                serde_json::from_str::<Value>(&execution)?["build"],
                d["build"]
            );
        }
        assert_eq!(count(&mut db, "SELECT count(*) FROM schedules").await?, 1);
        db.close().await?;
        s.close().await?;
    }
    Ok(())
}
#[tokio::test]
async fn resume_expiry_uses_earliest_original_token_and_frozen_window() -> Result {
    let dir = ScratchDirectory::new()?;
    let (mut s, mut db) = fixture(&dir.path().join("db")).await?;
    let mut d = definition(5, json!({"kind":"manual"}));
    d["policies"]["token_window_seconds"] = json!(1);
    let original = save(&mut s, 5, &d, false).await?;
    freeze(
        &mut s,
        &original,
        30,
        100,
        &json!({"tokens":[{"occurred_at_us":"0"},{"occurred_at_us":"100"}]}),
    )
    .await?;
    let paused = control(&mut s, &original, 40, Action::Pause, 500).await?;
    let mut newer = d.clone();
    newer["policies"]["token_window_seconds"] = json!(86400);
    let edited = s
        .save_schedule(Save {
            id: sid(5),
            workspace: workspace(),
            definition: &newer,
            expected_etag: paused["etag"].as_str(),
            paused: false,
            actor: "fixture",
            at_us: 600,
            receipt: None,
        })
        .await?;
    control(&mut s, &edited, 41, Action::Resume(None), 1_000_000).await?;
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM schedule_occurrences WHERE disposition='EXPIRED'"
        )
        .await?,
        1
    );
    let audit: String = sqlx::query_scalar(
        "SELECT evidence_json FROM audit_log WHERE operation='schedule_resumed'",
    )
    .fetch_one(&mut db)
    .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&audit)?["changes"][0]["reason"],
        "resume_expiry"
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn explicit_bounded_replay_creates_new_audited_occurrence_and_preserves_original_pins()
-> Result {
    let dir = ScratchDirectory::new()?;
    let (mut s, mut db) = fixture(&dir.path().join("db")).await?;
    let d = definition(5, leaf("dataset_published", "leaf", "pin", false));
    let paused = save(&mut s, 5, &d, true).await?;
    publish(&mut db, 1, 1, "publication", 20).await?;
    let report = s
        .scan_schedule_events(
            workspace(),
            sid(5),
            paused["trigger_epoch"].as_str().unwrap(),
            100,
            20,
        )
        .await?;
    assert_eq!(report.ignored, 1);
    let replayed = control(
        &mut s,
        &paused,
        40,
        Action::Resume(Some(Replay { after: 0, limit: 1 })),
        30,
    )
    .await?;
    assert_ne!(replayed["trigger_epoch"], paused["trigger_epoch"]);
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM schedule_occurrences WHERE disposition='QUEUED'"
        )
        .await?,
        1
    );
    let payload: String = sqlx::query_scalar("SELECT payload_json FROM schedule_occurrences")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&payload)?["input_pins"][0]["version_id"],
        version(1).to_string()
    );
    assert_eq!(
        control(
            &mut s,
            &paused,
            40,
            Action::Resume(Some(Replay { after: 0, limit: 1 })),
            50
        )
        .await?,
        replayed
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM audit_log WHERE operation='schedule_replayed'"
        )
        .await?,
        1
    );
    let snapshot = s.schedule_clock(workspace(), sid(5)).await?;
    assert_eq!(snapshot.evaluation.cursor.at_us, 30);
    let mut newer = d.clone();
    newer["description"] = json!("new description");
    s.save_schedule_with_replay(
        Save {
            id: sid(5),
            workspace: workspace(),
            definition: &newer,
            expected_etag: replayed["etag"].as_str(),
            paused: false,
            actor: "fixture",
            at_us: 40,
            receipt: None,
        },
        Some(Replay { after: 0, limit: 1 }),
    )
    .await?;
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        2
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn invalid_replay_or_full_manual_queue_rolls_back_state_and_receipt() -> Result {
    let dir = ScratchDirectory::new()?;
    let (mut s, mut db) = fixture(&dir.path().join("db")).await?;
    let mut d = definition(5, json!({"kind":"manual"}));
    d["policies"]["max_pending"] = json!(1);
    let paused = save(&mut s, 5, &d, true).await?;
    for n in 1..=2 {
        publish(&mut db, n, i64::from(n), "publication", 20 + i64::from(n)).await?;
    }
    assert!(matches!(
        control(
            &mut s,
            &paused,
            40,
            Action::Resume(Some(Replay { after: 0, limit: 1 })),
            30
        )
        .await,
        Err(Error::Limit)
    ));
    let mut rd = s.reader().await?;
    assert_eq!(
        rd.schedule(workspace(), sid(5)).await?,
        Some(paused.clone())
    );
    rd.close().await?;
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM schedule_pause_intervals WHERE ended_at_us IS NOT NULL"
        )
        .await?,
        0
    );
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM api_operations").await?,
        0
    );
    control(&mut s, &paused, 41, Action::Run, 31).await?;
    assert!(matches!(
        control(&mut s, &paused, 42, Action::Run, 32).await,
        Err(Error::Limit)
    ));
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        1
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn pause_and_manual_write_failures_rollback_each_mutation_and_retry_once() -> Result {
    for trigger in [
        "CREATE TRIGGER fail BEFORE INSERT ON schedule_pause_intervals BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE UPDATE ON schedule_occurrences BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE UPDATE ON schedules BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON audit_log BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON api_operations BEGIN SELECT RAISE(ABORT,'injected'); END",
    ] {
        let dir = ScratchDirectory::new()?;
        let (mut s, mut db) = fixture(&dir.path().join("db")).await?;
        let d = definition(5, json!({"kind":"manual"}));
        let current = save(&mut s, 5, &d, false).await?;
        freeze(&mut s, &current, 30, 20, &json!({})).await?;
        sqlx::query(trigger).execute(&mut db).await?;
        assert!(
            control(&mut s, &current, 40, Action::Pause, 30)
                .await
                .is_err()
        );
        let mut rd = s.reader().await?;
        assert_eq!(
            rd.schedule(workspace(), sid(5)).await?,
            Some(current.clone())
        );
        rd.close().await?;
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM schedule_pause_intervals").await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM schedule_occurrences WHERE disposition='ACCEPTED'"
            )
            .await?,
            1
        );
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM api_operations").await?,
            0
        );
        sqlx::query("DROP TRIGGER fail").execute(&mut db).await?;
        let paused = control(&mut s, &current, 40, Action::Pause, 30).await?;
        assert_eq!(
            control(&mut s, &current, 40, Action::Pause, 31).await?,
            paused
        );
        for trigger in [
            "CREATE TRIGGER fail BEFORE INSERT ON schedule_occurrences BEGIN SELECT RAISE(ABORT,'injected'); END",
            "CREATE TRIGGER fail BEFORE INSERT ON audit_log BEGIN SELECT RAISE(ABORT,'injected'); END",
            "CREATE TRIGGER fail BEFORE INSERT ON api_operations BEGIN SELECT RAISE(ABORT,'injected'); END",
        ] {
            sqlx::query(trigger).execute(&mut db).await?;
            assert!(control(&mut s, &paused, 41, Action::Run, 32).await.is_err());
            assert_eq!(count(&mut db,"SELECT count(*) FROM schedule_occurrences WHERE json_extract(payload_json,'$.manual')=1").await?,0);
            sqlx::query("DROP TRIGGER fail").execute(&mut db).await?;
        }
        control(&mut s, &paused, 41, Action::Run, 32).await?;
        db.close().await?;
        s.close().await?;
    }
    Ok(())
}
#[tokio::test]
async fn concurrent_controls_and_old_edit_guards_cannot_overwrite_operational_state() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("db");
    let (mut a, mut db) = fixture(&path).await?;
    let d = definition(5, json!({"kind":"manual"}));
    let original = save(&mut a, 5, &d, false).await?;
    let mut b = Store::open(&path).await?;
    let (first, second) = tokio::join!(
        control(&mut a, &original, 40, Action::Pause, 30),
        control(&mut b, &original, 41, Action::Pause, 30)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let paused = first.or(second)?;
    assert!(matches!(
        a.save_schedule(Save {
            id: sid(5),
            workspace: workspace(),
            definition: &d,
            expected_etag: original["etag"].as_str(),
            paused: false,
            actor: "fixture",
            at_us: 31,
            receipt: None
        })
        .await,
        Err(Error::Conflict { .. })
    ));
    assert_eq!(paused["paused"], true);
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_pause_intervals").await?,
        1
    );
    db.close().await?;
    a.close().await?;
    b.close().await?;
    Ok(())
}
#[tokio::test]
async fn schema_fifteen_upgrade_keeps_definitions_and_fences_initially_paused_evidence() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("db");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(5, leaf("dataset_published", "leaf", "signal_only", false));
    let paused = save(&mut s, 5, &d, true).await?;
    s.close().await?;
    sqlx::raw_sql("DROP TABLE schedule_pause_intervals; DELETE FROM schema_migrations WHERE version=16; PRAGMA user_version=15;").execute(&mut db).await?;
    publish(&mut db, 1, 1, "publication", 20).await?;
    let mut s = Store::open(&path).await?;
    assert_eq!(s.info().schema_version, 16);
    let resumed = control(&mut s, &paused, 40, Action::Resume(None), 30).await?;
    assert_eq!(resumed["definition"], d);
    let report = s
        .scan_schedule_events(
            workspace(),
            sid(5),
            resumed["trigger_epoch"].as_str().unwrap(),
            100,
            30,
        )
        .await?;
    assert_eq!((report.tokens, report.ignored), (0, 1));
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn resume_replay_failures_rollback_windows_held_work_tokens_epoch_audit_and_receipt() -> Result
{
    for trigger in [
        "CREATE TRIGGER fail BEFORE UPDATE ON schedule_pause_intervals BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE UPDATE ON schedule_occurrences BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE UPDATE ON schedules BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON trigger_tokens BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON schedule_occurrences BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON schedule_clock_state BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON audit_log WHEN NEW.operation='schedule_replayed' BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON api_operations BEGIN SELECT RAISE(ABORT,'injected'); END",
    ] {
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("db");
        let (mut s, mut db) = fixture(&path).await?;
        let d = definition(5, leaf("dataset_published", "leaf", "pin", false));
        let original = save(&mut s, 5, &d, false).await?;
        freeze(&mut s, &original, 30, 20, &json!({})).await?;
        let paused = control(&mut s, &original, 40, Action::Pause, 21).await?;
        publish(&mut db, 1, 1, "publication", 22).await?;
        sqlx::query(trigger).execute(&mut db).await?;
        assert!(
            control(
                &mut s,
                &paused,
                41,
                Action::Resume(Some(Replay { after: 0, limit: 1 })),
                30
            )
            .await
            .is_err()
        );
        let mut rd = s.reader().await?;
        assert_eq!(
            rd.schedule(workspace(), sid(5)).await?,
            Some(paused.clone())
        );
        rd.close().await?;
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM schedule_occurrences WHERE disposition='HELD'"
            )
            .await?,
            1
        );
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM trigger_tokens").await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM schedule_pause_intervals WHERE ended_at_us IS NOT NULL"
            )
            .await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM audit_log WHERE operation='schedule_replayed'"
            )
            .await?,
            0
        );
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM api_operations").await?,
            1
        );
        sqlx::query("DROP TRIGGER fail").execute(&mut db).await?;
        s.close().await?;
        let mut s = Store::open(&path).await?;
        let resumed = control(
            &mut s,
            &paused,
            41,
            Action::Resume(Some(Replay { after: 0, limit: 1 })),
            30,
        )
        .await?;
        assert_eq!(
            control(
                &mut s,
                &paused,
                41,
                Action::Resume(Some(Replay { after: 0, limit: 1 })),
                31
            )
            .await?,
            resumed
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM schedule_occurrences WHERE disposition='QUEUED'"
            )
            .await?,
            2
        );
        db.close().await?;
        s.close().await?;
    }
    Ok(())
}
