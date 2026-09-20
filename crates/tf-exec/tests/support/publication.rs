#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Shared synthetic publication fixture"
)]
use serde_json::json;
use sqlx::{Connection, SqliteConnection};
use std::{
    fs::{self, File, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_domain::execution::*;
use tf_domain::*;
use tf_exec::{
    ownership::{CoordinatorMode, RuntimeOwner},
    publication::{self, MaterializedFiles},
};
use tf_store::{
    artifacts::ArtifactStore,
    publication::{PublicationContract, PublicationRequest},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
pub fn workspace() -> WorkspaceId {
    WorkspaceId::from_bytes([1; 16])
}
pub fn branch() -> BranchId {
    BranchId::from_bytes([2; 16])
}
pub fn source() -> SourceSnapshotId {
    SourceSnapshotId::from_bytes([3; 16])
}
pub fn writer() -> serde_json::Value {
    json!({"engine":"import","version":"fixture","compression":"uncompressed","row_group_size":"8192"})
}
pub fn contract() -> PublicationContract {
    PublicationContract {
        compute_fingerprint: "a".repeat(64),
        check_fingerprint: "b".repeat(64),
        inputs: vec![],
        checks: vec![],
    }
}
pub struct Harness {
    pub root: PathBuf,
    pub owner: Option<RuntimeOwner>,
    pub digest: tf_protocol::canonical::ContentDigest,
    pub db: SqliteConnection,
}
impl Harness {
    pub async fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tf-publication-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self::at(root).await
    }
    pub async fn at(root: PathBuf) -> Self {
        fs::create_dir_all(root.join(".transflow/runtime")).unwrap();
        fs::set_permissions(
            root.join(".transflow/runtime"),
            Permissions::from_mode(0o700),
        )
        .unwrap();
        let root = root.canonicalize().unwrap();
        fs::create_dir_all(root.join("source")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/import/two-rows.parquet"),
            root.join("source/one"),
        )
        .unwrap();
        let mut owner =
            RuntimeOwner::acquire(&root, workspace(), CoordinatorMode::Temporary).unwrap();
        let mut store = owner.open_store().await.unwrap();
        store
            .repository()
            .unwrap()
            .register_workspace(workspace(), "fixture", 1)
            .await
            .unwrap();
        store.close().await.unwrap();
        publication::recover(&mut owner, 2).await.unwrap();
        let digest = {
            let store = ArtifactStore::open(&root).unwrap();
            store
                .prepare(
                    &File::open(root.join("source")).unwrap(),
                    &["one".into()],
                    writer(),
                    RequestId::from_bytes([250; 16]),
                    |_| Ok(()),
                )
                .unwrap()
                .digest()
                .unwrap()
        };
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(root.join(".transflow/runtime/catalog.sqlite"))
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full);
        let mut db = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,'master',0)").bind(branch().to_string()).bind(workspace().to_string()).execute(&mut db).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO source_snapshots(id,workspace_id,code_digest,selector_json,manifest_json,environment_json) VALUES(?,?,'code','{}','{}','{}')").bind(source().to_string()).bind(workspace().to_string()).execute(&mut db).await.unwrap();
        Self {
            root,
            owner: Some(owner),
            digest,
            db,
        }
    }
    pub fn session(&self) -> CoordinatorSessionId {
        self.owner
            .as_ref()
            .unwrap()
            .registration()
            .session()
            .unwrap()
    }
    pub fn files(&self, n: u8) -> MaterializedFiles {
        MaterializedFiles {
            root: File::open(self.root.join("source")).unwrap(),
            files: vec!["one".into()],
            writer: writer(),
            staging: RequestId::from_bytes([n; 16]),
        }
    }
    pub async fn seed(&mut self, n: u8, dataset: u8) -> Job {
        let id = RequestId::from_bytes([n; 16]).to_string();
        let dataset = DatasetId::from_bytes([dataset; 16]);
        let session = self.session();
        sqlx::query("INSERT OR IGNORE INTO datasets VALUES(?,?,1,NULL)")
            .bind(dataset.to_string())
            .bind(workspace().to_string())
            .execute(&mut self.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO build_plans(id,disposition,source_snapshot_id,candidate_json,registry_diff_json,context_json,targets_json,bindings_json,guards_json,digest) VALUES(?,'ACCEPTED',?,'{}','{}','{}','[]','[]','{}','plan')").bind(&id).bind(source().to_string()).execute(&mut self.db).await.unwrap();
        sqlx::query("INSERT INTO builds(id,plan_id,trigger_json,requested_by,state,created_at_us) VALUES(?,?,'{}','fixture','RUNNING',1)").bind(&id).bind(&id).execute(&mut self.db).await.unwrap();
        sqlx::query("INSERT INTO jobs(id,build_id,dataset_id,branch_id,state,bindings_json) VALUES(?,?,?,?,'STARTING','[]')").bind(&id).bind(&id).bind(dataset.to_string()).bind(branch().to_string()).execute(&mut self.db).await.unwrap();
        sqlx::query("INSERT INTO attempts(id,job_id,attempt_no,session_id,fence,state,started_at_us) VALUES(?,?,1,?,1,'STARTING',1)").bind(&id).bind(&id).bind(session.to_string()).execute(&mut self.db).await.unwrap();
        Job::new(
            JobId::from_bytes([n; 16]),
            BuildId::from_bytes([n; 16]),
            ExecutionBinding {
                plan: PlanId::from_bytes([n; 16]),
                source: source(),
            },
            OutputTarget {
                dataset: DatasetKey::new(workspace(), dataset),
                branch: branch(),
                expected_generation: 0,
            },
            Fence {
                session,
                generation: 1,
            },
            RetryPolicy::default(),
            EventTime(0),
        )
    }
    pub async fn start(
        &mut self,
        n: u8,
        dataset: u8,
        generation: u64,
        c: PublicationContract,
    ) -> PublicationRequest {
        self.start_with_evidence(n, dataset, generation, c, None)
            .await
    }
    pub async fn start_with_evidence(
        &mut self,
        n: u8,
        dataset: u8,
        generation: u64,
        c: PublicationContract,
        evidence: Option<serde_json::Value>,
    ) -> PublicationRequest {
        let mut job = self.seed(n, dataset).await;
        let target = OutputTarget {
            dataset: DatasetKey::new(workspace(), DatasetId::from_bytes([dataset; 16])),
            branch: branch(),
            expected_generation: generation,
        };
        if generation != 0 {
            job = Job::new(
                job.id(),
                job.build(),
                job.binding(),
                target,
                job.fence(),
                RetryPolicy::default(),
                EventTime(0),
            );
        }
        let session = self.session();
        let mut store = self.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        repo.reserve_publications(workspace(), job.build(), job.fence(), &[target], 2)
            .await
            .unwrap();
        repo.freeze_publication_contract(workspace(), session, job.id(), &c)
            .await
            .unwrap();
        if let Some(evidence) = evidence {
            repo.freeze_computation_evidence(
                &tf_store::cache::Request {
                    build: job.build(),
                    job: job.id(),
                    binding: job.binding(),
                    target,
                    fence: job.fence(),
                    contract: c.clone(),
                    at_us: 2,
                },
                &evidence,
            )
            .await
            .unwrap();
        }
        store.close().await.unwrap();
        let attempt = AttemptId::from_bytes([n; 16]);
        job.queue(job.fence(), EventTime(1)).unwrap();
        job.start_attempt(attempt, job.fence(), EventTime(2))
            .unwrap();
        for (k, p) in [
            Phase::ValidatingInputs,
            Phase::Running,
            Phase::Materializing,
            Phase::ValidatingOutputs,
        ]
        .into_iter()
        .enumerate()
        {
            job.advance(attempt, job.fence(), p, EventTime(k as u64 + 3))
                .unwrap();
        }
        sqlx::query("UPDATE jobs SET state='VALIDATING_OUTPUTS' WHERE id=?")
            .bind(job.id().to_string())
            .execute(&mut self.db)
            .await
            .unwrap();
        sqlx::query("UPDATE attempts SET state='VALIDATING_OUTPUTS' WHERE id=?")
            .bind(attempt.to_string())
            .execute(&mut self.db)
            .await
            .unwrap();
        let intent = job
            .prepare_publication(
                attempt,
                job.fence(),
                VersionId::from_bytes([n; 16]),
                self.digest.hex().parse().unwrap(),
                EventTime(7),
            )
            .unwrap();
        PublicationRequest {
            intent,
            contract: c,
            at_us: 20,
        }
    }
    pub async fn publish(
        &mut self,
        r: PublicationRequest,
        n: u8,
    ) -> Result<tf_store::publication::PublicationReceipt, publication::Error> {
        let files = self.files(n);
        let completion = publication::publish(self.owner.take().unwrap(), r, files, |_| Ok(()))
            .await
            .unwrap();
        self.owner = Some(completion.owner);
        completion.result
    }
    pub async fn scalar(&mut self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(&mut self.db)
            .await
            .unwrap()
    }
    pub async fn finish(&mut self, n: u8, state: &str) {
        sqlx::query("UPDATE builds SET state=?,finished_at_us=30 WHERE id=?")
            .bind(state)
            .bind(BuildId::from_bytes([n; 16]).to_string())
            .execute(&mut self.db)
            .await
            .unwrap();
        let session = self.session();
        let mut store = self.owner.as_mut().unwrap().open_store().await.unwrap();
        store
            .repository()
            .unwrap()
            .release_publications(workspace(), session, BuildId::from_bytes([n; 16]))
            .await
            .unwrap();
        store.close().await.unwrap();
    }
}
pub fn writable(path: &Path) {
    let m = fs::symlink_metadata(path).unwrap();
    if m.is_symlink() {
        return;
    }
    if m.is_dir() {
        fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
        for e in fs::read_dir(path).unwrap() {
            writable(&e.unwrap().path());
        }
    } else {
        fs::set_permissions(path, Permissions::from_mode(0o600)).unwrap();
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.owner.take();
        writable(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}
