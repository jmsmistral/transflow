//! Guarded durable clock observations. Timezone resolution runs before writer ownership.
use crate::{
    Reader, Store, StoreError,
    schedule_events::{TokenPayload, digest},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::BTreeSet;
use tf_domain::{
    ScheduleId, WorkspaceId,
    schedule_clock::{Cursor, Evaluation, Leaf, Misfire, Plan},
};
use tf_protocol::schedule::{Definition, PayloadMode, Trigger};
/// Definition/cursor conflict or bounded evidence failure, with no partial writes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Database failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Expected epoch, cursor or operational state changed.
    #[error("The schedule clock changed; prepare a new observation")]
    Conflict,
    /// Invalid prepared data, pending capacity or counters.
    #[error("The clock observation exceeds its bounds or contains invalid evidence")]
    Invalid,
}
/// Consistent bounded snapshot for resolution outside the SQLite transaction.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Stable schedule identity.
    pub schedule: ScheduleId,
    /// Current definition edit guard.
    pub etag: String,
    /// Current automatic trigger epoch.
    pub epoch: String,
    /// Authored clock leaves in stable tree order.
    pub leaves: Vec<Leaf>,
    /// Frozen cursor and policies.
    pub evaluation: Evaluation,
}
fn leaves(t: &Trigger, result: &mut Vec<Leaf>) {
    match t {
        Trigger::Cron {
            id,
            expression,
            timezone,
            duplicate_time,
        } => result.push(Leaf {
            id: id.clone(),
            expression: expression.clone(),
            timezone: timezone.clone(),
            duplicate_time: duplicate_time.clone(),
        }),
        Trigger::And { children } | Trigger::Or { children } => {
            for c in children {
                leaves(c, result);
            }
        }
        _ => {}
    }
}
async fn snapshot(
    db: &mut SqliteConnection,
    workspace: WorkspaceId,
    id: ScheduleId,
) -> Result<Snapshot, Error> {
    let r=sqlx::query("SELECT etag,trigger_epoch,paused,definition_json,saved_at_us FROM schedules WHERE workspace_id=? AND id=? AND deleted_at_us IS NULL AND needs_review=0 AND length(definition_json)<=262144")
        .bind(workspace.to_string()).bind(id.to_string()).fetch_optional(&mut *db).await.map_err(StoreError::from)?.ok_or(Error::Conflict)?;
    let epoch: String = r.try_get(1).map_err(StoreError::from)?;
    let raw: String = r.try_get(3).map_err(StoreError::from)?;
    let value: Value = serde_json::from_str(&raw).map_err(|_| Error::Invalid)?;
    let def = Definition::decode(&value).map_err(|_| Error::Invalid)?;
    let mut specs = vec![];
    leaves(&def.trigger, &mut specs);
    let current:Option<(i64,i64)>=sqlx::query_as("SELECT cursor_at_us,cursor_leaf FROM schedule_clock_state WHERE schedule_id=? AND trigger_epoch=?")
        .bind(id.to_string()).bind(&epoch).fetch_optional(&mut *db).await.map_err(StoreError::from)?;
    let (at, order) = current.unwrap_or((r.try_get(4).map_err(StoreError::from)?, 63));
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM (SELECT id FROM schedule_clock_ticks WHERE schedule_id=? AND trigger_epoch=? AND disposition='PENDING' LIMIT 101)")
        .bind(id.to_string()).bind(&epoch).fetch_one(&mut *db).await.map_err(StoreError::from)?;
    if pending > 100 || at < 0 {
        return Err(Error::Invalid);
    }
    let intervals: Vec<(i64,i64)> = sqlx::query_as("SELECT started_at_us,ended_at_us FROM schedule_pause_intervals WHERE schedule_id=? AND trigger_epoch=? AND ended_at_us IS NOT NULL AND ended_at_us>=? AND started_at_us<? ORDER BY started_at_us LIMIT 101")
        .bind(id.to_string()).bind(&epoch).bind(at).bind(at.saturating_add(86_400_000_000)).fetch_all(&mut *db).await.map_err(StoreError::from)?;
    if intervals.len() > 100 {
        return Err(Error::Invalid);
    }
    Ok(Snapshot {
        workspace,
        schedule: id,
        etag: r.try_get(0).map_err(StoreError::from)?,
        epoch,
        leaves: specs,
        evaluation: Evaluation {
            cursor: Cursor {
                at_us: at,
                leaf: u8::try_from(order).map_err(|_| Error::Invalid)?,
            },
            paused: r.try_get::<i64, _>(2).map_err(StoreError::from)? != 0,
            ignored_intervals: intervals,
            misfire: match def.policies.misfire_policy.as_str() {
                "skip" => Misfire::Skip,
                "coalesce_latest" => Misfire::CoalesceLatest,
                "catch_up" => Misfire::CatchUp,
                _ => return Err(Error::Invalid),
            },
            max_catch_up: def.policies.max_catch_up,
            pending: pending as u32,
        },
    })
}
impl Reader {
    /// Read definition/cursor together; no clock reads, timezone lookup or mutation.
    pub async fn schedule_clock(
        &mut self,
        workspace: WorkspaceId,
        id: ScheduleId,
    ) -> Result<Snapshot, Error> {
        let mut tx = self.db.begin().await.map_err(StoreError::from)?;
        let result = snapshot(&mut tx, workspace, id).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(result)
    }
}
impl Store {
    /// Read the same guarded clock snapshot through writer ownership.
    pub async fn schedule_clock(
        &mut self,
        workspace: WorkspaceId,
        id: ScheduleId,
    ) -> Result<Snapshot, Error> {
        let mut tx = self.db.begin().await.map_err(StoreError::from)?;
        let result = snapshot(&mut tx, workspace, id).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(result)
    }
    /// Commit prepared intended ticks, cursor/counters and audit together. False is
    /// a backward-clock or unchanged no-op. A conflict requires fresh preparation.
    pub async fn accept_schedule_clock(
        &mut self,
        expected: &Snapshot,
        plan: &Plan,
    ) -> Result<bool, Error> {
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(StoreError::from)?;
        let current = snapshot(&mut tx, expected.workspace, expected.schedule).await?;
        if current.etag != expected.etag
            || current.epoch != expected.epoch
            || current.evaluation.cursor != expected.evaluation.cursor
            || current.evaluation.paused != expected.evaluation.paused
            || current.evaluation.pending != expected.evaluation.pending
            || current.evaluation.ignored_intervals != expected.evaluation.ignored_intervals
        {
            return Err(Error::Conflict);
        }
        let e = &current.evaluation;
        if plan.cursor < e.cursor
            || plan.cursor.leaf > 63
            || plan.observed_at_us < 0
            || plan.ticks.len() > 100
            || plan.cursor.at_us > e.cursor.at_us.saturating_add(86_400_000_000)
            || plan.missed > plan.matched
            || plan.matched > 100_000
            || plan.ignored > plan.matched
            || plan.coalesced > plan.matched
            || plan.matched != plan.ticks.len() as u64 + plan.ignored + plan.coalesced
            || plan.tzdb_version.is_empty()
            || plan.tzdb_version.len() > 32
            || !plan.tzdb_version.bytes().all(|b| b.is_ascii_alphanumeric())
            || (e.paused && !plan.ticks.is_empty())
        {
            return Err(Error::Invalid);
        }
        if plan.cursor.at_us > plan.observed_at_us {
            if plan.cursor != e.cursor || !plan.ticks.is_empty() || plan.matched != 0 {
                return Err(Error::Invalid);
            }
            tx.commit().await.map_err(StoreError::from)?;
            return Ok(false);
        }
        if !e.paused
            && e.misfire == Misfire::CatchUp
            && plan.ticks.len() > e.max_catch_up.min(100 - e.pending) as usize
        {
            return Err(Error::Invalid);
        }
        if e.misfire != Misfire::CatchUp {
            let mut leaves = BTreeSet::new();
            if plan.ticks.iter().any(|t| !leaves.insert(t.key.leaf)) {
                return Err(Error::Invalid);
            }
        }
        let mut previous = e.cursor;
        for t in &plan.ticks {
            if t.key <= previous
                || t.key > plan.cursor
                || usize::from(t.key.leaf) >= current.leaves.len()
                || e.ignored_intervals
                    .iter()
                    .any(|(a, b)| t.key.at_us > *a && t.key.at_us <= *b)
            {
                return Err(Error::Invalid);
            }
            previous = t.key;
        }
        if plan.cursor == e.cursor && plan.ticks.is_empty() && plan.matched == 0 {
            tx.commit().await.map_err(StoreError::from)?;
            return Ok(false);
        }
        let mut replaced = 0i64;
        if e.misfire == Misfire::CoalesceLatest {
            for t in &plan.ticks {
                let leaf = &current.leaves[usize::from(t.key.leaf)];
                replaced+=sqlx::query("UPDATE schedule_clock_ticks SET disposition='COALESCED' WHERE schedule_id=? AND trigger_epoch=? AND leaf_id=? AND disposition='PENDING'")
                    .bind(current.schedule.to_string()).bind(&current.epoch).bind(&leaf.id).execute(&mut *tx).await.map_err(StoreError::from)?.rows_affected() as i64;
            }
        }
        if i64::from(e.pending) - replaced + plan.ticks.len() as i64 > 100 {
            return Err(Error::Invalid);
        }
        for t in &plan.ticks {
            let leaf = &current.leaves[usize::from(t.key.leaf)];
            let id = digest(&json!([
                "transflow.schedule.tick.v1",
                current.schedule.to_string(),
                current.epoch,
                leaf.id,
                t.key.at_us.to_string()
            ]))?;
            sqlx::query("INSERT INTO schedule_clock_ticks(id,schedule_id,trigger_epoch,leaf_id,leaf_order,at_us,tzdb_version,disposition) VALUES(?,?,?,?,?,?,?,'PENDING')")
                .bind(id).bind(current.schedule.to_string()).bind(&current.epoch).bind(&leaf.id).bind(i64::from(t.key.leaf)).bind(t.key.at_us).bind(&plan.tzdb_version).execute(&mut *tx).await.map_err(StoreError::from)?;
        }
        sqlx::query("INSERT INTO schedule_clock_state VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(schedule_id) DO UPDATE SET trigger_epoch=excluded.trigger_epoch,cursor_at_us=excluded.cursor_at_us,cursor_leaf=excluded.cursor_leaf,matched_count=CASE WHEN trigger_epoch=excluded.trigger_epoch THEN matched_count ELSE 0 END+excluded.matched_count,missed_count=CASE WHEN trigger_epoch=excluded.trigger_epoch THEN missed_count ELSE 0 END+excluded.missed_count,ignored_count=CASE WHEN trigger_epoch=excluded.trigger_epoch THEN ignored_count ELSE 0 END+excluded.ignored_count,coalesced_count=CASE WHEN trigger_epoch=excluded.trigger_epoch THEN coalesced_count ELSE 0 END+excluded.coalesced_count")
            .bind(current.schedule.to_string()).bind(&current.epoch).bind(plan.cursor.at_us).bind(i64::from(plan.cursor.leaf)).bind(plan.matched as i64).bind(plan.missed as i64).bind(plan.ignored as i64).bind(plan.coalesced as i64+replaced).execute(&mut *tx).await.map_err(StoreError::from)?;
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule_clock_observed',?,?)")
            .bind(json!({"schedule":current.schedule.to_string(),"trigger_epoch":current.epoch,"cursor_at_us":plan.cursor.at_us.to_string(),"cursor_leaf":plan.cursor.leaf,"matched":plan.matched.to_string(),"missed":plan.missed.to_string(),"ignored":plan.ignored.to_string(),"coalesced":(plan.coalesced+replaced as u64).to_string(),"more":plan.more,"tzdb_version":plan.tzdb_version}).to_string()).bind(plan.observed_at_us).execute(&mut *tx).await.map_err(StoreError::from)?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(true)
    }
    /// Deliver at most one pending intended tick per leaf. Catch-up waits while
    /// earlier eligible evidence is unconsumed; other policies may supersede it.
    /// Call T092 evaluation between deliveries so catch-up
    /// does not collapse independent ticks into its latest-per-leaf coalescing.
    /// Expired ticks are audited and ignored; this primitive launches no build.
    pub async fn deliver_schedule_ticks(
        &mut self,
        workspace: WorkspaceId,
        id: ScheduleId,
        epoch: &str,
        now_us: i64,
    ) -> Result<Delivery, Error> {
        if now_us < 0 {
            return Err(Error::Invalid);
        }
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(StoreError::from)?;
        let state = snapshot(&mut tx, workspace, id).await?;
        if state.epoch != epoch {
            return Err(Error::Conflict);
        }
        let mut result = Delivery::default();
        if state.evaluation.paused {
            tx.commit().await.map_err(StoreError::from)?;
            return Ok(result);
        }
        let raw: String = sqlx::query_scalar("SELECT definition_json FROM schedules WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(StoreError::from)?;
        let def = Definition::decode(&serde_json::from_str(&raw).map_err(|_| Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
        let rows=sqlx::query("SELECT id,leaf_id,at_us FROM schedule_clock_ticks WHERE schedule_id=? AND trigger_epoch=? AND disposition='PENDING' AND at_us<=? ORDER BY at_us,leaf_order LIMIT 100")
            .bind(id.to_string()).bind(epoch).bind(now_us).fetch_all(&mut *tx).await.map_err(StoreError::from)?;
        let mut seen = BTreeSet::new();
        for r in rows {
            let tick: String = r.try_get(0).map_err(StoreError::from)?;
            let leaf: String = r.try_get(1).map_err(StoreError::from)?;
            let at: i64 = r.try_get(2).map_err(StoreError::from)?;
            let expiry = def
                .policies
                .token_window_seconds
                .map(|s| {
                    at.checked_add(i64::from(s) * 1_000_000)
                        .ok_or(Error::Invalid)
                })
                .transpose()?;
            if expiry.is_some_and(|e| e <= now_us) {
                sqlx::query("UPDATE schedule_clock_ticks SET disposition='EXPIRED' WHERE id=?")
                    .bind(&tick)
                    .execute(&mut *tx)
                    .await
                    .map_err(StoreError::from)?;
                result.expired += 1;
                continue;
            }
            if !seen.insert(leaf.clone()) {
                continue;
            }
            let active:i64=sqlx::query_scalar("SELECT count(*) FROM (SELECT id FROM trigger_tokens WHERE schedule_id=? AND trigger_epoch=? AND leaf_id=? AND consumed_by IS NULL AND (expires_at_us IS NULL OR expires_at_us>?) LIMIT 1)")
                .bind(id.to_string()).bind(epoch).bind(&leaf).bind(now_us).fetch_one(&mut *tx).await.map_err(StoreError::from)?;
            if active != 0 && state.evaluation.misfire == Misfire::CatchUp {
                continue;
            }
            let token = digest(&json!(["transflow.schedule.tick.token.v1", tick]))?;
            let payload = TokenPayload {
                format_version: 1,
                event_id: tick.clone(),
                event_sequence: "0".into(),
                occurred_at_us: at.to_string(),
                event_kind: "schedule.tick".into(),
                causation: None,
                correlation: Some(id.to_string()),
                payload_mode: PayloadMode::SignalOnly,
                dataset: None,
                build_id: None,
                schedule_id: None,
                occurrence_id: None,
            };
            sqlx::query("INSERT INTO trigger_tokens(id,schedule_id,trigger_epoch,leaf_id,tick_id,payload_json,seen_at_us,expires_at_us) VALUES(?,?,?,?,?,?,?,?)")
                .bind(token).bind(id.to_string()).bind(epoch).bind(&leaf).bind(&tick).bind(serde_json::to_string(&payload).map_err(|_|Error::Invalid)?).bind(now_us).bind(expiry).execute(&mut *tx).await.map_err(StoreError::from)?;
            sqlx::query("UPDATE schedule_clock_ticks SET disposition='DELIVERED' WHERE id=?")
                .bind(&tick)
                .execute(&mut *tx)
                .await
                .map_err(StoreError::from)?;
            result.delivered += 1;
        }
        if result.delivered + result.expired > 0 {
            sqlx::query("UPDATE schedule_clock_state SET ignored_count=ignored_count+? WHERE schedule_id=? AND trigger_epoch=?").bind(i64::from(result.expired)).bind(id.to_string()).bind(epoch).execute(&mut *tx).await.map_err(StoreError::from)?;
            sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule_ticks_delivered',?,?)")
                .bind(json!({"schedule":id.to_string(),"trigger_epoch":epoch,"delivered":result.delivered,"expired":result.expired}).to_string()).bind(now_us).execute(&mut *tx).await.map_err(StoreError::from)?;
        }
        tx.commit().await.map_err(StoreError::from)?;
        Ok(result)
    }
}
/// Bounded token delivery facts, distinct from queued occurrences or executing jobs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Delivery {
    /// Newly delivered original intended ticks.
    pub delivered: u32,
    /// Old ticks ignored at their exclusive token expiry.
    pub expired: u32,
}
