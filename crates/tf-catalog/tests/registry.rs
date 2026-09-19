//! Exact registry identity, collision, policy and nonmutating lookup properties.
#![allow(
    clippy::unwrap_used,
    reason = "Synthetic test fixtures must fail loudly"
)]
use tf_catalog::*;
use tf_domain::{DatasetId, WorkspaceId};
fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
fn id(n: u128) -> String {
    DatasetId::from_bytes(n.to_be_bytes()).to_string()
}
fn dataset(n: u128, path: &str) -> String {
    format!(
        "[[datasets]]\nid = '{}'\npath = '{path}'\nkind = 'transform'\n",
        id(n)
    )
}
fn parse(records: &str) -> RegistrySnapshot {
    RegistrySnapshot::parse(workspace(), &format!("format_version=1\n{records}")).unwrap()
}
fn fails(records: &str, kind: RegistryErrorKind) {
    assert_eq!(
        RegistrySnapshot::parse(workspace(), &format!("format_version=1\n{records}"))
            .unwrap_err()
            .kind(),
        kind
    );
}
fn external(n: u128, alias: &str) -> String {
    format!(
        "[[external_registrations]]\nid = '{}'\nalias = '{alias}'\nprovider_workspace_id = '{}'\nprovider_dataset_id = '{}'\nprovider_display_path = 'raw/orders'\ndefault_branch = 'master'\n",
        id(n),
        WorkspaceId::from_bytes([2; 16]),
        id(7)
    )
}
#[test]
fn exact_paths_ids_aliases_and_dataset_namespace_prefixes() {
    let records = format!(
        "{}{}[[aliases]]\npath='old/orders'\ntarget_id='{}'\n",
        dataset(1, "raw/orders"),
        dataset(2, "raw/orders/daily"),
        id(1)
    );
    let s = parse(&records);
    let key = s.resolve("raw/orders").unwrap().key();
    assert_eq!(s.resolve(&format!("dataset:{}", id(1))).unwrap().key(), key);
    assert_eq!(s.resolve("old/orders").unwrap().key(), key);
    assert!(matches!(
        s.resolve("old/orders"),
        Ok(Resolved::Local {
            via_alias: true,
            ..
        })
    ));
    assert_eq!(s.resolve_output("raw/orders").unwrap().key(), key);
    assert_eq!(
        s.resolve("raw").unwrap_err().kind(),
        RegistryErrorKind::Namespace
    );
    for missing in ["raw/order", "other/raw/orders", "raw/orderss"] {
        assert_eq!(
            s.resolve(missing).unwrap_err().kind(),
            RegistryErrorKind::Missing
        );
    }
    assert_eq!(
        s.resolve("project:raw/orders").unwrap_err().kind(),
        RegistryErrorKind::Invalid
    );
    assert_ne!(s.resolve("raw/orders/daily").unwrap().key(), key);
}
#[test]
fn collisions_identify_both_records_without_echoing_content() {
    let input = format!("{}{}", dataset(1, "raw/one"), dataset(1, "raw/two"));
    let e =
        RegistrySnapshot::parse(workspace(), &format!("format_version=1\n{input}")).unwrap_err();
    assert_eq!(e.kind(), RegistryErrorKind::Collision);
    assert_eq!(e.location(), "/datasets/1/id");
    assert_eq!(e.related_location(), Some("/datasets/0/id"));
    assert!(!format!("{e:?}").contains("raw/one"));
    fails(
        &format!("{}{}", dataset(1, "raw/orders"), dataset(2, "raw/orders")),
        RegistryErrorKind::Collision,
    );
}
#[test]
fn unknown_code_fields_bad_types_and_versions_are_closed() {
    for suffix in [
        "engine='polars'",
        "inputs=[]",
        "checks=[]",
        "producer='private_secret'",
        "locator='/private/path'",
    ] {
        let e = RegistrySnapshot::parse(
            workspace(),
            &format!("format_version=1\n{}{suffix}", dataset(1, "raw/orders")),
        )
        .unwrap_err();
        assert_eq!(e.kind(), RegistryErrorKind::Syntax);
        assert!(!format!("{e:?}: {e}").contains("private_secret"));
    }
    assert_eq!(
        RegistrySnapshot::parse(workspace(), "format_version=2")
            .unwrap_err()
            .kind(),
        RegistryErrorKind::Version
    );
    for source in [
        "",
        "format_version='1'",
        "format_version=1\nformat_version=1",
        "format_version=1\nsecret=[",
    ] {
        assert_eq!(
            RegistrySnapshot::parse(workspace(), source)
                .unwrap_err()
                .kind(),
            RegistryErrorKind::Syntax
        );
    }
    assert!(
        RegistrySnapshot::parse(workspace(), "format_version=1\nsecret=[")
            .unwrap_err()
            .span()
            .is_some()
    );
    fails(
        &dataset(1, "external/provider/orders"),
        RegistryErrorKind::Invalid,
    );
    fails(&dataset(1, "raw/class"), RegistryErrorKind::Invalid);
    fails(
        &dataset(1, "raw/orders").replace(&id(1), "not-a-uuid"),
        RegistryErrorKind::Invalid,
    );
}
#[test]
fn aliases_target_only_active_ids_and_cannot_shadow_canonical_names() {
    let d = dataset(1, "raw/orders");
    fails(
        &format!("{d}[[aliases]]\npath='raw/orders'\ntarget_id='{}'", id(1)),
        RegistryErrorKind::Collision,
    );
    fails(
        &format!("{d}[[aliases]]\npath='old/orders'\ntarget_id='{}'", id(2)),
        RegistryErrorKind::AliasTarget,
    );
    fails(
        &format!("{d}[[aliases]]\npath='old/orders'\ntarget_id='old/alias'"),
        RegistryErrorKind::Invalid,
    );
    fails(
        &format!(
            "{d}[[aliases]]\npath='external/provider/orders'\ntarget_id='{}'",
            id(1)
        ),
        RegistryErrorKind::Invalid,
    );
    let alias = format!("[[aliases]]\npath='old/orders'\ntarget_id='{}'\n", id(1));
    fails(&format!("{d}{alias}{alias}"), RegistryErrorKind::Collision);
}
#[test]
fn tombstones_reserve_names_and_ids_without_becoming_datasets() {
    let tombstone = dataset(1, "old/orders").replace("[[datasets]]", "[[tombstones]]");
    let s = parse(&tombstone);
    assert!(s.datasets().next().unwrap().is_tombstone());
    for reference in ["old/orders".to_owned(), format!("dataset:{}", id(1))] {
        assert_eq!(
            s.resolve(&reference).unwrap_err().kind(),
            RegistryErrorKind::Tombstone
        );
    }
    fails(
        &format!("{tombstone}{}", dataset(2, "old/orders")),
        RegistryErrorKind::Collision,
    );
    fails(
        &format!("{tombstone}{}", dataset(1, "new/orders")),
        RegistryErrorKind::Collision,
    );
    fails(
        &format!(
            "{tombstone}[[aliases]]\npath='new/orders'\ntarget_id='{}'",
            id(1)
        ),
        RegistryErrorKind::AliasTarget,
    );
}
#[test]
#[allow(
    clippy::panic,
    reason = "Unexpected fixture variant must fail the test"
)]
fn foreign_registrations_preserve_policy_and_remain_read_boundaries() {
    let one = external(1, "external/market/orders");
    let two = external(2, "external/market/strict_orders") + "fallback_override=[]\n";
    let s = parse(&format!("{one}{two}"));
    let a = match s.resolve("external/market/orders").unwrap() {
        Resolved::External(e) => e,
        _ => panic!("Expected foreign boundary"),
    };
    let b = match s.resolve("external/market/strict_orders").unwrap() {
        Resolved::External(e) => e,
        _ => panic!("Expected foreign boundary"),
    };
    assert_eq!(a.key(), b.key());
    assert_ne!(a.id(), b.id());
    assert!(a.fallback_override().is_none());
    assert_eq!(b.fallback_override(), Some([].as_slice()));
    assert_eq!(a.default_branch().as_str(), "master");
    assert_eq!(s.external_registrations().count(), 2);
    assert_eq!(
        s.resolve_output("external/market/orders")
            .unwrap_err()
            .kind(),
        RegistryErrorKind::ForeignOutput
    );
    assert_eq!(
        s.resolve(&format!("dataset:{}", id(7))).unwrap_err().kind(),
        RegistryErrorKind::Missing
    );
    fails(&format!("{one}{one}"), RegistryErrorKind::Collision);
    fails(&external(1, "raw/orders"), RegistryErrorKind::Invalid);
    fails(&external(1, "external/orders"), RegistryErrorKind::Invalid);
    fails(
        &one.replace(
            &WorkspaceId::from_bytes([2; 16]).to_string(),
            &workspace().to_string(),
        ),
        RegistryErrorKind::Invalid,
    );
    fails(
        &(one + "locator='/machine/path'"),
        RegistryErrorKind::Syntax,
    );
}
#[test]
fn semantic_fingerprint_ignores_format_and_record_order_but_covers_policy() {
    let a = dataset(1, "raw/one");
    let b = dataset(2, "raw/two");
    let first = parse(&format!("{a}{b}"));
    let second = parse(&format!("# presentation\n{b}{a}\n"));
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert_ne!(first.raw_digest(), second.raw_digest());
    let foreign = external(1, "external/market/orders");
    let inherited = parse(&foreign);
    let empty = parse(&(foreign.clone() + "fallback_override=[]"));
    assert_ne!(inherited.fingerprint(), empty.fingerprint());
    let one = parse(&(foreign.clone() + "fallback_override=['master','release']"));
    let two = parse(&(foreign + "fallback_override=['release','master']"));
    assert_ne!(one.fingerprint(), two.fingerprint());
    let alias = parse(&format!(
        "{a}{b}[[aliases]]\npath='old/one'\ntarget_id='{}'",
        id(1)
    ));
    assert_ne!(first.fingerprint(), alias.fingerprint());
    let other = RegistrySnapshot::parse(
        WorkspaceId::from_bytes([3; 16]),
        &format!("format_version=1\n{a}{b}"),
    )
    .unwrap();
    assert_ne!(first.fingerprint(), other.fingerprint());
}
#[test]
fn rename_and_runtime_deletion_never_allocate_replacement_identity() {
    let original = parse(&dataset(1, "old/orders"));
    let renamed = parse(&format!(
        "{}[[aliases]]\npath='old/orders'\ntarget_id='{}'",
        dataset(1, "new/orders"),
        id(1)
    ));
    let before = original.resolve("old/orders").unwrap().key();
    assert_eq!(renamed.resolve("new/orders").unwrap().key(), before);
    assert_eq!(renamed.resolve("old/orders").unwrap().key(), before);
    // Parser consumes durable bytes only: no runtime directory or DB is consulted.
    let root = std::env::temp_dir().join(format!("tf-registry-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let registry = root.join("catalog.toml");
    let text = format!("format_version=1\n{}", dataset(1, "old/orders"));
    std::fs::write(&registry, &text).unwrap();
    let runtime = root.join("runtime.sqlite");
    std::fs::write(&runtime, b"synthetic disposable runtime").unwrap();
    std::fs::remove_file(runtime).unwrap();
    let moved = root.join("moved.toml");
    std::fs::rename(registry, &moved).unwrap();
    let reparsed =
        RegistrySnapshot::parse(workspace(), &std::fs::read_to_string(moved).unwrap()).unwrap();
    assert_eq!(reparsed.resolve("old/orders").unwrap().key(), before);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn bounded_inputs_and_deep_paths_fail_before_lookup() {
    assert_eq!(
        RegistrySnapshot::parse(workspace(), &" ".repeat(MAX_REGISTRY_BYTES + 1))
            .unwrap_err()
            .kind(),
        RegistryErrorKind::Limit
    );
    fails(
        &dataset(1, &vec!["a"; 65].join("/")),
        RegistryErrorKind::Limit,
    );
    fails(&dataset(1, &"a".repeat(4097)), RegistryErrorKind::Limit);
    let s = parse("");
    assert_eq!(
        s.resolve(&"a".repeat(4097)).unwrap_err().kind(),
        RegistryErrorKind::Limit
    );
}
#[test]
fn thousands_of_exact_lookups_are_nonmutating_and_order_independent() {
    let records: Vec<_> = (1..=5000)
        .map(|i| dataset(i, &format!("synthetic/item_{i}")))
        .collect();
    let forward = parse(&records.join(""));
    let reverse = parse(&records.iter().rev().cloned().collect::<String>());
    assert_eq!(forward.fingerprint(), reverse.fingerprint());
    let before = *forward.fingerprint();
    for i in 1..=5000 {
        assert_eq!(
            forward
                .resolve(&format!("synthetic/item_{i}"))
                .unwrap()
                .key()
                .dataset_id()
                .to_string(),
            id(i)
        );
    }
    for i in 5001..=5100 {
        assert!(forward.resolve(&format!("synthetic/item_{i}")).is_err());
    }
    assert_eq!(forward.datasets().count(), 5000);
    assert_eq!(forward.fingerprint(), &before);
}
