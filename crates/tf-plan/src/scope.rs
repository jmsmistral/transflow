//! Pure alias-preserving scope selection over a completely validated graph.
use crate::refresh::RefreshPolicy;
use std::collections::{BTreeMap, BTreeSet};
use tf_catalog::{
    candidate::CandidateIdentity as Id,
    validation::{ValidatedGraph, ValidationRequest},
};
use tf_domain::BranchName;
/// An omitted mode is Full.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    #[default]
    /// Consider relevant eligible ancestors.
    Full,
    /// Only explicit producer targets may run.
    Selected,
    /// Run paths after boundaries up to targets.
    Between,
}
/// Symbolic identities permit planning outputs before durable registration.
#[derive(Clone, Debug, Default)]
pub struct Request {
    /// Requested traversal rule.
    pub mode: Mode,
    /// Explicit active producer targets.
    pub targets: BTreeSet<Id>,
    /// Datasets deliberately supplied by retained versions.
    pub boundaries: BTreeSet<Id>,
    /// Producers forbidden from execution and traversal.
    pub exclusions: BTreeSet<Id>,
    /// Exact pins keyed by consumer and alias.
    pub pins: BTreeSet<(Id, String)>,
    /// Explicit source refresh authorizations within the scope.
    pub refresh_sources: BTreeSet<Id>,
}
#[derive(Clone, Debug)]
struct Edge {
    alias: String,
    parent: Id,
    off_branch: bool,
}
/// Immutable complete graph; only a matching structural certificate constructs it.
#[derive(Clone, Debug)]
pub struct Graph {
    order: Vec<Id>,
    inputs: BTreeMap<Id, Vec<Edge>>,
    sources: BTreeMap<Id, RefreshPolicy>,
    known: BTreeSet<Id>,
}
/// Every input of every selected producer appears exactly once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    /// Owning producer.
    pub consumer: Id,
    /// Original distinct input alias.
    pub alias: String,
    /// Qualified or proposed upstream identity.
    pub parent: Id,
    /// True binds a successful in-build result, never an old head.
    pub planned: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// Complete deterministic write and read scope.
pub struct Scope {
    /// Deterministic parents-before-consumers order.
    pub writes: Vec<Id>,
    /// All selected consumers’ inputs, including side and validation inputs.
    pub bindings: Vec<Binding>,
}
impl Graph {
    /// Construct only from matching full-workspace structural evidence.
    pub fn validated(
        graph: &ValidatedGraph,
        context: &ValidationRequest<'_>,
        output: &BranchName,
    ) -> Result<Self, &'static str> {
        if !graph.certificate().matches(context) {
            return Err("Graph validation context changed; validate the full capture again");
        }
        let mut inputs = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut known = BTreeSet::new();
        for d in graph.candidate().definitions() {
            known.insert(d.output.clone());
            let mut edges = Vec::new();
            for i in &d.inputs {
                known.insert(i.identity.clone());
                let foreign = matches!(&i.identity, Id::Registered(k) if k.workspace_id() != context.registry.workspace());
                edges.push(Edge {
                    alias: i.alias.clone(),
                    parent: i.identity.clone(),
                    off_branch: foreign
                        || (i.declaration["branch"]["kind"] == "named"
                            && i.declaration["branch"]["name"].as_str() != Some(output.as_str())),
                });
            }
            inputs.insert(d.output.clone(), edges);
            if d.declaration["source"] == true {
                let refresh = &d.declaration["refresh"];
                let policy = match refresh.as_str() {
                    Some("always") => RefreshPolicy::Always,
                    Some("manual") => RefreshPolicy::Manual,
                    _ => RefreshPolicy::Ttl(
                        refresh
                            .as_u64()
                            .and_then(std::num::NonZeroU64::new)
                            .ok_or("Invalid source refresh policy")?,
                    ),
                };
                sources.insert(d.output.clone(), policy);
            }
        }
        Ok(Self {
            order: graph.candidate().topological_order().to_vec(),
            inputs,
            sources,
            known,
        })
    }
    /// Source policies used to explain refresh and cache decisions.
    pub fn sources(&self) -> &BTreeMap<Id, RefreshPolicy> {
        &self.sources
    }
    /// Select without data reads, UUID allocation or execution.
    pub fn select(&self, request: &Request) -> Result<Scope, &'static str> {
        if request.targets.is_empty() || self.inputs.len() > 100_000 {
            return Err("Select at least one local producer within the graph limit");
        }
        if request.mode == Mode::Between && request.boundaries.is_empty() {
            return Err("Between mode requires a read boundary");
        }
        for id in request.targets.iter().chain(&request.refresh_sources) {
            if !self.inputs.contains_key(id) {
                return Err(
                    "Target is not an active local producer; import or refresh it in its owning workspace",
                );
            }
            if request.boundaries.contains(id) || request.exclusions.contains(id) {
                return Err("Target or refresh source conflicts with a boundary or exclusion");
            }
        }
        if request
            .boundaries
            .iter()
            .chain(&request.exclusions)
            .any(|id| !self.known.contains(id))
        {
            return Err("Boundary or exclusion is absent from the validated graph");
        }
        if request
            .refresh_sources
            .iter()
            .any(|id| !self.sources.contains_key(id))
        {
            return Err("Refresh names must identify source producers");
        }
        if request.pins.iter().any(|(id, alias)| {
            !self
                .inputs
                .get(id)
                .is_some_and(|edges| edges.iter().any(|e| &e.alias == alias))
        }) {
            return Err("Pin does not identify an input alias");
        }
        let barrier = |id: &Id| {
            request.boundaries.contains(id)
                || request.exclusions.contains(id)
                || !self.inputs.contains_key(id)
                || (self.sources.get(id) == Some(&RefreshPolicy::Manual)
                    && !request.targets.contains(id)
                    && !request.refresh_sources.contains(id))
        };
        let mut eligible = BTreeSet::new();
        let mut stack: Vec<_> = request.targets.iter().cloned().collect();
        while let Some(id) = stack.pop() {
            if barrier(&id) || !eligible.insert(id.clone()) {
                continue;
            }
            if request.mode != Mode::Selected
                && let Some(edges) = self.inputs.get(&id)
            {
                for e in edges {
                    if !e.off_branch && !request.pins.contains(&(id.clone(), e.alias.clone())) {
                        stack.push(e.parent.clone());
                    }
                }
            }
        }
        if request.mode == Mode::Between {
            let mut reached = BTreeSet::new();
            for id in &self.order {
                if !eligible.contains(id) {
                    continue;
                }
                if self.inputs.get(id).is_some_and(|edges| {
                    edges.iter().any(|e| {
                        request.boundaries.contains(&e.parent)
                            || (!e.off_branch
                                && !request.pins.contains(&(id.clone(), e.alias.clone()))
                                && reached.contains(&e.parent))
                    })
                }) {
                    reached.insert(id.clone());
                }
            }
            if !request.targets.is_subset(&reached) {
                return Err(
                    "Each between target needs a reachable boundary under the requested branch and exclusion barriers",
                );
            }
            eligible = reached;
        }
        if !request.refresh_sources.is_subset(&eligible) {
            return Err(
                "Refresh source is unrelated to the selected scope; it cannot expand the build",
            );
        }
        let mut bindings = Vec::new();
        for id in &eligible {
            if let Some(edges) = self.inputs.get(id) {
                for e in edges {
                    let pinned = request.pins.contains(&(id.clone(), e.alias.clone()));
                    if pinned && !e.off_branch && eligible.contains(&e.parent) {
                        return Err("Exact pin conflicts with a same-branch planned producer");
                    }
                    bindings.push(Binding {
                        consumer: id.clone(),
                        alias: e.alias.clone(),
                        parent: e.parent.clone(),
                        planned: !pinned && !e.off_branch && eligible.contains(&e.parent),
                    });
                }
            }
        }
        Ok(Scope {
            writes: self
                .order
                .iter()
                .filter(|id| eligible.contains(*id))
                .cloned()
                .collect(),
            bindings,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "Synthetic pure graph fixtures")]
    use super::*;
    fn id(n: u8) -> Id {
        Id::Registered(tf_domain::DatasetKey::new(
            tf_domain::WorkspaceId::from_bytes([1; 16]),
            tf_domain::DatasetId::from_bytes([n; 16]),
        ))
    }
    fn graph(edges: &[(u8, u8)]) -> Graph {
        let mut inputs = (1..=7)
            .map(|n| (id(n), Vec::new()))
            .collect::<BTreeMap<_, _>>();
        for &(parent, consumer) in edges {
            inputs.get_mut(&id(consumer)).unwrap().push(Edge {
                alias: format!("p{parent}"),
                parent: id(parent),
                off_branch: false,
            });
        }
        Graph {
            order: (1..=7).map(id).collect(),
            inputs,
            sources: BTreeMap::new(),
            known: (1..=7).map(id).collect(),
        }
    }
    fn request(mode: Mode, targets: &[u8], boundaries: &[u8]) -> Request {
        Request {
            mode,
            targets: targets.iter().map(|n| id(*n)).collect(),
            boundaries: boundaries.iter().map(|n| id(*n)).collect(),
            ..Default::default()
        }
    }
    fn writes(g: &Graph, r: &Request) -> Vec<Id> {
        g.select(r).unwrap().writes
    }
    #[test]
    fn diamond_selected_side_inputs_and_nested_boundary_barriers() {
        let g = graph(&[(1, 2), (2, 3), (2, 4), (3, 5), (4, 5), (6, 5)]);
        assert_eq!(
            writes(&g, &request(Mode::Full, &[5], &[])),
            vec![id(1), id(2), id(3), id(4), id(5), id(6)]
        );
        let s = g.select(&request(Mode::Selected, &[5, 3], &[])).unwrap();
        assert_eq!(s.writes, vec![id(3), id(5)]);
        assert_eq!(s.bindings.iter().filter(|b| b.planned).count(), 1);
        assert_eq!(s.bindings.len(), 4);
        assert_eq!(
            writes(&g, &request(Mode::Between, &[5], &[1, 2])),
            vec![id(3), id(4), id(5)]
        );
        assert_eq!(
            writes(&g, &request(Mode::Between, &[3, 5], &[2])),
            vec![id(3), id(4), id(5)]
        );
        assert!(g.select(&request(Mode::Between, &[5, 7], &[2])).is_err());
        assert!(g.select(&request(Mode::Between, &[5], &[])).is_err());
        assert!(g.select(&request(Mode::Full, &[2], &[2])).is_err());
        let mut r = request(Mode::Full, &[5], &[]);
        r.exclusions.insert(id(3));
        assert_eq!(writes(&g, &r), vec![id(1), id(2), id(4), id(5), id(6)]);
    }
    #[test]
    fn manual_sources_pins_off_branch_and_imports_never_expand_scope() {
        let mut g = graph(&[(1, 2), (2, 3), (4, 3), (5, 3)]);
        g.sources.insert(id(1), RefreshPolicy::Manual);
        g.inputs.remove(&id(5)); // imported/foreign leaf, not a producer
        g.inputs.get_mut(&id(3)).unwrap()[1].off_branch = true;
        let mut r = request(Mode::Full, &[3], &[]);
        assert_eq!(writes(&g, &r), vec![id(2), id(3)]);
        r.refresh_sources.insert(id(1));
        assert_eq!(writes(&g, &r), vec![id(1), id(2), id(3)]);
        r.refresh_sources.insert(id(4));
        assert!(g.select(&r).is_err());
        r.refresh_sources.clear();
        r.pins.insert((id(3), "p2".into()));
        assert_eq!(writes(&g, &r), vec![id(3)]);
        r.targets.insert(id(2));
        assert!(g.select(&r).is_err());
        assert!(g.select(&request(Mode::Full, &[5], &[])).is_err());
    }
    #[test]
    fn exhaustive_small_dags_full_and_between_match_path_oracle() {
        // Every DAG on four ordered nodes, every single target and boundary.
        let pairs = [(1, 2), (1, 3), (1, 4), (2, 3), (2, 4), (3, 4)];
        for mask in 0..64 {
            let edges: Vec<_> = pairs
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, e)| *e)
                .collect();
            let g = graph(&edges);
            for target in 1..=4 {
                let mut ancestors = BTreeSet::from([target]);
                for _ in 0..4 {
                    for &(p, c) in &edges {
                        if ancestors.contains(&c) {
                            ancestors.insert(p);
                        }
                    }
                }
                assert_eq!(
                    writes(&g, &request(Mode::Full, &[target], &[])),
                    ancestors.iter().map(|n| id(*n)).collect::<Vec<_>>()
                );
                for boundary in 1..target {
                    let mut descendants = BTreeSet::from([boundary]);
                    for _ in 0..4 {
                        for &(p, c) in &edges {
                            if descendants.contains(&p) {
                                descendants.insert(c);
                            }
                        }
                    }
                    let result = g.select(&request(Mode::Between, &[target], &[boundary]));
                    if !descendants.contains(&target) {
                        assert!(result.is_err());
                    } else {
                        let expected = ancestors
                            .intersection(&descendants)
                            .filter(|n| **n != boundary)
                            .map(|n| id(*n))
                            .collect::<Vec<_>>();
                        assert_eq!(result.unwrap().writes, expected);
                    }
                }
            }
        }
    }
}
