//! Security and durable-state regression coverage.
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tf_api::{ApiError, Application, Reply};
use tower::ServiceExt;
struct App(AtomicUsize);
impl Application for App {
    fn capabilities(&self) -> Value {
        json!({"api_version":1})
    }
    fn call(&self, _r: tf_api::Request) -> Result<Reply, ApiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Reply::metadata(
            json!({"entries":[],"total":"0","next_cursor":null,"schema":null,"freshness":"unknown"}),
        ))
    }
}
const TOKEN: &str = "test-only-bearer-credential-not-a-real-token";
fn router() -> Result<(axum::Router, Arc<App>), ApiError> {
    let app = Arc::new(App(AtomicUsize::new(0)));
    Ok((
        tf_api::router(
            "127.0.0.1:12345".parse().map_err(|_| ApiError::invalid())?,
            TOKEN.into(),
            app.clone(),
        )?,
        app,
    ))
}
fn request(method: &str, path: &str, body: &str) -> axum::http::request::Builder {
    let _ = body;
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:12345")
        .header("content-type", "application/json")
}
async fn data(r: axum::response::Response) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(
        &to_bytes(r.into_body(), 3 * 1024 * 1024).await?,
    )?)
}
#[tokio::test]
async fn authentication_origin_host_and_error_boundaries() -> Result<(), Box<dyn std::error::Error>>
{
    let (r, app) = router()?;
    for (auth, origin, host, status) in [
        (false, None, "127.0.0.1:12345", 401),
        (
            true,
            Some("https://attacker.invalid"),
            "127.0.0.1:12345",
            403,
        ),
        (true, None, "attacker.invalid", 403),
        (true, None, "127.0.0.1:12345", 200),
    ] {
        let mut b = Request::builder()
            .uri("/api/v1/datasets")
            .header("host", host);
        if auth {
            b = b.header("authorization", format!("Bearer {TOKEN}"));
        }
        if let Some(o) = origin {
            b = b.header("origin", o);
        }
        let response = r.clone().oneshot(b.body(Body::empty())?).await?;
        assert_eq!(response.status().as_u16(), status);
        assert!(response.headers().contains_key("x-request-id"));
        assert_eq!(response.headers()["cache-control"], "no-store");
        let value = data(response).await?;
        assert!(!value.to_string().contains(TOKEN));
        if status != 200 {
            assert!(value["error"]["request_id"].is_string());
        }
    }
    assert_eq!(app.0.load(Ordering::SeqCst), 1);
    Ok(())
}
#[tokio::test]
async fn one_time_exchange_cookie_csrf_and_no_origin_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let (r, app) = router()?;
    let launch = r
        .clone()
        .oneshot(
            request("POST", "/api/v1/sessions/launch", "{}")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::from("{}"))?,
        )
        .await?;
    let code = data(launch).await?["data"]["code"]
        .as_str()
        .ok_or("missing code")?
        .to_owned();
    let exchange = || {
        request("POST", "/api/v1/sessions/exchange", "")
            .header("origin", "http://127.0.0.1:12345")
            .body(Body::from(json!({"code":code}).to_string()))
    };
    let response = r.clone().oneshot(exchange()?).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let set = response.headers()["set-cookie"].to_str()?.to_owned();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict"));
    let cookie = set.split(';').next().ok_or("cookie")?;
    let csrf = data(response).await?["data"]["csrf"]
        .as_str()
        .ok_or("csrf")?
        .to_owned();
    assert_eq!(
        r.clone().oneshot(exchange()?).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    for (origin, header, status) in [(false, false, 401), (true, false, 401), (true, true, 200)] {
        let mut b = request("POST", "/api/v1/sessions/verify", "{}").header("cookie", cookie);
        if origin {
            b = b.header("origin", "http://127.0.0.1:12345");
        }
        if header {
            b = b.header("x-transflow-csrf", &csrf);
        }
        assert_eq!(
            r.clone()
                .oneshot(b.body(Body::from("{}"))?)
                .await?
                .status()
                .as_u16(),
            status
        );
    }
    assert_eq!(app.0.load(Ordering::SeqCst), 0);
    Ok(())
}
#[tokio::test]
async fn rejects_duplicate_json_oversize_unknown_methods_and_token_queries()
-> Result<(), Box<dyn std::error::Error>> {
    let (r, app) = router()?;
    for (method, path, body, status) in [
        (
            "POST",
            "/api/v1/builds",
            "{\"a\":1,\"a\":2}".to_owned(),
            400,
        ),
        (
            "POST",
            "/api/v1/builds",
            "x".repeat(tf_api::BODY_LIMIT + 1),
            413,
        ),
        ("GET", "/api/v1/datasets?token=secret", String::new(), 400),
        ("GET", "/api/v1/datasets?a=1&a=2", String::new(), 400),
        ("DELETE", "/api/v1/builds", String::new(), 405),
    ] {
        let b = request(method, path, &body).header("authorization", format!("Bearer {TOKEN}"));
        assert_eq!(
            r.clone()
                .oneshot(b.body(Body::from(body))?)
                .await?
                .status()
                .as_u16(),
            status
        );
    }
    assert_eq!(app.0.load(Ordering::SeqCst), 0);
    Ok(())
}
