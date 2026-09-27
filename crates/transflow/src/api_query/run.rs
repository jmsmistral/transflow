//! One admitted query: exact protected bindings, fresh supervised helper, bounded projection.
use super::*;
use crate::{
    api_read::Context,
    build_plan::{Error, failure},
    external_reads::Reads,
    provider::Selection,
    resources::Resolved,
};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use tf_catalog::{Resolved as CatalogEntry, workspace::Workspace};
use tf_domain::VersionId;
use tf_exec::{
    admission::{Demand, Reservation},
    environment::{self, EnvironmentAction, EnvironmentRequest},
    supervisor::{self, Launch, Policy},
    timing::{Budget, Phase, Work},
};
use tf_protocol::canonical::{DigestKind, content_digest};
struct Binding {
    root: PathBuf,
    origin: WorkspaceId,
    selection: Selection,
    version: VersionId,
}
pub(super) struct Input {
    pub limits: Value,
    pub resources: Resolved,
    body: Value,
    bindings: Vec<Binding>,
}
pub(super) fn environment(
    root: &Path,
    stop: &dyn Fn() -> bool,
) -> std::result::Result<PathBuf, Error> {
    let workspace = Workspace::load(root, Some(root)).map_err(failure)?;
    let config = workspace.config();
    let python = environment::recorded_interpreter(root).map_err(failure)?;
    let env = environment::inspect_interruptible(
        root,
        &python,
        &EnvironmentRequest {
            action: EnvironmentAction::Check,
            requirements: config.dependency_paths().0.into(),
            lock: config.dependency_paths().1.into(),
            minor: config.python_version().into(),
            runtime_wheel: None,
            runtime_version: env!("CARGO_PKG_VERSION").replace("-dev", ".dev0"),
            wheelhouse: None,
            offline: true,
        },
        stop,
    )
    .map_err(failure)?;
    if !env.query_available {
        return Err(failure("Qualified query dependencies are unavailable"));
    }
    Ok(env.interpreter)
}
pub(super) fn prepare(
    root: &Path,
    wid: WorkspaceId,
    c: &Context,
    body: &Value,
    pool: &Admission,
) -> Result<Input> {
    tf_protocol::validate_document("ApiQueryRequestV1", body).map_err(|_| E::invalid())?;
    let workspace = Workspace::load(root, Some(root)).map_err(bad)?;
    let config = workspace.config();
    let resources = Resolved::new(
        &config
            .execution_policy(
                &Default::default(),
                &Default::default(),
                &Default::default(),
            )
            .map_err(bad)?,
    )
    .map_err(bad)?;
    if resources.capacity != pool.capacity() {
        return Err(E::new(
            409,
            "TF_API_QUERY_CAPACITY",
            "Resource settings changed; restart serve",
        ));
    }
    let (_, rows, bytes) = config.interactive_limits();
    let rows = body["rows"]
        .as_u64()
        .unwrap_or(u64::from(rows))
        .min(u64::from(rows))
        .min(1000);
    let bytes = body["bytes"]
        .as_u64()
        .unwrap_or(bytes)
        .min(bytes)
        .min(2097152);
    let spill = body["spill_bytes"]
        .as_str()
        .map(str::parse::<u64>)
        .transpose()
        .map_err(|_| E::invalid())?
        .unwrap_or(268435456);
    if spill > 268435456 {
        return Err(E::invalid());
    }
    let limits = json!({"rows":rows,"bytes":bytes,"spill_bytes":spill.to_string(),"threads":resources.worker_threads,"memory_bytes":resources.capacity.memory_bytes.map(|v|v.to_string())});
    tf_protocol::validate_document("QueryLimitsV1", &limits).map_err(|_| E::invalid())?;
    let entries = tf_catalog::browse::entries(&c.registry);
    let mut bindings = Vec::new();
    let mut aliases = std::collections::BTreeSet::new();
    for b in body["bindings"].as_array().ok_or_else(E::invalid)? {
        let text = |k: &str| b[k].as_str().ok_or_else(E::invalid);
        let alias = text("alias")?;
        if !aliases.insert(alias.to_lowercase()) {
            return Err(E::invalid());
        }
        let origin = text("origin_workspace")?
            .parse::<WorkspaceId>()
            .map_err(|_| E::invalid())?;
        let entry = entries
            .iter()
            .find(|e| e["dataset_id"] == b["dataset"] && e["workspace_id"] == b["origin_workspace"])
            .ok_or_else(E::missing)?;
        let (provider, registered, fallback) = if origin == wid {
            (root.to_path_buf(), c.branch.to_string(), None)
        } else {
            let CatalogEntry::External(reg) = c
                .registry
                .resolve(entry["path"].as_str().ok_or_else(E::missing)?)
                .map_err(bad)?
            else {
                return Err(E::missing());
            };
            let path = workspace.local().provider(origin).ok_or_else(E::missing)?;
            (
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                },
                reg.default_branch().to_string(),
                reg.fallback_override()
                    .map(|v| v.iter().map(ToString::to_string).collect()),
            )
        };
        bindings.push(Binding {
            root: provider,
            origin,
            version: text("version")?.parse().map_err(|_| E::invalid())?,
            selection: Selection {
                consumer_workspace: wid.to_string(),
                consumer_dataset: text("dataset")?.into(),
                alias: alias.into(),
                dataset: text("dataset")?.into(),
                output: c.branch.to_string(),
                registered,
                declared: json!({"kind":"omitted","name":null}),
                stop: true,
                role: "data".into(),
                fallback,
            },
        });
    }
    Ok(Input {
        limits,
        resources,
        body: body.clone(),
        bindings,
    })
}
type Outcome = (
    &'static str,
    Option<String>,
    Option<tf_store::query_results::Page>,
    u128,
);
pub(super) fn execute(manager: &Manager, id: &str, input: Input, cancel: &Cancellation) -> Outcome {
    let started = Instant::now();
    let result = execute_inner(manager, id, input, cancel);
    match result {
        Ok(page) => ("SUCCEEDED", None, Some(page), started.elapsed().as_millis()),
        Err((state, error)) => (state, Some(error), None, started.elapsed().as_millis()),
    }
}
fn execute_inner(
    manager: &Manager,
    id: &str,
    input: Input,
    cancel: &Cancellation,
) -> std::result::Result<tf_store::query_results::Page, (&'static str, String)> {
    let failed = |_: Error| {
        (
            "FAILED",
            "Query failed; check SQL, exact input versions, qualified types and resource limits"
                .into(),
        )
    };
    let canceled = || {
        (
            "CANCELED",
            "Query canceled; helper and temporary results have been cleaned up".into(),
        )
    };
    let rt = api_read::runtime().map_err(|_| failed(failure("runtime")))?;
    let demand = Demand {
        cpu: input.resources.worker_threads,
        memory_bytes: input.resources.capacity.memory_bytes,
        ..Default::default()
    };
    let request = manager
        .pool
        .request(demand)
        .map_err(|e| failed(failure(e)))?;
    let reservation: Reservation = rt.block_on(async {
        tokio::pin!(request);
        loop {
            if cancel.requested() {
                return Err(canceled());
            }
            tokio::select! {
                r = &mut request => return r.map_err(|e| failed(failure(e))),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {},
            }
        }
    })?;
    let budget = Budget::new(input.resources.timers.clone(), "scratchpad".into())
        .map_err(|e| failed(failure(e)))?;
    budget
        .enter(Phase::Interactive)
        .map_err(|e| failed(failure(e)))?;
    if let Ok(mut jobs) = manager.jobs.lock()
        && let Some(j) = jobs.get_mut(id)
    {
        j.status["state"] = json!("RUNNING");
    }
    let check = || -> std::result::Result<(), (&'static str, String)> {
        if cancel.requested() {
            return Err(canceled());
        }
        if let Some(timeout) = budget.expired().map_err(|e| failed(failure(e)))? {
            return Err(("TIMED_OUT", timeout.to_string()));
        }
        Ok(())
    };
    check()?;
    let inspected = environment(&manager.root, &|| check().is_err());
    check()?;
    let python = inspected.map_err(failed)?;
    let mut reads = Reads::empty(cancel.clone());
    let mut bindings = Vec::new();
    for b in input.bindings {
        bindings.push(
            reads
                .query_binding(b.root, b.origin, b.selection, b.version, id)
                .map_err(failed)?,
        );
        check()?;
    }
    let temp = Temporary::new(&manager.root, id).map_err(failed)?;
    let result_dir = temp.0.join("results");
    let helper_dir = temp.0.join("helper");
    private_directory(&result_dir).map_err(failed)?;
    private_directory(&helper_dir).map_err(failed)?;
    let request = json!({"format_version":1,"protocol":{"major":1,"minor":0},"request_id":id,"attempt_id":id,"auth_token":"00".repeat(32),"limits":input.limits,
        "query":{"format_version":1,"request_id":id,"sql":input.body["sql"],"parameters":input.body["parameters"],"bindings":bindings,"result_directory":result_dir}});
    let report = supervisor::run(
        Launch {
            python,
            operation: tf_protocol::Operation::QueryPreview,
            request_schema: "QueryExecutionRequestV1",
            phase_events: None,
            request,
            policy: Policy::default(),
            log_directory: Some(helper_dir),
            redact: Vec::new(),
            timing: Some(Work {
                budget: budget.clone(),
                phase: Phase::Interactive,
            }),
            reservation: Some(reservation.worker().map_err(|e| failed(failure(e)))?),
            threads: input.resources.worker_threads,
        },
        cancel.clone(),
    );
    if let Some(timeout) = report.timeout {
        return Err(("TIMED_OUT", timeout.to_string()));
    }
    check()?;
    report
        .outcome
        .map_err(|e| ("FAILED", format!("Query helper failed: {e}")))?;
    reads.check().map_err(failed)?;
    let result = accept(
        &result_dir,
        id,
        &bindings,
        &input.limits,
        report.result.as_ref(),
    )
    .map_err(failed)?;
    check()?;
    reads.check().map_err(failed)?;
    drop(reads);
    // Completion is published only after process reap, lease release and file cleanup.
    temp.remove().map_err(failed)?;
    Ok(result)
}
fn accept(
    directory: &Path,
    id: &str,
    bindings: &[Value],
    limits: &Value,
    frame: Option<&tf_protocol::ControlFrame>,
) -> std::result::Result<tf_store::query_results::Page, Error> {
    use std::io::Read;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(directory.join("result.json"))
        .map_err(failure)?;
    let metadata = file.metadata().map_err(failure)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() > 65536
        || metadata.mode() & 0o077 != 0
    {
        return Err(failure("Invalid query result"));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() > 65536 {
        return Err(failure("Query result too large"));
    }
    let result: Value = serde_json::from_slice(&bytes).map_err(failure)?;
    tf_protocol::validate_document("ScratchpadResultV1", &result).map_err(failure)?;
    let message = &frame
        .ok_or_else(|| failure("Missing query result"))?
        .as_json()["message"];
    let digest = content_digest(DigestKind::Compute, &result).map_err(failure)?;
    let expected: Vec<_> = bindings.iter().map(|b| json!({"alias":b["alias"],"workspace_id":b["workspace_id"],"dataset_id":b["dataset_id"],"version_id":b["version_id"],"artifact_digest":b["artifact_digest"]})).collect();
    if message["results_path"] != "result.json"
        || message["results_digest"] != digest.hex()
        || result["request_id"] != id
        || result["bindings"] != json!(expected)
    {
        return Err(failure("Query result identity mismatch"));
    }
    let mut page = tf_store::query_results::read(
        &directory.join("result.arrow"),
        limits["rows"].as_u64().ok_or_else(|| failure("limit"))? as usize,
        limits["bytes"].as_u64().ok_or_else(|| failure("limit"))? as usize,
    )
    .map_err(failure)?;
    if !page.truncated
        && result["row_count"]
            .as_str()
            .and_then(|n| n.parse::<usize>().ok())
            != Some(page.rows.len())
    {
        return Err(failure("Query row count mismatch"));
    }
    page.truncated |= result["truncated"].as_bool().unwrap_or(false);
    Ok(page)
}
fn private_directory(path: &Path) -> std::result::Result<(), Error> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(failure)
}
struct Temporary(PathBuf);
impl Temporary {
    fn new(root: &Path, id: &str) -> std::result::Result<Self, Error> {
        let parent = root.join(".transflow/runtime/queries");
        match private_directory(&parent) {
            Ok(()) => {}
            Err(_) if parent.exists() => {}
            Err(e) => return Err(e),
        }
        let m = fs::symlink_metadata(&parent).map_err(failure)?;
        if !m.is_dir()
            || m.mode() & 0o077 != 0
            || fs::canonicalize(&parent).map_err(failure)? != parent
        {
            return Err(failure("Unsafe query directory"));
        }
        let path = parent.join(id);
        private_directory(&path)?;
        Ok(Self(path))
    }
    fn remove(self) -> std::result::Result<(), Error> {
        fs::remove_dir_all(&self.0).map_err(failure)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
