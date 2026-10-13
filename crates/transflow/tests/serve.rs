//! HTTP readiness must already support graceful SIGINT, including immediate restarts.
#![allow(clippy::unwrap_used, reason = "Synthetic test assertions")]
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("transflow-serve-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let workspace = Self(root);
        let output = Command::new(env!("CARGO_BIN_EXE_transflow"))
            .args(["init", "--json"])
            .arg(&workspace.0)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        workspace
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Coordinator(Option<Child>);
impl Coordinator {
    fn start(workspace: &Workspace) -> Self {
        Self(Some(
            Command::new(env!("CARGO_BIN_EXE_transflow"))
                .arg("--workspace")
                .arg(&workspace.0)
                .args(["serve", "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ))
    }
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }
    fn output(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.child().try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "Coordinator did not shut down");
            std::thread::yield_now();
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn immediate_http_ready_interrupt_stops_cleanly_and_releases_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = Workspace::new();
    let registration_path = workspace.0.join(".transflow/runtime/runtime.json");
    for iteration in 0..12 {
        let mut coordinator = Coordinator::start(&workspace);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if coordinator.child().try_wait().unwrap().is_some() {
                let output = coordinator.output();
                return Err(format!("Startup failed on restart {iteration}: {output:?}").into());
            }
            let ready = fs::read(&registration_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|registration| {
                    registration["pid"].as_u64() == Some(u64::from(coordinator.child().id()))
                        && registration["http_endpoint"].as_str().is_some()
                });
            if ready {
                break;
            }
            assert!(Instant::now() < deadline, "HTTP readiness timed out");
            std::thread::yield_now();
        }
        // No HTTP request or delay between observing readiness and interrupting.
        // This child is still owned and unreaped, so its PID cannot be reused.
        rustix::process::kill_process(
            rustix::process::Pid::from_child(coordinator.child()),
            rustix::process::Signal::INT,
        )
        .unwrap();
        let output = coordinator.output();
        assert!(
            output.status.success(),
            "Shutdown failed on restart {iteration}: {output:?}"
        );
        assert!(output.stderr.is_empty(), "{output:?}");
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["result"]["data"]["state"], "STOPPED");
    }
    Ok(())
}
