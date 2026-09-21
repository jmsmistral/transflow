//! Atomic, bounded FIFO admission. No lock is held during subprocess work.
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
};

/// Workspace-wide capacities, resolved once by the coordinator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capacity {
    /// Maximum active producer jobs and independently admitted query helpers.
    pub jobs: u32,
    /// Total CPU tokens, resolved from explicit policy or logical CPU availability.
    pub cpu: u32,
    /// Optional estimated-memory gate in bytes. None imposes no memory reservation.
    pub memory_bytes: Option<u64>,
    /// Optional staging/spill reservation budget; not a filesystem quota.
    pub disk_bytes: Option<u64>,
    /// Explicit bound on pending in-memory admission requests.
    pub queue: usize,
}
impl Default for Capacity {
    fn default() -> Self {
        Self {
            jobs: 2,
            cpu: 1,
            memory_bytes: None,
            disk_bytes: None,
            queue: 1024,
        }
    }
}
/// One atomic request. Admission never partially acquires these resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Demand {
    /// Requested threads/tokens; positive and no larger than the workspace pool.
    pub cpu: u32,
    /// Explicit estimate required when a memory admission budget is configured.
    pub memory_bytes: Option<u64>,
    /// Explicit estimate required when a disk reservation budget is configured.
    pub disk_bytes: Option<u64>,
    /// Unsupported on every current platform; never approximated by a reservation.
    pub hard_memory_bytes: Option<u64>,
}
impl Default for Demand {
    fn default() -> Self {
        Self {
            cpu: 1,
            memory_bytes: None,
            disk_bytes: None,
            hard_memory_bytes: None,
        }
    }
}
/// Recoverable admission errors; no failed request consumes a partial permit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    /// Invalid capacity or request values.
    #[error("Resource capacities and CPU requests must be positive")]
    Invalid,
    /// This request cannot ever fit in this workspace's configured capacity.
    #[error("Resource request exceeds the workspace capacity")]
    ExceedsCapacity,
    /// An optional admission gate was enabled without a matching job estimate.
    #[error("An explicit memory or disk budget requires a resource estimate")]
    MissingEstimate,
    /// Hard process memory enforcement is not supported by this backend.
    #[error("Hard process memory limits are unavailable; admission estimates are not RSS limits")]
    UnsupportedHardLimit,
    /// Finite queue is full; caller may retain work in its durable queue and retry later.
    #[error("Resource admission queue is full")]
    QueueFull,
    /// Parent worker/helper already occupies this reservation.
    #[error(
        "The job reservation already has an active worker; finish or transfer it before starting a helper"
    )]
    WorkerActive,
    /// Internal state is unavailable; no admission is authorized.
    #[error("Resource admission state is unavailable")]
    State,
}
/// Resource usage snapshot. Memory/disk are zero when their optional gates are absent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    /// Active reservations, including a dropped caller whose subprocess still runs.
    pub jobs: u32,
    /// Reserved CPU tokens.
    pub cpu: u32,
    /// Estimated memory reserved under an explicit budget.
    pub memory_bytes: u64,
    /// Staging/spill bytes reserved under an explicit budget.
    pub disk_bytes: u64,
    /// Bounded requests waiting for atomic admission.
    pub queued: usize,
}
struct Waiter {
    id: u64,
    demand: Demand,
    waker: Option<Waker>,
}
struct State {
    usage: Usage,
    queue: VecDeque<Waiter>,
    next: u64,
}
struct Inner {
    capacity: Capacity,
    state: Mutex<State>,
}
/// One coordinator-owned pool. Share clones within that coordinator, never create
/// a separate pool per job. Runtime ownership and durable queues remain caller-owned.
#[derive(Clone)]
pub struct Admission(Arc<Inner>);
fn wake(state: &State) -> Vec<Waker> {
    state.queue.iter().filter_map(|w| w.waker.clone()).collect()
}
fn notify(wakers: Vec<Waker>) {
    for waker in wakers {
        waker.wake();
    }
}
impl Admission {
    /// Resolve a finite pool. There is deliberately no implicit memory budget.
    pub fn new(capacity: Capacity) -> Result<Self, Error> {
        if capacity.jobs == 0
            || capacity.cpu == 0
            || capacity.queue == 0
            || capacity.memory_bytes == Some(0)
            || capacity.disk_bytes == Some(0)
        {
            return Err(Error::Invalid);
        }
        Ok(Self(Arc::new(Inner {
            capacity,
            state: Mutex::new(State {
                usage: Usage::default(),
                queue: VecDeque::new(),
                next: 0,
            }),
        })))
    }
    /// Default per-job threads divide the available pool across concurrent slots.
    pub fn default_demand(&self) -> Demand {
        Demand {
            cpu: (self.0.capacity.cpu / self.0.capacity.jobs.min(self.0.capacity.cpu)).max(1),
            ..Demand::default()
        }
    }
    /// Read exact in-memory reservations, including requests that have not yet been polled.
    pub fn usage(&self) -> Result<Usage, Error> {
        let state = self.0.state.lock().map_err(|_| Error::State)?;
        Ok(Usage {
            queued: state.queue.len(),
            ..state.usage
        })
    }
    /// Register one FIFO request. Dropping its future removes it and wakes successors.
    /// This queue intentionally allows head-of-line blocking rather than starving large jobs.
    pub fn request(&self, demand: Demand) -> Result<Request, Error> {
        if demand.hard_memory_bytes.is_some() {
            return Err(Error::UnsupportedHardLimit);
        }
        if demand.cpu == 0 {
            return Err(Error::Invalid);
        }
        if demand.cpu > self.0.capacity.cpu {
            return Err(Error::ExceedsCapacity);
        }
        for (cap, amount) in [
            (self.0.capacity.memory_bytes, demand.memory_bytes),
            (self.0.capacity.disk_bytes, demand.disk_bytes),
        ] {
            if let Some(cap) = cap {
                let amount = amount.ok_or(Error::MissingEstimate)?;
                if amount > cap {
                    return Err(Error::ExceedsCapacity);
                }
            }
        }
        let mut state = self.0.state.lock().map_err(|_| Error::State)?;
        if state.queue.len() >= self.0.capacity.queue {
            return Err(Error::QueueFull);
        }
        let id = state.next;
        state.next = state.next.checked_add(1).ok_or(Error::State)?;
        state.queue.push_back(Waiter {
            id,
            demand,
            waker: None,
        });
        Ok(Request {
            pool: self.clone(),
            id,
            done: false,
        })
    }
}
/// Cancellation-aware pending resource acquisition. No subprocess starts here.
pub struct Request {
    pool: Admission,
    id: u64,
    done: bool,
}
impl Future for Request {
    type Output = Result<Reservation, Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.done {
            return Poll::Ready(Err(Error::State));
        }
        let pool = self.pool.clone();
        let mut state = match pool.0.state.lock() {
            Ok(s) => s,
            Err(_) => return Poll::Ready(Err(Error::State)),
        };
        let Some(index) = state.queue.iter().position(|w| w.id == self.id) else {
            return Poll::Ready(Err(Error::State));
        };
        let Some(waiter) = state.queue.get_mut(index) else {
            return Poll::Ready(Err(Error::State));
        };
        waiter.waker = Some(cx.waker().clone());
        let demand = waiter.demand;
        let capacity = pool.0.capacity;
        let memory = capacity
            .memory_bytes
            .map(|_| demand.memory_bytes.unwrap_or(0))
            .unwrap_or(0);
        let disk = capacity
            .disk_bytes
            .map(|_| demand.disk_bytes.unwrap_or(0))
            .unwrap_or(0);
        let available = index == 0
            && state.usage.jobs < capacity.jobs
            && demand.cpu <= capacity.cpu - state.usage.cpu
            && capacity
                .memory_bytes
                .is_none_or(|cap| memory <= cap - state.usage.memory_bytes)
            && capacity
                .disk_bytes
                .is_none_or(|cap| disk <= cap - state.usage.disk_bytes);
        if !available {
            return Poll::Pending;
        }
        state.queue.pop_front();
        state.usage.jobs += 1;
        state.usage.cpu += demand.cpu;
        state.usage.memory_bytes += memory;
        state.usage.disk_bytes += disk;
        let wakers = wake(&state);
        drop(state);
        self.done = true;
        notify(wakers);
        Poll::Ready(Ok(Reservation(Arc::new(Lease {
            pool,
            demand,
            memory,
            disk,
            worker: AtomicBool::new(false),
        }))))
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        if !self.done {
            let mut state = self.pool.0.state.lock().unwrap_or_else(|e| e.into_inner());
            state.queue.retain(|w| w.id != self.id);
            let wakers = wake(&state);
            drop(state);
            notify(wakers);
        }
    }
}
struct Lease {
    pool: Admission,
    demand: Demand,
    memory: u64,
    disk: u64,
    worker: AtomicBool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.pool.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.usage.jobs -= 1;
        state.usage.cpu -= self.demand.cpu;
        state.usage.memory_bytes -= self.memory;
        state.usage.disk_bytes -= self.disk;
        let wakers = wake(&state);
        drop(state);
        notify(wakers);
    }
}
/// One active job's RAII reservation. Retain it through transform and all checks.
/// A helper uses worker() on this reservation instead of asking Admission again.
pub struct Reservation(Arc<Lease>);
impl Reservation {
    /// Check out its exclusive active worker/helper slot. It is returned only after
    /// the supervisor drops the permit, after process cleanup, even if the caller vanished.
    pub fn worker(&self) -> Result<WorkerPermit, Error> {
        self.0
            .worker
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::WorkerActive)?;
        Ok(WorkerPermit(self.0.clone()))
    }
    /// Frozen request and estimates, distinct from hard engine/OS limits.
    pub fn demand(&self) -> Demand {
        self.0.demand
    }
}
/// Exclusive worker ownership; move it into the blocking supervisor, never clone it.
pub struct WorkerPermit(Arc<Lease>);
impl WorkerPermit {
    /// Reserved threads to set before engine imports/connection creation.
    pub fn threads(&self) -> u32 {
        self.0.demand.cpu
    }
    /// Explicit staging/spill budget for a future engine adapter; no OS quota is claimed.
    pub fn disk_reservation(&self) -> Option<u64> {
        self.0.pool.0.capacity.disk_bytes.map(|_| self.0.disk)
    }
    /// Estimated reservation only; None means no Transflow admission budget.
    pub fn memory_reservation(&self) -> Option<u64> {
        self.0.pool.0.capacity.memory_bytes.map(|_| self.0.memory)
    }
}
impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.0.worker.store(false, Ordering::Release);
    }
}
