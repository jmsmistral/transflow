//! Read-only declared lineage over one completely validated source. No execution-scope pruning.
use crate::{
    candidate::CandidateIdentity as Id,
    validation::{ValidatedGraph, ValidationRequest},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tf_domain::{
    BranchName, BranchSelector, DatasetId, DatasetKey, DatasetPath, SourceSnapshotId, WorkspaceId,
    input::InputRole,
};
use tf_protocol::canonical::{DigestKind, content_digest};

/// Traversal rejects mismatched certificates/cursors instead of changing context mid-query.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    /// Full captured validation must match the selected context.
    #[error("Graph traversal requires a matching complete validation certificate")]
    Context,
    /// Exact active registry identity or pending producer path is required.
    #[error("The starting dataset is absent from this captured graph")]
    Missing,
    /// CLI adapters must not accept signed, fractional or overflowing depths.
    #[error("Depth must be a nonnegative integer")]
    Depth,
    /// Cursors are valid only for the same immutable context and traversal request.
    #[error("Graph cursor does not match this traversal; restart pagination")]
    Cursor,
    /// Transport batches are bounded independently of semantic traversal depth.
    #[error("Graph page size must be between 1 and 100")]
    PageSize,
}
/// Parse a supplied depth without accepting negative, signed, fractional or whitespace values.
/// Absence of a depth is represented by None in Request, not a numeric sentinel.
pub fn parse_depth(text: &str) -> Result<u64, Error> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Depth);
    }
    text.parse().map_err(|_| Error::Depth)
}
/// Directed distance is measured from the starting node, which is always at zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// Follow consumer-to-input links.
    Upstream,
    /// Follow input-to-consumer links, including consumers of a foreign starting node.
    Downstream,
}
/// Explicit, immutable query semantics. No build mode, force or exclusion can prune this graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// Registered owner-qualified identity or pending captured output path.
    pub start: Id,
    /// Direction along declared edges.
    pub direction: Direction,
    /// Maximum shortest hop distance; None returns the entire reachable local graph.
    pub depth: Option<u64>,
}
/// Source/registry/branch identity accompanying every page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Context {
    /// Owning local workspace.
    pub workspace: WorkspaceId,
    /// Captured source UUID, independent of data branch.
    pub source: SourceSnapshotId,
    /// Verified source bytes.
    pub source_digest: String,
    /// Complete registry revision, including aliases and external registrations.
    pub registry_fingerprint: String,
    /// Binds the entire validated candidate, configuration, environment and check semantics.
    pub certificate_fingerprint: String,
    /// Selected data branch label; traversal does not resolve moving heads or fallback.
    pub branch: BranchName,
}
/// One unique identity. Multiple external registrations/renames remain visible as paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    /// Owner-qualified identity or unregistered output symbol.
    pub identity: Id,
    /// All captured canonical/alias paths, in stable lexical order.
    pub paths: Vec<DatasetPath>,
    /// Explicit foreign read boundary; provider lineage is not loaded or executed.
    pub external: bool,
    /// Whether this source contains an executable local declaration.
    pub producer: bool,
}
/// One alias-qualified edge in original parent-to-consumer orientation in either query direction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edge {
    /// Input identity; repeated aliases never collapse this field into a single edge.
    pub parent: Id,
    /// Producing output identity.
    pub consumer: Id,
    /// Unique input alias within the consumer.
    pub alias: String,
    /// Both data and validation dependencies are traversed.
    pub role: InputRole,
    /// Omitted, CURRENT and named selectors remain distinct.
    pub declared_branch: BranchSelector,
    /// Authored fallback suppression, without resolving a head.
    pub stop_branch_fallback: bool,
    /// Complete normalized check declarations from the validated candidate.
    pub checks: Value,
}
/// One unique node with its minimum directed hop distance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Visit {
    /// Captured identity and boundary metadata.
    pub node: Node,
    /// Shortest distance, independent of alternate diamond paths and alias count.
    pub depth: u64,
}
/// Bounded transport page. Node and edge streams both continue through the same cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page {
    /// Full selected graph identity, including the validation certificate.
    pub context: Context,
    /// Exact starting node/direction/depth shared by every page.
    pub request: Request,
    /// Unique nodes ordered by (minimum depth, stable identity).
    pub nodes: Vec<Visit>,
    /// Unique edges ordered by (consumer identity, alias); endpoints may be on another page.
    pub edges: Vec<Edge>,
    /// Total unique nodes inside the requested depth.
    pub total_nodes: usize,
    /// Total alias-qualified induced edges inside the requested depth.
    pub total_edges: usize,
    /// Nodes inside the requested depth awaiting later pages.
    pub remaining_nodes: usize,
    /// Edges inside the requested depth awaiting later pages.
    pub remaining_edges: usize,
    /// Exact reachable node count excluded only by the requested depth.
    pub omitted_nodes: usize,
    /// Exact reachable edge count excluded only by the requested depth.
    pub omitted_edges: usize,
    /// True if the selected depth covers all reachable local/boundary nodes.
    /// This is independent of remaining delivery pages and does not imply provider expansion.
    pub scope_complete: bool,
    /// None only when both transport streams have been delivered in full.
    pub next_cursor: Option<String>,
}
/// Immutable indexed graph; construction is gated on complete structural validation.
#[derive(Clone, Debug)]
pub struct Graph {
    context: Context,
    nodes: BTreeMap<Id, Node>,
    edges: Vec<Edge>,
    upstream: BTreeMap<Id, BTreeSet<Id>>,
    downstream: BTreeMap<Id, BTreeSet<Id>>,
    names: BTreeMap<String, Id>,
}
impl Graph {
    /// Freeze every active registry node, pending output and declared edge. No source or data I/O.
    pub fn validated(
        graph: &ValidatedGraph,
        request: &ValidationRequest<'_>,
        branch: BranchName,
    ) -> Result<Self, Error> {
        if !graph.certificate().matches(request) {
            return Err(Error::Context);
        }
        let mut value = Self {
            context: Context {
                workspace: request.registry.workspace(),
                source: request.source,
                source_digest: request.source_digest.into(),
                registry_fingerprint: request.registry.fingerprint().hex(),
                certificate_fingerprint: graph.certificate().fingerprint().hex(),
                branch,
            },
            nodes: BTreeMap::new(),
            edges: vec![],
            upstream: BTreeMap::new(),
            downstream: BTreeMap::new(),
            names: BTreeMap::new(),
        };
        for dataset in request.registry.datasets().filter(|d| !d.is_tombstone()) {
            let id = Id::Registered(dataset.key());
            value.node(id.clone(), dataset.path().clone(), false, false);
            for alias in request.registry.aliases(dataset.key().dataset_id()) {
                value.node(
                    id.clone(),
                    alias.parse().map_err(|_| Error::Context)?,
                    false,
                    false,
                );
            }
        }
        for external in request.registry.external_registrations() {
            value.node(
                Id::Registered(external.key()),
                external.alias().clone(),
                true,
                false,
            );
        }
        for definition in graph.candidate().definitions() {
            value.node(
                definition.output.clone(),
                definition.path.clone(),
                false,
                true,
            );
        }
        for definition in graph.candidate().definitions() {
            for input in &definition.inputs {
                if !value.nodes.contains_key(&input.identity) {
                    return Err(Error::Context);
                }
                let d = &input.declaration;
                let declared_branch = match d["branch"]["kind"].as_str() {
                    Some("omitted") => BranchSelector::Omitted,
                    Some("current") => BranchSelector::Current,
                    Some("named") => BranchSelector::Named(
                        d["branch"]["name"]
                            .as_str()
                            .ok_or(Error::Context)?
                            .parse()
                            .map_err(|_| Error::Context)?,
                    ),
                    _ => return Err(Error::Context),
                };
                let role = match d["role"].as_str() {
                    Some("data") => InputRole::Data,
                    Some("validation") => InputRole::Validation,
                    _ => return Err(Error::Context),
                };
                value.edges.push(Edge {
                    parent: input.identity.clone(),
                    consumer: definition.output.clone(),
                    alias: input.alias.clone(),
                    role,
                    declared_branch,
                    stop_branch_fallback: d["stop_branch_fallback"]
                        .as_bool()
                        .ok_or(Error::Context)?,
                    checks: d["checks"].clone(),
                });
                value
                    .upstream
                    .entry(definition.output.clone())
                    .or_default()
                    .insert(input.identity.clone());
                value
                    .downstream
                    .entry(input.identity.clone())
                    .or_default()
                    .insert(definition.output.clone());
            }
        }
        value
            .edges
            .sort_by(|a, b| (&a.consumer, &a.alias).cmp(&(&b.consumer, &b.alias)));
        for node in value.nodes.values_mut() {
            node.paths.sort();
            node.paths.dedup();
        }
        Ok(value)
    }
    fn node(&mut self, id: Id, path: DatasetPath, external: bool, producer: bool) {
        self.names.insert(path.to_string(), id.clone());
        let node = self.nodes.entry(id.clone()).or_insert_with(|| Node {
            identity: id,
            paths: vec![],
            external,
            producer,
        });
        node.paths.push(path);
        node.producer |= producer;
    }
    /// Resolve exact local/foreign paths, aliases, pending paths or local dataset:UUID syntax.
    pub fn resolve(&self, reference: &str) -> Result<Id, Error> {
        if let Some(id) = self.names.get(reference) {
            return Ok(id.clone());
        }
        if let Some(id) = reference
            .strip_prefix("dataset:")
            .and_then(|s| s.parse::<DatasetId>().ok())
        {
            let key = Id::Registered(DatasetKey::new(self.context.workspace, id));
            if self.nodes.contains_key(&key) {
                return Ok(key);
            }
        }
        Err(Error::Missing)
    }
    /// One breadth-first search enqueues each identity once, with no recursion/path expansion.
    /// Counts omitted depth continuation exactly; page size never caps semantic reachability.
    pub fn traverse(&self, request: Request) -> Result<Traversal, Error> {
        if !self.nodes.contains_key(&request.start) {
            return Err(Error::Missing);
        }
        let adjacency = match request.direction {
            Direction::Upstream => &self.upstream,
            Direction::Downstream => &self.downstream,
        };
        let mut distances = BTreeMap::from([(request.start.clone(), 0u64)]);
        let mut queue = VecDeque::from([request.start.clone()]);
        while let Some(id) = queue.pop_front() {
            let next = distances
                .get(&id)
                .ok_or(Error::Context)?
                .checked_add(1)
                .ok_or(Error::Depth)?;
            if let Some(neighbours) = adjacency.get(&id) {
                for neighbour in neighbours {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        distances.entry(neighbour.clone())
                    {
                        entry.insert(next);
                        queue.push_back(neighbour.clone());
                    }
                }
            }
        }
        let mut nodes = vec![];
        let mut selected = BTreeSet::new();
        for (id, depth) in &distances {
            if request.depth.is_none_or(|limit| *depth <= limit) {
                nodes.push(Visit {
                    node: self.nodes.get(id).ok_or(Error::Context)?.clone(),
                    depth: *depth,
                });
                selected.insert(id.clone());
            }
        }
        nodes.sort_by(|a, b| (a.depth, &a.node.identity).cmp(&(b.depth, &b.node.identity)));
        let mut edges = vec![];
        let mut omitted_edges = 0;
        for edge in &self.edges {
            if distances.contains_key(&edge.parent) && distances.contains_key(&edge.consumer) {
                if selected.contains(&edge.parent) && selected.contains(&edge.consumer) {
                    edges.push(edge.clone());
                } else {
                    omitted_edges += 1;
                }
            }
        }
        let identity = json!({"traversal_version":1,"workspace":self.context.workspace.to_string(),"source":self.context.source.to_string(),"source_digest":self.context.source_digest,"registry":self.context.registry_fingerprint,"certificate":self.context.certificate_fingerprint,"branch":self.context.branch.as_str(),"start":identity(&request.start),"direction":match request.direction{Direction::Upstream=>"upstream",Direction::Downstream=>"downstream"},"depth":request.depth.map(|d|d.to_string())});
        let fingerprint = content_digest(DigestKind::Compute, &identity)
            .map_err(|_| Error::Context)?
            .hex();
        Ok(Traversal {
            context: self.context.clone(),
            request,
            omitted_nodes: distances.len() - nodes.len(),
            omitted_edges,
            nodes,
            edges,
            fingerprint,
        })
    }
}
fn identity(id: &Id) -> Value {
    match id {
        Id::Registered(key) => {
            json!({"workspace":key.workspace_id().to_string(),"dataset":key.dataset_id().to_string()})
        }
        Id::Pending(path) => json!({"pending":path.as_str()}),
    }
}
/// Frozen query result; pagination does not repeat graph traversal or consult changing state.
#[derive(Clone, Debug)]
pub struct Traversal {
    context: Context,
    request: Request,
    nodes: Vec<Visit>,
    edges: Vec<Edge>,
    omitted_nodes: usize,
    omitted_edges: usize,
    fingerprint: String,
}
impl Traversal {
    /// Read bounded node and edge batches. Changing source, registry, branch, environment,
    /// starting node, direction or depth invalidates a previous cursor. Page size may change.
    pub fn page(&self, cursor: Option<&str>, limit: usize) -> Result<Page, Error> {
        if !(1..=100).contains(&limit) {
            return Err(Error::PageSize);
        }
        let (node, edge) = if let Some(cursor) = cursor {
            if cursor.len() > 128 {
                return Err(Error::Cursor);
            }
            let parts: Vec<_> = cursor.split(':').collect();
            let [version, fingerprint, node, edge] = parts.as_slice() else {
                return Err(Error::Cursor);
            };
            if *version != "tftr1" || *fingerprint != self.fingerprint {
                return Err(Error::Cursor);
            }
            let number = |s: &str| -> Result<usize, Error> {
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(Error::Cursor);
                }
                s.parse().map_err(|_| Error::Cursor)
            };
            (number(node)?, number(edge)?)
        } else {
            (0, 0)
        };
        if node > self.nodes.len()
            || edge > self.edges.len()
            || (cursor.is_some() && node == self.nodes.len() && edge == self.edges.len())
        {
            return Err(Error::Cursor);
        }
        let node_end = node.saturating_add(limit).min(self.nodes.len());
        let edge_end = edge.saturating_add(limit).min(self.edges.len());
        let remaining_nodes = self.nodes.len() - node_end;
        let remaining_edges = self.edges.len() - edge_end;
        Ok(Page {
            context: self.context.clone(),
            request: self.request.clone(),
            nodes: self
                .nodes
                .get(node..node_end)
                .ok_or(Error::Cursor)?
                .to_vec(),
            edges: self
                .edges
                .get(edge..edge_end)
                .ok_or(Error::Cursor)?
                .to_vec(),
            total_nodes: self.nodes.len(),
            total_edges: self.edges.len(),
            remaining_nodes,
            remaining_edges,
            omitted_nodes: self.omitted_nodes,
            omitted_edges: self.omitted_edges,
            scope_complete: self.omitted_nodes == 0,
            next_cursor: (remaining_nodes > 0 || remaining_edges > 0)
                .then(|| format!("tftr1:{}:{node_end}:{edge_end}", self.fingerprint)),
        })
    }
}
