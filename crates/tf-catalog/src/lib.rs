//! Immutable durable catalogue parsing and exact reference lookup. No identity allocation or I/O.
mod registry;
pub use registry::*;
pub mod source;
pub mod workspace;
