//! Fixed retained evidence distinguishes attempts, publications, cache hits and queue time.
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tf_store::{
    Reader, Store,
    history::{MetricsQuery, timeline},
};
fn id(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

#[tokio::test]
async fn retained_history_metrics_cache_retry_and_context_isolation()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("tf-history-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("catalog.sqlite");
    Store::open(&path).await?.close().await?;
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(true),
    )
    .await?;
    sqlx::query("INSERT INTO workspaces(id,root_identity,created_at_us) VALUES(?,'fixture',0)")
        .bind(id(1))
        .execute(&mut db)
        .await?;
    sqlx::query("INSERT INTO datasets VALUES(?,?,0,NULL)")
        .bind(id(2))
        .bind(id(1))
        .execute(&mut db)
        .await?;
    sqlx::query("INSERT INTO source_snapshots VALUES(?,?,'digest','{}',NULL,'{}','{}')")
        .bind(id(3))
        .bind(id(1))
        .execute(&mut db)
        .await?;
    sqlx::query("INSERT INTO dataset_definitions VALUES(?,?,'raw/items','transform','{}','{}')")
        .bind(id(3))
        .bind(id(2))
        .execute(&mut db)
        .await?;
    sqlx::query("INSERT INTO transform_definitions VALUES(?,?,?,'items','items','polars','{}','fingerprint')").bind(id(4)).bind(id(3)).bind(id(2)).execute(&mut db).await?;
    for (n, name) in [(5, "main"), (6, "other")] {
        sqlx::query("INSERT INTO data_branches VALUES(?,?,?,NULL,0,NULL)")
            .bind(id(n))
            .bind(id(1))
            .bind(name)
            .execute(&mut db)
            .await?;
    }
    sqlx::raw_sql("INSERT INTO artifacts VALUES(printf('%064d',0),'{}','{}',1,0,42,'VERIFIED');")
        .execute(&mut db)
        .await?;
    for (b, state, duration) in [
        (1, "SUCCEEDED", Some(10_i64)),
        (2, "SUCCEEDED", Some(30)),
        (3, "SUCCEEDED", None),
        (4, "FAILED", Some(90)),
        (5, "CANCELED", Some(11)),
        (6, "INTERRUPTED", Some(12)),
        (7, "QUEUED", None),
        (8, "SUCCEEDED", Some(100)),
        (9, "SUCCEEDED", Some(999)),
        (10, "SUCCEEDED", None),
    ] {
        let job_state = if b == 3 { "CACHED" } else { state };
        let branch = if b == 8 { 6 } else { 5 };
        let plan = json!({"format_version":1,"id":id(100+b),"workspace":id(1),"source":id(3),"source_digest":"a".repeat(64),"registry":"","replacement":"","discovery":{},"environment":{},"source_evidence":{},"output":{"name":if b==8 {"other"} else {"main"},"branch":id(branch),"revision":0,"deleted":false,"dataset":null,"version":null,"generation":0},"guards":[],"writes":[{"dataset":id(2),"job":id(300+b),"generation":0,"bindings":[]}],"reads":[],"context":{"parameters":{"cutoff":"captured"}},"created_us":0,"expires_us":1000});
        let draft: tf_store::planning::DraftPlan = serde_json::from_value(plan.clone())?;
        sqlx::query(
            "INSERT INTO build_plans VALUES(?,'ACCEPTED',?,?,'{}','{}','[]','[]','[]',?,NULL)",
        )
        .bind(id(100 + b))
        .bind(id(3))
        .bind(plan.to_string())
        .bind(draft.digest()?.hex())
        .execute(&mut db)
        .await?;
        sqlx::query("INSERT INTO builds(id,plan_id,trigger_json,requested_by,state,created_at_us,finished_at_us) VALUES(?,?,'{}','fixture',?,?,?)").bind(id(200+b)).bind(id(100+b)).bind(state).bind(if b==9 {100} else {b as i64}).bind(if b==7 {None} else {Some(1000_i64)}).execute(&mut db).await?;
        sqlx::query("INSERT INTO jobs(id,build_id,dataset_id,definition_id,branch_id,state,bindings_json) VALUES(?,?,?,?,?,?,'[]')").bind(id(300+b)).bind(id(200+b)).bind(id(2)).bind(id(4)).bind(id(branch)).bind(job_state).execute(&mut db).await?;
        if b == 3 {
            sqlx::query("INSERT INTO cached_jobs VALUES(?,?,?,1,NULL,3,0,'session',1)")
                .bind(id(303))
                .bind(id(501))
                .bind(id(401))
                .execute(&mut db)
                .await?;
            continue;
        }
        if b == 7 {
            continue;
        }
        if b == 1 {
            sqlx::query("INSERT INTO attempts(id,job_id,attempt_no,session_id,fence,state,started_at_us,finished_at_us) VALUES(?,?,1,'s',1,'FAILED',10,20)").bind(id(400)).bind(id(301)).execute(&mut db).await?;
            sqlx::query("INSERT INTO phase_intervals VALUES(?,0,'Running',7,10,20)")
                .bind(id(400))
                .execute(&mut db)
                .await?;
        }
        sqlx::query("INSERT INTO attempts(id,job_id,attempt_no,session_id,fence,state,started_at_us,finished_at_us) VALUES(?,?,?,'s',1,?,100,?)").bind(id(400+b)).bind(id(300+b)).bind(if b==1 {2} else {1}).bind(state).bind(200+b as i64).execute(&mut db).await?;
        if let Some(n) = duration {
            sqlx::query("INSERT INTO phase_intervals VALUES(?,0,'Committing',?,100,?)")
                .bind(id(400 + b))
                .bind(n)
                .bind(200 + b as i64)
                .execute(&mut db)
                .await?;
        }
        if state == "SUCCEEDED" {
            sqlx::query("INSERT INTO dataset_versions VALUES(?,?,printf('%064d',0),?,NULL,?,?,'compute','check')").bind(id(500+b)).bind(id(2)).bind(id(400+b)).bind(id(3)).bind(200+b as i64).execute(&mut db).await?;
        }
    }
    db.close().await?;
    let mut rd = Reader::open_existing(&path).await?;
    let branch = "main".parse()?;
    let workspace = id(1).parse()?;
    let query = |materialized_any| MetricsQuery {
        workspace,
        branch: &branch,
        from_us: 0,
        to_us: 100,
        dataset: None,
        materialized_any,
        window: 1,
    };
    let m = rd.execution_metrics(query(false)).await?;
    assert_eq!(m["builds"], "8");
    assert_eq!(m["jobs_executed"], "6");
    assert_eq!(m["attempts"], "7");
    assert_eq!(
        m["failure_rate"],
        json!({"numerator":"1","denominator":"5"})
    );
    assert_eq!(m["materializations"], "3");
    assert_eq!(m["duration_samples"], "2");
    assert_eq!(m["missing_duration_samples"], "1");
    assert_eq!(m["median_ns"], json!({"numerator":"40","denominator":"2"}));
    assert!(m["trailing_mean_ns"].is_null());
    assert_eq!(m["trailing_samples"], "0");
    let mut complete = query(false);
    complete.to_us = 10;
    let complete = rd.execution_metrics(complete).await?;
    assert_eq!(
        complete["trailing_mean_ns"],
        json!({"numerator":"30","denominator":"1"})
    );
    assert_eq!(m["manual_requests"], "8");
    assert!(m["schedule_occurrences"].is_null());
    let filtered = rd.execution_metrics(query(true)).await?;
    assert_eq!(filtered["builds"], "3");
    assert_eq!(
        filtered["failure_rate"],
        json!({"numerator":"0","denominator":"3"})
    );
    let mut empty = query(false);
    empty.from_us = 101;
    empty.to_us = 102;
    let empty = rd.execution_metrics(empty).await?;
    assert_eq!(empty["duration_samples"], "0");
    assert!(empty["median_ns"].is_null() && empty["failure_rate"].is_null());
    let history = rd
        .dataset_history(id(1).parse()?, &branch, id(2).parse()?, "", 2)
        .await?;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0]["attempt_count"], "2");
    assert_eq!(history[0]["duration_ns"], "17");
    assert_eq!(history[0]["attempts"][0]["state"], "FAILED");
    assert_eq!(history[0]["attempts"][0]["duration_ns"], "7");
    assert_eq!(history[0]["attempts"][1]["duration_ns"], "10");
    let next = rd
        .dataset_history(
            id(1).parse()?,
            &branch,
            id(2).parse()?,
            history[1]["id"].as_str().ok_or("history cursor missing")?,
            2,
        )
        .await?;
    assert_eq!(next[0]["reused_version"], id(501));
    assert_eq!(next[0]["attempt_count"], "0");
    assert!(next[0]["duration_ns"].is_null());
    assert_eq!(next[0]["attempts"], json!([]));
    // Versions sharing a capture must select their own producing plan, not the first plan UUID.
    let (_, producing_plan) = rd
        .api_version_source(id(1).parse()?, id(2).parse()?, id(502).parse()?)
        .await?;
    assert_eq!(producing_plan.ok_or("producing plan missing")?.id, id(102));
    let report = rd.build_report(id(1).parse()?, id(201).parse()?).await?;
    let t = timeline(&report)?;
    assert_eq!(t["jobs"][0]["duration_ns"], "17");
    assert_eq!(t["initial_queue_wait_us"], "9");
    assert_eq!(t["critical_path"]["duration_ns"], "17");
    assert_eq!(report["jobs"][0]["attempts"][0]["state"], "FAILED");
    assert_eq!(
        report["plan"]["context"]["parameters"]["cutoff"],
        "captured"
    );
    let cached = timeline(&rd.build_report(id(1).parse()?, id(203).parse()?).await?)?;
    assert!(cached["jobs"][0]["duration_ns"].is_null());
    assert!(cached["initial_queue_wait_us"].is_null());
    assert_eq!(cached["jobs"][0]["attempts"], json!([]));
    assert_eq!(cached["critical_path"]["duration_ns"], "0");
    assert!(
        timeline(&rd.build_report(id(1).parse()?, id(207).parse()?).await?)?["critical_path"]
            .is_null()
    );
    let versions = rd
        .api_versions(id(1).parse()?, id(2).parse()?, "", 100, false)
        .await?;
    assert_eq!(versions[0]["row_count"], "0");
    assert_eq!(versions[0]["file_count"], "1");
    assert_eq!(versions[0]["byte_count"], "42");
    assert_eq!(
        rd.attempt_build(id(1).parse()?, id(400).parse()?)
            .await?
            .to_string(),
        id(201)
    );
    assert!(
        rd.attempt_build(id(999).parse()?, id(400).parse()?)
            .await
            .is_err()
    );
    tf_protocol::validate_document("ApiExecutionMetricsV1", &m)?;
    tf_protocol::validate_document("ApiExecutionTimelineV1", &t)?;
    let revision = rd.api_revision().await?;
    let mut writer =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path)).await?;
    sqlx::query("INSERT INTO phase_intervals VALUES(?,0,'Committing',NULL,100,NULL)")
        .bind(id(410))
        .execute(&mut writer)
        .await?;
    let opened = rd.api_revision().await?;
    assert_ne!(revision, opened);
    // Closing the phase does not change the already terminal attempt/job state.
    sqlx::query("UPDATE phase_intervals SET duration_ns=50,finished_at_us=210 WHERE attempt_id=?")
        .bind(id(410))
        .execute(&mut writer)
        .await?;
    assert_ne!(opened, rd.api_revision().await?);
    writer.close().await?;
    rd.close().await?;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn missing_phase_measurement_never_becomes_zero() {
    let attempt = json!({"state":"SUCCEEDED","phases":[{"finished_us":"10","duration_ns":null}]});
    assert_eq!(tf_store::history::attempt_duration(&attempt), None);
    let open: Value = json!({"state":"RUNNING","phases":[{"finished_us":null,"duration_ns":"0"}]});
    assert_eq!(tf_store::history::attempt_duration(&open), None);
}

#[test]
fn timeline_resolves_accepted_dependencies_and_includes_failed_retries()
-> Result<(), Box<dyn std::error::Error>> {
    let attempt = |i, state, duration: u64| json!({"id":id(i),"number":1,"state":state,"started_us":"10","finished_us":"20","phases":[{"phase":"Running","started_us":"10","finished_us":"20","duration_ns":duration.to_string()}]});
    let report = json!({"id":id(1),"state":"SUCCEEDED","source":id(2),"created_us":"0","finished_us":"40","plan":{"id":id(3),"writes":[{"dataset":id(4),"bindings":[]},{"dataset":id(5),"bindings":[{"kind":"planned","dataset":id(4)}]},{"dataset":id(6),"bindings":[{"kind":"planned","dataset":id(4)}]}]},"jobs":[
        {"id":id(10),"dataset":id(4),"state":"SUCCEEDED","attempts":[attempt(20,"FAILED",7),attempt(21,"SUCCEEDED",10)]},
        {"id":id(11),"dataset":id(5),"state":"SUCCEEDED","attempts":[attempt(22,"SUCCEEDED",30)]},
        {"id":id(12),"dataset":id(6),"state":"SUCCEEDED","attempts":[attempt(23,"SUCCEEDED",25)]}
    ]});
    let t = timeline(&report)?;
    assert_eq!(
        t["critical_path"],
        json!({"duration_ns":"47","jobs":[id(10),id(11)]})
    );
    assert_eq!(t["jobs"][0]["attempts"][0]["state"], "FAILED");
    assert_eq!(t["jobs"][0]["attempts"].as_array().map(Vec::len), Some(2));
    Ok(())
}
