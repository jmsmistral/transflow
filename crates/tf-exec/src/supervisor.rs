//! Owned subprocess groups, private authenticated control and bounded independent logs.
//!
//! This module never publishes data. A terminal frame is only worker evidence.
//! Callers must use their accepted attempt and coordinator fence for persistence.
mod control;
mod logs;
use crate::discovery::{Scratch, random_bytes};
use control::{Guard, Reader};
use logs::Drain;
pub use logs::Log;
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tf_protocol::{ControlFrame, MessageType, Operation, Session, validate_document};

/// Default retained limit for each attempt stream, including its truncation marker.
pub const DEFAULT_LOG_BYTES: usize = 10 * 1024 * 1024;
/// Supervisor failure classification; subprocess text is never part of its display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// Invalid launch policy or request; no subprocess was admitted.
    #[error("Worker launch configuration or request is invalid")]
    Configuration,
    /// Private file, pipe, socket or process operation failed.
    #[error("Worker supervision encountered an operating-system error")]
    Io,
    /// OS start identity changed, disappeared, or the child was already reaped.
    #[error("Worker process identity could not be verified; group signal refused")]
    Identity,
    /// Invalid, incomplete, incompatible or wrong-session control traffic.
    #[error("Worker returned invalid or incomplete control messages")]
    Protocol,
    /// The peer did not prove possession of the protected request nonce.
    #[error("Worker channel authentication failed")]
    Authentication,
    /// Explicit coordinator operation deadline, not a phase timeout.
    #[error("Worker exceeded the caller's operation deadline")]
    Deadline,
    /// A phase exhausted its independent configured budget.
    #[error("Worker exceeded its phase deadline; inspect the phase timeout evidence")]
    PhaseDeadline,
    /// Caller explicitly canceled or dropped its waiting future.
    #[error("Worker was canceled and its process group was stopped")]
    Canceled,
    /// Worker sent a validated error terminal.
    #[error("Worker reported an operation failure")]
    Worker,
    /// Process died or returned nonzero, regardless of terminal success claim.
    #[error("Worker exited unsuccessfully")]
    Exit,
}
impl From<std::io::Error> for Failure {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}
impl From<rustix::io::Errno> for Failure {
    fn from(_: rustix::io::Errno) -> Self {
        Self::Io
    }
}

/// Resolved launch settings. Phase admission/timers are composed by the caller (T056).
#[derive(Clone, Debug)]
pub struct Policy {
    /// Retained bytes per stream, including marker; must fit the workspace cap.
    pub log_bytes: usize,
    /// Maximum permitted per-stream retention for this workspace.
    pub workspace_log_cap: usize,
    /// Deadline for connect, authentication and hello; default ten seconds.
    pub startup_timeout: Duration,
    /// Optional whole-operation deadline. None does not impose a wall timeout.
    pub operation_timeout: Option<Duration>,
    /// TERM-to-KILL grace, default five seconds.
    pub termination_grace: Duration,
    /// Liveness diagnostic threshold; absence never independently fails work.
    pub heartbeat_warning: Duration,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            log_bytes: DEFAULT_LOG_BYTES,
            workspace_log_cap: DEFAULT_LOG_BYTES,
            startup_timeout: Duration::from_secs(10),
            operation_timeout: None,
            termination_grace: Duration::from_secs(5),
            heartbeat_warning: Duration::from_secs(30),
        }
    }
}
/// Validated phase observation; the receiver persists it without blocking stream draining.
#[derive(Debug)]
pub struct WorkerPhase {
    /// Exact bound attempt.
    pub attempt: tf_domain::AttemptId,
    /// Closed, protocol-validated phase name.
    pub name: String,
    /// Real monotonic observation time.
    pub observed: Instant,
}
/// One fresh managed worker. Fields are deliberately not Debug to avoid request leaks.
pub struct Launch {
    /// Absolute, already environment-verified interpreter path.
    pub python: PathBuf,
    /// Closed worker operation, never a shell string.
    pub operation: Operation,
    /// Existing authored schema for the operation request.
    pub request_schema: &'static str,
    /// Optional bounded nonblocking phase projection. Overflow fails the worker.
    pub phase_events: Option<std::sync::mpsc::SyncSender<WorkerPhase>>,
    /// Request identity and operation data. The launcher replaces auth_token.
    pub request: Value,
    /// Resolved resource-independent supervision policy.
    pub policy: Policy,
    /// Optional existing canonical mode-0700 attempt directory for durable logs.
    /// Files are create-new; caller owns retention. Required by run_async.
    pub log_directory: Option<PathBuf>,
    /// Known credential byte values, redacted even across read boundaries.
    /// At most 64 values, each 1..=4096 bytes. This is best-effort secret hygiene.
    pub redact: Vec<Vec<u8>>,
    /// Optional attempt timers. A timed launch cannot also impose an overall deadline.
    pub timing: Option<crate::timing::Work>,
    /// Exclusive reservation retained until child cleanup, including after caller drop.
    pub reservation: Option<crate::admission::WorkerPermit>,
    /// Thread limit for standalone discovery; an admitted worker uses its reservation.
    pub threads: u32,
}
/// Thread-safe, idempotent cooperative request to the owner; it never signals PIDs.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    /// Ask the owner to TERM, KILL and reap its managed child group.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    /// Observe a cancellation request without signaling processes.
    pub fn requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
/// Bounded evidence survives all normal error returns, independently of exit success.
#[derive(Debug)]
pub struct Report {
    /// Successful control completion and successful process exit, or a typed failure.
    pub outcome: Result<(), Failure>,
    /// Independent redacted stdout prefix and truncation evidence.
    pub stdout: Log,
    /// Independent redacted stderr prefix and truncation evidence.
    pub stderr: Log,
    /// Last validated terminal frame, if any. Treat its text as untrusted.
    pub terminal: Option<ControlFrame>,
    /// At most one bounded operation result reference, never dataframe rows.
    pub result: Option<ControlFrame>,
    /// Heartbeat silence was observed; this does not decide success/failure.
    pub heartbeat_delayed: bool,
    /// Managed child PID for diagnostics only; never a reusable signal authority.
    pub pid: Option<u32>,
    /// OS start record captured while the direct child was still owned.
    /// Diagnostic evidence only: a persisted PID/start pair is not signal authority.
    pub process_start: Option<String>,
    /// Native exit code; None also covers signal exit or no launch.
    pub exit_code: Option<i32>,
    /// Both streams reached EOF. False identifies escaped/inherited descriptors.
    pub logs_complete: bool,
    /// Blocking phase timeout, with subject/check and effective setting provenance.
    pub timeout: Option<crate::timing::Timeout>,
    /// Actual thread policy installed before interpreter/engine imports.
    pub threads: Option<u32>,
    /// Time spent terminating and draining the worker, outside its phase budget.
    pub cleanup_elapsed: Duration,
}
impl Default for Report {
    fn default() -> Self {
        Self {
            outcome: Err(Failure::Configuration),
            stdout: Log::default(),
            stderr: Log::default(),
            terminal: None,
            result: None,
            heartbeat_delayed: false,
            pid: None,
            process_start: None,
            exit_code: None,
            logs_complete: false,
            timeout: None,
            threads: None,
            cleanup_elapsed: Duration::ZERO,
        }
    }
}
struct OwnedChild {
    child: Child,
    reaped: bool,
    // Only fresh, owned worker group leaders get a start record. OS probes do not.
    process_start: Option<String>,
}
impl OwnedChild {
    fn pid(&self) -> Result<rustix::process::Pid, Failure> {
        rustix::process::Pid::from_raw(i32::try_from(self.child.id()).map_err(|_| Failure::Io)?)
            .ok_or(Failure::Io)
    }
    // WNOWAIT pins the child PID even after exit. Never reap before the final group
    // signal: a stale numeric PID cannot become an unrelated process/group here.
    fn exited(&self) -> Result<bool, Failure> {
        Ok(rustix::process::waitid(
            rustix::process::WaitId::Pid(self.pid()?),
            rustix::process::WaitIdOptions::EXITED
                | rustix::process::WaitIdOptions::NOHANG
                | rustix::process::WaitIdOptions::NOWAIT,
        )?
        .is_some())
    }
    fn signal(&self, signal: rustix::process::Signal) -> Result<(), Failure> {
        if self.reaped
            || self.process_start.is_none()
            || crate::ownership::process_start(self.child.id()).ok() != self.process_start
        {
            return Err(Failure::Identity);
        }
        // Prove it remains our waitable child. The start check alone has a
        // check-to-signal race (and macOS start records have second precision).
        self.exited()?;
        match rustix::process::kill_process_group(self.pid()?, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            #[cfg(target_os = "macos")]
            Err(rustix::io::Errno::PERM) if self.exited()? && self.zombie_group()? => Ok(()),
            Err(_) => Err(Failure::Io),
        }
    }
    fn zombie_group(&self) -> Result<bool, Failure> {
        // Darwin killpg1 excludes zombies then returns EPERM when none remain.
        // Do not suppress a genuine permission failure: inspect only this pinned
        // group and require every member to be a zombie. Bound ps output too.
        let mut command = Command::new("/bin/ps");
        #[cfg(target_os = "macos")]
        command.args(["-g", &self.child.id().to_string(), "-o", "stat="]);
        #[cfg(target_os = "linux")]
        command.args(["-e", "-o", "pgid=,stat="]);
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut probe = OwnedChild {
            child,
            reaped: false,
            process_start: None,
        };
        // This probe is not a group leader; it is reaped directly below.
        // A failed inspection never authorizes shortening the termination grace.
        let mut bytes = Vec::new();
        let read = probe
            .child
            .stdout
            .take()
            .ok_or(Failure::Io)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes);
        if bytes.len() > 1024 * 1024 {
            let _ = probe.child.kill();
        }
        let status = probe.reap()?;
        read?;
        if bytes.len() > 1024 * 1024 || (!status.success() && status.code() != Some(1)) {
            return Err(Failure::Io);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| Failure::Io)?;
        #[cfg(target_os = "macos")]
        {
            Ok(text.split_whitespace().all(|state| state.starts_with('Z')))
        }
        #[cfg(target_os = "linux")]
        {
            for line in text.lines() {
                let mut fields = line.split_whitespace();
                let group: u32 = fields
                    .next()
                    .ok_or(Failure::Io)?
                    .parse()
                    .map_err(|_| Failure::Io)?;
                let state = fields.next().ok_or(Failure::Io)?;
                if group == self.child.id() && !state.starts_with('Z') {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
    fn reap(&mut self) -> Result<std::process::ExitStatus, Failure> {
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.signal(rustix::process::Signal::KILL);
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn operation_name(op: Operation) -> &'static str {
    match op {
        Operation::Discover => "discover",
        Operation::Execute => "execute",
        Operation::EvaluateChecks => "evaluate_checks",
        Operation::QueryPreview => "query_preview",
        Operation::InferLineage => "infer_lineage",
        Operation::InspectEnvironment => "inspect_environment",
    }
}
fn valid(launch: &Launch) -> bool {
    let policy = &launch.policy;
    launch.request.is_object()
        && launch.threads > 0
        && launch.timing.as_ref().is_none_or(|work| {
            work.valid_for(launch.operation) && policy.operation_timeout.is_none()
        })
        && launch.python.is_absolute()
        && policy.log_bytes >= logs::MARKER.len()
        && policy.log_bytes <= policy.workspace_log_cap
        && !policy.startup_timeout.is_zero()
        && !policy.heartbeat_warning.is_zero()
        && launch.redact.len() <= 64
        && launch
            .redact
            .iter()
            .all(|s| !s.is_empty() && s.len() <= 4096)
}
struct RequestBytes(Vec<u8>);
impl Write for RequestBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > tf_protocol::MAX_FRAME_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("request limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn log_files(path: Option<&std::path::Path>) -> Result<(Option<File>, Option<File>), Failure> {
    let Some(path) = path else {
        return Ok((None, None));
    };
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW).bits() as i32)
        .open(path)?;
    if directory.metadata()?.permissions().mode() & 0o777 != 0o700 {
        return Err(Failure::Configuration);
    }
    let open = |name| -> Result<File, Failure> {
        Ok(rustix::fs::openat(
            &directory,
            name,
            rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_bits_truncate(0o600),
        )?
        .into())
    };
    Ok((Some(open("stdout.log")?), Some(open("stderr.log")?)))
}
/// Blocking owner loop. Run on a bounded blocking executor, never an async reactor.
/// No caller callback or unbounded channel can delay draining/control/cancellation.
pub fn run(mut launch: Launch, cancel: Cancellation) -> Report {
    let mut report = Report::default();
    report.outcome = run_inner(&mut launch, &cancel, &mut report);
    report
}
fn run_inner(
    launch: &mut Launch,
    cancel: &Cancellation,
    report: &mut Report,
) -> Result<(), Failure> {
    if !valid(launch) {
        return Err(Failure::Configuration);
    }
    if cancel.requested() {
        return Err(Failure::Canceled);
    }
    if let Some(path) = &launch.log_directory {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir()
            || metadata.permissions().mode() & 0o777 != 0o700
            || fs::canonicalize(path)? != *path
        {
            return Err(Failure::Configuration);
        }
    }
    let scratch = Scratch::create()?;
    let token = random_bytes::<32>()?;
    let token_hex: String = token.iter().map(|b| format!("{b:02x}")).collect();
    launch.request["auth_token"] = token_hex.clone().into();
    validate_document(launch.request_schema, &launch.request)
        .map_err(|_| Failure::Configuration)?;
    let mut encoded = RequestBytes(Vec::new());
    serde_json::to_writer(&mut encoded, &launch.request).map_err(|_| Failure::Configuration)?;
    let bytes = encoded.0;
    let mut guard = Guard::new(
        Session::new(
            launch.request["request_id"]
                .as_str()
                .ok_or(Failure::Configuration)?
                .parse()
                .map_err(|_| Failure::Configuration)?,
            launch.request["attempt_id"]
                .as_str()
                .ok_or(Failure::Configuration)?
                .parse()
                .map_err(|_| Failure::Configuration)?,
            launch.operation,
            [
                "discovery.v1".to_owned(),
                "polars.execute.v1".to_owned(),
                "expectation.ast.v1".to_owned(),
                "expectation.core.v1".to_owned(),
                "duckdb.checks.v1".to_owned(),
                "duckdb.samples.v1".to_owned(),
            ]
            .into(),
        )
        .map_err(|_| Failure::Configuration)?,
        launch.operation,
    );
    let request_path = scratch.path().join("request.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&request_path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    let socket_path = scratch.path().join("control.sock");
    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    let (out_file, err_file) = log_files(launch.log_directory.as_deref())?;
    if let Some(work) = &launch.timing {
        work.budget
            .enter(work.phase)
            .map_err(|_| Failure::Configuration)?;
    }
    check_phase(launch, report)?;
    let threads = launch
        .reservation
        .as_ref()
        .map(|p| p.threads())
        .unwrap_or(launch.threads);
    report.threads = Some(threads);
    let child = Command::new(&launch.python)
        .args([
            "-I",
            "-B",
            "-m",
            "transflow_worker",
            operation_name(launch.operation),
            "--request",
        ])
        .arg(request_path)
        .arg("--control-socket")
        .arg(socket_path)
        .current_dir(scratch.path())
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .env("POLARS_MAX_THREADS", threads.to_string())
        .env("OMP_NUM_THREADS", threads.to_string())
        .env("OPENBLAS_NUM_THREADS", threads.to_string())
        .env("MKL_NUM_THREADS", threads.to_string())
        .env("NUMEXPR_MAX_THREADS", threads.to_string())
        .env("VECLIB_MAXIMUM_THREADS", threads.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let mut owned = OwnedChild {
        child,
        reaped: false,
        process_start: None,
    };
    report.pid = Some(owned.child.id());
    owned.process_start =
        Some(crate::ownership::process_start(owned.child.id()).map_err(|_| Failure::Identity)?);
    report.process_start.clone_from(&owned.process_start);
    let mut secrets = launch.redact.clone();
    secrets.push(token.to_vec());
    secrets.push(token_hex.into_bytes());
    let mut stdout = Drain::new(
        owned.child.stdout.take().ok_or(Failure::Io)?,
        launch.policy.log_bytes,
        secrets.clone(),
        out_file,
    )?;
    let mut stderr = Drain::new(
        owned.child.stderr.take().ok_or(Failure::Io)?,
        launch.policy.log_bytes,
        secrets,
        err_file,
    )?;
    let started = Instant::now();
    let mut channel: Option<UnixStream> = None;
    let mut auth = [0; 32];
    let mut auth_len = 0;
    let mut ack_len = 0;
    let mut reader = Reader::default();
    let mut last_heartbeat = started;
    let mut hello = false;
    let mut exit_seen = None;
    let mut terminal_seen = None;
    let result = (|| {
        loop {
            stdout.tick()?;
            stderr.tick()?;
            if cancel.requested() {
                return Err(Failure::Canceled);
            }
            check_phase(launch, report)?;
            let now = Instant::now();
            if launch
                .policy
                .operation_timeout
                .is_some_and(|limit| now.duration_since(started) >= limit)
                || (!hello && now.duration_since(started) >= launch.policy.startup_timeout)
            {
                return Err(Failure::Deadline);
            }
            if hello && now.duration_since(last_heartbeat) >= launch.policy.heartbeat_warning {
                report.heartbeat_delayed = true;
            }
            if channel.is_none() {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(true)?;
                        channel = Some(stream);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err(Failure::Io),
                }
            }
            let mut control_drained = false;
            if let Some(stream) = &mut channel {
                if auth_len < auth.len() {
                    match stream.read(&mut auth[auth_len..]) {
                        Ok(0) => return Err(Failure::Authentication),
                        Ok(n) => {
                            auth_len += n;
                            if auth_len == auth.len() && auth != token {
                                return Err(Failure::Authentication);
                            }
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            ) => {}
                        Err(_) => return Err(Failure::Io),
                    }
                }
                if auth_len == auth.len() && ack_len < token.len() {
                    match stream.write(&token[ack_len..]) {
                        Ok(0) => return Err(Failure::Authentication),
                        Ok(n) => ack_len += n,
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            ) => {}
                        Err(_) => return Err(Failure::Io),
                    }
                }
                if ack_len == token.len() {
                    // Bound control work per tick as well as its frame allocation.
                    for _ in 0..8 {
                        let Some(frame) = reader.next(stream)? else {
                            control_drained = true;
                            break;
                        };
                        if frame.message_type() == MessageType::Hello {
                            hello = true;
                            last_heartbeat = now;
                        }
                        if frame.message_type() == MessageType::Heartbeat {
                            last_heartbeat = now;
                        }
                        check_phase(launch, report)?;
                        let phase = frame.as_json()["message"]["phase"]
                            .as_str()
                            .map(str::to_owned);
                        guard.accept(frame)?;
                        if let (Some(sender), Some(name)) = (&launch.phase_events, &phase) {
                            sender
                                .try_send(WorkerPhase {
                                    attempt: launch.request["attempt_id"]
                                        .as_str()
                                        .ok_or(Failure::Protocol)?
                                        .parse()
                                        .map_err(|_| Failure::Protocol)?,
                                    name: name.clone(),
                                    observed: now,
                                })
                                .map_err(|_| Failure::Protocol)?;
                        }
                        if let (Some(work), Some(phase)) = (&launch.timing, phase) {
                            work.message(launch.operation, &phase)
                                .map_err(|_| Failure::Protocol)?;
                        }
                        if guard.terminal.is_some() {
                            terminal_seen.get_or_insert(now);
                        }
                    }
                    if reader.eof && guard.terminal.is_none() {
                        return Err(Failure::Protocol);
                    }
                }
            }
            if terminal_seen.is_some_and(|at| {
                now.duration_since(at)
                    >= launch.policy.termination_grace.max(Duration::from_secs(1))
            }) && !owned.exited()?
            {
                return Err(Failure::Protocol);
            }
            if owned.exited()? {
                let at = *exit_seen.get_or_insert(now);
                if now.duration_since(at) >= Duration::from_secs(1) {
                    return Err(Failure::Protocol);
                }
                // Process exit can precede draining buffered terminal frames. Continue
                // until socket EOF; a descendant holding it open is bounded below.
                if channel.is_none() {
                    return Err(Failure::Exit);
                }
                if reader.eof || (guard.terminal.is_some() && control_drained && !reader.partial())
                {
                    return Ok(());
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    })();
    // Even success must close descendants. Keep leader waitable until the last
    // group signal so PID reuse cannot redirect cleanup. No process handle escapes.
    let cleanup_start = Instant::now();
    let _phase_cleanup = launch.timing.as_ref().map(|work| work.budget.cleanup());
    let mut cleanup = owned.signal(rustix::process::Signal::TERM);
    let grace_start = Instant::now();
    while grace_start.elapsed() < launch.policy.termination_grace {
        if let Err(e) = stdout.tick().and_then(|()| stderr.tick()) {
            cleanup = Err(e.into());
            break;
        }
        if owned.exited().unwrap_or(false)
            && stdout.eof
            && stderr.eof
            && owned.zombie_group().unwrap_or(false)
        {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    if let Err(e) = owned.signal(rustix::process::Signal::KILL) {
        cleanup = Err(e);
    }
    // Also kill the direct child if trusted code changed its own group. The
    // unreaped Child still pins its PID; no stored PID is used after reaping.
    if !owned.exited()? {
        owned.child.kill()?;
    }
    let status = owned.reap();
    let drain_start = Instant::now();
    while !(stdout.eof && stderr.eof) && drain_start.elapsed() < Duration::from_secs(1) {
        if let Err(e) = stdout.tick().and_then(|()| stderr.tick()) {
            cleanup = Err(e.into());
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    report.cleanup_elapsed = cleanup_start.elapsed();
    report.logs_complete = stdout.eof && stderr.eof;
    report.stdout = stdout.finish()?;
    report.stderr = stderr.finish()?;
    report.result = guard.result;
    report.terminal = guard.terminal;
    let status = status?;
    report.exit_code = status.code();
    result?;
    cleanup?;
    if report
        .terminal
        .as_ref()
        .is_some_and(|f| f.message_type() == MessageType::Error)
    {
        return Err(Failure::Worker);
    }
    if !status.success() {
        return Err(Failure::Exit);
    }
    if report.terminal.is_none() {
        return Err(Failure::Protocol);
    }
    Ok(())
}
fn check_phase(launch: &Launch, report: &mut Report) -> Result<(), Failure> {
    if let Some(work) = &launch.timing
        && let Some(timeout) = work.budget.expired().map_err(|_| Failure::Configuration)?
    {
        report.timeout = Some(timeout);
        return Err(Failure::PhaseDeadline);
    }
    Ok(())
}
struct CancelOnDrop(Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
/// Own work on Tokio's bounded blocking executor. Dropping/aborting the future
/// requests cleanup; the blocking owner remains alive to kill/reap and flush logs.
/// A persistent private log directory is required because the receiver may vanish.
/// Runtime shutdown waits for the owner; operation deadlines belong to the caller.
pub async fn run_async(launch: Launch, cancel: Cancellation) -> Report {
    if launch.log_directory.is_none() {
        return Report::default();
    }
    let guard = CancelOnDrop(cancel.clone());
    let result = tokio::task::spawn_blocking(move || run(launch, cancel)).await;
    drop(guard);
    result.unwrap_or_else(|_| Report {
        outcome: Err(Failure::Io),
        ..Report::default()
    })
}

#[cfg(test)]
mod identity_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)] // Isolated native fixtures.
    use super::*;
    #[test]
    fn stale_start_record_and_reaped_handle_never_signal_a_live_group() {
        // The canary is our own isolated group. Never target a parent/unrelated group.
        let child = Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let mut owned = OwnedChild {
            child,
            reaped: false,
            process_start: None,
        };
        let start = crate::ownership::process_start(owned.child.id()).unwrap();
        owned.process_start = Some(format!("stale:{start}"));
        for signal in [rustix::process::Signal::TERM, rustix::process::Signal::KILL] {
            assert_eq!(owned.signal(signal), Err(Failure::Identity));
            assert!(
                !owned.exited().unwrap(),
                "mismatched start must leave the canary alive"
            );
        }
        owned.process_start = Some(start);
        owned.signal(rustix::process::Signal::KILL).unwrap();
        owned.reap().unwrap();
        assert_eq!(
            owned.signal(rustix::process::Signal::TERM),
            Err(Failure::Identity)
        );
    }
}
