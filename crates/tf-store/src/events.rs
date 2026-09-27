//! Delivery retention is a sequence window over immutable audit evidence, never deletion.
use crate::{Reader, Result, StoreError};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
/// Maximum committed facts replayable on reconnect.
pub const REPLAY_WINDOW: i64 = 10_000;
impl Reader {
    /// One short read transaction, at most 100 facts and one MiB of retained payload.
    /// Missing cursors establish a current checkpoint; expired/future cursors demand resync.
    pub async fn event_page(&mut self, workspace: &str, after: Option<i64>) -> Result<Value> {
        if after.is_some_and(|n| n < 0) {
            return Err(StoreError::InvalidRequest);
        }
        let mut tx = self.db.begin().await?;
        let high: i64 = sqlx::query_scalar("SELECT coalesce(max(sequence),0) FROM events")
            .fetch_one(&mut *tx)
            .await?;
        let floor = high.saturating_sub(REPLAY_WINDOW).max(0);
        let start = after.unwrap_or(high);
        let resync = start < floor || start > high;
        let rows = sqlx::query("WITH page AS (SELECT *,sum(length(CAST(payload_json AS BLOB))) OVER (ORDER BY sequence) AS bytes FROM (SELECT sequence,id,type,CASE WHEN type IN ('dataset.published','dataset.head_changed','build.state','job.state','attempt.state') THEN payload_json ELSE '{}' END AS payload_json,causation_id,correlation_id,wall_time_us FROM events WHERE sequence>? AND sequence<=? ORDER BY sequence LIMIT 100)) SELECT sequence,id,type,payload_json,causation_id,correlation_id,wall_time_us FROM page WHERE bytes<=1048576 ORDER BY sequence")
            .bind(if resync {high} else {start}).bind(high).fetch_all(&mut *tx).await?;
        if rows.is_empty() && start < high && !resync {
            return Err(StoreError::InvalidRequest);
        }
        let mut events = Vec::new();
        let mut bytes = 0;
        let mut next = if resync { high } else { start };
        for r in rows {
            let kind: String = r.try_get(2)?;
            let raw: Option<String> = r.try_get(3)?;
            let raw = raw.ok_or(StoreError::InvalidRequest)?;
            if bytes + raw.len() > 1024 * 1024 {
                break;
            }
            bytes += raw.len();
            let payload: Value =
                serde_json::from_str(&raw).map_err(|_| StoreError::InvalidRequest)?;
            // Only product-authored publication/lifecycle payloads are public. Generic audit
            // appenders may carry private evidence and are intentionally not exposed.
            let public = matches!(
                kind.as_str(),
                "dataset.published"
                    | "dataset.head_changed"
                    | "build.state"
                    | "job.state"
                    | "attempt.state"
            );
            let payload = if public { payload } else { json!({}) };
            let seq: i64 = r.try_get(0)?;
            next = seq;
            events.push(json!({"sequence":seq.to_string(),"id":r.try_get::<String,_>(1)?,"type":kind,"payload":payload,"causation":r.try_get::<Option<String>,_>(4)?,"correlation":r.try_get::<Option<String>,_>(5)?,"timestamp_us":r.try_get::<i64,_>(6)?.to_string(),"workspace":workspace,"context":{"branch":payload["branch"],"source":payload["source"],"branch_id":payload["branch_id"]}}));
        }
        tx.commit().await?;
        Ok(
            json!({"events":events,"cursor":next.to_string(),"high_water":high.to_string(),"retention_floor":floor.to_string(),"resync_required":resync}),
        )
    }
}
