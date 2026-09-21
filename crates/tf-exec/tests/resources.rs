//! Atomic admission and hour-scale phase budgets without wall-clock sleeps.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Synthetic fixture assertions.
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tf_domain::resources::{Origin, Setting};
use tf_exec::{
    admission::{Admission, Capacity, Demand, Error, Request, Reservation},
    timing::{Budget, Clock, Limits, Phase, TimerError},
};
fn poll(request: &mut Request) -> Poll<Result<Reservation, Error>> {
    Pin::new(request).poll(&mut Context::from_waker(Waker::noop()))
}
#[allow(clippy::panic)] // A pending fixture acquisition is a failed test assertion.
fn take(mut request: Request) -> Reservation {
    match poll(&mut request) {
        Poll::Ready(Ok(r)) => r,
        _ => panic!("fixture requires immediately available resources"),
    }
}
#[derive(Default)]
struct Manual(AtomicU64);
impl Clock for Manual {
    fn now(&self) -> Duration {
        Duration::from_secs(self.0.load(Ordering::SeqCst))
    }
}
impl Manual {
    fn set(&self, value: u64) {
        self.0.store(value, Ordering::SeqCst);
    }
}
fn budget(limits: Limits) -> (Budget, Arc<Manual>) {
    let clock = Arc::new(Manual::default());
    (
        Budget::with_clock(limits, "curated/orders".into(), clock.clone()).unwrap(),
        clock,
    )
}
#[test]
fn admission_is_atomic_fifo_and_releases_every_counter() {
    let pool = Admission::new(Capacity {
        jobs: 2,
        cpu: 4,
        memory_bytes: Some(100),
        disk_bytes: Some(100),
        queue: 4,
    })
    .unwrap();
    let demand = Demand {
        cpu: 2,
        memory_bytes: Some(40),
        disk_bytes: Some(60),
        ..Demand::default()
    };
    let first = take(pool.request(demand).unwrap());
    let mut second = pool.request(demand).unwrap();
    assert!(poll(&mut second).is_pending()); // Disk, with spare job/CPU/memory, blocks the whole request.
    let mut third = pool
        .request(Demand {
            cpu: 1,
            memory_bytes: Some(1),
            disk_bytes: Some(1),
            ..Demand::default()
        })
        .unwrap();
    assert!(poll(&mut third).is_pending()); // Cannot pass the FIFO head.
    assert_eq!(
        (
            pool.usage().unwrap().jobs,
            pool.usage().unwrap().cpu,
            pool.usage().unwrap().memory_bytes,
            pool.usage().unwrap().disk_bytes
        ),
        (1, 2, 40, 60)
    );
    drop(second);
    let third = take(third);
    assert_eq!(pool.usage().unwrap().jobs, 2);
    drop(first);
    drop(third);
    assert_eq!(pool.usage().unwrap(), Default::default());
}
#[test]
fn validation_helpers_reuse_full_pool_and_cannot_double_spend_it() {
    let pool = Admission::new(Capacity {
        jobs: 2,
        cpu: 2,
        ..Capacity::default()
    })
    .unwrap();
    let jobs = [
        take(pool.request(Demand::default()).unwrap()),
        take(pool.request(Demand::default()).unwrap()),
    ];
    let mut blocked = pool.request(Demand::default()).unwrap();
    for job in &jobs {
        let producer = job.worker().unwrap();
        assert!(matches!(job.worker(), Err(Error::WorkerActive)));
        assert!(poll(&mut blocked).is_pending());
        drop(producer);
        let helper = job.worker().unwrap(); // No extra max_jobs/CPU acquisition.
        assert_eq!(helper.threads(), 1);
        assert_eq!(pool.usage().unwrap().jobs, 2);
        drop(helper);
    }
    drop(jobs);
    let last = take(blocked);
    drop(last);
    assert_eq!(pool.usage().unwrap(), Default::default());
}
#[test]
fn worker_ownership_holds_capacity_after_job_caller_is_dropped() {
    let pool = Admission::new(Capacity {
        jobs: 1,
        cpu: 1,
        ..Capacity::default()
    })
    .unwrap();
    let job = take(pool.request(Demand::default()).unwrap());
    let worker = job.worker().unwrap();
    drop(job);
    let mut next = pool.request(Demand::default()).unwrap();
    assert!(poll(&mut next).is_pending());
    drop(worker);
    let next = take(next);
    drop(next);
    assert_eq!(pool.usage().unwrap(), Default::default());
}
#[test]
fn memory_is_opt_in_and_never_a_hidden_hard_limit() {
    let pool = Admission::new(Capacity {
        jobs: 2,
        cpu: 2,
        ..Capacity::default()
    })
    .unwrap();
    let demand = Demand {
        memory_bytes: Some(u64::MAX),
        ..Demand::default()
    };
    let a = take(pool.request(demand).unwrap());
    let b = take(pool.request(demand).unwrap());
    assert_eq!(pool.usage().unwrap().memory_bytes, 0);
    assert_eq!(a.worker().unwrap().memory_reservation(), None);
    drop(a);
    drop(b);
    let limited = Admission::new(Capacity {
        memory_bytes: Some(64),
        ..Capacity::default()
    })
    .unwrap();
    assert!(matches!(
        limited.request(Demand::default()),
        Err(Error::MissingEstimate)
    ));
    assert!(matches!(
        limited.request(demand),
        Err(Error::ExceedsCapacity)
    ));
    assert!(matches!(
        pool.request(Demand {
            hard_memory_bytes: Some(1),
            ..Demand::default()
        }),
        Err(Error::UnsupportedHardLimit)
    ));
    assert_eq!(limited.usage().unwrap(), Default::default());
}
#[test]
fn bounded_queue_cancellation_and_cpu_limits_are_explicit() {
    let pool = Admission::new(Capacity {
        jobs: 2,
        cpu: 8,
        queue: 1,
        ..Capacity::default()
    })
    .unwrap();
    assert_eq!(pool.default_demand().cpu, 4);
    let first = pool.request(pool.default_demand()).unwrap();
    assert!(matches!(
        pool.request(Demand::default()),
        Err(Error::QueueFull)
    ));
    drop(first);
    assert!(matches!(
        pool.request(Demand {
            cpu: 9,
            ..Demand::default()
        }),
        Err(Error::ExceedsCapacity)
    ));
    assert!(matches!(
        pool.request(Demand {
            cpu: 0,
            ..Demand::default()
        }),
        Err(Error::Invalid)
    ));
    let r = take(
        pool.request(Demand {
            cpu: 8,
            ..Demand::default()
        })
        .unwrap(),
    );
    let mut next = pool.request(Demand::default()).unwrap();
    assert!(poll(&mut next).is_pending());
    drop(r);
    drop(take(next));
    assert_eq!(pool.usage().unwrap(), Default::default());
}
#[test]
fn shared_validation_queries_do_not_renew_the_hour_or_inherit_interactive_timeout() {
    let (job, clock) = budget(Limits::default());
    clock.set(10000); // Queue wait is excluded.
    job.enter(Phase::InputValidation).unwrap();
    clock.set(10301);
    assert!(job.expired().unwrap().is_none());
    job.check(Some("order_key".into())).unwrap();
    job.clone().enter(Phase::InputValidation).unwrap(); // New helper/query, same phase.
    clock.set(13599);
    assert!(job.expired().unwrap().is_none());
    clock.set(13600);
    let timeout = job.expired().unwrap().unwrap();
    assert_eq!(timeout.phase, Phase::InputValidation);
    assert_eq!(timeout.elapsed, Duration::from_secs(3600));
    assert_eq!(timeout.check.as_deref(), Some("order_key"));
    assert_eq!(timeout.limit.origin, Origin::Default);
    assert!(timeout.to_string().contains("validation.timeout_seconds"));
    let (query, qclock) = budget(Limits::default());
    query.enter(Phase::Interactive).unwrap();
    qclock.set(30);
    assert_eq!(query.expired().unwrap().unwrap().phase, Phase::Interactive);
}
#[test]
fn compute_setup_materialization_and_each_validation_phase_have_separate_budgets() {
    let (job, clock) = budget(Limits::default());
    job.enter(Phase::Transform).unwrap();
    clock.set(3500);
    job.enter(Phase::InputValidation).unwrap();
    clock.set(7099);
    assert!(job.expired().unwrap().is_none());
    job.enter(Phase::Transform).unwrap();
    clock.set(7198);
    assert!(job.expired().unwrap().is_none());
    assert_eq!(
        job.elapsed(Phase::Transform).unwrap(),
        Duration::from_secs(3599)
    );
    job.enter(Phase::OutputValidation).unwrap();
    clock.set(10797);
    assert!(job.expired().unwrap().is_none());
    clock.set(10798);
    assert_eq!(
        job.expired().unwrap().unwrap().phase,
        Phase::OutputValidation
    );
}
#[test]
fn disabled_and_overridden_limits_preserve_provenance_and_fail_on_clock_regression() {
    let limits = Limits {
        transform: Setting {
            value: 0,
            origin: Origin::Explicit,
        },
        validation: Setting {
            value: 7,
            origin: Origin::Build,
        },
        ..Limits::default()
    };
    let (job, clock) = budget(limits);
    job.enter(Phase::Transform).unwrap();
    clock.set(100000);
    assert!(job.expired().unwrap().is_none());
    job.enter(Phase::InputValidation).unwrap();
    clock.set(100007);
    let timeout = job.expired().unwrap().unwrap();
    assert_eq!(
        timeout.limit,
        Setting {
            value: 7,
            origin: Origin::Build
        }
    );
    clock.set(1);
    assert_eq!(job.expired().unwrap_err(), TimerError::Clock);
    clock.set(100007);
    job.finish().unwrap();
    assert_eq!(
        job.enter(Phase::Transform).unwrap_err(),
        TimerError::Complete
    );
}

#[test]
fn permit_release_wakes_an_async_waiter_without_repolling_loops() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let pool = Admission::new(Capacity {
            jobs: 1,
            ..Default::default()
        })
        .unwrap();
        let held = pool.request(Demand::default()).unwrap().await.unwrap();
        let pending = pool.request(Demand::default()).unwrap();
        let waiter = tokio::spawn(pending);
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        drop(held);
        let acquired = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(acquired);
        assert_eq!(pool.usage().unwrap(), Default::default());
    });
}

#[test]
fn operation_phase_mapping_cannot_switch_a_validation_helpers_budget() {
    use tf_exec::timing::Work;
    use tf_protocol::Operation;
    let (budget, clock) = budget(Limits::default());
    let work = Work {
        budget: budget.clone(),
        phase: Phase::Transform,
    };
    assert!(work.valid_for(Operation::Execute));
    assert!(!work.valid_for(Operation::EvaluateChecks));
    work.message(Operation::Execute, "setup").unwrap();
    clock.set(10);
    work.message(Operation::Execute, "validating_inputs")
        .unwrap();
    clock.set(50);
    work.message(Operation::Execute, "running").unwrap();
    clock.set(60);
    work.message(Operation::Execute, "materializing").unwrap();
    clock.set(70);
    work.message(Operation::Execute, "validating_outputs")
        .unwrap();
    assert_eq!(
        budget.elapsed(Phase::Transform).unwrap(),
        Duration::from_secs(30)
    );
    assert_eq!(
        budget.elapsed(Phase::InputValidation).unwrap(),
        Duration::from_secs(40)
    );
    let helper = Work {
        budget,
        phase: Phase::InputValidation,
    };
    assert!(helper.valid_for(Operation::EvaluateChecks));
    assert_eq!(
        helper.message(Operation::EvaluateChecks, "validating_outputs"),
        Err(TimerError::State)
    );
}
