//! Version-pinned projected reads. Cursor state contains positions/identity, never rows.
use crate::{
    api_read::{self, Result, bad},
    build_plan::failure,
    provider::Selection,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use tf_api::{ApiError as E, Reply, Request};
use tf_catalog::{Resolved, workspace::Workspace};
use tf_domain::{DatasetId, VersionId, WorkspaceId};
use tf_store::{
    artifacts::ArtifactStore,
    preview::{Options, Position},
};
#[derive(Default)]
pub(crate) struct Cursors(BTreeMap<String, (Instant, String, Position)>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionBody {
    dataset: String,
    version: String,
    origin_workspace: String,
    columns: Vec<String>,
    #[serde(default)]
    rows: Option<usize>,
    #[serde(default = "default_cell")]
    cell_bytes: usize,
    #[serde(default)]
    cursor: Option<String>,
}
fn default_cell() -> usize {
    4096
}
pub(crate) fn read(
    root: &Path,
    wid: WorkspaceId,
    r: &Request,
    cursors: &Mutex<Cursors>,
) -> Result<Reply> {
    api_read::keys(r, &[])?;
    let mut c = api_read::context(root, wid, r)?;
    let b: SelectionBody = serde_json::from_value(r.body.clone()).map_err(|_| E::invalid())?;
    let dataset: DatasetId = b.dataset.parse().map_err(|_| E::invalid())?;
    let origin: WorkspaceId = b.origin_workspace.parse().map_err(|_| E::invalid())?;
    let version: VersionId = b.version.parse().map_err(|_| E::invalid())?;
    if r.query.get("version").is_some_and(|v| v != &b.version)
        || r.query.get("dataset").is_some_and(|v| v != &b.dataset)
        || r.query
            .get("origin_workspace")
            .is_some_and(|v| v != &b.origin_workspace)
        || r.query.contains_key("plan")
    {
        return Err(E::conflict());
    }
    let entries = tf_catalog::browse::entries(&c.registry);
    let entry = entries
        .iter()
        .find(|e| e["dataset_id"] == b.dataset && e["workspace_id"] == b.origin_workspace)
        .ok_or_else(E::missing)?;
    let workspace = Workspace::load(root, Some(root)).map_err(bad)?;
    let (default_rows, max_rows, max_bytes) = workspace.config().interactive_limits();
    let rows = b
        .rows
        .unwrap_or(default_rows as usize)
        .min(max_rows as usize)
        .min(1000);
    let max_bytes = usize::try_from(max_bytes.min(2 * 1024 * 1024)).map_err(bad)?;
    let seconds = workspace.config().interactive_timeout_seconds();
    let timeout = (seconds != 0).then(|| Duration::from_secs(seconds));
    let (provider, registered, fallback) = if origin == wid {
        (root.to_path_buf(), c.branch.to_string(), None)
    } else {
        let Resolved::External(reg) = c
            .registry
            .resolve(entry["path"].as_str().ok_or_else(E::missing)?)
            .map_err(bad)?
        else {
            return Err(E::missing());
        };
        let path = workspace.local().provider(origin).ok_or_else(E::missing)?;
        (
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            },
            reg.default_branch().to_string(),
            reg.fallback_override()
                .map(|v| v.iter().map(ToString::to_string).collect()),
        )
    };
    let binding = api_read::digest(
        &json!({"workspace":wid.to_string(),"branch":c.branch.as_str(),"dataset":b.dataset,"origin":b.origin_workspace,"version":b.version,"columns":b.columns,"registry":c.value["registry"]}),
    )?;
    let position = if let Some(token) = &b.cursor {
        let cache = cursors.lock().map_err(|_| E::internal())?;
        let (at, key, p) = cache.0.get(token).ok_or_else(E::conflict)?;
        if at.elapsed() > Duration::from_secs(300) || key != &binding {
            return Err(E::conflict());
        }
        *p
    } else {
        Position::default()
    };
    let selection = Selection {
        consumer_workspace: wid.to_string(),
        consumer_dataset: dataset.to_string(),
        alias: "preview".into(),
        dataset: dataset.to_string(),
        output: c.branch.to_string(),
        registered,
        declared: json!({"kind":"omitted","name":null}),
        stop: true,
        role: "data".into(),
        fallback,
    };
    let (page, metadata) = crate::external_reads::preview(
        provider.clone(),
        origin,
        selection,
        version,
        r.id,
        |response, check| {
            let metadata = &response["metadata"];
            if metadata["workspace"] != origin.to_string()
                || metadata["dataset"] != dataset.to_string()
                || metadata["version"] != version.to_string()
            {
                return Err(failure("Preview origin mismatch"));
            }
            let digest = tf_protocol::canonical::ContentDigest::from_hex(
                tf_protocol::canonical::DigestKind::Artifact,
                metadata["artifact"]
                    .as_str()
                    .ok_or_else(|| failure("Missing artifact"))?,
            )
            .map_err(failure)?;
            let store = ArtifactStore::open(&provider).map_err(failure)?;
            let page = tf_store::preview::read(
                &store,
                digest,
                &metadata["manifest"],
                position,
                Options {
                    columns: &b.columns,
                    rows,
                    row_bytes: max_bytes.min(1536 * 1024),
                    timeout,
                    cell_bytes: b.cell_bytes,
                },
                check,
            )
            .map_err(failure)?;
            Ok((page, metadata.clone()))
        },
    )
    .map_err(|_| {
        E::new(
            422,
            "TF_API_PREVIEW",
            "The exact preview is unavailable, exceeds its read budget, or lost read protection",
        )
    })?;
    let next = if let Some(position) = page.next {
        let mut cache = cursors.lock().map_err(|_| E::internal())?;
        cache
            .0
            .retain(|_, v| v.0.elapsed() < Duration::from_secs(300));
        if cache.0.len() >= 256
            && let Some(old) = cache
                .0
                .iter()
                .min_by_key(|(_, v)| v.0)
                .map(|(k, _)| k.clone())
        {
            cache.0.remove(&old);
        }
        let token = tf_api::cursor_token()?;
        cache
            .0
            .insert(token.clone(), (Instant::now(), binding, position));
        Some(token)
    } else {
        None
    };
    c.value["selection"] = json!({"kind":if origin==wid{"local_version"}else{"foreign_version"},"id":b.version,"digest":api_read::digest(&metadata)?});
    c.value["source"] = if origin == wid {
        metadata["source"]["id"].clone()
    } else {
        Value::Null
    };
    c.value["graph"] = Value::Null;
    let mut identity = c.value.clone();
    identity
        .as_object_mut()
        .ok_or_else(E::internal)?
        .remove("fingerprint");
    c.fingerprint = api_read::digest(&identity)?;
    c.value["fingerprint"] = json!(c.fingerprint);
    let data = json!({"workspace":wid.to_string(),"origin_workspace":b.origin_workspace,"dataset":b.dataset,"version":b.version,"requested_branch":c.branch.as_str(),"resolved_branch":metadata["origin_branch"]["name"],"published_us":metadata["published_us"],"source":metadata["source"]["id"],"schema":page.schema,"rows":page.rows,"next_cursor":next,"physical_order":true,"integrity":"projected_read"});
    if serde_json::to_vec(&json!({"request_id":r.id.to_string(),"context":c.value,"data":data}))
        .map_err(bad)?
        .len()
        > max_bytes
    {
        return Err(E::new(
            413,
            "TF_API_PREVIEW_LIMIT",
            "Reduce the requested projection or cell budget",
        ));
    }
    Ok(c.reply(data))
}
