//! Real processes and pipes: no mock process-completion or cancellation assertions.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Fixture construction/assertions.
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tf_exec::{
    discovery::Scratch,
    supervisor::{self, Cancellation, Failure, Launch, Policy},
};
use tf_protocol::Operation;

fn launch(root: &Path, mode: &str) -> Launch {
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
            include_str!("fixtures/supervisor_peer.py")
        ),
    )
    .unwrap();
    fs::set_permissions(&peer, fs::Permissions::from_mode(0o700)).unwrap();
    Launch {
        python: peer,
        operation: Operation::Discover,
        request_schema: "DiscoveryRequestV1",
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
fn stopped(pid: u32) -> bool {
    // A descendant can briefly remain a zombie awaiting its system parent. It
    // must no longer execute, and the direct child must have been reaped.
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .unwrap();
    let status = String::from_utf8(output.stdout).unwrap();
    status.trim().is_empty() || status.trim().starts_with('Z')
}
#[test]
fn both_noisy_streams_drain_beyond_retention_without_corrupting_control() {
    let root = Scratch::create().unwrap();
    let report = supervisor::run(launch(root.path(), "noisy"), Cancellation::default());
    assert_eq!(report.outcome, Ok(()), "{report:?}");
    assert!(report.logs_complete);
    for (log, name) in [
        (&report.stdout, "stdout.log"),
        (&report.stderr, "stderr.log"),
    ] {
        assert!(log.observed_bytes > 4 * 1024 * 1024);
        assert!(log.truncated);
        assert!(log.bytes.len() <= 4096);
        assert!(log.bytes.ends_with(b"[transflow: log truncated]\n"));
        assert_eq!(fs::read(root.path().join(name)).unwrap(), log.bytes);
        assert_eq!(
            fs::metadata(root.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(stopped(report.pid.unwrap()));
}
#[test]
fn crash_and_startup_crash_preserve_independent_logs() {
    for mode in ["crash", "startup_crash", "nonzero", "worker_error"] {
        let root = Scratch::create().unwrap();
        let report = supervisor::run(launch(root.path(), mode), Cancellation::default());
        assert!(report.outcome.is_err(), "{mode}: {report:?}");
        assert!(String::from_utf8_lossy(&report.stdout.bytes).contains("stdout"));
        assert!(String::from_utf8_lossy(&report.stderr.bytes).contains("stderr"));
        assert!(report.logs_complete);
        assert!(stopped(report.pid.unwrap()));
    }
}
#[test]
fn invalid_frames_identity_phase_and_terminal_order_fail_closed() {
    for mode in [
        "bad_auth",
        "wrong_session",
        "wrong_major",
        "oversized",
        "invalid",
        "wrong_phase",
        "phase_regression",
        "missing_result",
        "post_terminal",
        "duplicate_result",
    ] {
        let root = Scratch::create().unwrap();
        let report = supervisor::run(launch(root.path(), mode), Cancellation::default());
        assert_eq!(
            report.outcome,
            Err(if mode == "bad_auth" {
                Failure::Authentication
            } else {
                Failure::Protocol
            }),
            "{mode}: {report:?}"
        );
        assert!(stopped(report.pid.unwrap()));
    }
}
#[test]
fn partial_control_and_missing_startup_obey_absolute_deadlines() {
    for mode in ["partial", "startup_hang", "no_hello"] {
        let root = Scratch::create().unwrap();
        let mut request = launch(root.path(), mode);
        request.policy.operation_timeout = Some(Duration::from_millis(150));
        let began = Instant::now();
        let report = supervisor::run(request, Cancellation::default());
        assert_eq!(report.outcome, Err(Failure::Deadline));
        assert!(began.elapsed() < Duration::from_secs(2));
        assert!(stopped(report.pid.unwrap()));
    }
}
#[test]
fn redaction_handles_split_secrets_nonce_and_binary_log_bytes() {
    let root = Scratch::create().unwrap();
    let report = supervisor::run(launch(root.path(), "redact"), Cancellation::default());
    assert_eq!(report.outcome, Ok(()));
    assert_eq!(
        report.stdout.bytes,
        b"prefix [REDACTED] suffix\nAuthorization: [REDACTED]\nCookie: [REDACTED]\n"
    );
    assert_eq!(report.stderr.bytes, b"[REDACTED] \xff\x00end\n");
}
#[test]
fn heartbeat_silence_is_diagnostic_and_success_cleans_descendants() {
    for mode in ["silent", "descendant", "quiet_descendant"] {
        let root = Scratch::create().unwrap();
        let mut request = launch(root.path(), mode);
        request.policy.heartbeat_warning = Duration::from_millis(30);
        if mode == "quiet_descendant" {
            request.policy.termination_grace = Duration::from_millis(250);
        }
        let report = supervisor::run(request, Cancellation::default());
        assert_eq!(report.outcome, Ok(()), "{mode}: {report:?}");
        if mode == "silent" {
            assert!(report.heartbeat_delayed);
        } else {
            let pid = fs::read_to_string(root.path().join("descendant.pid"))
                .unwrap()
                .parse()
                .unwrap();
            assert!(stopped(pid));
            if mode == "quiet_descendant" {
                // Its final pulse must occur well into the requested 250 ms grace,
                // even though this descendant closed both inherited log pipes.
                let term: f64 = fs::read_to_string(root.path().join("term.time"))
                    .unwrap()
                    .parse()
                    .unwrap();
                let pulse: f64 = fs::read_to_string(root.path().join("pulse"))
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!(
                    pulse - term >= 0.10,
                    "closed pipes must not shorten group grace"
                );
            }
        }
    }
}
#[test]
fn dropping_async_future_cancels_group_reaps_leader_and_flushes_logs() {
    let root = Scratch::create().unwrap();
    let mut request = launch(root.path(), "cancel");
    let pool = tf_exec::admission::Admission::new(tf_exec::admission::Capacity {
        jobs: 1,
        cpu: 1,
        ..Default::default()
    })
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let job = pool.request(Default::default()).unwrap().await.unwrap();
        request.reservation = Some(job.worker().unwrap());
        request.policy.operation_timeout = None;
        request.policy.termination_grace = Duration::from_millis(250);
        let mut limits = tf_exec::timing::Limits::default();
        limits.discovery.value = 0;
        request.timing = Some(tf_exec::timing::Work {
            budget: tf_exec::timing::Budget::new(limits, "cancel fixture".into()).unwrap(),
            phase: tf_exec::timing::Phase::Discovery,
        });
        drop(job);
        let task = tokio::spawn(supervisor::run_async(request, Cancellation::default()));
        let start = Instant::now();
        while !root.path().join("pulse").exists() {
            assert!(start.elapsed() < Duration::from_secs(5));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(pool.usage().unwrap().jobs, 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let leader = fs::read_to_string(root.path().join("leader.pid"))
            .unwrap()
            .parse()
            .unwrap();
        let descendant = fs::read_to_string(root.path().join("descendant.pid"))
            .unwrap()
            .parse()
            .unwrap();
        // A slow CI scheduler may resume us after cleanup already finished.
        // Capacity may only be free once both real processes have stopped.
        if pool.usage().unwrap().jobs == 0 {
            assert!(stopped(leader) && stopped(descendant));
        }
        while !(stopped(leader) && stopped(descendant)) {
            assert!(start.elapsed() < Duration::from_secs(5));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    // Runtime shutdown joins the blocking cleanup owner before files are inspected.
    drop(runtime);
    assert_eq!(pool.usage().unwrap(), Default::default());
    assert!(!fs::read(root.path().join("stdout.log")).unwrap().is_empty());
    assert!(!fs::read(root.path().join("stderr.log")).unwrap().is_empty());
    let pulse = fs::read(root.path().join("pulse")).unwrap();
    thread::sleep(Duration::from_millis(100));
    assert_eq!(fs::read(root.path().join("pulse")).unwrap(), pulse);
}
#[test]
fn invalid_policy_and_precanceled_work_never_spawn() {
    let root = Scratch::create().unwrap();
    let mut request = launch(root.path(), "silent");
    request.python = "python".into();
    assert_eq!(
        supervisor::run(request, Cancellation::default()).outcome,
        Err(Failure::Configuration)
    );
    let mut request = launch(root.path(), "silent");
    request.policy.log_bytes = request.policy.workspace_log_cap + 1;
    assert_eq!(
        supervisor::run(request, Cancellation::default()).outcome,
        Err(Failure::Configuration)
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert_eq!(
        supervisor::run(launch(root.path(), "silent"), cancel).outcome,
        Err(Failure::Canceled)
    );
    assert!(!root.path().join("leader.pid").exists());
}

#[test]
fn log_paths_and_requests_are_rejected_before_spawning_or_overwriting() {
    let root = Scratch::create().unwrap();
    let target = root.path().join("preserved");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, root.path().join("stdout.log")).unwrap();
    let report = supervisor::run(launch(root.path(), "silent"), Cancellation::default());
    assert_eq!(report.outcome, Err(Failure::Io));
    assert!(report.pid.is_none());
    assert_eq!(fs::read(target).unwrap(), b"keep");
    let mut request = launch(root.path(), "silent");
    request.request = json!([]);
    assert_eq!(
        supervisor::run(request, Cancellation::default()).outcome,
        Err(Failure::Configuration)
    );
    let mut request = launch(root.path(), "silent");
    request.request["capture_root"] = "z".repeat(1024 * 1024 + 1).into();
    assert_eq!(
        supervisor::run(request, Cancellation::default()).outcome,
        Err(Failure::Configuration)
    );
    assert!(!root.path().join("leader.pid").exists());
}

#[derive(Default)]
struct ManualClock(AtomicU64);
impl tf_exec::timing::Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_secs(self.0.load(Ordering::SeqCst))
    }
}
#[test]
fn real_validation_helpers_share_one_hour_and_one_reservation() {
    use tf_exec::{
        admission::{Admission, Capacity},
        timing::{Budget, Limits, Phase, Work},
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let pool = Admission::new(Capacity {
        jobs: 1,
        cpu: 3,
        ..Default::default()
    })
    .unwrap();
    let clock = Arc::new(ManualClock::default());
    let budget =
        Budget::with_clock(Limits::default(), "curated/orders".into(), clock.clone()).unwrap();
    runtime.block_on(async {
        let job = pool.request(pool.default_demand()).unwrap().await.unwrap();
        for (seconds, success) in [(301, true), (3599, true), (3600, false)] {
            let root = Scratch::create().unwrap();
            let mut request = launch(root.path(), "budget");
            // Synthetic peer uses a discovery-shaped payload only to locate its fixture files.
            request.operation = Operation::EvaluateChecks;
            request.policy.operation_timeout = None;
            request.timing = Some(Work {
                budget: budget.clone(),
                phase: Phase::InputValidation,
            });
            request.reservation = Some(job.worker().unwrap());
            let task = tokio::spawn(supervisor::run_async(request, Cancellation::default()));
            let start = Instant::now();
            while !root.path().join("ready").exists() {
                assert!(start.elapsed() < Duration::from_secs(5));
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            budget.check(Some("order_key".into())).unwrap();
            clock.0.store(seconds, Ordering::SeqCst);
            if success {
                fs::write(root.path().join("release"), b"").unwrap();
            }
            let report = task.await.unwrap();
            assert_eq!(report.threads, Some(3));
            let env: serde_json::Value =
                serde_json::from_slice(&fs::read(root.path().join("threads.json")).unwrap())
                    .unwrap();
            assert!(env.as_object().unwrap().values().all(|v| v == "3"));
            assert!(report.logs_complete);
            assert!(stopped(report.pid.unwrap()));
            assert_eq!(pool.usage().unwrap().jobs, 1);
            if success {
                assert_eq!(report.outcome, Ok(()), "{report:?}");
            } else {
                assert_eq!(report.outcome, Err(Failure::PhaseDeadline), "{report:?}");
                let timeout = report.timeout.unwrap();
                assert_eq!(timeout.phase, Phase::InputValidation);
                assert_eq!(timeout.elapsed, Duration::from_secs(3600));
                assert_eq!(timeout.check.as_deref(), Some("order_key"));
                assert!(String::from_utf8_lossy(&report.stdout.bytes).contains("budget stdout"));
                assert!(String::from_utf8_lossy(&report.stderr.bytes).contains("budget stderr"));
            }
        }
        drop(job);
        assert_eq!(pool.usage().unwrap(), Default::default());
    });
}
