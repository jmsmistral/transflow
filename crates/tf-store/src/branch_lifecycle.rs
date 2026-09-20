//! Explicit data-branch lifecycle. No Git commands, authoring edits or artifact deletion.
use crate::{Reader, Store, StoreError};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::BTreeMap;
use tf_domain::{BranchId, BranchName, SourceSnapshotId, WorkspaceId, input::BranchPolicySnapshot};
/// Lifecycle conflicts are distinct from SQLite failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Repository failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Name/revision/identity collision or tombstone.
    #[error("The branch is absent, deleted, already named, or changed; inspect branches and retry")]
    Conflict,
    /// Active references prevent the change.
    #[error("The branch still has active users or saved references; update them before retrying")]
    Referenced(Vec<String>),
    /// Invalid or excessive metadata.
    #[error("Branch metadata is invalid or exceeds its supported limit")]
    Metadata,
}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Store(e.into())
    }
}
/// Branch row with stable identity and metadata-only head count.
#[derive(Clone, Debug)]
pub struct BranchRecord {
    /// Stable historical identity.
    pub id: BranchId,
    /// Exact name, independent of Git.
    pub name: BranchName,
    /// Optimistic lifecycle revision.
    pub revision: i64,
    /// Tombstone, never an implicit restore.
    pub deleted: bool,
    /// Number of retained head pointers, even on a tombstone.
    pub heads: i64,
}
async fn rows(
    db: &mut SqliteConnection,
    w: WorkspaceId,
    after: Option<BranchId>,
    limit: u16,
) -> Result<Vec<BranchRecord>, Error> {
    if !(1..=101).contains(&limit) {
        return Err(Error::Metadata);
    }
    let rows=sqlx::query("SELECT b.id,b.name,b.revision,b.deleted_at_us,(SELECT count(*) FROM dataset_heads h WHERE h.branch_id=b.id) FROM data_branches b WHERE b.workspace_id=? AND b.id>? ORDER BY b.id LIMIT ?")
        .bind(w.to_string()).bind(after.map(|v|v.to_string()).unwrap_or_default()).bind(i64::from(limit)).fetch_all(db).await?;
    rows.into_iter()
        .map(|r| {
            Ok(BranchRecord {
                id: r
                    .try_get::<String, _>(0)?
                    .parse()
                    .map_err(|_| Error::Metadata)?,
                name: r
                    .try_get::<String, _>(1)?
                    .parse()
                    .map_err(|_| Error::Metadata)?,
                revision: r.try_get(2)?,
                deleted: r.try_get::<Option<i64>, _>(3)?.is_some(),
                heads: r.try_get(4)?,
            })
        })
        .collect()
}
async fn find(
    db: &mut SqliteConnection,
    w: WorkspaceId,
    name: &BranchName,
) -> Result<Option<BranchRecord>, Error> {
    let r=sqlx::query("SELECT id,revision,deleted_at_us,(SELECT count(*) FROM dataset_heads h WHERE h.branch_id=b.id) FROM data_branches b WHERE workspace_id=? AND name=?").bind(w.to_string()).bind(name.as_str()).fetch_optional(db).await?;
    r.map(|r| {
        Ok(BranchRecord {
            id: r
                .try_get::<String, _>(0)?
                .parse()
                .map_err(|_| Error::Metadata)?,
            name: name.clone(),
            revision: r.try_get(1)?,
            deleted: r.try_get::<Option<i64>, _>(2)?.is_some(),
            heads: r.try_get(3)?,
        })
    })
    .transpose()
}
async fn references(
    db: &mut SqliteConnection,
    branch: &BranchRecord,
    now: i64,
) -> Result<Vec<String>, Error> {
    // Generic retained JSON formats are still evolving. Exact scalar matching is conservative:
    // a same-named metadata value can block, but no saved reference is silently rewritten.
    let entries:Vec<String>=sqlx::query_scalar(r#"
        SELECT 'active build: ' || b.id FROM builds b JOIN build_plans p ON p.id=b.plan_id
        WHERE b.state IN ('QUEUED','RUNNING') AND (
          EXISTS(SELECT 1 FROM jobs j WHERE j.build_id=b.id AND j.branch_id=?1)
          OR EXISTS(SELECT 1 FROM json_tree(p.context_json) WHERE atom IN (?1,?2))
          OR EXISTS(SELECT 1 FROM json_tree(p.bindings_json) WHERE atom IN (?1,?2))
          OR EXISTS(SELECT 1 FROM jobs j JOIN publication_contracts c ON c.job_id=j.id, json_tree(c.contract_json) t WHERE j.build_id=b.id AND t.atom IN (?1,?2))
          OR EXISTS(SELECT 1 FROM jobs j JOIN job_inputs i ON i.job_id=j.id, json_tree(i.binding_json) t WHERE j.build_id=b.id AND t.atom IN (?1,?2)))
        UNION SELECT 'write reservation: ' || build_id FROM write_reservations WHERE branch_id=?1
        UNION SELECT 'saved schedule: ' || s.id FROM schedules s JOIN schedule_revisions r
          ON r.schedule_id=s.id AND r.revision=s.active_revision WHERE s.deleted_at_us IS NULL AND (
          EXISTS(SELECT 1 FROM json_tree(r.build_template_json) WHERE atom IN (?1,?2))
          OR EXISTS(SELECT 1 FROM json_tree(r.policies_json) WHERE atom IN (?1,?2)))
        UNION SELECT 'saved view: ' || v.id FROM graph_views v WHERE
          EXISTS(SELECT 1 FROM json_tree(v.context_json) WHERE atom IN (?1,?2))
        UNION SELECT 'live read lease: ' || l.id FROM read_leases l WHERE l.released=0 AND l.expires_at_us>?3 AND (
          EXISTS(SELECT 1 FROM dataset_heads h JOIN dataset_versions v ON v.id=h.version_id
            WHERE h.branch_id=?1 AND (l.version_id=v.id OR l.artifact_digest=v.artifact_digest))
          OR EXISTS(SELECT 1 FROM dataset_versions v JOIN attempts a ON a.id=v.attempt_id JOIN jobs j ON j.id=a.job_id
            WHERE j.branch_id=?1 AND l.version_id=v.id))
        ORDER BY 1 LIMIT 1001
    "#).bind(branch.id.to_string()).bind(branch.name.as_str()).bind(now).fetch_all(db).await?;
    if entries.len() > 1000 {
        return Err(Error::Metadata);
    }
    Ok(entries)
}
impl Reader {
    /// ID-keyset page; fetch one extra row to expose continuation explicitly.
    pub async fn branches(
        &mut self,
        w: WorkspaceId,
        after: Option<BranchId>,
        limit: u16,
    ) -> Result<Vec<BranchRecord>, Error> {
        rows(&mut self.db, w, after, limit).await
    }
    /// Exact metadata lookup; tombstones are returned as such.
    pub async fn branch(
        &mut self,
        w: WorkspaceId,
        n: &BranchName,
    ) -> Result<Option<BranchRecord>, Error> {
        find(&mut self.db, w, n).await
    }
    /// Bounded impact report; no branch or runtime is created by this read.
    pub async fn branch_references(
        &mut self,
        b: &BranchRecord,
        now: i64,
    ) -> Result<Vec<String>, Error> {
        references(&mut self.db, b, now).await
    }
}
/// Explicit lifecycle intent; rename preserves all historical foreign keys.
pub enum BranchChange {
    /// Assign a new metadata name, leaving Git and authored policies alone.
    Rename(BranchName),
    /// Tombstone only; retained heads/artifacts/provenance stay intact.
    Delete,
}
impl Store {
    /// Explicit manual create, idempotent only for an already live name; never copies heads.
    pub async fn create_branch(
        &mut self,
        w: WorkspaceId,
        id: BranchId,
        name: &BranchName,
        now: i64,
    ) -> Result<BranchRecord, Error> {
        if name.as_str().len() > 4096 || now < 0 {
            return Err(Error::Metadata);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row) = find(&mut tx, w, name).await? {
            if row.deleted {
                return Err(Error::Conflict);
            }
            tx.commit().await?;
            return Ok(row);
        }
        sqlx::query("INSERT INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,?,0)")
            .bind(id.to_string())
            .bind(w.to_string())
            .bind(name.as_str())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('branch.create',?,?)")
            .bind(json!({"branch_id":id.to_string(),"name":name.as_str(),"manual":true}).to_string()).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(BranchRecord {
            id,
            name: name.clone(),
            revision: 0,
            deleted: false,
            heads: 0,
        })
    }
    /// Recheck optimistic identity and references inside the same write transaction as the audit.
    /// The composition caller additionally checks current authored policy references.
    pub async fn change_branch(
        &mut self,
        w: WorkspaceId,
        expected: &BranchRecord,
        change: BranchChange,
        now: i64,
    ) -> Result<(), Error> {
        if now < 0 {
            return Err(Error::Metadata);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let current = find(&mut tx, w, &expected.name)
            .await?
            .ok_or(Error::Conflict)?;
        if current.deleted || current.id != expected.id || current.revision != expected.revision {
            return Err(Error::Conflict);
        }
        let refs = references(&mut tx, &current, now).await?;
        if !refs.is_empty() {
            return Err(Error::Referenced(refs));
        }
        let next = current.revision.checked_add(1).ok_or(Error::Metadata)?;
        let (operation, evidence) = match change {
            BranchChange::Rename(name) => {
                if name.as_str().len() > 4096 || find(&mut tx, w, &name).await?.is_some() {
                    return Err(Error::Conflict);
                }
                sqlx::query("UPDATE data_branches SET name=?,revision=? WHERE id=?")
                    .bind(name.as_str())
                    .bind(next)
                    .bind(current.id.to_string())
                    .execute(&mut *tx)
                    .await?;
                (
                    "branch.rename",
                    json!({"branch_id":current.id.to_string(),"old_name":current.name.as_str(),"new_name":name.as_str()}),
                )
            }
            BranchChange::Delete => {
                sqlx::query("UPDATE data_branches SET deleted_at_us=?,revision=? WHERE id=?")
                    .bind(now)
                    .bind(next)
                    .bind(current.id.to_string())
                    .execute(&mut *tx)
                    .await?;
                (
                    "branch.delete",
                    json!({"branch_id":current.id.to_string(),"name":current.name.as_str()}),
                )
            }
        };
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES(?,?,?)")
            .bind(operation)
            .bind(evidence.to_string())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Index an immutable authored policy under its actual source identity, never mutable defaults.
    pub async fn snapshot_branch_policies(
        &mut self,
        source: SourceSnapshotId,
        p: &BranchPolicySnapshot,
    ) -> Result<(), Error> {
        let mut entries = BTreeMap::from([(
            String::new(),
            json!(
                p.defaults()
                    .iter()
                    .map(BranchName::as_str)
                    .collect::<Vec<_>>()
            )
            .to_string(),
        )]);
        for (name, tail) in p.rules() {
            entries.insert(
                name.to_string(),
                json!(tail.iter().map(BranchName::as_str).collect::<Vec<_>>()).to_string(),
            );
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let old:Vec<(String,String)>=sqlx::query_as("SELECT starting_branch,policy_json FROM branch_policies WHERE source_snapshot_id=? ORDER BY starting_branch LIMIT 1026").bind(source.to_string()).fetch_all(&mut *tx).await?;
        if !old.is_empty() {
            if old.into_iter().collect::<BTreeMap<_, _>>() != entries {
                return Err(Error::Conflict);
            }
            tx.commit().await?;
            return Ok(());
        }
        for (name, value) in entries {
            sqlx::query("INSERT INTO branch_policies VALUES(?,?,?)")
                .bind(source.to_string())
                .bind(name)
                .bind(value)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
impl Reader {
    /// Rehydrate the exact source-indexed authored policy; missing snapshots are an error.
    pub async fn branch_policy_snapshot(
        &mut self,
        source: SourceSnapshotId,
    ) -> Result<BranchPolicySnapshot, Error> {
        let rows:Vec<(String,String)>=sqlx::query_as("SELECT starting_branch,policy_json FROM branch_policies WHERE source_snapshot_id=? LIMIT 1026").bind(source.to_string()).fetch_all(&mut self.db).await?;
        if rows.is_empty() || rows.len() > 1025 {
            return Err(Error::Metadata);
        }
        let mut defaults = None;
        let mut rules = BTreeMap::new();
        for (name, text) in rows {
            let value: Value = serde_json::from_str(&text).map_err(|_| Error::Metadata)?;
            let tail = value
                .as_array()
                .filter(|a| a.len() <= 1024)
                .ok_or(Error::Metadata)?
                .iter()
                .map(|n| {
                    n.as_str()
                        .ok_or(Error::Metadata)?
                        .parse()
                        .map_err(|_| Error::Metadata)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if name.is_empty() {
                defaults = Some(tail);
            } else {
                rules.insert(name.parse().map_err(|_| Error::Metadata)?, tail);
            }
        }
        BranchPolicySnapshot::new(defaults.ok_or(Error::Metadata)?, rules)
            .map_err(|_| Error::Metadata)
    }
}
