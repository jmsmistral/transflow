//! Guarded persistence for the accepted-plan dispatcher. No process or artifact I/O in transactions.
use crate::{
    Store,
    planning::DraftPlan,
    publication::{self, PublicationError},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use tf_domain::{
    BuildId, CoordinatorSessionId, WorkspaceId,
    execution::{AttemptOutcome, BuildState, Job, JobState},
};
type Result<T> = std::result::Result<T, PublicationError>;
async fn guard(db: &mut SqliteConnection, job: &Job) -> Result<()> {
    publication::authority(db, job.target().dataset.workspace_id(), job.fence().session).await?;
    let n:i64=sqlx::query_scalar("SELECT count(*) FROM jobs j JOIN builds b ON b.id=j.build_id JOIN build_plans p ON p.id=b.plan_id JOIN write_reservations r ON r.build_id=b.id AND r.dataset_id=j.dataset_id AND r.branch_id=j.branch_id WHERE j.id=? AND b.id=? AND p.id=? AND p.source_snapshot_id=? AND p.disposition='ACCEPTED' AND b.state='RUNNING' AND j.dataset_id=? AND j.branch_id=? AND r.session_id=? AND r.fence=? AND r.expected_head_generation=?")
        .bind(job.id().to_string()).bind(job.build().to_string()).bind(job.binding().plan.to_string()).bind(job.binding().source.to_string()).bind(job.target().dataset.dataset_id().to_string()).bind(job.target().branch.to_string()).bind(job.fence().session.to_string()).bind(i64::try_from(job.fence().generation).map_err(|_|PublicationError::Evidence)?).bind(i64::try_from(job.target().expected_generation).map_err(|_|PublicationError::Evidence)?).fetch_one(&mut *db).await?;
    if n != 1 {
        return Err(PublicationError::Fence);
    }
    Ok(())
}
impl Store {
    /// Read the exact accepted record. A resumed/nonterminal execution needs T066 recovery,
    /// never a second dispatch of an already-started build.
    pub async fn dispatch_plan(&mut self, build: BuildId) -> Result<DraftPlan> {
        let id:String=sqlx::query_scalar("SELECT p.id FROM builds b JOIN build_plans p ON p.id=b.plan_id WHERE b.id=? AND p.disposition='ACCEPTED' AND b.state='QUEUED'").bind(build.to_string()).fetch_optional(&mut self.db).await?.ok_or(PublicationError::Evidence)?;
        self.load_draft(id.parse().map_err(|_| PublicationError::Evidence)?)
            .await
            .map_err(|_| PublicationError::Evidence)
    }
    /// Start only an exact complete untouched accepted job set under its original reservations.
    pub async fn start_dispatch(&mut self, jobs: &[Job], at_us: i64) -> Result<()> {
        let first = jobs.first().ok_or(PublicationError::Evidence)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        publication::authority(
            &mut tx,
            first.target().dataset.workspace_id(),
            first.fence().session,
        )
        .await?;
        let n = sqlx::query(
            "UPDATE builds SET state='RUNNING' WHERE created_at_us<=? AND id=? AND state='QUEUED'",
        )
        .bind(at_us)
        .bind(first.build().to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n != 1 {
            return Err(PublicationError::Canceled);
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE build_id=?")
            .bind(first.build().to_string())
            .fetch_one(&mut *tx)
            .await?;
        if count != jobs.len() as i64 {
            return Err(PublicationError::Evidence);
        }
        let mut ids = std::collections::BTreeSet::new();
        for job in jobs {
            if job.build() != first.build()
                || job.binding() != first.binding()
                || job.state() != &JobState::Planned
                || !ids.insert(job.id())
            {
                return Err(PublicationError::Evidence);
            }
            guard(&mut tx, job).await?;
            let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=?")
                .bind(job.id().to_string())
                .fetch_one(&mut *tx)
                .await?;
            let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM attempts WHERE job_id=?")
                .bind(job.id().to_string())
                .fetch_one(&mut *tx)
                .await?;
            if state != "PLANNED" || attempts != 0 {
                return Err(PublicationError::Evidence);
            }
        }
        tx.commit().await?;
        Ok(())
    }
    /// Persist a domain-validated transition using a state/fence CAS. Publication and cache
    /// already commit terminal success atomically; this method only verifies those winners.
    pub async fn persist_execution(
        &mut self,
        previous: &Job,
        next: &Job,
        at_us: i64,
        duration_ns: i64,
    ) -> Result<()> {
        if previous.id() != next.id()
            || previous.build() != next.build()
            || previous.binding() != next.binding()
            || previous.target() != next.target()
            || previous.fence() != next.fence()
            || at_us < 0
            || duration_ns < 0
            || matches!(previous.state(), JobState::Finished(_))
        {
            return Err(PublicationError::Evidence);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        guard(&mut tx, previous).await?;
        let stored: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=?")
            .bind(next.id().to_string())
            .fetch_one(&mut *tx)
            .await?;
        let terminal = matches!(next.state(), JobState::Finished(_));
        let committed = matches!(next.state().name(), "SUCCEEDED" | "CACHED" | "COMMITTING");
        if next.state().name() == "COMMITTING" {
            let intent = next
                .attempts()
                .last()
                .and_then(|a| a.intent())
                .ok_or(PublicationError::Evidence)?;
            let n:i64=sqlx::query_scalar("SELECT count(*) FROM publication_intents WHERE attempt_id=? AND planned_version_id=? AND artifact_digest=? AND state='PREPARED'").bind(intent.attempt().to_string()).bind(intent.version().to_string()).bind(intent.artifact().hex()).fetch_one(&mut *tx).await?;
            if n != 1 {
                return Err(PublicationError::Evidence);
            }
        }
        if stored
            != if committed {
                next.state().name()
            } else {
                previous.state().name()
            }
        {
            return Err(PublicationError::Evidence);
        }
        if !terminal && !committed {
            let canceled: i64 =
                sqlx::query_scalar("SELECT cancel_requested FROM builds WHERE id=?")
                    .bind(next.build().to_string())
                    .fetch_one(&mut *tx)
                    .await?;
            if canceled != 0 {
                return Err(PublicationError::Canceled);
            }
        }
        if !committed {
            sqlx::query("UPDATE jobs SET state=? WHERE id=?")
                .bind(next.state().name())
                .bind(next.id().to_string())
                .execute(&mut *tx)
                .await?;
        }
        if let Some(a) = next.attempts().last() {
            if previous.attempts().is_empty() {
                if a.phase() != tf_domain::execution::Phase::Starting {
                    return Err(PublicationError::Evidence);
                }
                sqlx::query("INSERT INTO attempts(id,job_id,attempt_no,session_id,fence,state,started_at_us) VALUES(?,?,1,?,?,'STARTING',?)").bind(a.id().to_string()).bind(next.id().to_string()).bind(next.fence().session.to_string()).bind(i64::try_from(next.fence().generation).map_err(|_|PublicationError::Evidence)?).bind(at_us).execute(&mut *tx).await?;
            } else if previous.attempts().last().map(|a| a.id()) != Some(a.id()) {
                return Err(PublicationError::Evidence);
            } else if !committed {
                let failure = match a.outcome() {
                    Some(AttemptOutcome::Failed(e)) => Some(format!("{:?}", e.class)),
                    _ => None,
                };
                sqlx::query("UPDATE attempts SET state=?,finished_at_us=?,failure_class=? WHERE id=? AND session_id=? AND fence=?").bind(next.state().name()).bind(terminal.then_some(at_us)).bind(failure).bind(a.id().to_string()).bind(next.fence().session.to_string()).bind(i64::try_from(next.fence().generation).map_err(|_|PublicationError::Evidence)?).execute(&mut *tx).await?;
            }
            if previous.state() != next.state() {
                sqlx::query("UPDATE phase_intervals SET finished_at_us=?,duration_ns=? WHERE attempt_id=? AND finished_at_us IS NULL").bind(at_us).bind(duration_ns).bind(a.id().to_string()).execute(&mut *tx).await?;
                if !terminal {
                    sqlx::query("INSERT INTO phase_intervals(attempt_id,sequence,phase,started_at_us) SELECT ?,coalesce(max(sequence)+1,0),?,? FROM phase_intervals WHERE attempt_id=?").bind(a.id().to_string()).bind(next.state().name()).bind(at_us).bind(a.id().to_string()).execute(&mut *tx).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }
    /// Freeze final exact version bindings once. Accepted symbolic parents are resolved only
    /// from this build's successful/cached result; caller supplies the checked contract.
    pub async fn bind_execution_inputs(&mut self, r: &crate::cache::Request) -> Result<()> {
        self.freeze_cache_contract(r)
            .await
            .map_err(|_| PublicationError::Evidence)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        publication::authority(&mut tx, r.target.dataset.workspace_id(), r.fence.session).await?;
        publication::validate_inputs(&mut tx, r.target.dataset.workspace_id(), &r.contract).await?;
        for i in &r.contract.inputs {
            let binding = json!({"version":i.version.to_string(),"artifact":i.artifact.hex(),"dataset":i.dataset.dataset_id().to_string(),"workspace":i.dataset.workspace_id().to_string(),"resolution":i.resolution});
            let encoded = binding.to_string();
            sqlx::query("INSERT INTO job_inputs(job_id,alias,version_id,binding_json) VALUES(?,?,?,?) ON CONFLICT DO NOTHING").bind(r.job.to_string()).bind(&i.alias).bind(i.version.to_string()).bind(&encoded).execute(&mut *tx).await?;
            let row = sqlx::query(
                "SELECT version_id,binding_json FROM job_inputs WHERE job_id=? AND alias=?",
            )
            .bind(r.job.to_string())
            .bind(&i.alias)
            .fetch_one(&mut *tx)
            .await?;
            if row.try_get::<String, _>(0)? != i.version.to_string()
                || row.try_get::<String, _>(1)? != encoded
            {
                return Err(PublicationError::Evidence);
            }
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM job_inputs WHERE job_id=?")
            .bind(r.job.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if count != r.contract.inputs.len() as i64 {
            return Err(PublicationError::Evidence);
        }
        tx.commit().await?;
        Ok(())
    }
    /// Record bounded safe helper/producer process and log evidence after actual cleanup.
    pub async fn record_execution_report(
        &mut self,
        job: &Job,
        report: &Value,
        log_path: &str,
    ) -> Result<()> {
        let a = job.attempts().last().ok_or(PublicationError::Evidence)?;
        let encoded = report.to_string();
        if encoded.len() > 1024 * 1024 || log_path.len() > 4096 {
            return Err(PublicationError::Evidence);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        guard(&mut tx, job).await?;
        sqlx::query("UPDATE attempts SET process_json=?,log_path=? WHERE id=? AND session_id=?")
            .bind(encoded)
            .bind(log_path)
            .bind(a.id().to_string())
            .bind(job.fence().session.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Cancellation is queried by the dispatcher; signaling follows durable acknowledgement.
    pub async fn dispatch_canceled(&mut self, build: BuildId) -> Result<bool> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT cancel_requested FROM builds WHERE id=?")
                .bind(build.to_string())
                .fetch_one(&mut self.db)
                .await?
                != 0,
        )
    }
    /// Finish from the exact domain-aggregated terminal set, then release this build's writes
    /// and its accepted boundary leases. Job publications and other builds remain untouched.
    pub async fn finish_dispatch(
        &mut self,
        jobs: &[Job],
        state: BuildState,
        at_us: i64,
    ) -> Result<()> {
        let first = jobs.first().ok_or(PublicationError::Evidence)?;
        let name = match state {
            BuildState::Succeeded => "SUCCEEDED",
            BuildState::Failed => "FAILED",
            BuildState::Canceled => "CANCELED",
            BuildState::Interrupted => "INTERRUPTED",
            _ => return Err(PublicationError::Evidence),
        };
        let mut aggregate = tf_domain::execution::Build::new(
            first.build(),
            first.binding(),
            jobs.iter().map(|j| (j.id(), true)).collect(),
            tf_domain::execution::EventTime(0),
        )
        .map_err(|_| PublicationError::Evidence)?;
        aggregate
            .start(tf_domain::execution::EventTime(0))
            .map_err(|_| PublicationError::Evidence)?;
        aggregate
            .finish(
                jobs,
                jobs.iter()
                    .map(|j| j.last_event())
                    .max()
                    .ok_or(PublicationError::Evidence)?,
            )
            .map_err(|_| PublicationError::Evidence)?;
        if aggregate.state() != state {
            return Err(PublicationError::Evidence);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        for job in jobs {
            guard(&mut tx, job).await?;
            let stored: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=?")
                .bind(job.id().to_string())
                .fetch_one(&mut *tx)
                .await?;
            if !matches!(job.state(), JobState::Finished(_))
                || stored != job.state().name()
                || job.build() != first.build()
            {
                return Err(PublicationError::Evidence);
            }
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE build_id=?")
            .bind(first.build().to_string())
            .fetch_one(&mut *tx)
            .await?;
        if count != jobs.len() as i64 {
            return Err(PublicationError::Evidence);
        }
        sqlx::query("UPDATE builds SET state=?,finished_at_us=? WHERE id=? AND state='RUNNING'")
            .bind(name)
            .bind(at_us)
            .bind(first.build().to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM write_reservations WHERE build_id=? AND session_id=?")
            .bind(first.build().to_string())
            .bind(first.fence().session.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE read_leases SET released=1 WHERE owner_operation IN (?,?) AND kind='build'",
        )
        .bind(first.binding().plan.to_string())
        .bind(first.build().to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Renew accepted boundary leases while the coordinator holds their unchanged bindings.
    pub async fn renew_dispatch_boundaries(
        &mut self,
        workspace: WorkspaceId,
        session: CoordinatorSessionId,
        plan: &DraftPlan,
        at_us: i64,
    ) -> Result<()> {
        if plan.workspace != workspace.to_string() {
            return Err(PublicationError::Evidence);
        }
        crate::retention::clock(&mut self.db, at_us)
            .await
            .map_err(|_| PublicationError::Evidence)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        crate::retention::clock(&mut tx, at_us)
            .await
            .map_err(|_| PublicationError::Evidence)?;
        publication::authority(&mut tx, workspace, session).await?;
        for r in &plan.reads {
            let n=sqlx::query("UPDATE read_leases SET renewed_at_us=?,expires_at_us=max(expires_at_us,?),fence=fence+1 WHERE fence<9223372036854775807 AND owner_operation=? AND id=? AND version_id=? AND EXISTS(SELECT 1 FROM dataset_versions v WHERE v.id=read_leases.version_id AND v.artifact_digest=?) AND NOT EXISTS(SELECT 1 FROM artifact_gc_claims g JOIN dataset_versions v ON v.artifact_digest=g.digest WHERE v.id=read_leases.version_id) AND released=0 AND expires_at_us>? AND kind='build'").bind(at_us).bind(at_us.checked_add(3_600_000_000).ok_or(PublicationError::Evidence)?).bind(&plan.id).bind(&r.lease).bind(&r.version).bind(&r.artifact).bind(at_us).execute(&mut *tx).await?.rows_affected();
            if n != 1 {
                return Err(PublicationError::Evidence);
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
