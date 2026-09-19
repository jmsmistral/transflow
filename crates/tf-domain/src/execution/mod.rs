//! Pure execution state. Callers must persist transitions before exposing them.
//! No process control, clock, database transaction or artifact I/O is performed here.
mod build;
mod job;
use crate::diagnostic::{Diagnostic, DiagnosticCode, DiagnosticError, Redactor};
use crate::{BranchId, CoordinatorSessionId, DatasetKey, PlanId, SourceSnapshotId};
pub use build::{Build, BuildState};
pub use job::{
    Attempt, AttemptOutcome, Job, JobOutcome, JobState, PublicationEvidence, PublicationIntent,
};
use std::{collections::BTreeSet, fmt, num::NonZeroU32, str::FromStr};

/// Domain transition failure; no caller-supplied data is interpolated in its text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateError {
    /// Current state does not permit this event.
    InvalidTransition,
    /// Identity was already used or the expected job set does not match.
    InvalidIdentity,
    /// Caller evidence is incomplete or inconsistent.
    InvalidEvidence,
    /// Event time moved backwards or overflowed.
    InvalidTime,
    /// An obsolete coordinator/reservation attempted mutation.
    StaleFence,
    /// An event belongs to a different execution attempt.
    WrongAttempt,
    /// Retry delay has not elapsed.
    RetryNotReady,
    /// A cancellation request won before publication.
    CancellationPending,
    /// The branch head generation changed or cannot advance.
    PublicationConflict,
}
impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidTransition => "The current state does not permit this operation",
            Self::InvalidIdentity => {
                "The supplied execution identities do not match the recorded work"
            }
            Self::InvalidEvidence => "The operation requires complete, consistent evidence",
            Self::InvalidTime => "Execution event times must be ordered and representable",
            Self::StaleFence => "The coordinator no longer owns this execution reservation",
            Self::WrongAttempt => "The event belongs to a different execution attempt",
            Self::RetryNotReady => "The configured retry delay has not elapsed",
            Self::CancellationPending => {
                "Cancellation was requested before publication could commit"
            }
            Self::PublicationConflict => {
                "The output branch changed before publication could commit"
            }
        })
    }
}
impl std::error::Error for StateError {}
impl StateError {
    /// Shared human-first diagnostic; service context belongs in its result envelope.
    pub fn diagnostic(self) -> Result<Diagnostic, DiagnosticError> {
        let r = Redactor::default();
        Ok(Diagnostic::new(DiagnosticCode::StateTransition,r.text("Execution state could not be updated")?,r.text(&self.to_string())?,r.text("Refresh the recorded execution state and its ownership before issuing another operation.")?))
    }
}
/// Millisecond ticks on one caller-selected timeline. No timestamp is fabricated.
/// Callers reconcile clock changes before passing nondecreasing times per entity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EventTime(pub u64);
/// Exact coordinator session and reservation generation, checked on every attempt event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fence {
    /// Owning coordinator session.
    pub session: CoordinatorSessionId,
    /// Reservation generation within that session.
    pub generation: u64,
}
impl Fence {
    pub(crate) fn supersedes(self, old: Self) -> bool {
        self.session != old.session || self.generation > old.generation
    }
}
/// Immutable retry binding. The referenced accepted plan owns exact parameters/inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionBinding {
    /// Accepted immutable plan identity (complete selection belongs to the planner).
    pub plan: PlanId,
    /// Captured source identity shared by every retry.
    pub source: SourceSnapshotId,
}
/// Output reservation and head compare-and-swap guard, fixed when the job is accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputTarget {
    /// Owner-qualified dataset.
    pub dataset: DatasetKey,
    /// Destination data branch identity.
    pub branch: BranchId,
    /// Expected branch head generation at publication.
    pub expected_generation: u64,
}
/// Physical artifact hash carrier, distinct from publication UUIDs and compute keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactDigest([u8; 32]);
impl ArtifactDigest {
    /// Bind an already computed/verified hash; this does not inspect any file.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    /// Lowercase SHA-256 representation.
    pub fn hex(self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
impl FromStr for ArtifactDigest {
    type Err = StateError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 64
            || !s
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(StateError::InvalidEvidence);
        }
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(s.as_bytes().as_chunks::<2>().0) {
            let text = std::str::from_utf8(pair).map_err(|_| StateError::InvalidEvidence)?;
            *byte = u8::from_str_radix(text, 16).map_err(|_| StateError::InvalidEvidence)?;
        }
        Ok(Self(bytes))
    }
}
/// Real execution phases. Queue/blocked/cache/retry decisions are never attempt phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// Launching the managed worker.
    Starting,
    /// Checking exact bound inputs.
    ValidatingInputs,
    /// Calling user code.
    Running,
    /// Materializing one candidate.
    Materializing,
    /// Checking the candidate's exact bytes.
    ValidatingOutputs,
    /// Guarded publication intent, before the visibility transaction.
    Committing,
}
impl Phase {
    /// Stable phase names used by future persistence/protocol projections.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::ValidatingInputs => "VALIDATING_INPUTS",
            Self::Running => "RUNNING",
            Self::Materializing => "MATERIALIZING",
            Self::ValidatingOutputs => "VALIDATING_OUTPUTS",
            Self::Committing => "COMMITTING",
        }
    }
    pub(crate) fn next(self) -> Option<Self> {
        match self {
            Self::Starting => Some(Self::ValidatingInputs),
            Self::ValidatingInputs => Some(Self::Running),
            Self::Running => Some(Self::Materializing),
            Self::Materializing => Some(Self::ValidatingOutputs),
            _ => None,
        }
    }
}
/// Failure classification independent of a human diagnostic or WARN data policy.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FailureClass {
    /// Explicit transient process-launch availability failure.
    WorkerUnavailable,
    /// Explicit transient I/O failure after the old attempt is fenced out.
    TransientIo,
    /// Deterministic input violation.
    InputViolation,
    /// Deterministic output violation.
    OutputViolation,
    /// Invalid schema or data conversion.
    InvalidSchema,
    /// Import/syntax/definition failure.
    Import,
    /// Unsupported required operation.
    Unsupported,
    /// Branch-head publication conflict.
    PublicationConflict,
    /// Unknown/evaluator/other failure, not implicitly transient.
    Other,
}
impl FailureClass {
    fn transient(self) -> bool {
        matches!(self, Self::WorkerUnavailable | Self::TransientIo)
    }
}
/// Safe immutable failure evidence retained even after a later retry succeeds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailureEvidence {
    /// Explicit retry classification.
    pub class: FailureClass,
    /// Sanitized explanation and remediation.
    pub diagnostic: Diagnostic,
}
/// Explicit opt-in retry policy; defaults to one total attempt and no retry classes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    max_attempts: NonZeroU32,
    classes: BTreeSet<FailureClass>,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: NonZeroU32::MIN,
            classes: BTreeSet::new(),
        }
    }
}
impl RetryPolicy {
    /// Only approved transient classes can be retried. Total includes the first try.
    pub fn new(
        max_attempts: NonZeroU32,
        classes: BTreeSet<FailureClass>,
    ) -> Result<Self, StateError> {
        if classes.iter().any(|c| !c.transient()) {
            return Err(StateError::InvalidEvidence);
        }
        Ok(Self {
            max_attempts,
            classes,
        })
    }
    /// Maximum total attempts, including the first.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts.get()
    }
    pub(crate) fn permits(&self, class: FailureClass, count: usize) -> bool {
        count < self.max_attempts.get() as usize && self.classes.contains(&class)
    }
    /// Deterministic 1-second exponential delay capped at 30 seconds, after count tries.
    /// Jitter is intentionally absent; a future policy must record/inject it explicitly.
    pub fn delay_ms(&self, count: usize) -> u64 {
        (1000u64 << count.saturating_sub(1).min(5)).min(30_000)
    }
}
/// Idempotent cancellation-request result; a request is not proof of process cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelRequest {
    /// First recorded request.
    Requested,
    /// Already requested; no mutation.
    AlreadyRequested,
    /// Terminal evidence already won; no mutation.
    TooLate,
}
