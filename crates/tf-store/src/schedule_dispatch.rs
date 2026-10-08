//! Durable occurrence dispatch guards and restart reconciliation. No source imports here.
use crate::{Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use tf_domain::{BuildId, ScheduleId, ScheduleOccurrenceId, WorkspaceId};

/// Exact frozen request selected under runtime writer ownership.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    /// Stable occurrence, linked to at most one build by guarded acceptance.
    #[serde(serialize_with = "serialize_id", deserialize_with = "deserialize_id")]
    pub occurrence: ScheduleOccurrenceId,
    /// Owning schedule, independent of later definition edits.
    #[serde(serialize_with = "serialize_id", deserialize_with = "deserialize_id")]
    pub schedule: ScheduleId,
    /// Frozen template and operational policies.
    pub execution: Value,
    /// Original trigger evidence and exact input pins.
    pub payload: Value,
}
fn invalid() -> StoreError {
    StoreError::InvalidRequest
}
/// Acceptance repeats eligibility and exact frozen evidence in the same transaction as reservations.
pub(crate) async fn guard(
    db: &mut SqliteConnection,
    workspace: &str,
    pending: &Pending,
) -> crate::Result<()> {
    let r = sqlx::query("SELECT o.execution_json,o.payload_json FROM schedule_occurrences o JOIN schedules s ON s.id=o.schedule_id WHERE o.id=? AND o.schedule_id=? AND s.workspace_id=? AND o.disposition='QUEUED' AND o.build_id IS NULL AND (json_extract(o.payload_json,'$.manual')=1 OR (s.paused=0 AND s.needs_review=0 AND s.deleted_at_us IS NULL)) AND NOT EXISTS(SELECT 1 FROM builds WHERE occurrence_id=o.id)")
        .bind(pending.occurrence.to_string()).bind(pending.schedule.to_string()).bind(workspace).fetch_optional(&mut *db).await?.ok_or_else(invalid)?;
    let execution: Value =
        serde_json::from_str(&r.try_get::<String, _>(0)?).map_err(|_| invalid())?;
    let payload: Value =
        serde_json::from_str(&r.try_get::<String, _>(1)?).map_err(|_| invalid())?;
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM schedule_occurrences o JOIN builds b ON b.id=o.build_id WHERE o.schedule_id=? AND b.state IN ('QUEUED','RUNNING','WAITING','STARTING')")
        .bind(pending.schedule.to_string()).fetch_one(&mut *db).await?;
    if active != 0 && pending.execution["policies"]["allow_overlapping_builds"] != true {
        return Err(invalid());
    }
    if execution != pending.execution || payload != pending.payload {
        return Err(invalid());
    }
    Ok(())
}
impl Store {
    /// One eligible pending occurrence; frozen overlap policy controls same-schedule admission.
    pub async fn next_schedule_dispatch(
        &mut self,
        workspace: WorkspaceId,
        at_us: i64,
    ) -> crate::Result<Option<Pending>> {
        Ok(self
            .schedule_dispatch_candidates(workspace, at_us)
            .await?
            .into_iter()
            .next())
    }
    /// Bounded candidate batch; an overlapping writer cannot starve disjoint later requests.
    pub async fn schedule_dispatch_candidates(
        &mut self,
        workspace: WorkspaceId,
        at_us: i64,
    ) -> crate::Result<Vec<Pending>> {
        let r=sqlx::query("SELECT o.id,o.schedule_id,o.execution_json,o.payload_json FROM schedule_occurrences o JOIN schedules s ON s.id=o.schedule_id WHERE s.workspace_id=? AND o.disposition='QUEUED' AND o.build_id IS NULL AND length(o.execution_json)<=262144 AND length(o.payload_json)<=262144 AND (json_extract(o.payload_json,'$.manual')=1 OR (s.paused=0 AND s.needs_review=0 AND s.deleted_at_us IS NULL)) AND (json_extract(o.execution_json,'$.policies.allow_overlapping_builds')=1 OR NOT EXISTS(SELECT 1 FROM schedule_occurrences active JOIN builds b ON b.id=active.build_id WHERE active.schedule_id=o.schedule_id AND b.state IN ('QUEUED','RUNNING','WAITING','STARTING'))) AND (json_extract(o.payload_json,'$.manual')=1 OR NOT EXISTS(SELECT 1 FROM schedule_occurrences previous JOIN builds b ON b.id=previous.build_id WHERE previous.schedule_id=o.schedule_id AND b.finished_at_us IS NOT NULL AND b.finished_at_us + CAST(json_extract(o.execution_json,'$.policies.minimum_delay_seconds') AS INTEGER)*1000000 > ?)) ORDER BY o.logical_fire_at_us,o.id LIMIT 100")
            .bind(workspace.to_string()).bind(at_us).fetch_all(&mut self.db).await?;
        r.into_iter()
            .map(|r| {
                Ok(Pending {
                    occurrence: r.try_get::<String, _>(0)?.parse().map_err(|_| invalid())?,
                    schedule: r.try_get::<String, _>(1)?.parse().map_err(|_| invalid())?,
                    execution: serde_json::from_str(&r.try_get::<String, _>(2)?)
                        .map_err(|_| invalid())?,
                    payload: serde_json::from_str(&r.try_get::<String, _>(3)?)
                        .map_err(|_| invalid())?,
                })
            })
            .collect()
    }
    /// Bounded causal ancestry guard; manual requests explicitly bypass automatic retrigger rules.
    /// Never redispatch a schedule already present in its accepted build ancestry.
    pub async fn check_schedule_dispatch_ancestry(
        &mut self,
        pending: &Pending,
    ) -> crate::Result<()> {
        if pending.payload["manual"] == true {
            return Ok(());
        }
        let max = pending.execution["policies"]["max_consecutive_builds"]
            .as_u64()
            .ok_or_else(invalid)?;
        // A queued external evidence burst is bounded independently of causal hop depth.
        // A later event after the previous build finished begins a fresh burst.
        let ready: Option<i64> =
            sqlx::query_scalar("SELECT logical_fire_at_us FROM schedule_occurrences WHERE id=?")
                .bind(pending.occurrence.to_string())
                .fetch_optional(&mut self.db)
                .await?;
        if let Some(mut ready) = ready {
            let previous: Vec<(i64, i64, String)> = sqlx::query_as("SELECT o.logical_fire_at_us,b.finished_at_us,o.payload_json FROM schedule_occurrences o JOIN builds b ON b.id=o.build_id WHERE o.schedule_id=? AND b.finished_at_us IS NOT NULL ORDER BY b.finished_at_us DESC,b.id DESC LIMIT 100")
                .bind(pending.schedule.to_string()).fetch_all(&mut self.db).await?;
            let mut consecutive = 1u64;
            for (fired, finished, payload) in previous {
                if ready > finished {
                    break;
                }
                let payload: Value = serde_json::from_str(&payload).map_err(|_| invalid())?;
                if payload["manual"] == true {
                    break;
                }
                consecutive += 1;
                if consecutive > max {
                    return Err(invalid());
                }
                ready = fired;
            }
        }
        let mut queue = std::collections::VecDeque::new();
        let correlations = |payload: &Value| -> crate::Result<Vec<String>> {
            let tokens = payload["tokens"].as_array().ok_or_else(invalid)?;
            if tokens.len() > 64 {
                return Err(invalid());
            }
            Ok(tokens
                .iter()
                .filter_map(|t| t["build_id"].as_str().or_else(|| t["correlation"].as_str()))
                .map(str::to_owned)
                .collect())
        };
        queue.extend(
            correlations(&pending.payload)?
                .into_iter()
                .map(|id| (id, 0u64)),
        );
        let mut seen = std::collections::BTreeMap::new();
        while let Some((build, depth)) = queue.pop_front() {
            if depth >= 16 {
                return Err(invalid());
            }
            if seen.get(&build).is_some_and(|old| *old >= depth) {
                continue;
            }
            seen.insert(build.clone(), depth);
            if seen.len() > 1000 {
                return Err(invalid());
            }
            let trigger: Option<String> = sqlx::query_scalar(
                "SELECT trigger_json FROM builds WHERE id=? AND length(trigger_json)<=524288",
            )
            .bind(build)
            .fetch_optional(&mut self.db)
            .await?;
            if let Some(trigger) = trigger {
                let trigger: Value = serde_json::from_str(&trigger).map_err(|_| invalid())?;
                if trigger["kind"] == "schedule" {
                    if trigger["schedule_id"] == pending.schedule.to_string()
                        || depth + 1 >= max
                        || depth + 1 >= 16
                    {
                        return Err(invalid());
                    }
                    queue.extend(
                        correlations(&trigger["evidence"])?
                            .into_iter()
                            .map(|id| (id, depth + 1)),
                    );
                }
            }
        }
        Ok(())
    }
    /// Invalid clock definitions require review without stopping unrelated schedules.
    pub async fn fail_schedule_observation(
        &mut self,
        workspace: WorkspaceId,
        schedule: ScheduleId,
        etag: &str,
        at_us: i64,
    ) -> crate::Result<()> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let changed=sqlx::query("UPDATE schedules SET needs_review=1 WHERE id=? AND workspace_id=? AND etag=? AND needs_review=0").bind(schedule.to_string()).bind(workspace.to_string()).bind(etag).execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule.observation_failed',?,?)").bind(json!({"schedule":schedule.to_string(),"diagnostic":"The schedule clock definition cannot be resolved; save a valid cron expression and IANA timezone"}).to_string()).bind(at_us).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Preparation failure launches no job; preserve frozen request and a bounded safe diagnostic.
    pub async fn fail_schedule_dispatch(
        &mut self,
        workspace: WorkspaceId,
        pending: &Pending,
        diagnostic: &str,
        at_us: i64,
    ) -> crate::Result<()> {
        if diagnostic.is_empty() || diagnostic.len() > 8192 || at_us < 0 {
            return Err(invalid());
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        guard(&mut tx, &workspace.to_string(), pending).await?;
        sqlx::query("UPDATE schedule_occurrences SET disposition='FAILED' WHERE id=?")
            .bind(pending.occurrence.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule.dispatch_failed',?,?)").bind(json!({"schedule":pending.schedule.to_string(),"occurrence":pending.occurrence.to_string(),"diagnostic":diagnostic}).to_string()).bind(at_us).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Bounded recovery of occurrence outcomes after publication or coordinator interruption.
    /// A committed successful build emits its schedule-success event exactly once.
    pub async fn reconcile_schedule_dispatches(
        &mut self,
        workspace: WorkspaceId,
        at_us: i64,
    ) -> crate::Result<usize> {
        let rows:Vec<(String,String,String)>=sqlx::query_as("SELECT o.id,b.id,b.state FROM schedule_occurrences o JOIN schedules s ON s.id=o.schedule_id JOIN builds b ON b.id=o.build_id AND b.occurrence_id=o.id WHERE s.workspace_id=? AND o.disposition='RUNNING' AND b.state NOT IN ('QUEUED','RUNNING','WAITING','STARTING') ORDER BY o.id LIMIT 100").bind(workspace.to_string()).fetch_all(&mut self.db).await?;
        for (occurrence, build, state) in &rows {
            if state == "SUCCEEDED" {
                self.complete_schedule_success(
                    workspace,
                    occurrence.parse().map_err(|_| invalid())?,
                    build.parse::<BuildId>().map_err(|_| invalid())?,
                    at_us,
                )
                .await?;
            } else {
                let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
                sqlx::query("UPDATE schedule_occurrences SET disposition=? WHERE id=? AND build_id=? AND disposition='RUNNING'").bind(if state=="CANCELED" {"CANCELED"} else {"FAILED"}).bind(occurrence).bind(build).execute(&mut *tx).await?;
                sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule.dispatch_completed',?,?)").bind(json!({"occurrence":occurrence,"build":build,"state":state}).to_string()).bind(at_us).execute(&mut *tx).await?;
                tx.commit().await?;
            }
        }
        Ok(rows.len())
    }
}

fn serialize_id<T: std::fmt::Display, S: serde::Serializer>(
    id: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&id.to_string())
}
fn deserialize_id<'de, T: std::str::FromStr, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    String::deserialize(deserializer)?
        .parse()
        .map_err(|_| serde::de::Error::custom("Invalid schedule identity"))
}
