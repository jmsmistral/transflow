//! Explicit environment preparation using the embedded standard-library Python service.
use crate::ownership::RuntimeOwner;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const SERVICE: &str = include_str!("../../../python/worker/src/transflow_worker/environment.py");
/// Only explicit environment commands may resolve/install packages.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAction {
    /// Resolve a new hash lock.
    Lock,
    /// Prepare and verify a fresh managed environment.
    Sync,
    /// Verify existing bytes; never install or resolve.
    Check,
}
/// Resolved authoring inputs and explicit local package sources.
#[derive(Serialize)]
pub struct EnvironmentRequest {
    /// Explicit operation.
    pub action: EnvironmentAction,
    /// Relative dependency input path.
    pub requirements: String,
    /// Relative dependency lock path.
    pub lock: String,
    /// Configured supported Python minor.
    pub minor: String,
    /// Explicit application-matched wheel; no index fallback is allowed.
    pub runtime_wheel: Option<PathBuf>,
    /// Matched distribution version.
    pub runtime_version: String,
    /// Optional local wheelhouse used for controlled/offline setup.
    pub wheelhouse: Option<PathBuf>,
    /// Forbid dependency index access.
    pub offline: bool,
}
/// Typed operational failure; arbitrary stderr is never copied into diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    /// Process/filesystem failure.
    #[error(
        "Could not start the selected Python interpreter; prepare the qualified tooling and retry"
    )]
    Io(#[from] std::io::Error),
    /// Invalid bounded helper result.
    #[error("Environment service returned an invalid response")]
    Protocol,
    /// Explicit package operation failed with a fixed service explanation.
    #[error("{0}")]
    Rejected(String),
    /// Finite explicit setup deadline exceeded.
    #[error(
        "Environment preparation exceeded its 30-minute limit; no implicit retry was attempted"
    )]
    Timeout,
    /// Read-only inspection was stopped by its caller's cancellation or phase budget.
    #[error("Environment inspection interrupted")]
    Interrupted,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<String>,
}
/// Run while borrowing exclusive runtime ownership. There is no build-triggered call path.
pub fn run(
    owner: &mut RuntimeOwner,
    python: &Path,
    request: &EnvironmentRequest,
) -> Result<String, EnvironmentError> {
    let result = invoke(owner.workspace_root(), python, request, None)?;
    let action = result["action"]
        .as_str()
        .ok_or(EnvironmentError::Protocol)?;
    let packages = result["packages"]
        .as_u64()
        .ok_or(EnvironmentError::Protocol)?;
    let digest = result
        .get("fingerprint")
        .or_else(|| result.get("lock_sha256"))
        .and_then(|v| v.as_str())
        .ok_or(EnvironmentError::Protocol)?;
    Ok(format!(
        "Environment {action} completed: {packages} packages; fingerprint {digest}"
    ))
}
/// Read-only verified managed interpreter; preparation never installs dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedEnvironment {
    /// Qualified optional SQL engines are installed and byte-verified.
    pub query_available: bool,
    /// Verified interpreter entry point in this workspace's managed environment.
    pub interpreter: PathBuf,
    /// Actual installed bytes/configuration identity.
    pub fingerprint: String,
    /// Matched installed SDK/worker version.
    pub runtime_version: String,
}
/// Recover the explicit base interpreter selected by a completed `env sync`.
/// This private locator is local execution configuration, never a request-supplied path.
pub fn recorded_interpreter(root: &Path) -> Result<PathBuf, EnvironmentError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(root.join(".transflow/runtime/environment.json"))?;
    let m = file.metadata()?;
    if !m.is_file() || m.nlink() != 1 || m.mode() & 0o077 != 0 || m.len() > 1024 * 1024 {
        return Err(EnvironmentError::Protocol);
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(EnvironmentError::Protocol);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| EnvironmentError::Protocol)?;
    let path = PathBuf::from(
        value["interpreter_locator"]
            .as_str()
            .ok_or(EnvironmentError::Protocol)?,
    );
    if !path.is_absolute() {
        return Err(EnvironmentError::Protocol);
    }
    Ok(path)
}
/// Inspect with the explicit base interpreter recorded at env sync, without runtime writes.
pub fn inspect(
    root: &Path,
    python: &Path,
    request: &EnvironmentRequest,
) -> Result<ManagedEnvironment, EnvironmentError> {
    inspect_interruptible(root, python, request, &|| false)
}
/// Read-only inspection that cooperates with the caller's independent phase deadline.
/// Cancellation kills and reaps this fixed application-owned inspection process.
pub fn inspect_interruptible(
    root: &Path,
    python: &Path,
    request: &EnvironmentRequest,
    stop: &dyn Fn() -> bool,
) -> Result<ManagedEnvironment, EnvironmentError> {
    if !matches!(request.action, EnvironmentAction::Check) {
        return Err(EnvironmentError::Protocol);
    }
    let result = invoke(root, python, request, Some(stop))?;
    let interpreter = PathBuf::from(
        result["interpreter"]
            .as_str()
            .ok_or(EnvironmentError::Protocol)?,
    );
    let version = result["runtime_version"]
        .as_str()
        .ok_or(EnvironmentError::Protocol)?;
    if !interpreter.is_absolute()
        || !interpreter.starts_with(root.join(".transflow/runtime/environments"))
        || version != request.runtime_version
    {
        return Err(EnvironmentError::Protocol);
    }
    Ok(ManagedEnvironment {
        query_available: result["query_available"].as_bool().unwrap_or(false),
        interpreter,
        fingerprint: result["fingerprint"]
            .as_str()
            .ok_or(EnvironmentError::Protocol)?
            .into(),
        runtime_version: version.into(),
    })
}
fn invoke(
    root: &Path,
    python: &Path,
    request: &EnvironmentRequest,
    stop: Option<&dyn Fn() -> bool>,
) -> Result<serde_json::Value, EnvironmentError> {
    let mut value = serde_json::to_value(request).map_err(|_| EnvironmentError::Protocol)?;
    value
        .as_object_mut()
        .ok_or(EnvironmentError::Protocol)?
        .insert(
            "root".into(),
            serde_json::Value::String(root.to_string_lossy().into_owned()),
        );
    let json = serde_json::to_string(&value).map_err(|_| EnvironmentError::Protocol)?;
    if json.len() > 64 * 1024 {
        return Err(EnvironmentError::Protocol);
    }
    let mut child = Command::new(python)
        .args(["-I", "-B", "-c", SERVICE, &json])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let output = child.stdout.take().ok_or(EnvironmentError::Protocol)?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(e.into());
            }
        }
        let interrupted = stop.is_some_and(|stop| stop());
        if interrupted || start.elapsed() > Duration::from_secs(1800) {
            if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = child.wait();
            let _ = reader.join();
            return Err(if interrupted {
                EnvironmentError::Interrupted
            } else {
                EnvironmentError::Timeout
            });
        }
        thread::sleep(Duration::from_millis(20));
    };
    let bytes = reader.join().map_err(|_| EnvironmentError::Protocol)??;
    if bytes.len() > 1024 * 1024 {
        return Err(EnvironmentError::Protocol);
    }
    let report: Response =
        serde_json::from_slice(&bytes).map_err(|_| EnvironmentError::Protocol)?;
    if !report.ok || !status.success() {
        let error = report.error.ok_or(EnvironmentError::Protocol)?;
        if error.len() > 2048 {
            return Err(EnvironmentError::Protocol);
        }
        return Err(EnvironmentError::Rejected(error));
    }
    let result = report.result.ok_or(EnvironmentError::Protocol)?;
    let action = result
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or(EnvironmentError::Protocol)?;
    let packages = result
        .get("packages")
        .and_then(|v| v.as_u64())
        .ok_or(EnvironmentError::Protocol)?;
    let digest = result
        .get("fingerprint")
        .or_else(|| result.get("lock_sha256"))
        .and_then(|v| v.as_str())
        .ok_or(EnvironmentError::Protocol)?;
    if !matches!(action, "lock" | "sync" | "check")
        || digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(EnvironmentError::Protocol);
    }
    let _ = packages;
    Ok(result)
}
