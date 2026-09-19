use super::*;
use crate::{AttemptId, BuildId, JobId, VersionId};

/// Frozen candidate intent. Construction binds attempt, output, version and owner fence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationIntent {
    job: JobId,
    build: BuildId,
    binding: ExecutionBinding,
    attempt: AttemptId,
    version: VersionId,
    artifact: ArtifactDigest,
    target: OutputTarget,
    fence: Fence,
}
impl PublicationIntent {
    /// Planned job that owns the candidate.
    pub fn job(&self) -> JobId {
        self.job
    }
    /// Build whose guarded plan authorized the job.
    pub fn build(&self) -> BuildId {
        self.build
    }
    /// Immutable accepted-plan/source binding.
    pub fn binding(&self) -> ExecutionBinding {
        self.binding
    }
    /// Execution attempt authorized for this candidate.
    pub fn attempt(&self) -> AttemptId {
        self.attempt
    }
    /// Independent planned publication UUID.
    pub fn version(&self) -> VersionId {
        self.version
    }
    /// Physical byte identity; independent of the publication UUID.
    pub fn artifact(&self) -> ArtifactDigest {
        self.artifact
    }
    /// Dataset/branch and expected head generation.
    pub fn target(&self) -> OutputTarget {
        self.target
    }
    /// Exact coordinator/reservation identity.
    pub fn fence(&self) -> Fence {
        self.fence
    }
}
/// Domain acknowledgement of guarded publication. Storage must commit it atomically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationEvidence {
    intent: Box<PublicationIntent>,
    committed_at: EventTime,
    head_generation: u64,
}
impl PublicationEvidence {
    /// Immutable candidate and authority evidence.
    pub fn intent(&self) -> &PublicationIntent {
        &self.intent
    }
    /// Supplied commit time, never a fabricated cache interval.
    pub fn committed_at(&self) -> EventTime {
        self.committed_at
    }
    /// New head generation after compare-and-swap.
    pub fn head_generation(&self) -> u64 {
        self.head_generation
    }
}
/// A real attempt's immutable terminal outcome. No cached/blocked attempt exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttemptOutcome {
    /// Publication won the guard.
    Succeeded(PublicationEvidence),
    /// Failed execution evidence, retained through retries.
    Failed(FailureEvidence),
    /// Cleanup was acknowledged after a cancellation request.
    Canceled,
    /// Ownership changed without committed publication.
    Interrupted,
}
/// A started execution with one interval and immutable binding/terminal evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attempt {
    id: AttemptId,
    binding: ExecutionBinding,
    fence: Fence,
    phase: Phase,
    started_at: EventTime,
    finished_at: Option<EventTime>,
    outcome: Option<AttemptOutcome>,
    intent: Option<PublicationIntent>,
}
impl Attempt {
    /// Fresh identity for this try.
    pub fn id(&self) -> AttemptId {
        self.id
    }
    /// Captured source and accepted plan shared with every retry.
    pub fn binding(&self) -> ExecutionBinding {
        self.binding
    }
    /// Ownership when started.
    pub fn fence(&self) -> Fence {
        self.fence
    }
    /// Current or last observed execution phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }
    /// Actual supplied start time.
    pub fn started_at(&self) -> EventTime {
        self.started_at
    }
    /// Absent while active; terminal evidence has a supplied end time.
    pub fn finished_at(&self) -> Option<EventTime> {
        self.finished_at
    }
    /// Immutable terminal result; absence means active.
    pub fn outcome(&self) -> Option<&AttemptOutcome> {
        self.outcome.as_ref()
    }
    /// Candidate intent, possibly retained after a failed/canceled commit.
    pub fn intent(&self) -> Option<&PublicationIntent> {
        self.intent.as_ref()
    }
}
/// A job decision or executed result. Cached jobs refer to an existing version only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobOutcome {
    /// Newly materialized publication.
    Succeeded(PublicationEvidence),
    /// Reused retained version; no new attempt or publication timestamp.
    Cached(VersionId),
    /// Final attempt failure.
    Failed(FailureEvidence),
    /// Dependency could not supply its required result.
    Blocked(JobId),
    /// Cancellation acknowledged.
    Canceled,
    /// Interrupted ownership/recovery.
    Interrupted,
}
impl JobOutcome {
    /// Whether the job supplied an acceptable version for its required target.
    pub fn successful(&self) -> bool {
        matches!(self, Self::Succeeded(_) | Self::Cached(_))
    }
}
/// Planned/queued decisions are separate from actual execution and terminal evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobState {
    /// Accepted producer definition, not executed.
    Planned,
    /// Awaiting dependency decisions.
    WaitingDependencies,
    /// Ready for resource admission.
    Queued,
    /// One real attempt in an execution phase.
    Executing(Phase),
    /// Failed historical attempt awaiting an explicit next try.
    RetryWait(EventTime),
    /// Immutable terminal decision/result.
    Finished(JobOutcome),
}
impl JobState {
    /// Stable specification state name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Planned => "PLANNED",
            Self::WaitingDependencies => "WAITING_DEPENDENCIES",
            Self::Queued => "QUEUED",
            Self::Executing(p) => p.name(),
            Self::RetryWait(_) => "RETRY_WAIT",
            Self::Finished(o) => match o {
                JobOutcome::Succeeded(_) => "SUCCEEDED",
                JobOutcome::Cached(_) => "CACHED",
                JobOutcome::Failed(_) => "FAILED",
                JobOutcome::Blocked(_) => "BLOCKED",
                JobOutcome::Canceled => "CANCELED",
                JobOutcome::Interrupted => "INTERRUPTED",
            },
        }
    }
}
/// Mutable aggregate with private state. Invalid events leave it completely unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Job {
    id: JobId,
    build: BuildId,
    binding: ExecutionBinding,
    target: OutputTarget,
    fence: Fence,
    retry: RetryPolicy,
    state: JobState,
    attempts: Vec<Attempt>,
    cancel_requested: bool,
    last_event: EventTime,
}
impl Job {
    /// Create an unexecuted planned job bound to exact accepted-plan/source identities.
    pub fn new(
        id: JobId,
        build: BuildId,
        binding: ExecutionBinding,
        target: OutputTarget,
        fence: Fence,
        retry: RetryPolicy,
        at: EventTime,
    ) -> Self {
        Self {
            id,
            build,
            binding,
            target,
            fence,
            retry,
            state: JobState::Planned,
            attempts: Vec::new(),
            cancel_requested: false,
            last_event: at,
        }
    }
    /// Stable job identity.
    pub fn id(&self) -> JobId {
        self.id
    }
    /// Owning build identity.
    pub fn build(&self) -> BuildId {
        self.build
    }
    /// Read-only state; no direct terminal rewrite API exists.
    pub fn state(&self) -> &JobState {
        &self.state
    }
    /// All actual attempts including historical failures; cached/blocked has none.
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }
    /// Immutable source/plan binding.
    pub fn binding(&self) -> ExecutionBinding {
        self.binding
    }
    /// Last supplied transition timestamp.
    pub fn last_event(&self) -> EventTime {
        self.last_event
    }
    /// Current ownership fence.
    pub fn fence(&self) -> Fence {
        self.fence
    }
    /// Independent request flag, not evidence that a process stopped.
    pub fn cancellation_requested(&self) -> bool {
        self.cancel_requested
    }
    fn guard(&self, fence: Fence, at: EventTime) -> Result<(), StateError> {
        if fence != self.fence {
            return Err(StateError::StaleFence);
        }
        if at < self.last_event {
            return Err(StateError::InvalidTime);
        }
        if matches!(self.state, JobState::Finished(_)) {
            return Err(StateError::InvalidTransition);
        }
        Ok(())
    }
    fn pre_execution(&self) -> bool {
        matches!(
            self.state,
            JobState::Planned | JobState::WaitingDependencies | JobState::Queued
        ) && self.attempts.is_empty()
    }
    fn active(&self, id: AttemptId, fence: Fence, at: EventTime) -> Result<&Attempt, StateError> {
        self.guard(fence, at)?;
        let current = self.attempts.last().ok_or(StateError::InvalidTransition)?;
        if current.id != id {
            return Err(StateError::WrongAttempt);
        }
        if current.outcome.is_some() || !matches!(self.state, JobState::Executing(_)) {
            return Err(StateError::InvalidTransition);
        }
        Ok(current)
    }
    /// Record waiting without creating a worker attempt.
    pub fn wait_dependencies(&mut self, fence: Fence, at: EventTime) -> Result<(), StateError> {
        self.guard(fence, at)?;
        if self.state != JobState::Planned {
            return Err(StateError::InvalidTransition);
        }
        if self.cancel_requested {
            return Err(StateError::CancellationPending);
        }
        self.state = JobState::WaitingDependencies;
        self.last_event = at;
        Ok(())
    }
    /// Admit the plan to the ready queue, without inventing execution time.
    pub fn queue(&mut self, fence: Fence, at: EventTime) -> Result<(), StateError> {
        self.guard(fence, at)?;
        if !matches!(
            self.state,
            JobState::Planned | JobState::WaitingDependencies
        ) {
            return Err(StateError::InvalidTransition);
        }
        if self.cancel_requested {
            return Err(StateError::CancellationPending);
        }
        self.state = JobState::Queued;
        self.last_event = at;
        Ok(())
    }
    /// Record reuse after the caller validates its retained version/certificate/integrity.
    pub fn cache(
        &mut self,
        version: VersionId,
        fence: Fence,
        at: EventTime,
    ) -> Result<(), StateError> {
        self.guard(fence, at)?;
        if !self.pre_execution() {
            return Err(StateError::InvalidTransition);
        }
        if self.cancel_requested {
            return Err(StateError::CancellationPending);
        }
        self.state = JobState::Finished(JobOutcome::Cached(version));
        self.last_event = at;
        Ok(())
    }
    /// Record a failed same-build upstream dependency, without a fabricated attempt.
    /// Graph-edge membership remains the accepted planner's responsibility.
    pub fn block(
        &mut self,
        upstream: &Self,
        fence: Fence,
        at: EventTime,
    ) -> Result<(), StateError> {
        self.guard(fence, at)?;
        if !self.pre_execution() {
            return Err(StateError::InvalidTransition);
        }
        if upstream.build != self.build
            || upstream.binding != self.binding
            || upstream.id == self.id
        {
            return Err(StateError::InvalidIdentity);
        }
        if !matches!(&upstream.state,JobState::Finished(result) if !result.successful()) {
            return Err(StateError::InvalidEvidence);
        }
        if at < upstream.last_event {
            return Err(StateError::InvalidTime);
        }
        self.state = JobState::Finished(JobOutcome::Blocked(upstream.id));
        self.last_event = at;
        Ok(())
    }
    /// Begin a real fresh try; retries retain the binding and respect the delay/budget.
    pub fn start_attempt(
        &mut self,
        id: AttemptId,
        fence: Fence,
        at: EventTime,
    ) -> Result<(), StateError> {
        self.guard(fence, at)?;
        match self.state {
            JobState::Queued => {}
            JobState::RetryWait(ready) => {
                if at < ready {
                    return Err(StateError::RetryNotReady);
                }
            }
            _ => return Err(StateError::InvalidTransition),
        }
        if self.cancel_requested {
            return Err(StateError::CancellationPending);
        }
        if self.attempts.len() >= self.retry.max_attempts() as usize {
            return Err(StateError::InvalidTransition);
        }
        if self.attempts.iter().any(|a| a.id == id) {
            return Err(StateError::InvalidIdentity);
        }
        self.attempts.push(Attempt {
            id,
            binding: self.binding,
            fence,
            phase: Phase::Starting,
            started_at: at,
            finished_at: None,
            outcome: None,
            intent: None,
        });
        self.state = JobState::Executing(Phase::Starting);
        self.last_event = at;
        Ok(())
    }
    /// Advance exactly one real execution phase. COMMITTING requires an intent instead.
    pub fn advance(
        &mut self,
        id: AttemptId,
        fence: Fence,
        next: Phase,
        at: EventTime,
    ) -> Result<(), StateError> {
        let current = self.active(id, fence, at)?;
        if current.phase.next() != Some(next) {
            return Err(StateError::InvalidTransition);
        }
        if self.cancel_requested {
            return Err(StateError::CancellationPending);
        }
        self.attempts
            .last_mut()
            .ok_or(StateError::InvalidEvidence)?
            .phase = next;
        self.state = JobState::Executing(next);
        self.last_event = at;
        Ok(())
    }
    /// Finish the old attempt before scheduling an explicit transient retry.
    /// Its terminal state rejects every later message, even while a new attempt is active.
    pub fn fail(
        &mut self,
        id: AttemptId,
        fence: Fence,
        error: FailureEvidence,
        at: EventTime,
    ) -> Result<(), StateError> {
        self.active(id, fence, at)?;
        let next = if !self.cancel_requested && self.retry.permits(error.class, self.attempts.len())
        {
            JobState::RetryWait(EventTime(
                at.0.checked_add(self.retry.delay_ms(self.attempts.len()))
                    .ok_or(StateError::InvalidTime)?,
            ))
        } else {
            JobState::Finished(JobOutcome::Failed(error.clone()))
        };
        let attempt = self
            .attempts
            .last_mut()
            .ok_or(StateError::InvalidEvidence)?;
        attempt.finished_at = Some(at);
        attempt.outcome = Some(AttemptOutcome::Failed(error));
        self.state = next;
        self.last_event = at;
        Ok(())
    }
    /// Request cancellation idempotently; preserve already committed/terminal outcomes.
    pub fn request_cancel(&mut self) -> CancelRequest {
        if matches!(self.state, JobState::Finished(_)) {
            return CancelRequest::TooLate;
        }
        if self.cancel_requested {
            return CancelRequest::AlreadyRequested;
        }
        self.cancel_requested = true;
        CancelRequest::Requested
    }
    /// Record the caller's cleanup acknowledgement. This does not signal or reap processes.
    pub fn finish_canceled(&mut self, fence: Fence, at: EventTime) -> Result<(), StateError> {
        self.guard(fence, at)?;
        if !self.cancel_requested {
            return Err(StateError::InvalidEvidence);
        }
        if let Some(attempt) = self.attempts.last_mut().filter(|a| a.outcome.is_none()) {
            attempt.finished_at = Some(at);
            attempt.outcome = Some(AttemptOutcome::Canceled);
        }
        self.state = JobState::Finished(JobOutcome::Canceled);
        self.last_event = at;
        Ok(())
    }
    /// Seal candidate metadata after output validation. No durable file is installed here.
    pub fn prepare_publication(
        &mut self,
        id: AttemptId,
        fence: Fence,
        version: VersionId,
        artifact: ArtifactDigest,
        at: EventTime,
    ) -> Result<PublicationIntent, StateError> {
        if self.active(id, fence, at)?.phase != Phase::ValidatingOutputs {
            return Err(StateError::InvalidTransition);
        }
        if self.cancel_requested {
            return Err(StateError::CancellationPending);
        }
        if self
            .attempts
            .iter()
            .filter_map(|a| a.intent.as_ref())
            .any(|i| i.version == version)
        {
            return Err(StateError::InvalidIdentity);
        }
        let intent = PublicationIntent {
            job: self.id,
            build: self.build,
            binding: self.binding,
            attempt: id,
            version,
            artifact,
            target: self.target,
            fence,
        };
        let attempt = self
            .attempts
            .last_mut()
            .ok_or(StateError::InvalidEvidence)?;
        attempt.phase = Phase::Committing;
        attempt.intent = Some(intent.clone());
        self.state = JobState::Executing(Phase::Committing);
        self.last_event = at;
        Ok(intent)
    }
    /// Evaluate publication guards against currently observed ownership/head generation.
    /// Storage must atomically persist these guards and the returned success transition.
    pub fn commit(
        &mut self,
        intent: &PublicationIntent,
        observed_fence: Fence,
        observed_head: u64,
        at: EventTime,
    ) -> Result<(), StateError> {
        let attempt = self.active(intent.attempt, observed_fence, at)?;
        if attempt.phase != Phase::Committing || attempt.intent.as_ref() != Some(intent) {
            return Err(StateError::InvalidEvidence);
        }
        if self.cancel_requested {
            return Err(StateError::CancellationPending);
        }
        if observed_head != intent.target.expected_generation {
            return Err(StateError::PublicationConflict);
        }
        let next_head = observed_head
            .checked_add(1)
            .ok_or(StateError::PublicationConflict)?;
        let published = PublicationEvidence {
            intent: Box::new(intent.clone()),
            committed_at: at,
            head_generation: next_head,
        };
        let attempt = self
            .attempts
            .last_mut()
            .ok_or(StateError::InvalidEvidence)?;
        attempt.finished_at = Some(at);
        attempt.outcome = Some(AttemptOutcome::Succeeded(published.clone()));
        self.state = JobState::Finished(JobOutcome::Succeeded(published));
        self.last_event = at;
        Ok(())
    }
    /// Fence out a nonterminal job after ownership changes; committed terminal evidence is immutable.
    /// The caller must establish actual workspace ownership before supplying the new fence.
    pub fn interrupt_for_recovery(
        &mut self,
        new_fence: Fence,
        at: EventTime,
    ) -> Result<(), StateError> {
        self.guard(self.fence, at)?;
        if !new_fence.supersedes(self.fence) {
            return Err(StateError::StaleFence);
        }
        if let Some(attempt) = self.attempts.last_mut().filter(|a| a.outcome.is_none()) {
            attempt.finished_at = Some(at);
            attempt.outcome = Some(AttemptOutcome::Interrupted);
        }
        self.state = JobState::Finished(JobOutcome::Interrupted);
        self.fence = new_fence;
        self.last_event = at;
        Ok(())
    }
}
