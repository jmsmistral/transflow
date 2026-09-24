//! Captured input policies and alias-qualified identities; no catalogue or filesystem I/O.
use crate::{BranchName, BranchSelector, DatasetKey, FallbackPermission};
use std::collections::{BTreeMap, BTreeSet};

/// Malformed or excessively large input context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputError;
impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Input policy or alias is invalid or exceeds its supported bound")
    }
}
impl std::error::Error for InputError {}

/// Source of the selected fallback tail, before strict-input suppression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyOrigin {
    /// Explicit build/schedule tail, including an explicit empty tail.
    BuildOverride,
    /// Explicit foreign registration fallback tail.
    Registration,
    /// Authored rule for this input's starting branch.
    StartingBranch,
    /// Authored workspace default when no starting-branch rule exists.
    WorkspaceDefault,
}
/// Immutable authored policy, captured for a request rather than reread during execution.
#[derive(Clone, Debug)]
pub struct BranchPolicySnapshot {
    defaults: Vec<BranchName>,
    rules: BTreeMap<BranchName, Vec<BranchName>>,
}
fn bounded(names: &[BranchName]) -> bool {
    names.len() <= 1024 && names.iter().all(|n| n.as_str().len() <= 4096)
}
impl BranchPolicySnapshot {
    /// Authored default tail, independent of any request override.
    pub fn defaults(&self) -> &[BranchName] {
        &self.defaults
    }
    /// Explicit rules, including meaningful empty tails.
    pub fn rules(&self) -> &BTreeMap<BranchName, Vec<BranchName>> {
        &self.rules
    }

    /// Reject excessive policy input before allocating normalized candidate lists.
    pub fn new(
        defaults: Vec<BranchName>,
        rules: BTreeMap<BranchName, Vec<BranchName>>,
    ) -> Result<Self, InputError> {
        if !bounded(&defaults)
            || rules.len() > 1024
            || rules
                .iter()
                .any(|(n, v)| n.as_str().len() > 4096 || !bounded(v))
        {
            return Err(InputError);
        }
        Ok(Self { defaults, rules })
    }
    /// Provider policy: omitted uses registration; CURRENT uses the consumer branch.
    /// Consumer build overrides never enter this decision.
    pub fn external(
        &self,
        output: &BranchName,
        registered: &BranchName,
        declared: BranchSelector,
        permission: FallbackPermission,
        fallback: Option<&[BranchName]>,
    ) -> Result<InputPolicy, InputError> {
        let start = match &declared {
            BranchSelector::Omitted => registered.clone(),
            BranchSelector::Current => output.clone(),
            BranchSelector::Named(n) => n.clone(),
        };
        let mut policy = self.local(&start, BranchSelector::Current, permission, fallback)?;
        policy.output = output.clone();
        policy.declared = declared;
        if fallback.is_some() {
            policy.origin = PolicyOrigin::Registration;
        }
        Ok(policy)
    }
    /// Normalize a local binding. Named inputs use their own policy even when named like output.
    pub fn local(
        &self,
        output: &BranchName,
        declared: BranchSelector,
        permission: FallbackPermission,
        build_override: Option<&[BranchName]>,
    ) -> Result<InputPolicy, InputError> {
        let start = match &declared {
            BranchSelector::Named(n) => n.clone(),
            _ => output.clone(),
        };
        let (tail, origin) = if !matches!(declared, BranchSelector::Named(_))
            && let Some(tail) = build_override
        {
            (tail, PolicyOrigin::BuildOverride)
        } else if let Some(tail) = self.rules.get(&start) {
            (tail.as_slice(), PolicyOrigin::StartingBranch)
        } else {
            (self.defaults.as_slice(), PolicyOrigin::WorkspaceDefault)
        };
        if output.as_str().len() > 4096 || start.as_str().len() > 4096 || !bounded(tail) {
            return Err(InputError);
        }
        let mut seen = BTreeSet::from([start.clone()]);
        let mut candidates = vec![start.clone()];
        if permission == FallbackPermission::Allowed {
            for n in tail {
                if seen.insert(n.clone()) {
                    candidates.push(n.clone());
                }
            }
        }
        Ok(InputPolicy {
            output: output.clone(),
            declared,
            permission,
            start,
            origin,
            candidates,
        })
    }
}
/// Normalized local read context. An output override is not evidence of actual fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputPolicy {
    output: BranchName,
    declared: BranchSelector,
    permission: FallbackPermission,
    start: BranchName,
    origin: PolicyOrigin,
    candidates: Vec<BranchName>,
}
impl InputPolicy {
    /// Build output name, recorded independently of the input selection.
    pub fn output(&self) -> &BranchName {
        &self.output
    }
    /// Original selector, including omitted versus CURRENT.
    pub fn declared(&self) -> &BranchSelector {
        &self.declared
    }
    /// Strict input policy wins over every fallback tail.
    pub fn permission(&self) -> FallbackPermission {
        self.permission
    }
    /// Effective starting branch, before fallback.
    pub fn start(&self) -> &BranchName {
        &self.start
    }
    /// Why this tail was selected.
    pub fn origin(&self) -> PolicyOrigin {
        self.origin
    }
    /// Ordered, deduplicated, nonrecursive candidates including start.
    pub fn candidates(&self) -> &[BranchName] {
        &self.candidates
    }
}

/// A binding is identified by its consumer and alias, never only by its input dataset.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InputBindingKey {
    consumer: DatasetKey,
    alias: String,
}
impl InputBindingKey {
    /// Preserve alias spelling while rejecting empty/control-bearing/oversized identifiers.
    pub fn new(consumer: DatasetKey, alias: String) -> Result<Self, InputError> {
        if alias.is_empty() || alias.len() > 1024 || alias.chars().any(char::is_control) {
            return Err(InputError);
        }
        Ok(Self { consumer, alias })
    }
    /// Local output/consumer identity.
    pub fn consumer(&self) -> DatasetKey {
        self.consumer
    }
    /// Alias unique within the consumer, including validation-only dependencies.
    pub fn alias(&self) -> &str {
        &self.alias
    }
}
/// Role affects execution and fingerprints; both roles retain lineage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputRole {
    /// Function argument.
    Data,
    /// Validation dependency, not a function argument.
    Validation,
}
impl InputRole {
    /// Stable internal provenance spelling.
    pub fn name(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Validation => "validation",
        }
    }
}
/// Per-alias binding before exact versions or successful producer results are known.
#[derive(Clone, Debug)]
pub struct InputBinding {
    /// Consumer and alias; use this as the map key.
    pub key: InputBindingKey,
    /// Qualified origin, independent of local display paths.
    pub dataset: DatasetKey,
    /// Data or validation-only.
    pub role: InputRole,
    /// Captured selector and fallback policy.
    pub policy: InputPolicy,
}
/// Symbolic producer dependencies never fall back to an already available version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingDisposition {
    /// Bind to the planned producer's successful/cached result on the output branch.
    PlannedProducer,
    /// No eligible producer in this request; resolve a retained head.
    ReadBoundary,
    /// Independent named branch: this build may read but cannot rebuild it.
    OffBranch,
    /// Provider workspace is always a read boundary; it needs provider policy resolution.
    ForeignWorkspace,
}
impl InputBinding {
    /// Decide per binding without pruning declaration edges or mutating the write set.
    pub fn disposition(&self, writes: &BTreeSet<DatasetKey>) -> BindingDisposition {
        if self.dataset.workspace_id() != self.key.consumer().workspace_id() {
            BindingDisposition::ForeignWorkspace
        } else if self.policy.start() != self.policy.output() {
            BindingDisposition::OffBranch
        } else if writes.contains(&self.dataset) {
            BindingDisposition::PlannedProducer
        } else {
            BindingDisposition::ReadBoundary
        }
    }
}
