//! Audited lifecycle transactions. Accepted work and manual requests retain independent evidence.
use crate::{
    Store, StoreError,
    schedules::{Error, row},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::BTreeMap;
use tf_domain::{RequestId, ScheduleId, ScheduleOccurrenceId, WorkspaceId};
use tf_protocol::schedule::Definition;

/// Explicit replay of a bounded retained event tail; never implicit on resume/save.
#[derive(Clone, Copy, Debug)]
pub struct Replay {
    /// Exclusive committed local event sequence.
    pub after: i64,
    /// Entire replay interval must fit this 1–100 event budget.
    pub limit: u32,
}
/// Schedule lifecycle action; CLI/editor reuse these services in subsequent tasks.
pub enum Action {
    /// Hold accepted automatic requests; running/manual work is unchanged.
    Pause,
    /// Release held work under frozen policies; optionally replay bounded retained events.
    Resume(Option<Replay>),
    /// Tombstone the definition; retained history, manual requests and running work remain.
    Delete,
    /// Queue an independent manual request, even while paused.
    Run,
}
/// Audited and idempotent lifecycle intent.
pub struct Control<'a> {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Stable schedule ID.
    pub id: ScheduleId,
    /// Expected current opaque edit guard.
    pub etag: &'a str,
    /// Independent action identity, required for safe manual retry.
    pub key: RequestId,
    /// Canonical transport/action digest.
    pub digest: &'a str,
    /// Safe non-secret actor label.
    pub actor: &'a str,
    /// Frozen UTC action time.
    pub at_us: i64,
    /// Explicit action.
    pub action: Action,
}
fn invalid() -> Error {
    StoreError::InvalidRequest.into()
}
pub(crate) fn actor_time(actor: &str, at: i64) -> Result<(), Error> {
    if at < 0 || actor.is_empty() || actor.len() > 200 || actor.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}
/// Open an operational interval; current definition history is not retained.
pub(crate) async fn open_pause(
    db: &mut SqliteConnection,
    id: ScheduleId,
    epoch: &str,
    at: i64,
    after: i64,
) -> Result<(), Error> {
    sqlx::query("INSERT INTO schedule_pause_intervals(schedule_id,trigger_epoch,started_at_us,event_after) VALUES(?,?,?,?)")
        .bind(id.to_string()).bind(epoch).bind(at).bind(after).execute(db).await.map_err(StoreError::from)?;
    Ok(())
}
pub(crate) async fn close_pause(
    db: &mut SqliteConnection,
    id: ScheduleId,
    epoch: &str,
    at: i64,
    through: i64,
) -> Result<(), Error> {
    sqlx::query("UPDATE schedule_pause_intervals SET ended_at_us=max(started_at_us,?),event_through=max(event_after,?) WHERE schedule_id=? AND trigger_epoch=? AND ended_at_us IS NULL")
        .bind(at).bind(through).bind(id.to_string()).bind(epoch).execute(db).await.map_err(StoreError::from)?;
    Ok(())
}
async fn pending(db: &mut SqliteConnection, id: ScheduleId) -> Result<i64, Error> {
    let n: i64=sqlx::query_scalar("SELECT count(*) FROM (SELECT id FROM schedule_occurrences WHERE schedule_id=? AND disposition IN ('ACCEPTED','QUEUED','HELD') LIMIT 101)")
        .bind(id.to_string()).fetch_one(db).await.map_err(StoreError::from)?;
    if n > 100 {
        return Err(Error::Limit);
    }
    Ok(n)
}
async fn release(db: &mut SqliteConnection, id: ScheduleId, at: i64) -> Result<Value, Error> {
    pending(db, id).await?;
    let rows=sqlx::query("SELECT id,logical_fire_at_us,execution_json,payload_json FROM schedule_occurrences WHERE schedule_id=? AND disposition='HELD' AND coalesce(json_extract(payload_json,'$.manual'),0)=0 ORDER BY logical_fire_at_us DESC,id DESC LIMIT 101")
        .bind(id.to_string()).fetch_all(&mut *db).await.map_err(StoreError::from)?;
    if rows.len() > 100 {
        return Err(Error::Limit);
    }
    let mut latest = BTreeMap::new();
    let mut changes = vec![];
    for r in rows {
        let occurrence: String = r.try_get(0).map_err(StoreError::from)?;
        let fire: i64 = r.try_get(1).map_err(StoreError::from)?;
        let execution: String = r.try_get(2).map_err(StoreError::from)?;
        if execution.len() > 262144 {
            return Err(invalid());
        }
        let frozen: Value = serde_json::from_str(&execution).map_err(|_| invalid())?;
        let policies: tf_protocol::schedule::Policies =
            serde_json::from_value(frozen["policies"].clone()).map_err(|_| invalid())?;
        let raw: String = r.try_get(3).map_err(StoreError::from)?;
        if raw.len() > 262144 {
            return Err(invalid());
        }
        let payload: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
        let mut earliest = fire;
        if let Some(tokens) = payload["tokens"].as_array() {
            for token in tokens {
                let original = token["occurred_at_us"]
                    .as_str()
                    .ok_or_else(invalid)?
                    .parse::<i64>()
                    .map_err(|_| invalid())?;
                if original < 0 {
                    return Err(invalid());
                }
                earliest = earliest.min(original);
            }
        }
        let expired = policies
            .token_window_seconds
            .map(|s| {
                earliest
                    .checked_add(i64::from(s) * 1_000_000)
                    .ok_or_else(invalid)
            })
            .transpose()?
            .is_some_and(|t| t <= at);
        let (state, reason) = if expired {
            ("EXPIRED", "resume_expiry")
        } else {
            match policies.overlap_policy.as_str() {
                "skip" => ("SKIPPED", "resume_skip"),
                "queue" => ("QUEUED", "resume_queue"),
                "coalesce_latest" => {
                    if latest
                        .insert(execution.clone(), occurrence.clone())
                        .is_none()
                    {
                        ("QUEUED", "resume_latest")
                    } else {
                        ("COALESCED", "resume_coalesced")
                    }
                }
                _ => return Err(invalid()),
            }
        };
        sqlx::query("UPDATE schedule_occurrences SET disposition=? WHERE id=?")
            .bind(state)
            .bind(&occurrence)
            .execute(&mut *db)
            .await
            .map_err(StoreError::from)?;
        changes.push(json!({"occurrence":occurrence,"disposition":state,"reason":reason}));
    }
    Ok(json!(changes))
}
/// Replay into a fresh epoch; prior accepted evidence is immutable. Queueing uses
/// the same event matching/evaluator as normal observations, within this transaction.
pub(crate) async fn replay(
    db: &mut SqliteConnection,
    workspace: WorkspaceId,
    id: ScheduleId,
    request: Replay,
    at: i64,
) -> Result<String, Error> {
    if request.after < 0 || !(1..=100).contains(&request.limit) {
        return Err(invalid());
    }
    let high: i64 = sqlx::query_scalar("SELECT coalesce(max(sequence),0) FROM events")
        .fetch_one(&mut *db)
        .await
        .map_err(StoreError::from)?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT sequence FROM events WHERE sequence>? LIMIT 101)",
    )
    .bind(request.after)
    .fetch_one(&mut *db)
    .await
    .map_err(StoreError::from)?;
    if request.after > high || count > i64::from(request.limit) {
        return Err(Error::Limit);
    }
    let epoch: String = sqlx::query_scalar("SELECT lower(hex(randomblob(32)))")
        .fetch_one(&mut *db)
        .await
        .map_err(StoreError::from)?;
    sqlx::query("UPDATE schedules SET trigger_epoch=?,event_cursor=? WHERE id=?")
        .bind(&epoch)
        .bind(request.after)
        .bind(id.to_string())
        .execute(&mut *db)
        .await
        .map_err(StoreError::from)?;
    // Event replay does not implicitly replay paused clock ticks in its fresh epoch.
    sqlx::query("INSERT INTO schedule_clock_state VALUES(?,?,?,63,0,0,0,0) ON CONFLICT(schedule_id) DO UPDATE SET trigger_epoch=excluded.trigger_epoch,cursor_at_us=excluded.cursor_at_us,cursor_leaf=63,matched_count=0,missed_count=0,ignored_count=0,coalesced_count=0")
        .bind(id.to_string()).bind(&epoch).bind(at).execute(&mut *db).await.map_err(StoreError::from)?;
    sqlx::query("DELETE FROM trigger_tokens WHERE schedule_id=? AND consumed_by IS NULL")
        .bind(id.to_string())
        .execute(&mut *db)
        .await
        .map_err(StoreError::from)?;
    let mut scanned = 0;
    loop {
        let result = crate::schedule_events::scan(db, workspace, id, &epoch, 100, at)
            .await
            .map_err(Error::from)?;
        scanned += result.scanned;
        if !result.more {
            break;
        }
        if result.scanned == 0 || scanned > 100 {
            return Err(Error::Limit);
        }
    }
    let mut occurrences = vec![];
    for _ in 0..100 {
        match crate::schedule_evaluation::queue(db, workspace, id, &epoch, at)
            .await
            .map_err(|e| match e {
                crate::schedule_evaluation::Error::Store(s) => Error::Store(s),
                crate::schedule_evaluation::Error::Limit => Error::Limit,
                _ => invalid(),
            })? {
            Some(o) => occurrences.push(o.occurrence.to_string()),
            None => break,
        }
    }
    sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule_replayed',?,?)")
        .bind(json!({"schedule":id.to_string(),"trigger_epoch":epoch,"after":request.after.to_string(),"through":high.to_string(),"scanned":scanned,"occurrences":occurrences}).to_string()).bind(at).execute(db).await.map_err(StoreError::from)?;
    Ok(epoch)
}
impl Store {
    /// Atomic pause/resume/manual acceptance, audit and retry receipt. Automatic
    /// dispatch and the public CLI/editor are separate later integrations.
    pub async fn control_schedule(&mut self, r: Control<'_>) -> Result<Value, Error> {
        actor_time(r.actor, r.at_us)?;
        if r.digest.len() != 64
            || !r.digest.bytes().all(|b| b.is_ascii_hexdigit())
            || r.etag.is_empty()
            || r.etag.len() > 100
        {
            return Err(invalid());
        }
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(StoreError::from)?;
        let prior: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT digest,response_json FROM api_operations WHERE id=?")
                .bind(r.key.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(StoreError::from)?;
        if let Some((digest, response)) = prior {
            if digest != r.digest || response.is_none() {
                return Err(Error::Conflict { current_etag: None });
            }
            let value =
                serde_json::from_str(&response.ok_or_else(invalid)?).map_err(|_| invalid())?;
            tx.commit().await.map_err(StoreError::from)?;
            return Ok(value);
        }
        let current = row(&mut tx, r.workspace, r.id)
            .await?
            .ok_or(Error::Missing)?;
        if current["etag"] != r.etag
            || (current["needs_review"] == true && !matches!(r.action, Action::Delete))
        {
            return Err(Error::Conflict {
                current_etag: current["etag"].as_str().map(str::to_owned),
            });
        }
        if matches!(r.action, Action::Delete) {
            sqlx::query("UPDATE schedules SET deleted_at_us=? WHERE id=? AND workspace_id=? AND deleted_at_us IS NULL")
                .bind(r.at_us).bind(r.id.to_string()).bind(r.workspace.to_string()).execute(&mut *tx).await.map_err(StoreError::from)?;
            let canceled=sqlx::query("UPDATE schedule_occurrences SET disposition='CANCELED' WHERE schedule_id=? AND build_id IS NULL AND disposition IN ('ACCEPTED','QUEUED','HELD') AND json_extract(payload_json,'$.manual') IS NOT 1")
                .bind(r.id.to_string()).execute(&mut *tx).await.map_err(StoreError::from)?.rows_affected();
            sqlx::query("DELETE FROM trigger_tokens WHERE schedule_id=? AND consumed_by IS NULL")
                .bind(r.id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(StoreError::from)?;
            sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule_deleted',?,?)")
                .bind(json!({"schedule":r.id.to_string(),"actor":r.actor,"request_id":r.key.to_string(),"canceled_automatic":canceled.to_string()}).to_string()).bind(r.at_us).execute(&mut *tx).await.map_err(StoreError::from)?;
            let response = json!({"data":{"id":r.id.to_string(),"deleted":true,"retained_history":true,"canceled_automatic":canceled.to_string()},"context":null,"etag":null});
            sqlx::query("INSERT INTO api_operations(id,digest,response_json) VALUES(?,?,?)")
                .bind(r.key.to_string())
                .bind(r.digest)
                .bind(response.to_string())
                .execute(&mut *tx)
                .await
                .map_err(StoreError::from)?;
            tx.commit().await.map_err(StoreError::from)?;
            return Ok(response);
        }
        let def = Definition::decode(&current["definition"]).map_err(|_| invalid())?;
        let epoch = current["trigger_epoch"].as_str().ok_or_else(invalid)?;
        let paused = current["paused"] == true;
        let mut changes = json!([]);
        let (operation, data) = match r.action {
            Action::Delete => return Err(invalid()),
            Action::Run => {
                if pending(&mut tx, r.id).await? >= i64::from(def.policies.max_pending) {
                    return Err(Error::Limit);
                }
                let id = ScheduleOccurrenceId::from_bytes(
                    *crate::schedule_events::event_id(&json!([
                        "transflow.schedule.manual.v1",
                        r.id.to_string(),
                        r.key.to_string()
                    ]))?
                    .as_bytes(),
                );
                let evidence = crate::schedule_events::digest(&json!([
                    "transflow.schedule.manual.v1",
                    r.id.to_string(),
                    r.key.to_string(),
                    r.digest
                ]))?;
                let payload = json!({"format_version":1,"manual":true,"request_id":r.key.to_string(),"actor":r.actor,"input_pins":[],"tokens":[],"token_ids":[]});
                let execution = json!({"build":current["definition"]["build"],"policies":current["definition"]["policies"]});
                sqlx::query("INSERT INTO schedule_occurrences(id,schedule_id,trigger_epoch,evidence_digest,logical_fire_at_us,payload_json,execution_json,disposition) VALUES(?,?,?,?,?,?,?,'QUEUED')")
                    .bind(id.to_string()).bind(r.id.to_string()).bind(epoch).bind(&evidence).bind(r.at_us).bind(payload.to_string()).bind(execution.to_string()).execute(&mut *tx).await.map_err(StoreError::from)?;
                (
                    "schedule_manual_requested",
                    json!({"id":id.to_string(),"schedule_id":r.id.to_string(),"trigger_epoch":epoch,"evidence_digest":evidence,"logical_fire_at_us":r.at_us.to_string(),"disposition":"QUEUED","manual":true}),
                )
            }
            Action::Pause => {
                pending(&mut tx, r.id).await?;
                if !paused {
                    let high: i64 =
                        sqlx::query_scalar("SELECT coalesce(max(sequence),0) FROM events")
                            .fetch_one(&mut *tx)
                            .await
                            .map_err(StoreError::from)?;
                    open_pause(&mut tx, r.id, epoch, r.at_us, high).await?;
                    let etag: String = sqlx::query_scalar("SELECT lower(hex(randomblob(32)))")
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(StoreError::from)?;
                    sqlx::query("UPDATE schedules SET paused=1,etag=? WHERE id=?")
                        .bind(etag)
                        .bind(r.id.to_string())
                        .execute(&mut *tx)
                        .await
                        .map_err(StoreError::from)?;
                }
                sqlx::query("UPDATE schedule_occurrences SET disposition='HELD' WHERE schedule_id=? AND disposition IN ('ACCEPTED','QUEUED') AND coalesce(json_extract(payload_json,'$.manual'),0)=0").bind(r.id.to_string()).execute(&mut *tx).await.map_err(StoreError::from)?;
                (
                    "schedule_paused",
                    row(&mut tx, r.workspace, r.id)
                        .await?
                        .ok_or(Error::Missing)?,
                )
            }
            Action::Resume(replay_request) => {
                if paused {
                    let high: i64 =
                        sqlx::query_scalar("SELECT coalesce(max(sequence),0) FROM events")
                            .fetch_one(&mut *tx)
                            .await
                            .map_err(StoreError::from)?;
                    // A legacy/initially paused schedule might predate interval recording.
                    let open: i64=sqlx::query_scalar("SELECT count(*) FROM schedule_pause_intervals WHERE schedule_id=? AND trigger_epoch=? AND ended_at_us IS NULL").bind(r.id.to_string()).bind(epoch).fetch_one(&mut *tx).await.map_err(StoreError::from)?;
                    if open == 0 {
                        let cursor: i64 =
                            sqlx::query_scalar("SELECT event_cursor FROM schedules WHERE id=?")
                                .bind(r.id.to_string())
                                .fetch_one(&mut *tx)
                                .await
                                .map_err(StoreError::from)?;
                        open_pause(
                            &mut tx,
                            r.id,
                            epoch,
                            current["saved_at_us"]
                                .as_str()
                                .ok_or_else(invalid)?
                                .parse()
                                .map_err(|_| invalid())?,
                            cursor,
                        )
                        .await?;
                    }
                    close_pause(&mut tx, r.id, epoch, r.at_us, high).await?;
                    changes = release(&mut tx, r.id, r.at_us).await?;
                }
                if paused || replay_request.is_some() {
                    let etag: String = sqlx::query_scalar("SELECT lower(hex(randomblob(32)))")
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(StoreError::from)?;
                    sqlx::query("UPDATE schedules SET paused=0,etag=? WHERE id=?")
                        .bind(etag)
                        .bind(r.id.to_string())
                        .execute(&mut *tx)
                        .await
                        .map_err(StoreError::from)?;
                    if let Some(request) = replay_request {
                        replay(&mut tx, r.workspace, r.id, request, r.at_us).await?;
                    }
                }
                (
                    "schedule_resumed",
                    row(&mut tx, r.workspace, r.id)
                        .await?
                        .ok_or(Error::Missing)?,
                )
            }
        };
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES(?,?,?)")
            .bind(operation).bind(json!({"schedule":r.id.to_string(),"actor":r.actor,"request_id":r.key.to_string(),"etag_before":r.etag,"etag_after":data["etag"],"occurrence":data["id"],"changes":changes}).to_string()).bind(r.at_us).execute(&mut *tx).await.map_err(StoreError::from)?;
        let response = json!({"data":data,"context":null,"etag":data["etag"].as_str()});
        sqlx::query("INSERT INTO api_operations(id,digest,response_json) VALUES(?,?,?)")
            .bind(r.key.to_string())
            .bind(r.digest)
            .bind(response.to_string())
            .execute(&mut *tx)
            .await
            .map_err(StoreError::from)?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(response)
    }
}
