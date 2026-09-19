//! Logical dataset names are validated independently of stable identity.

use crate::{DatasetScope, DomainError, ErrorKind};
use std::{fmt, str::FromStr};

const KEYWORDS: &str = "False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield";

/// Validated logical path; never use this directly as a filesystem path.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DatasetPath(String);
impl DatasetPath {
    /// Validate a name for a known ownership scope without registering anything.
    pub fn parse_for_scope(text: &str, scope: DatasetScope) -> Result<Self, DomainError> {
        let path: Self = text.parse()?;
        path.validate_scope(scope)?;
        Ok(path)
    }
    /// Original path, without normalization or identity resolution.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Require ownership-compatible local or external namespace use.
    pub fn validate_scope(&self, scope: DatasetScope) -> Result<(), DomainError> {
        let mut parts = self.0.split('/');
        let external = parts.next() == Some("external");
        match scope {
            DatasetScope::Local if external => Err(DomainError::new(ErrorKind::ReservedNamespace)
                .at("0")
                .at("segments")),
            DatasetScope::Foreign if !external || parts.next().is_none() => {
                Err(DomainError::new(ErrorKind::MissingExternalAlias))
            }
            _ => Ok(()),
        }
    }
}
impl FromStr for DatasetPath {
    type Err = DomainError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        for (index, part) in text.split('/').enumerate() {
            let mut chars = part.bytes();
            let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
                && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
            let kind = if !valid {
                Some(ErrorKind::InvalidPath)
            } else if KEYWORDS.split_whitespace().any(|k| k == part) {
                Some(ErrorKind::ReservedKeyword)
            } else {
                None
            };
            if let Some(kind) = kind {
                return Err(DomainError::new(kind).at(index.to_string()).at("segments"));
            }
        }
        Ok(Self(text.to_owned()))
    }
}
impl fmt::Display for DatasetPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
