//! Read-only execution projections; never expose worker credentials or diagnostic row samples.
use crate::{Reader, Store, StoreError};
use serde_json::{Value, json};
use sqlx::Row;
use tf_domain::{BuildId, WorkspaceId};
type Result<T> = std::result::Result<T, StoreError>;
fn parse(text: String) -> Result<Value> {
    serde_json::from_str(&text).map_err(|_| StoreError::InvalidRequest)
}
impl Store {
    /// Read one untouched accepted queue at a time; restart does not buffer the backlog.
    pub async fn next_queued_build(&mut self) -> Result<Option<BuildId>> {
        sqlx::query_scalar::<_, String>(
            "SELECT id FROM builds WHERE state='QUEUED' ORDER BY created_at_us,id LIMIT 1",
        )
        .fetch_optional(&mut self.db)
        .await?
        .map(|s| s.parse().map_err(|_| StoreError::InvalidRequest))
        .transpose()
    }
    /// Whether an accepted build is still untouched and can be canceled without workers.
    pub async fn build_is_queued(&mut self, build: BuildId) -> Result<bool> {
        let queued: i64 =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM builds WHERE id=? AND state='QUEUED')")
                .bind(build.to_string())
                .fetch_one(&mut self.db)
                .await?;
        Ok(queued != 0)
    }
    /// Load the exact original accepted plan for a new retained replay.
    pub async fn original_plan(&mut self, build: BuildId) -> Result<crate::planning::DraftPlan> {
        let id: String = sqlx::query_scalar("SELECT plan_id FROM builds WHERE id=?")
            .bind(build.to_string())
            .fetch_one(&mut self.db)
            .await?;
        self.load_draft(id.parse().map_err(|_| StoreError::InvalidRequest)?)
            .await
            .map_err(|_| StoreError::InvalidRequest)
    }
}
impl Reader {
    /// Stable UUID cursor pagination over builds; rows include source, branch and status.
    pub async fn builds(
        &mut self,
        workspace: WorkspaceId,
        limit: u16,
        after: Option<BuildId>,
    ) -> Result<Value> {
        if !(1..=100).contains(&limit) {
            return Err(StoreError::InvalidRequest);
        }
        let rows=sqlx::query("SELECT b.id,b.state,b.created_at_us,b.finished_at_us,p.source_snapshot_id,p.context_json,(SELECT d.name FROM jobs j JOIN data_branches d ON d.id=j.branch_id WHERE j.build_id=b.id LIMIT 1) AS branch FROM builds b JOIN build_plans p ON p.id=b.plan_id JOIN source_snapshots s ON s.id=p.source_snapshot_id WHERE s.workspace_id=? AND b.id>? ORDER BY b.id LIMIT ?").bind(workspace.to_string()).bind(after.map(|id|id.to_string()).unwrap_or_default()).bind(i64::from(limit)+1).fetch_all(&mut self.db).await?;
        let more = rows.len() > usize::from(limit);
        let mut builds = vec![];
        for r in rows.iter().take(usize::from(limit)) {
            builds.push(json!({"id":r.try_get::<String,_>("id")?,"state":r.try_get::<String,_>("state")?,"source":r.try_get::<String,_>("source_snapshot_id")?,"branch":r.try_get::<String,_>("branch")?,"created_us":r.try_get::<i64,_>("created_at_us")?.to_string(),"finished_us":r.try_get::<Option<i64>,_>("finished_at_us")?.map(|n|n.to_string())}));
        }
        let next = if more {
            builds.last().map(|v| v["id"].clone())
        } else {
            None
        };
        Ok(json!({"builds":builds,"next_cursor":next}))
    }
    /// Complete accepted execution evidence with attempts, exact pins, checks and measured phases.
    pub async fn build_report(&mut self, workspace: WorkspaceId, build: BuildId) -> Result<Value> {
        // One read transaction gives a consistent projection while a coordinator publishes.
        use sqlx::Connection;
        let mut tx = self.db.begin().await?;
        let r=sqlx::query("SELECT b.*,p.candidate_json,p.source_snapshot_id FROM builds b JOIN build_plans p ON p.id=b.plan_id JOIN source_snapshots s ON s.id=p.source_snapshot_id WHERE b.id=? AND s.workspace_id=?").bind(build.to_string()).bind(workspace.to_string()).fetch_one(&mut *tx).await?;
        let plan = parse(r.try_get("candidate_json")?)?;
        let rows=sqlx::query("SELECT j.*,d.name AS branch FROM jobs j JOIN data_branches d ON d.id=j.branch_id WHERE j.build_id=? ORDER BY j.id").bind(build.to_string()).fetch_all(&mut *tx).await?;
        let mut jobs = vec![];
        for j in rows {
            let id: String = j.try_get("id")?;
            let rows = sqlx::query("SELECT * FROM attempts WHERE job_id=? ORDER BY attempt_no")
                .bind(&id)
                .fetch_all(&mut *tx)
                .await?;
            let mut attempts = vec![];
            for a in rows {
                let aid: String = a.try_get("id")?;
                let phases=sqlx::query("SELECT * FROM phase_intervals WHERE attempt_id=? ORDER BY sequence").bind(&aid).fetch_all(&mut *tx).await?.iter().map(|p|Ok(json!({"phase":p.try_get::<String,_>("phase")?,"started_us":p.try_get::<i64,_>("started_at_us")?.to_string(),"finished_us":p.try_get::<Option<i64>,_>("finished_at_us")?.map(|n|n.to_string()),"duration_ns":p.try_get::<Option<i64>,_>("duration_ns")?.map(|n|n.to_string())}))).collect::<Result<Vec<_>>>()?;
                let checks = checks(&mut tx, &aid).await?;
                attempts.push(json!({"id":aid,"number":a.try_get::<i64,_>("attempt_no")?,"state":a.try_get::<String,_>("state")?,"failure_class":a.try_get::<Option<String>,_>("failure_class")?,"started_us":a.try_get::<i64,_>("started_at_us")?.to_string(),"finished_us":a.try_get::<Option<i64>,_>("finished_at_us")?.map(|n|n.to_string()),"phases":phases,"checks":checks,"report":a.try_get::<Option<String>,_>("process_json")?.map(parse).transpose()?}));
            }
            let inputs=sqlx::query("SELECT alias,coalesce(version_id,foreign_version_id),binding_json FROM job_inputs WHERE job_id=? ORDER BY alias").bind(&id).fetch_all(&mut *tx).await?.iter().map(|i|Ok(json!({"alias":i.try_get::<String,_>(0)?,"version":i.try_get::<String,_>(1)?,"binding":parse(i.try_get(2)?)?}))).collect::<Result<Vec<_>>>()?;
            let produced:Option<String>=sqlx::query_scalar("SELECT v.id FROM dataset_versions v JOIN attempts a ON a.id=v.attempt_id WHERE a.job_id=? AND a.state='SUCCEEDED'").bind(&id).fetch_optional(&mut *tx).await?;
            let reused = sqlx::query(
                "SELECT version_id,original_attempt_id FROM cached_jobs WHERE job_id=?",
            )
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
            let (version, cached_from) = if let Some(c) = reused {
                let version: String = c.try_get(0)?;
                let original: String = c.try_get(1)?;
                (
                    Some(version.clone()),
                    json!({"version":version,"attempt":original,"checks":checks(&mut tx,&original).await?}),
                )
            } else {
                (produced, Value::Null)
            };
            jobs.push(json!({"id":id,"dataset":j.try_get::<String,_>("dataset_id")?,"branch":j.try_get::<String,_>("branch")?,"state":j.try_get::<String,_>("state")?,"inputs":inputs,"attempts":attempts,"version":version,"cached_from":cached_from}));
        }
        let result = json!({"id":build.to_string(),"state":r.try_get::<String,_>("state")?,"source":r.try_get::<String,_>("source_snapshot_id")?,"cancel_requested":r.try_get::<i64,_>("cancel_requested")?!=0,"created_us":r.try_get::<i64,_>("created_at_us")?.to_string(),"finished_us":r.try_get::<Option<i64>,_>("finished_at_us")?.map(|n|n.to_string()),"plan":plan,"jobs":jobs});
        tx.commit().await?;
        Ok(result)
    }
}

async fn checks(db: &mut sqlx::SqliteConnection, attempt: &str) -> Result<Vec<Value>> {
    let rows=sqlx::query("SELECT r.id,r.outcome,r.metrics_json,r.started_at_us,r.finished_at_us,d.stable_id,d.phase,d.target_alias,d.policy_json FROM check_results r JOIN check_definitions d ON d.fingerprint=r.definition_fingerprint WHERE r.attempt_id=? ORDER BY r.id").bind(attempt).fetch_all(db).await?;
    rows.iter().map(|c|Ok(json!({
        "id":c.try_get::<String,_>("id")?, "name":c.try_get::<String,_>("stable_id")?,
        "phase":c.try_get::<String,_>("phase")?, "alias":c.try_get::<Option<String>,_>("target_alias")?,
        "outcome":c.try_get::<String,_>("outcome")?, "metrics":parse(c.try_get("metrics_json")?)?,
        "policy":parse(c.try_get("policy_json")?)?,
        "started_us":c.try_get::<i64,_>("started_at_us")?.to_string(),
        "finished_us":c.try_get::<Option<i64>,_>("finished_at_us")?.map(|n|n.to_string())
    }))).collect()
}
