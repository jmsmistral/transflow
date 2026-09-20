//! Cache scans keep real runtime ownership off the async executor until adoption finishes.
use crate::ownership::{CoordinatorMode, OwnershipError, RuntimeOwner};
use std::io;
use tf_domain::RequestId;
use tf_store::{
    artifacts::{ArtifactError, ArtifactStore},
    cache::{self, Lookup, Receipt, Request},
};
/// Explicit failures preserve the selected candidate; no opportunistic fallback.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Ownership no longer matches the accepted request.
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    /// Missing/corrupt selected object.
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    /// Durable cache guards/evidence failed.
    #[error(transparent)]
    Cache(#[from] cache::Error),
    /// Lease cleanup failed.
    #[error(transparent)]
    Retention(#[from] tf_store::retention::RetentionError),
    /// Blocking runtime or clock failure.
    #[error("Cache verification could not finish")]
    Io(#[from] io::Error),
    /// Blocking worker failed.
    #[error("Cache verification worker was interrupted")]
    Join(#[from] tokio::task::JoinError),
    /// Provider metadata mode or another owner cannot adopt versions.
    #[error("Cache adoption requires the accepted build's writable runtime owner")]
    Context,
}
/// Owner returns even when candidate verification fails.
pub struct Completion {
    /// Held throughout verification and commit, including when the async future is dropped.
    pub owner: RuntimeOwner,
    /// None is a cache miss; original integrity errors are retained.
    pub result: Result<Option<Receipt>, Error>,
}
/// The caller has already finalized and frozen a deterministic, non-forced job contract.
/// A clock is supplied explicitly for deterministic expiry/race tests. Expired verification
/// must be retried with a new lease; a long scan never revives a stale ticket.
pub async fn reuse(
    mut owner: RuntimeOwner,
    mut request: Request,
    lease: RequestId,
    mut now: impl FnMut() -> io::Result<i64> + Send + 'static,
) -> Result<Completion, Error> {
    Ok(tokio::task::spawn_blocking(move || {
        let result = (|| {
            owner.validate_paths()?;
            if owner.registration().mode() == CoordinatorMode::MetadataOnly
                || owner.workspace_id()? != request.target.dataset.workspace_id()
                || owner.registration().session()? != request.fence.session
            {
                return Err(Error::Context);
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            request.at_us = now()?;
            let root = owner.workspace_root().to_owned();
            let mut store = runtime.block_on(owner.open_store())?;
            let result = (|| {
                match runtime.block_on(store.repository()?.lookup_cache(
                    &request,
                    lease,
                    3_600_000_000,
                ))? {
                    Lookup::Miss => Ok(None),
                    Lookup::Reused(r) => Ok(Some(r)),
                    Lookup::Candidate(candidate) => {
                        let result = (|| {
                            let artifact =
                                ArtifactStore::open(&root)?.verify(candidate.artifact())?;
                            // Store borrows the owner; it cannot disappear while I/O/adoption is active.
                            Ok(Some(runtime.block_on(store.repository()?.adopt_cache(
                                &candidate,
                                &artifact,
                                now()?,
                            ))?))
                        })();
                        let released =
                            runtime.block_on(store.repository()?.release_read(candidate.lease()));
                        match result {
                            Err(e) => Err(e),
                            Ok(r) => {
                                released?;
                                Ok(r)
                            }
                        }
                    }
                }
            })();
            let closed = runtime.block_on(store.close());
            match result {
                Err(e) => Err(e),
                Ok(r) => {
                    closed?;
                    Ok(r)
                }
            }
        })();
        Completion { owner, result }
    })
    .await?)
}
