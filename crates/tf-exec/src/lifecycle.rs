//! One admitted attempt: exact input gates, one materialization, exact output gate.
//! Call from a bounded blocking owner; retain input leases and verified environment.
use crate::{
    admission::Reservation,
    checks::{self, Subject},
    ownership::RuntimeOwner,
    polars,
    supervisor::{Cancellation, Launch, Report},
    timing::{Budget, Phase, Work},
};
use serde_json::{Value, json};
use tf_domain::{AttemptId, RequestId};
use tf_protocol::{
    canonical::{DigestKind, content_digest},
    validate_document,
};
use tf_store::{
    artifacts::{ArtifactStore, Candidate, VerifiedArtifact},
    publication::{
        CheckSubject, PublicationContract, PublicationReceipt, PublicationRequest, gate_definition,
    },
};

/// A lifecycle refusal never authorizes publication.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The accepted bindings/check obligations and supplied execution disagree.
    #[error("Execution requires the complete frozen input and output check contract")]
    Contract,
    /// FAIL violations and every evaluator ERROR block their phase.
    #[error("Validation blocked this attempt; inspect its check results")]
    Gate,
    /// Cancellation before the next phase or publication.
    #[error("The attempt was canceled before publication")]
    Canceled,
    /// Canonical evaluation failed before result construction.
    #[error(transparent)]
    Checks(#[from] checks::Error),
    /// Materialization failed; the bounded worker report is retained separately.
    #[error(transparent)]
    Execution(#[from] polars::Error),
    /// Candidate integrity/durability failure.
    #[error(transparent)]
    Artifact(#[from] tf_store::artifacts::ArtifactError),
    /// Admission permit could not be transferred to the next helper.
    #[error(transparent)]
    Admission(#[from] crate::admission::Error),
    /// Publication authority or exact evidence was refused.
    #[error(transparent)]
    Publication(#[from] tf_store::publication::PublicationError),
    /// Ownership guard is no longer valid.
    #[error(transparent)]
    Ownership(#[from] crate::ownership::OwnershipError),
    /// Attempt timer state was lost.
    #[error(transparent)]
    Timing(#[from] crate::timing::TimerError),
    /// Retention also failed; preserve both causes.
    #[error("Publication failed ({original}); candidate retention also failed ({retention})")]
    Retention {
        /// Original publication refusal.
        original: Box<Error>,
        /// Independent retention failure.
        retention: tf_store::artifacts::ArtifactError,
    },
    /// Durable storage failed.
    #[error(transparent)]
    Store(#[from] tf_store::StoreError),
}
/// Accepted attempt inputs. Policies must already be resolved by the coordinator.
pub struct Request {
    /// One captured producer request; no worker permit may be checked out yet.
    pub execute: Launch,
    /// One ordered list per exact input alias, including empty lists.
    pub input_checks: Vec<Vec<Value>>,
    /// All resolved output checks.
    pub output_checks: Vec<Value>,
    /// Frozen accepted obligations, compared again at publication.
    pub contract: PublicationContract,
    /// Shared attempt ledger, injected into every helper and transform.
    pub budget: Budget,
    /// Unique private candidate directory.
    pub staging: RequestId,
}
/// Unforgeable successful gates over a still-owned candidate; consuming it publishes once.
pub struct Approved<'a> {
    candidate: Candidate<'a>,
    attempt: AttemptId,
    contract: String,
    results: Vec<Value>,
    cancel: Cancellation,
}
impl Approved<'_> {
    /// Exact checked artifact identity, used to form the domain publication intent.
    pub fn digest(&self) -> Result<tf_protocol::canonical::ContentDigest, Error> {
        Ok(self.candidate.digest()?)
    }
}
/// Failure context, with durable invisible candidate bytes when materialization completed.
pub struct Rejected {
    /// Phase that prevented further work.
    pub phase: Phase,
    /// Original refusal (retention failure, if any, is separate).
    pub error: Error,
    /// Sealed object without a version/head reference; never normal dataset data.
    pub candidate: Option<VerifiedArtifact>,
    /// A retention failure must remain visible rather than masking the original error.
    pub retention_error: Option<tf_store::artifacts::ArtifactError>,
}
/// Logs and contextual evidence survive a gate failure; no provider certificate is mutated.
pub struct Completion<'a> {
    /// Exact per-binding input results followed by candidate output results.
    pub evaluations: Vec<checks::Completion>,
    /// Producer report is absent if an input gate blocked execution.
    pub execution: Option<Report>,
    /// Nonexecuted output checks, with explicit SKIPPED reasons (never PASS).
    pub skipped: Vec<Value>,
    /// Only the successful branch can be passed to `publish`.
    pub outcome: Result<Approved<'a>, Rejected>,
}
fn fingerprint(check: &Value) -> Result<String, Error> {
    validate_document("DeclarationCheckV1", check).map_err(|_| Error::Contract)?;
    if check["null_policy"].is_null() || check["sample_rows"].is_null() {
        return Err(Error::Contract);
    }
    Ok(content_digest(DigestKind::Compute, check)
        .map_err(|_| Error::Contract)?
        .hex())
}
fn declarations(resolved: &[Value], declared: &Value) -> Result<(), Error> {
    let declared = declared.as_array().ok_or(Error::Contract)?;
    if declared.len() != resolved.len() {
        return Err(Error::Contract);
    }
    for (r, d) in resolved.iter().zip(declared) {
        for key in [
            "id",
            "name",
            "expectation",
            "on_error",
            "description",
            "null_policy",
            "sample_rows",
        ] {
            if !d[key].is_null() && r[key] != d[key] {
                return Err(Error::Contract);
            }
        }
    }
    Ok(())
}
fn preflight(r: &Request, inputs: &[VerifiedArtifact]) -> Result<AttemptId, Error> {
    r.contract.canonical()?;
    validate_document("PolarsExecutionRequestV1", &r.execute.request)
        .map_err(|_| Error::Contract)?;
    let request = &r.execute.request;
    let bindings = request["inputs"].as_array().ok_or(Error::Contract)?;
    let declared = request["producer"]["inputs"]
        .as_array()
        .ok_or(Error::Contract)?;
    if r.execute.reservation.is_some()
        || bindings.len() != inputs.len()
        || bindings.len() != r.input_checks.len()
        || bindings.len() != r.contract.inputs.len()
        || bindings.len() != declared.len()
    {
        return Err(Error::Contract);
    }
    let mut obligations = Vec::new();
    for ((((binding, input), checks), pinned), declaration) in bindings
        .iter()
        .zip(inputs)
        .zip(&r.input_checks)
        .zip(&r.contract.inputs)
        .zip(declared)
    {
        if binding["alias"] != pinned.alias
            || declaration["alias"] != pinned.alias
            || binding["version_id"] != pinned.version.to_string()
            || binding["dataset"]["workspace_id"] != pinned.dataset.workspace_id().to_string()
            || binding["dataset"]["dataset_id"] != pinned.dataset.dataset_id().to_string()
            || input.digest() != pinned.artifact
            || binding["artifact_digest"] != pinned.artifact.hex()
            || binding["manifest"] != *input.manifest()
        {
            return Err(Error::Contract);
        }
        declarations(checks, &declaration["checks"])?;
        for c in checks {
            obligations.push((
                gate_definition(&fingerprint(c)?, Some(&pinned.alias))?,
                Some(pinned.alias.as_str()),
                c["on_error"] == "FAIL",
            ));
        }
    }
    declarations(&r.output_checks, &request["producer"]["output"]["checks"])?;
    for c in &r.output_checks {
        obligations.push((
            gate_definition(&fingerprint(c)?, None)?,
            None,
            c["on_error"] == "FAIL",
        ));
    }
    if obligations.len() != r.contract.checks.len() {
        return Err(Error::Contract);
    }
    for (digest, alias, required) in obligations {
        if !r.contract.checks.iter().any(|c| {
            c.definition == digest
                && c.required == required
                && match &c.subject {
                    CheckSubject::Output => alias.is_none(),
                    CheckSubject::Input(a) => alias == Some(a.as_str()),
                }
        }) {
            return Err(Error::Contract);
        }
    }
    request["attempt_id"]
        .as_str()
        .ok_or(Error::Contract)?
        .parse()
        .map_err(|_| Error::Contract)
}
fn permits(result: &Value) -> bool {
    result["checks"].as_array().is_some_and(|checks| {
        checks.iter().all(|c| {
            c["exact"] == true
                && (c["status"] == "PASS"
                    || (c["status"] == "VIOLATION" && c["severity"] == "WARN"))
        })
    })
}
/// Run input checks, invoke/sink once only when allowed, then check the closed bytes.
/// `helper` allocates a fresh private request/result directory per group; all identity,
/// timing, threads and permits are supplied here, never re-resolved from current heads.
/// `enter` persists attempt phase boundaries before work and may refuse stale/canceled work.
/// Output validation begins only after materialization has returned closed bytes.
/// Caller retains ownership, input leases and the reservation throughout this blocking call.
pub fn run<'a>(
    artifacts: &'a ArtifactStore,
    mut request: Request,
    inputs: &[VerifiedArtifact],
    reservation: &Reservation,
    cancel: Cancellation,
    mut helper: impl FnMut(Phase, Option<&str>) -> Result<Launch, Error>,
    mut enter: impl FnMut(Phase) -> Result<(), Error>,
) -> Completion<'a> {
    let mut evaluations = Vec::new();
    let mut execution = None;
    let mut candidate = None;
    let mut phase = Phase::InputValidation;
    let mut output_started = false;
    let identity = preflight(&request, inputs);
    let budget = request.budget.clone();
    let output_checks = request.output_checks.clone();
    let outcome = (|| {
        let attempt = identity?;
        let consumer = content_digest(DigestKind::Compute, &request.execute.request["producer"])
            .map_err(|_| Error::Contract)?
            .hex();
        let mut evaluate = |phase,
                            alias: Option<&str>,
                            version: Option<String>,
                            subject,
                            checks: &[Value]|
         -> Result<checks::Completion, Error> {
            let mut launch = helper(phase, alias)?;
            launch.request["attempt_id"] = json!(attempt.to_string());
            launch.request["consumer_definition"] = json!(consumer);
            launch.request["phase"] = json!(if phase == Phase::InputValidation {
                "input"
            } else {
                "output"
            });
            launch.request["binding"] = json!(alias);
            launch.request["subject_version"] = json!(version);
            launch.timing = Some(Work {
                budget: request.budget.clone(),
                phase,
            });
            launch.reservation = Some(reservation.worker()?);
            let result = checks::run(launch, subject, checks, cancel.clone())?;
            Ok(result)
        };
        if cancel.requested() {
            return Err(Error::Canceled);
        }
        enter(phase)?;
        // Independent aliases are all evaluated even when an earlier alias violates policy.
        for ((input, checks), binding) in inputs
            .iter()
            .zip(&request.input_checks)
            .zip(&request.contract.inputs)
        {
            if !checks.is_empty() {
                evaluations.push(evaluate(
                    phase,
                    Some(&binding.alias),
                    Some(binding.version.to_string()),
                    Subject::Pinned(input),
                    checks,
                )?);
            }
        }
        if evaluations.iter().any(|e| !permits(&e.result)) {
            return Err(Error::Gate);
        }
        if cancel.requested() {
            return Err(Error::Canceled);
        }
        phase = Phase::Transform;
        enter(phase)?;
        request.execute.timing = Some(Work {
            budget: request.budget.clone(),
            phase,
        });
        request.execute.reservation = Some(reservation.worker()?);
        let materialized = polars::run(
            artifacts,
            request.execute,
            inputs,
            cancel.clone(),
            request.staging,
        );
        execution = materialized.report;
        candidate = Some(materialized.candidate?);
        phase = Phase::OutputValidation;
        enter(phase)?;
        if cancel.requested() {
            return Err(Error::Canceled);
        }
        let c = candidate.as_ref().ok_or(Error::Contract)?;
        if !request.output_checks.is_empty() {
            output_started = true;
            evaluations.push(evaluate(
                phase,
                None,
                None,
                Subject::Candidate(c),
                &request.output_checks,
            )?);
        }
        if evaluations.iter().any(|e| !permits(&e.result)) {
            return Err(Error::Gate);
        }
        if cancel.requested() {
            return Err(Error::Canceled);
        }
        Ok(Approved {
            candidate: candidate.take().ok_or(Error::Contract)?,
            attempt,
            contract: request.contract.canonical()?,
            results: evaluations.iter().map(|e| e.result.clone()).collect(),
            cancel: cancel.clone(),
        })
    })();
    let outcome = match budget.finish() {
        Ok(_) => outcome,
        Err(timer) => match outcome {
            Ok(approved) => {
                candidate = Some(approved.candidate);
                Err(Error::Timing(timer))
            }
            Err(original) => Err(original),
        },
    };
    let skipped = if outcome.is_err() && !output_started {
        output_checks
            .iter()
            .map(|c| json!({"id":c["id"],"status":"SKIPPED","reason":"upstream_phase_failed"}))
            .collect()
    } else {
        Vec::new()
    };
    let outcome = outcome.map_err(|error| {
        let retained = candidate.map(|c| c.install(|_| Ok(()))).transpose();
        let (candidate, retention_error) = match retained {
            Ok(c) => (c, None),
            Err(e) => (None, Some(e)),
        };
        Rejected {
            phase,
            error,
            candidate,
            retention_error,
        }
    });
    Completion {
        evaluations,
        execution,
        skipped,
        outcome,
    }
}
/// Publish exactly the approved candidate, without rematerializing or copying it again.
/// Invoke on the same bounded blocking owner as `run`; no async cancellation can release
/// its owner while filesystem/SQLite work is running. The dispatcher supplies legal domain
/// intent and persisted attempt phases. Store guards remain the final cancellation authority.
pub fn publish(
    owner: &mut RuntimeOwner,
    runtime: &tokio::runtime::Runtime,
    request: PublicationRequest,
    approved: Approved<'_>,
) -> Result<PublicationReceipt, Error> {
    let Approved {
        candidate,
        attempt,
        contract,
        results,
        cancel,
    } = approved;
    let mut candidate = Some(candidate);
    let result = (|| {
        let checked = candidate.as_ref().ok_or(Error::Contract)?;
        if owner.registration().mode() == crate::ownership::CoordinatorMode::MetadataOnly
            || owner.workspace_id()? != request.intent.target().dataset.workspace_id()
            || owner.registration().session()? != request.intent.fence().session
            || attempt != request.intent.attempt()
            || contract != request.contract.canonical()?
            || checked.digest()?.hex() != request.intent.artifact().hex()
        {
            return Err(Error::Contract);
        }
        if cancel.requested() {
            return Err(Error::Canceled);
        }
        owner.validate_paths()?;
        let artifacts = ArtifactStore::open(owner.workspace_root())?;
        let mut store = runtime.block_on(owner.open_store())?;
        runtime.block_on(store.repository()?.record_gate_results(&request, &results))?;
        runtime.block_on(store.repository()?.prepare_publication(&request, checked))?;
        let object = candidate
            .take()
            .ok_or(Error::Contract)?
            .install(|_| Ok(()))?;
        let object = artifacts.verify(object.digest())?;
        if cancel.requested() {
            return Err(Error::Canceled);
        }
        let receipt =
            runtime.block_on(store.repository()?.commit_publication(&request, &object))?;
        runtime.block_on(store.close())?;
        Ok(receipt)
    })();
    if result.is_err()
        && let Some(candidate) = candidate
    {
        let retained = candidate.install(|_| Ok(()));
        if let Err(retention) = retained {
            return Err(Error::Retention {
                original: Box::new(result.err().ok_or(Error::Contract)?),
                retention,
            });
        }
    }
    result
}
