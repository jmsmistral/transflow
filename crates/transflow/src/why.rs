//! Shared freshness/why composition. No public CLI command or execution fallback.
use crate::build_plan::{Completion, Error, failure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use tf_catalog::{
    RegistrySnapshot,
    candidate::CandidateIdentity as Id,
    capture::SourceSnapshot,
    compute,
    validation::{self, ValidatedGraph, ValidationRequest},
    workspace::Workspace,
};
use tf_domain::{BranchName, DatasetKey, RequestId, input::InputBindingKey};
use tf_exec::{
    discovery,
    ownership::{CoordinatorMode, RuntimeOwner},
};
use tf_plan::{
    freshness::{self, AttemptState, Facts, InputQuality, Materialization, Quality, Status},
    scope::Graph,
};
use tf_protocol::canonical::{ContentDigest, DigestKind};
use tf_store::{
    artifacts::{ArtifactError, ArtifactStore},
    freshness::{Attempt, Head, Snapshot},
    retention::{LeaseKind, ReadTarget},
};

/// Caller-supplied semantic inputs must be the same policies used for cache/publication.
#[derive(Clone, Debug)]
pub struct Semantics {
    /// Typed parameter overrides, with omitted entries using captured defaults.
    pub parameters: Value,
    /// Complete writer/normalization policy.
    pub writer: Value,
    /// Frozen relative evaluation clock, if observed by the computation.
    pub evaluation_us: Option<i64>,
    /// Immutable version identifiers only; no secret values.
    pub secret_versions: BTreeMap<String, String>,
}
/// Internal why request. Missing semantic policy is explicitly unknown.
#[derive(Clone, Debug)]
pub struct Request {
    /// Prepared interpreter for captured discovery.
    pub python: Option<String>,
    /// Explicit data branch, independent of source selection.
    pub branch: BranchName,
    /// Overrides obey the same named-branch rules as build planning.
    pub fallbacks: Option<Vec<BranchName>>,
    /// Trusted computation policies by stable dataset identity.
    pub semantics: BTreeMap<DatasetKey, Semantics>,
    /// One frozen UTC clock used for all source TTL decisions.
    pub at_us: i64,
}
/// Contextual read model consumed unchanged by future CLI/API/UI adapters.
#[derive(Debug)]
pub struct Report {
    /// Exact captured source identity.
    pub source: String,
    /// Selected output branch.
    pub branch: BranchName,
    /// Event watermark of the single metadata snapshot.
    pub event_sequence: i64,
    /// Selected graph producer facts.
    pub datasets: BTreeMap<Id, Status>,
}
fn attempt_state(a: &Attempt) -> AttemptState {
    match a.state.as_str() {
        "SUCCEEDED" => AttemptState::Succeeded,
        "FAILED" => AttemptState::Failed,
        "CANCELED" => AttemptState::Canceled,
        "INTERRUPTED" => AttemptState::Interrupted,
        _ => AttemptState::Running,
    }
}
fn output_quality(head: Option<&Head>) -> Quality {
    let Some(head) = head else {
        return Quality::Unavailable;
    };
    let Some(checks) = head.contract.as_ref().and_then(|c| c["checks"].as_array()) else {
        return Quality::Unavailable;
    };
    let required: Vec<_> = checks
        .iter()
        .filter(|c| c["subject"]["output"] == true)
        .collect();
    let actual: Vec<_> = head
        .checks
        .iter()
        .filter(|c| c.subject.get("alias").is_none())
        .collect();
    if actual.len() != required.len() {
        return Quality::Unavailable;
    }
    if required.is_empty() {
        return Quality::NoChecks;
    }
    let mut quality = Quality::Passed;
    for obligation in required {
        let found: Vec<_> = actual
            .iter()
            .filter(|c| {
                Some(c.definition.as_str()) == obligation["definition"].as_str()
                    && c.subject == json!({"artifact_digest":head.artifact})
            })
            .collect();
        let [check] = found.as_slice() else {
            return Quality::Unavailable;
        };
        if check.finished_us.is_none_or(|t| t > head.published_us) {
            return Quality::Unavailable;
        }
        match check.outcome.as_str() {
            "PASS" => (),
            "VIOLATION" if obligation["required"] == false => quality = Quality::Warning,
            _ => return Quality::Unavailable,
        }
    }
    quality
}
fn input_quality(attempt: Option<&Attempt>) -> BTreeMap<String, InputQuality> {
    let mut result = BTreeMap::new();
    let Some(attempt) = attempt else {
        return result;
    };
    if let Some(obligations) = attempt
        .contract
        .as_ref()
        .and_then(|c| c["checks"].as_array())
    {
        for obligation in obligations {
            let Some(alias) = obligation["subject"]["input"].as_str() else {
                continue;
            };
            let input = attempt
                .contract
                .as_ref()
                .and_then(|c| c["inputs"].as_array())
                .and_then(|inputs| inputs.iter().find(|i| i["alias"] == alias));
            let expected=input.filter(|i|i["artifact"].is_string() && i["version"].is_string()).map(|i|json!({"alias":alias,"artifact_digest":i["artifact"],"version_id":i["version"]}));
            let found: Vec<_> = attempt
                .checks
                .iter()
                .filter(|c| {
                    Some(c.definition.as_str()) == obligation["definition"].as_str()
                        && expected.as_ref() == Some(&c.subject)
                })
                .collect();
            let quality = match found.as_slice() {
                [c] if c.finished_us.is_some() && obligation["required"].is_boolean() => {
                    match c.outcome.as_str() {
                        "PASS" => InputQuality::Passed,
                        "VIOLATION" if obligation["required"] == false => InputQuality::Warning,
                        "VIOLATION" | "ERROR" => InputQuality::Failed,
                        _ => InputQuality::Unavailable,
                    }
                }
                _ => InputQuality::Unavailable,
            };
            let old = result.entry(alias.into()).or_insert(InputQuality::Passed);
            let rank = |q: InputQuality| match q {
                InputQuality::Passed => 0,
                InputQuality::Warning => 1,
                InputQuality::Unavailable => 2,
                InputQuality::Failed => 3,
            };
            if rank(quality) > rank(*old) {
                *old = quality;
            }
        }
    }
    result
}
/// Evaluate only this immutable capture and this transaction's heads. No live reads occur here.
/// Artifact observations must concern these exact digests; omitted observations remain Unknown.
pub fn explain(
    graph: &ValidatedGraph,
    context: &ValidationRequest<'_>,
    capture: &SourceSnapshot,
    snapshot: &Snapshot,
    observations: &BTreeMap<String, Materialization>,
    request: &Request,
) -> Result<Report, Error> {
    if snapshot.workspace != context.registry.workspace()
        || capture.id().map_err(failure)? != context.source
        || capture.digest() != context.source_digest
    {
        return Err(failure("freshness source or workspace context differs"));
    }
    let scope = Graph::validated(graph, context, &request.branch).map_err(failure)?;
    // Pending registrations and foreign policies cannot be invented by a read-only explanation.
    let bindings = tf_catalog::input_bindings::local_read_bindings(
        graph,
        context,
        &request.branch,
        request.fallbacks.as_deref(),
    )
    .map_err(failure)?;
    let mut facts = BTreeMap::new();
    for definition in graph.candidate().definitions() {
        let Id::Registered(key) = definition.output else {
            facts.insert(
                definition.output.clone(),
                Facts {
                    version: None,
                    materialization: Materialization::NeverBuilt,
                    retained: None,
                    current: None,
                    published_us: None,
                    latest_attempt: None,
                    output_quality: Quality::Unavailable,
                    input_quality: BTreeMap::new(),
                    boundaries: BTreeSet::new(),
                    policies: BTreeMap::new(),
                    input_artifacts: BTreeMap::new(),
                    fallbacks: BTreeMap::new(),
                },
            );
            continue;
        };
        let head = snapshot
            .heads
            .get(&(key.dataset_id(), request.branch.clone()));
        let attempt = snapshot
            .attempts
            .get(&(key.dataset_id(), request.branch.clone()));
        let mut values = vec![];
        let mut boundaries = BTreeSet::new();
        let mut policies = BTreeMap::new();
        let mut input_artifacts = BTreeMap::new();
        let mut fallbacks = BTreeMap::new();
        let mut complete = true;
        for input in &definition.inputs {
            let Some(binding) =
                bindings.get(&InputBindingKey::new(key, input.alias.clone()).map_err(failure)?)
            else {
                complete = false;
                boundaries.insert(input.alias.clone());
                input_artifacts.insert(input.alias.clone(), Materialization::Unknown);
                continue;
            };
            let selected = binding.policy.candidates().iter().find_map(|branch| {
                snapshot
                    .heads
                    .get(&(binding.dataset.dataset_id(), branch.clone()))
                    .map(|h| (branch, h))
            });
            policies.insert(
                input.alias.clone(),
                format!(
                    "origin={:?}; candidates={:?}; permission={:?}",
                    binding.policy.origin(),
                    binding.policy.candidates(),
                    binding.policy.permission()
                ),
            );
            if let Some((branch, parent)) = selected {
                input_artifacts.insert(
                    input.alias.clone(),
                    observations
                        .get(&parent.artifact)
                        .copied()
                        .unwrap_or(Materialization::Unknown),
                );
                if branch != binding.policy.start() {
                    fallbacks.insert(
                        input.alias.clone(),
                        (binding.policy.start().to_string(), branch.to_string()),
                    );
                }
                if branch != &request.branch {
                    boundaries.insert(input.alias.clone());
                }
                values.push(json!({"consumer_workspace":key.workspace_id().to_string(),"consumer_dataset":key.dataset_id().to_string(),"alias":input.alias,"origin_workspace":binding.dataset.workspace_id().to_string(),"origin_dataset":binding.dataset.dataset_id().to_string(),"role":binding.role.name(),"declared":input.declaration["branch"],"starting_branch":binding.policy.start().as_str(),"resolved_branch":branch.as_str(),"version":parent.version.to_string(),"artifact":parent.artifact}));
            } else {
                complete = false;
                input_artifacts.insert(input.alias.clone(), Materialization::NeverBuilt);
            }
        }
        let current = if complete && let Some(semantics) = request.semantics.get(&key) {
            Some(
                compute::finalize(
                    graph,
                    context,
                    capture,
                    compute::Execution {
                        output: key,
                        parameters: &semantics.parameters,
                        inputs: &values,
                        writer: &semantics.writer,
                        evaluation_us: semantics.evaluation_us,
                        secret_versions: &semantics.secret_versions,
                    },
                )
                .map_err(failure)?
                .evidence()
                .clone(),
            )
        } else {
            None
        };
        let latest_attempt = attempt.map(attempt_state);
        facts.insert(
            definition.output.clone(),
            Facts {
                version: head.map(|h| h.version),
                materialization: head
                    .map(|h| {
                        observations
                            .get(&h.artifact)
                            .copied()
                            .unwrap_or(Materialization::Unknown)
                    })
                    .unwrap_or(Materialization::NeverBuilt),
                retained: head
                    .and_then(|h| h.evidence.as_ref())
                    .and_then(|v| serde_json::from_value(v.clone()).ok()),
                current,
                published_us: head.map(|h| h.published_us),
                latest_attempt,
                output_quality: output_quality(head),
                input_quality: input_quality(attempt),
                boundaries,
                policies,
                input_artifacts,
                fallbacks,
            },
        );
    }
    let mut datasets = freshness::evaluate(&scope, &facts, request.at_us).map_err(failure)?;
    for (id, status) in &mut datasets {
        if matches!(id, Id::Pending(_)) {
            status.reasons.push(freshness::Reason {
                code: "CATALOG_ADDITION_PENDING",
                path: vec![id.clone()],
                aliases: vec![],
                subject: "pending registration".into(),
                before: None,
                after: None,
                policy: None,
            });
        }
    }
    // Retained rows absent from this source remain inspectable but cannot become executable.
    for registered in context.registry.datasets().filter(|d| !d.is_tombstone()) {
        let key = registered.key();
        let id = Id::Registered(key);
        if datasets.contains_key(&id) {
            continue;
        }
        let tuple = (key.dataset_id(), request.branch.clone());
        let head = snapshot.heads.get(&tuple);
        let attempt = snapshot.attempts.get(&tuple);
        datasets.insert(
            id.clone(),
            Status {
                version: head.map(|h| h.version),
                published_us: head.map(|h| h.published_us),
                materialization: head
                    .map(|h| {
                        observations
                            .get(&h.artifact)
                            .copied()
                            .unwrap_or(Materialization::Unknown)
                    })
                    .unwrap_or(Materialization::NeverBuilt),
                direct_data: freshness::Freshness::Unknown,
                direct_logic: freshness::Freshness::Unknown,
                inherited: freshness::Freshness::Unknown,
                latest_attempt: attempt.map(attempt_state),
                output_quality: output_quality(head),
                input_quality: input_quality(attempt),
                reasons: vec![freshness::Reason {
                    code: "SOURCE_DEFINITION_UNAVAILABLE",
                    path: vec![id],
                    aliases: vec![],
                    subject: "no executable definition in selected source".into(),
                    before: head.map(|h| h.version.to_string()),
                    after: None,
                    policy: None,
                }],
            },
        );
    }

    Ok(Report {
        source: context.source.to_string(),
        branch: request.branch.clone(),
        event_sequence: snapshot.event_sequence,
        datasets,
    })
}
/// Capture/validate, snapshot once, lease exact versions, verify bytes, and evaluate on a blocking
/// worker holding runtime ownership. Invalid source never falls back to a previous executable graph.
pub async fn inspect(owner: RuntimeOwner, request: Request) -> Result<Completion<Report>, Error> {
    tokio::task::spawn_blocking(move || {
        let mut owner = owner;
        let result = (|| {
            owner.validate_paths().map_err(failure)?;
            if owner.registration().mode() == CoordinatorMode::MetadataOnly {
                return Err(failure("metadata-only owners cannot discover source"));
            }
            let workspace = Workspace::load(owner.workspace_root(), Some(owner.workspace_root()))
                .map_err(failure)?;
            if owner.workspace_id().map_err(failure)? != workspace.config().id() {
                return Err(failure("workspace identity changed during inspection"));
            }
            let inspection = crate::preparation::inspect(&workspace, request.python.as_ref(), true)
                .map_err(crate::build_plan::preparation_failure)?;
            let config = String::from_utf8(
                inspection
                    .capture
                    .read(Path::new("workspace.toml"), 16 * 1024 * 1024)
                    .map_err(failure)?,
            )
            .map_err(failure)?;
            let modules = serde_json::from_value(inspection.result["module_index"].clone())
                .map_err(failure)?;
            let context = ValidationRequest {
                registry: &inspection.registry,
                source: inspection.capture.id().map_err(failure)?,
                source_digest: inspection.capture.digest(),
                config_toml: &config,
                modules: &modules,
                discovery: &inspection.result,
                environment_fingerprint: &inspection.env.fingerprint,
                sdk_version: &inspection.env.runtime_version,
                check_semantics: validation::CHECK_SEMANTICS,
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(failure)?;
            runtime.block_on(async {
                inspect_captured(
                    &mut owner,
                    &inspection.graph,
                    &context,
                    &inspection.capture,
                    inspection.request,
                    &request,
                )
                .await
            })
        })();
        Completion { owner, result }
    })
    .await
    .map_err(failure)
}

/// Evaluate the same captured graph used by a draft, holding exact artifact read leases.
pub(crate) async fn inspect_captured(
    owner: &mut RuntimeOwner,
    graph: &ValidatedGraph,
    context: &ValidationRequest<'_>,
    capture: &SourceSnapshot,
    operation: RequestId,
    request: &Request,
) -> Result<Report, Error> {
    let workspace =
        Workspace::load(owner.workspace_root(), Some(owner.workspace_root())).map_err(failure)?;
    if owner.workspace_id().map_err(failure)? != workspace.config().id() {
        return Err(failure("workspace identity changed during inspection"));
    }
    let objects = ArtifactStore::open(workspace.root()).map_err(failure)?;
    let mut owned = owner.open_store().await.map_err(failure)?;
    let store = owned.repository().map_err(failure)?;
    let mut reader = store.reader().await.map_err(failure)?;
    let snapshot = reader
        .freshness_snapshot(workspace.config().id())
        .await
        .map_err(failure)?;
    reader.close().await.map_err(failure)?;
    let mut observations = BTreeMap::new();
    let bindings = tf_catalog::input_bindings::local_read_bindings(
        graph,
        context,
        &request.branch,
        request.fallbacks.as_deref(),
    )
    .map_err(failure)?;
    let mut selected: BTreeSet<_> = snapshot
        .heads
        .iter()
        .filter(|((_, branch), _)| branch == &request.branch)
        .map(|(_, h)| h.version)
        .collect();
    for binding in bindings.values() {
        if let Some(head) = binding.policy.candidates().iter().find_map(|branch| {
            snapshot
                .heads
                .get(&(binding.dataset.dataset_id(), branch.clone()))
        }) {
            selected.insert(head.version);
        }
    }
    for head in snapshot
        .heads
        .values()
        .filter(|h| selected.contains(&h.version))
    {
        if observations.contains_key(&head.artifact) {
            continue;
        }
        let lease_id: RequestId = discovery::random_id()
            .map_err(failure)?
            .parse()
            .map_err(failure)?;
        let lease = store
            .acquire_read(
                lease_id,
                operation,
                LeaseKind::Query,
                ReadTarget::Version(head.version),
                request.at_us,
                3_600_000_000,
            )
            .await
            .map_err(failure)?;
        let digest =
            ContentDigest::from_hex(DigestKind::Artifact, &head.artifact).map_err(failure)?;
        let observed = match objects.verify(digest) {
            Ok(_) => Materialization::Available,
            Err(ArtifactError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                Materialization::Missing
            }
            Err(
                ArtifactError::Integrity
                | ArtifactError::Metadata
                | ArtifactError::Normalization(_),
            ) => Materialization::Corrupt,
            Err(ArtifactError::Io(_)) => Materialization::Unknown,
        };
        store.release_read(&lease).await.map_err(failure)?;
        observations.insert(head.artifact.clone(), observed);
    }
    owned.close().await.map_err(failure)?;
    explain(graph, context, capture, &snapshot, &observations, request)
}

pub(crate) fn identity(id: &Id) -> String {
    match id {
        Id::Pending(p) => format!("pending:{p}"),
        Id::Registered(k) => format!("dataset:{}:{}", k.workspace_id(), k.dataset_id()),
    }
}

/// Stable structured freshness projection shared by public inspection adapters.
pub(crate) fn freshness_value(report: &crate::why::Report, registry: &RegistrySnapshot) -> Value {
    let names: BTreeMap<_, _> = registry
        .datasets()
        .map(|d| (Id::Registered(d.key()), d.path().to_string()))
        .collect();
    json!(report.datasets.iter().map(|(id,s)|json!({"identity":identity(id),"path":names.get(id).cloned().or_else(||match id {Id::Pending(p)=>Some(p.to_string()),_=>None}),"version":s.version.map(|v|v.to_string()),"materialization":format!("{:?}",s.materialization),"direct_data":format!("{:?}",s.direct_data),"direct_logic":format!("{:?}",s.direct_logic),"inherited":format!("{:?}",s.inherited),"latest_attempt":s.latest_attempt.map(|a|format!("{a:?}")),"output_quality":format!("{:?}",s.output_quality),"input_quality":s.input_quality.iter().map(|(a,q)|json!({"alias":a,"quality":format!("{q:?}")})).collect::<Vec<_>>(),"reasons":s.reasons.iter().map(|r|json!({"code":r.code,"message":r.human(&names),"path":r.path.iter().map(identity).collect::<Vec<_>>(),"aliases":r.aliases,"subject":r.subject,"before":r.before,"after":r.after,"policy":r.policy})).collect::<Vec<_>>()})).collect::<Vec<_>>())
}

/// Inspect one target even when missing boundaries prevent creating an executable draft.
/// Source capture and metadata are shared with the selection preview; no rediscovery occurs.
pub async fn inspect_selection(
    owner: RuntimeOwner,
    request: crate::build_plan::Request,
    mut options: crate::build_plan::Options,
) -> Result<Completion<SelectionReport>, Error> {
    tokio::task::spawn_blocking(move || {
        let mut owner = owner;
        let result = (|| {
            owner.validate_paths().map_err(failure)?;
            if owner.registration().mode() == CoordinatorMode::MetadataOnly {
                return Err(failure("metadata-only owners cannot discover source"));
            }
            let workspace = Workspace::load(owner.workspace_root(), Some(owner.workspace_root()))
                .map_err(failure)?;
            if owner.workspace_id().map_err(failure)? != workspace.config().id() {
                return Err(failure("workspace identity changed during inspection"));
            }
            let inspected = crate::preparation::inspect_selected(
                &workspace,
                request.python.as_ref(),
                true,
                None,
                options.git_ref.as_deref().map(|r| (r, &request.branch)),
            )
            .map_err(crate::build_plan::preparation_failure)?;
            let config = String::from_utf8(
                inspected
                    .capture
                    .read(Path::new("workspace.toml"), 16 * 1024 * 1024)
                    .map_err(failure)?,
            )
            .map_err(failure)?;
            let modules = serde_json::from_value(inspected.result["module_index"].clone())
                .map_err(failure)?;
            let context = ValidationRequest {
                registry: &inspected.registry,
                source: inspected.capture.id().map_err(failure)?,
                source_digest: inspected.capture.digest(),
                config_toml: &config,
                modules: &modules,
                discovery: &inspected.result,
                environment_fingerprint: &inspected.env.fingerprint,
                sdk_version: &inspected.env.runtime_version,
                check_semantics: validation::CHECK_SEMANTICS,
            };
            let graph = tf_catalog::traversal::Graph::validated(
                &inspected.graph,
                &context,
                request.branch.clone(),
            )
            .map_err(failure)?;
            let target = request
                .targets
                .first()
                .ok_or_else(|| failure("why requires a target"))?
                .clone();
            let selected = graph.resolve(&target).map_err(failure)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(failure)?;
            runtime.block_on(async {
                let report = inspect_captured(
                    &mut owner,
                    &inspected.graph,
                    &context,
                    &inspected.capture,
                    inspected.request,
                    &Request {
                        python: request.python.clone(),
                        branch: request.branch.clone(),
                        fallbacks: request.fallbacks.clone(),
                        semantics: BTreeMap::new(),
                        at_us: crate::preparation::now().map_err(failure)?,
                    },
                )
                .await?;
                let statuses = freshness_value(&report, &inspected.registry);
                let status = statuses
                    .as_array()
                    .and_then(|v| v.iter().find(|s| s["identity"] == identity(&selected)))
                    .cloned();
                let branch = request.branch.clone();
                let target = target.clone();
                options.explain = false;
                let preview =
                    crate::build_plan::prepare_captured(&mut owner, request, options, &inspected)
                        .await;
                let (plan, error) = match preview {
                    Ok(plan) => (Some(plan), None),
                    Err(e) => (None, Some(e.to_string())),
                };
                // Public formatting is performed by the adapter; this service returns the exact draft.
                Ok(SelectionReport {
                    workspace: context.registry.workspace().to_string(),
                    source: report.source,
                    source_digest: context.source_digest.to_owned(),
                    branch: branch.to_string(),
                    target,
                    status,
                    plan,
                    planning_error: error,
                })
            })
        })();
        Completion { owner, result }
    })
    .await
    .map_err(failure)
}
/// Why evidence and optional prospective draft from one captured source.
pub struct SelectionReport {
    /// Owning workspace.
    pub workspace: String,
    /// Captured source identity.
    pub source: String,
    /// Immutable source digest.
    pub source_digest: String,
    /// Selected data branch.
    pub branch: String,
    /// Requested exact reference.
    pub target: String,
    /// Selected-branch freshness, absent only for foreign provider metadata.
    pub status: Option<Value>,
    /// Guarded selection preview when all boundaries are available.
    pub plan: Option<tf_store::planning::DraftPlan>,
    /// Explicit planning refusal, without hiding the target's inspectable state.
    pub planning_error: Option<String>,
}
