//! Committed event adapters. One bounded schedule scan commits tokens and its cursor together.
use crate::{Store, StoreError, retention};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use tf_domain::{
    BranchName, BuildId, DatasetKey, RequestId, ScheduleId, ScheduleOccurrenceId, VersionId,
    WorkspaceId,
};
use tf_protocol::schedule::{BuildTemplate, Definition, PayloadMode, Trigger};

type Result<T> = crate::Result<T>;
fn invalid() -> StoreError {
    StoreError::InvalidRequest
}

/// Dataset event semantics are independent of whether an immutable version is reused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetCause {
    /// Newly published, checked data, including an explicit import.
    Publication,
    /// Adoption of an already published version, never a new materialization.
    CacheReuse,
    /// Explicit head reset; excluded unless a leaf opts in.
    Reset,
}
/// Exact observed version. Signal-only tokens retain this provenance without binding it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedDataset {
    /// Original provider identity, never the consumer's alias.
    pub workspace_id: String,
    /// Stable dataset identity.
    pub dataset_id: String,
    /// Immutable version at event time.
    pub version_id: String,
    /// Explicit event branch, frozen at delivery rather than resolved later.
    pub branch: String,
    /// Provider head generation, as a lossless decimal carrier.
    pub generation: String,
    /// Publication, cache adoption or reset.
    pub cause: DatasetCause,
}
/// Closed persisted evidence, shared by event matching and later token evaluation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenPayload {
    /// Independent evidence format.
    pub format_version: u8,
    /// Stable committed local event identity.
    pub event_id: String,
    /// Local monotonic ordering, not wall-clock delivery order.
    pub event_sequence: String,
    /// Original occurrence time, not a retry/delivery timestamp.
    pub occurred_at_us: String,
    /// Committed event category.
    pub event_kind: String,
    /// Original causation evidence.
    pub causation: Option<String>,
    /// Original build/operation correlation.
    pub correlation: Option<String>,
    /// Whether the observed dataset becomes a downstream read boundary.
    pub payload_mode: PayloadMode,
    /// Exact observation is preserved even for signal-only evidence.
    pub dataset: Option<ObservedDataset>,
    /// Successful upstream build, when applicable.
    pub build_id: Option<String>,
    /// Successful upstream schedule, when applicable.
    pub schedule_id: Option<String>,
    /// Successful occurrence, when applicable.
    pub occurrence_id: Option<String>,
}
impl TokenPayload {
    /// Exact binding contribution; signal-only leaves contribute no input pin.
    pub fn input_pin(&self) -> Option<&ObservedDataset> {
        (self.payload_mode == PayloadMode::Pin)
            .then_some(self.dataset.as_ref())
            .flatten()
    }
}
/// Trusted provider delivery after its immutable metadata has been retained locally.
/// This repository adapter opens no provider and copies no artifact bytes.
pub struct ForeignDatasetEvent {
    /// Original provider event identity, reused on retries.
    pub event: RequestId,
    /// Original registered provider dataset.
    pub dataset: DatasetKey,
    /// Exact retained provider version.
    pub version: VersionId,
    /// Explicit provider branch.
    pub branch: BranchName,
    /// Provider head generation.
    pub generation: u64,
    /// Whether this was new data, adoption or reset.
    pub cause: DatasetCause,
    /// Original event timestamp in microseconds.
    pub at_us: i64,
}
/// Results of a bounded committed-event scan; no builds are enqueued here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Scan {
    /// Last committed event scanned.
    pub cursor: i64,
    /// Events examined in this transaction.
    pub scanned: usize,
    /// New durable tokens.
    pub tokens: usize,
    /// Matching events ignored because paused, expired or awaiting review.
    pub ignored: usize,
    /// More committed events remain after this page.
    pub more: bool,
}
struct Fact {
    dataset: Option<ObservedDataset>,
    build: Option<String>,
    schedule: Option<String>,
    occurrence: Option<String>,
    jobs: Vec<(DatasetKey, String, String)>,
    materialized: bool,
}
fn digest(value: &Value) -> Result<String> {
    let bytes = tf_protocol::canonical::canonical_json(value).map_err(|_| invalid())?;
    Ok(tf_protocol::canonical::file_digest(&mut bytes.as_slice())
        .map_err(|_| invalid())?
        .hex())
}
fn event_id(value: &Value) -> Result<RequestId> {
    let hex = digest(value)?;
    let mut bytes = [0; 16];
    for (out, pair) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
        *out = u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| invalid())?, 16)
            .map_err(|_| invalid())?;
    }
    Ok(RequestId::from_bytes(bytes))
}
fn leaves<'a>(tree: &'a Trigger, out: &mut Vec<&'a Trigger>) {
    match tree {
        Trigger::And { children } | Trigger::Or { children } => {
            for child in children {
                leaves(child, out);
            }
        }
        Trigger::Manual | Trigger::Cron { .. } => {}
        leaf => out.push(leaf),
    }
}
fn matched<'a>(leaf: &'a Trigger, f: &Fact) -> Result<Option<(&'a str, PayloadMode)>> {
    let value = match leaf {
        Trigger::DatasetHeadChanged {
            id,
            dataset,
            branch,
            payload_mode,
            include_resets,
        }
        | Trigger::DatasetPublished {
            id,
            dataset,
            branch,
            payload_mode,
            include_resets,
        } => {
            let Some(observed) = &f.dataset else {
                return Ok(None);
            };
            let publication = matches!(leaf, Trigger::DatasetPublished { .. });
            if dataset.key().map_err(|_| invalid())?
                != DatasetKey::from_text(&observed.workspace_id, &observed.dataset_id)
                    .map_err(|_| invalid())?
                || branch != &observed.branch
                || (publication && observed.cause != DatasetCause::Publication)
                || (!include_resets && observed.cause == DatasetCause::Reset)
            {
                return Ok(None);
            }
            (id.as_str(), *payload_mode)
        }
        Trigger::BuildSucceeded {
            id,
            targets,
            branch,
        } => {
            if f.build.is_none() || f.schedule.is_some() {
                return Ok(None);
            }
            for target in targets {
                let key = target.key().map_err(|_| invalid())?;
                if !f.jobs.iter().any(|(k, b, _)| *k == key && b == branch) {
                    return Ok(None);
                }
            }
            (id.as_str(), PayloadMode::SignalOnly)
        }
        Trigger::ScheduleSucceeded {
            id,
            schedule_id,
            require_materialization,
        } => {
            if f.schedule.as_ref() != Some(schedule_id)
                || (*require_materialization && !f.materialized)
            {
                return Ok(None);
            }
            (id.as_str(), PayloadMode::SignalOnly)
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
async fn successful_jobs(
    db: &mut SqliteConnection,
    build: &str,
) -> Result<Vec<(DatasetKey, String, String)>> {
    let rows = sqlx::query("SELECT d.workspace_id,j.dataset_id,coalesce(json_extract(p.candidate_json,'$.output.name'),b.name),j.state FROM jobs j JOIN datasets d ON d.id=j.dataset_id JOIN data_branches b ON b.id=j.branch_id JOIN builds owner ON owner.id=j.build_id JOIN build_plans p ON p.id=owner.plan_id WHERE j.build_id=? ORDER BY j.id LIMIT 10001")
        .bind(build).fetch_all(db).await?;
    if rows.len() > 10000 {
        return Err(invalid());
    }
    rows.iter()
        .map(|r| {
            let state: String = r.try_get(3)?;
            if !matches!(state.as_str(), "SUCCEEDED" | "CACHED") {
                return Err(invalid());
            }
            Ok((
                DatasetKey::from_text(&r.try_get::<String, _>(0)?, &r.try_get::<String, _>(1)?)
                    .map_err(|_| invalid())?,
                r.try_get(2)?,
                state,
            ))
        })
        .collect()
}
async fn fact(
    db: &mut SqliteConnection,
    id: &str,
    kind: &str,
    payload: &Value,
) -> Result<Option<Fact>> {
    let mut result = Fact {
        dataset: None,
        build: None,
        schedule: None,
        occurrence: None,
        jobs: vec![],
        materialized: false,
    };
    match kind {
        "dataset.published" | "dataset.head_changed" => {
            let r = sqlx::query("SELECT d.workspace_id,h.dataset_id,h.new_version_id,b.name,h.generation,h.cause FROM head_changes h JOIN datasets d ON d.id=h.dataset_id JOIN data_branches b ON b.id=h.branch_id JOIN dataset_versions v ON v.id=h.new_version_id AND v.dataset_id=h.dataset_id WHERE h.event_id=?")
                .bind(id).fetch_optional(db).await?;
            let Some(r) = r else { return Ok(None) };
            let cause: String = r.try_get(5)?;
            let cause = match cause.as_str() {
                "publication" => DatasetCause::Publication,
                "cache_reuse" => DatasetCause::CacheReuse,
                "reset" | "head_reset" => DatasetCause::Reset,
                _ => return Err(invalid()),
            };
            result.dataset = Some(ObservedDataset {
                workspace_id: r.try_get(0)?,
                dataset_id: r.try_get(1)?,
                version_id: r.try_get(2)?,
                branch: payload["branch"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or(r.try_get(3)?),
                generation: r.try_get::<i64, _>(4)?.to_string(),
                cause,
            });
        }
        "schedule.dataset_event" => {
            let observed: ObservedDataset =
                serde_json::from_value(payload["dataset"].clone()).map_err(|_| invalid())?;
            let present:i64=sqlx::query_scalar("SELECT count(*) FROM foreign_versions WHERE workspace_id=? AND dataset_id=? AND version_id=?").bind(&observed.workspace_id).bind(&observed.dataset_id).bind(&observed.version_id).fetch_one(db).await?;
            if present != 1 {
                return Err(invalid());
            }
            result.dataset = Some(observed);
        }
        "build.state" if payload["state"] == "SUCCEEDED" => {
            let build = payload["build"].as_str().ok_or_else(invalid)?;
            let present: i64 =
                sqlx::query_scalar("SELECT count(*) FROM builds WHERE id=? AND state='SUCCEEDED'")
                    .bind(build)
                    .fetch_one(&mut *db)
                    .await?;
            if present != 1 {
                return Ok(None);
            }
            result.jobs = successful_jobs(db, build).await?;
            result.build = Some(build.to_owned());
        }
        "schedule.succeeded" => {
            let occurrence = payload["occurrence"].as_str().ok_or_else(invalid)?;
            let r=sqlx::query("SELECT o.schedule_id,o.build_id,o.execution_json FROM schedule_occurrences o JOIN builds b ON b.id=o.build_id AND b.occurrence_id=o.id WHERE o.id=? AND o.disposition='SUCCEEDED' AND b.state='SUCCEEDED'").bind(occurrence).fetch_optional(&mut *db).await?;
            let Some(r) = r else { return Ok(None) };
            let build: String = r.try_get(1)?;
            let execution: Value =
                serde_json::from_str(&r.try_get::<String, _>(2)?).map_err(|_| invalid())?;
            let template: BuildTemplate =
                serde_json::from_value(execution["build"].clone()).map_err(|_| invalid())?;
            result.jobs = successful_jobs(db, &build).await?;
            result.materialized = template.targets.iter().any(|t| {
                t.key().is_ok_and(|k| {
                    result
                        .jobs
                        .iter()
                        .any(|(key, _, state)| *key == k && state == "SUCCEEDED")
                })
            });
            result.build = Some(build);
            result.schedule = Some(r.try_get(0)?);
            result.occurrence = Some(occurrence.to_owned());
        }
        _ => return Ok(None),
    }
    Ok(Some(result))
}
impl Store {
    /// Convert at most 100 committed events to tokens for this expected definition epoch.
    /// The owner calls this primitive; it never activates a scheduler or dispatches a build.
    pub async fn scan_schedule_events(
        &mut self,
        workspace: WorkspaceId,
        schedule: ScheduleId,
        epoch: &str,
        limit: u32,
        now_us: i64,
    ) -> Result<Scan> {
        if !(1..=100).contains(&limit) || now_us < 0 {
            return Err(invalid());
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        retention::clock(&mut tx, now_us)
            .await
            .map_err(|_| invalid())?;
        let r=sqlx::query("SELECT definition_json,event_cursor,paused,needs_review FROM schedules WHERE workspace_id=? AND id=? AND trigger_epoch=? AND deleted_at_us IS NULL").bind(workspace.to_string()).bind(schedule.to_string()).bind(epoch).fetch_optional(&mut *tx).await?.ok_or_else(invalid)?;
        let paused = r.try_get::<i64, _>(2)? != 0;
        let review = r.try_get::<i64, _>(3)? != 0;
        let raw: String = r.try_get(0)?;
        let def = if review {
            None
        } else {
            Some(
                Definition::decode(&serde_json::from_str(&raw).map_err(|_| invalid())?)
                    .map_err(|_| invalid())?,
            )
        };
        let mut leafs = vec![];
        if let Some(d) = &def {
            leaves(&d.trigger, &mut leafs)
        }
        let mut report = Scan {
            cursor: r.try_get(1)?,
            ..Scan::default()
        };
        let rows=sqlx::query("SELECT sequence,id,type,CASE WHEN type IN ('dataset.published','dataset.head_changed','build.state','schedule.succeeded','schedule.dataset_event') THEN payload_json ELSE '{}' END,wall_time_us,causation_id,correlation_id FROM events WHERE sequence>? ORDER BY sequence LIMIT ?").bind(report.cursor).bind(limit).fetch_all(&mut *tx).await?;
        let mut bytes = 0;
        for e in rows {
            let raw: String = e.try_get(3)?;
            bytes += raw.len();
            if raw.len() > 262144 {
                return Err(invalid());
            }
            if bytes > 1048576 {
                break;
            }
            let sequence: i64 = e.try_get(0)?;
            let at_us: i64 = e.try_get(4)?;
            if at_us < 0 {
                return Err(invalid());
            }
            let id: String = e.try_get(1)?;
            let kind: String = e.try_get(2)?;
            let payload: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
            if let Some(f) = fact(&mut tx, &id, &kind, &payload).await? {
                for leaf in &leafs {
                    let Some((leaf, mode)) = matched(leaf, &f)? else {
                        continue;
                    };
                    let expires = def
                        .as_ref()
                        .and_then(|d| d.policies.token_window_seconds)
                        .map(|s| {
                            at_us
                                .checked_add(i64::from(s) * 1000000)
                                .ok_or_else(invalid)
                        })
                        .transpose()?;
                    if paused || review || expires.is_some_and(|t| t <= now_us) {
                        report.ignored += 1;
                        continue;
                    }
                    if let Some(observed) = &f.dataset
                        && observed.workspace_id == workspace.to_string()
                    {
                        retention::version_available(
                            &mut tx,
                            observed.version_id.parse().map_err(|_| invalid())?,
                        )
                        .await
                        .map_err(|_| invalid())?;
                    }
                    let evidence = TokenPayload {
                        format_version: 1,
                        event_id: id.clone(),
                        event_sequence: sequence.to_string(),
                        occurred_at_us: at_us.to_string(),
                        event_kind: kind.clone(),
                        causation: e.try_get(5)?,
                        correlation: e.try_get(6)?,
                        payload_mode: mode,
                        dataset: f.dataset.clone(),
                        build_id: f.build.clone(),
                        schedule_id: f.schedule.clone(),
                        occurrence_id: f.occurrence.clone(),
                    };
                    let encoded = serde_json::to_string(&evidence).map_err(|_| invalid())?;
                    let token = digest(&json!([
                        "transflow.schedule.token.v1",
                        schedule.to_string(),
                        epoch,
                        leaf,
                        id
                    ]))?;
                    let n=sqlx::query("INSERT INTO trigger_tokens(id,schedule_id,trigger_epoch,leaf_id,event_id,tick_id,payload_json,seen_at_us,expires_at_us,consumed_by) VALUES(?,?,?,?,?,NULL,?,?,?,NULL) ON CONFLICT(id) DO NOTHING").bind(&token).bind(schedule.to_string()).bind(epoch).bind(leaf).bind(&id).bind(&encoded).bind(now_us).bind(expires).execute(&mut *tx).await?.rows_affected();
                    if n == 0 {
                        let existing: String = sqlx::query_scalar(
                            "SELECT payload_json FROM trigger_tokens WHERE id=?",
                        )
                        .bind(token)
                        .fetch_one(&mut *tx)
                        .await?;
                        if existing != encoded {
                            return Err(invalid());
                        }
                    }
                    report.tokens += usize::try_from(n).map_err(|_| invalid())?;
                }
            }
            report.cursor = sequence;
            report.scanned += 1;
        }
        sqlx::query("UPDATE schedules SET event_cursor=? WHERE id=? AND trigger_epoch=?")
            .bind(report.cursor)
            .bind(schedule.to_string())
            .bind(epoch)
            .execute(&mut *tx)
            .await?;
        if report.scanned > 0 {
            sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule_events_scanned',?,?)").bind(json!({"schedule":schedule.to_string(),"epoch":epoch,"cursor":report.cursor.to_string(),"scanned":report.scanned,"tokens":report.tokens,"ignored":report.ignored,"needs_review":review}).to_string()).bind(now_us).execute(&mut *tx).await?;
        }
        report.more =
            sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM events WHERE sequence>?)")
                .bind(report.cursor)
                .fetch_one(&mut *tx)
                .await?
                != 0;
        tx.commit().await?;
        Ok(report)
    }
    /// Deduplicate a provider publication by origin identity/version, or a head change by
    /// origin identity/generation. Changed evidence for an existing event fails closed.
    pub async fn receive_foreign_schedule_event(
        &mut self,
        event: ForeignDatasetEvent,
    ) -> Result<RequestId> {
        if event.at_us < 0 || event.generation == 0 || event.generation > i64::MAX as u64 {
            return Err(invalid());
        }
        let observed = ObservedDataset {
            workspace_id: event.dataset.workspace_id().to_string(),
            dataset_id: event.dataset.dataset_id().to_string(),
            version_id: event.version.to_string(),
            branch: event.branch.to_string(),
            generation: event.generation.to_string(),
            cause: event.cause,
        };
        let identity = match event.cause {
            DatasetCause::Publication => json!([
                "transflow.foreign.publication.v1",
                observed.workspace_id,
                observed.dataset_id,
                observed.version_id
            ]),
            _ => json!([
                "transflow.foreign.head.v1",
                observed.workspace_id,
                observed.dataset_id,
                observed.branch,
                observed.generation
            ]),
        };
        let id = event_id(&identity)?;
        let payload = json!({"origin_event":event.event.to_string(),"dataset":observed});
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let present:i64=sqlx::query_scalar("SELECT count(*) FROM foreign_versions f WHERE f.workspace_id=? AND f.dataset_id=? AND f.version_id=? AND EXISTS(SELECT 1 FROM external_registrations r WHERE r.provider_workspace_id=f.workspace_id AND r.provider_dataset_id=f.dataset_id) AND f.workspace_id!=(SELECT id FROM workspaces)").bind(event.dataset.workspace_id().to_string()).bind(event.dataset.dataset_id().to_string()).bind(event.version.to_string()).fetch_one(&mut *tx).await?;
        if present != 1 {
            return Err(invalid());
        }
        let n=sqlx::query("INSERT INTO events(id,type,payload_json,causation_id,wall_time_us) VALUES(?,'schedule.dataset_event',?,?,?) ON CONFLICT(id) DO NOTHING").bind(id.to_string()).bind(payload.to_string()).bind(event.event.to_string()).bind(event.at_us).execute(&mut *tx).await?.rows_affected();
        if n == 0 {
            let prior: (String, String, i64) =
                sqlx::query_as("SELECT type,payload_json,wall_time_us FROM events WHERE id=?")
                    .bind(id.to_string())
                    .fetch_one(&mut *tx)
                    .await?;
            if prior
                != (
                    "schedule.dataset_event".to_owned(),
                    payload.to_string(),
                    event.at_us,
                )
            {
                return Err(invalid());
            }
        }
        tx.commit().await?;
        Ok(id)
    }
    /// Record successful accepted schedule work with its event in one transaction.
    /// Failed/canceled builds, changed identities and incomplete targets never qualify.
    pub async fn complete_schedule_success(
        &mut self,
        workspace: WorkspaceId,
        occurrence: ScheduleOccurrenceId,
        build: BuildId,
        at_us: i64,
    ) -> Result<RequestId> {
        if at_us < 0 {
            return Err(invalid());
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let r=sqlx::query("SELECT o.schedule_id,o.disposition,o.execution_json FROM schedule_occurrences o JOIN schedules s ON s.id=o.schedule_id JOIN builds b ON b.id=? AND b.occurrence_id=o.id WHERE o.id=? AND s.workspace_id=? AND b.state='SUCCEEDED' AND (o.build_id IS NULL OR o.build_id=b.id)").bind(build.to_string()).bind(occurrence.to_string()).bind(workspace.to_string()).fetch_optional(&mut *tx).await?.ok_or_else(invalid)?;
        let disposition: String = r.try_get(1)?;
        if !matches!(
            disposition.as_str(),
            "ACCEPTED" | "QUEUED" | "RUNNING" | "SUCCEEDED"
        ) {
            return Err(invalid());
        }
        let execution: Value =
            serde_json::from_str(&r.try_get::<String, _>(2)?).map_err(|_| invalid())?;
        let template: BuildTemplate =
            serde_json::from_value(execution["build"].clone()).map_err(|_| invalid())?;
        if template.targets.is_empty() || template.targets.len() > 1000 {
            return Err(invalid());
        }
        let jobs = successful_jobs(&mut tx, &build.to_string()).await?;
        for t in &template.targets {
            let key = t.key().map_err(|_| invalid())?;
            if !jobs
                .iter()
                .any(|(k, b, _)| *k == key && b == &template.data_branch)
            {
                return Err(invalid());
            }
        }
        let schedule: String = r.try_get(0)?;
        let id = event_id(&json!([
            "transflow.schedule.success.v1",
            schedule,
            occurrence.to_string()
        ]))?;
        if disposition == "SUCCEEDED" {
            let exists:i64=sqlx::query_scalar("SELECT count(*) FROM events WHERE id=? AND type='schedule.succeeded' AND json_extract(payload_json,'$.build')=?").bind(id.to_string()).bind(build.to_string()).fetch_one(&mut *tx).await?;
            if exists != 1 {
                return Err(invalid());
            }
        } else {
            sqlx::query(
                "UPDATE schedule_occurrences SET disposition='SUCCEEDED',build_id=? WHERE id=?",
            )
            .bind(build.to_string())
            .bind(occurrence.to_string())
            .execute(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO events(id,type,payload_json,causation_id,correlation_id,wall_time_us) VALUES(?,'schedule.succeeded',?,?,?,?)").bind(id.to_string()).bind(json!({"schedule":schedule,"occurrence":occurrence.to_string(),"build":build.to_string()}).to_string()).bind(occurrence.to_string()).bind(build.to_string()).bind(at_us).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(id)
    }
}
