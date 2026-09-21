//! Whole-capture structural validation. No selection, force, registry write or data execution.
use crate::{
    RegistrySnapshot,
    candidate::{CandidateCatalog, CandidateError, CandidateIdentity, Location},
    workspace::{WorkspaceConfig, relative_path},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{
    SourceSnapshotId,
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticError, Redactor, SourceRange},
};
use tf_protocol::{
    canonical::{ContentDigest, DigestKind, content_digest, file_digest},
    validate_document,
};

/// Current installed SDK declaration contract. A changed SDK invalidates retained evidence.
pub const SDK_VERSION: &str = "0.0.0.dev0";
/// Current typed AST semantics; changing these invalidates retained validation certificates.
pub const CHECK_SEMANTICS: &str = "expectation-ast-v1:typed-composition:strict-null-v1";
/// Inputs obtained from one captured source and authenticated, matched worker discovery.
/// `modules` is the complete pre-import ModuleIndex (including helpers/namespaces), not a target subset.
/// The caller verifies source bytes and environment drift before invoking this pure service.
pub struct ValidationRequest<'a> {
    /// Immutable durable registry used for discovery.
    pub registry: &'a RegistrySnapshot,
    /// Captured source identity, never the live working tree.
    pub source: SourceSnapshotId,
    /// Verified whole-source content digest.
    pub source_digest: &'a str,
    /// Exact captured configuration bytes; defaults are resolved here.
    pub config_toml: &'a str,
    /// Complete validated Python module index, name to workspace-relative path.
    pub modules: &'a BTreeMap<String, String>,
    /// Complete successful discovery response. Import errors cannot supply this result.
    pub discovery: &'a Value,
    /// Verified installed environment identity from the environment service.
    pub environment_fingerprint: &'a str,
    /// Matched worker SDK version, checked against supported semantics.
    pub sdk_version: &'a str,
    /// Exact current expectation declaration semantics.
    pub check_semantics: &'a str,
}
/// Actionable failure retains the entire cycle/location list separately from bounded rendering.
#[derive(Clone, Debug, thiserror::Error)]
#[error("Workspace validation failed: {message}")]
pub struct ValidationError {
    /// Safe fixed explanation, never a raw Python exception or source dump.
    pub message: &'static str,
    /// All captured source anchors associated with the failure.
    pub locations: Vec<Location>,
    /// Complete closed path when the graph is cyclic.
    pub cycle: Vec<String>,
}
impl ValidationError {
    /// Human-first bounded T016 diagnostic. Full structured cycle data remains on this error.
    pub fn diagnostic(&self, redactor: &Redactor) -> Result<Diagnostic, DiagnosticError> {
        let code = if self.cycle.is_empty() {
            DiagnosticCode::OperationFailed
        } else {
            DiagnosticCode::GraphCycle
        };
        let sources = self
            .locations
            .iter()
            .take(32)
            .map(|l| {
                let line = u32::try_from(l.line).map_err(|_| DiagnosticError)?;
                SourceRange::new(redactor.text(&l.path)?, (line, 1), (line, 2))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let affected = self
            .cycle
            .iter()
            .take(32)
            .map(|s| redactor.text(s))
            .collect::<Result<Vec<_>, _>>()?;
        let remediation = if self.cycle.len() > 32 || self.locations.len() > 32 {
            "Correct the declarations and validate the complete source again. Display is limited to 32 references; the structured cycle is complete. No producer functions were run."
        } else {
            "Correct the declarations and validate the complete source again. No producer functions were run."
        };
        Diagnostic::new(
            code,
            redactor.text("Workspace validation failed")?,
            redactor.text(self.message)?,
            redactor.text(remediation)?,
        )
        .with_sources(sources)?
        .with_affected(affected)
    }
}
fn fail(message: &'static str, locations: Vec<Location>) -> ValidationError {
    ValidationError {
        message,
        locations,
        cycle: Vec::new(),
    }
}
fn location(d: &Value) -> Location {
    Location {
        path: d["path"].as_str().unwrap_or("unknown.py").to_owned(),
        line: d["line"].as_u64().unwrap_or(1),
    }
}
fn array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, ValidationError> {
    v[key]
        .as_array()
        .ok_or_else(|| fail("Declaration metadata is malformed", vec![]))
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, ValidationError> {
    v[key]
        .as_str()
        .ok_or_else(|| fail("Declaration metadata is malformed", vec![]))
}
fn digest(v: &Value) -> Result<ContentDigest, ValidationError> {
    content_digest(DigestKind::Compute, v)
        .map_err(|_| fail("Validation metadata exceeds its canonical limits", vec![]))
}
fn raw(bytes: &[u8]) -> Result<String, ValidationError> {
    file_digest(&mut &*bytes)
        .map(|d| d.hex())
        .map_err(|_| fail("Captured bytes could not be fingerprinted", vec![]))
}
fn unique(values: &[Value], key: Option<&str>) -> bool {
    let names: Option<BTreeSet<_>> = values
        .iter()
        .map(|v| key.map_or(v, |k| &v[k]).as_str())
        .collect();
    names.is_some_and(|names| names.len() == values.len())
}
fn alias(name: &str) -> bool {
    !name.starts_with('_')
        && !name.is_empty()
        && ![
            "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
            "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
            "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
            "return", "try", "while", "with", "yield",
        ]
        .contains(&name)
        && name.chars().enumerate().all(|(i, c)| {
            if i == 0 {
                unicode_ident::is_xid_start(c)
            } else {
                unicode_ident::is_xid_continue(c)
            }
        })
}
fn context(
    request: &ValidationRequest<'_>,
) -> Result<(WorkspaceConfig, ContentDigest, Value), ValidationError> {
    let config = WorkspaceConfig::parse(request.config_toml).map_err(|_| {
        fail(
            "Captured workspace configuration is invalid",
            vec![Location {
                path: "workspace.toml".into(),
                line: 1,
            }],
        )
    })?;
    if config.id() != request.registry.workspace() {
        return Err(fail(
            "Configuration and catalogue belong to different workspaces",
            vec![],
        ));
    }
    for value in [request.source_digest, request.environment_fingerprint] {
        ContentDigest::from_hex(DigestKind::File, value).map_err(|_| {
            fail(
                "Captured source or environment identity is malformed",
                vec![],
            )
        })?;
    }
    if request.sdk_version != SDK_VERSION || request.check_semantics != CHECK_SEMANTICS {
        return Err(fail(
            "SDK or check semantics are unsupported; use the matched environment",
            vec![],
        ));
    }
    validate_document("DiscoveryResultV1", request.discovery).map_err(|_| {
        // Keep a source anchor when a particular declaration (such as its engine) is invalid.
        let locations = request.discovery["definitions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| validate_document("DeclarationV1", d).is_err())
            .take(32)
            .map(location)
            .collect();
        fail(
            "Discovery failed or returned unsupported declaration metadata",
            locations,
        )
    })?;
    if request.discovery["environment_fingerprint"] != request.environment_fingerprint {
        return Err(fail(
            "Discovery belongs to a different installed environment",
            vec![],
        ));
    }
    if request.modules.len() > 100_000 {
        return Err(fail(
            "Captured module index exceeds its supported limit",
            vec![],
        ));
    }
    for (module, path) in request.modules {
        if module.len() > 4096
            || module.is_empty()
            || relative_path(path).is_err()
            || !config
                .source_roots()
                .iter()
                .any(|root| std::path::Path::new(path).starts_with(root))
        {
            return Err(fail(
                "Captured module index is malformed or outside the source roots",
                vec![],
            ));
        }
    }
    let imported = array(request.discovery, "imported_modules")?;
    let names: Option<BTreeSet<_>> = imported.iter().map(Value::as_str).collect();
    if names.as_ref().is_none_or(|n| {
        n.len() != imported.len() || *n != request.modules.keys().map(String::as_str).collect()
    }) {
        return Err(fail(
            "Discovery did not successfully import the complete captured module index",
            vec![],
        ));
    }
    for d in array(request.discovery, "definitions")? {
        if request.modules.get(text(d, "module")?).map(String::as_str) != Some(text(d, "path")?) {
            return Err(fail(
                "A producer location is absent from its captured module index",
                vec![location(d)],
            ));
        }
    }
    let projection = request
        .registry
        .sdk_projection(request.source)
        .map_err(|_| fail("Catalogue projection is invalid", vec![]))?;
    let evidence = json!({
        "validation_format":1,
        "workspace_id":config.id().to_string(),
        "source_snapshot_id":request.source.to_string(),
        "source_digest":request.source_digest,
        "registry_bytes":request.registry.raw_digest().hex(),
        "catalog_fingerprint":projection["catalog_fingerprint"],
        "configuration":raw(request.config_toml.as_bytes())?,
        "module_index":digest(&json!(request.modules))?.hex(),
        "discovery":digest(request.discovery)?.hex(),
        "environment":request.environment_fingerprint,
        "sdk":request.sdk_version,
        "checks":request.check_semantics
    });
    let key = digest(&evidence)?;
    Ok((config, key, evidence))
}
/// Work still required on exact selected schemas or materialized data. This is never a PASS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeferredReason {
    /// No authoritative schema exists yet (including foreign or off-branch bindings).
    SchemaUnavailable,
    /// A declared schema must be checked against exact materialized bytes.
    VerifyDeclaredSchema,
    /// Metadata is valid, but evaluating a check requires exact data.
    EvaluateData,
}
/// Explicit deferred runtime obligation, retaining the producer and input alias/check identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredValidation {
    /// Consumer/output canonical dataset path.
    pub dataset: String,
    /// Input alias; None identifies the output.
    pub input_alias: Option<String>,
    /// Check identity; None identifies schema verification.
    pub check_id: Option<String>,
    /// Why runtime evidence is still required.
    pub reason: DeferredReason,
}
/// Immutable successful structural evidence, not a data quality or publication certificate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationCertificate {
    evidence: Value,
    context: ContentDigest,
    candidate: ContentDigest,
    fingerprint: ContentDigest,
}
impl ValidationCertificate {
    /// Complete domain-separated certificate identity.
    pub fn fingerprint(&self) -> &ContentDigest {
        &self.fingerprint
    }
    /// Bind exact normalized candidate metadata and effective inherited check policies.
    pub fn candidate_fingerprint(&self) -> &ContentDigest {
        &self.candidate
    }
    /// Detached internal evidence descriptor for inspection; persistence/API codecs are later work.
    pub fn evidence(&self) -> Value {
        self.evidence.clone()
    }
    /// Exact-context comparison; invalid input cannot reuse a valid certificate.
    pub fn matches(&self, request: &ValidationRequest<'_>) -> bool {
        context(request).is_ok_and(|(_, key, _)| key == self.context)
    }
}
/// Only complete successful validation can construct this value.
#[derive(Clone, Debug)]
pub struct ValidatedGraph {
    candidate: CandidateCatalog,
    certificate: ValidationCertificate,
    deferred: Vec<DeferredValidation>,
}
impl ValidatedGraph {
    /// Complete graph with normalized effective check metadata.
    pub fn candidate(&self) -> &CandidateCatalog {
        &self.candidate
    }
    /// Structural certificate bound to this graph's capture and semantics.
    pub fn certificate(&self) -> &ValidationCertificate {
        &self.certificate
    }
    /// Required runtime schema/data work; never treated as passing checks.
    pub fn deferred(&self) -> &[DeferredValidation] {
        &self.deferred
    }
}
pub(crate) fn parameter(logical: &Value, value: &Value) -> bool {
    if logical["type"] != value["type"] || logical["type"] == "binary" {
        return false;
    }
    if ["f32", "f64"].iter().any(|k| logical["type"] == *k)
        && ["NaN", "Infinity", "-Infinity"]
            .iter()
            .any(|v| value["value"] == *v)
    {
        return false;
    }
    for key in ["precision", "scale", "unit", "timezone"] {
        if logical.get(key).is_some_and(|v| Some(v) != value.get(key)) {
            return false;
        }
    }
    let field = |f: &Value, v: &Value| {
        v["type"] == "null" && f["nullable"] == true || parameter(&f["logical_type"], v)
    };
    if logical["type"] == "list" {
        return value["values"]
            .as_array()
            .is_some_and(|a| a.iter().all(|v| field(&logical["element"], v)));
    }
    if logical["type"] == "struct" {
        let (Some(fields), Some(values)) =
            (logical["fields"].as_array(), value["fields"].as_array())
        else {
            return false;
        };
        return fields.len() == values.len()
            && fields
                .iter()
                .zip(values)
                .all(|(f, v)| f["name"] == v["name"] && field(f, &v["value"]));
    }
    true
}
fn declaration(d: &mut Value, config: &WorkspaceConfig) -> Result<(), ValidationError> {
    let loc = location(d);
    let error = |message| fail(message, vec![loc.clone()]);
    if d["source"] == true {
        if !array(d, "inputs")?.is_empty()
            || !d["cache"].is_null()
            || d["refresh"].is_null()
            || !d["lineage_json"].is_null()
        {
            return Err(error(
                "Source declarations require zero inputs and explicit refresh semantics",
            ));
        }
    } else if d["cache"].is_null()
        || !d["refresh"].is_null()
        || !array(d, "secret_refs")?.is_empty()
    {
        return Err(error(
            "Transform declarations have incompatible cache or source-only settings",
        ));
    }
    if !unique(array(d, "secret_refs")?, None) {
        return Err(error("Secret reference names must be unique"));
    }
    if let Some(lineage) = d["lineage_json"].as_str()
        && (lineage.len() > 1024 * 1024 || serde_json::from_str::<Value>(lineage).is_err())
    {
        return Err(error("Lineage must contain bounded valid JSON metadata"));
    }
    let inputs = array(d, "inputs")?;
    for input in inputs {
        let name = text(input, "alias")?;
        if !alias(name)
            || [
                "output",
                "engine",
                "params",
                "resources",
                "cache",
                "lineage",
                "ctx",
            ]
            .contains(&name)
        {
            return Err(error(
                "Input alias must be a public non-reserved Python identifier",
            ));
        }
        if (input["branch"]["kind"] == "named") != input["branch"]["name"].is_string() {
            return Err(error("Input branch selector and branch name disagree"));
        }
    }
    let params = array(d, "parameters")?;
    if !unique(params, Some("name"))
        || params.iter().any(|p| {
            !p["name"].as_str().is_some_and(alias) || !parameter(&p["logical_type"], &p["default"])
        })
    {
        return Err(error("Parameter names or typed defaults are invalid"));
    }
    let aliases: BTreeSet<String> = inputs
        .iter()
        .filter_map(|i| i["alias"].as_str().map(str::to_owned))
        .collect();
    let Some(inputs) = d["inputs"].as_array_mut() else {
        return Err(error("Inputs are malformed"));
    };
    for input in inputs {
        effective_checks(&mut input["checks"], config, &loc, &aliases)?;
    }
    effective_checks(&mut d["output"]["checks"], config, &loc, &aliases)?;
    Ok(())
}
fn effective_checks(
    checks: &mut Value,
    config: &WorkspaceConfig,
    loc: &Location,
    aliases: &BTreeSet<String>,
) -> Result<(), ValidationError> {
    let error = |message| fail(message, vec![loc.clone()]);
    let Some(checks) = checks.as_array_mut() else {
        return Err(error("Check declarations are malformed"));
    };
    if !unique(checks, Some("id")) {
        return Err(error("Check IDs must be unique within each binding"));
    }
    for check in checks {
        // Upgrade the pre-AST declaration seed before fingerprinting; never emit it.
        if check["expectation"].get("ast_version").is_none() {
            check["expectation"]["ast_version"] = 1.into();
        }
        tf_protocol::expectation::decode(
            &check["expectation"],
            &aliases.iter().map(String::as_str).collect(),
        )
        .map_err(|e| error(e.0))?;
        if check["null_policy"].is_null() {
            check["null_policy"] = config.null_policy().into();
        }
        if check["sample_rows"].is_null() {
            check["sample_rows"] = config.sample_rows().to_string().into();
        }
    }
    Ok(())
}
fn candidate_error(e: CandidateError, registry: &RegistrySnapshot) -> ValidationError {
    let cycle = e
        .cycle
        .iter()
        .map(|id| match id {
            CandidateIdentity::Pending(path) => path.as_str().to_owned(),
            CandidateIdentity::Registered(key) => registry
                .datasets()
                .find(|d| d.key() == *key)
                .map(|d| d.path().as_str().to_owned())
                .unwrap_or_else(|| format!("dataset:{}", key.dataset_id())),
        })
        .collect();
    ValidationError {
        message: e.message,
        locations: e.locations,
        cycle,
    }
}
fn schema_columns(schema: &Value) -> Option<BTreeSet<&str>> {
    schema["fields"]
        .as_array()
        .map(|a| a.iter().filter_map(|f| f["name"].as_str()).collect())
}
/// Validate the complete requested source snapshot. There is deliberately no target/force argument.
pub fn validate(request: &ValidationRequest<'_>) -> Result<ValidatedGraph, ValidationError> {
    let (config, context, evidence) = context(request)?;
    let mut normalized = request.discovery.clone();
    let Some(defs) = normalized["definitions"].as_array_mut() else {
        return Err(fail("Definitions are missing", vec![]));
    };
    for d in defs.iter_mut() {
        declaration(d, &config)?;
    }
    let candidate = CandidateCatalog::prepare(request.registry, request.source, &normalized)
        .map_err(|e| candidate_error(e, request.registry))?;
    let known: BTreeMap<_, _> = candidate
        .definitions()
        .iter()
        .map(|d| (d.output.clone(), &d.declaration["output"]["schema"]))
        .collect();
    let mut deferred = Vec::new();
    for d in candidate.definitions() {
        let bindings = std::iter::once((
            None,
            &d.declaration["output"],
            &d.declaration["output"]["schema"],
        ))
        .chain(d.inputs.iter().map(|i| {
            let schema = if i.declaration["branch"]["kind"] == "named" {
                &Value::Null
            } else {
                known.get(&i.identity).copied().unwrap_or(&Value::Null)
            };
            (Some(i.alias.clone()), &i.declaration, schema)
        }));
        for (input_alias, binding, schema) in bindings {
            let columns = schema_columns(schema);
            deferred.push(DeferredValidation {
                dataset: d.path.as_str().into(),
                input_alias: input_alias.clone(),
                check_id: None,
                reason: if columns.is_some() {
                    DeferredReason::VerifyDeclaredSchema
                } else {
                    DeferredReason::SchemaUnavailable
                },
            });
            for check in array(binding, "checks")? {
                let aliases = d.inputs.iter().map(|i| i.alias.as_str()).collect();
                let ast = tf_protocol::expectation::decode(&check["expectation"], &aliases)
                    .map_err(|e| fail(e.0, vec![d.location.clone()]))?;
                tf_protocol::expectation::validate_schema(&ast, schema)
                    .map_err(|e| fail(e.0, vec![d.location.clone()]))?;
                deferred.push(DeferredValidation {
                    dataset: d.path.as_str().into(),
                    input_alias: input_alias.clone(),
                    check_id: Some(text(check, "id")?.into()),
                    reason: if columns.is_some() {
                        DeferredReason::EvaluateData
                    } else {
                        DeferredReason::SchemaUnavailable
                    },
                });
            }
        }
    }
    let candidate_digest = digest(
        &json!({"candidate_format":1,"effective_discovery":normalized,"context":context.hex()}),
    )?;
    let fingerprint = digest(
        &json!({"certificate_format":1,"context":context.hex(),"candidate":candidate_digest.hex()}),
    )?;
    Ok(ValidatedGraph {
        candidate,
        certificate: ValidationCertificate {
            evidence,
            context,
            candidate: candidate_digest,
            fingerprint,
        },
        deferred,
    })
}
/// Display may retain the last valid graph; execution eligibility follows the latest submission.
#[derive(Default)]
pub struct ValidationState {
    last_valid: Option<ValidatedGraph>,
    latest_valid: bool,
}
impl ValidationState {
    /// A failed requested source never overwrites the display graph or silently enables its reuse.
    pub fn submit(
        &mut self,
        request: &ValidationRequest<'_>,
    ) -> Result<&ValidatedGraph, ValidationError> {
        self.latest_valid = false;
        let graph = validate(request)?;
        self.last_valid = Some(graph);
        self.latest_valid = true;
        self.current_for(request)
    }
    /// Last successful immutable graph for explicitly stale/invalid-working-copy display.
    pub fn last_valid(&self) -> Option<&ValidatedGraph> {
        self.last_valid.as_ref()
    }
    /// Requested source must match and the latest submission must have succeeded.
    pub fn current_for(
        &self,
        request: &ValidationRequest<'_>,
    ) -> Result<&ValidatedGraph, ValidationError> {
        self.last_valid
            .as_ref()
            .filter(|g| self.latest_valid && g.certificate.matches(request))
            .ok_or_else(|| {
                fail(
                    "The requested source has no current successful structural validation",
                    vec![],
                )
            })
    }
}
