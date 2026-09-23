//! Coordinator ownership, process supervision and execution services.
pub mod environment;
pub mod ownership;

/// Authenticated bounded discovery process and temporary request storage.
pub mod discovery;

/// Owner-held durable object publication and startup reconciliation.
pub mod publication;

/// Exact selected-input integrity verification without fallback on data errors.
pub mod input_read;

/// Owner-held cache artifact verification and atomic reuse.
pub mod cache;

/// Owned worker groups, authenticated control and bounded log retention.
pub mod supervisor;

/// Bounded atomic job/CPU and optional memory/disk reservations.
pub mod admission;

/// Monotonic execution, validation, interactive and discovery budgets.
pub mod timing;

/// Persisted build cancellation, attempt-bound workers and client disconnect policy.
pub mod cancellation;

/// Pinned Polars scans, one materialization and independently validated staging.
pub mod polars;

/// Canonical exact checks over verified Parquet subjects.
pub mod checks;

/// One-attempt input/output gates and approved-candidate publication.
pub mod lifecycle;

/// Authenticated emergency worker cleanup after coordinator loss.
pub mod recovery;

/// Safe descriptor-relative retained log reads.
pub mod retained_logs;
