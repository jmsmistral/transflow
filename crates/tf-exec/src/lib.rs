//! Coordinator ownership foundations; worker process supervision remains later work.
pub mod environment;
pub mod ownership;

/// Authenticated bounded discovery process and temporary request storage.
pub mod discovery;

/// Owner-held durable object publication and startup reconciliation.
pub mod publication;
