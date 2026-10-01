//! Retained execution evidence only: no source imports, artifact scans or provider access.
use crate::{Reader, Result, StoreError};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
use std::collections::BTreeMap;
use tf_domain::{
    AttemptId, BranchName, BuildId, DatasetId, WorkspaceId,
    history::{Ratio, TimedJob},
};

fn invalid() -> StoreError {
    StoreError::InvalidRequest
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().ok_or_else(invalid)
}
fn array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    v[key].as_array().ok_or_else(invalid)
}
fn number(v: &Value) -> Option<u128> {
    v.as_str()?.parse().ok()
}
fn ratio(r: Option<Ratio>) -> Value {
    r.map(|r| json!({"numerator":r.numerator.to_string(),"denominator":r.denominator.to_string()}))
        .unwrap_or(Value::Null)
}
fn terminal(state: &str) -> bool {
    matches!(state, "SUCCEEDED" | "FAILED" | "CANCELED" | "INTERRUPTED")
}
/// Monotonic phase sum only when every phase is closed and measured. Never infer from wall time.
pub fn attempt_duration(attempt: &Value) -> Option<u128> {
    if !terminal(attempt["state"].as_str()?) {
        return None;
    }
    let phases = attempt["phases"].as_array()?;
    if phases.is_empty() {
        return None;
    }
    phases.iter().try_fold(0_u128, |sum, p| {
        p["finished_us"].as_str()?;
        sum.checked_add(number(&p["duration_ns"])?)
    })
}
/// Build Gantt uses recorded phases; cached/blocked jobs have no execution duration.
pub fn timeline(report: &Value) -> Result<Value> {
    let jobs = array(report, "jobs")?;
    let indices: BTreeMap<_, _> = jobs
        .iter()
        .enumerate()
        .map(|(i, j)| Ok((text(j, "dataset")?.to_owned(), i)))
        .collect::<Result<_>>()?;
    let work = array(&report["plan"], "writes")?;
    let mut timed = vec![];
    let mut entries = vec![];
    let mut counts = BTreeMap::<String, usize>::new();
    let mut first_start = None::<u128>;
    for job in jobs {
        let state = text(job, "state")?;
        *counts.entry(state.to_owned()).or_default() += 1;
        let attempts = array(job, "attempts")?;
        let mut durations = vec![];
        let mut intervals = vec![];
        for attempt in attempts {
            if let Some(n) = number(&attempt["started_us"]) {
                first_start = Some(first_start.map_or(n, |old| old.min(n)));
            }
            let duration = attempt_duration(attempt);
            durations.push(duration);
            intervals.push(json!({"attempt":attempt["id"],"number":attempt["number"].as_u64().ok_or_else(invalid)?.to_string(),"state":attempt["state"],"started_us":attempt["started_us"],"finished_us":attempt["finished_us"],"duration_ns":duration.map(|n|n.to_string()),"phases":attempt["phases"]}));
        }
        let duration = if attempts.is_empty() {
            None
        } else {
            durations
                .iter()
                .try_fold(0_u128, |sum, n| sum.checked_add((*n)?))
        };
        let mut parents = vec![];
        let w = work
            .iter()
            .find(|w| w["dataset"] == job["dataset"])
            .ok_or_else(invalid)?;
        for b in array(w, "bindings")? {
            if b["kind"] == "planned" {
                parents.push(*indices.get(text(b, "dataset")?).ok_or_else(invalid)?);
            }
        }
        parents.sort_unstable();
        parents.dedup();
        let weight = if attempts.is_empty()
            && matches!(state, "CACHED" | "BLOCKED" | "CANCELED" | "INTERRUPTED")
        {
            Some(0)
        } else {
            duration
        };
        timed.push(TimedJob {
            duration_ns: weight,
            parents,
        });
        entries.push(json!({"job":job["id"],"dataset":job["dataset"],"state":state,"duration_ns":duration.map(|n|n.to_string()),"attempts":intervals}));
    }
    let path = terminal(text(report, "state")?)
        .then(|| tf_domain::history::critical_path(&timed))
        .flatten();
    let critical = path.map(|(n,path)|json!({"duration_ns":n.to_string(),"jobs":path.iter().filter_map(|&i| jobs.get(i).map(|j|j["id"].clone())).collect::<Vec<_>>()}));
    let created = number(&report["created_us"]).ok_or_else(invalid)?;
    Ok(
        json!({"build":report["id"],"state":report["state"],"plan":report["plan"]["id"],"source":report["source"],"queued_us":report["created_us"],"started_us":first_start.map(|n|n.to_string()),"finished_us":report["finished_us"],"wall_duration_us":number(&report["finished_us"]).and_then(|n|n.checked_sub(created)).map(|n|n.to_string()),"initial_queue_wait_us":first_start.and_then(|n|n.checked_sub(created)).map(|n|n.to_string()),"resource_wait_us":null,"eta_us":null,"critical_path":critical,"job_counts":counts.into_iter().map(|(state,n)|json!({"state":state,"count":n.to_string()})).collect::<Vec<_>>(),"jobs":entries}),
    )
}

/// Bounded metrics cohort: builds accepted in [from_us,to_us), with optional dataset filter.
pub struct MetricsQuery<'a> {
    /// Workspace identity.
    pub workspace: WorkspaceId,
    /// Exact output branch; never fallback.
    pub branch: &'a BranchName,
    /// Inclusive UTC microsecond acceptance time.
    pub from_us: i64,
    /// Exclusive UTC microsecond acceptance time.
    pub to_us: i64,
    /// Only builds containing this dataset and only this dataset's jobs.
    pub dataset: Option<DatasetId>,
    /// Restrict to builds that produced at least one committed version.
    pub materialized_any: bool,
    /// Trailing success sample limit (1..1000; API default 10).
    pub window: usize,
}
impl Reader {
    /// Dataset job history includes cache reuse and all retry summaries, independently of versions.
    pub async fn dataset_history(
        &mut self,
        workspace: WorkspaceId,
        branch: &BranchName,
        dataset: DatasetId,
        after: &str,
        limit: u32,
    ) -> Result<Vec<Value>> {
        if !(1..=201).contains(&limit) {
            return Err(invalid());
        }
        let mut tx = self.db.begin().await?;
        let rows = sqlx::query("SELECT j.id,j.build_id,j.state,p.id AS plan,p.source_snapshot_id,b.created_at_us,b.finished_at_us,c.version_id AS reused_version,c.original_attempt_id,(SELECT count(*) FROM attempts a WHERE a.job_id=j.id) AS attempt_count,(SELECT v.id FROM dataset_versions v JOIN attempts a ON a.id=v.attempt_id WHERE a.job_id=j.id LIMIT 1) AS produced_version FROM jobs j JOIN data_branches d ON d.id=j.branch_id JOIN builds b ON b.id=j.build_id JOIN build_plans p ON p.id=b.plan_id LEFT JOIN cached_jobs c ON c.job_id=j.id WHERE d.workspace_id=? AND d.name=? AND j.dataset_id=? AND j.id>? ORDER BY j.id LIMIT ?")
            .bind(workspace.to_string()).bind(branch.as_str()).bind(dataset.to_string()).bind(after).bind(limit).fetch_all(&mut *tx).await?;
        let mut entries = vec![];
        let mut total_attempts = 0_usize;
        let mut total_phases = 0_usize;
        for r in rows {
            let job: String = r.try_get("id")?;
            let records = sqlx::query("SELECT id,attempt_no,state,started_at_us,finished_at_us FROM attempts WHERE job_id=? ORDER BY attempt_no LIMIT 10001")
                .bind(&job).fetch_all(&mut *tx).await?;
            total_attempts += records.len();
            if total_attempts > 10000 {
                return Err(invalid());
            }
            let mut attempts = vec![];
            for a in records {
                let id: String = a.try_get("id")?;
                let phases = sqlx::query("SELECT phase,started_at_us,finished_at_us,duration_ns FROM phase_intervals WHERE attempt_id=? ORDER BY sequence LIMIT 1001")
                    .bind(&id).fetch_all(&mut *tx).await?;
                total_phases += phases.len();
                if phases.len() > 1000 || total_phases > 50000 {
                    return Err(invalid());
                }
                let phases = phases.iter().map(|p| Ok(json!({"phase":p.try_get::<String,_>("phase")?,"started_us":p.try_get::<i64,_>("started_at_us")?.to_string(),"finished_us":p.try_get::<Option<i64>,_>("finished_at_us")?.map(|n|n.to_string()),"duration_ns":p.try_get::<Option<i64>,_>("duration_ns")?.map(|n|n.to_string())}))).collect::<Result<Vec<_>>>()?;
                let evidence = json!({"state":a.try_get::<String,_>("state")?,"phases":phases});
                attempts.push(json!({"attempt":id,"number":a.try_get::<i64,_>("attempt_no")?.to_string(),"state":evidence["state"],"started_us":a.try_get::<i64,_>("started_at_us")?.to_string(),"finished_us":a.try_get::<Option<i64>,_>("finished_at_us")?.map(|n|n.to_string()),"duration_ns":attempt_duration(&evidence).map(|n|n.to_string()),"phases":phases}));
            }
            let duration = if attempts.is_empty() {
                None
            } else {
                attempts
                    .iter()
                    .try_fold(0_u128, |sum, a| sum.checked_add(number(&a["duration_ns"])?))
            };
            entries.push(json!({"id":job,"build":r.try_get::<String,_>("build_id")?,"state":r.try_get::<String,_>("state")?,"plan":r.try_get::<String,_>("plan")?,"source":r.try_get::<String,_>("source_snapshot_id")?,"created_us":r.try_get::<i64,_>("created_at_us")?.to_string(),"finished_us":r.try_get::<Option<i64>,_>("finished_at_us")?.map(|n|n.to_string()),"attempt_count":r.try_get::<i64,_>("attempt_count")?.to_string(),"produced_version":r.try_get::<Option<String>,_>("produced_version")?,"reused_version":r.try_get::<Option<String>,_>("reused_version")?,"original_attempt":r.try_get::<Option<String>,_>("original_attempt_id")?,"duration_ns":duration.map(|n|n.to_string()),"attempts":attempts}));
        }
        if serde_json::to_vec(&entries).map_err(|_| invalid())?.len() > 32 * 1024 * 1024 {
            return Err(invalid());
        }
        tx.commit().await?;
        Ok(entries)
    }
    /// Find the owning build/branch/source without guessing from the current dataset head.
    pub async fn attempt_build(
        &mut self,
        workspace: WorkspaceId,
        attempt: AttemptId,
    ) -> Result<BuildId> {
        let id:String=sqlx::query_scalar("SELECT j.build_id FROM attempts a JOIN jobs j ON j.id=a.job_id JOIN data_branches d ON d.id=j.branch_id WHERE a.id=? AND d.workspace_id=?").bind(attempt.to_string()).bind(workspace.to_string()).fetch_one(&mut self.db).await?;
        id.parse().map_err(|_| invalid())
    }
    /// Exact metrics over a single read transaction, rejecting oversized cohorts instead of truncating.
    pub async fn execution_metrics(&mut self, q: MetricsQuery<'_>) -> Result<Value> {
        if q.from_us < 0 || q.to_us <= q.from_us || !(1..=1000).contains(&q.window) {
            return Err(invalid());
        }
        let mut tx = self.db.begin().await?;
        let rows=sqlx::query("SELECT b.id,b.state,b.occurrence_id FROM builds b JOIN build_plans p ON p.id=b.plan_id JOIN source_snapshots s ON s.id=p.source_snapshot_id WHERE s.workspace_id=? AND b.created_at_us>=? AND b.created_at_us<? AND EXISTS(SELECT 1 FROM jobs j JOIN data_branches d ON d.id=j.branch_id WHERE j.build_id=b.id AND d.name=? AND (? IS NULL OR j.dataset_id=?)) AND (?=0 OR EXISTS(SELECT 1 FROM dataset_versions v JOIN attempts a ON a.id=v.attempt_id JOIN jobs j ON j.id=a.job_id WHERE j.build_id=b.id)) ORDER BY b.created_at_us,b.id LIMIT 1001")
            .bind(q.workspace.to_string()).bind(q.from_us).bind(q.to_us).bind(q.branch.as_str()).bind(q.dataset.map(|d|d.to_string())).bind(q.dataset.map(|d|d.to_string())).bind(q.materialized_any).fetch_all(&mut *tx).await?;
        if rows.len() > 1000 {
            return Err(invalid());
        }
        let mut states = BTreeMap::<String, usize>::new();
        let mut job_states = BTreeMap::<String, usize>::new();
        let mut samples = vec![];
        let mut missing = 0_usize;
        let mut attempts = 0_usize;
        let mut executed = 0_usize;
        let mut jobs = 0_usize;
        let mut manual = 0_usize;
        let mut scheduled = 0_usize;
        for b in &rows {
            *states.entry(b.try_get("state")?).or_default() += 1;
            if b.try_get::<Option<String>, _>("occurrence_id")?.is_some() {
                scheduled += 1;
            } else {
                manual += 1;
            }
            let records=sqlx::query("SELECT j.id,j.state,a.id AS attempt,a.state AS attempt_state,a.finished_at_us,v.id AS version,(SELECT count(*) FROM phase_intervals p WHERE p.attempt_id=a.id) AS phases,(SELECT count(*) FROM phase_intervals p WHERE p.attempt_id=a.id AND (p.duration_ns IS NULL OR p.finished_at_us IS NULL)) AS incomplete FROM jobs j LEFT JOIN attempts a ON a.job_id=j.id LEFT JOIN dataset_versions v ON v.attempt_id=a.id WHERE j.build_id=? AND (? IS NULL OR j.dataset_id=?) ORDER BY j.id,a.attempt_no LIMIT 10001")
                .bind(b.try_get::<String,_>("id")?).bind(q.dataset.map(|d|d.to_string())).bind(q.dataset.map(|d|d.to_string())).fetch_all(&mut *tx).await?;
            if records.len() > 10000 {
                return Err(invalid());
            }
            let mut last = String::new();
            for r in records {
                let id: String = r.try_get("id")?;
                let attempt: Option<String> = r.try_get("attempt")?;
                if id != last {
                    jobs += 1;
                    *job_states.entry(r.try_get("state")?).or_default() += 1;
                    if attempt.is_some() {
                        executed += 1;
                    }
                    last = id;
                }
                if jobs > 10000 {
                    return Err(invalid());
                }
                if let Some(a) = attempt {
                    attempts += 1;
                    if attempts > 10000 {
                        return Err(invalid());
                    }
                    if r.try_get::<Option<String>, _>("version")?.is_some()
                        && r.try_get::<String, _>("attempt_state")? == "SUCCEEDED"
                    {
                        if r.try_get::<i64, _>("phases")? == 0
                            || r.try_get::<i64, _>("incomplete")? != 0
                        {
                            missing += 1;
                            samples.push((r.try_get::<i64, _>("finished_at_us")?, a, None));
                            continue;
                        }
                        let phases:Vec<i64>=sqlx::query_scalar("SELECT duration_ns FROM phase_intervals WHERE attempt_id=? ORDER BY sequence LIMIT 1001").bind(&a).fetch_all(&mut *tx).await?;
                        if phases.len() > 1000 {
                            return Err(invalid());
                        }
                        let duration = phases
                            .into_iter()
                            .try_fold(0_u128, |sum, n| sum.checked_add(u128::try_from(n).ok()?))
                            .ok_or_else(invalid)?;
                        samples.push((r.try_get::<i64, _>("finished_at_us")?, a, Some(duration)));
                    }
                }
            }
        }
        tx.commit().await?;
        samples.sort();
        let stats = tf_domain::history::statistics(
            &samples.iter().filter_map(|s| s.2).collect::<Vec<_>>(),
            q.window,
        )
        .ok_or_else(invalid)?;
        // A missing recent timing must not silently substitute an older success.
        let trailing = samples
            .iter()
            .rev()
            .take(q.window)
            .filter_map(|s| s.2)
            .collect::<Vec<_>>();
        let trailing_stats =
            tf_domain::history::statistics(&trailing, q.window).ok_or_else(invalid)?;
        let trailing_mean = (trailing.len() == samples.len().min(q.window))
            .then_some(trailing_stats.trailing_mean)
            .flatten();
        let failed = *states.get("FAILED").unwrap_or(&0);
        let succeeded = *states.get("SUCCEEDED").unwrap_or(&0);
        Ok(
            json!({"from_us":q.from_us.to_string(),"to_us":q.to_us.to_string(),"time_zone":"UTC","duration_unit":"ns","cohort":"build_accepted","branch":q.branch.as_str(),"dataset":q.dataset.map(|d|d.to_string()),"materialized_any":q.materialized_any,"builds":rows.len().to_string(),"build_states":states.into_iter().map(|(state,n)|json!({"state":state,"count":n.to_string()})).collect::<Vec<_>>(),"manual_requests":manual.to_string(),"scheduled_builds":scheduled.to_string(),"schedule_occurrences":null,"jobs":jobs.to_string(),"job_states":job_states.into_iter().map(|(state,n)|json!({"state":state,"count":n.to_string()})).collect::<Vec<_>>(),"jobs_executed":executed.to_string(),"attempts":attempts.to_string(),"failure_rate":(failed+succeeded>0).then(||json!({"numerator":failed.to_string(),"denominator":(failed+succeeded).to_string()})),"materializations":samples.len().to_string(),"duration_samples":stats.samples.to_string(),"missing_duration_samples":missing.to_string(),"median_ns":ratio(stats.median),"trailing_mean_ns":ratio(trailing_mean),"trailing_window":q.window.to_string(),"trailing_samples":trailing.len().to_string()}),
        )
    }
}
