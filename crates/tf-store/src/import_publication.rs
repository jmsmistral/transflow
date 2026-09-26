//! Explicit imports have import provenance, never fabricated transform attempts or check PASSes.
use crate::{
    Store,
    artifacts::VerifiedArtifact,
    planning::HeadGuard,
    publication::{self, PublicationError, PublicationReceipt},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
use tf_domain::{
    BranchId, CoordinatorSessionId, DatasetKey, RequestId, SourceSnapshotId, VersionId,
    execution::OutputTarget,
};
use tf_protocol::canonical::{DigestKind, canonical_json, content_digest};

/// Captured authoring context and a head guard, checked by the runtime owner before commit.
pub struct ImportPublication {
    /// Registered imported identity.
    pub dataset: DatasetKey,
    /// Explicit import operation, independent of the published version.
    pub import: RequestId,
    /// New immutable version.
    pub version: VersionId,
    /// Current real runtime owner.
    pub session: CoordinatorSessionId,
    /// Original branch/head revision, including absence.
    pub head: HeadGuard,
    /// Branch identity to allocate only if still absent.
    pub new_branch: BranchId,
    /// Verified capture with final registered IDs.
    pub source: SourceSnapshotId,
    /// Exact captured source digest.
    pub source_digest: String,
    /// Captured source file hashes and explicit import provenance.
    pub manifest: Value,
    /// Optional captured Git provenance.
    pub git: Value,
    /// Matched environment evidence.
    pub environment: Value,
    /// UTC microseconds for this publication.
    pub at_us: i64,
}
fn encoded(value: &Value) -> Result<String, PublicationError> {
    String::from_utf8(canonical_json(value).map_err(|_| PublicationError::Evidence)?)
        .map_err(|_| PublicationError::Evidence)
}
impl Store {
    /// Publish a strictly verified object and import provenance in one visibility transaction.
    /// Filesystem installation is completed before entering; a crash leaves at most an orphan.
    pub async fn commit_import(
        &mut self,
        r: &ImportPublication,
        artifact: &VerifiedArtifact,
    ) -> Result<PublicationReceipt, PublicationError> {
        if r.at_us < 0
            || r.head.dataset.as_deref() != Some(&r.dataset.dataset_id().to_string())
            || r.head.deleted
            || r.head.generation < 0
            || r.head.name.parse::<tf_domain::BranchName>().is_err()
            || self.path.parent() != Some(artifact.workspace().join(".transflow/runtime").as_path())
            || tf_protocol::canonical::ContentDigest::from_hex(DigestKind::Source, &r.source_digest)
                .is_err()
        {
            return Err(PublicationError::Evidence);
        }
        let current = self
            .plan_guard(
                r.dataset.workspace_id(),
                &r.head.name,
                Some(r.dataset.dataset_id()),
            )
            .await
            .map_err(|_| PublicationError::Conflict)?;
        if current != r.head {
            return Err(PublicationError::Conflict);
        }
        let manifest = artifact.manifest();
        let files = manifest["files"]
            .as_array()
            .ok_or(PublicationError::Evidence)?;
        let sum = |key: &str| {
            files.iter().try_fold(0i64, |n, f| {
                n.checked_add(
                    f[key]
                        .as_str()
                        .and_then(|v| v.parse::<i64>().ok())
                        .ok_or(PublicationError::Evidence)?,
                )
                .ok_or(PublicationError::Evidence)
            })
        };
        let rows = sum("row_count")?;
        let bytes = sum("byte_length")?;
        let encoded_manifest = encoded(manifest)?;
        let compute = content_digest(DigestKind::Compute, &json!({"kind":"import","artifact":artifact.digest().hex(),"source":r.source.to_string()})).map_err(|_| PublicationError::Evidence)?.hex();
        let checks = content_digest(DigestKind::Compute, &json!({"kind":"import","checks":[]}))
            .map_err(|_| PublicationError::Evidence)?
            .hex();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        publication::authority(&mut tx, r.dataset.workspace_id(), r.session).await?;
        // Repeat branch/head CAS within the visibility transaction, including rename/deletion.
        let branch_row = sqlx::query(
            "SELECT id,revision,deleted_at_us FROM data_branches WHERE workspace_id=? AND name=?",
        )
        .bind(r.dataset.workspace_id().to_string())
        .bind(&r.head.name)
        .fetch_optional(&mut *tx)
        .await?;
        let branch = match (branch_row, &r.head.branch) {
            (Some(row), Some(expected))
                if row.try_get::<String, _>(0)? == *expected
                    && Some(row.try_get::<i64, _>(1)?) == r.head.revision
                    && row.try_get::<Option<i64>, _>(2)?.is_none() =>
            {
                expected
                    .parse::<BranchId>()
                    .map_err(|_| PublicationError::Evidence)?
            }
            (None, None) => {
                sqlx::query(
                    "INSERT INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,?,0)",
                )
                .bind(r.new_branch.to_string())
                .bind(r.dataset.workspace_id().to_string())
                .bind(&r.head.name)
                .execute(&mut *tx)
                .await?;
                r.new_branch
            }
            _ => return Err(PublicationError::Conflict),
        };
        let valid:i64=sqlx::query_scalar("SELECT count(*) FROM datasets WHERE id=? AND workspace_id=? AND tombstoned_at_us IS NULL").bind(r.dataset.dataset_id().to_string()).bind(r.dataset.workspace_id().to_string()).fetch_one(&mut *tx).await?;
        let occupied: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM write_reservations WHERE branch_id=? AND dataset_id=?",
        )
        .bind(branch.to_string())
        .bind(r.dataset.dataset_id().to_string())
        .fetch_one(&mut *tx)
        .await?;
        let collecting: i64 =
            sqlx::query_scalar("SELECT count(*) FROM artifact_gc_claims WHERE digest=?")
                .bind(artifact.digest().hex())
                .fetch_one(&mut *tx)
                .await?;
        if valid != 1 || occupied != 0 || collecting != 0 {
            return Err(PublicationError::Fence);
        }
        let existing: Option<(String, i64)> = sqlx::query_as(
            "SELECT version_id,generation FROM dataset_heads WHERE branch_id=? AND dataset_id=?",
        )
        .bind(branch.to_string())
        .bind(r.dataset.dataset_id().to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if existing.as_ref().map(|x| &x.0) != r.head.version.as_ref()
            || existing.as_ref().map_or(0, |x| x.1) != r.head.generation
        {
            return Err(PublicationError::Conflict);
        }
        // Snapshot identities are immutable; each import captures a fresh source ID.
        sqlx::query("INSERT INTO source_snapshots(id,workspace_id,code_digest,selector_json,git_json,manifest_json,environment_json) VALUES(?,?,?,'{\"kind\":\"import\"}',?,?,?)")
            .bind(r.source.to_string()).bind(r.dataset.workspace_id().to_string()).bind(&r.source_digest).bind(encoded(&r.git)?).bind(encoded(&r.manifest)?).bind(encoded(&r.environment)?).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO artifacts(digest,manifest_json,schema_json,file_count,row_count,byte_count,integrity_state) VALUES(?,?,?,?,?,?,'VERIFIED') ON CONFLICT(digest) DO NOTHING")
            .bind(artifact.digest().hex()).bind(&encoded_manifest).bind(encoded(&manifest["logical_schema"])?).bind(files.len() as i64).bind(rows).bind(bytes).execute(&mut *tx).await?;
        let stored: String =
            sqlx::query_scalar("SELECT manifest_json FROM artifacts WHERE digest=?")
                .bind(artifact.digest().hex())
                .fetch_one(&mut *tx)
                .await?;
        if stored != encoded_manifest {
            return Err(PublicationError::Evidence);
        }
        sqlx::query("INSERT INTO dataset_versions(id,dataset_id,artifact_digest,import_id,source_snapshot_id,published_at_us,compute_fingerprint,check_fingerprint) VALUES(?,?,?,?,?,?,?,?)")
            .bind(r.version.to_string()).bind(r.dataset.dataset_id().to_string()).bind(artifact.digest().hex()).bind(r.import.to_string()).bind(r.source.to_string()).bind(r.at_us).bind(compute).bind(checks).execute(&mut *tx).await?;
        let receipt = publication::publish_head(
            &mut tx,
            OutputTarget {
                dataset: r.dataset,
                branch,
                expected_generation: r.head.generation as u64,
            },
            r.version,
            artifact.digest().hex(),
            r.import.to_string(),
            r.import.to_string(),
            r.at_us,
        )
        .await?;
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('dataset.import',?,?)").bind(encoded(&json!({"import":r.import.to_string(),"version":r.version.to_string(),"branch":r.head.name,"source":r.source.to_string()}))?).bind(r.at_us).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(receipt)
    }
}
