//! Convert validated discovery bindings without losing aliases or declaration-graph edges.
use crate::{
    candidate::CandidateIdentity,
    validation::{ValidatedGraph, ValidationRequest},
    workspace::WorkspaceConfig,
};
use std::collections::BTreeMap;
use tf_domain::{
    BranchName, BranchSelector, FallbackPermission,
    input::{InputBinding, InputBindingKey, InputRole},
};
/// A pending identity, provider policy or malformed context is not a local binding.
#[derive(Debug, thiserror::Error)]
#[error(
    "Input binding preparation requires registered identities and an explicit local/provider policy"
)]
pub struct BindingError;
/// Prepare all local bindings in the validated graph, keyed by consumer plus alias.
/// External policy belongs to provider registration resolution; never use consumer defaults for it.
/// This does not prune off-branch or validation-only edges from structural cycle checks.
pub fn local_bindings(
    graph: &ValidatedGraph,
    context: &ValidationRequest<'_>,
    output: &BranchName,
    build_override: Option<&[BranchName]>,
) -> Result<BTreeMap<InputBindingKey, InputBinding>, BindingError> {
    if !graph.certificate().matches(context) {
        return Err(BindingError);
    }
    let config = WorkspaceConfig::parse(context.config_toml).map_err(|_| BindingError)?;
    let policies = config.input_policies().map_err(|_| BindingError)?;
    let mut bindings = BTreeMap::new();
    for definition in graph.candidate().definitions() {
        let CandidateIdentity::Registered(consumer) = definition.output else {
            return Err(BindingError);
        };
        for input in &definition.inputs {
            if bindings.len() >= 100_000 {
                return Err(BindingError);
            }
            let CandidateIdentity::Registered(dataset) = input.identity else {
                return Err(BindingError);
            };
            if dataset.workspace_id() != consumer.workspace_id() {
                return Err(BindingError);
            }
            let value = &input.declaration;
            let declared = match value["branch"]["kind"].as_str() {
                Some("omitted") => BranchSelector::Omitted,
                Some("current") => BranchSelector::Current,
                Some("named") => BranchSelector::Named(
                    value["branch"]["name"]
                        .as_str()
                        .ok_or(BindingError)?
                        .parse()
                        .map_err(|_| BindingError)?,
                ),
                _ => return Err(BindingError),
            };
            let permission = match value["stop_branch_fallback"].as_bool() {
                Some(true) => FallbackPermission::Prohibited,
                Some(false) => FallbackPermission::Allowed,
                _ => return Err(BindingError),
            };
            let role = match value["role"].as_str() {
                Some("data") => InputRole::Data,
                Some("validation") => InputRole::Validation,
                _ => return Err(BindingError),
            };
            let key =
                InputBindingKey::new(consumer, input.alias.clone()).map_err(|_| BindingError)?;
            let policy = policies
                .local(output, declared, permission, build_override)
                .map_err(|_| BindingError)?;
            let binding = InputBinding {
                key: key.clone(),
                dataset,
                role,
                policy,
            };
            if bindings.insert(key, binding).is_some() {
                return Err(BindingError);
            }
        }
    }
    Ok(bindings)
}
