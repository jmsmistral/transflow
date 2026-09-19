//! Initialization against real files, with no Python, Git or package installer on PATH.
#![allow(clippy::unwrap_used, reason = "Synthetic test assertions")]
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "transflow-init-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn init(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_transflow"))
        .args(["init", "--json"])
        .arg(root)
        .env("PATH", "")
        .output()
        .unwrap()
}
#[test]
fn creates_and_repeats_without_changing_identity_or_user_files() {
    let t = Temp::new();
    fs::write(t.0.join("requirements.in"), "synthetic-package==1\n").unwrap();
    fs::write(t.0.join(".gitignore"), ".transflow/\n*.toml").unwrap();
    let first = init(&t.0);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert!(first.stderr.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(json["result"]["kind"], "workspace_init");
    let config = fs::read(t.0.join("workspace.toml")).unwrap();
    let registry = fs::read(t.0.join(".transflow/catalog.toml")).unwrap();
    let ignore = fs::read(t.0.join(".gitignore")).unwrap();
    assert!(init(&t.0).status.success());
    assert_eq!(fs::read(t.0.join("workspace.toml")).unwrap(), config);
    assert_eq!(
        fs::read(t.0.join(".transflow/catalog.toml")).unwrap(),
        registry
    );
    assert_eq!(fs::read(t.0.join(".gitignore")).unwrap(), ignore);
    assert_eq!(
        fs::read_to_string(t.0.join("requirements.in")).unwrap(),
        "synthetic-package==1\n"
    );
    assert!(!t.0.join(".git").exists());
    assert!(!t.0.join("requirements.lock").exists());
    assert!(!t.0.join(".transflow/runtime/catalog.sqlite").exists());
}
#[test]
fn partial_state_and_symlinks_fail_without_overwriting() {
    let t = Temp::new();
    fs::write(t.0.join("workspace.toml"), "existing").unwrap();
    assert_eq!(init(&t.0).status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(t.0.join("workspace.toml")).unwrap(),
        "existing"
    );
    assert!(!t.0.join(".transflow").exists());
    let t = Temp::new();
    let outside = Temp::new();
    std::os::unix::fs::symlink(&outside.0, t.0.join(".transflow")).unwrap();
    assert_eq!(init(&t.0).status.code(), Some(1));
    assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
}
#[test]
fn root_selection_uses_nearest_or_explicit_without_sibling_search() {
    let outer = Temp::new();
    assert!(init(&outer.0).status.success());
    let inner = outer.0.join("nested");
    fs::create_dir(&inner).unwrap();
    assert!(init(&inner).status.success());
    let child = inner.join("child");
    fs::create_dir(&child).unwrap();
    use tf_catalog::workspace::select_workspace;
    assert_eq!(
        select_workspace(&child, None).unwrap(),
        inner.canonicalize().unwrap()
    );
    assert_eq!(
        select_workspace(&child, Some(&outer.0)).unwrap(),
        outer.0.canonicalize().unwrap()
    );
    assert!(select_workspace(&child, Some(&child)).is_err());
}
#[test]
fn git_confirms_registry_trackable_runtime_and_locators_ignored() {
    let t = Temp::new();
    fs::write(t.0.join(".gitignore"), "*\n.transflow/\n*.toml\n").unwrap();
    assert!(init(&t.0).status.success());
    let status = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&t.0)
        .status()
        .unwrap();
    assert!(status.success());
    let ignored = |path: &str| {
        Command::new("git")
            .args(["check-ignore", "--no-index", "-q", path])
            .current_dir(&t.0)
            .status()
            .unwrap()
            .success()
    };
    assert!(!ignored(".transflow/catalog.toml"));
    assert!(ignored(".transflow/runtime/file"));
    assert!(ignored("transflow.local.toml"));
}

#[test]
fn loading_rejects_escaped_roots_and_local_policy_without_mutation() {
    let t = Temp::new();
    let outside = Temp::new();
    assert!(init(&t.0).status.success());
    use tf_catalog::workspace::Workspace;
    let loaded = Workspace::load(&t.0, None).unwrap();
    assert_eq!(loaded.roots(), [t.0.join("src").canonicalize().unwrap()]);
    fs::remove_dir(t.0.join("src")).unwrap();
    std::os::unix::fs::symlink(&outside.0, t.0.join("src")).unwrap();
    assert!(Workspace::load(&t.0, None).is_err());
    assert!(!init(&t.0).status.success());
    assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
    fs::remove_file(t.0.join("src")).unwrap();
    fs::create_dir(t.0.join("src")).unwrap();
    fs::write(t.0.join("transflow.local.toml"), "[execution]\nmax_jobs=9").unwrap();
    assert!(Workspace::load(&t.0, None).is_err());
}
#[test]
fn runtime_remnants_cannot_receive_a_new_identity() {
    let t = Temp::new();
    fs::create_dir_all(t.0.join(".transflow/runtime")).unwrap();
    fs::write(t.0.join(".transflow/runtime/history"), "preserve").unwrap();
    assert!(!init(&t.0).status.success());
    assert!(!t.0.join("workspace.toml").exists());
    assert_eq!(
        fs::read_to_string(t.0.join(".transflow/runtime/history")).unwrap(),
        "preserve"
    );
}

#[test]
fn init_help_performs_no_initialization() {
    let t = Temp::new();
    let output = Command::new(env!("CARGO_BIN_EXE_transflow"))
        .args(["init", "--help"])
        .arg(&t.0)
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(fs::read_dir(&t.0).unwrap().count(), 0);
}
