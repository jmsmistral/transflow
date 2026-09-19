//! Environment CLI diagnostics, explicit context and no installation on inspection failures.
#![allow(clippy::unwrap_used, reason = "Synthetic CLI assertions")]
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
#[test]
fn environment_help_needs_no_workspace_or_python() {
    let out = Command::new(env!("CARGO_BIN_EXE_transflow"))
        .args(["env", "--help"])
        .env("PATH", "")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
}
#[test]
fn missing_interpreter_is_an_operational_json_failure_without_installation() {
    let root = std::env::temp_dir().join(format!(
        "transflow-env-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let init = Command::new(env!("CARGO_BIN_EXE_transflow"))
        .arg("init")
        .arg(&root)
        .output()
        .unwrap();
    assert!(init.status.success());
    let before = fs::read(root.join("workspace.toml")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_transflow"))
        .arg("--workspace")
        .arg(&root)
        .args([
            "env",
            "check",
            "--python",
            "/nonexistent/transflow-python",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["outcome"], "failure");
    assert_eq!(fs::read(root.join("workspace.toml")).unwrap(), before);
    assert!(!root.join("requirements.lock").exists());
    assert!(!root.join(".transflow/runtime/environment.json").exists());
    fs::remove_dir_all(root).unwrap();
}
