//! Atomic overlap disposition. Frozen payloads/templates and active builds are never rewritten.
use crate::StoreError;
use serde_json::{Value, json};
use sqlx::SqliteConnection;
use tf_domain::{ScheduleId, ScheduleOccurrenceId};
use tf_protocol::schedule::Definition;

pub(crate) async fn apply(
    db: &mut SqliteConnection,
    schedule: ScheduleId,
    definition: &Definition,
    execution: &Value,
    ready_us: i64,
    occurrence: ScheduleOccurrenceId,
    now_us: i64,
) -> crate::Result<&'static str> {
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM schedule_occurrences o JOIN builds b ON b.id=o.build_id WHERE o.schedule_id=? AND b.state IN ('QUEUED','RUNNING','WAITING','STARTING')")
        .bind(schedule.to_string()).fetch_one(&mut *db).await?;
    if active == 0 || definition.policies.allow_overlapping_builds {
        return Ok("QUEUED");
    }
    let policy = definition.policies.overlap_policy.as_str();
    if policy == "queue" {
        return Ok("QUEUED");
    }
    if policy == "skip" {
        record(
            db,
            schedule,
            occurrence,
            "SKIPPED",
            "An occurrence of this schedule is active",
            &[],
            now_us,
        )
        .await?;
        return Ok("SKIPPED");
    }
    if policy != "coalesce_latest" {
        return Err(StoreError::InvalidRequest);
    }
    // Only replace equivalent frozen execution contexts. A later definition edit cannot
    // change or discard a queued request with different source, pins policy or settings.
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT id,logical_fire_at_us FROM schedule_occurrences WHERE schedule_id=? AND disposition='QUEUED' AND build_id IS NULL AND execution_json=? AND json_extract(payload_json,'$.manual') IS NOT 1 ORDER BY logical_fire_at_us DESC,id DESC LIMIT 101")
        .bind(schedule.to_string()).bind(execution.to_string()).fetch_all(&mut *db).await?;
    if rows.len() > 100 {
        return Err(StoreError::InvalidRequest);
    }
    if rows.first().is_some_and(|(_, time)| *time > ready_us) {
        record(
            db,
            schedule,
            occurrence,
            "COALESCED",
            "Newer eligible evidence is already queued",
            &[],
            now_us,
        )
        .await?;
        return Ok("COALESCED");
    }
    let mut replaced = Vec::new();
    for (id, _) in rows {
        sqlx::query("UPDATE schedule_occurrences SET disposition='COALESCED' WHERE id=? AND disposition='QUEUED' AND build_id IS NULL")
            .bind(&id).execute(&mut *db).await?;
        replaced.push(id);
    }
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM schedule_occurrences WHERE schedule_id=? AND disposition IN ('ACCEPTED','QUEUED','HELD')")
        .bind(schedule.to_string()).fetch_one(&mut *db).await?;
    let disposition = if pending >= i64::from(definition.policies.max_pending) {
        "SKIPPED"
    } else {
        "QUEUED"
    };
    record(
        db,
        schedule,
        occurrence,
        disposition,
        if disposition == "SKIPPED" {
            "Pending capacity is occupied by distinct frozen requests"
        } else {
            "Newest eligible evidence replaces equivalent pending requests"
        },
        &replaced,
        now_us,
    )
    .await?;
    Ok(disposition)
}
async fn record(
    db: &mut SqliteConnection,
    schedule: ScheduleId,
    occurrence: ScheduleOccurrenceId,
    disposition: &str,
    reason: &str,
    replaced: &[String],
    at_us: i64,
) -> crate::Result<()> {
    sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule.overlap',?,?)")
        .bind(json!({"schedule":schedule.to_string(),"occurrence":occurrence.to_string(),"disposition":disposition,"reason":reason,"replaced":replaced}).to_string())
        .bind(at_us).execute(db).await?;
    Ok(())
}
