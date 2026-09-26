//! Exact read leases and GC exclusion. Callers hold the real runtime owner, including
//! metadata-only provider owners. No filesystem deletion or worker execution occurs here.
use crate::{Store, StoreError};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::BTreeSet;
use tf_domain::{BranchId, DatasetId, RequestId, SourceSnapshotId, VersionId};
use tf_protocol::canonical::{ContentDigest, DigestKind};
/// Lease/root refusal, distinct from database failure.
#[derive(Debug, thiserror::Error)]
pub enum RetentionError {
    /// Database or repository failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Missing data or an exclusive collection claim.
    #[error("The requested retained data is unavailable or being collected")]
    Unavailable,
    /// Wrong operation, stale fence, expired or released lease.
    #[error("The read lease expired, was released, or has a newer renewal")]
    Lease,
    /// Invalid bounds or a wall-clock rollback that requires clock reconciliation.
    #[error("Retention requires bounded requests and a nondecreasing clock")]
    ClockOrLimit,
    /// A live root still protects the candidate.
    #[error("The artifact is retained by a live version, lease, pin or build")]
    Rooted,
}
impl From<sqlx::Error> for RetentionError {
    fn from(e: sqlx::Error) -> Self {
        Self::Store(e.into())
    }
}
type Result<T> = std::result::Result<T, RetentionError>;
/// Different lifetimes share the same exact pin mechanism; plan validity is separate.
#[derive(Clone, Copy, Debug)]
pub enum LeaseKind {
    /// Interactive query/preview.
    Query,
    /// Draft read protection, not draft validity.
    Plan,
    /// Legacy copy pin retained for old runtime compatibility.
    Copy,
    /// Active build read.
    Build,
}
impl LeaseKind {
    fn name(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Plan => "plan",
            Self::Copy => "copy",
            Self::Build => "build",
        }
    }
}
/// A requested resource; Head is resolved only at initial acquisition.
#[derive(Clone, Copy, Debug)]
pub enum ReadTarget {
    /// Atomic head read plus lease.
    Head(BranchId, DatasetId),
    /// Exact historical local version.
    Version(VersionId),
    /// Exact verified local artifact/replica.
    Artifact(ContentDigest),
}
/// Opaque renewable capability. Cloning does not bypass renewal fencing.
#[derive(Clone, Debug)]
pub struct ReadLease {
    id: RequestId,
    operation: RequestId,
    fence: i64,
    version: Option<VersionId>,
    artifact: ContentDigest,
    expires: i64,
}
impl ReadLease {
    pub(crate) fn same_binding(&self, other: &Self) -> bool {
        self.id == other.id
            && self.operation == other.operation
            && self.version == other.version
            && self.artifact == other.artifact
            && other.fence > self.fence
    }

    /// Pinned version, unaffected by later head changes.
    pub fn version(&self) -> Option<VersionId> {
        self.version
    }
    /// Exact physical bytes to verify/read while this lease is live.
    pub fn artifact(&self) -> ContentDigest {
        self.artifact
    }
    /// UTC expiry, independent of head currentness and plan expiration.
    pub fn expires_at_us(&self) -> i64 {
        self.expires
    }
}
/// Explicit roots used by retained history, plans, schedules, backups or replicas.
#[derive(Clone, Copy, Debug)]
pub enum PinTarget {
    /// Local historical version and transitive inputs.
    Version(VersionId),
    /// Local object or foreign replica bytes.
    Artifact(ContentDigest),
    /// Retained captured source.
    Source(SourceSnapshotId),
}
/// Meaning of an explicit retention pin.
#[derive(Clone, Copy, Debug)]
pub enum PinKind {
    /// Retained historical version.
    History,
    /// Valid preview-plan pin; caller controls its independent deadline.
    Plan,
    /// Pending schedule evidence/occurrence.
    Schedule,
    /// In-progress consistent backup.
    Backup,
    /// Explicit retained foreign replica.
    Replica,
}
impl PinKind {
    fn name(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Plan => "plan",
            Self::Schedule => "schedule",
            Self::Backup => "backup",
            Self::Replica => "replica",
        }
    }
}
/// Latest-count/age retention is a union, never a maximum cap on explicit roots.
#[derive(Clone, Copy, Debug)]
pub struct RetentionPolicy {
    /// Number of versions retained per dataset/branch, at least one.
    pub latest: u32,
    /// Age window in microseconds, nonnegative.
    pub age_us: i64,
}
impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            latest: 20,
            age_us: 30 * 24 * 60 * 60 * 1_000_000,
        }
    }
}
/// Bounded transitive root snapshot. Collection rechecks in its own transaction.
#[derive(Debug)]
pub struct RetentionRoots {
    /// Retained local versions.
    pub versions: BTreeSet<VersionId>,
    /// Protected physical objects, including foreign input replicas.
    pub artifacts: BTreeSet<String>,
    /// Captured source snapshots needed by retained evidence.
    pub sources: BTreeSet<SourceSnapshotId>,
}
/// Exclusive collection capability. It is not user approval to delete files.
pub struct CollectionClaim {
    id: RequestId,
    digest: ContentDigest,
}
impl CollectionClaim {
    /// Exact object protected against new pins/publications.
    pub fn artifact(&self) -> ContentDigest {
        self.digest
    }
}
pub(crate) async fn clock(db: &mut SqliteConnection, now: i64) -> Result<()> {
    let changed =
        sqlx::query("UPDATE retention_clock SET now_us=? WHERE singleton=1 AND now_us<=?")
            .bind(now)
            .bind(now)
            .execute(db)
            .await?
            .rows_affected();
    if changed != 1 {
        return Err(RetentionError::ClockOrLimit);
    }
    Ok(())
}
fn expiry(now: i64, ttl: i64) -> Result<i64> {
    if ttl <= 0 {
        return Err(RetentionError::ClockOrLimit);
    }
    now.checked_add(ttl).ok_or(RetentionError::ClockOrLimit)
}
fn artifact(text: &str) -> Result<ContentDigest> {
    ContentDigest::from_hex(DigestKind::Artifact, text).map_err(|_| RetentionError::Unavailable)
}
pub(crate) async fn available(db: &mut SqliteConnection, digest: ContentDigest) -> Result<()> {
    if digest.kind() != DigestKind::Artifact {
        return Err(RetentionError::Unavailable);
    }
    let n:i64=sqlx::query_scalar("SELECT count(*) FROM artifacts a WHERE digest=? AND NOT EXISTS(SELECT 1 FROM artifact_gc_claims g WHERE g.digest=a.digest)").bind(digest.hex()).fetch_one(db).await?;
    if n != 1 {
        return Err(RetentionError::Unavailable);
    }
    Ok(())
}
async fn resolve(
    db: &mut SqliteConnection,
    target: ReadTarget,
) -> Result<(Option<VersionId>, ContentDigest)> {
    let row=match target{
 ReadTarget::Head(branch,dataset)=>sqlx::query("SELECT v.id,v.artifact_digest FROM dataset_heads h JOIN dataset_versions v ON v.id=h.version_id JOIN data_branches b ON b.id=h.branch_id WHERE h.branch_id=? AND h.dataset_id=? AND b.deleted_at_us IS NULL").bind(branch.to_string()).bind(dataset.to_string()).fetch_optional(&mut *db).await?,
 ReadTarget::Version(id)=>sqlx::query("SELECT id,artifact_digest FROM dataset_versions WHERE id=?").bind(id.to_string()).fetch_optional(&mut *db).await?,
 ReadTarget::Artifact(d)=>{available(db,d).await?;return Ok((None,d));}
 }.ok_or(RetentionError::Unavailable)?;
    let version = row
        .try_get::<String, _>(0)?
        .parse()
        .map_err(|_| RetentionError::Unavailable)?;
    let digest = artifact(&row.try_get::<String, _>(1)?)?;
    available(db, digest).await?;
    version_available(db, version).await?;
    Ok((Some(version), digest))
}
impl Store {
    /// Recover a fenced provider read capability for authenticated metadata transport.
    /// UUIDs are operation-scoped; other lease kinds cannot be controlled here.
    pub async fn external_read_lease(
        &mut self,
        id: RequestId,
        operation: RequestId,
        fence: i64,
    ) -> Result<ReadLease> {
        let row = sqlx::query("SELECT version_id,artifact_digest,expires_at_us FROM read_leases WHERE id=? AND owner_operation=? AND fence=? AND kind='query' AND released=0")
            .bind(id.to_string()).bind(operation.to_string()).bind(fence).fetch_optional(&mut self.db).await?.ok_or(RetentionError::Lease)?;
        let version: Option<String> = row.try_get(0)?;
        let version: Option<VersionId> = version
            .map(|s| s.parse().map_err(|_| RetentionError::Lease))
            .transpose()?;
        let digest = if let Some(v) = version {
            let d: String =
                sqlx::query_scalar("SELECT artifact_digest FROM dataset_versions WHERE id=?")
                    .bind(v.to_string())
                    .fetch_one(&mut self.db)
                    .await?;
            artifact(&d)?
        } else {
            artifact(&row.try_get::<String, _>(1)?)?
        };
        Ok(ReadLease {
            id,
            operation,
            fence,
            version,
            artifact: digest,
            expires: row.try_get(2)?,
        })
    }
    /// Read a head/exact identity and acquire its lease in one write transaction.
    pub async fn acquire_read(
        &mut self,
        id: RequestId,
        operation: RequestId,
        kind: LeaseKind,
        target: ReadTarget,
        now_us: i64,
        ttl_us: i64,
    ) -> Result<ReadLease> {
        let expires = expiry(now_us, ttl_us)?;
        // Commit the observed clock floor even when the following request is refused.
        clock(&mut self.db, now_us).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        clock(&mut tx, now_us).await?;
        let lease =
            lease_in_transaction(&mut tx, id, operation, kind, target, now_us, expires).await?;
        tx.commit().await?;
        Ok(lease)
    }
    /// Renew the same immutable binding. Expired/released/stale capabilities cannot revive it.
    pub async fn renew_read(
        &mut self,
        lease: &ReadLease,
        now_us: i64,
        ttl_us: i64,
    ) -> Result<ReadLease> {
        let expires = expiry(now_us, ttl_us)?;
        // Commit the observed clock floor even when the following request is refused.
        clock(&mut self.db, now_us).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        clock(&mut tx, now_us).await?;
        let n=sqlx::query("UPDATE read_leases SET renewed_at_us=?,expires_at_us=max(expires_at_us,?),fence=fence+1 WHERE id=? AND owner_operation=? AND fence=? AND expires_at_us>? AND released=0 AND fence<9223372036854775807").bind(now_us).bind(expires).bind(lease.id.to_string()).bind(lease.operation.to_string()).bind(lease.fence).bind(now_us).execute(&mut *tx).await?.rows_affected();
        if n != 1 {
            return Err(RetentionError::Lease);
        }
        tx.commit().await?;
        let mut new = lease.clone();
        new.fence += 1;
        new.expires = new.expires.max(expires);
        Ok(new)
    }
    /// Release only the current lease generation. Keep its identity to prevent ABA reuse.
    pub async fn release_read(&mut self, lease: &ReadLease) -> Result<()> {
        let n=sqlx::query("UPDATE read_leases SET released=1 WHERE id=? AND owner_operation=? AND fence=? AND released=0").bind(lease.id.to_string()).bind(lease.operation.to_string()).bind(lease.fence).execute(&mut self.db).await?.rows_affected();
        if n != 1 {
            return Err(RetentionError::Lease);
        }
        Ok(())
    }
    /// Explicit retention root. Expiry is independent of read leases and draft state.
    pub async fn pin_retained(
        &mut self,
        id: RequestId,
        owner: RequestId,
        kind: PinKind,
        target: PinTarget,
        now_us: i64,
        expires_at_us: Option<i64>,
    ) -> Result<()> {
        if expires_at_us.is_some_and(|t| t <= now_us) {
            return Err(RetentionError::ClockOrLimit);
        }
        // Commit the observed clock floor even when the following request is refused.
        clock(&mut self.db, now_us).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        clock(&mut tx, now_us).await?;
        let (version, digest, source) = match target {
            PinTarget::Version(v) => {
                resolve(&mut tx, ReadTarget::Version(v)).await?;
                (Some(v.to_string()), None, None)
            }
            PinTarget::Artifact(d) => {
                available(&mut tx, d).await?;
                (None, Some(d.hex()), None)
            }
            PinTarget::Source(s) => (None, None, Some(s.to_string())),
        };
        sqlx::query("INSERT INTO pins(id,artifact_digest,version_id,source_snapshot_id,owner_type,owner_id,expires_at_us) VALUES(?,?,?,?,?,?,?)").bind(id.to_string()).bind(digest).bind(version).bind(source).bind(kind.name()).bind(owner.to_string()).bind(expires_at_us).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Remove an operation's explicit pin without affecting another owner's root.
    pub async fn unpin_retained(&mut self, id: RequestId, owner: RequestId) -> Result<()> {
        let n = sqlx::query("DELETE FROM pins WHERE id=? AND owner_id=?")
            .bind(id.to_string())
            .bind(owner.to_string())
            .execute(&mut self.db)
            .await?
            .rows_affected();
        if n != 1 {
            return Err(RetentionError::Lease);
        }
        Ok(())
    }
    /// Snapshot roots using an injected UTC sample (virtual clocks need no sleeps).
    pub async fn retention_roots(
        &mut self,
        now_us: i64,
        policy: RetentionPolicy,
    ) -> Result<RetentionRoots> {
        // Commit the observed clock floor even when the following request is refused.
        clock(&mut self.db, now_us).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        clock(&mut tx, now_us).await?;
        let roots = roots(&mut tx, now_us, policy).await?;
        tx.commit().await?;
        Ok(roots)
    }
    /// Exclude new readers/publication after atomically rechecking all current roots.
    /// The later GC service must additionally enforce grace/quarantine, dry-run/approval,
    /// perform filesystem deletion, and reconcile interrupted claims before releasing them.
    pub async fn claim_collection(
        &mut self,
        id: RequestId,
        digest: ContentDigest,
        now_us: i64,
        policy: RetentionPolicy,
    ) -> Result<CollectionClaim> {
        // Commit the observed clock floor even when the following request is refused.
        clock(&mut self.db, now_us).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        clock(&mut tx, now_us).await?;
        available(&mut tx, digest).await?;
        if roots(&mut tx, now_us, policy)
            .await?
            .artifacts
            .contains(&digest.hex())
        {
            return Err(RetentionError::Rooted);
        }
        sqlx::query("INSERT INTO artifact_gc_claims VALUES(?,?,?)")
            .bind(digest.hex())
            .bind(id.to_string())
            .bind(now_us)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(CollectionClaim { id, digest })
    }
    /// Abandon a claim only when its bytes have not been removed. No automatic drop release.
    pub async fn abandon_collection(&mut self, claim: CollectionClaim) -> Result<()> {
        let n = sqlx::query("DELETE FROM artifact_gc_claims WHERE digest=? AND id=?")
            .bind(claim.digest.hex())
            .bind(claim.id.to_string())
            .execute(&mut self.db)
            .await?
            .rows_affected();
        if n != 1 {
            return Err(RetentionError::Lease);
        }
        Ok(())
    }
}
async fn roots(
    db: &mut SqliteConnection,
    now: i64,
    policy: RetentionPolicy,
) -> Result<RetentionRoots> {
    if policy.latest == 0 || policy.latest > 100_000 || policy.age_us < 0 {
        return Err(RetentionError::ClockOrLimit);
    }
    let cutoff = now
        .checked_sub(policy.age_us)
        .ok_or(RetentionError::ClockOrLimit)?;
    let versions:Vec<String>=sqlx::query_scalar(r#"WITH RECURSIVE
 ranked AS (SELECT new_version_id,row_number() OVER(PARTITION BY dataset_id,branch_id ORDER BY generation DESC) AS n FROM head_changes),
 live_build_inputs AS (SELECT ji.version_id FROM job_inputs ji JOIN jobs j ON j.id=ji.job_id JOIN builds b ON b.id=j.build_id WHERE b.state IN ('QUEUED','RUNNING') AND ji.version_id IS NOT NULL),
 contract_inputs AS (SELECT json_extract(i.value,'$.version') AS id FROM publication_contracts c JOIN jobs j ON j.id=c.job_id JOIN builds b ON b.id=j.build_id,json_each(c.contract_json,'$.inputs') i WHERE b.state IN ('QUEUED','RUNNING') AND json_extract(i.value,'$.workspace')=(SELECT id FROM workspaces)),
 retained(id) AS (
 SELECT h.version_id FROM dataset_heads h JOIN data_branches b ON b.id=h.branch_id WHERE b.deleted_at_us IS NULL
 UNION SELECT version_id FROM pins WHERE version_id IS NOT NULL AND (expires_at_us IS NULL OR expires_at_us>?)
 UNION SELECT version_id FROM read_leases WHERE released=0 AND expires_at_us>? AND version_id IS NOT NULL
 UNION SELECT new_version_id FROM ranked WHERE n<=?
 UNION SELECT id FROM dataset_versions WHERE published_at_us>=?
 UNION SELECT version_id FROM live_build_inputs
 UNION SELECT id FROM contract_inputs
 UNION SELECT c.version_id FROM cached_jobs c JOIN jobs j ON j.id=c.job_id JOIN builds b ON b.id=j.build_id WHERE b.state IN ('QUEUED','RUNNING')
 UNION SELECT v.id FROM version_inputs i JOIN retained r ON r.id=i.output_version_id JOIN dataset_versions v ON v.id=i.origin_version_id AND v.dataset_id=i.origin_dataset_id WHERE i.origin_workspace_id=(SELECT id FROM workspaces))
 SELECT id FROM retained LIMIT 100001"#).bind(now).bind(now).bind(i64::from(policy.latest)).bind(cutoff).fetch_all(&mut *db).await?;
    if versions.len() > 100_000 {
        return Err(RetentionError::ClockOrLimit);
    }
    let mut out = RetentionRoots {
        versions: BTreeSet::new(),
        artifacts: BTreeSet::new(),
        sources: BTreeSet::new(),
    };
    // Bounded temporary work table avoids an unbounded SQL parameter list and keeps all
    // recursive roots/provenance in the same transaction snapshot.
    sqlx::raw_sql("CREATE TEMP TABLE IF NOT EXISTS retained_work(id TEXT PRIMARY KEY); DELETE FROM retained_work;").execute(&mut *db).await?;
    for version in versions {
        out.versions
            .insert(version.parse().map_err(|_| RetentionError::Unavailable)?);
        sqlx::query("INSERT INTO retained_work VALUES(?)")
            .bind(version)
            .execute(&mut *db)
            .await?;
    }
    let digests:Vec<String>=sqlx::query_scalar(r#"SELECT artifact_digest FROM dataset_versions WHERE id IN (SELECT id FROM retained_work)
 UNION SELECT i.artifact_digest FROM version_inputs i JOIN retained_work r ON r.id=i.output_version_id WHERE i.origin_workspace_id=(SELECT id FROM workspaces)
 UNION SELECT artifact_digest FROM pins WHERE artifact_digest IS NOT NULL AND (expires_at_us IS NULL OR expires_at_us>?)
 UNION SELECT artifact_digest FROM read_leases WHERE artifact_digest IS NOT NULL AND released=0 AND expires_at_us>?
 UNION SELECT p.artifact_digest FROM publication_intents p JOIN attempts a ON a.id=p.attempt_id JOIN jobs j ON j.id=a.job_id JOIN builds b ON b.id=j.build_id WHERE p.state='PREPARED' AND b.state IN ('QUEUED','RUNNING')
 UNION SELECT artifact_digest FROM failed_check_candidates
 UNION SELECT json_extract(i.value,'$.artifact') FROM publication_contracts c JOIN jobs j ON j.id=c.job_id JOIN builds b ON b.id=j.build_id,json_each(c.contract_json,'$.inputs') i WHERE b.state IN ('QUEUED','RUNNING') AND json_extract(i.value,'$.workspace')=(SELECT id FROM workspaces) LIMIT 100001"#).bind(now).bind(now).fetch_all(&mut *db).await?;
    let sources:Vec<String>=sqlx::query_scalar("SELECT source_snapshot_id FROM dataset_versions WHERE id IN (SELECT id FROM retained_work) UNION SELECT source_snapshot_id FROM pins WHERE source_snapshot_id IS NOT NULL AND (expires_at_us IS NULL OR expires_at_us>?) UNION SELECT p.source_snapshot_id FROM build_plans p JOIN builds b ON b.plan_id=p.id WHERE b.state IN ('QUEUED','RUNNING') AND p.source_snapshot_id IS NOT NULL LIMIT 100001").bind(now).fetch_all(&mut *db).await?;
    if digests.len() > 100_000 || sources.len() > 100_000 {
        return Err(RetentionError::ClockOrLimit);
    }
    for d in digests {
        artifact(&d)?;
        out.artifacts.insert(d);
    }
    for s in sources {
        out.sources
            .insert(s.parse().map_err(|_| RetentionError::Unavailable)?);
    }
    sqlx::query("DELETE FROM retained_work").execute(db).await?;
    Ok(out)
}

/// Prevent newly retained lineage from crossing an existing exclusive collection claim.
pub(crate) async fn version_available(db: &mut SqliteConnection, version: VersionId) -> Result<()> {
    let row=sqlx::query(r#"WITH RECURSIVE lineage(id) AS (
 SELECT ? UNION SELECT v.id FROM version_inputs i JOIN lineage l ON l.id=i.output_version_id JOIN dataset_versions v ON v.id=i.origin_version_id AND v.dataset_id=i.origin_dataset_id WHERE i.origin_workspace_id=(SELECT id FROM workspaces) LIMIT 100001)
 SELECT count(*),coalesce(sum(CASE WHEN EXISTS(SELECT 1 FROM dataset_versions v JOIN artifact_gc_claims g ON g.digest=v.artifact_digest WHERE v.id=l.id) OR EXISTS(SELECT 1 FROM version_inputs i JOIN artifact_gc_claims g ON g.digest=i.artifact_digest WHERE i.output_version_id=l.id) THEN 1 ELSE 0 END),0) FROM lineage l"#).bind(version.to_string()).fetch_one(db).await?;
    if row.try_get::<i64, _>(0)? > 100_000 {
        return Err(RetentionError::ClockOrLimit);
    }
    if row.try_get::<i64, _>(1)? != 0 {
        return Err(RetentionError::Unavailable);
    }
    Ok(())
}

/// Caller owns an IMMEDIATE transaction and has persisted the sampled clock floor.
pub(crate) async fn lease_in_transaction(
    db: &mut SqliteConnection,
    id: RequestId,
    operation: RequestId,
    kind: LeaseKind,
    target: ReadTarget,
    now: i64,
    expires: i64,
) -> Result<ReadLease> {
    let (version, artifact) = resolve(db, target).await?;
    sqlx::query("INSERT INTO read_leases(id,version_id,artifact_digest,owner_operation,renewed_at_us,expires_at_us,fence,kind) VALUES(?,?,?,?,?,?,0,?)")
        .bind(id.to_string()).bind(version.map(|v| v.to_string()))
        .bind(if version.is_none() { Some(artifact.hex()) } else { None })
        .bind(operation.to_string()).bind(now).bind(expires).bind(kind.name())
        .execute(db).await?;
    Ok(ReadLease {
        id,
        operation,
        fence: 0,
        version,
        artifact,
        expires,
    })
}

pub(crate) async fn check_live_lease(
    db: &mut SqliteConnection,
    lease: &ReadLease,
    now: i64,
) -> Result<()> {
    let valid:i64=sqlx::query_scalar("SELECT count(*) FROM read_leases WHERE id=? AND owner_operation=? AND fence=? AND version_id=? AND expires_at_us>? AND released=0")
        .bind(lease.id.to_string()).bind(lease.operation.to_string()).bind(lease.fence)
        .bind(lease.version.map(|v|v.to_string())).bind(now).fetch_one(db).await?;
    if valid != 1 {
        return Err(RetentionError::Lease);
    }
    Ok(())
}
