//! Security and durable-state regression coverage.
use serde_json::json;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tf_domain::{DatasetId, DatasetKey, VersionId, WorkspaceId};
use tf_store::{Reader, Store, api::Receipt};

#[tokio::test]
async fn scheduler_observation_audits_preserve_context_but_lifecycle_audits_invalidate_it()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "tf-api-observation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&dir)?;
    let path = dir.join("catalog.sqlite");
    let store = Store::open(&path).await?;
    let mut reader = Reader::open_existing(&path).await?;
    let mut writer =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path)).await?;
    let initial = reader.api_revision().await?;
    for operation in ["schedule_clock_observed", "schedule_events_scanned"] {
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES(?,'{}',1)")
            .bind(operation)
            .execute(&mut writer)
            .await?;
        assert_eq!(reader.api_revision().await?, initial);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log")
            .fetch_one(&mut writer)
            .await?,
        2
    );
    let mut previous = initial;
    for operation in [
        "schedule_saved",
        "schedule.observation_failed",
        "schedule_occurrence_queued",
        "plan.accept",
    ] {
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES(?,'{}',2)")
            .bind(operation)
            .execute(&mut writer)
            .await?;
        let current = reader.api_revision().await?;
        assert_ne!(current, previous);
        previous = current;
    }
    writer.close().await?;
    reader.close().await?;
    store.close().await?;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[tokio::test]
async fn dataset_inspection_reads_creation_and_latest_foreign_provenance()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "tf-dataset-inspection-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::create_dir(&dir)?;
    let path = dir.join("catalog.sqlite");
    let mut store = Store::open(&path).await?;
    let local_workspace = WorkspaceId::from_bytes([1; 16]);
    let local_dataset = DatasetId::from_bytes([2; 16]);
    store
        .register_workspace(local_workspace, "synthetic-root", 10)
        .await?;
    store
        .register_dataset(local_workspace, local_dataset, 123)
        .await?;
    let provider = WorkspaceId::from_bytes([3; 16]);
    let foreign_dataset = DatasetId::from_bytes([4; 16]);
    let key = DatasetKey::new(provider, foreign_dataset);
    let schema = json!({"format_version":1,"fields":[{"name":"id","logical_type":{"type":"i64"},"nullable":false}]});
    for (seed, branch, published) in [(5, "older", "100"), (6, "newer", "200")] {
        let version = VersionId::from_bytes([seed; 16]);
        store.record_foreign_metadata(
            key,
            version,
            &json!({"workspace":provider.to_string(),"dataset":foreign_dataset.to_string(),"version":version.to_string(),"artifact":"a".repeat(64),"manifest":{"logical_schema":schema,"files":[{"row_count":"2","byte_length":"8"},{"row_count":"3","byte_length":"13"}]},"origin_branch":{"name":branch},"published_us":published}),
        ).await?;
    }
    store.close().await?;
    let mut reader = Reader::open_existing(&path).await?;
    let local = reader
        .api_dataset_inspection(local_workspace, local_dataset, false)
        .await?;
    assert_eq!(local["created_us"], "123");
    assert!(local["suggested_head"].is_null());
    let foreign = reader
        .api_dataset_inspection(provider, foreign_dataset, true)
        .await?;
    assert!(foreign["created_us"].is_null());
    assert_eq!(foreign["suggested_head"]["branch"], "newer");
    assert_eq!(foreign["suggested_head"]["published_us"], "200");
    assert_eq!(foreign["suggested_head"]["schema"], schema);
    assert_eq!(foreign["suggested_head"]["row_count"], "5");
    assert_eq!(foreign["suggested_head"]["file_count"], "2");
    assert_eq!(foreign["suggested_head"]["byte_count"], "21");
    reader.close().await?;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}
#[tokio::test]
async fn api_receipts_replay_conflict_and_survive_interruption()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("tf-api-receipt-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("catalog.sqlite");
    let mut store = Store::open(&path).await?;
    let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse()?;
    let digest = "a".repeat(64);
    assert_eq!(store.reserve_api(id, &digest).await?, Receipt::New);
    store.close().await?;
    let mut store = Store::open(&path).await?;
    assert_eq!(store.reserve_api(id, &digest).await?, Receipt::Pending);
    assert_eq!(
        store.reserve_api(id, &"b".repeat(64)).await?,
        Receipt::Conflict
    );
    let value = json!({"data":{"build":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"}});
    store.finish_api(id, &digest, &value).await?;
    assert!(store.finish_api(id, &digest, &json!({})).await.is_err());
    let mut read = Reader::open_existing(&path).await?;
    assert_eq!(
        read.api_receipt(id, &digest).await?,
        Receipt::Complete(value)
    );
    assert!(!read.api_revision().await?.is_empty());
    read.close().await?;
    store.close().await?;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}
