//! Actual framed streams and session failures, using synthetic IDs and bounded payloads.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Test setup/assertions only.
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{self, Cursor, Read},
};
use tf_protocol::*;
const REQUEST: &str = "00000000-0000-4000-8000-000000000001";
const ATTEMPT: &str = "00000000-0000-4000-8000-000000000002";
fn raw(sequence: u64, message: Value) -> Value {
    json!({"protocol":{"major":1,"minor":0},"request_id":REQUEST,"attempt_id":ATTEMPT,"sequence":sequence.to_string(),"required_capabilities":[],"extensions":[],"message":message})
}
fn frame(sequence: u64, message: Value) -> ControlFrame {
    ControlFrame::from_json(raw(sequence, message)).unwrap()
}
fn hello() -> Value {
    json!({"type":"hello","operation":"execute","capabilities":[]})
}
fn session() -> Session {
    Session::new(
        REQUEST.parse().unwrap(),
        ATTEMPT.parse().unwrap(),
        Operation::Execute,
        BTreeSet::new(),
    )
    .unwrap()
}
#[test]
fn runtime_assertions_agree_with_all_shared_schema_cases() {
    let versions: Value = serde_json::from_str(FORMAT_VERSIONS).unwrap();
    assert_eq!(versions["worker_protocol"]["major"], PROTOCOL_MAJOR);
    assert_eq!(versions["worker_protocol"]["minor"], PROTOCOL_MINOR);
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json")).unwrap();
    assert_eq!(cases.len(), 243);
    for case in cases {
        assert_eq!(
            validate_document(case["schema"].as_str().unwrap(), &case["value"]).is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
    assert!(validate_document("UnknownContract", &Value::Null).is_err());
}
#[test]
fn multiple_frames_round_trip_and_clean_eof_is_distinct() {
    let messages = [
        hello(),
        json!({"type":"metric","name":"total","value":{"type":"u64","value":"18446744073709551615"}}),
        json!({"type":"completed"}),
    ];
    let mut bytes = Vec::new();
    for (n, m) in messages.iter().enumerate() {
        write_frame(&mut bytes, &frame(n as u64, m.clone())).unwrap();
    }
    let mut reader = Cursor::new(bytes);
    for (n, m) in messages.iter().enumerate() {
        let got = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(got.sequence(), n as u64);
        assert_eq!(got.as_json()["message"], *m);
    }
    assert!(read_frame(&mut reader).unwrap().is_none());
}
struct Bytewise<R>(R);
impl<R: Read> Read for Bytewise<R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = b.len().min(1);
        self.0.read(&mut b[..n])
    }
}
#[test]
fn fragmented_reads_and_every_truncation_fail_correctly() {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &frame(0, hello())).unwrap();
    assert!(
        read_frame(&mut Bytewise(Cursor::new(&bytes)))
            .unwrap()
            .is_some()
    );
    for end in 1..bytes.len() {
        assert!(
            matches!(
                read_frame(&mut Cursor::new(&bytes[..end])),
                Err(ProtocolError::Truncated)
            ),
            "cut {end}"
        );
    }
}
#[test]
fn byte_limits_apply_before_reading_body_or_writing_output() {
    for n in [0, MAX_FRAME_BYTES + 1, u32::MAX as usize] {
        assert!(matches!(
            read_frame(&mut Cursor::new((n as u32).to_be_bytes())),
            Err(ProtocolError::Size)
        ));
    }
    let mut raw = raw(
        0,
        json!({"type":"error","code":"synthetic","message":"","retryable":false}),
    );
    let base = serde_json::to_vec(&raw).unwrap().len();
    raw["message"]["message"] = json!("x".repeat(MAX_FRAME_BYTES - base));
    let exact = serde_json::to_vec(&raw).unwrap();
    assert_eq!(exact.len(), MAX_FRAME_BYTES);
    assert!(decode_payload(&exact).is_ok());
    raw["message"]["message"] = json!("x".repeat(MAX_FRAME_BYTES - base + 1));
    let large = ControlFrame::from_json(raw).unwrap();
    let mut sink = Vec::new();
    assert!(matches!(
        write_frame(&mut sink, &large),
        Err(ProtocolError::Size)
    ));
    assert!(sink.is_empty());
}
#[test]
fn strict_json_rejects_duplicates_invalid_utf8_nonfinite_and_deep_payloads() {
    for text in [
        b"\xff".as_slice(),
        b"{} {}",
        b"{\"x\":1,\"x\":2}",
        b"{\"x\":{\"y\":1,\"y\":2}}",
        b"{\"x\":NaN}",
        b"{\"x\":\"\\ud800\"}",
    ] {
        assert!(matches!(decode_payload(text), Err(ProtocolError::Json)));
    }
    let deep = format!("{}0{}", "[".repeat(200), "]".repeat(200));
    assert!(decode_payload(deep.as_bytes()).is_err());
    let mut v = raw(0, hello());
    v["protocol"]["major"] = json!(2);
    assert!(matches!(
        decode_payload(&serde_json::to_vec(&v).unwrap()),
        Err(ProtocolError::Version)
    ));
    v = raw(0, json!({"type":"run_untrusted_code"}));
    assert!(ControlFrame::from_json(v).is_err());
}
#[test]
fn session_requires_hello_and_checks_operation_before_advancing() {
    let mut s = session();
    assert!(matches!(
        s.accept(&frame(1, json!({"type":"heartbeat"}))),
        Err(ProtocolError::Order)
    ));
    assert!(s.negotiated().is_none());
    let wrong = frame(
        0,
        json!({"type":"hello","operation":"discover","capabilities":[]}),
    );
    assert!(matches!(s.accept(&wrong), Err(ProtocolError::Order)));
    assert!(s.negotiated().is_none());
    s.accept(&frame(0, hello())).unwrap();
    assert!(matches!(
        s.accept(&frame(1, hello())),
        Err(ProtocolError::Order)
    ));
}
#[test]
fn wrong_id_replay_and_post_terminal_messages_are_rejected() {
    let mut s = session();
    let mut wrong = raw(0, hello());
    wrong["attempt_id"] = json!(REQUEST);
    assert!(matches!(
        s.accept(&ControlFrame::from_json(wrong).unwrap()),
        Err(ProtocolError::Identity)
    ));
    s.accept(&frame(4, hello())).unwrap();
    for sequence in [0, 4] {
        assert!(matches!(
            s.accept(&frame(sequence, json!({"type":"heartbeat"}))),
            Err(ProtocolError::Sequence)
        ));
    }
    s.accept(&frame(10, json!({"type":"completed"}))).unwrap();
    assert!(matches!(
        s.accept(&frame(11, json!({"type":"heartbeat"}))),
        Err(ProtocolError::Order)
    ));
}
#[test]
fn capabilities_are_an_intersection_even_with_a_higher_minor() {
    let capability = "diagnostic.note.v1".to_owned();
    let mut s = Session::new(
        REQUEST.parse().unwrap(),
        ATTEMPT.parse().unwrap(),
        Operation::Execute,
        BTreeSet::from([capability.clone()]),
    )
    .unwrap();
    let mut first = raw(
        0,
        json!({"type":"hello","operation":"execute","capabilities":[capability,"unknown.optional.v1"]}),
    );
    first["protocol"]["minor"] = json!(7);
    s.accept(&ControlFrame::from_json(first).unwrap()).unwrap();
    assert_eq!(s.negotiated().unwrap().minor(), 0);
    assert_eq!(s.negotiated().unwrap().capabilities().len(), 1);
    let mut v = raw(1, json!({"type":"heartbeat"}));
    v["extensions"] =
        json!([{"capability":"diagnostic.note.v1","value":{"type":"string","value":"synthetic"}}]);
    s.accept(&ControlFrame::from_json(v.clone()).unwrap())
        .unwrap();
    let mut missing = session();
    missing.accept(&frame(0, hello())).unwrap();
    assert!(matches!(
        missing.accept(&ControlFrame::from_json(v).unwrap()),
        Err(ProtocolError::Capability)
    ));
    let mut unknown = raw(2, json!({"type":"heartbeat"}));
    unknown["required_capabilities"] = json!(["future.required"]);
    assert!(ControlFrame::from_json(unknown).is_err());
}
#[test]
fn transform_print_bytes_are_not_control_frames() {
    let text = serde_json::to_vec(&raw(0, hello())).unwrap();
    assert!(matches!(
        read_frame(&mut Cursor::new(text)),
        Err(ProtocolError::Size)
    ));
}

#[cfg(unix)]
#[test]
fn python_peer_uses_a_private_socket_and_separate_log_streams() {
    use std::{
        fs,
        os::unix::{
            fs::{DirBuilderExt, PermissionsExt},
            net::UnixListener,
        },
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let root = std::env::temp_dir().join(format!("tf-wire-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let path = root.join("c");
    let listener = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let source_paths =
        std::env::join_paths([repo.join("python/sdk/src"), repo.join("python/worker/src")])
            .unwrap();
    let child =
        Command::new(std::env::var_os("TRANSFLOW_TEST_PYTHON").unwrap_or_else(|| "python".into()))
            .arg(repo.join("crates/tf-protocol/tests/fixtures/peer.py"))
            .arg(&path)
            .env("PYTHONPATH", source_paths)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "socket readiness timed out");
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "peer exited before connecting"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
            }
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut guard = session();
    guard
        .accept(&read_frame(&mut stream).unwrap().unwrap())
        .unwrap();
    write_frame(&mut stream,&frame(0,json!({"type":"metric","name":"total","value":{"type":"u64","value":"18446744073709551615"}}))).unwrap();
    guard
        .accept(&read_frame(&mut stream).unwrap().unwrap())
        .unwrap();
    assert!(child.0.wait().unwrap().success());
    let mut stdout = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let mut stderr = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stdout.contains("hello"));
    assert!(stderr.contains("synthetic diagnostic"));
    assert!(read_frame(&mut Cursor::new(stdout)).is_err());
}

#[test]
fn discovery_results_require_the_operation_and_capability() {
    let ready = frame(
        1,
        json!({"type":"discovery_ready","result_path":"discovery.json","result_digest":"a".repeat(64)}),
    );
    let mut accepted = Session::new(
        REQUEST.parse().unwrap(),
        ATTEMPT.parse().unwrap(),
        Operation::Discover,
        BTreeSet::from(["discovery.v1".to_string()]),
    )
    .unwrap();
    accepted
        .accept(&frame(
            0,
            json!({"type":"hello","operation":"discover","capabilities":["discovery.v1"]}),
        ))
        .unwrap();
    accepted.accept(&ready).unwrap();
    let mut denied = Session::new(
        REQUEST.parse().unwrap(),
        ATTEMPT.parse().unwrap(),
        Operation::Discover,
        BTreeSet::new(),
    )
    .unwrap();
    denied
        .accept(&frame(
            0,
            json!({"type":"hello","operation":"discover","capabilities":[]}),
        ))
        .unwrap();
    assert!(matches!(
        denied.accept(&ready),
        Err(ProtocolError::Capability)
    ));
    let mut wrong = session();
    wrong.accept(&frame(0, hello())).unwrap();
    assert!(matches!(wrong.accept(&ready), Err(ProtocolError::Order)));
}
