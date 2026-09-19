//! Candidate reconciliation is pure; only an explicit proposal callback allocates IDs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic assertions"
)]
use serde_json::{Value, json};
use tf_catalog::{
    RegistrySnapshot,
    candidate::{CandidateCatalog, CandidateIdentity},
};
use tf_domain::{DatasetId, SourceSnapshotId, WorkspaceId};
fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
fn source() -> SourceSnapshotId {
    SourceSnapshotId::from_bytes([2; 16])
}
fn id(n: u8) -> DatasetId {
    DatasetId::from_bytes([n; 16])
}
fn registry(text: &str) -> RegistrySnapshot {
    RegistrySnapshot::parse(workspace(), &format!("format_version=1\n{text}")).unwrap()
}
fn input(alias: &str, path: &str) -> Value {
    json!({"alias":alias,"ref":{"form":"string","value":path},"branch":{"kind":"omitted","name":null},"stop_branch_fallback":false,"role":"data","checks":[]})
}
fn definition(module: &str, path: &str, inputs: Vec<Value>) -> Value {
    json!({"module":module,"function":"produce","path":format!("src/{module}.py"),"line":3,"source":false,"engine":"polars","inputs":inputs,"output":{"ref":{"form":"string","value":path},"checks":[],"schema":null},"parameters":[],"wall_timeout_seconds":null,"cache":"deterministic","refresh":null,"secret_refs":[],"lineage_json":null})
}
fn result(r: &RegistrySnapshot, defs: Vec<Value>) -> Value {
    json!({"format_version":1,"source_snapshot_id":source().to_string(),"catalog_fingerprint":r.sdk_projection(source()).unwrap()["catalog_fingerprint"],"environment_fingerprint":"a".repeat(64),"definitions":defs,"imported_modules":[]})
}
#[test]
fn fresh_forward_graph_is_order_independent_and_validation_allocates_nothing() {
    let r = registry("");
    let original = *r.raw_digest();
    let mut defs = vec![
        definition(
            "consumer",
            "curated/items",
            vec![input("items", "raw/items")],
        ),
        definition("producer", "raw/items", vec![]),
    ];
    let first = CandidateCatalog::prepare(&r, source(), &result(&r, defs.clone())).unwrap();
    defs.reverse();
    let second = CandidateCatalog::prepare(&r, source(), &result(&r, defs)).unwrap();
    assert_eq!(first.pending(), second.pending());
    assert_eq!(first.topological_order(), second.topological_order());
    assert_eq!(first.pending().len(), 2);
    assert_eq!(r.raw_digest(), &original);
    assert_eq!(r.datasets().count(), 0);
    assert_eq!(
        first.topological_order()[0],
        CandidateIdentity::Pending("raw/items".parse().unwrap())
    );
    let mut count = 0;
    let draft = first
        .propose(|| {
            count += 1;
            Ok(id(count))
        })
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(draft.assignments().len(), 2);
    let parsed = RegistrySnapshot::parse(workspace(), draft.replacement()).unwrap();
    for (path, identity, _) in draft.assignments() {
        assert_eq!(
            parsed.resolve(path.as_str()).unwrap().key().dataset_id(),
            *identity
        );
    }
    assert_eq!(draft.clone().replacement(), draft.replacement());
}
#[test]
fn unknown_inputs_and_unregistered_ids_do_not_create_placeholders() {
    let r = registry("");
    for missing in [
        "raw/typo",
        &format!("dataset:{}", id(9)),
        "external/provider/missing",
    ] {
        let e = CandidateCatalog::prepare(
            &r,
            source(),
            &result(
                &r,
                vec![definition("consumer", "out", vec![input("items", missing)])],
            ),
        )
        .unwrap_err();
        assert!(e.message.contains("unresolved"));
        assert_eq!(e.locations.len(), 1);
    }
    let d = definition("producer", &format!("dataset:{}", id(9)), vec![]);
    assert!(CandidateCatalog::prepare(&r, source(), &result(&r, vec![d])).is_err());
}
#[test]
fn duplicate_outputs_modules_and_aliases_fail_before_proposals() {
    let r = registry("");
    for defs in [
        vec![
            definition("one", "out", vec![]),
            definition("two", "out", vec![]),
        ],
        vec![
            definition("one", "a", vec![]),
            definition("one", "b", vec![]),
        ],
    ] {
        let e = CandidateCatalog::prepare(&r, source(), &result(&r, defs)).unwrap_err();
        assert_eq!(e.locations.len(), 2);
    }
    let defs = vec![
        definition("one", "a", vec![]),
        definition("two", "b", vec![input("same", "a"), input("same", "a")]),
    ];
    assert!(
        CandidateCatalog::prepare(&r, source(), &result(&r, defs))
            .unwrap_err()
            .message
            .contains("aliases")
    );
}
#[test]
fn whole_graph_cycles_include_validation_and_off_branch_edges() {
    let r = registry("");
    let mut dependency = input("reference", "b");
    dependency["role"] = "validation".into();
    dependency["branch"] = json!({"kind":"named","name":"historical"});
    let defs = vec![
        definition("unrelated", "target", vec![]),
        definition("one", "a", vec![dependency]),
        definition("two", "b", vec![input("value", "a")]),
    ];
    let e = CandidateCatalog::prepare(&r, source(), &result(&r, defs)).unwrap_err();
    assert_eq!(e.cycle.len(), 3);
    assert_eq!(e.cycle.first(), e.cycle.last());
    assert_eq!(e.locations.len(), 2);
}
#[test]
fn repeated_bindings_keep_aliases_branch_and_stop_policy() {
    let r = registry("");
    let mut historical = input("old", "a");
    historical["branch"] = json!({"kind":"named","name":"master"});
    historical["stop_branch_fallback"] = true.into();
    let defs = vec![
        definition("one", "a", vec![]),
        definition("two", "b", vec![input("new", "a"), historical.clone()]),
    ];
    let c = CandidateCatalog::prepare(&r, source(), &result(&r, defs)).unwrap();
    let inputs = &c.definitions()[1].inputs;
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0].identity, inputs[1].identity);
    assert_ne!(inputs[0].alias, inputs[1].alias);
    assert_eq!(inputs[1].declaration, historical);
}
fn retained() -> RegistrySnapshot {
    registry(&format!(
        "[[datasets]]\nid='{}'\npath='raw/items'\nkind='transform'\n[[aliases]]\npath='old/items'\ntarget_id='{}'\n[[tombstones]]\nid='{}'\npath='deleted/items'\nkind='source'\n[[external_registrations]]\nid='{}'\nalias='external/vendor/items'\nprovider_workspace_id='{}'\nprovider_dataset_id='{}'\nprovider_display_path='raw/items'\ndefault_branch='feature/δ'\nfallback_override=['master','develop']\n",
        id(3),
        id(3),
        id(4),
        id(5),
        WorkspaceId::from_bytes([9; 16]),
        id(6)
    ))
}
#[test]
fn registry_aliases_tombstones_and_foreign_policies_survive_additive_rendering() {
    let r = retained();
    let defs = vec![
        definition("existing", "old/items", vec![]),
        definition(
            "new",
            "out",
            vec![input("foreign", "external/vendor/items")],
        ),
    ];
    let candidate = CandidateCatalog::prepare(&r, source(), &result(&r, defs)).unwrap();
    assert_eq!(candidate.pending().len(), 1);
    let proposal = candidate.propose(|| Ok(id(7))).unwrap();
    let after = RegistrySnapshot::parse(workspace(), proposal.replacement()).unwrap();
    assert_eq!(
        after.resolve("old/items").unwrap().key(),
        r.resolve("raw/items").unwrap().key()
    );
    assert!(after.resolve("deleted/items").is_err());
    let external = after.external_registrations().next().unwrap();
    assert_eq!(external.default_branch().as_str(), "feature/δ");
    assert_eq!(external.fallback_override().unwrap().len(), 2);
    assert_eq!(proposal.expected_old(), r.raw_digest());
    assert!(candidate.propose(|| Ok(id(3))).is_err());
    for path in ["deleted/items", "external/vendor/items"] {
        assert!(
            CandidateCatalog::prepare(
                &r,
                source(),
                &result(&r, vec![definition("invalid", path, vec![])])
            )
            .is_err()
        );
    }
    let mut kind_change = definition("change", "raw/items", vec![]);
    kind_change["source"] = true.into();
    assert!(CandidateCatalog::prepare(&r, source(), &result(&r, vec![kind_change])).is_err());
}
#[test]
fn stale_bound_references_and_source_snapshots_fail() {
    let r = retained();
    let mut d = definition("bound", "out", vec![input("data", "raw/items")]);
    d["inputs"][0]["ref"] = json!({"form":"bound","workspace_id":workspace().to_string(),"dataset_id":id(3).to_string(),"path":"raw/items","catalog_fingerprint":r.sdk_projection(source()).unwrap()["catalog_fingerprint"]});
    let good = result(&r, vec![d]);
    assert!(CandidateCatalog::prepare(&r, source(), &good).is_ok());
    for field in ["workspace_id", "dataset_id", "catalog_fingerprint"] {
        let mut bad = good.clone();
        bad["definitions"][0]["inputs"][0]["ref"][field] = if field == "catalog_fingerprint" {
            "f".repeat(64).into()
        } else {
            id(8).to_string().into()
        };
        assert!(CandidateCatalog::prepare(&r, source(), &bad).is_err());
    }
    assert!(CandidateCatalog::prepare(&r, SourceSnapshotId::from_bytes([8; 16]), &good).is_err());
}
#[test]
fn helper_only_and_dataset_namespace_prefixes_are_valid() {
    let r = retained();
    let empty = CandidateCatalog::prepare(&r, source(), &result(&r, vec![])).unwrap();
    assert!(empty.definitions().is_empty());
    let candidate = CandidateCatalog::prepare(
        &r,
        source(),
        &result(&r, vec![definition("prefix", "raw", vec![])]),
    )
    .unwrap();
    assert_eq!(candidate.pending().len(), 1);
    let draft = candidate.propose(|| Ok(id(7))).unwrap();
    assert!(
        RegistrySnapshot::parse(workspace(), draft.replacement())
            .unwrap()
            .resolve("raw/items")
            .is_ok()
    );
}

#[test]
fn sdk_projection_retains_all_aliases_and_bound_aliases_resolve_like_strings() {
    let original = retained();
    let text = original.render_additions(&[]).unwrap();
    let text = format!(
        "{text}\n[[external_registrations]]\nid='{}'\nalias='external/alternate/items'\nprovider_workspace_id='{}'\nprovider_dataset_id='{}'\nprovider_display_path='raw/items'\ndefault_branch='other'\n",
        id(8),
        WorkspaceId::from_bytes([9; 16]),
        id(6)
    );
    let registry = RegistrySnapshot::parse(workspace(), &text).unwrap();
    let projection = registry.sdk_projection(source()).unwrap();
    assert_eq!(projection["entries"].as_array().unwrap().len(), 2);
    assert_eq!(projection["aliases"].as_array().unwrap().len(), 2);
    assert_eq!(projection["aliases"][0]["path"], "external/vendor/items");
    assert_eq!(projection["aliases"][1]["path"], "old/items");
    let mut bound = input("items", "old/items");
    bound["ref"] = json!({"form":"bound","workspace_id":workspace().to_string(),"dataset_id":id(3).to_string(),"path":"old/items","catalog_fingerprint":projection["catalog_fingerprint"]});
    let c = CandidateCatalog::prepare(
        &registry,
        source(),
        &result(
            &registry,
            vec![definition("consumer", "new/items", vec![bound])],
        ),
    )
    .unwrap();
    assert_eq!(
        c.definitions()[0].inputs[0].identity,
        CandidateIdentity::Registered(registry.resolve("raw/items").unwrap().key())
    );
    let mut edited = projection.clone();
    edited["aliases"][1]["path"] = json!("older/items");
    assert_ne!(
        tf_protocol::canonical::catalog_fingerprint(&edited)
            .unwrap()
            .hex(),
        projection["catalog_fingerprint"]
    );
}
