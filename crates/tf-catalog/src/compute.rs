//! Conservative versioned compute keys from validated captured code and fully bound aliases.
use crate::{
    candidate::CandidateIdentity,
    capture::SourceSnapshot,
    validation::{ValidatedGraph, ValidationRequest},
    workspace::WorkspaceConfig,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::DatasetKey;
use tf_protocol::canonical::{ContentDigest, DigestKind, content_digest};
/// Missing bindings defer reuse; no speculative downstream key is emitted.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Validation or source/environment identity no longer matches.
    #[error("Compute context does not match the validated captured graph")]
    Context,
    /// Exact upstream publication identities are not all known yet.
    #[error("Upstream will run; reuse decision follows its result")]
    Pending,
    /// Invalid normalized parameter, input, writer or clock carrier.
    #[error("Compute semantics are incomplete or unsupported")]
    Semantics,
}
/// Exact semantic execution inputs; resource limits/presentation are intentionally absent.
pub struct Execution<'a> {
    /// Stable registered/proposed output identity.
    pub output: DatasetKey,
    /// Fully normalized typed parameter object.
    pub parameters: &'a Value,
    /// Per-alias exact input carriers from resolution, including validation-only aliases.
    pub inputs: &'a [Value],
    /// Result-affecting writer and normalization policy.
    pub writer: &'a Value,
    /// Explicit evaluation clock only when the computation/check observes relative time.
    pub evaluation_us: Option<i64>,
    /// Non-secret immutable version identifiers, never secret values.
    pub secret_versions: &'a BTreeMap<String, String>,
}
/// Computation identity is distinct from source snapshot and physical artifact identity.
#[derive(Clone, Debug)]
pub struct ComputeKey {
    digest: ContentDigest,
    checks: ContentDigest,
    reusable: bool,
}
impl ComputeKey {
    /// Version-one purpose-separated identity.
    pub fn digest(&self) -> ContentDigest {
        self.digest
    }
    /// Independent normalized check/evaluator policy identity.
    pub fn check_fingerprint(&self) -> ContentDigest {
        self.checks
    }
    /// Only declarations opting into deterministic reuse permit cache lookup.
    /// This is not certification of undeclared external I/O or future bit-identical reruns.
    pub fn reusable(&self) -> bool {
        self.reusable
    }
}
fn semantic_checks(value: &mut Value) {
    if let Some(checks) = value.as_array_mut() {
        for check in checks {
            if let Some(object) = check.as_object_mut() {
                for key in ["name", "description", "sample_rows"] {
                    object.remove(key);
                }
            }
        }
    }
}
/// Normalize typed overrides against declared defaults, rejecting unknown names and mismatched types.
pub fn parameters(declaration: &Value, overrides: &Value) -> Result<Value, Error> {
    let overrides = overrides.as_object().ok_or(Error::Semantics)?;
    let declarations = declaration["parameters"]
        .as_array()
        .ok_or(Error::Semantics)?;
    let names: BTreeSet<_> = declarations
        .iter()
        .map(|p| p["name"].as_str().ok_or(Error::Semantics))
        .collect::<Result<_, _>>()?;
    if overrides.keys().any(|k| !names.contains(k.as_str())) {
        return Err(Error::Semantics);
    }
    let mut normalized = serde_json::Map::new();
    for p in declarations {
        let name = p["name"].as_str().ok_or(Error::Semantics)?;
        let value = overrides.get(name).unwrap_or(&p["default"]);
        tf_protocol::validate_document("ScalarValue", value).map_err(|_| Error::Semantics)?;
        if !crate::validation::parameter(&p["logical_type"], value) {
            return Err(Error::Semantics);
        }
        normalized.insert(name.into(), value.clone());
    }
    Ok(Value::Object(normalized))
}
/// Finalize only after every input has a real version/artifact. Snapshot UUIDs, catalogue
/// labels/layout, check descriptions and operational time/memory/admission limits are excluded.
pub fn finalize(
    graph: &ValidatedGraph,
    context: &ValidationRequest<'_>,
    capture: &SourceSnapshot,
    execution: Execution<'_>,
) -> Result<ComputeKey, Error> {
    if !graph.certificate().matches(context)
        || capture.id().map_err(|_| Error::Context)? != context.source
        || capture.digest() != context.source_digest
    {
        return Err(Error::Context);
    }
    let d = graph
        .candidate()
        .definitions()
        .iter()
        .find(|d| d.output == CandidateIdentity::Registered(execution.output))
        .ok_or(Error::Context)?;
    let normalized_parameters = parameters(&d.declaration, execution.parameters)?;
    let expected: BTreeSet<_> = d.inputs.iter().map(|i| i.alias.as_str()).collect();
    let mut inputs = BTreeMap::new();
    for input in execution.inputs {
        let alias = input["alias"].as_str().ok_or(Error::Semantics)?;
        let definition = d
            .inputs
            .iter()
            .find(|i| i.alias == alias)
            .ok_or(Error::Semantics)?;
        let CandidateIdentity::Registered(key) = definition.identity else {
            return Err(Error::Context);
        };
        if input["consumer_workspace"] != execution.output.workspace_id().to_string()
            || input["consumer_dataset"] != execution.output.dataset_id().to_string()
            || input["origin_workspace"] != key.workspace_id().to_string()
            || input["origin_dataset"] != key.dataset_id().to_string()
            || input["role"] != definition.declaration["role"]
            || input["declared"] != definition.declaration["branch"]
        {
            return Err(Error::Semantics);
        }
        input["version"]
            .as_str()
            .ok_or(Error::Pending)?
            .parse::<tf_domain::VersionId>()
            .map_err(|_| Error::Semantics)?;
        ContentDigest::from_hex(
            DigestKind::Artifact,
            input["artifact"].as_str().ok_or(Error::Pending)?,
        )
        .map_err(|_| Error::Semantics)?;
        for name in ["starting_branch", "resolved_branch"] {
            input[name]
                .as_str()
                .ok_or(Error::Semantics)?
                .parse::<tf_domain::BranchName>()
                .map_err(|_| Error::Semantics)?;
        }
        if inputs.insert(alias, input.clone()).is_some() {
            return Err(Error::Semantics);
        }
    }
    if inputs.keys().copied().collect::<BTreeSet<_>>() != expected {
        return Err(Error::Pending);
    }
    if !execution.parameters.is_object()
        || !execution.writer.is_object()
        || execution.evaluation_us.is_some_and(|v| v < 0)
    {
        return Err(Error::Semantics);
    }
    let declared_secrets: BTreeSet<_> = d.declaration["secret_refs"]
        .as_array()
        .ok_or(Error::Semantics)?
        .iter()
        .map(|v| v.as_str().ok_or(Error::Semantics))
        .collect::<Result<_, _>>()?;
    if execution
        .secret_versions
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != declared_secrets
        || execution
            .secret_versions
            .values()
            .any(|v| v.is_empty() || v.len() > 1024)
    {
        return Err(Error::Semantics);
    }
    let mut declaration = d.declaration.clone();
    let object = declaration.as_object_mut().ok_or(Error::Semantics)?;
    for key in ["line", "lineage_json", "wall_timeout_seconds"] {
        object.remove(key);
    }
    declaration["output"]["ref"] = json!({"workspace":execution.output.workspace_id().to_string(),"dataset":execution.output.dataset_id().to_string()});
    semantic_checks(&mut declaration["output"]["checks"]);
    let mut checks = json!({"output":declaration["output"]["checks"],"inputs":{},"semantics":context.check_semantics,"environment":context.environment_fingerprint});
    for (input, definition) in declaration["inputs"]
        .as_array_mut()
        .ok_or(Error::Semantics)?
        .iter_mut()
        .zip(&d.inputs)
    {
        let CandidateIdentity::Registered(key) = definition.identity else {
            return Err(Error::Context);
        };
        input["ref"] = json!({"workspace":key.workspace_id().to_string(),"dataset":key.dataset_id().to_string()});
        semantic_checks(&mut input["checks"]);
        checks["inputs"][&definition.alias] = input["checks"].clone();
    }
    let config = WorkspaceConfig::parse(context.config_toml).map_err(|_| Error::Context)?;
    let semantic = json!({"key_version":1,"output":{"workspace":execution.output.workspace_id().to_string(),"dataset":execution.output.dataset_id().to_string()},"source_files":capture.computation_files(),"source_roots":capture.source_roots(),"configuration":{"python_minor":config.python_version(),"validation_null_policy":config.null_policy()},"environment":context.environment_fingerprint,"sdk_worker":context.sdk_version,"check_semantics":context.check_semantics,"declaration":declaration,"parameters":normalized_parameters,"inputs":inputs,"writer":execution.writer,"evaluation_us":execution.evaluation_us.map(|v|v.to_string()),"secret_versions":execution.secret_versions});
    Ok(ComputeKey {
        digest: content_digest(DigestKind::Compute, &semantic).map_err(|_| Error::Semantics)?,
        checks: content_digest(
            DigestKind::Compute,
            &json!({"check_key_version":1,"checks":checks}),
        )
        .map_err(|_| Error::Semantics)?,
        reusable: d.declaration["cache"] == "deterministic",
    })
}
