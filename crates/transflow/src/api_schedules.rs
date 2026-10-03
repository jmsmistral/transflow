//! Guarded schedule definitions and lifecycle actions, composed with persistent dispatch.
use crate::api_read::{self, Result, bad};
use std::path::Path;
use tf_api::{ApiError as E, Reply, Request};
use tf_domain::{ScheduleId, WorkspaceId};
use tf_exec::ownership::RuntimeOwner;
use tf_store::{
    api::Receipt,
    schedules::{Error, Save},
};

pub(crate) fn matches(path: &str) -> bool {
    path == "/api/v1/schedules" || path.starts_with("/api/v1/schedules/")
}
fn error(e: Error) -> E {
    match e {
        Error::Conflict { current_etag } => {
            let mut error = E::new(
                409,
                "TF_API_SCHEDULE_CONFLICT",
                "The schedule changed; reload its current definition before saving",
            );
            error.details = serde_json::json!({"current_etag":current_etag});
            error
        }
        Error::Limit => E::new(
            409,
            "TF_API_SCHEDULE_LIMIT",
            "The schedule request exceeds its pending or replay limit",
        ),
        Error::Missing => E::missing(),
        Error::Store(tf_store::StoreError::InvalidRequest) => E::invalid(),
        e => bad(e),
    }
}
pub(crate) fn read(root: &Path, workspace: WorkspaceId, r: &Request) -> Result<Reply> {
    if r.query.keys().any(|key| key != "after") {
        return Err(E::invalid());
    }
    let rt = api_read::runtime()?;
    let value = rt.block_on(async {
        let mut rd = api_read::reader(root).await?;
        let result = if r.path == "/api/v1/schedules" {
            let after = r.query.get("after").map(String::as_str).unwrap_or("");
            if !after.is_empty() {
                after.parse::<ScheduleId>().map_err(|_| E::invalid())?;
            }
            rd.schedules(workspace, after).await.map_err(error)
        } else {
            if !r.query.is_empty() {
                return Err(E::invalid());
            }
            let id = r
                .path
                .strip_prefix("/api/v1/schedules/")
                .ok_or_else(E::invalid)?;
            rd.schedule(workspace, id.parse().map_err(|_| E::invalid())?)
                .await
                .map_err(error)?
                .ok_or_else(E::missing)
        };
        rd.close().await.map_err(bad)?;
        result
    })?;
    let etag = value["etag"].as_str().map(str::to_owned);
    Ok(Reply {
        data: value,
        context: None,
        etag,
    })
}
pub(crate) fn save(
    owner: &mut RuntimeOwner,
    r: &Request,
    rt: &tokio::runtime::Runtime,
    digest: &str,
) -> Result<Reply> {
    if r.query
        .keys()
        .any(|k| !matches!(k.as_str(), "replay_after_event" | "replay_limit"))
        || (r.method != "PUT" && !r.query.is_empty())
    {
        return Err(E::invalid());
    }
    let key = r.key.ok_or_else(E::invalid)?;
    let workspace = owner.workspace_id().map_err(bad)?;
    // Receipts are checked under the owner, then committed atomically with a successful save.
    let prior = rt.block_on(async {
        let mut store = owner.open_store().await.map_err(bad)?;
        let mut reader = store
            .repository()
            .map_err(bad)?
            .reader()
            .await
            .map_err(bad)?;
        let prior = reader.api_receipt(key, digest).await.map_err(bad);
        reader.close().await.map_err(bad)?;
        store.close().await.map_err(bad)?;
        prior
    })?;
    match prior {
        Receipt::Complete(v) => {
            return Ok(Reply {
                data: v["data"].clone(),
                context: None,
                etag: v["etag"].as_str().map(str::to_owned),
            });
        }
        Receipt::New => {}
        _ => return Err(E::conflict()),
    }
    if r.method == "POST" && r.path != "/api/v1/schedules" {
        return control(owner, r, rt, digest);
    }
    let replay = replay_request(
        &serde_json::json!({"replay_after_event":r.query.get("replay_after_event"),"replay_limit":r.query.get("replay_limit").map(|s|s.parse::<u32>()).transpose().map_err(|_|E::invalid())?}),
    )?;
    let (id, definition, paused, expected) = if r.method == "POST" && r.path == "/api/v1/schedules"
    {
        if r.if_match.is_some() {
            return Err(E::invalid());
        }
        (
            r.body["id"]
                .as_str()
                .ok_or_else(E::invalid)?
                .parse()
                .map_err(|_| E::invalid())?,
            &r.body["definition"],
            r.body["paused"].as_bool().ok_or_else(E::invalid)?,
            None,
        )
    } else if r.method == "PUT" {
        let id = r
            .path
            .strip_prefix("/api/v1/schedules/")
            .ok_or_else(E::invalid)?
            .parse()
            .map_err(|_| E::invalid())?;
        (
            id,
            &r.body,
            false,
            Some(r.if_match.as_deref().ok_or_else(E::invalid)?),
        )
    } else {
        return Err(E::invalid());
    };
    let normalized =
        tf_protocol::schedule::Definition::decode(definition).map_err(|_| E::invalid())?;
    validate_source(owner.workspace_root(), workspace, &normalized)?;
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(bad)?
            .as_micros(),
    )
    .map_err(bad)?;
    let data = rt.block_on(async {
        let mut store = owner.open_store().await.map_err(bad)?;
        let result = store
            .repository()
            .map_err(bad)?
            .save_schedule_with_replay(
                Save {
                    id,
                    workspace,
                    definition,
                    expected_etag: expected,
                    paused,
                    actor: "authenticated_api",
                    at_us: now,
                    receipt: Some((key, digest)),
                },
                replay,
            )
            .await
            .map_err(error);
        store.close().await.map_err(bad)?;
        result
    })?;
    let etag = data["etag"].as_str().map(str::to_owned);
    Ok(Reply {
        data,
        context: None,
        etag,
    })
}

fn replay_request(
    value: &serde_json::Value,
) -> Result<Option<tf_store::schedule_lifecycle::Replay>> {
    if value["replay_after_event"].is_null() && value["replay_limit"].is_null() {
        return Ok(None);
    }
    let after = value["replay_after_event"]
        .as_str()
        .ok_or_else(E::invalid)?
        .parse::<i64>()
        .map_err(|_| E::invalid())?;
    let limit = value["replay_limit"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(E::invalid)?;
    if after < 0 || !(1..=100).contains(&limit) {
        return Err(E::invalid());
    }
    Ok(Some(tf_store::schedule_lifecycle::Replay { after, limit }))
}
fn control(
    owner: &mut RuntimeOwner,
    r: &Request,
    rt: &tokio::runtime::Runtime,
    digest: &str,
) -> Result<Reply> {
    use tf_store::schedule_lifecycle::{Action, Control};
    let (id, action) = r
        .path
        .strip_prefix("/api/v1/schedules/")
        .and_then(|s| s.split_once('/'))
        .ok_or_else(E::invalid)?;
    let action = match action {
        "pause" => {
            tf_protocol::validate_document("ApiEmptyV1", &r.body).map_err(|_| E::invalid())?;
            Action::Pause
        }
        "resume" => {
            tf_protocol::validate_document("ApiScheduleResumeV1", &r.body)
                .map_err(|_| E::invalid())?;
            Action::Resume(replay_request(&r.body)?)
        }
        "run" => {
            tf_protocol::validate_document("ApiEmptyV1", &r.body).map_err(|_| E::invalid())?;
            Action::Run
        }
        _ => return Err(E::invalid()),
    };
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(bad)?
            .as_micros(),
    )
    .map_err(bad)?;
    let workspace = owner.workspace_id().map_err(bad)?;
    let id = id.parse().map_err(|_| E::invalid())?;
    let etag = r.if_match.as_deref().ok_or_else(E::invalid)?;
    let value = rt.block_on(async {
        let mut store = owner.open_store().await.map_err(bad)?;
        let result = store
            .repository()
            .map_err(bad)?
            .control_schedule(Control {
                workspace,
                id,
                etag,
                key: r.key.ok_or_else(E::invalid)?,
                digest,
                actor: "authenticated_api",
                at_us: now,
                action,
            })
            .await
            .map_err(error);
        store.close().await.map_err(bad)?;
        result
    })?;
    Ok(Reply {
        data: value["data"].clone(),
        context: None,
        etag: value["etag"].as_str().map(str::to_owned),
    })
}
// Saving checks captured registry identity without importing Python or executing a producer.
fn validate_source(
    root: &Path,
    workspace: WorkspaceId,
    definition: &tf_protocol::schedule::Definition,
) -> Result<()> {
    use tf_catalog::{RegistrySnapshot, capture::SourceSnapshot, workspace::Workspace};
    use tf_protocol::schedule::Source;
    let selected = Workspace::load(root, Some(root)).map_err(bad)?;
    let capture = match &definition.build.source {
        Source::FixedSnapshot { snapshot_id } => SourceSnapshot::open(&root.join(".transflow/runtime/source-snapshots"), snapshot_id.parse().map_err(|_| E::invalid())?).map_err(|_| E::missing())?,
        Source::GitRef { reference } => tf_catalog::git::capture_ref(&selected, reference, Some(&definition.build.data_branch.parse().map_err(|_| E::invalid())?), crate::preparation::limits()).map_err(|_| E::new(422,"TF_API_SCHEDULE_SOURCE","The saved source ref cannot be captured; check its committed workspace and registered datasets"))?,
        Source::WorkingTree { .. } => tf_catalog::git::capture_working_tree(&selected, crate::preparation::limits()).map_err(bad)?,
    };
    let config_bytes = capture
        .read(Path::new("workspace.toml"), 16 * 1024 * 1024)
        .map_err(bad)?;
    let config = tf_catalog::workspace::WorkspaceConfig::parse(
        std::str::from_utf8(&config_bytes).map_err(bad)?,
    )
    .map_err(bad)?;
    if config.id() != workspace {
        return Err(E::missing());
    }
    let bytes = capture
        .read(Path::new(".transflow/catalog.toml"), 16 * 1024 * 1024)
        .map_err(bad)?;
    let registry = RegistrySnapshot::parse(workspace, std::str::from_utf8(&bytes).map_err(bad)?)
        .map_err(bad)?;
    for item in definition
        .build
        .targets
        .iter()
        .chain(&definition.build.exclusions)
        .chain(&definition.build.refresh_sources)
    {
        let key = item.key().map_err(|_| E::invalid())?;
        if !registry
            .datasets()
            .any(|d| d.key() == key && !d.is_tombstone())
        {
            return Err(E::missing());
        }
    }
    for provider in definition.build.provider_fallback_policies.keys() {
        if !registry
            .external_registrations()
            .any(|e| e.key().workspace_id().to_string() == *provider)
        {
            return Err(E::missing());
        }
    }
    // Boundaries may be registered external datasets; provider data stays provider owned.
    let entries = tf_catalog::browse::entries(&registry);
    for item in &definition.build.boundaries {
        if !entries
            .iter()
            .any(|e| e["workspace_id"] == item.workspace_id && e["dataset_id"] == item.dataset_id)
        {
            return Err(E::missing());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "Synthetic workspace and API fixtures")]
    use super::*;
    use serde_json::{Value, json};
    use sqlx::Connection;
    use std::{collections::BTreeMap, fs, process::Command};
    use tf_catalog::{capture::SourceSnapshot, workspace::Workspace};
    use tf_domain::{DatasetId, RequestId};
    use tf_exec::ownership::CoordinatorMode;
    struct Tree(std::path::PathBuf);
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn git(root: &Path, args: &[&str]) {
        let o = Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    #[test]
    fn definitions_support_git_free_capture_clean_ref_cas_and_exact_receipt_replay() {
        let tree =
            Tree(std::env::temp_dir().join(format!("tf-schedule-api-{}", std::process::id())));
        fs::create_dir_all(tree.0.join("src")).unwrap();
        fs::create_dir_all(tree.0.join(".transflow")).unwrap();
        let workspace: WorkspaceId = "00000001-0000-4000-8000-000000000001".parse().unwrap();
        let dataset: DatasetId = "00000002-0000-4000-8000-000000000002".parse().unwrap();
        fs::write(
            tree.0.join("workspace.toml"),
            format!("format_version=1\nworkspace_id='{workspace}'\n"),
        )
        .unwrap();
        fs::write(tree.0.join(".transflow/catalog.toml"),format!("format_version=1\n[[datasets]]\nid='{dataset}'\npath='example/result'\nkind='transform'\n")).unwrap();
        fs::write(tree.0.join("src/example.py"), "VALUE=1\n").unwrap();
        fs::write(tree.0.join("requirements.in"), "polars==1.44.2\n").unwrap();
        fs::write(tree.0.join("requirements.lock"), "synthetic-lock\n").unwrap();
        fs::write(tree.0.join(".gitignore"), ".transflow/runtime/\n").unwrap();
        let rt = api_read::runtime().unwrap();
        fs::create_dir_all(tree.0.join(".transflow/runtime")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            tree.0.join(".transflow/runtime"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let w = Workspace::load(&tree.0, Some(&tree.0)).unwrap();
        let capture = SourceSnapshot::capture(&w, crate::preparation::limits()).unwrap();
        let mut owner =
            RuntimeOwner::acquire(&tree.0, workspace, CoordinatorMode::MetadataOnly).unwrap();
        rt.block_on(async {
            let mut store = owner.open_store().await.unwrap();
            let repo = store.repository().unwrap();
            repo.register_workspace(workspace, "synthetic", 1)
                .await
                .unwrap();
            repo.register_dataset(workspace, dataset, 1).await.unwrap();
            store.close().await.unwrap();
            let mut db = sqlx::SqliteConnection::connect_with(
                &sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(tree.0.join(".transflow/runtime/catalog.sqlite")),
            )
            .await
            .unwrap();
            sqlx::query("INSERT INTO source_snapshots VALUES(?,?,?,'{}',NULL,'{}','{}')")
                .bind(capture.id().unwrap().to_string())
                .bind(workspace.to_string())
                .bind("a".repeat(64))
                .execute(&mut db)
                .await
                .unwrap();
            db.close().await.unwrap();
        });
        let cases: Value =
            serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json"))
                .unwrap();
        let mut definition = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "schedule-definition-fixed")
            .unwrap()["value"]
            .clone();
        definition["build"]["source"]["snapshot_id"] = json!(capture.id().unwrap().to_string());
        let id = ScheduleId::from_bytes([8; 16]);
        let mut request = Request {
            id: RequestId::from_bytes([9; 16]),
            method: "POST".into(),
            path: "/api/v1/schedules".into(),
            query: BTreeMap::new(),
            body: json!({"id":id.to_string(),"paused":true,"definition":definition}),
            key: Some(RequestId::from_bytes([10; 16])),
            if_match: None,
        };
        let first = save(&mut owner, &request, &rt, &"a".repeat(64)).unwrap();
        assert_eq!(
            save(&mut owner, &request, &rt, &"a".repeat(64))
                .unwrap()
                .data,
            first.data
        );
        request.method = "PUT".into();
        request.path = format!("/api/v1/schedules/{id}");
        request.body = definition.clone();
        request.if_match = first.etag.clone();
        request.key = Some(RequestId::from_bytes([11; 16]));
        let second = save(&mut owner, &request, &rt, &"b".repeat(64)).unwrap();
        request.key = Some(RequestId::from_bytes([12; 16]));
        let conflict = save(&mut owner, &request, &rt, &"c".repeat(64))
            .err()
            .unwrap();
        assert_eq!(conflict.status, 409);
        assert_eq!(conflict.details["current_etag"], second.data["etag"]);
        git(&tree.0, &["init", "--quiet", "--initial-branch=stable"]);
        git(&tree.0, &["add", "."]);
        git(&tree.0, &["commit", "--quiet", "-m", "synthetic"]);
        git(&tree.0, &["checkout", "--quiet", "-b", "other"]);
        fs::write(tree.0.join("src/example.py"), "VALUE=2\n").unwrap();
        git(&tree.0, &["add", "."]);
        git(&tree.0, &["commit", "--quiet", "-m", "synthetic other"]);
        request.if_match = second.etag.clone();
        request.body["build"]["source"] = json!({"kind":"git_ref","ref":"refs/heads/stable"});
        request.body["build"]["data_branch"] = json!("output");
        request.key = Some(RequestId::from_bytes([13; 16]));
        let third = save(&mut owner, &request, &rt, &"d".repeat(64)).unwrap();
        assert_eq!(
            fs::read_to_string(tree.0.join("src/example.py")).unwrap(),
            "VALUE=2\n"
        );
        let selected = tf_catalog::git::capture_ref(
            &Workspace::load(&tree.0, Some(&tree.0)).unwrap(),
            "refs/heads/stable",
            Some(&"output".parse().unwrap()),
            crate::preparation::limits(),
        )
        .unwrap();
        assert_eq!(
            selected.read(Path::new("src/example.py"), 100).unwrap(),
            b"VALUE=1\n"
        );
        request.method = "GET".into();
        request.query.clear();
        request.query.insert("branch".into(), "other".into());
        assert!(matches!(read(&tree.0,workspace,&request),Err(e) if e.status==400));
        request.query.clear();
        let current = read(&tree.0, workspace, &request).unwrap();
        assert_eq!(current.data, third.data);
        assert_eq!(
            current.data["definition"]["build"]["source"]["ref"],
            "refs/heads/stable"
        );
        assert_eq!(current.data["definition"]["build"]["data_branch"], "output");
        let mut action = Request {
            method: "POST".into(),
            path: format!("/api/v1/schedules/{id}/run"),
            body: json!({}),
            query: BTreeMap::new(),
            key: Some(RequestId::from_bytes([80; 16])),
            if_match: third.etag.clone(),
            ..request.clone()
        };
        let manual = save(&mut owner, &action, &rt, &"0".repeat(64)).unwrap();
        tf_protocol::validate_document("ScheduleRunV1", &manual.data).unwrap();
        assert_eq!(
            save(&mut owner, &action, &rt, &"0".repeat(64))
                .unwrap()
                .data,
            manual.data
        );
        action.key = Some(RequestId::from_bytes([81; 16]));
        action.path = format!("/api/v1/schedules/{id}/pause");
        let paused = save(&mut owner, &action, &rt, &"1".repeat(64)).unwrap();
        assert_eq!(paused.data["paused"], true);
        action.key = Some(RequestId::from_bytes([82; 16]));
        action.path = format!("/api/v1/schedules/{id}/resume");
        action.if_match = paused.etag.clone();
        let resumed = save(&mut owner, &action, &rt, &"2".repeat(64)).unwrap();
        assert_eq!(resumed.data["paused"], false);
        action.key = Some(RequestId::from_bytes([83; 16]));
        action.path = format!("/api/v1/schedules/{id}/pause");
        assert!(matches!(save(&mut owner,&action,&rt,&"3".repeat(64)),Err(e) if e.status==409));
        action.if_match = resumed.etag;
        action.body = json!({"extra":true});
        assert!(matches!(save(&mut owner,&action,&rt,&"3".repeat(64)),Err(e) if e.status==400));
        assert!(
            matches!(save(&mut owner,&Request{method:"PUT".into(),body:{let mut bad=request.body.clone();bad["build"]["source"]=json!({"kind":"git_ref","ref":"HEAD"});bad},key:Some(RequestId::from_bytes([14;16])),if_match:third.etag,..request},&rt,&"e".repeat(64)),Err(e) if e.status==400)
        );
    }
}
