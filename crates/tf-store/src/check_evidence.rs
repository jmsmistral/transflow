//! Complete immutable evidence. Sample rows are accessible only by an explicit diagnostic read.
use crate::{
    Store,
    artifacts::VerifiedArtifact,
    publication::{self, CheckRequirement, CheckSubject, PublicationContract, PublicationError},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{AttemptId, CoordinatorSessionId, RequestId, WorkspaceId, execution::Job};
use tf_protocol::{
    canonical::{DigestKind, artifact_digest, canonical_json, content_digest},
    validate_document,
};
type Result<T> = std::result::Result<T, PublicationError>;
/// Qualified normalization semantics, independent of SDK and database schema versions.
pub const NORMALIZATION: &str = "parquet-normalized-v1";
/// Accepted attempt identity; callers hold runtime ownership and subject leases.
pub struct Context<'a> {
    /// Frozen accepted job and reservation identity.
    pub job: &'a Job,
    /// Current attempt, never a cached-job fabricated attempt.
    pub attempt: AttemptId,
    /// Exact frozen obligations and input bindings.
    pub contract: &'a PublicationContract,
    /// Observation/reuse time; original evaluation timestamps remain separate.
    pub at_us: i64,
    /// Captured consumer identity expected by this execution.
    pub consumer_definition: &'a str,
    /// Expected diagnostic projection policy, frozen by the caller.
    pub sample_policy: &'a Value,
}
fn encode(v: &Value) -> Result<String> {
    String::from_utf8(canonical_json(v).map_err(|_| PublicationError::Evidence)?)
        .map_err(|_| PublicationError::Evidence)
}
fn digest(v: &Value) -> Result<String> {
    Ok(content_digest(DigestKind::Compute, v)
        .map_err(|_| PublicationError::Evidence)?
        .hex())
}
fn count(v: &Value) -> Result<i64> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .ok_or(PublicationError::Evidence)
}
/// Stable result identity within one attempt; a replay never replaces its row.
pub fn result_id(attempt: AttemptId, definition: &str) -> Result<RequestId> {
    let h = digest(&json!({"attempt":attempt.to_string(),"definition":definition}))?;
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
    .parse()
    .map_err(|_| PublicationError::Evidence)
}
fn policy(result: &Value) -> Value {
    result
        .get("sample_policy")
        .cloned()
        .unwrap_or_else(|| json!({"allowed_columns":[],"sensitive_columns":[],"max_rows":20}))
}
fn key(result: &Value, check: &Value, normalization: &str) -> Value {
    json!({"definition":check["definition_digest"],"artifact":result["artifact_digest"],"version":result["subject_version"],"phase":result["phase"],"binding":result["binding"],"consumer":result["consumer_definition"],"evaluator":result["evaluator"],"semantics":result["semantics"],"normalization":normalization,"sample_policy":policy(result)})
}
async fn guard(db: &mut SqliteConnection, c: &Context<'_>) -> Result<()> {
    publication::authority(
        db,
        c.job.target().dataset.workspace_id(),
        c.job.fence().session,
    )
    .await?;
    let row=sqlx::query("SELECT pc.contract_json FROM attempts a JOIN jobs j ON j.id=a.job_id JOIN builds b ON b.id=j.build_id JOIN build_plans p ON p.id=b.plan_id JOIN publication_contracts pc ON pc.job_id=j.id JOIN write_reservations r ON r.build_id=b.id AND r.dataset_id=j.dataset_id AND r.branch_id=j.branch_id AND r.session_id=a.session_id AND r.fence=a.fence WHERE a.id=? AND j.id=? AND b.id=? AND p.id=? AND p.source_snapshot_id=? AND p.disposition='ACCEPTED' AND a.session_id=? AND a.fence=? AND a.state!='SUCCEEDED' AND j.dataset_id=? AND j.branch_id=? AND r.expected_head_generation=?")
        .bind(c.attempt.to_string()).bind(c.job.id().to_string()).bind(c.job.build().to_string()).bind(c.job.binding().plan.to_string()).bind(c.job.binding().source.to_string()).bind(c.job.fence().session.to_string()).bind(i64::try_from(c.job.fence().generation).map_err(|_|PublicationError::Evidence)?)
        .bind(c.job.target().dataset.dataset_id().to_string()).bind(c.job.target().branch.to_string()).bind(i64::try_from(c.job.target().expected_generation).map_err(|_|PublicationError::Evidence)?)
        .fetch_optional(&mut *db).await?.ok_or(PublicationError::Fence)?;
    if row.try_get::<String, _>(0)? != c.contract.canonical()? || c.at_us < 0 {
        return Err(PublicationError::Evidence);
    }
    Ok(())
}
fn subject(c: &Context<'_>, result: &Value, definition: &str) -> Result<Value> {
    let requirement = c
        .contract
        .checks
        .iter()
        .find(|r| r.definition == definition)
        .ok_or(PublicationError::Evidence)?;
    match &requirement.subject {
        CheckSubject::Output
            if result["phase"] == "output"
                && result["binding"].is_null()
                && result["subject_version"].is_null() =>
        {
            Ok(json!({"artifact_digest":result["artifact_digest"]}))
        }
        CheckSubject::Input(alias) if result["phase"] == "input" && result["binding"] == *alias => {
            let input = c
                .contract
                .inputs
                .iter()
                .find(|i| &i.alias == alias)
                .ok_or(PublicationError::Evidence)?;
            if result["subject_version"] != input.version.to_string()
                || result["artifact_digest"] != input.artifact.hex()
            {
                return Err(PublicationError::Evidence);
            }
            Ok(
                json!({"alias":alias,"version_id":input.version.to_string(),"artifact_digest":input.artifact.hex()}),
            )
        }
        _ => Err(PublicationError::Evidence),
    }
}
/// Explicit diagnostic intent. This is not a normal dataset-preview or export API.
pub enum Inspection {
    /// User explicitly requested diagnostic content, including potential sensitive values.
    ExplicitDiagnostic,
}
impl Store {
    /// Register resolved declarations before freezing a publication contract. All writes
    /// are append-only; a conflicting definition can never change historical evidence.
    pub async fn register_check_definitions(
        &mut self,
        workspace: WorkspaceId,
        session: CoordinatorSessionId,
        alias: Option<&str>,
        checks: &[Value],
    ) -> Result<Vec<CheckRequirement>> {
        if checks.len() > 128 {
            return Err(PublicationError::Evidence);
        }
        let mut prepared = vec![];
        let mut ids = BTreeSet::new();
        for check in checks {
            validate_document("DeclarationCheckV1", check)
                .map_err(|_| PublicationError::Evidence)?;
            if !ids.insert(check["id"].to_string())
                || check["null_policy"].is_null()
                || check["sample_rows"].is_null()
            {
                return Err(PublicationError::Evidence);
            }
            let definition = publication::gate_definition(&digest(check)?, alias)?;
            let policy = json!({"severity":check["on_error"],"declaration":check});
            prepared.push((
                CheckRequirement {
                    definition,
                    subject: alias
                        .map(|a| CheckSubject::Input(a.into()))
                        .unwrap_or(CheckSubject::Output),
                    required: check["on_error"] == "FAIL",
                },
                encode(&check["expectation"])?,
                check["id"]
                    .as_str()
                    .ok_or(PublicationError::Evidence)?
                    .to_owned(),
                encode(&policy)?,
            ));
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        publication::authority(&mut tx, workspace, session).await?;
        for (r, ast, id, policy) in &prepared {
            sqlx::query(
                "INSERT INTO check_definitions VALUES(?,?,1,?,?,?,?) ON CONFLICT DO NOTHING",
            )
            .bind(&r.definition)
            .bind(ast)
            .bind(if alias.is_some() { "input" } else { "output" })
            .bind(alias)
            .bind(id)
            .bind(policy)
            .execute(&mut *tx)
            .await?;
            let existing=sqlx::query("SELECT ast_json,policy_json,stable_id,phase,target_alias FROM check_definitions WHERE fingerprint=?").bind(&r.definition).fetch_one(&mut *tx).await?;
            if existing.try_get::<String, _>(0)? != *ast
                || existing.try_get::<String, _>(1)? != *policy
                || existing.try_get::<String, _>(2)? != *id
                || existing.try_get::<String, _>(3)?
                    != if alias.is_some() { "input" } else { "output" }
                || existing.try_get::<Option<String>, _>(4)?.as_deref() != alias
            {
                return Err(PublicationError::Evidence);
            }
        }
        tx.commit().await?;
        Ok(prepared.into_iter().map(|v| v.0).collect())
    }
    /// Retain full PASS/VIOLATION/ERROR evidence and separate private samples, including
    /// failed consumer checks. No version/head/provider-certificate row is changed.
    pub async fn record_check_evaluation(
        &mut self,
        c: &Context<'_>,
        result: &Value,
        manifest: &Value,
        samples: &BTreeMap<String, Value>,
    ) -> Result<Vec<RequestId>> {
        validate_document("CheckEvaluationResultV1", result)
            .map_err(|_| PublicationError::Evidence)?;
        validate_document("ArtifactManifestV1", manifest)
            .map_err(|_| PublicationError::Evidence)?;
        if result["consumer_definition"] != c.consumer_definition
            || result["attempt_id"] != c.attempt.to_string()
            || result["artifact_digest"]
                != artifact_digest(manifest)
                    .map_err(|_| PublicationError::Evidence)?
                    .hex()
            || result["evaluator"] != "duckdb-1.5.5:core-v1"
            || result["semantics"] != "strict-null-key-v1"
        {
            return Err(PublicationError::Evidence);
        }
        let (started, finished) = (
            count(&result["started_us"])?,
            count(&result["finished_us"])?,
        );
        if started > finished || finished > c.at_us {
            return Err(PublicationError::Evidence);
        }
        let sample_policy = policy(result);
        if sample_policy != *c.sample_policy {
            return Err(PublicationError::Evidence);
        }
        validate_document("CheckSamplePolicyV1", &sample_policy)
            .map_err(|_| PublicationError::Evidence)?;
        let checks = result["checks"]
            .as_array()
            .ok_or(PublicationError::Evidence)?;
        let mut rows = vec![];
        let mut seen = BTreeSet::new();
        let mut sample_ids = BTreeSet::new();
        for check in checks {
            let definition = publication::gate_definition(
                check["definition_digest"]
                    .as_str()
                    .ok_or(PublicationError::Evidence)?,
                result["binding"].as_str(),
            )?;
            if !seen.insert(definition.clone()) {
                return Err(PublicationError::Evidence);
            }
            let required = c
                .contract
                .checks
                .iter()
                .find(|r| r.definition == definition)
                .ok_or(PublicationError::Evidence)?
                .required;
            if check["severity"] != if required { "FAIL" } else { "WARN" }
                || (check["status"] == "ERROR") != (check["exact"] == false)
                || (check["status"] == "ERROR") != !check["error"].is_null()
            {
                return Err(PublicationError::Evidence);
            }
            if let Some(sample) = check["sample"].as_str() {
                sample_ids.insert(sample.to_owned());
                let value = samples.get(sample).ok_or(PublicationError::Evidence)?;
                validate_document("CheckSampleV1", value)
                    .map_err(|_| PublicationError::Evidence)?;
                if check["status"] != "VIOLATION"
                    || digest(value)? != sample
                    || encode(value)?.len() > 65536
                    || value["limit"].as_u64() > sample_policy["max_rows"].as_u64()
                {
                    return Err(PublicationError::Evidence);
                }
                let cols = value["columns"]
                    .as_array()
                    .ok_or(PublicationError::Evidence)?;
                for col in cols {
                    if !sample_policy["allowed_columns"]
                        .as_array()
                        .ok_or(PublicationError::Evidence)?
                        .contains(&col["name"])
                        || sample_policy["sensitive_columns"]
                            .as_array()
                            .ok_or(PublicationError::Evidence)?
                            .contains(&col["name"])
                        || !manifest["logical_schema"]["fields"]
                            .as_array()
                            .ok_or(PublicationError::Evidence)?
                            .contains(col)
                    {
                        return Err(PublicationError::Evidence);
                    }
                }
                let sample_rows = value["rows"].as_array().ok_or(PublicationError::Evidence)?;
                if sample_rows.len()
                    > value["limit"].as_u64().ok_or(PublicationError::Evidence)? as usize
                {
                    return Err(PublicationError::Evidence);
                }
                for row in sample_rows {
                    let row = row.as_array().ok_or(PublicationError::Evidence)?;
                    if row.len() != cols.len() {
                        return Err(PublicationError::Evidence);
                    }
                    for (cell, col) in row.iter().zip(cols) {
                        let mut typ = cell.clone();
                        typ.as_object_mut()
                            .ok_or(PublicationError::Evidence)?
                            .remove("value");
                        if cell["type"] != "null" && typ != col["logical_type"] {
                            return Err(PublicationError::Evidence);
                        }
                    }
                }
            }
            let id = result_id(c.attempt, &definition)?;
            let subject = encode(&subject(c, result, &definition)?)?;
            rows.push((id, definition, subject, check));
        }
        let expected = c
            .contract
            .checks
            .iter()
            .filter(|r| match &r.subject {
                CheckSubject::Output => result["phase"] == "output",
                CheckSubject::Input(a) => result["phase"] == "input" && result["binding"] == *a,
            })
            .map(|r| r.definition.clone())
            .collect::<BTreeSet<_>>();
        if seen != expected {
            return Err(PublicationError::Evidence);
        }
        if samples.keys().cloned().collect::<BTreeSet<_>>() != sample_ids {
            return Err(PublicationError::Evidence);
        }
        let envelope = encode(result)?;
        if envelope.len() > 16 * 1024 * 1024 {
            return Err(PublicationError::Evidence);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        guard(&mut tx, c).await?;
        let evaluation = result["request_id"]
            .as_str()
            .ok_or(PublicationError::Evidence)?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT envelope_json FROM check_evaluations WHERE id=?")
                .bind(evaluation)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(existing) = existing {
            if existing != envelope {
                return Err(PublicationError::Evidence);
            }
            tx.commit().await?;
            return Ok(rows.iter().map(|r| r.0).collect());
        }
        sqlx::query("INSERT INTO check_evaluations VALUES(?,?,?,?)")
            .bind(evaluation)
            .bind(c.attempt.to_string())
            .bind(envelope)
            .bind(NORMALIZATION)
            .execute(&mut *tx)
            .await?;
        for (hash, value) in samples {
            sqlx::query("INSERT INTO check_samples VALUES(?,?) ON CONFLICT DO NOTHING")
                .bind(hash)
                .bind(encode(value)?)
                .execute(&mut *tx)
                .await?;
        }
        for (id, definition, subject, check) in &rows {
            let policy: String =
                sqlx::query_scalar("SELECT policy_json FROM check_definitions WHERE fingerprint=?")
                    .bind(definition)
                    .fetch_one(&mut *tx)
                    .await?;
            let declared: Value =
                serde_json::from_str(&policy).map_err(|_| PublicationError::Evidence)?;
            if digest(&declared["declaration"])? != check["definition_digest"]
                || declared["declaration"]["id"] != check["id"]
                || declared["declaration"]["name"] != check["name"]
            {
                return Err(PublicationError::Evidence);
            }
            if let Some(hash) = check["sample"].as_str() {
                let requested = count(&declared["declaration"]["sample_rows"])?;
                if requested == 0
                    || samples[hash]["limit"]
                        .as_i64()
                        .ok_or(PublicationError::Evidence)?
                        > requested
                {
                    return Err(PublicationError::Evidence);
                }
            }
            let already:i64=sqlx::query_scalar("SELECT count(*) FROM check_results WHERE attempt_id=? AND definition_fingerprint=?").bind(c.attempt.to_string()).bind(definition).fetch_one(&mut *tx).await?;
            if already != 0 {
                return Err(PublicationError::Evidence);
            }
            sqlx::query("INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,outcome,metrics_json,started_at_us,finished_at_us,sample_ref) VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(id.to_string()).bind(c.attempt.to_string()).bind(subject).bind(definition).bind(check["status"].as_str()).bind(encode(check)?).bind(started).bind(finished).bind(check["sample"].as_str()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO check_evidence VALUES(?,?)")
                .bind(id.to_string())
                .bind(evaluation)
                .execute(&mut *tx)
                .await?;
            if check["status"] == "PASS"
                || (check["status"] == "VIOLATION" && check["severity"] == "WARN")
            {
                let key = key(result, check, NORMALIZATION);
                sqlx::query("INSERT INTO check_certificates VALUES(?,?,?) ON CONFLICT DO NOTHING")
                    .bind(digest(&key)?)
                    .bind(id.to_string())
                    .bind(encode(&key)?)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(rows.iter().map(|r| r.0).collect())
    }
    /// Persist a sealed failed candidate as diagnostic evidence only. It becomes a
    /// retention root, never a dataset version or ordinary preview target.
    pub async fn record_failed_check_candidate(
        &mut self,
        c: &Context<'_>,
        artifact: &VerifiedArtifact,
    ) -> Result<()> {
        let manifest = artifact.manifest();
        let files = manifest["files"]
            .as_array()
            .ok_or(PublicationError::Evidence)?;
        let sum = |name: &str| {
            files.iter().try_fold(0i64, |n, f| {
                n.checked_add(count(&f[name])?)
                    .ok_or(PublicationError::Evidence)
            })
        };
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        guard(&mut tx, c).await?;
        let published: i64 =
            sqlx::query_scalar("SELECT count(*) FROM dataset_versions WHERE attempt_id=?")
                .bind(c.attempt.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if published != 0 {
            return Err(PublicationError::Evidence);
        }
        sqlx::query("INSERT INTO artifacts(digest,manifest_json,schema_json,file_count,row_count,byte_count,integrity_state) VALUES(?,?,?,?,?,?,'VERIFIED') ON CONFLICT DO NOTHING")
            .bind(artifact.digest().hex()).bind(encode(manifest)?).bind(encode(&manifest["logical_schema"])?).bind(i64::try_from(files.len()).map_err(|_|PublicationError::Evidence)?).bind(sum("row_count")?).bind(sum("byte_length")?).execute(&mut *tx).await?;
        let stored: String =
            sqlx::query_scalar("SELECT manifest_json FROM artifacts WHERE digest=?")
                .bind(artifact.digest().hex())
                .fetch_one(&mut *tx)
                .await?;
        if stored != encode(manifest)? {
            return Err(PublicationError::Evidence);
        }
        sqlx::query("INSERT INTO failed_check_candidates VALUES(?,?) ON CONFLICT DO NOTHING")
            .bind(c.attempt.to_string())
            .bind(artifact.digest().hex())
            .execute(&mut *tx)
            .await?;
        let stored: String = sqlx::query_scalar(
            "SELECT artifact_digest FROM failed_check_candidates WHERE attempt_id=?",
        )
        .bind(c.attempt.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if stored != artifact.digest().hex() {
            return Err(PublicationError::Evidence);
        }
        tx.commit().await?;
        Ok(())
    }
    /// Safe evidence view: original envelope/times and explicit reuse time, with sample
    /// digests only. Does not access sample payloads or failed candidate file paths.
    pub async fn check_evidence(&mut self, id: RequestId) -> Result<Value> {
        let row=sqlx::query("SELECT r.attempt_id,e.envelope_json,u.original_result_id,u.reused_at_us FROM check_results r LEFT JOIN check_reuses u ON u.result_id=r.id JOIN check_evidence d ON d.result_id=coalesce(u.original_result_id,r.id) JOIN check_evaluations e ON e.id=d.evaluation_id WHERE r.id=?").bind(id.to_string()).fetch_optional(&mut self.db).await?.ok_or(PublicationError::Evidence)?;
        let evaluation: Value = serde_json::from_str(&row.try_get::<String, _>(1)?)
            .map_err(|_| PublicationError::Evidence)?;
        Ok(
            json!({"result_id":id.to_string(),"attempt_id":row.try_get::<String,_>(0)?,"evaluation":evaluation,"original_result_id":row.try_get::<Option<String>,_>(2)?,"reused_at_us":row.try_get::<Option<i64>,_>(3)?.map(|n|n.to_string())}),
        )
    }
    /// Explicit opt-in diagnostic read. Normal evidence, logs and export surfaces use
    /// `check_evidence`, which does not load or return sample rows.
    pub async fn inspect_check_sample(
        &mut self,
        id: RequestId,
        _intent: Inspection,
    ) -> Result<Value> {
        let payload:Option<String>=sqlx::query_scalar("SELECT s.payload_json FROM check_results r JOIN check_samples s ON s.digest=r.sample_ref WHERE r.id=?").bind(id.to_string()).fetch_optional(&mut self.db).await?;
        let sample = payload
            .map(|s| serde_json::from_str::<Value>(&s).map_err(|_| PublicationError::Evidence))
            .transpose()?;
        Ok(
            json!({"warning":"Explicit check diagnostics may contain data values; this is not normal dataset preview.","sample":sample}),
        )
    }
    /// Explicit inspection of quarantined candidate metadata, never a moving dataset head.
    pub async fn inspect_failed_check_candidate(
        &mut self,
        attempt: AttemptId,
        _intent: Inspection,
    ) -> Result<Value> {
        let row=sqlx::query("SELECT f.artifact_digest,a.manifest_json FROM failed_check_candidates f JOIN artifacts a ON a.digest=f.artifact_digest WHERE f.attempt_id=?").bind(attempt.to_string()).fetch_optional(&mut self.db).await?.ok_or(PublicationError::Evidence)?;
        let manifest: Value = serde_json::from_str(&row.try_get::<String, _>(1)?)
            .map_err(|_| PublicationError::Evidence)?;
        Ok(
            json!({"warning":"This failed attempt candidate is unpublished and may violate checks.","artifact_digest":row.try_get::<String,_>(0)?,"manifest":manifest}),
        )
    }
}
/// Expected immutable input semantics. Changing any field makes the certificate miss.
pub struct InputCertificateQuery<'a> {
    /// Exact alias-qualified retained version and physical artifact.
    pub input: &'a publication::InputProvenance,
    /// Effective declaration including resolved null/sample/severity policy.
    pub check: &'a Value,
    /// Captured consumer definition, not a changing producer lookup.
    pub consumer_definition: &'a str,
    /// Engine and semantic version identities.
    pub evaluator: &'a str,
    /// Exact check semantics.
    pub semantics: &'a str,
    /// Exact Arrow/Parquet normalization semantics.
    pub normalization: &'a str,
    /// Frozen allowed/non-sensitive sampling policy.
    pub sample_policy: &'a Value,
}
/// A retained acceptable evaluation; construction requires exact key and checked bytes.
pub struct Certificate {
    fingerprint: String,
    original: RequestId,
    key: Value,
    definition: String,
}
impl Certificate {
    /// Original evaluation result, unchanged by reuse.
    pub fn original_result(&self) -> RequestId {
        self.original
    }
}
impl Store {
    /// Lookup only pure checks over exact pinned inputs. Call after strict full-byte
    /// verification with the input lease held. Policy/evaluator changes are ordinary misses.
    pub async fn lookup_input_certificate(
        &mut self,
        workspace: WorkspaceId,
        session: CoordinatorSessionId,
        q: &InputCertificateQuery<'_>,
        verified: &VerifiedArtifact,
    ) -> Result<Option<Certificate>> {
        validate_document("DeclarationCheckV1", q.check).map_err(|_| PublicationError::Evidence)?;
        validate_document("CheckSamplePolicyV1", q.sample_policy)
            .map_err(|_| PublicationError::Evidence)?;
        validate_document("Sha256", &json!(q.consumer_definition))
            .map_err(|_| PublicationError::Evidence)?;
        if q.input.artifact != verified.digest()
            || q.check["null_policy"].is_null()
            || q.check["sample_rows"].is_null()
        {
            return Err(PublicationError::Evidence);
        }
        let declaration = digest(q.check)?;
        let definition = publication::gate_definition(&declaration, Some(&q.input.alias))?;
        let value = json!({"definition":declaration,"artifact":q.input.artifact.hex(),"version":q.input.version.to_string(),"phase":"input","binding":q.input.alias,"consumer":q.consumer_definition,"evaluator":q.evaluator,"semantics":q.semantics,"normalization":q.normalization,"sample_policy":q.sample_policy});
        let fingerprint = digest(&value)?;
        let mut tx = self.db.begin().await?;
        publication::authority(&mut tx, workspace, session).await?;
        publication::validate_inputs(
            &mut tx,
            workspace,
            &PublicationContract {
                compute_fingerprint: "0".repeat(64),
                check_fingerprint: "0".repeat(64),
                inputs: vec![q.input.clone()],
                checks: vec![],
            },
        )
        .await?;
        let row=sqlx::query("SELECT c.result_id,c.key_json,r.outcome,d.policy_json FROM check_certificates c JOIN check_results r ON r.id=c.result_id JOIN check_definitions d ON d.fingerprint=r.definition_fingerprint WHERE c.fingerprint=? AND r.definition_fingerprint=?")
            .bind(&fingerprint).bind(&definition).fetch_optional(&mut *tx).await?;
        let found = if let Some(row) = row {
            let outcome: String = row.try_get(2)?;
            let policy: Value = serde_json::from_str(&row.try_get::<String, _>(3)?)
                .map_err(|_| PublicationError::Evidence)?;
            if row.try_get::<String, _>(1)? != encode(&value)?
                || policy["declaration"] != *q.check
                || !(outcome == "PASS" || (outcome == "VIOLATION" && q.check["on_error"] == "WARN"))
            {
                return Err(PublicationError::Evidence);
            }
            Some(Certificate {
                fingerprint,
                original: row
                    .try_get::<String, _>(0)?
                    .parse()
                    .map_err(|_| PublicationError::Evidence)?,
                key: value,
                definition,
            })
        } else {
            None
        };
        tx.commit().await?;
        Ok(found)
    }
    /// Reuse creates an explicit occurrence and a current-attempt association, while
    /// preserving original status, metrics, sample and evaluation timestamps verbatim.
    pub async fn reuse_input_certificate(
        &mut self,
        c: &Context<'_>,
        certificate: Certificate,
    ) -> Result<RequestId> {
        let id = result_id(c.attempt, &certificate.definition)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        guard(&mut tx, c).await?;
        let canceled: i64 = sqlx::query_scalar("SELECT cancel_requested FROM builds WHERE id=?")
            .bind(c.job.build().to_string())
            .fetch_one(&mut *tx)
            .await?;
        if canceled != 0 {
            return Err(PublicationError::Canceled);
        }
        let context: String = sqlx::query_scalar("SELECT context_json FROM build_plans WHERE id=?")
            .bind(c.job.binding().plan.to_string())
            .fetch_one(&mut *tx)
            .await?;
        let context: Value =
            serde_json::from_str(&context).map_err(|_| PublicationError::Evidence)?;
        if context["force"] == true
            || context["source_decisions"][c.job.target().dataset.dataset_id().to_string()]["executes"]
                == true
        {
            return Err(PublicationError::Evidence);
        }
        let obligation = c
            .contract
            .checks
            .iter()
            .find(|r| r.definition == certificate.definition)
            .ok_or(PublicationError::Evidence)?;
        let alias = match &obligation.subject {
            CheckSubject::Input(a) => a,
            _ => return Err(PublicationError::Evidence),
        };
        let input = c
            .contract
            .inputs
            .iter()
            .find(|i| &i.alias == alias)
            .ok_or(PublicationError::Evidence)?;
        if certificate.key["sample_policy"] != *c.sample_policy
            || certificate.key["consumer"] != c.consumer_definition
            || certificate.key["binding"] != *alias
            || certificate.key["version"] != input.version.to_string()
            || certificate.key["artifact"] != input.artifact.hex()
        {
            return Err(PublicationError::Evidence);
        }
        publication::validate_inputs(&mut tx, c.job.target().dataset.workspace_id(), c.contract)
            .await?;
        let old=sqlx::query("SELECT r.subject_json,r.outcome,r.metrics_json,r.started_at_us,r.finished_at_us,r.sample_ref FROM check_results r JOIN check_certificates cert ON cert.result_id=r.id WHERE r.id=? AND cert.fingerprint=? AND cert.key_json=?")
            .bind(certificate.original.to_string()).bind(&certificate.fingerprint).bind(encode(&certificate.key)?).fetch_optional(&mut *tx).await?.ok_or(PublicationError::Evidence)?;
        let outcome: String = old.try_get(1)?;
        let finish: i64 = old.try_get(4)?;
        if finish > c.at_us
            || !(outcome == "PASS" || (outcome == "VIOLATION" && !obligation.required))
        {
            return Err(PublicationError::Evidence);
        }
        let existing: Option<(String, i64)> = sqlx::query_as(
            "SELECT original_result_id,reused_at_us FROM check_reuses WHERE result_id=?",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((original, at)) = existing {
            if original != certificate.original.to_string() || at > c.at_us {
                return Err(PublicationError::Evidence);
            }
            tx.commit().await?;
            return Ok(id);
        }
        let existing: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM check_results WHERE attempt_id=? AND definition_fingerprint=?",
        )
        .bind(c.attempt.to_string())
        .bind(&certificate.definition)
        .fetch_one(&mut *tx)
        .await?;
        if existing != 0 {
            return Err(PublicationError::Evidence);
        }
        sqlx::query("INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,outcome,metrics_json,started_at_us,finished_at_us,sample_ref) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(id.to_string()).bind(c.attempt.to_string()).bind(old.try_get::<String,_>(0)?).bind(&certificate.definition).bind(outcome).bind(old.try_get::<String,_>(2)?).bind(old.try_get::<i64,_>(3)?).bind(finish).bind(old.try_get::<Option<String>,_>(5)?).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO check_reuses VALUES(?,?,?,?)")
            .bind(id.to_string())
            .bind(certificate.original.to_string())
            .bind(certificate.fingerprint)
            .bind(c.at_us)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(id)
    }
}
