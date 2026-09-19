use super::*;
use crate::{BuildId, JobId};
use std::collections::BTreeMap;
/// Whole-build state, separate from each job's attempt history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildState {
    /// Accepted request awaiting dispatch.
    Queued,
    /// Executing or resolving scoped jobs.
    Running,
    /// All required jobs supplied acceptable versions.
    Succeeded,
    /// A mandatory job failed or was blocked.
    Failed,
    /// Required work was canceled after acknowledgement.
    Canceled,
    /// Required work was interrupted without committed results.
    Interrupted,
}
impl BuildState {
    /// Terminal states cannot be rewritten.
    pub fn terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}
/// Build completion checks exact job membership and required outcomes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Build {
    id: BuildId,
    jobs: BTreeMap<JobId, bool>,
    binding: ExecutionBinding,
    state: BuildState,
    cancel_requested: bool,
    last_event: EventTime,
}
impl Build {
    /// Register the accepted plan's complete job set and mandatory flags.
    /// At least one required job is necessary; an all-cached build is still meaningful.
    pub fn new(
        id: BuildId,
        binding: ExecutionBinding,
        jobs: Vec<(JobId, bool)>,
        at: EventTime,
    ) -> Result<Self, StateError> {
        let count = jobs.len();
        let jobs: BTreeMap<_, _> = jobs.into_iter().collect();
        if jobs.len() != count || !jobs.values().any(|v| *v) {
            return Err(StateError::InvalidIdentity);
        }
        Ok(Self {
            id,
            jobs,
            binding,
            state: BuildState::Queued,
            cancel_requested: false,
            last_event: at,
        })
    }
    /// Read-only build state.
    pub fn state(&self) -> BuildState {
        self.state
    }
    /// Independent cancellation request; it does not cancel jobs by itself.
    pub fn cancellation_requested(&self) -> bool {
        self.cancel_requested
    }
    /// Begin dispatch after acceptance; no attempts are created by this transition.
    pub fn start(&mut self, at: EventTime) -> Result<(), StateError> {
        if self.state != BuildState::Queued {
            return Err(StateError::InvalidTransition);
        }
        if at < self.last_event {
            return Err(StateError::InvalidTime);
        }
        if self.cancel_requested {
            return Err(StateError::CancellationPending);
        }
        self.state = BuildState::Running;
        self.last_event = at;
        Ok(())
    }
    /// Idempotent request; future supervision propagates it to this build's jobs only.
    pub fn request_cancel(&mut self) -> CancelRequest {
        if self.state.terminal() {
            return CancelRequest::TooLate;
        }
        if self.cancel_requested {
            return CancelRequest::AlreadyRequested;
        }
        self.cancel_requested = true;
        CancelRequest::Requested
    }
    /// Complete only from the exact same-build terminal job set. Cached work is successful.
    /// Mandatory failure/blocked takes precedence, then interruption, then cancellation.
    /// A late cancel request never rewrites successful committed/cached work.
    pub fn finish(&mut self, jobs: &[Job], at: EventTime) -> Result<(), StateError> {
        if self.state.terminal() {
            return Err(StateError::InvalidTransition);
        }
        if at < self.last_event {
            return Err(StateError::InvalidTime);
        }
        let ids: BTreeSet<_> = jobs.iter().map(Job::id).collect();
        if ids.len() != jobs.len()
            || ids != self.jobs.keys().copied().collect()
            || jobs
                .iter()
                .any(|j| j.build() != self.id || j.binding() != self.binding)
        {
            return Err(StateError::InvalidIdentity);
        }
        if jobs.iter().any(|j| j.last_event() > at) {
            return Err(StateError::InvalidTime);
        }
        if self.state == BuildState::Queued && jobs.iter().any(|j| !j.attempts().is_empty()) {
            return Err(StateError::InvalidTransition);
        }
        let mut failure = false;
        let mut interrupted = false;
        let mut canceled = false;
        for job in jobs {
            let JobState::Finished(outcome) = job.state() else {
                return Err(StateError::InvalidEvidence);
            };
            if self.jobs.get(&job.id()) == Some(&true) {
                match outcome {
                    JobOutcome::Failed(_) | JobOutcome::Blocked(_) => failure = true,
                    JobOutcome::Interrupted => interrupted = true,
                    JobOutcome::Canceled => canceled = true,
                    _ => {}
                }
            }
        }
        self.state = if failure {
            BuildState::Failed
        } else if interrupted {
            BuildState::Interrupted
        } else if canceled {
            BuildState::Canceled
        } else {
            BuildState::Succeeded
        };
        self.last_event = at;
        Ok(())
    }
}
