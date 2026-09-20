//! Shared local draft/acceptance service. CLI, API and schedules can call the same pipeline.
//! All filesystem hashing/imports/environment inspection run on an owned blocking worker.
use crate::preparation::{self, Inspection};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tf_catalog::{
    RegistrySnapshot,
    candidate::{CandidateError, CandidateIdentity, RegistryProposal},
    capture::SourceSnapshot,
    validation::{self, ValidationRequest},
    workspace::{Workspace, WorkspaceConfig},
};
use tf_domain::{BranchName, BuildId, DatasetKey, RequestId, input::InputBindingKey};
use tf_exec::{discovery, environment, ownership::RuntimeOwner};
use tf_plan::{
    pins::{self, PinRequest},
    refresh,
    scope::{Graph, Mode, Request as ScopeRequest},
};
use tf_store::{
    input_resolution::ReadRequest,
    planning::{DraftPlan, PlannedRead, PlannedWrite},
    retention::LeaseKind,
};
/// Explicit local planning request. Reference strings resolve only within the captured registry.
#[derive(Clone, Debug)]
pub struct Request {
    /// Base interpreter used by the already prepared managed environment.
    pub python: Option<String>,
    /// Resolved output data branch; source selection remains independent.
    pub branch: BranchName,
    /// Default full, selected-only or between.
    pub mode: Mode,
    /// Exact local producer paths or dataset UUID references.
    pub targets: Vec<String>,
    /// Read boundary references.
    pub boundaries: Vec<String>,
    /// Prohibited producer references.
    pub exclusions: Vec<String>,
    /// Explicit source refresh references within the closure.
    pub refresh_sources: Vec<String>,
    /// Parsed per-alias or dataset exact historical pins.
    pub pins: Vec<PinRequest>,
    /// Explicit ordered fallback override; None uses captured workspace policy.
    pub fallbacks: Option<Vec<BranchName>>,
    /// Bypass reuse only within the selected eligible scope.
    pub force: bool,
    /// Typed parameter overrides keyed by exact dataset path/UUID reference; defaults are frozen.
    pub parameters: Value,
}
/// Safe diagnostic retains internal errors without emitting user source or environment paths.
#[derive(Debug, thiserror::Error)]
#[error("Build preparation failed: {0}")]
pub struct Error(String);
fn failure(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}
fn id<T: std::str::FromStr>() -> Result<T, Error> {
    discovery::random_id()
        .map_err(failure)?
        .parse()
        .map_err(|_| failure("invalid generated identity"))
}
/// Ownership remains held until all blocking work actually completes.
pub struct Completion<T> {
    /// Same runtime owner, returned even when a guard or preparation step fails.
    pub owner: RuntimeOwner,
    /// Draft or accepted result; failure launches no transform.
    pub result: Result<T, Error>,
}
/// Frozen accepted build; execution must use its retained source and symbolic job bindings.
#[derive(Debug)]
pub struct Accepted {
    /// New execution identity.
    pub build: BuildId,
    /// Exact accepted proposal, no rediscovery.
    pub plan: DraftPlan,
}
/// Prepare and retain a fifteen-minute draft without authoring/branch changes or producer calls.
pub async fn prepare(
    owner: RuntimeOwner,
    request: Request,
) -> Result<Completion<DraftPlan>, Error> {
    tokio::task::spawn_blocking(move || {
        let mut owner = owner;
        let result = (|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(failure)?;
            runtime.block_on(prepare_inner(&mut owner, request))
        })();
        Completion { owner, result }
    })
    .await
    .map_err(failure)
}
async fn prepare_inner(owner: &mut RuntimeOwner, request: Request) -> Result<DraftPlan, Error> {
    owner.validate_paths().map_err(failure)?;
    if owner.registration().mode() == tf_exec::ownership::CoordinatorMode::MetadataOnly {
        return Err(failure(
            "metadata-only coordinators cannot import or accept producer code",
        ));
    }
    let workspace =
        Workspace::load(owner.workspace_root(), Some(owner.workspace_root())).map_err(failure)?;
    let Inspection {
        capture,
        registry: _,
        mut result,
        graph,
        env,
        python,
        environment_request,
        scratch: _scratch,
        ..
    } = preparation::inspect(&workspace, request.python.as_ref(), true).map_err(failure)?;
    let proposal = graph
        .candidate()
        .propose(|| {
            id().map_err(|_| CandidateError {
                message: "Could not allocate draft IDs",
                locations: vec![],
                cycle: vec![],
            })
        })
        .map_err(failure)?;
    let proposed = RegistrySnapshot::parse(workspace.config().id(), proposal.replacement())
        .map_err(failure)?;
    result["catalog_fingerprint"] = proposed
        .sdk_projection(capture.id().map_err(failure)?)
        .map_err(failure)?["catalog_fingerprint"]
        .clone();
    let fingerprint = result["catalog_fingerprint"]
        .as_str()
        .ok_or_else(|| failure("missing catalogue fingerprint"))?
        .to_owned();
    preparation::rebind(&mut result, &fingerprint);
    let graph = preparation::validate(&proposed, &capture, &result, &env).map_err(failure)?;
    let config = String::from_utf8(
        capture
            .read(Path::new("workspace.toml"), 16 * 1024 * 1024)
            .map_err(failure)?,
    )
    .map_err(failure)?;
    let modules = serde_json::from_value(result["module_index"].clone()).map_err(failure)?;
    let context = ValidationRequest {
        registry: &proposed,
        source: capture.id().map_err(failure)?,
        source_digest: capture.digest(),
        config_toml: &config,
        modules: &modules,
        discovery: &result,
        environment_fingerprint: &env.fingerprint,
        sdk_version: &env.runtime_version,
        check_semantics: validation::CHECK_SEMANTICS,
    };
    let graph_scope = Graph::validated(&graph, &context, &request.branch).map_err(failure)?;
    let bindings = tf_catalog::input_bindings::local_bindings(
        &graph,
        &context,
        &request.branch,
        request.fallbacks.as_deref(),
    )
    .map_err(failure)?;
    let resolve = |refs: &[String]| -> Result<BTreeSet<CandidateIdentity>, Error> {
        refs.iter()
            .map(|r| {
                proposed
                    .resolve(r)
                    .map(|r| CandidateIdentity::Registered(r.key()))
                    .map_err(failure)
            })
            .collect()
    };
    let pins =
        pins::qualify(&request.pins, &proposed, &bindings, &BTreeSet::new()).map_err(failure)?;
    let scope_request = ScopeRequest {
        mode: request.mode,
        targets: resolve(&request.targets)?,
        boundaries: resolve(&request.boundaries)?,
        exclusions: resolve(&request.exclusions)?,
        refresh_sources: resolve(&request.refresh_sources)?,
        pins: pins
            .keys()
            .map(|k| {
                (
                    CandidateIdentity::Registered(k.consumer()),
                    k.alias().into(),
                )
            })
            .collect(),
    };
    let mut scope = graph_scope.select(&scope_request).map_err(failure)?;
    let overrides = request
        .parameters
        .as_object()
        .ok_or_else(|| failure("parameter overrides must be keyed by dataset reference"))?;
    let mut by_dataset = BTreeMap::new();
    for (reference, values) in overrides {
        let key = proposed.resolve_output(reference).map_err(failure)?.key();
        if !scope.writes.contains(&CandidateIdentity::Registered(key))
            || by_dataset.insert(key, values).is_some()
        {
            return Err(failure(
                "parameter dataset is outside the scope or repeated through aliases",
            ));
        }
    }
    let mut parameters = serde_json::Map::new();
    for definition in graph.candidate().definitions() {
        if scope.writes.contains(&definition.output) {
            let key = registered(&definition.output)?;
            let empty = json!({});
            let normalized = tf_catalog::compute::parameters(
                &definition.declaration,
                by_dataset.get(&key).copied().unwrap_or(&empty),
            )
            .map_err(failure)?;
            parameters.insert(key.dataset_id().to_string(), normalized);
        }
    }
    let now = preparation::now().map_err(failure)?;
    let expires = now
        .checked_add(900_000_000)
        .ok_or_else(|| failure("draft clock overflow"))?;
    let plan_id: RequestId = id()?;
    let workspace_id = workspace.config().id();
    let mut owned = owner.open_store().await.map_err(failure)?;
    let store = owned.repository().map_err(failure)?;
    store
        .register_workspace(
            workspace_id,
            workspace
                .root()
                .to_str()
                .ok_or_else(|| failure("workspace path is not UTF-8"))?,
            now,
        )
        .await
        .map_err(failure)?;
    let output = store
        .plan_guard(workspace_id, request.branch.as_str(), None)
        .await
        .map_err(failure)?;
    if output.deleted {
        return Err(failure("output branch was deleted; choose a live branch"));
    }
    let mut decisions = serde_json::Map::new();
    let mut retained = BTreeSet::new();
    let mut guards = Vec::new();
    for identity in &scope.writes {
        let key = registered(identity)?;
        let guard = store
            .plan_guard(
                workspace_id,
                request.branch.as_str(),
                Some(key.dataset_id()),
            )
            .await
            .map_err(failure)?;
        if let Some(policy) = graph_scope.sources().get(identity) {
            let published = store
                .source_publication_time(workspace_id, guard.version.as_deref())
                .await
                .map_err(failure)?;
            let decision = refresh::decide(
                *policy,
                scope_request.targets.contains(identity)
                    || scope_request.refresh_sources.contains(identity),
                request.force,
                now,
                published,
            )
            .map_err(failure)?;
            decisions.insert(key.dataset_id().to_string(),json!({"reason":format!("{decision:?}"),"executes":decision.executes(),"evaluated_us":now.to_string(),"published_us":published.map(|v|v.to_string())}));
            if !decision.executes() {
                retained.insert(identity.clone());
            }
        }
        guards.push(guard);
    }
    scope.writes.retain(|k| !retained.contains(k));
    for b in &mut scope.bindings {
        if retained.contains(&b.parent) {
            b.planned = false;
        }
    }
    let writes_set = scope
        .writes
        .iter()
        .map(registered)
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut reads = Vec::new();
    for selected in &scope.bindings {
        if selected.planned || retained.contains(&selected.consumer) {
            continue;
        }
        let key = InputBindingKey::new(registered(&selected.consumer)?, selected.alias.clone())
            .map_err(failure)?;
        let binding = bindings
            .get(&key)
            .ok_or_else(|| failure("missing validated alias"))?
            .clone();
        let lease: RequestId = id()?;
        let read_request = ReadRequest {
            lease,
            operation: plan_id,
            kind: LeaseKind::Plan,
            now_us: now,
            ttl_us: 900_000_000,
        };
        let read = if let Some(version) = pins.get(&key) {
            store
                .resolve_pin(binding, *version, &writes_set, read_request)
                .await
        } else {
            store
                .resolve_input(binding, &writes_set, read_request)
                .await
        }
        .map_err(failure)?;
        // Record every prior missing fallback as well as the selected head, detecting earlier-head arrival.
        if !read.is_exact_pin() {
            for name in read
                .binding()
                .policy
                .candidates()
                .iter()
                .take(read.fallback_index() + 1)
            {
                let guard = store
                    .plan_guard(
                        workspace_id,
                        name.as_str(),
                        Some(read.binding().dataset.dataset_id()),
                    )
                    .await
                    .map_err(failure)?;
                if name == read.resolved_branch()
                    && guard.version.as_deref() != Some(&read.version().to_string())
                {
                    return Err(failure("input moved while preparing; retry planning"));
                }
                guards.push(guard);
            }
        }
        // This entire pipeline is on a blocking worker retaining ownership; no SQLite transaction spans I/O.
        tf_store::artifacts::ArtifactStore::open(workspace.root())
            .map_err(failure)?
            .verify(read.lease().artifact())
            .map_err(failure)?;
        let p = read.provenance();
        reads.push(PlannedRead{consumer:key.consumer().dataset_id().to_string(),alias:key.alias().into(),dataset:read.binding().dataset.dataset_id().to_string(),version:read.version().to_string(),artifact:read.lease().artifact().hex(),lease:lease.to_string(),semantic:read.semantic_value(),provenance:json!({"alias":p.alias,"workspace":p.dataset.workspace_id().to_string(),"dataset":p.dataset.dataset_id().to_string(),"version":p.version.to_string(),"artifact":p.artifact.hex(),"declared_branch":p.declared_branch,"starting_branch":p.starting_branch.as_str(),"resolved_branch":p.resolved_branch.as_str(),"role":p.role.name(),"resolution":p.resolution})});
    }
    let mut writes = Vec::new();
    for identity in &scope.writes {
        let key = registered(identity)?;
        let generation = guards
            .iter()
            .find(|g| {
                g.name == request.branch.as_str()
                    && g.dataset.as_deref() == Some(&key.dataset_id().to_string())
            })
            .ok_or_else(|| failure("missing output guard"))?
            .generation;
        let input_values=scope.bindings.iter().filter(|b|&b.consumer==identity).map(|b|Ok(json!({"alias":b.alias,"dataset":registered(&b.parent)?.dataset_id().to_string(),"kind":if b.planned{"planned"}else{"boundary"}}))).collect::<Result<Vec<_>,Error>>()?;
        writes.push(PlannedWrite {
            dataset: key.dataset_id().to_string(),
            job: id::<tf_domain::JobId>()?.to_string(),
            generation,
            bindings: json!(input_values),
        });
    }
    let policies = workspace.config().input_policies().map_err(failure)?;
    let mut policy_json = BTreeMap::from([(
        String::new(),
        json!(
            policies
                .defaults()
                .iter()
                .map(BranchName::as_str)
                .collect::<Vec<_>>()
        ),
    )]);
    for (name, tail) in policies.rules() {
        policy_json.insert(
            name.to_string(),
            json!(tail.iter().map(BranchName::as_str).collect::<Vec<_>>()),
        );
    }
    let plan = DraftPlan {
        format_version: 1,
        id: plan_id.to_string(),
        workspace: workspace_id.to_string(),
        source: capture.id().map_err(failure)?.to_string(),
        source_digest: capture.digest().into(),
        registry: String::from_utf8(
            capture
                .read(Path::new(".transflow/catalog.toml"), 16 * 1024 * 1024)
                .map_err(failure)?,
        )
        .map_err(failure)?,
        replacement: proposal.replacement().into(),
        discovery: result,
        environment: json!({"fingerprint":env.fingerprint,"runtime_version":env.runtime_version,"interpreter":env.interpreter,"base_python":python}),
        source_evidence: json!({"selector":{"kind":"working_tree"},"git":capture.git(),"files":capture.manifest_files()}),
        output,
        guards,
        writes,
        reads,
        context: json!({"parameters":parameters,"force":request.force,"mode":format!("{:?}",request.mode),"targets":request.targets,"boundaries":request.boundaries,"exclusions":request.exclusions,"refresh_sources":request.refresh_sources,"source_decisions":decisions,"branch_policies":policy_json,"fallback_override":request.fallbacks.map(|v|v.into_iter().map(|n|n.to_string()).collect::<Vec<_>>()),"configuration":config}),
        created_us: now,
        expires_us: expires,
    };
    capture
        .verify_working_copy(&workspace, false)
        .map_err(failure)?;
    if environment::inspect(workspace.root(), &python, &environment_request).map_err(failure)?
        != env
    {
        return Err(failure("environment changed during planning"));
    }
    store.save_draft(&plan).await.map_err(failure)?;
    store
        .check_draft(&plan, preparation::now().map_err(failure)?)
        .await
        .map_err(failure)?;
    owned.close().await.map_err(failure)?;
    Ok(plan)
}
fn registered(identity: &CandidateIdentity) -> Result<DatasetKey, Error> {
    match identity {
        CandidateIdentity::Registered(key) => Ok(*key),
        _ => Err(failure("draft identity was not assigned")),
    }
}
/// Accept precisely the stored proposal. Failed guards never start execution or silently replan.
pub async fn accept(
    owner: RuntimeOwner,
    plan_id: RequestId,
) -> Result<Completion<Accepted>, Error> {
    tokio::task::spawn_blocking(move || {
        let mut owner = owner;
        let result = (|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(failure)?;
            runtime.block_on(accept_inner(&mut owner, plan_id))
        })();
        Completion { owner, result }
    })
    .await
    .map_err(failure)
}
async fn accept_inner(owner: &mut RuntimeOwner, plan_id: RequestId) -> Result<Accepted, Error> {
    owner.validate_paths().map_err(failure)?;
    if owner.registration().mode() == tf_exec::ownership::CoordinatorMode::MetadataOnly {
        return Err(failure(
            "metadata-only coordinators cannot import or accept producer code",
        ));
    }
    let root = owner.workspace_root().to_owned();
    let workspace = Workspace::load(&root, Some(&root)).map_err(failure)?;
    let mut owned = owner.open_store().await.map_err(failure)?;
    let plan = owned
        .repository()
        .map_err(failure)?
        .load_draft(plan_id)
        .await
        .map_err(failure)?;
    owned
        .repository()
        .map_err(failure)?
        .check_draft(&plan, preparation::now().map_err(failure)?)
        .await
        .map_err(failure)?;
    owned.close().await.map_err(failure)?;
    if plan.workspace != workspace.config().id().to_string() {
        return Err(failure("saved draft belongs to another workspace"));
    }
    let capture = SourceSnapshot::open(
        &root.join(".transflow/runtime/source-snapshots"),
        plan.source.parse().map_err(failure)?,
    )
    .map_err(failure)?;
    if capture.digest() != plan.source_digest {
        return Err(failure("saved source identity changed"));
    }
    capture
        .verify_working_copy(&workspace, true)
        .map_err(failure)?;
    let current_registry = std::fs::read(root.join(".transflow/catalog.toml")).map_err(failure)?;
    if current_registry != plan.registry.as_bytes()
        && current_registry != plan.replacement.as_bytes()
    {
        return Err(failure(
            "catalogue changed since planning; prepare a new plan",
        ));
    }
    let config = WorkspaceConfig::parse(
        plan.context["configuration"]
            .as_str()
            .ok_or_else(|| failure("missing saved configuration"))?,
    )
    .map_err(failure)?;
    let python = PathBuf::from(
        plan.environment["base_python"]
            .as_str()
            .ok_or_else(|| failure("missing original interpreter"))?,
    );
    let env_request = environment::EnvironmentRequest {
        action: environment::EnvironmentAction::Check,
        requirements: config.dependency_paths().0.into(),
        lock: config.dependency_paths().1.into(),
        minor: config.python_version().into(),
        runtime_wheel: None,
        runtime_version: env!("CARGO_PKG_VERSION").replace("-dev", ".dev0"),
        wheelhouse: None,
        offline: true,
    };
    let env = environment::inspect(&root, &python, &env_request).map_err(failure)?;
    if plan.environment["fingerprint"] != env.fingerprint
        || plan.environment["runtime_version"] != env.runtime_version
        || plan.environment["interpreter"] != json!(env.interpreter)
    {
        return Err(failure("managed environment changed since planning"));
    }
    let original = RegistrySnapshot::parse(config.id(), &plan.registry).map_err(failure)?;
    let proposed = RegistrySnapshot::parse(config.id(), &plan.replacement).map_err(failure)?;
    let mut original_discovery = plan.discovery.clone();
    original_discovery["catalog_fingerprint"] = original
        .sdk_projection(capture.id().map_err(failure)?)
        .map_err(failure)?["catalog_fingerprint"]
        .clone();
    let fingerprint = original_discovery["catalog_fingerprint"]
        .as_str()
        .ok_or_else(|| failure("missing saved catalogue context"))?
        .to_owned();
    preparation::rebind(&mut original_discovery, &fingerprint);
    let graph =
        preparation::validate(&original, &capture, &original_discovery, &env).map_err(failure)?;
    let mut ids = graph.candidate().pending().keys().map(|p| {
        proposed
            .resolve_output(p.as_str())
            .map(|d| d.key().dataset_id())
            .map_err(|_| CandidateError {
                message: "Saved proposed identity is absent",
                locations: vec![],
                cycle: vec![],
            })
    });
    let proposal: RegistryProposal = graph
        .candidate()
        .propose(|| {
            ids.next().ok_or(CandidateError {
                message: "Missing saved identity",
                locations: vec![],
                cycle: vec![],
            })?
        })
        .map_err(failure)?;
    if proposal.replacement() != plan.replacement {
        return Err(failure("saved additive registry proposal changed"));
    }
    preparation::validate(&proposed, &capture, &plan.discovery, &env).map_err(failure)?;
    // Registry durability shares its existing recovery-journal pipeline. No new UUID allocation.
    if current_registry != plan.replacement.as_bytes() {
        crate::reconcile::execute_owned(
            owner,
            crate::reconcile::ReconcileRequest {
                capture,
                proposal,
                context: tf_catalog::registry_write::WriteContext::WorkingTree,
                id: id()?,
                at_us: preparation::now().map_err(failure)?,
            },
        )
        .await
        .map_err(failure)?;
    }
    let capture = SourceSnapshot::open(
        &root.join(".transflow/runtime/source-snapshots"),
        plan.source.parse().map_err(failure)?,
    )
    .map_err(failure)?;
    capture
        .verify_working_copy(&workspace, true)
        .map_err(failure)?;
    if std::fs::read(root.join(".transflow/catalog.toml")).map_err(failure)?
        != plan.replacement.as_bytes()
    {
        return Err(failure(
            "catalogue changed after registration; registered outputs remain unbuilt",
        ));
    }
    if environment::inspect(&root, &python, &env_request).map_err(failure)? != env {
        return Err(failure("environment changed during acceptance"));
    }
    for read in &plan.reads {
        tf_store::artifacts::ArtifactStore::open(&root)
            .map_err(failure)?
            .verify(
                tf_protocol::canonical::ContentDigest::from_hex(
                    tf_protocol::canonical::DigestKind::Artifact,
                    &read.artifact,
                )
                .map_err(failure)?,
            )
            .map_err(failure)?;
    }
    capture
        .verify_working_copy(&workspace, true)
        .map_err(failure)?;
    if std::fs::read(root.join(".transflow/catalog.toml")).map_err(failure)?
        != plan.replacement.as_bytes()
    {
        return Err(failure("catalogue changed before acceptance"));
    }
    let session = owner.registration().session().map_err(failure)?;
    let mut owned = owner.open_store().await.map_err(failure)?;
    let store = owned.repository().map_err(failure)?;
    let now = preparation::now().map_err(failure)?;
    store
        .recover_publications(config.id(), session, now)
        .await
        .map_err(failure)?;
    for dataset in proposed.datasets().filter(|d| !d.is_tombstone()) {
        store
            .register_dataset(config.id(), dataset.key().dataset_id(), now)
            .await
            .map_err(failure)?;
    }
    let build = id()?;
    store
        .accept_draft(&plan, build, id()?, session, now)
        .await
        .map_err(failure)?;
    owned.close().await.map_err(failure)?;
    Ok(Accepted { build, plan })
}
