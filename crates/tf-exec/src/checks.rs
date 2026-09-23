//! Canonical checks over borrowed verified objects or closed candidates. Never publishes.
//! Run on a bounded blocking owner with leases and the producer's reservation retained.
mod compiler;
mod results;
use crate::{
    supervisor::{self, Cancellation, Launch, Report},
    timing::Phase,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tf_protocol::{
    Operation,
    canonical::{DigestKind, content_digest},
    validate_document,
};
use tf_store::artifacts::{ArtifactStore, Candidate, VerifiedArtifact};

/// Artifact lifetime/lease authority remains with the caller through helper cleanup.
pub enum Subject<'a> {
    /// Previously strictly verified immutable object; caller retains its read lease.
    Pinned(&'a VerifiedArtifact),
    /// Closed, invisible staged bytes borrowed until evaluation finishes.
    Candidate(&'a Candidate<'a>),
}
impl Subject<'_> {
    fn manifest(&self) -> &Value {
        match self {
            Self::Pinned(v) => v.manifest(),
            Self::Candidate(v) => v.manifest(),
        }
    }
    fn root(&self) -> Result<PathBuf, Error> {
        Ok(match self {
            Self::Pinned(v) => {
                let verified = ArtifactStore::open(v.workspace())?.verify(v.digest())?;
                if verified.manifest() != v.manifest() {
                    return Err(Error::Evidence);
                }
                let h = v.digest().hex();
                v.workspace()
                    .join(".transflow/runtime/objects")
                    .join(&h[..2])
                    .join(h)
            }
            Self::Candidate(v) => v.evaluation_root()?,
        })
    }
}
/// Errors before a trustworthy check request can be formed. No evaluation PASS exists.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid context, unresolved policies, identity or result evidence.
    #[error("Canonical evaluation requires exact subjects, resolved checks and validation timing")]
    Evidence,
    /// A named closed contract was rejected, without exposing request values.
    #[error("Canonical check contract is invalid: {0}")]
    Contract(&'static str),
    /// Input/candidate bytes failed strict verification.
    #[error(transparent)]
    Artifact(#[from] tf_store::artifacts::ArtifactError),
    /// Private result file inspection failed.
    #[error("Canonical check evidence could not be read")]
    Io(#[from] std::io::Error),
}
/// Complete evaluation evidence; ERROR blocks even when the check severity is WARN.
pub struct Completion {
    /// Exact verified manifest for durable evidence binding.
    pub manifest: Value,
    /// Private sample payloads keyed by digest; never ordinary log/export fields.
    pub samples: BTreeMap<String, Value>,
    /// Validated result envelope. Only sample digests, never rows, are included here.
    pub result: Value,
    /// Bounded helper logs, process outcome and timeout details when a helper ran.
    pub report: Option<Report>,
}
fn now() -> Result<String, Error> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Evidence)?
        .as_micros()
        .to_string())
}
fn read_result(directory: &std::path::Path) -> Result<Value, Error> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(directory.join("checks.json"))?;
    let m = f.metadata()?;
    if !m.is_file() || m.nlink() != 1 || m.len() > 16 * 1024 * 1024 {
        return Err(Error::Evidence);
    }
    let mut bytes = vec![];
    f.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(Error::Evidence);
    }
    let v = serde_json::from_slice(&bytes).map_err(|_| Error::Evidence)?;
    validate_document("CheckAggregatesV1", &v).map_err(|_| Error::Evidence)?;
    Ok(v)
}
/// Compile all independent checks, supervise one isolated helper and reduce exact counts.
/// `launch.request` supplies session/context/resource policy; caller SQL, paths and query
/// parameters are overwritten from the typed checks and verified subject. An attempt-wide
/// input/output validation Budget is mandatory; successive helpers reuse it. No fresh timer.
pub fn run(
    mut launch: Launch,
    subject: Subject<'_>,
    checks: &[Value],
    cancel: Cancellation,
) -> Result<Completion, Error> {
    let start = Instant::now();
    let started = now()?;
    if launch.operation != Operation::EvaluateChecks
        || launch.request_schema != "CheckEvaluationRequestV1"
        || checks.len() > 128
    {
        return Err(Error::Evidence);
    }
    let work = launch.timing.as_ref().ok_or(Error::Evidence)?;
    let phase = match work.phase {
        Phase::InputValidation => "input",
        Phase::OutputValidation => "output",
        _ => return Err(Error::Evidence),
    };
    if launch.request["phase"] != phase {
        return Err(Error::Evidence);
    }
    if (phase == "input") == launch.request["binding"].is_null() {
        return Err(Error::Evidence);
    }
    let budget = work.budget.clone();
    budget.enter(work.phase).map_err(|_| Error::Evidence)?;
    let cancellation = cancel.clone();
    for c in checks {
        validate_document("DeclarationCheckV1", c).map_err(|_| Error::Contract("declaration"))?;
        if c["null_policy"].is_null() || c["sample_rows"].is_null() {
            return Err(Error::Evidence);
        }
    }
    let ids = checks
        .iter()
        .map(|c| c["id"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if ids.len() != checks.len() {
        return Err(Error::Evidence);
    }
    let root = subject.root()?;
    let manifest = subject.manifest();
    let digest = tf_protocol::canonical::artifact_digest(manifest)
        .map_err(|_| Error::Evidence)?
        .hex();
    launch.request["artifact_root"] = json!(root);
    launch.request["manifest"] = manifest.clone();
    launch.request["artifact_digest"] = json!(digest);
    if (matches!(subject, Subject::Candidate(_)) && !launch.request["subject_version"].is_null())
        || (matches!(subject, Subject::Pinned(_)) && launch.request["subject_version"].is_null())
    {
        return Err(Error::Evidence);
    }
    let supported = compiler::schema_supported(&manifest["logical_schema"]);
    let mut plan = compiler::compile(checks, &manifest["logical_schema"]);
    for n in &mut plan.nodes {
        if let Err(e) = supported {
            *n = Err(e);
        }
    }
    let policy = launch
        .request
        .get("sample_policy")
        .cloned()
        .unwrap_or_else(|| json!({"allowed_columns":[],"sensitive_columns":[],"max_rows":20}));
    validate_document("CheckSamplePolicyV1", &policy)
        .map_err(|_| Error::Contract("sample policy"))?;
    let allowed = policy["allowed_columns"]
        .as_array()
        .ok_or(Error::Evidence)?;
    if allowed
        .iter()
        .map(Value::to_string)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != allowed.len()
    {
        return Err(Error::Evidence);
    }
    let sample_queries = compiler::samples(checks, &manifest["logical_schema"], &policy);
    launch.request["sample_policy"] = policy.clone();
    launch.request["sample_queries"] = json!(sample_queries);
    launch.request["queries"] = json!(plan.queries);
    let threads = launch
        .reservation
        .as_ref()
        .map(|p| p.threads())
        .unwrap_or(launch.threads);
    launch.request["threads"] = json!(threads.to_string());
    let spill = crate::discovery::Scratch::create()?;
    launch.request["spill_directory"] = json!(spill.path());
    validate_document("CheckEvaluationRequestV1", &launch.request)
        .map_err(|_| Error::Contract("request"))?;
    let request = launch.request.clone();
    let directory = PathBuf::from(
        request["result_directory"]
            .as_str()
            .ok_or(Error::Evidence)?,
    );
    let dir = File::open(&directory)?;
    let before = dir.metadata()?;
    if directory.canonicalize()? != directory
        || !before.is_dir()
        || before.mode() & 0o077 != 0
        || directory.join("checks.json").try_exists()?
    {
        return Err(Error::Evidence);
    }
    let mut report = None;
    let mut aggregate_rows = vec![];
    let mut sample_rows = vec![];
    let mut samples = BTreeMap::new();
    let mut failure = if cancellation.requested() {
        Some("canceled")
    } else if budget.expired().map_err(|_| Error::Evidence)?.is_some() {
        Some("validation_timeout")
    } else {
        None
    };
    if failure.is_none()
        && plan.nodes.iter().any(|n| n.is_ok())
        && (!plan.queries.is_empty() || sample_queries.iter().any(Option::is_some))
    {
        let r = supervisor::run(launch, cancel);
        failure = match r.outcome {
            Ok(()) => None,
            Err(supervisor::Failure::PhaseDeadline) => Some("validation_timeout"),
            Err(supervisor::Failure::Canceled) => Some("canceled"),
            Err(_) => Some("helper_error"),
        };
        if failure.is_none() {
            let parsed = (|| {
                let actual = std::fs::metadata(&directory)?;
                if before.dev() != actual.dev() || before.ino() != actual.ino() {
                    return Err(Error::Evidence);
                }
                let value = read_result(&directory)?;
                let f = r.result.as_ref().ok_or(Error::Evidence)?.as_json();
                if f["message"]["results_path"] != "checks.json"
                    || f["message"]["results_digest"]
                        != content_digest(DigestKind::Compute, &value)
                            .map_err(|_| Error::Evidence)?
                            .hex()
                    || value["request_id"] != request["request_id"]
                    || value["artifact_digest"] != request["artifact_digest"]
                {
                    return Err(Error::Evidence);
                }
                let rows = value["queries"].as_array().ok_or(Error::Evidence)?;
                if rows.len() != plan.queries.len() {
                    return Err(Error::Evidence);
                }
                for (q, row) in plan.queries.iter().zip(rows) {
                    if row["error"].is_null()
                        && row["counts"].as_array().map(Vec::len)
                            != q["width"].as_u64().map(|n| n as usize)
                    {
                        return Err(Error::Evidence);
                    }
                }
                let sampled = value["samples"].as_array().ok_or(Error::Evidence)?;
                if sampled.len() != checks.len() {
                    return Err(Error::Evidence);
                }
                for (query, sample) in sample_queries.iter().zip(sampled) {
                    match (query, sample.is_null()) {
                        (None, true) => (),
                        (Some(query), false) => {
                            if sample["columns"] != query["columns"]
                                || sample["limit"] != query["limit"]
                                || sample["reason"] != query["reason"]
                                || sample["rows"].as_array().is_none_or(|r| {
                                    r.len() > query["limit"].as_u64().unwrap_or(0) as usize
                                })
                                || serde_json::to_vec(sample)
                                    .map_err(|_| Error::Evidence)?
                                    .len()
                                    > 65536
                            {
                                return Err(Error::Evidence);
                            }
                            for row in sample["rows"].as_array().ok_or(Error::Evidence)? {
                                let row = row.as_array().ok_or(Error::Evidence)?;
                                let cols = query["columns"].as_array().ok_or(Error::Evidence)?;
                                if row.len() != cols.len() {
                                    return Err(Error::Evidence);
                                }
                                for (cell, col) in row.iter().zip(cols) {
                                    let mut typ = cell.clone();
                                    typ.as_object_mut().ok_or(Error::Evidence)?.remove("value");
                                    if cell["type"] != "null" && typ != col["logical_type"] {
                                        return Err(Error::Evidence);
                                    }
                                }
                            }
                        }
                        _ => return Err(Error::Evidence),
                    }
                }
                subject.root()?; // No result may certify changed bytes.
                Ok((rows.clone(), sampled.clone()))
            })();
            match parsed {
                Ok((rows, sampled)) => {
                    aggregate_rows = rows;
                    sample_rows = sampled;
                }
                Err(_) => failure = Some("result_integrity"),
            }
        }
        report = Some(r);
    }
    if cancellation.requested() {
        failure = Some("canceled");
    }
    if budget.expired().map_err(|_| Error::Evidence)?.is_some() {
        failure = Some("validation_timeout");
    }
    let finished = now()?;
    let elapsed = start.elapsed().as_micros().to_string();
    let mut outcomes = vec![];
    for (index, (c, n)) in checks.iter().zip(&plan.nodes).enumerate() {
        let mut metrics = vec![];
        let outcome = if let Some(e) = failure {
            Err(e)
        } else {
            n.as_ref().map_err(|e| *e).and_then(|n| {
                results::outcome(
                    n,
                    &aggregate_rows,
                    c["null_policy"] == "ignore",
                    "$",
                    &mut metrics,
                )
            })
        };
        let (status, failed, error, exact) = match outcome {
            Ok((pass, failed)) => (
                if pass { "PASS" } else { "VIOLATION" },
                failed.map(|n| n.to_string()),
                None,
                true,
            ),
            Err(e) => {
                metrics.clear();
                ("ERROR", None, Some(e), false)
            }
        };
        let mut sample = Value::Null;
        if status == "VIOLATION"
            && let Some(value) = sample_rows.get(index).filter(|s| !s.is_null())
        {
            let mut value = value.clone();
            if let Some(total) = failed.as_ref().and_then(|s| s.parse::<u64>().ok()) {
                value["truncated"] = json!(
                    value["truncated"] == true
                        || total > value["rows"].as_array().map_or(0, Vec::len) as u64
                );
            }
            let key = content_digest(DigestKind::Compute, &value)
                .map_err(|_| Error::Evidence)?
                .hex();
            samples.insert(key.clone(), value);
            sample = json!(key);
        }
        outcomes.push(json!({"id":c["id"],"name":c["name"],"definition_digest":content_digest(DigestKind::Compute,c).map_err(|_|Error::Evidence)?.hex(),"status":status,"severity":c["on_error"],"exact":exact,"failed_rows":failed,"error":error,"metrics":metrics,"sample":sample}));
    }
    let result = json!({"format_version":1,"request_id":request["request_id"],"attempt_id":request["attempt_id"],"artifact_digest":digest,"subject_version":request["subject_version"],"consumer_definition":request["consumer_definition"],"binding":request["binding"],"phase":phase,"evaluator":"duckdb-1.5.5:core-v1","semantics":"strict-null-key-v1","started_us":started,"finished_us":finished,"duration_us":elapsed,"checks":outcomes,"sample_policy":policy});
    validate_document("CheckEvaluationResultV1", &result).map_err(|_| Error::Contract("result"))?;
    Ok(Completion {
        manifest: manifest.clone(),
        result,
        report,
        samples,
    })
}

#[cfg(test)]
mod tests;
