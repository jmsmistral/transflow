//! Native single-attempt gate qualification, using real SQLite, Parquet and wheel workers.
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    error::Error,
    fs::{self, File},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tf_domain::{execution::*, *};
use tf_exec::{
    admission::{Admission, Capacity},
    lifecycle,
    supervisor::{Cancellation, Launch, Policy},
    timing::{Budget, Limits, Phase as TimedPhase},
};
use tf_protocol::Operation;
use tf_store::{
    artifacts::ArtifactStore,
    publication::{CheckRequirement, CheckSubject, InputProvenance, InputRole, PublicationRequest},
};
#[path = "../tests/support/publication.rs"]
mod fixture;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let root = PathBuf::from(args.first().ok_or("workspace")?);
    let python = PathBuf::from(args.get(1).ok_or("worker")?);
    let mut request: Value = serde_json::from_reader(File::open(args.get(2).ok_or("request")?)?)?;
    let mode = args.get(3).and_then(|s| s.to_str()).unwrap_or("normal");
    let rt = fixture::runtime();
    let mut h = rt.block_on(fixture::Harness::at(root));
    for (n, d) in [(10, 10), (11, 20)] {
        let mut baseline_contract = fixture::contract();
        if n == 10 {
            rt.block_on(sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'output',NULL,'provider','{\"severity\":\"FAIL\"}')").bind("f".repeat(64)).execute(&mut h.db))?;
            baseline_contract.checks.push(CheckRequirement {
                definition: "f".repeat(64),
                subject: CheckSubject::Output,
                required: true,
            });
        }
        let baseline = rt.block_on(h.start(n, d, 0, baseline_contract));
        if n == 10 {
            rt.block_on(sqlx::query("INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,outcome,metrics_json,started_at_us,finished_at_us) VALUES(?,?,?,?,'PASS','{}',5,10)")
                .bind(RequestId::from_bytes([100;16]).to_string()).bind(baseline.intent.attempt().to_string())
                .bind(json!({"artifact_digest":h.digest.hex()}).to_string()).bind("f".repeat(64)).execute(&mut h.db))?;
        }
        rt.block_on(h.publish(baseline, n))?;
        rt.block_on(h.finish(n, "SUCCEEDED"));
    }
    let artifacts = ArtifactStore::open(&h.root)?;
    let mut contract = fixture::contract();
    let mut inputs = vec![];
    let mut input_checks = vec![];
    let declarations = request["producer"]["inputs"]
        .as_array()
        .ok_or("inputs")?
        .clone();
    let mut bindings = vec![];
    for declaration in &declarations {
        let alias = declaration["alias"].as_str().ok_or("alias")?;
        let verified = artifacts.verify(h.digest)?;
        let hex = h.digest.hex();
        bindings.push(json!({"alias":alias,"dataset":{"workspace_id":fixture::workspace().to_string(),"dataset_id":DatasetId::from_bytes([10;16]).to_string()},"version_id":VersionId::from_bytes([10;16]).to_string(),"artifact_digest":hex,"manifest":verified.manifest(),"artifact_root":h.root.join(".transflow/runtime/objects").join(&hex[..2]).join(&hex)}));
        inputs.push(verified);
        input_checks.push(declaration["checks"].as_array().ok_or("checks")?.clone());
        contract.inputs.push(InputProvenance {
            alias: alias.into(),
            dataset: DatasetKey::new(fixture::workspace(), DatasetId::from_bytes([10; 16])),
            version: VersionId::from_bytes([10; 16]),
            artifact: h.digest,
            declared_branch: json!({"kind":"omitted","name":null}),
            starting_branch: "master".parse()?,
            resolved_branch: "master".parse()?,
            role: InputRole::Data,
            resolution: json!({}),
        });
    }
    request["inputs"] = json!(bindings);
    let attempt = AttemptId::from_bytes([20; 16]);
    request["attempt_id"] = json!(attempt.to_string());
    request["context"]["build_id"] = json!(BuildId::from_bytes([20; 16]).to_string());
    request["context"]["job_id"] = json!(JobId::from_bytes([20; 16]).to_string());
    let output_checks = request["producer"]["output"]["checks"]
        .as_array()
        .ok_or("outputs")?
        .clone();
    for (alias, checks) in contract
        .inputs
        .iter()
        .map(|i| Some(i.alias.as_str()))
        .zip(&input_checks)
        .chain(std::iter::once((None, &output_checks)))
    {
        let session = h.session();
        let mut store = rt.block_on(h.owner.as_mut().ok_or("owner")?.open_store())?;
        contract
            .checks
            .extend(rt.block_on(store.repository()?.register_check_definitions(
                fixture::workspace(),
                session,
                alias,
                checks,
            ))?);
        rt.block_on(store.close())?;
    }
    let seeded = rt.block_on(h.seed(20, 20));
    let target = OutputTarget {
        dataset: DatasetKey::new(fixture::workspace(), DatasetId::from_bytes([20; 16])),
        branch: fixture::branch(),
        expected_generation: 1,
    };
    let mut job = Job::new(
        seeded.id(),
        seeded.build(),
        seeded.binding(),
        target,
        seeded.fence(),
        RetryPolicy::default(),
        EventTime(0),
    );
    {
        let mut store = rt.block_on(h.owner.as_mut().ok_or("owner")?.open_store())?;
        rt.block_on(store.repository()?.reserve_publications(
            fixture::workspace(),
            job.build(),
            job.fence(),
            &[target],
            2,
        ))?;
        rt.block_on(store.repository()?.freeze_publication_contract(
            fixture::workspace(),
            job.fence().session,
            job.id(),
            &contract,
        ))?;
        rt.block_on(store.close())?;
    }
    job.queue(job.fence(), EventTime(1))?;
    job.start_attempt(attempt, job.fence(), EventTime(2))?;
    let pool = Admission::new(Capacity {
        jobs: 1,
        cpu: 1,
        ..Capacity::default()
    })?;
    let reservation = rt.block_on(pool.request(pool.default_demand())?)?;
    let cancel = Cancellation::default();
    let check_root = h.root.join("checks");
    fs::create_dir(&check_root)?;
    let launch = |operation, request_schema, request| Launch {
        python: python.clone(),
        operation,
        request_schema,
        phase_events: None,
        request,
        policy: Policy::default(),
        log_directory: None,
        redact: vec![],
        timing: None,
        reservation: None,
        threads: 1,
    };
    let consumer = tf_protocol::canonical::content_digest(
        tf_protocol::canonical::DigestKind::Compute,
        &request["producer"],
    )?
    .hex();
    let execute = launch(Operation::Execute, "PolarsExecutionRequestV1", request);
    let mut requested = lifecycle::Request {
        execute,
        input_checks,
        output_checks,
        contract: contract.clone(),
        budget: Budget::new(Limits::default(), "synthetic lifecycle".into())?,
        staging: tf_exec::discovery::random_id()?.parse()?,
    };
    if mode == "omitted" {
        requested.output_checks.clear();
    }
    if mode == "wrong-version" {
        requested.execute.request["inputs"][0]["version_id"] =
            json!(VersionId::from_bytes([99; 16]).to_string());
    }
    if mode == "canceled" {
        cancel.cancel();
    }
    let mut phases = vec![];
    let mut helper_count = 0;
    let mut current = Phase::Starting;
    let completion = lifecycle::run(
        &artifacts,
        requested,
        &inputs,
        &reservation,
        cancel.clone(),
        |phase, alias| {
            helper_count += 1;
            let dir = check_root.join(helper_count.to_string());
            fs::create_dir(&dir).map_err(tf_store::artifacts::ArtifactError::from)?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                .map_err(tf_store::artifacts::ArtifactError::from)?;
            if mode == "helper-error" && phase == TimedPhase::OutputValidation {
                return Ok(Launch {
                    python: PathBuf::from("/no-such-worker"),
                    ..launch(
                        Operation::EvaluateChecks,
                        "CheckEvaluationRequestV1",
                        json!({"format_version":1,"protocol":{"major":1,"minor":0},"request_id":tf_exec::discovery::random_id().map_err(tf_store::artifacts::ArtifactError::from)?,"auth_token":"0".repeat(64),"result_directory":dir,"memory_bytes":null,"spill_bytes":"268435456"}),
                    )
                });
            }
            let _ = alias;
            Ok(launch(
                Operation::EvaluateChecks,
                "CheckEvaluationRequestV1",
                json!({"format_version":1,"protocol":{"major":1,"minor":0},"request_id":tf_exec::discovery::random_id().map_err(tf_store::artifacts::ArtifactError::from)?,"auth_token":"0".repeat(64),"result_directory":dir,"memory_bytes":null,"spill_bytes":"268435456"}),
            ))
        },
        |event| {
            let phase = match event {
                lifecycle::Event::Enter(phase) => phase,
                lifecycle::Event::Evaluated(e) => {
                    let context = tf_store::check_evidence::Context {
                        consumer_definition: &consumer,
                        sample_policy: &json!({"allowed_columns":[],"sensitive_columns":[],"max_rows":20}),
                        job: &job,
                        attempt,
                        contract: &contract,
                        at_us: i64::try_from(
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map_err(|_| lifecycle::Error::Contract)?
                                .as_micros(),
                        )
                        .map_err(|_| lifecycle::Error::Contract)?,
                    };
                    let mut store = rt.block_on(
                        h.owner
                            .as_mut()
                            .ok_or(lifecycle::Error::Contract)?
                            .open_store(),
                    )?;
                    rt.block_on(store.repository()?.record_check_evaluation(
                        &context,
                        &e.result,
                        &e.manifest,
                        &e.samples,
                    ))?;
                    rt.block_on(store.close())?;
                    return Ok(());
                }
                lifecycle::Event::Rejected(f) => {
                    if let Some(candidate) = &f.candidate {
                        let context = tf_store::check_evidence::Context {
                            consumer_definition: &consumer,
                            sample_policy: &json!({"allowed_columns":[],"sensitive_columns":[],"max_rows":20}),
                            job: &job,
                            attempt,
                            contract: &contract,
                            at_us: i64::try_from(
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .map_err(|_| lifecycle::Error::Contract)?
                                    .as_micros(),
                            )
                            .map_err(|_| lifecycle::Error::Contract)?,
                        };
                        let mut store = rt.block_on(
                            h.owner
                                .as_mut()
                                .ok_or(lifecycle::Error::Contract)?
                                .open_store(),
                        )?;
                        rt.block_on(
                            store
                                .repository()?
                                .record_failed_check_candidate(&context, candidate),
                        )?;
                        rt.block_on(store.close())?;
                    }
                    return Ok(());
                }
            };
            phases.push(phase.name());
            let transitions: &[Phase] = match phase {
                TimedPhase::InputValidation => &[Phase::ValidatingInputs],
                TimedPhase::Transform => &[Phase::Running],
                TimedPhase::OutputValidation => &[Phase::Materializing, Phase::ValidatingOutputs],
                _ => return Err(lifecycle::Error::Contract),
            };
            for next in transitions {
                let mut store = rt.block_on(
                    h.owner
                        .as_mut()
                        .ok_or(lifecycle::Error::Contract)?
                        .open_store(),
                )?;
                rt.block_on(
                    store
                        .repository()?
                        .advance_gate_phase(&job, attempt, current, *next),
                )?;
                rt.block_on(store.close())?;
                job.advance(
                    attempt,
                    job.fence(),
                    *next,
                    EventTime(job.last_event().0 + 1),
                )
                .map_err(|_| lifecycle::Error::Contract)?;
                current = *next;
            }
            if mode == "cancel-after-materialize" && phase == TimedPhase::OutputValidation {
                cancel.cancel();
            }
            Ok(())
        },
    );
    if let Some(e) = completion.evidence_error {
        return Err(e.into());
    }
    let results: Vec<_> = completion
        .evaluations
        .iter()
        .map(|e| e.result.clone())
        .collect();
    let mut candidate = None;
    let mut candidate_readable = false;
    let outcome = match completion.outcome {
        Err(f) => {
            if let Some(c) = f.candidate {
                candidate = Some(c.digest().hex());
                candidate_readable = artifacts.verify(c.digest()).is_ok();
            }
            json!({"published":false,"error":f.error.to_string(),"phase":f.phase.name(),"retention_error":f.retention_error.map(|e|e.to_string())})
        }
        Ok(approved) => {
            let digest = approved.digest()?;
            candidate = Some(digest.hex());
            if mode == "conflict" {
                rt.block_on(
                    sqlx::query("UPDATE dataset_heads SET generation=2 WHERE dataset_id=?")
                        .bind(DatasetId::from_bytes([20; 16]).to_string())
                        .execute(&mut h.db),
                )?;
            }
            if mode == "cancel-before-publish" {
                cancel.cancel();
            }
            if mode == "persisted-cancel" {
                rt.block_on(
                    sqlx::query("UPDATE builds SET cancel_requested=1 WHERE id=?")
                        .bind(job.build().to_string())
                        .execute(&mut h.db),
                )?;
            }
            if mode == "tamper" {
                let staging = fs::read_dir(h.root.join(".transflow/runtime/objects"))?
                    .filter_map(Result::ok)
                    .find(|e| e.file_name().to_string_lossy().starts_with(".staging-"))
                    .ok_or("staging")?
                    .path();
                fixture::writable(&staging);
                fs::write(staging.join("part-00000.parquet"), b"changed")?;
            }
            let intent = job.prepare_publication(
                attempt,
                job.fence(),
                VersionId::from_bytes([20; 16]),
                digest.hex().parse()?,
                EventTime(10),
            )?;
            let request = PublicationRequest {
                intent,
                contract,
                at_us: i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros())?,
            };
            let published =
                lifecycle::publish(h.owner.as_mut().ok_or("owner")?, &rt, request, approved);
            candidate_readable = artifacts.verify(digest).is_ok();
            match published {
                Ok(r) => json!({"published":true,"generation":r.generation}),
                Err(e) => json!({"published":false,"error":e.to_string()}),
            }
        }
    };
    let head: String = rt.block_on(
        sqlx::query_scalar("SELECT version_id FROM dataset_heads WHERE dataset_id=?")
            .bind(DatasetId::from_bytes([20; 16]).to_string())
            .fetch_one(&mut h.db),
    )?;
    let linked:Vec<_>=rt.block_on(sqlx::query("SELECT r.outcome,r.subject_json,r.metrics_json FROM check_results r JOIN version_check_results v ON v.result_id=r.id WHERE v.version_id=?").bind(VersionId::from_bytes([20;16]).to_string()).fetch_all(&mut h.db))?.into_iter().map(|r|json!({"outcome":r.get::<String,_>(0),"subject":r.get::<String,_>(1),"metrics":r.get::<String,_>(2)})).collect();
    let provider_checks=rt.block_on(h.scalar("SELECT count(*) FROM version_check_results v JOIN check_results r ON r.id=v.result_id WHERE v.version_id='0a0a0a0a-0a0a-0a0a-0a0a-0a0a0a0a0a0a' AND r.outcome='PASS' AND r.metrics_json='{}'"));
    let state: String = rt.block_on(
        sqlx::query_scalar("SELECT state FROM attempts WHERE id=?")
            .bind(attempt.to_string())
            .fetch_one(&mut h.db),
    )?;
    let retained_checks = rt.block_on(h.scalar("SELECT count(*) FROM check_evidence"));
    let failed_candidates = rt.block_on(h.scalar("SELECT count(*) FROM failed_check_candidates"));
    println!(
        "{}",
        json!({"retained_checks":retained_checks,"failed_candidates":failed_candidates,"outcome":outcome,"results":results,"producer_ran":completion.execution.is_some(),"skipped":completion.skipped,"candidate":candidate,"candidate_readable":candidate_readable,"head":head,"linked":linked,"provider_checks":provider_checks,"phases":phases,"attempt_state":state,"versions":rt.block_on(h.scalar("SELECT count(*) FROM dataset_versions"))})
    );
    rt.block_on(h.handoff());
    Ok(())
}
