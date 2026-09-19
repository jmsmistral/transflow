//! Whole-pass symbolic resolution. No filesystem, random allocation or storage access.
use crate::{DatasetKind, RegistryErrorKind, RegistrySnapshot, Resolved};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{DatasetId, DatasetKey, DatasetPath, DatasetScope, SourceSnapshotId};
use tf_protocol::{canonical::ContentDigest, validate_document};

/// Validation identity: pending names cannot be confused with durable UUIDs.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum CandidateIdentity {
    /// Existing local or explicitly registered foreign identity.
    Registered(DatasetKey),
    /// Pure-validation symbol, not a UUID allocation or persistent record.
    Pending(DatasetPath),
}
/// Definition/input location retained for structured diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Location {
    /// Captured workspace-relative source file.
    pub path: String,
    /// One-based declaration line.
    pub line: u64,
}
/// Actionable preparation failure; no failed candidate is returned.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CandidateError {
    /// Safe human explanation.
    pub message: &'static str,
    /// Relevant source locations, including both sides of collisions.
    pub locations: Vec<Location>,
    /// Concrete closed cycle path when cycle validation fails.
    pub cycle: Vec<CandidateIdentity>,
}
fn error(message: &'static str, locations: Vec<Location>) -> CandidateError {
    CandidateError {
        message,
        locations,
        cycle: Vec::new(),
    }
}
/// Alias-preserving dependency; named branches never erase graph edges.
#[derive(Clone, Debug)]
pub struct CandidateInput {
    /// Authored alias.
    pub alias: String,
    /// Resolved symbolic or durable identity.
    pub identity: CandidateIdentity,
    /// Immutable validated wire binding including branch/role/check policies.
    pub declaration: Value,
}
/// One originating producer and its resolved output.
#[derive(Clone, Debug)]
pub struct CandidateDefinition {
    /// Registered or pending output identity.
    pub output: CandidateIdentity,
    /// Canonical output path.
    pub path: DatasetPath,
    /// Source location.
    pub location: Location,
    /// Every binding remains separate, including repeated same-dataset aliases.
    pub inputs: Vec<CandidateInput>,
    /// Complete immutable declaration carrier for later structural/planning checks.
    pub declaration: Value,
}
/// Structurally reconciled candidate. Construction collects outputs before inputs.
#[derive(Clone, Debug)]
pub struct CandidateCatalog {
    registry: RegistrySnapshot,
    source: SourceSnapshotId,
    catalog_fingerprint: String,
    definitions: Vec<CandidateDefinition>,
    pending: BTreeMap<DatasetPath, DatasetKind>,
    order: Vec<CandidateIdentity>,
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, CandidateError> {
    value[key]
        .as_str()
        .ok_or_else(|| error("Discovery metadata is malformed", Vec::new()))
}
fn reference(
    registry: &RegistrySnapshot,
    fingerprint: &str,
    value: &Value,
) -> Result<String, CandidateError> {
    if value["form"] == "string" {
        return Ok(text(value, "value")?.to_owned());
    }
    if value["catalog_fingerprint"] != fingerprint {
        return Err(error(
            "A bound C reference belongs to a different catalogue snapshot",
            Vec::new(),
        ));
    }
    let path = text(value, "path")?;
    let resolved = registry.resolve(path).map_err(|_| {
        error(
            "A bound C reference is not registered in this snapshot",
            Vec::new(),
        )
    })?;
    if value["workspace_id"] != resolved.key().workspace_id().to_string()
        || value["dataset_id"] != resolved.key().dataset_id().to_string()
    {
        return Err(error(
            "A bound C reference has a mismatched dataset owner or identity",
            Vec::new(),
        ));
    }
    Ok(path.to_owned())
}
impl CandidateCatalog {
    /// Reconcile an entire discovery result against the exact source/registry context.
    /// No UUID is allocated and no external workspace is opened.
    pub fn prepare(
        registry: &RegistrySnapshot,
        source: SourceSnapshotId,
        result: &Value,
    ) -> Result<Self, CandidateError> {
        validate_document("DiscoveryResultV1", result).map_err(|_| {
            error(
                "Discovery result does not match its supported contract",
                Vec::new(),
            )
        })?;
        let projection = registry
            .sdk_projection(source)
            .map_err(|_| error("Catalogue projection could not be verified", Vec::new()))?;
        if result["source_snapshot_id"] != source.to_string()
            || result["catalog_fingerprint"] != projection["catalog_fingerprint"]
        {
            return Err(error(
                "Discovery result is stale for this source or registry",
                Vec::new(),
            ));
        }
        let fingerprint = text(&projection, "catalog_fingerprint")?.to_owned();
        let declarations = result["definitions"]
            .as_array()
            .ok_or_else(|| error("Discovery definitions are missing", Vec::new()))?;
        let mut definitions = Vec::new();
        let mut pending = BTreeMap::new();
        let mut modules = BTreeMap::new();
        let mut outputs = BTreeMap::new();
        for declaration in declarations {
            let location = Location {
                path: text(declaration, "path")?.to_owned(),
                line: declaration["line"]
                    .as_u64()
                    .ok_or_else(|| error("Discovery source line is invalid", Vec::new()))?,
            };
            if let Some(previous) =
                modules.insert(text(declaration, "module")?.to_owned(), location.clone())
            {
                return Err(error(
                    "A producing module must have exactly one producer",
                    vec![previous, location],
                ));
            }
            let output = reference(registry, &fingerprint, &declaration["output"]["ref"]).map_err(
                |mut e| {
                    e.locations.push(location.clone());
                    e
                },
            )?;
            let kind = if declaration["source"] == true {
                DatasetKind::Source
            } else {
                DatasetKind::Transform
            };
            let (identity, path) = match registry.resolve_output(&output) {
                Ok(dataset) => {
                    if dataset.kind() != kind {
                        return Err(error(
                            "An output cannot silently change an existing dataset kind",
                            vec![location],
                        ));
                    }
                    (
                        CandidateIdentity::Registered(dataset.key()),
                        dataset.path().clone(),
                    )
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        RegistryErrorKind::Missing | RegistryErrorKind::Namespace
                    ) && declaration["output"]["ref"]["form"] == "string"
                        && !output.starts_with("dataset:") =>
                {
                    let path = DatasetPath::parse_for_scope(&output, DatasetScope::Local).map_err(
                        |_| {
                            error(
                                "A new output must be a valid local dataset path",
                                vec![location.clone()],
                            )
                        },
                    )?;
                    pending.insert(path.clone(), kind);
                    (CandidateIdentity::Pending(path.clone()), path)
                }
                Err(_) => {
                    return Err(error(
                        "Output is missing, tombstoned or foreign; only new local string paths can register",
                        vec![location],
                    ));
                }
            };
            if let Some(previous) = outputs.insert(identity.clone(), location.clone()) {
                return Err(error(
                    "More than one producer declares the same dataset",
                    vec![previous, location],
                ));
            }
            definitions.push(CandidateDefinition {
                output: identity,
                path,
                location,
                inputs: Vec::new(),
                declaration: declaration.clone(),
            });
        }
        let candidates: BTreeMap<_, _> = definitions
            .iter()
            .map(|d| (d.path.as_str().to_owned(), d.output.clone()))
            .collect();
        for definition in &mut definitions {
            let inputs = definition.declaration["inputs"].as_array().ok_or_else(|| {
                error(
                    "Input declarations are missing",
                    vec![definition.location.clone()],
                )
            })?;
            if definition.declaration["source"] == true && !inputs.is_empty() {
                return Err(error(
                    "Source transforms cannot declare dataframe inputs",
                    vec![definition.location.clone()],
                ));
            }
            let mut aliases = BTreeSet::new();
            for input in inputs {
                let alias = text(input, "alias")?.to_owned();
                if !aliases.insert(alias.clone()) {
                    return Err(error(
                        "Input aliases must be unique within a producer",
                        vec![definition.location.clone()],
                    ));
                }
                let name = reference(registry, &fingerprint, &input["ref"]).map_err(|mut e| {
                    e.locations.push(definition.location.clone());
                    e
                })?;
                let identity = match registry.resolve(&name) {
                    Ok(Resolved::Local { dataset, .. }) => {
                        CandidateIdentity::Registered(dataset.key())
                    }
                    Ok(Resolved::External(e)) => CandidateIdentity::Registered(e.key()),
                    Err(e)
                        if matches!(
                            e.kind(),
                            RegistryErrorKind::Missing | RegistryErrorKind::Namespace
                        ) =>
                    {
                        candidates.get(&name).cloned().ok_or_else(|| {
                            error(
                                "An input is unresolved; inputs alone never register datasets",
                                vec![definition.location.clone()],
                            )
                        })?
                    }
                    Err(_) => {
                        return Err(error(
                            "An input reference is invalid or tombstoned",
                            vec![definition.location.clone()],
                        ));
                    }
                };
                definition.inputs.push(CandidateInput {
                    alias,
                    identity,
                    declaration: input.clone(),
                });
            }
        }
        definitions.sort_by(|a, b| a.output.cmp(&b.output));
        let order = acyclic_order(&definitions)?;
        Ok(Self {
            registry: registry.clone(),
            source,
            catalog_fingerprint: fingerprint,
            definitions,
            pending,
            order,
        })
    }
    /// Complete resolved definitions; validation-only and repeated aliases are retained.
    pub fn definitions(&self) -> &[CandidateDefinition] {
        &self.definitions
    }
    /// Canonically ordered new names, carrying no UUIDs during validation.
    pub fn pending(&self) -> &BTreeMap<DatasetPath, DatasetKind> {
        &self.pending
    }
    /// Parents before consumers; unrelated registered identities use stable-ID ordering.
    pub fn topological_order(&self) -> &[CandidateIdentity] {
        &self.order
    }
    /// Allocate proposed IDs only after successful candidate validation. Saving the returned
    /// proposal, not rerunning this allocator, preserves a draft's exact identities.
    pub fn propose(
        &self,
        mut allocate: impl FnMut() -> Result<DatasetId, CandidateError>,
    ) -> Result<RegistryProposal, CandidateError> {
        let mut assignments = Vec::new();
        for (path, kind) in &self.pending {
            assignments.push((path.clone(), allocate()?, *kind));
        }
        let replacement = self.registry.render_additions(&assignments).map_err(|_| {
            error(
                "Proposed dataset IDs collide with durable registry identities",
                Vec::new(),
            )
        })?;
        Ok(RegistryProposal {
            workspace: self.registry.workspace(),
            source: self.source,
            expected_old: *self.registry.raw_digest(),
            catalog_fingerprint: self.catalog_fingerprint.clone(),
            assignments,
            replacement,
        })
    }
}
/// An exact additive draft, distinct from a pure candidate or committed registry.
#[derive(Clone, Debug)]
pub struct RegistryProposal {
    workspace: tf_domain::WorkspaceId,
    source: SourceSnapshotId,
    expected_old: ContentDigest,
    catalog_fingerprint: String,
    assignments: Vec<(DatasetPath, DatasetId, DatasetKind)>,
    replacement: String,
}
impl RegistryProposal {
    /// Durable workspace context.
    pub fn workspace(&self) -> tf_domain::WorkspaceId {
        self.workspace
    }
    /// Source context to recheck before writes.
    pub fn source(&self) -> SourceSnapshotId {
        self.source
    }
    /// Byte-level expected-old registry guard.
    pub fn expected_old(&self) -> &ContentDigest {
        &self.expected_old
    }
    /// Captured SDK catalogue projection fingerprint.
    pub fn catalog_fingerprint(&self) -> &str {
        &self.catalog_fingerprint
    }
    /// Exact assigned IDs retained by this draft.
    pub fn assignments(&self) -> &[(DatasetPath, DatasetId, DatasetKind)] {
        &self.assignments
    }
    /// Complete deterministic proposed registry bytes, not yet written anywhere.
    pub fn replacement(&self) -> &str {
        &self.replacement
    }
}
fn acyclic_order(
    definitions: &[CandidateDefinition],
) -> Result<Vec<CandidateIdentity>, CandidateError> {
    let graph: BTreeMap<_, Vec<_>> = definitions
        .iter()
        .map(|d| {
            (
                d.output.clone(),
                d.inputs.iter().map(|i| i.identity.clone()).collect(),
            )
        })
        .collect();
    let mut colors = BTreeMap::new();
    let mut order = Vec::new();
    for root in graph.keys() {
        if colors.get(root) == Some(&2) {
            continue;
        }
        let mut stack = vec![(root.clone(), 0usize)];
        colors.insert(root.clone(), 1);
        while let Some((node, index)) = stack.last().cloned() {
            let children = graph.get(&node).map(Vec::as_slice).unwrap_or(&[]);
            if let Some(child) = children.get(index) {
                if let Some(frame) = stack.last_mut() {
                    frame.1 += 1;
                }
                if !graph.contains_key(child) {
                    continue;
                }
                match colors.get(child) {
                    Some(1) => {
                        let start = stack.iter().position(|(n, _)| n == child).unwrap_or(0);
                        let mut cycle: Vec<_> =
                            stack.iter().skip(start).map(|(n, _)| n.clone()).collect();
                        cycle.push(child.clone());
                        let locations = definitions
                            .iter()
                            .filter(|d| cycle.contains(&d.output))
                            .map(|d| d.location.clone())
                            .collect();
                        return Err(CandidateError {
                            message: "A dependency cycle was found in the complete local graph",
                            locations,
                            cycle,
                        });
                    }
                    Some(2) => {}
                    _ => {
                        colors.insert(child.clone(), 1);
                        stack.push((child.clone(), 0));
                    }
                }
            } else {
                stack.pop();
                colors.insert(node.clone(), 2);
                order.push(node);
            }
        }
    }
    // Stable-ID/path tie-breaking among all currently ready producers, not DFS roots.
    let mut degree = BTreeMap::new();
    let mut consumers: BTreeMap<CandidateIdentity, BTreeSet<CandidateIdentity>> = BTreeMap::new();
    for (node, parents) in &graph {
        let unique: BTreeSet<_> = parents.iter().filter(|p| graph.contains_key(*p)).collect();
        degree.insert(node.clone(), unique.len());
        for parent in unique {
            consumers
                .entry(parent.clone())
                .or_default()
                .insert(node.clone());
        }
    }
    let mut ready: BTreeSet<_> = degree
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(k, _)| k.clone())
        .collect();
    order.clear();
    while let Some(node) = ready.pop_first() {
        if let Some(children) = consumers.get(&node) {
            for child in children {
                if let Some(count) = degree.get_mut(child) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }
        order.push(node);
    }
    Ok(order)
}
