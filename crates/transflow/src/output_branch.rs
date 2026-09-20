//! Validated mutating-preparation gate. Planning uses the read-only branch preview.
use tf_catalog::{
    git::{OutputBranchOrigin, OutputBranchSelection},
    validation::{ValidatedGraph, ValidationRequest},
};
use tf_domain::BranchId;
use tf_exec::ownership::{CoordinatorMode, OwnershipError, RuntimeOwner};
use tf_store::branches::{BranchCreationEvidence, BranchError, OutputBranchPreview};
/// Branch creation errors retain their authority/context distinction.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Runtime ownership or directory identity changed.
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    /// Revision/tombstone or repository refusal.
    #[error(transparent)]
    Branch(#[from] BranchError),
    /// Graph is stale, from another workspace, still unregistered, or owner is read-only.
    #[error("Output branch creation requires current validated and registered build preparation")]
    Context,
}
/// Privately bound structural context for a later mutating preparation step.
pub struct AuthorizedOutputBranch {
    preview: OutputBranchPreview,
    evidence: BranchCreationEvidence,
}
/// Verify complete structural evidence in the caller's blocking preparation work.
/// This authorizes no database write by itself and never creates a branch.
pub fn authorize_output_branch(
    selection: &OutputBranchSelection,
    preview: OutputBranchPreview,
    graph: &ValidatedGraph,
    context: &ValidationRequest<'_>,
) -> Result<AuthorizedOutputBranch, Error> {
    if preview.workspace() != context.registry.workspace()
        || preview.name() != &selection.name
        || !graph.certificate().matches(context)
        || !graph.candidate().pending().is_empty()
    {
        return Err(Error::Context);
    }
    let evidence = BranchCreationEvidence {
        source: context.source,
        certificate: *graph.certificate().fingerprint(),
        git_ref: if selection.origin == OutputBranchOrigin::Git {
            Some(selection.name.clone())
        } else {
            None
        },
    };
    Ok(AuthorizedOutputBranch { preview, evidence })
}
/// Lazily create only the authorized output after valid mutating preparation.
/// The later build service verifies capture/environment and reconciles the registry.
/// No hashing, Git operation, file scan or head copying runs on this async path.
pub async fn ensure_output_branch(
    owner: &mut RuntimeOwner,
    authorized: &AuthorizedOutputBranch,
    new_id: BranchId,
    at_us: i64,
) -> Result<BranchId, Error> {
    if owner.registration().mode() == CoordinatorMode::MetadataOnly
        || owner.workspace_id()? != authorized.preview.workspace()
    {
        return Err(Error::Context);
    }
    let mut store = owner.open_store().await?;
    let result = store
        .repository()?
        .create_output_branch(&authorized.preview, new_id, &authorized.evidence, at_us)
        .await?;
    store.close().await?;
    Ok(result)
}
