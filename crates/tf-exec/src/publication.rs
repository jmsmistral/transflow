//! Ownership travels with blocking work even if its caller drops the async future.
use crate::ownership::{CoordinatorMode, OwnershipError, RuntimeOwner};
use std::{fs::File, io, path::PathBuf};
use tf_domain::RequestId;
use tf_store::{
    artifacts::{ArtifactError, ArtifactStore},
    publication::{PublicationError, PublicationReceipt, PublicationRequest},
};
/// Fixed filesystem/SQLite/notification boundaries; observers run outside SQL transactions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    /// File/manifest/rename durability boundary.
    Artifact(tf_store::artifacts::Boundary),
    /// Durable intent exists; no installed object is required yet.
    IntentPrepared,
    /// Durable verified object exists, immediately before SQLite visibility.
    BeforeCommit,
    /// Version/head/provenance/checks/event/outbox committed together.
    Committed,
    /// Notification consumers may now drain the persisted outbox.
    BeforeNotification,
}
/// Recoverable publication/orchestration failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Owner guard or namespace changed.
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    /// Strict candidate verification or object durability failed.
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    /// Database authority/evidence/head guard refused publication.
    #[error(transparent)]
    Publication(#[from] PublicationError),
    /// Failure at an observable I/O boundary.
    #[error("Publication was interrupted; inspect the durable intent and event before retrying")]
    Io(#[from] io::Error),
    /// Blocking task failed; its ownership remains with it until work actually ends.
    #[error("Publication task was interrupted")]
    Join(#[from] tokio::task::JoinError),
    /// Provider metadata-only owners cannot execute/publicize a version.
    #[error("Metadata-only access cannot publish dataset versions")]
    Mode,
}
/// Already materialized and validated bytes. This service never invokes user code.
pub struct MaterializedFiles {
    /// Pinned directory containing the exact worker output, not a glob.
    pub root: File,
    /// Explicit ordered relative paths.
    pub files: Vec<PathBuf>,
    /// Closed writer provenance in ArtifactManifestV1.
    pub writer: serde_json::Value,
    /// Unique private staging allocation.
    pub staging: RequestId,
}
/// Owner returned after work really finishes, including an operation failure.
pub struct Completion {
    /// Still-held runtime lock for the coordinator's next operation.
    pub owner: RuntimeOwner,
    /// Durable success receipt or inspectable error.
    pub result: Result<PublicationReceipt, Error>,
}
/// Reconcile DB sessions only after acquiring the actual local runtime lock.
/// Worker PID cleanup is the later execution supervisor's responsibility.
pub async fn recover(owner: &mut RuntimeOwner, at_us: i64) -> Result<(), Error> {
    if owner.registration().mode() == CoordinatorMode::MetadataOnly {
        return Err(Error::Mode);
    }
    let workspace = owner.workspace_id()?;
    let session = owner.registration().session()?;
    let mut store = owner.open_store().await?;
    store
        .repository()?
        .recover_publications(workspace, session, at_us)
        .await?;
    store.close().await?;
    Ok(())
}
/// Copy/verify candidate, journal intent, install durably, then commit visible metadata.
/// Observers may stop or fail at boundaries but never execute within a write transaction.
/// Notification is intentionally an outbox read/ack operation, with stable event IDs.
pub async fn publish(
    mut owner: RuntimeOwner,
    request: PublicationRequest,
    files: MaterializedFiles,
    mut observer: impl FnMut(Boundary) -> io::Result<()> + Send + 'static,
) -> Result<Completion, Error> {
    let task = tokio::task::spawn_blocking(move || {
        let result = (|| {
            if owner.registration().mode() == CoordinatorMode::MetadataOnly {
                return Err(Error::Mode);
            }
            owner.validate_paths()?;
            if owner.workspace_id()? != request.intent.target().dataset.workspace_id()
                || owner.registration().session()? != request.intent.fence().session
            {
                return Err(PublicationError::Fence.into());
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let artifacts = ArtifactStore::open(owner.workspace_root())?;
            let candidate = artifacts.prepare(
                &files.root,
                &files.files,
                files.writer,
                files.staging,
                |b| observer(Boundary::Artifact(b)),
            )?;
            let mut store = runtime.block_on(owner.open_store())?;
            runtime.block_on(
                store
                    .repository()?
                    .prepare_publication(&request, &candidate),
            )?;
            observer(Boundary::IntentPrepared)?;
            let object = candidate.install(|b| observer(Boundary::Artifact(b)))?;
            observer(Boundary::BeforeCommit)?;
            // Reverify after observer/pauses; real ownership and immutable object handles remain held.
            let object = artifacts.verify(object.digest())?;
            let receipt =
                runtime.block_on(store.repository()?.commit_publication(&request, &object))?;
            observer(Boundary::Committed)?;
            observer(Boundary::BeforeNotification)?;
            runtime.block_on(store.close())?;
            Ok(receipt)
        })();
        Completion { owner, result }
    });
    Ok(task.await?)
}
