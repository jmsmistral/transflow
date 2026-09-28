//! Bounded API projections. These methods never inspect provider bytes or import source.
use crate::{Reader, Result, Store, StoreError, planning::DraftPlan};
use serde_json::{Value, json};
use sqlx::Row;
use tf_domain::{DatasetId, RequestId, WorkspaceId};
/// Durable replay state; an incomplete receipt must never execute again.
#[derive(Debug, PartialEq)]
pub enum Receipt {
    /// First reservation.
    New,
    /// Same request, completed response.
    Complete(Value),
    /// Interrupted or currently executing request.
    Pending,
    /// Key reused for different input.
    Conflict,
}
impl Store {
    /// Reserve before any domain side effect; serialized by the existing runtime owner.
    pub async fn reserve_api(&mut self, id: RequestId, digest: &str) -> Result<Receipt> {
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StoreError::InvalidRequest);
        }
        let inserted = sqlx::query(
            "INSERT INTO api_operations(id,digest) VALUES(?,?) ON CONFLICT(id) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(digest)
        .execute(&mut self.db)
        .await?
        .rows_affected();
        if inserted == 1 {
            return Ok(Receipt::New);
        }
        receipt(&mut self.db, id, digest).await
    }
    /// Freeze the safe public response. Raw errors and request credentials are never stored.
    pub async fn finish_api(
        &mut self,
        id: RequestId,
        digest: &str,
        response: &Value,
    ) -> Result<()> {
        let encoded = serde_json::to_string(response).map_err(|_| StoreError::InvalidRequest)?;
        if encoded.len() > 32 * 1024 * 1024 {
            return Err(StoreError::InvalidRequest);
        }
        let n=sqlx::query("UPDATE api_operations SET response_json=? WHERE id=? AND digest=? AND response_json IS NULL").bind(encoded).bind(id.to_string()).bind(digest).execute(&mut self.db).await?.rows_affected();
        if n != 1 {
            return Err(StoreError::InvalidRequest);
        }
        Ok(())
    }
}
async fn receipt(db: &mut sqlx::SqliteConnection, id: RequestId, digest: &str) -> Result<Receipt> {
    let row = sqlx::query("SELECT digest,response_json FROM api_operations WHERE id=?")
        .bind(id.to_string())
        .fetch_optional(db)
        .await?;
    let Some(row) = row else {
        return Ok(Receipt::New);
    };
    if row.try_get::<String, _>(0)? != digest {
        return Ok(Receipt::Conflict);
    }
    Ok(match row.try_get::<Option<String>, _>(1)? {
        None => Receipt::Pending,
        Some(s) => {
            Receipt::Complete(serde_json::from_str(&s).map_err(|_| StoreError::InvalidRequest)?)
        }
    })
}
impl Reader {
    /// Replay receipts during an active build without acquiring a writer.
    pub async fn api_receipt(&mut self, id: RequestId, digest: &str) -> Result<Receipt> {
        receipt(&mut self.db, id, digest).await
    }
    /// Conservative metadata revision; unrelated publications may invalidate a page safely.
    pub async fn api_revision(&mut self) -> Result<String> {
        // Publication may make an attempt terminal just before its final phase is closed.
        // Fence that timing-only update as well as lifecycle events.
        let r=sqlx::query("SELECT (SELECT coalesce(max(sequence),0) FROM events),(SELECT count(*) FROM data_branches),(SELECT coalesce(sum(revision),0) FROM data_branches),(SELECT count(*) FROM dataset_versions),(SELECT count(*) FROM foreign_versions),(SELECT count(*) FROM audit_log),(SELECT count(*) FROM phase_intervals),(SELECT count(*) FROM phase_intervals WHERE finished_at_us IS NOT NULL)").fetch_one(&mut self.db).await?;
        Ok((0..8)
            .map(|i| r.try_get::<i64, _>(i).map(|n| n.to_string()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join(":"))
    }
    /// Authenticate an immutable saved proposal without migration or imports.
    pub async fn api_plan(&mut self, id: RequestId) -> Result<DraftPlan> {
        let row=sqlx::query("SELECT candidate_json,digest FROM build_plans WHERE id=? AND length(candidate_json)<=16777216").bind(id.to_string()).fetch_one(&mut self.db).await?;
        let plan: DraftPlan = serde_json::from_str(&row.try_get::<String, _>(0)?)
            .map_err(|_| StoreError::InvalidRequest)?;
        if plan.id != id.to_string()
            || plan.digest().map_err(|_| StoreError::InvalidRequest)?.hex()
                != row.try_get::<String, _>(1)?
        {
            return Err(StoreError::InvalidRequest);
        }
        Ok(plan)
    }
    /// Retained versions, ordered by immutable UUID with a caller-bound context cursor.
    pub async fn api_versions(
        &mut self,
        workspace: WorkspaceId,
        dataset: DatasetId,
        after: &str,
        limit: u32,
        foreign: bool,
    ) -> Result<Vec<Value>> {
        if limit == 0 || limit > 201 {
            return Err(StoreError::InvalidRequest);
        }
        if foreign {
            let rows=sqlx::query("SELECT version_id,CASE WHEN length(provenance_json)<=1048576 THEN provenance_json END FROM foreign_versions WHERE workspace_id=? AND dataset_id=? AND version_id>? ORDER BY version_id LIMIT ?").bind(workspace.to_string()).bind(dataset.to_string()).bind(after).bind(limit).fetch_all(&mut self.db).await?;
            return rows.into_iter().map(|r|Ok(json!({"version":r.try_get::<String,_>(0)?,"origin_workspace":workspace.to_string(),"dataset":dataset.to_string(),"metadata":serde_json::from_str::<Value>(&r.try_get::<String,_>(1)?).map_err(|_|StoreError::InvalidRequest)?,"availability":"not_verified","origin":"external"}))).collect();
        }
        let rows=sqlx::query("SELECT v.id,v.source_snapshot_id,v.published_at_us,v.artifact_digest,CASE WHEN length(a.schema_json)<=1048576 THEN a.schema_json END,v.attempt_id,v.import_id,a.row_count,a.byte_count,a.integrity_state,a.file_count FROM dataset_versions v JOIN datasets d ON d.id=v.dataset_id LEFT JOIN artifacts a ON a.digest=v.artifact_digest WHERE d.workspace_id=? AND d.id=? AND v.id>? ORDER BY v.id LIMIT ?").bind(workspace.to_string()).bind(dataset.to_string()).bind(after).bind(limit).fetch_all(&mut self.db).await?;
        rows.into_iter().map(|r|Ok(json!({"version":r.try_get::<String,_>(0)?,"source":r.try_get::<String,_>(1)?,"published_at_us":r.try_get::<i64,_>(2)?.to_string(),"artifact":r.try_get::<String,_>(3)?,"schema":serde_json::from_str::<Value>(&r.try_get::<String,_>(4)?).map_err(|_|StoreError::InvalidRequest)?,"attempt":r.try_get::<Option<String>,_>(5)?,"import":r.try_get::<Option<String>,_>(6)?,"row_count":r.try_get::<i64,_>(7)?.to_string(),"byte_count":r.try_get::<i64,_>(8)?.to_string(),"integrity_state":r.try_get::<String,_>(9)?,"file_count":r.try_get::<i64,_>(10)?.to_string(),"origin_workspace":workspace.to_string(),"dataset":dataset.to_string(),"availability":"not_verified","origin":"local"}))).collect()
    }
}
impl Reader {
    /// Bind a local exact version to its immutable source and, when available, accepted plan.
    pub async fn api_version_source(
        &mut self,
        workspace: WorkspaceId,
        dataset: DatasetId,
        version: tf_domain::VersionId,
    ) -> Result<(tf_domain::SourceSnapshotId, Option<DraftPlan>)> {
        let source:String=sqlx::query_scalar("SELECT v.source_snapshot_id FROM dataset_versions v JOIN datasets d ON d.id=v.dataset_id WHERE d.workspace_id=? AND d.id=? AND v.id=?").bind(workspace.to_string()).bind(dataset.to_string()).bind(version.to_string()).fetch_one(&mut self.db).await?;
        let plan:Option<String>=sqlx::query_scalar("SELECT id FROM build_plans WHERE source_snapshot_id=? AND disposition='ACCEPTED' ORDER BY id LIMIT 1").bind(&source).fetch_optional(&mut self.db).await?;
        let plan = if let Some(id) = plan {
            Some(
                self.api_plan(id.parse().map_err(|_| StoreError::InvalidRequest)?)
                    .await?,
            )
        } else {
            None
        };
        Ok((
            source.parse().map_err(|_| StoreError::InvalidRequest)?,
            plan,
        ))
    }
    /// Foreign evidence is metadata only, never an implicit local source capture.
    pub async fn api_foreign_version(
        &mut self,
        workspace: WorkspaceId,
        dataset: DatasetId,
        version: tf_domain::VersionId,
    ) -> Result<Value> {
        let text:String=sqlx::query_scalar("SELECT provenance_json FROM foreign_versions WHERE workspace_id=? AND dataset_id=? AND version_id=? AND length(provenance_json)<=1048576").bind(workspace.to_string()).bind(dataset.to_string()).bind(version.to_string()).fetch_one(&mut self.db).await?;
        serde_json::from_str(&text).map_err(|_| StoreError::InvalidRequest)
    }
}
impl Reader {
    /// Resolve metadata in the shared policy's ordered candidates. An existing bad artifact
    /// is returned with its integrity status; it never triggers opportunistic fallback.
    pub async fn api_head(
        &mut self,
        workspace: WorkspaceId,
        dataset: DatasetId,
        candidates: &[tf_domain::BranchName],
    ) -> Result<Option<Value>> {
        for branch in candidates {
            let row=sqlx::query("SELECT h.version_id,h.generation,v.source_snapshot_id,v.published_at_us,CASE WHEN length(a.schema_json)<=1048576 THEN a.schema_json END,a.row_count,a.byte_count,a.integrity_state FROM dataset_heads h JOIN data_branches b ON b.id=h.branch_id LEFT JOIN dataset_versions v ON v.id=h.version_id LEFT JOIN artifacts a ON a.digest=v.artifact_digest WHERE b.workspace_id=? AND b.name=? AND b.deleted_at_us IS NULL AND h.dataset_id=?").bind(workspace.to_string()).bind(branch.as_str()).bind(dataset.to_string()).fetch_optional(&mut self.db).await?;
            if let Some(r) = row {
                return Ok(Some(
                    json!({"version":r.try_get::<String,_>(0)?,"generation":r.try_get::<i64,_>(1)?.to_string(),"source":r.try_get::<String,_>(2)?,"published_at_us":r.try_get::<i64,_>(3)?.to_string(),"schema":serde_json::from_str::<Value>(&r.try_get::<String,_>(4)?).map_err(|_|StoreError::InvalidRequest)?,"row_count":r.try_get::<i64,_>(5)?.to_string(),"byte_count":r.try_get::<i64,_>(6)?.to_string(),"integrity_state":r.try_get::<String,_>(7)?,"resolved_branch":branch.as_str(),"availability":"not_verified"}),
                ));
            }
        }
        Ok(None)
    }
}
impl Reader {
    /// Branch metadata keyset page; head/version bytes are never read.
    pub async fn api_branches(
        &mut self,
        workspace: WorkspaceId,
        after: &str,
        limit: u32,
        datasets: Option<&[DatasetId]>,
    ) -> Result<Vec<Value>> {
        if !(1..=201).contains(&limit) {
            return Err(StoreError::InvalidRequest);
        }
        let filter = datasets
            .map(|ids| {
                serde_json::to_string(&ids.iter().map(ToString::to_string).collect::<Vec<_>>())
            })
            .transpose()
            .map_err(|_| StoreError::InvalidRequest)?;
        let rows=sqlx::query("SELECT b.id,b.name,b.revision,b.deleted_at_us FROM data_branches b WHERE b.workspace_id=? AND b.id>? AND (? IS NULL OR EXISTS (SELECT 1 FROM dataset_heads h WHERE h.branch_id=b.id AND h.dataset_id IN (SELECT value FROM json_each(?)))) ORDER BY b.id LIMIT ?").bind(workspace.to_string()).bind(after).bind(&filter).bind(&filter).bind(limit).fetch_all(&mut self.db).await?;
        rows.into_iter().map(|r|Ok(json!({"id":r.try_get::<String,_>(0)?,"name":r.try_get::<String,_>(1)?,"revision":r.try_get::<i64,_>(2)?.to_string(),"deleted":r.try_get::<Option<i64>,_>(3)?.is_some()}))).collect()
    }
    /// Accepted builds for the requested branch, ordered by immutable identity.
    pub async fn api_builds(
        &mut self,
        workspace: WorkspaceId,
        branch: &tf_domain::BranchName,
        after: &str,
        limit: u32,
    ) -> Result<Vec<Value>> {
        if !(1..=201).contains(&limit) {
            return Err(StoreError::InvalidRequest);
        }
        let rows=sqlx::query("SELECT b.id,b.state,p.source_snapshot_id,b.created_at_us,b.finished_at_us,b.plan_id FROM builds b JOIN build_plans p ON p.id=b.plan_id JOIN source_snapshots s ON s.id=p.source_snapshot_id WHERE s.workspace_id=? AND json_extract(p.candidate_json,'$.output.name')=? AND b.id>? ORDER BY b.id LIMIT ?").bind(workspace.to_string()).bind(branch.as_str()).bind(after).bind(limit).fetch_all(&mut self.db).await?;
        rows.into_iter().map(|r|Ok(json!({"id":r.try_get::<String,_>(0)?,"state":r.try_get::<String,_>(1)?,"source":r.try_get::<String,_>(2)?,"created_us":r.try_get::<i64,_>(3)?.to_string(),"finished_us":r.try_get::<Option<i64>,_>(4)?.map(|v|v.to_string()),"plan":r.try_get::<String,_>(5)?,"branch":branch.as_str()}))).collect()
    }
}
