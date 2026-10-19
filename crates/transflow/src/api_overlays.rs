//! Presentation projection of retained evidence; never imports producers or verifies bytes.
use crate::api_read::{Context, Result, bad};
use serde_json::{Value, json};
use tf_catalog::candidate::CandidateIdentity as Id;
use tf_plan::freshness::{Freshness, InputQuality, Materialization};
use tf_store::freshness::Snapshot;

pub(crate) fn project(
    context: &Context,
    reader: &mut tf_store::Reader,
    runtime: &tokio::runtime::Runtime,
    id: &Id,
    external: bool,
    snapshot: Option<&Snapshot>,
    report: Option<&crate::why::Report>,
) -> Result<Value> {
    let status = report.and_then(|r| r.datasets.get(id));
    let mut value = json!({"version":null,"rows":null,"files":null,"bytes":null,"latest_attempt":status.and_then(|s|s.latest_attempt).map(|s|format!("{s:?}")),"freshness":"unknown","reasons":[],"health":"Unavailable","input_failed":status.is_some_and(|s|s.input_quality.values().any(|q|*q==InputQuality::Failed)),"durations":null});
    if external || snapshot.is_none() {
        return Ok(value);
    }
    let snapshot = snapshot.ok_or_else(tf_api::ApiError::internal)?;
    if let Some(status) = status {
        value["freshness"] = if status.materialization == Materialization::NeverBuilt {
            "never_built"
        } else if status.direct_data == Freshness::Stale {
            "data"
        } else if status.direct_logic == Freshness::Stale {
            "logic"
        } else if status.inherited == Freshness::Stale {
            "ancestor"
        } else if [status.direct_data, status.direct_logic, status.inherited]
            .iter()
            .all(|s| *s == Freshness::Current)
        {
            "current"
        } else {
            "unknown"
        }
        .into();
        value["reasons"] = json!(bounded_reasons(
            status
                .reasons
                .iter()
                .map(|r| format!("{}: {}", r.code, r.subject))
        ));
    }
    if report.is_none() {
        value["reasons"] = json!(["Freshness explanation unavailable: causal path safety limit"]);
    }
    if let Id::Registered(key) = id {
        if report.is_none() {
            let attempt = snapshot
                .attempts
                .get(&(key.dataset_id(), context.branch.clone()));
            value["input_failed"] = crate::why::input_quality(attempt)
                .values()
                .any(|quality| *quality == InputQuality::Failed)
                .into();
            value["latest_attempt"] = json!(
                snapshot
                    .attempts
                    .get(&(key.dataset_id(), context.branch.clone()))
                    .map(|attempt| format!("{:?}", crate::why::attempt_state(attempt)))
            );
        }
        let candidates: Vec<tf_domain::BranchName> = context.value["fallback_policy"]
            .as_array()
            .ok_or_else(tf_api::ApiError::internal)?
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(tf_api::ApiError::internal)?
                    .parse()
                    .map_err(bad)
            })
            .collect::<Result<_>>()?;
        let head = candidates
            .iter()
            .find_map(|b| snapshot.heads.get(&(key.dataset_id(), b.clone())));
        value["health"] = format!("{:?}", crate::why::output_quality(head)).into();
        if let Some(head) = head {
            value["version"] = head.version.to_string().into();
            // Off-branch publication does not establish selected-branch currentness.
            if status.is_some_and(|s| s.version.is_none()) {
                value["freshness"] = "unknown".into();
            }
            let metadata = runtime
                .block_on(reader.api_head(key.workspace_id(), key.dataset_id(), &candidates))
                .map_err(bad)?;
            if let Some(m) = metadata {
                value["rows"] = m["row_count"].clone();
                value["files"] = m["file_count"].clone();
                value["bytes"] = m["byte_count"].clone();
            }
        }
        value["durations"] = runtime
            .block_on(reader.overlay_durations(
                key.workspace_id(),
                &context.branch,
                key.dataset_id(),
            ))
            .map_err(bad)?;
    }
    Ok(value)
}

/// Only the bounded explanation budget is recoverable as unknown presentation.
pub(crate) fn bounded_report(
    report: std::result::Result<crate::why::Report, crate::build_plan::Error>,
) -> std::result::Result<Option<crate::why::Report>, crate::build_plan::Error> {
    match report {
        Err(error) if error.causal_path_limit() => Ok(None),
        other => other.map(Some),
    }
}
/// Bound tooltip payloads and disclose omitted evidence instead of silently truncating it.
fn bounded_reasons(reasons: impl Iterator<Item = String>) -> Vec<String> {
    let mut shown = Vec::new();
    let mut count = 0;
    for reason in reasons {
        count += 1;
        if shown.len() < 100 {
            shown.push(reason);
        }
    }
    if count > 100 {
        shown.truncate(99);
        shown.push(format!(
            "{} additional reasons omitted; inspect build plan details",
            count - 99
        ));
    }
    shown
}
#[cfg(test)]
mod tests {
    #[test]
    fn only_explanation_budget_is_recoverable_as_unknown() {
        assert!(matches!(
            super::bounded_report(Err(crate::build_plan::failure(
                tf_plan::freshness::CAUSAL_PATH_LIMIT
            ))),
            Ok(None)
        ));
        assert!(super::bounded_report(Err(crate::build_plan::failure("invalid source"))).is_err());
    }
    #[test]
    fn reason_limit_discloses_omitted_count() {
        let shown = super::bounded_reasons((0..102).map(|i| format!("reason {i}")));
        assert_eq!(shown.len(), 100);
        assert_eq!(
            shown[99],
            "3 additional reasons omitted; inspect build plan details"
        );
        assert_eq!(super::bounded_reasons(std::iter::empty()).len(), 0);
    }
}

/// Direct input publication recency, independent of code changes, checks and byte integrity.
/// Resolve named branches and fallbacks through the same bindings used by planning.
pub(crate) fn data_freshness(
    context: &Context,
    graph: &tf_catalog::validation::ValidatedGraph,
    request: &tf_catalog::validation::ValidationRequest<'_>,
    snapshot: &Snapshot,
) -> Result<std::collections::BTreeMap<Id, &'static str>> {
    let bindings = tf_catalog::input_bindings::local_read_bindings(
        graph,
        request,
        &context.branch,
        Some(&context.candidates[1..]),
    )
    .map_err(bad)?;
    let mut result = std::collections::BTreeMap::new();
    for definition in graph.candidate().definitions() {
        let Id::Registered(key) = definition.output else {
            continue;
        };
        if key.workspace_id() != snapshot.workspace {
            continue;
        }
        let head = context
            .candidates
            .iter()
            .find_map(|b| snapshot.heads.get(&(key.dataset_id(), b.clone())));
        let parents = definition.inputs.iter().map(|input| {
            let binding_key =
                tf_domain::input::InputBindingKey::new(key, input.alias.clone()).ok()?;
            let binding = bindings.get(&binding_key)?;
            binding.policy.candidates().iter().find_map(|b| {
                snapshot
                    .heads
                    .get(&(binding.dataset.dataset_id(), b.clone()))
                    .map(|h| h.published_us)
            })
        });
        result.insert(
            definition.output.clone(),
            publication_recency(head.map(|h| h.published_us), parents),
        );
    }
    Ok(result)
}
fn publication_recency(
    output: Option<i64>,
    inputs: impl Iterator<Item = Option<i64>>,
) -> &'static str {
    let Some(output) = output else {
        return "never_built";
    };
    let mut unknown = false;
    for input in inputs {
        match input {
            Some(t) if t > output => return "stale",
            None => unknown = true,
            _ => (),
        }
    }
    if unknown { "unknown" } else { "current" }
}
#[cfg(test)]
mod recency_tests {
    use super::publication_recency;
    #[test]
    fn recency_uses_successful_publications_not_attempts_or_code() {
        assert_eq!(
            publication_recency(Some(10), [Some(9), Some(10)].into_iter()),
            "current"
        );
        assert_eq!(
            publication_recency(Some(10), [Some(11)].into_iter()),
            "stale"
        );
        assert_eq!(
            publication_recency(Some(10), [None, Some(11)].into_iter()),
            "stale"
        );
        assert_eq!(publication_recency(Some(10), [None].into_iter()), "unknown");
        assert_eq!(
            publication_recency(None, [Some(11)].into_iter()),
            "never_built"
        );
        assert_eq!(publication_recency(Some(10), std::iter::empty()), "current");
    }
}
