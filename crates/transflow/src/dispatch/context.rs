use super::*;
use tf_catalog::{
    RegistrySnapshot,
    capture::SourceSnapshot,
    validation::{self, ValidationRequest},
    workspace::WorkspaceConfig,
};
use tf_domain::resources::{Origin, Setting};
use tf_exec::environment::{self, EnvironmentAction, EnvironmentRequest};
/// Verified accepted source and complete executable graph, independent of the checkout.
pub(super) struct Prepared {
    pub plan: tf_store::planning::DraftPlan,
    pub capture: SourceSnapshot,
    pub catalog: Value,
    pub interpreter: PathBuf,
    pub declarations: BTreeMap<JobId, (Value, Value)>,
    pub parents: BTreeMap<JobId, Vec<JobId>>,
    pub jobs: BTreeMap<JobId, Job>,
    pub capacity: tf_exec::admission::Capacity,
    pub contextual: bool,
    pub abort_on_failure: bool,
}
pub(super) fn prepare(
    owner: &mut RuntimeOwner,
    rt: &tokio::runtime::Runtime,
    build: BuildId,
) -> Result<Prepared> {
    owner.validate_paths().map_err(fail)?;
    if owner.registration().mode() == tf_exec::ownership::CoordinatorMode::MetadataOnly {
        return Err(fail("metadata-only access cannot dispatch"));
    }
    let mut owned = rt.block_on(owner.open_store()).map_err(fail)?;
    let plan = rt
        .block_on(owned.repository().map_err(fail)?.dispatch_plan(build))
        .map_err(fail)?;
    rt.block_on(owned.close()).map_err(fail)?;
    let workspace = owner.workspace_id().map_err(fail)?;
    let session = owner.registration().session().map_err(fail)?;
    let mut owned = rt.block_on(owner.open_store()).map_err(fail)?;
    rt.block_on(owned.repository().map_err(fail)?.renew_dispatch_boundaries(
        workspace,
        session,
        &plan,
        now()?,
    ))
    .map_err(fail)?;
    rt.block_on(owned.close()).map_err(fail)?;
    let root = owner.workspace_root().to_owned();
    let capture = SourceSnapshot::open(
        &root.join(".transflow/runtime/source-snapshots"),
        plan.source.parse().map_err(fail)?,
    )
    .map_err(fail)?;
    if capture.digest() != plan.source_digest
        || plan.workspace != owner.workspace_id().map_err(fail)?.to_string()
    {
        return Err(fail("accepted source identity changed"));
    }
    let config = WorkspaceConfig::parse(text(&plan.context, "configuration")?).map_err(fail)?;
    let env = environment::inspect(
        &root,
        Path::new(text(&plan.environment, "base_python")?),
        &EnvironmentRequest {
            action: EnvironmentAction::Check,
            requirements: config.dependency_paths().0.into(),
            lock: config.dependency_paths().1.into(),
            minor: config.python_version().into(),
            runtime_wheel: None,
            runtime_version: text(&plan.environment, "runtime_version")?.into(),
            wheelhouse: None,
            offline: true,
        },
    )
    .map_err(fail)?;
    if env.fingerprint != plan.environment["fingerprint"]
        || env.interpreter.to_str() != plan.environment["interpreter"].as_str()
    {
        return Err(fail("accepted environment no longer matches"));
    }
    let registry = RegistrySnapshot::parse(config.id(), &plan.replacement).map_err(fail)?;
    let modules = serde_json::from_value(plan.discovery["module_index"].clone()).map_err(fail)?;
    let graph = validation::validate(&ValidationRequest {
        registry: &registry,
        source: capture.id().map_err(fail)?,
        source_digest: capture.digest(),
        config_toml: text(&plan.context, "configuration")?,
        modules: &modules,
        discovery: &plan.discovery,
        environment_fingerprint: &env.fingerprint,
        sdk_version: &env.runtime_version,
        check_semantics: validation::CHECK_SEMANTICS,
    })
    .map_err(fail)?;
    // The discovery ABI does not yet expose context use. Conservatively key the frozen
    // clock whenever any captured Python file can reference the reserved context name.
    let contextual = capture
        .discovery_files()
        .as_array()
        .ok_or_else(|| fail("missing captured files"))?
        .iter()
        .try_fold(false, |found, f| -> Result<bool> {
            let bytes = std::fs::read(capture.files_root().join(text(f, "path")?)).map_err(fail)?;
            Ok(found || bytes.windows(3).any(|w| w == b"ctx"))
        })?;
    let mut declarations = BTreeMap::new();
    let mut jobs = BTreeMap::new();
    let mut parents = BTreeMap::new();
    let by_dataset = plan
        .writes
        .iter()
        .map(|w| Ok((w.dataset.clone(), w.job.parse::<JobId>().map_err(fail)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut capacity = None;
    for w in &plan.writes {
        let job = w.job.parse().map_err(fail)?;
        let key = tf_domain::DatasetKey::new(config.id(), w.dataset.parse().map_err(fail)?);
        let d = graph
            .candidate()
            .definitions()
            .iter()
            .find(|d| d.output == tf_catalog::candidate::CandidateIdentity::Registered(key))
            .ok_or_else(|| fail("accepted producer is absent"))?;
        let raw = plan.discovery["definitions"]
            .as_array()
            .ok_or_else(|| fail("missing definitions"))?
            .iter()
            .find(|r| {
                r["module"] == d.declaration["module"] && r["function"] == d.declaration["function"]
            })
            .ok_or_else(|| fail("missing executable declaration"))?
            .clone();
        if raw["secret_refs"].as_array().is_none_or(|v| !v.is_empty()) {
            return Err(fail(
                "declared secret delivery is not available in this dispatcher",
            ));
        }
        if raw["engine"] != "polars" {
            return Err(fail("this dispatcher requires a Polars producer"));
        }
        let resolved = &plan.context["resources"][&w.dataset];
        let cap = tf_exec::admission::Capacity {
            jobs: number(resolved, "max_jobs")?.try_into().map_err(fail)?,
            cpu: number(resolved, "cpu_tokens")?.try_into().map_err(fail)?,
            memory_bytes: resolved["memory_budget_mib"]
                .as_str()
                .map(|v| {
                    v.parse::<u64>()
                        .ok()
                        .and_then(|n| n.checked_mul(1024 * 1024))
                        .ok_or_else(|| fail("invalid memory capacity"))
                })
                .transpose()?,
            ..Default::default()
        };
        if capacity.is_some_and(|old| old != cap) {
            return Err(fail("accepted coordinator capacities disagree"));
        }
        capacity = Some(cap);
        limits(resolved)?;
        let mut dependencies = vec![];
        let bindings = w
            .bindings
            .as_array()
            .ok_or_else(|| fail("invalid accepted bindings"))?;
        if bindings.len() != d.inputs.len() {
            return Err(fail("incomplete accepted bindings"));
        }
        let mut aliases = std::collections::BTreeSet::new();
        for binding in bindings {
            let alias = text(binding, "alias")?;
            let input = d
                .inputs
                .iter()
                .find(|i| i.alias == alias)
                .ok_or_else(|| fail("unknown accepted input alias"))?;
            if !aliases.insert(alias) {
                return Err(fail("duplicate input alias"));
            }
            if binding["kind"] == "planned" {
                let dataset = text(binding, "dataset")?;
                if input.identity
                    != tf_catalog::candidate::CandidateIdentity::Registered(
                        tf_domain::DatasetKey::new(config.id(), dataset.parse().map_err(fail)?),
                    )
                {
                    return Err(fail("wrong planned input identity"));
                }
                dependencies.push(
                    *by_dataset
                        .get(dataset)
                        .ok_or_else(|| fail("required parent is outside accepted scope"))?,
                );
            } else if binding["kind"] == "boundary" {
                let r = plan
                    .reads
                    .iter()
                    .find(|r| r.consumer == w.dataset && r.alias == alias)
                    .ok_or_else(|| fail("required boundary is unresolved"))?;
                if input.identity
                    != tf_catalog::candidate::CandidateIdentity::Registered(
                        tf_domain::DatasetKey::new(
                            text(&r.provenance, "workspace")?.parse().map_err(fail)?,
                            r.dataset.parse().map_err(fail)?,
                        ),
                    )
                {
                    return Err(fail("wrong boundary input identity"));
                }
                let digest = tf_protocol::canonical::ContentDigest::from_hex(
                    DigestKind::Artifact,
                    &r.artifact,
                )
                .map_err(fail)?;
                tf_store::artifacts::ArtifactStore::open(&root)
                    .map_err(fail)?
                    .verify(digest)
                    .map_err(fail)?;
            } else {
                return Err(fail("unknown input binding kind"));
            }
        }
        let mut owned = rt.block_on(owner.open_store()).map_err(fail)?;
        let (_, target, fence) = rt
            .block_on(
                owned
                    .repository()
                    .map_err(fail)?
                    .cache_job_context(build, job),
            )
            .map_err(fail)?;
        rt.block_on(owned.close()).map_err(fail)?;
        if fence.session != owner.registration().session().map_err(fail)? {
            return Err(fail("accepted owner changed"));
        }
        jobs.insert(
            job,
            Job::new(
                job,
                build,
                ExecutionBinding {
                    plan: plan.id.parse().map_err(fail)?,
                    source: plan.source.parse().map_err(fail)?,
                },
                target,
                fence,
                config.failure_policy().0,
                EventTime(0),
            ),
        );
        declarations.insert(job, (raw, d.declaration.clone()));
        parents.insert(job, dependencies);
    }
    let catalog = registry
        .sdk_projection(capture.id().map_err(fail)?)
        .map_err(fail)?;
    Ok(Prepared {
        plan,
        capture,
        catalog,
        interpreter: env.interpreter,
        declarations,
        parents,
        jobs,
        contextual,
        abort_on_failure: config.failure_policy().1,
        capacity: capacity.ok_or_else(|| fail("empty write set"))?,
    })
}
pub(super) fn number(v: &Value, k: &str) -> Result<u64> {
    text(v, k)?.parse().map_err(fail)
}
pub(super) fn limits(v: &Value) -> Result<tf_exec::timing::Limits> {
    let setting = |key: &str, origin: &str| -> Result<Setting> {
        Ok(Setting {
            value: number(v, key)?,
            origin: match text(v, origin)? {
                "Default" => Origin::Default,
                "Workspace" => Origin::Workspace,
                "Definition" => Origin::Definition,
                "Build" => Origin::Build,
                "Explicit" => Origin::Explicit,
                _ => return Err(fail("invalid resource origin")),
            },
        })
    };
    Ok(tf_exec::timing::Limits {
        transform: setting("timeout_seconds", "timeout_origin")?,
        validation: setting("validation_timeout_seconds", "validation_timeout_origin")?,
        discovery: setting("discovery_timeout_seconds", "discovery_timeout_origin")?,
        interactive: setting("interactive_timeout_seconds", "interactive_timeout_origin")?,
    })
}
