//! Binding qualification keeps aliases and effective selector conflicts visible.
#![allow(clippy::unwrap_used, reason = "Synthetic pin fixtures")]
use std::collections::{BTreeMap, BTreeSet};
use tf_catalog::RegistrySnapshot;
use tf_domain::{input::*, *};
use tf_plan::pins::*;
fn key(n: u8) -> DatasetKey {
    DatasetKey::new(
        WorkspaceId::from_bytes([1; 16]),
        DatasetId::from_bytes([n; 16]),
    )
}
fn version(n: u8) -> VersionId {
    VersionId::from_bytes([n; 16])
}
fn registry() -> RegistrySnapshot {
    RegistrySnapshot::parse(key(1).workspace_id(),&format!("format_version=1\n[[datasets]]\nid='{}'\npath='raw/orders'\nkind='source'\n[[datasets]]\nid='{}'\npath='analytics/revenue'\nkind='transform'\n",key(1).dataset_id(),key(2).dataset_id())).unwrap()
}
fn binding(alias: &str, branch: &str, strict: bool) -> InputBinding {
    InputBinding {
        key: InputBindingKey::new(key(2), alias.into()).unwrap(),
        dataset: key(1),
        role: InputRole::Data,
        policy: BranchPolicySnapshot::new(vec!["master".parse().unwrap()], BTreeMap::new())
            .unwrap()
            .local(
                &"feature".parse().unwrap(),
                BranchSelector::Named(branch.parse().unwrap()),
                if strict {
                    FallbackPermission::Prohibited
                } else {
                    FallbackPermission::Allowed
                },
                None,
            )
            .unwrap(),
    }
}
fn bindings(values: Vec<InputBinding>) -> BTreeMap<InputBindingKey, InputBinding> {
    values.into_iter().map(|b| (b.key.clone(), b)).collect()
}
fn pin(reference: &str, n: u8) -> PinRequest {
    format!("{reference}={}", version(n)).parse().unwrap()
}
#[test]
fn syntax_exact_refs_and_qualified_aliases() {
    for bad in [
        "",
        "x",
        "x=1",
        "=00000000-0000-0000-0000-000000000000",
        "x#=00000000-0000-0000-0000-000000000000",
        "x#a#b=00000000-0000-0000-0000-000000000000",
    ] {
        assert!(bad.parse::<PinRequest>().is_err());
    }
    let b = bindings(vec![binding("orders", "master", false)]);
    for reference in [
        "raw/orders".into(),
        format!("dataset:{}", key(1).dataset_id()),
        "analytics/revenue#orders".into(),
    ] {
        let result = qualify(&[pin(&reference, 3)], &registry(), &b, &BTreeSet::new()).unwrap();
        assert_eq!(
            result.values().copied().collect::<Vec<_>>(),
            vec![version(3)]
        );
    }
    assert_eq!(
        qualify(&[pin("orders", 3)], &registry(), &b, &BTreeSet::new()),
        Err(PinError::Invalid)
    );
    assert_eq!(
        qualify(
            &[pin("analytics/revenue#missing", 3)],
            &registry(),
            &b,
            &BTreeSet::new()
        ),
        Err(PinError::Invalid)
    );
}
#[test]
fn ambiguity_lists_exact_alternatives_and_keeps_aliases_independent() {
    let b = bindings(vec![
        binding("live", "master", false),
        binding("prior", "release", true),
    ]);
    let error = qualify(&[pin("raw/orders", 3)], &registry(), &b, &BTreeSet::new()).unwrap_err();
    assert_eq!(
        error,
        PinError::Ambiguous(vec![
            format!("analytics/revenue#live={}", version(3)),
            format!("analytics/revenue#prior={}", version(3))
        ])
    );
    let result = qualify(
        &[
            pin("analytics/revenue#live", 3),
            pin("analytics/revenue#prior", 4),
        ],
        &registry(),
        &b,
        &BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(
        result[&InputBindingKey::new(key(2), "prior".into()).unwrap()],
        version(4)
    );
}
#[test]
fn equal_effective_selectors_expand_shorthand_and_repeated_pins_must_agree() {
    let b = bindings(vec![
        binding("a", "master", false),
        binding("b", "master", false),
    ]);
    assert_eq!(
        qualify(
            &[pin("raw/orders", 3), pin("analytics/revenue#a", 3)],
            &registry(),
            &b,
            &BTreeSet::new()
        )
        .unwrap()
        .len(),
        2
    );
    assert_eq!(
        qualify(
            &[pin("raw/orders", 3), pin("analytics/revenue#a", 4)],
            &registry(),
            &b,
            &BTreeSet::new()
        ),
        Err(PinError::Conflict)
    );
}
#[test]
fn starting_branch_producer_conflicts_but_distinct_branch_is_a_boundary() {
    let b = bindings(vec![binding("a", "feature", false)]);
    assert_eq!(
        qualify(
            &[pin("raw/orders", 3)],
            &registry(),
            &b,
            &BTreeSet::from([key(1)])
        ),
        Err(PinError::WriteConflict)
    );
    let b = bindings(vec![binding("a", "master", true)]);
    assert!(
        qualify(
            &[pin("raw/orders", 3)],
            &registry(),
            &b,
            &BTreeSet::from([key(1)])
        )
        .is_ok()
    );
}
