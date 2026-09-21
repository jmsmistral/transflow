//! Versioned contracts, bounded private-channel codecs and session guards.
//! No worker execution, process launching or HTTP endpoints are implemented here.

/// JSON Schema definitions plus explicitly asserted Transflow format/invariant rules.
pub const CONTRACT_SCHEMA: &str = include_str!("../../../schemas/contracts-v1.schema.json");

/// Independently versioned format baselines; SQLite 0 means no product migration yet.
pub const FORMAT_VERSIONS: &str = include_str!("../../../schemas/versions.json");

mod error;
mod frame;
mod session;
mod validation;
pub use error::ProtocolError;
pub use frame::{
    ControlFrame, MAX_FRAME_BYTES, MessageType, PROTOCOL_MAJOR, PROTOCOL_MINOR, decode_payload,
    read_frame, write_frame,
};
pub use session::{Negotiated, Operation, Session};
pub use validation::validate_document;

/// Canonical metadata encoding and purpose-separated content hashes.
pub mod canonical;

/// Versioned CLI result envelopes.
pub mod diagnostic;

/// Expectation AST-v1 decoding and deferred-schema semantic validation.
pub mod expectation;
