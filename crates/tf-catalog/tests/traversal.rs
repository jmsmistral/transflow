//! Exact neighbourhoods, alias-preserving pagination and immutable context rejection.
#![allow(clippy::unwrap_used, reason = "Synthetic graph qualification")]
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tf_catalog::{
    RegistrySnapshot,
    candidate::CandidateIdentity as Id,
    traversal::{self, Direction, Error, Graph, Request, Traversal},
    validation::{self, ValidationRequest},
};
use tf_domain::{BranchSelector, DatasetId, SourceSnapshotId, WorkspaceId, input::InputRole};
fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
fn input(alias: &str, path: &str) -> Value {
    json!({"alias":alias,"ref":{"form":"string","value":path},"branch":{"kind":"omitted","name":null},"stop_branch_fallback":false,"role":"data","checks":[]})
}
fn definition(n: usize, inputs: Vec<Value>) -> Value {
    json!({"module":format!("n{n}"),"function":"produce","path":format!("src/n{n}.py"),"line":1,"source":false,"engine":"polars","inputs":inputs,"output":{"ref":{"form":"string","value":format!("data/n{n}")},"checks":[],"schema":null},"parameters":[],"wall_timeout_seconds":null,"cache":"deterministic","refresh":null,"secret_refs":[],"lineage_json":null})
}
struct Fixture {
    registry: RegistrySnapshot,
    config: String,
    modules: BTreeMap<String, String>,
    discovery: Value,
    source: SourceSnapshotId,
    environment: String,
    source_digest: String,
}
impl Fixture {
    fn new(registry: &str, defs: Vec<Value>) -> Self {
        let registry =
            RegistrySnapshot::parse(workspace(), &format!("format_version=1\n{registry}")).unwrap();
        let source = SourceSnapshotId::from_bytes([2; 16]);
        let modules: BTreeMap<_, _> = defs
            .iter()
            .map(|d| {
                (
                    d["module"].as_str().unwrap().into(),
                    d["path"].as_str().unwrap().into(),
                )
            })
            .collect();
        let discovery = json!({"format_version":1,"source_snapshot_id":source.to_string(),"catalog_fingerprint":registry.sdk_projection(source).unwrap()["catalog_fingerprint"],"environment_fingerprint":"a".repeat(64),"definitions":defs,"imported_modules":modules.keys().collect::<Vec<_>>()});
        Self {
            registry,
            source,
            modules,
            discovery,
            config: format!("format_version=1\nworkspace_id='{}'\n", workspace()),
            environment: "a".repeat(64),
            source_digest: "b".repeat(64),
        }
    }
    fn context(&self) -> ValidationRequest<'_> {
        ValidationRequest {
            registry: &self.registry,
            source: self.source,
            source_digest: &self.source_digest,
            config_toml: &self.config,
            modules: &self.modules,
            discovery: &self.discovery,
            environment_fingerprint: &self.environment,
            sdk_version: validation::SDK_VERSION,
            check_semantics: validation::CHECK_SEMANTICS,
        }
    }
    fn graph(&self, branch: &str) -> Graph {
        let context = self.context();
        let g = validation::validate(&context).unwrap();
        Graph::validated(&g, &context, branch.parse().unwrap()).unwrap()
    }
}
fn diamond() -> Fixture {
    Fixture::new(
        "",
        vec![
            definition(0, vec![]),
            definition(1, vec![input("a", "data/n0")]),
            definition(2, vec![input("a", "data/n0")]),
            definition(3, vec![input("a", "data/n1"), input("b", "data/n2")]),
            definition(4, vec![input("a", "data/n3"), input("shortcut", "data/n0")]),
        ],
    )
}
fn query(g: &Graph, start: &str, direction: Direction, depth: Option<u64>) -> Traversal {
    g.traverse(Request {
        start: g.resolve(start).unwrap(),
        direction,
        depth,
    })
    .unwrap()
}
fn all(t: &Traversal, limit: usize) -> (Vec<traversal::Visit>, Vec<traversal::Edge>) {
    let mut cursor = None;
    let mut nodes = vec![];
    let mut edges = vec![];
    let mut pages = 0;
    loop {
        let page = t.page(cursor.as_deref(), limit).unwrap();
        pages += 1;
        assert!(page.nodes.len() <= limit && page.edges.len() <= limit);
        nodes.extend(page.nodes);
        edges.extend(page.edges);
        assert_eq!(nodes.len() + page.remaining_nodes, page.total_nodes);
        assert_eq!(edges.len() + page.remaining_edges, page.total_edges);
        assert!(pages <= page.total_nodes + page.total_edges + 1);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        nodes
            .iter()
            .map(|n| &n.node.identity)
            .collect::<BTreeSet<_>>()
            .len(),
        nodes.len()
    );
    assert_eq!(
        edges
            .iter()
            .map(|e| (&e.consumer, &e.alias))
            .collect::<BTreeSet<_>>()
            .len(),
        edges.len()
    );
    (nodes, edges)
}
#[test]
fn diamond_depth_zero_one_n_and_unlimited_use_shortest_distance() {
    let g = diamond().graph("feature");
    for (depth, count, omitted) in [
        (Some(0), 1, 4),
        (Some(1), 4, 1),
        (Some(2), 5, 0),
        (None, 5, 0),
    ] {
        let t = query(&g, "data/n0", Direction::Downstream, depth);
        let p = t.page(None, 1).unwrap();
        let (nodes, edges) = all(&t, 1);
        assert_eq!(nodes.len(), count);
        assert_eq!(p.omitted_nodes, omitted);
        assert_eq!(p.scope_complete, omitted == 0);
        assert_eq!(nodes[0].depth, 0);
        if depth == Some(0) {
            assert!(edges.is_empty());
            assert_eq!(p.omitted_edges, 6);
        }
        if omitted == 0 {
            assert_eq!(edges.len(), 6);
            assert_eq!(
                nodes
                    .iter()
                    .find(|n| n.node.identity == g.resolve("data/n4").unwrap())
                    .unwrap()
                    .depth,
                1
            );
        }
    }
    let (nodes, edges) = all(&query(&g, "data/n4", Direction::Upstream, None), 2);
    assert_eq!(
        nodes.iter().map(|n| n.depth).collect::<Vec<_>>(),
        vec![0, 1, 1, 2, 2]
    );
    assert_eq!(edges.len(), 6);
}
#[test]
fn large_wide_and_deep_graphs_have_no_hidden_semantic_page_cap() {
    let wide = Fixture::new(
        "",
        (0..=1100)
            .map(|n| {
                definition(
                    n,
                    if n == 0 {
                        vec![]
                    } else {
                        vec![input("root", "data/n0")]
                    },
                )
            })
            .collect(),
    )
    .graph("master");
    let (nodes, edges) = all(&query(&wide, "data/n0", Direction::Downstream, None), 37);
    assert_eq!((nodes.len(), edges.len()), (1101, 1100));
    assert!(nodes.iter().skip(1).all(|n| n.depth == 1));
    let deep = Fixture::new(
        "",
        (0..=1100)
            .map(|n| {
                definition(
                    n,
                    if n == 0 {
                        vec![]
                    } else {
                        vec![input("parent", &format!("data/n{}", n - 1))]
                    },
                )
            })
            .collect(),
    )
    .graph("master");
    let (nodes, edges) = all(&query(&deep, "data/n1100", Direction::Upstream, None), 41);
    assert_eq!((nodes.len(), edges.len()), (1101, 1100));
    assert_eq!(nodes.last().unwrap().depth, 1100);
    let p = query(&deep, "data/n1100", Direction::Upstream, Some(1))
        .page(None, 100)
        .unwrap();
    assert_eq!(p.omitted_nodes, 1099);
    assert!(!p.scope_complete);
}
#[test]
fn edge_stream_continues_after_all_nodes_and_keeps_roles_branches_and_checks() {
    let mut inputs = (0..=249)
        .map(|n| input(&format!("alias_{n}"), "data/n0"))
        .collect::<Vec<_>>();
    inputs[0]["role"] = json!("validation");
    inputs[0]["branch"] = json!({"kind":"named","name":"old"});
    inputs[0]["stop_branch_fallback"] = json!(true);
    inputs[0]["checks"] = json!([{"id":"key","name":"Key","expectation":{"kind":"non_null","columns":["id"]},"on_error":"WARN","null_policy":null,"sample_rows":null,"description":null}]);
    let g = Fixture::new("", vec![definition(0, vec![]), definition(1, inputs)]).graph("feature");
    let t = query(&g, "data/n1", Direction::Upstream, None);
    let p = t.page(None, 100).unwrap();
    assert_eq!(p.remaining_nodes, 0);
    assert_eq!(p.remaining_edges, 150);
    assert!(p.next_cursor.is_some());
    let (nodes, edges) = all(&t, 100);
    assert_eq!((nodes.len(), edges.len()), (2, 250));
    let e = edges.iter().find(|e| e.alias == "alias_0").unwrap();
    assert_eq!(e.role, InputRole::Validation);
    assert_eq!(
        e.declared_branch,
        BranchSelector::Named("old".parse().unwrap())
    );
    assert!(e.stop_branch_fallback);
    assert_eq!(e.checks[0]["on_error"], "WARN");
}
#[test]
fn foreign_boundaries_are_qualified_and_repeated_registrations_share_one_node() {
    let id = DatasetId::from_bytes([3; 16]);
    let foreign = WorkspaceId::from_bytes([8; 16]);
    let mut registry = format!(
        "[[datasets]]\nid='{id}'\npath='data/n0'\nkind='transform'\n[[aliases]]\npath='old/root'\ntarget_id='{id}'\n"
    );
    for (n, alias) in [(4, "one"), (5, "two")] {
        registry.push_str(&format!("[[external_registrations]]\nid='{}'\nalias='external/{alias}/value'\nprovider_workspace_id='{foreign}'\nprovider_dataset_id='{id}'\nprovider_display_path='raw/value'\ndefault_branch='master'\n",DatasetId::from_bytes([n;16])));
    }
    let g = Fixture::new(
        &registry,
        vec![
            definition(0, vec![]),
            definition(
                1,
                vec![
                    input("local", "old/root"),
                    input("remote", "external/one/value"),
                    input("same_remote", "external/two/value"),
                ],
            ),
        ],
    )
    .graph("master");
    let (nodes, edges) = all(&query(&g, "data/n1", Direction::Upstream, None), 1);
    assert_eq!((nodes.len(), edges.len()), (3, 3));
    let external = nodes.iter().find(|n| n.node.external).unwrap();
    assert_eq!(external.node.paths.len(), 2);
    assert!(!external.node.producer);
    assert_ne!(
        external.node.identity,
        g.resolve(&format!("dataset:{id}")).unwrap()
    );
    assert_eq!(g.resolve("old/root"), g.resolve("data/n0"));
    assert_eq!(
        all(
            &query(&g, "external/one/value", Direction::Upstream, None),
            1
        )
        .0
        .len(),
        1
    );
    assert_eq!(
        all(
            &query(&g, "external/two/value", Direction::Downstream, None),
            1
        )
        .0
        .len(),
        2
    );
}
#[test]
fn cursors_bind_source_environment_registry_branch_and_query() {
    let mut f = diamond();
    let g = f.graph("feature");
    let t = query(&g, "data/n0", Direction::Downstream, None);
    let cursor = t.page(None, 1).unwrap().next_cursor.unwrap();
    assert!(t.page(Some(&cursor), 2).is_ok());
    for other in [
        query(&g, "data/n1", Direction::Downstream, None),
        query(&g, "data/n0", Direction::Upstream, None),
        query(&g, "data/n0", Direction::Downstream, Some(1)),
        query(&f.graph("master"), "data/n0", Direction::Downstream, None),
    ] {
        assert_eq!(other.page(Some(&cursor), 1), Err(Error::Cursor));
    }
    f.source_digest = "c".repeat(64);
    assert_eq!(
        query(&f.graph("feature"), "data/n0", Direction::Downstream, None).page(Some(&cursor), 1),
        Err(Error::Cursor)
    );
    f.source_digest = "b".repeat(64);
    f.environment = "d".repeat(64);
    f.discovery["environment_fingerprint"] = json!(f.environment);
    assert_eq!(
        query(&f.graph("feature"), "data/n0", Direction::Downstream, None).page(Some(&cursor), 1),
        Err(Error::Cursor)
    );
    f.environment = "a".repeat(64);
    f.discovery["environment_fingerprint"] = json!(f.environment);
    f.registry = RegistrySnapshot::parse(
        workspace(),
        &format!(
            "format_version=1\n[[datasets]]\nid='{}'\npath='unused'\nkind='imported'\n",
            DatasetId::from_bytes([9; 16])
        ),
    )
    .unwrap();
    f.discovery["catalog_fingerprint"] =
        f.registry.sdk_projection(f.source).unwrap()["catalog_fingerprint"].clone();
    assert_eq!(
        query(&f.graph("feature"), "data/n0", Direction::Downstream, None).page(Some(&cursor), 1),
        Err(Error::Cursor)
    );
    // The original frozen query remains valid; later contexts cannot mutate it.
    assert_eq!(all(&t, 2).0.len(), 5);
}
#[test]
fn invalid_depth_cursor_start_and_complete_validation_fail_explicitly() {
    for text in [
        "",
        "-1",
        "-0",
        "+1",
        "1.0",
        "1e3",
        " 1",
        "18446744073709551616",
    ] {
        assert_eq!(traversal::parse_depth(text), Err(Error::Depth));
    }
    assert_eq!(traversal::parse_depth("0"), Ok(0));
    assert_eq!(traversal::parse_depth("18446744073709551615"), Ok(u64::MAX));
    let mut f = diamond();
    let context = f.context();
    let valid = validation::validate(&context).unwrap();
    let g = f.graph("master");
    let t = query(&g, "data/n0", Direction::Downstream, None);
    assert_eq!(t.page(None, 0), Err(Error::PageSize));
    assert_eq!(t.page(None, 101), Err(Error::PageSize));
    for cursor in ["bad", "tftr1:wrong:0:0", "tftr1:wrong:-1:0"] {
        assert_eq!(t.page(Some(cursor), 1), Err(Error::Cursor));
    }
    let cursor = t.page(None, 1).unwrap().next_cursor.unwrap();
    let prefix = cursor.rsplitn(3, ':').last().unwrap();
    for offsets in [
        "-1:0",
        "0:+1",
        "6:0",
        "0:7",
        "5:6",
        "18446744073709551616:0",
    ] {
        assert_eq!(
            t.page(Some(&format!("{prefix}:{offsets}")), 1),
            Err(Error::Cursor)
        );
    }
    assert_eq!(g.resolve("missing"), Err(Error::Missing));
    assert!(matches!(
        g.traverse(Request {
            start: Id::Pending("missing".parse().unwrap()),
            direction: Direction::Upstream,
            depth: None
        }),
        Err(Error::Missing)
    ));
    f.source_digest = "d".repeat(64);
    assert!(matches!(
        Graph::validated(&valid, &f.context(), "master".parse().unwrap()),
        Err(Error::Context)
    ));
    f.discovery["definitions"][0]["inputs"] = json!([input("cycle", "data/n4")]);
    assert!(validation::validate(&f.context()).is_err());
}
#[test]
fn stable_identity_ties_ignore_discovery_and_input_order() {
    let mut f = diamond();
    let first = all(
        &query(&f.graph("master"), "data/n0", Direction::Downstream, None),
        2,
    );
    f.discovery["definitions"].as_array_mut().unwrap().reverse();
    for d in f.discovery["definitions"].as_array_mut().unwrap() {
        d["inputs"].as_array_mut().unwrap().reverse();
    }
    assert_eq!(
        first,
        all(
            &query(&f.graph("master"), "data/n0", Direction::Downstream, None),
            3
        )
    );
}
#[test]
fn seeded_dags_match_topological_shortest_distance_reference() {
    for seed in 0..24usize {
        let mut pairs = vec![];
        let mut defs = vec![];
        for consumer in 0..12 {
            let parents = (0..consumer)
                .filter(|parent| (parent * 17 + consumer * 31 + seed * 13) % 7 < 2)
                .collect::<Vec<_>>();
            pairs.extend(parents.iter().map(|parent| (*parent, consumer)));
            defs.push(definition(
                consumer,
                parents
                    .iter()
                    .map(|parent| input(&format!("p{parent}"), &format!("data/n{parent}")))
                    .collect(),
            ));
        }
        let g = Fixture::new("", defs).graph("master");
        for direction in [Direction::Upstream, Direction::Downstream] {
            let start = if direction == Direction::Upstream {
                11
            } else {
                0
            };
            let mut expected = BTreeMap::from([(start, 0u64)]);
            let order: Vec<_> = if direction == Direction::Upstream {
                (0..12).rev().collect()
            } else {
                (0..12).collect()
            };
            for n in order {
                if let Some(depth) = expected.get(&n).copied() {
                    for &(parent, consumer) in &pairs {
                        let (from, to) = if direction == Direction::Upstream {
                            (consumer, parent)
                        } else {
                            (parent, consumer)
                        };
                        if from == n {
                            expected
                                .entry(to)
                                .and_modify(|d| *d = (*d).min(depth + 1))
                                .or_insert(depth + 1);
                        }
                    }
                }
            }
            for depth in [Some(0), Some(1), Some(3), None] {
                let (nodes, edges) =
                    all(&query(&g, &format!("data/n{start}"), direction, depth), 3);
                let expected: BTreeMap<_, _> = expected
                    .iter()
                    .filter(|(_, d)| depth.is_none_or(|limit| **d <= limit))
                    .map(|(n, d)| (g.resolve(&format!("data/n{n}")).unwrap(), *d))
                    .collect();
                assert_eq!(
                    nodes
                        .into_iter()
                        .map(|v| (v.node.identity, v.depth))
                        .collect::<BTreeMap<_, _>>(),
                    expected
                );
                assert_eq!(
                    edges.len(),
                    pairs
                        .iter()
                        .filter(|(p, c)| expected
                            .contains_key(&g.resolve(&format!("data/n{p}")).unwrap())
                            && expected.contains_key(&g.resolve(&format!("data/n{c}")).unwrap()))
                        .count()
                );
            }
        }
    }
}
