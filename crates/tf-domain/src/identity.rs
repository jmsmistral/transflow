//! Stable identities contain no display paths, branch names or allocation policy.

use crate::{DomainError, ErrorKind};
use std::{fmt, str::FromStr};

fn parse_uuid(text: &str) -> Result<[u8; 16], DomainError> {
    let invalid = || DomainError::new(ErrorKind::InvalidUuid);
    if text.len() != 36 {
        return Err(invalid());
    }
    let mut nibbles = Vec::with_capacity(32);
    for (i, c) in text.bytes().enumerate() {
        if [8, 13, 18, 23].contains(&i) {
            if c != b'-' {
                return Err(invalid());
            }
        } else {
            nibbles.push(match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                _ => return Err(invalid()),
            });
        }
    }
    let mut bytes = [0; 16];
    for (byte, [high, low]) in bytes.iter_mut().zip(nibbles.as_chunks::<2>().0) {
        *byte = high * 16 + low;
    }
    Ok(bytes)
}
fn display_uuid(bytes: &[u8; 16], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for (i, byte) in bytes.iter().enumerate() {
        if [4, 6, 8, 10].contains(&i) {
            f.write_str("-")?;
        }
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}
macro_rules! id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name([u8; 16]);
        impl $name {
            /// Preserve an existing UUID's bytes; this does not allocate a new identity.
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }
            /// Raw UUID bytes, independent of its display spelling.
            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }
        }
        impl FromStr for $name {
            type Err = DomainError;
            fn from_str(text: &str) -> Result<Self, Self::Err> {
                parse_uuid(text).map(Self)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                display_uuid(&self.0, f)
            }
        }
    };
}
id!(
    RequestId,
    "Worker request UUID, independent of execution attempt identity."
);
id!(
    ExternalRegistrationId,
    "Explicit local registration of a foreign dataset."
);
id!(BuildId, "Build request identity.");
id!(JobId, "Planned producer job identity.");
id!(
    PlanId,
    "Immutable accepted plan identity, binding parameters and exact inputs."
);
id!(
    CoordinatorSessionId,
    "Coordinator ownership session identity used by fencing."
);
id!(WorkspaceId, "Stable owner workspace UUID.");
id!(
    DatasetId,
    "Stable dataset UUID; renames and branches do not change it."
);
id!(
    VersionId,
    "Immutable publication UUID, distinct from an artifact digest."
);
id!(
    AttemptId,
    "Execution attempt UUID, distinct from a published version."
);
id!(
    BranchId,
    "Stable data branch UUID, independent of its mutable name."
);
id!(
    SourceSnapshotId,
    "Captured source UUID; content fingerprints are separate."
);

/// Globally qualified dataset identity; a path is deliberately not part of it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DatasetKey {
    workspace_id: WorkspaceId,
    dataset_id: DatasetId,
}
impl DatasetKey {
    /// Associate an existing dataset identity with its owner.
    pub const fn new(workspace_id: WorkspaceId, dataset_id: DatasetId) -> Self {
        Self {
            workspace_id,
            dataset_id,
        }
    }
    /// Parse both UUIDs with errors pointing to the invalid identity member.
    pub fn from_text(workspace: &str, dataset: &str) -> Result<Self, DomainError> {
        Ok(Self::new(
            workspace
                .parse()
                .map_err(|e: DomainError| e.at("workspace_id"))?,
            dataset
                .parse()
                .map_err(|e: DomainError| e.at("dataset_id"))?,
        ))
    }
    /// Owning workspace, including for locally replicated foreign data.
    pub const fn workspace_id(self) -> WorkspaceId {
        self.workspace_id
    }
    /// Stable identity within its owning workspace.
    pub const fn dataset_id(self) -> DatasetId {
        self.dataset_id
    }
    /// Classify identity relative to an explicitly selected workspace.
    pub fn scope(self, current: WorkspaceId) -> DatasetScope {
        if self.workspace_id == current {
            DatasetScope::Local
        } else {
            DatasetScope::Foreign
        }
    }
}
/// Ownership classification; it does not authorize execution or registration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatasetScope {
    /// Owned by the selected workspace.
    Local,
    /// Owned by another workspace, even when bytes are locally replicated.
    Foreign,
}
