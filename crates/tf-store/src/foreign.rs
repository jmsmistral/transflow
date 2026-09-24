//! Immutable provider metadata and verified local replicas. No producer execution or heads.
use crate::{
    Store, StoreError,
    artifacts::VerifiedArtifact,
    input_resolution::ReadRequest,
    retention::{self, ReadLease, ReadTarget},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
use tf_domain::{DatasetKey, VersionId};
use tf_protocol::canonical::{ContentDigest, DigestKind, canonical_json};
type Result<T> = std::result::Result<T, StoreError>;
fn encoded(v: &Value) -> Result<String> {
    let bytes = canonical_json(v).map_err(|_| StoreError::InvalidRequest)?;
    if bytes.len() > 768 * 1024 {
        return Err(StoreError::InvalidRequest);
    }
    String::from_utf8(bytes).map_err(|_| StoreError::InvalidRequest)
}
fn parsed(s: String) -> Result<Value> {
    serde_json::from_str(&s).map_err(|_| StoreError::InvalidRequest)
}
impl Store {
    /// Safe retained metadata only: exact inputs and output certificate identities/outcomes,
    /// without sample rows, user logs, environment values or source file contents.
    pub async fn export_metadata(&mut self, key: DatasetKey, version: VersionId) -> Result<Value> {
        let row = sqlx::query("SELECT v.artifact_digest,v.source_snapshot_id,v.published_at_us,s.code_digest,a.manifest_json FROM dataset_versions v JOIN artifacts a ON a.digest=v.artifact_digest JOIN datasets d ON d.id=v.dataset_id JOIN source_snapshots s ON s.id=v.source_snapshot_id WHERE d.workspace_id=? AND d.id=? AND v.id=?")
            .bind(key.workspace_id().to_string()).bind(key.dataset_id().to_string()).bind(version.to_string()).fetch_optional(&mut self.db).await?.ok_or(StoreError::InvalidRequest)?;
        let origin:(String,String)=sqlx::query_as("SELECT b.id,b.name FROM head_changes h JOIN data_branches b ON b.id=h.branch_id JOIN events e ON e.id=h.event_id WHERE h.new_version_id=? ORDER BY e.sequence LIMIT 1").bind(version.to_string()).fetch_one(&mut self.db).await?;
        let inputs = sqlx::query("SELECT alias,origin_workspace_id,origin_dataset_id,origin_version_id,artifact_digest,declared_branch_json,starting_branch,resolved_branch,role,resolution_json FROM version_inputs WHERE output_version_id=? ORDER BY alias LIMIT 10001")
            .bind(version.to_string()).fetch_all(&mut self.db).await?;
        let checks = sqlx::query("SELECT c.fingerprint,r.definition_fingerprint,r.outcome,r.started_at_us,r.finished_at_us FROM check_certificates c JOIN check_results r ON r.id=c.result_id JOIN check_definitions d ON d.fingerprint=r.definition_fingerprint JOIN dataset_versions v ON v.attempt_id=r.attempt_id WHERE v.id=? AND d.phase='output' ORDER BY c.fingerprint LIMIT 10001")
            .bind(version.to_string()).fetch_all(&mut self.db).await?;
        if inputs.len() > 10000 || checks.len() > 10000 {
            return Err(StoreError::InvalidRequest);
        }
        let inputs = inputs.iter().map(|r| Ok(json!({"alias":r.try_get::<String,_>(0)?,"workspace":r.try_get::<String,_>(1)?,"dataset":r.try_get::<String,_>(2)?,"version":r.try_get::<String,_>(3)?,"artifact":r.try_get::<String,_>(4)?,"declared_branch":parsed(r.try_get(5)?)?,"starting_branch":r.try_get::<String,_>(6)?,"resolved_branch":r.try_get::<String,_>(7)?,"role":r.try_get::<String,_>(8)?,"resolution":parsed(r.try_get(9)?)?}))).collect::<Result<Vec<_>>>()?;
        let checks = checks.iter().map(|r| Ok(json!({"fingerprint":r.try_get::<String,_>(0)?,"definition":r.try_get::<String,_>(1)?,"outcome":r.try_get::<String,_>(2)?,"started_us":r.try_get::<i64,_>(3)?.to_string(),"finished_us":r.try_get::<Option<i64>,_>(4)?.map(|n|n.to_string())}))).collect::<Result<Vec<_>>>()?;
        let value = json!({"workspace":key.workspace_id().to_string(),"dataset":key.dataset_id().to_string(),"version":version.to_string(),"artifact":row.try_get::<String,_>(0)?,"manifest":parsed(row.try_get(4)?)?,"source":{"id":row.try_get::<String,_>(1)?,"digest":row.try_get::<String,_>(3)?,"availability":"metadata_only"},"origin_branch":{"id":origin.0,"name":origin.1},"published_us":row.try_get::<i64,_>(2)?.to_string(),"inputs":inputs,"output_certificates":checks});
        encoded(&value)?;
        Ok(value)
    }
    /// Persist the copy journal before byte access. Repeated identities must have identical
    /// immutable metadata; a different provider clone cannot rewrite an existing origin.
    pub async fn begin_replica(
        &mut self,
        key: DatasetKey,
        version: VersionId,
        metadata: &Value,
    ) -> Result<()> {
        if metadata["workspace"] != key.workspace_id().to_string()
            || metadata["dataset"] != key.dataset_id().to_string()
            || metadata["version"] != version.to_string()
        {
            return Err(StoreError::InvalidRequest);
        }
        let digest = metadata["artifact"]
            .as_str()
            .ok_or(StoreError::InvalidRequest)?;
        ContentDigest::from_hex(DigestKind::Artifact, digest)
            .map_err(|_| StoreError::InvalidRequest)?;
        let value = encoded(metadata)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("INSERT INTO foreign_versions(workspace_id,dataset_id,version_id,provenance_json,manifest_digest,availability) VALUES(?,?,?,?,?,'COPYING') ON CONFLICT DO NOTHING").bind(key.workspace_id().to_string()).bind(key.dataset_id().to_string()).bind(version.to_string()).bind(&value).bind(digest).execute(&mut *tx).await?;
        let old: (String,String) = sqlx::query_as("SELECT provenance_json,manifest_digest FROM foreign_versions WHERE workspace_id=? AND dataset_id=? AND version_id=?").bind(key.workspace_id().to_string()).bind(key.dataset_id().to_string()).bind(version.to_string()).fetch_one(&mut *tx).await?;
        // Branch names are mutable display labels, unlike branch UUID/content/provenance.
        // Keep the first observed label for offline history; a provider rename is not a fork.
        let mut prior = parsed(old.0)?;
        let label = metadata["origin_branch"]["name"]
            .as_str()
            .ok_or(StoreError::InvalidRequest)?;
        label
            .parse::<tf_domain::BranchName>()
            .map_err(|_| StoreError::InvalidRequest)?;
        prior
            .get_mut("origin_branch")
            .and_then(Value::as_object_mut)
            .ok_or(StoreError::InvalidRequest)?
            .insert("name".into(), json!(label));
        if encoded(&prior)? != value || old.1 != digest {
            return Err(StoreError::InvalidRequest);
        }
        tx.commit().await?;
        Ok(())
    }
    /// Atomically make a durable, strictly verified artifact/provenance replica usable and
    /// acquire its local read pin. The provider lease must still be held by the caller.
    pub async fn finish_replica(
        &mut self,
        key: DatasetKey,
        version: VersionId,
        artifact: &VerifiedArtifact,
        request: ReadRequest,
    ) -> Result<ReadLease> {
        if self.path.parent() != Some(artifact.workspace().join(".transflow/runtime").as_path()) {
            return Err(StoreError::InvalidRequest);
        }
        let manifest = artifact.manifest();
        let value = encoded(manifest)?;
        let files = manifest["files"]
            .as_array()
            .ok_or(StoreError::InvalidRequest)?;
        let sum = |field: &str| {
            files.iter().try_fold(0i64, |n, f| {
                n.checked_add(
                    f[field]
                        .as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .ok_or(StoreError::InvalidRequest)?,
                )
                .ok_or(StoreError::InvalidRequest)
            })
        };
        let expires = request
            .now_us
            .checked_add(request.ttl_us)
            .filter(|_| request.ttl_us > 0)
            .ok_or(StoreError::InvalidRequest)?;
        retention::clock(&mut self.db, request.now_us)
            .await
            .map_err(|_| StoreError::InvalidRequest)?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let metadata:String=sqlx::query_scalar("SELECT provenance_json FROM foreign_versions WHERE workspace_id=? AND dataset_id=? AND version_id=? AND manifest_digest=?").bind(key.workspace_id().to_string()).bind(key.dataset_id().to_string()).bind(version.to_string()).bind(artifact.digest().hex()).fetch_optional(&mut *tx).await?.ok_or(StoreError::InvalidRequest)?;
        sqlx::query("INSERT INTO artifacts(digest,manifest_json,schema_json,file_count,row_count,byte_count,integrity_state) VALUES(?,?,?,?,?,?,'VERIFIED') ON CONFLICT DO NOTHING").bind(artifact.digest().hex()).bind(&value).bind(encoded(&manifest["logical_schema"])?).bind(files.len() as i64).bind(sum("row_count")?).bind(sum("byte_length")?).execute(&mut *tx).await?;
        let stored: String =
            sqlx::query_scalar("SELECT manifest_json FROM artifacts WHERE digest=?")
                .bind(artifact.digest().hex())
                .fetch_one(&mut *tx)
                .await?;
        if stored != value {
            return Err(StoreError::InvalidRequest);
        }
        let changed=sqlx::query("INSERT INTO replicas(workspace_id,dataset_id,version_id,artifact_digest,provenance_json,copy_state,retention_json) VALUES(?,?,?,?,?,'VERIFIED','{}') ON CONFLICT(workspace_id,dataset_id,version_id) DO UPDATE SET copy_state='VERIFIED' WHERE replicas.artifact_digest=excluded.artifact_digest AND replicas.provenance_json=excluded.provenance_json").bind(key.workspace_id().to_string()).bind(key.dataset_id().to_string()).bind(version.to_string()).bind(artifact.digest().hex()).bind(metadata).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(StoreError::InvalidRequest);
        }
        sqlx::query("UPDATE foreign_versions SET availability='VERIFIED' WHERE workspace_id=? AND dataset_id=? AND version_id=?").bind(key.workspace_id().to_string()).bind(key.dataset_id().to_string()).bind(version.to_string()).execute(&mut *tx).await?;
        let lease = retention::lease_in_transaction(
            &mut tx,
            request.lease,
            request.operation,
            request.kind,
            ReadTarget::Artifact(artifact.digest()),
            request.now_us,
            expires,
        )
        .await
        .map_err(|_| StoreError::InvalidRequest)?;
        tx.commit().await?;
        Ok(lease)
    }
    /// Retained provenance remains inspectable independently of artifact availability.
    /// This is never sufficient to grant a data read or satisfy a latest-head request.
    pub async fn foreign_metadata(
        &mut self,
        key: DatasetKey,
        version: VersionId,
    ) -> Result<Option<Value>> {
        let value:Option<String>=sqlx::query_scalar("SELECT provenance_json FROM foreign_versions WHERE workspace_id=? AND dataset_id=? AND version_id=?").bind(key.workspace_id().to_string()).bind(key.dataset_id().to_string()).bind(version.to_string()).fetch_optional(&mut self.db).await?;
        value.map(parsed).transpose()
    }
    /// A verified replica is local availability, never evidence of the provider's latest head.
    pub async fn replica_metadata(
        &mut self,
        key: DatasetKey,
        version: VersionId,
    ) -> Result<Option<Value>> {
        let value:Option<String>=sqlx::query_scalar("SELECT r.provenance_json FROM replicas r JOIN artifacts a ON a.digest=r.artifact_digest WHERE r.workspace_id=? AND r.dataset_id=? AND r.version_id=? AND r.copy_state='VERIFIED' AND NOT EXISTS(SELECT 1 FROM artifact_gc_claims g WHERE g.digest=r.artifact_digest)").bind(key.workspace_id().to_string()).bind(key.dataset_id().to_string()).bind(version.to_string()).fetch_optional(&mut self.db).await?;
        value.map(parsed).transpose()
    }
}
