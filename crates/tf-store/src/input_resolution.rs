//! Choose the first existing head and pin it atomically. Integrity/check failures never retry a tail.
use crate::{
    Store, StoreError,
    publication::InputProvenance,
    retention::{self, LeaseKind, ReadLease, ReadTarget, RetentionError},
};
use serde_json::{Value, json};
use sqlx::{Connection, Row};
use std::collections::BTreeSet;
use tf_domain::{
    BranchId, BranchName, BranchSelector, RequestId, VersionId,
    input::{BindingDisposition, InputBinding, PolicyOrigin},
};

/// A missing candidate can allow fallback; every other error terminates resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbsenceReason {
    /// No branch row exists.
    BranchAbsent,
    /// Explicitly deleted data branch; it has no readable head.
    BranchDeleted,
    /// Branch exists but this dataset has no committed head.
    HeadAbsent,
}
impl AbsenceReason {
    fn name(self) -> &'static str {
        match self {
            Self::BranchAbsent => "branch_absent",
            Self::BranchDeleted => "branch_deleted",
            Self::HeadAbsent => "head_absent",
        }
    }
}
/// An attempted candidate whose absence justified proceeding.
#[derive(Clone, Debug)]
pub struct MissingCandidate {
    /// Exact candidate name.
    pub branch: BranchName,
    /// Why no head was read.
    pub reason: AbsenceReason,
}
/// Operational errors are not disguised as missing-head fallback.
#[derive(Debug, thiserror::Error)]
pub enum ResolutionError {
    /// Database failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Collection, clock, or lease failure at a selected head.
    #[error(transparent)]
    Retention(#[from] RetentionError),
    /// Wrong owner, corrupt metadata or invalid resolution request.
    #[error("Input binding cannot be resolved in this workspace or has inconsistent metadata")]
    Context,
    /// The normalized policy exhausted its candidates.
    #[error("No published input head exists in the selected branches")]
    Missing(Vec<MissingCandidate>),
    /// A producer selected for this binding must run, even if a fallback is available.
    #[error("This input must bind to its planned producer result")]
    PlannedProducer,
}
impl From<sqlx::Error> for ResolutionError {
    fn from(e: sqlx::Error) -> Self {
        Self::Store(e.into())
    }
}
/// Exact retained read. This metadata still requires strict byte verification and input checks.
#[derive(Clone, Debug)]
pub struct ResolvedRead {
    binding: InputBinding,
    branch_id: BranchId,
    resolved: BranchName,
    version: VersionId,
    missing: Vec<MissingCandidate>,
    lease: ReadLease,
    exact: bool,
}
impl ResolvedRead {
    /// True for a historical override; no branch-head lookup or fallback occurred.
    pub fn is_exact_pin(&self) -> bool {
        self.exact
    }
    /// Original independent alias and normalized policy.
    pub fn binding(&self) -> &InputBinding {
        &self.binding
    }
    /// Stable selected branch identity.
    pub fn branch_id(&self) -> BranchId {
        self.branch_id
    }
    /// Actual branch used, not merely a declared override.
    pub fn resolved_branch(&self) -> &BranchName {
        &self.resolved
    }
    /// Exact immutable publication, pinned before any byte access.
    pub fn version(&self) -> VersionId {
        self.version
    }
    /// Only meaningful for a non-exact read. Zero means the starting branch; a named override alone does not make fallback nonzero.
    pub fn fallback_index(&self) -> usize {
        self.missing.len()
    }
    /// Absence explanations in traversal order.
    pub fn missing_candidates(&self) -> &[MissingCandidate] {
        &self.missing
    }
    /// Renewable lease retained until the reader finishes or cancels.
    pub fn lease(&self) -> &ReadLease {
        &self.lease
    }
    /// Replace the ticket after a successful fenced renewal of this same read.
    pub fn renewed(&mut self, lease: ReadLease) -> Result<(), ResolutionError> {
        if !self.lease.same_binding(&lease) {
            return Err(ResolutionError::Context);
        }
        self.lease = lease;
        Ok(())
    }
    /// Typed resolution rendered for immutable version lineage.
    pub fn provenance(&self) -> InputProvenance {
        let policy = &self.binding.policy;
        let origin = match policy.origin() {
            PolicyOrigin::Registration => "registration",
            PolicyOrigin::BuildOverride => "build_override",
            PolicyOrigin::StartingBranch => "starting_branch",
            PolicyOrigin::WorkspaceDefault => "workspace_default",
        };
        InputProvenance {
            alias: self.binding.key.alias().to_owned(),
            dataset: self.binding.dataset,
            version: self.version,
            artifact: self.lease.artifact(),
            declared_branch: selector(policy.declared()),
            starting_branch: policy.start().clone(),
            resolved_branch: self.resolved.clone(),
            role: self.binding.role,
            resolution: json!({
                "kind": if self.exact { "exact_pin" } else { "branch_head" },
                "selector_override": self.exact,
                "output_branch": policy.output().as_str(),
                "policy_origin": origin,
                "stop_branch_fallback": policy.permission() == tf_domain::FallbackPermission::Prohibited,
                "candidates": policy.candidates().iter().map(BranchName::as_str).collect::<Vec<_>>(),
                "attempted": if self.exact { vec![] } else { self.missing.iter().map(|m| json!({
                    "branch": m.branch.as_str(), "absence": m.reason.name()
                })).chain(std::iter::once(json!({
                    "branch": self.resolved.as_str(), "version": self.version.to_string()
                }))).collect::<Vec<_>>() },
                "fallback_index": if self.exact { Value::Null } else { json!(self.fallback_index()) },
                "branch_id": self.branch_id.to_string()
            }),
        }
    }
    /// Semantic cache-input material includes every alias, role, origin and exact version.
    /// Full compute fingerprints still include source/environment/declarations in the planner.
    pub fn semantic_value(&self) -> Value {
        let p = self.provenance();
        json!({
            "consumer_workspace": self.binding.key.consumer().workspace_id().to_string(),
            "consumer_dataset": self.binding.key.consumer().dataset_id().to_string(),
            "alias": p.alias,
            "origin_workspace": p.dataset.workspace_id().to_string(),
            "origin_dataset": p.dataset.dataset_id().to_string(),
            "role": p.role.name(),
            "declared": p.declared_branch,
            "starting_branch": p.starting_branch.as_str(),
            "resolved_branch": p.resolved_branch.as_str(),
            "version": p.version.to_string(),
            "artifact": p.artifact.hex()
        })
    }
}
fn selector(s: &BranchSelector) -> Value {
    match s {
        BranchSelector::Omitted => json!({"kind":"omitted","name":null}),
        BranchSelector::Current => json!({"kind":"current","name":null}),
        BranchSelector::Named(n) => json!({"kind":"named","name":n.as_str()}),
    }
}
/// Lease allocation supplied by the owner; samples and IDs are deterministic in tests.
pub struct ReadRequest {
    /// Unique lease identity.
    pub lease: RequestId,
    /// Owning operation.
    pub operation: RequestId,
    /// Reader purpose, independent of plan validity.
    pub kind: LeaseKind,
    /// UTC microseconds sampled by the owner.
    pub now_us: i64,
    /// Positive lease duration.
    pub ttl_us: i64,
}
impl Store {
    /// Resolve a foreign request under its provider's owner. Reuse the local atomic
    /// selector/lease transaction, then restore the consumer-qualified provenance.
    pub async fn resolve_export(
        &mut self,
        provider: tf_domain::WorkspaceId,
        mut binding: InputBinding,
        pin: Option<VersionId>,
        request: ReadRequest,
    ) -> Result<ResolvedRead, ResolutionError> {
        if binding.dataset.workspace_id() != provider || !matches!(request.kind, LeaseKind::Query) {
            return Err(ResolutionError::Context);
        }
        let original = binding.key.clone();
        binding.key =
            tf_domain::input::InputBindingKey::new(binding.dataset, original.alias().into())
                .map_err(|_| ResolutionError::Context)?;
        let mut read = if let Some(version) = pin {
            self.resolve_pin(binding, version, &BTreeSet::new(), request)
                .await?
        } else {
            self.resolve_input(binding, &BTreeSet::new(), request)
                .await?
        };
        read.binding.key = original;
        Ok(read)
    }
    /// Local read boundary only. The owner serializes this with publication/collection.
    /// No filesystem scan, check evaluator or user function runs inside the transaction.
    pub async fn resolve_input(
        &mut self,
        binding: InputBinding,
        writes: &BTreeSet<tf_domain::DatasetKey>,
        request: ReadRequest,
    ) -> Result<ResolvedRead, ResolutionError> {
        match binding.disposition(writes) {
            BindingDisposition::PlannedProducer => return Err(ResolutionError::PlannedProducer),
            BindingDisposition::ForeignWorkspace => return Err(ResolutionError::Context),
            _ => (),
        }
        let expires = request
            .now_us
            .checked_add(request.ttl_us)
            .filter(|_| request.ttl_us > 0)
            .ok_or(ResolutionError::Context)?;
        retention::clock(&mut self.db, request.now_us).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        retention::clock(&mut tx, request.now_us).await?;
        let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM datasets WHERE id=? AND workspace_id=? AND tombstoned_at_us IS NULL")
            .bind(binding.dataset.dataset_id().to_string()).bind(binding.dataset.workspace_id().to_string()).fetch_one(&mut *tx).await?;
        if exists != 1 {
            return Err(ResolutionError::Context);
        }
        let mut missing = Vec::new();
        for name in binding.policy.candidates() {
            let branch = sqlx::query(
                "SELECT id,deleted_at_us FROM data_branches WHERE workspace_id=? AND name=?",
            )
            .bind(binding.dataset.workspace_id().to_string())
            .bind(name.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            let Some(branch) = branch else {
                missing.push(MissingCandidate {
                    branch: name.clone(),
                    reason: AbsenceReason::BranchAbsent,
                });
                continue;
            };
            if branch.try_get::<Option<i64>, _>(1)?.is_some() {
                missing.push(MissingCandidate {
                    branch: name.clone(),
                    reason: AbsenceReason::BranchDeleted,
                });
                continue;
            }
            let branch_id: BranchId = branch
                .try_get::<String, _>(0)?
                .parse()
                .map_err(|_| ResolutionError::Context)?;
            let head: Option<String> = sqlx::query_scalar(
                "SELECT version_id FROM dataset_heads WHERE branch_id=? AND dataset_id=?",
            )
            .bind(branch_id.to_string())
            .bind(binding.dataset.dataset_id().to_string())
            .fetch_optional(&mut *tx)
            .await?;
            let Some(head) = head else {
                missing.push(MissingCandidate {
                    branch: name.clone(),
                    reason: AbsenceReason::HeadAbsent,
                });
                continue;
            };
            let version = head.parse().map_err(|_| ResolutionError::Context)?;
            let lease = retention::lease_in_transaction(
                &mut tx,
                request.lease,
                request.operation,
                request.kind,
                ReadTarget::Head(branch_id, binding.dataset.dataset_id()),
                request.now_us,
                expires,
            )
            .await?;
            let resolved = name.clone();
            tx.commit().await?;
            return Ok(ResolvedRead {
                binding,
                branch_id,
                resolved,
                version,
                missing,
                lease,
                exact: false,
            });
        }
        Err(ResolutionError::Missing(missing))
    }
}

impl Store {
    /// Pin one local historical version, independent of live/deleted branches and head movement.
    /// Ownership and availability are checked in the same transaction as the lease. Byte integrity
    /// still requires tf-exec's strict input verification before the consumer is allowed to run.
    pub async fn resolve_pin(
        &mut self,
        binding: InputBinding,
        version: VersionId,
        writes: &BTreeSet<tf_domain::DatasetKey>,
        request: ReadRequest,
    ) -> Result<ResolvedRead, ResolutionError> {
        match binding.disposition(writes) {
            BindingDisposition::PlannedProducer => return Err(ResolutionError::PlannedProducer),
            BindingDisposition::ForeignWorkspace => return Err(ResolutionError::Context),
            _ => (),
        }
        let expires = request
            .now_us
            .checked_add(request.ttl_us)
            .filter(|_| request.ttl_us > 0)
            .ok_or(ResolutionError::Context)?;
        retention::clock(&mut self.db, request.now_us).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        retention::clock(&mut tx, request.now_us).await?;
        // Immutable publication history supplies origin, never a current head. Imports and
        // executions both publish a head-change event; later adoption does not change this origin.
        let row = sqlx::query("SELECT b.id,b.name FROM dataset_versions v JOIN datasets d ON d.id=v.dataset_id JOIN head_changes h ON h.new_version_id=v.id JOIN data_branches b ON b.id=h.branch_id JOIN events e ON e.id=h.event_id WHERE v.id=? AND d.id=? AND d.workspace_id=? ORDER BY e.sequence LIMIT 1")
            .bind(version.to_string()).bind(binding.dataset.dataset_id().to_string())
            .bind(binding.dataset.workspace_id().to_string()).fetch_optional(&mut *tx).await?
            .ok_or(ResolutionError::Context)?;
        let branch_id = row
            .try_get::<String, _>(0)?
            .parse()
            .map_err(|_| ResolutionError::Context)?;
        let resolved = row
            .try_get::<String, _>(1)?
            .parse()
            .map_err(|_| ResolutionError::Context)?;
        let lease = retention::lease_in_transaction(
            &mut tx,
            request.lease,
            request.operation,
            request.kind,
            ReadTarget::Version(version),
            request.now_us,
            expires,
        )
        .await?;
        tx.commit().await?;
        Ok(ResolvedRead {
            binding,
            branch_id,
            resolved,
            version,
            missing: vec![],
            lease,
            exact: true,
        })
    }
}
