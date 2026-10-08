//! Exact persisted occurrence/build metrics, scoped cursors and tombstone retention.
use serde_json::{Value, json};
use sqlx::Connection;
use tf_store::{
    Store,
    schedule_lifecycle::{Action, Control},
    schedules::Save,
};
#[allow(dead_code, reason = "Shared synthetic storage fixtures")]
#[path = "../../../tests/support/mod.rs"]
mod support;
fn id(n: u8) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}
#[tokio::test]
async fn metrics_cursors_and_delete_preserve_exact_history_and_missing_measurements()
-> Result<(), Box<dyn std::error::Error>> {
    let scratch = support::filesystem::ScratchDirectory::new()?;
    let path = scratch.path().join("schedule-history.sqlite");
    let mut store = Store::open(&path).await?;
    let workspace = id(1).parse()?;
    store
        .register_workspace(workspace, "schedule-history", 0)
        .await?;
    store.register_dataset(workspace, id(2).parse()?, 0).await?;
    let cases: Value =
        serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json"))?;
    let mut definition = cases
        .as_array()
        .ok_or("fixtures")?
        .iter()
        .find(|v| v["name"] == "schedule-definition-fixed")
        .ok_or("definition")?["value"]
        .clone();
    definition["build"]["source"] = json!({"kind":"working_tree","allow_additive_sync":false});
    definition["build"]["targets"] = json!([{"workspace_id":id(1),"dataset_id":id(2)}]);
    let schedule = id(3).parse()?;
    let saved = store
        .save_schedule(Save {
            id: schedule,
            workspace,
            definition: &definition,
            expected_etag: None,
            paused: false,
            actor: "test",
            at_us: 0,
            receipt: None,
        })
        .await?;
    let mut db = support::database::connect(&path).await?;
    for (n, state, finished) in [
        (10, "SUCCEEDED", Some(30)),
        (11, "SUCCEEDED", Some(50)),
        (12, "SUCCEEDED", None),
        (13, "FAILED", Some(50)),
    ] {
        sqlx::query("INSERT INTO build_plans VALUES(?,'ACCEPTED',NULL,'{}','{}','{}','[]','[]','[]','digest',NULL)").bind(id(n+50)).execute(&mut db).await?;
        sqlx::query("INSERT INTO builds(id,plan_id,trigger_json,requested_by,state,created_at_us,finished_at_us) VALUES(?,?,'{}','test',?,10,?)").bind(id(n+60)).bind(id(n+50)).bind(state).bind(finished).execute(&mut db).await?;
        sqlx::query("INSERT INTO schedule_occurrences VALUES(?,?,'epoch',?,?,'{\"format_version\":1,\"coalesced\":[{\"count\":\"2\"}]}',?, ?, ?)").bind(id(n)).bind(id(3)).bind(format!("evidence-{n}")).bind(i64::from(n)).bind(json!({"build":{"data_branch":"master"}}).to_string()).bind(state).bind(id(n+60)).execute(&mut db).await?;
    }
    sqlx::query(
        "INSERT INTO schedule_occurrences VALUES(?,?,'epoch','queued',20,'{}','{}','QUEUED',NULL)",
    )
    .bind(id(14))
    .bind(id(3))
    .execute(&mut db)
    .await?;
    let mut reader = store.reader().await?;
    let metrics = reader
        .schedule_metrics(workspace, schedule, 0, 100, 1)
        .await?;
    tf_protocol::validate_document("ApiScheduleMetricsV1", &metrics)?;
    assert_eq!(metrics["occurrences"], "5");
    assert_eq!(metrics["builds"], "4");
    assert_eq!(
        metrics["failure_rate"],
        json!({"numerator":"1","denominator":"4"})
    );
    assert_eq!(
        metrics["median_us"],
        json!({"numerator":"60","denominator":"2"})
    );
    assert_eq!(metrics["missing_duration_samples"], "1");
    assert_eq!(metrics["trailing_mean_us"], Value::Null);
    // Completion ordering includes missing timing; a missing latest selected success is unknown.
    let page = reader
        .schedule_history(workspace, schedule, None, 2)
        .await?;
    tf_protocol::validate_document("ApiScheduleHistoryV1", &page)?;
    assert_eq!(page["occurrences"].as_array().ok_or("page")?.len(), 2);
    let next = page["next_cursor"].as_str().ok_or("cursor")?.parse()?;
    let rest = reader
        .schedule_history(workspace, schedule, Some(next), 100)
        .await?;
    assert_eq!(rest["occurrences"].as_array().ok_or("rest")?.len(), 3);
    assert_eq!(rest["next_cursor"], Value::Null);
    assert!(
        reader
            .schedule_history(id(99).parse()?, schedule, None, 100)
            .await
            .is_err()
    );
    assert!(
        reader
            .schedule_history(workspace, id(99).parse()?, Some(next), 100)
            .await
            .is_err()
    );
    assert!(
        reader
            .schedule_metrics(workspace, schedule, 10, 10, 1)
            .await
            .is_err()
    );
    reader.close().await?;
    let etag = saved["etag"].as_str().ok_or("etag")?;
    let digest = "a".repeat(64);
    let key = id(40).parse()?;
    // Tombstoning preserves explicit manual requests and already accepted active work.
    sqlx::query("INSERT INTO schedule_occurrences VALUES(?,?,'epoch','manual',21,'{\"manual\":true}','{}','QUEUED',NULL)").bind(id(15)).bind(id(3)).execute(&mut db).await?;
    sqlx::query("INSERT INTO builds(id,plan_id,trigger_json,requested_by,state,created_at_us) VALUES(?,?,'{}','test','RUNNING',20)").bind(id(80)).bind(id(60)).execute(&mut db).await?;
    sqlx::query(
        "INSERT INTO schedule_occurrences VALUES(?,?,'epoch','running',22,'{}','{}','RUNNING',?)",
    )
    .bind(id(16))
    .bind(id(3))
    .bind(id(80))
    .execute(&mut db)
    .await?;
    sqlx::raw_sql("CREATE TRIGGER stop_schedule_delete BEFORE UPDATE OF deleted_at_us ON schedules BEGIN SELECT RAISE(ABORT,'injected deletion failure'); END;").execute(&mut db).await?;
    assert!(
        store
            .control_schedule(Control {
                workspace,
                id: schedule,
                etag,
                key,
                digest: &digest,
                actor: "test",
                at_us: 100,
                action: Action::Delete
            })
            .await
            .is_err()
    );
    let untouched:(Option<i64>,String)=sqlx::query_as("SELECT s.deleted_at_us,o.disposition FROM schedules s JOIN schedule_occurrences o ON o.schedule_id=s.id WHERE o.id=?").bind(id(14)).fetch_one(&mut db).await?;
    assert_eq!(untouched, (None, "QUEUED".into()));
    sqlx::raw_sql("DROP TRIGGER stop_schedule_delete;")
        .execute(&mut db)
        .await?;
    let response = store
        .control_schedule(Control {
            workspace,
            id: schedule,
            etag,
            key,
            digest: &digest,
            actor: "test",
            at_us: 100,
            action: Action::Delete,
        })
        .await?;
    tf_protocol::validate_document("ApiScheduleDeletedV1", &response["data"])?;
    assert_eq!(response["data"]["canceled_automatic"], "1");
    assert_eq!(
        store
            .control_schedule(Control {
                workspace,
                id: schedule,
                etag,
                key,
                digest: &digest,
                actor: "test",
                at_us: 101,
                action: Action::Delete
            })
            .await?,
        response
    );
    let mut reader = store.reader().await?;
    assert!(reader.schedule(workspace, schedule).await?.is_none());
    let history = reader
        .schedule_history(workspace, schedule, None, 100)
        .await?;
    assert_eq!(
        history["occurrences"].as_array().ok_or("retained")?.len(),
        7
    );
    reader.close().await?;
    let retained: Vec<(String, String)> = sqlx::query_as(
        "SELECT id,disposition FROM schedule_occurrences WHERE id IN (?,?) ORDER BY id",
    )
    .bind(id(15))
    .bind(id(16))
    .fetch_all(&mut db)
    .await?;
    assert_eq!(
        retained,
        vec![(id(15), "QUEUED".into()), (id(16), "RUNNING".into())]
    );
    db.close().await?;
    store.close().await?;
    Ok(())
}
