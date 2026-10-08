//! Atomic automatic occurrence acceptance. Dispatch and overlap lifecycle remain separate.
use crate::{
    Store, StoreError, retention,
    schedule_events::{TokenPayload, digest, event_id},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
use std::collections::BTreeMap;
use tf_domain::{
    DatasetKey, ScheduleId, ScheduleOccurrenceId, VersionId, WorkspaceId,
    schedule::{self, Expression, Token},
};
use tf_protocol::schedule::{Definition, PayloadMode, Trigger};

/// One durable queued build request, with frozen evidence and execution settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Queued {
    /// Deterministic occurrence identity.
    pub occurrence: ScheduleOccurrenceId,
    /// Canonical chosen-evidence digest.
    pub evidence_digest: String,
    /// Original ready time, independent of evaluation/restart time.
    pub ready_at_us: i64,
    /// Chosen event/tick tokens.
    pub selected: usize,
    /// Superseded eligible tokens consumed with the chosen leaves.
    pub coalesced: u64,
}
/// Guard and evidence failures preserve the complete pre-evaluation state.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Repository failure, including an atomic enqueue rollback.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The current definition was edited, deleted or requires review.
    #[error("The schedule changed or requires review before trigger evaluation")]
    Conflict,
    /// Malformed or conflicting retained trigger evidence.
    #[error("The retained trigger evidence is invalid or has conflicting input versions")]
    Evidence,
    /// A bounded evidence or queue budget was exceeded.
    #[error("The scheduling evidence exceeds its supported evaluation budget")]
    Limit,
}
fn tree<'a>(t: &'a Trigger, leaves: &mut BTreeMap<&'a str, &'a Trigger>) -> Expression<'a> {
    match t {
        Trigger::Manual => Expression::Manual,
        Trigger::And { children } => {
            Expression::And(children.iter().map(|c| tree(c, leaves)).collect())
        }
        Trigger::Or { children } => {
            Expression::Or(children.iter().map(|c| tree(c, leaves)).collect())
        }
        Trigger::Cron { id, .. }
        | Trigger::DatasetHeadChanged { id, .. }
        | Trigger::DatasetPublished { id, .. }
        | Trigger::BuildSucceeded { id, .. }
        | Trigger::ScheduleSucceeded { id, .. } => {
            leaves.insert(id, t);
            Expression::Leaf(id)
        }
    }
}
fn number(s: &str) -> Result<i64, Error> {
    let n = s.parse::<i64>().map_err(|_| Error::Evidence)?;
    if n < 0 || n.to_string() != s {
        return Err(Error::Evidence);
    }
    Ok(n)
}
fn evidence(leaf: &Trigger, p: &TokenPayload) -> Result<(), Error> {
    if p.format_version != 1 {
        return Err(Error::Evidence);
    }
    match leaf {
        Trigger::DatasetHeadChanged {
            dataset,
            branch,
            payload_mode,
            include_resets,
            ..
        }
        | Trigger::DatasetPublished {
            dataset,
            branch,
            payload_mode,
            include_resets,
            ..
        } => {
            let d = p.dataset.as_ref().ok_or(Error::Evidence)?;
            let key = DatasetKey::from_text(&d.workspace_id, &d.dataset_id)
                .map_err(|_| Error::Evidence)?;
            d.version_id
                .parse::<VersionId>()
                .map_err(|_| Error::Evidence)?;
            if number(&d.generation)? == 0
                || key != dataset.key().map_err(|_| Error::Evidence)?
                || d.branch != *branch
                || p.payload_mode != *payload_mode
                || (!include_resets && d.cause == crate::schedule_events::DatasetCause::Reset)
                || (matches!(leaf, Trigger::DatasetPublished { .. })
                    && d.cause != crate::schedule_events::DatasetCause::Publication)
                || !matches!(
                    p.event_kind.as_str(),
                    "dataset.published" | "dataset.head_changed" | "schedule.dataset_event"
                )
            {
                return Err(Error::Evidence);
            }
        }
        Trigger::BuildSucceeded { .. }
            if p.event_kind == "build.state"
                && p.build_id.is_some()
                && p.schedule_id.is_none()
                && p.dataset.is_none()
                && p.payload_mode == PayloadMode::SignalOnly => {}
        Trigger::ScheduleSucceeded { schedule_id, .. }
            if p.event_kind == "schedule.succeeded"
                && p.schedule_id.as_ref() == Some(schedule_id)
                && p.occurrence_id.is_some()
                && p.dataset.is_none()
                && p.payload_mode == PayloadMode::SignalOnly => {}
        Trigger::Cron { .. }
            if p.event_kind == "schedule.tick"
                && p.dataset.is_none()
                && p.payload_mode == PayloadMode::SignalOnly => {}
        _ => return Err(Error::Evidence),
    }
    Ok(())
}
impl Store {
    /// Evaluate one current tree and atomically enqueue at most one occurrence.
    /// The QUEUED occurrence is the durable request; no build/job is launched here.
    /// A full pending queue or paused schedule leaves evidence untouched for later evaluation.
    pub async fn queue_schedule_occurrence(
        &mut self,
        workspace: WorkspaceId,
        id: ScheduleId,
        epoch: &str,
        now_us: i64,
    ) -> Result<Option<Queued>, Error> {
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(StoreError::from)?;
        let result = queue(&mut tx, workspace, id, epoch, now_us).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(result)
    }
}

/// Shared acceptance used within an already-owned lifecycle transaction.
pub(crate) async fn queue(
    db: &mut sqlx::SqliteConnection,
    workspace: WorkspaceId,
    id: ScheduleId,
    epoch: &str,
    now_us: i64,
) -> Result<Option<Queued>, Error> {
    if now_us < 0 {
        return Err(Error::Evidence);
    }
    retention::clock(&mut *db, now_us)
        .await
        .map_err(|_| Error::Evidence)?;
    let row = sqlx::query("SELECT definition_json,paused,needs_review FROM schedules WHERE workspace_id=? AND id=? AND trigger_epoch=? AND deleted_at_us IS NULL")
            .bind(workspace.to_string()).bind(id.to_string()).bind(epoch).fetch_optional(&mut *db).await.map_err(StoreError::from)?.ok_or(Error::Conflict)?;
    if row.try_get::<i64, _>(2).map_err(StoreError::from)? != 0 {
        return Err(Error::Conflict);
    }
    if row.try_get::<i64, _>(1).map_err(StoreError::from)? != 0 {
        return Ok(None);
    }
    let raw: String = row.try_get(0).map_err(StoreError::from)?;
    if raw.len() > 262144 {
        return Err(Error::Limit);
    }
    let value: Value = serde_json::from_str(&raw).map_err(|_| Error::Evidence)?;
    let def = Definition::decode(&value).map_err(|_| Error::Evidence)?;
    let mut leaves = BTreeMap::new();
    let expression = tree(&def.trigger, &mut leaves);
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT id FROM schedule_occurrences WHERE schedule_id=? AND disposition IN ('ACCEPTED','QUEUED','HELD') LIMIT 101)")
            .bind(id.to_string()).fetch_one(&mut *db).await.map_err(StoreError::from)?;
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM schedule_occurrences o JOIN builds b ON b.id=o.build_id WHERE o.schedule_id=? AND b.state IN ('QUEUED','RUNNING','WAITING','STARTING')")
        .bind(id.to_string()).fetch_one(&mut *db).await.map_err(StoreError::from)?;
    if pending >= i64::from(def.policies.max_pending)
        && (active == 0
            || def.policies.allow_overlapping_builds
            || def.policies.overlap_policy != "coalesce_latest")
    {
        return Ok(None);
    }
    let pending_tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT id FROM trigger_tokens WHERE schedule_id=? AND trigger_epoch=? AND consumed_by IS NULL LIMIT 100001)")
            .bind(id.to_string()).bind(epoch).fetch_one(&mut *db).await.map_err(StoreError::from)?;
    if pending_tokens > 100_000 {
        return Err(Error::Limit);
    }
    // Rank in SQLite so arbitrarily many older tokens can coalesce without loading them.
    // Original occurrence time precedes arrival sequence in the latest-evidence ordering.
    let rows = sqlx::query("SELECT id,leaf_id,event_id,tick_id,CASE WHEN length(CAST(payload_json AS BLOB))<=4096 THEN payload_json ELSE NULL END,expires_at_us FROM (SELECT *,row_number() OVER(PARTITION BY leaf_id ORDER BY CAST(json_extract(payload_json,'$.occurred_at_us') AS INTEGER) DESC,CAST(json_extract(payload_json,'$.event_sequence') AS INTEGER) DESC,id DESC) AS rank FROM trigger_tokens WHERE schedule_id=? AND trigger_epoch=? AND consumed_by IS NULL AND (expires_at_us IS NULL OR expires_at_us>?) AND CAST(json_extract(payload_json,'$.occurred_at_us') AS INTEGER)<=?) WHERE rank=1 ORDER BY leaf_id LIMIT 65")
            .bind(id.to_string()).bind(epoch).bind(now_us).bind(now_us).fetch_all(&mut *db).await.map_err(StoreError::from)?;
    if rows.len() > 64 {
        return Err(Error::Limit);
    }
    let mut payloads = vec![];
    let mut ids = vec![];
    let mut names = vec![];
    let mut times = vec![];
    let mut sequences = vec![];
    let mut expiries = vec![];
    for row in rows {
        let raw: Option<String> = row.try_get(4).map_err(StoreError::from)?;
        let p: TokenPayload =
            serde_json::from_str(&raw.ok_or(Error::Limit)?).map_err(|_| Error::Evidence)?;
        let token: String = row.try_get(0).map_err(StoreError::from)?;
        let leaf: String = row.try_get(1).map_err(StoreError::from)?;
        evidence(leaves.get(leaf.as_str()).ok_or(Error::Evidence)?, &p)?;
        let time = number(&p.occurred_at_us)?;
        let sequence = number(&p.event_sequence)?;
        let event: Option<String> = row.try_get(2).map_err(StoreError::from)?;
        let tick: Option<String> = row.try_get(3).map_err(StoreError::from)?;
        if let Some(event) = event {
            if tick.is_some() || event != p.event_id {
                return Err(Error::Evidence);
            }
            let original: Option<(i64, i64, String)> =
                sqlx::query_as("SELECT sequence,wall_time_us,type FROM events WHERE id=?")
                    .bind(&event)
                    .fetch_optional(&mut *db)
                    .await
                    .map_err(StoreError::from)?;
            if original != Some((sequence, time, p.event_kind.clone()))
                || token
                    != digest(&json!([
                        "transflow.schedule.token.v1",
                        id.to_string(),
                        epoch,
                        leaf,
                        event
                    ]))?
            {
                return Err(Error::Evidence);
            }
        } else {
            let tick = tick.ok_or(Error::Evidence)?;
            let original: Option<(String,String,String,i64)> = sqlx::query_as("SELECT schedule_id,trigger_epoch,leaf_id,at_us FROM schedule_clock_ticks WHERE id=? AND disposition='DELIVERED'")
                    .bind(&tick).fetch_optional(&mut *db).await.map_err(StoreError::from)?;
            if tick != p.event_id
                || p.event_kind != "schedule.tick"
                || sequence != 0
                || original != Some((id.to_string(), epoch.into(), leaf.clone(), time))
                || token != digest(&json!(["transflow.schedule.tick.token.v1", tick]))?
            {
                return Err(Error::Evidence);
            }
        }
        if let Some(d) = &p.dataset {
            if d.workspace_id == workspace.to_string() {
                let version = d
                    .version_id
                    .parse::<VersionId>()
                    .map_err(|_| Error::Evidence)?;
                retention::version_available(&mut *db, version)
                    .await
                    .map_err(|_| Error::Evidence)?;
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM dataset_versions WHERE id=? AND dataset_id=?",
                )
                .bind(&d.version_id)
                .bind(&d.dataset_id)
                .fetch_one(&mut *db)
                .await
                .map_err(StoreError::from)?;
                if count != 1 {
                    return Err(Error::Evidence);
                }
            } else {
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM foreign_versions WHERE workspace_id=? AND dataset_id=? AND version_id=?")
                        .bind(&d.workspace_id).bind(&d.dataset_id).bind(&d.version_id).fetch_one(&mut *db).await.map_err(StoreError::from)?;
                if count != 1 {
                    return Err(Error::Evidence);
                }
            }
        }
        ids.push(token);
        names.push(leaf);
        times.push(time);
        sequences.push(sequence as u64);
        expiries.push(row.try_get::<Option<i64>, _>(5).map_err(StoreError::from)?);
        payloads.push(p);
    }
    let tokens: Vec<_> = ids
        .iter()
        .zip(&names)
        .zip(&times)
        .zip(&sequences)
        .zip(&expiries)
        .map(|((((id, leaf), time), sequence), expires)| Token {
            id,
            leaf,
            at_us: *time,
            sequence: *sequence,
            expires_at_us: *expires,
        })
        .collect();
    let Some(chosen) =
        schedule::evaluate(&expression, &tokens, now_us).map_err(|_| Error::Evidence)?
    else {
        return Ok(None);
    };
    let mut selected = vec![];
    let mut selected_ids = vec![];
    let mut pins = BTreeMap::new();
    let mut canonical = vec![];
    let mut coalesced = vec![];
    let mut total_coalesced = 0;
    for index in &chosen.selected {
        let p = payloads.get(*index).ok_or(Error::Evidence)?;
        let token = ids.get(*index).ok_or(Error::Evidence)?;
        let leaf = names.get(*index).ok_or(Error::Evidence)?;
        canonical.push(json!({"leaf_id":leaf,"token_id":token,"payload":p}));
        selected.push(p);
        selected_ids.push(token);
        if let Some(pin) = p.input_pin() {
            let key = (
                pin.workspace_id.clone(),
                pin.dataset_id.clone(),
                pin.branch.clone(),
            );
            if let Some(old) = pins.insert(key, pin)
                && old.version_id != pin.version_id
            {
                return Err(Error::Evidence);
            }
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM trigger_tokens WHERE schedule_id=? AND trigger_epoch=? AND leaf_id=? AND consumed_by IS NULL AND (expires_at_us IS NULL OR expires_at_us>?) AND CAST(json_extract(payload_json,'$.occurred_at_us') AS INTEGER)<=?")
                .bind(id.to_string()).bind(epoch).bind(leaf).bind(now_us).bind(now_us).fetch_one(&mut *db).await.map_err(StoreError::from)?;
        if count < 1 {
            return Err(Error::Evidence);
        }
        total_coalesced += (count - 1) as u64;
        coalesced.push(json!({"leaf_id":leaf,"count":(count-1).to_string()}));
    }
    let evidence_digest = digest(&json!({"format_version":1,"selected":canonical}))?;
    let occurrence = ScheduleOccurrenceId::from_bytes(
        *event_id(&json!([
            "transflow.schedule.occurrence.v1",
            id.to_string(),
            epoch,
            evidence_digest
        ]))?
        .as_bytes(),
    );
    let payload = json!({"format_version":1,"token_ids":selected_ids,"tokens":selected,"input_pins":pins.into_values().collect::<Vec<_>>(),"coalesced":coalesced});
    let encoded_payload = serde_json::to_string(&payload).map_err(|_| Error::Evidence)?;
    if encoded_payload.len() > 262144 {
        return Err(Error::Limit);
    }
    let execution = json!({"build":value["build"],"policies":value["policies"]});
    let disposition = crate::schedule_overlap::apply(
        db,
        id,
        &def,
        &execution,
        chosen.ready_at_us,
        occurrence,
        now_us,
    )
    .await?;
    sqlx::query("INSERT INTO schedule_occurrences(id,schedule_id,trigger_epoch,evidence_digest,logical_fire_at_us,payload_json,execution_json,disposition) VALUES(?,?,?,?,?,?,?,?)")
            .bind(occurrence.to_string()).bind(id.to_string()).bind(epoch).bind(&evidence_digest).bind(chosen.ready_at_us).bind(encoded_payload).bind(execution.to_string()).bind(disposition).execute(&mut *db).await.map_err(StoreError::from)?;
    for index in &chosen.selected {
        let leaf = names.get(*index).ok_or(Error::Evidence)?;
        sqlx::query("UPDATE trigger_tokens SET consumed_by=? WHERE schedule_id=? AND trigger_epoch=? AND leaf_id=? AND consumed_by IS NULL AND (expires_at_us IS NULL OR expires_at_us>?) AND CAST(json_extract(payload_json,'$.occurred_at_us') AS INTEGER)<=?")
                .bind(occurrence.to_string()).bind(id.to_string()).bind(epoch).bind(leaf).bind(now_us).bind(now_us).execute(&mut *db).await.map_err(StoreError::from)?;
    }
    sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule_occurrence_queued',?,?)")
            .bind(json!({"schedule":id.to_string(),"trigger_epoch":epoch,"occurrence":occurrence.to_string(),"evidence_digest":evidence_digest,"selected":chosen.selected.len(),"coalesced":total_coalesced.to_string(),"disposition":disposition}).to_string()).bind(now_us).execute(&mut *db).await.map_err(StoreError::from)?;
    Ok(Some(Queued {
        occurrence,
        evidence_digest,
        ready_at_us: chosen.ready_at_us,
        selected: chosen.selected.len(),
        coalesced: total_coalesced,
    }))
}
