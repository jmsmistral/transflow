use super::*;
use tf_exec::{
    admission::Reservation,
    lifecycle,
    supervisor::{Launch, Policy},
    timing::Budget,
};
use tf_store::artifacts::{ArtifactStore, VerifiedArtifact};
pub(super) enum Observation {
    Phase(tf_exec::timing::Phase),
    Evaluation {
        result: Value,
        manifest: Value,
        samples: BTreeMap<String, Value>,
    },
    Rejected(tf_protocol::canonical::ContentDigest),
}
pub(super) enum Message<'a> {
    Observe(JobId, Observation, std::sync::mpsc::SyncSender<bool>),
    Finished(JobId, Box<lifecycle::Completion<'a>>, Reservation),
}
pub(super) struct Work {
    pub request: lifecycle::Request,
    pub inputs: Vec<VerifiedArtifact>,
    pub reservation: Reservation,
    pub cancel: Cancellation,
    pub directory: PathBuf,
    pub python: PathBuf,
}
pub(super) fn private(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !path.exists() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(fail)?;
    }
    let m = std::fs::symlink_metadata(path).map_err(fail)?;
    if !m.is_dir()
        || m.permissions().mode() & 0o777 != 0o700
        || std::fs::canonicalize(path).map_err(fail)? != path
    {
        return Err(fail("execution directory is not private"));
    }
    Ok(())
}
pub(super) fn launch(
    python: PathBuf,
    request: Value,
    operation: tf_protocol::Operation,
    dir: PathBuf,
    threads: u32,
) -> Launch {
    Launch {
        python,
        operation,
        request_schema: if operation == tf_protocol::Operation::Execute {
            "PolarsExecutionRequestV1"
        } else {
            "CheckEvaluationRequestV1"
        },
        phase_events: None,
        request,
        policy: Policy::default(),
        log_directory: Some(dir),
        redact: vec![],
        timing: None,
        reservation: None,
        threads,
    }
}
pub(super) fn run<'a>(
    artifacts: &'a ArtifactStore,
    job: JobId,
    work: Work,
    events: std::sync::mpsc::SyncSender<Message<'a>>,
) {
    let mut count = 0u32;
    let result = lifecycle::run(
        artifacts,
        work.request,
        &work.inputs,
        &work.reservation,
        work.cancel,
        |_, _| {
            count += 1;
            let dir = work.directory.join(format!("check-{count}"));
            private(&dir).map_err(|_| lifecycle::Error::Contract)?;
            Ok(launch(
                work.python.clone(),
                json!({"format_version":1,"protocol":{"major":1,"minor":0},"request_id":tf_exec::discovery::random_id().map_err(tf_store::artifacts::ArtifactError::from)?,"auth_token":"0".repeat(64),"result_directory":dir,"memory_bytes":work.reservation.demand().memory_bytes.map(|n|n.to_string()),"spill_bytes":"1073741824"}),
                tf_protocol::Operation::EvaluateChecks,
                dir,
                work.reservation.demand().cpu,
            ))
        },
        |e| {
            let observation = match e {
                lifecycle::Event::Enter(p) => Observation::Phase(p),
                lifecycle::Event::Evaluated(e) => Observation::Evaluation {
                    result: e.result.clone(),
                    manifest: e.manifest.clone(),
                    samples: e.samples.clone(),
                },
                lifecycle::Event::Rejected(e) => {
                    let Some(c) = &e.candidate else { return Ok(()) };
                    Observation::Rejected(c.digest())
                }
            };
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            events
                .send(Message::Observe(job, observation, tx))
                .map_err(|_| lifecycle::Error::Contract)?;
            if rx.recv().map_err(|_| lifecycle::Error::Contract)? {
                Ok(())
            } else {
                Err(lifecycle::Error::Contract)
            }
        },
    );
    let _ = events.send(Message::Finished(job, Box::new(result), work.reservation));
}
pub(super) fn request(
    p: &context::Prepared,
    id: JobId,
    attempt: AttemptId,
    contract: tf_store::publication::PublicationContract,
    inputs: &[VerifiedArtifact],
    directory: &Path,
    threads: u32,
) -> Result<lifecycle::Request> {
    let (raw, resolved) = p
        .declarations
        .get(&id)
        .ok_or_else(|| fail("missing producer"))?;
    let j = p.jobs.get(&id).ok_or_else(|| fail("missing job"))?;
    let key = j.target().dataset.dataset_id().to_string();
    let mut bindings = vec![];
    for (input, artifact) in contract.inputs.iter().zip(inputs) {
        let digest = artifact.digest().hex();
        bindings.push(json!({"alias":input.alias,"dataset":{"workspace_id":input.dataset.workspace_id().to_string(),"dataset_id":input.dataset.dataset_id().to_string()},"version_id":input.version.to_string(),"artifact_digest":digest,"artifact_root":artifact.workspace().join(".transflow/runtime/objects").join(&digest[..2]).join(&digest),"manifest":artifact.manifest()}));
    }
    let parameters = p.plan.context["parameters"][&key]
        .as_object()
        .ok_or_else(|| fail("missing parameters"))?
        .iter()
        .map(|(name, value)| json!({"name":name,"value":value}))
        .collect::<Vec<_>>();
    let output = directory.join("transform");
    private(&output)?;
    let value = json!({"format_version":1,"protocol":{"major":1,"minor":0},"request_id":new_id()?.to_string(),"attempt_id":attempt.to_string(),"auth_token":"0".repeat(64),"capture_root":p.capture.files_root(),"source_roots":p.capture.source_roots(),"files":p.capture.discovery_files(),"catalog":p.catalog,"environment_fingerprint":p.plan.environment["fingerprint"],"result_directory":output,"producer":raw,"inputs":bindings,"compression":"zstd","row_group_size":"8192","threads":threads.to_string(),"context":{"build_id":j.build().to_string(),"job_id":id.to_string(),"evaluation_time":timestamp(p.plan.created_us)?,"random_seed":"0","parameters":parameters}});
    let checks = |v: &Value| {
        v.as_array()
            .cloned()
            .ok_or_else(|| fail("invalid resolved checks"))
    };
    let input_checks = resolved["inputs"]
        .as_array()
        .ok_or_else(|| fail("invalid resolved inputs"))?
        .iter()
        .map(|i| checks(&i["checks"]))
        .collect::<Result<Vec<_>>>()?;
    Ok(lifecycle::Request {
        execute: launch(
            p.interpreter.clone(),
            value,
            tf_protocol::Operation::Execute,
            output,
            threads,
        ),
        input_checks,
        output_checks: checks(&resolved["output"]["checks"])?,
        contract,
        budget: Budget::new(
            context::limits(&p.plan.context["resources"][&key])?,
            format!("dataset {key}"),
        )
        .map_err(fail)?,
        staging: new_id()?,
    })
}
// Gregorian conversion from Unix days, bounded by the wire timestamp contract.
fn timestamp(us: i64) -> Result<Value> {
    if us < 0 {
        return Err(fail("negative evaluation clock"));
    }
    let days = us / 86_400_000_000;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(month <= 2);
    let seconds = us / 1_000_000 % 86400;
    let value = json!({"type":"timestamp","value":format!("{y:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",seconds/3600,seconds/60%60,seconds%60,us%1_000_000),"unit":"us","timezone":"UTC"});
    tf_protocol::validate_document("ScalarValue", &value).map_err(fail)?;
    Ok(value)
}
pub(super) fn report(c: &lifecycle::Completion<'_>) -> Value {
    let summarize = |r: &tf_exec::supervisor::Report| json!({"pid":r.pid,"process_start":r.process_start,"exit_code":r.exit_code,"logs_complete":r.logs_complete,"stdout_truncated":r.stdout.truncated,"stderr_truncated":r.stderr.truncated,"outcome":r.outcome.as_ref().err().map(ToString::to_string)});
    json!({"transform":c.execution.as_ref().map(&summarize),"checks":c.evaluations.iter().filter_map(|e|e.report.as_ref()).map(summarize).collect::<Vec<_>>(),"skipped":c.skipped,"rejection":c.outcome.as_ref().err().map(|e|json!({"phase":format!("{:?}",e.phase),"error":e.error.to_string(),"retention_error":e.retention_error.as_ref().map(ToString::to_string)})),"evidence_error":c.evidence_error.as_ref().map(ToString::to_string)})
}

#[cfg(test)]
mod tests {
    #[test]
    fn frozen_clock_preserves_microseconds_and_gregorian_boundaries() {
        for (us, expected) in [
            (0, "1970-01-01T00:00:00.000000Z"),
            (951_868_799_123_456, "2000-02-29T23:59:59.123456Z"),
            (1_000_000, "1970-01-01T00:00:01.000000Z"),
        ] {
            assert_eq!(
                super::timestamp(us).map(|v| v["value"].clone()).ok(),
                Some(serde_json::json!(expected))
            );
        }
        assert!(super::timestamp(-1).is_err());
        assert!(super::timestamp(i64::MAX).is_err());
    }
}
