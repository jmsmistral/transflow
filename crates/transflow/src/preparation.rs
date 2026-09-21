//! Shared full-workspace preparation; read-only adapters never acquire a runtime writer.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tf_catalog::{
    DatasetKind, RegistrySnapshot,
    candidate::CandidateError,
    capture::{CaptureLimits, SourceSnapshot},
    editor::{EditorCache, EditorError, Overlay},
    registry_write::WriteContext,
    validation::{self, ValidatedGraph, ValidationRequest},
    workspace::{Workspace, WorkspaceConfig},
};
use tf_domain::{DatasetId, RequestId};
use tf_exec::{
    discovery::{self, Scratch},
    environment::{self, EnvironmentAction, EnvironmentRequest, ManagedEnvironment},
    ownership::{CoordinatorMode, RuntimeOwner},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(
        "Catalogue report exceeds its display limit; reduce --limit or inspect an exact identity"
    )]
    ReportLimit,
    #[error(transparent)]
    Store(#[from] tf_store::StoreError),
    #[error(transparent)]
    Config(#[from] tf_catalog::workspace::ConfigError),
    #[error(transparent)]
    Resources(#[from] crate::resources::Error),
    #[error(transparent)]
    Git(#[from] tf_catalog::git::GitError),
    #[error(transparent)]
    Capture(#[from] tf_catalog::capture::CaptureError),
    #[error(transparent)]
    Registry(#[from] tf_catalog::RegistryError),
    #[error(transparent)]
    Environment(#[from] environment::EnvironmentError),
    #[error(transparent)]
    Discovery(#[from] discovery::DiscoveryError),
    #[error(transparent)]
    Validation(#[from] validation::ValidationError),
    #[error(transparent)]
    Owner(#[from] tf_exec::ownership::OwnershipError),
    #[error(transparent)]
    Reconcile(#[from] crate::reconcile::ReconcileError),
    #[error(transparent)]
    Candidate(#[from] CandidateError),
    #[error(transparent)]
    Editor(#[from] EditorError),
    #[error("Preparation could not access its files")]
    Io(#[from] std::io::Error),
    #[error(
        "Preparation context changed or returned unsupported metadata; retry the complete command"
    )]
    Context,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Validate,
    Sync,
    Check,
}
pub(crate) struct Report {
    pub value: Value,
    pub changes_required: bool,
}
impl Report {
    pub fn human(&self) -> Result<String, tf_domain::diagnostic::DiagnosticError> {
        let redactor = tf_domain::diagnostic::Redactor::default();
        let mut text = format!(
            "Structural validation passed. Deferred schema/data checks: {}.\n",
            self.value["deferred_checks"].as_str().unwrap_or("0")
        );
        for (key, label) in [
            ("registrations", "Registrations"),
            ("unchanged", "Unchanged producers"),
            ("absent_producers", "Absent producers (retained)"),
        ] {
            let part = &self.value[key];
            text.push_str(&format!(
                "{label}: {}\n",
                part["total"].as_str().unwrap_or("0")
            ));
            if let Some(entries) = part["entries"].as_array() {
                for entry in entries {
                    text.push_str(&format!(
                        "  {}{}\n",
                        redactor
                            .text(entry["path"].as_str().unwrap_or("unknown"))?
                            .as_str(),
                        entry["id"]
                            .as_str()
                            .map(|id| format!(" ({id})"))
                            .unwrap_or_default()
                    ));
                }
                if part["total"]
                    .as_str()
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0)
                    > entries.len()
                {
                    text.push_str("  Display limited to the first 64 entries.\n");
                }
            }
        }
        if self.changes_required {
            text.push_str("Catalogue additions are required; run catalog sync.\n");
        }
        Ok(text)
    }
}
fn limits() -> CaptureLimits {
    CaptureLimits {
        file_bytes: 16 * 1024 * 1024,
        total_bytes: 256 * 1024 * 1024,
    }
}
fn text(capture: &SourceSnapshot, path: &str) -> Result<String, Error> {
    String::from_utf8(capture.read(Path::new(path), 16 * 1024 * 1024)?).map_err(|_| Error::Context)
}
pub(crate) fn validate(
    registry: &RegistrySnapshot,
    capture: &SourceSnapshot,
    result: &Value,
    env: &ManagedEnvironment,
) -> Result<ValidatedGraph, Error> {
    let modules: BTreeMap<String, String> =
        serde_json::from_value(result["module_index"].clone()).map_err(|_| Error::Context)?;
    Ok(validation::validate(&ValidationRequest {
        registry,
        source: capture.id()?,
        source_digest: capture.digest(),
        config_toml: &text(capture, "workspace.toml")?,
        modules: &modules,
        discovery: result,
        environment_fingerprint: &env.fingerprint,
        sdk_version: &env.runtime_version,
        check_semantics: validation::CHECK_SEMANTICS,
    })?)
}
pub(crate) fn rebind(value: &mut Value, fingerprint: &str) {
    fn reference(value: &mut Value, fingerprint: &str) {
        if value["form"] == "bound" {
            value["catalog_fingerprint"] = fingerprint.into();
        }
    }
    if let Some(definitions) = value["definitions"].as_array_mut() {
        for definition in definitions {
            reference(&mut definition["output"]["ref"], fingerprint);
            if let Some(inputs) = definition["inputs"].as_array_mut() {
                for input in inputs {
                    reference(&mut input["ref"], fingerprint);
                }
            }
        }
    }
}
fn section(entries: Vec<Value>) -> Value {
    json!({"total":entries.len().to_string(),"entries":entries.into_iter().take(64).collect::<Vec<_>>()})
}
/// Complete verified discovery retained for an explicit catalogue operation.
pub(crate) struct Inspection {
    pub capture: SourceSnapshot,
    pub registry: RegistrySnapshot,
    pub result: Value,
    pub graph: ValidatedGraph,
    pub env: ManagedEnvironment,
    pub python: PathBuf,
    pub environment_request: EnvironmentRequest,
    pub request: RequestId,
    pub scratch: Scratch,
}
pub(crate) fn inspect(
    workspace: &Workspace,
    python: Option<&String>,
    persistent: bool,
) -> Result<Inspection, Error> {
    inspect_overlay(workspace, python, persistent, None)
}
/// Explicit import registration may supply a validated pending registry overlay before imports.
/// The captured original registry must match its exact expected-old bytes.
pub(crate) fn inspect_overlay(
    workspace: &Workspace,
    python: Option<&String>,
    persistent: bool,
    overlay: Option<(&RegistrySnapshot, &RegistrySnapshot)>,
) -> Result<Inspection, Error> {
    inspect_selected(workspace, python, persistent, overlay, None)
}
/// Explicit source selection never changes the caller checkout.
pub(crate) fn inspect_selected(
    workspace: &Workspace,
    python: Option<&String>,
    persistent: bool,
    overlay: Option<(&RegistrySnapshot, &RegistrySnapshot)>,
    selected: Option<(&str, &tf_domain::BranchName)>,
) -> Result<Inspection, Error> {
    let current = std::env::current_dir()?;
    let scratch = Scratch::create()?;
    let capture = if let Some((reference, branch)) = selected {
        tf_catalog::git::capture_ref(workspace, reference, Some(branch), limits())?
    } else if persistent {
        tf_catalog::git::capture_working_tree(workspace, limits())?
    } else {
        tf_catalog::git::capture_temporary(workspace, scratch.path(), limits())?
    };
    let config = WorkspaceConfig::parse(&text(&capture, "workspace.toml")?)?;
    // The installed environment must match the selected source dependencies.
    if selected.is_some() {
        for path in [config.dependency_paths().0, config.dependency_paths().1] {
            if capture.read(Path::new(path), 16 * 1024 * 1024)?
                != std::fs::read(workspace.root().join(path))?
            {
                return Err(Error::Context);
            }
        }
    }
    let python = python
        .map(PathBuf::from)
        .or_else(|| workspace.local().interpreter().map(Path::to_owned))
        .unwrap_or_else(|| PathBuf::from(format!("python{}", config.python_version())));
    let python = if python.is_relative() && python.components().count() > 1 {
        current.join(python)
    } else {
        python
    };
    let environment_request = EnvironmentRequest {
        action: EnvironmentAction::Check,
        requirements: config.dependency_paths().0.into(),
        lock: config.dependency_paths().1.into(),
        minor: config.python_version().into(),
        runtime_wheel: None,
        runtime_version: env!("CARGO_PKG_VERSION").replace("-dev", ".dev0"),
        wheelhouse: None,
        offline: true,
    };
    let env = environment::inspect(workspace.root(), &python, &environment_request)?;
    let request: RequestId = discovery::random_id()?
        .parse()
        .map_err(|_| Error::Context)?;
    let registry =
        RegistrySnapshot::parse(config.id(), &text(&capture, ".transflow/catalog.toml")?)?;
    let registry = if let Some((original, proposed)) = overlay {
        if registry.raw_digest() != original.raw_digest()
            || registry.workspace() != proposed.workspace()
        {
            return Err(Error::Context);
        }
        proposed.clone()
    } else {
        registry
    };
    let resource_policy = config.execution_policy(
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )?;
    let resolved_resources = crate::resources::Resolved::new(&resource_policy)?;
    let result = discovery::discover(
        &env.interpreter,
        &scratch,
        json!({"format_version":1,"protocol":{"major":1,"minor":0},"request_id":request.to_string(),"attempt_id":discovery::random_id()?,"capture_root":capture.files_root(),"source_roots":capture.source_roots(),"files":capture.discovery_files(),"catalog":registry.sdk_projection(capture.id()?)?,"environment_fingerprint":env.fingerprint}),
        resolved_resources.timers,
        resolved_resources.worker_threads,
    )?;
    let graph = validate(&registry, &capture, &result, &env)?;
    verify_selection(&capture, workspace, false)?;
    if environment::inspect(workspace.root(), &python, &environment_request)? != env {
        return Err(Error::Context);
    }
    Ok(Inspection {
        capture,
        registry,
        result,
        graph,
        env,
        python,
        environment_request,
        request,
        scratch,
    })
}
pub(crate) fn verify_selection(
    capture: &SourceSnapshot,
    workspace: &Workspace,
    registry_replaced: bool,
) -> Result<(), Error> {
    if capture.git().is_some_and(|g| g.requested_ref().is_some()) {
        let verified = SourceSnapshot::open(
            &workspace.root().join(".transflow/runtime/source-snapshots"),
            capture.id()?,
        )?;
        if verified.digest() != capture.digest() {
            return Err(Error::Context);
        }
        let config = WorkspaceConfig::parse(&text(capture, "workspace.toml")?)?;
        for path in [config.dependency_paths().0, config.dependency_paths().1] {
            if capture.read(Path::new(path), 16 * 1024 * 1024)?
                != std::fs::read(workspace.root().join(path))?
            {
                return Err(Error::Context);
            }
        }
        Ok(())
    } else {
        Ok(capture.verify_working_copy(workspace, registry_replaced)?)
    }
}
pub(crate) fn execute(
    operation: Operation,
    python: Option<&String>,
    explicit: Option<&String>,
) -> Result<Report, Error> {
    let current = std::env::current_dir()?;
    let workspace = Workspace::load(&current, explicit.map(Path::new))?;
    let config = workspace.config();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut owner = if operation == Operation::Sync {
        let mut owner =
            RuntimeOwner::acquire(workspace.root(), config.id(), CoordinatorMode::Temporary)?;
        let recovered = runtime.block_on(crate::reconcile::recover(&mut owner, now()?))?;
        if recovered
            .iter()
            .any(|o| matches!(o, crate::reconcile::RecoveryOutcome::Conflict(_)))
        {
            return Err(Error::Context);
        }
        Some(owner)
    } else {
        None
    };
    let Inspection {
        capture,
        registry,
        mut result,
        mut graph,
        env,
        python,
        environment_request,
        request,
        scratch: _scratch,
    } = inspect(&workspace, python, owner.is_some())?;
    let pending: BTreeSet<_> = graph
        .candidate()
        .pending()
        .keys()
        .map(ToString::to_string)
        .collect();
    let produced: BTreeSet<_> = graph
        .candidate()
        .definitions()
        .iter()
        .map(|d| d.path.to_string())
        .collect();
    let absent = section(
        registry
            .datasets()
            .filter(|d| {
                !d.is_tombstone()
                    && matches!(d.kind(), DatasetKind::Source | DatasetKind::Transform)
                    && !produced.contains(&d.path().to_string())
            })
            .map(|d| json!({"path":d.path().to_string(),"id":d.key().dataset_id().to_string()}))
            .collect(),
    );
    let mut final_registry = registry.clone();
    let final_capture = if let Some(taken) = owner.take() {
        let proposal = graph.candidate().propose(|| {
            discovery::random_id()
                .ok()
                .and_then(|s| s.parse::<DatasetId>().ok())
                .ok_or(CandidateError {
                    message: "Could not allocate a dataset identity",
                    locations: vec![],
                    cycle: vec![],
                })
        })?;
        let expected_registry = proposal.replacement().to_owned();
        let id = capture.id()?;
        let completion = runtime.block_on(crate::reconcile::reconcile(
            taken,
            crate::reconcile::ReconcileRequest {
                capture,
                proposal,
                context: WriteContext::WorkingTree,
                id: request,
                at_us: now()?,
            },
        ))?;
        owner = Some(completion.owner);
        completion.outcome?;
        let previous = SourceSnapshot::open(
            &workspace.root().join(".transflow/runtime/source-snapshots"),
            id,
        )?;
        if pending.is_empty() {
            previous
        } else {
            previous.verify_working_copy(&workspace, true)?;
            let updated = SourceSnapshot::capture(&workspace, limits())?;
            previous.verify_working_copy(&workspace, true)?;
            // Only the already validated additive registry delta may change the source context.
            if !previous.same_inputs_except_registry(&updated)
                || text(&updated, ".transflow/catalog.toml")? != expected_registry
            {
                return Err(Error::Context);
            }
            final_registry =
                RegistrySnapshot::parse(config.id(), &text(&updated, ".transflow/catalog.toml")?)?;
            result["source_snapshot_id"] = updated.id()?.to_string().into();
            let projection = final_registry.sdk_projection(updated.id()?)?;
            result["catalog_fingerprint"] = projection["catalog_fingerprint"].clone();
            rebind(
                &mut result,
                projection["catalog_fingerprint"]
                    .as_str()
                    .ok_or(Error::Context)?,
            );
            graph = validate(&final_registry, &updated, &result, &env)?;
            updated
        }
    } else {
        capture
    };
    if environment::inspect(workspace.root(), &python, &environment_request)? != env {
        return Err(Error::Context);
    }
    if let Some(owner) = owner.as_ref() {
        let guard = || {
            owner.validate_paths().map_err(|_| EditorError::Conflict)?;
            final_capture
                .verify_working_copy(&workspace, false)
                .map_err(|_| EditorError::Conflict)
        };
        let overlay = Overlay::render(&final_registry.sdk_projection(final_capture.id()?)?)?;
        EditorCache::open(workspace.root())?.refresh(&overlay, request, |_| guard())?;
        tf_catalog::graph_cache::retain(
            workspace.root(),
            final_capture.id()?,
            &graph,
            &result,
            request,
            guard,
        )?;
    }
    let entries = |selected: bool| {
        section(produced.iter().filter(|p|pending.contains(*p)==selected).map(|p|json!({"path":p,"id":final_registry.resolve_output(p).ok().map(|d|d.key().dataset_id().to_string())})).collect())
    };
    let value = json!({"kind":"preparation","operation":match operation {Operation::Validate=>"validate",Operation::Sync=>"catalog_sync",Operation::Check=>"catalog_sync_check"},"source_snapshot_id":final_capture.id()?.to_string(),"catalog_fingerprint":final_registry.sdk_projection(final_capture.id()?)?["catalog_fingerprint"],"certificate_fingerprint":graph.certificate().fingerprint().hex(),"registrations":entries(true),"unchanged":entries(false),"absent_producers":absent,"deferred_checks":graph.deferred().len().to_string()});
    Ok(Report {
        value,
        changes_required: operation == Operation::Check && !pending.is_empty(),
    })
}
pub(crate) fn now() -> Result<i64, Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_micros()).ok())
        .ok_or(Error::Context)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_rebinding_never_changes_user_parameter_values() {
        let bound = json!({"form":"bound","catalog_fingerprint":"old"});
        let mut value = json!({"definitions":[{"output":{"ref":bound},"inputs":[{"ref":bound}],"params":{"default":bound}}]});
        rebind(&mut value, "new");
        assert_eq!(
            value["definitions"][0]["output"]["ref"]["catalog_fingerprint"],
            "new"
        );
        assert_eq!(
            value["definitions"][0]["inputs"][0]["ref"]["catalog_fingerprint"],
            "new"
        );
        assert_eq!(value["definitions"][0]["params"]["default"], bound);
    }
}
