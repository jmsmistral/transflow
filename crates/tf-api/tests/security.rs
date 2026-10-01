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

struct EventsApp;
impl Application for EventsApp {
    fn capabilities(&self) -> Value {
        json!({"workspace":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"})
    }
    fn call(&self, r: tf_api::Request) -> Result<Reply, ApiError> {
        let workspace = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let events = if r.query.get("after").is_none_or(|s| s == "0") {
            (1..=2).map(|n|json!({"sequence":n.to_string(),"id":format!("00000000-0000-4000-8000-{n:012}"),"type":"job.state","payload":{},"context":{},"workspace":workspace,"timestamp_us":"1","causation":null,"correlation":null})).collect::<Vec<_>>()
        } else {
            vec![]
        };
        Ok(Reply::metadata(
            json!({"events":events,"cursor":"2","high_water":"2","retention_floor":"0","resync_required":r.query.get("after").is_some_and(|s|s=="9")}),
        ))
    }
}
#[tokio::test]
async fn unread_streams_are_bounded_leave_command_capacity_and_explicitly_resync()
-> Result<(), Box<dyn std::error::Error>> {
    let r = tf_api::router(
        "127.0.0.1:12345".parse()?,
        TOKEN.into(),
        Arc::new(EventsApp),
    )?;
    let send = |path: &str| {
        request("GET", path, "")
            .header("authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
    };
    let mut held = Vec::new();
    for _ in 0..4 {
        let response = r.clone().oneshot(send("/api/v1/events?after=0")?).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        held.push(response);
    }
    assert_eq!(
        r.clone().oneshot(send("/api/v1/events")?).await?.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        r.clone()
            .oneshot(send("/api/v1/capabilities")?)
            .await?
            .status(),
        StatusCode::OK
    );
    drop(held);
    // A separate transport has no held clients and closes an expired-cursor stream after resync.
    let r = tf_api::router(
        "127.0.0.1:12345".parse()?,
        TOKEN.into(),
        Arc::new(EventsApp),
    )?;
    let response = r.oneshot(send("/api/v1/events?after=9")?).await?;
    let text = String::from_utf8(to_bytes(response.into_body(), 4096).await?.to_vec())?;
    assert!(text.contains("event: resync_required"));
    assert!(text.contains("refetch_context_and_read_models"));
    Ok(())
}

#[tokio::test]
async fn browser_event_facade_requires_origin_session_and_csrf()
-> Result<(), Box<dyn std::error::Error>> {
    let r = tf_api::router(
        "127.0.0.1:12345".parse()?,
        TOKEN.into(),
        Arc::new(EventsApp),
    )?;
    let launch = r
        .clone()
        .oneshot(
            request("POST", "/api/v1/sessions/launch", "{}")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::from("{}"))?,
        )
        .await?;
    let code = data(launch).await?["data"]["code"].clone();
    let response = r
        .clone()
        .oneshot(
            request("POST", "/api/v1/sessions/exchange", "")
                .header("origin", "http://127.0.0.1:12345")
                .body(Body::from(json!({"code":code}).to_string()))?,
        )
        .await?;
    let set = response.headers()["set-cookie"].to_str()?.to_owned();
    let cookie = set.split(';').next().ok_or("cookie")?;
    let csrf = data(response).await?["data"]["csrf"]
        .as_str()
        .ok_or("csrf")?
        .to_owned();
    let body = json!({"path":"/api/v1/events","query":{"after":"9"}}).to_string();
    for (origin, csrf_header, expected) in
        [(false, false, 401), (true, false, 401), (true, true, 200)]
    {
        let mut b = request("POST", "/api/v1/read", &body).header("cookie", cookie);
        if origin {
            b = b.header("origin", "http://127.0.0.1:12345");
        }
        if csrf_header {
            b = b.header("x-transflow-csrf", &csrf);
        }
        let response = r.clone().oneshot(b.body(Body::from(body.clone()))?).await?;
        assert_eq!(response.status().as_u16(), expected);
        if expected == 200 {
            assert_eq!(response.headers()["content-type"], "text/event-stream");
            let bytes = to_bytes(response.into_body(), 4096).await?;
            assert!(std::str::from_utf8(&bytes)?.contains("resync_required"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn contributor_assets_are_frozen_bounded_and_keep_transport_security()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("tf-ui-{}", tf_api::cursor_token()?));
    std::fs::create_dir_all(root.join("assets"))?;
    std::fs::write(root.join("index.html"), "<h1>Synthetic UI</h1>")?;
    std::fs::write(root.join("assets/app.js"), "// synthetic bundle")?;
    std::fs::write(root.join("assets/app.css"), "body {}")?;
    let assets = tf_api::Assets::load(&root)?;
    // Requests cannot observe mutable files after startup, nor arbitrary adjacent files.
    std::fs::write(root.join("assets/app.js"), "// replaced")?;
    std::fs::write(root.join("private.txt"), "must not be served")?;
    let r = tf_api::router_with_assets(
        "127.0.0.1:12345".parse()?,
        TOKEN.into(),
        Arc::new(App(AtomicUsize::new(0))),
        Some(assets),
    )?;
    for (path, status) in [
        ("/", 200),
        ("/?view=00000000-0000-4000-8000-000000000001", 200),
        ("/?view=invalid", 400),
        ("/?view=00000000-0000-4000-8000-000000000001&token=x", 400),
        ("/?token=x", 400),
        ("/assets/app.js", 200),
        ("/assets/app.css", 200),
        ("/private.txt", 401),
        ("/assets/../private.txt", 401),
        ("/assets/app.js?token=x", 400),
    ] {
        let response = r
            .clone()
            .oneshot(request("GET", path, "").body(Body::empty())?)
            .await?;
        assert_eq!(response.status().as_u16(), status, "{path}");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(
            response.headers()["content-security-policy"]
                .to_str()?
                .contains("style-src 'self'")
        );
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        assert!(!String::from_utf8_lossy(&bytes).contains("must not be served"));
        if path == "/assets/app.js" {
            assert_eq!(bytes.as_ref(), b"// synthetic bundle");
        }
    }
    let forbidden = r
        .oneshot(
            request("GET", "/assets/app.js", "")
                .header("origin", "https://attacker.invalid")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("private.txt"), root.join("assets/leak.js"))?;
        assert!(tf_api::Assets::load(&root).is_err());
        std::fs::remove_file(root.join("assets/leak.js"))?;
    }
    std::fs::File::create(root.join("assets/large.js"))?.set_len(8 * 1024 * 1024 + 1)?;
    assert!(tf_api::Assets::load(&root).is_err());
    std::fs::remove_dir_all(root)?;
    Ok(())
}

struct SchedulesApp;
impl Application for SchedulesApp {
    fn capabilities(&self) -> Value {
        json!({"api_version":1})
    }
    fn call(&self, r: tf_api::Request) -> Result<Reply, ApiError> {
        if r.method != "PUT" || r.if_match != Some("a".repeat(64)) || r.key.is_none() {
            return Err(ApiError::invalid());
        }
        Ok(Reply {
            data: json!({"id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","etag":"b".repeat(64),"trigger_epoch":"c".repeat(64),"paused":true,"needs_review":false,"definition":r.body,"saved_at_us":"1"}),
            context: None,
            etag: Some("b".repeat(64)),
        })
    }
}
#[tokio::test]
async fn schedule_put_requires_authenticated_json_and_guard_and_cookie_csrf()
-> Result<(), Box<dyn std::error::Error>> {
    let r = tf_api::router(
        "127.0.0.1:12345".parse()?,
        TOKEN.into(),
        Arc::new(SchedulesApp),
    )?;
    let cases: Value =
        serde_json::from_str(include_str!("../../../schemas/fixtures/conformance.json"))?;
    let body = cases
        .as_array()
        .ok_or("cases")?
        .iter()
        .find(|c| c["name"] == "schedule-definition-fixed")
        .ok_or("definition")?["value"]
        .to_string();
    let path = "/api/v1/schedules/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    for (auth, origin, status) in [
        (false, None, 401),
        (true, Some("https://attacker.invalid"), 403),
        (true, None, 200),
    ] {
        let mut b = request("PUT", path, &body)
            .header("if-match", format!("\"{}\"", "a".repeat(64)))
            .header("idempotency-key", "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
        if auth {
            b = b.header("authorization", format!("Bearer {TOKEN}"));
        }
        if let Some(o) = origin {
            b = b.header("origin", o);
        }
        let reply = r.clone().oneshot(b.body(Body::from(body.clone()))?).await?;
        assert_eq!(reply.status().as_u16(), status);
        if status == 200 {
            assert_eq!(reply.headers()["etag"], format!("\"{}\"", "b".repeat(64)));
        }
    }
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
        .ok_or("code")?
        .to_owned();
    let exchange = r
        .clone()
        .oneshot(
            request("POST", "/api/v1/sessions/exchange", "")
                .header("origin", "http://127.0.0.1:12345")
                .body(Body::from(json!({"code":code}).to_string()))?,
        )
        .await?;
    let cookie = exchange.headers()["set-cookie"]
        .to_str()?
        .split(';')
        .next()
        .ok_or("cookie")?
        .to_owned();
    let csrf = data(exchange).await?["data"]["csrf"]
        .as_str()
        .ok_or("csrf")?
        .to_owned();
    for (has_csrf, status) in [(false, 401), (true, 200)] {
        let mut b = request("PUT", path, &body)
            .header("cookie", &cookie)
            .header("origin", "http://127.0.0.1:12345")
            .header("if-match", format!("\"{}\"", "a".repeat(64)))
            .header("idempotency-key", "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
        if has_csrf {
            b = b.header("x-transflow-csrf", &csrf);
        }
        assert_eq!(
            r.clone()
                .oneshot(b.body(Body::from(body.clone()))?)
                .await?
                .status()
                .as_u16(),
            status
        );
    }
    Ok(())
}
