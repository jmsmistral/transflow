//! Native CLI external registrations, with no Python environment or Git side effects.
#![allow(clippy::unwrap_used, reason = "Synthetic workspace tests")]
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "tf-external-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let t = Self(path.canonicalize().unwrap());
        t.run(&["init"], true);
        t
    }
    fn run(&self, args: &[&str], ok: bool) -> Value {
        let o = Command::new(env!("CARGO_BIN_EXE_transflow"))
            .arg("--json")
            .arg("--workspace")
            .arg(&self.0)
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert_eq!(
            o.status.success(),
            ok,
            "{}",
            String::from_utf8_lossy(&o.stdout)
        );
        assert!(
            o.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        tf_protocol::validate_document("CliEnvelopeV1", &v).unwrap();
        v
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn provider() -> Tree {
    let p = Tree::new();
    fs::write(p.0.join(".transflow/catalog.toml"),"format_version=1\n[[datasets]]\nid='1837c4ad-54ed-4ea9-8301-cde739093271'\npath='raw/orders'\nkind='transform'\n").unwrap();
    fs::write(
        p.0.join("src/poison.py"),
        "raise RuntimeError('PROVIDER MUST NEVER IMPORT')\n",
    )
    .unwrap();
    p
}
fn add(c: &Tree, p: &Tree, name: &str, extra: &[&str], ok: bool) -> Value {
    let mut args = vec![
        "external",
        "add",
        "--workspace",
        p.0.to_str().unwrap(),
        "--dataset",
        "raw/orders",
        "--as",
        name,
    ];
    args.extend(extra);
    c.run(&args, ok)
}
#[test]
fn registration_before_environment_preserves_identity_policy_and_local_privacy() {
    let c = Tree::new();
    let p = provider();
    fs::write(
        c.0.join("src/poison.py"),
        "raise RuntimeError('CONSUMER ADD MUST NOT IMPORT')\n",
    )
    .unwrap();
    let config = fs::read_to_string(p.0.join("workspace.toml"))
        .unwrap()
        .replace(
            "default_data_branch = \"master\"",
            "default_data_branch = \"production\"",
        );
    fs::write(p.0.join("workspace.toml"), config).unwrap();
    fs::write(c.0.join("transflow.local.toml"),"[python]\nexecutable='/synthetic/python'\n[secret_references]\ntoken='env:FIXTURE_TOKEN'\n").unwrap();
    let first = add(&c, &p, "market.orders", &[], true);
    let e = &first["result"]["entries"][0];
    assert_eq!(e["reference"], "external/market/orders");
    assert_eq!(e["c_reference"], "C.external.market.orders");
    assert_eq!(e["default_branch"], "production");
    assert!(e["fallback_override"].is_null());
    assert_eq!(e["locator_status"], "available");
    let again = add(&c, &p, "market.orders", &[], true);
    assert_eq!(
        again["result"]["entries"][0]["registration_id"],
        e["registration_id"]
    );
    let strict = add(
        &c,
        &p,
        "market.strict",
        &["--branch", "feature", "--no-fallback"],
        true,
    );
    assert_eq!(
        strict["result"]["entries"][0]["fallback_override"],
        serde_json::json!([])
    );
    let policy = add(
        &c,
        &p,
        "market.alternate",
        &["--fallback", "stable", "--fallback", "master"],
        true,
    );
    assert_eq!(
        policy["result"]["entries"][0]["fallback_override"],
        serde_json::json!(["stable", "master"])
    );
    let registry = fs::read_to_string(c.0.join(".transflow/catalog.toml")).unwrap();
    assert!(!registry.contains(p.0.to_str().unwrap()));
    assert!(!registry.contains("FIXTURE_TOKEN"));
    let local = fs::read_to_string(c.0.join("transflow.local.toml")).unwrap();
    assert!(local.contains(p.0.to_str().unwrap()));
    assert!(local.contains("/synthetic/python"));
    assert!(local.contains("env:FIXTURE_TOKEN"));
    assert!(!p.0.join(".transflow/runtime/catalog.sqlite").exists());
    assert!(!c.0.join("requirements.lock").exists());
    let page = c.run(&["external", "list", "--limit", "1"], true);
    assert_eq!(page["result"]["total"], "3");
    let cursor = page["result"]["next_cursor"].as_str().unwrap();
    let next = c.run(&["external", "list", "--cursor", cursor], true);
    assert_eq!(next["result"]["entries"].as_array().unwrap().len(), 2);
    assert_ne!(
        next["result"]["entries"][0]["registration_id"],
        page["result"]["entries"][0]["registration_id"]
    );
    add(&c, &p, "market.more", &[], true);
    c.run(&["external", "list", "--cursor", cursor], false);
    let human = Command::new(env!("CARGO_BIN_EXE_transflow"))
        .args([
            "--workspace",
            c.0.to_str().unwrap(),
            "external",
            "show",
            "market.orders",
        ])
        .output()
        .unwrap();
    assert!(human.status.success());
    assert!(
        String::from_utf8(human.stdout)
            .unwrap()
            .contains("C.external.market.orders")
    );
}
#[test]
fn same_names_different_owners_never_retarget_aliases_and_removal_reserves_identity() {
    let c = Tree::new();
    let p = provider();
    let other = provider();
    let first = add(&c, &p, "one.orders", &[], true);
    let before = fs::read(c.0.join(".transflow/catalog.toml")).unwrap();
    let local = fs::read(c.0.join("transflow.local.toml")).unwrap();
    add(&c, &other, "one.orders", &[], false);
    add(&c, &other, "one.different", &[], false);
    add(&c, &p, "one.orders", &["--branch", "different"], false);
    assert_eq!(
        fs::read(c.0.join(".transflow/catalog.toml")).unwrap(),
        before
    );
    assert_eq!(fs::read(c.0.join("transflow.local.toml")).unwrap(), local);
    let second = add(&c, &other, "two.orders", &[], true);
    assert_ne!(
        first["result"]["entries"][0]["provider_workspace_id"],
        second["result"]["entries"][0]["provider_workspace_id"]
    );
    let before = fs::read(c.0.join(".transflow/catalog.toml")).unwrap();
    let preview = c.run(&["external", "remove", "one.orders"], true);
    assert_eq!(preview["result"]["applied"], false);
    assert_eq!(
        fs::read(c.0.join(".transflow/catalog.toml")).unwrap(),
        before
    );
    let local_before_removal = fs::read(c.0.join("transflow.local.toml")).unwrap();
    let removed = c.run(
        &["external", "remove", "external/one/orders", "--yes"],
        true,
    );
    assert_eq!(removed["result"]["entries"][0]["tombstone"], true);
    assert_eq!(
        removed["result"]["entries"][0]["registration_id"],
        first["result"]["entries"][0]["registration_id"]
    );
    add(&c, &p, "one.orders", &[], false);
    add(&c, &other, "one.new", &[], false);
    c.run(&["external", "show", "one.orders"], true);
    assert_eq!(
        fs::read(c.0.join("transflow.local.toml")).unwrap(),
        local_before_removal
    );
}
#[test]
fn offline_unconfigured_and_replaced_provider_are_distinct_and_no_search_occurs() {
    let c = Tree::new();
    let p = provider();
    let other = provider();
    add(&c, &p, "market.orders", &[], true);
    let local = fs::read_to_string(c.0.join("transflow.local.toml")).unwrap();
    fs::remove_file(c.0.join("transflow.local.toml")).unwrap();
    assert_eq!(
        c.run(&["external", "show", "market.orders"], true)["result"]["entries"][0]["locator_status"],
        "unconfigured"
    );
    fs::write(
        c.0.join("transflow.local.toml"),
        local.replace(p.0.to_str().unwrap(), "/synthetic/unavailable"),
    )
    .unwrap();
    assert_eq!(
        c.run(&["external", "show", "market.orders"], true)["result"]["entries"][0]["locator_status"],
        "unavailable"
    );
    fs::write(
        c.0.join("transflow.local.toml"),
        local.replace(p.0.to_str().unwrap(), other.0.to_str().unwrap()),
    )
    .unwrap();
    assert_eq!(
        c.run(&["external", "show", "market.orders"], true)["result"]["entries"][0]["locator_status"],
        "identity_mismatch"
    );
    add(&c, &p, "market.orders", &[], true);
    fs::write(p.0.join(".transflow/catalog.toml"), "format_version=1\n").unwrap();
    assert_eq!(
        c.run(&["external", "show", "market.orders"], true)["result"]["entries"][0]["locator_status"],
        "dataset_unavailable"
    );
    c.run(&["external", "remove", "market.orders", "--yes"], true);
}
#[test]
fn invalid_aliases_self_registration_missing_dataset_and_symlink_locators_fail() {
    let c = provider();
    let p = provider();
    add(&c, &c, "self.orders", &[], false);
    for name in [
        "market",
        "Market.orders",
        "market.class",
        "market/order",
        "market..orders",
    ] {
        add(&c, &p, name, &[], false);
    }
    c.run(
        &[
            "external",
            "add",
            "--workspace",
            p.0.to_str().unwrap(),
            "--dataset",
            "raw/typo",
            "--as",
            "market.orders",
        ],
        false,
    );
    std::os::unix::fs::symlink(p.0.join("workspace.toml"), c.0.join("transflow.local.toml"))
        .unwrap();
    add(&c, &p, "market.orders", &[], false);
    assert_eq!(
        fs::read_to_string(c.0.join(".transflow/catalog.toml")).unwrap(),
        fs::read_to_string(p.0.join(".transflow/catalog.toml")).unwrap()
    );
}

#[test]
fn provider_git_feature_does_not_override_configured_default_data_branch() {
    let c = Tree::new();
    let p = provider();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&p.0)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "--initial-branch=feature"]);
    git(&["add", "workspace.toml", ".transflow/catalog.toml"]);
    git(&[
        "-c",
        "user.name=Synthetic Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-m",
        "Synthetic provider",
    ]);
    let result = add(&c, &p, "market.orders", &[], true);
    assert_eq!(result["result"]["entries"][0]["default_branch"], "master");
}
