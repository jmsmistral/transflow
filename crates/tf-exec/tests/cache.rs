//! Cache qualification uses real SQLite, immutable Parquet and the production publication service.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Synthetic integration assertions"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::json;
use tf_domain::{execution::*, *};
use tf_store::{
    cache::{self, Lookup},
    publication::*,
};

async fn request(
    h: &mut Harness,
    n: u8,
    generation: u64,
    c: PublicationContract,
) -> cache::Request {
    let job = h.seed(n, 10).await;
    sqlx::query("DELETE FROM attempts WHERE job_id=?")
        .bind(job.id().to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='PLANNED' WHERE id=?")
        .bind(job.id().to_string())
        .execute(&mut h.db)
        .await
        .unwrap();
    let target = OutputTarget {
        dataset: DatasetKey::new(workspace(), DatasetId::from_bytes([10; 16])),
        branch: branch(),
        expected_generation: generation,
    };
    let r = cache::Request {
        build: job.build(),
        job: job.id(),
        binding: job.binding(),
        target,
        fence: job.fence(),
        contract: c,
        at_us: 40,
    };
    let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
    let store = owned.repository().unwrap();
    store
        .reserve_publications(workspace(), r.build, r.fence, &[target], 40)
        .await
        .unwrap();
    store.freeze_cache_contract(&r).await.unwrap();
    owned.close().await.unwrap();
    r
}
async fn publish(h: &mut Harness, n: u8, generation: u64, c: PublicationContract) {
    let r = h.start(n, 10, generation, c).await;
    h.publish(r, n).await.unwrap();
    h.finish(n, "SUCCEEDED").await;
}
async fn reuse(
    h: &mut Harness,
    r: cache::Request,
) -> Result<Option<cache::Receipt>, tf_exec::cache::Error> {
    let lease = RequestId::from_bytes(*r.job.as_bytes());
    let done = tf_exec::cache::reuse(h.owner.take().unwrap(), r, lease, || Ok(40))
        .await
        .unwrap();
    h.owner = Some(done.owner);
    done.result
}
#[test]
fn unchanged_selected_job_reuses_original_version_without_attempt_or_new_event() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0, contract()).await;
        let r = request(&mut h, 20, 1, contract()).await;
        let receipt = reuse(&mut h, r.clone()).await.unwrap().unwrap();
        assert_eq!(receipt.version, VersionId::from_bytes([10; 16]));
        assert_eq!(receipt.original_attempt, AttemptId::from_bytes([10; 16]));
        assert_eq!(receipt.generation, 1);
        assert!(receipt.event.is_none());
        assert_eq!(reuse(&mut h, r).await.unwrap(), Some(receipt));
        for query in [
            "SELECT count(*) FROM attempts",
            "SELECT count(*) FROM dataset_versions",
            "SELECT count(*) FROM events",
            "SELECT count(*) FROM jobs WHERE state='CACHED'",
            "SELECT count(*) FROM cached_jobs",
            "SELECT count(*) FROM audit_log WHERE operation='cache_reuse'",
        ] {
            assert_eq!(h.scalar(query).await, 1, "{query}");
        }
        assert_eq!(
            h.scalar("SELECT count(*) FROM read_leases WHERE released=0")
                .await,
            0
        );
        assert_eq!(
            h.scalar("SELECT published_at_us FROM dataset_versions")
                .await,
            20
        );
        assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await, 1);
    });
}
#[test]
fn older_matching_version_adoption_is_atomic_audited_and_emits_only_head_change() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0, contract()).await;
        let mut other = contract();
        other.compute_fingerprint = "c".repeat(64);
        publish(&mut h, 11, 1, other).await;
        let r = request(&mut h, 20, 2, contract()).await;
        let receipt = reuse(&mut h, r.clone()).await.unwrap().unwrap();
        assert_eq!(receipt.version, VersionId::from_bytes([10; 16]));
        assert_eq!(receipt.generation, 3);
        assert!(receipt.event.is_some());
        assert_eq!(reuse(&mut h, r).await.unwrap(), Some(receipt));
        assert_eq!(
            h.scalar("SELECT count(*) FROM events WHERE type='dataset.published'")
                .await,
            2
        );
        assert_eq!(
            h.scalar("SELECT count(*) FROM events WHERE type='dataset.head_changed'")
                .await,
            1
        );
        assert_eq!(h.scalar("SELECT count(*) FROM outbox_deliveries").await, 3);
        assert_eq!(h.scalar("SELECT count(*) FROM artifacts").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 2);
        assert_eq!(h.scalar("SELECT count(*) FROM attempts").await, 2);
        assert_eq!(
            h.scalar("SELECT count(*) FROM head_changes WHERE cause='cache_reuse'")
                .await,
            1
        );
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let parent = owned
            .repository()
            .unwrap()
            .completed_parent(
                BuildId::from_bytes([20; 16]),
                DatasetId::from_bytes([10; 16]),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parent.0, VersionId::from_bytes([10; 16]));
        owned.close().await.unwrap();
    });
}
#[test]
fn branch_scope_compute_check_and_normalization_mismatches_cannot_hit() {
    runtime().block_on(async {
        for change in ["compute","checks","branch"] {
            let mut h=Harness::new().await;publish(&mut h,10,0,contract()).await;
            let mut c=contract();match change {"compute"=>c.compute_fingerprint="d".repeat(64),"checks"=>c.check_fingerprint="d".repeat(64),_=>{}}
            let r=request(&mut h,20,1,c).await;
            if change=="branch" {
                sqlx::query("INSERT INTO data_branches(id,workspace_id,name,revision) VALUES(?,?,'other',0)").bind(BranchId::from_bytes([9;16]).to_string()).bind(workspace().to_string()).execute(&mut h.db).await.unwrap();
                sqlx::query("UPDATE jobs SET branch_id=? WHERE id=?").bind(BranchId::from_bytes([9;16]).to_string()).bind(JobId::from_bytes([10;16]).to_string()).execute(&mut h.db).await.unwrap();
            }
            assert!(reuse(&mut h,r).await.unwrap().is_none(),"{change}");assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await,1);assert_eq!(h.scalar("SELECT count(*) FROM cached_jobs").await,0);
        }
    });
}
#[test]
fn warning_certificates_retain_ids_outcomes_and_original_timestamps() {
    runtime().block_on(async {
        let mut h=Harness::new().await;let definition="c".repeat(64);
        sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'output',NULL,'check','{\"severity\":\"WARN\"}')").bind(&definition).execute(&mut h.db).await.unwrap();
        let mut c=contract();c.checks.push(CheckRequirement{definition:definition.clone(),subject:CheckSubject::Output,required:false});
        let p=h.start(10,10,0,c.clone()).await;
        sqlx::query("INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,outcome,metrics_json,started_at_us,finished_at_us) VALUES(?,?,?,?,'VIOLATION','{}',5,10)").bind(RequestId::from_bytes([50;16]).to_string()).bind(p.intent.attempt().to_string()).bind(json!({"artifact_digest":h.digest.hex()}).to_string()).bind(&definition).execute(&mut h.db).await.unwrap();
        h.publish(p,10).await.unwrap();h.finish(10,"SUCCEEDED").await;
        let r=request(&mut h,20,1,c.clone()).await;assert!(reuse(&mut h,r).await.unwrap().is_some());h.finish(20,"SUCCEEDED").await;
        assert_eq!(h.scalar("SELECT count(*) FROM validation_reuse").await,1);assert_eq!(h.scalar("SELECT count(*) FROM check_results WHERE outcome='VIOLATION' AND started_at_us=5 AND finished_at_us=10").await,1);
        // Real policy changes affect both fingerprints and normalized definitions.
        let strict="e".repeat(64);sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'output',NULL,'check','{\"severity\":\"FAIL\"}')").bind(&strict).execute(&mut h.db).await.unwrap();
        c.check_fingerprint="d".repeat(64);c.compute_fingerprint="f".repeat(64);c.checks[0]=CheckRequirement{definition:strict,subject:CheckSubject::Output,required:true};
        let r=request(&mut h,21,1,c).await;assert!(reuse(&mut h,r).await.unwrap().is_none());
        assert_eq!(h.scalar("SELECT count(*) FROM validation_reuse").await,1);
    });
}
#[test]
fn cancellation_generation_fence_terminal_and_force_guards_preserve_head() {
    runtime().block_on(async {
        for change in ["cancel","generation","fence","terminal","force","contract","attempt","deleted","incomplete"] {
            let mut h=Harness::new().await;publish(&mut h,10,0,contract()).await;
            let mut r=request(&mut h,20,1,contract()).await;
            let sql=match change {
                "cancel"=>"UPDATE builds SET cancel_requested=1 WHERE state='RUNNING'",
                "generation"=>"UPDATE dataset_heads SET generation=generation+1",
                "fence"=>"UPDATE write_reservations SET fence=fence+1",
                "terminal"=>"UPDATE jobs SET state='FAILED' WHERE state='PLANNED'",
                "force"=>"UPDATE build_plans SET context_json='{\"force\":true}'",
                "attempt"=>"INSERT INTO attempts SELECT j.id,j.id,1,NULL,w.session_id,w.fence,'STARTING',NULL,40,NULL,NULL FROM jobs j JOIN write_reservations w ON w.build_id=j.build_id",
                "deleted"=>"UPDATE data_branches SET deleted_at_us=40",
                "incomplete"=>"DELETE FROM write_reservations",
                _=>{r.contract.compute_fingerprint="e".repeat(64);"SELECT 1"}
            };
            sqlx::query(sql).execute(&mut h.db).await.unwrap();
            assert!(reuse(&mut h,r).await.is_err(),"{change}");assert_eq!(h.scalar("SELECT count(*) FROM cached_jobs").await,0);assert_eq!(h.scalar("SELECT count(*) FROM events").await,1);
        }
    });
}
#[test]
fn missing_and_corrupt_objects_fail_without_cache_fallback_and_release_lease() {
    runtime().block_on(async {
        for missing in [true, false] {
            let mut h = Harness::new().await;
            publish(&mut h, 10, 0, contract()).await;
            let r = request(&mut h, 20, 1, contract()).await;
            let artifacts = tf_store::artifacts::ArtifactStore::open(&h.root).unwrap();
            let verified = artifacts.verify(h.digest).unwrap();
            let relative = verified.manifest()["files"][0]["path"].as_str().unwrap();
            let path = h
                .root
                .join(".transflow/runtime/objects")
                .join(&h.digest.hex()[..2])
                .join(h.digest.hex())
                .join(relative);
            writable(&h.root);
            if missing {
                std::fs::remove_file(path).unwrap();
            } else {
                std::fs::write(path, b"broken parquet").unwrap();
            }
            assert!(matches!(
                reuse(&mut h, r).await,
                Err(tf_exec::cache::Error::Artifact(_))
            ));
            assert_eq!(h.scalar("SELECT count(*) FROM cached_jobs").await, 0);
            assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await, 1);
            assert_eq!(
                h.scalar("SELECT count(*) FROM read_leases WHERE released=0")
                    .await,
                0
            );
        }
    });
}
#[test]
fn expired_or_released_lease_and_changed_evidence_cannot_adopt() {
    runtime().block_on(async {
        for change in ["expired","released","cancel","evidence","rollback"] {
            let mut h=Harness::new().await;publish(&mut h,10,0,contract()).await;
            let mut other=contract();other.compute_fingerprint="c".repeat(64);publish(&mut h,11,1,other).await;
            let r=request(&mut h,20,2,contract()).await;
            let mut owned=h.owner.as_mut().unwrap().open_store().await.unwrap();
            let store=owned.repository().unwrap();
            let Lookup::Candidate(candidate)=store.lookup_cache(&r,RequestId::from_bytes([60;16]),10).await.unwrap() else {panic!("candidate expected")};
            let artifact=tf_store::artifacts::ArtifactStore::open(&h.root).unwrap().verify(h.digest).unwrap();
            match change {
                "released"=>store.release_read(candidate.lease()).await.unwrap(),
                "cancel"=>{store.cancel_publications(workspace(),r.fence.session,r.build).await.unwrap();},
                "evidence"=>{sqlx::query("DELETE FROM publication_contracts WHERE job_id='absent'").execute(&mut h.db).await.unwrap();sqlx::query("UPDATE attempts SET state='FAILED' WHERE id=?").bind(AttemptId::from_bytes([10;16]).to_string()).execute(&mut h.db).await.unwrap_err();sqlx::query("UPDATE jobs SET state='FAILED' WHERE id=?").bind(JobId::from_bytes([10;16]).to_string()).execute(&mut h.db).await.unwrap();},
                "rollback"=>{sqlx::raw_sql("CREATE TRIGGER fail_cache_audit BEFORE INSERT ON audit_log WHEN NEW.operation='cache_reuse' BEGIN SELECT RAISE(ABORT,'injected disk write failure'); END;").execute(&mut h.db).await.unwrap();},_=>{}
            }
            assert!(store.adopt_cache(&candidate,&artifact,if change=="expired"{50}else{41}).await.is_err(),"{change}");owned.close().await.unwrap();
            assert_eq!(h.scalar("SELECT count(*) FROM cached_jobs").await,0);assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await,2);assert_eq!(h.scalar("SELECT count(*) FROM events").await,2);
        }
    });
}
#[test]
fn cached_parent_binding_ignores_later_heads_and_survives_recovery() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        publish(&mut h, 10, 0, contract()).await;
        let r = request(&mut h, 20, 1, contract()).await;
        reuse(&mut h, r).await.unwrap();
        h.finish(20, "SUCCEEDED").await;
        publish(&mut h, 11, 1, contract()).await;
        h.owner.take();
        let mut owner = tf_exec::ownership::RuntimeOwner::acquire(
            &h.root,
            workspace(),
            tf_exec::ownership::CoordinatorMode::Temporary,
        )
        .unwrap();
        tf_exec::publication::recover(&mut owner, 50).await.unwrap();
        let mut owned = owner.open_store().await.unwrap();
        let store = owned.repository().unwrap();
        assert_eq!(
            store
                .completed_parent(
                    BuildId::from_bytes([20; 16]),
                    DatasetId::from_bytes([10; 16])
                )
                .await
                .unwrap()
                .unwrap()
                .0,
            VersionId::from_bytes([10; 16])
        );
        assert!(
            store
                .completed_parent(
                    BuildId::from_bytes([99; 16]),
                    DatasetId::from_bytes([10; 16])
                )
                .await
                .unwrap()
                .is_none()
        );
        owned.close().await.unwrap();
        h.owner = Some(owner);
        assert_eq!(h.scalar("SELECT count(*) FROM cached_jobs").await, 1);
    });
}

#[test]
fn publication_retry_reads_original_event_after_cached_head_adoption() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let original = h.start(10, 10, 0, contract()).await;
        let published = h.publish(original.clone(), 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let mut other = contract();
        other.compute_fingerprint = "c".repeat(64);
        publish(&mut h, 11, 1, other).await;
        let r = request(&mut h, 20, 2, contract()).await;
        reuse(&mut h, r.clone()).await.unwrap();
        let artifact = tf_store::artifacts::ArtifactStore::open(&h.root)
            .unwrap()
            .verify(h.digest)
            .unwrap();
        let mut owned = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert_eq!(
            owned
                .repository()
                .unwrap()
                .commit_publication(&original, &artifact)
                .await
                .unwrap(),
            published
        );
        owned.close().await.unwrap();
        let mut stale = r;
        stale.fence.generation += 1;
        assert!(reuse(&mut h, stale).await.is_err());
        assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await, 3);
    });
}

#[test]
fn exact_input_alias_certificates_are_reused_and_cannot_be_silently_omitted() {
    runtime().block_on(async {
        let mut h=Harness::new().await;
        let p=h.start(9,9,0,contract()).await;h.publish(p,9).await.unwrap();h.finish(9,"SUCCEEDED").await;
        let mut c=contract();
        for (n,alias) in [(1,"current"),(2,"baseline")] {
            let definition=format!("{n:x}").repeat(64);
            c.inputs.push(InputProvenance{alias:alias.into(),dataset:DatasetKey::new(workspace(),DatasetId::from_bytes([9;16])),version:VersionId::from_bytes([9;16]),artifact:h.digest,declared_branch:json!({"kind":"current","name":null}),starting_branch:"master".parse().unwrap(),resolved_branch:"master".parse().unwrap(),role:InputRole::Data,resolution:json!({})});
            c.checks.push(CheckRequirement{definition:definition.clone(),subject:CheckSubject::Input(alias.into()),required:true});
            sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'input',?,'check','{\"severity\":\"FAIL\"}')").bind(definition).bind(alias).execute(&mut h.db).await.unwrap();
        }
        let p=h.start(10,10,0,c.clone()).await;
        for (n,alias) in [(1,"current"),(2,"baseline")] {
            sqlx::query("INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,outcome,metrics_json,started_at_us,finished_at_us) VALUES(?,?,?,?,'PASS','{}',5,10)").bind(RequestId::from_bytes([60+n;16]).to_string()).bind(p.intent.attempt().to_string()).bind(json!({"alias":alias,"version_id":VersionId::from_bytes([9;16]).to_string(),"artifact_digest":h.digest.hex()}).to_string()).bind(format!("{n:x}").repeat(64)).execute(&mut h.db).await.unwrap();
        }
        h.publish(p,10).await.unwrap();h.finish(10,"SUCCEEDED").await;
        let r=request(&mut h,20,1,c.clone()).await;reuse(&mut h,r).await.unwrap();h.finish(20,"SUCCEEDED").await;
        assert_eq!(h.scalar("SELECT count(*) FROM validation_reuse").await,2);
        assert_eq!(h.scalar("SELECT count(*) FROM check_results WHERE finished_at_us=10").await,2);
        // Even a faulty caller reusing the old fingerprints cannot drop an obligation.
        c.checks.pop();let r=request(&mut h,21,1,c).await;assert!(reuse(&mut h,r).await.is_err());
        assert_eq!(h.scalar("SELECT count(*) FROM cached_jobs").await,1);
    });
}

#[test]
fn comparison_evidence_refuses_wrong_keys_and_is_immutable_and_idempotent() {
    runtime().block_on(async {
        let mut h=Harness::new().await;
        let r=request(&mut h,20,0,contract()).await;
        let mut evidence=json!({"format_version":1,"compute":"f".repeat(64),"reusable":true,"components":{},"inputs":{},"files":{}});
        let mut owned=h.owner.as_mut().unwrap().open_store().await.unwrap();
        let store=owned.repository().unwrap();
        assert!(store.freeze_computation_evidence(&r,&evidence).await.is_err());
        evidence["compute"]=json!(r.contract.compute_fingerprint);
        store.freeze_computation_evidence(&r,&evidence).await.unwrap();
        store.freeze_computation_evidence(&r,&evidence).await.unwrap();
        evidence["files"]=json!({"src/helper.py":"e".repeat(64)});
        assert!(store.freeze_computation_evidence(&r,&evidence).await.is_err());
        owned.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM computation_evidence").await,1);
        assert!(sqlx::query("DELETE FROM computation_evidence").execute(&mut h.db).await.is_err());
    });
}
