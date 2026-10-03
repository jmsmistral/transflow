//! One current definition, guarded replacement and independently frozen accepted execution.
use crate::{Reader, Store, StoreError};
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use tf_domain::{RequestId, ScheduleId, ScheduleOccurrenceId, WorkspaceId};
use tf_protocol::schedule::{Definition, Source};

/// A stale editor receives the current edit guard, never a silent overwrite.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Repository, bounds or integrity failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Concurrent definition or operational-state edit.
    #[error("The schedule changed; reload its current definition before saving")]
    Conflict {
        /// Current opaque guard when the schedule exists.
        current_etag: Option<String>,
    },
    /// Missing retained schedule or required source.
    #[error("The schedule or its required retained source is unavailable")]
    Missing,
    /// Bounded pending capacity or explicit replay interval exceeded.
    #[error("The schedule request exceeds its pending or replay limit")]
    Limit,
}
/// Guarded save intent. Creation is explicit; replacement preserves pause state.
pub struct Save<'a> {
    /// Stable schedule identity, allocated independently of definitions.
    pub id: ScheduleId,
    /// Selected owning workspace.
    pub workspace: WorkspaceId,
    /// Normalized current definition.
    pub definition: &'a Value,
    /// None creates; Some replaces exactly this current guard.
    pub expected_etag: Option<&'a str>,
    /// Initial operational state; ignored during replacement.
    pub paused: bool,
    /// Audited non-secret actor label.
    pub actor: &'a str,
    /// UTC save timestamp.
    pub at_us: i64,
    /// Optional HTTP receipt identity/digest, committed with the definition.
    pub receipt: Option<(RequestId, &'a str)>,
}
/// Accepted occurrence intent, independent of future definition replacements.
pub struct Freeze<'a> {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Stable schedule identity.
    pub id: ScheduleId,
    /// Expected current trigger epoch.
    pub epoch: &'a str,
    /// Stable occurrence identity.
    pub occurrence: ScheduleOccurrenceId,
    /// Canonical evidence digest.
    pub evidence: &'a str,
    /// Frozen trigger payload.
    pub payload: &'a Value,
    /// Logical fire timestamp in microseconds.
    pub at_us: i64,
}
fn bad() -> Error {
    StoreError::InvalidRequest.into()
}
pub(crate) async fn row(
    db: &mut SqliteConnection,
    workspace: WorkspaceId,
    id: ScheduleId,
) -> Result<Option<Value>, Error> {
    let r=sqlx::query("SELECT id,etag,trigger_epoch,paused,needs_review,definition_json,saved_at_us FROM schedules WHERE workspace_id=? AND id=? AND deleted_at_us IS NULL AND length(definition_json)<=262144").bind(workspace.to_string()).bind(id.to_string()).fetch_optional(db).await.map_err(StoreError::from)?;
    r.map(|r|Ok(json!({"id":r.try_get::<String,_>(0).map_err(StoreError::from)?,"etag":r.try_get::<String,_>(1).map_err(StoreError::from)?,"trigger_epoch":r.try_get::<String,_>(2).map_err(StoreError::from)?,"paused":r.try_get::<i64,_>(3).map_err(StoreError::from)?==1,"needs_review":r.try_get::<i64,_>(4).map_err(StoreError::from)?==1,"definition":serde_json::from_str::<Value>(&r.try_get::<String,_>(5).map_err(StoreError::from)?).map_err(|_|bad())?,"saved_at_us":r.try_get::<i64,_>(6).map_err(StoreError::from)?.to_string()}))).transpose()
}
impl Store {
    /// Replace the current snapshot atomically with audit/token reset and optional API receipt.
    /// Validation performs no imports, worker launches or provider contact.
    pub async fn save_schedule(&mut self, request: Save<'_>) -> Result<Value, Error> {
        self.save_schedule_with_replay(request, None).await
    }
    /// Guarded replacement plus an explicitly bounded retained-event replay.
    pub async fn save_schedule_with_replay(
        &mut self,
        request: Save<'_>,
        replay: Option<crate::schedule_lifecycle::Replay>,
    ) -> Result<Value, Error> {
        let definition = Definition::decode(request.definition).map_err(|_| bad())?;
        if request.at_us < 0
            || request.actor.is_empty()
            || request.actor.len() > 200
            || request.actor.chars().any(char::is_control)
        {
            return Err(bad());
        }
        let encoded = serde_json::to_string(request.definition).map_err(|_| bad())?;
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(StoreError::from)?;
        let current = row(&mut tx, request.workspace, request.id).await?;
        if current.is_none() {
            let occupied: i64 = sqlx::query_scalar("SELECT count(*) FROM schedules WHERE id=?")
                .bind(request.id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(StoreError::from)?;
            if occupied != 0 {
                return Err(Error::Conflict { current_etag: None });
            }
        }
        if current.as_ref().and_then(|v| v["etag"].as_str()) != request.expected_etag
            || (current.is_none() && request.expected_etag.is_some())
        {
            return Err(Error::Conflict {
                current_etag: current.and_then(|v| v["etag"].as_str().map(str::to_owned)),
            });
        }
        for target in definition
            .build
            .targets
            .iter()
            .chain(&definition.build.exclusions)
            .chain(&definition.build.refresh_sources)
        {
            if target.key().map_err(|_| bad())?.workspace_id() != request.workspace {
                return Err(bad());
            }
            let present:i64=sqlx::query_scalar("SELECT count(*) FROM datasets WHERE workspace_id=? AND id=? AND tombstoned_at_us IS NULL").bind(request.workspace.to_string()).bind(&target.dataset_id).fetch_one(&mut *tx).await.map_err(StoreError::from)?;
            if present != 1 {
                return Err(Error::Missing);
            }
        }
        if let Source::FixedSnapshot { snapshot_id } = &definition.build.source {
            let present: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM source_snapshots WHERE workspace_id=? AND id=?",
            )
            .bind(request.workspace.to_string())
            .bind(snapshot_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(StoreError::from)?;
            if present != 1 {
                return Err(Error::Missing);
            }
        }
        let (etag, epoch): (String, String) =
            sqlx::query_as("SELECT lower(hex(randomblob(32))),lower(hex(randomblob(32)))")
                .fetch_one(&mut *tx)
                .await
                .map_err(StoreError::from)?;
        let cursor: i64 = sqlx::query_scalar("SELECT COALESCE(max(sequence),0) FROM events")
            .fetch_one(&mut *tx)
            .await
            .map_err(StoreError::from)?;
        let paused = current
            .as_ref()
            .and_then(|v| v["paused"].as_bool())
            .unwrap_or(request.paused);
        if current.is_some() {
            sqlx::query("UPDATE schedules SET name=?,definition_json=?,etag=?,trigger_epoch=?,needs_review=0,saved_at_us=?,saved_by=?,event_cursor=? WHERE id=? AND workspace_id=?")
             .bind(&definition.name).bind(&encoded).bind(&etag).bind(&epoch).bind(request.at_us).bind(request.actor).bind(cursor).bind(request.id.to_string()).bind(request.workspace.to_string()).execute(&mut *tx).await.map_err(StoreError::from)?;
        } else {
            sqlx::query("INSERT INTO schedules(id,workspace_id,name,definition_json,etag,trigger_epoch,paused,needs_review,saved_at_us,saved_by,event_cursor) VALUES(?,?,?,?,?,?,?,0,?,?,?)")
             .bind(request.id.to_string()).bind(request.workspace.to_string()).bind(&definition.name).bind(&encoded).bind(&etag).bind(&epoch).bind(paused).bind(request.at_us).bind(request.actor).bind(cursor).execute(&mut *tx).await.map_err(StoreError::from)?;
        }
        if let Some(old) = &current {
            crate::schedule_lifecycle::close_pause(
                &mut tx,
                request.id,
                old["trigger_epoch"].as_str().ok_or_else(bad)?,
                request.at_us,
                cursor,
            )
            .await?;
        }
        if paused {
            crate::schedule_lifecycle::open_pause(
                &mut tx,
                request.id,
                &epoch,
                request.at_us,
                cursor,
            )
            .await?;
        }
        // A saved definition cannot combine old, unaccepted leaf evidence with new rules.
        // Consumed tokens and accepted occurrence evidence remain historical execution facts.
        sqlx::query("DELETE FROM trigger_tokens WHERE schedule_id=? AND consumed_by IS NULL")
            .bind(request.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(StoreError::from)?;
        let epoch = if let Some(replay) = replay {
            if paused {
                return Err(bad());
            }
            crate::schedule_lifecycle::replay(
                &mut tx,
                request.workspace,
                request.id,
                replay,
                request.at_us,
            )
            .await?
        } else {
            epoch
        };
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('schedule_saved',?,?)").bind(json!({"schedule":request.id.to_string(),"etag":etag,"trigger_epoch":epoch,"actor":request.actor,"created":current.is_none()}).to_string()).bind(request.at_us).execute(&mut *tx).await.map_err(StoreError::from)?;
        let saved = json!({"id":request.id.to_string(),"etag":etag,"trigger_epoch":epoch,"paused":paused,"needs_review":false,"definition":request.definition,"saved_at_us":request.at_us.to_string()});
        if let Some((id, digest)) = request.receipt {
            if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad());
            }
            sqlx::query("INSERT INTO api_operations(id,digest,response_json) VALUES(?,?,?)")
                .bind(id.to_string())
                .bind(digest)
                .bind(json!({"data":saved,"context":null,"etag":etag}).to_string())
                .execute(&mut *tx)
                .await
                .map_err(StoreError::from)?;
        }
        tx.commit().await.map_err(StoreError::from)?;
        Ok(saved)
    }
    /// Freeze an accepted occurrence's execution settings under its expected trigger epoch.
    /// Automatic evidence acceptance uses queue_schedule_occurrence; this primitive launches nothing.
    pub async fn freeze_schedule_occurrence(&mut self, request: Freeze<'_>) -> Result<bool, Error> {
        let Freeze {
            workspace,
            id,
            epoch,
            occurrence,
            evidence,
            payload,
            at_us,
        } = request;
        if evidence.len() != 64
            || !evidence.bytes().all(|b| b.is_ascii_hexdigit())
            || at_us < 0
            || serde_json::to_vec(payload).map_err(|_| bad())?.len() > 262144
        {
            return Err(bad());
        }
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(StoreError::from)?;
        let current = row(&mut tx, workspace, id).await?.ok_or(Error::Missing)?;
        if current["needs_review"] == true || current["trigger_epoch"] != epoch {
            return Err(Error::Conflict {
                current_etag: current["etag"].as_str().map(str::to_owned),
            });
        }
        let def = &current["definition"];
        Definition::decode(def).map_err(|_| bad())?;
        let n=sqlx::query("INSERT INTO schedule_occurrences(id,schedule_id,trigger_epoch,evidence_digest,logical_fire_at_us,payload_json,execution_json,disposition) VALUES(?,?,?,?,?,?,?,'ACCEPTED') ON CONFLICT(schedule_id,trigger_epoch,evidence_digest) DO NOTHING")
        .bind(occurrence.to_string()).bind(id.to_string()).bind(epoch).bind(evidence).bind(at_us).bind(payload.to_string()).bind(json!({"build":def["build"],"policies":def["policies"]}).to_string()).execute(&mut *tx).await.map_err(StoreError::from)?.rows_affected();
        tx.commit().await.map_err(StoreError::from)?;
        Ok(n == 1)
    }
}
impl Reader {
    /// Read the current snapshot only; legacy placeholders remain explicitly needs-review.
    pub async fn schedule(
        &mut self,
        workspace: WorkspaceId,
        id: ScheduleId,
    ) -> Result<Option<Value>, Error> {
        row(&mut self.db, workspace, id).await
    }
    /// Bounded ID-keyset page with explicit continuation.
    pub async fn schedules(&mut self, workspace: WorkspaceId, after: &str) -> Result<Value, Error> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT id FROM schedules WHERE workspace_id=? AND deleted_at_us IS NULL AND id>? ORDER BY id LIMIT 101").bind(workspace.to_string()).bind(after).fetch_all(&mut self.db).await.map_err(StoreError::from)?;
        let mut more = ids.len() > 100;
        let mut bytes = 0usize;
        let mut values = vec![];
        for id in ids.iter().take(100) {
            let value = row(&mut self.db, workspace, id.parse().map_err(|_| bad())?)
                .await?
                .ok_or(Error::Missing)?;
            let size = serde_json::to_vec(&value).map_err(|_| bad())?.len();
            if bytes + size > 1_048_576 && !values.is_empty() {
                more = true;
                break;
            }
            bytes += size;
            values.push(value);
        }
        Ok(
            json!({"next_cursor":if more {values.last().map(|v|v["id"].clone())}else{None},"schedules":values}),
        )
    }
}
