//! Native canonical evaluator qualification over strictly normalized synthetic artifacts.
use serde_json::{Value, json};
use std::{error::Error, fs::File, path::PathBuf, sync::Arc, time::Duration};
use tf_exec::{
    checks::{self, Subject},
    supervisor::{Cancellation, Launch, Policy},
    timing::{Budget, Clock, Limits, Phase, Work},
};
use tf_protocol::{
    Operation,
    canonical::{ContentDigest, DigestKind},
};
use tf_store::artifacts::ArtifactStore;
// The native query-start marker advances only this fixture's virtual phase clock.
// Production startup still consumes its normal phase budget; cold imports must not
// make this query-interruption test expire before the query is reached.
struct QueryClock(PathBuf);
impl Clock for QueryClock {
    fn now(&self) -> Duration {
        Duration::from_secs(if self.0.is_file() { 4 } else { 0 })
    }
}
fn main() -> Result<(), Box<dyn Error>> {
    let a = std::env::args_os().skip(1).collect::<Vec<_>>();
    let workspace = PathBuf::from(a.first().ok_or("workspace")?);
    let python = PathBuf::from(a.get(1).ok_or("python")?);
    let mut input: Value = serde_json::from_reader(File::open(a.get(2).ok_or("request")?)?)?;
    let mode = a.get(3).and_then(|v| v.to_str()).unwrap_or("input");
    let checks = input["checks"].as_array().ok_or("checks")?.clone();
    input.as_object_mut().ok_or("object")?.remove("checks");
    let store = ArtifactStore::open(&workspace)?;
    let digest = ContentDigest::from_hex(
        DigestKind::Artifact,
        input["artifact_digest"].as_str().ok_or("digest")?,
    )?;
    let verified = store.verify(digest)?;
    let phase = if mode == "output" {
        Phase::OutputValidation
    } else {
        Phase::InputValidation
    };
    let mut limits = Limits::default();
    if mode == "timeout" {
        // Keep the three-second policy; inject its expiry at the observed query boundary.
        limits.validation.value = 3;
    }
    if mode == "disabled" {
        limits.validation.value = 0;
    }
    let log = PathBuf::from(input["result_directory"].as_str().ok_or("result")?);
    let budget = if mode == "timeout" {
        Budget::with_clock(
            limits,
            "synthetic canonical checks".into(),
            Arc::new(QueryClock(log.join("measurement.json"))),
        )?
    } else {
        Budget::new(limits, "synthetic canonical checks".into())?
    };
    let cancel = Cancellation::default();
    if mode == "canceled" {
        cancel.cancel();
    }
    input["phase"] = json!(if mode == "output" { "output" } else { "input" });
    if mode == "output" {
        input["subject_version"] = Value::Null;
        input["binding"] = Value::Null;
    }
    let launch = Launch {
        python,
        operation: Operation::EvaluateChecks,
        request_schema: "CheckEvaluationRequestV1",
        phase_events: None,
        request: input,
        policy: Policy {
            termination_grace: Duration::from_millis(100),
            ..Policy::default()
        },
        log_directory: Some(log),
        redact: vec![],
        threads: 1,
        reservation: None,
        timing: Some(Work { budget, phase }),
    };
    let result = if mode == "output" {
        let hex = digest.hex();
        let root = workspace
            .join(".transflow/runtime/objects")
            .join(&hex[..2])
            .join(hex);
        let paths = verified.manifest()["files"]
            .as_array()
            .ok_or("files")?
            .iter()
            .map(|v| v["path"].as_str().map(PathBuf::from).ok_or("path"))
            .collect::<Result<Vec<_>, _>>()?;
        let candidate = store.prepare(
            &File::open(root)?,
            &paths,
            verified.manifest()["writer"].clone(),
            tf_exec::discovery::random_id()?.parse()?,
            |_| Ok(()),
        )?;
        checks::run(launch, Subject::Candidate(&candidate), &checks, cancel)?
    } else {
        checks::run(launch, Subject::Pinned(&verified), &checks, cancel)?
    };
    println!(
        "{}",
        json!({"result":result.result,"samples":result.samples,"worker":result.report.as_ref().map(|r|json!({"outcome":format!("{:?}",r.outcome),"timeout":r.timeout.as_ref().map(ToString::to_string),"stdout":String::from_utf8_lossy(&r.stdout.bytes),"stderr":String::from_utf8_lossy(&r.stderr.bytes),"logs_complete":r.logs_complete}))})
    );
    Ok(())
}
