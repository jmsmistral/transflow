//! Presentation-only services; writes are serialized by the existing runtime owner.
use crate::api_read::{self, Result, bad};
use serde_json::Value;
use std::path::Path;
use tf_api::{ApiError as E, Reply, Request};
use tf_exec::ownership::RuntimeOwner;
pub(crate) fn read(root: &Path, r: &Request) -> Result<Reply> {
    if r.query.keys().any(|k| k != "after" && k != "search") {
        return Err(E::invalid());
    }
    let rt = api_read::runtime()?;
    let value = rt.block_on(async {
        let mut rd = api_read::reader(root).await?;
        let result = if r.path == "/api/v1/views" {
            let after = r.query.get("after").map(String::as_str).unwrap_or("");
            if !after.is_empty() {
                after
                    .parse::<tf_domain::RequestId>()
                    .map_err(|_| E::invalid())?;
            }
            let search = r.query.get("search").map(String::as_str).unwrap_or("");
            if search.chars().count() > 200 {
                return Err(E::invalid());
            }
            rd.views(after, search).await.map_err(bad)
        } else {
            if !r.query.is_empty() {
                return Err(E::invalid());
            }
            let id = r
                .path
                .strip_prefix("/api/v1/views/")
                .ok_or_else(E::invalid)?;
            id.parse::<tf_domain::RequestId>()
                .map_err(|_| E::invalid())?;
            rd.view(id).await.map_err(|_| E::missing())
        };
        rd.close().await.map_err(bad)?;
        result
    })?;
    Ok(Reply::metadata(value))
}
pub(crate) fn save(
    owner: &mut RuntimeOwner,
    r: &Request,
    rt: &tokio::runtime::Runtime,
) -> Result<Value> {
    tf_store::views::validate(&r.body).map_err(|_| E::invalid())?;
    let c = api_read::context(
        owner.workspace_root(),
        owner.workspace_id().map_err(bad)?,
        r,
    )?;
    let selector = &r.body["selector"];
    let fallback = c.value["fallback_policy"]
        .as_array()
        .ok_or_else(E::internal)?;
    if selector["branch"] != c.value["branch"]
        || selector["fallback"].as_array()
            != Some(&fallback.iter().skip(1).cloned().collect::<Vec<_>>())
    {
        return Err(E::conflict());
    }
    let entries = tf_catalog::browse::entries(&c.registry);
    for node in r.body["datasets"].as_array().ok_or_else(E::invalid)? {
        let identity = node["identity"].as_str().ok_or_else(E::invalid)?;
        if !entries.iter().any(|e| {
            format!(
                "dataset:{}:{}",
                e["workspace_id"].as_str().unwrap_or(""),
                e["dataset_id"].as_str().unwrap_or("")
            ) == identity
        }) {
            return Err(E::missing());
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(bad)?
        .as_micros();
    let now = i64::try_from(now).map_err(bad)?;
    rt.block_on(async {
        let mut store = owner.open_store().await.map_err(bad)?;
        let result = store
            .repository()
            .map_err(bad)?
            .save_view(&r.body, now)
            .await
            .map_err(bad)?
            .ok_or_else(E::conflict);
        store.close().await.map_err(bad)?;
        result
    })
}
