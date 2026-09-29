//! Saved-view durability, conflict and semantic validation coverage.
use serde_json::{Value, json};
use tf_store::{Reader, Store, views::validate};
fn example() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(
        serde_json::from_str::<Value>(include_str!("../../../schemas/fixtures/conformance.json"))?
            .as_array()
            .ok_or("missing fixture value")?
            .iter()
            .find(|v| v["name"] == "graph-view-v1")
            .ok_or("missing fixture value")?["value"]
            .clone(),
    )
}
#[tokio::test]
async fn save_reopen_conflicts_and_audit_leave_execution_context_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("tf-views-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let path = root.join("catalog.sqlite");
    let mut store = Store::open(&path).await?;
    let mut reader = Reader::open_existing(&path).await?;
    let before = reader.api_revision().await?;
    let v = example()?;
    let saved = store
        .save_view(&v, 1)
        .await?
        .ok_or("missing fixture value")?;
    assert_eq!(saved["revision"], 1);
    assert_eq!(reader.api_revision().await?, before);
    assert!(store.save_view(&v, 2).await?.is_none());
    let updated = store
        .save_view(&saved, 3)
        .await?
        .ok_or("missing fixture value")?;
    assert_eq!(updated["revision"], 2);
    assert!(store.save_view(&saved, 4).await?.is_none());
    let mut invalid = updated.clone();
    invalid["datasets"]
        .as_array_mut()
        .ok_or("missing fixture value")?
        .push(v["datasets"][0].clone());
    assert!(store.save_view(&invalid, 5).await.is_err());
    assert_eq!(
        reader
            .view(v["id"].as_str().ok_or("missing fixture value")?)
            .await?,
        updated
    );
    assert_eq!(
        reader.views("").await?["views"]
            .as_array()
            .ok_or("missing fixture value")?
            .len(),
        1
    );
    reader.close().await?;
    store.close().await?;
    let mut reopened = Reader::open_existing(&path).await?;
    assert_eq!(
        reopened
            .view(v["id"].as_str().ok_or("missing fixture value")?)
            .await?,
        updated
    );
    reopened.close().await?;
    std::fs::remove_dir_all(root)?;
    Ok(())
}
#[test]
fn view_membership_and_context_invariants_are_fail_closed() -> Result<(), Box<dyn std::error::Error>>
{
    let original = example()?;
    assert!(validate(&original).is_ok());
    let mut v = original.clone();
    v["selector"]["mode"] = json!("snapshot");
    assert!(validate(&v).is_err());
    let mut v = original.clone();
    v["selector"]["fallback"] = json!(["master"]);
    assert!(validate(&v).is_err());
    let mut v = original.clone();
    v["datasets"][0]["identity"] = json!("pending:raw/items");
    assert!(validate(&v).is_err());
    let mut v = original;
    v["groups"] = json!([{"id":"cccccccc-cccc-4ccc-8ccc-cccccccccccc","name":"bad","members":["missing"],"collapsed":false,"position":{"x":0,"y":0}}]);
    assert!(validate(&v).is_err());
    Ok(())
}
