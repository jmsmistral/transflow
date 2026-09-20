//! Finalize cache keys from retained accepted source and actual in-build parent results.
//! This internal service runs no transforms and does not inspect a later checkout.
use crate::build_plan::Completion;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tf_catalog::{
    RegistrySnapshot,
    capture::SourceSnapshot,
    compute,
    validation::{self, ValidationRequest},
};
use tf_domain::{BuildId, JobId, execution::ExecutionBinding};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
use tf_protocol::canonical::{ContentDigest, DigestKind};
use tf_store::{
    cache::Request,
    publication::{CheckRequirement, InputProvenance, InputRole, PublicationContract},
};
/// Caller-owned normalization and check obligations, supplied by the trusted check planner.
/// Canonical evaluator/worker integration remains in T059–T064.
pub struct Semantics {
    /// Result-affecting writer/normalization policy.
    pub writer: Value,
    /// Relative-time checks/computations freeze a clock rather than reusing across times.
    pub evaluation_us: Option<i64>,
    /// Non-secret dependency version identifiers.
    pub secret_versions: BTreeMap<String, String>,
    /// Complete normalized definitions already retained by the check planner.
    pub checks: Vec<CheckRequirement>,
}
/// Exact reason reuse is deferred or bypassed.
#[derive(Debug)]
pub enum Decision {
    /// A same-build parent has not succeeded or been cached yet; no key was frozen.
    Pending,
    /// The job must execute with this fully bound contract.
    Execute {
        /// Force, source refresh, or cache=never.
        reason: &'static str,
        /// Same contract used by publication; checks are still mandatory.
        request: Box<Request>,
    },
    /// Fully bound deterministic job can use tf-exec's owner-held reuse service.
    Ready(Box<Request>),
}
/// Safe preparation diagnostic.
#[derive(Debug, thiserror::Error)]
#[error("Cache preparation failed: {0}")]
pub struct Error(String);
fn fail(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value[key]
        .as_str()
        .ok_or_else(|| fail("incomplete accepted cache context"))
}
/// Resolve exact boundaries and successful/cached parents, then freeze conservative keys.
/// Ownership remains with the blocking task even if the awaiting future is dropped.
pub async fn prepare(
    owner: RuntimeOwner,
    build: BuildId,
    job: JobId,
    semantics: Semantics,
    at_us: i64,
) -> Result<Completion<Decision>, Error> {
    let completed = tokio::task::spawn_blocking(move || {
        let mut owner = owner;
        let result = (|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(fail)?;
            runtime.block_on(prepare_inner(&mut owner, build, job, semantics, at_us))
        })();
        (owner, result)
    })
    .await
    .map_err(fail)?;
    // The shared completion carrier uses the preparation error type.
    Ok(Completion {
        owner: completed.0,
        result: completed.1.map_err(crate::build_plan::failure),
    })
}
async fn prepare_inner(
    owner: &mut RuntimeOwner,
    build: BuildId,
    job: JobId,
    semantics: Semantics,
    at_us: i64,
) -> Result<Decision, Error> {
    owner.validate_paths().map_err(fail)?;
    if owner.registration().mode() == CoordinatorMode::MetadataOnly {
        return Err(fail("metadata-only owners cannot prepare reuse"));
    }
    let root = owner.workspace_root().to_owned();
    let session = owner.registration().session().map_err(fail)?;
    let workspace = owner.workspace_id().map_err(fail)?;
    let mut owned = owner.open_store().await.map_err(fail)?;
    let store = owned.repository().map_err(fail)?;
    let (plan, target, fence) = store.cache_job_context(build, job).await.map_err(fail)?;
    if fence.session != session || target.dataset.workspace_id() != workspace {
        return Err(fail("accepted job belongs to another coordinator"));
    }
    let capture = SourceSnapshot::open(
        &root.join(".transflow/runtime/source-snapshots"),
        plan.source.parse().map_err(fail)?,
    )
    .map_err(fail)?;
    if capture.digest() != plan.source_digest {
        return Err(fail("accepted source capture changed"));
    }
    let registry = RegistrySnapshot::parse(workspace, &plan.replacement).map_err(fail)?;
    let modules = serde_json::from_value(plan.discovery["module_index"].clone()).map_err(fail)?;
    let context = ValidationRequest {
        registry: &registry,
        source: capture.id().map_err(fail)?,
        source_digest: capture.digest(),
        config_toml: text(&plan.context, "configuration")?,
        modules: &modules,
        discovery: &plan.discovery,
        environment_fingerprint: text(&plan.environment, "fingerprint")?,
        sdk_version: text(&plan.environment, "runtime_version")?,
        check_semantics: validation::CHECK_SEMANTICS,
    };
    let graph = validation::validate(&context).map_err(fail)?;
    let definition = graph
        .candidate()
        .definitions()
        .iter()
        .find(|d| d.output == tf_catalog::candidate::CandidateIdentity::Registered(target.dataset))
        .ok_or_else(|| fail("accepted producer is absent"))?;
    // Check compilation/evaluation belongs to T059–T064. Until then, callers must
    // supply every declared obligation; an empty/partial list must never certify reuse.
    let mut expected = Vec::new();
    for check in definition.declaration["output"]["checks"]
        .as_array()
        .ok_or_else(|| fail("invalid output checks"))?
    {
        expected.push((None, check["on_error"] == "FAIL"));
    }
    for input in &definition.inputs {
        for check in input.declaration["checks"]
            .as_array()
            .ok_or_else(|| fail("invalid input checks"))?
        {
            expected.push((Some(input.alias.as_str()), check["on_error"] == "FAIL"));
        }
    }
    let mut supplied = semantics
        .checks
        .iter()
        .map(|c| {
            (
                match &c.subject {
                    tf_store::publication::CheckSubject::Output => None,
                    tf_store::publication::CheckSubject::Input(alias) => Some(alias.as_str()),
                },
                c.required,
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    supplied.sort();
    if supplied != expected {
        return Err(fail("complete matching check obligations are required"));
    }
    let write = plan
        .writes
        .iter()
        .find(|w| w.job == job.to_string() && w.dataset == target.dataset.dataset_id().to_string())
        .ok_or_else(|| fail("job is absent from accepted write set"))?;
    let bindings = write
        .bindings
        .as_array()
        .ok_or_else(|| fail("invalid accepted bindings"))?;
    let mut values = vec![];
    let mut inputs = vec![];
    for binding in bindings {
        let alias = text(binding, "alias")?;
        let declared = definition
            .inputs
            .iter()
            .find(|i| i.alias == alias)
            .ok_or_else(|| fail("accepted alias is absent"))?;
        let value = if binding["kind"] == "planned" {
            let dataset = text(binding, "dataset")?.parse().map_err(fail)?;
            let Some((version, artifact)) =
                store.completed_parent(build, dataset).await.map_err(fail)?
            else {
                owned.close().await.map_err(fail)?;
                return Ok(Decision::Pending);
            };
            json!({"consumer_workspace":workspace.to_string(),"consumer_dataset":target.dataset.dataset_id().to_string(),"alias":alias,"origin_workspace":workspace.to_string(),"origin_dataset":dataset.to_string(),"role":declared.declaration["role"],"declared":declared.declaration["branch"],"starting_branch":plan.output.name,"resolved_branch":plan.output.name,"version":version.to_string(),"artifact":artifact.hex()})
        } else if binding["kind"] == "boundary" {
            plan.reads
                .iter()
                .find(|r| r.consumer == write.dataset && r.alias == alias)
                .ok_or_else(|| fail("accepted boundary is absent"))?
                .semantic
                .clone()
        } else {
            return Err(fail("unsupported accepted binding"));
        };
        let resolution = if binding["kind"] == "planned" {
            json!({"kind":"in_build","build":build.to_string()})
        } else {
            plan.reads
                .iter()
                .find(|r| r.consumer == write.dataset && r.alias == alias)
                .ok_or_else(|| fail("missing boundary"))?
                .provenance["resolution"]
                .clone()
        };
        inputs.push(InputProvenance {
            alias: alias.into(),
            dataset: tf_domain::DatasetKey::new(
                text(&value, "origin_workspace")?.parse().map_err(fail)?,
                text(&value, "origin_dataset")?.parse().map_err(fail)?,
            ),
            version: text(&value, "version")?.parse().map_err(fail)?,
            artifact: ContentDigest::from_hex(DigestKind::Artifact, text(&value, "artifact")?)
                .map_err(fail)?,
            declared_branch: value["declared"].clone(),
            starting_branch: text(&value, "starting_branch")?.parse().map_err(fail)?,
            resolved_branch: text(&value, "resolved_branch")?.parse().map_err(fail)?,
            role: match text(&value, "role")? {
                "data" => InputRole::Data,
                "validation" => InputRole::Validation,
                _ => return Err(fail("invalid dependency role")),
            },
            resolution,
        });
        values.push(value);
    }
    let key = compute::finalize(
        &graph,
        &context,
        &capture,
        compute::Execution {
            output: target.dataset,
            parameters: &plan.context["parameters"][&write.dataset],
            inputs: &values,
            writer: &semantics.writer,
            evaluation_us: semantics.evaluation_us,
            secret_versions: &semantics.secret_versions,
        },
    )
    .map_err(fail)?;
    let contract = PublicationContract {
        compute_fingerprint: key.digest().hex(),
        check_fingerprint: key.check_fingerprint().hex(),
        inputs,
        checks: semantics.checks,
    };
    let request = Request {
        build,
        job,
        binding: ExecutionBinding {
            plan: plan.id.parse().map_err(fail)?,
            source: plan.source.parse().map_err(fail)?,
        },
        target,
        fence,
        contract,
        at_us,
    };
    store
        .freeze_computation_evidence(
            &request,
            &serde_json::to_value(key.evidence()).map_err(fail)?,
        )
        .await
        .map_err(fail)?;
    let reason = if plan.context["force"] == true {
        Some("forced")
    } else if plan.context["source_decisions"][&write.dataset]["executes"] == true {
        Some("source_refresh")
    } else if !key.reusable() {
        Some("cache_never")
    } else {
        None
    };
    owned.close().await.map_err(fail)?;
    Ok(match reason {
        Some(reason) => Decision::Execute {
            reason,
            request: Box::new(request),
        },
        None => Decision::Ready(Box::new(request)),
    })
}
