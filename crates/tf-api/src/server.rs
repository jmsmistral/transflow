use crate::{ApiError, Application, BODY_LIMIT, IN_FLIGHT_LIMIT, Reply, Request, Service, auth};
use axum::{
    Router,
    body::to_bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    net::{SocketAddr, TcpListener},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tf_domain::RequestId;

struct Shared {
    assets: Option<crate::Assets>,
    app: Service,
    token: String,
    host: String,
    origin: String,
    grants: Mutex<auth::Grants>,
    permits: Arc<tokio::sync::Semaphore>,
    streams: Arc<tokio::sync::Semaphore>,
}
/// Running loopback server. Drop stops listening before coordinator ownership can be released.
pub struct Server {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    address: SocketAddr,
}
impl Server {
    /// Bind loopback before starting the server. LAN binds are never supported.
    pub fn bind(address: SocketAddr, token: String, app: Service) -> Result<Self, ApiError> {
        Self::bind_with_assets(address, token, app, None)
    }
    /// Bind with an explicitly supplied immutable contributor UI bundle.
    pub fn bind_with_assets(
        address: SocketAddr,
        token: String,
        app: Service,
        assets: Option<crate::Assets>,
    ) -> Result<Self, ApiError> {
        if !address.ip().is_loopback() || token.len() < 32 {
            return Err(ApiError::invalid());
        }
        let listener = TcpListener::bind(address).map_err(|_| ApiError::internal())?;
        listener
            .set_nonblocking(true)
            .map_err(|_| ApiError::internal())?;
        let address = listener.local_addr().map_err(|_| ApiError::internal())?;
        let shared = state(address, token, app, assets);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let app = routes(shared.clone());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                let _ = tx.send(false);
                return;
            };
            rt.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                    let _ = tx.send(false);
                    return;
                };
                let _ = tx.send(true);
                let shutdown = async move {
                    while !stopping.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                };
                // Stop accepting immediately on shutdown. Dropping the serve future closes transport;
                // queued domain mutations retain coordinator ownership and idempotency receipts.
                let serve = axum::serve(listener, app).into_future();
                tokio::pin!(serve);
                tokio::pin!(shutdown);
                tokio::select! { _=&mut serve=>{}, _=&mut shutdown=>{} }
            });
            rt.shutdown_timeout(Duration::from_secs(2));
        });
        if rx.recv_timeout(Duration::from_secs(5)).ok() != Some(true) {
            stop.store(true, Ordering::Release);
            let _ = thread.join();
            return Err(ApiError::internal());
        }
        Ok(Self {
            shared,
            stop,
            thread: Some(thread),
            address,
        })
    }
    /// Bound endpoint, suitable for private runtime discovery metadata.
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    /// Issue one short-lived browser URL. Never print/persist the long-lived bearer credential.
    pub fn launch_url(&self) -> Result<String, ApiError> {
        let code = self
            .shared
            .grants
            .lock()
            .map_err(|_| ApiError::internal())?
            .launch(Instant::now())?;
        Ok(format!("{}/#{}", self.shared.origin, code))
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn state(
    address: SocketAddr,
    token: String,
    app: Service,
    assets: Option<crate::Assets>,
) -> Arc<Shared> {
    Arc::new(Shared {
        assets,
        app,
        token,
        host: address.to_string(),
        origin: format!("http://{address}"),
        grants: Mutex::new(auth::Grants::new()),
        permits: Arc::new(tokio::sync::Semaphore::new(IN_FLIGHT_LIMIT)),
        streams: Arc::new(tokio::sync::Semaphore::new(4)),
    })
}
fn routes(shared: Arc<Shared>) -> Router {
    Router::new().fallback(handle).with_state(shared)
}
/// Identical transport middleware exposed for deterministic HTTP security tests.
pub fn router(
    address: SocketAddr,
    token: String,
    app: Arc<dyn Application>,
) -> Result<Router, ApiError> {
    router_with_assets(address, token, app, None)
}
/// Identical middleware with a frozen contributor asset bundle, for transport qualification.
pub fn router_with_assets(
    address: SocketAddr,
    token: String,
    app: Arc<dyn Application>,
    assets: Option<crate::Assets>,
) -> Result<Router, ApiError> {
    if !address.ip().is_loopback() || token.len() < 32 {
        return Err(ApiError::invalid());
    }
    Ok(routes(state(address, token, app, assets)))
}
fn one<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ApiError> {
    if headers.get_all(name).iter().count() > 1 {
        return Err(ApiError::invalid());
    }
    headers
        .get(name)
        .map(|v| v.to_str().map_err(|_| ApiError::invalid()))
        .transpose()
}
fn id() -> Result<RequestId, ApiError> {
    let s = auth::secret()?;
    let mut b = [0u8; 16];
    for (i, v) in b.iter_mut().enumerate() {
        *v = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| ApiError::internal())?;
    }
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    Ok(RequestId::from_bytes(b))
}
fn error(e: ApiError, id: RequestId) -> Response {
    let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status,axum::Json(json!({"request_id":id.to_string(),"error":{"code":e.code,"message":e.message,"details":e.details,"retryable":e.retryable,"request_id":id.to_string()}}))).into_response()
}
fn reply(r: Reply, id: RequestId) -> Result<Response, ApiError> {
    let value = json!({"request_id":id.to_string(),"context":r.context,"data":r.data});
    let bytes = serde_json::to_vec(&value).map_err(|_| ApiError::internal())?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(ApiError::new(
            413,
            "TF_API_RESPONSE_LIMIT",
            "Reduce the requested page or source range",
        ));
    }
    let mut response = (
        StatusCode::OK,
        [("content-type", "application/json")],
        bytes,
    )
        .into_response();
    if let Some(etag) = r.etag {
        response.headers_mut().insert(
            "etag",
            HeaderValue::from_str(&format!("\"{etag}\"")).map_err(|_| ApiError::internal())?,
        );
    }
    Ok(response)
}
async fn handle(State(s): State<Arc<Shared>>, request: axum::extract::Request) -> Response {
    let id = match id() {
        Ok(id) => id,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let result = dispatch(s, request, id).await;
    let mut response = result.unwrap_or_else(|e| error(e, id));
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; worker-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    if let Ok(value) = HeaderValue::from_str(&id.to_string()) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}
async fn dispatch(
    s: Arc<Shared>,
    request: axum::extract::Request,
    id: RequestId,
) -> Result<Response, ApiError> {
    let permit = s
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy())?;
    let (parts, body) = request.into_parts();
    let h = &parts.headers;
    if one(h, "host")? != Some(s.host.as_str()) {
        return Err(ApiError::new(
            403,
            "TF_API_HOST",
            "The request host is not this coordinator",
        ));
    }
    let origin = one(h, "origin")?;
    if origin.is_some_and(|v| v != s.origin)
        || one(h, "sec-fetch-site")?.is_some_and(|v| !matches!(v, "same-origin" | "none"))
    {
        return Err(ApiError::new(
            403,
            "TF_API_ORIGIN",
            "Cross-origin requests are not allowed",
        ));
    }
    let method = parts.method.as_str();
    let path = parts.uri.path();
    if !matches!(method, "GET" | "POST" | "PUT") {
        return Err(ApiError::new(
            405,
            "TF_API_METHOD",
            "This method is not supported",
        ));
    }
    if method == "GET"
        && let Some(asset) = s.assets.as_ref().and_then(|a| a.get(path))
    {
        if let Some(query) = parts.uri.query() {
            let view = query.strip_prefix("view=").filter(|_| path == "/");
            if !view.is_some_and(|id| id.parse::<RequestId>().is_ok()) {
                return Err(ApiError::invalid());
            }
        }
        return Ok(([("content-type", asset.0)], asset.1).into_response());
    }
    if method == "GET" && matches!(path, "/" | "/bootstrap.js" | "/health") {
        if parts.uri.query().is_some() {
            return Err(ApiError::invalid());
        }
        if path == "/health" {
            return reply(
                Reply::metadata(json!({"status":"ready","api_version":1})),
                id,
            );
        }
        return Ok(if path == "/" {
            ([( "content-type","text/html; charset=utf-8")],"<!doctype html><meta charset=utf-8><title>Transflow</title><h1>Transflow coordinator</h1><p id=status>Connecting…</p><script src=/bootstrap.js defer></script>").into_response()
        } else {
            (
                [("content-type", "text/javascript; charset=utf-8")],
                include_str!("bootstrap.js"),
            )
                .into_response()
        });
    }
    let bearer = one(h, "authorization")?
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|v| auth::same(v, &s.token));
    let exchange = method == "POST" && path == "/api/v1/sessions/exchange";
    if exchange && origin != Some(s.origin.as_str()) {
        return Err(ApiError::unauthorized());
    }
    if !bearer && !exchange {
        if origin != Some(s.origin.as_str()) {
            return Err(ApiError::unauthorized());
        }
        let cookies = one(h, "cookie")?.unwrap_or("");
        let mut selected = cookies
            .split(';')
            .map(str::trim)
            .filter_map(|c| c.strip_prefix("tf_session="));
        let cookie = selected.next().ok_or_else(ApiError::unauthorized)?;
        if selected.next().is_some()
            || !s.grants.lock().map_err(|_| ApiError::internal())?.validate(
                cookie,
                one(h, "x-transflow-csrf")?,
                method != "GET",
                Instant::now(),
            )
        {
            return Err(ApiError::unauthorized());
        }
    }
    let query = query_pairs(parts.uri.query().unwrap_or(""))?;
    let mut params = BTreeMap::new();
    for (k, v) in query {
        if matches!(k.as_str(), "token" | "code" | "access_token") || params.insert(k, v).is_some()
        {
            return Err(ApiError::invalid());
        }
    }
    let content = one(h, "content-type")?;
    if matches!(method, "POST" | "PUT") && content != Some("application/json") {
        return Err(ApiError::new(
            415,
            "TF_API_CONTENT_TYPE",
            "Send an application/json request body",
        ));
    }
    let bytes = tokio::time::timeout(Duration::from_secs(10), to_bytes(body, BODY_LIMIT))
        .await
        .map_err(|_| ApiError::invalid())?
        .map_err(|_| ApiError::new(413, "TF_API_BODY_LIMIT", "The request body exceeds 2 MiB"))?;
    let value = if method == "GET" {
        if !bytes.is_empty() {
            return Err(ApiError::invalid());
        }
        json!({})
    } else {
        tf_protocol::decode_json(&bytes, BODY_LIMIT).map_err(|_| ApiError::invalid())?
    };
    if !value.is_object() {
        return Err(ApiError::invalid());
    }
    if path.starts_with("/api/v1/sessions/") {
        if !params.is_empty() {
            return Err(ApiError::invalid());
        }
        if exchange {
            if value.as_object().is_none_or(|v| v.len() != 1) {
                return Err(ApiError::invalid());
            }
            let code = value["code"].as_str().ok_or_else(ApiError::invalid)?;
            let (cookie, csrf) = s
                .grants
                .lock()
                .map_err(|_| ApiError::internal())?
                .exchange(code, Instant::now())?;
            let mut r = reply(
                Reply::metadata(json!({"csrf":csrf,"expires_in_seconds":43200})),
                id,
            )?;
            r.headers_mut().insert(
                "set-cookie",
                HeaderValue::from_str(&format!(
                    "tf_session={cookie}; HttpOnly; SameSite=Strict; Path=/api/v1; Max-Age=43200"
                ))
                .map_err(|_| ApiError::internal())?,
            );
            return Ok(r);
        }
        if method != "POST" || value != json!({}) {
            return Err(ApiError::invalid());
        }
        return match path {
            "/api/v1/sessions/launch" if bearer => {
                let code = s
                    .grants
                    .lock()
                    .map_err(|_| ApiError::internal())?
                    .launch(Instant::now())?;
                reply(
                    Reply::metadata(json!({"code":code,"expires_in_seconds":60})),
                    id,
                )
            }
            "/api/v1/sessions/verify" => reply(Reply::metadata(json!({"authenticated":true})), id),
            _ => Err(ApiError::missing()),
        };
    }
    if path == "/api/v1/capabilities" && method == "GET" && params.is_empty() {
        return reply(Reply::metadata(capabilities(&s)), id);
    }
    if path == "/api/v1/openapi.json" && method == "GET" && params.is_empty() {
        return Ok((
            [("content-type", "application/json")],
            include_str!("../../../schemas/generated/openapi-v1.json"),
        )
            .into_response());
    }
    let key = one(h, "idempotency-key")?
        .map(|v| v.parse().map_err(|_| ApiError::invalid()))
        .transpose()?;
    let if_match = one(h, "if-match")?
        .map(|s| {
            let value = s
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or_else(ApiError::invalid)?;
            if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(ApiError::invalid());
            }
            Ok(value.to_owned())
        })
        .transpose()?;
    let mut request = Request {
        id,
        method: method.to_owned(),
        path: path.to_owned(),
        query: params,
        body: value,
        key,
        if_match,
    };
    let route = crate::contracts::route(&request.method, &request.path)?;
    crate::contracts::request(route, &request.body)?;
    if request.path == "/api/v1/read" {
        // Browser GETs usually omit Origin. This CSRF-protected read-only POST facade
        // preserves the mandatory Origin rule without exposing bearer credentials.
        request.path = request.body["path"]
            .as_str()
            .ok_or_else(ApiError::invalid)?
            .to_owned();
        request.query = serde_json::from_value(request.body["query"].clone())
            .map_err(|_| ApiError::invalid())?;
        request.method = "GET".into();
        request.body = json!({});
        request.key = None;
        request.if_match = None;
    }
    if request.method == "GET" && request.path == "/api/v1/capabilities" && request.query.is_empty()
    {
        return reply(Reply::metadata(capabilities(&s)), id);
    }
    let route = crate::contracts::route(&request.method, &request.path)?;
    if request.method == "GET" && request.path == "/api/v1/events" {
        if let Some(last) = one(h, "last-event-id")? {
            if request.query.get("after").is_some_and(|v| v != last) {
                return Err(ApiError::invalid());
            }
            request.query.insert("after".into(), last.into());
        }
        let cookie = one(h, "cookie")?
            .and_then(|v| {
                v.split(';')
                    .map(str::trim)
                    .find_map(|v| v.strip_prefix("tf_session="))
            })
            .map(str::to_owned);
        drop(permit);
        return event_stream(s, request, bearer, cookie).await;
    }
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let result = s.app.call(request)?;
        crate::contracts::response(route, &result.data)?;
        if let Some(c) = &result.context {
            crate::contracts::validate("ApiContextV1", c).map_err(|_| ApiError::internal())?;
        }
        Ok(result)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .and_then(|r| reply(r, id))
}

fn query_pairs(query: &str) -> Result<Vec<(String, String)>, ApiError> {
    fn decode(s: &str) -> Result<String, ApiError> {
        let mut out = Vec::with_capacity(s.len());
        let mut bytes = s.bytes();
        while let Some(b) = bytes.next() {
            match b {
                b'+' => out.push(b' '),
                b'%' => {
                    let a = bytes
                        .next()
                        .and_then(|c| (c as char).to_digit(16))
                        .ok_or_else(ApiError::invalid)?;
                    let b = bytes
                        .next()
                        .and_then(|c| (c as char).to_digit(16))
                        .ok_or_else(ApiError::invalid)?;
                    out.push((a * 16 + b) as u8);
                }
                b => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| ApiError::invalid())
    }
    if query.len() > 16384 {
        return Err(ApiError::invalid());
    }
    if query.is_empty() {
        return Ok(vec![]);
    }
    query
        .split('&')
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((decode(k)?, decode(v)?))
        })
        .collect()
}

async fn event_stream(
    s: Arc<Shared>,
    mut request: Request,
    bearer: bool,
    cookie: Option<String>,
) -> Result<Response, ApiError> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use std::convert::Infallible;
    let slot = s
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy())?;
    let app = s.app.clone();
    let first_request = request.clone();
    let first = tokio::task::spawn_blocking(move || app.call(first_request))
        .await
        .map_err(|_| ApiError::internal())??;
    crate::contracts::validate("ApiEventsV1", &first.data).map_err(|_| ApiError::internal())?;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(1);
    tokio::spawn(async move {
        let _slot = slot;
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut page = first;
        loop {
            if !bearer
                && !s.grants.lock().is_ok_and(|mut g| {
                    g.validate(cookie.as_deref().unwrap_or(""), None, false, Instant::now())
                })
            {
                break;
            }
            let resync = page.data["resync_required"] == true;
            let Some(cursor) = page.data["cursor"].as_str().map(str::to_owned) else {
                break;
            };
            let Some(events) = page.data["events"].as_array() else {
                break;
            };
            for fact in events {
                if Instant::now() >= deadline {
                    return;
                }
                let Some(seq) = fact["sequence"].as_str() else {
                    return;
                };
                let event = Event::default()
                    .event("fact")
                    .id(seq)
                    .data(fact.to_string());
                if !matches!(
                    tokio::time::timeout(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_secs(5)),
                        tx.send(Ok(event)),
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    return;
                }
            }
            let message = Event::default().event(if resync {"resync_required"}else{"checkpoint"}).id(&cursor).data(json!({"cursor":cursor,"workspace":s.app.capabilities()["workspace"],"action":if resync {"refetch_context_and_read_models"}else{"resume_after_cursor"}}).to_string());
            if !matches!(
                tokio::time::timeout(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(5)),
                    tx.send(Ok(message)),
                )
                .await,
                Ok(Ok(()))
            ) || resync
                || Instant::now() >= deadline
            {
                break;
            }
            request.query.insert("after".into(), cursor);
            if events.is_empty() {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if tx.is_closed() {
                break;
            }
            let Ok(permit) = s.permits.clone().try_acquire_owned() else {
                break;
            };
            let app = s.app.clone();
            let r = request.clone();
            let result = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                app.call(r)
            })
            .await;
            match result {
                Ok(Ok(p)) if crate::contracts::validate("ApiEventsV1", &p.data).is_ok() => page = p,
                _ => break,
            }
        }
    });
    Ok(Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response())
}

fn capabilities(s: &Shared) -> serde_json::Value {
    let mut value = s.app.capabilities();
    value["ui"] = json!(s.assets.is_some());
    value
}
