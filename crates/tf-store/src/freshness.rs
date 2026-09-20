//! One SQLite read transaction freezes heads, original certificates and latest attempts.
use crate::{Reader, Result, Store, StoreError, cache};
use serde_json::Value;
use sqlx::{Connection, Row};
use std::collections::BTreeMap;
use tf_domain::{BranchName, DatasetId, VersionId, WorkspaceId};

/// Retained original check result; no synthetic cache timestamps.
#[derive(Clone, Debug)]
pub struct Check {
    /// Exact immutable definition identity.
    pub definition: String,
    /// Output or exact consumer input subject.
    pub subject: Value,
    /// PASS, VIOLATION, ERROR or SKIPPED.
    pub outcome: String,
    /// Original completion time, absent while unavailable.
    pub finished_us: Option<i64>,
}
/// Latest actual attempt on one dataset/branch, not necessarily the head's attempt.
#[derive(Clone, Debug)]
pub struct Attempt {
    /// Durable attempt identity.
    pub id: String,
    /// Persisted execution state.
    pub state: String,
    /// Frozen check obligations for this attempt.
    pub contract: Option<Value>,
    /// Results belong only to this attempt.
    pub checks: Vec<Check>,
}
/// Metadata about a retained head; physical availability requires separate verification.
#[derive(Clone, Debug)]
pub struct Head {
    /// Exact immutable publication identity.
    pub version: VersionId,
    /// Physical content identity.
    pub artifact: String,
    /// Original publication clock, never adoption time.
    pub published_us: i64,
    /// Compare only when present and supported.
    pub evidence: Option<Value>,
    /// Original complete check obligation contract, absent for legacy/imported versions.
    pub contract: Option<Value>,
    /// Only checks linked to this publication, never a failed later attempt.
    pub checks: Vec<Check>,
}
/// All rows belong to one transaction snapshot and the named workspace.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Explicit workspace prevents accidental cross-workspace comparisons.
    pub workspace: WorkspaceId,
    /// Highest event at snapshot acquisition, for contextual read-model diagnostics.
    pub event_sequence: i64,
    /// Nondeleted branch heads, including potential fallback candidates.
    pub heads: BTreeMap<(DatasetId, BranchName), Head>,
    /// Latest real attempts, independently retained even if there is no head.
    pub attempts: BTreeMap<(DatasetId, BranchName), Attempt>,
}
fn decode(value: Option<String>) -> Result<Option<Value>> {
    value
        .map(|s| serde_json::from_str(&s).map_err(|_| StoreError::InvalidRequest))
        .transpose()
}
fn check(row: &sqlx::sqlite::SqliteRow) -> Result<Check> {
    Ok(Check {
        definition: row.try_get("definition_fingerprint")?,
        subject: serde_json::from_str(&row.try_get::<String, _>("subject_json")?)
            .map_err(|_| StoreError::InvalidRequest)?,
        outcome: row.try_get("outcome")?,
        finished_us: row.try_get("finished_at_us")?,
    })
}
impl Store {
    /// Freeze comparison evidence before execution using the same authority and contract as cache.
    /// Retrying identical evidence is harmless; rewriting or retrofitting a completed attempt fails.
    pub async fn freeze_computation_evidence(
        &mut self,
        request: &cache::Request,
        evidence: &Value,
    ) -> std::result::Result<(), cache::Error> {
        self.freeze_cache_contract(request).await?;
        let encoded =
            tf_protocol::canonical::canonical_json(evidence).map_err(|_| cache::Error::Evidence)?;
        if encoded.len() > 1024 * 1024 {
            return Err(cache::Error::Evidence);
        }
        let encoded = String::from_utf8(encoded).map_err(|_| cache::Error::Evidence)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let old: Option<String> =
            sqlx::query_scalar("SELECT evidence_json FROM computation_evidence WHERE job_id=?")
                .bind(request.job.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(old) = old {
            if old != encoded {
                return Err(cache::Error::Evidence);
            }
        } else {
            sqlx::query("INSERT INTO computation_evidence VALUES(?,?)")
                .bind(request.job.to_string())
                .bind(encoded)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
impl Reader {
    /// Capture metadata in one bounded read transaction. No filesystem scans, Python imports,
    /// or caller callbacks run while the snapshot is open. Limits fail explicitly, never truncate.
    pub async fn freshness_snapshot(&mut self, workspace: WorkspaceId) -> Result<Snapshot> {
        let mut tx = self.db.begin().await?;
        let owner: Option<String> = sqlx::query_scalar("SELECT id FROM workspaces LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;
        if owner
            .as_ref()
            .is_some_and(|id| id != &workspace.to_string())
        {
            return Err(StoreError::InvalidRequest);
        }
        let event_sequence: i64 =
            sqlx::query_scalar("SELECT coalesce(max(sequence),0) FROM events")
                .fetch_one(&mut *tx)
                .await?;
        let rows = sqlx::query("SELECT h.dataset_id,b.name,v.id,v.artifact_digest,v.published_at_us,e.evidence_json,c.contract_json FROM dataset_heads h JOIN data_branches b ON b.id=h.branch_id JOIN datasets d ON d.id=h.dataset_id JOIN dataset_versions v ON v.id=h.version_id LEFT JOIN attempts a ON a.id=v.attempt_id LEFT JOIN computation_evidence e ON e.job_id=a.job_id LEFT JOIN publication_contracts c ON c.job_id=a.job_id WHERE d.workspace_id=? AND b.workspace_id=d.workspace_id AND b.deleted_at_us IS NULL AND d.tombstoned_at_us IS NULL ORDER BY h.dataset_id,b.name LIMIT 100001")
            .bind(workspace.to_string()).fetch_all(&mut *tx).await?;
        if rows.len() > 100_000 {
            return Err(StoreError::InvalidRequest);
        }
        let mut heads = BTreeMap::new();
        let mut checks_read = 0;
        for row in rows {
            let version: String = row.try_get("id")?;
            let checks = sqlx::query("SELECT r.definition_fingerprint,r.subject_json,r.outcome,r.finished_at_us FROM version_check_results v JOIN check_results r ON r.id=v.result_id WHERE v.version_id=? ORDER BY r.id LIMIT 10001").bind(&version).fetch_all(&mut *tx).await?;
            checks_read += checks.len();
            if checks.len() > 10_000 || checks_read > 100_000 {
                return Err(StoreError::InvalidRequest);
            }
            heads.insert(
                (
                    row.try_get::<String, _>("dataset_id")?
                        .parse()
                        .map_err(|_| StoreError::InvalidRequest)?,
                    row.try_get::<String, _>("name")?
                        .parse()
                        .map_err(|_| StoreError::InvalidRequest)?,
                ),
                Head {
                    version: version.parse().map_err(|_| StoreError::InvalidRequest)?,
                    artifact: row.try_get("artifact_digest")?,
                    published_us: row.try_get("published_at_us")?,
                    evidence: decode(row.try_get("evidence_json")?)?,
                    contract: decode(row.try_get("contract_json")?)?,
                    checks: checks.iter().map(check).collect::<Result<_>>()?,
                },
            );
        }
        let rows = sqlx::query("WITH ranked AS (SELECT j.dataset_id,b.name,a.id,a.state,c.contract_json,row_number() OVER(PARTITION BY j.dataset_id,b.id ORDER BY a.started_at_us DESC,build.created_at_us DESC,a.attempt_no DESC,a.id DESC) AS rank FROM attempts a JOIN jobs j ON j.id=a.job_id JOIN builds build ON build.id=j.build_id LEFT JOIN publication_contracts c ON c.job_id=j.id JOIN datasets d ON d.id=j.dataset_id JOIN data_branches b ON b.id=j.branch_id WHERE d.workspace_id=? AND b.workspace_id=d.workspace_id AND b.deleted_at_us IS NULL AND d.tombstoned_at_us IS NULL) SELECT * FROM ranked WHERE rank=1 LIMIT 100001").bind(workspace.to_string()).fetch_all(&mut *tx).await?;
        if rows.len() > 100_000 {
            return Err(StoreError::InvalidRequest);
        }
        let mut attempts = BTreeMap::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let checks = sqlx::query("SELECT definition_fingerprint,subject_json,outcome,finished_at_us FROM check_results WHERE attempt_id=? ORDER BY id LIMIT 10001").bind(&id).fetch_all(&mut *tx).await?;
            checks_read += checks.len();
            if checks.len() > 10_000 || checks_read > 200_000 {
                return Err(StoreError::InvalidRequest);
            }
            attempts.insert(
                (
                    row.try_get::<String, _>("dataset_id")?
                        .parse()
                        .map_err(|_| StoreError::InvalidRequest)?,
                    row.try_get::<String, _>("name")?
                        .parse()
                        .map_err(|_| StoreError::InvalidRequest)?,
                ),
                Attempt {
                    id,
                    state: row.try_get("state")?,
                    contract: decode(row.try_get("contract_json")?)?,
                    checks: checks.iter().map(check).collect::<Result<_>>()?,
                },
            );
        }
        tx.commit().await?;
        Ok(Snapshot {
            workspace,
            event_sequence,
            heads,
            attempts,
        })
    }
}
