//! Immutable durable catalogue parsing and exact reference lookup. No identity allocation or I/O.
mod registry;
pub use registry::*;
/// Nonpersistent candidate reconciliation and proposed additive identities.
pub mod candidate;
pub mod capture;
pub mod git;
pub mod source;
pub mod workspace;
