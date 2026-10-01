//! Bounded typed schedule definition codec. Source selection is explicit and independent of checkout.
use crate::{ProtocolError, validate_document};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{BranchName, DatasetKey, ScheduleId, SourceSnapshotId, WorkspaceId};

/// Globally qualified registered dataset reference.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    /// Owning workspace UUID.
    pub workspace_id: String,
    /// Dataset UUID within its owner.
    pub dataset_id: String,
}
impl Dataset {
    /// Decode the identity without resolving a display path.
    pub fn key(&self) -> Result<DatasetKey, ProtocolError> {
        DatasetKey::from_text(&self.workspace_id, &self.dataset_id).map_err(|_| invalid())
    }
}
/// Explicit source authority; a Git ref may advance but never follows ambient HEAD.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// Exact retained validated source capture.
    FixedSnapshot {
        /// Captured source identity.
        snapshot_id: String,
    },
    /// Saved full branch/tag ref or exact commit, resolved independently at dispatch.
    GitRef {
        /// Explicit ref; HEAD and relative revision expressions are rejected.
        #[serde(rename = "ref")]
        reference: String,
    },
    /// Deliberately capture mutable working files each occurrence.
    WorkingTree {
        /// Explicit permission for guarded additive registration.
        allow_additive_sync: bool,
    },
}
/// Frozen starting-branch policies for named bindings or a provider workspace.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FallbackPolicy {
    /// Default ordered tail for otherwise unmatched starting branches.
    pub default: Vec<String>,
    /// Exact starting-branch overrides, including explicit empty tails.
    pub rules: BTreeMap<String, Vec<String>>,
}
/// Immutable execution settings copied from the current definition at acceptance.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildTemplate {
    /// Explicit source selector.
    pub source: Source,
    /// Output data branch, independent of code branch.
    pub data_branch: String,
    /// Ordered tail for current-branch input reads.
    pub fallback_branches: Vec<String>,
    /// Frozen local policy for independently named input branches.
    pub input_fallback_policy: FallbackPolicy,
    /// Frozen policies keyed by registered provider workspace UUID.
    pub provider_fallback_policies: BTreeMap<String, FallbackPolicy>,
    /// Local registered outputs.
    pub targets: Vec<Dataset>,
    /// Scope selection mode.
    pub build_mode: BuildMode,
    /// Explicit read boundaries.
    pub boundaries: Vec<Dataset>,
    /// Prohibited producer identities.
    pub exclusions: Vec<Dataset>,
    /// Explicit eligible source refresh requests.
    pub refresh_sources: Vec<Dataset>,
    /// Lossless parameter overrides keyed by dataset UUID and parameter name.
    pub parameters: BTreeMap<String, Value>,
    /// Bypass reuse within the selected scope.
    pub force: bool,
    /// Require current boundary evidence.
    pub require_current: bool,
    /// Independent transform deadline; zero disables it.
    pub timeout_seconds: u32,
    /// Independent validation deadline; zero disables it.
    pub validation_timeout_seconds: u32,
}
/// Supported build scope.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildMode {
    /// Relevant ancestors.
    Full,
    /// Explicit targets only.
    Selected,
    /// Paths among selected endpoints with legacy between semantics.
    Between,
    /// All connecting producers.
    Connecting,
}
/// Event payload binding policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadMode {
    /// Bind the exact event version.
    Pin,
    /// Record causation without binding input data.
    SignalOnly,
}
/// Normalized trigger tree; evaluation remains a later scheduling task.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// Explicit manual-only schedule.
    Manual,
    /// Five-field cron; timezone/DST evaluation belongs to T093.
    Cron {
        /// Stable leaf name.
        id: String,
        /// Five-field expression.
        expression: String,
        /// Recorded IANA timezone.
        timezone: String,
        /// Earliest or both duplicate local times.
        duplicate_time: String,
    },
    /// Committed head-change event.
    DatasetHeadChanged {
        /// Stable leaf name.
        id: String,
        /// Exact origin identity.
        dataset: Dataset,
        /// Explicit event data branch.
        branch: String,
        /// Binding or signal-only.
        payload_mode: PayloadMode,
        /// Whether resets qualify.
        include_resets: bool,
    },
    /// New materialization, excluding cache adoption.
    DatasetPublished {
        /// Stable leaf name.
        id: String,
        /// Exact origin identity.
        dataset: Dataset,
        /// Explicit event data branch.
        branch: String,
        /// Binding or signal-only.
        payload_mode: PayloadMode,
        /// Must be false for publication leaves.
        include_resets: bool,
    },
    /// Another schedule's successful occurrence.
    ScheduleSucceeded {
        /// Stable leaf name.
        id: String,
        /// Stable upstream schedule UUID.
        schedule_id: String,
        /// Require new materialization.
        require_materialization: bool,
    },
    /// Successful build filtered by exact target identities and branch.
    BuildSucceeded {
        /// Stable leaf name.
        id: String,
        /// Required target identities.
        targets: Vec<Dataset>,
        /// Explicit data branch.
        branch: String,
    },
    /// All children contribute evidence.
    And {
        /// Bounded child nodes.
        children: Vec<Trigger>,
    },
    /// Deterministically choose eligible child evidence.
    Or {
        /// Bounded child nodes.
        children: Vec<Trigger>,
    },
}
/// Persisted operational policies; no scheduler is activated by saving them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policies {
    /// Maximum attempts per job.
    pub max_attempts: u32,
    /// Explicit retryable infrastructure classes.
    pub retryable_classes: Vec<String>,
    /// Stop dependent work after a required failure.
    pub abort_on_failure: bool,
    /// Coalesce, bounded queue or skip.
    pub overlap_policy: String,
    /// Bounded accepted pending occurrences.
    pub max_pending: u32,
    /// Permit concurrent occurrences subject to write reservations.
    pub allow_overlapping_builds: bool,
    /// Skip, coalesce latest or bounded catch-up.
    pub misfire_policy: String,
    /// Maximum missed ticks per pass.
    pub max_catch_up: u32,
    /// Token expiry window; None requires explicit acknowledgment.
    pub token_window_seconds: Option<u32>,
    /// Explicit acknowledgment of unlimited token lifetime.
    pub acknowledge_no_expiry: bool,
    /// Bounded drain of new external evidence.
    pub max_consecutive_builds: u32,
    /// Minimum delay between consecutive automatic occurrences.
    pub minimum_delay_seconds: u32,
}
/// One replaceable normalized definition, with no version history.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    /// Independent authored format version.
    pub format_version: u8,
    /// User-visible schedule name.
    pub name: String,
    /// Optional prose description.
    pub description: String,
    /// Typed frozen build template.
    pub build: BuildTemplate,
    /// Typed trigger AST.
    pub trigger: Trigger,
    /// Operational behavior when scheduling is implemented.
    pub policies: Policies,
}
fn invalid() -> ProtocolError {
    ProtocolError::InvalidDocument
}
fn branch(name: &str) -> Result<(), ProtocolError> {
    name.parse::<BranchName>()
        .map(|_| ())
        .map_err(|_| invalid())
}
fn tail(names: &[String], starting: Option<&str>) -> Result<(), ProtocolError> {
    let mut seen = BTreeSet::new();
    for name in names {
        branch(name)?;
        if Some(name.as_str()) == starting || !seen.insert(name) {
            return Err(invalid());
        }
    }
    Ok(())
}
fn policy(p: &FallbackPolicy) -> Result<(), ProtocolError> {
    if p.rules.len() > 100 {
        return Err(invalid());
    }
    tail(&p.default, None)?;
    for (name, names) in &p.rules {
        branch(name)?;
        tail(names, Some(name))?;
    }
    Ok(())
}
fn datasets(items: &[Dataset]) -> Result<BTreeSet<DatasetKey>, ProtocolError> {
    let mut keys = BTreeSet::new();
    for item in items {
        if !keys.insert(item.key()?) {
            return Err(invalid());
        }
    }
    Ok(keys)
}
impl Definition {
    /// Validate the closed shared contract and semantic bounds before any storage mutation.
    pub fn decode(value: &Value) -> Result<Self, ProtocolError> {
        if serde_json::to_vec(value).map_err(|_| invalid())?.len() > 262144 {
            return Err(invalid());
        }
        validate_document("ScheduleDefinitionV1", value)?;
        let result: Self = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
        branch(&result.build.data_branch)?;
        tail(
            &result.build.fallback_branches,
            Some(&result.build.data_branch),
        )?;
        policy(&result.build.input_fallback_policy)?;
        if result.build.provider_fallback_policies.len() > 100 || result.name.trim().is_empty() {
            return Err(invalid());
        }
        for (workspace, p) in &result.build.provider_fallback_policies {
            workspace.parse::<WorkspaceId>().map_err(|_| invalid())?;
            policy(p)?;
        }
        match &result.build.source {
            Source::FixedSnapshot { snapshot_id } => {
                snapshot_id
                    .parse::<SourceSnapshotId>()
                    .map_err(|_| invalid())?;
            }
            Source::GitRef { reference } => {
                let full =
                    reference.starts_with("refs/heads/") || reference.starts_with("refs/tags/");
                let commit = [40, 64].contains(&reference.len())
                    && reference.bytes().all(|b| b.is_ascii_hexdigit());
                if (!full && !commit)
                    || reference.ends_with('/')
                    || (full
                        && reference.split('/').any(|part| {
                            part.is_empty()
                                || part.starts_with('.')
                                || part.ends_with('.')
                                || part.ends_with(".lock")
                        }))
                    || reference.contains([' ', '~', '^', ':', '?', '*', '[', '\\'])
                    || reference.contains("..")
                    || reference.contains("@{")
                    || reference.chars().any(char::is_control)
                {
                    return Err(invalid());
                }
            }
            Source::WorkingTree { .. } => {}
        }
        let targets = datasets(&result.build.targets)?;
        let excluded = datasets(&result.build.exclusions)?;
        let boundaries = datasets(&result.build.boundaries)?;
        let refreshed = datasets(&result.build.refresh_sources)?;
        if !refreshed.is_disjoint(&excluded)
            || result
                .policies
                .retryable_classes
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != result.policies.retryable_classes.len()
        {
            return Err(invalid());
        }
        if !targets.is_disjoint(&excluded) || !targets.is_disjoint(&boundaries) {
            return Err(invalid());
        }
        if result.policies.token_window_seconds.is_none() && !result.policies.acknowledge_no_expiry
        {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        let mut bindings = BTreeMap::new();
        let mut leaves = 0;
        walk(
            &result.trigger,
            1,
            true,
            &mut ids,
            &mut bindings,
            &mut leaves,
        )?;
        Ok(result)
    }
}
fn walk<'a>(
    trigger: &'a Trigger,
    depth: usize,
    root: bool,
    ids: &mut BTreeSet<&'a str>,
    bindings: &mut BTreeMap<(DatasetKey, &'a str), PayloadMode>,
    leaves: &mut usize,
) -> Result<(), ProtocolError> {
    if depth > 8 {
        return Err(invalid());
    }
    let id = match trigger {
        Trigger::Manual => {
            if !root {
                return Err(invalid());
            }
            return Ok(());
        }
        Trigger::And { children } | Trigger::Or { children } => {
            for child in children {
                walk(child, depth + 1, false, ids, bindings, leaves)?;
            }
            return Ok(());
        }
        Trigger::Cron {
            id,
            expression,
            timezone,
            ..
        } => {
            if !cron_syntax(expression)
                || timezone.chars().any(char::is_control)
                || !(timezone == "UTC"
                    || (timezone.contains('/')
                        && timezone.split('/').all(|part| {
                            !part.is_empty()
                                && part != "."
                                && part != ".."
                                && part.bytes().all(|b| {
                                    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+')
                                })
                        })))
            {
                return Err(invalid());
            }
            id
        }
        Trigger::DatasetHeadChanged {
            id,
            dataset,
            branch: b,
            payload_mode,
            ..
        }
        | Trigger::DatasetPublished {
            id,
            dataset,
            branch: b,
            payload_mode,
            ..
        } => {
            branch(b)?;
            if matches!(
                trigger,
                Trigger::DatasetPublished {
                    include_resets: true,
                    ..
                }
            ) {
                return Err(invalid());
            }
            if let Some(old) = bindings.insert((dataset.key()?, b), *payload_mode)
                && old != *payload_mode
            {
                return Err(invalid());
            }
            id
        }
        Trigger::ScheduleSucceeded {
            id, schedule_id, ..
        } => {
            schedule_id.parse::<ScheduleId>().map_err(|_| invalid())?;
            id
        }
        Trigger::BuildSucceeded {
            id,
            targets,
            branch: b,
        } => {
            datasets(targets)?;
            branch(b)?;
            id
        }
    };
    *leaves += 1;
    if *leaves > 64 || !ids.insert(id) || id.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(())
}

// Definition syntax only. IANA lookup, tick generation and DST behavior belong to T093.
fn cron_syntax(expression: &str) -> bool {
    let fields: Vec<_> = expression.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    fields
        .iter()
        .zip([(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)])
        .all(|(field, (low, high))| {
            field.split(',').all(|part| {
                let (range, step) = part
                    .split_once('/')
                    .map_or((part, None), |(r, s)| (r, Some(s)));
                if step.is_some_and(|s| {
                    s.parse::<u32>()
                        .ok()
                        .is_none_or(|n| n == 0 || n > high - low + 1)
                }) {
                    return false;
                }
                if range == "*" {
                    return true;
                }
                let (a, b) = range
                    .split_once('-')
                    .map_or((range, range), |(a, b)| (a, b));
                match (a.parse::<u32>(), b.parse::<u32>()) {
                    (Ok(a), Ok(b)) => a >= low && b <= high && a <= b,
                    _ => false,
                }
            })
        })
}
