//! Security and durable-state regression coverage.
use serde_json::json;
use tf_store::{Reader, Store, api::Receipt};
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
