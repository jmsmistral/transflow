//! Read-only inventories, observed orphan grace and durable physical deletion intents.
//! Filesystem operations stay outside transactions and require real runtime ownership.
use crate::{
    Reader, Store,
    artifacts::VerifiedArtifact,
    retention::{self, RetentionError, RetentionPolicy, RetentionRoots},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::RequestId;
use tf_protocol::canonical::{ContentDigest, DigestKind, canonical_json};
type Result<T> = std::result::Result<T, RetentionError>;
/// Seven-day grace for digest-verified unknown objects and unrooted source captures.
pub const GRACE_US: i64 = 7 * 24 * 60 * 60 * 1_000_000;
/// Independent retention for completed attempt logs/output directories.
pub const LOGS_US: i64 = 30 * 24 * 60 * 60 * 1_000_000;
/// A bounded metadata snapshot; unknown filesystem entries are inspected by the owner.
pub struct Inventory {
    /// Exact roots, recursively including local retained inputs.
    pub roots: RetentionRoots,
    /// Known physical digest, bytes and storage state.
    pub artifacts: BTreeMap<String, (u64, String, bool)>,
    /// Specific directly observed retention reasons; transitive roots remain labelled separately.
    pub reasons: BTreeMap<String, String>,
    /// Captured source identities known to runtime metadata.
    pub sources: BTreeSet<String>,
    /// Finished attempts eligible for independent log expiry.
    pub logs: BTreeSet<String>,
    /// First-seen timestamps and durable collection status, keyed by kind and identity.
    pub items: BTreeMap<(String, String), Value>,
    /// Legacy claims lacking a physical deletion intent; retain them for explicit reconciliation.
    pub legacy_claims: BTreeSet<String>,
}
async fn inventory(
    db: &mut SqliteConnection,
    now: i64,
    policy: RetentionPolicy,
) -> Result<Inventory> {
    let floor: i64 = sqlx::query_scalar("SELECT now_us FROM retention_clock WHERE singleton=1")
        .fetch_one(&mut *db)
        .await?;
    let unknown: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_deliveries WHERE target!='publication' OR schema_version!=1",
    )
    .fetch_one(&mut *db)
    .await?;
    if now < floor {
        return Err(RetentionError::ClockOrLimit);
    }
    if unknown != 0 {
        return Err(RetentionError::UnknownRoots);
    }
    let roots = retention::roots(db, now, policy).await?;
    let rows = sqlx::query(
        "SELECT digest,byte_count,CASE WHEN EXISTS(SELECT 1 FROM gc_items g WHERE g.kind='artifact' AND g.object_key=artifacts.digest AND g.state='DELETED') THEN 'EXPIRED' ELSE integrity_state END,EXISTS(SELECT 1 FROM dataset_versions v WHERE v.artifact_digest=artifacts.digest) FROM artifacts ORDER BY digest LIMIT 100001",
    )
    .fetch_all(&mut *db)
    .await?;
    let sources: Vec<String> =
        sqlx::query_scalar("SELECT id FROM source_snapshots ORDER BY id LIMIT 100001")
            .fetch_all(&mut *db)
            .await?;
    let logs:Vec<String>=sqlx::query_scalar("SELECT a.id FROM attempts a JOIN jobs j ON j.id=a.job_id JOIN builds b ON b.id=j.build_id WHERE a.finished_at_us IS NOT NULL AND a.finished_at_us<=? AND b.state NOT IN ('QUEUED','RUNNING') ORDER BY a.id LIMIT 100001").bind(now.saturating_sub(LOGS_US)).fetch_all(&mut *db).await?;
    let items=sqlx::query("SELECT kind,object_key,first_seen_us,state,claim_id FROM gc_items ORDER BY kind,object_key LIMIT 100001").fetch_all(&mut *db).await?;
    let legacy:Vec<String>=sqlx::query_scalar("SELECT digest FROM artifact_gc_claims a WHERE NOT EXISTS(SELECT 1 FROM gc_items g WHERE g.kind='artifact' AND g.object_key=a.digest AND g.claim_id=a.id AND g.state='CLAIMED') LIMIT 100001").fetch_all(&mut *db).await?;
    if [
        rows.len(),
        sources.len(),
        logs.len(),
        items.len(),
        legacy.len(),
    ]
    .into_iter()
    .any(|n| n > 100_000)
    {
        return Err(RetentionError::ClockOrLimit);
    }
    let mut artifacts = BTreeMap::new();
    for row in rows {
        let bytes: i64 = row.try_get(1)?;
        let key: String = row.try_get(0)?;
        ContentDigest::from_hex(DigestKind::Artifact, &key)
            .map_err(|_| RetentionError::Unavailable)?;
        artifacts.insert(
            key,
            (
                bytes.try_into().map_err(|_| RetentionError::Unavailable)?,
                row.try_get(2)?,
                row.try_get(3)?,
            ),
        );
    }
    let mut observed = BTreeMap::new();
    for row in items {
        let kind: String = row.try_get(0)?;
        let key: String = row.try_get(1)?;
        let state: String = row.try_get(3)?;
        let claim: Option<String> = row.try_get(4)?;
        if state == "CLAIMED"
            && claim
                .as_ref()
                .is_none_or(|id| id.parse::<RequestId>().is_err())
        {
            return Err(RetentionError::UnknownRoots);
        }
        observed.insert((kind,key),json!({"first_seen_us":row.try_get::<i64,_>(2)?.to_string(),"state":row.try_get::<String,_>(3)?,"claim_id":row.try_get::<Option<String>,_>(4)?}));
    }
    let reasons=sqlx::query(r#"WITH ranked AS (SELECT new_version_id,row_number() OVER(PARTITION BY dataset_id,branch_id ORDER BY generation DESC) AS n FROM head_changes), reasons(digest,reason) AS (
 SELECT v.artifact_digest,'live branch head' FROM dataset_heads h JOIN data_branches b ON b.id=h.branch_id JOIN dataset_versions v ON v.id=h.version_id WHERE b.deleted_at_us IS NULL
 UNION SELECT artifact_digest,'within publication age retention' FROM dataset_versions WHERE published_at_us>=?
 UNION SELECT v.artifact_digest,'latest per-branch publication window' FROM ranked r JOIN dataset_versions v ON v.id=r.new_version_id WHERE r.n<=?
 UNION SELECT coalesce(l.artifact_digest,v.artifact_digest),'active read lease' FROM read_leases l LEFT JOIN dataset_versions v ON v.id=l.version_id WHERE l.released=0 AND l.expires_at_us>?
 UNION SELECT coalesce(p.artifact_digest,v.artifact_digest),'explicit history/plan/schedule/backup pin' FROM pins p LEFT JOIN dataset_versions v ON v.id=p.version_id WHERE (p.expires_at_us IS NULL OR p.expires_at_us>?) AND (p.artifact_digest IS NOT NULL OR p.version_id IS NOT NULL)
 UNION SELECT f.artifact_digest,'failed-candidate quarantine or incomplete terminal evidence' FROM failed_check_candidates f JOIN attempts a ON a.id=f.attempt_id WHERE a.finished_at_us IS NULL OR a.finished_at_us>?
 ) SELECT digest,group_concat(reason,'; ') FROM reasons WHERE digest IS NOT NULL GROUP BY digest ORDER BY digest LIMIT 100001"#).bind(now.checked_sub(policy.age_us).ok_or(RetentionError::ClockOrLimit)?).bind(i64::from(policy.latest)).bind(now).bind(now).bind(now.saturating_sub(GRACE_US)).fetch_all(&mut *db).await?;
    if reasons.len() > 100_000 {
        return Err(RetentionError::ClockOrLimit);
    }
    let reasons = reasons
        .into_iter()
        .map(|row| Ok((row.try_get(0)?, row.try_get(1)?)))
        .collect::<std::result::Result<BTreeMap<_, _>, sqlx::Error>>()?;
    Ok(Inventory {
        roots,
        artifacts,
        reasons,
        sources: sources.into_iter().collect(),
        logs: logs.into_iter().collect(),
        items: observed,
        legacy_claims: legacy.into_iter().collect(),
    })
}
impl Reader {
    /// Consistent metadata-only dry run. No durable clock, observations or migrations are written.
    pub async fn gc_inventory(&mut self, now: i64, policy: RetentionPolicy) -> Result<Inventory> {
        let mut tx = self.db.begin().await?;
        let result = inventory(&mut tx, now, policy).await?;
        tx.commit().await?;
        Ok(result)
    }
}
impl Store {
    /// Apply-side inventory under the caller's real runtime writer fence.
    pub async fn gc_inventory(&mut self, now: i64, policy: RetentionPolicy) -> Result<Inventory> {
        retention::clock(&mut self.db, now).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let result = inventory(&mut tx, now, policy).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Record a verified orphan's complete metadata without publication or dataset identity.
    pub async fn observe_orphan(&mut self, artifact: &VerifiedArtifact, now: i64) -> Result<()> {
        if self.path.parent() != Some(artifact.workspace().join(".transflow/runtime").as_path()) {
            return Err(RetentionError::Unavailable);
        }
        let manifest = artifact.manifest();
        let files = manifest["files"]
            .as_array()
            .ok_or(RetentionError::Unavailable)?;
        // Canonical encoding errors are reported without manifest contents.
        let manifest_text =
            String::from_utf8(canonical_json(manifest).map_err(|_| RetentionError::Unavailable)?)
                .map_err(|_| RetentionError::Unavailable)?;
        let schema = String::from_utf8(
            canonical_json(&manifest["logical_schema"]).map_err(|_| RetentionError::Unavailable)?,
        )
        .map_err(|_| RetentionError::Unavailable)?;
        let sum = |name: &str| {
            files
                .iter()
                .try_fold(0i64, |n, f| n.checked_add(f[name].as_str()?.parse().ok()?))
        };
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        retention::clock(&mut tx, now).await?;
        sqlx::query(
            "INSERT INTO artifacts VALUES(?,?,?,?,?,?,'VERIFIED') ON CONFLICT(digest) DO NOTHING",
        )
        .bind(artifact.digest().hex())
        .bind(&manifest_text)
        .bind(schema)
        .bind(i64::try_from(files.len()).map_err(|_| RetentionError::Unavailable)?)
        .bind(sum("row_count").ok_or(RetentionError::Unavailable)?)
        .bind(sum("byte_length").ok_or(RetentionError::Unavailable)?)
        .execute(&mut *tx)
        .await?;
        let stored: String =
            sqlx::query_scalar("SELECT manifest_json FROM artifacts WHERE digest=?")
                .bind(artifact.digest().hex())
                .fetch_one(&mut *tx)
                .await?;
        if stored != manifest_text {
            return Err(RetentionError::Unavailable);
        }
        sqlx::query("INSERT INTO gc_items(kind,object_key,first_seen_us,state) VALUES('artifact',?,?,'OBSERVED') ON CONFLICT(kind,object_key) DO UPDATE SET first_seen_us=excluded.first_seen_us,state='OBSERVED',claim_id=NULL,claimed_at_us=NULL,deleted_at_us=NULL WHERE gc_items.state='DELETED'").bind(artifact.digest().hex()).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Record a strictly verified source capture; first observation is not deletion permission.
    pub async fn observe_source(&mut self, key: &str, now: i64) -> Result<()> {
        key.parse::<tf_domain::SourceSnapshotId>()
            .map_err(|_| RetentionError::Unavailable)?;
        retention::clock(&mut self.db, now).await?;
        sqlx::query("INSERT INTO gc_items(kind,object_key,first_seen_us,state) VALUES('source',?,?,'OBSERVED') ON CONFLICT DO NOTHING").bind(key).bind(now).execute(&mut self.db).await?;
        Ok(())
    }
    /// Atomically recheck every root and persist the exact deletion intent before bytes move.
    pub async fn begin_gc(
        &mut self,
        kind: &str,
        key: &str,
        id: RequestId,
        now: i64,
        policy: RetentionPolicy,
    ) -> Result<()> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        retention::clock(&mut tx, now).await?;
        if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM outbox_deliveries WHERE target!='publication' OR schema_version!=1")
            .fetch_one(&mut *tx)
            .await?
            != 0
        {
            return Err(RetentionError::UnknownRoots);
        }
        let roots = retention::roots(&mut tx, now, policy).await?;
        let observed: Option<(i64, String)> = sqlx::query_as(
            "SELECT first_seen_us,state FROM gc_items WHERE kind=? AND object_key=?",
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        if observed
            .as_ref()
            .is_some_and(|(_, state)| state == "CLAIMED")
        {
            return Err(RetentionError::Unavailable);
        }
        if kind == "artifact" {
            let digest = ContentDigest::from_hex(DigestKind::Artifact, key)
                .map_err(|_| RetentionError::Unavailable)?;
            retention::available(&mut tx, digest).await?;
            if roots.artifacts.contains(key) {
                return Err(RetentionError::Rooted);
            }
            let published: i64 =
                sqlx::query_scalar("SELECT count(*) FROM dataset_versions WHERE artifact_digest=?")
                    .bind(key)
                    .fetch_one(&mut *tx)
                    .await?;
            if published == 0
                && observed
                    .as_ref()
                    .is_none_or(|(at, _)| now.saturating_sub(*at) < GRACE_US)
            {
                return Err(RetentionError::Rooted);
            }
            if observed
                .as_ref()
                .is_some_and(|(at, _)| now.saturating_sub(*at) < GRACE_US)
            {
                return Err(RetentionError::Rooted);
            }
            sqlx::query("INSERT INTO artifact_gc_claims VALUES(?,?,?)")
                .bind(key)
                .bind(id.to_string())
                .bind(now)
                .execute(&mut *tx)
                .await?;
        } else if kind == "source" {
            let source = key.parse().map_err(|_| RetentionError::Unavailable)?;
            if roots.sources.contains(&source)
                || observed
                    .as_ref()
                    .is_none_or(|(at, _)| now.saturating_sub(*at) < GRACE_US)
            {
                return Err(RetentionError::Rooted);
            }
        } else if kind == "logs" {
            let eligible:i64=sqlx::query_scalar("SELECT count(*) FROM attempts a JOIN jobs j ON j.id=a.job_id JOIN builds b ON b.id=j.build_id WHERE a.id=? AND a.finished_at_us IS NOT NULL AND a.finished_at_us<=? AND b.state NOT IN ('QUEUED','RUNNING')").bind(key).bind(now.saturating_sub(LOGS_US)).fetch_one(&mut *tx).await?;
            if eligible != 1 {
                return Err(RetentionError::Rooted);
            }
        } else {
            return Err(RetentionError::Unavailable);
        }
        sqlx::query("INSERT INTO gc_items(kind,object_key,first_seen_us,state,claim_id,claimed_at_us) VALUES(?,?,?,'CLAIMED',?,?) ON CONFLICT(kind,object_key) DO UPDATE SET state='CLAIMED',claim_id=excluded.claim_id,claimed_at_us=excluded.claimed_at_us,deleted_at_us=NULL").bind(kind).bind(key).bind(now).bind(id.to_string()).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Commit missing-byte evidence and release only the matching active artifact fence.
    pub async fn finish_gc(
        &mut self,
        kind: &str,
        key: &str,
        id: RequestId,
        now: i64,
    ) -> Result<()> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        retention::clock(&mut tx, now).await?;
        let changed=sqlx::query("UPDATE gc_items SET state='DELETED',deleted_at_us=? WHERE kind=? AND object_key=? AND claim_id=? AND state='CLAIMED'").bind(now).bind(kind).bind(key).bind(id.to_string()).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(RetentionError::Unavailable);
        }
        if kind == "artifact" {
            let changed = sqlx::query("DELETE FROM artifact_gc_claims WHERE digest=? AND id=?")
                .bind(key)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if changed != 1 {
                return Err(RetentionError::Unavailable);
            }
        }
        sqlx::query("INSERT INTO gc_deletions VALUES(?,?,?,?)")
            .bind(id.to_string())
            .bind(kind)
            .bind(key)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
/// Availability changes only after the caller presents a newly verified durable object.
/// Immutable artifact/version evidence and prior collection receipts remain untouched.
pub(crate) async fn restored(
    db: &mut SqliteConnection,
    digest: String,
    at: i64,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query("UPDATE gc_items SET state='OBSERVED',first_seen_us=max(first_seen_us,coalesce(deleted_at_us,0),?),claim_id=NULL,claimed_at_us=NULL,deleted_at_us=NULL WHERE kind='artifact' AND object_key=? AND state='DELETED'").bind(at).bind(digest).execute(db).await?;
    Ok(())
}
