// SPDX-License-Identifier: AGPL-3.0-or-later

//! The gateway's routes, under `/api/v1/public/` (`docs/api.md#the-public-gateway`).

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use heron_core::Module;
use jiff::Timestamp;
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::WatchStream;
use tower_http::cors::{Any, CorsLayer};

use crate::Gateway;
use crate::actions::{self, Delivery};

/// The script of the shows' pages and small widgets.
const WIDGET_JS: &str = include_str!("widget.js");
/// Writes are small: a first name and a message at most.
const MAX_BODY: usize = 16 * 1024;
/// Cached reads kept at most; past it, the cache starts again.
const MAX_CACHED: usize = 2_000;
const PROOF_HEADER: &str = "x-heron-proof";

pub fn router(gateway: Arc<Gateway>) -> Router {
    Router::new()
        .route("/api/v1/public/challenge", get(challenge))
        .route(
            "/api/v1/public/stations/{code}/{name}",
            get(read).post(write_show),
        )
        .route(
            "/api/v1/public/stations/{code}/{module}/{action}",
            get(read_action).post(write_action),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY))
        // Any origin may read and ask; writes check theirs (`origin_allowed`).
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([header::CONTENT_TYPE, HeaderName::from_static(PROOF_HEADER)])
                .max_age(Duration::from_secs(3_600)),
        )
        .with_state(gateway)
}

/// An error, as the API writes them.
fn refuse(
    status: StatusCode,
    code: &str,
    reason: &'static str,
    message: impl Into<String>,
) -> Response {
    metrics::counter!("heron_public_refused_total", "reason" => reason).increment(1);
    (
        status,
        axum::Json(json!({"error": code, "message": message.into()})),
    )
        .into_response()
}

fn too_many(reason: &'static str, wait: Duration) -> Response {
    let mut response = refuse(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_requests",
        reason,
        "too many requests, try again later",
    );
    let seconds = wait.as_secs().max(1).to_string();
    if let Ok(value) = HeaderValue::from_str(&seconds) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

fn not_found(what: &str) -> Response {
    refuse(
        StatusCode::NOT_FOUND,
        "not_found",
        "invalid",
        format!("{what} not found"),
    )
}

fn json_with(value: Value, max_age: u32) -> Response {
    let cache = format!("public, max-age={max_age}");
    let mut response = axum::Json(value).into_response();
    if let Ok(value) = HeaderValue::from_str(&cache) {
        response.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    response
}

/// A show module of the path, by its key.
fn show_module(key: &str) -> Option<Module> {
    Module::from_key(key).filter(|module| module.is_show())
}

async fn challenge(
    State(gateway): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let address = gateway.client(&headers, peer.ip());
    if let Err(wait) = gateway.writes.hit(address, Instant::now()) {
        return too_many("address", wait);
    }
    match gateway.proofs.challenge(Timestamp::now().as_millisecond()) {
        Ok(challenge) => {
            let mut response = axum::Json(challenge).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error) => refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "unavailable",
            error.to_string(),
        ),
    }
}

async fn read(
    State(gateway): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((code, name)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    let address = gateway.client(&headers, peer.ip());
    if let Err(wait) = gateway.reads.hit(address, Instant::now()) {
        return too_many("address", wait);
    }
    if gateway.config.token.is_none() {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "unavailable",
            "the gateway has no token for the API",
        );
    }
    if !gateway.has_station(&code) {
        return not_found(&format!("station {code}"));
    }
    let now = Timestamp::now();
    let kind = if name.ends_with(".js") || name.ends_with(".html") {
        "script"
    } else if name == "events" {
        "stream"
    } else {
        "read"
    };
    metrics::counter!("heron_public_requests_total", "kind" => kind).increment(1);
    let ttl = match name.as_str() {
        "now-playing" | "queue" => Some(5),
        "history" => Some(15),
        _ => None,
    };
    if let Some(ttl) = ttl {
        let path = match query {
            Some(query) => format!("/api/v1/public/stations/{code}/{name}?{query}"),
            None => format!("/api/v1/public/stations/{code}/{name}"),
        };
        return gateway.cache.get(&gateway, path, ttl, false).await;
    }
    match name.as_str() {
        "events" => events(&gateway, &code, address),
        "shows" => {
            let shows = gateway
                .station(&code, now)
                .map(|station| station["shows"].clone())
                .unwrap_or_default();
            json_with(shows, 1)
        }
        _ => {
            if let Some(key) = name.strip_suffix(".js") {
                return script(&gateway, &code, key, now);
            }
            if let Some(key) = name.strip_suffix(".html") {
                return page(&gateway, &code, key, now);
            }
            match show_module(&name).and_then(|module| gateway.show(&code, module.key(), now)) {
                Some(show) => json_with(show, 1),
                None => not_found(&format!("show {name}")),
            }
        }
    }
}

/// A show's own read (the dedications' search), from the API with the
/// gateway token, cached per query.
async fn read_action(
    State(gateway): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((code, module, name)): Path<(String, String, String)>,
    Query(given): Query<Vec<(String, String)>>,
) -> Response {
    let address = gateway.client(&headers, peer.ip());
    if let Err(wait) = gateway.reads.hit(address, Instant::now()) {
        return too_many("address", wait);
    }
    if gateway.config.token.is_none() {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "unavailable",
            "the gateway has no token for the API",
        );
    }
    metrics::counter!("heron_public_requests_total", "kind" => "read").increment(1);
    let Some(show) =
        show_module(&module).and_then(|m| gateway.show(&code, m.key(), Timestamp::now()))
    else {
        return not_found(&format!("show {module}"));
    };
    let Some(read) = actions::find_read(actions::READS, &module, &name) else {
        return not_found(&format!("{module}/{name}"));
    };
    if read.open && show["open"] != true {
        return refuse(
            StatusCode::CONFLICT,
            "conflict",
            "closed",
            format!("{module} is closed"),
        );
    }
    let query = match read.query(&given) {
        Ok(query) => query,
        Err(message) => {
            return refuse(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid",
                "invalid",
                message,
            );
        }
    };
    let path = format!("/api/v1/gateway/stations/{code}/{module}/{name}?{query}");
    gateway.cache.get(&gateway, path, read.ttl, true).await
}

fn events(gateway: &Gateway, code: &str, address: std::net::IpAddr) -> Response {
    let Some(slot) = gateway.streams.open(address) else {
        return too_many("streams", Duration::from_secs(30));
    };
    let Some(receiver) = gateway.subscribe(code) else {
        return not_found(&format!("station {code}"));
    };
    let stream = WatchStream::new(receiver).map(move |state| {
        // The slot is freed when the widget goes.
        let _open = &slot;
        Ok::<_, Infallible>(Event::default().event("state").data(state.as_str()))
    });
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    // Reverse proxies must not hold the events back.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn script(gateway: &Gateway, code: &str, key: &str, now: Timestamp) -> Response {
    if show_module(key)
        .and_then(|module| gateway.show(code, module.key(), now))
        .is_none()
    {
        return not_found(&format!("show {key}"));
    }
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        WIDGET_JS,
    )
        .into_response()
}

fn page(gateway: &Gateway, code: &str, key: &str, now: Timestamp) -> Response {
    let Some(show) = show_module(key).and_then(|module| gateway.show(code, module.key(), now))
    else {
        return not_found(&format!("show {key}"));
    };
    let title = show["title"].as_str().unwrap_or(key);
    let escaped: String = title
        .chars()
        .map(|c| match c {
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '&' => "&amp;".to_owned(),
            '"' => "&quot;".to_owned(),
            other => other.to_string(),
        })
        .collect();
    let html = format!(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{escaped}</title>\n\
         <style>body{{margin:0;padding:16px;background:#f4f4f2;font-family:system-ui,sans-serif}}\
         main{{max-width:640px;margin:0 auto}}</style>\n</head>\n<body>\n<main>\n\
         <script src=\"{key}.js\" data-mode=\"page\" async></script>\n</main>\n</body>\n</html>\n"
    );
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        html,
    )
        .into_response()
}

async fn write_show(
    State(gateway): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((code, module)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    write(&gateway, peer, &headers, &code, &module, "", &body).await
}

async fn write_action(
    State(gateway): State<Arc<Gateway>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((code, module, action)): Path<(String, String, String)>,
    body: Bytes,
) -> Response {
    write(&gateway, peer, &headers, &code, &module, &action, &body).await
}

fn header_text(headers: &HeaderMap, name: impl header::AsHeaderName) -> &str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

/// A listener's write, checked in order
/// (`docs/listener-interaction.md#writes-checked-first`).
async fn write(
    gateway: &Gateway,
    peer: SocketAddr,
    headers: &HeaderMap,
    code: &str,
    module: &str,
    name: &str,
    body: &[u8],
) -> Response {
    metrics::counter!("heron_public_requests_total", "kind" => "write").increment(1);
    let address = gateway.client(headers, peer.ip());
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if !gateway.origin_allowed(origin) {
        return refuse(
            StatusCode::FORBIDDEN,
            "forbidden",
            "origin",
            "this website may not send to the station",
        );
    }
    let now = Instant::now();
    if let Err(wait) = gateway.writes.hit(address, now) {
        return too_many("address", wait);
    }
    let Some(token) = gateway.config.token.as_deref() else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "unavailable",
            "the gateway has no token for the API",
        );
    };
    // The show: on, with this write, and open.
    let at = Timestamp::now();
    let Some(show) = show_module(module).and_then(|module| gateway.show(code, module.key(), at))
    else {
        return not_found(&format!("show {module}"));
    };
    let Some(action) = actions::find(&gateway.actions, module, name).copied() else {
        return not_found(&format!("{module}/{name}"));
    };
    if show["open"] != true {
        return refuse(
            StatusCode::CONFLICT,
            "conflict",
            "closed",
            format!("{module} is closed"),
        );
    }
    if let Err(refused) = gateway.proofs.verify(
        headers
            .get(PROOF_HEADER)
            .and_then(|value| value.to_str().ok()),
        at.as_millisecond(),
    ) {
        return refuse(
            StatusCode::FORBIDDEN,
            "forbidden",
            "proof",
            format!("no valid proof of work ({refused:?})"),
        );
    }
    let fields = match serde_json::from_slice::<Value>(body)
        .map_err(|error| error.to_string())
        .and_then(|body| action.check(&body))
    {
        Ok(fields) => fields,
        Err(message) => {
            return refuse(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid",
                "invalid",
                message,
            );
        }
    };
    // Only what the show has open now: the track playing, for a like.
    if let Some(stale) = action.stale(&fields, &show["state"]) {
        return refuse(
            StatusCode::CONFLICT,
            "conflict",
            "stale",
            format!("{stale} is not open to writes any more"),
        );
    }
    let device = gateway.proofs.device(
        address,
        header_text(headers, header::USER_AGENT),
        header_text(headers, header::ACCEPT_LANGUAGE),
    );
    if action.per_device > 0 {
        // Until the opening range ends, a day for a show without hours.
        let lifetime = show["closes_at_unix_ms"]
            .as_i64()
            .map(|closes| {
                Duration::from_millis(u64::try_from(closes - at.as_millisecond()).unwrap_or(0))
            })
            .unwrap_or(Duration::from_secs(86_400));
        let key = format!("{code}/{}/{device}", action.path());
        if let Err(wait) = gateway.devices.hit(&key, action.per_device, now, lifetime) {
            return too_many("device", wait);
        }
    }
    let path = format!("{code}/{}", action.path());
    match action.delivery {
        Delivery::Batch => {
            gateway.add_to_batch(path, fields);
            (StatusCode::ACCEPTED, axum::Json(json!({"accepted": true}))).into_response()
        }
        Delivery::Latest => {
            gateway.keep_latest(path, &device, fields);
            (StatusCode::ACCEPTED, axum::Json(json!({"accepted": true}))).into_response()
        }
        Delivery::Forward => {
            if let Err(wait) = gateway.api_writes.hit(
                &(),
                gateway.config.writes_per_second,
                now,
                Duration::from_secs(1),
            ) {
                return too_many("api", wait);
            }
            forward(gateway, token, &path, &device, fields).await
        }
    }
}

/// Sends a write on to the API, and its answer back.
async fn forward(
    gateway: &Gateway,
    token: &str,
    path: &str,
    device: &str,
    fields: serde_json::Map<String, Value>,
) -> Response {
    let sent = gateway
        .http
        .post(gateway.api(&format!("/api/v1/gateway/stations/{path}")))
        .bearer_auth(token)
        .header("x-heron-device", device)
        .json(&fields)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%path, %error, "cannot send a write on");
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "unavailable",
                "the station cannot take it now, try again later",
            );
        }
    };
    metrics::counter!("heron_public_writes_total", "delivery" => "forward").increment(1);
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = response.bytes().await.unwrap_or_default();
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// A read of the API's public endpoints, kept for its time.
struct Cached {
    fetched: Instant,
    status: StatusCode,
    body: Bytes,
}

type Slot = Arc<tokio::sync::Mutex<Option<Cached>>>;

/// The API's public endpoints, read once per period whatever the number of
/// listeners asking: the first asks, the others wait for its answer.
#[derive(Default)]
pub struct ReadCache {
    slots: Mutex<HashMap<String, Slot>>,
}

impl ReadCache {
    /// `auth`: with the gateway token (a show's read).
    async fn get(&self, gateway: &Gateway, path: String, ttl: u32, auth: bool) -> Response {
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
            if slots.len() >= MAX_CACHED && !slots.contains_key(&path) {
                slots.clear();
            }
            Arc::clone(slots.entry(path.clone()).or_default())
        };
        let mut cached = slot.lock().await;
        let fresh = cached
            .as_ref()
            .is_some_and(|cached| cached.fetched.elapsed() < Duration::from_secs(u64::from(ttl)));
        if !fresh {
            let mut request = gateway.http.get(gateway.api(&path));
            if let (true, Some(token)) = (auth, gateway.config.token.as_deref()) {
                request = request.bearer_auth(token);
            }
            match request.send().await {
                Ok(response) => {
                    let status = StatusCode::from_u16(response.status().as_u16())
                        .unwrap_or(StatusCode::BAD_GATEWAY);
                    if let Ok(body) = response.bytes().await {
                        *cached = Some(Cached {
                            fetched: Instant::now(),
                            status,
                            body,
                        });
                    }
                }
                // What was last read, while the API does not answer.
                Err(error) => tracing::debug!(%path, %error, "cannot read the API"),
            }
        }
        match cached.as_ref() {
            Some(cached) => (
                cached.status,
                [
                    (header::CONTENT_TYPE, "application/json".to_owned()),
                    (header::CACHE_CONTROL, format!("public, max-age={ttl}")),
                ],
                cached.body.clone(),
            )
                .into_response(),
            None => refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "unavailable",
                "the station cannot be read now",
            ),
        }
    }

    /// Forgets reads nobody asked for in a while.
    pub fn prune(&self) {
        self.slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, slot| {
                slot.try_lock().map_or(true, |cached| {
                    cached
                        .as_ref()
                        .is_some_and(|cached| cached.fetched.elapsed() < Duration::from_secs(60))
                })
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::tests::{MESSAGE, VOTE};
    use crate::proof::{Proofs, solve};
    use crate::{Config, tests::state};
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    /// A show's read as the API got it: path, query, token.
    type ShowRead = (String, Option<String>, Option<String>);

    /// A pretend API: counts its calls, keeps what it receives.
    #[derive(Default)]
    struct FakeApi {
        reads: AtomicUsize,
        /// A show's reads: path and query, with the token sent.
        show_reads: Mutex<Vec<ShowRead>>,
        received: Mutex<Vec<(String, Option<String>, Value)>>,
    }

    async fn fake_api() -> (Arc<FakeApi>, String) {
        let api = Arc::new(FakeApi::default());
        let shared = Arc::clone(&api);
        let app = Router::new()
            .route(
                "/api/v1/public/stations/main/now-playing",
                get(move || {
                    let api = Arc::clone(&shared);
                    async move {
                        api.reads.fetch_add(1, Ordering::SeqCst);
                        axum::Json(json!({"now": null}))
                    }
                }),
            )
            .route(
                "/api/v1/gateway/stations/{*path}",
                get({
                    let api = Arc::clone(&api);
                    move |Path(path): Path<String>, RawQuery(query): RawQuery, headers: HeaderMap| {
                        let api = Arc::clone(&api);
                        async move {
                            let token = headers
                                .get(header::AUTHORIZATION)
                                .map(|value| value.to_str().unwrap().to_owned());
                            api.show_reads.lock().unwrap().push((path, query, token));
                            axum::Json(json!([{"track_id": 7, "artist": "Cassia Bloom", "title": "Neon Orchard"}]))
                        }
                    }
                })
                .post({
                    let api = Arc::clone(&api);
                    move |Path(path): Path<String>, headers: HeaderMap, body: axum::Json<Value>| {
                        let api = Arc::clone(&api);
                        async move {
                            let device = headers
                                .get("x-heron-device")
                                .map(|value| value.to_str().unwrap().to_owned());
                            api.received.lock().unwrap().push((path, device, body.0));
                            (StatusCode::CREATED, axum::Json(json!({"id": 1})))
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (api, url)
    }

    fn gateway(api_url: String, config: Config, bits: u8) -> Arc<Gateway> {
        let config = Config {
            api_url,
            token: Some("token".to_owned()),
            ..config
        };
        let gateway =
            Gateway::with_proofs(config, vec![MESSAGE, VOTE], Proofs::with_key([3; 32], bits))
                .unwrap();
        // Dedications open all week; the match is a show too.
        let mut open = state(
            json!([{"days": [1, 2, 3, 4, 5, 6, 7], "start_hour": 0, "end_hour": 24}]),
            0,
        );
        let mut matched = open.stations[0].shows[0].clone();
        matched.module = Module::Match;
        open.stations[0].shows.push(matched);
        gateway.apply(Some(open), Timestamp::now());
        gateway
    }

    async fn call(
        gateway: &Arc<Gateway>,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
        from: [u8; 4],
    ) -> (StatusCode, HeaderMap, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        let mut request = request.body(body).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((from, 40_000))));
        let response = router(Arc::clone(gateway)).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, headers, value)
    }

    const LISTENER: [u8; 4] = [198, 51, 100, 7];

    #[tokio::test]
    async fn reads_of_the_api_are_cached() {
        let (api, url) = fake_api().await;
        let gateway = gateway(url, Config::default(), 0);
        for _ in 0..20 {
            let (status, headers, body) = call(
                &gateway,
                Method::GET,
                "/api/v1/public/stations/main/now-playing",
                &[],
                None,
                LISTENER,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=5");
            assert_eq!(body, json!({"now": null}));
        }
        assert_eq!(
            api.reads.load(Ordering::SeqCst),
            1,
            "one read for twenty listeners"
        );
        let (status, _, _) = call(
            &gateway,
            Method::GET,
            "/api/v1/public/stations/other/now-playing",
            &[],
            None,
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn shows_their_script_and_page_are_served_when_on() {
        let (_, url) = fake_api().await;
        let gateway = gateway(url, Config::default(), 0);
        let (status, _, show) = call(
            &gateway,
            Method::GET,
            "/api/v1/public/stations/main/dedications",
            &[],
            None,
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(show["open"], true);
        assert_eq!(show["color"], "#c62828");
        let (status, headers, _) = call(
            &gateway,
            Method::GET,
            "/api/v1/public/stations/main/dedications.js",
            &[],
            None,
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/javascript")
        );
        let (status, _, page) = call(
            &gateway,
            Method::GET,
            "/api/v1/public/stations/main/dedications.html",
            &[],
            None,
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.as_str()
                .unwrap()
                .contains(r#"<script src="dedications.js" data-mode="page""#)
        );
        // Top 30 is off for the station: nothing of it.
        for path in ["top30", "top30.js", "top30.html", "medleys"] {
            let (status, _, _) = call(
                &gateway,
                Method::GET,
                &format!("/api/v1/public/stations/main/{path}"),
                &[],
                None,
                LISTENER,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
        let (status, _, shows) = call(
            &gateway,
            Method::GET,
            "/api/v1/public/stations/main/shows",
            &[],
            None,
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(shows.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_address_over_its_limit_gets_429() {
        let (_, url) = fake_api().await;
        let config = Config {
            reads_per_minute: 3,
            ..Config::default()
        };
        let gateway = gateway(url, config, 0);
        for _ in 0..3 {
            let (status, _, _) = call(
                &gateway,
                Method::GET,
                "/api/v1/public/stations/main/shows",
                &[],
                None,
                LISTENER,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, headers, body) = call(
            &gateway,
            Method::GET,
            "/api/v1/public/stations/main/shows",
            &[],
            None,
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["error"], "too_many_requests");
        assert!(
            headers[header::RETRY_AFTER]
                .to_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > 0
        );
        let (status, _, _) = call(
            &gateway,
            Method::GET,
            "/api/v1/public/stations/main/shows",
            &[],
            None,
            [203, 0, 113, 1],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "another listener");
    }

    async fn proof(gateway: &Arc<Gateway>, bits: u8) -> String {
        let (status, _, challenge) = call(
            gateway,
            Method::GET,
            "/api/v1/public/challenge",
            &[],
            None,
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let challenge = challenge["challenge"].as_str().unwrap().to_owned();
        format!("{challenge}:{}", solve(&challenge, bits))
    }

    #[tokio::test]
    async fn a_shows_reads_go_to_the_api_with_the_token_and_are_cached() {
        let (api, url) = fake_api().await;
        let gateway = gateway(url, Config::default(), 0);
        let uri = "/api/v1/public/stations/main/dedications/search?q=n%C3%A9on%20o";
        for _ in 0..3 {
            let (status, _, body) = call(&gateway, Method::GET, uri, &[], None, LISTENER).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body[0]["title"], "Neon Orchard");
        }
        {
            let reads = api.show_reads.lock().unwrap();
            assert_eq!(reads.len(), 1, "cached");
            assert_eq!(reads[0].0, "main/dedications/search");
            assert_eq!(reads[0].1.as_deref(), Some("q=n%C3%A9on%20o"));
            assert_eq!(reads[0].2.as_deref(), Some("Bearer token"));
        }
        for (uri, status) in [
            (
                "/api/v1/public/stations/main/dedications/search",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/public/stations/main/dedications/search?q=a&email=x",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/public/stations/main/dedications/other?q=a",
                StatusCode::NOT_FOUND,
            ),
            (
                "/api/v1/public/stations/main/likes/search?q=ab",
                StatusCode::NOT_FOUND,
            ),
        ] {
            let (got, _, body) = call(&gateway, Method::GET, uri, &[], None, LISTENER).await;
            assert_eq!(got, status, "{uri}: {body}");
        }
        assert_eq!(api.show_reads.lock().unwrap().len(), 1);

        // Closed: no search, without asking the API; the follow link still answers.
        gateway.apply(Some(state(json!([]), 0)), Timestamp::now());
        let (status, _, _) = call(&gateway, Method::GET, uri, &[], None, LISTENER).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let follow = format!(
            "/api/v1/public/stations/main/dedications/follow?id={}",
            "a".repeat(32)
        );
        let (status, _, _) = call(&gateway, Method::GET, &follow, &[], None, LISTENER).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(api.show_reads.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn writes_are_checked_then_sent_on() {
        let (api, url) = fake_api().await;
        let config = Config {
            allowed_origins: vec!["https://radio.example.org".to_owned()],
            ..Config::default()
        };
        let gateway = gateway(url, config, 8);
        let uri = "/api/v1/public/stations/main/dedications";
        let body = json!({"first_name": "Marc", "message": "Hello"});

        // Another website, no proof, a wrong field.
        let (status, _, refused) = call(
            &gateway,
            Method::POST,
            uri,
            &[("origin", "https://elsewhere.example")],
            Some(body.clone()),
            LISTENER,
        )
        .await;
        assert_eq!(
            (status, refused["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("forbidden"))
        );
        let (status, _, _) = call(
            &gateway,
            Method::POST,
            uri,
            &[],
            Some(body.clone()),
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "no proof");
        let proof_header = proof(&gateway, 8).await;
        let (status, _, refused) = call(
            &gateway,
            Method::POST,
            uri,
            &[(PROOF_HEADER, &proof_header)],
            Some(json!({"first_name": "Marcel-Antoine"})),
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");

        // Sent on, with the device's hash, and the API's answer back.
        let proof_header = proof(&gateway, 8).await;
        let headers = [
            (PROOF_HEADER, proof_header.as_str()),
            ("origin", "https://radio.example.org"),
            ("user-agent", "Firefox"),
        ];
        let (status, _, answer) = call(
            &gateway,
            Method::POST,
            uri,
            &headers,
            Some(body.clone()),
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{answer}");
        assert_eq!(answer, json!({"id": 1}));
        {
            let received = api.received.lock().unwrap();
            let (path, device, fields) = &received[0];
            assert_eq!(path, "main/dedications");
            assert_eq!(device.as_deref().map(str::len), Some(32));
            assert!(
                !device.as_deref().unwrap().contains("198.51"),
                "no address is sent"
            );
            assert_eq!(fields, &body);
        }

        // The same proof twice, then a second write from the same device.
        let (status, _, _) = call(
            &gateway,
            Method::POST,
            uri,
            &headers,
            Some(body.clone()),
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a challenge is used once");
        let proof_header = proof(&gateway, 8).await;
        let again = [
            (PROOF_HEADER, proof_header.as_str()),
            ("user-agent", "Firefox"),
        ];
        let (status, _, refused) = call(
            &gateway,
            Method::POST,
            uri,
            &again,
            Some(body.clone()),
            LISTENER,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "one per device: {refused}"
        );
        // Another device may.
        let proof_header = proof(&gateway, 8).await;
        let other = [
            (PROOF_HEADER, proof_header.as_str()),
            ("user-agent", "Chrome"),
        ];
        let (status, _, _) = call(&gateway, Method::POST, uri, &other, Some(body), LISTENER).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(api.received.lock().unwrap().len(), 2);

        // A write no show takes.
        let (status, _, _) = call(
            &gateway,
            Method::POST,
            "/api/v1/public/stations/main/dedications/other",
            &[],
            Some(json!({})),
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_api_gets_no_more_than_its_share_per_second() {
        let (api, url) = fake_api().await;
        let config = Config {
            writes_per_second: 2,
            writes_per_minute: 100,
            ..Config::default()
        };
        let gateway = gateway(url, config, 0);
        let mut statuses = Vec::new();
        for n in 0..5u8 {
            let agent = format!("browser {n}");
            let headers = [("user-agent", agent.as_str())];
            let (status, _, _) = call(
                &gateway,
                Method::POST,
                "/api/v1/public/stations/main/dedications",
                &headers,
                Some(json!({"first_name": "Zoë"})),
                LISTENER,
            )
            .await;
            statuses.push(status);
        }
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CREATED)
                .count(),
            2
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::TOO_MANY_REQUESTS)
                .count(),
            3
        );
        assert_eq!(api.received.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn votes_are_added_up_and_sent_in_one_batch() {
        let (api, url) = fake_api().await;
        let config = Config {
            writes_per_minute: 100,
            ..Config::default()
        };
        let gateway = gateway(url, config, 0);
        for choice in ["a", "b", "a", "a"] {
            let (status, _, _) = call(
                &gateway,
                Method::POST,
                "/api/v1/public/stations/main/match/vote",
                &[],
                Some(json!({"choice": choice})),
                LISTENER,
            )
            .await;
            assert_eq!(status, StatusCode::ACCEPTED);
        }
        assert!(
            api.received.lock().unwrap().is_empty(),
            "nothing before the batch"
        );
        gateway.flush().await;
        let received = api.received.lock().unwrap();
        assert_eq!(received.len(), 1);
        let (path, _, body) = &received[0];
        assert_eq!(path, "main/match/vote");
        let mut counts: Vec<(String, u64)> = body["counts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|count| {
                (
                    count["fields"]["choice"].as_str().unwrap().to_owned(),
                    count["count"].as_u64().unwrap(),
                )
            })
            .collect();
        counts.sort();
        assert_eq!(counts, [("a".to_owned(), 3), ("b".to_owned(), 1)]);
    }

    #[tokio::test]
    async fn likes_keep_each_devices_last_vote_on_the_track_playing() {
        let (api, url) = fake_api().await;
        let config = Config {
            api_url: url,
            token: Some("token".to_owned()),
            writes_per_minute: 100,
            ..Config::default()
        };
        let gateway = Gateway::with_proofs(
            config,
            vec![crate::actions::LIKE],
            Proofs::with_key([3; 32], 0),
        )
        .unwrap();
        // Likes have no hours: open all the time, on play 7.
        let on_air = |play: Value| {
            let mut open = state(json!([]), 0);
            let mut likes = open.stations[0].shows[0].clone();
            likes.module = Module::Likes;
            likes.state = json!({ "play_id": play });
            open.stations[0].shows = vec![likes];
            open
        };
        gateway.apply(Some(on_air(json!(7))), Timestamp::now());
        let vote = |body: Value, from: [u8; 4]| {
            let gateway = Arc::clone(&gateway);
            async move {
                call(
                    &gateway,
                    Method::POST,
                    "/api/v1/public/stations/main/likes/vote",
                    &[],
                    Some(body),
                    from,
                )
                .await
            }
        };
        let other = [203, 0, 113, 9];
        for (body, from) in [
            (json!({"play_id": 7, "vote": "like"}), LISTENER),
            // Changes its mind: only this one goes.
            (json!({"play_id": 7, "vote": "dislike"}), LISTENER),
            (json!({"play_id": 7, "vote": "like"}), other),
        ] {
            let (status, _, answer) = vote(body, from).await;
            assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
        }
        // A past track, a vote that is neither.
        let (status, _, _) = vote(json!({"play_id": 6, "vote": "like"}), LISTENER).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _, _) = vote(json!({"play_id": 7, "vote": "love"}), LISTENER).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        gateway.flush().await;
        {
            let received = api.received.lock().unwrap();
            assert_eq!(received.len(), 1);
            let (path, _, body) = &received[0];
            assert_eq!(path, "main/likes/vote");
            let mut writes: Vec<(String, String)> = body["writes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|write| {
                    assert_eq!(write["fields"]["play_id"], 7);
                    (
                        write["device"].as_str().unwrap().to_owned(),
                        write["fields"]["vote"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            writes.sort_by(|a, b| a.1.cmp(&b.1));
            assert_eq!(writes.len(), 2, "one per device");
            assert_ne!(writes[0].0, writes[1].0);
            assert_eq!(writes[0].1, "dislike");
            assert_eq!(writes[1].1, "like");
        }
        // A jingle: nothing to vote on.
        gateway.apply(Some(on_air(Value::Null)), Timestamp::now());
        let (status, _, _) = vote(json!({"play_id": 7, "vote": "like"}), LISTENER).await;
        assert_eq!(status, StatusCode::CONFLICT);
        gateway.flush().await;
        assert_eq!(api.received.lock().unwrap().len(), 1, "nothing more sent");
    }

    #[tokio::test]
    async fn a_closed_show_takes_nothing() {
        let (api, url) = fake_api().await;
        let gateway = gateway(url, Config::default(), 0);
        gateway.apply(Some(state(json!([]), 0)), Timestamp::now());
        let (status, _, refused) = call(
            &gateway,
            Method::POST,
            "/api/v1/public/stations/main/dedications",
            &[],
            Some(json!({"first_name": "Zoë"})),
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        let (status, _, _) = call(
            &gateway,
            Method::POST,
            "/api/v1/public/stations/main/match/vote",
            &[],
            Some(json!({"choice": "a"})),
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "the match is off now");
        assert!(api.received.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn event_streams_are_limited_per_address() {
        let (_, url) = fake_api().await;
        let config = Config {
            streams_per_address: 1,
            ..Config::default()
        };
        let gateway = gateway(url, config, 0);
        let request = || {
            let mut request = Request::get("/api/v1/public/stations/main/events")
                .body(Body::empty())
                .unwrap();
            request
                .extensions_mut()
                .insert(ConnectInfo(SocketAddr::from((LISTENER, 1))));
            request
        };
        let open = router(Arc::clone(&gateway))
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(open.status(), StatusCode::OK);
        assert_eq!(open.headers()[header::CONTENT_TYPE], "text/event-stream");
        let refused = router(Arc::clone(&gateway))
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        drop(open);
        assert_eq!(gateway.streams.total(), 0);
        let again = router(Arc::clone(&gateway))
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn without_a_token_nothing_is_served() {
        let gateway =
            Gateway::with_proofs(Config::default(), Vec::new(), Proofs::with_key([3; 32], 0))
                .unwrap();
        let (status, _, _) = call(
            &gateway,
            Method::GET,
            "/api/v1/public/stations/main/shows",
            &[],
            None,
            LISTENER,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
}
