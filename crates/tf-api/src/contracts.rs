//! Routing and runtime codecs share the authored OpenAPI input table.
use crate::ApiError;
use serde_json::Value;
use std::sync::OnceLock;
fn routes() -> Result<&'static Value, ApiError> {
    static ROUTES: OnceLock<Option<Value>> = OnceLock::new();
    ROUTES
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../../schemas/api-routes-v1.json")).ok()
        })
        .as_ref()
        .ok_or_else(ApiError::internal)
}
pub(crate) fn route(method: &str, path: &str) -> Result<&'static Value, ApiError> {
    routes()?
        .as_array()
        .ok_or_else(ApiError::internal)?
        .iter()
        .find(|v| {
            if v["method"] != method {
                return false;
            }
            let Some(template) = v["path"].as_str() else {
                return false;
            };
            let a = template.split('/');
            let b = path.split('/');
            a.clone().count() == b.clone().count()
                && a.zip(b)
                    .all(|(a, b)| a == b || (a.starts_with('{') && !b.is_empty()))
        })
        .ok_or_else(ApiError::missing)
}
pub(crate) fn validate(name: &str, value: &Value) -> Result<(), ApiError> {
    tf_protocol::validate_document(name, value).map_err(|_| ApiError::invalid())
}
pub(crate) fn request(route: &Value, body: &Value) -> Result<(), ApiError> {
    if let Some(name) = route["request"].as_str() {
        validate(name, body)?;
    }
    Ok(())
}
pub(crate) fn response(route: &Value, value: &Value) -> Result<(), ApiError> {
    validate(
        route["response"].as_str().ok_or_else(ApiError::internal)?,
        value,
    )
    .map_err(|_| ApiError::internal())
}
