//! Historical total build duration estimates, separate from execution and ETA.
use crate::{Reader, Result, StoreError};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
use tf_domain::{BuildId, WorkspaceId};

impl Reader {
    /// Mean accepted-to-finished wall duration of earlier successful builds with
    /// the same workspace, output branch and exact dataset job set. Cache reuse
    /// is included as a measured build operation, never as execution duration.
    pub async fn build_duration_estimate(
        &mut self,
        workspace: WorkspaceId,
        build: BuildId,
        window: usize,
    ) -> Result<Value> {
        if !(1..=1000).contains(&window) {
            return Err(StoreError::InvalidRequest);
        }
        let mut tx = self.db.begin().await?;
        let current = sqlx::query("SELECT b.created_at_us,json_extract(p.candidate_json,'$.output.name') AS branch FROM builds b JOIN build_plans p ON p.id=b.plan_id JOIN source_snapshots s ON s.id=p.source_snapshot_id WHERE b.id=? AND s.workspace_id=?")
            .bind(build.to_string()).bind(workspace.to_string()).fetch_one(&mut *tx).await?;
        let accepted: i64 = current.try_get("created_at_us")?;
        let branch: String = current.try_get("branch")?;
        let datasets: Vec<String> = sqlx::query_scalar(
            "SELECT dataset_id FROM jobs WHERE build_id=? ORDER BY dataset_id LIMIT 2001",
        )
        .bind(build.to_string())
        .fetch_all(&mut *tx)
        .await?;
        if datasets.len() > 2000 {
            return Err(StoreError::InvalidRequest);
        }
        let rows = if datasets.is_empty() {
            Vec::new()
        } else {
            let profile =
                serde_json::to_string(&datasets).map_err(|_| StoreError::InvalidRequest)?;
            sqlx::query("SELECT b.created_at_us,b.finished_at_us FROM builds b JOIN build_plans p ON p.id=b.plan_id JOIN source_snapshots s ON s.id=p.source_snapshot_id WHERE s.workspace_id=? AND json_extract(p.candidate_json,'$.output.name')=? AND b.state='SUCCEEDED' AND (b.created_at_us<? OR (b.created_at_us=? AND b.id<?)) AND (b.finished_at_us IS NULL OR b.finished_at_us<=?) AND (SELECT count(*) FROM jobs j WHERE j.build_id=b.id)=? AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.build_id=b.id AND j.dataset_id NOT IN (SELECT value FROM json_each(?))) ORDER BY b.created_at_us DESC,b.id DESC LIMIT ?")
                .bind(workspace.to_string()).bind(branch).bind(accepted).bind(accepted).bind(build.to_string()).bind(accepted).bind(datasets.len() as i64).bind(profile).bind(window as i64).fetch_all(&mut *tx).await?
        };
        let mut sum = 0_u128;
        let mut missing = 0_usize;
        for row in &rows {
            let start: i64 = row.try_get("created_at_us")?;
            let end: Option<i64> = row.try_get("finished_at_us")?;
            if let Some(duration) = end
                .and_then(|end| end.checked_sub(start))
                .filter(|n| *n >= 0)
            {
                sum = sum
                    .checked_add(duration as u128)
                    .ok_or(StoreError::InvalidRequest)?;
            } else {
                missing += 1;
            }
        }
        let mean = if rows.is_empty() || missing != 0 {
            Value::Null
        } else {
            json!({"numerator":sum.to_string(),"denominator":rows.len().to_string()})
        };
        let value = json!({"window":window.to_string(),"samples":rows.len().to_string(),"missing_samples":missing.to_string(),"mean_us":mean});
        tx.commit().await?;
        Ok(value)
    }
}
