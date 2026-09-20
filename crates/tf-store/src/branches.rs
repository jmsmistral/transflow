//! Branch names are opaque metadata. Preview never creates rows; mutation never copies heads.
use crate::{Reader, Store, StoreError};
use serde_json::json;
use sqlx::{Connection, Row, SqliteConnection};
use tf_domain::{BranchId, BranchName, SourceSnapshotId, WorkspaceId};
use tf_protocol::canonical::{ContentDigest, DigestKind};
/// Output selection/storage errors with explicit tombstone and revision handling.
#[derive(Debug, thiserror::Error)]
pub enum BranchError {
    /// SQLite/repository error.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// An explicit restore/new-name decision is required.
    #[error("This data branch was deleted; explicitly restore it or choose another name")]
    Tombstoned,
    /// Selected branch/workspace changed since the preview.
    #[error("The selected output branch changed; prepare the request again")]
    Conflict,
    /// Invalid identity, excessive name or absent validation evidence.
    #[error("Output branch creation requires complete validated preparation")]
    Evidence,
}
impl From<sqlx::Error> for BranchError {
    fn from(e: sqlx::Error) -> Self {
        Self::Store(e.into())
    }
}
type Result<T> = std::result::Result<T, BranchError>;
/// Current existence/revision, captured without side effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchExistence {
    /// Will be created only during valid mutating preparation.
    Absent,
    /// Existing branch, with no assumption that any head exists.
    Present {
        /// Stable branch identity.
        id: BranchId,
        /// Revision guarding lifecycle races.
        revision: i64,
    },
}
/// Workspace-qualified output preview; private identity fields prevent accidental retargeting.
#[derive(Clone, Debug)]
pub struct OutputBranchPreview {
    workspace: WorkspaceId,
    name: BranchName,
    state: BranchExistence,
}
impl OutputBranchPreview {
    /// Name displayed in a plan, preserving case/Unicode/slashes.
    pub fn name(&self) -> &BranchName {
        &self.name
    }
    /// Existence or expected lazy creation.
    pub fn state(&self) -> BranchExistence {
        self.state
    }
    /// Owning workspace.
    pub fn workspace(&self) -> WorkspaceId {
        self.workspace
    }
    /// A plan made before runtime initialization. Mutation rechecks actual absence.
    pub fn without_runtime(workspace: WorkspaceId, name: BranchName) -> Self {
        Self {
            workspace,
            name,
            state: BranchExistence::Absent,
        }
    }
}
/// Successful preparation evidence retained in the creation audit.
/// Only the composition service can verify the complete structural context; the
/// repository validates its carrier and enforces workspace/revision/tombstone guards.
pub struct BranchCreationEvidence {
    /// Captured source used by structural validation.
    pub source: SourceSnapshotId,
    /// Actual structural certificate fingerprint.
    pub certificate: ContentDigest,
    /// Associated Git name, only when Git selected this data branch.
    pub git_ref: Option<BranchName>,
}
async fn inspect(
    db: &mut SqliteConnection,
    workspace: WorkspaceId,
    name: &BranchName,
) -> Result<OutputBranchPreview> {
    if name.as_str().len() > 4096 {
        return Err(BranchError::Evidence);
    }
    let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM workspaces WHERE id=?")
        .bind(workspace.to_string())
        .fetch_one(&mut *db)
        .await?;
    if exists != 1 {
        return Err(BranchError::Conflict);
    }
    let row = sqlx::query(
        "SELECT id,revision,deleted_at_us FROM data_branches WHERE workspace_id=? AND name=?",
    )
    .bind(workspace.to_string())
    .bind(name.as_str())
    .fetch_optional(db)
    .await?;
    let state = if let Some(row) = row {
        if row.try_get::<Option<i64>, _>(2)?.is_some() {
            return Err(BranchError::Tombstoned);
        }
        BranchExistence::Present {
            id: row
                .try_get::<String, _>(0)?
                .parse()
                .map_err(|_| BranchError::Evidence)?,
            revision: row.try_get(1)?,
        }
    } else {
        BranchExistence::Absent
    };
    Ok(OutputBranchPreview {
        workspace,
        name: name.clone(),
        state,
    })
}
impl Reader {
    /// Read-only planning lookup. Absent names remain absent; tombstones are explicit errors.
    pub async fn preview_output_branch(
        &mut self,
        workspace: WorkspaceId,
        name: &BranchName,
    ) -> Result<OutputBranchPreview> {
        inspect(&mut self.db, workspace, name).await
    }
}
impl Store {
    /// Create an absent branch only after valid mutating preparation, under runtime ownership.
    /// Application callers must use the context-validating composition service.
    pub async fn create_output_branch(
        &mut self,
        preview: &OutputBranchPreview,
        new_id: BranchId,
        evidence: &BranchCreationEvidence,
        at_us: i64,
    ) -> Result<BranchId> {
        if evidence.certificate.kind() != DigestKind::Compute
            || evidence
                .git_ref
                .as_ref()
                .is_some_and(|n| n != preview.name())
        {
            return Err(BranchError::Evidence);
        }
        let audit = json!({
            "branch_id": new_id.to_string(),
            "name": preview.name.as_str(),
            "source_snapshot_id": evidence.source.to_string(),
            "certificate_fingerprint": evidence.certificate.hex(),
            "git_ref": evidence.git_ref.as_ref().map(BranchName::as_str)
        })
        .to_string();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let current = inspect(&mut tx, preview.workspace, &preview.name).await?;
        if current.state != preview.state {
            return Err(BranchError::Conflict);
        }
        if let BranchExistence::Present { id, .. } = current.state {
            tx.commit().await?;
            return Ok(id);
        }
        sqlx::query(
            "INSERT INTO data_branches(id,workspace_id,name,git_ref,revision) VALUES(?,?,?,?,0)",
        )
        .bind(new_id.to_string())
        .bind(preview.workspace.to_string())
        .bind(preview.name.as_str())
        .bind(evidence.git_ref.as_ref().map(BranchName::as_str))
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('branch.create',?,?)").bind(audit).bind(at_us).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(new_id)
    }
}
