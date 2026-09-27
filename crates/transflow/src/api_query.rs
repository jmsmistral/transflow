//! Ephemeral, bounded query jobs. The shared coordinator admission gate owns worker capacity.
use crate::api_read::{self, Result, bad};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tf_api::{ApiError as E, Reply, Request};
use tf_domain::WorkspaceId;
use tf_exec::{admission::Admission, supervisor::Cancellation};
mod run;
const RETENTION: Duration = Duration::from_secs(300);
struct Job {
    key: String,
    digest: String,
    context: Value,
    branch: String,
    status: Value,
    cancel: Cancellation,
    finished: Option<Instant>,
    page: Option<tf_store::query_results::Page>,
    thread: Option<thread::JoinHandle<()>>,
}
pub(crate) struct Manager {
    root: PathBuf,
    workspace: WorkspaceId,
    pool: Admission,
    available: bool,
    jobs: Mutex<BTreeMap<String, Job>>,
}
impl Manager {
    pub(crate) fn new(root: PathBuf, workspace: WorkspaceId, pool: Admission) -> Arc<Self> {
        let started = Instant::now();
        let available =
            run::environment(&root, &|| started.elapsed() > Duration::from_secs(10)).is_ok();
        Arc::new(Self {
            root,
            workspace,
            pool,
            available,
            jobs: Mutex::new(BTreeMap::new()),
        })
    }
    pub(crate) fn available(&self) -> bool {
        self.available
    }
    pub(crate) fn active(&self) -> bool {
        self.jobs.lock().is_ok_and(|jobs| {
            jobs.values()
                .any(|j| j.thread.as_ref().is_some_and(|t| !t.is_finished()))
        })
    }
    pub(crate) fn cancel_all(&self) {
        if let Ok(jobs) = self.jobs.lock() {
            for j in jobs.values() {
                j.cancel.cancel();
            }
        }
    }
    pub(crate) fn tick(&self) {
        if let Ok(mut jobs) = self.jobs.lock() {
            for j in jobs.values_mut() {
                if j.thread.as_ref().is_some_and(|t| t.is_finished())
                    && let Some(t) = j.thread.take()
                {
                    let _ = t.join();
                }
                if j.finished.is_some_and(|t| t.elapsed() >= RETENTION) {
                    j.page = None;
                }
            }
        }
    }
    pub(crate) fn call(self: &Arc<Self>, r: &Request) -> Result<Reply> {
        if r.query
            .keys()
            .any(|k| !matches!(k.as_str(), "branch" | "context" | "offset" | "limit"))
        {
            return Err(E::invalid());
        }
        if r.path == "/api/v1/queries" && r.method == "POST" {
            return self.submit(r);
        }
        let tail = r
            .path
            .strip_prefix("/api/v1/queries/")
            .ok_or_else(E::missing)?;
        let (id, action) = tail.split_once('/').unwrap_or((tail, ""));
        let _: tf_domain::RequestId = id.parse().map_err(|_| E::invalid())?;
        let mut jobs = self.jobs.lock().map_err(bad)?;
        let j = jobs.get_mut(id).ok_or_else(E::missing)?;
        if r.query.get("branch") != Some(&j.branch)
            || r.query
                .get("context")
                .is_some_and(|v| Some(v.as_str()) != j.context["fingerprint"].as_str())
        {
            return Err(E::conflict());
        }
        if j.finished.is_some_and(|t| t.elapsed() >= RETENTION) {
            return Err(expired());
        }
        let data = match (r.method.as_str(), action) {
            ("GET", "") => status(j),
            ("POST", "cancel") => {
                if r.body != json!({}) || r.key.is_none() {
                    return Err(E::invalid());
                }
                if r.if_match.as_deref() != j.context["fingerprint"].as_str() {
                    return Err(E::conflict());
                }
                if j.finished.is_none() {
                    j.cancel.cancel();
                }
                status(j)
            }
            ("GET", "results") => {
                let p = j.page.as_ref().ok_or_else(|| {
                    E::new(
                        409,
                        "TF_API_QUERY_RESULTS",
                        "Query results are not available; inspect query status",
                    )
                })?;
                let offset = number(r, "offset", 0, 1000)?;
                let limit = number(r, "limit", 200, 1000)?;
                if limit == 0 || offset > p.rows.len() {
                    return Err(E::invalid());
                }
                let end = (offset + limit).min(p.rows.len());
                json!({"id":id,"schema":p.schema,"rows":p.rows[offset..end],"next_offset":(end < p.rows.len()).then_some(end),"truncated":j.status["truncated"]})
            }
            _ => return Err(E::missing()),
        };
        if action == "results"
            && serde_json::to_vec(
                &json!({"request_id":r.id.to_string(),"context":j.context,"data":data}),
            )
            .map_err(bad)?
            .len()
                > j.status["limits"]["bytes"]
                    .as_u64()
                    .ok_or_else(E::internal)? as usize
        {
            return Err(E::new(
                413,
                "TF_API_QUERY_LIMIT",
                "Reduce the result page size to fit the response byte budget",
            ));
        }
        Ok(reply(j, data))
    }
    fn submit(self: &Arc<Self>, r: &Request) -> Result<Reply> {
        if !self.available {
            return Err(E::new(
                422,
                "TF_API_QUERY_ENVIRONMENT",
                "Prepare the matched worker with qualified DuckDB and PyArrow, then restart serve",
            ));
        }
        if r.query.contains_key("offset") || r.query.contains_key("limit") {
            return Err(E::invalid());
        }
        let key = r.key.ok_or_else(E::invalid)?.to_string();
        let digest = api_read::digest(&json!([r.body, r.query, r.if_match]))?;
        let mut jobs = self.jobs.lock().map_err(bad)?;
        if let Some(j) = jobs.values().find(|j| j.key == key) {
            if j.digest != digest {
                return Err(E::conflict());
            }
            if j.finished.is_some_and(|t| t.elapsed() >= RETENTION) {
                return Err(expired());
            }
            return Ok(reply(j, status(j)));
        }
        if jobs.values().filter(|j| j.finished.is_none()).count() >= 4 {
            return Err(E::busy());
        }
        // Do not evict live receipts/results to admit another job. Expiry bounds retention.
        jobs.retain(|_, j| {
            !j.finished.is_some_and(|t| t.elapsed() >= RETENTION)
                || j.thread.as_ref().is_some_and(|t| !t.is_finished())
        });
        if jobs.len() >= 64 {
            return Err(E::busy());
        }
        let c = api_read::context(&self.root, self.workspace, r)?;
        if r.if_match.as_deref() != Some(&c.fingerprint) {
            return Err(E::conflict());
        }
        let input = run::prepare(&self.root, self.workspace, &c, &r.body, &self.pool)?;
        let id = tf_exec::discovery::random_id().map_err(bad)?;
        let cancel = Cancellation::default();
        let j = Job {
            key,
            digest,
            context: c.value.clone(),
            branch: c.branch.to_string(),
            cancel: cancel.clone(),
            finished: None,
            page: None,
            thread: None,
            status: json!({"id":id,"state":"QUEUED","bindings":r.body["bindings"],"sql":r.body["sql"],"parameters":r.body["parameters"],"elapsed_ms":"0","error":null,"truncated":false,"rows":"0","limits":input.limits,"timeout_seconds":input.resources.timers.interactive.value.to_string(),"expires_in_seconds":null}),
        };
        let response = reply(&j, status(&j));
        jobs.insert(id.clone(), j);
        let manager = self.clone();
        let job_id = id.clone();
        let thread = thread::Builder::new()
            .name("transflow-query".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run::execute(&manager, &job_id, input, &cancel)
                }));
                if let Ok(mut jobs) = manager.jobs.lock()
                    && let Some(j) = jobs.get_mut(&job_id)
                {
                    let (state, error, page, elapsed) = result.unwrap_or_else(|_| {
                        (
                            "FAILED",
                            Some("Query worker failed during cleanup".into()),
                            None,
                            0,
                        )
                    });
                    j.status["state"] = json!(if cancel.requested() && state == "SUCCEEDED" {
                        "CANCELED"
                    } else {
                        state
                    });
                    j.status["error"] = json!(error);
                    j.status["elapsed_ms"] = json!(elapsed.to_string());
                    if j.status["state"] == "SUCCEEDED"
                        && let Some(p) = page
                    {
                        j.status["truncated"] = json!(p.truncated);
                        j.status["rows"] = json!(p.rows.len().to_string());
                        j.page = Some(p);
                    }
                    j.finished = Some(Instant::now());
                }
            })
            .map_err(|_| {
                jobs.remove(&id);
                E::busy()
            })?;
        jobs.get_mut(&id).ok_or_else(E::internal)?.thread = Some(thread);
        Ok(response)
    }
}
fn expired() -> E {
    E::new(
        410,
        "TF_API_QUERY_EXPIRED",
        "Query expired; submit a new query with a new idempotency key",
    )
}
fn status(j: &Job) -> Value {
    let mut value = j.status.clone();
    value["expires_in_seconds"] = j.finished.map_or(Value::Null, |t| {
        json!(RETENTION.saturating_sub(t.elapsed()).as_secs())
    });
    value
}
fn reply(j: &Job, data: Value) -> Reply {
    Reply {
        data,
        context: Some(j.context.clone()),
        etag: j.context["fingerprint"].as_str().map(str::to_owned),
    }
}
fn number(r: &Request, key: &str, default: usize, max: usize) -> Result<usize> {
    let value = r
        .query
        .get(key)
        .map(|s| s.parse::<usize>().map_err(|_| E::invalid()))
        .transpose()?
        .unwrap_or(default);
    if value > max {
        Err(E::invalid())
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn retained_results_expire_without_sleep_and_cancel_respects_frozen_context() {
        let id = "11111111-1111-4111-8111-111111111111";
        let cancel = Cancellation::default();
        let job = Job {
            key: id.into(),
            digest: String::new(),
            context: json!({"fingerprint":"a".repeat(64)}),
            branch: "main".into(),
            status: json!({"state":"RUNNING"}),
            cancel: cancel.clone(),
            finished: None,
            page: None,
            thread: None,
        };
        let manager = Arc::new(Manager {
            root: PathBuf::new(),
            workspace: id.parse().unwrap(),
            pool: Admission::new(Default::default()).unwrap(),
            available: true,
            jobs: Mutex::new(BTreeMap::from([(id.into(), job)])),
        });
        let mut r = Request {
            id: id.parse().unwrap(),
            method: "POST".into(),
            path: format!("/api/v1/queries/{id}/cancel"),
            query: BTreeMap::from([("branch".into(), "main".into())]),
            body: json!({}),
            key: Some(id.parse().unwrap()),
            if_match: Some("b".repeat(64)),
        };
        assert!(manager.call(&r).is_err());
        assert!(!cancel.requested());
        r.if_match = Some("a".repeat(64));
        assert!(manager.call(&r).is_ok());
        assert!(cancel.requested());
        {
            let mut jobs = manager.jobs.lock().unwrap();
            let j = jobs.get_mut(id).unwrap();
            j.finished = Some(Instant::now() - RETENTION);
            j.page = Some(tf_store::query_results::Page {
                schema: json!({}),
                rows: vec![json!([1])],
                truncated: false,
            });
        }
        manager.tick();
        assert!(manager.jobs.lock().unwrap()[id].page.is_none());
        r.method = "GET".into();
        r.path = format!("/api/v1/queries/{id}/results");
        assert!(manager.call(&r).is_err());
        assert!(!manager.active());
    }
}
