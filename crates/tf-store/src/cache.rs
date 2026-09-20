//! Branch-local reuse of published versions. Writers require the real coordinator lock.
//! No filesystem work occurs in these short transactions.
use crate::{
    Store, StoreError,
    artifacts::VerifiedArtifact,
    publication::{self, PublicationContract, PublicationError},
    retention::{self, LeaseKind, ReadLease, ReadTarget, RetentionError},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use tf_domain::{
    AttemptId, BuildId, DatasetId, JobId, RequestId, VersionId,
    execution::{ExecutionBinding, Fence, OutputTarget},
};
use tf_protocol::canonical::{ContentDigest, DigestKind, canonical_json};

/// A cache miss is ordinary; invalid state/integrity is an explicit error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Persistent storage failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Publication identity, cancellation or generation guard failed.
    #[error(transparent)]
    Publication(#[from] PublicationError),
    /// Expired lease or unavailable retained object.
    #[error(transparent)]
    Retention(#[from] RetentionError),
    /// Incomplete or contradictory accepted cache evidence.
    #[error("Cache reuse requires matching accepted computation and original check evidence")]
    Evidence,
}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Store(e.into())
    }
}
type Result<T> = std::result::Result<T, Error>;
/// A trusted planner supplies the finalized key only after every parent binds.
#[derive(Clone, Debug)]
pub struct Request {
    /// Accepted build identity.
    pub build: BuildId,
    /// Job with no execution attempts.
    pub job: JobId,
    /// Frozen plan and source.
    pub binding: ExecutionBinding,
    /// Exact output branch and expected generation.
    pub target: OutputTarget,
    /// Current coordinator and complete write-reservation fence.
    pub fence: Fence,
    /// Complete semantic/check key and exact input obligations.
    pub contract: PublicationContract,
    /// UTC observation time; original result timestamps are never rewritten.
    pub at_us: i64,
}
/// Immutable cached result; its version and producing attempt are historical.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// Reused original version.
    pub version: VersionId,
    /// Original successful materialization, never an attempt for this cached job.
    pub original_attempt: AttemptId,
    /// Output generation after adoption, unchanged for an already-current version.
    pub generation: u64,
    /// Present only when the head changed; event type is dataset.head_changed.
    pub event: Option<RequestId>,
}
/// Opaque leased candidate, not yet proof of file integrity.
pub struct Candidate {
    request: Request,
    version: VersionId,
    attempt: AttemptId,
    lease: ReadLease,
}
impl Candidate {
    /// Exact object to verify, without consulting another head.
    pub fn artifact(&self) -> ContentDigest {
        self.lease.artifact()
    }
    /// Release on failure or success; owner-held verification prevents collection races.
    pub fn lease(&self) -> &ReadLease {
        &self.lease
    }
}
/// Bounded lookup returns one deterministic candidate, a miss, or durable retry readback.
pub enum Lookup {
    /// No retained match in this output branch.
    Miss,
    /// Requires strict full-byte artifact verification before adoption.
    Candidate(Box<Candidate>),
    /// This exact job has already committed reuse.
    Reused(Receipt),
}
fn num(n: u64) -> Result<i64> {
    i64::try_from(n).map_err(|_| Error::Evidence)
}
fn encode(value: &Value) -> Result<String> {
    let bytes = canonical_json(value).map_err(|_| Error::Evidence)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::Evidence);
    }
    String::from_utf8(bytes).map_err(|_| Error::Evidence)
}
async fn identity(db: &mut SqliteConnection, r: &Request) -> Result<()> {
    publication::authority(db, r.target.dataset.workspace_id(), r.fence.session).await?;
    let row=sqlx::query("SELECT c.contract_json,p.context_json FROM jobs j JOIN builds b ON b.id=j.build_id JOIN build_plans p ON p.id=b.plan_id JOIN publication_contracts c ON c.job_id=j.id JOIN datasets d ON d.id=j.dataset_id JOIN data_branches branch ON branch.id=j.branch_id WHERE j.id=? AND b.id=? AND p.id=? AND p.source_snapshot_id=? AND p.disposition='ACCEPTED' AND j.dataset_id=? AND j.branch_id=? AND d.workspace_id=? AND branch.workspace_id=?")
        .bind(r.job.to_string()).bind(r.build.to_string()).bind(r.binding.plan.to_string()).bind(r.binding.source.to_string()).bind(r.target.dataset.dataset_id().to_string()).bind(r.target.branch.to_string()).bind(r.target.dataset.workspace_id().to_string()).bind(r.target.dataset.workspace_id().to_string()).fetch_optional(&mut *db).await?.ok_or(Error::Evidence)?;
    if row.try_get::<String, _>(0)? != r.contract.canonical()? {
        return Err(Error::Evidence);
    }
    let context: Value =
        serde_json::from_str(&row.try_get::<String, _>(1)?).map_err(|_| Error::Evidence)?;
    if context["force"] == true
        || context["source_decisions"][r.target.dataset.dataset_id().to_string()]["executes"]
            == true
    {
        return Err(Error::Evidence);
    }
    Ok(())
}
async fn guards(db: &mut SqliteConnection, r: &Request) -> Result<()> {
    let row=sqlx::query("SELECT b.cancel_requested,b.state,j.state FROM jobs j JOIN builds b ON b.id=j.build_id JOIN datasets d ON d.id=j.dataset_id JOIN data_branches branch ON branch.id=j.branch_id WHERE j.id=? AND d.tombstoned_at_us IS NULL AND branch.deleted_at_us IS NULL AND NOT EXISTS(SELECT 1 FROM attempts a WHERE a.job_id=j.id)")
        .bind(r.job.to_string()).fetch_optional(&mut *db).await?.ok_or(Error::Evidence)?;
    if row.try_get::<i64, _>(0)? != 0 {
        return Err(PublicationError::Canceled.into());
    }
    if !matches!(row.try_get::<String, _>(1)?.as_str(), "QUEUED" | "RUNNING")
        || !matches!(
            row.try_get::<String, _>(2)?.as_str(),
            "PLANNED" | "WAITING_DEPENDENCIES" | "QUEUED"
        )
    {
        return Err(Error::Evidence);
    }
    let reserved:i64=sqlx::query_scalar("SELECT count(*) FROM write_reservations WHERE build_id=? AND dataset_id=? AND branch_id=? AND session_id=? AND fence=? AND expected_head_generation=?")
        .bind(r.build.to_string()).bind(r.target.dataset.dataset_id().to_string()).bind(r.target.branch.to_string()).bind(r.fence.session.to_string()).bind(num(r.fence.generation)?).bind(num(r.target.expected_generation)?).fetch_one(&mut *db).await?;
    let incomplete:i64=sqlx::query_scalar("SELECT count(*) FROM jobs j WHERE j.build_id=? AND NOT EXISTS(SELECT 1 FROM write_reservations w WHERE w.build_id=j.build_id AND w.dataset_id=j.dataset_id AND w.branch_id=j.branch_id AND w.session_id=? AND w.fence=?)")
        .bind(r.build.to_string()).bind(r.fence.session.to_string()).bind(num(r.fence.generation)?).fetch_one(&mut *db).await?;
    if reserved != 1 || incomplete != 0 {
        return Err(PublicationError::Fence.into());
    }
    let generation: Option<i64> = sqlx::query_scalar(
        "SELECT generation FROM dataset_heads WHERE dataset_id=? AND branch_id=?",
    )
    .bind(r.target.dataset.dataset_id().to_string())
    .bind(r.target.branch.to_string())
    .fetch_optional(&mut *db)
    .await?;
    if generation.unwrap_or(0) != num(r.target.expected_generation)? {
        return Err(PublicationError::Conflict.into());
    }
    publication::validate_inputs(db, r.target.dataset.workspace_id(), &r.contract).await?;
    Ok(())
}
async fn receipt(db: &mut SqliteConnection, r: &Request) -> Result<Option<Receipt>> {
    sqlx::query(
        "SELECT version_id,original_attempt_id,generation,event_id,expected_generation,session_id,fence FROM cached_jobs WHERE job_id=?",
    )
    .bind(r.job.to_string())
    .fetch_optional(db)
    .await?
    .map(|row| {
        if row.try_get::<i64, _>(4)? != num(r.target.expected_generation)?
            || row.try_get::<String, _>(5)? != r.fence.session.to_string()
            || row.try_get::<i64, _>(6)? != num(r.fence.generation)?
        {
            return Err(PublicationError::Fence.into());
        }
        Ok(Receipt {
            version: row
                .try_get::<String, _>(0)?
                .parse()
                .map_err(|_| Error::Evidence)?,
            original_attempt: row
                .try_get::<String, _>(1)?
                .parse()
                .map_err(|_| Error::Evidence)?,
            generation: u64::try_from(row.try_get::<i64, _>(2)?).map_err(|_| Error::Evidence)?,
            event: row
                .try_get::<Option<String>, _>(3)?
                .map(|s| s.parse().map_err(|_| Error::Evidence))
                .transpose()?,
        })
    })
    .transpose()
}
async fn evidence(
    db: &mut SqliteConnection,
    r: &Request,
    version: VersionId,
    attempt: AttemptId,
    artifact: ContentDigest,
) -> Result<Vec<String>> {
    retention::version_available(db, version).await?;
    retention::available(db, artifact).await?;
    let valid:i64=sqlx::query_scalar("SELECT count(*) FROM dataset_versions v JOIN attempts a ON a.id=v.attempt_id JOIN jobs j ON j.id=a.job_id JOIN publication_contracts c ON c.job_id=j.id WHERE v.id=? AND v.dataset_id=? AND v.artifact_digest=? AND v.attempt_id=? AND v.compute_fingerprint=? AND v.check_fingerprint=? AND v.published_at_us<=? AND a.state='SUCCEEDED' AND j.state='SUCCEEDED' AND j.branch_id=? AND json_extract(c.contract_json,'$.compute')=v.compute_fingerprint AND json_extract(c.contract_json,'$.checks_fingerprint')=v.check_fingerprint")
        .bind(version.to_string()).bind(r.target.dataset.dataset_id().to_string()).bind(artifact.hex()).bind(attempt.to_string()).bind(&r.contract.compute_fingerprint).bind(&r.contract.check_fingerprint).bind(r.at_us).bind(r.target.branch.to_string()).fetch_one(&mut *db).await?;
    if valid != 1 {
        return Err(Error::Evidence);
    }
    let ids =
        publication::validation_results(db, attempt, &artifact.hex(), &r.contract, r.at_us).await?;
    let linked:Vec<String>=sqlx::query_scalar("SELECT result_id FROM version_check_results WHERE version_id=? ORDER BY result_id LIMIT 10001").bind(version.to_string()).fetch_all(db).await?;
    let mut sorted = ids.clone();
    sorted.sort();
    if sorted != linked {
        return Err(Error::Evidence);
    }
    Ok(ids)
}
impl Store {
    /// Select one branch-scoped retained match and lease it atomically before file reads.
    /// Corrupt selected bytes must be reported, never hidden by trying an older candidate.
    pub async fn lookup_cache(
        &mut self,
        r: &Request,
        lease: RequestId,
        ttl_us: i64,
    ) -> Result<Lookup> {
        let expires = r
            .at_us
            .checked_add(ttl_us)
            .filter(|_| ttl_us > 0 && r.at_us >= 0)
            .ok_or(Error::Evidence)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        identity(&mut tx, r).await?;
        if let Some(receipt) = receipt(&mut tx, r).await? {
            tx.commit().await?;
            return Ok(Lookup::Reused(receipt));
        }
        guards(&mut tx, r).await?;
        retention::clock(&mut tx, r.at_us).await?;
        let row=sqlx::query("SELECT v.id,v.attempt_id,v.artifact_digest FROM dataset_versions v JOIN attempts a ON a.id=v.attempt_id JOIN jobs j ON j.id=a.job_id WHERE v.dataset_id=? AND j.branch_id=? AND v.compute_fingerprint=? AND v.check_fingerprint=? AND a.state='SUCCEEDED' AND j.state='SUCCEEDED' AND NOT EXISTS(SELECT 1 FROM artifact_gc_claims g WHERE g.digest=v.artifact_digest) ORDER BY EXISTS(SELECT 1 FROM dataset_heads h WHERE h.branch_id=j.branch_id AND h.dataset_id=v.dataset_id AND h.version_id=v.id) DESC,v.published_at_us DESC,v.id LIMIT 1")
            .bind(r.target.dataset.dataset_id().to_string()).bind(r.target.branch.to_string()).bind(&r.contract.compute_fingerprint).bind(&r.contract.check_fingerprint).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(Lookup::Miss);
        };
        let version: VersionId = row
            .try_get::<String, _>(0)?
            .parse()
            .map_err(|_| Error::Evidence)?;
        let attempt: AttemptId = row
            .try_get::<String, _>(1)?
            .parse()
            .map_err(|_| Error::Evidence)?;
        let artifact = ContentDigest::from_hex(DigestKind::Artifact, &row.try_get::<String, _>(2)?)
            .map_err(|_| Error::Evidence)?;
        evidence(&mut tx, r, version, attempt, artifact).await?;
        let lease = retention::lease_in_transaction(
            &mut tx,
            lease,
            RequestId::from_bytes(*r.build.as_bytes()),
            LeaseKind::Build,
            ReadTarget::Version(version),
            r.at_us,
            expires,
        )
        .await?;
        tx.commit().await?;
        Ok(Lookup::Candidate(Box::new(Candidate {
            request: r.clone(),
            version,
            attempt,
            lease,
        })))
    }
    /// Complete reuse atomically after strict artifact verification. Original checks are linked,
    /// never copied/re-timestamped. All reservations remain until the build is terminal.
    pub async fn adopt_cache(
        &mut self,
        c: &Candidate,
        artifact: &VerifiedArtifact,
        now_us: i64,
    ) -> Result<Receipt> {
        let r = &c.request;
        if artifact.digest() != c.artifact()
            || self.path.parent() != Some(artifact.workspace().join(".transflow/runtime").as_path())
            || now_us < r.at_us
        {
            return Err(Error::Evidence);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        identity(&mut tx, r).await?;
        if let Some(receipt) = receipt(&mut tx, r).await? {
            tx.commit().await?;
            return Ok(receipt);
        }
        guards(&mut tx, r).await?;
        retention::clock(&mut tx, now_us).await?;
        retention::check_live_lease(&mut tx, &c.lease, now_us).await?;
        let checks = evidence(&mut tx, r, c.version, c.attempt, c.artifact()).await?;
        let manifest: String =
            sqlx::query_scalar("SELECT manifest_json FROM artifacts WHERE digest=?")
                .bind(c.artifact().hex())
                .fetch_one(&mut *tx)
                .await?;
        if manifest != encode(artifact.manifest())? {
            return Err(Error::Evidence);
        }
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT version_id FROM dataset_heads WHERE branch_id=? AND dataset_id=?",
        )
        .bind(r.target.branch.to_string())
        .bind(r.target.dataset.dataset_id().to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let changed = previous.as_deref() != Some(&c.version.to_string());
        let generation = num(r.target.expected_generation)?
            .checked_add(i64::from(changed))
            .ok_or(Error::Evidence)?;
        let event = if changed {
            Some(RequestId::from_bytes(*r.job.as_bytes()))
        } else {
            None
        };
        let payload = encode(
            &json!({"job_id":r.job.to_string(),"version_id":c.version.to_string(),"original_attempt_id":c.attempt.to_string(),"dataset_id":r.target.dataset.dataset_id().to_string(),"branch_id":r.target.branch.to_string(),"generation":generation.to_string(),"cause":"cache_reuse"}),
        )?;
        if let Some(event) = event {
            if r.target.expected_generation == 0 {
                sqlx::query("INSERT INTO dataset_heads VALUES(?,?,?,?)")
                    .bind(r.target.branch.to_string())
                    .bind(r.target.dataset.dataset_id().to_string())
                    .bind(c.version.to_string())
                    .bind(generation)
                    .execute(&mut *tx)
                    .await?;
            } else {
                let n=sqlx::query("UPDATE dataset_heads SET version_id=?,generation=? WHERE branch_id=? AND dataset_id=? AND generation=?").bind(c.version.to_string()).bind(generation).bind(r.target.branch.to_string()).bind(r.target.dataset.dataset_id().to_string()).bind(num(r.target.expected_generation)?).execute(&mut *tx).await?.rows_affected();
                if n != 1 {
                    return Err(PublicationError::Conflict.into());
                }
            }
            sqlx::query("INSERT INTO events(id,type,payload_json,causation_id,correlation_id,wall_time_us) VALUES(?,'dataset.head_changed',?,?,?,?)").bind(event.to_string()).bind(&payload).bind(r.job.to_string()).bind(r.build.to_string()).bind(now_us).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO head_changes VALUES(?,?,?,?,?,?,?,?)")
                .bind(event.to_string())
                .bind(r.target.branch.to_string())
                .bind(r.target.dataset.dataset_id().to_string())
                .bind(previous)
                .bind(c.version.to_string())
                .bind(generation)
                .bind("cache_reuse")
                .bind(now_us)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO outbox_deliveries(event_id,target,payload_json,schema_version,attempts,next_attempt_at_us) VALUES(?,'publication',?,1,0,?)").bind(event.to_string()).bind(&payload).bind(now_us).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE jobs SET state='CACHED',compute_key=? WHERE id=?")
            .bind(&r.contract.compute_fingerprint)
            .bind(r.job.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO cached_jobs VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(r.job.to_string())
            .bind(c.version.to_string())
            .bind(c.attempt.to_string())
            .bind(generation)
            .bind(event.map(|e| e.to_string()))
            .bind(now_us)
            .bind(num(r.target.expected_generation)?)
            .bind(r.fence.session.to_string())
            .bind(num(r.fence.generation)?)
            .execute(&mut *tx)
            .await?;
        for result in checks {
            sqlx::query("INSERT INTO validation_reuse VALUES(?,?,?,?)")
                .bind(format!("{}:{result}", r.job))
                .bind(r.job.to_string())
                .bind(result)
                .bind(&r.contract.check_fingerprint)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('cache_reuse',?,?)",
        )
        .bind(payload)
        .bind(now_us)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Receipt {
            version: c.version,
            original_attempt: c.attempt,
            generation: generation as u64,
            event,
        })
    }
    /// Bind a child's key to the exact successful/cached parent of this build, never a later head.
    pub async fn completed_parent(
        &mut self,
        build: BuildId,
        dataset: DatasetId,
    ) -> Result<Option<(VersionId, ContentDigest)>> {
        let row=sqlx::query("SELECT v.id,v.artifact_digest FROM jobs j JOIN dataset_versions v ON v.dataset_id=j.dataset_id WHERE j.build_id=? AND j.dataset_id=? AND ((j.state='CACHED' AND EXISTS(SELECT 1 FROM cached_jobs c WHERE c.job_id=j.id AND c.version_id=v.id)) OR (j.state='SUCCEEDED' AND EXISTS(SELECT 1 FROM attempts a WHERE a.job_id=j.id AND a.id=v.attempt_id AND a.state='SUCCEEDED'))) LIMIT 2").bind(build.to_string()).bind(dataset.to_string()).fetch_all(&mut self.db).await?;
        if row.len() > 1 {
            return Err(Error::Evidence);
        }
        row.first()
            .map(|row| {
                Ok((
                    row.try_get::<String, _>(0)?
                        .parse()
                        .map_err(|_| Error::Evidence)?,
                    ContentDigest::from_hex(DigestKind::Artifact, &row.try_get::<String, _>(1)?)
                        .map_err(|_| Error::Evidence)?,
                ))
            })
            .transpose()
    }
}

impl Store {
    /// Read the actual accepted job/reservation context, never a caller-supplied draft clone.
    pub async fn cache_job_context(
        &mut self,
        build: BuildId,
        job: JobId,
    ) -> Result<(crate::planning::DraftPlan, OutputTarget, Fence)> {
        let row=sqlx::query("SELECT b.plan_id,j.dataset_id,j.branch_id,w.expected_head_generation,w.session_id,w.fence FROM jobs j JOIN builds b ON b.id=j.build_id JOIN build_plans p ON p.id=b.plan_id JOIN write_reservations w ON w.build_id=b.id AND w.dataset_id=j.dataset_id AND w.branch_id=j.branch_id WHERE b.id=? AND j.id=? AND p.disposition='ACCEPTED'").bind(build.to_string()).bind(job.to_string()).fetch_optional(&mut self.db).await?.ok_or(Error::Evidence)?;
        let plan = self
            .load_draft(
                row.try_get::<String, _>(0)?
                    .parse()
                    .map_err(|_| Error::Evidence)?,
            )
            .await
            .map_err(|_| Error::Evidence)?;
        let target = OutputTarget {
            dataset: tf_domain::DatasetKey::new(
                plan.workspace.parse().map_err(|_| Error::Evidence)?,
                row.try_get::<String, _>(1)?
                    .parse()
                    .map_err(|_| Error::Evidence)?,
            ),
            branch: row
                .try_get::<String, _>(2)?
                .parse()
                .map_err(|_| Error::Evidence)?,
            expected_generation: u64::try_from(row.try_get::<i64, _>(3)?)
                .map_err(|_| Error::Evidence)?,
        };
        let fence = Fence {
            session: row
                .try_get::<String, _>(4)?
                .parse()
                .map_err(|_| Error::Evidence)?,
            generation: u64::try_from(row.try_get::<i64, _>(5)?).map_err(|_| Error::Evidence)?,
        };
        Ok((plan, target, fence))
    }
    /// Freeze once, or verify exact equality on a cache miss/retry. This cannot rewrite a contract.
    pub async fn freeze_cache_contract(&mut self, r: &Request) -> Result<()> {
        publication::authority(
            &mut self.db,
            r.target.dataset.workspace_id(),
            r.fence.session,
        )
        .await?;
        let old: Option<String> =
            sqlx::query_scalar("SELECT contract_json FROM publication_contracts WHERE job_id=?")
                .bind(r.job.to_string())
                .fetch_optional(&mut self.db)
                .await?;
        if let Some(old) = old {
            if old != r.contract.canonical()? {
                return Err(Error::Evidence);
            }
        } else {
            self.freeze_publication_contract(
                r.target.dataset.workspace_id(),
                r.fence.session,
                r.job,
                &r.contract,
            )
            .await?;
        }
        Ok(())
    }
}
