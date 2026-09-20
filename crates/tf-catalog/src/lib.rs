//! Immutable durable catalogue parsing and exact reference lookup. No identity allocation or I/O.
mod registry;
pub use registry::*;
/// Nonpersistent candidate reconciliation and proposed additive identities.
pub mod candidate;
pub mod capture;
pub mod git;
pub mod source;
pub mod workspace;

/// Guarded authoring writes; SQLite composition belongs to the application crate.
pub mod registry_write;

/// Workspace-local typing-only catalogue generations and atomic refresh.
pub mod editor;

/// Complete local structural validation and immutable context-bound certificates.
pub mod validation;

/// Persisted structural evidence, never an execution fallback.
pub mod graph_cache;

/// Exact catalogue pages and retained identity lookup.
pub mod browse;

/// Explicit local-file selection for import preparation.
pub mod local_files;

/// Complete conservative computation and check fingerprints.
pub mod compute;
/// Alias-preserving input preparation over the complete validated graph.
pub mod input_bindings;
