//! Independent monotonic phase budgets. A worker/query boundary never renews a phase.
use std::{
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tf_domain::resources::{Origin, Setting};

/// Timed work, independent of job admission and transport liveness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// Transform import/setup, function invocation and materialization.
    Transform,
    /// All checks on the exact input bindings of one attempt.
    InputValidation,
    /// All checks on one staged output candidate.
    OutputValidation,
    /// Interactive preview/scratchpad only.
    Interactive,
    /// Captured declaration discovery, using execution-derived policy.
    Discovery,
}
impl Phase {
    fn index(self) -> usize {
        match self {
            Self::Transform => 0,
            Self::InputValidation => 1,
            Self::OutputValidation => 2,
            Self::Interactive => 3,
            Self::Discovery => 4,
        }
    }
    /// Workspace key whose effective setting controls this phase.
    pub fn setting_key(self) -> &'static str {
        match self {
            Self::Transform | Self::Discovery => "execution.wall_timeout_seconds",
            Self::InputValidation | Self::OutputValidation => "validation.timeout_seconds",
            Self::Interactive => "interactive.query_timeout_seconds",
        }
    }
    /// Stable diagnostic phase name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Transform => "transform",
            Self::InputValidation => "input validation",
            Self::OutputValidation => "output validation",
            Self::Interactive => "interactive query",
            Self::Discovery => "discovery",
        }
    }
}
/// Resolved policy frozen by the coordinator. No overall job deadline exists here.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Execution and materialization budget.
    pub transform: Setting,
    /// Independent budget for each input/output validation phase.
    pub validation: Setting,
    /// Interactive-only budget.
    pub interactive: Setting,
    /// Separately identified discovery budget.
    pub discovery: Setting,
}
impl Default for Limits {
    fn default() -> Self {
        let hour = Setting {
            value: 3600,
            origin: Origin::Default,
        };
        Self {
            transform: hour,
            validation: hour,
            interactive: Setting {
                value: 30,
                origin: Origin::Default,
            },
            discovery: hour,
        }
    }
}
impl Limits {
    /// Exact setting for a phase, preserving explicit zero and winning origin.
    pub fn setting(&self, phase: Phase) -> Setting {
        match phase {
            Phase::Transform => self.transform,
            Phase::InputValidation | Phase::OutputValidation => self.validation,
            Phase::Interactive => self.interactive,
            Phase::Discovery => self.discovery,
        }
    }
}
/// Injectable monotonic clock. Implementations must never use calendar time.
pub trait Clock: Send + Sync {
    /// Monotonic elapsed time in this clock's coordinate system.
    fn now(&self) -> Duration;
}
/// Production clock based on std::time::Instant.
pub struct Monotonic(Instant);
impl Default for Monotonic {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for Monotonic {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}
/// Invalid clock movement, concurrent state damage or reuse after final completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TimerError {
    /// The supplied clock moved backwards.
    #[error("Phase timing clock moved backwards")]
    Clock,
    /// The attempt was already closed.
    #[error("This attempt's phase timers are already complete")]
    Complete,
    /// Shared timer state is unavailable.
    #[error("Phase timing state is unavailable")]
    State,
    /// Subject/check context is unbounded or empty.
    #[error("Phase timing requires bounded subject and check context")]
    Context,
}
/// Blocking timeout evidence. Logs and cleanup status are held by the supervisor report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Timeout {
    /// Timed-out phase.
    pub phase: Phase,
    /// Dataset/operation name supplied by accepted coordinator context.
    pub subject: String,
    /// Check name when the caller supplied one; None for a phase-wide timeout.
    pub check: Option<String>,
    /// Actual phase time, excluding time spent in other phases and queue wait.
    pub elapsed: Duration,
    /// Effective seconds and winning origin; zero never produces a timeout.
    pub limit: Setting,
}
impl fmt::Display for Timeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redactor = tf_domain::diagnostic::Redactor::default();
        let subject = redactor.text(&self.subject).map_err(|_| fmt::Error)?;
        write!(
            f,
            "{} timed out for {} after {:.3}s (limit {}s from {:?}); increase {} or set it to 0 to disable this deadline",
            self.phase.name(),
            subject.as_str(),
            self.elapsed.as_secs_f64(),
            self.limit.value,
            self.limit.origin,
            self.phase.setting_key()
        )?;
        if let Some(check) = &self.check {
            let name = redactor.text(check).map_err(|_| fmt::Error)?;
            write!(f, "; check {}", name.as_str())?;
        }
        Ok(())
    }
}
impl std::error::Error for Timeout {}
#[derive(Clone, Default)]
struct Timer {
    elapsed: Duration,
}
struct State {
    timers: [Timer; 5],
    active: Option<(Phase, Duration)>,
    last: Duration,
    complete: bool,
    subject: String,
    check: Option<String>,
}
/// One attempt's shared phase ledger, reused by successive validation helpers.
/// Clones share deadlines; they do not create new budgets. Worker admission must
/// prevent simultaneous work against this ledger.
#[derive(Clone)]
pub struct Budget {
    limits: Limits,
    clock: Arc<dyn Clock>,
    state: Arc<Mutex<State>>,
}
impl Budget {
    /// Create an unstarted budget. Admission/queue wait consumes no phase time.
    pub fn new(limits: Limits, subject: String) -> Result<Self, TimerError> {
        Self::with_clock(limits, subject, Arc::new(Monotonic::default()))
    }
    /// Inject a monotonic clock for deterministic hour-scale qualification.
    pub fn with_clock(
        limits: Limits,
        subject: String,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, TimerError> {
        if subject.is_empty() || subject.len() > 1024 {
            return Err(TimerError::Context);
        }
        let now = clock.now();
        Ok(Self {
            limits,
            clock,
            state: Arc::new(Mutex::new(State {
                timers: std::array::from_fn(|_| Timer::default()),
                active: None,
                last: now,
                complete: false,
                subject,
                check: None,
            })),
        })
    }
    fn with_state<T>(
        &self,
        f: impl FnOnce(&mut State, Duration) -> Result<T, TimerError>,
    ) -> Result<T, TimerError> {
        let mut state = self.state.lock().map_err(|_| TimerError::State)?;
        let now = self.clock.now();
        if now < state.last {
            return Err(TimerError::Clock);
        }
        state.last = now;
        if state.complete {
            return Err(TimerError::Complete);
        }
        f(&mut state, now)
    }
    /// Enter/resume a phase. Repeated entry (including a new query/helper) never resets it.
    /// Execution time accumulates across input checks, so setup and materialization share one limit.
    pub fn enter(&self, phase: Phase) -> Result<(), TimerError> {
        self.with_state(|state, now| {
            if state.active.is_some_and(|(old, _)| old == phase) {
                return Ok(());
            }
            if let Some((old, start)) = state.active {
                state.timers[old.index()].elapsed = state.timers[old.index()]
                    .elapsed
                    .saturating_add(now - start);
            }
            state.active = Some((phase, now));
            state.check = None;
            Ok(())
        })
    }
    /// Update the current check label without modifying any phase deadline.
    pub fn check(&self, name: Option<String>) -> Result<(), TimerError> {
        if name
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 1024)
        {
            return Err(TimerError::Context);
        }
        self.with_state(|state, _| {
            state.check = name;
            Ok(())
        })
    }
    /// Current timeout evidence; zero disables only the selected phase.
    pub fn expired(&self) -> Result<Option<Timeout>, TimerError> {
        self.with_state(|state, now| {
            let Some((phase, start)) = state.active else {
                return Ok(None);
            };
            let elapsed = state.timers[phase.index()]
                .elapsed
                .saturating_add(now - start);
            let limit = self.limits.setting(phase);
            Ok(
                (limit.value > 0 && elapsed >= Duration::from_secs(limit.value)).then(|| Timeout {
                    phase,
                    subject: state.subject.clone(),
                    check: state.check.clone(),
                    elapsed,
                    limit,
                }),
            )
        })
    }
    /// Phase timings so far; queue and termination grace are excluded by caller boundaries.
    pub fn elapsed(&self, phase: Phase) -> Result<Duration, TimerError> {
        self.with_state(|state, now| {
            Ok(state.timers[phase.index()].elapsed.saturating_add(
                state
                    .active
                    .filter(|(p, _)| *p == phase)
                    .map(|(_, start)| now - start)
                    .unwrap_or_default(),
            ))
        })
    }
    /// Freeze all timings at actual attempt completion; completed budgets cannot be reused.
    pub fn finish(&self) -> Result<[Duration; 5], TimerError> {
        self.with_state(|state, now| {
            if let Some((phase, start)) = state.active.take() {
                state.timers[phase.index()].elapsed = state.timers[phase.index()]
                    .elapsed
                    .saturating_add(now - start);
            }
            state.complete = true;
            Ok(std::array::from_fn(|i| state.timers[i].elapsed))
        })
    }
}

/// One managed worker's role within a shared attempt budget.
#[derive(Clone)]
pub struct Work {
    /// Attempt-wide shared timers. Query/helper launches reuse this value.
    pub budget: Budget,
    /// Coordinator-selected initial phase; workers cannot choose their own budget.
    pub phase: Phase,
}
impl Work {
    /// Validate initial phase against the requested operation before any child launch.
    pub fn valid_for(&self, operation: tf_protocol::Operation) -> bool {
        use tf_protocol::Operation;
        matches!(
            (operation, self.phase),
            (Operation::Discover, Phase::Discovery)
                | (Operation::Execute, Phase::Transform)
                | (
                    Operation::EvaluateChecks,
                    Phase::InputValidation | Phase::OutputValidation
                )
                | (
                    Operation::QueryPreview | Operation::InferLineage,
                    Phase::Interactive
                )
                | (Operation::InspectEnvironment, Phase::Discovery)
        )
    }
    /// Apply an already schema/session-validated phase message. No query renews a timer.
    pub fn message(&self, operation: tf_protocol::Operation, name: &str) -> Result<(), TimerError> {
        let phase = if operation == tf_protocol::Operation::Execute {
            match name {
                "setup" | "running" | "materializing" => Phase::Transform,
                "validating_inputs" => Phase::InputValidation,
                "validating_outputs" => Phase::OutputValidation,
                _ => return Err(TimerError::State),
            }
        } else {
            if operation == tf_protocol::Operation::EvaluateChecks
                && name != "setup"
                && !matches!(
                    (self.phase, name),
                    (Phase::InputValidation, "validating_inputs")
                        | (Phase::OutputValidation, "validating_outputs")
                )
            {
                return Err(TimerError::State);
            }
            self.phase
        };
        self.budget.enter(phase)
    }
}
/// Known cleanup interval, excluded from the active phase wall budget on drop.
/// It is held by the process owner, never by the disappearing async receiver.
pub(crate) struct Cleanup {
    budget: Budget,
    started: Duration,
}
impl Budget {
    pub(crate) fn cleanup(&self) -> Cleanup {
        Cleanup {
            budget: self.clone(),
            started: self.clock.now(),
        }
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let now = self.budget.clock.now();
        if let Ok(mut state) = self.budget.state.lock() {
            // Keep the cleanup boundary in the monotonic history too. Otherwise a
            // regressing injected clock could precede the shifted active start.
            state.last = state.last.max(now);
            if now >= self.started
                && let Some((_, start)) = &mut state.active
            {
                *start = start.saturating_add(now - self.started);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Manual(AtomicU64);
    impl Clock for Manual {
        fn now(&self) -> Duration {
            Duration::from_secs(self.0.load(Ordering::SeqCst))
        }
    }
    #[test]
    fn cleanup_grace_does_not_consume_a_shared_helper_budget() -> Result<(), TimerError> {
        let clock = Arc::new(Manual(AtomicU64::new(0)));
        let budget = Budget::with_clock(Limits::default(), "fixture".into(), clock.clone())?;
        clock.0.store(10000, Ordering::SeqCst);
        budget.enter(Phase::InputValidation)?;
        clock.0.store(13599, Ordering::SeqCst);
        let cleanup = budget.cleanup();
        clock.0.store(13609, Ordering::SeqCst);
        drop(cleanup);
        clock.0.store(10001, Ordering::SeqCst);
        assert_eq!(budget.expired(), Err(TimerError::Clock));
        clock.0.store(13609, Ordering::SeqCst);
        budget.enter(Phase::InputValidation)?;
        assert!(budget.expired()?.is_none());
        assert_eq!(
            budget.elapsed(Phase::InputValidation)?,
            Duration::from_secs(3599)
        );
        clock.0.store(13610, Ordering::SeqCst);
        assert!(budget.expired()?.is_some());
        Ok(())
    }
}
