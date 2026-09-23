//! Serialized SQLite repositories. The coordinator owns one Store; no transaction escapes.
pub mod catalog_mutations;
mod migrations;
mod path;
use sqlx::{
    Connection, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tf_domain::{DatasetId, WorkspaceId};

/// Latest supported runtime schema. Authoring registry versions are independent.
pub const SCHEMA_VERSION: i64 = 8;

/// Branch-scoped retained version reuse and guarded adoption.
pub mod cache;
/// Default maximum wait for an externally held SQLite writer lock.
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(250);
/// Storage failure with safe summary and retained technical source.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite rejected an operation; source is for controlled diagnostics only.
    #[error("The runtime database operation failed")]
    Database(#[from] sqlx::Error),
    /// The path could not be validated or opened.
    #[error("The runtime storage path is unavailable")]
    Io(#[from] std::io::Error),
    /// Filesystem is outside the qualified local set.
    #[error("Runtime storage requires a supported local filesystem")]
    Filesystem,
    /// SQLite has an unqualified WAL implementation.
    #[error("The linked SQLite library lacks the required WAL fix")]
    SqliteVersion,
    /// Never downgrade or silently overwrite incompatible state.
    #[error("The runtime database schema is newer than this application")]
    NewerSchema,
    /// Database identity, version sequence or checksums disagree.
    #[error("Runtime schema identity or migration evidence is inconsistent")]
    Migration,
    /// Bounded data or repository preconditions failed.
    #[error("The runtime repository request is invalid or exceeds its limit")]
    InvalidRequest,
}
/// Repository result.
pub type Result<T> = std::result::Result<T, StoreError>;
/// Actual connection diagnostics, suitable for a future doctor command.
#[derive(Clone, Debug)]
pub struct StorageInfo {
    /// Library selected by the linked SQLx SQLite driver.
    pub sqlite_version: String,
    /// Exact SQLite source identity.
    pub sqlite_source_id: String,
    /// Verified foreign-key enforcement.
    pub foreign_keys: bool,
    /// Verified SQLite synchronous level (2 means FULL).
    pub synchronous: i64,
    /// Verified connection busy timeout in milliseconds.
    pub busy_timeout_ms: i64,
    /// Installed schema version.
    pub schema_version: i64,
}
/// One non-cloneable writer, owned by the coordinator's serialized mutation loop.
/// `&mut self` and private connections prevent overlapping mutation transactions.
/// No user callbacks, process waits or artifact scans can run inside its transactions.
pub struct Store {
    db: SqliteConnection,
    path: PathBuf,
    info: StorageInfo,
}
impl Store {
    /// Open/create a database beneath an existing supported local directory and migrate.
    /// The caller must hold runtime ownership (T019); this does not elect a coordinator.
    pub async fn open(path: &Path) -> Result<Self> {
        let path = path::validate(path)?;
        let mut db =
            SqliteConnection::connect_with(&options(&path).create_if_missing(true)).await?;
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&mut db)
            .await?;
        let parts: Vec<u32> = version
            .split('.')
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| StoreError::SqliteVersion)?;
        if parts.len() != 3 || parts.as_slice() < [3, 51, 3].as_slice() {
            return Err(StoreError::SqliteVersion);
        }
        migrations::preflight(&mut db).await?;
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode=WAL")
            .fetch_one(&mut db)
            .await?;
        if mode != "wal" {
            return Err(StoreError::Filesystem);
        }
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut db)
            .await?;
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut db)
            .await?;
        let busy_timeout_ms: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut db)
            .await?;
        if foreign_keys != 1 || synchronous != 2 || busy_timeout_ms != 250 {
            return Err(StoreError::InvalidRequest);
        }
        migrations::apply(&mut db).await?;
        let source = sqlx::query_scalar("SELECT sqlite_source_id()")
            .fetch_one(&mut db)
            .await?;
        Ok(Self {
            db,
            path,
            info: StorageInfo {
                foreign_keys: foreign_keys == 1,
                synchronous,
                busy_timeout_ms,
                sqlite_version: version,
                sqlite_source_id: source,
                schema_version: SCHEMA_VERSION,
            },
        })
    }
    /// Actual linked library and supported schema evidence.
    pub fn info(&self) -> &StorageInfo {
        &self.info
    }
    /// Open a separate read-only connection; read methods never return live cursors.
    pub async fn reader(&self) -> Result<Reader> {
        let db = SqliteConnection::connect_with(
            &options(&self.path)
                .read_only(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
        Ok(Reader { db })
    }
    /// Register an existing durable workspace identity. Reopening the same binding is idempotent.
    pub async fn register_workspace(
        &mut self,
        id: WorkspaceId,
        root_identity: &str,
        at_us: i64,
    ) -> Result<()> {
        if root_identity.is_empty() || root_identity.len() > 4096 {
            return Err(StoreError::InvalidRequest);
        }
        sqlx::query("INSERT INTO workspaces(id,root_identity,created_at_us) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET root_identity=excluded.root_identity WHERE workspaces.root_identity=excluded.root_identity")
            .bind(id.to_string()).bind(root_identity).bind(at_us).execute(&mut self.db).await?.rows_affected().eq(&1).then_some(()).ok_or(StoreError::InvalidRequest)
    }
    /// Index an already allocated registry identity; this never allocates a UUID.
    pub async fn register_dataset(
        &mut self,
        workspace: WorkspaceId,
        id: DatasetId,
        at_us: i64,
    ) -> Result<()> {
        sqlx::query("INSERT INTO datasets(id,workspace_id,created_at_us) VALUES(?,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(id.to_string()).bind(workspace.to_string()).bind(at_us).execute(&mut self.db).await?;
        let owner: String = sqlx::query_scalar("SELECT workspace_id FROM datasets WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&mut self.db)
            .await?;
        if owner != workspace.to_string() {
            return Err(StoreError::InvalidRequest);
        }
        Ok(())
    }
    /// Append one bounded event and audit record atomically, returning its monotonic cursor.
    pub async fn append_event(
        &mut self,
        id: tf_domain::RequestId,
        kind: &str,
        payload: &serde_json::Value,
        at_us: i64,
    ) -> Result<i64> {
        let payload = serde_json::to_string(payload).map_err(|_| StoreError::InvalidRequest)?;
        if kind.is_empty() || kind.len() > 128 || payload.len() > 1024 * 1024 {
            return Err(StoreError::InvalidRequest);
        }
        let mut tx = self.db.begin().await?;
        let sequence =
            sqlx::query("INSERT INTO events(id,type,payload_json,wall_time_us) VALUES(?,?,?,?)")
                .bind(id.to_string())
                .bind(kind)
                .bind(&payload)
                .bind(at_us)
                .execute(&mut *tx)
                .await?
                .last_insert_rowid();
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES(?,?,?)")
            .bind(kind)
            .bind(&payload)
            .bind(at_us)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(sequence)
    }
    /// Close SQLite cleanly; application shutdown should explicitly await this.
    pub async fn close(self) -> Result<()> {
        self.db.close().await?;
        Ok(())
    }
}
fn options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .foreign_keys(true)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(BUSY_TIMEOUT)
}
/// Short-lived read repository. Results are owned and capped, never streaming snapshots.
pub struct Reader {
    db: SqliteConnection,
}
/// Retained event projection; cursor order is independent of wall-clock order.
#[derive(Debug, PartialEq)]
pub struct Event {
    /// Monotonic durable sequence.
    pub sequence: i64,
    /// Stable event category.
    pub kind: String,
    /// Retained JSON evidence.
    pub payload: String,
    /// UTC microseconds.
    pub wall_time_us: i64,
}
impl Reader {
    /// Open existing runtime state without creating a database or applying migrations.
    pub async fn open_existing(path: &Path) -> Result<Self> {
        let path = path::validate(path)?;
        if !path.is_file() {
            return Err(StoreError::InvalidRequest);
        }
        let mut db = SqliteConnection::connect_with(&options(&path).read_only(true)).await?;
        migrations::preflight(&mut db).await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut db)
            .await?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::Migration);
        }
        Ok(Self { db })
    }
    /// Bounded retained version/schema information; external replicas are a separate service.
    pub async fn catalog_metadata(
        &mut self,
        workspace: WorkspaceId,
        id: DatasetId,
    ) -> Result<serde_json::Value> {
        let count = self.dataset_version_count(workspace, id).await?;
        let rows=sqlx::query("SELECT v.id,v.published_at_us,a.schema_json FROM dataset_versions v JOIN datasets d ON d.id=v.dataset_id JOIN artifacts a ON a.digest=v.artifact_digest WHERE d.workspace_id=? AND d.id=? ORDER BY v.published_at_us DESC,v.id DESC LIMIT 10").bind(workspace.to_string()).bind(id.to_string()).fetch_all(&mut self.db).await?;
        let mut versions = Vec::new();
        for row in rows {
            let schema: String = row.try_get(2)?;
            if schema.len() > 1024 * 1024 {
                return Err(StoreError::InvalidRequest);
            }
            versions.push(serde_json::json!({"id":row.try_get::<String,_>(0)?,"published_at_us":row.try_get::<i64,_>(1)?.to_string(),"schema_available":!schema.is_empty()}));
        }
        Ok(serde_json::json!({"version_count":count.to_string(),"recent_versions":versions}))
    }
    /// Whether this runtime indexes a particular durable identity.
    pub async fn contains_dataset(
        &mut self,
        workspace: WorkspaceId,
        id: DatasetId,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM datasets WHERE workspace_id=? AND id=?",
        )
        .bind(workspace.to_string())
        .bind(id.to_string())
        .fetch_one(&mut self.db)
        .await?
            == 1)
    }
    /// Count materialized versions for an owner-qualified identity; registration alone is zero.
    pub async fn dataset_version_count(
        &mut self,
        workspace: WorkspaceId,
        id: DatasetId,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM dataset_versions v JOIN datasets d ON d.id=v.dataset_id WHERE d.workspace_id=? AND d.id=?")
            .bind(workspace.to_string()).bind(id.to_string()).fetch_one(&mut self.db).await?)
    }
    /// Read at most 100 events after a cursor. No long-lived transaction escapes.
    pub async fn events_after(&mut self, cursor: i64, limit: u32) -> Result<Vec<Event>> {
        if cursor < 0 || limit == 0 || limit > 100 {
            return Err(StoreError::InvalidRequest);
        }
        let rows=sqlx::query("SELECT sequence,type,payload_json,wall_time_us FROM events WHERE sequence>? ORDER BY sequence LIMIT ?").bind(cursor).bind(limit).fetch_all(&mut self.db).await?;
        rows.into_iter()
            .map(|r| {
                Ok(Event {
                    sequence: r.try_get(0)?,
                    kind: r.try_get(1)?,
                    payload: r.try_get(2)?,
                    wall_time_us: r.try_get(3)?,
                })
            })
            .collect()
    }
    /// Release the read connection.
    pub async fn close(self) -> Result<()> {
        self.db.close().await?;
        Ok(())
    }
}

/// Guarded copy-only local Parquet import preparation.
pub mod imports;

/// Logical schema normalization, precision guards and bounded typed cells.
pub mod normalization;

/// Immutable, strictly verified multi-file artifact installation.
pub mod artifacts;

/// Fenced per-dataset visibility transactions and durable replay.
pub mod publication;

/// Exact renewable read pins and fenced retention roots.
pub mod retention;

/// Read-only output previews and guarded lazy branch creation.
pub mod branches;

/// Atomic local input fallback resolution with exact renewable read leases.
pub mod input_resolution;

/// Audited data-branch lifecycle and immutable source-indexed policy snapshots.
pub mod branch_lifecycle;

/// Immutable draft persistence and atomic guarded acceptance.
pub mod planning;
/// Retained original replay identity and exact-boundary leases.
pub mod replay;

/// Transactionally frozen freshness history, independent of graph evaluation.
pub mod freshness;

/// Durable exact check evidence, certificate reuse and explicit private diagnostics.
pub mod check_evidence;

/// Guarded accepted-plan dispatch persistence.
pub mod execution;
