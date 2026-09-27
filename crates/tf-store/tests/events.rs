//! Real WAL snapshots, privacy projection and replay-retention boundaries.
use serde_json::json;
use sqlx::{Connection, SqliteConnection};
use tf_store::Store;
#[tokio::test]
async fn event_replay_is_bounded_private_and_explicitly_resynchronizes()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!("tf-events-{}.sqlite", std::process::id()));
    let mut s = Store::open(&path).await?;
    let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse()?;
    s.append_event(
        id,
        "private.audit",
        &json!({"secret":"must-not-leak", "branch":"private", "padding":"x".repeat(512 * 1024)}),
        1,
    )
    .await?;
    let mut r = s.reader().await?;
    let current = r.event_page("workspace", None).await?;
    assert_eq!(current["cursor"], "1");
    assert_eq!(current["events"], json!([]));
    let page = r.event_page("workspace", Some(0)).await?;
    assert_eq!(page["events"][0]["id"], id.to_string());
    assert_eq!(page["events"][0]["payload"], json!({}));
    assert!(page["events"][0]["context"]["branch"].is_null());
    assert_eq!(
        r.event_page("workspace", Some(2)).await?["resync_required"],
        true
    );
    assert_eq!(
        r.event_page("workspace", Some(1)).await?["events"],
        json!([])
    );
    r.close().await?;
    s.close().await?;
    let mut db =
        SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await?;
    sqlx::raw_sql("WITH RECURSIVE n(x) AS (SELECT 2 UNION ALL SELECT x+1 FROM n WHERE x<10002) INSERT INTO events(id,type,payload_json,wall_time_us) SELECT printf('00000000-0000-4000-8000-%012d',x),'private.audit','{}',x FROM n").execute(&mut db).await?;
    db.close().await?;
    let mut r = tf_store::Reader::open_existing(&path).await?;
    assert_eq!(
        r.event_page("workspace", Some(0)).await?["resync_required"],
        true
    );
    let boundary = r.event_page("workspace", Some(2)).await?;
    assert_eq!(boundary["resync_required"], false);
    assert_eq!(boundary["events"].as_array().ok_or("events")?.len(), 100);
    r.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
