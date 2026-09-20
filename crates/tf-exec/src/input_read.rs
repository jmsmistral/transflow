//! Strict verification after exact head selection; never retries another branch.
use crate::ownership::{OwnershipError, RuntimeOwner};
use tf_store::{
    artifacts::{ArtifactError, ArtifactStore, VerifiedArtifact},
    input_resolution::ResolvedRead,
};
/// Failures retain the selected head's meaning rather than treating it as absence.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Runtime ownership/namespace changed.
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    /// Missing/corrupt/incompatible selected bytes.
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    /// Operation does not belong to this owner.
    #[error("The selected input belongs to another runtime owner")]
    Context,
    /// Blocking verification failed to finish normally.
    #[error("Input verification was interrupted")]
    Join(#[from] tokio::task::JoinError),
}
/// Ownership remains held until blocking I/O really finishes, even on async cancellation.
pub struct Completion {
    /// Owner returned on success or an ordinary verification error.
    pub owner: RuntimeOwner,
    /// Always returned, including on failure, so the caller can release its lease.
    pub read: ResolvedRead,
    /// Verified exact bytes or selected-head error; freshness and checks remain later gates.
    pub result: Result<VerifiedArtifact, Error>,
}
/// Verify selected bytes off the async executor while retaining runtime ownership.
/// Callers renew/release the returned lease during read/check lifetime, including on failure.
/// If the async caller is dropped, the lease expires as crash recovery; ownership remains
/// held throughout this scan, so GC cannot race the blocking work.
pub async fn verify(owner: RuntimeOwner, read: ResolvedRead) -> Result<Completion, Error> {
    Ok(tokio::task::spawn_blocking(move || {
        let result = (|| {
            owner.validate_paths()?;
            if owner.workspace_id()? != read.binding().dataset.workspace_id() {
                return Err(Error::Context);
            }
            let artifacts = ArtifactStore::open(owner.workspace_root())?;
            let artifact = artifacts.verify(read.lease().artifact())?;
            Ok(artifact)
        })();
        Completion {
            owner,
            read,
            result,
        }
    })
    .await?)
}

/// Replay scan result retains all leases even when one original object fails verification.
pub struct ReplayCompletion {
    /// Runtime owner retained through blocking I/O.
    pub owner: RuntimeOwner,
    /// Immutable metadata and renewable exact tickets.
    pub read: tf_store::replay::ReplayRead,
    /// Verified original artifacts in boundary order; source/environment checks remain required.
    pub result: Result<Vec<VerifiedArtifact>, Error>,
}
/// Verify the original replay objects without consulting current branch heads.
pub async fn verify_replay(
    owner: RuntimeOwner,
    read: tf_store::replay::ReplayRead,
) -> Result<ReplayCompletion, Error> {
    Ok(tokio::task::spawn_blocking(move || {
        let result = (|| {
            owner.validate_paths()?;
            let boundaries = read.manifest.boundaries().map_err(|_| Error::Context)?;
            if boundaries.len() != read.leases.len() {
                return Err(Error::Context);
            }
            let artifacts = ArtifactStore::open(owner.workspace_root())?;
            boundaries
                .iter()
                .zip(&read.leases)
                .map(|(b, lease)| {
                    if owner.workspace_id()? != b.input.dataset.workspace_id()
                        || lease.version() != Some(b.input.version)
                        || lease.artifact() != b.input.artifact
                    {
                        return Err(Error::Context);
                    }
                    Ok(artifacts.verify(lease.artifact())?)
                })
                .collect()
        })();
        ReplayCompletion {
            owner,
            read,
            result,
        }
    })
    .await?)
}
