//! Native CLI branch operations, with no Python environment or Git side effects.
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
            "tf-branch-cli-{}-{}",
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
#[test]
fn list_preview_create_rename_delete_and_tombstone_are_explicit() {
    let t = Tree::new();
    let config = fs::read(t.0.join("workspace.toml")).unwrap();
    let empty = t.run(&["branch", "list"], true);
    assert_eq!(empty["result"]["entries"], serde_json::json!([]));
    assert!(!t.0.join(".transflow/runtime/catalog.sqlite").exists());
    let preview = t.run(&["branch", "create", "feature/Δ", "--dry-run"], true);
    assert_eq!(preview["result"]["requested_name"], "feature/Δ");
    assert!(!t.0.join(".transflow/runtime/catalog.sqlite").exists());
    let created = t.run(&["branch", "create", "feature/Δ"], true);
    let id = created["result"]["entries"][0]["id"].clone();
    let again = t.run(&["branch", "create", "feature/Δ"], true);
    assert_eq!(again["result"]["entries"][0]["id"], id);
    let renamed = t.run(&["branch", "rename", "feature/Δ", "new/name"], true);
    assert_eq!(renamed["result"]["entries"][0]["id"], id);
    let preview = t.run(&["branch", "delete", "new/name"], true);
    assert_eq!(preview["result"]["confirmation_required"], true);
    assert_eq!(preview["result"]["applied"], false);
    let deleted = t.run(&["branch", "delete", "new/name", "--yes"], true);
    assert_eq!(deleted["result"]["entries"][0]["deleted"], true);
    t.run(&["branch", "create", "new/name"], false);
    t.run(&["branch", "create", "new/name", "--dry-run"], false);
    assert_eq!(fs::read(t.0.join("workspace.toml")).unwrap(), config);
    assert!(!t.0.join(".git").exists());
}
#[test]
fn collisions_authored_policies_and_page_continuation_are_visible() {
    let t = Tree::new();
    t.run(&["branch", "create", "master"], true);
    t.run(&["branch", "create", "feature"], true);
    let blocked = t.run(&["branch", "delete", "master", "--yes"], false);
    assert!(blocked["result"]["impacts"].as_array().unwrap().len() >= 2);
    t.run(
        &["branch", "rename", "feature", "master", "--dry-run"],
        false,
    );
    let page = t.run(&["branch", "list", "--limit", "1"], true);
    let cursor = page["result"]["next_cursor"].as_str().unwrap();
    let next = t.run(
        &["branch", "list", "--limit", "1", "--cursor", cursor],
        true,
    );
    assert!(next["result"]["next_cursor"].is_null());
    assert_ne!(
        page["result"]["entries"][0]["id"],
        next["result"]["entries"][0]["id"]
    );
}
#[test]
fn git_branch_changes_never_rename_or_delete_data_branches() {
    let t = Tree::new();
    t.run(&["branch", "create", "old_git_name"], true);
    assert!(
        Command::new("git")
            .args(["init", "-q", "-b", "old_git_name"])
            .current_dir(&t.0)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["symbolic-ref", "HEAD", "refs/heads/new_git_name"])
            .current_dir(&t.0)
            .status()
            .unwrap()
            .success()
    );
    for args in [
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "base",
        ],
        vec!["checkout", "-qb", "old_git_name"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "feature",
        ],
        vec!["checkout", "-q", "new_git_name"],
        vec!["merge", "--ff-only", "--quiet", "old_git_name"],
        vec!["branch", "-d", "old_git_name"],
    ] {
        let o = Command::new("git")
            .args(args)
            .current_dir(&t.0)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    let before = fs::read(t.0.join(".git/HEAD")).unwrap();
    let list = t.run(&["branch", "list"], true);
    assert_eq!(list["result"]["entries"][0]["name"], "old_git_name");
    t.run(&["branch", "rename", "old_git_name", "renamed_data"], true);
    assert_eq!(fs::read(t.0.join(".git/HEAD")).unwrap(), before);
}

#[test]
fn human_output_handles_full_length_names_and_escapes_bidi_without_changing_identity() {
    let t = Tree::new();
    for name in ["x".repeat(4096), "analysis\u{202e}".into()] {
        let o = Command::new(env!("CARGO_BIN_EXE_transflow"))
            .arg("--workspace")
            .arg(&t.0)
            .args(["branch", "create", &name])
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let text = String::from_utf8(o.stdout).unwrap();
        assert!(!text.contains('\u{202e}'));
        let list = t.run(&["branch", "list"], true);
        assert!(
            list["result"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"] == name)
        );
    }
}
