//! Authoritative T012 wire-contract documents. Runtime transport is not implemented.

/// JSON Schema definitions plus explicitly asserted Transflow format/invariant rules.
pub const CONTRACT_SCHEMA: &str = include_str!("../../../schemas/contracts-v1.schema.json");

/// Independently versioned format baselines; SQLite 0 means no product migration yet.
pub const FORMAT_VERSIONS: &str = include_str!("../../../schemas/versions.json");
