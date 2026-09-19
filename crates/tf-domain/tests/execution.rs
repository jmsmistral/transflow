//! Deterministic state-machine properties and publication/retry ordering regressions.
#![allow(clippy::unwrap_used, reason = "Test fixtures must fail loudly")]
use std::{collections::BTreeSet, num::NonZeroU32};
use tf_domain::{execution::*, *};
fn fence() -> Fence {
    Fence {
        session: CoordinatorSessionId::from_bytes([1; 16]),
        generation: 1,
    }
}
fn binding() -> ExecutionBinding {
    ExecutionBinding {
        plan: PlanId::from_bytes([2; 16]),
        source: SourceSnapshotId::from_bytes([3; 16]),
    }
}
fn target() -> OutputTarget {
    OutputTarget {
        dataset: DatasetKey::new(
            WorkspaceId::from_bytes([4; 16]),
            DatasetId::from_bytes([5; 16]),
        ),
        branch: BranchId::from_bytes([6; 16]),
        expected_generation: 7,
    }
}
fn attempt(n: u8) -> AttemptId {
    AttemptId::from_bytes([n; 16])
}
fn version(n: u8) -> VersionId {
    VersionId::from_bytes([n; 16])
}
fn retry() -> RetryPolicy {
    RetryPolicy::new(
        NonZeroU32::new(3).unwrap(),
        BTreeSet::from([FailureClass::TransientIo, FailureClass::WorkerUnavailable]),
    )
    .unwrap()
}
fn job(n: u8, policy: RetryPolicy) -> Job {
    Job::new(
        JobId::from_bytes([n; 16]),
        BuildId::from_bytes([7; 16]),
        binding(),
        target(),
        fence(),
        policy,
        EventTime(0),
    )
}
fn error(class: FailureClass) -> FailureEvidence {
    FailureEvidence {
        class,
        diagnostic: StateError::InvalidEvidence.diagnostic().unwrap(),
    }
}
fn started(policy: RetryPolicy) -> Job {
    let mut j = job(1, policy);
    j.queue(fence(), EventTime(1)).unwrap();
    j.start_attempt(attempt(1), fence(), EventTime(2)).unwrap();
    j
}
const PHASES: [Phase; 6] = [
    Phase::Starting,
    Phase::ValidatingInputs,
    Phase::Running,
    Phase::Materializing,
    Phase::ValidatingOutputs,
    Phase::Committing,
];
fn candidate(j: &mut Job, id: AttemptId, v: VersionId, start: u64) -> PublicationIntent {
    for (offset, phase) in PHASES[1..5].iter().enumerate() {
        j.advance(id, fence(), *phase, EventTime(start + offset as u64 + 1))
            .unwrap();
    }
    j.prepare_publication(
        id,
        fence(),
        v,
        ArtifactDigest::from_bytes([9; 32]),
        EventTime(start + 5),
    )
    .unwrap()
}
fn check(j: &Job) {
    let ids: BTreeSet<_> = j.attempts().iter().map(Attempt::id).collect();
    assert_eq!(ids.len(), j.attempts().len());
    for (i, a) in j.attempts().iter().enumerate() {
        assert_eq!(a.binding(), j.binding());
        assert_eq!(a.outcome().is_some(), a.finished_at().is_some());
        if let Some(end) = a.finished_at() {
            assert!(end >= a.started_at());
            assert!(end <= j.last_event());
        }
        if i + 1 < j.attempts().len() {
            assert!(a.outcome().is_some());
        }
    }
    match j.state() {
        JobState::Executing(p) => {
            let a = j.attempts().last().unwrap();
            assert_eq!(a.phase(), *p);
            assert!(a.outcome().is_none());
        }
        JobState::RetryWait(_) => assert!(matches!(
            j.attempts().last().unwrap().outcome(),
            Some(AttemptOutcome::Failed(_))
        )),
        JobState::Finished(JobOutcome::Cached(_) | JobOutcome::Blocked(_)) => {
            assert!(j.attempts().is_empty())
        }
        JobState::Finished(JobOutcome::Succeeded(p)) => assert!(
            matches!(j.attempts().last().unwrap().outcome(),Some(AttemptOutcome::Succeeded(a)) if a==p)
        ),
        JobState::Finished(_) => assert!(j.attempts().iter().all(|a| a.outcome().is_some())),
        _ => assert!(j.attempts().is_empty()),
    }
}
#[test]
fn every_phase_pair_obeys_the_execution_order_without_partial_mutation() {
    let mut current = started(RetryPolicy::default());
    for (index, phase) in PHASES[..5].iter().enumerate() {
        assert_eq!(current.state(), &JobState::Executing(*phase));
        for proposed in PHASES {
            let mut trial = current.clone();
            let result = trial.advance(attempt(1), fence(), proposed, EventTime(20));
            let allowed = index < 4 && proposed == PHASES[index + 1];
            assert_eq!(result.is_ok(), allowed, "{phase:?} -> {proposed:?}");
            if result.is_err() {
                assert_eq!(trial, current);
            }
            check(&trial);
        }
        if index < 4 {
            current
                .advance(
                    attempt(1),
                    fence(),
                    PHASES[index + 1],
                    EventTime(3 + index as u64),
                )
                .unwrap();
        }
    }
}
#[test]
fn successful_retry_preserves_failed_attempt_and_exact_binding() {
    let mut j = started(retry());
    j.fail(
        attempt(1),
        fence(),
        error(FailureClass::TransientIo),
        EventTime(3),
    )
    .unwrap();
    assert_eq!(j.state(), &JobState::RetryWait(EventTime(1003)));
    let history = j.attempts()[0].clone();
    let before = j.clone();
    assert_eq!(
        j.start_attempt(attempt(2), fence(), EventTime(1002)),
        Err(StateError::RetryNotReady)
    );
    assert_eq!(j, before);
    assert_eq!(
        j.start_attempt(attempt(1), fence(), EventTime(1003)),
        Err(StateError::InvalidIdentity)
    );
    j.start_attempt(attempt(2), fence(), EventTime(1003))
        .unwrap();
    assert_eq!(
        j.advance(attempt(1), fence(), Phase::Running, EventTime(1004)),
        Err(StateError::WrongAttempt)
    );
    let intent = candidate(&mut j, attempt(2), version(2), 1003);
    j.commit(&intent, fence(), 7, EventTime(1009)).unwrap();
    assert_eq!(j.attempts()[0], history);
    assert_eq!(j.attempts()[1].binding(), history.binding());
    assert!(matches!(
        j.state(),
        JobState::Finished(JobOutcome::Succeeded(_))
    ));
    check(&j);
}
#[test]
fn retries_are_opt_in_classified_capped_and_budgeted() {
    assert_eq!(RetryPolicy::default().max_attempts(), 1);
    for class in [
        FailureClass::InputViolation,
        FailureClass::OutputViolation,
        FailureClass::InvalidSchema,
        FailureClass::Import,
        FailureClass::Unsupported,
        FailureClass::PublicationConflict,
        FailureClass::Other,
    ] {
        assert!(RetryPolicy::new(NonZeroU32::new(5).unwrap(), BTreeSet::from([class])).is_err());
        let mut j = started(retry());
        j.fail(attempt(1), fence(), error(class), EventTime(3))
            .unwrap();
        assert!(matches!(
            j.state(),
            JobState::Finished(JobOutcome::Failed(_))
        ));
    }
    let policy = retry();
    assert_eq!(
        (1..=8).map(|n| policy.delay_ms(n)).collect::<Vec<_>>(),
        [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]
    );
    let mut no_retry = started(RetryPolicy::default());
    no_retry
        .fail(
            attempt(1),
            fence(),
            error(FailureClass::TransientIo),
            EventTime(3),
        )
        .unwrap();
    assert_eq!(no_retry.state().name(), "FAILED");
    let mut j = started(retry());
    for (id, at) in [(1, 3), (2, 1004), (3, 3005)] {
        if id > 1 {
            j.start_attempt(attempt(id), fence(), EventTime(at - 1))
                .unwrap();
        }
        j.fail(
            attempt(id),
            fence(),
            error(FailureClass::TransientIo),
            EventTime(at),
        )
        .unwrap();
    }
    assert_eq!(j.state().name(), "FAILED");
    assert_eq!(j.attempts().len(), 3);
}
#[test]
fn cancellation_and_publication_have_both_orderings() {
    let mut cancel_first = started(RetryPolicy::default());
    let intent = candidate(&mut cancel_first, attempt(1), version(1), 2);
    assert_eq!(cancel_first.request_cancel(), CancelRequest::Requested);
    assert_eq!(
        cancel_first.request_cancel(),
        CancelRequest::AlreadyRequested
    );
    let before = cancel_first.clone();
    assert_eq!(
        cancel_first.commit(&intent, fence(), 7, EventTime(8)),
        Err(StateError::CancellationPending)
    );
    assert_eq!(cancel_first, before);
    cancel_first.finish_canceled(fence(), EventTime(9)).unwrap();
    assert_eq!(cancel_first.state().name(), "CANCELED");
    assert!(matches!(
        cancel_first.attempts()[0].outcome(),
        Some(AttemptOutcome::Canceled)
    ));
    let mut commit_first = started(RetryPolicy::default());
    let intent = candidate(&mut commit_first, attempt(1), version(1), 2);
    commit_first
        .commit(&intent, fence(), 7, EventTime(8))
        .unwrap();
    let committed = commit_first.clone();
    assert_eq!(commit_first.request_cancel(), CancelRequest::TooLate);
    assert_eq!(commit_first, committed);
    assert!(commit_first.finish_canceled(fence(), EventTime(9)).is_err());
    assert_eq!(commit_first, committed);
    let mut queued = job(3, RetryPolicy::default());
    assert!(queued.finish_canceled(fence(), EventTime(1)).is_err());
    queued.request_cancel();
    assert!(queued.queue(fence(), EventTime(2)).is_err());
    queued.finish_canceled(fence(), EventTime(3)).unwrap();
    assert!(queued.attempts().is_empty());
}
#[test]
fn publication_fences_head_guards_and_independent_version_ids() {
    let mut j = started(retry());
    let intent = candidate(&mut j, attempt(1), version(1), 2);
    assert_eq!(intent.target(), target());
    assert_eq!(intent.fence(), fence());
    let original = j.clone();
    let newer = Fence {
        generation: 2,
        ..fence()
    };
    assert_eq!(
        j.commit(&intent, newer, 7, EventTime(8)),
        Err(StateError::StaleFence)
    );
    assert_eq!(
        j.commit(&intent, fence(), 8, EventTime(8)),
        Err(StateError::PublicationConflict)
    );
    assert_eq!(j, original);
    j.interrupt_for_recovery(newer, EventTime(9)).unwrap();
    assert_eq!(
        j.commit(&intent, fence(), 7, EventTime(10)),
        Err(StateError::StaleFence)
    );
    assert_eq!(j.state().name(), "INTERRUPTED");
    check(&j);
    let mut first = original;
    first.commit(&intent, fence(), 7, EventTime(8)).unwrap();
    let first_saved = first.clone();
    assert!(first.interrupt_for_recovery(newer, EventTime(9)).is_err());
    assert_eq!(first, first_saved);
    let mut second = started(RetryPolicy::default());
    let other = candidate(&mut second, attempt(1), version(2), 2);
    second.commit(&other, fence(), 7, EventTime(8)).unwrap();
    assert_eq!(intent.artifact(), other.artifact());
    assert_ne!(intent.version(), other.version());
}
#[test]
fn cached_and_blocked_jobs_never_fabricate_attempt_intervals() {
    let mut cached = job(2, RetryPolicy::default());
    cached.cache(version(3), fence(), EventTime(1)).unwrap();
    assert!(cached.attempts().is_empty());
    assert_eq!(
        cached.state(),
        &JobState::Finished(JobOutcome::Cached(version(3)))
    );
    let saved = cached.clone();
    assert!(
        cached
            .start_attempt(attempt(1), fence(), EventTime(2))
            .is_err()
    );
    assert_eq!(cached, saved);
    let mut parent = started(RetryPolicy::default());
    parent
        .fail(
            attempt(1),
            fence(),
            error(FailureClass::Other),
            EventTime(3),
        )
        .unwrap();
    let mut blocked = job(3, RetryPolicy::default());
    blocked.wait_dependencies(fence(), EventTime(1)).unwrap();
    blocked.block(&parent, fence(), EventTime(4)).unwrap();
    assert!(blocked.attempts().is_empty());
    assert_eq!(
        blocked.state(),
        &JobState::Finished(JobOutcome::Blocked(parent.id()))
    );
    let mut ready = job(4, RetryPolicy::default());
    assert!(ready.block(&cached, fence(), EventTime(4)).is_err());
}
#[test]
fn builds_require_exact_terminal_job_membership_and_preserve_partial_results() {
    let mut cached = job(1, RetryPolicy::default());
    cached.cache(version(1), fence(), EventTime(2)).unwrap();
    let mut build = Build::new(
        cached.build(),
        binding(),
        vec![(cached.id(), true)],
        EventTime(0),
    )
    .unwrap();
    assert!(
        build
            .finish(std::slice::from_ref(&cached), EventTime(1))
            .is_err()
    );
    build
        .finish(std::slice::from_ref(&cached), EventTime(2))
        .unwrap();
    assert_eq!(build.state(), BuildState::Succeeded);
    let before = build.clone();
    assert_eq!(build.request_cancel(), CancelRequest::TooLate);
    assert!(build.start(EventTime(3)).is_err());
    assert_eq!(build, before);
    let mut failed = job(2, RetryPolicy::default());
    failed.queue(fence(), EventTime(1)).unwrap();
    failed
        .start_attempt(attempt(2), fence(), EventTime(2))
        .unwrap();
    failed
        .fail(
            attempt(2),
            fence(),
            error(FailureClass::Other),
            EventTime(3),
        )
        .unwrap();
    for required in [true, false] {
        let mut b = Build::new(
            cached.build(),
            binding(),
            vec![(cached.id(), true), (failed.id(), required)],
            EventTime(0),
        )
        .unwrap();
        b.start(EventTime(1)).unwrap();
        let before = b.clone();
        assert!(
            b.finish(std::slice::from_ref(&cached), EventTime(4))
                .is_err()
        );
        assert_eq!(b, before);
        b.request_cancel();
        b.finish(&[cached.clone(), failed.clone()], EventTime(4))
            .unwrap();
        assert_eq!(
            b.state(),
            if required {
                BuildState::Failed
            } else {
                BuildState::Succeeded
            }
        );
        assert_eq!(cached.state().name(), "CACHED");
    }
    assert!(
        Build::new(
            cached.build(),
            binding(),
            vec![(cached.id(), true), (cached.id(), true)],
            EventTime(0)
        )
        .is_err()
    );
    let mut wrong = Build::new(
        cached.build(),
        ExecutionBinding {
            plan: PlanId::from_bytes([99; 16]),
            ..binding()
        },
        vec![(cached.id(), true)],
        EventTime(0),
    )
    .unwrap();
    assert_eq!(
        wrong.finish(&[cached], EventTime(4)),
        Err(StateError::InvalidIdentity)
    );
}
#[test]
fn time_and_counter_overflow_rejections_do_not_mutate() {
    let mut j = started(retry());
    let saved = j.clone();
    assert_eq!(
        j.advance(attempt(1), fence(), Phase::ValidatingInputs, EventTime(1)),
        Err(StateError::InvalidTime)
    );
    assert_eq!(
        j.fail(
            attempt(1),
            fence(),
            error(FailureClass::TransientIo),
            EventTime(u64::MAX)
        ),
        Err(StateError::InvalidTime)
    );
    assert_eq!(j, saved);
    let mut j = Job::new(
        JobId::from_bytes([1; 16]),
        BuildId::from_bytes([7; 16]),
        binding(),
        OutputTarget {
            expected_generation: u64::MAX,
            ..target()
        },
        fence(),
        RetryPolicy::default(),
        EventTime(0),
    );
    j.queue(fence(), EventTime(1)).unwrap();
    j.start_attempt(attempt(1), fence(), EventTime(2)).unwrap();
    let intent = candidate(&mut j, attempt(1), version(1), 2);
    let saved = j.clone();
    assert_eq!(
        j.commit(&intent, fence(), u64::MAX, EventTime(8)),
        Err(StateError::PublicationConflict)
    );
    assert_eq!(j, saved);
    assert!("A".repeat(64).parse::<ArtifactDigest>().is_err());
    assert!("0".repeat(63).parse::<ArtifactDigest>().is_err());
    assert_eq!(
        "ab".repeat(32).parse::<ArtifactDigest>().unwrap().hex(),
        "ab".repeat(32)
    );
}
fn event(j: &mut Job, kind: usize, tick: u64) -> Result<(), StateError> {
    let id = j.attempts().last().map(Attempt::id).unwrap_or(attempt(1));
    let at = EventTime(tick);
    let f = j.fence();
    match kind {
        0 => j.queue(f, at),
        1 => j.wait_dependencies(f, at),
        2 => j.cache(version(1), f, at),
        3 => j.start_attempt(AttemptId::from_bytes((tick as u128).to_be_bytes()), f, at),
        4..=9 => j.advance(id, f, PHASES[kind - 4], at),
        10 => j.fail(id, f, error(FailureClass::TransientIo), at),
        11 => j.fail(id, f, error(FailureClass::OutputViolation), at),
        12 => {
            j.request_cancel();
            Ok(())
        }
        13 => j.finish_canceled(f, at),
        14 => j.interrupt_for_recovery(
            Fence {
                generation: f.generation + 1,
                ..f
            },
            at,
        ),
        15 => j
            .prepare_publication(id, f, version(2), ArtifactDigest::from_bytes([8; 32]), at)
            .map(|_| ()),
        16 => {
            let intent = j
                .attempts()
                .last()
                .and_then(Attempt::intent)
                .ok_or(StateError::InvalidTransition)?
                .clone();
            j.commit(&intent, f, 7, at)
        }
        _ => j.queue(Fence { generation: 0, ..f }, at),
    }
}
#[test]
fn generated_event_histories_preserve_invariants_and_terminal_evidence() {
    let mut seeds = vec![job(1, retry()), started(retry())];
    let mut current = started(retry());
    for (i, p) in PHASES[1..5].iter().enumerate() {
        current
            .advance(attempt(1), fence(), *p, EventTime(3 + i as u64))
            .unwrap();
        seeds.push(current.clone());
    }
    current
        .prepare_publication(
            attempt(1),
            fence(),
            version(1),
            ArtifactDigest::from_bytes([9; 32]),
            EventTime(7),
        )
        .unwrap();
    seeds.push(current);
    let mut operations = 0;
    for (seed_id, seed) in seeds.iter().enumerate() {
        for history in 0..128u64 {
            let mut j = seed.clone();
            let mut random = history + seed_id as u64 * 128 + 1;
            for step in 0..64u64 {
                random = random
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let before = j.clone();
                let result = event(
                    &mut j,
                    (random >> 32) as usize % 18,
                    100_000 + step * 100_000,
                );
                if result.is_err() || matches!(before.state(), JobState::Finished(_)) {
                    assert_eq!(j, before);
                }
                for (old, new) in before.attempts().iter().zip(j.attempts()) {
                    if old.outcome().is_some() {
                        assert_eq!(old, new);
                    }
                }
                check(&j);
                operations += 1;
            }
        }
    }
    assert_eq!(operations, 57_344);
}

#[test]
fn build_completion_rejects_live_or_foreign_work_and_freezes_every_terminal_state() {
    let raw = job(1, RetryPolicy::default());
    let mut build =
        Build::new(raw.build(), binding(), vec![(raw.id(), true)], EventTime(0)).unwrap();
    let before = build.clone();
    assert_eq!(
        build.finish(std::slice::from_ref(&raw), EventTime(1)),
        Err(StateError::InvalidEvidence)
    );
    assert_eq!(build, before);
    let mut foreign = Job::new(
        raw.id(),
        BuildId::from_bytes([99; 16]),
        binding(),
        target(),
        fence(),
        RetryPolicy::default(),
        EventTime(0),
    );
    foreign.cache(version(1), fence(), EventTime(1)).unwrap();
    assert_eq!(
        build.finish(&[foreign], EventTime(2)),
        Err(StateError::InvalidIdentity)
    );
    assert_eq!(build, before);
    let mut outcomes = Vec::new();
    let mut success = started(RetryPolicy::default());
    let intent = candidate(&mut success, attempt(1), version(1), 2);
    success.commit(&intent, fence(), 7, EventTime(8)).unwrap();
    outcomes.push((success, BuildState::Succeeded));
    let mut failed = started(RetryPolicy::default());
    failed
        .fail(
            attempt(1),
            fence(),
            error(FailureClass::Other),
            EventTime(3),
        )
        .unwrap();
    outcomes.push((failed, BuildState::Failed));
    let mut canceled = started(RetryPolicy::default());
    canceled.request_cancel();
    canceled.finish_canceled(fence(), EventTime(3)).unwrap();
    outcomes.push((canceled, BuildState::Canceled));
    let mut interrupted = started(RetryPolicy::default());
    interrupted
        .interrupt_for_recovery(
            Fence {
                generation: 2,
                ..fence()
            },
            EventTime(3),
        )
        .unwrap();
    outcomes.push((interrupted, BuildState::Interrupted));
    for (j, expected) in outcomes {
        let mut b = Build::new(j.build(), binding(), vec![(j.id(), true)], EventTime(0)).unwrap();
        b.start(EventTime(1)).unwrap();
        b.finish(std::slice::from_ref(&j), EventTime(9)).unwrap();
        assert_eq!(b.state(), expected);
        let before = b.clone();
        assert_eq!(b.request_cancel(), CancelRequest::TooLate);
        assert!(b.finish(&[j], EventTime(10)).is_err());
        assert!(b.start(EventTime(10)).is_err());
        assert_eq!(b, before);
    }
    assert_eq!(build.request_cancel(), CancelRequest::Requested);
    assert_eq!(build.request_cancel(), CancelRequest::AlreadyRequested);
    assert_eq!(
        build.start(EventTime(1)),
        Err(StateError::CancellationPending)
    );
}
#[test]
fn candidates_cannot_be_reused_across_jobs_or_retries() {
    let mut first = started(retry());
    let intent = candidate(&mut first, attempt(1), version(1), 2);
    assert_eq!(intent.job(), first.id());
    assert_eq!(intent.build(), first.build());
    assert_eq!(intent.binding(), binding());
    let mut other = job(2, RetryPolicy::default());
    other.queue(fence(), EventTime(1)).unwrap();
    other
        .start_attempt(attempt(1), fence(), EventTime(2))
        .unwrap();
    let other_intent = candidate(&mut other, attempt(1), version(1), 2);
    let saved = other.clone();
    assert_eq!(
        other.commit(&intent, fence(), 7, EventTime(8)),
        Err(StateError::InvalidEvidence)
    );
    assert_eq!(other, saved);
    other
        .commit(&other_intent, fence(), 7, EventTime(8))
        .unwrap();
    first
        .fail(
            attempt(1),
            fence(),
            error(FailureClass::TransientIo),
            EventTime(8),
        )
        .unwrap();
    first
        .start_attempt(attempt(2), fence(), EventTime(1008))
        .unwrap();
    for (i, p) in PHASES[1..5].iter().enumerate() {
        first
            .advance(attempt(2), fence(), *p, EventTime(1009 + i as u64))
            .unwrap();
    }
    let saved = first.clone();
    assert_eq!(
        first.prepare_publication(
            attempt(2),
            fence(),
            version(1),
            ArtifactDigest::from_bytes([9; 32]),
            EventTime(1013)
        ),
        Err(StateError::InvalidIdentity)
    );
    assert_eq!(first, saved);
}

#[test]
fn blocked_evidence_requires_same_plan_and_ordered_time() {
    let mut upstream = started(RetryPolicy::default());
    upstream
        .fail(
            attempt(1),
            fence(),
            error(FailureClass::Other),
            EventTime(10),
        )
        .unwrap();
    let mut dependent = job(2, RetryPolicy::default());
    let before = dependent.clone();
    assert_eq!(
        dependent.block(&upstream, fence(), EventTime(9)),
        Err(StateError::InvalidTime)
    );
    assert_eq!(dependent, before);
    let mut other_binding = binding();
    other_binding.plan = PlanId::from_bytes([99; 16]);
    let mut other = Job::new(
        JobId::from_bytes([3; 16]),
        BuildId::from_bytes([7; 16]),
        other_binding,
        target(),
        fence(),
        RetryPolicy::default(),
        EventTime(0),
    );
    let before = other.clone();
    assert_eq!(
        other.block(&upstream, fence(), EventTime(10)),
        Err(StateError::InvalidIdentity)
    );
    assert_eq!(other, before);
    dependent.block(&upstream, fence(), EventTime(10)).unwrap();
    assert!(dependent.attempts().is_empty());
}
