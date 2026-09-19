//! Validated domain identities, names, logical schemas and lossless value carriers.
//!
//! Constructors perform no I/O, identity allocation, catalogue lookup or conversion
//! through an engine. JSON codecs belong to tf-protocol; storage and execution are
//! separate crate responsibilities.
//!
//! Identity roles cannot be interchanged accidentally:
//! ```compile_fail
//! use tf_domain::{AttemptId, DatasetId};
//! let dataset = DatasetId::from_bytes([0; 16]);
//! let attempt: AttemptId = dataset;
//! ```

pub mod branch;
mod error;
pub mod identity;
pub mod path;
pub mod schema;
pub mod value;

pub use branch::{BranchName, BranchSelector, FallbackPermission};
pub use error::{DomainError, ErrorKind};
pub use identity::{
    AttemptId, BranchId, DatasetId, DatasetKey, DatasetScope, RequestId, SourceSnapshotId,
    VersionId, WorkspaceId,
};
pub use path::DatasetPath;

/// Human-first diagnostics, safe text and the shared exit-status contract.
pub mod diagnostic;
