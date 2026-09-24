//! Immutable original replay context and atomic exact-boundary lease acquisition.
//! Availability here is metadata/retention availability, not a promise that code environments
//! exist or that physical bytes pass verification. Execution must verify both before running.
use crate::{
    Store, StoreError,
    publication::InputProvenance,
    retention::{self, LeaseKind, ReadLease, ReadTarget, RetentionError},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::BTreeSet;
use tf_domain::{BranchName, BuildId, DatasetKey, RequestId, SourceSnapshotId, WorkspaceId};
use tf_protocol::canonical::{ContentDigest, DigestKind, canonical_json};

/// A replay cannot silently replace missing or mismatched original evidence.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Repository error.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Missing or collecting original bytes.
    #[error(transparent)]
    Retention(#[from] RetentionError),
    /// Missing, malformed, conflicting or wrong-owner manifest/version.
    #[error(
        "Original replay evidence is unavailable, inconsistent, or exceeds its supported bound"
    )]
    Evidence,
}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Store(e.into())
    }
}
/// One original read boundary, including its consumer and complete alias/selector provenance.
#[derive(Clone, Debug)]
pub struct ReplayBoundary {
    /// Consumer in the original effective write scope.
    pub consumer: DatasetKey,
    /// Immutable version and selection explanation; never a request for latest data.
    pub input: InputProvenance,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: String,
    workspace: String,
    code_digest: String,
    selector: Value,
    git: Option<Value>,
    files: Value,
    environment: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Boundary {
    consumer: String,
    alias: String,
    workspace: String,
    dataset: String,
    version: String,
    artifact: String,
    declared_branch: Value,
    starting_branch: String,
    resolved_branch: String,
    role: String,
    resolution: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u8,
    build: String,
    source: Source,
    parameters: Value,
    scope: Vec<String>,
    boundaries: Vec<Boundary>,
}
/// Retained metadata, deliberately separate from byte/environment execution verification.
#[derive(Clone, Debug)]
pub struct ReplayManifest(Manifest);
impl ReplayManifest {
    /// Original effective local write scope, independent of the new destination branch.
    pub fn scope(&self) -> Result<Vec<DatasetKey>, Error> {
        let workspace = self
            .0
            .source
            .workspace
            .parse()
            .map_err(|_| Error::Evidence)?;
        self.0
            .scope
            .iter()
            .map(|id| {
                Ok(DatasetKey::new(
                    workspace,
                    id.parse().map_err(|_| Error::Evidence)?,
                ))
            })
            .collect()
    }
    /// Original source identity; no fresh checkout discovery is implied.
    pub fn source(&self) -> Result<SourceSnapshotId, Error> {
        self.0.source.id.parse().map_err(|_| Error::Evidence)
    }
    /// Original environment identity/requirements, to be verified before execution.
    pub fn environment(&self) -> &Value {
        &self.0.source.environment
    }
    /// Captured file manifest and code identity for subsequent source verification.
    pub fn source_evidence(&self) -> Value {
        serde_json::to_value(&self.0.source).unwrap_or(Value::Null)
    }
    /// Original parameters with lossless protocol values retained unchanged.
    pub fn parameters(&self) -> &Value {
        &self.0.parameters
    }
    /// Entire canonical record for explanations and source/scope preparation.
    pub fn value(&self) -> Result<Value, Error> {
        serde_json::to_value(&self.0).map_err(|_| Error::Evidence)
    }
    /// Original boundary identities, preserving repeated aliases and selection evidence.
    pub fn boundaries(&self) -> Result<Vec<ReplayBoundary>, Error> {
        let workspace = self
            .0
            .source
            .workspace
            .parse()
            .map_err(|_| Error::Evidence)?;
        self.0
            .boundaries
            .iter()
            .map(|b| {
                Ok(ReplayBoundary {
                    consumer: DatasetKey::new(
                        workspace,
                        b.consumer.parse().map_err(|_| Error::Evidence)?,
                    ),
                    input: InputProvenance {
                        alias: b.alias.clone(),
                        dataset: DatasetKey::new(
                            b.workspace.parse().map_err(|_| Error::Evidence)?,
                            b.dataset.parse().map_err(|_| Error::Evidence)?,
                        ),
                        version: b.version.parse().map_err(|_| Error::Evidence)?,
                        artifact: ContentDigest::from_hex(DigestKind::Artifact, &b.artifact)
                            .map_err(|_| Error::Evidence)?,
                        declared_branch: b.declared_branch.clone(),
                        starting_branch: b.starting_branch.parse().map_err(|_| Error::Evidence)?,
                        resolved_branch: b.resolved_branch.parse().map_err(|_| Error::Evidence)?,
                        role: match b.role.as_str() {
                            "data" => crate::publication::InputRole::Data,
                            "validation" => crate::publication::InputRole::Validation,
                            _ => return Err(Error::Evidence),
                        },
                        resolution: b.resolution.clone(),
                    },
                })
            })
            .collect()
    }
}
/// Exact leases for all original boundaries. Dropping metadata does not release leases;
/// owners must renew/release tickets and verify source/environment and every artifact.
pub struct ReplayRead {
    /// Immutable original evidence.
    pub manifest: ReplayManifest,
    /// Explicit destination, independent of original input selectors; not created by this read.
    pub destination: BranchName,
    /// One ticket per boundary in manifest order. No partial set is committed on failure.
    pub leases: Vec<ReadLease>,
}
fn validate(m: &ReplayManifest) -> Result<(), Error> {
    if m.0.format != 1
        || m.0.scope.is_empty()
        || m.0.scope.len() > 10000
        || m.0.boundaries.len() > 10000
    {
        return Err(Error::Evidence);
    }
    let _: BuildId = m.0.build.parse().map_err(|_| Error::Evidence)?;
    let _: WorkspaceId = m.0.source.workspace.parse().map_err(|_| Error::Evidence)?;
    m.source()?;
    let mut scope = BTreeSet::new();
    for id in &m.0.scope {
        let _: tf_domain::DatasetId = id.parse().map_err(|_| Error::Evidence)?;
        if !scope.insert(id.as_str()) {
            return Err(Error::Evidence);
        }
    }
    let mut aliases = BTreeSet::new();
    for b in m.boundaries()? {
        if !scope.contains(b.consumer.dataset_id().to_string().as_str())
            || b.input.alias.is_empty()
            || b.input.alias.len() > 1024
            || b.input.alias.chars().any(char::is_control)
            || !aliases.insert((b.consumer, b.input.alias))
        {
            return Err(Error::Evidence);
        }
    }
    Ok(())
}
fn encode(m: &ReplayManifest) -> Result<String, Error> {
    validate(m)?;
    let bytes = canonical_json(&m.value()?).map_err(|_| Error::Evidence)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::Evidence);
    }
    String::from_utf8(bytes).map_err(|_| Error::Evidence)
}
async fn original_source(db: &mut SqliteConnection, build: BuildId) -> Result<Source, Error> {
    let r = sqlx::query("SELECT s.id,s.workspace_id,s.code_digest,s.selector_json,s.git_json,s.manifest_json,s.environment_json FROM builds b JOIN build_plans p ON p.id=b.plan_id JOIN source_snapshots s ON s.id=p.source_snapshot_id WHERE b.id=? AND p.disposition='ACCEPTED'")
        .bind(build.to_string()).fetch_optional(db).await?.ok_or(Error::Evidence)?;
    let json = |text: String| serde_json::from_str(&text).map_err(|_| Error::Evidence);
    Ok(Source {
        id: r.try_get(0)?,
        workspace: r.try_get(1)?,
        code_digest: r.try_get(2)?,
        selector: json(r.try_get(3)?)?,
        git: r.try_get::<Option<String>, _>(4)?.map(json).transpose()?,
        files: json(r.try_get(5)?)?,
        environment: json(r.try_get(6)?)?,
    })
}
async fn check_boundary(db: &mut SqliteConnection, b: &Boundary) -> Result<ReadTarget, Error> {
    let local: i64 = sqlx::query_scalar("SELECT count(*) FROM workspaces WHERE id=?")
        .bind(&b.workspace)
        .fetch_one(&mut *db)
        .await?;
    let matches: i64 = if local == 1 {
        sqlx::query_scalar("SELECT count(*) FROM dataset_versions v JOIN datasets d ON d.id=v.dataset_id WHERE v.id=? AND d.id=? AND d.workspace_id=? AND v.artifact_digest=?").bind(&b.version).bind(&b.dataset).bind(&b.workspace).bind(&b.artifact).fetch_one(&mut *db).await?
    } else {
        sqlx::query_scalar("SELECT count(*) FROM replicas WHERE version_id=? AND dataset_id=? AND workspace_id=? AND artifact_digest=? AND copy_state='VERIFIED'").bind(&b.version).bind(&b.dataset).bind(&b.workspace).bind(&b.artifact).fetch_one(&mut *db).await?
    };
    if matches != 1 {
        return Err(Error::Evidence);
    }
    Ok(if local == 1 {
        ReadTarget::Version(b.version.parse().map_err(|_| Error::Evidence)?)
    } else {
        ReadTarget::Artifact(
            ContentDigest::from_hex(DigestKind::Artifact, &b.artifact)
                .map_err(|_| Error::Evidence)?,
        )
    })
}

impl Store {
    /// Freeze complete accepted boundary evidence once. The caller supplies original effective
    /// scope/parameters and all resolved boundaries during acceptance, not during later replay.
    /// Idempotent retry is allowed only for byte-identical canonical evidence.
    pub async fn retain_replay(
        &mut self,
        build: BuildId,
        parameters: Value,
        scope: &[DatasetKey],
        boundaries: &[ReplayBoundary],
    ) -> Result<ReplayManifest, Error> {
        if scope.len() > 10000 || boundaries.len() > 10000 {
            return Err(Error::Evidence);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let source = original_source(&mut tx, build).await?;
        if scope
            .iter()
            .any(|d| d.workspace_id().to_string() != source.workspace)
        {
            return Err(Error::Evidence);
        }
        let m = ReplayManifest(Manifest {
            format: 1,
            build: build.to_string(),
            source,
            parameters,
            scope: scope.iter().map(|d| d.dataset_id().to_string()).collect(),
            boundaries: boundaries
                .iter()
                .map(|b| Boundary {
                    consumer: b.consumer.dataset_id().to_string(),
                    alias: b.input.alias.clone(),
                    workspace: b.input.dataset.workspace_id().to_string(),
                    dataset: b.input.dataset.dataset_id().to_string(),
                    version: b.input.version.to_string(),
                    artifact: b.input.artifact.hex(),
                    declared_branch: b.input.declared_branch.clone(),
                    starting_branch: b.input.starting_branch.to_string(),
                    resolved_branch: b.input.resolved_branch.to_string(),
                    role: b.input.role.name().into(),
                    resolution: b.input.resolution.clone(),
                })
                .collect(),
        });
        if boundaries
            .iter()
            .any(|b| b.consumer.workspace_id().to_string() != m.0.source.workspace)
        {
            return Err(Error::Evidence);
        }
        let text = encode(&m)?;
        for b in &m.0.boundaries {
            check_boundary(&mut tx, b).await?;
        }
        let old: Option<String> =
            sqlx::query_scalar("SELECT manifest_json FROM replay_manifests WHERE build_id=?")
                .bind(build.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        match old {
            Some(old) if old != text => return Err(Error::Evidence),
            Some(_) => (),
            None => {
                sqlx::query("INSERT INTO replay_manifests VALUES(?,?)")
                    .bind(build.to_string())
                    .bind(text)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(m)
    }
    /// Read original evidence and lease every exact local boundary atomically. Never looks up a
    /// head or current authoring configuration, and never creates the requested destination.
    pub async fn lease_replay(
        &mut self,
        build: BuildId,
        destination: BranchName,
        operation: RequestId,
        lease_ids: &[RequestId],
        now_us: i64,
        ttl_us: i64,
    ) -> Result<ReplayRead, Error> {
        let expires = now_us
            .checked_add(ttl_us)
            .filter(|_| ttl_us > 0)
            .ok_or(Error::Evidence)?;
        if lease_ids.len() > 10000 || destination.as_str().len() > 4096 {
            return Err(Error::Evidence);
        }
        retention::clock(&mut self.db, now_us).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        retention::clock(&mut tx, now_us).await?;
        let text: String =
            sqlx::query_scalar("SELECT manifest_json FROM replay_manifests WHERE build_id=?")
                .bind(build.to_string())
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(Error::Evidence)?;
        if text.len() > 1024 * 1024 {
            return Err(Error::Evidence);
        }
        let manifest = ReplayManifest(serde_json::from_str(&text).map_err(|_| Error::Evidence)?);
        validate(&manifest)?;
        if manifest.0.build != build.to_string()
            || lease_ids.len() != manifest.0.boundaries.len()
            || lease_ids.iter().collect::<BTreeSet<_>>().len() != lease_ids.len()
            || serde_json::to_value(original_source(&mut tx, build).await?)
                .map_err(|_| Error::Evidence)?
                != manifest.source_evidence()
        {
            return Err(Error::Evidence);
        }
        let mut leases = Vec::with_capacity(lease_ids.len());
        for (b, id) in manifest.0.boundaries.iter().zip(lease_ids) {
            let target = check_boundary(&mut tx, b).await?;
            let lease = retention::lease_in_transaction(
                &mut tx,
                *id,
                operation,
                LeaseKind::Build,
                target,
                now_us,
                expires,
            )
            .await?;
            if lease.artifact().hex() != b.artifact {
                return Err(Error::Evidence);
            }
            leases.push(lease);
        }
        tx.commit().await?;
        Ok(ReplayRead {
            manifest,
            destination,
            leases,
        })
    }
}

/// Reuse the closed replay validator for the acceptance transaction's original evidence.
pub(crate) fn encode_value(value: Value) -> Result<String, Error> {
    let manifest = ReplayManifest(serde_json::from_value(value).map_err(|_| Error::Evidence)?);
    encode(&manifest)
}
