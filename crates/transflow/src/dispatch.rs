//! Execute an immutable accepted plan using shared cache, worker, check and publication services.
mod context;
pub(crate) use cancel::unstarted as cancel_unstarted;
mod worker;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tf_domain::{
    AttemptId, BuildId, JobId, RequestId,
    execution::{
        Build, BuildState, EventTime, ExecutionBinding, Job, JobState, Phase, RetryPolicy,
    },
};
use tf_exec::{ownership::RuntimeOwner, supervisor::Cancellation};
use tf_protocol::canonical::{DigestKind, content_digest};
use tf_store::{
    artifacts::ArtifactStore,
    retention::{LeaseKind, ReadLease, ReadTarget},
};
type Result<T> = std::result::Result<T, Error>;
/// Safe dispatcher failure; a failed job is reported in `Report`, not as a service error.
#[derive(Debug, thiserror::Error)]
#[error("Build dispatch failed: {0}")]
pub struct Error(String);
fn fail(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .ok_or_else(|| fail("incomplete execution context"))
}
fn now() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(fail)?
        .as_micros()
        .try_into()
        .map_err(fail)
}
fn new_id() -> Result<RequestId> {
    tf_exec::discovery::random_id()
        .map_err(fail)?
        .parse()
        .map_err(fail)
}
macro_rules! repository {
    ($owner:expr,$rt:expr,$s:ident,$call:expr) => {{
        let mut owned = $rt.block_on($owner.open_store()).map_err(fail)?;
        let $s = owned.repository().map_err(fail)?;
        let result = $rt.block_on($call).map_err(fail);
        $rt.block_on(owned.close()).map_err(fail)?;
        result?
    }};
}
mod cancel;
mod coordinator;

/// Complete immutable job outcomes; no check samples or raw worker logs are exposed.
#[derive(Debug)]
pub struct Report {
    /// Accepted build identity.
    pub build: BuildId,
    /// Aggregate mandatory job outcome.
    pub state: BuildState,
    /// Exact job/attempt/version identities and state transitions.
    pub jobs: Vec<Job>,
}
/// The owner is retained through cleanup even when the result is a service failure.
pub struct Completion {
    /// Still-held workspace ownership.
    pub owner: RuntimeOwner,
    /// Terminal report, or refusal requiring inspection/recovery before a new dispatch.
    pub result: Result<Report>,
}
struct CancelOnDrop(Option<Cancellation>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(c) = &self.0 {
            c.cancel()
        }
    }
}
/// Explicit estimates for optional admission budgets; these are not hard OS limits.
#[derive(Default)]
pub struct Options {
    /// Authenticated metadata-only requests served by this same runtime owner.
    pub providers: Option<crate::provider::Mailbox>,
    /// Per-job estimated peak bytes, required only with a configured memory budget.
    pub memory_estimates: BTreeMap<JobId, u64>,
    /// Bounded best-effort phase observations; terminal storage remains authoritative.
    pub progress: Option<std::sync::mpsc::SyncSender<Value>>,
    /// Authenticated coordinator cancellation requests; acknowledgement follows durable commit.
    pub commands:
        Option<std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<CancelCommand>>>>,
}
/// A transport-validated cancellation request. Only the coordinator mutates SQLite.
pub struct CancelCommand {
    /// Explicit requested build, never an implicit workspace-wide cancellation.
    pub build: BuildId,
    /// Bounded acknowledgement, sent after the publication/cancellation transaction.
    pub reply: std::sync::mpsc::SyncSender<
        std::result::Result<tf_domain::execution::CancelRequest, String>,
    >,
}
/// Dispatch an accepted, untouched build. Dropping this coordinator future requests cleanup;
/// a persistent server must retain the future independently of its client connection.
pub async fn run(owner: RuntimeOwner, build: BuildId, cancel: Cancellation) -> Result<Completion> {
    run_with_options(owner, build, cancel, Options::default()).await
}
/// Dispatch with caller-provided resource estimates, validated before any producer starts.
pub async fn run_with_options(
    owner: RuntimeOwner,
    build: BuildId,
    cancel: Cancellation,
    options: Options,
) -> Result<Completion> {
    let mut guard = CancelOnDrop(Some(cancel.clone()));
    let result = tokio::task::spawn_blocking(move || {
        let mut owner = owner;
        let result = (|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(fail)?;
            execute(&mut owner, &rt, build, cancel, options)
        })();
        Completion { owner, result }
    })
    .await
    .map_err(fail)?;
    guard.0 = None;
    Ok(result)
}
struct Active {
    contract: tf_store::publication::PublicationContract,
    consumer: String,
    directory: PathBuf,
    leases: Vec<ReadLease>,
}
struct Coordinator<'a> {
    owner: &'a mut RuntimeOwner,
    rt: &'a tokio::runtime::Runtime,
    prepared: context::Prepared,
    active: BTreeMap<JobId, Active>,
    phase_start: BTreeMap<JobId, Instant>,
    started: Instant,
    contracts: BTreeMap<JobId, tf_store::cache::Request>,
    retry_leases: BTreeMap<JobId, Vec<ReadLease>>,
    progress: Option<std::sync::mpsc::SyncSender<Value>>,
}

fn execute(
    owner: &mut RuntimeOwner,
    rt: &tokio::runtime::Runtime,
    build_id: BuildId,
    cancel: Cancellation,
    options: Options,
) -> Result<Report> {
    let ready = (|| -> Result<_> {
        let prepared = context::prepare(owner, rt, build_id)?;
        if let Some(capacity) = prepared.capacity.memory_bytes {
            for id in prepared.jobs.keys() {
                if !options
                    .memory_estimates
                    .get(id)
                    .is_some_and(|n| *n > 0 && *n <= capacity)
                {
                    return Err(fail(
                        "memory-budget dispatch requires a positive per-job estimate within capacity",
                    ));
                }
            }
        }
        let workspace = tf_catalog::workspace::Workspace::load(
            owner.workspace_root(),
            Some(owner.workspace_root()),
        )
        .map_err(fail)?;
        let mut draft_pins = crate::external_reads::DraftPins::new(&workspace, &prepared.plan.id);
        for read in &prepared.plan.reads {
            draft_pins.add(read);
        }
        let foreign =
            crate::external_reads::Reads::open(&workspace, &prepared.plan, cancel.clone(), true)
                .map_err(fail)?;
        let mut owned = rt.block_on(owner.open_store()).map_err(fail)?;
        rt.block_on(foreign.record(owned.repository().map_err(fail)?))
            .map_err(fail)?;
        rt.block_on(owned.close()).map_err(fail)?;
        // Release replaced draft pins even when verification fails before dispatch.
        drop(draft_pins);
        let artifacts = ArtifactStore::open(owner.workspace_root()).map_err(fail)?;
        let attempts = owner.workspace_root().join(".transflow/runtime/attempts");
        worker::private(&attempts)?;
        Ok((prepared, artifacts, attempts, foreign))
    })();
    let (prepared, artifacts, attempts, foreign) = match ready {
        Ok(v) => v,
        Err(error) => {
            if let Err(cleanup) = cancel_unstarted(owner, rt, build_id) {
                return Err(fail(format!("{error}; cleanup also failed: {cleanup}")));
            }
            return Err(error);
        }
    };
    let ids = prepared
        .plan
        .writes
        .iter()
        .map(|w| w.job.parse().map_err(fail))
        .collect::<Result<Vec<JobId>>>()?;
    let mut build = Build::new(
        build_id,
        prepared.jobs[&ids[0]].binding(),
        ids.iter().map(|i| (*i, true)).collect(),
        EventTime(0),
    )
    .map_err(fail)?;
    let pool = tf_exec::admission::Admission::new(prepared.capacity).map_err(fail)?;
    let mut coordinator = Coordinator {
        owner,
        rt,
        prepared,
        active: BTreeMap::new(),
        phase_start: BTreeMap::new(),
        started: Instant::now(),
        contracts: BTreeMap::new(),
        retry_leases: BTreeMap::new(),
        progress: options.progress,
    };
    repository!(
        coordinator.owner,
        rt,
        s,
        s.start_dispatch(
            &coordinator
                .prepared
                .jobs
                .values()
                .cloned()
                .collect::<Vec<_>>(),
            now()?
        )
    );
    build.start(coordinator.time()).map_err(fail)?;
    std::thread::scope(|scope| -> Result<Report> {
        let (events, rx) = std::sync::mpsc::sync_channel(
            (coordinator.prepared.capacity.jobs as usize).saturating_mul(2),
        );
        let (phases, phase_rx) = std::sync::mpsc::sync_channel(
            (coordinator.prepared.capacity.jobs as usize).saturating_mul(8),
        );
        // Declared after the receivers: on error it signals cleanup before scoped joins.
        let mut control =
            tf_exec::cancellation::BuildControl::new(coordinator.owner, build_id, ids.len())
                .map_err(fail)?;
        let mut handles = BTreeMap::new();
        let mut abort = false;
        let mut provider_failure = None;
        let mut renewed = Instant::now();
        let mut cancellation_checked = Instant::now()
            .checked_sub(Duration::from_millis(100))
            .unwrap_or_else(Instant::now);
        loop {
            if let Err(error) = foreign.check() {
                provider_failure = Some(error.to_string());
                if !abort {
                    rt.block_on(control.request(coordinator.owner))
                        .map_err(fail)?;
                    abort = true;
                }
            }
            if let Some(providers) = &options.providers {
                crate::provider::drain(coordinator.owner, rt, providers).map_err(fail)?;
            }
            if let Some(commands) = &options.commands {
                while let Ok(command) = commands.lock().map_err(fail)?.try_recv() {
                    if command.build == build_id {
                        let result = rt.block_on(control.request(coordinator.owner));
                        let _ = command
                            .reply
                            .try_send(result.as_ref().copied().map_err(ToString::to_string));
                        result.map_err(fail)?;
                        abort = true;
                    } else {
                        let result = crate::build_transport::cancel_owned(
                            coordinator.owner,
                            rt,
                            command.build,
                        );
                        let _ = command.reply.try_send(result.map_err(|e| e.to_string()));
                    }
                }
            }
            if !abort && cancellation_checked.elapsed() >= Duration::from_millis(100) {
                if repository!(coordinator.owner, rt, s, s.dispatch_canceled(build_id)) {
                    rt.block_on(control.request(coordinator.owner))
                        .map_err(fail)?;
                    abort = true;
                }
                cancellation_checked = Instant::now();
            }
            if cancel.requested() && !abort {
                rt.block_on(control.request(coordinator.owner))
                    .map_err(fail)?;
                abort = true;
            }
            if renewed.elapsed() >= Duration::from_secs(30) {
                coordinator.renew()?;
                renewed = Instant::now();
            }
            for id in &ids {
                let job = &coordinator.prepared.jobs[id];
                if matches!(job.state(), JobState::Finished(_) | JobState::Executing(_)) {
                    continue;
                }
                let parents = &coordinator.prepared.parents[id];
                if let Some(failed) = parents
                    .iter()
                    .map(|p| &coordinator.prepared.jobs[p])
                    .find(|p| matches!(p.state(),JobState::Finished(o) if matches!(o,tf_domain::execution::JobOutcome::Failed(_)|tf_domain::execution::JobOutcome::Blocked(_))))
                    .cloned()
                {
                    coordinator.transition(*id, |j, t| j.block(&failed, j.fence(), t))?;
                    continue;
                }
                if abort {
                    coordinator.cancel_job(*id)?;
                    continue;
                }
                if matches!(job.state(), JobState::RetryWait(ready) if coordinator.time() < *ready)
                {
                    continue;
                }
                if parents.iter().any(|p|!matches!(coordinator.prepared.jobs[p].state(),JobState::Finished(o) if o.successful())){
                    if job.state()==&JobState::Planned{coordinator.transition(*id,|j,t|j.wait_dependencies(j.fence(),t))?;}continue
                }
                let usage = pool.usage().map_err(fail)?;
                let mut demand = pool.default_demand();
                demand.memory_bytes = options.memory_estimates.get(id).copied();
                if usage.jobs >= coordinator.prepared.capacity.jobs
                    || demand.cpu > coordinator.prepared.capacity.cpu - usage.cpu
                    || coordinator
                        .prepared
                        .capacity
                        .memory_bytes
                        .is_some_and(|cap| {
                            demand.memory_bytes.unwrap_or(0) > cap - usage.memory_bytes
                        })
                {
                    continue;
                }
                let (request, reuse) = coordinator.contract(*id)?;
                repository!(coordinator.owner, rt, s, s.bind_execution_inputs(&request));
                if foreign.check().is_err() {
                    continue;
                }
                if reuse && coordinator.cache(&request, &artifacts, &foreign)? {
                    continue;
                }
                let mut inputs = vec![];
                let mut leases = vec![];
                for i in &request.contract.inputs {
                    if i.dataset.workspace_id() != request.target.dataset.workspace_id() {
                        inputs.push(
                            foreign
                                .verify(&request.target.dataset.dataset_id().to_string(), &i.alias)
                                .map_err(fail)?,
                        );
                        continue;
                    }
                    let lease = repository!(
                        coordinator.owner,
                        rt,
                        s,
                        s.acquire_read(
                            new_id()?,
                            RequestId::from_bytes(*build_id.as_bytes()),
                            LeaseKind::Build,
                            ReadTarget::Version(i.version),
                            now()?,
                            3_600_000_000
                        )
                    );
                    if lease.artifact() != i.artifact {
                        return Err(fail("accepted input bytes changed"));
                    }
                    inputs.push(artifacts.verify(i.artifact).map_err(fail)?);
                    leases.push(lease);
                }
                if let Some(old) = coordinator.retry_leases.remove(id) {
                    for lease in old {
                        repository!(coordinator.owner, rt, s, s.release_read(&lease));
                    }
                }
                let reservation = rt
                    .block_on(pool.request(demand).map_err(fail)?)
                    .map_err(fail)?;
                if !matches!(
                    coordinator.prepared.jobs[id].state(),
                    JobState::RetryWait(_)
                ) {
                    coordinator.transition(*id, |j, t| j.queue(j.fence(), t))?;
                }
                let attempt = AttemptId::from_bytes(*new_id()?.as_bytes());
                let directory = attempts.join(attempt.to_string());
                worker::private(&directory)?;
                let mut execute = worker::request(
                    &coordinator.prepared,
                    *id,
                    attempt,
                    request.contract.clone(),
                    &inputs,
                    &directory,
                    demand.cpu,
                )?;
                execute.execute.phase_events = Some(phases.clone());
                coordinator.transition(*id, |j, t| j.start_attempt(attempt, j.fence(), t))?;
                let token = rt
                    .block_on(control.register(coordinator.owner, attempt, request.fence))
                    .map_err(fail)?
                    .cancellation();
                let consumer = content_digest(
                    DigestKind::Compute,
                    &coordinator.prepared.declarations[id].0,
                )
                .map_err(fail)?
                .hex();
                coordinator.active.insert(
                    *id,
                    Active {
                        contract: request.contract,
                        consumer,
                        directory: directory.clone(),
                        leases,
                    },
                );
                let work = worker::Work {
                    request: execute,
                    inputs,
                    reservation,
                    cancel: token,
                    directory,
                    python: coordinator.prepared.interpreter.clone(),
                };
                let events = events.clone();
                let artifacts = &artifacts;
                let id = *id;
                handles.insert(
                    id,
                    scope.spawn(move || worker::run(artifacts, id, work, events)),
                );
            }
            if coordinator
                .prepared
                .jobs
                .values()
                .all(|j| matches!(j.state(), JobState::Finished(_)))
            {
                break;
            }
            while let Ok(event) = phase_rx.try_recv() {
                if !abort {
                    coordinator.materialization(event)?;
                }
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(worker::Message::Observe(id, event, ack)) => {
                    // The worker queues its materialization frame before its output phase event.
                    while let Ok(event) = phase_rx.try_recv() {
                        if !abort {
                            coordinator.materialization(event)?;
                        }
                    }
                    if abort && matches!(event, worker::Observation::Phase(_)) {
                        let _ = ack.send(false);
                        continue;
                    }
                    let result = coordinator.observe(id, event);
                    let _ = ack.send(result.is_ok());
                    result?;
                }
                Ok(worker::Message::Finished(id, completion, reservation)) => {
                    while let Ok(event) = phase_rx.try_recv() {
                        if !abort {
                            coordinator.materialization(event)?;
                        }
                    }
                    if !abort
                        && (cancel.requested()
                            || repository!(coordinator.owner, rt, s, s.dispatch_canceled(build_id)))
                    {
                        rt.block_on(control.request(coordinator.owner))
                            .map_err(fail)?;
                        abort = true;
                    }
                    if let Err(error) = foreign.check() {
                        provider_failure = Some(error.to_string());
                        if !abort {
                            rt.block_on(control.request(coordinator.owner))
                                .map_err(fail)?;
                            abort = true;
                        }
                    }
                    let success = coordinator.finish(id, *completion, abort, &foreign)?;
                    handles
                        .remove(&id)
                        .ok_or_else(|| fail("missing worker join"))?
                        .join()
                        .map_err(|_| fail("worker panicked"))?;
                    control
                        .finished(
                            coordinator.prepared.jobs[&id]
                                .attempts()
                                .last()
                                .ok_or_else(|| fail("missing completed attempt"))?
                                .id(),
                        )
                        .map_err(fail)?;
                    drop(reservation);
                    if !success
                        && !abort
                        && coordinator.prepared.abort_on_failure
                        && matches!(
                            coordinator.prepared.jobs[&id].state(),
                            JobState::Finished(tf_domain::execution::JobOutcome::Failed(_))
                        )
                    {
                        rt.block_on(control.request(coordinator.owner))
                            .map_err(fail)?;
                        abort = true;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if handles.values().any(|h| h.is_finished()) {
                        return Err(fail("worker ended without completion evidence"));
                    }
                }
                Err(_) => return Err(fail("worker event stream disconnected")),
            }
        }
        let jobs = coordinator
            .prepared
            .jobs
            .values()
            .cloned()
            .collect::<Vec<_>>();
        build.finish(&jobs, coordinator.time()).map_err(fail)?;
        repository!(
            coordinator.owner,
            rt,
            s,
            s.finish_dispatch(&jobs, build.state(), now()?)
        );
        if let Some(error) = provider_failure {
            return Err(fail(error));
        }
        Ok(Report {
            build: build_id,
            state: build.state(),
            jobs,
        })
    })
}
