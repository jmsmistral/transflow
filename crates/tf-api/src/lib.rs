//! Authenticated loopback HTTP transport over application services; no storage or worker access.
mod assets;
mod auth;
pub use assets::Assets;
mod contracts;
mod server;
use serde_json::{Value, json};
pub use server::{Server, router, router_with_assets};
use std::{collections::BTreeMap, sync::Arc};
use tf_domain::RequestId;

/// Hard body bound before JSON parsing, advertised in capability metadata.
pub const BODY_LIMIT: usize = 2 * 1024 * 1024;
/// Maximum simultaneous HTTP application requests, including blocking service work.
pub const IN_FLIGHT_LIMIT: usize = 16;
/// HTTP/application failure with safe messages; never attach raw source/DB/token errors.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    /// HTTP response status.
    pub status: u16,
    /// Stable product error code.
    pub code: &'static str,
    /// Human-readable safe explanation.
    pub message: &'static str,
    /// Whether an unchanged request may be retried.
    pub retryable: bool,
    /// Safe structured details, without stack traces or credentials.
    pub details: Value,
}
impl ApiError {
    /// Create a deliberate public diagnostic.
    pub fn new(status: u16, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            retryable: false,
            details: json!({}),
        }
    }
    /// Invalid request or unsupported fields.
    pub fn invalid() -> Self {
        Self::new(
            400,
            "TF_API_REQUEST",
            "The API request is invalid or unsupported",
        )
    }
    /// Missing or invalid authentication.
    pub fn unauthorized() -> Self {
        Self::new(401, "TF_API_AUTH", "Coordinator authentication is required")
    }
    /// Context/revision changed; never silently replan.
    pub fn conflict() -> Self {
        Self::new(
            409,
            "TF_API_CONFLICT",
            "The selected context changed; refresh it before retrying",
        )
    }
    /// Bounded queues or a busy mutation owner.
    pub fn busy() -> Self {
        let mut e = Self::new(
            503,
            "TF_API_BUSY",
            "The coordinator is busy; retry when the operation finishes",
        );
        e.retryable = true;
        e
    }
    /// Missing exact retained resource.
    pub fn missing() -> Self {
        Self::new(
            404,
            "TF_API_NOT_FOUND",
            "The requested resource is unavailable in this context",
        )
    }
    /// Safe failure for unexpected implementation/storage state.
    pub fn internal() -> Self {
        Self::new(
            500,
            "TF_API_FAILED",
            "The operation could not complete; inspect the retained workspace evidence",
        )
    }
}
/// Validated HTTP transport request. The application decides the shared domain operation.
#[derive(Clone)]
pub struct Request {
    /// Server-generated diagnostic identity.
    pub id: RequestId,
    /// GET, POST or PUT (unsupported methods are refused).
    pub method: String,
    /// Decoded path, without query strings or credentials.
    pub path: String,
    /// Strict query parameters; unknown keys must fail at the service boundary.
    pub query: BTreeMap<String, String>,
    /// JSON request object, never arbitrary command-line arguments.
    pub body: Value,
    /// Durable idempotency identity required for mutations.
    pub key: Option<RequestId>,
    /// Optimistic context/revision guard.
    pub if_match: Option<String>,
}
/// Successful domain result with explicit selection context.
pub struct Reply {
    /// Response data validated against the endpoint's shared schema.
    pub data: Value,
    /// Context fingerprint plus branch/source/version facts, when applicable.
    pub context: Option<Value>,
    /// Current optimistic revision, if applicable.
    pub etag: Option<String>,
}
impl Reply {
    /// Uncontextualized metadata result.
    pub fn metadata(data: Value) -> Self {
        Self {
            data,
            context: None,
            etag: None,
        }
    }
}
/// Composition boundary. Implementations run on a bounded blocking pool, never the HTTP reactor.
pub trait Application: Send + Sync + 'static {
    /// Dispatch one authenticated request to shared application services.
    fn call(&self, request: Request) -> Result<Reply, ApiError>;
    /// Supported operations/engines and explicit implementation limits.
    fn capabilities(&self) -> Value;
}
/// Type-erased application service, not a second coordinator.
pub type Service = Arc<dyn Application>;

/// Unpredictable opaque cursor identity; carries no credential or encoded source path.
pub fn cursor_token() -> Result<String, ApiError> {
    auth::secret()
}
