//! Structured protocol failures never include incoming payload text.
use std::{fmt, io};

/// Framing, compatibility or session validation failure.
#[derive(Debug)]
pub enum ProtocolError {
    /// Failed to read/write the private channel.
    Io(io::Error),
    /// An incomplete length prefix or payload arrived.
    Truncated,
    /// A payload is empty or exceeds the frame byte cap.
    Size,
    /// Payload is invalid UTF-8/JSON, contains duplicate keys or exceeds JSON nesting limits.
    Json,
    /// Data violates a known schema or required semantic rule.
    InvalidDocument,
    /// The compiled schema could not be loaded.
    Schema,
    /// Peer protocol major is incompatible.
    Version,
    /// Frame identifies another request or attempt.
    Identity,
    /// Sequence repeated or decreased.
    Sequence,
    /// Hello/operation/terminal ordering is invalid.
    Order,
    /// A required feature was not mutually negotiated.
    Capability,
}
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Io(_) => "Worker control channel I/O failed",
            Self::Truncated => "Worker control frame was truncated",
            Self::Size => "Worker control payload must contain 1–1048576 bytes",
            Self::Json => "Worker control payload is not strict bounded JSON",
            Self::InvalidDocument => "Worker control data violates its declared contract",
            Self::Schema => "Compiled worker contract is unavailable",
            Self::Version => "Worker protocol major version is incompatible",
            Self::Identity => "Worker frame belongs to another request or attempt",
            Self::Sequence => "Worker frame sequence must increase",
            Self::Order => "Worker hello, operation or terminal ordering is invalid",
            Self::Capability => "Worker feature was not negotiated by both peers",
        })
    }
}
impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}
impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
