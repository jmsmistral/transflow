//! Real SQLite clock acceptance, recovery, catch-up delivery and T092 queue integration.
#![allow(
    clippy::unwrap_used,
    reason = "Synthetic schedule and persisted-state fixtures"
)]
#[allow(dead_code, reason = "Shared fixture support")]
#[path = "../../../tests/support/mod.rs"]
mod support;
use chrono::DateTime;
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection};
use support::filesystem::ScratchDirectory;
use tf_domain::{DatasetId, ScheduleId, WorkspaceId, schedule_clock::Plan};
use tf_schedule::{Clock, prepare};
use tf_store::{Store, schedule_clock::Error, schedules::Save};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;
struct Virtual(i64);
impl Clock for Virtual {
    fn now_us(&self) -> std::result::Result<i64, tf_schedule::Error> {
        Ok(self.0)
    }
}
fn at(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s).unwrap().timestamp_micros()
}
fn workspace() -> WorkspaceId {
    "00000001-0000-4000-8000-000000000001".parse().unwrap()
}
fn dataset() -> DatasetId {
    "00000002-0000-4000-8000-000000000002".parse().unwrap()
}
fn id() -> ScheduleId {
    ScheduleId::from_bytes([5; 16])
}
fn cron(expr: &str, zone: &str, both: bool) -> Value {
    json!({"kind":"cron","id":"clock","expression":expr,"timezone":zone,"duplicate_time":if both {"both"}else{"earliest"}})
}
fn definition(trigger: Value, policy: &str) -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json")).unwrap();
    let mut v = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "schedule-definition-fixed")
        .unwrap()["value"]
        .clone();
    v["trigger"] = trigger;
    v["policies"]["misfire_policy"] = json!(policy);
    v["policies"]["max_pending"] = json!(100);
    v["policies"]["max_catch_up"] = json!(100);
    v
}
async fn fixture(
    path: &std::path::Path,
    trigger: Value,
    policy: &str,
    start: i64,
) -> std::result::Result<(Store, SqliteConnection, Value), Box<dyn std::error::Error>> {
    let mut s = Store::open(path).await?;
    s.register_workspace(workspace(), "synthetic", 1).await?;
    s.register_dataset(workspace(), dataset(), 2).await?;
    let mut db = support::database::connect(path).await?;
    let d = definition(trigger, policy);
    sqlx::query("INSERT INTO source_snapshots VALUES(?,?,?,'{}',NULL,'{}','{}')")
        .bind(d["build"]["source"]["snapshot_id"].as_str().unwrap())
        .bind(workspace().to_string())
        .bind("a".repeat(64))
        .execute(&mut db)
        .await?;
    let saved = s
        .save_schedule(Save {
            id: id(),
            workspace: workspace(),
            definition: &d,
            expected_etag: None,
            paused: false,
            actor: "fixture",
            at_us: start,
            receipt: None,
        })
        .await?;
    Ok((s, db, saved))
}
async fn poll(s: &mut Store, now: i64) -> std::result::Result<Plan, Box<dyn std::error::Error>> {
    let state = s.schedule_clock(workspace(), id()).await?;
    let plan = prepare(&state.leaves, &state.evaluation, &Virtual(now))?;
    s.accept_schedule_clock(&state, &plan).await?;
    Ok(plan)
}
async fn count(
    db: &mut SqliteConnection,
    sql: &'static str,
) -> std::result::Result<i64, sqlx::Error> {
    sqlx::query_scalar(sql).fetch_one(db).await
}
#[tokio::test]
async fn restart_retry_and_backward_clock_never_duplicate_intended_ticks_or_occurrences() -> Result
{
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let now = start + 3 * 60_000_000;
    let (mut s, mut db, saved) =
        fixture(&path, cron("* * * * *", "UTC", false), "catch_up", start).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    let first = poll(&mut s, now).await?;
    assert_eq!(first.ticks.len(), 3);
    s.close().await?;
    s = Store::open(&path).await?;
    assert!(poll(&mut s, now - 60_000_000).await?.ticks.is_empty());
    assert!(poll(&mut s, now).await?.ticks.is_empty());
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_clock_ticks").await?,
        3
    );
    for expected in 1..=3 {
        assert_eq!(
            s.deliver_schedule_ticks(workspace(), id(), epoch, now)
                .await?
                .delivered,
            1
        );
        // Retrying delivery before evaluation must preserve the earlier evidence.
        assert_eq!(
            s.deliver_schedule_ticks(workspace(), id(), epoch, now)
                .await?
                .delivered,
            0
        );
        let q = s
            .queue_schedule_occurrence(workspace(), id(), epoch, now)
            .await?
            .unwrap();
        assert_eq!(q.ready_at_us, start + expected * 60_000_000);
        assert_eq!(q.coalesced, 0);
        s.close().await?;
        s = Store::open(&path).await?;
    }
    assert!(
        s.queue_schedule_occurrence(workspace(), id(), epoch, now)
            .await?
            .is_none()
    );
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        3
    );
    assert_eq!(count(&mut db, "SELECT count(*) FROM builds").await?, 0);
    let bad = sqlx::query("UPDATE schedule_clock_ticks SET at_us=at_us+1")
        .execute(&mut db)
        .await;
    assert!(bad.is_err());
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn skip_latest_pause_and_expiry_have_persisted_counts() -> Result {
    for policy in ["skip", "coalesce_latest", "catch_up"] {
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("runtime.sqlite");
        let start = at("2026-10-03T00:00:00Z");
        let now = start + 5 * 60_000_000 + 30_000_000;
        let (mut s, mut db, saved) =
            fixture(&path, cron("* * * * *", "UTC", false), policy, start).await?;
        let plan = poll(&mut s, now).await?;
        assert_eq!(plan.matched, 5);
        assert_eq!(plan.ticks.len(), if policy == "catch_up" { 5 } else { 1 });
        let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT matched_count,missed_count,ignored_count,coalesced_count FROM schedule_clock_state").fetch_one(&mut db).await?;
        assert_eq!(
            counts,
            (
                5,
                4,
                if policy == "skip" { 4 } else { 0 },
                if policy == "coalesce_latest" { 4 } else { 0 }
            )
        );
        sqlx::query("UPDATE schedules SET paused=1")
            .execute(&mut db)
            .await?;
        let pause = poll(&mut s, now + 2 * 60_000_000).await?;
        assert_eq!((pause.matched, pause.ignored, pause.ticks.len()), (2, 2, 0));
        assert_eq!(
            s.deliver_schedule_ticks(
                workspace(),
                id(),
                saved["trigger_epoch"].as_str().unwrap(),
                now + 2 * 60_000_000
            )
            .await?
            .delivered,
            0
        );
        sqlx::query("UPDATE schedules SET paused=0")
            .execute(&mut db)
            .await?;
        assert!(poll(&mut s, now + 2 * 60_000_000).await?.ticks.is_empty());
        db.close().await?;
        s.close().await?;
    }
    Ok(())
}
#[tokio::test]
async fn coalesce_replaces_undelivered_ticks_and_expired_catchup_is_audited() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let (mut s, mut db, saved) = fixture(
        &path,
        cron("* * * * *", "UTC", false),
        "coalesce_latest",
        start,
    )
    .await?;
    poll(&mut s, start + 2 * 60_000_000).await?;
    poll(&mut s, start + 4 * 60_000_000).await?;
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM schedule_clock_ticks WHERE disposition='PENDING'"
        )
        .await?,
        1
    );
    assert_eq!(
        count(
            &mut db,
            "SELECT count(*) FROM schedule_clock_ticks WHERE disposition='COALESCED'"
        )
        .await?,
        1
    );
    assert_eq!(
        count(&mut db, "SELECT coalesced_count FROM schedule_clock_state").await?,
        3
    );
    let expired = s
        .deliver_schedule_ticks(
            workspace(),
            id(),
            saved["trigger_epoch"].as_str().unwrap(),
            start + 2 * 86_400_000_000,
        )
        .await?;
    assert_eq!((expired.expired, expired.delivered), (1, 0));
    assert_eq!(
        count(&mut db, "SELECT ignored_count FROM schedule_clock_state").await?,
        1
    );
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM trigger_tokens").await?,
        0
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn dst_fold_both_keys_survive_restart_and_spring_gap_has_no_tick() -> Result {
    for (expr, start, end, n) in [
        (
            "30 1 * * *",
            "2026-11-01T00:00:00Z",
            "2026-11-01T07:00:00Z",
            2,
        ),
        (
            "30 2 * * *",
            "2026-03-08T00:00:00Z",
            "2026-03-08T08:00:00Z",
            0,
        ),
    ] {
        let dir = ScratchDirectory::new()?;
        let path = dir.path().join("runtime.sqlite");
        let start = at(start);
        let end = at(end);
        let (mut s, mut db, _) = fixture(
            &path,
            cron(expr, "America/New_York", true),
            "catch_up",
            start,
        )
        .await?;
        let p = poll(&mut s, end).await?;
        assert_eq!(p.ticks.len(), n);
        s.close().await?;
        s = Store::open(&path).await?;
        assert!(poll(&mut s, end).await?.ticks.is_empty());
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM schedule_clock_ticks").await?,
            n as i64
        );
        db.close().await?;
        s.close().await?;
    }
    Ok(())
}
#[tokio::test]
async fn observation_write_failures_rollback_ticks_cursor_counts_and_audit() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let now = start + 60_000_000;
    let (mut s, mut db, _) =
        fixture(&path, cron("* * * * *", "UTC", false), "catch_up", start).await?;
    for ddl in [
        "CREATE TRIGGER fail BEFORE INSERT ON schedule_clock_ticks BEGIN SELECT RAISE(ABORT,'fixture'); END;",
        "CREATE TRIGGER fail BEFORE INSERT ON schedule_clock_state BEGIN SELECT RAISE(ABORT,'fixture'); END;",
        "CREATE TRIGGER fail BEFORE INSERT ON audit_log WHEN NEW.operation='schedule_clock_observed' BEGIN SELECT RAISE(ABORT,'fixture'); END;",
    ] {
        sqlx::raw_sql(ddl).execute(&mut db).await?;
        assert!(poll(&mut s, now).await.is_err());
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM schedule_clock_ticks").await?,
            0
        );
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM schedule_clock_state").await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM audit_log WHERE operation='schedule_clock_observed'"
            )
            .await?,
            0
        );
        sqlx::raw_sql("DROP TRIGGER fail").execute(&mut db).await?;
    }
    s.close().await?;
    s = Store::open(&path).await?;
    poll(&mut s, now).await?;
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_clock_ticks").await?,
        1
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn delivery_failures_preserve_pending_tick_and_retry_enqueues_once() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let now = start + 60_000_000;
    let (mut s, mut db, saved) =
        fixture(&path, cron("* * * * *", "UTC", false), "catch_up", start).await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    poll(&mut s, now).await?;
    for ddl in [
        "CREATE TRIGGER fail BEFORE INSERT ON trigger_tokens BEGIN SELECT RAISE(ABORT,'fixture'); END;",
        "CREATE TRIGGER fail BEFORE UPDATE ON schedule_clock_ticks WHEN NEW.disposition='DELIVERED' BEGIN SELECT RAISE(ABORT,'fixture'); END;",
        "CREATE TRIGGER fail BEFORE INSERT ON audit_log WHEN NEW.operation='schedule_ticks_delivered' BEGIN SELECT RAISE(ABORT,'fixture'); END;",
    ] {
        sqlx::raw_sql(ddl).execute(&mut db).await?;
        assert!(
            s.deliver_schedule_ticks(workspace(), id(), epoch, now)
                .await
                .is_err()
        );
        assert_eq!(
            count(&mut db, "SELECT count(*) FROM trigger_tokens").await?,
            0
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM schedule_clock_ticks WHERE disposition='PENDING'"
            )
            .await?,
            1
        );
        assert_eq!(
            count(
                &mut db,
                "SELECT count(*) FROM audit_log WHERE operation='schedule_ticks_delivered'"
            )
            .await?,
            0
        );
        sqlx::raw_sql("DROP TRIGGER fail").execute(&mut db).await?;
    }
    s.close().await?;
    s = Store::open(&path).await?;
    assert_eq!(
        s.deliver_schedule_ticks(workspace(), id(), epoch, now)
            .await?
            .delivered,
        1
    );
    assert_eq!(
        s.deliver_schedule_ticks(workspace(), id(), epoch, now)
            .await?
            .delivered,
        0
    );
    assert!(
        s.queue_schedule_occurrence(workspace(), id(), epoch, now)
            .await?
            .is_some()
    );
    assert!(
        s.queue_schedule_occurrence(workspace(), id(), epoch, now)
            .await?
            .is_none()
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn concurrent_preparations_and_definition_edit_receive_conflicts() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let now = start + 60_000_000;
    let (mut a, mut db, saved) =
        fixture(&path, cron("* * * * *", "UTC", false), "catch_up", start).await?;
    let mut b = Store::open(&path).await?;
    let sa = a.schedule_clock(workspace(), id()).await?;
    let sb = b.schedule_clock(workspace(), id()).await?;
    let pa = prepare(&sa.leaves, &sa.evaluation, &Virtual(now))?;
    let pb = prepare(&sb.leaves, &sb.evaluation, &Virtual(now))?;
    let (aresult, bresult) = tokio::join!(
        a.accept_schedule_clock(&sa, &pa),
        b.accept_schedule_clock(&sb, &pb)
    );
    assert_eq!(
        usize::from(aresult.is_ok()) + usize::from(bresult.is_ok()),
        1
    );
    assert!(matches!(aresult, Err(Error::Conflict)) || matches!(bresult, Err(Error::Conflict)));
    let state = a.schedule_clock(workspace(), id()).await?;
    let stale = prepare(&state.leaves, &state.evaluation, &Virtual(now + 60_000_000))?;
    let d = definition(cron("0 * * * *", "UTC", false), "skip");
    a.save_schedule(Save {
        id: id(),
        workspace: workspace(),
        definition: &d,
        expected_etag: saved["etag"].as_str(),
        paused: false,
        actor: "fixture",
        at_us: now,
        receipt: None,
    })
    .await?;
    assert!(matches!(
        a.accept_schedule_clock(&state, &stale).await,
        Err(Error::Conflict)
    ));
    assert!(matches!(
        a.deliver_schedule_ticks(workspace(), id(), &state.epoch, now)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_clock_ticks").await?,
        1
    );
    assert_eq!(
        a.schedule_clock(workspace(), id())
            .await?
            .evaluation
            .cursor
            .at_us,
        now
    );
    db.close().await?;
    a.close().await?;
    b.close().await?;
    Ok(())
}
#[tokio::test]
async fn compound_same_instant_catchup_limits_and_pending_capacity_do_not_lose_ticks() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let now = start + 60_000_000;
    let mut other = cron("* * * * *", "UTC", false);
    other["id"] = json!("second");
    let (mut s, mut db, saved) = fixture(
        &path,
        json!({"kind":"or","children":[cron("* * * * *","UTC",false),other]}),
        "catch_up",
        start,
    )
    .await?;
    sqlx::query("UPDATE schedules SET definition_json=json_set(definition_json,'$.policies.max_catch_up',1)").execute(&mut db).await?;
    assert!(poll(&mut s, now).await?.more);
    assert!(!poll(&mut s, now).await?.more);
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_clock_ticks").await?,
        2
    );
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    assert_eq!(
        s.deliver_schedule_ticks(workspace(), id(), epoch, now)
            .await?
            .delivered,
        2
    );
    for _ in 0..2 {
        assert!(
            s.queue_schedule_occurrence(workspace(), id(), epoch, now)
                .await?
                .is_some()
        );
    }
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        2
    );
    db.close().await?;
    s.close().await?;
    // A full pending tick queue keeps the high-water key so later draining can continue.
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let (mut s, mut db, _) =
        fixture(&path, cron("* * * * *", "UTC", false), "catch_up", start).await?;
    assert!(poll(&mut s, start + 101 * 60_000_000).await?.more);
    let full = poll(&mut s, start + 101 * 60_000_000).await?;
    assert!(full.more);
    assert_eq!(full.matched, 0);
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_clock_ticks").await?,
        100
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn schema_fourteen_upgrade_preserves_definitions_and_rejects_unknown_zone_without_writes()
-> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let (s, mut db, saved) = fixture(
        &path,
        cron("* * * * *", "Mars/Olympus", false),
        "catch_up",
        start,
    )
    .await?;
    s.close().await?;
    sqlx::raw_sql("DROP TABLE schedule_clock_ticks; DROP TABLE schedule_clock_state; DELETE FROM schema_migrations WHERE version=15; PRAGMA user_version=14;").execute(&mut db).await?;
    let mut s = Store::open(&path).await?;
    assert_eq!(s.info().schema_version, 15);
    assert_eq!(count(&mut db, "SELECT count(*) FROM schedules").await?, 1);
    assert!(poll(&mut s, start + 60_000_000).await.is_err());
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_clock_state").await?,
        0
    );
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM trigger_tokens").await?,
        0
    );
    assert_eq!(
        s.schedule_clock(workspace(), id()).await?.epoch,
        saved["trigger_epoch"].as_str().unwrap()
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn latest_clock_evidence_supersedes_older_signal_and_forged_tick_fails_closed() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let (mut s, mut db, saved) = fixture(
        &path,
        cron("* * * * *", "UTC", false),
        "coalesce_latest",
        start,
    )
    .await?;
    let epoch = saved["trigger_epoch"].as_str().unwrap();
    for n in 1..=2 {
        let now = start + n * 60_000_000;
        poll(&mut s, now).await?;
        assert_eq!(
            s.deliver_schedule_ticks(workspace(), id(), epoch, now)
                .await?
                .delivered,
            1
        );
    }
    let now = start + 2 * 60_000_000;
    sqlx::query("UPDATE trigger_tokens SET tick_id='forged' WHERE CAST(json_extract(payload_json,'$.occurred_at_us') AS INTEGER)=?").bind(now).execute(&mut db).await?;
    assert!(
        s.queue_schedule_occurrence(workspace(), id(), epoch, now)
            .await
            .is_err()
    );
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_occurrences").await?,
        0
    );
    sqlx::query("UPDATE trigger_tokens SET tick_id=json_extract(payload_json,'$.event_id')")
        .execute(&mut db)
        .await?;
    let q = s
        .queue_schedule_occurrence(workspace(), id(), epoch, now)
        .await?
        .unwrap();
    assert_eq!((q.ready_at_us, q.coalesced), (now, 1));
    db.close().await?;
    s.close().await?;
    Ok(())
}
#[tokio::test]
async fn invalid_prepared_capacity_and_pause_race_do_not_mutate_clock_state() -> Result {
    let dir = ScratchDirectory::new()?;
    let path = dir.path().join("runtime.sqlite");
    let start = at("2026-10-03T00:00:00Z");
    let (mut s, mut db, _) =
        fixture(&path, cron("* * * * *", "UTC", false), "catch_up", start).await?;
    sqlx::query("UPDATE schedules SET definition_json=json_set(definition_json,'$.policies.max_catch_up',1)").execute(&mut db).await?;
    let state = s.schedule_clock(workspace(), id()).await?;
    let mut p = prepare(
        &state.leaves,
        &state.evaluation,
        &Virtual(start + 2 * 60_000_000),
    )?;
    let mut extra = p.ticks[0];
    extra.key.at_us += 60_000_000;
    p.ticks.push(extra);
    p.cursor = extra.key;
    p.matched = 2;
    p.missed = 1;
    assert!(matches!(
        s.accept_schedule_clock(&state, &p).await,
        Err(Error::Invalid)
    ));
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_clock_state").await?,
        0
    );
    let p = prepare(
        &state.leaves,
        &state.evaluation,
        &Virtual(start + 60_000_000),
    )?;
    sqlx::query("UPDATE schedules SET paused=1")
        .execute(&mut db)
        .await?;
    assert!(matches!(
        s.accept_schedule_clock(&state, &p).await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        count(&mut db, "SELECT count(*) FROM schedule_clock_ticks").await?,
        0
    );
    db.close().await?;
    s.close().await?;
    Ok(())
}
