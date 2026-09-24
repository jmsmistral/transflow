//! Exact registry browsing with revision-bound cursors; no ambient runtime access.
use crate::{RegistryError, RegistryErrorKind, RegistrySnapshot};
use serde_json::{Value, json};
use tf_domain::DatasetId;

fn invalid() -> RegistryError {
    crate::registry::error(RegistryErrorKind::Invalid, "/cursor")
}
/// One bounded page. Cursor binds the complete semantic registry, including tombstones.
pub fn page(
    registry: &RegistrySnapshot,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Value, RegistryError> {
    if !(1..=100).contains(&limit) {
        return Err(invalid());
    }
    let fp = registry.fingerprint().hex();
    let offset = if let Some(cursor) = cursor {
        let (revision, n) = cursor.split_once(':').ok_or_else(invalid)?;
        if revision != fp || n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        n.parse::<usize>().map_err(|_| invalid())?
    } else {
        0
    };
    let entries = entries(registry);
    if offset > entries.len() {
        return Err(invalid());
    }
    let end = (offset + limit).min(entries.len());
    Ok(
        json!({"entries":entries[offset..end],"next_cursor":(end<entries.len()).then(||format!("{fp}:{end}")),"total":entries.len().to_string()}),
    )
}
/// Detached registry entries; callers separately describe retained/runtime state.
pub fn entries(registry: &RegistrySnapshot) -> Vec<Value> {
    let mut entries:Vec<_>=registry.datasets().map(|d|json!({"workspace_id":d.key().workspace_id().to_string(),"dataset_id":d.key().dataset_id().to_string(),"path":d.path().as_str(),"kind":d.kind(),"origin":"local","tombstone":d.is_tombstone(),"alias_count":registry.aliases(d.key().dataset_id()).len().to_string(),"aliases":registry.aliases(d.key().dataset_id()).into_iter().take(8).collect::<Vec<_>>()})).chain(registry.external_history().map(|e|json!({"workspace_id":e.key().workspace_id().to_string(),"dataset_id":e.key().dataset_id().to_string(),"path":e.alias().as_str(),"kind":"external","origin":"external","tombstone":e.is_tombstone(),"aliases":[],"alias_count":"0"}))).collect();
    entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    entries
}
/// Include exact tombstoned identity/path inspection without resolving it as an active input.
pub fn show(registry: &RegistrySnapshot, reference: &str) -> Result<Value, RegistryError> {
    let all = entries(registry);
    if let Some(exact) = all.iter().find(|e| e["path"] == reference) {
        return Ok(exact.clone());
    }
    let key = registry.resolve(reference).ok().map(|r| r.key());
    let id = reference
        .strip_prefix("dataset:")
        .and_then(|s| s.parse::<DatasetId>().ok());
    all.into_iter()
        .find(|e| {
            e["path"] == reference
                || (e["origin"] == "local"
                    && id.is_some_and(|id| e["dataset_id"] == id.to_string()))
                || key.is_some_and(|key| {
                    e["dataset_id"] == key.dataset_id().to_string()
                        && e["workspace_id"] == key.workspace_id().to_string()
                })
        })
        .ok_or_else(|| registry.resolve(reference).err().unwrap_or_else(invalid))
}
