//! HTTP composition for retained execution history; calculations live below the transport.
use crate::api_read::{Context, Result, bad, cursor, keys, limit, reader, runtime};
use serde_json::{Value, json};
use std::path::Path;
use tf_api::{ApiError as E, Request};
use tf_domain::{AttemptId, DatasetId, WorkspaceId};
pub(crate) fn handles(path: &str) -> bool {
    path == "metrics"
        || path.starts_with("attempts/")
        || (path.starts_with("builds/") && path.ends_with("/timeline"))
        || (path.starts_with("datasets/") && path.ends_with("/history"))
}
fn current_only(r: &Request) -> Result<()> {
    // Cross-version history is a branch query. An exact-version/plan selection is never ignored.
    if ["plan", "version", "origin_workspace"]
        .iter()
        .any(|k| r.query.contains_key(*k))
    {
        return Err(E::invalid());
    }
    Ok(())
}
pub(crate) fn read(
    root: &Path,
    wid: WorkspaceId,
    r: &Request,
    c: &Context,
    path: &str,
) -> Result<Value> {
    let rt = runtime()?;
    let mut rd = rt.block_on(reader(root))?;
    let data = if path == "metrics" {
        keys(r, &["from_us", "to_us", "window", "materialized_any"])?;
        current_only(r)?;
        let from_us = r
            .query
            .get("from_us")
            .ok_or_else(E::invalid)?
            .parse()
            .map_err(|_| E::invalid())?;
        let to_us = r
            .query
            .get("to_us")
            .ok_or_else(E::invalid)?
            .parse()
            .map_err(|_| E::invalid())?;
        let window = r
            .query
            .get("window")
            .map(|s| s.parse::<usize>().map_err(|_| E::invalid()))
            .transpose()?
            .unwrap_or(10);
        let dataset = r
            .query
            .get("dataset")
            .map(|s| s.parse::<DatasetId>().map_err(|_| E::invalid()))
            .transpose()?;
        let materialized_any = match r.query.get("materialized_any").map(String::as_str) {
            None | Some("false") => false,
            Some("true") => true,
            _ => return Err(E::invalid()),
        };
        if from_us < 0 || to_us <= from_us || !(1..=1000).contains(&window) {
            return Err(E::invalid());
        }
        rt.block_on(rd.execution_metrics(tf_store::history::MetricsQuery {
            workspace: wid,
            branch: &c.branch,
            from_us,
            to_us,
            dataset,
            materialized_any,
            window,
        }))
        .map_err(|e| match e {
            tf_store::StoreError::InvalidRequest => E::new(
                413,
                "TF_API_HISTORY_LIMIT",
                "Narrow the execution history time range or dataset filter",
            ),
            _ => bad(e),
        })?
    } else if let Some(id) = path
        .strip_prefix("datasets/")
        .and_then(|s| s.strip_suffix("/history"))
    {
        keys(r, &["limit", "cursor"])?;
        current_only(r)?;
        if r.query.contains_key("dataset") {
            return Err(E::invalid());
        }
        let id: DatasetId = id.parse().map_err(|_| E::invalid())?;
        let n = limit(r, 50, 200)?;
        let (binding, after) = cursor(r, c)?;
        if !after.is_empty() {
            let _: tf_domain::JobId = after.parse().map_err(|_| E::invalid())?;
        }
        let mut entries = rt
            .block_on(rd.dataset_history(wid, &c.branch, id, &after, n as u32 + 1))
            .map_err(bad)?;
        let more = entries.len() > n;
        entries.truncate(n);
        let next = if more {
            entries
                .last()
                .and_then(|e| e["id"].as_str())
                .map(|s| format!("{binding}:{s}"))
        } else {
            None
        };
        json!({"entries":entries,"next_cursor":next})
    } else {
        let log_attempt = path
            .strip_prefix("attempts/")
            .and_then(|s| s.strip_suffix("/logs"));
        keys(
            r,
            if log_attempt.is_some() {
                &["helper", "stream", "offset", "limit"]
            } else {
                &[]
            },
        )?;
        let attempt = path
            .strip_prefix("attempts/")
            .map(|id| id.strip_suffix("/logs").unwrap_or(id))
            .map(|id| id.parse::<AttemptId>().map_err(|_| E::invalid()))
            .transpose()?;
        let build = if let Some(id) = attempt {
            rt.block_on(rd.attempt_build(wid, id))
                .map_err(|_| E::missing())?
        } else {
            path.strip_prefix("builds/")
                .and_then(|s| s.strip_suffix("/timeline"))
                .ok_or_else(E::invalid)?
                .parse()
                .map_err(|_| E::invalid())?
        };
        let report = rt.block_on(rd.build_report(wid, build)).map_err(bad)?;
        if report["plan"]["output"]["name"] != c.branch.as_str()
            || c.source.is_none_or(|s| report["source"] != s.to_string())
            || r.query
                .get("plan")
                .is_some_and(|p| report["plan"]["id"] != *p)
        {
            return Err(E::conflict());
        }
        if let Some(id) = attempt {
            let jobs = report["jobs"].as_array().ok_or_else(E::internal)?;
            let (job, a) = jobs
                .iter()
                .find_map(|j| {
                    j["attempts"]
                        .as_array()?
                        .iter()
                        .find(|a| a["id"] == id.to_string())
                        .map(|a| (j, a))
                })
                .ok_or_else(E::missing)?;
            if r.query.get("version").is_some_and(|v| job["version"] != *v)
                || r.query.get("dataset").is_some_and(|d| job["dataset"] != *d)
            {
                return Err(E::conflict());
            }
            if log_attempt.is_some() {
                rt.block_on(rd.close()).map_err(bad)?;
                return crate::api_logs::read(root, id, r);
            }
            json!({"attempt":id.to_string(),"build":build.to_string(),"job":job["id"],"dataset":job["dataset"],"plan":report["plan"]["id"],"source":report["source"],"source_availability":if tf_catalog::capture::SourceSnapshot::open(&root.join(".transflow/runtime/source-snapshots"),c.source.ok_or_else(E::missing)?).is_ok() {"retained"} else {"unavailable"},"parameters":report["plan"]["context"]["parameters"],"inputs":job["inputs"],"evidence":a,"duration_ns":tf_store::history::attempt_duration(a).map(|n|n.to_string()),"log_command":format!("transflow build logs {build}")})
        } else {
            if r.query.contains_key("version")
                || r.query.contains_key("dataset")
                || r.query.contains_key("origin_workspace")
            {
                return Err(E::invalid());
            }
            let mut timeline = tf_store::history::timeline(&report).map_err(bad)?;
            timeline["duration_estimate"] = rt
                .block_on(rd.build_duration_estimate(wid, build, 10))
                .map_err(bad)?;
            let paths: std::collections::BTreeMap<String, String> =
                tf_catalog::browse::entries(&c.registry)
                    .into_iter()
                    .filter(|e| e["origin"] == "local")
                    .filter_map(|e| {
                        Some((
                            e["dataset_id"].as_str()?.to_owned(),
                            e["path"].as_str()?.to_owned(),
                        ))
                    })
                    .collect();
            for job in timeline["jobs"].as_array_mut().ok_or_else(E::internal)? {
                if let Some(path) = job["dataset"].as_str().and_then(|id| paths.get(id)) {
                    job["path"] = json!(path);
                }
            }
            timeline
        }
    };
    rt.block_on(rd.close()).map_err(bad)?;
    Ok(data)
}
