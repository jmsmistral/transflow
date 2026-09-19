//! Location-aware errors from domain constructors; no input values are echoed.

use std::{error::Error, fmt};

/// Stable class of invalid domain input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// A UUID did not use canonical lowercase hexadecimal grouping.
    InvalidUuid,
    /// A path segment is not an allowed identifier.
    InvalidPath,
    /// A Python hard keyword was used as a path segment.
    ReservedKeyword,
    /// A local path used the reserved external namespace.
    ReservedNamespace,
    /// A foreign registration lacked its external namespace and alias.
    MissingExternalAlias,
    /// A branch was empty or contained a control character.
    InvalidBranch,
    /// A field name was empty, oversized or contained an ASCII control.
    InvalidName,
    /// Two fields have the same name within one container.
    DuplicateName,
    /// A number did not use the required string spelling.
    InvalidNumber,
    /// A number cannot be represented in its declared width.
    OutOfRange,
    /// Decimal precision or scale is outside the supported range.
    InvalidDecimalType,
    /// Decimal digits disagree with precision or scale.
    InvalidDecimalValue,
    /// Calendar text contains an invalid date or time.
    InvalidDateTime,
    /// Timestamp text disagrees with its unit or timezone.
    InvalidTimestamp,
    /// Timezone text is not UTC or an IANA-style carrier name.
    InvalidTimezone,
}
impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidUuid => "Expected a lowercase UUID in 8-4-4-4-12 form",
            Self::InvalidPath => "Use slash-separated lowercase dataset identifiers",
            Self::ReservedKeyword => "A dataset path segment cannot be a Python keyword",
            Self::ReservedNamespace => {
                "The external namespace is reserved for foreign registrations"
            }
            Self::MissingExternalAlias => "A foreign path needs external/ followed by an alias",
            Self::InvalidBranch => "Use a nonempty branch name without control characters",
            Self::InvalidName => "Use a field name of 1–256 characters without ASCII controls",
            Self::DuplicateName => "Field names must be unique within their container",
            Self::InvalidNumber => "Use the declared numeric type's decimal string format",
            Self::OutOfRange => "The number cannot be represented without overflow or underflow",
            Self::InvalidDecimalType => "Decimal precision must be 1–38 and scale must be −38–38",
            Self::InvalidDecimalValue => {
                "Decimal digits must match the declared precision and scale"
            }
            Self::InvalidDateTime => "Use a valid Gregorian calendar date and time",
            Self::InvalidTimestamp => {
                "Timestamp fraction and UTC suffix must match its unit and timezone"
            }
            Self::InvalidTimezone => "Use UTC or a slash-separated timezone name",
        })
    }
}
/// A typed validation error with a JSON Pointer location relative to its input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainError {
    kind: ErrorKind,
    location: String,
}
impl DomainError {
    pub(crate) fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            location: String::new(),
        }
    }
    /// Error category, independent of the rendered explanation.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
    /// JSON Pointer; an empty string names the constructor's root input.
    pub fn location(&self) -> &str {
        &self.location
    }
    /// Add a containing member/index, escaping it as a JSON Pointer segment.
    pub fn at(mut self, segment: impl AsRef<str>) -> Self {
        let escaped = segment.as_ref().replace('~', "~0").replace('/', "~1");
        self.location = format!("/{escaped}{}", self.location);
        self
    }
}
impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.location.is_empty() {
            self.kind.fmt(f)
        } else {
            write!(f, "{} at {}", self.kind, self.location)
        }
    }
}
impl Error for DomainError {}
