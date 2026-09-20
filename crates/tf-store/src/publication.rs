//! Fixed, short SQLite visibility transactions. Never performs filesystem/user work.
//! Every writer call requires the caller's real runtime ownership guard (tf-exec).
use crate::{
    Store, StoreError,
    artifacts::{Candidate, VerifiedArtifact},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::BTreeSet;
use tf_domain::execution::{CancelRequest, Fence, OutputTarget, PublicationIntent};
use tf_domain::{
    BranchName, BuildId, CoordinatorSessionId, DatasetKey, JobId, RequestId, VersionId, WorkspaceId,
};
use tf_protocol::canonical::{ContentDigest, DigestKind, canonical_json};
/// Publication refusal is distinct from a storage failure.
#[derive(Debug, thiserror::Error)]
pub enum PublicationError {
    /// A repository or database failure retains its original cause.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Stale runtime, missing reservation or changed accepted execution identity.
    #[error("This coordinator no longer owns the publication reservation")]
    Fence,
    /// Head changed after the plan's output generation was reserved.
    #[error("The output head changed; prepare a new plan")]
    Conflict,
    /// Cancellation won before the visibility transaction.
    #[error("Cancellation was recorded before publication")]
    Canceled,
    /// Exact frozen inputs, checks or candidate metadata are absent/inconsistent.
    #[error("Publication requires complete matching input and validation evidence")]
    Evidence,
}
impl From<sqlx::Error> for PublicationError {
    fn from(e: sqlx::Error) -> Self {
        Self::Store(e.into())
    }
}
type Result<T> = std::result::Result<T, PublicationError>;
/// Exact historical provenance, including repeated datasets under distinct aliases.
#[derive(Clone, Debug)]
pub struct InputProvenance {
    /// Input alias within this producer.
    pub alias: String,
    /// Origin identity, even for a verified local replica.
    pub dataset: DatasetKey,
    /// Exact retained publication identity.
    pub version: VersionId,
    /// Independently verified physical identity.
    pub artifact: ContentDigest,
    /// Original branch selector evidence.
    pub declared_branch: Value,
    /// Starting branch and actual resolved branch are independent.
    pub starting_branch: BranchName,
    /// Branch where the exact version resolved.
    pub resolved_branch: BranchName,
    /// Data or validation-only dependency role.
    pub role: InputRole,
    /// Bounded resolution explanation retained from planning.
    pub resolution: Value,
}
/// Dependency role shared with alias-qualified planning bindings.
pub use tf_domain::input::InputRole;
impl InputProvenance {
    fn value(&self) -> Value {
        json!({"alias":self.alias,"workspace":self.dataset.workspace_id().to_string(),"dataset":self.dataset.dataset_id().to_string(),"version":self.version.to_string(),"artifact":self.artifact.hex(),"declared_branch":self.declared_branch,"starting_branch":self.starting_branch.as_str(),"resolved_branch":self.resolved_branch.as_str(),"role":self.role.name(),"resolution":self.resolution})
    }
}
/// The exact bytes a check must certify.
#[derive(Clone, Debug)]
pub enum CheckSubject {
    /// Materialized candidate.
    Output,
    /// A frozen input alias.
    Input(String),
}
/// Frozen declaration obligation. Missing/evaluator-error/skipped checks never pass.
#[derive(Clone, Debug)]
pub struct CheckRequirement {
    /// Fingerprint of the retained immutable check definition.
    pub definition: String,
    /// Output or exact input alias.
    pub subject: CheckSubject,
    /// FAIL rather than WARN violation policy. Evaluator errors always prohibit commit.
    pub required: bool,
}
/// Immutable per-job execution contract, frozen before checks/function execution.
#[derive(Clone, Debug)]
pub struct PublicationContract {
    /// Compute fingerprint produced by accepted planning.
    pub compute_fingerprint: String,
    /// Validation declaration fingerprint (not a fabricated result timestamp).
    pub check_fingerprint: String,
    /// Ordered exact provenance.
    pub inputs: Vec<InputProvenance>,
    /// Complete check obligations for this job, including input checks.
    pub checks: Vec<CheckRequirement>,
}
fn digest(text: &str) -> Result<()> {
    ContentDigest::from_hex(DigestKind::File, text).map_err(|_| PublicationError::Evidence)?;
    Ok(())
}
fn encoded(value: &Value) -> Result<String> {
    let b = canonical_json(value).map_err(|_| PublicationError::Evidence)?;
    if b.len() > 1024 * 1024 {
        return Err(PublicationError::Evidence);
    }
    String::from_utf8(b).map_err(|_| PublicationError::Evidence)
}
fn num(n: u64) -> Result<i64> {
    i64::try_from(n).map_err(|_| PublicationError::Evidence)
}
impl PublicationContract {
    fn value(&self) -> Result<Value> {
        digest(&self.compute_fingerprint)?;
        digest(&self.check_fingerprint)?;
        if self.inputs.len() > 10_000 || self.checks.len() > 10_000 {
            return Err(PublicationError::Evidence);
        }
        let mut aliases = BTreeSet::new();
        for i in &self.inputs {
            if i.alias.is_empty()
                || i.alias.len() > 1024
                || !aliases.insert(&i.alias)
                || i.artifact.kind() != DigestKind::Artifact
            {
                return Err(PublicationError::Evidence);
            }
        }
        let mut checks = BTreeSet::new();
        for c in &self.checks {
            digest(&c.definition)?;
            if !checks.insert(&c.definition) {
                return Err(PublicationError::Evidence);
            }
            if let CheckSubject::Input(alias) = &c.subject
                && !aliases.contains(alias)
            {
                return Err(PublicationError::Evidence);
            }
        }
        Ok(
            json!({"compute":self.compute_fingerprint,"checks_fingerprint":self.check_fingerprint,"inputs":self.inputs.iter().map(InputProvenance::value).collect::<Vec<_>>(),"checks":self.checks.iter().map(|c|json!({"definition":c.definition,"subject":match &c.subject{CheckSubject::Output=>json!({"output":true}),CheckSubject::Input(alias)=>json!({"input":alias})},"required":c.required})).collect::<Vec<_>>() }),
        )
    }
    /// Canonical frozen contract, useful for retained planner evidence.
    pub fn canonical(&self) -> Result<String> {
        encoded(&self.value()?)
    }
}
/// Complete publication command. Intent comes from the domain's legal state transition.
#[derive(Clone, Debug)]
pub struct PublicationRequest {
    /// Attempt/version/accepted-plan identity, generation and owner fence.
    pub intent: PublicationIntent,
    /// Must exactly equal the previously frozen job contract.
    pub contract: PublicationContract,
    /// Supplied UTC commit timestamp; all visible rows share it.
    pub at_us: i64,
}
/// Durable identity returned for both first commit and idempotent readback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationReceipt {
    /// Independent version UUID; repeated bytes may have different versions.
    pub version: VersionId,
    /// Head generation allocated by the commit.
    pub generation: u64,
    /// Stable durable event identity, reused on notification retry.
    pub event: RequestId,
}
async fn authority(
    db: &mut SqliteConnection,
    workspace: WorkspaceId,
    session: CoordinatorSessionId,
) -> Result<()> {
    let valid: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspaces WHERE id=? AND runtime_owner=?")
            .bind(workspace.to_string())
            .bind(session.to_string())
            .fetch_one(db)
            .await?;
    if valid != 1 {
        return Err(PublicationError::Fence);
    }
    Ok(())
}
impl Store {
    /// Establish a newly acquired OS-lock session, interrupting only uncommitted old work.
    /// The caller MUST hold the actual exclusive runtime lock, never merely supply a UUID.
    pub async fn recover_publications(
        &mut self,
        workspace: WorkspaceId,
        session: CoordinatorSessionId,
        at_us: i64,
    ) -> Result<()> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let old: Option<String> =
            sqlx::query_scalar("SELECT runtime_owner FROM workspaces WHERE id=?")
                .bind(workspace.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if old.as_deref() == Some(&session.to_string()) {
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("UPDATE workspaces SET runtime_owner=? WHERE id=?")
            .bind(session.to_string())
            .bind(workspace.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE publication_intents SET state='ABANDONED' WHERE state='PREPARED'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE attempts SET state='INTERRUPTED',finished_at_us=?,failure_class='coordinator_lost' WHERE state NOT IN ('SUCCEEDED','FAILED','CANCELED','INTERRUPTED')").bind(at_us).execute(&mut *tx).await?;
        sqlx::query("UPDATE jobs SET state='INTERRUPTED' WHERE state NOT IN ('SUCCEEDED','CACHED','FAILED','BLOCKED','CANCELED','INTERRUPTED')").execute(&mut *tx).await?;
        sqlx::query("UPDATE builds SET state='INTERRUPTED',finished_at_us=? WHERE state IN ('QUEUED','RUNNING')").bind(at_us).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM write_reservations")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Reserve the complete accepted build write set atomically before launching workers.
    pub async fn reserve_publications(
        &mut self,
        workspace: WorkspaceId,
        build: BuildId,
        fence: Fence,
        targets: &[OutputTarget],
        at_us: i64,
    ) -> Result<()> {
        if targets.is_empty()
            || targets.len() > 10_000
            || targets
                .iter()
                .map(|t| t.dataset)
                .collect::<BTreeSet<_>>()
                .len()
                != targets.len()
        {
            return Err(PublicationError::Evidence);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        authority(&mut tx, workspace, fence.session).await?;
        let valid:i64=sqlx::query_scalar("SELECT count(*) FROM builds b JOIN build_plans p ON p.id=b.plan_id WHERE b.id=? AND p.disposition='ACCEPTED' AND b.state IN ('QUEUED','RUNNING') AND b.cancel_requested=0").bind(build.to_string()).fetch_one(&mut *tx).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE build_id=?")
            .bind(build.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if valid != 1 || count != num(targets.len() as u64)? {
            return Err(PublicationError::Evidence);
        }
        for target in targets {
            if target.dataset.workspace_id() != workspace {
                return Err(PublicationError::Evidence);
            }
            let matched:i64=sqlx::query_scalar("SELECT count(*) FROM jobs j JOIN data_branches b ON b.id=j.branch_id JOIN datasets d ON d.id=j.dataset_id WHERE j.build_id=? AND j.branch_id=? AND j.dataset_id=? AND b.workspace_id=? AND d.workspace_id=? AND b.deleted_at_us IS NULL AND d.tombstoned_at_us IS NULL").bind(build.to_string()).bind(target.branch.to_string()).bind(target.dataset.dataset_id().to_string()).bind(workspace.to_string()).bind(workspace.to_string()).fetch_one(&mut *tx).await?;
            let generation: Option<i64> = sqlx::query_scalar(
                "SELECT generation FROM dataset_heads WHERE branch_id=? AND dataset_id=?",
            )
            .bind(target.branch.to_string())
            .bind(target.dataset.dataset_id().to_string())
            .fetch_optional(&mut *tx)
            .await?;
            if matched != 1 {
                return Err(PublicationError::Evidence);
            }
            if generation.unwrap_or(0) != num(target.expected_generation)? {
                return Err(PublicationError::Conflict);
            }
            sqlx::query("INSERT INTO write_reservations(branch_id,dataset_id,build_id,session_id,fence,acquired_at_us,expected_head_generation) VALUES(?,?,?,?,?,?,?)").bind(target.branch.to_string()).bind(target.dataset.dataset_id().to_string()).bind(build.to_string()).bind(fence.session.to_string()).bind(num(fence.generation)?).bind(at_us).bind(num(target.expected_generation)?).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Freeze exact inputs and complete check obligations before any attempt leaves STARTING.
    /// Planning owns these declarations; workers cannot create or alter this contract.
    pub async fn freeze_publication_contract(
        &mut self,
        workspace: WorkspaceId,
        session: CoordinatorSessionId,
        job: JobId,
        contract: &PublicationContract,
    ) -> Result<()> {
        let encoded = contract.canonical()?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        authority(&mut tx, workspace, session).await?;
        let eligible:i64=sqlx::query_scalar("SELECT count(*) FROM jobs j JOIN write_reservations r ON r.build_id=j.build_id AND r.branch_id=j.branch_id AND r.dataset_id=j.dataset_id WHERE j.id=? AND j.state IN ('PLANNED','WAITING_DEPENDENCIES','QUEUED','STARTING') AND r.session_id=? AND NOT EXISTS(SELECT 1 FROM attempts a WHERE a.job_id=j.id AND a.state!='STARTING')").bind(job.to_string()).bind(session.to_string()).fetch_one(&mut *tx).await?;
        if eligible != 1 {
            return Err(PublicationError::Evidence);
        }
        validate_inputs(&mut tx, workspace, contract).await?;
        for check in &contract.checks {
            validate_definition(&mut tx, check).await?;
        }
        sqlx::query("INSERT INTO publication_contracts(job_id,contract_json) VALUES(?,?)")
            .bind(job.to_string())
            .bind(encoded)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Persist intent before installing files; no version/head/event exists at this boundary.
    pub async fn prepare_publication(
        &mut self,
        request: &PublicationRequest,
        candidate: &Candidate<'_>,
    ) -> Result<()> {
        let intent = &request.intent;
        let manifest = candidate.manifest();
        if candidate
            .digest()
            .map_err(|_| PublicationError::Evidence)?
            .hex()
            != intent.artifact().hex()
        {
            return Err(PublicationError::Evidence);
        }
        let encoded_manifest = encoded(manifest)?;
        let schema = encoded(&manifest["logical_schema"])?;
        let files = manifest["files"]
            .as_array()
            .ok_or(PublicationError::Evidence)?;
        let sum = |key: &str| {
            files.iter().try_fold(0i64, |n, v| {
                let i = v[key]
                    .as_str()
                    .and_then(|s| s.parse::<i64>().ok())
                    .ok_or(PublicationError::Evidence)?;
                n.checked_add(i).ok_or(PublicationError::Evidence)
            })
        };
        let (rows, bytes) = (sum("row_count")?, sum("byte_length")?);
        let contract = request.contract.canonical()?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        guards(&mut tx, request, &contract, false).await?;
        check_results(&mut tx, request).await?;
        sqlx::query("INSERT INTO artifacts(digest,manifest_json,schema_json,file_count,row_count,byte_count,integrity_state) VALUES(?,?,?,?,?,?,'MANIFEST') ON CONFLICT(digest) DO NOTHING").bind(intent.artifact().hex()).bind(&encoded_manifest).bind(schema).bind(num(files.len() as u64)?).bind(rows).bind(bytes).execute(&mut *tx).await?;
        let stored: String =
            sqlx::query_scalar("SELECT manifest_json FROM artifacts WHERE digest=?")
                .bind(intent.artifact().hex())
                .fetch_one(&mut *tx)
                .await?;
        if stored != encoded_manifest {
            return Err(PublicationError::Evidence);
        }
        sqlx::query("INSERT INTO publication_intents(attempt_id,planned_version_id,artifact_digest,expected_head_generation,session_id,fence,state) VALUES(?,?,?,?,?,?,'PREPARED')").bind(intent.attempt().to_string()).bind(intent.version().to_string()).bind(intent.artifact().hex()).bind(num(intent.target().expected_generation)?).bind(intent.fence().session.to_string()).bind(num(intent.fence().generation)?).execute(&mut *tx).await?;
        sqlx::query("UPDATE attempts SET state='COMMITTING' WHERE id=?")
            .bind(intent.attempt().to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE jobs SET state='COMMITTING' WHERE id=?")
            .bind(intent.job().to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Commit only a strictly verified, durable object. All visibility rows share one transaction.
    /// Caller revalidates its real owner immediately before this call, after filesystem work.
    pub async fn commit_publication(
        &mut self,
        request: &PublicationRequest,
        artifact: &VerifiedArtifact,
    ) -> Result<PublicationReceipt> {
        let i = &request.intent;
        let t = i.target();
        if artifact.digest().hex() != i.artifact().hex()
            || self.path.parent() != Some(artifact.workspace().join(".transflow/runtime").as_path())
        {
            return Err(PublicationError::Evidence);
        }
        let contract = request.contract.canonical()?;
        let manifest = encoded(artifact.manifest())?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        authority(&mut tx, t.dataset.workspace_id(), i.fence().session).await?;
        if let Some(receipt) = committed(&mut tx, request).await? {
            tx.commit().await?;
            return Ok(receipt);
        }
        guards(&mut tx, request, &contract, true).await?;
        let valid:i64=sqlx::query_scalar("SELECT count(*) FROM publication_intents p JOIN artifacts a ON a.digest=p.artifact_digest WHERE p.attempt_id=? AND p.planned_version_id=? AND p.artifact_digest=? AND p.expected_head_generation=? AND p.session_id=? AND p.fence=? AND p.state='PREPARED' AND a.manifest_json=?").bind(i.attempt().to_string()).bind(i.version().to_string()).bind(i.artifact().hex()).bind(num(t.expected_generation)?).bind(i.fence().session.to_string()).bind(num(i.fence().generation)?).bind(manifest).fetch_one(&mut *tx).await?;
        if valid != 1 {
            return Err(PublicationError::Evidence);
        }
        let checks = check_results(&mut tx, request).await?;
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT version_id FROM dataset_heads WHERE branch_id=? AND dataset_id=?",
        )
        .bind(t.branch.to_string())
        .bind(t.dataset.dataset_id().to_string())
        .fetch_optional(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO dataset_versions(id,dataset_id,artifact_digest,attempt_id,source_snapshot_id,published_at_us,compute_fingerprint,check_fingerprint) VALUES(?,?,?,?,?,?,?,?)").bind(i.version().to_string()).bind(t.dataset.dataset_id().to_string()).bind(i.artifact().hex()).bind(i.attempt().to_string()).bind(i.binding().source.to_string()).bind(request.at_us).bind(&request.contract.compute_fingerprint).bind(&request.contract.check_fingerprint).execute(&mut *tx).await?;
        for input in &request.contract.inputs {
            sqlx::query("INSERT INTO version_inputs(output_version_id,alias,origin_workspace_id,origin_dataset_id,origin_version_id,artifact_digest,declared_branch_json,starting_branch,resolved_branch,role,resolution_json) VALUES(?,?,?,?,?,?,?,?,?,?,?)").bind(i.version().to_string()).bind(&input.alias).bind(input.dataset.workspace_id().to_string()).bind(input.dataset.dataset_id().to_string()).bind(input.version.to_string()).bind(input.artifact.hex()).bind(encoded(&input.declared_branch)?).bind(input.starting_branch.as_str()).bind(input.resolved_branch.as_str()).bind(input.role.name()).bind(encoded(&input.resolution)?).execute(&mut *tx).await?;
        }
        for result in checks {
            sqlx::query("INSERT INTO version_check_results VALUES(?,?)")
                .bind(i.version().to_string())
                .bind(result)
                .execute(&mut *tx)
                .await?;
        }
        let generation = num(t.expected_generation)?
            .checked_add(1)
            .ok_or(PublicationError::Conflict)?;
        let changed = if t.expected_generation == 0 {
            sqlx::query("INSERT INTO dataset_heads VALUES(?,?,?,?) ON CONFLICT DO NOTHING")
                .bind(t.branch.to_string())
                .bind(t.dataset.dataset_id().to_string())
                .bind(i.version().to_string())
                .bind(generation)
                .execute(&mut *tx)
                .await?
                .rows_affected()
        } else {
            sqlx::query("UPDATE dataset_heads SET version_id=?,generation=? WHERE branch_id=? AND dataset_id=? AND generation=?").bind(i.version().to_string()).bind(generation).bind(t.branch.to_string()).bind(t.dataset.dataset_id().to_string()).bind(num(t.expected_generation)?).execute(&mut *tx).await?.rows_affected()
        };
        if changed != 1 {
            return Err(PublicationError::Conflict);
        }
        let event = RequestId::from_bytes(*i.version().as_bytes());
        let payload = encoded(
            &json!({"version_id":i.version().to_string(),"artifact_digest":i.artifact().hex(),"dataset_id":t.dataset.dataset_id().to_string(),"branch_id":t.branch.to_string(),"generation":generation.to_string()}),
        )?;
        sqlx::query("INSERT INTO events(id,type,payload_json,causation_id,correlation_id,wall_time_us) VALUES(?,'dataset.published',?,?,?,?)").bind(event.to_string()).bind(&payload).bind(i.attempt().to_string()).bind(i.build().to_string()).bind(request.at_us).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO head_changes VALUES(?,?,?,?,?,?,?,?)")
            .bind(event.to_string())
            .bind(t.branch.to_string())
            .bind(t.dataset.dataset_id().to_string())
            .bind(previous)
            .bind(i.version().to_string())
            .bind(generation)
            .bind("publication")
            .bind(request.at_us)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO outbox_deliveries(event_id,target,payload_json,schema_version,attempts,next_attempt_at_us) VALUES(?,'publication',?,1,0,?)").bind(event.to_string()).bind(payload).bind(request.at_us).execute(&mut *tx).await?;
        sqlx::query("UPDATE publication_intents SET state='COMMITTED' WHERE attempt_id=?")
            .bind(i.attempt().to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE attempts SET state='SUCCEEDED',finished_at_us=? WHERE id=?")
            .bind(request.at_us)
            .bind(i.attempt().to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE jobs SET state='SUCCEEDED' WHERE id=?")
            .bind(i.job().to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(PublicationReceipt {
            version: i.version(),
            generation: generation as u64,
            event,
        })
    }
    /// Serialized cancellation: committed outputs remain immutable; only future commits are blocked.
    pub async fn cancel_publications(
        &mut self,
        workspace: WorkspaceId,
        session: CoordinatorSessionId,
        build: BuildId,
    ) -> Result<CancelRequest> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        authority(&mut tx, workspace, session).await?;
        let (state, canceled): (String, i64) =
            sqlx::query_as("SELECT state,cancel_requested FROM builds WHERE id=?")
                .bind(build.to_string())
                .fetch_one(&mut *tx)
                .await?;
        let live:i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE build_id=? AND state NOT IN ('SUCCEEDED','CACHED','FAILED','BLOCKED','CANCELED','INTERRUPTED')").bind(build.to_string()).fetch_one(&mut *tx).await?;
        let result = if !matches!(state.as_str(), "RUNNING" | "QUEUED") || live == 0 {
            CancelRequest::TooLate
        } else if canceled == 1 {
            CancelRequest::AlreadyRequested
        } else {
            sqlx::query("UPDATE builds SET cancel_requested=1 WHERE id=?")
                .bind(build.to_string())
                .execute(&mut *tx)
                .await?;
            CancelRequest::Requested
        };
        tx.commit().await?;
        Ok(result)
    }
    /// Release all reservations only after the build reaches a durable terminal state.
    pub async fn release_publications(
        &mut self,
        workspace: WorkspaceId,
        session: CoordinatorSessionId,
        build: BuildId,
    ) -> Result<()> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        authority(&mut tx, workspace, session).await?;
        let valid:i64=sqlx::query_scalar("SELECT count(*) FROM builds WHERE id=? AND state IN ('SUCCEEDED','FAILED','CANCELED','INTERRUPTED')").bind(build.to_string()).fetch_one(&mut *tx).await?;
        if valid != 1 {
            return Err(PublicationError::Evidence);
        }
        sqlx::query("DELETE FROM write_reservations WHERE build_id=? AND session_id=?")
            .bind(build.to_string())
            .bind(session.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Replay at most 100 undelivered events, with stable identity and no execution side effects.
    pub async fn pending_publications(&mut self, limit: u32) -> Result<Vec<(RequestId, Value)>> {
        if limit == 0 || limit > 100 {
            return Err(PublicationError::Evidence);
        }
        let rows=sqlx::query("SELECT o.event_id,o.payload_json FROM outbox_deliveries o JOIN events e ON e.id=o.event_id WHERE o.target='publication' AND o.acknowledged_at_us IS NULL ORDER BY e.sequence LIMIT ?").bind(limit).fetch_all(&mut self.db).await?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    r.try_get::<String, _>(0)?
                        .parse()
                        .map_err(|_| PublicationError::Evidence)?,
                    serde_json::from_str(&r.try_get::<String, _>(1)?)
                        .map_err(|_| PublicationError::Evidence)?,
                ))
            })
            .collect()
    }
    /// Acknowledge after delivery. A crash before ack yields an at-least-once replay of the same ID.
    pub async fn acknowledge_publication(
        &mut self,
        workspace: WorkspaceId,
        session: CoordinatorSessionId,
        event: RequestId,
        at_us: i64,
    ) -> Result<()> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        authority(&mut tx, workspace, session).await?;
        let n=sqlx::query("UPDATE outbox_deliveries SET acknowledged_at_us=coalesce(acknowledged_at_us,?) WHERE event_id=? AND target='publication'").bind(at_us).bind(event.to_string()).execute(&mut *tx).await?.rows_affected();
        if n != 1 {
            return Err(PublicationError::Evidence);
        }
        tx.commit().await?;
        Ok(())
    }
}
async fn validate_inputs(
    db: &mut SqliteConnection,
    workspace: WorkspaceId,
    c: &PublicationContract,
) -> Result<()> {
    for input in &c.inputs {
        if input.dataset.workspace_id() == workspace {
            crate::retention::version_available(db, input.version)
                .await
                .map_err(|_| PublicationError::Evidence)?;
        }
        crate::retention::available(db, input.artifact)
            .await
            .map_err(|_| PublicationError::Evidence)?;
        let valid: i64 = if input.dataset.workspace_id() == workspace {
            sqlx::query_scalar("SELECT count(*) FROM dataset_versions WHERE dataset_id=? AND id=? AND artifact_digest=?").bind(input.dataset.dataset_id().to_string()).bind(input.version.to_string()).bind(input.artifact.hex()).fetch_one(&mut *db).await?
        } else {
            sqlx::query_scalar("SELECT count(*) FROM replicas WHERE workspace_id=? AND dataset_id=? AND version_id=? AND artifact_digest=? AND copy_state='VERIFIED'").bind(input.dataset.workspace_id().to_string()).bind(input.dataset.dataset_id().to_string()).bind(input.version.to_string()).bind(input.artifact.hex()).fetch_one(&mut *db).await?
        };
        if valid != 1 {
            return Err(PublicationError::Evidence);
        }
    }
    Ok(())
}
async fn validate_definition(db: &mut SqliteConnection, c: &CheckRequirement) -> Result<()> {
    let row = sqlx::query(
        "SELECT phase,target_alias,policy_json FROM check_definitions WHERE fingerprint=?",
    )
    .bind(&c.definition)
    .fetch_optional(&mut *db)
    .await?
    .ok_or(PublicationError::Evidence)?;
    let phase: String = row.try_get(0)?;
    let alias: Option<String> = row.try_get(1)?;
    let policy: Value = serde_json::from_str(&row.try_get::<String, _>(2)?)
        .map_err(|_| PublicationError::Evidence)?;
    let subject = match &c.subject {
        CheckSubject::Output => phase == "output" && alias.is_none(),
        CheckSubject::Input(a) => phase == "input" && alias.as_ref() == Some(a),
    };
    if !subject || policy["severity"] != if c.required { "FAIL" } else { "WARN" } {
        return Err(PublicationError::Evidence);
    }
    Ok(())
}
async fn guards(
    db: &mut SqliteConnection,
    r: &PublicationRequest,
    contract: &str,
    committing: bool,
) -> Result<()> {
    let i = &r.intent;
    let t = i.target();
    let collecting: i64 =
        sqlx::query_scalar("SELECT count(*) FROM artifact_gc_claims WHERE digest=?")
            .bind(i.artifact().hex())
            .fetch_one(&mut *db)
            .await?;
    if collecting != 0 {
        return Err(PublicationError::Evidence);
    }

    authority(db, t.dataset.workspace_id(), i.fence().session).await?;
    let row=sqlx::query("SELECT b.cancel_requested,b.state,a.state,j.state,p.source_snapshot_id,p.disposition,c.contract_json FROM attempts a JOIN jobs j ON j.id=a.job_id JOIN builds b ON b.id=j.build_id JOIN build_plans p ON p.id=b.plan_id JOIN publication_contracts c ON c.job_id=j.id JOIN data_branches d ON d.id=j.branch_id JOIN datasets ds ON ds.id=j.dataset_id WHERE a.id=? AND j.id=? AND b.id=? AND p.id=? AND j.branch_id=? AND j.dataset_id=? AND a.session_id=? AND a.fence=? AND d.deleted_at_us IS NULL AND ds.tombstoned_at_us IS NULL").bind(i.attempt().to_string()).bind(i.job().to_string()).bind(i.build().to_string()).bind(i.binding().plan.to_string()).bind(t.branch.to_string()).bind(t.dataset.dataset_id().to_string()).bind(i.fence().session.to_string()).bind(num(i.fence().generation)?).fetch_optional(&mut *db).await?.ok_or(PublicationError::Fence)?;
    if row.try_get::<i64, _>(0)? == 1 {
        return Err(PublicationError::Canceled);
    }
    let state = if committing {
        "COMMITTING"
    } else {
        "VALIDATING_OUTPUTS"
    };
    if row.try_get::<String, _>(1)? != "RUNNING"
        || row.try_get::<String, _>(2)? != state
        || row.try_get::<String, _>(3)? != state
        || row.try_get::<String, _>(4)? != i.binding().source.to_string()
        || row.try_get::<String, _>(5)? != "ACCEPTED"
        || row.try_get::<String, _>(6)? != contract
    {
        return Err(PublicationError::Evidence);
    }
    let reserved:i64=sqlx::query_scalar("SELECT count(*) FROM write_reservations WHERE branch_id=? AND dataset_id=? AND build_id=? AND session_id=? AND fence=? AND expected_head_generation=?").bind(t.branch.to_string()).bind(t.dataset.dataset_id().to_string()).bind(i.build().to_string()).bind(i.fence().session.to_string()).bind(num(i.fence().generation)?).bind(num(t.expected_generation)?).fetch_one(&mut *db).await?;
    if reserved != 1 {
        return Err(PublicationError::Fence);
    }
    let complete:i64=sqlx::query_scalar("SELECT count(*) FROM jobs j WHERE j.build_id=? AND NOT EXISTS(SELECT 1 FROM write_reservations r WHERE r.build_id=j.build_id AND r.branch_id=j.branch_id AND r.dataset_id=j.dataset_id AND r.session_id=? AND r.fence=?)").bind(i.build().to_string()).bind(i.fence().session.to_string()).bind(num(i.fence().generation)?).fetch_one(&mut *db).await?;
    if complete != 0 {
        return Err(PublicationError::Fence);
    }

    let generation: Option<i64> = sqlx::query_scalar(
        "SELECT generation FROM dataset_heads WHERE branch_id=? AND dataset_id=?",
    )
    .bind(t.branch.to_string())
    .bind(t.dataset.dataset_id().to_string())
    .fetch_optional(&mut *db)
    .await?;
    if generation.unwrap_or(0) != num(t.expected_generation)? {
        return Err(PublicationError::Conflict);
    }
    validate_inputs(db, t.dataset.workspace_id(), &r.contract).await
}
async fn check_results(db: &mut SqliteConnection, r: &PublicationRequest) -> Result<Vec<String>> {
    let mut ids = vec![];
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM check_results WHERE attempt_id=?")
        .bind(r.intent.attempt().to_string())
        .fetch_one(&mut *db)
        .await?;
    if count != num(r.contract.checks.len() as u64)? {
        return Err(PublicationError::Evidence);
    }
    for check in &r.contract.checks {
        validate_definition(db, check).await?;
        let subject = match &check.subject {
            CheckSubject::Output => json!({"artifact_digest":r.intent.artifact().hex()}),
            CheckSubject::Input(a) => {
                let i = r
                    .contract
                    .inputs
                    .iter()
                    .find(|i| &i.alias == a)
                    .ok_or(PublicationError::Evidence)?;
                json!({"alias":a,"artifact_digest":i.artifact.hex(),"version_id":i.version.to_string()})
            }
        };
        let rows=sqlx::query("SELECT id,subject_json,outcome,finished_at_us,started_at_us FROM check_results WHERE attempt_id=? AND definition_fingerprint=? LIMIT 2").bind(r.intent.attempt().to_string()).bind(&check.definition).fetch_all(&mut *db).await?;
        if rows.len() != 1 {
            return Err(PublicationError::Evidence);
        }
        let row = rows.first().ok_or(PublicationError::Evidence)?;
        let stored: Value = serde_json::from_str(&row.try_get::<String, _>(1)?)
            .map_err(|_| PublicationError::Evidence)?;
        let outcome: String = row.try_get(2)?;
        let finish: Option<i64> = row.try_get(3)?;
        let started: i64 = row.try_get(4)?;
        if stored != subject
            || finish.is_none_or(|t| t < started || t > r.at_us)
            || !(outcome == "PASS" || (!check.required && outcome == "VIOLATION"))
        {
            return Err(PublicationError::Evidence);
        }
        ids.push(row.try_get(0)?);
    }
    Ok(ids)
}
async fn committed(
    db: &mut SqliteConnection,
    r: &PublicationRequest,
) -> Result<Option<PublicationReceipt>> {
    let i = &r.intent;
    let row=sqlx::query("SELECT v.id,v.artifact_digest,h.generation,h.event_id,c.contract_json FROM publication_intents p JOIN dataset_versions v ON v.id=p.planned_version_id AND v.attempt_id=p.attempt_id JOIN head_changes h ON h.new_version_id=v.id JOIN publication_contracts c ON c.job_id=? WHERE p.attempt_id=? AND p.state='COMMITTED' AND p.session_id=? AND p.fence=? AND p.expected_head_generation=? AND h.branch_id=? AND v.dataset_id=? AND v.source_snapshot_id=? AND EXISTS(SELECT 1 FROM attempts a JOIN jobs j ON j.id=a.job_id JOIN builds b ON b.id=j.build_id WHERE a.id=p.attempt_id AND j.id=? AND b.id=? AND b.plan_id=?)").bind(i.job().to_string()).bind(i.attempt().to_string()).bind(i.fence().session.to_string()).bind(num(i.fence().generation)?).bind(num(i.target().expected_generation)?).bind(i.target().branch.to_string()).bind(i.target().dataset.dataset_id().to_string()).bind(i.binding().source.to_string()).bind(i.job().to_string()).bind(i.build().to_string()).bind(i.binding().plan.to_string()).fetch_optional(db).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<String, _>(0)? != i.version().to_string()
        || row.try_get::<String, _>(1)? != i.artifact().hex()
        || row.try_get::<String, _>(4)? != r.contract.canonical()?
    {
        return Err(PublicationError::Evidence);
    }
    Ok(Some(PublicationReceipt {
        version: i.version(),
        generation: row.try_get::<i64, _>(2)? as u64,
        event: row
            .try_get::<String, _>(3)?
            .parse()
            .map_err(|_| PublicationError::Evidence)?,
    }))
}
