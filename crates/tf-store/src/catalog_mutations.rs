//! Durable registry intents; no authoring-file I/O or identity allocation occurs here.
use crate::{Result, Store, StoreError};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
use std::collections::BTreeSet;
use tf_domain::{DatasetId, DatasetPath, RequestId, SourceSnapshotId, WorkspaceId};
use tf_protocol::canonical::{ContentDigest, DigestKind};

/// Exact assignments and guards retained before any authoring replacement.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogMutation {
    /// Unique operation identity, never a dataset identity.
    pub id: RequestId,
    /// Local registry owner.
    pub workspace: WorkspaceId,
    /// Source capture that authorized the proposal.
    pub source: SourceSnapshotId,
    /// Exact original registry bytes.
    pub old_digest: ContentDigest,
    /// Exact proposed registry bytes.
    pub new_digest: ContentDigest,
    /// New local paths and their already allocated IDs; empty for explicit lifecycle edits.
    pub assignments: Vec<(DatasetPath, DatasetId)>,
    /// All local registry identities, including historical/tombstoned IDs.
    pub index_ids: Vec<DatasetId>,
}
impl CatalogMutation {
    fn validate(&self) -> Result<()> {
        let ids: BTreeSet<_> = self.index_ids.iter().copied().collect();
        let names: BTreeSet<_> = self.assignments.iter().map(|(p, _)| p).collect();
        let proposed: BTreeSet<_> = self.assignments.iter().map(|(_, id)| id).collect();
        if self.old_digest.kind() != DigestKind::File
            || self.new_digest.kind() != DigestKind::File
            || self.old_digest == self.new_digest
            || self.index_ids.len() > 100_000
            || ids.len() != self.index_ids.len()
            || names.len() != self.assignments.len()
            || proposed.len() != self.assignments.len()
            || self.assignments.iter().any(|(_, id)| !ids.contains(id))
        {
            return Err(StoreError::InvalidRequest);
        }
        Ok(())
    }
}
/// Journal states are evidence, not dataset publication states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationState {
    /// Intent durable, replacement may or may not have occurred.
    Prepared,
    /// Parent directory was synced after replacement.
    Replaced,
    /// Exact IDs and journal completion committed together.
    Indexed,
    /// No recovery write is permitted; evidence remains inspectable.
    Conflict,
}
fn state(s: &str) -> Result<MutationState> {
    match s {
        "PREPARED" => Ok(MutationState::Prepared),
        "REPLACED" => Ok(MutationState::Replaced),
        "INDEXED" => Ok(MutationState::Indexed),
        "CONFLICT" => Ok(MutationState::Conflict),
        _ => Err(StoreError::InvalidRequest),
    }
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().ok_or(StoreError::InvalidRequest)
}
fn parse<T: std::str::FromStr>(s: &str) -> Result<T> {
    s.parse().map_err(|_| StoreError::InvalidRequest)
}
impl Store {
    /// Conservatively refuse lifecycle edits while runtime users/templates may retain names.
    /// Detailed schedule/view reference resolution belongs to their later repositories.
    pub async fn catalog_lifecycle_blockers(&mut self, at_us: i64) -> Result<Vec<String>> {
        lifecycle_blockers(&mut self.db, at_us).await
    }
    /// Persist a complete intent under FULL synchronous SQLite before file replacement.
    /// Only one unfinished intent is permitted; recovery must run before another mutation.
    pub async fn prepare_catalog_mutation(&mut self, m: &CatalogMutation) -> Result<()> {
        m.validate()?;
        let assignments = json!(
            m.assignments
                .iter()
                .map(|(p, id)| json!({"path":p.as_str(),"id":id.to_string()}))
                .collect::<Vec<_>>()
        )
        .to_string();
        let evidence = json!({"format_version":1,"workspace":m.workspace.to_string(),"source":m.source.to_string(),"index_ids":m.index_ids.iter().map(ToString::to_string).collect::<Vec<_>>()}).to_string();
        if assignments.len() + evidence.len() > 16 * 1024 * 1024 {
            return Err(StoreError::InvalidRequest);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog_mutations WHERE state IN ('PREPARED','REPLACED')",
        )
        .fetch_one(&mut *tx)
        .await?;
        if pending != 0 {
            return Err(StoreError::InvalidRequest);
        }
        sqlx::query("INSERT INTO catalog_mutations(id,expected_old_digest,expected_new_digest,proposed_ids_json,state,evidence_json) VALUES(?,?,?,?,'PREPARED',?)")
            .bind(m.id.to_string()).bind(m.old_digest.hex()).bind(m.new_digest.hex()).bind(assignments).bind(evidence).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Read bounded, validated outstanding intents. Corrupt evidence fails closed.
    pub async fn pending_catalog_mutations(&mut self) -> Result<Vec<CatalogMutation>> {
        let rows=sqlx::query("SELECT id,expected_old_digest,expected_new_digest,proposed_ids_json,evidence_json FROM catalog_mutations WHERE state IN ('PREPARED','REPLACED') LIMIT 2").fetch_all(&mut self.db).await?;
        if rows.len() > 1 {
            return Err(StoreError::InvalidRequest);
        }
        rows.into_iter()
            .map(|r| {
                let assignments: String = r.try_get(3)?;
                let evidence: String = r.try_get(4)?;
                if assignments.len() + evidence.len() > 16 * 1024 * 1024 {
                    return Err(StoreError::InvalidRequest);
                }
                let a: Value =
                    serde_json::from_str(&assignments).map_err(|_| StoreError::InvalidRequest)?;
                let e: Value =
                    serde_json::from_str(&evidence).map_err(|_| StoreError::InvalidRequest)?;
                if e["format_version"] != 1 {
                    return Err(StoreError::InvalidRequest);
                }
                let m = CatalogMutation {
                    id: parse(r.try_get(0)?)?,
                    old_digest: ContentDigest::from_hex(DigestKind::File, r.try_get(1)?)
                        .map_err(|_| StoreError::InvalidRequest)?,
                    new_digest: ContentDigest::from_hex(DigestKind::File, r.try_get(2)?)
                        .map_err(|_| StoreError::InvalidRequest)?,
                    workspace: parse(text(&e, "workspace")?)?,
                    source: parse(text(&e, "source")?)?,
                    assignments: a
                        .as_array()
                        .ok_or(StoreError::InvalidRequest)?
                        .iter()
                        .map(|v| Ok((parse(text(v, "path")?)?, parse(text(v, "id")?)?)))
                        .collect::<Result<_>>()?,
                    index_ids: e["index_ids"]
                        .as_array()
                        .ok_or(StoreError::InvalidRequest)?
                        .iter()
                        .map(|v| parse(v.as_str().ok_or(StoreError::InvalidRequest)?))
                        .collect::<Result<_>>()?,
                };
                m.validate()?;
                Ok(m)
            })
            .collect()
    }
    /// Record a synced replacement; PREPARED alone also suffices for crash recovery.
    pub async fn catalog_replaced(&mut self, id: RequestId) -> Result<()> {
        let n = sqlx::query(
            "UPDATE catalog_mutations SET state='REPLACED' WHERE id=? AND state='PREPARED'",
        )
        .bind(id.to_string())
        .execute(&mut self.db)
        .await?
        .rows_affected();
        if n != 1 {
            return Err(StoreError::InvalidRequest);
        }
        Ok(())
    }
    /// Finish using the persisted IDs, atomically with the INDEXED state. Never creates versions.
    pub async fn index_catalog_mutation(&mut self, id: RequestId, at_us: i64) -> Result<()> {
        let m = self
            .pending_catalog_mutations()
            .await?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or(StoreError::InvalidRequest)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        for dataset in &m.index_ids {
            sqlx::query("INSERT INTO datasets(id,workspace_id,created_at_us) VALUES(?,?,?) ON CONFLICT(id) DO NOTHING").bind(dataset.to_string()).bind(m.workspace.to_string()).bind(at_us).execute(&mut *tx).await?;
            let owner: String = sqlx::query_scalar("SELECT workspace_id FROM datasets WHERE id=?")
                .bind(dataset.to_string())
                .fetch_one(&mut *tx)
                .await?;
            if owner != m.workspace.to_string() {
                return Err(StoreError::InvalidRequest);
            }
        }
        let n=sqlx::query("UPDATE catalog_mutations SET state='INDEXED' WHERE id=? AND state IN ('PREPARED','REPLACED')").bind(id.to_string()).execute(&mut *tx).await?.rows_affected();
        if n != 1 {
            return Err(StoreError::InvalidRequest);
        }
        tx.commit().await?;
        Ok(())
    }
    /// Preserve conflicting/abandoned evidence without modifying authoring bytes.
    pub async fn catalog_conflict(&mut self, id: RequestId) -> Result<()> {
        let n=sqlx::query("UPDATE catalog_mutations SET state='CONFLICT' WHERE id=? AND state IN ('PREPARED','REPLACED')").bind(id.to_string()).execute(&mut self.db).await?.rows_affected();
        if n != 1 {
            return Err(StoreError::InvalidRequest);
        }
        Ok(())
    }
    /// Inspect one retained intent's state without importing or opening authoring files.
    pub async fn catalog_mutation_state(&mut self, id: RequestId) -> Result<Option<MutationState>> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT state FROM catalog_mutations WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut self.db)
                .await?;
        value.as_deref().map(state).transpose()
    }
}

impl crate::Reader {
    /// Read-only lifecycle impact preview using the same conservative checks as application.
    pub async fn catalog_lifecycle_blockers(&mut self, at_us: i64) -> Result<Vec<String>> {
        lifecycle_blockers(&mut self.db, at_us).await
    }
}
async fn lifecycle_blockers(db: &mut sqlx::SqliteConnection, at_us: i64) -> Result<Vec<String>> {
    let mut blockers = Vec::new();
    for (label, query) in [
        (
            "active builds",
            "SELECT count(*) FROM builds WHERE state IN ('QUEUED','RUNNING')",
        ),
        (
            "saved schedules",
            "SELECT count(*) FROM schedules WHERE deleted_at_us IS NULL",
        ),
        ("saved graph views", "SELECT count(*) FROM graph_views"),
    ] {
        let count: i64 = sqlx::query_scalar(query).fetch_one(&mut *db).await?;
        if count > 0 {
            blockers.push(format!("{label}: {count}"));
        }
    }
    let leases: i64 = sqlx::query_scalar("SELECT count(*) FROM read_leases WHERE expires_at_us>?")
        .bind(at_us)
        .fetch_one(&mut *db)
        .await?;
    if leases > 0 {
        blockers.push(format!("active read leases: {leases}"));
    }
    Ok(blockers)
}
