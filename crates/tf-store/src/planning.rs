//! Immutable draft records and atomic acceptance. Filesystem/environment guards belong to composition.
use crate::{Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::BTreeSet;
use tf_domain::{
    BranchId, BuildId, CoordinatorSessionId, DatasetId, JobId, RequestId, SourceSnapshotId,
    VersionId, WorkspaceId,
};
use tf_protocol::canonical::{ContentDigest, DigestKind, content_digest};
/// Planning conflict retains a fixed actionable explanation.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// Persistent storage failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Stale/expired/conflicting draft, never an implicit replan.
    #[error("Saved plan cannot be accepted: {0}. Prepare a new plan")]
    Conflict(&'static str),
}
impl From<sqlx::Error> for PlanError {
    fn from(e: sqlx::Error) -> Self {
        Self::Store(e.into())
    }
}
type Result<T> = std::result::Result<T, PlanError>;
/// Exact branch lifecycle and optional dataset head, including absent guards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadGuard {
    /// Opaque branch name.
    pub name: String,
    /// Stable branch identity when present.
    pub branch: Option<String>,
    /// Lifecycle revision when present.
    pub revision: Option<i64>,
    /// Tombstoned branches can be recorded as unavailable fallback candidates.
    pub deleted: bool,
    /// Dataset to inspect; None guards only branch existence.
    pub dataset: Option<String>,
    /// Exact current version, None for an absent head.
    pub version: Option<String>,
    /// Publication generation, zero for absence.
    pub generation: i64,
}
/// One symbolic job, with a stable saved ID and guarded output generation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedWrite {
    /// Local stable dataset ID, including proposed registrations.
    pub dataset: String,
    /// Stable proposed job identity.
    pub job: String,
    /// Expected output head generation.
    pub generation: i64,
    /// All alias inputs; internal parents remain symbolic.
    pub bindings: Value,
}
/// Exact verified boundary and its expiring lease.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedRead {
    /// Consumer dataset ID.
    pub consumer: String,
    /// Consumer alias.
    pub alias: String,
    /// Input dataset identity, qualified by provenance.workspace.
    pub dataset: String,
    /// Exact retained version.
    pub version: String,
    /// Verified physical digest.
    pub artifact: String,
    /// Draft lease ID in the owning workspace (provider for foreign reads).
    pub lease: String,
    /// Semantic per-alias identity, independent of presentational metadata.
    pub semantic: Value,
    /// Immutable full input provenance.
    pub provenance: Value,
}
/// Closed internal format; callers retain exact captured discovery and assigned IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftPlan {
    /// Internal persistence version.
    pub format_version: u32,
    /// Immutable plan ID.
    pub id: String,
    /// Owning workspace.
    pub workspace: String,
    /// Retained verified capture ID.
    pub source: String,
    /// Complete source capture identity.
    pub source_digest: String,
    /// Original authoring registry bytes.
    pub registry: String,
    /// Exact additive replacement, including the proposed UUIDs.
    pub replacement: String,
    /// Frozen full validated discovery against the replacement registry.
    pub discovery: Value,
    /// Observed managed environment identity.
    pub environment: Value,
    /// Source manifest and optional Git provenance.
    pub source_evidence: Value,
    /// Output branch absent/present guard.
    pub output: HeadGuard,
    /// All output and read-candidate guards, including absent earlier fallbacks.
    pub guards: Vec<HeadGuard>,
    /// Topologically ordered producers.
    pub writes: Vec<PlannedWrite>,
    /// Every exact read boundary.
    pub reads: Vec<PlannedRead>,
    /// Frozen parameters, source decisions, resource/policy/request context.
    pub context: Value,
    /// Frozen evaluation/creation time in UTC microseconds.
    pub created_us: i64,
    /// Default lifetime is fifteen minutes.
    pub expires_us: i64,
}
fn conflict(reason: &'static str) -> PlanError {
    PlanError::Conflict(reason)
}
impl DraftPlan {
    /// Canonical digest over the complete closed record. Invalid/oversized carriers fail closed.
    pub fn digest(&self) -> Result<ContentDigest> {
        if self.format_version != 1
            || self.created_us < 0
            || self.expires_us <= self.created_us
            || self.writes.is_empty()
            || self.writes.len() > 10_000
            || self.reads.len() > 100_000
            || self.guards.len() > 100_000
            || self.output.dataset.is_some()
            || self.output.deleted
        {
            return Err(conflict("invalid draft structure"));
        }
        self.id
            .parse::<RequestId>()
            .map_err(|_| conflict("invalid plan identity"))?;
        self.workspace
            .parse::<WorkspaceId>()
            .map_err(|_| conflict("invalid workspace"))?;
        self.source
            .parse::<SourceSnapshotId>()
            .map_err(|_| conflict("invalid source"))?;
        ContentDigest::from_hex(DigestKind::Source, &self.source_digest)
            .map_err(|_| conflict("invalid source digest"))?;
        let mut datasets = BTreeSet::new();
        let mut jobs = BTreeSet::new();
        let mut aliases = BTreeSet::new();
        let mut leases = BTreeSet::new();
        for w in &self.writes {
            w.dataset
                .parse::<DatasetId>()
                .map_err(|_| conflict("invalid output identity"))?;
            w.job
                .parse::<JobId>()
                .map_err(|_| conflict("invalid job identity"))?;
            if w.generation < 0 || !datasets.insert(&w.dataset) || !jobs.insert(&w.job) {
                return Err(conflict("duplicate or invalid output"));
            }
        }
        for r in &self.reads {
            r.version
                .parse::<VersionId>()
                .map_err(|_| conflict("invalid input version"))?;
            r.dataset
                .parse::<DatasetId>()
                .map_err(|_| conflict("invalid input dataset"))?;
            r.lease
                .parse::<RequestId>()
                .map_err(|_| conflict("invalid lease"))?;
            ContentDigest::from_hex(DigestKind::Artifact, &r.artifact)
                .map_err(|_| conflict("invalid artifact"))?;
            if !datasets.contains(&r.consumer)
                || r.alias.is_empty()
                || !aliases.insert((&r.consumer, &r.alias))
                || !leases.insert(&r.lease)
            {
                return Err(conflict("invalid boundary alias or lease"));
            }
        }
        let value = serde_json::to_value(self).map_err(|_| conflict("invalid draft encoding"))?;
        content_digest(DigestKind::Compute, &value)
            .map_err(|_| conflict("draft exceeds canonical metadata limits"))
    }
}
async fn inspect(
    db: &mut SqliteConnection,
    workspace: &str,
    name: &str,
    dataset: Option<&str>,
) -> Result<HeadGuard> {
    let row = sqlx::query(
        "SELECT id,revision,deleted_at_us FROM data_branches WHERE workspace_id=? AND name=?",
    )
    .bind(workspace)
    .bind(name)
    .fetch_optional(&mut *db)
    .await?;
    let mut guard = HeadGuard {
        name: name.into(),
        branch: None,
        revision: None,
        deleted: false,
        dataset: dataset.map(str::to_owned),
        version: None,
        generation: 0,
    };
    if let Some(row) = row {
        guard.branch = Some(row.try_get(0)?);
        guard.revision = Some(row.try_get(1)?);
        guard.deleted = row.try_get::<Option<i64>, _>(2)?.is_some();
        if let Some(dataset) = dataset && let Some(head)=sqlx::query("SELECT version_id,generation FROM dataset_heads WHERE branch_id=? AND dataset_id=?").bind(&guard.branch).bind(dataset).fetch_optional(&mut *db).await?{guard.version=Some(head.try_get(0)?);guard.generation=head.try_get(1)?;}
    }
    Ok(guard)
}
impl Store {
    /// Retained source publication time for TTL evaluation; no live wall clock is read.
    pub async fn source_publication_time(
        &mut self,
        workspace: WorkspaceId,
        version: Option<&str>,
    ) -> Result<Option<i64>> {
        let Some(version) = version else {
            return Ok(None);
        };
        let published=sqlx::query_scalar("SELECT v.published_at_us FROM dataset_versions v JOIN datasets d ON d.id=v.dataset_id WHERE v.id=? AND d.workspace_id=?").bind(version).bind(workspace.to_string()).fetch_optional(&mut self.db).await?.ok_or(conflict("source head refers to missing or foreign metadata"))?;
        Ok(Some(published))
    }
    /// Capture a short read guard; this does not create branches, datasets or heads.
    pub async fn plan_guard(
        &mut self,
        workspace: WorkspaceId,
        name: &str,
        dataset: Option<DatasetId>,
    ) -> Result<HeadGuard> {
        inspect(
            &mut self.db,
            &workspace.to_string(),
            name,
            dataset.map(|id| id.to_string()).as_deref(),
        )
        .await
    }
    /// Save exact immutable metadata only. Proposed IDs are not indexed or authored.
    pub async fn save_draft(&mut self, plan: &DraftPlan) -> Result<()> {
        let digest = plan.digest()?.hex();
        let encoded = serde_json::to_string(plan).map_err(|_| conflict("invalid encoding"))?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) =
            sqlx::query_scalar::<_, String>("SELECT digest FROM build_plans WHERE id=?")
                .bind(&plan.id)
                .fetch_optional(&mut *tx)
                .await?
        {
            if existing != digest {
                return Err(conflict("plan ID already holds different content"));
            }
        } else {
            sqlx::query("INSERT INTO build_plans(id,disposition,candidate_json,registry_diff_json,context_json,targets_json,bindings_json,guards_json,digest,expires_at_us) VALUES(?,'DRAFT',?,?,?,?,?,?,?,?)").bind(&plan.id).bind(encoded).bind(json!({"replacement":plan.replacement}).to_string()).bind(plan.context.to_string()).bind(json!(plan.writes).to_string()).bind(json!(plan.reads).to_string()).bind(json!(plan.guards).to_string()).bind(digest).bind(plan.expires_us).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Load and authenticate the original frozen proposal; never recapture or rediscover it.
    pub async fn load_draft(&mut self, id: RequestId) -> Result<DraftPlan> {
        let row = sqlx::query("SELECT candidate_json,digest FROM build_plans WHERE id=? AND length(candidate_json)<=16777216")
            .bind(id.to_string())
            .fetch_optional(&mut self.db)
            .await?
            .ok_or(conflict("draft is missing"))?;
        let plan: DraftPlan = serde_json::from_str(&row.try_get::<String, _>(0)?)
            .map_err(|_| conflict("draft format is corrupt or unsupported"))?;
        if plan.id != id.to_string() || plan.digest()?.hex() != row.try_get::<String, _>(1)? {
            return Err(conflict("saved draft content changed"));
        }
        Ok(plan)
    }
    /// Recheck all database guards before any authoring mutation. Acceptance repeats this inside its transaction.
    pub async fn check_draft(&mut self, plan: &DraftPlan, now: i64) -> Result<()> {
        check(&mut self.db, plan, now).await
    }
    /// Accept and reserve the entire write set or neither. Caller verified source/environment,
    /// committed the exact additive registry diff and still holds the real runtime owner.
    pub async fn accept_draft(
        &mut self,
        plan: &DraftPlan,
        build: BuildId,
        new_branch: BranchId,
        session: CoordinatorSessionId,
        now: i64,
    ) -> Result<BranchId> {
        plan.digest()?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        check(&mut tx, plan, now).await?;
        let authority: i64 =
            sqlx::query_scalar("SELECT count(*) FROM workspaces WHERE id=? AND runtime_owner=?")
                .bind(&plan.workspace)
                .bind(session.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if authority != 1 {
            return Err(conflict("coordinator ownership changed"));
        }
        let branch = if let Some(id) = &plan.output.branch {
            id.parse::<BranchId>()
                .map_err(|_| conflict("invalid output branch"))?
        } else {
            sqlx::query("INSERT INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,?,0)")
                .bind(new_branch.to_string())
                .bind(&plan.workspace)
                .bind(&plan.output.name)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('branch.create',?,?)").bind(json!({"branch_id":new_branch.to_string(),"name":plan.output.name,"plan":plan.id}).to_string()).bind(now).execute(&mut *tx).await?;
            new_branch
        };
        let source_json = json!({"selector":plan.source_evidence["selector"],"files":plan.source_evidence["files"]});
        let previous=sqlx::query("SELECT workspace_id,code_digest,selector_json,git_json,manifest_json,environment_json FROM source_snapshots WHERE id=?").bind(&plan.source).fetch_optional(&mut *tx).await?;
        if let Some(row) = previous {
            if row.try_get::<String, _>(0)? != plan.workspace
                || row.try_get::<String, _>(1)? != plan.source_digest
            {
                return Err(conflict("retained source identity changed"));
            }
            for (index, expected) in [
                (2, &source_json["selector"]),
                (3, &plan.source_evidence["git"]),
                (4, &source_json["files"]),
                (5, &plan.environment),
            ] {
                let text: Option<String> = row.try_get(index)?;
                let actual: Value = text
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(|_| conflict("corrupt retained source metadata"))?
                    .unwrap_or(Value::Null);
                if actual != *expected {
                    return Err(conflict("retained source/environment metadata changed"));
                }
            }
        }
        sqlx::query("INSERT INTO source_snapshots(id,workspace_id,code_digest,selector_json,git_json,manifest_json,environment_json) VALUES(?,?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING").bind(&plan.source).bind(&plan.workspace).bind(&plan.source_digest).bind(source_json["selector"].to_string()).bind(plan.source_evidence["git"].to_string()).bind(source_json["files"].to_string()).bind(plan.environment.to_string()).execute(&mut *tx).await?;
        let policies = plan.context["branch_policies"]
            .as_object()
            .ok_or(conflict("missing captured branch policies"))?;
        let old: Vec<(String, String)> = sqlx::query_as(
            "SELECT starting_branch,policy_json FROM branch_policies WHERE source_snapshot_id=?",
        )
        .bind(&plan.source)
        .fetch_all(&mut *tx)
        .await?;
        if !old.is_empty()
            && (old.len() != policies.len()
                || old.iter().any(|(k, v)| {
                    serde_json::from_str::<Value>(v).ok().as_ref() != policies.get(k)
                }))
        {
            return Err(conflict("captured branch policies changed"));
        }
        for (start, policy) in policies {
            sqlx::query("INSERT INTO branch_policies(source_snapshot_id,starting_branch,policy_json) VALUES(?,?,?) ON CONFLICT(source_snapshot_id,starting_branch) DO NOTHING").bind(&plan.source).bind(start).bind(policy.to_string()).execute(&mut *tx).await?;
        }
        sqlx::query(
            "UPDATE build_plans SET disposition='ACCEPTED',source_snapshot_id=? WHERE id=?",
        )
        .bind(&plan.source)
        .bind(&plan.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO builds(id,plan_id,trigger_json,requested_by,state,created_at_us) VALUES(?,?,'{}','local','QUEUED',?)").bind(build.to_string()).bind(&plan.id).bind(now).execute(&mut *tx).await?;
        for w in &plan.writes {
            let valid:i64=sqlx::query_scalar("SELECT count(*) FROM datasets WHERE id=? AND workspace_id=? AND tombstoned_at_us IS NULL").bind(&w.dataset).bind(&plan.workspace).fetch_one(&mut *tx).await?;
            if valid != 1 {
                return Err(conflict("proposed dataset has not been registered"));
            }
            let occupied: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM write_reservations WHERE branch_id=? AND dataset_id=?",
            )
            .bind(branch.to_string())
            .bind(&w.dataset)
            .fetch_one(&mut *tx)
            .await?;
            if occupied != 0 {
                return Err(conflict(
                    "another build owns an output; retry acceptance before expiry",
                ));
            }
            sqlx::query("INSERT INTO jobs(id,build_id,dataset_id,branch_id,state,bindings_json) VALUES(?,?,?,?,'PLANNED',?)").bind(&w.job).bind(build.to_string()).bind(&w.dataset).bind(branch.to_string()).bind(w.bindings.to_string()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO write_reservations(branch_id,dataset_id,build_id,session_id,fence,acquired_at_us,expected_head_generation) VALUES(?,?,?,?,1,?,?)").bind(branch.to_string()).bind(&w.dataset).bind(build.to_string()).bind(session.to_string()).bind(now).bind(w.generation).execute(&mut *tx).await?;
        }
        let boundaries: Vec<Value> = plan
            .reads
            .iter()
            .map(|r| {
                let mut value = r.provenance.clone();
                value["consumer"] = json!(r.consumer);
                value
            })
            .collect();
        let replay=crate::replay::encode_value(json!({"format":1,"build":build.to_string(),"source":{"id":plan.source,"workspace":plan.workspace,"code_digest":plan.source_digest,"selector":source_json["selector"],"git":plan.source_evidence["git"],"files":source_json["files"],"environment":plan.environment},"parameters":plan.context["parameters"],"scope":plan.writes.iter().map(|w|&w.dataset).collect::<Vec<_>>(),"boundaries":boundaries})).map_err(|_|conflict("original replay evidence is incomplete or too large"))?;
        sqlx::query("INSERT INTO replay_manifests(build_id,manifest_json) VALUES(?,?)")
            .bind(build.to_string())
            .bind(replay)
            .execute(&mut *tx)
            .await?;
        let expiry = now
            .checked_add(3_600_000_000)
            .ok_or(conflict("lease clock overflow"))?;
        for read in &plan.reads {
            sqlx::query(
                "UPDATE read_leases SET kind='build',expires_at_us=?,fence=fence+1 WHERE id=?",
            )
            .bind(expiry)
            .bind(&read.lease)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('plan.accept',?,?)",
        )
        .bind(
            json!({"plan":plan.id,"build":build.to_string(),"digest":plan.digest()?.hex()})
                .to_string(),
        )
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(branch)
    }
}
async fn check(db: &mut SqliteConnection, plan: &DraftPlan, now: i64) -> Result<()> {
    if now < plan.created_us || now >= plan.expires_us {
        return Err(conflict(
            "draft expired or evaluation clock moved backwards",
        ));
    }
    let row = sqlx::query("SELECT disposition,digest FROM build_plans WHERE id=?")
        .bind(&plan.id)
        .fetch_optional(&mut *db)
        .await?
        .ok_or(conflict("draft is missing"))?;
    if row.try_get::<String, _>(0)? != "DRAFT"
        || row.try_get::<String, _>(1)? != plan.digest()?.hex()
    {
        return Err(conflict("draft was already accepted or changed"));
    }
    for guard in std::iter::once(&plan.output).chain(&plan.guards) {
        if inspect(db, &plan.workspace, &guard.name, guard.dataset.as_deref()).await? != *guard {
            return Err(conflict("branch or input/output head changed"));
        }
    }
    let clock: i64 = sqlx::query_scalar("SELECT now_us FROM retention_clock WHERE singleton=1")
        .fetch_one(&mut *db)
        .await?;
    if now < clock {
        return Err(conflict("retention clock moved backwards"));
    }
    for read in &plan.reads {
        let origin = read.provenance["workspace"]
            .as_str()
            .ok_or_else(|| conflict("missing input origin"))?;
        origin
            .parse::<WorkspaceId>()
            .map_err(|_| conflict("invalid input origin"))?;
        if read.semantic["origin_workspace"] != origin
            || read.semantic["origin_dataset"] != read.dataset
            || read.provenance["dataset"] != read.dataset
            || read.provenance["version"] != read.version
            || read.semantic["version"] != read.version
            || read.provenance["artifact"] != read.artifact
            || read.semantic["artifact"] != read.artifact
        {
            return Err(conflict("input provenance identity mismatch"));
        }
        let valid: i64 = if origin == plan.workspace {
            sqlx::query_scalar("SELECT count(*) FROM read_leases l JOIN dataset_versions v ON v.id=l.version_id JOIN datasets d ON d.id=v.dataset_id WHERE l.id=? AND l.owner_operation=? AND l.version_id=? AND v.artifact_digest=? AND l.released=0 AND l.expires_at_us>? AND v.dataset_id=? AND d.workspace_id=? AND NOT EXISTS(SELECT 1 FROM artifact_gc_claims g WHERE g.digest=v.artifact_digest)").bind(&read.lease).bind(&plan.id).bind(&read.version).bind(&read.artifact).bind(now).bind(&read.dataset).bind(&plan.workspace).fetch_one(&mut *db).await?
        } else {
            // Provider leases are checked by composition before acceptance/execution.
            sqlx::query_scalar("SELECT count(*) FROM foreign_versions WHERE workspace_id=? AND dataset_id=? AND version_id=? AND manifest_digest=?").bind(origin).bind(&read.dataset).bind(&read.version).bind(&read.artifact).fetch_one(&mut *db).await?
        };
        if valid != 1 {
            return Err(conflict(
                "an exact input lease expired or its original bytes are unavailable",
            ));
        }
    }
    Ok(())
}
