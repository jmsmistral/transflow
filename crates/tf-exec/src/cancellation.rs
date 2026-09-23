//! Persist the database winner before asking owned workers to stop.
//!
//! The coordinator retains one control per build. A waiting CLI is an observer,
//! not the owner of the supervisor future. No API accepts a PID to signal.
use crate::{
    ownership::{CoordinatorMode, OwnershipError, RuntimeOwner},
    supervisor::{self, Cancellation, Launch, Report},
};
use std::collections::BTreeMap;
use tf_domain::{
    AttemptId, BuildId, CoordinatorSessionId, WorkspaceId,
    execution::{CancelRequest, Fence},
};
use tf_store::publication::PublicationError;

/// Cancellation/registration refusal with no worker signal on storage failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Runtime owner validation failed.
    #[error(transparent)]
    Ownership(#[from] OwnershipError),
    /// The persisted attempt, session, reservation or publication winner refused it.
    #[error(transparent)]
    Publication(#[from] PublicationError),
    /// Duplicate registration or the bounded per-build attempt budget is exhausted.
    #[error("Worker registration is duplicate or exceeds this build's attempt limit")]
    Registration,
    /// Provider metadata access cannot dispatch or cancel builds.
    #[error("Metadata-only access cannot control build execution")]
    Mode,
}
/// Client events are separate from dropping the coordinator's supervisor future.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientEvent {
    /// Waiting connection ended without an explicit cancellation request.
    Disconnected,
    /// Ctrl-C or an authenticated cancellation request for this client's build.
    Cancel,
}
/// Outcome reported to the client; request acknowledgement is not cleanup completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disposition {
    /// Persistent coordinator still owns the work after an observer disconnects.
    Continuing,
    /// Durable database winner, including already requested and too late.
    Cancellation(CancelRequest),
}
/// Attempt-bound launch capability. Clones share cancellation for validation
/// helpers; each launch still needs its own admission permit and private nonce.
/// Random channel authentication never becomes a PID credential.
#[derive(Clone)]
pub struct Worker {
    attempt: AttemptId,
    cancel: Cancellation,
}
impl Worker {
    /// Attempt-bound token for composed input/transform/output helpers.
    pub fn cancellation(&self) -> Cancellation {
        self.cancel.clone()
    }
    fn matches(&self, launch: &Launch) -> bool {
        launch.request["attempt_id"]
            .as_str()
            .and_then(|value| value.parse::<AttemptId>().ok())
            == Some(self.attempt)
    }
    /// Run on a blocking coordinator thread and return only after actual cleanup.
    pub fn run(self, launch: Launch) -> Report {
        if !self.matches(&launch) {
            return Report::default();
        }
        supervisor::run(launch, self.cancel)
    }
    /// Coordinator-owned future. CLI observation must not own or abort this future.
    /// Dropping the coordinator future still requests actual process cleanup.
    pub async fn run_async(self, launch: Launch) -> Report {
        if !self.matches(&launch) {
            return Report::default();
        }
        supervisor::run_async(launch, self.cancel).await
    }
}
/// Bounded build-scoped control, tied to the OS owner's workspace/session/mode.
/// Retain it until all supervisors finish. Persisted cancellation blocks future
/// publication even if a process has not yet exited. Cleanup does not undo success.
pub struct BuildControl {
    workspace: WorkspaceId,
    session: CoordinatorSessionId,
    build: BuildId,
    mode: CoordinatorMode,
    limit: usize,
    workers: BTreeMap<AttemptId, Cancellation>,
}
impl Drop for BuildControl {
    fn drop(&mut self) {
        // Losing the coordinator control is emergency cleanup, not a durable
        // user cancellation receipt. Recovery fences any unfinished DB state.
        for cancel in self.workers.values() {
            cancel.cancel();
        }
    }
}
impl BuildControl {
    /// Bind to the real owner, with an explicit bounded lifetime attempt budget.
    /// Registering a worker or mutating a build additionally checks SQLite authority.
    pub fn new(owner: &RuntimeOwner, build: BuildId, attempt_limit: usize) -> Result<Self, Error> {
        owner.validate_paths()?;
        if owner.registration().mode() == CoordinatorMode::MetadataOnly {
            return Err(Error::Mode);
        }
        if !(1..=10_000).contains(&attempt_limit) {
            return Err(Error::Registration);
        }
        Ok(Self {
            workspace: owner.workspace_id()?,
            session: owner.registration().session()?,
            build,
            mode: owner.registration().mode(),
            limit: attempt_limit,
            workers: BTreeMap::new(),
        })
    }
    fn check(&self, owner: &RuntimeOwner) -> Result<(), Error> {
        owner.validate_paths()?;
        if owner.workspace_id()? != self.workspace
            || owner.registration().session()? != self.session
            || owner.registration().mode() != self.mode
        {
            return Err(PublicationError::Fence.into());
        }
        Ok(())
    }
    /// Register once, before spawn. An attempt's cancellation token cannot be
    /// replaced; it remains set for every not-yet-started helper clone too.
    pub async fn register(
        &mut self,
        owner: &mut RuntimeOwner,
        attempt: AttemptId,
        fence: Fence,
    ) -> Result<Worker, Error> {
        self.check(owner)?;
        if fence.session != self.session {
            return Err(PublicationError::Fence.into());
        }
        if self.workers.len() >= self.limit || self.workers.contains_key(&attempt) {
            return Err(Error::Registration);
        }
        let mut store = owner.open_store().await?;
        store
            .repository()?
            .authorize_worker(self.workspace, self.build, attempt, fence)
            .await?;
        store.close().await?;
        let cancel = Cancellation::default();
        self.workers.insert(attempt, cancel.clone());
        Ok(Worker { attempt, cancel })
    }
    /// Persist cancellation first, then request TERM/grace/KILL/reap. A committed
    /// publication remains success, and a database error never signals any worker.
    /// Retry an interrupted acknowledgement: the durable request is idempotent.
    pub async fn request(&mut self, owner: &mut RuntimeOwner) -> Result<CancelRequest, Error> {
        self.check(owner)?;
        let mut store = owner.open_store().await?;
        let result = store
            .repository()?
            .cancel_publications(self.workspace, self.session, self.build)
            .await?;
        if result != CancelRequest::TooLate {
            for cancel in self.workers.values() {
                cancel.cancel();
            }
        }
        store.close().await?;
        Ok(result)
    }
    /// An ordinary daemon observer disconnect does not cancel; transient disconnect
    /// and explicit Ctrl-C do. Caller awaits owned workers before transient shutdown.
    pub async fn client_event(
        &mut self,
        owner: &mut RuntimeOwner,
        event: ClientEvent,
    ) -> Result<Disposition, Error> {
        self.check(owner)?;
        if event == ClientEvent::Disconnected && self.mode == CoordinatorMode::Persistent {
            Ok(Disposition::Continuing)
        } else {
            Ok(Disposition::Cancellation(self.request(owner).await?))
        }
    }
}
