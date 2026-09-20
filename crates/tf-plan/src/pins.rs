//! Exact historical overrides are planning inputs, never public decorator arguments.
use std::collections::{BTreeMap, BTreeSet};
use tf_catalog::RegistrySnapshot;
use tf_domain::{
    DatasetKey, VersionId,
    input::{BindingDisposition, InputBinding, InputBindingKey},
};

/// A parsed request still needs catalogue, binding and retained-version qualification.
#[derive(Clone, Debug)]
pub struct PinRequest {
    reference: String,
    alias: Option<String>,
    version: VersionId,
}
/// Errors preserve binding-qualified alternatives for an ambiguous dataset shorthand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PinError {
    /// Invalid syntax, reference, binding, or excessive request size.
    Invalid,
    /// More than one effective input selector exists for this dataset.
    Ambiguous(Vec<String>),
    /// Repeated overrides disagree on the exact version.
    Conflict,
    /// A pin cannot replace an output selected for production in this build.
    WriteConflict,
}
impl std::fmt::Display for PinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => {
                f.write_str("Invalid pin; use dataset=version or consumer#alias=version")
            }
            Self::Ambiguous(alternatives) => write!(
                f,
                "Ambiguous pin; qualify a binding: {}",
                alternatives.join(", ")
            ),
            Self::Conflict => f.write_str("Repeated pins disagree on the exact input version"),
            Self::WriteConflict => f.write_str(
                "A pinned boundary cannot also be selected for production on its starting branch",
            ),
        }
    }
}
impl std::error::Error for PinError {}
impl std::str::FromStr for PinRequest {
    type Err = PinError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() > 8192 {
            return Err(PinError::Invalid);
        }
        let (binding, version) = text.split_once('=').ok_or(PinError::Invalid)?;
        let version = version.parse().map_err(|_| PinError::Invalid)?;
        let (reference, alias) = match binding.split_once('#') {
            Some((r, a))
                if !a.is_empty()
                    && !a.contains('#')
                    && a.len() <= 1024
                    && !a.chars().any(char::is_control) =>
            {
                (r, Some(a.to_owned()))
            }
            Some(_) => return Err(PinError::Invalid),
            None => (binding, None),
        };
        if reference.is_empty() {
            return Err(PinError::Invalid);
        }
        Ok(Self {
            reference: reference.into(),
            alias,
            version,
        })
    }
}
/// Qualify aliases without I/O or version lookup. The store subsequently checks ownership,
/// availability and collection fences before acquiring an exact-version read lease.
pub fn qualify(
    requests: &[PinRequest],
    registry: &RegistrySnapshot,
    bindings: &BTreeMap<InputBindingKey, InputBinding>,
    writes: &BTreeSet<DatasetKey>,
) -> Result<BTreeMap<InputBindingKey, VersionId>, PinError> {
    if requests.len() > 1024 || bindings.len() > 10000 {
        return Err(PinError::Invalid);
    }
    let mut result = BTreeMap::new();
    for request in requests {
        let key = registry
            .resolve(&request.reference)
            .map_err(|_| PinError::Invalid)?
            .key();
        let matches: Vec<_> = bindings
            .values()
            .filter(|b| match &request.alias {
                Some(alias) => b.key.consumer() == key && b.key.alias() == alias,
                None => b.dataset == key,
            })
            .collect();
        let first = matches.first().ok_or(PinError::Invalid)?;
        // Spelling omitted versus CURRENT does not create a different effective selector.
        if request.alias.is_none()
            && matches.iter().any(|b| {
                b.policy.start() != first.policy.start()
                    || b.policy.candidates() != first.policy.candidates()
                    || b.policy.permission() != first.policy.permission()
            })
        {
            let alternatives = matches
                .iter()
                .map(|b| {
                    let consumer = registry
                        .by_id(b.key.consumer().dataset_id())
                        .map(|d| d.path().to_string())
                        .unwrap_or_else(|_| format!("dataset:{}", b.key.consumer().dataset_id()));
                    format!("{consumer}#{}={}", b.key.alias(), request.version)
                })
                .collect();
            return Err(PinError::Ambiguous(alternatives));
        }
        for binding in matches {
            if binding.disposition(writes) == BindingDisposition::PlannedProducer {
                return Err(PinError::WriteConflict);
            }
            if let Some(old) = result.insert(binding.key.clone(), request.version)
                && old != request.version
            {
                return Err(PinError::Conflict);
            }
        }
    }
    Ok(result)
}
