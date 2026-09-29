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
    let mut v = example()?;
    v["description"] = json!("Preserve  intentional spacing");
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
        reader.views("", "").await?["views"]
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

#[tokio::test]
async fn legacy_views_reopen_without_retired_features_and_keep_cas_revision()
-> Result<(), Box<dyn std::error::Error>> {
    use sqlx::Connection;
    let root = std::env::temp_dir().join(format!("tf-legacy-views-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let path = root.join("catalog.sqlite");
    let mut store = Store::open(&path).await?;
    let current = example()?;
    let mut old = current.clone();
    old["revision"] = json!(7);
    old["viewport"] = json!({"x":500,"y":900,"zoom":0.1});
    old["groups"] = json!([]);
    old["annotations"] = json!([]);
    old["filters"] = json!({"path":"raw"});
    old["description"] = json!("Old\n description");
    old["selector"]["mode"] = json!("snapshot");
    old["selector"]["source"] = json!("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    old["selector"]["graph"] = json!("a".repeat(64));
    let mut db = sqlx::SqliteConnection::connect(&format!("sqlite:{}", path.display())).await?;
    sqlx::query("INSERT INTO graph_views VALUES(?,7,1,?,?,1)")
        .bind(old["id"].as_str().ok_or("missing id")?)
        .bind(old.to_string())
        .bind(old["selector"].to_string())
        .execute(&mut db)
        .await?;
    let mut reader = Reader::open_existing(&path).await?;
    let reopened = reader.view(old["id"].as_str().ok_or("missing id")?).await?;
    assert_eq!(reopened["revision"], 7);
    assert_eq!(reopened["selector"], current["selector"]);
    assert_eq!(reopened["description"], "Old description");
    for key in ["groups", "annotations", "filters", "viewport"] {
        assert!(reopened.get(key).is_none());
    }
    let bytes: String = sqlx::query_scalar("SELECT view_json FROM graph_views")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(bytes, old.to_string());
    assert!(store.save_view(&old, 2).await.is_err());
    let saved = store
        .save_view(&reopened, 3)
        .await?
        .ok_or("save conflict")?;
    assert_eq!(saved["revision"], 8);
    assert_eq!(
        reader.view(old["id"].as_str().ok_or("missing id")?).await?,
        saved
    );
    sqlx::query("UPDATE graph_views SET view_json='[]'")
        .execute(&mut db)
        .await?;
    assert!(
        reader
            .view(old["id"].as_str().ok_or("missing id")?)
            .await
            .is_err()
    );
    db.close().await?;
    reader.close().await?;
    store.close().await?;
    std::fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test]
async fn saved_lineage_search_crosses_pages_and_reports_committed_save_time()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("tf-view-search-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let path = root.join("catalog.sqlite");
    let mut store = Store::open(&path).await?;
    let mut reader = Reader::open_existing(&path).await?;
    for index in 1..=205 {
        let mut v = example()?;
        v["id"] = json!(format!("aaaaaaaa-aaaa-4aaa-8aaa-{index:012x}"));
        v["name"] = json!(if index < 105 {
            "Other"
        } else {
            "Review Ånalysis"
        });
        v["selector"]["branch"] = json!(format!("branch-{index}"));
        store.save_view(&v, index).await?.ok_or("save failed")?;
    }
    let page = reader.views("", "RVÅ").await?;
    tf_protocol::validate_document("ApiViewsV1", &page)?;
    assert_eq!(page["views"].as_array().ok_or("no views")?.len(), 100);
    assert_eq!(page["views"][0]["saved_at_us"], "105");
    assert_eq!(page["views"][0]["branch"], "branch-105");
    let next = reader
        .views(page["next_cursor"].as_str().ok_or("no cursor")?, "RVÅ")
        .await?;
    assert_eq!(next["views"].as_array().ok_or("no views")?.len(), 1);
    assert_eq!(next["views"][0]["saved_at_us"], "205");
    assert!(next["next_cursor"].is_null());
    assert!(
        reader.views("", "absent").await?["views"]
            .as_array()
            .ok_or("no views")?
            .is_empty()
    );
    reader.close().await?;
    store.close().await?;
    std::fs::remove_dir_all(root)?;
    Ok(())
}
