//! Real SQLite atomic trigger selection, queueing, retention and failure recovery.
#![allow(clippy::unwrap_used, reason = "Synthetic scheduling fixtures")]
#[allow(dead_code, reason = "Shared fixture utilities")]
#[path = "../../../tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection};
use support::filesystem::ScratchDirectory;
use tf_domain::{DatasetId, ScheduleId, VersionId, WorkspaceId};
use tf_store::{Store, retention::RetentionPolicy, schedule_evaluation::Error, schedules::Save};
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
#[tokio::test]
async fn nested_selection_and_or_remainder_are_not_global_debounce() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let mut d = definition(
        5,
        json!({"kind":"or","children":[leaf("dataset_published","a","signal_only",false),leaf("dataset_head_changed","b","signal_only",false)]}),
    );
    d["policies"]["max_pending"] = json!(2);
    d["policies"]["overlap_policy"] = json!("queue");
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    publish(&mut db, 11, 2, "publication", 30).await?;
    publish(&mut db, 12, 3, "cache_reuse", 40).await?;
    scan(&mut s, epoch, 50).await?;
    let first = s
        .queue_schedule_occurrence(workspace(), sid(5), epoch, 50)
        .await?
        .unwrap();
    assert_eq!(
        (first.ready_at_us, first.selected, first.coalesced),
        (30, 1, 1)
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NULL"
        )
        .await?,
        3
    );
    let second = s
        .queue_schedule_occurrence(workspace(), sid(5), epoch, 50)
        .await?
        .unwrap();
    assert_eq!(
        (second.ready_at_us, second.selected, second.coalesced),
        (40, 1, 2)
    );
    assert_ne!(first.occurrence, second.occurrence);
    assert!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 50)
            .await?
            .is_none()
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM schedule_occurrences WHERE disposition='QUEUED'"
        )
        .await?,
        2
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NULL"
        )
        .await?,
        0
    );
    assert_eq!(count(&mut db, "SELECT count(*) FROM builds").await?, 0);
    let pins: i64 = sqlx::query_scalar(
        "SELECT sum(json_array_length(payload_json,'$.input_pins')) FROM schedule_occurrences",
    )
    .fetch_one(&mut db)
    .await?;
    assert_eq!(pins, 0);
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn and_coalesces_only_selected_leaves_and_freezes_exact_pins() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(
        5,
        json!({"kind":"and","children":[{"kind":"or","children":[leaf("dataset_published","a","pin",false),leaf("dataset_head_changed","b","pin",false)]},leaf("dataset_published","c","pin",false)]}),
    );
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    assert!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 10)
            .await?
            .is_none()
    );
    publish(&mut db, 10, 1, "publication", 20).await?;
    publish(&mut db, 11, 2, "publication", 30).await?;
    scan(&mut s, epoch, 40).await?;
    let queued = s
        .queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
        .await?
        .unwrap();
    assert_eq!(
        (queued.selected, queued.coalesced, queued.ready_at_us),
        (2, 2, 30)
    );
    let payload: String =
        sqlx::query_scalar("SELECT payload_json FROM schedule_occurrences WHERE id=?")
            .bind(queued.occurrence.to_string())
            .fetch_one(&mut db)
            .await?;
    let payload: Value = serde_json::from_str(&payload)?;
    assert_eq!(payload["input_pins"].as_array().unwrap().len(), 1);
    assert_eq!(
        payload["input_pins"][0]["version_id"],
        version(11).to_string()
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE leaf_id='b' AND consumed_by IS NULL"
        )
        .await?,
        2
    );
    publish(&mut db, 12, 3, "publication", 50).await?;
    let policy = RetentionPolicy {
        latest: 1,
        age_us: 0,
    };
    let roots = s.retention_roots(1_000_000_000_000, policy).await?;
    assert!(roots.versions.contains(&version(11)));
    let mut edited = d.clone();
    edited["build"]["force"] = json!(true);
    s.save_schedule(Save {
        id: sid(5),
        workspace: workspace(),
        definition: &edited,
        expected_etag: saved["etag"].as_str(),
        paused: false,
        actor: "fixture",
        at_us: 60,
        receipt: None,
    })
    .await?;
    let execution: String =
        sqlx::query_scalar("SELECT execution_json FROM schedule_occurrences WHERE id=?")
            .bind(queued.occurrence.to_string())
            .fetch_one(&mut db)
            .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&execution)?["build"]["force"],
        d["build"]["force"]
    );
    assert!(matches!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 1_000_000_000_001)
            .await,
        Err(Error::Conflict)
    ));
    let roots = s.retention_roots(1_000_000_000_002, policy).await?;
    assert!(roots.versions.contains(&version(11)));
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn failures_rollback_queue_consumption_and_audit_then_reopen_once() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(5, leaf("dataset_published", "a", "pin", false));
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    scan(&mut s, epoch, 30).await?;
    for failure in [
        "CREATE TRIGGER injected BEFORE INSERT ON schedule_occurrences BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;",
        "CREATE TRIGGER injected BEFORE UPDATE OF consumed_by ON trigger_tokens BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;",
        "CREATE TRIGGER injected BEFORE INSERT ON audit_log WHEN NEW.operation='schedule_occurrence_queued' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;",
    ] {
        sqlx::raw_sql(failure).execute(&mut db).await?;
        assert!(
            s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
                .await
                .is_err()
        );
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NOT NULL"
            )
            .await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM audit_log WHERE operation='schedule_occurrence_queued'"
            )
            .await?,
            0
        );
        sqlx::raw_sql("DROP TRIGGER injected")
            .execute(&mut db)
            .await?;
    }
    s.close().await?;
    s = Store::open(&path).await?;
    let first = s
        .queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
        .await?
        .unwrap();
    s.close().await?;
    s = Store::open(&path).await?;
    assert!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 41)
            .await?
            .is_none()
    );
    let retained: String = sqlx::query_scalar("SELECT id FROM schedule_occurrences")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(retained, first.occurrence.to_string());
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM audit_log WHERE operation='schedule_occurrence_queued'"
        )
        .await?,
        1
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn simultaneous_owners_enqueue_one_occurrence() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(5, leaf("dataset_published", "a", "pin", false));
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    scan(&mut s, epoch, 30).await?;
    let mut other = Store::open(&path).await?;
    let (a, b) = tokio::join!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40),
        other.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
    );
    assert_eq!(usize::from(a?.is_some()) + usize::from(b?.is_some()), 1);
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        1
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NOT NULL"
        )
        .await?,
        1
    );
    other.close().await?;
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn expired_paused_and_full_queue_do_not_consume_new_evidence() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let mut d = definition(5, leaf("dataset_published", "a", "pin", false));
    d["policies"]["token_window_seconds"] = json!(1);
    d["policies"]["max_pending"] = json!(1);
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    scan(&mut s, epoch, 30).await?;
    assert!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 1_000_020)
            .await?
            .is_none()
    );
    publish(&mut db, 11, 2, "publication", 1_000_030).await?;
    scan(&mut s, epoch, 1_000_040).await?;
    sqlx::query("UPDATE schedules SET paused=1")
        .execute(&mut db)
        .await?;
    assert!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 1_000_050)
            .await?
            .is_none()
    );
    sqlx::query("UPDATE schedules SET paused=0")
        .execute(&mut db)
        .await?;
    s.queue_schedule_occurrence(workspace(), sid(5), epoch, 1_000_060)
        .await?
        .unwrap();
    publish(&mut db, 12, 3, "publication", 1_000_070).await?;
    scan(&mut s, epoch, 1_000_080).await?;
    assert!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 1_000_090)
            .await?
            .is_none()
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NULL"
        )
        .await?,
        2
    );
    sqlx::query("UPDATE schedule_occurrences SET disposition='RUNNING'")
        .execute(&mut db)
        .await?;
    let next = s
        .queue_schedule_occurrence(workspace(), sid(5), epoch, 1_000_100)
        .await?
        .unwrap();
    assert_eq!(next.ready_at_us, 1_000_070);
    assert_eq!(next.coalesced, 0);
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn malformed_and_conflicting_evidence_fail_closed() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(
        5,
        json!({"kind":"and","children":[leaf("dataset_published","a","pin",false),leaf("dataset_head_changed","b","pin",false)]}),
    );
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    publish(&mut db, 11, 2, "cache_reuse", 30).await?;
    scan(&mut s, epoch, 40).await?;
    assert!(matches!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
            .await,
        Err(Error::Evidence)
    ));
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        0
    );
    sqlx::query("UPDATE trigger_tokens SET payload_json=json_set(payload_json,'$.format_version',2) WHERE leaf_id='a'").execute(&mut db).await?;
    assert!(matches!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
            .await,
        Err(Error::Evidence)
    ));
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NOT NULL"
        )
        .await?,
        0
    );
    sqlx::query("UPDATE schedules SET needs_review=1")
        .execute(&mut db)
        .await?;
    assert!(matches!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
            .await,
        Err(Error::Conflict)
    ));
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn schema_thirteen_upgrade_preserves_evidence_and_uses_pending_indexes() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(5, leaf("dataset_published", "a", "pin", false));
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    scan(&mut s, epoch, 30).await?;
    s.close().await?;
    sqlx::raw_sql("DROP INDEX schedule_pending_tokens; DROP INDEX schedule_pending_occurrences; DELETE FROM schema_migrations WHERE version=14; PRAGMA user_version=13;").execute(&mut db).await?;
    s = Store::open(&path).await?;
    assert_eq!(s.info().schema_version, 14);
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM trigger_tokens").await?,
        1
    );
    let token_plan:Vec<(i64,i64,i64,String)>=sqlx::query_as("EXPLAIN QUERY PLAN SELECT id FROM trigger_tokens WHERE schedule_id=? AND trigger_epoch=? AND consumed_by IS NULL LIMIT 100001").bind(sid(5).to_string()).bind(epoch).fetch_all(&mut db).await?;
    assert!(
        token_plan
            .iter()
            .any(|r| r.3.contains("schedule_pending_tokens"))
    );
    let pending_plan:Vec<(i64,i64,i64,String)>=sqlx::query_as("EXPLAIN QUERY PLAN SELECT id FROM schedule_occurrences WHERE schedule_id=? AND disposition IN ('ACCEPTED','QUEUED','HELD') LIMIT 101").bind(sid(5).to_string()).fetch_all(&mut db).await?;
    assert!(
        pending_plan
            .iter()
            .any(|r| r.3.contains("schedule_pending_occurrences"))
    );
    s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
        .await?
        .unwrap();
    s.close().await?;
    s = Store::open(&path).await?;
    assert!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 41)
            .await?
            .is_none()
    );
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        1
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn late_older_delivery_keeps_newer_event_pin_and_future_event_unconsumed() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(5, leaf("dataset_published", "a", "pin", false));
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 30).await?;
    publish(&mut db, 11, 2, "publication", 20).await?;
    publish(&mut db, 12, 3, "publication", 50).await?;
    scan(&mut s, epoch, 40).await?;
    let queued = s
        .queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
        .await?
        .unwrap();
    assert_eq!((queued.ready_at_us, queued.coalesced), (30, 1));
    let payload: String = sqlx::query_scalar("SELECT payload_json FROM schedule_occurrences")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&payload)?["input_pins"][0]["version_id"],
        version(10).to_string()
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NULL"
        )
        .await?,
        1
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[test]
fn enqueue_transaction_crash_child() -> Result {
    let Some(path) = std::env::var_os("TRANSFLOW_TEST_ENQUEUE_CRASH_DB") else {
        return Ok(());
    };
    let phase: u8 = std::env::var("TRANSFLOW_TEST_ENQUEUE_CRASH_PHASE")?.parse()?;
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let mut db=support::database::connect(std::path::Path::new(&path)).await?;
        sqlx::raw_sql("BEGIN IMMEDIATE").execute(&mut db).await?;
        if phase>=1 {
            sqlx::query("INSERT INTO schedule_occurrences(id,schedule_id,trigger_epoch,evidence_digest,logical_fire_at_us,payload_json,execution_json,disposition) SELECT ?,id,trigger_epoch,?,40,'{}','{}','QUEUED' FROM schedules")
                .bind(tf_domain::ScheduleOccurrenceId::from_bytes([90;16]).to_string()).bind("b".repeat(64)).execute(&mut db).await?;
        }
        if phase>=2 {
            sqlx::query("UPDATE trigger_tokens SET consumed_by=?").bind(tf_domain::ScheduleOccurrenceId::from_bytes([90;16]).to_string()).execute(&mut db).await?;
        }
        if phase>=3 {
            sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule_occurrence_queued','{}',40)").execute(&mut db).await?;
        }
        use std::io::Write;
        println!("enqueue-crash-ready");std::io::stdout().flush()?;
        loop {std::thread::park();}
        #[allow(unreachable_code,reason="The parent kills this durability probe after the marker")]
        Ok::<(),Box<dyn std::error::Error>>(())
    })
}
#[tokio::test]
async fn abrupt_death_at_enqueue_boundaries_rolls_back_and_retry_is_unique() -> Result {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(5, leaf("dataset_published", "a", "pin", false));
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    scan(&mut s, epoch, 30).await?;
    for phase in 0..=3 {
        let mut child = Command::new(std::env::current_exe()?)
            .args(["--exact", "enqueue_transaction_crash_child", "--nocapture"])
            .env("TRANSFLOW_TEST_ENQUEUE_CRASH_DB", &path)
            .env("TRANSFLOW_TEST_ENQUEUE_CRASH_PHASE", phase.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or("missing durability probe output")?;
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line.is_ok_and(|s| s == "enqueue-crash-ready") {
                    let _ = tx.send(());
                }
            }
        });
        let ready = rx.recv_timeout(Duration::from_secs(10));
        let _ = child.kill();
        child.wait()?;
        reader.join().map_err(|_| "probe output reader failed")?;
        ready?;
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NOT NULL"
            )
            .await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM audit_log WHERE operation='schedule_occurrence_queued'"
            )
            .await?,
            0
        );
    }
    s.close().await?;
    s = Store::open(&path).await?;
    s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
        .await?
        .unwrap();
    s.close().await?;
    s = Store::open(&path).await?;
    assert!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 41)
            .await?
            .is_none()
    );
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        1
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn pending_backlog_and_payload_limits_fail_before_acceptance() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let d = definition(5, leaf("dataset_published", "a", "pin", false));
    let saved = save(&mut s, 5, &d, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    scan(&mut s, epoch, 30).await?;
    sqlx::query("UPDATE trigger_tokens SET payload_json=json_set(payload_json,'$.causation',?)")
        .bind("x".repeat(4096))
        .execute(&mut db)
        .await?;
    assert!(matches!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
            .await,
        Err(Error::Limit)
    ));
    sqlx::query(
        "UPDATE trigger_tokens SET payload_json=json_set(payload_json,'$.causation','cause')",
    )
    .execute(&mut db)
    .await?;
    sqlx::raw_sql("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO trigger_tokens(id,schedule_id,trigger_epoch,leaf_id,event_id,payload_json,seen_at_us,expires_at_us) SELECT 'synthetic-old-'||x,schedule_id,trigger_epoch,leaf_id,event_id,payload_json,seen_at_us,expires_at_us FROM n,trigger_tokens WHERE x<=100000;").execute(&mut db).await?;
    assert!(matches!(
        s.queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
            .await,
        Err(Error::Limit)
    ));
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        0
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM trigger_tokens WHERE consumed_by IS NOT NULL"
        )
        .await?,
        0
    );
    sqlx::query("DELETE FROM trigger_tokens WHERE id LIKE 'synthetic-old-%'")
        .execute(&mut db)
        .await?;
    let queued = s
        .queue_schedule_occurrence(workspace(), sid(5), epoch, 40)
        .await?
        .unwrap();
    assert_eq!(queued.coalesced, 0);
    db.close().await?;
    s.close().await?;
    Ok(())
}
