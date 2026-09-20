//! Atomic persisted state, exact validation gates, owner fences and cancellation winners.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic integration assertions"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use serde_json::json;
use tf_domain::execution::CancelRequest;
use tf_domain::*;
use tf_exec::publication;
use tf_store::publication::*;
#[test]
fn commits_one_version_head_event_and_outbox_then_replays_without_execution() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let request = h.start(10, 10, 0, contract()).await;
        let receipt = h.publish(request, 10).await.unwrap();
        for table in [
            "SELECT count(*) FROM dataset_versions",
            "SELECT count(*) FROM dataset_heads",
            "SELECT count(*) FROM head_changes",
            "SELECT count(*) FROM events",
            "SELECT count(*) FROM outbox_deliveries",
        ] {
            assert_eq!(h.scalar(table).await, 1);
        }
        assert_eq!(
            h.scalar("SELECT count(*) FROM attempts WHERE state='SUCCEEDED'")
                .await,
            1
        );
        let session = h.session();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        let repo = store.repository().unwrap();
        assert_eq!(
            repo.cancel_publications(workspace(), session, BuildId::from_bytes([10; 16]))
                .await
                .unwrap(),
            CancelRequest::TooLate
        );
        let events = repo.pending_publications(100).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, receipt.event);
        assert_eq!(repo.pending_publications(100).await.unwrap(), events);
        repo.acknowledge_publication(workspace(), session, receipt.event, 30)
            .await
            .unwrap();
        assert!(repo.pending_publications(100).await.unwrap().is_empty());
        store.close().await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await, 0);
    });
}
#[test]
fn equal_bytes_have_independent_versions_and_later_failure_preserves_last_good() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        h.finish(10, "SUCCEEDED").await;
        let r = h.start(11, 10, 1, contract()).await;
        h.publish(r, 11).await.unwrap();
        h.finish(11, "SUCCEEDED").await;
        assert_eq!(h.scalar("SELECT count(*) FROM artifacts").await, 1);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 2);
        assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await, 2);
        let r = h.start(12, 10, 2, contract()).await;
        let session = h.session();
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert_eq!(
            store
                .repository()
                .unwrap()
                .cancel_publications(workspace(), session, BuildId::from_bytes([12; 16]))
                .await
                .unwrap(),
            CancelRequest::Requested
        );
        store.close().await.unwrap();
        assert!(matches!(
            h.publish(r, 12).await,
            Err(publication::Error::Publication(PublicationError::Canceled))
        ));
        h.finish(12, "FAILED").await;
        assert_eq!(h.scalar("SELECT generation FROM dataset_heads").await, 2);
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 2);
    });
}
#[test]
fn stale_generation_fence_and_contract_changes_never_publish() {
    runtime().block_on(async {
        for change in ["fence", "generation", "contract"] {
            let mut h = Harness::new().await;
            let mut r = h.start(10, 10, 0, contract()).await;
            match change {
                "fence" => {
                    sqlx::query("UPDATE write_reservations SET fence=2")
                        .execute(&mut h.db)
                        .await
                        .unwrap();
                }
                "generation" => {
                    sqlx::query("UPDATE write_reservations SET expected_head_generation=1")
                        .execute(&mut h.db)
                        .await
                        .unwrap();
                }
                _ => {
                    r.contract.compute_fingerprint = "c".repeat(64);
                }
            }
            assert!(h.publish(r, 10).await.is_err(), "{change}");
            assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 0);
            assert_eq!(h.scalar("SELECT count(*) FROM events").await, 0);
        }
    });
}
#[test]
fn failed_required_missing_wrong_subject_and_evaluator_errors_block_even_warn() {
    runtime().block_on(async{
 for (required,outcome,subject_ok,passes) in [(true,"PASS",true,true),(false,"VIOLATION",true,true),(true,"VIOLATION",true,false),(false,"ERROR",true,false),(false,"SKIPPED",true,false),(true,"MISSING",true,false),(true,"PASS",false,false)]{
 let mut h=Harness::new().await;let fingerprint="c".repeat(64);let policy=json!({"severity":if required{"FAIL"}else{"WARN"}}).to_string();
 sqlx::query("INSERT INTO check_definitions VALUES(?,'{}',1,'output',NULL,'check',?)").bind(&fingerprint).bind(policy).execute(&mut h.db).await.unwrap();
 let mut c=contract();c.checks.push(CheckRequirement{definition:fingerprint.clone(),subject:CheckSubject::Output,required});let r=h.start(10,10,0,c).await;
 if outcome!="MISSING" {sqlx::query("INSERT INTO check_results(id,attempt_id,subject_json,definition_fingerprint,outcome,metrics_json,started_at_us,finished_at_us) VALUES(?,?,?,?,?,'{}',5,10)").bind(RequestId::from_bytes([40;16]).to_string()).bind(r.intent.attempt().to_string()).bind(json!({"artifact_digest":if subject_ok{h.digest.hex()}else{"d".repeat(64)}}).to_string()).bind(fingerprint).bind(outcome).execute(&mut h.db).await.unwrap();}
 assert_eq!(h.publish(r,10).await.is_ok(),passes,"{required} {outcome} {subject_ok}");assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await,i64::from(passes));assert_eq!(h.scalar("SELECT count(*) FROM version_check_results").await,i64::from(passes));
 if passes {assert!(sqlx::query("UPDATE check_results SET outcome='ERROR'").execute(&mut h.db).await.is_err());}
 }
});
}
#[test]
fn repeated_input_aliases_retain_exact_historical_version_and_branch_evidence() {
    runtime().block_on(async{
 let mut h=Harness::new().await;let r=h.start(10,10,0,contract()).await;h.publish(r,10).await.unwrap();h.finish(10,"SUCCEEDED").await;
 let mut c=contract();for alias in ["current","baseline"]{c.inputs.push(InputProvenance{alias:alias.into(),dataset:DatasetKey::new(workspace(),DatasetId::from_bytes([10;16])),version:VersionId::from_bytes([10;16]),artifact:h.digest,declared_branch:json!({"named":"master"}),starting_branch:"feature".parse().unwrap(),resolved_branch:"master".parse().unwrap(),role:InputRole::Data,resolution:json!({"fallback":true})});}
 let r=h.start(11,11,0,c).await;h.publish(r,11).await.unwrap();assert_eq!(h.scalar("SELECT count(*) FROM version_inputs WHERE starting_branch='feature' AND resolved_branch='master'").await,2);assert_eq!(h.scalar("SELECT count(DISTINCT origin_version_id) FROM version_inputs").await,1);
});
}
#[test]
fn recovery_interrupts_only_uncommitted_work_and_fences_old_session() {
    runtime().block_on(async {
        let mut h = Harness::new().await;
        let r = h.start(10, 10, 0, contract()).await;
        h.publish(r, 10).await.unwrap();
        let old = h.session();
        let r = h.start(11, 11, 0, contract()).await;
        h.owner.take();
        let mut owner = tf_exec::ownership::RuntimeOwner::acquire(
            &h.root,
            workspace(),
            tf_exec::ownership::CoordinatorMode::Temporary,
        )
        .unwrap();
        assert_ne!(old, owner.registration().session().unwrap());
        publication::recover(&mut owner, 40).await.unwrap();
        h.owner = Some(owner);
        assert_eq!(
            h.scalar("SELECT count(*) FROM attempts WHERE state='SUCCEEDED'")
                .await,
            1
        );
        assert_eq!(
            h.scalar("SELECT count(*) FROM attempts WHERE state='INTERRUPTED'")
                .await,
            1
        );
        assert_eq!(h.scalar("SELECT count(*) FROM dataset_heads").await, 1);
        assert!(h.publish(r, 11).await.is_err());
        let mut store = h.owner.as_mut().unwrap().open_store().await.unwrap();
        assert_eq!(
            store
                .repository()
                .unwrap()
                .pending_publications(100)
                .await
                .unwrap()
                .len(),
            1
        );
        store.close().await.unwrap();
    });
}

#[test]
fn readers_observe_only_committed_heads_and_late_sql_failure_rolls_every_visibility_row_back() {
    runtime().block_on(async {
        for abort in [false, true] {
            let mut h=Harness::new().await;
            let request=h.start(10,10,0,contract()).await;
            if abort {sqlx::raw_sql("CREATE TRIGGER injected_outbox_failure BEFORE INSERT ON outbox_deliveries BEGIN SELECT RAISE(ABORT,'injected journal write failure'); END;").execute(&mut h.db).await.unwrap();}
            let root=h.root.clone();let files=h.files(10);
            let completion=publication::publish(h.owner.take().unwrap(),request.clone(),files,move |point| {
                if matches!(point,publication::Boundary::BeforeCommit|publication::Boundary::Committed) {
                    use sqlx::Connection;
                    runtime().block_on(async {
                        let options=sqlx::sqlite::SqliteConnectOptions::new().filename(root.join(".transflow/runtime/catalog.sqlite")).read_only(true);
                        let mut db=sqlx::SqliteConnection::connect_with(&options).await.unwrap();
                        let count:i64=sqlx::query_scalar("SELECT count(*) FROM dataset_heads").fetch_one(&mut db).await.unwrap();
                        assert_eq!(count,if point==publication::Boundary::Committed {1}else{0});
                        db.close().await.unwrap();
                    });
                }
                Ok(())
            }).await.unwrap();h.owner=Some(completion.owner);
            assert_eq!(completion.result.is_ok(),!abort);
            assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await,i64::from(!abort));
            assert_eq!(h.scalar("SELECT count(*) FROM head_changes").await,i64::from(!abort));
            assert_eq!(h.scalar("SELECT count(*) FROM events").await,i64::from(!abort));
            if abort {assert_eq!(h.scalar("SELECT count(*) FROM publication_intents WHERE state='PREPARED'").await,1);}
            else {
                let object=tf_store::artifacts::ArtifactStore::open(&h.root).unwrap().verify(h.digest).unwrap();
                let mut store=h.owner.as_mut().unwrap().open_store().await.unwrap();
                let again=store.repository().unwrap().commit_publication(&request,&object).await.unwrap();
                assert_eq!(again,completion.result.unwrap());store.close().await.unwrap();
                assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await,1);
            }
        }
    });
}

#[test]
fn cancellation_or_head_conflict_after_install_preserves_the_previous_publication() {
    runtime().block_on(async {
        for cancel in [true, false] {
            let mut h = Harness::new().await;
            let old = h.start(10, 10, 0, contract()).await;
            h.publish(old, 10).await.unwrap();
            h.finish(10, "SUCCEEDED").await;
            let request = h.start(11, 10, 1, contract()).await;
            let root = h.root.clone();
            let files = h.files(11);
            let completion =
                publication::publish(h.owner.take().unwrap(), request, files, move |point| {
                    if point == publication::Boundary::BeforeCommit {
                        use sqlx::Connection;
                        runtime().block_on(async {
                            let options = sqlx::sqlite::SqliteConnectOptions::new()
                                .filename(root.join(".transflow/runtime/catalog.sqlite"));
                            let mut db = sqlx::SqliteConnection::connect_with(&options)
                                .await
                                .unwrap();
                            if cancel {
                                sqlx::query(
                                    "UPDATE builds SET cancel_requested=1 WHERE state='RUNNING'",
                                )
                                .execute(&mut db)
                                .await
                                .unwrap();
                            } else {
                                sqlx::query("UPDATE dataset_heads SET generation=2")
                                    .execute(&mut db)
                                    .await
                                    .unwrap();
                            }
                            db.close().await.unwrap();
                        });
                    }
                    Ok(())
                })
                .await
                .unwrap();
            h.owner = Some(completion.owner);
            if cancel {
                assert!(matches!(
                    completion.result,
                    Err(publication::Error::Publication(PublicationError::Canceled))
                ));
            } else {
                assert!(matches!(
                    completion.result,
                    Err(publication::Error::Publication(PublicationError::Conflict))
                ));
            }
            assert_eq!(h.scalar("SELECT count(*) FROM dataset_versions").await, 1);
            assert_eq!(
                h.scalar("SELECT count(*) FROM attempts WHERE state='SUCCEEDED'")
                    .await,
                1
            );
            assert_eq!(h.scalar("SELECT count(*) FROM events").await, 1);
        }
    });
}

#[test]
fn complete_reservations_and_metadata_only_authority_are_required() {
    runtime().block_on(async{
 let mut h=Harness::new().await;let job=h.seed(10,10).await;
 sqlx::query("INSERT INTO jobs(id,build_id,dataset_id,branch_id,state,bindings_json) VALUES(?,?,?,?,'PLANNED','[]')").bind(JobId::from_bytes([11;16]).to_string()).bind(job.build().to_string()).bind(DatasetId::from_bytes([11;16]).to_string()).bind(branch().to_string()).execute(&mut h.db).await.unwrap_err();
 sqlx::query("INSERT INTO datasets VALUES(?,?,1,NULL)").bind(DatasetId::from_bytes([11;16]).to_string()).bind(workspace().to_string()).execute(&mut h.db).await.unwrap();
 sqlx::query("INSERT INTO jobs(id,build_id,dataset_id,branch_id,state,bindings_json) VALUES(?,?,?,?,'PLANNED','[]')").bind(JobId::from_bytes([11;16]).to_string()).bind(job.build().to_string()).bind(DatasetId::from_bytes([11;16]).to_string()).bind(branch().to_string()).execute(&mut h.db).await.unwrap();
 let target=tf_domain::execution::OutputTarget{dataset:DatasetKey::new(workspace(),DatasetId::from_bytes([10;16])),branch:branch(),expected_generation:0};
 let mut store=h.owner.as_mut().unwrap().open_store().await.unwrap();assert!(store.repository().unwrap().reserve_publications(workspace(),job.build(),job.fence(),&[target],2).await.is_err());store.close().await.unwrap();assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await,0);
 h.owner.take();let mut owner=tf_exec::ownership::RuntimeOwner::acquire(&h.root,workspace(),tf_exec::ownership::CoordinatorMode::MetadataOnly).unwrap();assert!(matches!(publication::recover(&mut owner,40).await,Err(publication::Error::Mode)));h.owner=Some(owner);
});
}
