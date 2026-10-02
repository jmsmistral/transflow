//! Real SQLite committed-event matching, duplicate delivery, exact pins and rollback.
#![allow(clippy::unwrap_used, reason = "Synthetic scheduling fixtures")]
#[allow(dead_code, reason = "Shared fixture utilities")]
#[path = "../../../tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection};
use support::filesystem::ScratchDirectory;
use tf_domain::{
    BuildId, DatasetId, DatasetKey, ScheduleId, ScheduleOccurrenceId, VersionId, WorkspaceId,
};
use tf_store::{
    Store,
    retention::RetentionPolicy,
    schedule_events::{DatasetCause, ForeignDatasetEvent, TokenPayload},
    schedules::{Freeze, Save},
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
async fn payloads(
    db: &mut SqliteConnection,
    n: u8,
) -> std::result::Result<Vec<TokenPayload>, Box<dyn std::error::Error>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT payload_json FROM trigger_tokens WHERE schedule_id=? ORDER BY seen_at_us,id",
    )
    .bind(sid(n).to_string())
    .fetch_all(db)
    .await?;
    rows.iter().map(|v| Ok(serde_json::from_str(v)?)).collect()
}
#[tokio::test]
async fn publication_cache_reset_and_signal_only_have_distinct_evidence() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let definitions = [
        definition(5, leaf("dataset_published", "new", "pin", false)),
        definition(6, leaf("dataset_head_changed", "head", "pin", false)),
        definition(
            7,
            leaf("dataset_head_changed", "signal", "signal_only", true),
        ),
    ];
    let mut epochs = vec![];
    for (n, v) in (5..=7).zip(&definitions) {
        epochs.push(
            save(&mut s, n, v, false).await?["trigger_epoch"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    publish(&mut db, 10, 1, "publication", 20).await?;
    publish(&mut db, 11, 2, "cache_reuse", 30).await?;
    publish(&mut db, 12, 3, "reset", 40).await?;
    // Event branch labels stay frozen even if the mutable branch catalogue changes.
    sqlx::query("UPDATE data_branches SET name='feature' WHERE id=?")
        .bind(branch())
        .execute(&mut db)
        .await?;
    for (n, epoch) in (5..=7).zip(&epochs) {
        let scan = s
            .scan_schedule_events(workspace(), sid(n), epoch, 100, 50)
            .await?;
        assert_eq!(scan.tokens, usize::from(n - 4));
        assert!(!scan.more);
        assert_eq!(
            s.scan_schedule_events(workspace(), sid(n), epoch, 100, 50)
                .await?
                .tokens,
            0
        );
        // Simulate redelivery after a cursor checkpoint retry. Stable token IDs deduplicate.
        sqlx::query("UPDATE schedules SET event_cursor=0 WHERE id=?")
            .bind(sid(n).to_string())
            .execute(&mut db)
            .await?;
        assert_eq!(
            s.scan_schedule_events(workspace(), sid(n), epoch, 100, 50)
                .await?
                .tokens,
            0
        );
    }
    let observed = payloads(&mut db, 5).await?;
    assert_eq!(
        observed[0].input_pin().unwrap().version_id,
        version(10).to_string()
    );
    assert_eq!(observed[0].occurred_at_us, "20");
    assert_eq!(observed[0].causation.as_deref(), Some("cause"));
    let signal = payloads(&mut db, 7).await?;
    assert!(
        signal
            .iter()
            .all(|p| p.input_pin().is_none() && p.dataset.is_some())
    );
    assert!(
        signal
            .iter()
            .any(|p| p.dataset.as_ref().unwrap().cause == DatasetCause::Reset)
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn token_and_cursor_rollback_together_and_pages_resume_after_reopen() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let v = definition(5, leaf("dataset_head_changed", "head", "pin", false));
    let current = save(&mut s, 5, &v, false).await?;
    let epoch = current["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    publish(&mut db, 11, 2, "publication", 30).await?;
    sqlx::raw_sql("CREATE TRIGGER injected_token_failure BEFORE INSERT ON trigger_tokens BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").execute(&mut db).await?;
    assert!(
        s.scan_schedule_events(workspace(), sid(5), epoch, 1, 40)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT event_cursor FROM schedules")
            .fetch_one(&mut db)
            .await?,
        0
    );
    assert!(payloads(&mut db, 5).await?.is_empty());
    sqlx::raw_sql("DROP TRIGGER injected_token_failure")
        .execute(&mut db)
        .await?;
    let first = s
        .scan_schedule_events(workspace(), sid(5), epoch, 1, 40)
        .await?;
    assert!(first.more);
    assert_eq!(first.tokens, 1);
    s.close().await?;
    let mut s = Store::open(&path).await?;
    let second = s
        .scan_schedule_events(workspace(), sid(5), epoch, 1, 41)
        .await?;
    assert!(!second.more);
    assert_eq!(second.tokens, 1);
    assert!(second.cursor > first.cursor);
    assert!(
        s.scan_schedule_events(workspace(), sid(5), "stale-epoch", 1, 42)
            .await
            .is_err()
    );
    assert!(
        s.scan_schedule_events(workspace(), sid(5), epoch, 101, 42)
            .await
            .is_err()
    );
    assert!(
        s.scan_schedule_events(workspace(), sid(5), epoch, 0, 42)
            .await
            .is_err()
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn failed_uncommitted_expired_and_paused_events_do_not_mint_tokens() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let mut v = definition(5, leaf("dataset_published", "new", "pin", false));
    v["policies"]["token_window_seconds"] = json!(1);
    let current = save(&mut s, 5, &v, false).await?;
    let epoch = current["trigger_epoch"].as_str().unwrap();
    let mut tx = db.begin().await?;
    sqlx::query("INSERT INTO events(id,type,payload_json,wall_time_us) VALUES('rolled-back','dataset.published','{}',20)").execute(&mut *tx).await?;
    let mut reader = s.reader().await?;
    assert_eq!(
        reader.event_page("workspace", Some(0)).await?["events"],
        json!([])
    );
    reader.close().await?;
    tx.rollback().await?;
    s.append_event(
        tf_domain::RequestId::from_bytes([99; 16]),
        "dataset.published",
        &json!({"version_id":version(10).to_string()}),
        20,
    )
    .await?;
    assert_eq!(
        s.scan_schedule_events(workspace(), sid(5), epoch, 100, 31)
            .await?
            .tokens,
        0
    );
    publish(&mut db, 10, 1, "publication", 40).await?;
    let expired = s
        .scan_schedule_events(workspace(), sid(5), epoch, 100, 1000040)
        .await?;
    assert_eq!(expired.ignored, 1);
    assert_eq!(expired.tokens, 0);
    let v = definition(6, leaf("dataset_published", "new", "pin", false));
    let paused = save(&mut s, 6, &v, true).await?;
    publish(&mut db, 11, 2, "publication", 1000050).await?;
    let scan = s
        .scan_schedule_events(
            workspace(),
            sid(6),
            paused["trigger_epoch"].as_str().unwrap(),
            100,
            1000060,
        )
        .await?;
    assert_eq!(scan.ignored, 1);
    assert_eq!(scan.tokens, 0);
    sqlx::query("UPDATE schedules SET paused=0 WHERE id=?")
        .bind(sid(6).to_string())
        .execute(&mut db)
        .await?;
    assert_eq!(
        s.scan_schedule_events(
            workspace(),
            sid(6),
            paused["trigger_epoch"].as_str().unwrap(),
            100,
            1000061
        )
        .await?
        .tokens,
        0
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn queued_tokens_and_accepted_occurrences_keep_exact_versions_rooted() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let mut v = definition(5, leaf("dataset_published", "new", "pin", false));
    v["policies"]["token_window_seconds"] = json!(1);
    let saved = save(&mut s, 5, &v, false).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    publish(&mut db, 10, 1, "publication", 20).await?;
    publish(&mut db, 11, 2, "publication", 30).await?;
    let policy = RetentionPolicy {
        latest: 1,
        age_us: 0,
    };
    assert!(
        s.retention_roots(40, policy)
            .await?
            .versions
            .contains(&version(10))
    );
    assert_eq!(
        s.scan_schedule_events(workspace(), sid(5), epoch, 100, 41)
            .await?
            .tokens,
        2
    );
    assert!(
        s.retention_roots(42, policy)
            .await?
            .versions
            .contains(&version(10))
    );
    let payload = json!({"tokens":payloads(&mut db,5).await?});
    let occurrence = ScheduleOccurrenceId::from_bytes([80; 16]);
    s.freeze_schedule_occurrence(Freeze {
        workspace: workspace(),
        id: sid(5),
        epoch,
        occurrence,
        evidence: &"b".repeat(64),
        payload: &payload,
        at_us: 43,
    })
    .await?;
    sqlx::query("UPDATE trigger_tokens SET consumed_by=?")
        .bind(occurrence.to_string())
        .execute(&mut db)
        .await?;
    assert!(
        s.retention_roots(2000000, policy)
            .await?
            .versions
            .contains(&version(10))
    );
    let frozen: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>("SELECT payload_json FROM schedule_occurrences")
            .fetch_one(&mut db)
            .await?,
    )?;
    let pins: Vec<TokenPayload> = serde_json::from_value(frozen["tokens"].clone())?;
    assert!(pins.iter().any(|p| {
        p.input_pin()
            .is_some_and(|v| v.version_id == version(10).to_string())
    }));
    sqlx::query("UPDATE schedule_occurrences SET disposition='CANCELED'")
        .execute(&mut db)
        .await?;
    assert!(
        !s.retention_roots(2000001, policy)
            .await?
            .versions
            .contains(&version(10))
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn provider_delivery_is_deduplicated_origin_qualified_and_metadata_only() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let provider = WorkspaceId::from_bytes([90; 16]);
    let key = DatasetKey::new(provider, dataset());
    let mut trigger = leaf("dataset_published", "external", "pin", false);
    trigger["dataset"]["workspace_id"] = json!(provider.to_string());
    let v = definition(5, trigger);
    let saved = save(&mut s, 5, &v, false).await?;
    let source = v["build"]["source"]["snapshot_id"].as_str().unwrap();
    sqlx::query("INSERT INTO external_registrations VALUES(?,'external','foreign',?,?,'{}','{}')")
        .bind(source)
        .bind(provider.to_string())
        .bind(dataset().to_string())
        .execute(&mut db)
        .await?;
    for n in [10, 11] {
        sqlx::query("INSERT INTO foreign_versions VALUES(?,?,?,'{}',?,'METADATA_ONLY')")
            .bind(provider.to_string())
            .bind(dataset().to_string())
            .bind(version(n).to_string())
            .bind("a".repeat(64))
            .execute(&mut db)
            .await?;
    }
    let delivery = || ForeignDatasetEvent {
        event: tf_domain::RequestId::from_bytes([91; 16]),
        dataset: key,
        version: version(10),
        branch: "master".parse().unwrap(),
        generation: 1,
        cause: DatasetCause::Publication,
        at_us: 20,
    };
    let mut newer = delivery();
    newer.event = tf_domain::RequestId::from_bytes([93; 16]);
    newer.version = version(11);
    newer.generation = 2;
    newer.at_us = 40;
    s.receive_foreign_schedule_event(newer).await?;
    // Older provider evidence arrives later; both observations remain exact and ordered locally.
    let first = s.receive_foreign_schedule_event(delivery()).await?;
    assert_eq!(s.receive_foreign_schedule_event(delivery()).await?, first);
    let mut changed = delivery();
    changed.event = tf_domain::RequestId::from_bytes([92; 16]);
    assert!(s.receive_foreign_schedule_event(changed).await.is_err());
    let mut wrong = delivery();
    wrong.version = version(12);
    assert!(s.receive_foreign_schedule_event(wrong).await.is_err());
    assert_eq!(
        s.scan_schedule_events(
            workspace(),
            sid(5),
            saved["trigger_epoch"].as_str().unwrap(),
            100,
            50
        )
        .await?
        .tokens,
        2
    );
    let evidence = payloads(&mut db, 5).await?;
    assert!(evidence.iter().any(|p| {
        p.input_pin()
            .is_some_and(|v| v.version_id == version(10).to_string())
    }));
    assert!(evidence.iter().any(|p| {
        p.input_pin()
            .is_some_and(|v| v.version_id == version(11).to_string())
    }));
    assert_eq!(
        evidence[0].input_pin().unwrap().workspace_id,
        provider.to_string()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artifacts")
            .fetch_one(&mut db)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM replicas")
            .fetch_one(&mut db)
            .await?,
        0
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
async fn build(
    db: &mut SqliteConnection,
    n: u8,
    state: &str,
    job_state: &str,
    occurrence: Option<ScheduleOccurrenceId>,
) -> std::result::Result<BuildId, Box<dyn std::error::Error>> {
    let id = BuildId::from_bytes([n; 16]);
    let plan = tf_domain::PlanId::from_bytes([n + 1; 16]);
    sqlx::query("INSERT INTO build_plans VALUES(?,'ACCEPTED',NULL,?,'{}','{}','[]','[]','[]','digest',NULL)").bind(plan.to_string()).bind(json!({"output":{"name":"master"}}).to_string()).execute(&mut *db).await?;
    sqlx::query("INSERT INTO builds(id,plan_id,occurrence_id,trigger_json,requested_by,state,created_at_us,finished_at_us) VALUES(?,?,?,'{}','fixture','RUNNING',20,NULL)").bind(id.to_string()).bind(plan.to_string()).bind(occurrence.map(|o|o.to_string())).execute(&mut *db).await?;
    sqlx::query("INSERT INTO jobs VALUES(?,?,?,NULL,?,?,'{}',NULL)")
        .bind(tf_domain::JobId::from_bytes([n + 2; 16]).to_string())
        .bind(id.to_string())
        .bind(dataset().to_string())
        .bind(branch())
        .bind(job_state)
        .execute(&mut *db)
        .await?;
    sqlx::query("UPDATE builds SET state=?,finished_at_us=30 WHERE id=?")
        .bind(state)
        .bind(id.to_string())
        .execute(&mut *db)
        .await?;
    Ok(id)
}
#[tokio::test]
async fn successful_build_filters_and_schedule_materialization_requirements() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let mut v = definition(
        5,
        json!({"kind":"build_succeeded","id":"build","targets":[{"workspace_id":workspace().to_string(),"dataset_id":dataset().to_string()}],"branch":"master"}),
    );
    v["policies"]["token_window_seconds"] = Value::Null;
    v["policies"]["acknowledge_no_expiry"] = json!(true);
    let saved = save(&mut s, 5, &v, false).await?;
    let mut other_branch = v.clone();
    other_branch["name"] = json!("other-branch");
    other_branch["trigger"]["branch"] = json!("feature");
    let other_branch = save(&mut s, 9, &other_branch, false).await?;
    build(&mut db, 20, "FAILED", "FAILED", None).await?;
    let success = build(&mut db, 30, "SUCCEEDED", "CACHED", None).await?;
    assert_eq!(
        s.scan_schedule_events(
            workspace(),
            sid(5),
            saved["trigger_epoch"].as_str().unwrap(),
            100,
            40
        )
        .await?
        .tokens,
        1
    );
    assert_eq!(
        payloads(&mut db, 5).await?[0].build_id.as_deref(),
        Some(success.to_string().as_str())
    );
    assert_eq!(
        s.scan_schedule_events(
            workspace(),
            sid(9),
            other_branch["trigger_epoch"].as_str().unwrap(),
            100,
            40
        )
        .await?
        .tokens,
        0
    );
    let upstream = definition(6, json!({"kind":"manual"}));
    let up = save(&mut s, 6, &upstream, false).await?;
    let occurrence = ScheduleOccurrenceId::from_bytes([60; 16]);
    s.freeze_schedule_occurrence(Freeze {
        workspace: workspace(),
        id: sid(6),
        epoch: up["trigger_epoch"].as_str().unwrap(),
        occurrence,
        evidence: &"c".repeat(64),
        payload: &json!({}),
        at_us: 41,
    })
    .await?;
    let cached = build(&mut db, 40, "SUCCEEDED", "CACHED", Some(occurrence)).await?;
    let mut down = definition(
        7,
        json!({"kind":"schedule_succeeded","id":"upstream","schedule_id":sid(6).to_string(),"require_materialization":false}),
    );
    down["policies"]["token_window_seconds"] = Value::Null;
    down["policies"]["acknowledge_no_expiry"] = json!(true);
    let d = save(&mut s, 7, &down, false).await?;
    let mut strict = down.clone();
    strict["name"] = json!("strict");
    strict["trigger"]["require_materialization"] = json!(true);
    let strict = save(&mut s, 8, &strict, false).await?;
    sqlx::raw_sql("CREATE TRIGGER injected_success_failure BEFORE INSERT ON events WHEN NEW.type='schedule.succeeded' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").execute(&mut db).await?;
    assert!(
        s.complete_schedule_success(workspace(), occurrence, cached, 50)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT disposition FROM schedule_occurrences WHERE id=?")
            .bind(occurrence.to_string())
            .fetch_one(&mut db)
            .await?,
        "ACCEPTED"
    );
    sqlx::raw_sql("DROP TRIGGER injected_success_failure")
        .execute(&mut db)
        .await?;
    let event = s
        .complete_schedule_success(workspace(), occurrence, cached, 50)
        .await?;
    assert_eq!(
        s.complete_schedule_success(workspace(), occurrence, cached, 51)
            .await?,
        event
    );
    assert_eq!(
        s.scan_schedule_events(
            workspace(),
            sid(7),
            d["trigger_epoch"].as_str().unwrap(),
            100,
            52
        )
        .await?
        .tokens,
        1
    );
    assert_eq!(
        s.scan_schedule_events(
            workspace(),
            sid(8),
            strict["trigger_epoch"].as_str().unwrap(),
            100,
            52
        )
        .await?
        .tokens,
        0
    );
    assert!(
        s.complete_schedule_success(workspace(), occurrence, success, 53)
            .await
            .is_err()
    );
    let occurrence2 = ScheduleOccurrenceId::from_bytes([61; 16]);
    s.freeze_schedule_occurrence(Freeze {
        workspace: workspace(),
        id: sid(6),
        epoch: up["trigger_epoch"].as_str().unwrap(),
        occurrence: occurrence2,
        evidence: &"d".repeat(64),
        payload: &json!({}),
        at_us: 54,
    })
    .await?;
    let failed = build(&mut db, 50, "FAILED", "FAILED", Some(occurrence2)).await?;
    assert!(
        s.complete_schedule_success(workspace(), occurrence2, failed, 55)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events WHERE type='schedule.succeeded'")
            .fetch_one(&mut db)
            .await?,
        1
    );
    let occurrence3 = ScheduleOccurrenceId::from_bytes([62; 16]);
    s.freeze_schedule_occurrence(Freeze {
        workspace: workspace(),
        id: sid(6),
        epoch: up["trigger_epoch"].as_str().unwrap(),
        occurrence: occurrence3,
        evidence: &"e".repeat(64),
        payload: &json!({}),
        at_us: 56,
    })
    .await?;
    let materialized = build(&mut db, 70, "SUCCEEDED", "SUCCEEDED", Some(occurrence3)).await?;
    s.complete_schedule_success(workspace(), occurrence3, materialized, 57)
        .await?;
    assert_eq!(
        s.scan_schedule_events(
            workspace(),
            sid(8),
            strict["trigger_epoch"].as_str().unwrap(),
            100,
            58
        )
        .await?
        .tokens,
        1
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}

#[tokio::test]
async fn expired_unconsumed_tokens_release_their_exact_retention_roots() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db) = fixture(&path).await?;
    let mut v = definition(5, leaf("dataset_published", "new", "signal_only", false));
    v["policies"]["token_window_seconds"] = json!(1);
    let saved = save(&mut s, 5, &v, false).await?;
    publish(&mut db, 10, 1, "publication", 20).await?;
    publish(&mut db, 11, 2, "publication", 30).await?;
    s.scan_schedule_events(
        workspace(),
        sid(5),
        saved["trigger_epoch"].as_str().unwrap(),
        100,
        40,
    )
    .await?;
    let policy = RetentionPolicy {
        latest: 1,
        age_us: 0,
    };
    assert!(
        s.retention_roots(50, policy)
            .await?
            .versions
            .contains(&version(10))
    );
    assert!(
        !s.retention_roots(1000031, policy)
            .await?
            .versions
            .contains(&version(10))
    );
    // Expiry releases bytes but keeps immutable event/observation metadata inspectable.
    assert_eq!(payloads(&mut db, 5).await?.len(), 2);
    db.close().await?;
    s.close().await?;
    Ok(())
}
