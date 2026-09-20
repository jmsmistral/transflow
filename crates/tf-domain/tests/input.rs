//! Local selector, strict fallback and per-alias boundary contracts.
#![allow(clippy::unwrap_used, reason = "Synthetic input policies")]
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{input::*, *};
fn name(s: &str) -> BranchName {
    s.parse().unwrap()
}
fn names(v: &[&str]) -> Vec<BranchName> {
    v.iter().map(|s| name(s)).collect()
}
fn policy() -> BranchPolicySnapshot {
    BranchPolicySnapshot::new(
        names(&["master"]),
        BTreeMap::from([
            (name("develop"), names(&["stable", "master"])),
            (name("master"), vec![]),
            (name("feature"), names(&["develop", "feature", "develop"])),
        ]),
    )
    .unwrap()
}
#[test]
fn all_six_selectors_keep_start_and_strict_permission_independent() {
    for selector in [
        BranchSelector::Omitted,
        BranchSelector::Current,
        BranchSelector::Named(name("develop")),
    ] {
        for permission in [FallbackPermission::Allowed, FallbackPermission::Prohibited] {
            let p = policy()
                .local(
                    &name("feature"),
                    selector.clone(),
                    permission,
                    Some(&names(&["override"])),
                )
                .unwrap();
            let expected = if matches!(selector, BranchSelector::Named(_)) {
                if permission == FallbackPermission::Allowed {
                    names(&["develop", "stable", "master"])
                } else {
                    names(&["develop"])
                }
            } else if permission == FallbackPermission::Allowed {
                names(&["feature", "override"])
            } else {
                names(&["feature"])
            };
            assert_eq!(p.candidates(), expected);
            assert_eq!(p.declared(), &selector);
            assert_eq!(p.permission(), permission);
        }
    }
}
#[test]
fn rules_are_nonrecursive_deduplicated_and_empty_is_not_default() {
    let p = policy()
        .local(
            &name("feature"),
            BranchSelector::Omitted,
            FallbackPermission::Allowed,
            None,
        )
        .unwrap();
    assert_eq!(p.candidates(), names(&["feature", "develop"]));
    assert_eq!(p.origin(), PolicyOrigin::StartingBranch);
    let p = policy()
        .local(
            &name("master"),
            BranchSelector::Current,
            FallbackPermission::Allowed,
            None,
        )
        .unwrap();
    assert_eq!(p.candidates(), names(&["master"]));
    let p = policy()
        .local(
            &name("new"),
            BranchSelector::Omitted,
            FallbackPermission::Allowed,
            None,
        )
        .unwrap();
    assert_eq!(p.candidates(), names(&["new", "master"]));
    assert_eq!(p.origin(), PolicyOrigin::WorkspaceDefault);
    let p = policy()
        .local(
            &name("feature"),
            BranchSelector::Current,
            FallbackPermission::Allowed,
            Some(&[]),
        )
        .unwrap();
    assert_eq!(p.candidates(), names(&["feature"]));
    assert_eq!(p.origin(), PolicyOrigin::BuildOverride);
    // A named selector equal to output still uses the named branch's own policy.
    let p = policy()
        .local(
            &name("develop"),
            BranchSelector::Named(name("develop")),
            FallbackPermission::Allowed,
            Some(&[]),
        )
        .unwrap();
    assert_eq!(p.candidates(), names(&["develop", "stable", "master"]));
}
#[test]
fn per_alias_write_eligibility_never_rebuilds_an_independent_branch_or_provider() {
    let w = WorkspaceId::from_bytes([1; 16]);
    let d = DatasetKey::new(w, DatasetId::from_bytes([2; 16]));
    let key = InputBindingKey::new(
        DatasetKey::new(w, DatasetId::from_bytes([3; 16])),
        "left".into(),
    )
    .unwrap();
    let make = |selector| InputBinding {
        key: key.clone(),
        dataset: d,
        role: InputRole::Data,
        policy: policy()
            .local(
                &name("feature"),
                selector,
                FallbackPermission::Allowed,
                None,
            )
            .unwrap(),
    };
    for selector in [
        BranchSelector::Omitted,
        BranchSelector::Current,
        BranchSelector::Named(name("feature")),
    ] {
        let b = make(selector);
        assert_eq!(
            b.disposition(&BTreeSet::from([d])),
            BindingDisposition::PlannedProducer
        );
        assert_eq!(
            b.disposition(&BTreeSet::new()),
            BindingDisposition::ReadBoundary
        );
    }
    let mut b = make(BranchSelector::Named(name("master")));
    assert_eq!(
        b.disposition(&BTreeSet::from([d])),
        BindingDisposition::OffBranch
    );
    b.dataset = DatasetKey::new(WorkspaceId::from_bytes([4; 16]), d.dataset_id());
    assert_eq!(
        b.disposition(&BTreeSet::from([b.dataset])),
        BindingDisposition::ForeignWorkspace
    );
    let other = InputBindingKey::new(key.consumer(), "right".into()).unwrap();
    assert_ne!(key, other);
}
#[test]
fn excessive_policies_and_invalid_aliases_fail_explicitly() {
    assert!(BranchPolicySnapshot::new(vec![name("x"); 1025], BTreeMap::new()).is_err());
    let consumer = DatasetKey::new(
        WorkspaceId::from_bytes([1; 16]),
        DatasetId::from_bytes([2; 16]),
    );
    for alias in ["", "a\nb"] {
        assert!(InputBindingKey::new(consumer, alias.into()).is_err());
    }
}
