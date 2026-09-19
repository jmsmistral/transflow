//! Branch names and declarations; no branch lookup or fallback resolution.

use crate::{DomainError, ErrorKind};
use std::{fmt, str::FromStr};

/// Opaque, case-sensitive metadata, not a filesystem path or Git ref parser.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BranchName(String);
impl BranchName {
    /// Preserve the exact selected name, including slash and Unicode characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl FromStr for BranchName {
    type Err = DomainError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(DomainError::new(ErrorKind::InvalidBranch));
        }
        Ok(Self(text.to_owned()))
    }
}
impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
/// Declared input selector; omitted and CURRENT remain distinct for foreign inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BranchSelector {
    /// Local build branch or registered foreign default, resolved later.
    Omitted,
    /// Explicitly use the consumer build branch name, including for foreign inputs.
    Current,
    /// Start at this named branch; this alone does not forbid fallback.
    Named(BranchName),
}
/// Fallback permission is independent of the branch selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FallbackPermission {
    /// Follow the effective captured policy if needed.
    Allowed,
    /// Stop resolution at the selected starting branch.
    Prohibited,
}
