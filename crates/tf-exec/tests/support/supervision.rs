#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)] // Shared native test fixtures.
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tf_exec::supervisor::{Launch, Policy};
use tf_protocol::Operation;

pub fn launch(root: &Path, mode: &str) -> Launch {
    // Use the same prepared Python as the Rust contract fixtures, only stdlib needed.
    let python = Command::new("python")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    let interpreter = String::from_utf8(python.stdout).unwrap();
    let peer = root.join("managed-python");
    fs::write(
        &peer,
        format!(
            "#!{}\n{}",
            interpreter.trim(),
            include_str!("../fixtures/supervisor_peer.py")
        ),
    )
    .unwrap();
    fs::set_permissions(&peer, fs::Permissions::from_mode(0o700)).unwrap();
    Launch {
        python: peer,
        operation: Operation::Discover,
        request_schema: "DiscoveryRequestV1",
        phase_events: None,
        request: json!({"format_version":1,"protocol":{"major":1,"minor":0},"request_id":"00000000-0000-4000-8000-000000000001","attempt_id":"00000000-0000-4000-8000-000000000002","capture_root":mode,"source_roots":[],"files":[],"catalog":{"format_version":1,"workspace_id":"00000000-0000-4000-8000-000000000003","source_snapshot_id":"00000000-0000-4000-8000-000000000004","catalog_fingerprint":"a".repeat(64),"entries":[]},"environment_fingerprint":"b".repeat(64),"result_directory":root}),
        policy: Policy {
            log_bytes: 4096,
            operation_timeout: Some(Duration::from_secs(10)),
            termination_grace: Duration::from_millis(70),
            ..Policy::default()
        },
        log_directory: Some(root.to_owned()),
        redact: vec![b"synthetic-credential".to_vec()],
        timing: None,
        reservation: None,
        threads: 1,
    }
}
pub fn stopped(pid: u32) -> bool {
    // A descendant can briefly remain a zombie awaiting its system parent. It
    // must no longer execute, and the direct child must have been reaped.
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .unwrap();
    let status = String::from_utf8(output.stdout).unwrap();
    status.trim().is_empty() || status.trim().starts_with('Z')
}
