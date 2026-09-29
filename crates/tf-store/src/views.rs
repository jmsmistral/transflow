//! Saved presentation documents. A compare-and-swap and its audit fact commit together.
use crate::{Reader, Result, Store, StoreError};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
use std::collections::BTreeSet;
/// Validate cross-field invariants in addition to the shared closed schema.
pub fn validate(view: &Value) -> Result<()> {
    tf_protocol::validate_document("GraphViewV1", view).map_err(|_| StoreError::InvalidRequest)?;
    let invalid = || StoreError::InvalidRequest;
    if view["description"]
        .as_str()
        .is_some_and(|s| s.contains(['\n', '\r']))
    {
        return Err(invalid());
    }
    let selector = &view["selector"];
    selector["branch"]
        .as_str()
        .ok_or_else(invalid)?
        .parse::<tf_domain::BranchName>()
        .map_err(|_| invalid())?;
    let mut fallbacks = BTreeSet::new();
    for name in selector["fallback"].as_array().ok_or_else(invalid)? {
        name.as_str()
            .ok_or_else(invalid)?
            .parse::<tf_domain::BranchName>()
            .map_err(|_| invalid())?;
        if !fallbacks.insert(name.as_str()) || name == &selector["branch"] {
            return Err(invalid());
        }
    }
    let mut ids = BTreeSet::new();
    for node in view["datasets"].as_array().ok_or_else(invalid)? {
        let id = node["identity"].as_str().ok_or_else(invalid)?;
        let Some((workspace, dataset)) =
            id.strip_prefix("dataset:").and_then(|s| s.split_once(':'))
        else {
            return Err(invalid());
        };
        workspace
            .parse::<tf_domain::WorkspaceId>()
            .map_err(|_| invalid())?;
        dataset
            .parse::<tf_domain::DatasetId>()
            .map_err(|_| invalid())?;
        if !ids.insert(id) {
            return Err(invalid());
        }
    }
    Ok(())
}
impl Store {
    /// Returns None on revision conflict. No last-write-wins and no build/head changes.
    pub async fn save_view(&mut self, view: &Value, at_us: i64) -> Result<Option<Value>> {
        validate(view)?;
        let revision = view["revision"]
            .as_i64()
            .ok_or(StoreError::InvalidRequest)?;
        if revision >= 2147483647 {
            return Err(StoreError::InvalidRequest);
        }
        let id = view["id"].as_str().ok_or(StoreError::InvalidRequest)?;
        let mut saved = view.clone();
        saved["revision"] = json!(revision + 1);
        let encoded = serde_json::to_string(&saved).map_err(|_| StoreError::InvalidRequest)?;
        if encoded.len() > 1024 * 1024 {
            return Err(StoreError::InvalidRequest);
        }
        let context =
            serde_json::to_string(&view["selector"]).map_err(|_| StoreError::InvalidRequest)?;
        let mut tx = self.db.begin().await?;
        let changed = if revision == 0 {
            sqlx::query("INSERT INTO graph_views VALUES(?,1,1,?,?,?) ON CONFLICT(id) DO NOTHING")
                .bind(id)
                .bind(encoded)
                .bind(context)
                .bind(at_us)
                .execute(&mut *tx)
                .await?
                .rows_affected()
        } else {
            sqlx::query("UPDATE graph_views SET revision=revision+1,view_json=?,context_json=?,saved_at_us=? WHERE id=? AND revision=?").bind(encoded).bind(context).bind(at_us).bind(id).bind(revision).execute(&mut *tx).await?.rows_affected()
        };
        if changed == 0 {
            tx.rollback().await?;
            return Ok(None);
        }
        sqlx::query("INSERT INTO audit_log(operation,evidence_json,wall_time_us) VALUES('graph_view_saved',?,?)").bind(json!({"view":id,"revision":revision+1}).to_string()).bind(at_us).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some(saved))
    }
}
impl Reader {
    /// Page summaries only; never silently truncate saved-view lists.
    pub async fn views(&mut self, after: &str) -> Result<Value> {
        let rows = sqlx::query("SELECT id,revision,json_extract(view_json,'$.name') FROM graph_views WHERE id>? ORDER BY id LIMIT 101").bind(after).fetch_all(&mut self.db).await?;
        let more = rows.len() > 100;
        let views = rows.iter().take(100).map(|r| Ok(json!({"id":r.try_get::<String,_>(0)?,"revision":r.try_get::<i64,_>(1)?,"name":r.try_get::<String,_>(2)?}))).collect::<Result<Vec<_>>>()?;
        Ok(
            json!({"next_cursor":if more {views.last().map(|v|v["id"].clone())}else{None},"views":views}),
        )
    }
    /// Read the current presentation document, omitting retired pre-release fields.
    pub async fn view(&mut self, id: &str) -> Result<Value> {
        let text: String = sqlx::query_scalar(
            "SELECT view_json FROM graph_views WHERE id=? AND length(view_json)<=1048576",
        )
        .bind(id)
        .fetch_one(&mut self.db)
        .await?;
        let mut value: Value =
            serde_json::from_str(&text).map_err(|_| StoreError::InvalidRequest)?;
        // Compatibility is read-only: old bytes and revision remain intact until an explicit save.
        // Unknown fields still fail the closed schema; only retired presentation fields are dropped.
        if let Some(object) = value.as_object_mut() {
            for key in ["groups", "annotations", "filters"] {
                object.remove(key);
            }
        }
        if let Some(selector) = value.get_mut("selector").and_then(Value::as_object_mut) {
            for key in ["mode", "source", "graph"] {
                selector.remove(key);
            }
        }
        if let Some(description) = value["description"].as_str()
            && description.contains(['\n', '\r'])
        {
            value["description"] =
                json!(description.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        validate(&value)?;
        Ok(value)
    }
}
