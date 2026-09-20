//! Pure evaluation over one certified graph and caller-frozen head/semantic snapshot.
use crate::{refresh, scope::Graph};
use std::collections::{BTreeMap, BTreeSet};
use tf_catalog::{candidate::CandidateIdentity as Id, compute::Evidence};
use tf_domain::VersionId;

/// Unavailable facts must never be rendered as current or zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Freshness {
    /// All required facts agree.
    Current,
    /// At least one known semantic change.
    Stale,
    /// Evidence is absent or incompatible.
    Unknown,
}
/// Byte availability is independent of computation and check history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Materialization {
    /// No head on the selected branch.
    NeverBuilt,
    /// Exact artifact bytes verified.
    Available,
    /// Selected artifact is absent.
    Missing,
    /// Selected artifact fails integrity checks.
    Corrupt,
    /// Byte availability has not been established.
    Unknown,
}
/// Latest actual execution attempt; cache adoption creates no new attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptState {
    /// Attempt is in an active phase.
    Running,
    /// Latest attempt completed successfully.
    Succeeded,
    /// Latest attempt failed.
    Failed,
    /// Latest attempt was canceled.
    Canceled,
    /// Latest attempt was interrupted.
    Interrupted,
}
/// Provider output health; consumer input checks are a different fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Quality {
    /// All required output checks passed.
    Passed,
    /// Only nonblocking violations occurred.
    Warning,
    /// The retained contract explicitly has no output checks.
    NoChecks,
    /// Complete original evidence is unavailable.
    Unavailable,
}
/// Contextual consumer health does not overwrite provider output quality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputQuality {
    /// Exact consumer checks passed.
    Passed,
    /// A WARN check found a violation.
    Warning,
    /// A required check failed or evaluator errored.
    Failed,
    /// Results are missing, incomplete or skipped.
    Unavailable,
}
/// Facts frozen before evaluating any ancestor. No callbacks or database handles are accepted.
#[derive(Clone, Debug)]
pub struct Facts {
    /// Retained version identity, if one exists on the selected branch.
    pub version: Option<VersionId>,
    /// Strict artifact verification result, or Unknown when not verified.
    pub materialization: Materialization,
    /// Comparison evidence for the last-good version, never the latest failed attempt.
    pub retained: Option<Evidence>,
    /// Current semantics with exact aliases from this same head snapshot.
    pub current: Option<Evidence>,
    /// Original publication time; cached adoption does not reset TTL.
    pub published_us: Option<i64>,
    /// Latest actual attempt, independent of the head.
    pub latest_attempt: Option<AttemptState>,
    /// Original output certificate health.
    pub output_quality: Quality,
    /// Per-consumer latest-attempt input health, including failed checks.
    pub input_quality: BTreeMap<String, InputQuality>,
    /// Aliases supplied by a different source/branch or exact historical pin.
    /// Do not attribute selected-branch ancestor logic to those inputs.
    pub boundaries: BTreeSet<String>,
    /// Effective per-alias policy text frozen by the shared resolver.
    pub policies: BTreeMap<String, String>,
    /// Availability of selected input artifacts; bad existing heads never trigger fallback.
    pub input_artifacts: BTreeMap<String, Materialization>,
    /// Starting and resolved branch names for actual fallback bindings.
    pub fallbacks: BTreeMap<String, (String, String)>,
}
/// One explanation, with the same facts used by the machine decision and human renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reason {
    /// Stable code, including explicit unknown evidence.
    pub code: &'static str,
    /// Root-to-consumer path. Diamond paths retain a deterministic shortest representative.
    pub path: Vec<Id>,
    /// Alias per edge in path, preserving repeated inputs.
    pub aliases: Vec<String>,
    /// Changed alias, logical dimension or file.
    pub subject: String,
    /// Retained value; missing is meaningful and never a zero sentinel.
    pub before: Option<String>,
    /// Current value.
    pub after: Option<String>,
    /// Effective policy relevant to the cause.
    pub policy: Option<String>,
}
impl Reason {
    /// Format this reason directly; presentation adds no freshness inference.
    pub fn human(&self, names: &BTreeMap<Id, String>) -> String {
        let description = match self.code {
            "NEVER_BUILT" => "No published version exists",
            "INPUT_VERSION_CHANGED" => "An input binding changed",
            "CODE_CHANGED" => "Captured code or declaration changed",
            "ENVIRONMENT_CHANGED" => "The execution environment changed",
            "PARAMETERS_CHANGED" => "Parameter values changed",
            "CHECK_DEFINITION_CHANGED" => "The check contract changed",
            "EXECUTION_POLICY_CHANGED" => "Writer, clock or secret version policy changed",
            "SOURCE_REFRESH_DUE" => "Source refresh is due",
            "ARTIFACT_MISSING" => "The retained artifact is missing",
            "ARTIFACT_CORRUPT" => "The retained artifact is corrupt",
            "ANCESTOR_STALE" => "An ancestor needs updating",
            "BOUNDARY_FALLBACK" => "An input uses the resolved fallback branch",
            "CATALOG_ADDITION_PENDING" => "The output needs catalogue registration",
            "SOURCE_DEFINITION_UNAVAILABLE" => "The selected source has no executable definition",
            "BOUNDARY_STALE" => "A selected input artifact is unavailable",
            "CACHE_DISABLED" => "The declaration does not certify deterministic reuse",
            "CACHE_MATCH" => "Retained computation matches this context",
            _ => "Freshness evidence is unavailable",
        };
        let path = self
            .path
            .iter()
            .map(|id| names.get(id).cloned().unwrap_or_else(|| format!("{id:?}")))
            .collect::<Vec<_>>()
            .join(" -> ");
        format!(
            "{description}: {} [{}]; before={}; after={}{}",
            self.subject,
            path,
            self.before.as_deref().unwrap_or("unknown"),
            self.after.as_deref().unwrap_or("unknown"),
            self.policy
                .as_ref()
                .map(|p| format!("; policy={p}"))
                .unwrap_or_default()
        )
    }
}
/// Independent dimensions for one dataset; no overloaded status colour/string.
#[derive(Clone, Debug)]
pub struct Status {
    /// Exact last-good publication, independent of the latest attempt.
    pub version: Option<VersionId>,
    /// Original publication timestamp, absent when no head exists.
    pub published_us: Option<i64>,
    /// Independent physical availability.
    pub materialization: Materialization,
    /// Exact input changes or due source refresh.
    pub direct_data: Freshness,
    /// Code, environment, parameters and semantic policies.
    pub direct_logic: Freshness,
    /// Staleness or uncertainty in eligible ancestors.
    pub inherited: Freshness,
    /// Latest execution, independent of retained head.
    pub latest_attempt: Option<AttemptState>,
    /// Original provider output certificate.
    pub output_quality: Quality,
    /// Latest consumer checks by alias.
    pub input_quality: BTreeMap<String, InputQuality>,
    /// Shared machine and human explanation facts.
    pub reasons: Vec<Reason>,
}
fn reason(
    id: &Id,
    code: &'static str,
    subject: &str,
    before: Option<String>,
    after: Option<String>,
    policy: Option<String>,
) -> Reason {
    Reason {
        code,
        path: vec![id.clone()],
        aliases: vec![],
        subject: subject.into(),
        before,
        after,
        policy,
    }
}
fn valid(evidence: &Option<Evidence>) -> Option<&Evidence> {
    evidence.as_ref().filter(|e| e.valid())
}
/// Memoized, iterative parents-before-consumers evaluation. Invalid/missing contexts fail closed.
/// Uses one frozen clock, preserves separate unknown dimensions and never reads a moving head.
pub fn evaluate(
    graph: &Graph,
    facts: &BTreeMap<Id, Facts>,
    at_us: i64,
) -> Result<BTreeMap<Id, Status>, &'static str> {
    if at_us < 0
        || graph.order.len() > 100_000
        || graph.inputs.keys().any(|id| !facts.contains_key(id))
    {
        return Err("Freshness requires complete facts for the validated graph and a frozen clock");
    }
    let mut result: BTreeMap<Id, Status> = BTreeMap::new();
    let mut path_cells = 0usize;
    // One shortest path per root cause; no exponential enumeration of diamond paths.
    let mut causes: BTreeMap<Id, BTreeMap<(Id, String, String), Reason>> = BTreeMap::new();
    for id in &graph.order {
        let Some(f) = facts.get(id) else {
            continue;
        };
        if !graph.inputs.contains_key(id) {
            continue;
        }
        let mut reasons = vec![];
        let mut data = Freshness::Unknown;
        let mut logic = Freshness::Unknown;
        match f.materialization {
            Materialization::NeverBuilt => reasons.push(reason(
                id,
                "NEVER_BUILT",
                "materialization",
                None,
                None,
                None,
            )),
            Materialization::Missing => reasons.push(reason(
                id,
                "ARTIFACT_MISSING",
                "artifact",
                f.version.map(|v| v.to_string()),
                None,
                None,
            )),
            Materialization::Corrupt => reasons.push(reason(
                id,
                "ARTIFACT_CORRUPT",
                "artifact",
                f.version.map(|v| v.to_string()),
                None,
                None,
            )),
            Materialization::Unknown => reasons.push(reason(
                id,
                "METADATA_UNKNOWN",
                "artifact",
                f.version.map(|v| v.to_string()),
                None,
                None,
            )),
            Materialization::Available => (),
        }
        if let (Some(old), Some(new)) = (valid(&f.retained), valid(&f.current)) {
            data = Freshness::Current;
            logic = Freshness::Current;
            for alias in old
                .inputs
                .keys()
                .chain(new.inputs.keys())
                .collect::<BTreeSet<_>>()
            {
                if old.inputs.get(alias) != new.inputs.get(alias) {
                    data = Freshness::Stale;
                    reasons.push(reason(
                        id,
                        "INPUT_VERSION_CHANGED",
                        alias,
                        old.inputs.get(alias).cloned(),
                        new.inputs.get(alias).cloned(),
                        f.policies.get(alias).cloned(),
                    ));
                }
            }
            for (component, code) in [
                ("code", "CODE_CHANGED"),
                ("environment", "ENVIRONMENT_CHANGED"),
                ("parameters", "PARAMETERS_CHANGED"),
                ("checks", "CHECK_DEFINITION_CHANGED"),
                ("execution", "EXECUTION_POLICY_CHANGED"),
            ] {
                if old.components.get(component) != new.components.get(component) {
                    logic = Freshness::Stale;
                    reasons.push(reason(
                        id,
                        code,
                        component,
                        old.components.get(component).cloned(),
                        new.components.get(component).cloned(),
                        None,
                    ));
                }
            }
            for file in old
                .files
                .keys()
                .chain(new.files.keys())
                .collect::<BTreeSet<_>>()
            {
                if old.files.get(file) != new.files.get(file) {
                    logic = Freshness::Stale;
                    reasons.push(reason(
                        id,
                        "CODE_CHANGED",
                        file,
                        old.files.get(file).cloned(),
                        new.files.get(file).cloned(),
                        None,
                    ));
                }
            }
            // Incompatible evidence must not produce false currentness.
            if data == Freshness::Current
                && logic == Freshness::Current
                && old.compute != new.compute
            {
                logic = Freshness::Unknown;
                reasons.push(reason(
                    id,
                    "METADATA_UNKNOWN",
                    "unclassified computation change",
                    Some(old.compute.clone()),
                    Some(new.compute.clone()),
                    None,
                ));
            }
        } else {
            reasons.push(reason(
                id,
                "METADATA_UNKNOWN",
                "comparison evidence",
                f.retained.as_ref().map(|e| e.compute.clone()),
                f.current.as_ref().map(|e| e.compute.clone()),
                None,
            ));
        }
        if valid(&f.current).is_some_and(|e| !e.reusable) {
            if logic != Freshness::Stale {
                logic = Freshness::Unknown;
            }
            reasons.push(reason(
                id,
                "CACHE_DISABLED",
                "cache=never",
                None,
                None,
                None,
            ));
        }
        for (alias, availability) in &f.input_artifacts {
            if *availability != Materialization::Available {
                let unknown = *availability == Materialization::Unknown;
                if !unknown {
                    data = Freshness::Stale;
                } else if data != Freshness::Stale {
                    data = Freshness::Unknown;
                }
                reasons.push(reason(
                    id,
                    if unknown {
                        "METADATA_UNKNOWN"
                    } else {
                        "BOUNDARY_STALE"
                    },
                    alias,
                    None,
                    Some(format!("{availability:?}")),
                    f.policies.get(alias).cloned(),
                ));
            }
        }
        for (alias, (start, resolved)) in &f.fallbacks {
            reasons.push(reason(
                id,
                "BOUNDARY_FALLBACK",
                alias,
                Some(start.clone()),
                Some(resolved.clone()),
                f.policies.get(alias).cloned(),
            ));
        }
        if let Some(policy) = graph.sources().get(id) {
            let decision = refresh::decide(*policy, false, false, at_us, f.published_us)?;
            if decision.executes() {
                data = Freshness::Stale;
                reasons.push(reason(
                    id,
                    "SOURCE_REFRESH_DUE",
                    "source",
                    f.published_us.map(|t| t.to_string()),
                    Some(at_us.to_string()),
                    Some(format!("{policy:?}")),
                ));
            }
        }
        let mut roots = BTreeMap::new();
        for r in reasons.iter().filter(|r| r.code != "BOUNDARY_FALLBACK") {
            roots.insert(
                (id.clone(), r.code.to_string(), r.subject.clone()),
                r.clone(),
            );
        }
        let mut inherited = Freshness::Current;
        if let Some(edges) = graph.inputs.get(id) {
            for edge in edges {
                if edge.off_branch || f.boundaries.contains(&edge.alias) {
                    if inherited != Freshness::Stale {
                        inherited = Freshness::Unknown;
                    }
                    let r = reason(
                        id,
                        "METADATA_UNKNOWN",
                        &format!("boundary freshness: {}", edge.alias),
                        None,
                        None,
                        f.policies.get(&edge.alias).cloned(),
                    );
                    roots.insert((id.clone(), r.code.into(), r.subject.clone()), r.clone());
                    reasons.push(r);
                    continue;
                }
                if let Some(parent) = result.get(&edge.parent) {
                    let stale = parent.direct_data == Freshness::Stale
                        || parent.direct_logic == Freshness::Stale
                        || parent.inherited == Freshness::Stale
                        || matches!(
                            parent.materialization,
                            Materialization::NeverBuilt
                                | Materialization::Missing
                                | Materialization::Corrupt
                        );
                    let unknown = parent.direct_data == Freshness::Unknown
                        || parent.direct_logic == Freshness::Unknown
                        || parent.inherited == Freshness::Unknown
                        || parent.materialization == Materialization::Unknown;
                    if stale {
                        inherited = Freshness::Stale;
                    } else if unknown && inherited != Freshness::Stale {
                        inherited = Freshness::Unknown;
                    }
                    if (stale || unknown)
                        && let Some(parent_causes) = causes.get(&edge.parent)
                    {
                        for (key, cause) in parent_causes {
                            let mut next = cause.clone();
                            path_cells += next.path.len() + 1;
                            if path_cells > 1_000_000 {
                                return Err(
                                    "Freshness causal paths exceed the supported read-model bound",
                                );
                            }
                            next.path.push(id.clone());
                            next.aliases.push(edge.alias.clone());
                            let replace = roots.get(key).is_none_or(|old: &Reason| {
                                (next.path.len(), &next.path, &next.aliases)
                                    < (old.path.len(), &old.path, &old.aliases)
                            });
                            if replace {
                                roots.insert(key.clone(), next);
                            }
                        }
                    }
                } else if graph.inputs.contains_key(&edge.parent) {
                    return Err("Freshness graph is not in validated dependency order");
                }
            }
        }
        for cause in roots.values().filter(|r| r.path.len() > 1) {
            let mut inherited_reason = cause.clone();
            if !matches!(cause.code, "METADATA_UNKNOWN" | "CACHE_DISABLED") {
                inherited_reason.code = "ANCESTOR_STALE";
            }
            inherited_reason.subject = format!("{}: {}", cause.code, cause.subject);
            reasons.push(inherited_reason);
        }
        if reasons.is_empty()
            && f.materialization == Materialization::Available
            && inherited == Freshness::Current
        {
            reasons.push(reason(
                id,
                "CACHE_MATCH",
                "computation",
                f.retained.as_ref().map(|e| e.compute.clone()),
                f.current.as_ref().map(|e| e.compute.clone()),
                None,
            ));
        }
        causes.insert(id.clone(), roots);
        result.insert(
            id.clone(),
            Status {
                version: f.version,
                published_us: f.published_us,
                materialization: f.materialization,
                direct_data: data,
                direct_logic: logic,
                inherited,
                latest_attempt: f.latest_attempt,
                output_quality: f.output_quality,
                input_quality: f.input_quality.clone(),
                reasons,
            },
        );
    }
    Ok(result)
}
