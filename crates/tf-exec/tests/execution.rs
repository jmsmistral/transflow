//! Dispatcher transitions require exact accepted membership, durable ownership and state CAS.
#![allow(
    clippy::unwrap_used,
    reason = "Synthetic SQLite integration assertions"
)]
#[path = "support/publication.rs"]
mod fixture;
use fixture::*;
use tf_domain::{
    AttemptId,
    execution::{BuildState, EventTime, Phase},
};
#[test]
fn accepted_execution_cas_and_terminal_aggregation_refuse_fabricated_progress() {
    runtime().block_on(async {
        let mut h=Harness::new().await;
        let mut job=h.seed(10,10).await;
        sqlx::query("DELETE FROM attempts").execute(&mut h.db).await.unwrap();
        sqlx::query("UPDATE jobs SET state='PLANNED'").execute(&mut h.db).await.unwrap();
        sqlx::query("UPDATE builds SET state='QUEUED'").execute(&mut h.db).await.unwrap();
        sqlx::query("INSERT INTO write_reservations(branch_id,dataset_id,build_id,session_id,fence,acquired_at_us,expected_head_generation) VALUES(?,?,?,?,1,1,0)").bind(job.target().branch.to_string()).bind(job.target().dataset.dataset_id().to_string()).bind(job.build().to_string()).bind(job.fence().session.to_string()).execute(&mut h.db).await.unwrap();
        let mut owned=h.owner.as_mut().unwrap().open_store().await.unwrap();
        let s=owned.repository().unwrap();
        assert!(s.start_dispatch(&[job.clone(),job.clone()],2).await.is_err());
        s.start_dispatch(&[job.clone()],2).await.unwrap();
        assert!(s.start_dispatch(&[job.clone()],2).await.is_err());
        let old=job.clone();job.queue(job.fence(),EventTime(1)).unwrap();
        s.persist_execution(&old,&job,3,0).await.unwrap();
        assert!(s.persist_execution(&old,&job,3,0).await.is_err());
        let old=job.clone();let attempt=AttemptId::from_bytes([11;16]);job.start_attempt(attempt,job.fence(),EventTime(2)).unwrap();
        s.persist_execution(&old,&job,4,0).await.unwrap();
        let old=job.clone();job.advance(attempt,job.fence(),Phase::ValidatingInputs,EventTime(3)).unwrap();
        assert!(s.persist_execution(&old,&job,5,-1).await.is_err());
        s.persist_execution(&old,&job,5,50).await.unwrap();
        s.cancel_publications(workspace(),job.fence().session,job.build()).await.unwrap();
        let old=job.clone();let mut forbidden=job.clone();forbidden.advance(attempt,job.fence(),Phase::Running,EventTime(4)).unwrap();
        assert!(s.persist_execution(&old,&forbidden,6,50).await.is_err());
        job.request_cancel();job.finish_canceled(job.fence(),EventTime(4)).unwrap();
        s.persist_execution(&old,&job,6,60).await.unwrap();
        assert!(s.finish_dispatch(&[job.clone()],BuildState::Succeeded,7).await.is_err());
        assert!(s.finish_dispatch(&[job.clone(),job.clone()],BuildState::Canceled,7).await.is_err());
        s.finish_dispatch(&[job],BuildState::Canceled,7).await.unwrap();
        owned.close().await.unwrap();
        assert_eq!(h.scalar("SELECT count(*) FROM attempts WHERE state='CANCELED'").await,1);
        assert_eq!(h.scalar("SELECT count(*) FROM phase_intervals WHERE finished_at_us IS NULL").await,0);
        assert_eq!(h.scalar("SELECT count(*) FROM write_reservations").await,0);
    });
}
