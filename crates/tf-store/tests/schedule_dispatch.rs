//! Real SQLite acceptance rollback, pause eligibility and restart outcome linking.
use serde_json::{Value, json};
use sqlx::Connection;
use tf_store::{Store, planning::DraftPlan, schedule_dispatch::Pending};
#[allow(dead_code, reason = "Shared filesystem and SQLite fixture utilities")]
#[path = "../../../tests/support/mod.rs"]
mod support;
fn id(n: u8) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}
#[tokio::test]
async fn scheduled_acceptance_links_once_and_rolls_back_all_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let tree = support::filesystem::ScratchDirectory::new()?;
    let path = tree.path().join("catalog.sqlite");
    let mut store = Store::open(&path).await?;
    let mut db = support::database::connect(&path).await?;
    let workspace = id(1).parse()?;
    store.register_workspace(workspace, "fixture", 0).await?;
    store.register_dataset(workspace, id(2).parse()?, 0).await?;
    sqlx::query("UPDATE workspaces SET runtime_owner=? WHERE id=?")
        .bind(id(3))
        .bind(id(1))
        .execute(&mut db)
        .await?;
    let pending = Pending {
        occurrence: id(4).parse()?,
        schedule: id(5).parse()?,
        execution: json!({"build":{"targets":[{"workspace_id":id(1),"dataset_id":id(2)}],"data_branch":"master"},"policies":{"max_consecutive_builds":2,"minimum_delay_seconds":0}}),
        payload: json!({"manual":true,"tokens":[],"input_pins":[]}),
    };
    sqlx::query(
        "INSERT INTO schedules VALUES(?,?,'fixture','{}','etag','epoch',1,0,0,'test',0,NULL)",
    )
    .bind(id(5))
    .bind(id(1))
    .execute(&mut db)
    .await?;
    sqlx::query(
        "INSERT INTO schedule_occurrences VALUES(?,?,'epoch','digest',0,?,?,'QUEUED',NULL)",
    )
    .bind(id(4))
    .bind(id(5))
    .bind(pending.payload.to_string())
    .bind(pending.execution.to_string())
    .execute(&mut db)
    .await?;
    assert!(
        store.next_schedule_dispatch(workspace, 10).await?.is_some(),
        "manual dispatch works while paused"
    );
    let plan: DraftPlan = serde_json::from_value(
        json!({"format_version":1,"id":id(6),"workspace":id(1),"source":id(7),"source_digest":"a".repeat(64),"registry":"","replacement":"","discovery":{},"environment":{},"source_evidence":{"selector":{"kind":"working_tree"},"files":[],"git":null},"output":{"name":"master","branch":null,"revision":null,"deleted":false,"dataset":null,"version":null,"generation":0},"guards":[],"writes":[{"dataset":id(2),"job":id(8),"generation":0,"bindings":[]}],"reads":[],"context":{"schedule":serde_json::to_value(&pending)?,"branch_policies":{"":[]},"parameters":{}},"created_us":0,"expires_us":1000}),
    )?;
    store.save_draft(&plan).await?;
    sqlx::raw_sql(
        "CREATE TRIGGER injected BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT,'injected'); END;",
    )
    .execute(&mut db)
    .await?;
    assert!(
        store
            .accept_draft(&plan, id(9).parse()?, id(10).parse()?, id(3).parse()?, 10)
            .await
            .is_err()
    );
    for table in [
        "builds",
        "jobs",
        "data_branches",
        "source_snapshots",
        "write_reservations",
    ] {
        let n: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                .fetch_one(&mut db)
                .await?;
        assert_eq!(n, 0, "rollback must leave {table} unchanged");
    }
    let state: (String, Option<String>) =
        sqlx::query_as("SELECT disposition,build_id FROM schedule_occurrences")
            .fetch_one(&mut db)
            .await?;
    assert_eq!(state, ("QUEUED".into(), None));
    sqlx::raw_sql("DROP TRIGGER injected")
        .execute(&mut db)
        .await?;
    store
        .accept_draft(&plan, id(9).parse()?, id(10).parse()?, id(3).parse()?, 10)
        .await?;
    assert!(
        store
            .accept_draft(&plan, id(11).parse()?, id(12).parse()?, id(3).parse()?, 11)
            .await
            .is_err()
    );
    let linked:(String,String)=sqlx::query_as("SELECT b.occurrence_id,o.build_id FROM builds b JOIN schedule_occurrences o ON o.id=b.occurrence_id").fetch_one(&mut db).await?;
    assert_eq!(linked, (id(4), id(9)));
    let mut automatic = pending.clone();
    automatic.payload = json!({"tokens":[{"build_id":id(9)}],"input_pins":[]});
    assert!(
        store
            .check_schedule_dispatch_ancestry(&automatic)
            .await
            .is_err()
    );
    automatic.schedule = id(11).parse()?;
    automatic.execution["policies"]["max_consecutive_builds"] = json!(1);
    assert!(
        store
            .check_schedule_dispatch_ancestry(&automatic)
            .await
            .is_err()
    );
    automatic.payload["manual"] = json!(true);
    store.check_schedule_dispatch_ancestry(&automatic).await?;
    assert!(store.next_schedule_dispatch(workspace, 20).await?.is_none());
    // Simulate restart fencing; terminal build outcome resolves occurrence, without resubmission.
    sqlx::query("UPDATE builds SET state='INTERRUPTED',finished_at_us=20 WHERE id=?")
        .bind(id(9))
        .execute(&mut db)
        .await?;
    assert_eq!(store.reconcile_schedule_dispatches(workspace, 20).await?, 1);
    assert_eq!(store.reconcile_schedule_dispatches(workspace, 21).await?, 0);
    let state: String = sqlx::query_scalar("SELECT disposition FROM schedule_occurrences")
        .fetch_one(&mut db)
        .await?;
    assert_eq!(state, "FAILED");
    db.close().await?;
    store.close().await?;
    Ok(())
}
#[tokio::test]
async fn automatic_pause_guard_and_frozen_evidence_conflict()
-> Result<(), Box<dyn std::error::Error>> {
    let tree = support::filesystem::ScratchDirectory::new()?;
    let path = tree.path().join("catalog.sqlite");
    let mut store = Store::open(&path).await?;
    let workspace = id(1).parse()?;
    store.register_workspace(workspace, "fixture", 0).await?;
    let mut db = support::database::connect(&path).await?;
    let pending = Pending {
        occurrence: id(4).parse()?,
        schedule: id(5).parse()?,
        execution: json!({"policies":{"max_consecutive_builds":2,"minimum_delay_seconds":0}}),
        payload: json!({"tokens":[],"input_pins":[]}),
    };
    sqlx::query(
        "INSERT INTO schedules VALUES(?,?,'fixture','{}','etag','epoch',1,0,0,'test',0,NULL)",
    )
    .bind(id(5))
    .bind(id(1))
    .execute(&mut db)
    .await?;
    sqlx::query(
        "INSERT INTO schedule_occurrences VALUES(?,?,'epoch','digest',0,?,?,'QUEUED',NULL)",
    )
    .bind(id(4))
    .bind(id(5))
    .bind(pending.payload.to_string())
    .bind(pending.execution.to_string())
    .execute(&mut db)
    .await?;
    assert!(store.next_schedule_dispatch(workspace, 10).await?.is_none());
    assert!(
        store
            .fail_schedule_dispatch(workspace, &pending, "fixture failure", 10)
            .await
            .is_err()
    );
    sqlx::raw_sql("UPDATE schedules SET paused=0")
        .execute(&mut db)
        .await?;
    let selected = store
        .next_schedule_dispatch(workspace, 10)
        .await?
        .ok_or("missing queued occurrence")?;
    assert_eq!(selected.execution, pending.execution);
    let mut changed = selected.clone();
    changed.execution = Value::Null;
    assert!(
        store
            .fail_schedule_dispatch(workspace, &changed, "fixture failure", 10)
            .await
            .is_err()
    );
    store
        .fail_schedule_dispatch(
            workspace,
            &selected,
            "Register outputs in the selected source",
            10,
        )
        .await?;
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE type='schedule.succeeded'")
            .fetch_one(&mut db)
            .await?;
    assert_eq!(events, 0);
    db.close().await?;
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn causal_hops_and_consecutive_external_bursts_are_bounded()
-> Result<(), Box<dyn std::error::Error>> {
    let tree = support::filesystem::ScratchDirectory::new()?;
    let path = tree.path().join("causal.sqlite");
    let mut store = Store::open(&path).await?;
    let workspace = id(1).parse()?;
    store
        .register_workspace(workspace, "causal-fixture", 0)
        .await?;
    let mut db = support::database::connect(&path).await?;
    sqlx::query(
        "INSERT INTO schedules VALUES(?,?,'causal','{}','etag','epoch',0,0,0,'test',0,NULL)",
    )
    .bind(id(5))
    .bind(id(1))
    .execute(&mut db)
    .await?;
    let mut parent: Option<String> = None;
    for n in 40..56 {
        let plan = id(n + 60);
        sqlx::query("INSERT INTO build_plans VALUES(?,'ACCEPTED',NULL,'{}','{}','{}','[]','[]','[]','digest',NULL)").bind(&plan).execute(&mut db).await?;
        let trigger = json!({"kind":"schedule","schedule_id":id(n + 100),"evidence":{"tokens":parent.as_ref().map(|id|vec![json!({"build_id":id})]).unwrap_or_default()}});
        sqlx::query("INSERT INTO builds(id,plan_id,trigger_json,requested_by,state,created_at_us,finished_at_us) VALUES(?,?,?,'schedule','FAILED',0,10)").bind(id(n)).bind(plan).bind(trigger.to_string()).execute(&mut db).await?;
        parent = Some(id(n));
    }
    let mut pending = Pending {
        occurrence: id(6).parse()?,
        schedule: id(5).parse()?,
        execution: json!({"policies":{"max_consecutive_builds":100}}),
        payload: json!({"tokens":[{"build_id":id(54)}]}),
    };
    store.check_schedule_dispatch_ancestry(&pending).await?;
    pending.payload["tokens"][0]["build_id"] = json!(id(55));
    assert!(
        store
            .check_schedule_dispatch_ancestry(&pending)
            .await
            .is_err(),
        "the runtime hop ceiling is independent of a larger configured burst limit"
    );
    pending.payload = json!({"tokens":[]});
    pending.execution["policies"]["max_consecutive_builds"] = json!(1);
    sqlx::query(
        "INSERT INTO schedule_occurrences VALUES(?,?,'epoch','prior',0,'{}','{}','SUCCEEDED',?)",
    )
    .bind(id(7))
    .bind(id(5))
    .bind(id(40))
    .execute(&mut db)
    .await?;
    sqlx::query(
        "INSERT INTO schedule_occurrences VALUES(?,?,'epoch','queued',5,'{}','{}','QUEUED',NULL)",
    )
    .bind(id(6))
    .bind(id(5))
    .execute(&mut db)
    .await?;
    assert!(
        store
            .check_schedule_dispatch_ancestry(&pending)
            .await
            .is_err(),
        "external evidence queued during the previous build still belongs to the bounded drain"
    );
    pending.occurrence = id(8).parse()?;
    sqlx::query(
        "INSERT INTO schedule_occurrences VALUES(?,?,'epoch','fresh',11,'{}','{}','QUEUED',NULL)",
    )
    .bind(id(8))
    .bind(id(5))
    .execute(&mut db)
    .await?;
    store.check_schedule_dispatch_ancestry(&pending).await?;
    pending.occurrence = id(6).parse()?;
    pending.payload["manual"] = json!(true);
    store.check_schedule_dispatch_ancestry(&pending).await?;
    db.close().await?;
    store.close().await?;
    Ok(())
}
