//! Short owner-fenced registry mutation. Filesystem work runs off the async executor.
use std::{collections::BTreeSet, io};
use tf_catalog::{
    RegistrySnapshot,
    candidate::RegistryProposal,
    capture::SourceSnapshot,
    registry_write::{
        Boundary, RegistryFile, RegistryWrite, RegistryWriteError, WriteContext, registry_digest,
    },
    workspace::Workspace,
};
use tf_domain::RequestId;
use tf_exec::ownership::{CoordinatorMode, OwnershipError, RuntimeOwner};
use tf_store::{StoreError, catalog_mutations::CatalogMutation};

/// Durable reconciliation failures, preserving typed diagnostic causes.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    /// Authoring permissions and captured context are required.
    #[error("This coordinator cannot modify the selected workspace registry")]
    Authority,
    /// Filesystem/source guard or durability failure.
    #[error(transparent)]
    Registry(#[from] RegistryWriteError),
    /// Runtime ownership or directory identity changed.
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    /// Journal/index transaction failed; retain the journal for recovery.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A blocking task failed; its durable journal remains authoritative.
    #[error("Registry work was interrupted; recover the pending journal before retrying")]
    Interrupted(#[from] tokio::task::JoinError),
}
/// Application service result.
pub type Result<T> = std::result::Result<T, ReconcileError>;
/// An accepted proposal's explicit context. Structural validation is the preparation caller's gate.
pub struct ReconcileRequest {
    /// Immutable source used by discovery and validation.
    pub capture: SourceSnapshot,
    /// Exact assignments retained by the prepared/saved draft.
    pub proposal: RegistryProposal,
    /// Fixed selections never authorize working-copy writes.
    pub context: WriteContext,
    /// Unique mutation identity supplied by the application, independent of dataset IDs.
    pub id: RequestId,
    /// UTC microseconds for new runtime identity records.
    pub at_us: i64,
}
/// Registry completion does not imply any materialized version exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconcileOutcome {
    /// No pending additions; no authoring rewrite or journal creation.
    Unchanged,
    /// Whole registry and exact IDs are durably committed.
    Indexed(RequestId),
}
struct Work<F> {
    write: RegistryWrite,
    observer: F,
}
#[derive(Clone, Copy)]
enum Step {
    Stage,
    Rename,
    Sync,
    BeforeIndex,
    Indexed,
}
fn step<F>(
    mut work: Work<F>,
    step: Step,
    store: &mut tf_exec::ownership::OwnedStore<'_>,
) -> Result<Work<F>>
where
    F: FnMut(Boundary) -> io::Result<()>,
{
    let result = (|| -> std::result::Result<(), RegistryWriteError> {
        match step {
            Step::Stage => {
                (work.observer)(Boundary::JournalPrepared)?;
                store
                    .repository()
                    .map_err(|_| RegistryWriteError::Conflict)?;
                work.write.stage()?;
                (work.observer)(Boundary::TemporarySynced)?;
            }
            Step::Rename => {
                (work.observer)(Boundary::BeforeRename)?;
                store
                    .repository()
                    .map_err(|_| RegistryWriteError::Conflict)?;
                work.write.rename()?;
                (work.observer)(Boundary::Renamed)?;
            }
            Step::Sync => {
                store
                    .repository()
                    .map_err(|_| RegistryWriteError::Conflict)?;
                work.write.sync()?;
                (work.observer)(Boundary::DirectorySynced)?;
            }
            Step::BeforeIndex => {
                (work.observer)(Boundary::BeforeIndex)?;
                store
                    .repository()
                    .map_err(|_| RegistryWriteError::Conflict)?;
                work.write.guard(true)?;
            }
            Step::Indexed => {
                (work.observer)(Boundary::Indexed)?;
            }
        }
        Ok(())
    })();
    result?;
    Ok(work)
}
/// Ownership is returned even when reconciliation fails, so the caller can recover it.
pub struct ReconcileCompletion {
    /// The same owner, never reacquired with a different fence.
    pub owner: RuntimeOwner,
    /// Durable outcome or recoverable failure.
    pub outcome: Result<ReconcileOutcome>,
}
/// Commit additive IDs on a dedicated blocking worker. Ownership moves with the work:
/// dropping the waiting future cannot release its lock while a rename is still running.
/// Once dispatched this short durability section finishes; execution cancellation is separate.
pub async fn reconcile(
    owner: RuntimeOwner,
    request: ReconcileRequest,
) -> Result<ReconcileCompletion> {
    reconcile_with_observer(owner, request, |_| Ok(())).await
}
/// Run deterministic durability observers outside SQLite transactions on the blocking worker.
pub async fn reconcile_with_observer<F>(
    mut owner: RuntimeOwner,
    request: ReconcileRequest,
    observer: F,
) -> Result<ReconcileCompletion>
where
    F: FnMut(Boundary) -> io::Result<()> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(RegistryWriteError::from)?;
        let outcome = runtime.block_on(execute(&mut owner, request, observer));
        Ok(ReconcileCompletion { owner, outcome })
    })
    .await?
}
async fn execute<F>(
    owner: &mut RuntimeOwner,
    request: ReconcileRequest,
    observer: F,
) -> Result<ReconcileOutcome>
where
    F: FnMut(Boundary) -> io::Result<()>,
{
    if owner.registration().mode() == CoordinatorMode::MetadataOnly
        || owner.workspace_id()? != request.proposal.workspace()
    {
        return Err(ReconcileError::Authority);
    }
    if request.proposal.assignments().is_empty() {
        return Ok(ReconcileOutcome::Unchanged);
    }
    let root = owner.workspace_root().to_owned();
    let root_identity = root.to_str().ok_or(ReconcileError::Authority)?.to_owned();
    let workspace = request.proposal.workspace();
    let source = request.proposal.source();
    let old_digest = *request.proposal.expected_old();
    let assignments = request
        .proposal
        .assignments()
        .iter()
        .map(|(p, id, _)| (p.clone(), *id))
        .collect();
    let write = RegistryWrite::prepare(
        &root,
        request.capture,
        request.proposal,
        request.id,
        request.context,
    )?;
    let index_ids = write.index_ids()?;
    let mutation = CatalogMutation {
        id: request.id,
        workspace,
        source,
        old_digest,
        new_digest: write.new_digest(),
        assignments,
        index_ids,
    };
    let mut store = owner.open_store().await?;
    store
        .repository()?
        .register_workspace(workspace, &root_identity, request.at_us)
        .await?;
    store
        .repository()?
        .prepare_catalog_mutation(&mutation)
        .await?;
    let work = step(Work { write, observer }, Step::Stage, &mut store)?;
    let work = step(work, Step::Rename, &mut store)?;
    let work = step(work, Step::Sync, &mut store)?;
    store.repository()?.catalog_replaced(request.id).await?;
    let work = step(work, Step::BeforeIndex, &mut store)?;
    store
        .repository()?
        .index_catalog_mutation(request.id, request.at_us)
        .await?;
    let _work = step(work, Step::Indexed, &mut store)?;
    store.close().await?;
    Ok(ReconcileOutcome::Indexed(request.id))
}
/// Recovery only reads/syncs authoring state. It never renames, allocates or overwrites files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryOutcome {
    /// Exact replacement exists; the persisted IDs were indexed idempotently.
    Indexed(RequestId),
    /// Old registry is intact; incomplete intent is retained as CONFLICT/abandoned.
    NotApplied(RequestId),
    /// Neither digest/identity guard matches. Capture and reconcile explicitly before proceeding.
    Conflict(RequestId),
}
/// Reconcile outstanding journal evidence before any new authoring operation.
/// Caller must surface Conflict and not continue build acceptance as though recovery succeeded.
pub async fn recover(owner: &mut RuntimeOwner, at_us: i64) -> Result<Vec<RecoveryOutcome>> {
    if owner.registration().mode() == CoordinatorMode::MetadataOnly {
        return Err(ReconcileError::Authority);
    }
    let root = owner.workspace_root().to_owned();
    let workspace = owner.workspace_id()?;
    let mut store = owner.open_store().await?;
    let mut outcomes = Vec::new();
    for mutation in store.repository()?.pending_catalog_mutations().await? {
        if mutation.workspace != workspace {
            return Err(ReconcileError::Authority);
        }
        let id = mutation.id;
        let root = root.clone();
        let outcome =
            tokio::task::spawn_blocking(move || -> std::result::Result<_, RegistryWriteError> {
                let selected = Workspace::load(&root, Some(&root))
                    .map_err(|_| RegistryWriteError::Conflict)?;
                if selected.config().id() != mutation.workspace {
                    return Err(RegistryWriteError::Conflict);
                }
                let file = RegistryFile::open(&root)?;
                let bytes = file.read()?;
                let digest = registry_digest(&bytes)?;
                if digest == mutation.old_digest {
                    return Ok(RecoveryOutcome::NotApplied(id));
                }
                if digest != mutation.new_digest {
                    return Ok(RecoveryOutcome::Conflict(id));
                }
                let registry = RegistrySnapshot::parse(
                    mutation.workspace,
                    std::str::from_utf8(&bytes).map_err(|_| RegistryWriteError::Conflict)?,
                )
                .map_err(|_| RegistryWriteError::Conflict)?;
                let ids: BTreeSet<_> = registry.datasets().map(|d| d.key().dataset_id()).collect();
                if ids != mutation.index_ids.iter().copied().collect()
                    || mutation.assignments.iter().any(|(path, id)| {
                        registry.resolve(path.as_str()).map_or(true, |r| {
                            r.key().dataset_id() != *id
                                || r.key().workspace_id() != mutation.workspace
                        })
                    })
                {
                    return Ok(RecoveryOutcome::Conflict(id));
                }
                file.sync(&mutation.new_digest)?;
                Ok(RecoveryOutcome::Indexed(id))
            })
            .await?;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(RegistryWriteError::Conflict) => RecoveryOutcome::Conflict(id),
            Err(e) => return Err(e.into()),
        };
        match outcome {
            RecoveryOutcome::Indexed(_) => {
                store
                    .repository()?
                    .index_catalog_mutation(id, at_us)
                    .await?
            }
            RecoveryOutcome::NotApplied(_) | RecoveryOutcome::Conflict(_) => {
                store.repository()?.catalog_conflict(id).await?
            }
        }
        outcomes.push(outcome);
    }
    store.close().await?;
    Ok(outcomes)
}
