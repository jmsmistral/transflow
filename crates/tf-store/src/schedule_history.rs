//! Bounded schedule history and exact operation metrics over one read snapshot.
use crate::{Reader, StoreError};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::BTreeMap;
use tf_domain::{ScheduleId, ScheduleOccurrenceId, WorkspaceId};

fn invalid() -> StoreError {
    StoreError::InvalidRequest
}
async fn exists(
    db: &mut SqliteConnection,
    workspace: WorkspaceId,
    id: ScheduleId,
) -> crate::Result<()> {
    let found: i64 =
        sqlx::query_scalar("SELECT count(*) FROM schedules WHERE id=? AND workspace_id=?")
            .bind(id.to_string())
            .bind(workspace.to_string())
            .fetch_one(db)
            .await?;
    if found != 1 {
        return Err(invalid());
    }
    Ok(())
}
fn ratio(value: Option<tf_domain::history::Ratio>) -> Value {
    value.map(|r|json!({"numerator":r.numerator.to_string(),"denominator":r.denominator.to_string()})).unwrap_or(Value::Null)
}
impl Reader {
    /// Include tombstoned schedules when reading their retained evidence.
    pub async fn schedule_presence(
        &mut self,
        workspace: WorkspaceId,
        id: ScheduleId,
    ) -> crate::Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM schedules WHERE id=? AND workspace_id=?",
        )
        .bind(id.to_string())
        .bind(workspace.to_string())
        .fetch_one(&mut self.db)
        .await?
            == 1)
    }

    /// Exclusive descending UUID cursor is scoped and resolved to its retained ready time.
    /// Tombstoned definitions keep their history; metadata reads perform no discovery.
    pub async fn schedule_history(
        &mut self,
        workspace: WorkspaceId,
        id: ScheduleId,
        after: Option<ScheduleOccurrenceId>,
        limit: usize,
    ) -> crate::Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(invalid());
        }
        let mut tx = self.db.begin().await?;
        exists(&mut tx, workspace, id).await?;
        let cutoff: Option<i64> = if let Some(after) = after {
            Some(sqlx::query_scalar("SELECT logical_fire_at_us FROM schedule_occurrences WHERE id=? AND schedule_id=?")
                .bind(after.to_string()).bind(id.to_string()).fetch_optional(&mut *tx).await?.ok_or_else(invalid)?)
        } else {
            None
        };
        let rows = sqlx::query("SELECT o.id,o.logical_fire_at_us,o.disposition,o.build_id,o.payload_json,o.execution_json,b.plan_id,p.source_snapshot_id,b.created_at_us,b.finished_at_us,b.state AS build_state,(SELECT count(*) FROM jobs WHERE build_id=b.id) AS jobs,(SELECT count(*) FROM attempts a JOIN jobs j ON j.id=a.job_id WHERE j.build_id=b.id) AS attempts FROM schedule_occurrences o LEFT JOIN builds b ON b.id=o.build_id LEFT JOIN build_plans p ON p.id=b.plan_id WHERE o.schedule_id=? AND (? IS NULL OR o.logical_fire_at_us<? OR (o.logical_fire_at_us=? AND o.id<?)) ORDER BY o.logical_fire_at_us DESC,o.id DESC LIMIT ?")
            .bind(id.to_string()).bind(cutoff).bind(cutoff).bind(cutoff).bind(after.map(|v|v.to_string())).bind((limit+1) as i64).fetch_all(&mut *tx).await?;
        let more = rows.len() > limit;
        let mut entries = Vec::new();
        for row in rows.iter().take(limit) {
            let payload_text: String = row.try_get("payload_json")?;
            let execution_text: String = row.try_get("execution_json")?;
            if payload_text.len() > 262144 || execution_text.len() > 262144 {
                return Err(invalid());
            }
            let payload: Value = serde_json::from_str(&payload_text).map_err(|_| invalid())?;
            let execution: Value = serde_json::from_str(&execution_text).map_err(|_| invalid())?;
            let start: Option<i64> = row.try_get("created_at_us")?;
            let end: Option<i64> = row.try_get("finished_at_us")?;
            let duration = start
                .zip(end)
                .and_then(|(s, e)| e.checked_sub(s))
                .filter(|n| *n >= 0);
            let manual = if payload["manual"] == true {
                Some(true)
            } else if payload["format_version"] == 1 {
                Some(false)
            } else {
                None
            };
            let coalesced = payload["coalesced"].as_array().and_then(|v| {
                v.iter().try_fold(0u128, |sum, leaf| {
                    sum.checked_add(leaf["count"].as_str()?.parse().ok()?)
                })
            });
            entries.push(json!({"id":row.try_get::<String,_>("id")?,"ready_us":row.try_get::<i64,_>("logical_fire_at_us")?.to_string(),"disposition":row.try_get::<String,_>("disposition")?,"build":row.try_get::<Option<String>,_>("build_id")?,"branch":execution["build"]["data_branch"].as_str(),"plan":row.try_get::<Option<String>,_>("plan_id")?,"source":row.try_get::<Option<String>,_>("source_snapshot_id")?,"build_state":row.try_get::<Option<String>,_>("build_state")?,"manual":manual,"started_us":start.map(|v|v.to_string()),"finished_us":end.map(|v|v.to_string()),"wall_duration_us":duration.map(|v|v.to_string()),"jobs":row.try_get::<i64,_>("jobs")?.to_string(),"attempts":row.try_get::<i64,_>("attempts")?.to_string(),"coalesced_tokens":coalesced.map(|v|v.to_string())}));
        }
        tx.commit().await?;
        let cursor = more
            .then(|| entries.last().map(|v| v["id"].clone()))
            .flatten();
        Ok(json!({"schedule_id":id.to_string(),"occurrences":entries,"next_cursor":cursor}))
    }
    /// Ready-time occurrence cohort; wall durations are operation time, including cache reuse,
    /// never inferred execution duration. Observation counts use their separately labelled window.
    pub async fn schedule_metrics(
        &mut self,
        workspace: WorkspaceId,
        id: ScheduleId,
        from: i64,
        to: i64,
        window: usize,
    ) -> crate::Result<Value> {
        if from < 0 || to <= from || !(1..=1000).contains(&window) {
            return Err(invalid());
        }
        let mut tx = self.db.begin().await?;
        exists(&mut tx, workspace, id).await?;
        let rows = sqlx::query("SELECT o.disposition,o.build_id,o.logical_fire_at_us,b.state,b.created_at_us,b.finished_at_us,(SELECT count(*) FROM jobs j WHERE j.build_id=b.id) AS jobs,(SELECT count(*) FROM attempts a JOIN jobs j ON j.id=a.job_id WHERE j.build_id=b.id) AS attempts FROM schedule_occurrences o LEFT JOIN builds b ON b.id=o.build_id LEFT JOIN build_plans p ON p.id=b.plan_id WHERE o.schedule_id=? AND o.logical_fire_at_us>=? AND o.logical_fire_at_us<? ORDER BY b.finished_at_us,o.id LIMIT 1001")
            .bind(id.to_string()).bind(from).bind(to).fetch_all(&mut *tx).await?;
        if rows.len() > 1000 {
            return Err(invalid());
        }
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        let mut samples = Vec::new();
        let mut missing = 0usize;
        let mut unknown_completion_order = false;
        let mut builds = 0u64;
        let mut jobs = 0i64;
        let mut attempts = 0i64;
        let mut succeeded = 0u64;
        let mut failed = 0u64;
        for row in &rows {
            *counts.entry(row.try_get("disposition")?).or_default() += 1;
            if row.try_get::<Option<String>, _>("build_id")?.is_none() {
                continue;
            }
            builds += 1;
            jobs = jobs
                .checked_add(row.try_get::<i64, _>("jobs")?)
                .ok_or_else(invalid)?;
            attempts = attempts
                .checked_add(row.try_get::<i64, _>("attempts")?)
                .ok_or_else(invalid)?;
            if jobs > 10000 || attempts > 10000 {
                return Err(invalid());
            }
            match row.try_get::<Option<String>, _>("state")?.as_deref() {
                Some("FAILED") => failed += 1,
                Some("SUCCEEDED") => {
                    succeeded += 1;
                    if row.try_get::<Option<i64>, _>("finished_at_us")?.is_none() {
                        unknown_completion_order = true;
                    }
                    let duration = row
                        .try_get::<Option<i64>, _>("created_at_us")?
                        .zip(row.try_get::<Option<i64>, _>("finished_at_us")?)
                        .and_then(|(s, e)| e.checked_sub(s))
                        .and_then(|n| u128::try_from(n).ok());
                    if duration.is_none() {
                        missing += 1;
                    }
                    samples.push(duration);
                }
                _ => {}
            }
        }
        let ignored: Vec<i64> = sqlx::query_scalar("SELECT coalesce(CAST(json_extract(evidence_json,'$.ignored') AS INTEGER),0) FROM audit_log WHERE operation='schedule_events_scanned' AND json_extract(evidence_json,'$.schedule')=? AND wall_time_us>=? AND wall_time_us<? LIMIT 100001")
            .bind(id.to_string()).bind(from).bind(to).fetch_all(&mut *tx).await?;
        if ignored.len() > 100000 || ignored.iter().any(|n| *n < 0) {
            return Err(invalid());
        }
        let clock: Vec<String> = sqlx::query_scalar("SELECT disposition FROM schedule_clock_ticks WHERE schedule_id=? AND at_us>=? AND at_us<? LIMIT 100001")
            .bind(id.to_string()).bind(from).bind(to).fetch_all(&mut *tx).await?;
        if clock.len() > 100000 {
            return Err(invalid());
        }
        let mut ticks: BTreeMap<String, u64> = BTreeMap::new();
        for state in clock {
            *ticks.entry(state).or_default() += 1;
        }
        tx.commit().await?;
        let measured: Vec<_> = samples.iter().copied().flatten().collect();
        let stats = tf_domain::history::statistics(&measured, window).ok_or_else(invalid)?;
        let latest: Vec<_> = samples.iter().rev().take(window).copied().collect();
        let trailing = if unknown_completion_order || latest.iter().any(Option::is_none) {
            None
        } else {
            tf_domain::history::statistics(
                &latest.into_iter().flatten().collect::<Vec<_>>(),
                window,
            )
            .ok_or_else(invalid)?
            .trailing_mean
        };
        Ok(
            json!({"schedule_id":id.to_string(),"from_us":from.to_string(),"to_us":to.to_string(),"cohort":"occurrence_ready","duration_unit":"us","occurrences":rows.len().to_string(),"dispositions":counts.into_iter().map(|(state,count)|json!({"state":state,"count":count.to_string()})).collect::<Vec<_>>(),"builds":builds.to_string(),"jobs":jobs.to_string(),"attempts":attempts.to_string(),"failure_rate":(succeeded+failed>0).then(||json!({"numerator":failed.to_string(),"denominator":(failed+succeeded).to_string()})),"duration_samples":stats.samples.to_string(),"missing_duration_samples":missing.to_string(),"median_us":ratio(stats.median),"trailing_mean_us":ratio(trailing),"trailing_window":window.to_string(),"trailing_samples":samples.len().min(window).to_string(),"ignored_event_matches":ignored.into_iter().map(|n|n as u128).sum::<u128>().to_string(),"clock_ticks":ticks.into_iter().map(|(state,count)|json!({"state":state,"count":count.to_string()})).collect::<Vec<_>>(),"evidence_cohort":"observed_in_range"}),
        )
    }
}
