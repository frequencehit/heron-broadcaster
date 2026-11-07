// SPDX-License-Identifier: AGPL-3.0-or-later

//! End to end: the API over HTTP and WebSocket, a scratch MariaDB database
//! (`HERON_TEST_DATABASE_URL`) and a real engine playing a small library in
//! real time, in process. Skipped without a test database.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use heron_engine::config::EngineConfig;
use heron_engine::engine::{Engine, EngineOptions};
use heron_engine::grpc::EngineService;
use heron_service::db::{DatabaseHealth, ScratchDatabase};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_tungstenite::tungstenite;

use crate::app::{self, AppState};
use crate::auth::{Passwords, Role};
use crate::library::Library;
use crate::{engine, stations, users, webhooks};

/// Tracks short enough to see several play during a test.
const TRACK_SECONDS: f64 = 3.0;

struct Station {
    db: ScratchDatabase,
    library: TempDir,
    _state_dir: TempDir,
    staging: TempDir,
    _backups: TempDir,
    /// The covers folder.
    covers: TempDir,
    _engine: Arc<Engine>,
    state: AppState,
    api: SocketAddr,
    tracks: Vec<u64>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

fn write_wav(path: &Path, seconds: f64, freq: f64) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..(seconds * 44_100.0) as u64 {
        let t = n as f64 / 44_100.0;
        let value = (t * freq * std::f64::consts::TAU).sin() * 0.3 * f64::from(i16::MAX);
        writer.write_sample(value as i16).unwrap();
        writer.write_sample(value as i16).unwrap();
    }
    writer.finalize().unwrap();
}

/// An engine in this process, reached over gRPC on a free port, with its
/// own state directory: its configuration saved before it starts, as `ctl`
/// left it.
async fn start_engine(
    library: &Path,
    config: Option<EngineConfig>,
) -> (Arc<Engine>, TempDir, String, tokio::task::JoinHandle<()>) {
    let state_dir = tempfile::tempdir().unwrap();
    if let Some(config) = config {
        config.save(state_dir.path()).unwrap();
    }
    let engine = Engine::start(
        EngineOptions::new(library.to_path_buf(), state_dir.path().to_path_buf()),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let grpc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_addr = grpc.local_addr().unwrap();
    let service = EngineService::server(Arc::clone(&engine));
    let task = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(TcpListenerStream::new(grpc))
            .await;
    });
    (engine, state_dir, format!("http://{grpc_addr}"), task)
}

impl Station {
    async fn start() -> Option<Self> {
        Self::start_with(None).await
    }

    /// With the engine's configuration saved before it starts, as `ctl` left it.
    async fn start_with(engine_config: Option<EngineConfig>) -> Option<Self> {
        let db = ScratchDatabase::create().await?;
        let pool = db.pool.clone();

        // A library of analyzed tracks, as the scanner and analyzer leave it.
        let library = tempfile::tempdir().unwrap();
        let mut tracks = Vec::new();
        for n in 1..=5 {
            let path = format!("Artist {n}/Track {n}.wav");
            std::fs::create_dir_all(library.path().join(format!("Artist {n}"))).unwrap();
            write_wav(
                &library.path().join(&path),
                TRACK_SECONDS,
                220.0 * f64::from(n),
            );
            let id = sqlx::query(
                "INSERT INTO tracks (path, title, artist, album, duration_ms) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&path)
            .bind(format!("Track {n}"))
            .bind(format!("Artist {n}"))
            .bind("Album")
            .bind(3_000u32)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_id();
            sqlx::query(
                "INSERT INTO track_analysis (track_id, analysis_version, params_hash, cue_in_ms, cue_out_ms,
                                             integrated_lufs, true_peak_db)
                 VALUES (?, 1, ?, 0, 3000, -13.0, -7.0)",
            )
            .bind(id)
            .bind([0u8; 32].as_slice())
            .execute(&pool)
            .await
            .unwrap();
            tracks.push(id);
        }

        let staging = tempfile::tempdir().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let covers = tempfile::tempdir().unwrap();
        let cover_files = heron_library::covers::Files::new(covers.path());
        let (engine, state_dir, grpc_url, grpc_task) =
            start_engine(library.path(), engine_config).await;
        let mut tasks = vec![grpc_task];

        let webhooks = webhooks::Webhooks::with_backoff(
            pool.clone(),
            webhooks::Backoff(vec![Duration::from_millis(200); 2]),
        )
        .unwrap();
        let stations = stations::Stations::start(
            pool.clone(),
            grpc_url,
            std::collections::BTreeMap::new(),
            Some("Test".to_owned()),
            Arc::clone(&webhooks),
        )
        .await
        .unwrap();
        tasks.push(tokio::spawn(Arc::clone(&webhooks).send_loop()));

        // A fixed reading, so a test can tell who receives it.
        let (_, system) = tokio::sync::watch::channel(Some(crate::system::reading(
            12.0,
            1,
            4,
            [0.5, 0.4, 0.3],
            2,
        )));
        let state = AppState {
            pool: pool.clone(),
            health: DatabaseHealth(pool.clone()),
            library: Arc::new(Library::new(
                pool.clone(),
                library.path().to_path_buf(),
                cover_files.clone(),
            )),
            stations,
            passwords: Arc::new(Passwords::default()),
            staging: Arc::new(staging.path().to_path_buf()),
            webhooks,
            // Driven by the tests, minute by minute.
            listeners: crate::listeners::Collector::new(pool.clone()).unwrap(),
            backups: crate::backups::Backups::new(
                backups.path().to_path_buf(),
                db.url(),
                cover_files,
            ),
            system,
            about: Arc::new(crate::about::Own::new(vec![
                heron_service::about::ConfigItem::secret("HERON_DATABASE_URL", Some(&db.url())),
                heron_service::about::ConfigItem::flag("HERON_LIBRARY_WATCH", true),
            ])),
            two_factor: Default::default(),
        };
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = http.local_addr().unwrap();
        let router = app::router(state.clone());
        tasks.push(tokio::spawn(async move {
            let _ = axum::serve(http, router).await;
        }));

        for (name, role) in [("admin", Role::Admin), ("viewer", Role::Viewer)] {
            users::add_user(&pool, &state.passwords, name, "correct horse", role)
                .await
                .unwrap();
        }
        Some(Self {
            db,
            library,
            _state_dir: state_dir,
            staging,
            _backups: backups,
            covers,
            _engine: engine,
            state,
            api,
            tracks,
            tasks,
        })
    }

    fn client(&self) -> Client {
        Client {
            http: reqwest::Client::new(),
            base: format!("http://{}/api/v1", self.api),
            cookie: None,
            bearer: None,
        }
    }

    async fn signed_in(&self, username: &str) -> Client {
        let mut client = self.client();
        let response = client
            .http
            .post(format!("{}/auth/login", client.base))
            .json(&json!({"username": username, "password": "correct horse"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("HttpOnly"));
        client.cookie = Some(cookie.split(';').next().unwrap().to_owned());
        client
    }

    async fn stop(self) {
        for task in self.tasks {
            task.abort();
        }
        self.db.drop().await;
    }
}

struct Client {
    http: reqwest::Client,
    base: String,
    cookie: Option<String>,
    bearer: Option<String>,
}

impl Client {
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(cookie) = &self.cookie {
            request = request
                .header("cookie", cookie)
                .header("x-requested-with", "test");
        }
        if let Some(token) = &self.bearer {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        let value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, value)
    }

    async fn get(&self, path: &str) -> Value {
        let (status, value) = self.request(Method::GET, path, None).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {value}");
        value
    }

    async fn send(&self, method: Method, path: &str, body: Value) -> (StatusCode, Value) {
        self.request(method, path, Some(body)).await
    }

    /// Sends a chunk of an upload.
    async fn put_bytes(&self, path: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
        let mut request = self
            .http
            .put(format!("{}{path}", self.base))
            .header("content-type", "application/octet-stream")
            .body(bytes);
        if let Some(cookie) = &self.cookie {
            request = request
                .header("cookie", cookie)
                .header("x-requested-with", "test");
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }
}

impl Client {
    /// A download, as it is.
    async fn get_bytes(&self, path: &str) -> Vec<u8> {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if let Some(cookie) = &self.cookie {
            request = request.header("cookie", cookie);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "GET {path}");
        response.bytes().await.unwrap().to_vec()
    }

    /// Uploads a file, with a backup key in its header.
    async fn post_bytes(
        &self,
        path: &str,
        bytes: Vec<u8>,
        key: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header("content-type", "application/octet-stream")
            .body(bytes);
        if let Some(cookie) = &self.cookie {
            request = request
                .header("cookie", cookie)
                .header("x-requested-with", "test");
        }
        if let Some(key) = key {
            request = request.header("x-heron-backup-key", key);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }
}

/// Polls `check` until it returns something, for at most `timeout`.
async fn eventually<T, F: Future<Output = Option<T>>>(
    what: &str,
    timeout: Duration,
    mut check: impl FnMut() -> F,
) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = check().await {
            return value;
        }
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn waiting_tracks(queue: &Value) -> Vec<u64> {
    queue
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["track"]["track_id"].as_u64().unwrap())
        .collect()
}

/// The About page: the build for everyone, the rest for admins, and a secret
/// never shown.
#[tokio::test(flavor = "multi_thread")]
async fn about_page_roles_and_secrets() {
    let Some(station) = Station::start().await else {
        return;
    };
    let anonymous = station.client();
    let (status, _) = anonymous.request(Method::GET, "/about", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let viewer = station.signed_in("viewer").await;
    let about = viewer.get("/about").await;
    let api = &about["services"][0];
    assert_eq!(api["service"], "heron-api");
    assert_eq!(api["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(api["running"], true);
    assert!(about["runtime"].is_null());
    assert!(about["dependencies"].is_null());
    assert!(about["configuration"].is_null());

    // The workers have not been seen: listed as not running.
    let worker = |about: &Value, name: &str| {
        about["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|service| service["service"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(worker(&about, "heron-analyzer")["running"], false);
    assert!(worker(&about, "heron-analyzer")["version"].is_null());

    // A recent heartbeat is a worker running; an old one, a worker stopped.
    for (service, seen) in [("heron-analyzer", "0"), ("heron-librarian", "600")] {
        sqlx::query(
            "INSERT INTO service_heartbeats
                 (service, instance, version, started_at, seen_at, config)
             VALUES (?, 'box', '0.2.0', UTC_TIMESTAMP(6) - INTERVAL 1 HOUR,
                     UTC_TIMESTAMP(6) - INTERVAL ? SECOND,
                     '[{\"name\":\"HERON_NICE\",\"kind\":\"text\",\"value\":\"10\"}]')",
        )
        .bind(service)
        .bind(seen)
        .execute(&station.state.pool)
        .await
        .unwrap();
    }
    let about = viewer.get("/about").await;
    let analyzer = worker(&about, "heron-analyzer");
    assert_eq!(analyzer["running"], true);
    assert_eq!(analyzer["version"], "0.2.0");
    assert_eq!(analyzer["instance"], "box");
    assert_eq!(worker(&about, "heron-librarian")["running"], false);
    assert_eq!(worker(&about, "heron-librarian")["version"], "0.2.0");

    // The stopped ones are taken off the list by an admin, the running ones stay.
    let (status, _) = viewer.request(Method::DELETE, "/about/stopped", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let admin = station.signed_in("admin").await;
    let (status, cleared) = admin.request(Method::DELETE, "/about/stopped", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["removed"], 1);
    let about = viewer.get("/about").await;
    assert_eq!(worker(&about, "heron-analyzer")["running"], true);
    assert!(worker(&about, "heron-librarian")["version"].is_null());

    let about = admin.get("/about").await;
    assert!(about["runtime"]["cpu_cores"].as_u64().unwrap() >= 1);
    assert!(about["dependencies"]["database"]["version"].is_string());
    assert!(
        about["dependencies"]["database"]["migrations"]
            .as_i64()
            .unwrap()
            > 0
    );
    let api_config = about["configuration"][0]["items"].as_array().unwrap();
    let url = api_config
        .iter()
        .find(|item| item["name"] == "HERON_DATABASE_URL")
        .unwrap();
    assert_eq!(url["kind"], "secret");
    assert_eq!(url["value"], true);
    // The secret itself is nowhere in the answer.
    assert!(!about.to_string().contains(&station.db.url()));
    assert!(!about.to_string().contains("mysql://"));
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_and_roles() {
    let Some(station) = Station::start().await else {
        return;
    };
    let anonymous = station.client();
    // Public.
    let status = anonymous.get("/status").await;
    assert_eq!(status["database"], "ok");
    assert_eq!(anonymous.get("/library/status").await["tracks"], 5);
    let openapi = anonymous.get("/openapi.json").await;
    assert!(openapi["paths"]["/api/v1/queue"]["post"].is_object());
    assert!(openapi["components"]["schemas"]["LiveMessage"].is_object());

    // Everything else needs a session or a token.
    let (status, body) = anonymous.request(Method::GET, "/queue", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
    let (status, _) = anonymous
        .send(
            Method::POST,
            "/auth/login",
            json!({"username": "admin", "password": "wrong password"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let viewer = station.signed_in("viewer").await;
    assert_eq!(viewer.get("/auth/me").await["role"], "viewer");
    let (status, _) = viewer.request(Method::POST, "/player/skip", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = viewer.request(Method::GET, "/users", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A cookie alone cannot change anything (cross-site requests).
    let admin = station.signed_in("admin").await;
    let response = admin
        .http
        .post(format!("{}/player/skip", admin.base))
        .header("cookie", admin.cookie.as_deref().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // API tokens act as their user.
    let (status, token) = admin
        .send(Method::POST, "/tokens", json!({"name": "automation"}))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let mut scripted = station.client();
    scripted.bearer = Some(token["token"].as_str().unwrap().to_owned());
    assert_eq!(scripted.get("/auth/me").await["username"], "admin");
    let (status, _) = admin
        .request(Method::DELETE, &format!("/tokens/{}", token["id"]), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = scripted.request(Method::GET, "/auth/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The last admin stays an admin.
    let me = admin.get("/auth/me").await;
    let (status, _) = admin
        .send(
            Method::PATCH,
            &format!("/users/{}", me["id"]),
            json!({"role": "viewer"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Signing out ends the session.
    let (status, _) = admin.request(Method::POST, "/auth/logout", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin.request(Method::GET, "/auth/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_manager_feeds_the_engine_and_records_history() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;

    // Autofill keeps three tracks waiting, and something goes on air.
    let queue = eventually("autofill", Duration::from_secs(10), || async {
        let queue = admin.get("/queue").await;
        (queue.as_array().unwrap().len() >= 3).then_some(queue)
    })
    .await;
    assert!(
        queue.as_array().unwrap()[0]["track"]["source"] == "rotation",
        "{queue}"
    );
    let player = eventually("a track on air", Duration::from_secs(10), || async {
        let player = admin.get("/player").await;
        let on_air = player["decks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|deck| deck["on_air"] == true);
        on_air.then_some(player)
    })
    .await;
    assert_eq!(player["paused"], false);

    // A track queued next, then moved and removed.
    let manual = station.tracks[4];
    let (status, queued) = admin
        .send(
            Method::POST,
            "/queue",
            json!({"track_ids": [manual], "position": 0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{queued}");
    let entry = queued["entry_ids"][0].as_u64().unwrap();
    let waiting = admin.get("/queue").await;
    assert_eq!(waiting[0]["entry_id"], entry);
    assert_eq!(waiting[0]["track"]["source"], "manual");
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/queue/{entry}/position"),
            json!({"position": 2}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let waiting = admin.get("/queue").await;
    assert_eq!(waiting[2]["entry_id"], entry, "{waiting}");
    let (status, _) = admin
        .request(Method::DELETE, &format!("/queue/{entry}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!waiting_tracks(&admin.get("/queue").await).is_empty());

    // Missing files are refused.
    sqlx::query("UPDATE tracks SET missing_since = NOW(6) WHERE id = ?")
        .bind(manual)
        .execute(&station.db.pool)
        .await
        .unwrap();
    let (status, body) = admin
        .send(Method::POST, "/queue", json!({"track_ids": [manual]}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // The live socket reports track changes.
    let url = format!("ws://{}/api/v1/live?levels=false", station.api);
    let mut request = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    request
        .headers_mut()
        .insert("cookie", admin.cookie.as_deref().unwrap().parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let mut next = async || -> Value {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("live message")
            .unwrap()
            .unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    };
    let first = next().await;
    assert_eq!(first["type"], "state");
    assert!(first["player"]["decks"].is_array());
    // Operators and admins get the machine's load right after the state.
    let system = next().await;
    assert_eq!(system["type"], "system", "{system}");
    assert_eq!(system["cpu_percent"], 12.0);
    assert_eq!(system["mem_percent"], 25.0);
    assert_eq!(system["cores"], 2);
    assert_eq!(system["load_avg"].as_array().unwrap().len(), 3);

    // Viewers never do.
    let viewer = station.signed_in("viewer").await;
    let url = format!("ws://{}/api/v1/live?levels=false", station.api);
    let mut request = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    request
        .headers_mut()
        .insert("cookie", viewer.cookie.as_deref().unwrap().parse().unwrap());
    let (mut viewer_socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    // Positions keep coming, so listen for a fixed time.
    let mut types = Vec::new();
    let until = tokio::time::Instant::now() + Duration::from_millis(1500);
    while let Ok(Some(Ok(message))) = tokio::time::timeout_at(until, viewer_socket.next()).await {
        let message: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        types.push(message["type"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        types.first().map(String::as_str),
        Some("state"),
        "{types:?}"
    );
    assert!(!types.iter().any(|kind| kind == "system"), "{types:?}");

    // Skip: the play ends as skipped in the history, the next one starts.
    let (status, _) = admin.request(Method::POST, "/player/skip", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let mut saw_event = false;
    for _ in 0..50 {
        let message = next().await;
        if message["type"] == "track" {
            saw_event = true;
            break;
        }
    }
    assert!(saw_event, "no track event on the live socket");
    let history = eventually("a skipped play", Duration::from_secs(10), || async {
        let history = admin.get("/history").await;
        let skipped = history
            .as_array()
            .unwrap()
            .iter()
            .any(|play| play["end_reason"] == "skipped");
        skipped.then_some(history)
    })
    .await;
    let play = &history[0];
    assert!(
        play["track_id"].is_u64() && play["artist"].is_string(),
        "{play}"
    );
    assert_eq!(play["source"], "rotation");

    // Every play that ended is complete.
    eventually("finished plays", Duration::from_secs(10), || async {
        let history = admin.get("/history").await;
        history
            .as_array()
            .unwrap()
            .iter()
            .any(|play| {
                play["end_reason"] == "finished" && play["played_ms"].as_u64() > Some(2_000)
            })
            .then_some(())
    })
    .await;

    // Autofill off: the queue drains.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/queue",
            json!({"autofill": false, "min_queued": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(waiting_tracks(&admin.get("/queue").await).is_empty());
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_and_outputs_reach_the_engine() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    eventually("the engine", Duration::from_secs(10), || async {
        let engine = admin.get("/engine").await;
        (engine["config"]["state"] == "synced").then_some(())
    })
    .await;

    // Playout settings.
    let mut playout = admin.get("/settings/playout").await;
    playout["crossfade"]["min_track_ms"] = json!(30_000);
    let (status, body) = admin.send(Method::PUT, "/settings/playout", playout).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    eventually(
        "playout settings in the engine",
        Duration::from_secs(5),
        || async {
            let config = station.state.stations.first().engine.config().await.ok()?;
            config
                .playout_settings_json
                .contains("\"min_track_ms\":30000")
                .then_some(())
        },
    )
    .await;

    // The external FFmpeg encoder: options checked, never a command line.
    let (status, body) = admin
        .send(
            Method::POST,
            "/encoder-profiles",
            json!({"name": "AAC+", "encoder": "external_ffmpeg", "bitrate_kbps": 48,
                   "sample_rate": 44100, "channels": 2,
                   "options": {"codec": "libfdk_aac -y /tmp/x", "profile": "aac_he_v2"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, aac) = admin
        .send(
            Method::POST,
            "/encoder-profiles",
            json!({"name": "AAC+", "encoder": "external_ffmpeg", "bitrate_kbps": 48,
                   "sample_rate": 44100, "channels": 2,
                   "options": {"codec": "libfdk_aac", "profile": "aac_he_v2"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{aac}");
    assert_eq!(aac["options"]["codec"], "libfdk_aac");
    let (status, _) = admin
        .request(
            Method::DELETE,
            &format!("/encoder-profiles/{}", aac["id"]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A stream server: its passwords never shown.
    let profiles = admin.get("/encoder-profiles").await;
    let profile = profiles[0]["id"].as_u64().unwrap();
    let (status, body) = admin
        .send(
            Method::POST,
            "/stream-servers",
            json!({"name": "No password", "host": "127.0.0.1"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, server) = admin
        .send(
            Method::POST,
            "/stream-servers",
            json!({"name": "Local", "host": "127.0.0.1", "port": 9, "source_password": "hackme",
                   "admin_password": "adm1n-pw"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{server}");
    assert_eq!(server["source_password_set"], true);
    assert_eq!(server["admin_password_set"], true);
    assert!(!server.to_string().contains("hackme") && !server.to_string().contains("adm1n-pw"));
    let server_id = server["id"].as_u64().unwrap();

    // An output: validated, its own settings only, pushed in full.
    let (status, body) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Bad", "kind": "icecast", "encoder_profile_id": profile,
                   "server_id": server_id, "config": {"mount": "live"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Inline", "kind": "icecast", "encoder_profile_id": profile,
                   "server_id": server_id,
                   "config": {"host": "127.0.0.1", "mount": "/live", "password": "x"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "No server", "kind": "icecast", "encoder_profile_id": profile,
                   "config": {"mount": "/live"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, details) = admin
        .send(
            Method::PUT,
            "/settings/stream-details",
            json!({"name": "My station", "genre": "Pop", "public": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{details}");
    let (status, output) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile,
                   "server_id": server_id, "config": {"mount": "/live"},
                   "details": {"name": "My station HD"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{output}");
    assert_eq!(output["config"], json!({"mount": "/live"}));
    assert_eq!(output["details"], json!({"name": "My station HD"}));
    assert_eq!(output["listen_url"], "http://127.0.0.1:9/live");
    let id = output["id"].as_u64().unwrap();
    let engine_output = |json: &str, id: u64| {
        let config: heron_core::OutputsConfig = serde_json::from_str(json).unwrap();
        config.outputs.into_iter().find(|o| o.id == id)
    };
    let pushed = eventually(
        "the output in the engine",
        Duration::from_secs(10),
        || async {
            let config = station.state.stations.first().engine.config().await.ok()?;
            engine_output(&config.outputs_json, id)
        },
    )
    .await;
    assert_eq!(
        pushed.config,
        json!({"host": "127.0.0.1", "port": 9, "mount": "/live", "username": "source",
               "password": "hackme", "name": "My station HD", "genre": "Pop", "public": true})
    );

    // The station details: an output follows those it does not override.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/stream-details",
            json!({"name": "My station", "genre": "Hits"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    eventually("the new genre", Duration::from_secs(10), || async {
        let config = station.state.stations.first().engine.config().await.ok()?;
        let output = engine_output(&config.outputs_json, id)?;
        (output.config["genre"] == "Hits"
            && output.config["name"] == "My station HD"
            && output.config["public"] == false)
            .then_some(())
    })
    .await;

    // Changing the server's password pushes its outputs again; its admin
    // credentials and public address never reach the engine.
    let (status, changed) = admin
        .send(
            Method::PUT,
            &format!("/stream-servers/{server_id}"),
            json!({"name": "Local", "host": "127.0.0.1", "port": 9, "source_password": "n3w-pw",
                   "public_url": "https://stream.example.org/"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["admin_password_set"], true);
    assert_eq!(changed["outputs"], json!([{"id": id, "name": "Icecast"}]));
    let pushed = eventually(
        "the new password in the engine",
        Duration::from_secs(10),
        || async {
            let config = station.state.stations.first().engine.config().await.ok()?;
            config
                .outputs_json
                .contains("n3w-pw")
                .then_some(config.outputs_json)
        },
    )
    .await;
    assert!(!pushed.contains("hackme") && !pushed.contains("adm1n-pw"));
    let outputs = eventually("the output's state", Duration::from_secs(10), || async {
        let outputs = admin.get("/outputs").await;
        outputs[0]["status"].is_object().then_some(outputs)
    })
    .await;
    assert_eq!(outputs[0]["listen_url"], "https://stream.example.org/live");
    assert_ne!(outputs[0]["status"]["state"], "connected");

    // Duplicate: a disabled copy on another mount.
    let (status, copy) = admin
        .request(Method::POST, &format!("/outputs/{id}/duplicate"), None)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{copy}");
    assert_eq!(copy["name"], "Icecast (copy)");
    assert_eq!(copy["config"], json!({"mount": "/live-copy"}));
    assert_eq!(copy["enabled"], false);
    assert_eq!(copy["server_id"], server_id);
    assert_eq!(copy["details"], json!({"name": "My station HD"}));
    let (_, second) = admin
        .request(Method::POST, &format!("/outputs/{id}/duplicate"), None)
        .await;
    assert_eq!(second["name"], "Icecast (copy 2)");
    assert_eq!(second["config"], json!({"mount": "/live-copy-2"}));

    // A relay: read for the statistics, no output can pick it.
    let (status, relay) = admin
        .send(
            Method::POST,
            "/stream-servers",
            json!({"name": "Relay", "host": "127.0.0.2", "relay_of": server_id, "tls": true,
                   "relay_mounts": [{"mount": "/live", "relay_mount": "/relay"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{relay}");
    let relay_id = relay["id"].as_u64().unwrap();
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/outputs/{}", copy["id"]),
            json!({"name": "Copy", "kind": "icecast", "encoder_profile_id": profile,
                   "server_id": relay_id, "config": {"mount": "/live-copy"}, "enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/stream-servers",
            json!({"name": "TLS", "host": "127.0.0.3", "tls": true, "source_password": "x"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // A server in use cannot be deleted: the answer names what uses it.
    let (status, body) = admin
        .request(
            Method::DELETE,
            &format!("/stream-servers/{server_id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("Icecast (copy)"),
        "{body}"
    );
    for output in [&copy, &second] {
        let (status, _) = admin
            .request(Method::DELETE, &format!("/outputs/{}", output["id"]), None)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    // A profile in use cannot be deleted.
    let (status, _) = admin
        .request(
            Method::DELETE,
            &format!("/encoder-profiles/{profile}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = admin
        .request(Method::DELETE, &format!("/outputs/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    eventually(
        "the output gone from the engine",
        Duration::from_secs(10),
        || async {
            let config = station.state.stations.first().engine.config().await.ok()?;
            (!config.outputs_json.contains("n3w-pw")).then_some(())
        },
    )
    .await;
    // The master still has its relay.
    let (status, body) = admin
        .request(
            Method::DELETE,
            &format!("/stream-servers/{server_id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    for server in [relay_id, server_id] {
        let (status, _) = admin
            .request(Method::DELETE, &format!("/stream-servers/{server}"), None)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    // Library settings never show the AcoustID key.
    let (status, library) = admin
        .send(
            Method::PATCH,
            "/settings/library",
            json!({"acoustid_key": "s3cret"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(library["acoustid_key_set"], true);
    assert!(!library.to_string().contains("s3cret"));
    let (status, _) = admin
        .send(
            Method::PATCH,
            "/settings/library",
            json!({"layout": "{nope}"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn listener_statistics_through_the_api() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    eventually("the engine", Duration::from_secs(10), || async {
        let engine = admin.get("/engine").await;
        (engine["config"]["state"] == "synced").then_some(())
    })
    .await;

    // The stream server's admin credentials: never shown, never pushed.
    let profile = admin.get("/encoder-profiles").await[0]["id"]
        .as_u64()
        .unwrap();
    let (status, server) = admin
        .send(
            Method::POST,
            "/stream-servers",
            json!({"name": "Local", "host": "127.0.0.1", "port": 9, "source_password": "hackme",
                   "admin_password": "adm1n-pw"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{server}");
    assert_eq!(server["admin_password_set"], true);
    assert!(!server.to_string().contains("adm1n-pw"));
    let server_id = server["id"].as_u64().unwrap();
    let (status, output) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile,
                   "server_id": server_id, "config": {"mount": "/live"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{output}");
    let id = output["id"].as_u64().unwrap();
    let pushed = eventually(
        "the output in the engine",
        Duration::from_secs(10),
        || async {
            let config = station.state.stations.first().engine.config().await.ok()?;
            config
                .outputs_json
                .contains("hackme")
                .then_some(config.outputs_json)
        },
    )
    .await;
    assert!(!pushed.contains("adm1n-pw") && !pushed.contains("statistics"));

    // Changing them leaves the engine's configuration as it was.
    let (status, changed) = admin
        .send(
            Method::PUT,
            &format!("/stream-servers/{server_id}"),
            json!({"name": "Local", "host": "127.0.0.1", "port": 9, "admin_username": "ops"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["admin_username"], "ops");
    assert_eq!(changed["admin_password_set"], true);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        station
            .state
            .stations
            .first()
            .engine
            .config()
            .await
            .unwrap()
            .outputs_json,
        pushed
    );
    let (_, removed) = admin
        .send(
            Method::PUT,
            &format!("/stream-servers/{server_id}"),
            json!({"name": "Local", "host": "127.0.0.1", "port": 9, "admin_password": null}),
        )
        .await;
    assert_eq!(removed["admin_password_set"], false);
    assert_eq!(removed["source_password_set"], true);

    // Settings.
    assert_eq!(
        viewer.get("/settings/statistics").await,
        json!({"min_session_s": 60, "retention_days": 90})
    );
    let (status, _) = viewer
        .send(
            Method::PUT,
            "/settings/statistics",
            json!({"min_session_s": 30}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/statistics",
            json!({"retention_days": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, saved) = admin
        .send(
            Method::PUT,
            "/settings/statistics",
            json!({"min_session_s": 30}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved, json!({"min_session_s": 30, "retention_days": 90}));

    // Totals as the collector stores them; viewers read them.
    let pool = &station.state.pool;
    for (output_id, day, hour, minutes, peak, average, distinct) in [
        (None, "2026-10-19", None, 1_440, 12, 4.0, Some(30)),
        (None, "2026-10-19", Some(8), 60, 12, 9.0, Some(20)),
        (None, "2026-10-25", None, 720, 20, 8.0, Some(50)),
        (Some(id), "2026-10-25", None, 720, 20, 8.0, Some(50)),
    ] {
        sqlx::query(
            "INSERT INTO listener_totals (station_id, output_id, day, hour, minutes, peak, average,
                 listening_hours, sessions, session_s, distinct_listeners)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(output_id)
        .bind(day)
        .bind(hour)
        .bind(minutes)
        .bind(peak)
        .bind(average)
        .bind(average * f64::from(minutes) / 60.0)
        .bind(distinct.map(|d: u32| d * 2))
        .bind(distinct.map(|d: u32| u64::from(d) * 600))
        .bind(distinct)
        .execute(pool)
        .await
        .unwrap();
    }
    let days = viewer
        .get("/listeners/days?from=2026-10-19&to=2026-10-25")
        .await;
    assert_eq!(days.as_array().unwrap().len(), 2);
    assert_eq!(days[0]["day"], "2026-10-19");
    assert_eq!(days[0]["distinct_listeners"], 30.0);
    assert_eq!(days[0]["average_session_s"], 300.0);
    let week = viewer
        .get("/listeners/days?from=2026-10-19&to=2026-10-25&group=week")
        .await;
    assert_eq!(week.as_array().unwrap().len(), 1);
    assert_eq!(week[0]["distinct_listeners"], 40.0);
    assert_eq!(week[0]["listener_days"], 80);
    assert_eq!(week[0]["peak"], 20);
    let output_days = viewer
        .get(&format!(
            "/listeners/days?from=2026-10-19&to=2026-10-25&output_id={id}"
        ))
        .await;
    assert_eq!(output_days.as_array().unwrap().len(), 1);
    let hours = viewer
        .get("/listeners/hours-of-day?from=2026-10-19&to=2026-10-25")
        .await;
    assert_eq!(
        hours,
        json!([{"hour": 8, "days": 1, "average": 9.0, "peak": 12, "listening_hours": 9.0,
                "tune_in": null, "tune_out": null}])
    );
    // One server's part.
    sqlx::query(
        "INSERT INTO listener_totals (station_id, output_id, server_id, day, hour, minutes, peak,
             average, listening_hours)
         VALUES (1, NULL, 7, '2026-10-19', NULL, 60, 3, 1.5, 1.5)",
    )
    .execute(pool)
    .await
    .unwrap();
    let relay_days = viewer
        .get("/listeners/days?from=2026-10-19&to=2026-10-25&server_id=7")
        .await;
    assert_eq!(relay_days.as_array().unwrap().len(), 1);
    assert_eq!(relay_days[0]["peak"], 3);
    assert_eq!(
        viewer
            .get("/listeners/days?from=2026-10-19&to=2026-10-25")
            .await[0]["peak"],
        12
    );
    // The audience figures need samples and sessions, purged here.
    let figures = viewer
        .get("/listeners/audience?from=2026-10-19&to=2026-10-25&server_id=7")
        .await;
    assert_eq!(figures["current"]["from"], "2026-10-19");
    assert_eq!(figures["current"]["days"], 0);
    assert_eq!(figures["current"]["ttsl_hours"], json!(null));
    assert_eq!(figures["previous"]["from"], "2026-10-12");
    assert_eq!(figures["previous"]["to"], "2026-10-18");

    // CSV.
    let csv = viewer
        .http
        .get(format!(
            "{}/listeners/days?from=2026-10-19&to=2026-10-25&format=csv",
            viewer.base
        ))
        .header("cookie", viewer.cookie.as_deref().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(csv.status(), StatusCode::OK);
    assert_eq!(csv.headers()["content-type"], "text/csv; charset=utf-8");
    assert!(
        csv.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("listeners-2026-10-19-2026-10-25.csv")
    );
    let csv = csv.text().await.unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 3, "{csv}");
    assert!(lines[0].starts_with("day,hour,days,minutes,peak,average,listening_hours"));
    assert_eq!(
        lines[1],
        "2026-10-19,,1,1440,12,4.00,96.00,60,300.00,30.00,30"
    );

    // The other routes answer, and check their parameters.
    assert_eq!(
        viewer.get("/listeners/now").await,
        json!({"outputs": [], "total": null, "servers": []})
    );
    assert_eq!(
        viewer
            .get("/listeners/tracks?from=2026-10-19&to=2026-10-25")
            .await,
        json!([])
    );
    assert_eq!(
        viewer
            .get("/listeners/players?from=2026-10-19&to=2026-10-25")
            .await,
        json!([])
    );
    assert_eq!(
        viewer
            .get("/listeners/samples?from_unix_ms=0&to_unix_ms=60000")
            .await,
        json!([])
    );
    for bad in [
        "/listeners/samples?from_unix_ms=0&to_unix_ms=999999999",
        "/listeners/days?from=2026-10-25&to=2026-10-19",
        "/listeners/days?from=yesterday&to=2026-10-19",
        "/listeners/tracks?from=2026-10-19&to=2026-10-25&limit=0",
        "/listeners/players?from=2026-10-19&to=2026-10-25&limit=101",
        "/listeners/audience?from=2026-10-25&to=2026-10-19",
    ] {
        let (status, _) = viewer.request(Method::GET, bad, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    for path in [
        format!("/listeners/samples?from_unix_ms=0&to_unix_ms=60000&output_id={id}"),
        format!("/listeners/players?from=2026-10-19&to=2026-10-25&output_id={id}&limit=5"),
        "/listeners/tracks?from=2026-10-19&to=2026-10-25&min_plays=1&limit=5".to_owned(),
        format!("/listeners/tracks?from=2026-10-19&to=2026-10-25&output_id={id}&server_id=7"),
        "/listeners/players?from=2026-10-19&to=2026-10-25&server_id=7".to_owned(),
    ] {
        assert_eq!(viewer.get(&path).await, json!([]), "{path}");
    }
    let csv = |path: &str| {
        let request = viewer
            .http
            .get(format!("{}{path}", viewer.base))
            .header("cookie", viewer.cookie.as_deref().unwrap());
        async move { request.send().await.unwrap().text().await.unwrap() }
    };
    assert_eq!(
        csv("/listeners/hours-of-day?from=2026-10-19&to=2026-10-25&format=csv").await,
        "hour,days,average,peak,listening_hours,tune_in,tune_out\n8,1,9.00,12,9.00,,\n"
    );
    let audience = csv("/listeners/audience?from=2026-10-19&to=2026-10-25&format=csv").await;
    let lines: Vec<&str> = audience.lines().collect();
    assert_eq!(lines[0], "figure,period,previous_period");
    assert_eq!(lines[1], "from,2026-10-19,2026-10-12");
    assert!(lines.contains(&"ttsl_hours,,"), "{audience}");
    assert!(lines.contains(&"min_session_s,5,5"), "{audience}");
    assert_eq!(
        csv("/listeners/tracks?from=2026-10-19&to=2026-10-25&format=csv").await,
        "track_id,artist,title,plays,listeners_start,listeners_end,departures,departure_rate\n"
    );
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn library_tracks_overrides_and_categories() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let page = admin.get("/tracks?q=artist%203&limit=10").await;
    assert_eq!(page["total"], 1);
    let id = page["items"][0]["id"].as_u64().unwrap();
    assert_eq!(admin.get("/tracks?filter=playable").await["total"], 5);

    let (status, category) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Gold", "color": "#FFAA00"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(category["fx_player"], false);
    let (status, duplicate) = admin
        .send(Method::POST, "/categories", json!({"name": "Gold"}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{duplicate}");
    let (status, liners) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Liner cap", "fx_player": true}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{liners}");
    assert_eq!(liners["fx_player"], true);
    assert_eq!(liners["type_id"], Value::Null);

    // Category types: Song, Jingle and Ad, and the station's own.
    let types = admin.get("/category-types").await;
    let names: Vec<&str> = types
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Song", "Jingle", "Ad"]);
    let jingle = types[1]["id"].as_u64().unwrap();
    let viewer = station.signed_in("viewer").await;
    let (status, _) = viewer
        .send(Method::POST, "/category-types", json!({"name": "Liner"}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, liner) = admin
        .send(Method::POST, "/category-types", json!({"name": " Liner "}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{liner}");
    assert_eq!(liner["name"], "Liner");
    let (status, _) = admin
        .send(Method::POST, "/category-types", json!({"name": "liner"}))
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "names are unique ignoring case"
    );
    let (status, liners) = admin
        .send(
            Method::PUT,
            &format!("/categories/{}", liners["id"]),
            json!({"name": "Liner cap", "fx_player": true, "type_id": jingle}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{liners}");
    assert_eq!(liners["type_name"], "Jingle");
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/categories/{}", liners["id"]),
            json!({"name": "Liner cap", "type_id": 999}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(admin.get("/category-types").await[1]["categories"], 1);
    let (status, _) = admin
        .request(Method::DELETE, &format!("/category-types/{jingle}"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "still used");
    let (status, renamed) = admin
        .send(
            Method::PUT,
            &format!("/category-types/{}", liner["id"]),
            json!({"name": "Liners"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    let (status, _) = admin
        .request(
            Method::DELETE,
            &format!("/category-types/{}", liner["id"]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, assigned) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{id}/categories"),
            json!([{"category_id": category["id"], "weight": 2.0}]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let in_gold = admin
        .get(&format!("/tracks?category_id={}", category["id"]))
        .await;
    assert_eq!(in_gold["total"], 1);
    // What the decks and the queue show: each track's categories.
    let other = station.tracks.iter().find(|track| **track != id).unwrap();
    let shown = viewer
        .get(&format!("/tracks/categories?ids={id},{other},{id}"))
        .await;
    let shown = shown.as_array().unwrap();
    assert_eq!(shown.len(), 2, "{shown:?}");
    let mine = shown.iter().find(|t| t["track_id"] == id).unwrap();
    assert_eq!(mine["categories"][0]["name"], "Gold");
    assert_eq!(mine["categories"][0]["id"], category["id"]);
    let theirs = shown.iter().find(|t| t["track_id"] == *other).unwrap();
    assert_eq!(theirs["categories"], json!([]));
    let (status, body) = viewer
        .request(Method::GET, "/tracks/categories?ids=1,x", None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Requests the API cannot read are JSON errors too, with what is wrong.
    assert_eq!(body["error"], "invalid_request", "{body}");
    assert!(!body["message"].as_str().unwrap().is_empty(), "{body}");
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{id}/overrides"),
            json!({"cue_in_ms": "soon"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid", "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("cue_in_ms"),
        "{body}"
    );
    let (status, body) = viewer.request(Method::GET, "/no-such-route", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found", "{body}");

    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{id}/overrides"),
            json!({"cue_in_ms": 500, "crossfade": {"crossfade": false}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let detail = admin.get(&format!("/tracks/{id}")).await;
    assert_eq!(detail["playout"]["cue_in_ms"], 500);
    assert_eq!(detail["overrides"]["crossfade"]["crossfade"], false);
    assert_eq!(detail["categories"][0]["name"], "Gold");
    assert_eq!(detail["analysis"]["cue_out_ms"], 3000);
    // The list gives the cue points previews play between, overrides applied.
    let listed = admin
        .get(&format!("/tracks?category_id={}", category["id"]))
        .await;
    assert_eq!(
        (
            &listed["items"][0]["cue_in_ms"],
            &listed["items"][0]["cue_out_ms"]
        ),
        (&json!(500), &detail["analysis"]["cue_out_ms"])
    );
    // Loudness: −13 LUFS brought to −16.
    assert_eq!(detail["playout"]["gain_db"], -3.0);
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{id}/overrides"),
            json!({"cue_in_ms": 4000, "cue_out_ms": 1000}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // The intro point, set by hand: within the file, removed with null.
    let intro = format!("/tracks/{id}/intro");
    let (status, markers) = admin
        .send(Method::PUT, &intro, json!({"intro_ms": 1500}))
        .await;
    assert_eq!(status, StatusCode::OK, "{markers}");
    assert_eq!(markers["intro_ms"], 1500);
    assert_eq!(
        admin.get(&format!("/tracks/{id}")).await["markers"]["intro_ms"],
        1500
    );
    for wrong in [0, 3_600_000] {
        let (status, _) = admin
            .send(Method::PUT, &intro, json!({"intro_ms": wrong}))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{wrong}");
    }
    let (status, _) = viewer
        .send(Method::PUT, &intro, json!({"intro_ms": 1000}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, markers) = admin
        .send(Method::PUT, &intro, json!({"intro_ms": null}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(markers["intro_ms"], Value::Null);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/tracks/999999/intro",
            json!({"intro_ms": 1000}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The hook: in before out, within the file; both null remove it.
    let hook = format!("/tracks/{id}/hook");
    let (status, markers) = admin
        .send(
            Method::PUT,
            &hook,
            json!({"hook_in_ms": 500, "hook_out_ms": 2500}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{markers}");
    assert_eq!(
        (&markers["hook_in_ms"], &markers["hook_out_ms"]),
        (&json!(500), &json!(2500))
    );
    for wrong in [
        json!({"hook_in_ms": 2500, "hook_out_ms": 500}),
        json!({"hook_in_ms": 500, "hook_out_ms": 3_600_000}),
        json!({"hook_in_ms": 500, "hook_out_ms": null}),
    ] {
        let (status, _) = admin.send(Method::PUT, &hook, wrong.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{wrong}");
    }
    let (status, _) = viewer
        .send(
            Method::PUT,
            &hook,
            json!({"hook_in_ms": null, "hook_out_ms": null}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, markers) = admin
        .send(
            Method::PUT,
            &hook,
            json!({"hook_in_ms": null, "hook_out_ms": null}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(markers["hook_in_ms"], Value::Null);

    let (status, preview) = admin
        .send(
            Method::POST,
            "/playout/transition",
            json!({"from_track_id": station.tracks[0], "to_track_id": id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    // Tracks under 20 s play back to back.
    assert_eq!(preview["overlap_ms"], 0);
    // Overrides not saved yet, for a track's page.
    let cue_out = preview["outgoing"]["cue_out_ms"].as_u64().unwrap();
    let (status, tried) = admin
        .send(
            Method::POST,
            "/playout/transition",
            json!({"from_track_id": station.tracks[0], "to_track_id": id,
                   "from_overrides": {"cue_out_ms": cue_out - 1000},
                   "to_overrides": {"cue_in_ms": 500}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{tried}");
    assert_eq!(tried["outgoing"]["cue_out_ms"], cue_out - 1000);
    assert_eq!(tried["incoming"]["cue_in_ms"], 500);
    assert_eq!(tried["next_start_ms"], cue_out - 1000);

    let (status, _) = admin
        .send(
            Method::PATCH,
            &format!("/tracks/{id}"),
            json!({"enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(admin.get("/tracks?filter=disabled").await["total"], 1);
    station.stop().await;
}

/// A jingle linked to the next slot is queued with its track, linked to it.
#[tokio::test(flavor = "multi_thread")]
async fn linked_clock_slots_queue_a_jingle_with_its_track() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let t = &station.tracks;
    let mut categories = Vec::new();
    for (name, members) in [("Jingles", &t[..2]), ("Songs", &t[2..])] {
        let (status, category) = admin
            .send(
                Method::POST,
                "/categories",
                json!({"name": name, "separation": {"track_min": 1}}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        for track in members {
            let (status, _) = admin
                .send(
                    Method::PUT,
                    &format!("/tracks/{track}/categories"),
                    json!([{"category_id": category["id"]}]),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
        }
        categories.push(category["id"].as_u64().unwrap());
    }
    let (jingles, songs) = (categories[0], categories[1]);
    // No chains: a slot linked to the next one, itself linked.
    for slots in [
        json!([{"category_id": jingles, "linked_to_next": true}]),
        json!([
            {"category_id": jingles, "linked_to_next": true},
            {"category_id": jingles, "linked_to_next": true},
            {"category_id": songs},
        ]),
    ] {
        let (status, body) = admin
            .send(
                Method::POST,
                "/clocks",
                json!({"name": "Chain", "slots": slots}),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    let (status, clock) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Linked", "slots": [
                {"category_id": songs},
                {"category_id": jingles, "linked_to_next": true},
                {"category_id": songs},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    assert_eq!(clock["slots"][0]["linked_to_next"], false);
    assert_eq!(clock["slots"][1]["linked_to_next"], true);
    let clock_id = clock["id"].as_u64().unwrap();
    let week: Vec<Value> = (1..=7)
        .flat_map(|day| {
            (0..24).map(move |hour| json!({"day_of_week": day, "hour": hour, "clock_id": clock_id}))
        })
        .collect();
    let (status, _) = admin.send(Method::PUT, "/schedule", json!(week)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/queue",
            json!({"autofill": true, "min_queued": 6}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A jingle and its track, after the first waiting track.
    let pair = || async {
        let queue = admin.get("/queue").await;
        let entries = queue.as_array().unwrap().clone();
        entries
            .windows(2)
            .skip(1)
            .find(|pair| pair[0]["linked_to"] == pair[1]["entry_id"])
            .map(|pair| {
                (
                    pair[0]["entry_id"].as_u64().unwrap(),
                    pair[1]["entry_id"].as_u64().unwrap(),
                )
            })
    };
    let entries = || async {
        let queue = admin.get("/queue").await;
        queue
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["entry_id"].as_u64().unwrap())
            .collect::<Vec<u64>>()
    };
    let (jingle, track) = eventually("a linked pair", Duration::from_secs(20), pair).await;
    // Moving the track to the end moves its jingle with it.
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/queue/{track}/position"),
            json!({"position": 99}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let waiting = entries().await;
    let at = waiting.iter().position(|id| *id == track).unwrap();
    assert_eq!(waiting[at - 1], jingle, "{waiting:?}");
    // Removing the track removes its jingle.
    let (status, _) = admin
        .request(Method::DELETE, &format!("/queue/{track}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let waiting = entries().await;
    assert!(
        !waiting.contains(&jingle) && !waiting.contains(&track),
        "{waiting:?}"
    );
    // Removing a jingle keeps its track.
    let (jingle, track) = eventually("another pair", Duration::from_secs(20), pair).await;
    let (status, _) = admin
        .request(Method::DELETE, &format!("/queue/{jingle}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let waiting = entries().await;
    assert!(
        !waiting.contains(&jingle) && waiting.contains(&track),
        "{waiting:?}"
    );

    // The rotation log: the jingle's pick says it is linked, and the next
    // pick is the next slot's.
    let picks = admin.get("/rotation/picks?limit=100").await;
    let picks: Vec<Value> = picks.as_array().unwrap().iter().rev().cloned().collect();
    let linked: Vec<usize> = (0..picks.len().saturating_sub(1))
        .filter(|index| picks[*index]["explanation"]["linked"] == true)
        .collect();
    assert!(!linked.is_empty());
    for index in linked {
        assert_eq!(picks[index]["explanation"]["slot"], 1);
        assert_eq!(picks[index + 1]["explanation"]["slot"], 2);
    }
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_tracks_follow_the_library() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/queue",
            json!({"autofill": true, "min_queued": 4}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let queue = eventually("four waiting tracks", Duration::from_secs(10), || async {
        let queue = admin.get("/queue").await;
        (queue.as_array().unwrap().len() >= 4).then_some(queue)
    })
    .await;

    // The librarian moves the last waiting track's file, and records it.
    let last = &queue[3];
    let (entry, track) = (
        last["entry_id"].as_u64().unwrap(),
        last["track"]["track_id"].as_u64().unwrap(),
    );
    let old = last["track"]["path"].as_str().unwrap().to_owned();
    let new = format!("Moved/{}", old.replace('/', " - "));
    std::fs::create_dir_all(station.library.path().join("Moved")).unwrap();
    std::fs::rename(
        station.library.path().join(&old),
        station.library.path().join(&new),
    )
    .unwrap();
    sqlx::query("UPDATE tracks SET path = ? WHERE id = ?")
        .bind(&new)
        .bind(track)
        .execute(&station.db.pool)
        .await
        .unwrap();
    eventually(
        "the queued track to follow its file",
        Duration::from_secs(8),
        || async {
            let queue = admin.get("/queue").await;
            queue
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["entry_id"] == entry && e["track"]["path"] == new.as_str())
                .then_some(())
        },
    )
    .await;

    // A waiting track whose file disappeared leaves the queue.
    let queue = admin.get("/queue").await;
    let gone = &queue[queue.as_array().unwrap().len() - 1];
    let gone_entry = gone["entry_id"].as_u64().unwrap();
    sqlx::query("UPDATE tracks SET missing_since = NOW(6) WHERE id = ?")
        .bind(gone["track"]["track_id"].as_u64().unwrap())
        .execute(&station.db.pool)
        .await
        .unwrap();
    eventually(
        "the missing track out of the queue",
        Duration::from_secs(8),
        || async {
            let queue = admin.get("/queue").await;
            (!queue
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["entry_id"] == gone_entry))
            .then_some(())
        },
    )
    .await;
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn adopts_outputs_configured_on_the_engine() {
    let engine_config: EngineConfig = serde_json::from_value(json!({
        "playout": {"crossfade": {"min_track_ms": 25000}},
        "eq": {"music": {"enabled": true, "gains_db": [2, 0, 0, 0, 0, 0, 0, 0, 0, 0]}},
        "agc": {"bypass_all": false, "final_stage": {"ceiling_db": -2}},
        "outputs": {
            "encoder_profiles": [
                {"id": 1, "name": "MP3 128k", "encoder": "mp3", "bitrate_kbps": 128,
                 "sample_rate": 44100, "channels": 2},
                {"id": 7, "name": "MP3 64k", "encoder": "mp3", "bitrate_kbps": 64,
                 "sample_rate": 44100, "channels": 2}
            ],
            "outputs": [
                {"id": 3, "name": "Low", "kind": "icecast", "encoder_profile_id": 7,
                 "config": {"host": "127.0.0.1", "port": 9, "mount": "/low", "password": "p"}}
            ]
        }
    }))
    .unwrap();
    let Some(station) = Station::start_with(Some(engine_config.clone())).await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    eventually("the first sync", Duration::from_secs(10), || async {
        let engine = admin.get("/engine").await;
        (engine["config"]["state"] == "synced").then_some(())
    })
    .await;
    let outputs = admin.get("/outputs").await;
    assert_eq!(outputs[0]["id"], 3);
    assert_eq!(outputs[0]["encoder_profile_id"], 7);
    // Grouped into a stream server, its own settings only.
    assert_eq!(outputs[0]["config"], json!({"mount": "/low"}));
    let servers = admin.get("/stream-servers").await;
    assert_eq!(servers[0]["name"], "127.0.0.1:9");
    assert_eq!(outputs[0]["server_id"], servers[0]["id"]);
    let playout = admin.get("/settings/playout").await;
    assert_eq!(playout["crossfade"]["min_track_ms"], 25000);
    let eq = admin.get("/settings/eq").await;
    assert_eq!(eq["music"]["enabled"], true);
    assert_eq!(eq["music"]["gains_db"][0], 2.0);
    let agc = admin.get("/settings/agc").await;
    assert_eq!(agc["bypass_all"], false);
    assert_eq!(agc["final_stage"]["ceiling_db"], -2.0);

    // Same IDs, and the same settings written as the engine had them: the
    // engine kept its configuration as it was.
    let config = station
        .state
        .stations
        .first()
        .engine
        .config()
        .await
        .unwrap();
    let outputs: heron_core::OutputsConfig = serde_json::from_str(&config.outputs_json).unwrap();
    assert_eq!(outputs.outputs, engine_config.outputs.outputs);

    // A station managed before M10: no AGC chain saved, the engine's set
    // with `ctl set-agc` meanwhile. It is kept, not replaced by the defaults.
    sqlx::query("DELETE FROM station_settings WHERE name = 'agc'")
        .execute(&station.state.pool)
        .await
        .unwrap();
    let first = station.state.stations.first();
    let mut chain: heron_core::AgcSettings =
        serde_json::from_str(&config.agc_settings_json).unwrap();
    chain.final_stage.ceiling_db = -3.0;
    first
        .engine
        .set_agc_settings(serde_json::to_string(&chain).unwrap())
        .await
        .unwrap();
    first.sync.changed();
    eventually("the AGC chain kept", Duration::from_secs(10), || async {
        let agc = admin.get("/settings/agc").await;
        (agc["final_stage"]["ceiling_db"] == -3.0).then_some(())
    })
    .await;
    let config = first.engine.config().await.unwrap();
    let kept: heron_core::AgcSettings = serde_json::from_str(&config.agc_settings_json).unwrap();
    assert_eq!(kept.final_stage.ceiling_db, -3.0);
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn web_ui_routes() {
    let Some(station) = Station::start().await else {
        return;
    };
    let pool = &station.state.pool;
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let track = station.tracks[0];

    // The theme each user picked.
    assert_eq!(admin.get("/auth/me").await["theme"], "system");
    let (status, me) = admin
        .send(Method::PUT, "/auth/preferences", json!({"theme": "dark"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["theme"], "dark");
    assert_eq!(admin.get("/auth/me").await["theme"], "dark");
    assert_eq!(viewer.get("/auth/me").await["theme"], "system");

    // And the language: changing one preference keeps the other.
    assert_eq!(me["language"], "system");
    let (status, me) = admin
        .send(Method::PUT, "/auth/preferences", json!({"language": "fr"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(
        (&me["language"], &me["theme"]),
        (&json!("fr"), &json!("dark"))
    );

    // And the width: a centred column unless the user wants 80 % or the
    // whole window.
    assert_eq!(me["width"], "normal");
    for width in ["wide", "full"] {
        let (status, me) = admin
            .send(Method::PUT, "/auth/preferences", json!({"width": width}))
            .await;
        assert_eq!(status, StatusCode::OK, "{me}");
        assert_eq!(
            (&me["width"], &me["language"], &me["theme"]),
            (&json!(width), &json!("fr"), &json!("dark"))
        );
    }
    assert_eq!(admin.get("/auth/me").await["width"], "full");
    assert_eq!(viewer.get("/auth/me").await["width"], "normal");
    for refused in [json!({"width": "80"}), json!({"full_width": true})] {
        let (status, _) = admin.send(Method::PUT, "/auth/preferences", refused).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    // And the whole queue on On air: off unless the user turns it on.
    assert_eq!(me["on_air_whole_queue"], false);
    let (status, me) = admin
        .send(
            Method::PUT,
            "/auth/preferences",
            json!({"on_air_whole_queue": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(
        (&me["on_air_whole_queue"], &me["width"], &me["theme"]),
        (&json!(true), &json!("full"), &json!("dark"))
    );
    assert_eq!(admin.get("/auth/me").await["on_air_whole_queue"], true);
    assert_eq!(viewer.get("/auth/me").await["on_air_whole_queue"], false);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/auth/preferences",
            json!({"on_air_whole_queue": "yes"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/auth/preferences",
            json!({"language": "French"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(admin.get("/auth/me").await["language"], "fr");
    assert_eq!(viewer.get("/auth/me").await["language"], "system");

    // The server clock, for the dashboard.
    let status = station.client().get("/status").await;
    assert!(status["clock"].is_object());
    assert!(status["server_time_unix_ms"].as_i64().unwrap() > 0);

    // Waveforms come with the analysis.
    let (status, _) = viewer
        .request(Method::GET, &format!("/tracks/{track}/waveform"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    sqlx::query("INSERT INTO track_waveforms (track_id, interval_ms, peaks) VALUES (?, 50, ?)")
        .bind(track)
        .bind([0u8, 128, 255].as_slice())
        .execute(pool)
        .await
        .unwrap();
    let waveform = viewer.get(&format!("/tracks/{track}/waveform")).await;
    assert_eq!(waveform["interval_ms"], 50);
    assert_eq!(waveform["peaks"], "AID/");

    // Audio for previews, with range requests.
    let audio = format!("{}/tracks/{track}/audio", viewer.base);
    let response = viewer
        .http
        .get(&audio)
        .header("cookie", viewer.cookie.as_deref().unwrap())
        .header("range", "bytes=0-3")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(&response.bytes().await.unwrap()[..], b"RIFF");
    let response = reqwest::get(&audio).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Organizing goes through heron-librarian, after a dry run.
    let roots = heron_librarian::journal::Roots {
        library: station.library.path().to_path_buf(),
        incoming: station.library.path().join(".incoming"),
        staging: station.staging.path().to_path_buf(),
        covers: station.state.library.covers().clone(),
    };
    let librarian = heron_librarian::tasks::TaskRunner::new(
        pool.clone(),
        roots,
        heron_librarian::identify::Endpoints::default(),
    );
    let (status, _) = viewer
        .request(Method::POST, "/library/organize", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, plan) = admin.request(Method::POST, "/library/organize", None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{plan}");
    assert_eq!(plan["status"], "pending");
    assert_eq!(plan["request"]["kind"], "organize_plan");
    let plan_id = plan["id"].as_u64().unwrap();
    let apply = format!("/library/organize/{plan_id}/apply");
    let (status, _) = admin.request(Method::POST, &apply, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "not finished yet");
    assert!(librarian.run_next().await.unwrap());
    let plan = admin.get(&format!("/library/tasks/{plan_id}")).await;
    assert_eq!(plan["status"], "done", "{plan}");
    assert_eq!(plan["plan"]["moves"].as_array().unwrap().len(), 5);
    assert_eq!(
        plan["plan"]["moves"][0]["to"],
        "Unknown Artist/Unknown Album/Track 1.wav"
    );
    let (status, applying) = admin.request(Method::POST, &apply, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{applying}");
    let (status, _) = admin.request(Method::POST, &apply, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "applied once");
    assert!(librarian.run_next().await.unwrap());
    let applied = admin
        .get(&format!("/library/tasks/{}", applying["id"]))
        .await;
    assert_eq!(applied["report"]["done"], 5, "{applied}");
    assert!(
        station
            .library
            .path()
            .join("Unknown Artist/Unknown Album/Track 1.wav")
            .exists()
    );
    let batches = viewer.get("/library/batches").await;
    assert_eq!(batches[0]["kind"], "organize");
    assert_eq!(batches[0]["files"], 5);
    let batch = batches[0]["id"].as_u64().unwrap();
    let (status, undo) = admin
        .request(
            Method::POST,
            &format!("/library/batches/{batch}/undo"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{undo}");
    assert!(librarian.run_next().await.unwrap());
    assert!(station.library.path().join("Artist 1/Track 1.wav").exists());
    let (status, _) = admin
        .request(
            Method::POST,
            &format!("/library/batches/{batch}/undo"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "undone once");
    assert_eq!(
        viewer.get("/library/tasks").await.as_array().unwrap().len(),
        3
    );

    // Review: a candidate release, or the file's own tags.
    let candidates = json!([{"score": 0.7, "recording_id": "rec", "release_id": "rel",
                             "release_title": "Album", "disc": 1, "track": 1}]);
    sqlx::query(
        "INSERT INTO track_identifications (track_id, status, reason, score, candidates)
         VALUES (?, 'needs_review', 'doubtful', 0.7, ?)",
    )
    .bind(track)
    .bind(candidates.to_string())
    .execute(pool)
    .await
    .unwrap();
    let review = viewer.get("/library/review").await;
    assert_eq!(review["total"], 1);
    assert_eq!(review["items"][0]["track"]["id"], track);
    assert_eq!(review["items"][0]["candidates"][0]["release_id"], "rel");
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/tracks/{track}/review/apply"),
            json!({"release_id": "other", "recording_id": "rec"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, picked) = admin
        .send(
            Method::POST,
            &format!("/tracks/{track}/review/apply"),
            json!({"release_id": "rel", "recording_id": "rec"}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{picked}");
    assert_eq!(picked["request"]["track"], 1);
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/tracks/{track}/review/apply"),
            json!({"release_id": "rel", "recording_id": "rec"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "already being applied");
    let keep = format!("/tracks/{track}/review/keep");
    let (status, _) = viewer.request(Method::POST, &keep, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin.request(Method::POST, &keep, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin.request(Method::POST, &keep, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(viewer.get("/library/review").await["total"], 0);
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rotation_follows_clocks_restrictions_and_logs_its_picks() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let pool = &station.db.pool;
    let t = &station.tracks;

    crate::artists::link_pending(pool).await.unwrap();
    assert_eq!(admin.get("/artists?q=artist%201").await["total"], 1);

    // Artists from the tags: names matched loosely, featuring split.
    for (track, artist) in [
        (t[0], "Neal Morse"),
        (t[1], "NÉAL  MORSE feat. Mike Portnoy"),
        (t[2], "The Neal Morse Band"),
        (t[3], "Simon & Garfunkel"),
        (t[4], "Paul Simon"),
    ] {
        sqlx::query("UPDATE tracks SET artist = ? WHERE id = ?")
            .bind(artist)
            .bind(track)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE tracks SET artist = 'Simon' WHERE id = ?")
        .bind(t[4])
        .execute(pool)
        .await
        .unwrap();
    crate::artists::link_pending(pool).await.unwrap();
    let page = admin.get("/artists?q=neal%20morse").await;
    assert_eq!(page["total"], 2, "{page}");
    let neal = page["artists"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Neal Morse")
        .unwrap()
        .clone();
    assert_eq!(neal["tracks"], 2, "{neal}");
    assert_eq!(neal["names"].as_array().unwrap().len(), 1);
    let band = page["artists"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "The Neal Morse Band")
        .unwrap()
        .clone();
    assert_eq!(admin.get("/artists?q=portnoy").await["total"], 1);
    // Linking again changes nothing; the artists of the old tags are gone.
    assert_eq!(crate::artists::link_pending(pool).await.unwrap(), 0);
    assert_eq!(admin.get("/artists?q=artist%201").await["total"], 0);

    // "Simon & Garfunkel" names Simon: suggested, never applied alone.
    let suggestions = admin.get("/artist-suggestions").await;
    let suggestion = &suggestions.as_array().unwrap()[0];
    assert_eq!(suggestion["source"], "names", "{suggestions}");
    let (status, _) = viewer
        .send(
            Method::POST,
            &format!("/artist-suggestions/{}", suggestion["id"]),
            json!({"accept": true}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/artist-suggestions/{}", suggestion["id"]),
            json!({"accept": true}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        admin
            .get("/artist-suggestions")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    let simon = &admin.get("/artists?q=garfunkel").await["artists"][0];
    assert_eq!(simon["similar"].as_array().unwrap().len(), 1, "{simon}");

    // The band is the same artist: merged, its tracks follow.
    let (status, merged) = admin
        .send(
            Method::POST,
            &format!("/artists/{}/merge", neal["id"]),
            json!({"artist_id": band["id"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{merged}");
    assert_eq!(merged["tracks"], 3);
    assert_eq!(merged["names"].as_array().unwrap().len(), 2);
    // A name moved back to an artist of its own.
    let band_name = merged["names"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "The Neal Morse Band")
        .unwrap()["id"]
        .clone();
    let (status, own) = admin
        .send(
            Method::PUT,
            &format!("/artist-names/{band_name}"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(own["tracks"], 1);
    let (status, _) = admin
        .request(
            Method::PUT,
            &format!("/artists/{}/similar/{}", neal["id"], own["id"]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Settings: the time zone and the rotation rules are checked.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/station",
            json!({"timezone": "Mars/Olympus"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/station",
            json!({"timezone": "Europe/Paris"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/rotation",
            json!({"weights": {"play_factor": 0.1}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // The draw depth: 1 to 100 %, the start not above the maximum.
    for draw in [
        json!({"start_pct": 60, "max_pct": 40}),
        json!({"start_pct": 0}),
        json!({"max_pct": 101}),
    ] {
        let (status, _) = admin
            .send(Method::PUT, "/settings/rotation", json!({"draw": draw}))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{draw}");
    }
    let (status, rotation) = admin
        .send(
            Method::PUT,
            "/settings/rotation",
            json!({"draw": {"start_pct": 30, "max_pct": 60}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rotation["draw"], json!({"start_pct": 30, "max_pct": 60}));
    let (status, _) = admin
        .send(Method::PUT, "/settings/rotation", json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        admin.get("/settings/rotation").await["draw"],
        json!({"start_pct": 20, "max_pct": 50})
    );
    let (status, _) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Too deep", "separation": {"draw_start_pct": 70, "draw_max_pct": 60}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Two categories, a clock calling them in turn, all week.
    let mut categories = Vec::new();
    for (name, members) in [("A", &t[..3]), ("B", &t[3..])] {
        let (status, category) = admin
            .send(
                Method::POST,
                "/categories",
                json!({"name": name, "separation": {"track_min": 1, "draw_max_pct": 80}}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        assert_eq!(category["separation"]["track_min"], 1);
        assert_eq!(category["separation"]["draw_max_pct"], 80);
        for track in members {
            let (status, _) = admin
                .send(
                    Method::PUT,
                    &format!("/tracks/{track}/categories"),
                    json!([{"category_id": category["id"]}]),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
        }
        categories.push(category["id"].as_u64().unwrap());
    }
    let (status, _) = viewer
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Nope", "slots": []}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, clock) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Hour", "slots": [
                {"category_id": categories[0], "method": "random"},
                {"category_id": categories[1], "method": "weighted",
                 "separation": {"artist_min": 0, "draw_start_pct": 40}},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    let clock_id = clock["id"].as_u64().unwrap();
    assert_eq!(clock["slots"][1]["separation"]["artist_min"], 0);
    assert_eq!(clock["slots"][1]["separation"]["draw_start_pct"], 40);
    assert_eq!(clock["looping"], false);
    let week: Vec<Value> = (1..=7)
        .flat_map(|day| {
            (0..24).map(move |hour| json!({"day_of_week": day, "hour": hour, "clock_id": clock_id}))
        })
        .collect();
    let (status, schedule) = admin.send(Method::PUT, "/schedule", json!(week)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(schedule.as_array().unwrap().len(), 168);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/schedule",
            json!([{"day_of_week": 8, "hour": 0, "clock_id": clock_id}]),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = admin
        .send(
            Method::DELETE,
            &format!("/categories/{}", categories[0]),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "clocks use it");

    // The last track is banned all week: never picked.
    let banned = t[4];
    let (status, restriction) = admin
        .send(
            Method::POST,
            "/restrictions",
            json!({
                "target": {"kind": "track", "id": banned},
                "rule": {"kind": "daypart", "mode": "ban", "days": [1, 2, 3, 4, 5, 6, 7],
                         "start_hour": 0, "end_hour": 24},
                "note": "pulled",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{restriction}");
    assert!(
        restriction["target_name"]
            .as_str()
            .unwrap()
            .contains("Simon")
    );
    let (status, _) = admin
        .send(
            Method::POST,
            "/restrictions",
            json!({
                "target": {"kind": "category", "id": categories[0]},
                "rule": {"kind": "daypart", "mode": "ban", "days": [], "start_hour": 0, "end_hour": 24},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        admin
            .get(&format!("/restrictions?target=track&target_id={banned}"))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Weights halve with each play.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/rotation",
            json!({"weights": {"play_factor": 0.5}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // From now on the clock runs: A, B, A, B…
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let first_pick: u64 =
        sqlx::query_scalar("SELECT CAST(COALESCE(MAX(id), 0) AS UNSIGNED) FROM rotation_picks")
            .fetch_one(pool)
            .await
            .unwrap();
    let picks = eventually(
        "clock picks that played",
        Duration::from_secs(20),
        || async {
            let picks = admin.get("/rotation/picks?limit=200").await;
            let picks: Vec<Value> = picks
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| p["id"].as_u64().unwrap() > first_pick)
                .cloned()
                .collect();
            let played = picks
                .iter()
                .filter(|p| p["played_at_unix_ms"].is_i64())
                .count();
            (picks.len() >= 6 && played >= 3).then_some(picks)
        },
    )
    .await;
    let mut slots: Vec<(u64, u64)> = picks
        .iter()
        .rev()
        .map(|p| {
            let explanation = &p["explanation"];
            assert_eq!(explanation["clock_id"], clock_id, "{p}");
            assert_ne!(p["track_id"], banned, "{p}");
            // The random slot has no draw; the weighted one starts at its
            // own 40 %, up to its category's 80 %.
            if explanation["slot"] == 0 {
                assert!(explanation["draw"].is_null(), "{p}");
            } else if explanation["step"].as_u64().unwrap() < 11 {
                assert_eq!(explanation["draw"]["start_pct"], 40, "{p}");
                assert_eq!(explanation["draw"]["max_pct"], 80, "{p}");
            }
            (
                explanation["slot"].as_u64().unwrap(),
                explanation["slot_category_id"].as_u64().unwrap(),
            )
        })
        .collect();
    slots.dedup();
    assert!(slots.len() >= 4, "slots alternate: {slots:?}");
    for (slot, category) in &slots {
        assert_eq!(*category, categories[*slot as usize]);
    }
    // Tracks repeat within a minute: rules were relaxed, and the plays say so.
    let relaxed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM play_history h JOIN rotation_picks p ON p.play_id = h.id
         WHERE p.step > 0 AND JSON_VALUE(h.relaxation_applied, '$.step') = p.step",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let relaxed_picks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rotation_picks WHERE step > 0 AND play_id IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(relaxed_picks > 0);
    assert_eq!(relaxed, relaxed_picks);
    let weight: f64 =
        sqlx::query_scalar("SELECT MIN(weight) FROM track_categories WHERE category_id = ?")
            .bind(categories[0])
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(weight < 1.0, "weights moved: {weight}");

    // The clock as it will sound, and a simulation of it.
    let (status, preview) = admin
        .send(
            Method::POST,
            "/clocks/preview",
            json!({"slots": clock["slots"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["slots"][0]["tracks"], 3, "{preview}");
    assert_eq!(preview["slots"][1]["average_ms"], 3_000);
    assert!(preview["runs_per_hour"].as_f64().unwrap() > 100.0);
    // A looping clock: nothing falls past the hour.
    let long: Vec<Value> = (0..30)
        .map(|_| json!({"category_id": categories[0]}))
        .collect();
    let (status, looped) = admin
        .send(
            Method::POST,
            "/clocks/preview",
            json!({"slots": long, "looping": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{looped}");
    assert!(
        looped["slots"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["beyond_hour"] == false)
    );
    let (status, saved) = admin
        .send(
            Method::PUT,
            &format!("/clocks/{clock_id}"),
            json!({"name": "Hour", "looping": true, "slots": clock["slots"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["looping"], true);
    let (status, report) = admin
        .send(
            Method::POST,
            "/rotation/simulate",
            json!({"hours": 2, "seed": 7}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let simulation = &report["simulation"];
    assert!(
        simulation["violations"].as_array().unwrap().is_empty(),
        "{simulation}"
    );
    let simulated = simulation["picks"].as_array().unwrap();
    assert!(simulated.len() > 2_000);
    assert!(simulated.iter().all(|p| p["track_id"] != banned));
    assert_eq!(report["tracks"].as_array().unwrap().len(), 4);
    let (status, _) = viewer
        .send(Method::POST, "/rotation/simulate", json!({"hours": 1}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Never two jingles in a row: A becomes a jingle category, and a clock
    // calling it twice running passes the second slot over.
    let jingle = admin.get("/category-types").await[1].clone();
    assert_eq!(
        (&jingle["name"], &jingle["no_two_in_a_row"]),
        (&json!("Jingle"), &json!(true))
    );
    let (status, a) = admin
        .send(
            Method::PUT,
            &format!("/categories/{}", categories[0]),
            json!({"name": "A", "type_id": jingle["id"], "separation": {"track_min": 1}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{a}");
    let (status, saved) = admin
        .send(
            Method::PUT,
            &format!("/clocks/{clock_id}"),
            json!({"name": "Hour", "slots": [
                {"category_id": categories[0], "method": "random"},
                {"category_id": categories[0], "method": "random"},
                {"category_id": categories[1], "method": "random"},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let simulate = || async {
        let (status, report) = admin
            .send(
                Method::POST,
                "/rotation/simulate",
                json!({"hours": 1, "seed": 3}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{report}");
        report["simulation"].clone()
    };
    let simulation = simulate().await;
    assert!(
        simulation["violations"].as_array().unwrap().is_empty(),
        "{simulation}"
    );
    let picks = simulation["picks"].as_array().unwrap();
    let from_a: Vec<bool> = picks
        .iter()
        .map(|p| p["explanation"]["slot_category_id"] == categories[0])
        .collect();
    assert!(from_a.iter().any(|a| *a), "jingles still play");
    assert!(
        !from_a.windows(2).any(|pair| pair[0] && pair[1]),
        "two in a row"
    );
    assert!(picks.iter().any(|p| {
        p["explanation"]["skipped_slots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["reason"] == "same_type_in_a_row")
    }));
    // Turned off on the type, the clock plays both slots.
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/category-types/{}", jingle["id"]),
            json!({"name": "Jingle", "no_two_in_a_row": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let simulation = simulate().await;
    let picks = simulation["picks"].as_array().unwrap();
    assert!(picks.windows(2).any(|pair| {
        pair.iter()
            .all(|p| p["explanation"]["slot_category_id"] == categories[0])
    }));
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn track_details_labels_programming_and_tags() {
    let Some(station) = Station::start().await else {
        return;
    };
    let pool = &station.db.pool;
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;

    // 1,000 tracks never analyzed (autofill leaves them alone), half of
    // them at a low bitrate, added a year ago but the last ten.
    sqlx::query(
        "INSERT INTO tracks (path, title, artist, bitrate_kbps, created_at)
         SELECT CONCAT('bulk/', seq, '.mp3'), CONCAT('Bulk ', seq), 'Bulk artist',
                IF(seq % 2 = 0, 96, 320),
                IF(seq > 990, NOW(6), NOW(6) - INTERVAL 1 YEAR)
         FROM seq_1_to_1000",
    )
    .execute(pool)
    .await
    .unwrap();
    let bulk: Vec<u64> =
        sqlx::query_scalar("SELECT id FROM tracks WHERE path LIKE 'bulk/%' ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
    let (a, b, c) = (bulk[0], bulk[1], bulk[2]);
    // a: two plays that count and a skip; b: only on the FX player;
    // c: playing for a minute (its end not known yet).
    for (track, deck, played_ms, ago_s) in [
        (a, "a", Some(40_000), 3_600),
        (a, "b", Some(10_000), 1_800),
        (a, "a", Some(35_000), 600),
        (b, "fx", Some(60_000), 300),
        (c, "a", None, 60),
    ] {
        sqlx::query(
            "INSERT INTO play_history (station_id, track_id, title, started_at, ended_at, played_ms,
                                       deck, source)
             VALUES (1, ?, 'x', NOW(6) - INTERVAL ? SECOND,
                     IF(? IS NULL, NULL, NOW(6) - INTERVAL ? SECOND), ?, ?, 'manual')",
        )
        .bind(track)
        .bind(ago_s)
        .bind(played_ms)
        .bind(ago_s - 60)
        .bind(played_ms)
        .bind(deck)
        .execute(pool)
        .await
        .unwrap();
    }
    let page = admin.get("/tracks?q=Bulk&sort=play_count&limit=3").await;
    assert_eq!(page["total"], 1000);
    let counts: Vec<(u64, u64)> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["id"].as_u64().unwrap(), t["play_count"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        counts[..2],
        [(a, 2), (c, 1)],
        "skips and FX plays do not count"
    );
    let detail = admin.get(&format!("/tracks/{a}")).await;
    assert_eq!(detail["bitrate_kbps"], 320);
    let last_played = detail["last_played_at_unix_ms"].as_i64().unwrap();
    let ten_minutes_ago = jiff::Timestamp::now().as_millisecond() - 600_000;
    assert!(
        (last_played - ten_minutes_ago).abs() < 5_000,
        "the skip is not the last play"
    );
    // Every start counts with a minimum of 0 s.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/programming",
            json!({"min_play_s": 0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(admin.get(&format!("/tracks/{a}")).await["play_count"], 3);
    admin
        .send(
            Method::PUT,
            "/settings/programming",
            json!({"min_play_s": 30}),
        )
        .await;

    assert_eq!(
        admin.get("/tracks?q=Bulk&never_played=true").await["total"],
        998
    );
    assert_eq!(
        admin.get("/tracks?q=Bulk&not_played_days=1").await["total"],
        998
    );
    assert_eq!(admin.get("/tracks?q=Bulk&added_days=7").await["total"], 10);
    assert_eq!(
        admin.get("/tracks?q=Bulk&bitrate_below=128").await["total"],
        500
    );
    let lowest = admin.get("/tracks?q=Bulk&sort=bitrate&limit=1").await;
    assert_eq!(lowest["items"][0]["bitrate_kbps"], 96);
    let highest = admin
        .get("/tracks?q=Bulk&sort=bitrate&desc=true&limit=1")
        .await;
    assert_eq!(highest["items"][0]["bitrate_kbps"], 320);

    // A label on the 1,000 tracks in one action, then off them.
    let (status, christmas) = admin
        .send(
            Method::POST,
            "/labels",
            json!({"name": "Christmas", "color": "#AA0000"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{christmas}");
    let (_, live) = admin
        .send(Method::POST, "/labels", json!({"name": "live"}))
        .await;
    let (status, _) = admin
        .send(Method::POST, "/labels", json!({"name": "christmas"}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "unique ignoring case");
    let (christmas, live) = (
        christmas["id"].as_u64().unwrap(),
        live["id"].as_u64().unwrap(),
    );
    let (status, _) = viewer
        .send(
            Method::POST,
            "/tracks/labels",
            json!({"selection": {"filters": {"q": "Bulk"}}, "add": [christmas]}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let started = Instant::now();
    let (status, added) = admin
        .send(
            Method::POST,
            "/tracks/labels",
            json!({"selection": {"filters": {"q": "Bulk"}}, "add": [christmas]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(
        (added["tracks"].as_u64(), added["changed"].as_u64()),
        (Some(1000), Some(1000))
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    admin
        .send(
            Method::POST,
            "/tracks/labels",
            json!({"selection": {"track_ids": [a, b]}, "add": [live]}),
        )
        .await;
    assert_eq!(
        admin.get(&format!("/tracks?labels_all={christmas}")).await["total"],
        1000
    );
    assert_eq!(
        admin
            .get(&format!("/tracks?labels_all={christmas},{live}"))
            .await["total"],
        2
    );
    assert_eq!(
        admin
            .get(&format!(
                "/tracks?labels_any={christmas}&labels_none={live}&bitrate_below=128"
            ))
            .await["total"],
        499
    );
    assert_eq!(
        admin.get(&format!("/tracks?labels_none={christmas}")).await["total"],
        5
    );
    let page = admin.get(&format!("/tracks?labels_all={live}")).await;
    assert_eq!(page["items"][0]["labels"].as_array().unwrap().len(), 2);
    let labels = admin.get("/labels").await;
    assert_eq!(labels[0]["name"], "Christmas");
    assert_eq!(labels[0]["tracks"], 1000);

    // A category filled from the label.
    let (_, category) = admin
        .send(Method::POST, "/categories", json!({"name": "Holidays"}))
        .await;
    let (status, filled) = admin
        .send(
            Method::POST,
            "/tracks/categories",
            json!({"selection": {"filters": {"labels_any": christmas.to_string()}},
                   "add": [{"category_id": category["id"]}]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{filled}");
    assert_eq!(
        admin.get(&format!("/categories/{}", category["id"])).await["tracks"],
        1000
    );
    // Its total time: never analyzed, the files' lengths (unknown: 0), cue
    // overrides applied.
    assert_eq!(
        admin.get(&format!("/categories/{}", category["id"])).await["total_ms"],
        0
    );
    sqlx::query("UPDATE tracks SET duration_ms = 200000 WHERE path LIKE 'bulk/%'")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO track_overrides (track_id, cue_in_ms, cue_out_ms) VALUES (?, 10000, 999999)",
    )
    .bind(a)
    .execute(pool)
    .await
    .unwrap();
    let categories = admin.get("/categories").await;
    let holidays = categories
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == category["id"])
        .unwrap();
    assert_eq!(holidays["total_ms"], 1000 * 200_000 - 10_000, "{holidays}");
    sqlx::query("UPDATE tracks SET duration_ms = NULL WHERE path LIKE 'bulk/%'")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM track_overrides WHERE track_id = ?")
        .bind(a)
        .execute(pool)
        .await
        .unwrap();

    let (status, removed) = admin
        .send(
            Method::POST,
            "/tracks/labels",
            json!({"selection": {"filters": {"labels_any": christmas.to_string()}},
                   "remove": [christmas]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["changed"], 1000);
    assert_eq!(
        admin.get(&format!("/tracks?labels_any={christmas}")).await["total"],
        0
    );
    let (status, merged) = admin
        .send(
            Method::POST,
            &format!("/labels/{live}/merge"),
            json!({"into_id": christmas}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{merged}");
    assert_eq!(merged["tracks"], 2);

    // Mood, sound code and tempo on a selection; eras from the year.
    let (status, set) = admin
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": {"track_ids": [a, b]}, "mood": "3", "sound_code": "hard",
                   "bpm": 98.44}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    let (status, _) = admin
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": {"track_ids": [a]}, "mood": "wistful"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "not on the scale");
    admin
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": {"track_ids": [c]}, "sound_code": "soft"}),
        )
        .await;
    let detail = admin.get(&format!("/tracks/{a}")).await;
    assert_eq!(
        (detail["mood"].as_str(), detail["sound_code"].as_str()),
        (Some("3"), Some("hard"))
    );
    assert_eq!(detail["bpm"], 98.4);
    assert_eq!(detail["bpm_by_hand"], 98.4);
    assert_eq!(admin.get("/tracks?mood=3").await["total"], 2);
    let by_code = admin.get("/tracks?q=Bulk&sort=sound_code&limit=3").await;
    assert_eq!(
        by_code["items"][0]["sound_code"], "soft",
        "in the order of the scale"
    );
    assert_eq!(by_code["items"][1]["sound_code"], "hard");
    sqlx::query("UPDATE tracks SET year = 1975 WHERE id = ?")
        .bind(a)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO track_musicbrainz (track_id, release_id, recording_id, first_release_year)
         VALUES (?, 'rel', 'rec', 2004)",
    )
    .bind(b)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(admin.get(&format!("/tracks/{a}")).await["era"], "70s");
    assert_eq!(admin.get(&format!("/tracks/{b}")).await["era"], "2000s");
    assert_eq!(admin.get("/tracks?era=70s").await["total"], 1);
    admin
        .send(
            Method::PUT,
            "/settings/programming",
            json!({"eras": [{"name": "classics", "to_year": 1989},
                            {"name": "recurrents", "from_year": 1990}]}),
        )
        .await;
    assert_eq!(admin.get(&format!("/tracks/{a}")).await["era"], "classics");
    assert_eq!(admin.get("/tracks?era=recurrents").await["total"], 1);

    // Tags edited from the browser, written by heron-librarian, undone byte
    // for byte.
    let track = station.tracks[0];
    let file = station.library.path().join("Artist 1/Track 1.wav");
    let original = std::fs::read(&file).unwrap();
    let roots = heron_librarian::journal::Roots {
        library: station.library.path().to_path_buf(),
        incoming: station.library.path().join(".incoming"),
        staging: station.staging.path().to_path_buf(),
        covers: station.state.library.covers().clone(),
    };
    let librarian = heron_librarian::tasks::TaskRunner::new(
        pool.clone(),
        roots,
        heron_librarian::identify::Endpoints::default(),
    );
    let (status, invalid) = admin
        .send(
            Method::POST,
            "/tracks/tags",
            json!({"selection": {"track_ids": [track]}, "set": {"year": "soon"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{invalid}");
    let (status, task) = admin
        .send(
            Method::POST,
            "/tracks/tags",
            json!({"selection": {"track_ids": [track]},
                   "set": {"genre": "Jazz", "year": "1999", "album_artist": "Various"}}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    assert!(librarian.run_next().await.unwrap());
    let task = admin.get(&format!("/library/tasks/{}", task["id"])).await;
    assert_eq!(task["report"]["done"], 1, "{task}");
    let detail = admin.get(&format!("/tracks/{track}")).await;
    assert_eq!(detail["genre"], "Jazz");
    assert_eq!(detail["album_artist"], "Various");
    assert_eq!(detail["year"], 1999);
    assert_eq!(detail["id"], track, "same track");
    assert_eq!(detail["analysis"]["cue_out_ms"], 3000, "not analyzed again");
    assert_ne!(std::fs::read(&file).unwrap(), original);
    let batches = admin.get("/library/batches").await;
    assert_eq!(batches[0]["kind"], "edit");
    let (status, _) = admin
        .request(
            Method::POST,
            &format!("/library/batches/{}/undo", batches[0]["id"]),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(librarian.run_next().await.unwrap());
    assert_eq!(std::fs::read(&file).unwrap(), original, "byte for byte");
    assert_eq!(
        admin.get(&format!("/tracks/{track}")).await["genre"],
        Value::Null
    );

    // With write_tags off, edits are kept in Heron's database: the file is
    // left alone, and a task asked before the switch writes nothing.
    let (status, queued) = admin
        .send(
            Method::POST,
            "/tracks/tags",
            json!({"selection": {"track_ids": [track]}, "set": {"genre": "Soul"}}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let (status, settings) = admin
        .send(
            Method::PATCH,
            "/settings/library",
            json!({"write_tags": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["write_tags"], false);
    assert!(librarian.run_next().await.unwrap());
    let queued = admin.get(&format!("/library/tasks/{}", queued["id"])).await;
    assert_eq!(queued["status"], "failed", "{queued}");
    assert_eq!(std::fs::read(&file).unwrap(), original);

    let (status, refused) = admin
        .send(
            Method::POST,
            "/tracks/tags",
            json!({"selection": {"track_ids": [track]}, "set": {"genre": "Soul"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let (status, refused) = viewer
        .send(
            Method::POST,
            "/tracks/tag-overrides",
            json!({"selection": {"track_ids": [track]}, "set": {"title": "x"}}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, refused) = admin
        .send(
            Method::POST,
            "/tracks/tag-overrides",
            json!({"selection": {"track_ids": [track]}, "set": {"isrc": "USRC17607839"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, result) = admin
        .send(
            Method::POST,
            "/tracks/tag-overrides",
            json!({"selection": {"track_ids": [track]},
                   "set": {"title": " Corrected ", "year": "1977", "album": "Live", "disc": null}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["changed"], 4);
    assert_eq!(std::fs::read(&file).unwrap(), original, "file untouched");
    let detail = admin.get(&format!("/tracks/{track}")).await;
    assert_eq!(detail["title"], "Corrected");
    assert_eq!(detail["year"], 1977);
    assert_eq!(detail["album"], "Live");
    assert_eq!(
        detail["tag_overrides"],
        json!(["title", "album", "year", "disc"])
    );

    // The scanner reading the file again keeps them.
    let read = heron_library::tags::read(&file).unwrap();
    let mut connection = pool.acquire().await.unwrap();
    heron_library::details::save(
        &mut connection,
        station.state.library.covers(),
        track,
        &read,
        None,
    )
    .await
    .unwrap();
    drop(connection);
    assert_eq!(
        admin.get(&format!("/tracks/{track}")).await["title"],
        "Corrected"
    );

    // Cleared, a field shows the file's tag again (this file has none).
    let (status, result) = admin
        .send(
            Method::POST,
            "/tracks/tag-overrides/clear",
            json!({"selection": {"track_ids": [track]}, "fields": ["title"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["changed"], 1);
    let detail = admin.get(&format!("/tracks/{track}")).await;
    assert_eq!(detail["title"], Value::Null);
    assert_eq!(detail["year"], 1977);
    assert_eq!(detail["tag_overrides"], json!(["album", "year", "disc"]));

    // Back to writing the files: a field written is dropped from the
    // overrides, the others stay.
    admin
        .send(
            Method::PATCH,
            "/settings/library",
            json!({"write_tags": true}),
        )
        .await;
    let (status, refused) = admin
        .send(
            Method::POST,
            "/tracks/tag-overrides",
            json!({"selection": {"track_ids": [track]}, "set": {"title": "x"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let (status, task) = admin
        .send(
            Method::POST,
            "/tracks/tags",
            json!({"selection": {"track_ids": [track]}, "set": {"year": "2001"}}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    assert!(librarian.run_next().await.unwrap());
    let detail = admin.get(&format!("/tracks/{track}")).await;
    assert_eq!(detail["year"], 2001);
    assert_eq!(detail["album"], "Live");
    assert_eq!(detail["tag_overrides"], json!(["album", "disc"]));
    let (status, result) = admin
        .send(
            Method::POST,
            "/tracks/tag-overrides/clear",
            json!({"selection": {"track_ids": [track]}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["changed"], 2);
    let detail = admin.get(&format!("/tracks/{track}")).await;
    assert_eq!(detail["album"], Value::Null);
    assert_eq!(detail["year"], 2001);
    assert_eq!(detail["tag_overrides"], json!([]));
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_wait_in_the_buffer_zone_until_imported() {
    let Some(station) = Station::start().await else {
        return;
    };
    let pool = &station.db.pool;
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let roots = heron_librarian::journal::Roots {
        library: station.library.path().to_path_buf(),
        incoming: station.library.path().join(".incoming"),
        staging: station.staging.path().to_path_buf(),
        covers: station.state.library.covers().clone(),
    };
    let endpoints = heron_librarian::identify::Endpoints::default();
    let librarian =
        heron_librarian::tasks::TaskRunner::new(pool.clone(), roots.clone(), endpoints.clone());
    let mut stager = heron_librarian::staging::Stager::new(pool.clone(), roots, endpoints);

    let (status, _) = viewer
        .send(Method::POST, "/staging/groups", json!({"name": "Drop"}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, group) = admin
        .send(Method::POST, "/staging/groups", json!({"name": "Drop"}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group_id = group["id"].as_u64().unwrap();

    // Refused: not audio, larger than the limit.
    let (status, _) = admin
        .send(
            Method::POST,
            "/staging/uploads",
            json!({"group_id": group_id, "name": "Drop/notes.txt", "size": 10}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = admin
        .send(
            Method::PATCH,
            "/settings/library",
            json!({"max_upload_mb": 1}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, refused) = admin
        .send(
            Method::POST,
            "/staging/uploads",
            json!({"group_id": group_id, "name": "Drop/big.flac", "size": 2 * 1024 * 1024}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, _) = admin
        .send(
            Method::PATCH,
            "/settings/library",
            json!({"max_upload_mb": 2048}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // A file sent in chunks; the connection drops after the first.
    let source = station.staging.path().join("source.wav");
    write_wav(&source, 10.0, 330.0);
    let content = std::fs::read(&source).unwrap();
    std::fs::remove_file(&source).unwrap();
    let (status, file) = admin
        .send(
            Method::POST,
            "/staging/uploads",
            json!({"group_id": group_id, "name": "Drop/../Song.wav", "size": content.len()}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    let id = file["id"].as_u64().unwrap();
    assert_eq!(
        (file["status"].as_str(), file["received"].as_u64()),
        (Some("receiving"), Some(0))
    );
    let chunk = format!("/staging/uploads/{id}?offset=0");
    let (status, file) = admin.put_bytes(&chunk, content[..100_000].to_vec()).await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["received"], 100_000);
    // Sent again after the drop: refused, the client asks and carries on.
    let (status, _) = admin.put_bytes(&chunk, content[..100_000].to_vec()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let file = admin.get(&format!("/staging/uploads/{id}")).await;
    assert_eq!(file["received"], 100_000);
    // An incomplete upload never shows as ready.
    assert!(stager.identify_once().await.unwrap().ready == 0);
    let buffer = admin.get("/staging").await;
    assert_eq!(buffer["groups"][0]["files"][0]["status"], "receiving");
    let (status, file) = admin
        .put_bytes(
            &format!("/staging/uploads/{id}?offset=100000"),
            content[100_000..].to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["status"], "identifying");
    let staged = station
        .staging
        .path()
        .join(format!("{group_id}/Drop/Song.wav"));
    assert_eq!(
        std::fs::read(&staged).unwrap(),
        content,
        "complete, byte for byte"
    );

    // A broken file.
    let (_, broken) = admin
        .send(
            Method::POST,
            "/staging/uploads",
            json!({"group_id": group_id, "name": "Drop/broken.mp3", "size": 4096}),
        )
        .await;
    let broken_id = broken["id"].as_u64().unwrap();
    let (status, _) = admin
        .put_bytes(
            &format!("/staging/uploads/{broken_id}?offset=0"),
            vec![0x5a; 4096],
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Identified (no AcoustID key: its own tags) and checked.
    assert_eq!(stager.identify_once().await.unwrap().ready, 2);
    let buffer = viewer.get("/staging").await;
    let files = buffer["groups"][0]["files"].as_array().unwrap().clone();
    let song = files.iter().find(|f| f["id"] == id).unwrap();
    assert_eq!(song["status"], "ready", "{song}");
    assert_eq!(
        song["planned_path"],
        "Unknown Artist/Unknown Album/Song.wav"
    );
    assert_eq!(song["identification"]["status"], "off");
    assert_eq!(song["flags"], json!([]));
    let broken = files.iter().find(|f| f["id"] == broken_id).unwrap();
    assert_eq!(broken["flags"].as_array().unwrap().len(), 1, "{broken}");
    let response = reqwest::Client::new()
        .get(format!("{}/staging/files/{id}/audio", admin.base))
        .header("cookie", admin.cookie.clone().unwrap())
        .header("range", "bytes=0-3")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(&response.bytes().await.unwrap()[..], b"RIFF");
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/staging/files/{broken_id}/choice"),
            json!({"kind": "release", "release_id": "x", "recording_id": "y"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "not a candidate");

    // Prepared for when they are imported.
    let (_, gold) = admin
        .send(Method::POST, "/categories", json!({"name": "Gold"}))
        .await;
    let (_, live) = admin
        .send(Method::POST, "/labels", json!({"name": "live"}))
        .await;
    let preparation = json!({
        "file_ids": [id, broken_id],
        "add_categories": [{"category_id": gold["id"], "weight": 2.0}],
        "add_labels": [live["id"]],
        "mood": "4",
        "sound_code": "hard",
    });
    let (status, _) = viewer
        .send(
            Method::POST,
            "/staging/files/preparation",
            preparation.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin
        .send(
            Method::POST,
            "/staging/files/preparation",
            json!({"file_ids": [id], "mood": "furious"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "not on the scale");
    let (status, prepared) = admin
        .send(Method::POST, "/staging/files/preparation", preparation)
        .await;
    assert_eq!(
        (status, &prepared["files"]),
        (StatusCode::OK, &json!(2)),
        "{prepared}"
    );
    let (status, _) = admin
        .send(
            Method::POST,
            "/staging/files/preparation",
            json!({"file_ids": [id], "sound_code": null}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let song = admin.get(&format!("/staging/uploads/{id}")).await;
    assert_eq!(song["categories"][0]["name"], "Gold", "{song}");
    assert_eq!(song["categories"][0]["weight"], 2.0);
    assert_eq!(song["labels"][0]["name"], "live");
    assert_eq!(
        (&song["mood"], &song["sound_code"]),
        (&json!("4"), &json!(null))
    );
    // Tags edited: the librarian works the path out again.
    let (status, _) = admin
        .send(
            Method::POST,
            "/staging/files/tags",
            json!({"file_ids": [id], "set": {"genre": "Rock"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "not edited here");
    let (status, _) = admin
        .send(
            Method::POST,
            "/staging/files/tags",
            json!({"file_ids": [id], "set": {"year": "nineteen"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "not a year");
    let (status, edited) = admin
        .send(
            Method::POST,
            "/staging/files/tags",
            json!({"file_ids": [id], "set": {"artist": "The Band", "title": "Song", "year": "1999"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let (status, _) = admin
        .send(
            Method::POST,
            "/staging/files/tags",
            json!({"file_ids": [id], "reset": ["year"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let song = admin.get(&format!("/staging/uploads/{id}")).await;
    assert_eq!(song["status"], "identifying");
    assert_eq!(
        song["tag_edits"],
        json!({"artist": "The Band", "title": "Song"})
    );
    stager.identify_once().await.unwrap();
    let song = admin.get(&format!("/staging/uploads/{id}")).await;
    assert_eq!(song["status"], "ready", "{song}");
    assert_eq!(song["tags"]["artist"], "The Band");
    assert_eq!(song["own_tags"]["artist"], json!(null));
    assert_eq!(song["planned_path"], "The Band/Unknown Album/Song.wav");

    // Not while it is imported.
    sqlx::query("UPDATE staged_files SET status = 'importing' WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    let (status, _) = admin
        .send(
            Method::POST,
            "/staging/files/preparation",
            json!({"file_ids": [id], "remove_labels": [live["id"]]}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    sqlx::query("UPDATE staged_files SET status = 'ready' WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();

    // Discarded: it never reaches the library.
    let (status, discarded) = admin
        .send(
            Method::POST,
            "/staging/discard",
            json!({"file_ids": [broken_id]}),
        )
        .await;
    assert_eq!(
        (status, discarded["discarded"].as_u64()),
        (StatusCode::OK, Some(1))
    );
    assert!(
        !station
            .staging
            .path()
            .join(format!("{group_id}/Drop/broken.mp3"))
            .exists()
    );
    let left: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM staged_file_categories WHERE staged_file_id = ?)
              + (SELECT COUNT(*) FROM staged_file_labels WHERE staged_file_id = ?)
              + (SELECT COUNT(*) FROM staged_files WHERE id = ? AND mood IS NOT NULL)",
    )
    .bind(broken_id)
    .bind(broken_id)
    .bind(broken_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(left, 0, "nothing left of its preparation");

    // Imported, as a batch.
    let (status, task) = admin
        .send(
            Method::POST,
            "/staging/import",
            json!({"file_ids": [id, broken_id]}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    assert!(librarian.run_next().await.unwrap());
    let task = admin.get(&format!("/library/tasks/{}", task["id"])).await;
    assert_eq!(task["report"]["done"], 1, "{task}");
    assert_eq!(
        task["report"]["skipped"].as_array().unwrap().len(),
        1,
        "the discarded one"
    );
    let written = heron_library::tags::read(
        &station
            .library
            .path()
            .join("The Band/Unknown Album/Song.wav"),
    )
    .unwrap()
    .tags;
    assert_eq!(
        (written.artist.as_deref(), written.title.as_deref()),
        (Some("The Band"), Some("Song")),
        "filed with its edited tags"
    );
    assert!(
        admin.get("/staging").await["groups"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let batches = admin.get("/library/batches").await;
    assert_eq!(batches[0]["kind"], "import");

    // Its track is ready to air, and found by its import.
    let batch = task["report"]["batch_id"].as_u64().unwrap();
    let page = admin
        .get(&format!("/tracks?batch_id={batch}&sort=added"))
        .await;
    assert_eq!(page["total"], 1, "{page}");
    let track = &page["items"][0];
    assert_eq!(track["path"], "The Band/Unknown Album/Song.wav");
    assert_eq!(track["artist"], "The Band");
    assert_eq!(track["labels"][0]["name"], "live");
    assert_eq!(
        (&track["mood"], &track["sound_code"]),
        (&json!("4"), &json!(null))
    );
    let detail = admin.get(&format!("/tracks/{}", track["id"])).await;
    assert_eq!(detail["categories"][0]["name"], "Gold", "{detail}");
    assert_eq!(detail["categories"][0]["weight"], 2.0);
    assert_eq!(
        admin.get("/tracks?sort=added").await["items"][0]["id"],
        track["id"],
        "the newest track first"
    );
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_tracks_leave_the_queue_and_go_to_the_trash() {
    let Some(station) = Station::start().await else {
        return;
    };
    let pool = &station.db.pool;
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let roots = heron_librarian::journal::Roots {
        library: station.library.path().to_path_buf(),
        incoming: station.library.path().join(".incoming"),
        staging: station.staging.path().to_path_buf(),
        covers: station.state.library.covers().clone(),
    };
    let librarian = heron_librarian::tasks::TaskRunner::new(
        pool.clone(),
        roots,
        heron_librarian::identify::Endpoints::default(),
    );
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/queue",
            json!({"autofill": true, "min_queued": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let on_air = eventually("a track on air", Duration::from_secs(10), || async {
        let player = admin.get("/player").await;
        player["decks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|deck| deck["on_air"] == true)
            .and_then(|deck| deck["entry"]["track"]["track_id"].as_u64())
    })
    .await;
    let queue = eventually("three waiting tracks", Duration::from_secs(10), || async {
        let queue = admin.get("/queue").await;
        (queue.as_array().unwrap().len() >= 3).then_some(queue)
    })
    .await;
    let queued = waiting_tracks(&queue)[2];
    let missing = *station
        .tracks
        .iter()
        .find(|&&id| id != on_air && !waiting_tracks(&queue).contains(&id))
        .unwrap();
    sqlx::query("UPDATE tracks SET missing_since = NOW(6) WHERE id = ?")
        .bind(missing)
        .execute(pool)
        .await
        .unwrap();
    let playing: u64 = eventually("the play on air", Duration::from_secs(5), || async {
        let history = admin.get("/history").await;
        history.as_array().unwrap().iter().find_map(|play| {
            (play["track_id"] == on_air && play["ended_at_unix_ms"].is_null())
                .then(|| play["id"].as_u64())
                .flatten()
        })
    })
    .await;

    let deleting = json!({"track_ids": [on_air, queued, missing]});
    let (status, _) = viewer
        .send(Method::POST, "/tracks/delete", deleting.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, task) = admin.send(Method::POST, "/tracks/delete", deleting).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    assert!(librarian.run_next().await.unwrap());
    let task = admin.get(&format!("/library/tasks/{}", task["id"])).await;
    assert_eq!(task["report"]["done"], 3, "{task}");

    // Out of the library and the queue; the track on air plays to its end.
    let tracks = admin.get("/tracks").await;
    assert_eq!(tracks["total"], 2);
    eventually(
        "the deleted track out of the queue",
        Duration::from_secs(8),
        || async { (!waiting_tracks(&admin.get("/queue").await).contains(&queued)).then_some(()) },
    )
    .await;
    eventually(
        "the play on air to finish",
        Duration::from_secs(10),
        || async {
            let history = admin.get("/history").await;
            history
                .as_array()
                .unwrap()
                .iter()
                .find(|play| play["id"] == playing && !play["end_reason"].is_null())
                .map(|play| assert_eq!(play["end_reason"], "finished", "{play}"))
        },
    )
    .await;

    // In the trash, until restored.
    let trash = viewer.get("/library/trash").await;
    assert_eq!(trash["total"], 3);
    assert_eq!(trash["trash_days"], 30);
    let item = &trash["items"].as_array().unwrap()[0];
    let batch = item["batch_id"].as_u64().unwrap();
    assert!(item["purge_at_unix_ms"].as_i64() > item["deleted_at_unix_ms"].as_i64());
    let detail = admin.get(&format!("/tracks/{queued}")).await;
    assert!(detail["deleted_at_unix_ms"].is_i64(), "{detail}");
    let (status, task) = admin
        .request(
            Method::POST,
            &format!("/library/trash/{batch}/restore"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    assert!(librarian.run_next().await.unwrap());
    assert_eq!(viewer.get("/library/trash").await["total"], 0);
    assert_eq!(admin.get("/tracks").await["total"], 5);
    let path: String = sqlx::query_scalar("SELECT path FROM tracks WHERE id = ?")
        .bind(queued)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(station.library.path().join(path).is_file());

    // Emptying the trash is for admins: here, deleted again and purged.
    let (_, task) = admin
        .send(
            Method::POST,
            "/tracks/delete",
            json!({"track_ids": [queued]}),
        )
        .await;
    assert!(librarian.run_next().await.unwrap());
    assert_eq!(
        admin.get(&format!("/library/tasks/{}", task["id"])).await["report"]["done"],
        1
    );
    let (status, _) = viewer
        .request(Method::POST, "/library/trash/empty", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, task) = admin
        .request(Method::POST, "/library/trash/empty", None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(librarian.run_next().await.unwrap());
    let task = admin.get(&format!("/library/tasks/{}", task["id"])).await;
    assert_eq!(task["purge"]["tracks"], 1, "{task}");
    let (status, _) = admin
        .request(Method::GET, &format!("/tracks/{queued}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    station.stop().await;
}

/// What a webhook receiver got: when, on which path, the headers, the body.
type Received = (u64, String, axum::http::HeaderMap, String);

async fn webhook_receiver() -> (tokio::sync::mpsc::UnboundedReceiver<Received>, SocketAddr) {
    let (sender, received) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().route(
        "/{name}",
        axum::routing::post(
            move |axum::extract::Path(name): axum::extract::Path<String>,
                  headers: axum::http::HeaderMap,
                  body: String| {
                let sender = sender.clone();
                async move {
                    let at = engine::unix_ms(std::time::SystemTime::now());
                    let _ = sender.send((at, name, headers, body));
                    "ok"
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (received, addr)
}

#[tokio::test(flavor = "multi_thread")]
async fn webhooks_report_track_changes_and_public_endpoints_answer_anyone() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let (mut received, receiver) = webhook_receiver().await;

    let (status, created) = admin
        .send(
            Method::POST,
            "/webhooks",
            json!({"name": "Website", "url": format!("http://{receiver}/json"),
                   "events": ["track.started", "track.ended", "track.skipped"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let secret = created["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("whsec_"));
    let webhook = created["webhook"]["id"].as_u64().unwrap();
    let (status, created) = admin
        .send(
            Method::POST,
            "/webhooks",
            json!({"name": "Display", "url": format!("http://{receiver}/text"),
                   "events": ["track.started"], "content_type": "text/plain",
                   "body_template": "{{artist}} – {{title}}"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, refused) = admin
        .send(
            Method::POST,
            "/webhooks",
            json!({"name": "Bad", "url": format!("http://{receiver}/bad"),
                   "events": ["track.started"], "body_template": "{{song}}"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let listed = admin.get("/webhooks").await;
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert!(!listed.to_string().contains(&secret), "the secret leaked");
    let (status, _) = viewer.request(Method::GET, "/webhooks", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Every track change the engine reports reaches the receiver within a
    // second, signed; a skip is reported as such.
    let mut events = station.state.stations.first().engine.events();
    let mut expected = Vec::new();
    let mut got = Vec::new();
    let mut skipped = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && (expected.len() < 6 || !skipped) {
        tokio::select! {
            event = events.recv() => {
                if let Ok(engine::LinkEvent::Track(event)) = event {
                    let kind = webhooks::WebhookEvent::of(&event).as_str();
                    let at = match &event {
                        engine::TrackEvent::Started { at_unix_ms, .. }
                        | engine::TrackEvent::Ended { at_unix_ms, .. } => *at_unix_ms,
                    };
                    expected.push((kind.to_owned(), at));
                    let starts = expected.iter().filter(|(kind, _)| kind == "track.started").count();
                    if kind == "track.started" && starts == 2 {
                        let (status, _) = admin.request(Method::POST, "/player/skip", None).await;
                        assert_eq!(status, StatusCode::NO_CONTENT);
                    }
                }
            }
            Some(delivery) = received.recv() => got.push(delivery),
        }
        skipped = got
            .iter()
            .any(|(_, _, headers, _)| headers["x-heron-event"] == "track.skipped");
    }
    // Deliveries of the last events may still be on their way.
    while let Ok(Some(delivery)) =
        tokio::time::timeout(Duration::from_secs(1), received.recv()).await
    {
        got.push(delivery);
    }
    assert!(skipped, "no track.skipped among {expected:?}");
    let mut reported = Vec::new();
    for (at, name, headers, body) in &got {
        if name == "text" {
            assert!(
                body.starts_with("Artist ") && body.contains(" – Track "),
                "{body}"
            );
            assert_eq!(headers["content-type"], "text/plain");
            continue;
        }
        let timestamp: u64 = headers["x-heron-timestamp"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            headers["x-heron-signature"].to_str().unwrap(),
            webhooks::signature(&secret, timestamp, body)
        );
        let payload: Value = serde_json::from_str(body).unwrap();
        let event_at = payload["at_unix_ms"].as_u64().unwrap();
        assert!(
            at.saturating_sub(event_at) < 1_000,
            "{} arrived {} ms late",
            payload["event"],
            at - event_at
        );
        assert_eq!(headers["x-heron-event"], payload["event"].as_str().unwrap());
        reported.push((payload["event"].as_str().unwrap().to_owned(), event_at));
    }
    for change in &expected {
        assert!(
            reported.contains(change),
            "{change:?} was not reported: {reported:?}"
        );
    }
    let texts = got.iter().filter(|(_, name, ..)| name == "text").count();
    let starts = expected
        .iter()
        .filter(|(kind, _)| kind == "track.started")
        .count();
    assert!(
        texts >= starts,
        "{texts} templated deliveries for {starts} starts"
    );

    // The delivery log, a test ping and a retry. The station plays on:
    // deliveries of the latest track changes may still be on their way, and
    // one the receiver got is recorded as delivered just after. Those it got
    // were delivered on their first attempt; none failed.
    let bodies: Vec<&str> = got.iter().map(|(_, _, _, body)| body.as_str()).collect();
    let log = eventually(
        "the deliveries recorded",
        Duration::from_secs(10),
        || async {
            let log = admin.get(&format!("/webhooks/{webhook}/deliveries")).await;
            let settled = log.as_array().unwrap().iter().all(|delivery| {
                !bodies.contains(&delivery["body"].as_str().unwrap())
                    || (delivery["status"] == "delivered" && delivery["attempts"] == 1)
            });
            settled.then_some(log)
        },
    )
    .await;
    let log = log.as_array().unwrap();
    assert!(log.len() >= expected.len());
    assert!(
        log.iter().all(|delivery| delivery["status"] != "failed"),
        "{log:?}"
    );
    let (status, ping) = admin
        .request(Method::POST, &format!("/webhooks/{webhook}/test"), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{ping}");
    let ping_id = ping["id"].as_u64().unwrap();
    let ping_got = loop {
        let (_, name, headers, body) = received.recv().await.unwrap();
        if name == "json" && headers["x-heron-event"] == "ping" {
            break serde_json::from_str::<Value>(&body).unwrap();
        }
    };
    assert_eq!(ping_got["delivery_id"], ping_id);
    // A delivery the receiver got, not the latest, which may be on its way.
    let delivered = log
        .iter()
        .find(|delivery| delivery["status"] == "delivered")
        .and_then(|delivery| delivery["id"].as_u64())
        .unwrap();
    let (status, retried) = admin
        .request(
            Method::POST,
            &format!("/webhook-deliveries/{delivered}/retry"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retried}");
    loop {
        let (_, _, headers, _) = received.recv().await.unwrap();
        if headers["x-heron-delivery"] == delivered.to_string().as_str() {
            break;
        }
    }

    // Public endpoints: no sign-in, any origin, no IDs.
    let anyone = reqwest::Client::new();
    let base = format!("http://{}/api/v1/public", station.api);
    let response = anyone
        .get(format!("{base}/now-playing"))
        .header("origin", "https://station.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert_eq!(response.headers()["cache-control"], "public, max-age=5");
    let now: Value = response.json().await.unwrap();
    assert!(
        now["now"]["title"].as_str().unwrap().starts_with("Track "),
        "{now}"
    );
    assert!(now["now"].get("track_id").is_none());
    let response = anyone
        .get(format!("{base}/history?limit=5"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let history: Value = response.json().await.unwrap();
    let plays = history.as_array().unwrap();
    assert!(!plays.is_empty() && plays.len() <= 5);
    for play in plays {
        assert!(
            play.get("id").is_none() && play.get("track_id").is_none(),
            "{play}"
        );
        assert!(play["artist"].is_string());
    }
    let response = anyone
        .get(format!("{base}/history?limit=500"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let older = plays.last().unwrap()["started_at_unix_ms"]
        .as_i64()
        .unwrap();
    let history: Value = anyone
        .get(format!("{base}/history?before={older}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        history
            .as_array()
            .unwrap()
            .iter()
            .all(|play| play["started_at_unix_ms"].as_i64().unwrap() < older)
    );

    // Editing a webhook keeps its secret; a new one replaces it.
    let edited = json!({"name": "Website", "url": format!("http://{receiver}/json"),
                        "events": ["track.started"], "enabled": false});
    let (status, changed) = admin
        .send(Method::PUT, &format!("/webhooks/{webhook}"), edited.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["enabled"], false);
    assert_eq!(changed["events"], json!(["track.started"]));
    let fetched = admin.get(&format!("/webhooks/{webhook}")).await;
    assert_eq!(fetched["enabled"], false);
    assert_eq!(fetched["events"], changed["events"]);
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/webhooks/{webhook}"),
            json!({"name": "Display", "url": format!("http://{receiver}/json"),
                   "events": ["track.started"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = admin.send(Method::PUT, "/webhooks/999999", edited).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, rotated) = admin
        .request(Method::POST, &format!("/webhooks/{webhook}/secret"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(rotated["secret"].as_str().unwrap(), secret);
    let (status, _) = admin
        .request(Method::POST, "/webhooks/999999/secret", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A disabled webhook's ping fails; the log filters and pages.
    let (status, ping) = admin
        .request(Method::POST, &format!("/webhooks/{webhook}/test"), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let ping = ping["id"].as_u64().unwrap();
    let failed = eventually("the ping to fail", Duration::from_secs(5), || async {
        let failed = admin
            .get(&format!("/webhooks/{webhook}/deliveries?status=failed"))
            .await;
        (!failed.as_array().unwrap().is_empty()).then_some(failed)
    })
    .await;
    assert_eq!(failed[0]["id"], ping, "{failed}");
    let page = admin
        .get(&format!(
            "/webhooks/{webhook}/deliveries?before={ping}&limit=2"
        ))
        .await;
    let page = page.as_array().unwrap();
    assert_eq!(page.len(), 2);
    assert!(
        page.iter()
            .all(|delivery| delivery["id"].as_u64().unwrap() < ping)
    );
    let (status, _) = admin
        .request(
            Method::GET,
            &format!("/webhooks/{webhook}/deliveries?limit=0"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = admin
        .request(Method::DELETE, &format!("/webhooks/{webhook}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for path in [
        format!("/webhooks/{webhook}"),
        format!("/webhooks/{webhook}/deliveries"),
    ] {
        let (status, _) = admin.request(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = admin
        .request(Method::DELETE, &format!("/webhooks/{webhook}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Jingles send no metadata: tracks 1 and 2 go in a Jingle category; while
    // one is on air, the public now-playing keeps the song before, and the
    // public history and webhooks leave it out.
    let jingle_type = admin.get("/category-types").await[1].clone();
    assert_eq!(
        (
            &jingle_type["name"],
            &jingle_type["sends_metadata"],
            &jingle_type["plays_over_intro"]
        ),
        (&json!("Jingle"), &json!(false), &json!(true))
    );
    let (status, jingles) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Jingles", "type_id": jingle_type["id"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{jingles}");
    for track in &station.tracks[..2] {
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{track}/categories"),
                json!([{"category_id": jingles["id"]}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    // Hours without a clock rotate the station's categories: the songs are
    // in one too.
    let (status, music) = admin
        .send(Method::POST, "/categories", json!({"name": "Music"}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{music}");
    for track in &station.tracks[2..] {
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{track}/categories"),
                json!([{"category_id": music["id"]}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let jingle_titles = ["Track 1", "Track 2"];
    // Queued again, with what the tracks are now.
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let mut texts = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let on_air = loop {
        assert!(Instant::now() < deadline, "no jingle went on air");
        while let Ok(Some((_, name, _, body))) =
            tokio::time::timeout(Duration::from_millis(10), received.recv()).await
        {
            if name == "text" {
                texts.push(body);
            }
        }
        let player = admin.get("/player").await;
        let jingle = player["decks"].as_array().unwrap().iter().find_map(|deck| {
            let track = &deck["entry"]["track"];
            // No intro marker on the next track: the jingle plays on its own.
            (deck["on_air"] == true && track["no_metadata"] == true && track["over_intro"] == true)
                .then(|| track["title"].clone())
        });
        if let Some(title) = jingle {
            break title;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        jingle_titles.contains(&on_air.as_str().unwrap()),
        "{on_air}"
    );
    let now: Value = anyone
        .get(format!("{base}/now-playing"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let shown = now["now"]["title"].as_str().unwrap_or_default();
    assert!(
        shown.starts_with("Track ") && !jingle_titles.contains(&shown),
        "the jingle's title shown: {now}"
    );
    let history: Value = anyone
        .get(format!("{base}/history?limit=50"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let in_history: Vec<&str> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|play| play["title"].as_str().unwrap_or_default())
        .collect();
    assert!(!in_history.is_empty());
    let played_jingle: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM play_history h JOIN track_categories tc ON tc.track_id = h.track_id
         WHERE tc.category_id = ?",
    )
    .bind(jingles["id"].as_u64())
    .fetch_one(&station.db.pool)
    .await
    .unwrap();
    assert!(played_jingle > 0);
    assert!(
        !in_history.iter().any(|title| jingle_titles.contains(title)),
        "{in_history:?}"
    );
    // Display (no FX player) is never told about the jingle.
    tokio::time::sleep(Duration::from_millis(500)).await;
    while let Ok(Some((_, name, _, body))) =
        tokio::time::timeout(Duration::from_millis(10), received.recv()).await
    {
        if name == "text" {
            texts.push(body);
        }
    }
    assert!(
        !texts
            .iter()
            .any(|text| jingle_titles.iter().any(|title| text.ends_with(title))),
        "{texts:?}"
    );
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn importing_from_another_software_and_undoing_it() {
    let Some(station) = Station::start().await else {
        return;
    };
    let pool = &station.db.pool;
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let [one, two, three, ..] = station.tracks[..] else {
        panic!("five tracks");
    };
    // A title already fixed in Heron: the import's differs.
    sqlx::query(
        "INSERT INTO track_tag_overrides (track_id, field, value) VALUES (?, 'title', 'Fixed')",
    )
    .bind(two)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE tracks SET title = 'Fixed' WHERE id = ?")
        .bind(two)
        .execute(pool)
        .await
        .unwrap();

    let row = |file: &str, title: &str, category: u64, problem: u64| {
        json!({
            "filename": format!("6-final-mp3/{file}"), "title": title, "artist": "Band\\'s",
            "year": "1980", "created_at": "2021-02-07 20:36:51", "mbid": "ignored",
            "category_id": category, "category_name": if category == 15 { "frigo" } else { "gold" },
            "category_parent_id": 0, "category_can_schedule": if category == 15 { 0 } else { 1 },
            "category_is_jingle": 0, "problem_id": problem, "problem_name": "a remplacer",
            "final_duration": 2.9, "cue_in": 0.5, "intro": 1.5, "intro2": 0, "fade_in": 0,
            "fade_out": 0.5, "hook_in": 0.2, "hook_out": 2.0, "start_next_soft": 0,
            "start_next_hard": 0,
        })
    };
    let rows = json!([
        row("Track 1.wav", "One", 10, 0),
        row("Track 2.wav", "Two", 15, 8),
        row("Nowhere.wav", "Lost", 10, 0),
        row("Track 3.wav", "Three", 10, 0),
        row("TRACK 3.wav", "Three again", 10, 0),
    ]);
    let (status, _) = viewer
        .send(
            Method::POST,
            "/library/import/preview",
            json!({"rows": rows}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, preview) = admin
        .send(
            Method::POST,
            "/library/import/preview",
            json!({"rows": rows}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let statuses: Vec<&str> = preview["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        [
            "matched",
            "matched",
            "not_found",
            "duplicate_in_export",
            "duplicate_in_export"
        ]
    );
    let first = &preview["rows"][0];
    assert_eq!(first["track_id"], one);
    assert_eq!(first["values"]["artist"], "Band's");
    assert_eq!(first["values"]["intro_ms"], 1000, "moved by the cue-in");
    assert_eq!(
        first["values"]["hook_in_ms"],
        Value::Null,
        "before the cue-in"
    );
    assert_eq!(first["values"]["fade_out_ms"], 500);
    assert_eq!(first["length_mismatch"], false);
    assert_eq!(first["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(first["conflicts"], json!([]));
    let second = &preview["rows"][1];
    assert_eq!(second["current"]["title"], "Fixed");
    assert_eq!(second["conflicts"], json!(["title"]));
    assert_eq!(preview["categories"].as_array().unwrap().len(), 2);
    assert_eq!(preview["categories"][1]["can_schedule"], false);
    assert_eq!(preview["problems"][0]["name"], "a remplacer");

    // Track 2's conflict settled by keeping Heron's title.
    let mut kept = second["values"].clone();
    kept.as_object_mut().unwrap().remove("title");
    let (status, import) = admin
        .send(
            Method::POST,
            "/library/import/apply",
            json!({
                "categories": [
                    {"id": 10, "name": "gold"},
                    {"id": 15, "name": "frigo", "disable_tracks": true},
                ],
                "problems": [{"id": 8, "action": "label_and_disable", "label": "Problème : a remplacer"}],
                "rows": [
                    {"track_id": one, "values": first["values"]},
                    {"track_id": two, "values": kept},
                ],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{import}");
    assert_eq!(import["summary"]["tracks"], 2);
    assert_eq!(
        import["summary"]["categories_created"],
        json!(["gold", "frigo"])
    );
    assert_eq!(import["summary"]["disabled"], 1);

    let detail = admin.get(&format!("/tracks/{one}")).await;
    assert_eq!(detail["title"], "One");
    assert_eq!(detail["artist"], "Band's");
    assert_eq!(detail["year"], 1980);
    assert_eq!(detail["markers"]["intro_ms"], 1000);
    assert_eq!(detail["overrides"]["crossfade"]["fade_out"]["time_ms"], 500);
    assert_eq!(detail["categories"][0]["name"], "gold");
    assert_eq!(detail["enabled"], true);
    let detail = admin.get(&format!("/tracks/{two}")).await;
    assert_eq!(detail["title"], "Fixed", "Heron's title kept");
    assert_eq!(detail["enabled"], false);
    assert_eq!(detail["labels"][0]["name"], "Problème : a remplacer");
    assert_eq!(
        admin.get(&format!("/tracks/{three}")).await["markers"],
        Value::Null
    );
    let added: i64 = sqlx::query_scalar("SELECT YEAR(created_at) FROM tracks WHERE id = ?")
        .bind(one)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(added, 2021);

    // Undone: every track as it was.
    let list = admin.get("/library/imports").await;
    assert_eq!(list[0]["id"], import["id"]);
    let undo = format!("/library/imports/{}/undo", import["id"]);
    let (status, undone) = admin.send(Method::POST, &undo, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{undone}");
    assert!(undone["undone_at_unix_ms"].is_i64());
    let detail = admin.get(&format!("/tracks/{one}")).await;
    assert_eq!(detail["title"], "Track 1");
    assert_eq!(detail["artist"], "Artist 1");
    assert_eq!(detail["year"], Value::Null);
    assert_eq!(detail["markers"], Value::Null);
    assert_eq!(detail["overrides"]["crossfade"]["fade_out"], Value::Null);
    assert_eq!(detail["categories"], json!([]));
    assert_eq!(detail["tag_overrides"], json!([]));
    let detail = admin.get(&format!("/tracks/{two}")).await;
    assert_eq!(detail["title"], "Fixed");
    assert_eq!(detail["tag_overrides"], json!(["title"]));
    assert_eq!(detail["enabled"], true);
    assert_eq!(detail["labels"], json!([]));
    let added: i64 = sqlx::query_scalar("SELECT YEAR(created_at) FROM tracks WHERE id = ?")
        .bind(one)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(added > 2021);
    let (status, _) = admin.send(Method::POST, &undo, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn medleys_are_planned_rendered_and_linked() {
    let Some(station) = Station::start().await else {
        return;
    };
    let pool = &station.db.pool;
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let tracks = station.tracks.clone();

    // Songs 1 to 4 with a hook, track 5 a liner; the medley goes in Medleys.
    let mut categories = Vec::new();
    for name in ["Songs", "Liners", "Medleys"] {
        let (status, category) = admin
            .send(Method::POST, "/categories", json!({"name": name}))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        categories.push(category["id"].as_u64().unwrap());
    }
    let [songs, liners, target] = categories[..] else {
        panic!("three categories");
    };
    for (index, track) in tracks.iter().enumerate() {
        let category = if index < 4 { songs } else { liners };
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{track}/categories"),
                json!([{"category_id": category}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        if index < 4 {
            let (status, _) = admin
                .send(
                    Method::PUT,
                    &format!("/tracks/{track}/hook"),
                    json!({"hook_in_ms": 500, "hook_out_ms": 2_500}),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
        }
    }

    // Where medleys go and where their liners come from: the settings.
    let defaults = viewer.get("/settings/medleys").await;
    assert_eq!(defaults["defaults"]["hooks"], 3);
    assert_eq!(defaults["defaults"]["liner_at_start"], true);
    assert_eq!(defaults["name_from"], "artists");
    let settings = json!({"category_id": target, "liner_category_id": liners});
    let (status, _) = viewer
        .send(Method::PUT, "/settings/medleys", settings.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/medleys",
            json!({"category_id": 999_999}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "no such category");
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/medleys",
            json!({"defaults": {"hooks": 1}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "one hook");
    let (status, saved) = admin.send(Method::PUT, "/settings/medleys", settings).await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let recipe = json!({
        "category_ids": [songs], "hooks": 3, "hook_min_ms": 1_000, "hook_max_ms": 2_500,
        "crossfade_ms": 200, "liner_every": 1, "liner_at_start": false, "liner_at_end": false
    });
    let medley = json!({"recipe": recipe});
    let (status, _) = viewer.send(Method::POST, "/medleys", medley.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin
        .send(
            Method::POST,
            "/medleys",
            json!({"recipe": {"category_ids": [songs], "hooks": 1}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, created) = admin.send(Method::POST, "/medleys", medley).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_u64().unwrap();
    assert!(
        viewer.get("/medleys").await[0]["name"].is_null(),
        "named when rendered"
    );

    // Planned: hook, liner, hook, liner, hook; three different songs.
    let (status, plan) = admin
        .request(Method::POST, &format!("/medleys/{id}/plan"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["qualified"], 4);
    let parts = plan["parts"].as_array().unwrap().clone();
    let kinds: Vec<&str> = parts.iter().map(|p| p["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["hook", "liner", "hook", "liner", "hook"]);
    let hooks: Vec<u64> = parts
        .iter()
        .filter(|p| p["kind"] == "hook")
        .map(|p| p["track_id"].as_u64().unwrap())
        .collect();
    assert_eq!(
        hooks.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
    assert!(
        parts
            .iter()
            .filter(|p| p["kind"] == "hook")
            .all(|p| p["from_ms"] == 500 && p["to_ms"] == 2_500)
    );
    // The fourth song, to replace one.
    let (status, other) = admin
        .send(
            Method::POST,
            &format!("/medleys/{id}/pick"),
            json!({"kind": "hook", "exclude": hooks}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    let fourth = other["track_id"].as_u64().unwrap();
    assert!(!hooks.contains(&fourth));
    // Or one chosen from the list: every song with a hook, by artist.
    let candidates = admin
        .get(&format!("/medleys/{id}/candidates?kind=hook"))
        .await;
    let listed: Vec<u64> = candidates
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["track_id"].as_u64().unwrap())
        .collect();
    assert_eq!(listed, tracks[..4]);
    assert_eq!(candidates[0]["to_ms"], 2_500);
    let liners_listed = admin
        .get(&format!("/medleys/{id}/candidates?kind=liner"))
        .await;
    assert_eq!(liners_listed[0]["track_id"], tracks[4]);
    // No opening liners set: the start takes the liners.
    let opening = admin
        .get(&format!("/medleys/{id}/candidates?kind=liner&slot=start"))
        .await;
    assert_eq!(opening[0]["track_id"], tracks[4]);
    let (status, _) = viewer
        .request(
            Method::GET,
            &format!("/medleys/{id}/candidates?kind=hook"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Previewed first, a hook nudged by 0.1 s: mixed outside the library.
    let preview = format!("/medleys/{id}/preview");
    let mut nudged = parts.clone();
    nudged[0]["to_ms"] = json!(nudged[0]["to_ms"].as_u64().unwrap() - 100);
    let mut past_the_end = parts.clone();
    past_the_end[0]["to_ms"] = json!(3_100);
    let (status, _) = admin
        .send(Method::POST, &preview, json!({"parts": past_the_end}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "past the file");
    let (status, task) = admin
        .send(Method::POST, &preview, json!({"parts": nudged}))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    assert_eq!(task["request"]["kind"], "preview_medley");
    let previewer = heron_librarian::tasks::TaskRunner::new(
        pool.clone(),
        heron_librarian::journal::Roots {
            library: station.library.path().to_path_buf(),
            incoming: station.library.path().join(".incoming"),
            staging: station.staging.path().to_path_buf(),
            covers: station.state.library.covers().clone(),
        },
        heron_librarian::identify::Endpoints::default(),
    );
    assert!(previewer.run_next().await.unwrap());
    let done = admin.get(&format!("/library/tasks/{}", task["id"])).await;
    assert_eq!(done["status"], "done", "{done}");
    let audio = viewer
        .http
        .get(format!("{}{preview}", viewer.base))
        .header("cookie", viewer.cookie.as_deref().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(audio.status(), StatusCode::OK);
    let wav = audio.bytes().await.unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    // 0.1 s shorter than the render below.
    let frames = (wav.len() - 44) / 4;
    assert_eq!(frames, (3 * 2_000 + 2 * 3_000 - 4 * 200 - 100) * 48);
    assert!(
        !station
            .library
            .path()
            .join(format!("medleys/medley-{id}.wav"))
            .exists(),
        "no medley rendered"
    );
    // Hidden next to the medleys: the scanner never sees it.
    assert!(
        station
            .library
            .path()
            .join(format!("medleys/.previews/medley-{id}.wav"))
            .exists()
    );
    assert!(
        admin.get(&format!("/medleys/{id}")).await["parts"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Rendered by heron-librarian into the library.
    let render = format!("/medleys/{id}/render");
    let (status, _) = admin
        .send(Method::POST, &render, json!({"parts": [{"kind": "liner", "track_id": tracks[4], "from_ms": 0, "to_ms": 1_000}]}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "no hook");
    let (status, task) = admin
        .send(Method::POST, &render, json!({"parts": parts}))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    assert_eq!(task["request"]["kind"], "render_medley");
    let roots = heron_librarian::journal::Roots {
        library: station.library.path().to_path_buf(),
        incoming: station.library.path().join(".incoming"),
        staging: station.staging.path().to_path_buf(),
        covers: station.state.library.covers().clone(),
    };
    let librarian = heron_librarian::tasks::TaskRunner::new(
        pool.clone(),
        roots,
        heron_librarian::identify::Endpoints::default(),
    );
    assert!(librarian.run_next().await.unwrap());
    let rendered = admin.get(&format!("/medleys/{id}")).await;
    assert_eq!(rendered["task_status"], "done", "{rendered}");
    // Named after its hooks' artists, in order.
    let artists: Vec<&str> = parts
        .iter()
        .filter(|p| p["kind"] == "hook")
        .map(|p| p["artist"].as_str().unwrap())
        .collect();
    let name = rendered["name"].as_str().unwrap().to_owned();
    assert!(name.starts_with("Medley 20"), "{name}");
    assert!(
        name.ends_with(&format!("({})", artists.join(" - "))),
        "{name}"
    );
    let path = rendered["path"].as_str().unwrap().to_owned();
    // The layout it was rendered with, as long as the file.
    assert_eq!(
        rendered["rendered_layout"]["length_ms"],
        3 * 2_000 + 2 * 3_000 - 4 * 200,
        "{rendered}"
    );
    assert_eq!(
        rendered["rendered_layout"]["parts"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    assert_eq!(path, format!("medleys/medley-{id}.wav"));
    assert_eq!(rendered["parts"].as_array().unwrap().len(), 5);
    let reader = hound::WavReader::open(station.library.path().join(&path)).unwrap();
    let frames = u64::from(reader.duration());
    // Three hooks of 2 s (whole, not cut) and two liners of 3 s, four crossfades of 0.2 s.
    assert_eq!(frames, (3 * 2_000 + 2 * 3_000 - 4 * 200) * 48);

    // The scanner finds the file: the medley gets its track, named after
    // it, in its category, and it sends no metadata.
    let track = sqlx::query("INSERT INTO tracks (path, duration_ms) VALUES (?, 9700)")
        .bind(&path)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id();
    assert_eq!(crate::medleys::link(pool).await.unwrap(), 1);
    assert_eq!(crate::medleys::link(pool).await.unwrap(), 0);
    let detail = admin.get(&format!("/tracks/{track}")).await;
    assert_eq!(detail["title"], name.as_str());
    assert_eq!(detail["categories"][0]["category_id"], target);
    assert_eq!(
        admin.get(&format!("/medleys/{id}")).await["track_id"],
        track
    );
    // Queued, it is sent to the engine without metadata.
    let (status, _) = admin
        .send(Method::POST, "/queue", json!({"track_ids": [track]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    eventually(
        "the medley in the queue",
        Duration::from_secs(10),
        || async {
            let queue = admin.get("/queue").await;
            queue
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["track"]["track_id"] == track)
                .map(|entry| assert_eq!(entry["track"]["no_metadata"], true, "{entry}"))
        },
    )
    .await;

    // Rendered: never built anew, but duplicated with its parts, not
    // rendered; the copy can be built, and the song left out comes first.
    let (status, _) = admin
        .request(Method::POST, &format!("/medleys/{id}/plan"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, copy) = admin
        .request(Method::POST, &format!("/medleys/{id}/duplicate"), None)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{copy}");
    assert_eq!(copy["recipe"], rendered["recipe"]);
    assert!(copy["name"].is_null());
    assert_eq!(
        copy["parts"],
        admin.get(&format!("/medleys/{id}")).await["parts"]
    );
    assert!(copy["path"].is_null() && copy["track_id"].is_null());
    let copy_id = copy["id"].as_u64().unwrap();
    let (status, plan) = admin
        .request(Method::POST, &format!("/medleys/{copy_id}/plan"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let again: Vec<u64> = plan["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["kind"] == "hook")
        .map(|p| p["track_id"].as_u64().unwrap())
        .collect();
    assert!(again.contains(&fourth), "{again:?}");
    // The medley's own track never gives a hook.
    assert!(!again.contains(&track));
    // Nor a liner, when medleys go in the liner category.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/medleys",
            json!({"category_id": target, "liner_category_id": target}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let as_liners = admin
        .get(&format!("/medleys/{copy_id}/candidates?kind=liner"))
        .await;
    assert_eq!(as_liners, json!([]), "the medley is no liner");

    // The copy rendered with the same hooks: a name of its own.
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/medleys/{copy_id}/render"),
            json!({"parts": parts}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        admin.get(&format!("/medleys/{copy_id}")).await["name"],
        format!("{name} (2)")
    );
    // Rendered again by title: named again, its track follows. Deleted:
    // its track stays.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/medleys",
            json!({"category_id": target, "liner_category_id": liners, "name_from": "titles"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .send(Method::POST, &render, json!({"parts": parts}))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let titles: Vec<&str> = parts
        .iter()
        .filter(|p| p["kind"] == "hook")
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    let retitled = admin.get(&format!("/tracks/{track}")).await["title"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        retitled.ends_with(&format!("({})", titles.join(" - "))),
        "{retitled}"
    );
    let (status, _) = admin
        .request(Method::DELETE, &format!("/medleys/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        admin.get(&format!("/tracks/{track}")).await["title"],
        retitled.as_str()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_selection_is_disabled_and_enabled() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let tracks = station.tracks.clone();
    let body = json!({"selection": {"track_ids": [tracks[0], tracks[1]]}, "enabled": false});
    let (status, _) = viewer
        .send(Method::POST, "/tracks/enabled", body.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, done) = admin.send(Method::POST, "/tracks/enabled", body).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        (done["tracks"].as_u64(), done["changed"].as_u64()),
        (Some(2), Some(2))
    );
    assert_eq!(admin.get("/tracks?filter=disabled").await["total"], 2);
    // Every disabled track enabled again, by filters; the others did not change.
    let (_, again) = admin
        .send(
            Method::POST,
            "/tracks/enabled",
            json!({"selection": {"filters": {}}, "enabled": true}),
        )
        .await;
    assert_eq!(
        (again["tracks"].as_u64(), again["changed"].as_u64()),
        (Some(tracks.len() as u64), Some(2))
    );
    assert_eq!(admin.get("/tracks?filter=disabled").await["total"], 0);
}

/// A track's play ratio: set, filtered, held back by rotation with a
/// counter per station, disabled at 0 and enabled again at a ratio.
#[tokio::test(flavor = "multi_thread")]
async fn play_ratios_hold_tracks_back_and_disable_them_at_zero() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let t = station.tracks.clone();
    let track = |id: u64| format!("/tracks/{id}");
    let (status, _) = viewer
        .send(Method::PATCH, &track(t[0]), json!({"play_ratio": 1}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for wrong in [
        json!({"play_ratio": 11}),
        json!({"enabled": true, "play_ratio": 0}),
    ] {
        let (status, body) = admin.send(Method::PATCH, &track(t[0]), wrong).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    // A second station, with no engine: its counters never move.
    let (status, body) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 80", "code": "hits80"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let other = body["id"].as_u64().unwrap();
    // 1 time out of 10, from 10, while autofill waits.
    let autofill = |on: bool| {
        let admin = &admin;
        async move {
            let (status, _) = admin
                .send(
                    Method::PUT,
                    "/settings/queue",
                    json!({"autofill": on, "min_queued": 3}),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    };
    autofill(false).await;
    let before = admin.get("/rotation/picks?limit=1").await[0]["id"]
        .as_u64()
        .unwrap_or(0);
    let (status, detail) = admin
        .send(
            Method::PATCH,
            &track(t[0]),
            json!({"play_ratio": 1, "reset_counters": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["play_ratio"], 1);
    assert_eq!(detail["enabled"], true);
    assert_eq!(detail["station_ratios"].as_array().unwrap().len(), 2);
    // Sorted by artist: Artist 2's track has the default ratio.
    assert_eq!(admin.get("/tracks").await["items"][1]["play_ratio"], 10);
    let below = admin.get("/tracks?ratio_max=9").await;
    assert_eq!(below["total"], 1, "{below}");
    assert_eq!(below["items"][0]["id"], t[0]);
    let sorted = admin.get("/tracks?sort=play_ratio").await;
    assert_eq!(sorted["items"][0]["id"], t[0]);

    // Autofill offers it, and its ratio holds it back: the log says so.
    autofill(true).await;
    let held = eventually(
        "a pick that held the track back",
        Duration::from_secs(90),
        || async {
            let picks = admin.get("/rotation/picks?limit=200").await;
            picks.as_array().unwrap().iter().find_map(|pick| {
                pick["explanation"]["held_by_ratio"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|held| held["track_id"] == t[0])
                    .then(|| pick.clone())
            })
        },
    )
    .await;
    let entry = held["explanation"]["held_by_ratio"]
        .as_array()
        .unwrap()
        .iter()
        .find(|held| held["track_id"] == t[0])
        .unwrap()
        .clone();
    assert_eq!(entry["ratio"], 1);
    assert!(
        (1..=9).contains(&entry["counter"].as_u64().unwrap()),
        "{entry}"
    );
    assert_ne!(held["track_id"], t[0]);
    assert!(
        held["held_tracks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|label| label["id"] == t[0] && label["title"] == "Track 1"),
        "{held}"
    );
    // Since, the track was picked only at 10.
    let picks = admin
        .get(&format!("/rotation/picks?track_id={}&limit=200", t[0]))
        .await;
    for pick in picks.as_array().unwrap() {
        if pick["id"].as_u64().unwrap() > before {
            assert_eq!(pick["explanation"]["counter"], 10, "{pick}");
        }
    }
    // A counter per station: the other one has not moved.
    let detail = admin.get(&track(t[0])).await;
    let counters = detail["station_ratios"].as_array().unwrap();
    let on = |id: u64| counters.iter().find(|c| c["station_id"] == id).unwrap()["counter"].clone();
    assert_eq!(on(other), 10);

    // Autofill off: nothing else moves the counters.
    autofill(false).await;
    let (status, detail) = admin
        .send(Method::PATCH, &track(t[0]), json!({"reset_counters": true}))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert!(
        detail["station_ratios"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["counter"] == 10),
        "{detail}"
    );
    // Queued by hand: plays, and its counter stays.
    let (status, body) = admin
        .send(Method::POST, "/queue", json!({"track_ids": [t[0]]}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entry = body["entry_ids"][0].as_u64().unwrap();
    let detail = admin.get(&track(t[0])).await;
    assert!(
        detail["station_ratios"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["counter"] == 10),
        "{detail}"
    );

    // 0 disables it: it keeps its ratio, leaves the queue and cannot be
    // queued by hand.
    let (status, detail) = admin
        .send(Method::PATCH, &track(t[0]), json!({"play_ratio": 0}))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["enabled"], false);
    assert_eq!(detail["play_ratio"], 1);
    assert_eq!(admin.get("/tracks?ratio_max=0").await["total"], 1);
    assert_eq!(admin.get("/tracks?filter=disabled").await["total"], 1);
    eventually(
        "the disabled track out of the queue",
        Duration::from_secs(10),
        || async {
            let queue = admin.get("/queue").await;
            (!queue
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["entry_id"] == entry))
            .then_some(())
        },
    )
    .await;
    let (status, body) = admin
        .send(Method::POST, "/queue", json!({"track_ids": [t[0]]}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // Enabled again, at the ratio asked for.
    let (status, detail) = admin
        .send(
            Method::PATCH,
            &track(t[0]),
            json!({"enabled": true, "play_ratio": 5, "reset_counters": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(
        (detail["enabled"].clone(), detail["play_ratio"].clone()),
        (json!(true), json!(5))
    );

    // In bulk: a ratio, 0 disabling, enabled again at one ratio for all.
    let selection = json!({"track_ids": [t[1], t[2]]});
    let (status, done) = admin
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": selection, "play_ratio": 2}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["changed"], 2);
    let low = admin.get("/tracks?ratio_min=1&ratio_max=3").await;
    assert_eq!(low["total"], 2, "{low}");
    let (status, done) = admin
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": selection, "play_ratio": 0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(admin.get("/tracks?filter=disabled").await["total"], 2);
    let (status, body) = admin
        .send(
            Method::POST,
            "/tracks/enabled",
            json!({"selection": selection, "enabled": false, "play_ratio": 4}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, done) = admin
        .send(
            Method::POST,
            "/tracks/enabled",
            json!({"selection": selection, "enabled": true, "play_ratio": 4, "reset_counters": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    for id in [t[1], t[2]] {
        let detail = admin.get(&track(id)).await;
        assert_eq!(
            (detail["enabled"].clone(), detail["play_ratio"].clone()),
            (json!(true), json!(4))
        );
    }
    station.stop().await;
}

/// Play ratios per station: a station's own ratio, a track disabled on
/// one station only, who disabled it, bulk edits and a CSV import for the
/// station shown, a station's counters reset, and the roles they take.
#[tokio::test(flavor = "multi_thread")]
async fn play_ratios_per_station() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let (_engine, _state_dir, second_url, grpc) = start_engine(station.library.path(), None).await;
    station.tasks.push(grpc);
    station.state.stations.set_engine("hits80", &second_url);
    let admin = station.signed_in("admin").await;
    let t = station.tracks.clone();
    let track = |id: u64| format!("/tracks/{id}");
    // Autofill off: only this test moves the counters.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/queue",
            json!({"autofill": false, "min_queued": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(Duration::from_secs(1)).await;
    sqlx::query("DELETE FROM track_ratio_counters")
        .execute(&station.db.pool)
        .await
        .unwrap();
    let first = admin.get("/stations").await[0]["id"].as_u64().unwrap();
    let first_code = admin.get("/stations").await[0]["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, body) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 80", "code": "hits80"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let second = body["id"].as_u64().unwrap();
    let (status, category) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Both", "color": null, "station_ids": [first, second]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{category}");
    for id in &t {
        let (status, body) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{id}/categories"),
                json!([{"category_id": category["id"]}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    // A viewer of what is shared and of the first station, operator of
    // the second.
    let (status, body) = admin
        .send(
            Method::POST,
            "/users",
            json!({"username": "eighties", "password": "correct horse", "role": "viewer",
                   "station_roles": [{"station_id": first, "role": "viewer"},
                                     {"station_id": second, "role": "operator"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let eighties = station.signed_in("eighties").await;
    let viewer = station.signed_in("viewer").await;
    let on = |detail: &Value, station_id: u64| -> Value {
        detail["station_ratios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|on| on["station_id"] == station_id)
            .unwrap()
            .clone()
    };

    // Roles: the station's for its ratios, the shared one for the default.
    for (client, patch, expected) in [
        (
            &eighties,
            json!({"station_id": first, "play_ratio": 3}),
            StatusCode::FORBIDDEN,
        ),
        (&eighties, json!({"play_ratio": 3}), StatusCode::FORBIDDEN),
        (
            &viewer,
            json!({"station_id": second, "play_ratio": 3}),
            StatusCode::NOT_FOUND,
        ),
        (
            &admin,
            json!({"default_ratio": true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            &admin,
            json!({"station_id": second, "default_ratio": true, "play_ratio": 3}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (status, body) = client.send(Method::PATCH, &track(t[0]), patch).await;
        assert_eq!(status, expected, "{body}");
    }
    let (status, detail) = eighties
        .send(
            Method::PATCH,
            &track(t[0]),
            json!({"station_id": second, "play_ratio": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["play_ratio"], 10);
    let there = on(&detail, second);
    assert_eq!(
        (&there["play_ratio"], &there["ratio"], &there["enabled"]),
        (&json!(3), &json!(3), &json!(true))
    );
    let here = on(&detail, first);
    assert_eq!(
        (&here["play_ratio"], &here["ratio"]),
        (&Value::Null, &json!(10))
    );
    // The list shows the station's ratio, and filters on it.
    let page = admin
        .get(&format!("/tracks?on_station_id={second}&ratio_max=3"))
        .await;
    assert_eq!(page["total"], 1, "{page}");
    assert_eq!(page["items"][0]["station_ratio"]["ratio"], 3);
    assert_eq!(
        admin
            .get(&format!("/tracks?on_station_id={first}&ratio_max=3"))
            .await["total"],
        0
    );
    assert_eq!(
        admin.get("/tracks").await["items"][0]["station_ratio"],
        Value::Null
    );

    // 0 on the first station disables it there only, and says who did.
    let (status, detail) = admin
        .send(
            Method::PATCH,
            &track(t[0]),
            json!({"station_id": first, "play_ratio": 0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["enabled"], true);
    let here = on(&detail, first);
    assert_eq!(
        (&here["enabled"], &here["ratio"]),
        (&json!(false), &json!(0))
    );
    assert_eq!(here["disabled_by"], "admin");
    assert!(here["disabled_at_unix_ms"].as_i64().unwrap() > 0);
    assert_eq!(on(&detail, second)["ratio"], 3);
    let disabled = |station_id: u64| {
        let admin = &admin;
        async move {
            admin
                .get(&format!(
                    "/tracks?filter=disabled&on_station_id={station_id}"
                ))
                .await["total"]
                .clone()
        }
    };
    assert_eq!(disabled(first).await, 1);
    assert_eq!(disabled(second).await, 0);
    assert_eq!(admin.get("/tracks?filter=disabled").await["total"], 0);
    let listed = admin
        .get(&format!("/tracks?on_station_id={first}&ratio_max=0"))
        .await;
    assert_eq!(listed["items"][0]["station_ratio"]["disabled_by"], "admin");
    // Not queued by hand there, never picked by its rotation.
    let (status, body) = admin
        .send(Method::POST, "/queue", json!({"track_ids": [t[0]]}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("disabled on this station"),
        "{body}"
    );
    let (status, report) = admin
        .send(
            Method::POST,
            "/rotation/simulate",
            json!({"hours": 1, "seed": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let picks = report["simulation"]["picks"].as_array().unwrap();
    assert!(!picks.is_empty());
    assert!(picks.iter().all(|pick| pick["track_id"] != t[0]));
    assert!(
        picks.iter().all(|pick| pick["explanation"]["ratio"] == 10),
        "{}",
        picks[0]
    );
    // Queued, then disabled on the station: it leaves the queue.
    let (status, body) = admin
        .send(Method::POST, "/queue", json!({"track_ids": [t[1]]}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entry = body["entry_ids"][0].as_u64().unwrap();
    let (status, body) = admin
        .send(
            Method::PATCH,
            &track(t[1]),
            json!({"station_id": first, "enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    eventually(
        "the track out of the queue",
        Duration::from_secs(10),
        || async {
            let queue = admin.get("/queue").await;
            (!queue
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["entry_id"] == entry))
            .then_some(())
        },
    )
    .await;
    // Enabled again there at a ratio, then back to the default.
    let (status, detail) = admin
        .send(
            Method::PATCH,
            &track(t[0]),
            json!({"station_id": first, "enabled": true, "play_ratio": 6}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let here = on(&detail, first);
    assert_eq!(
        (&here["enabled"], &here["play_ratio"], &here["disabled_by"]),
        (&json!(true), &json!(6), &Value::Null)
    );
    for id in [t[0], t[1]] {
        let (status, detail) = admin
            .send(
                Method::PATCH,
                &track(id),
                json!({"station_id": first, "default_ratio": true}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        let here = on(&detail, first);
        assert_eq!(
            (&here["enabled"], &here["play_ratio"], &here["ratio"]),
            (&json!(true), &Value::Null, &json!(10))
        );
    }

    // In bulk, for the station shown.
    let selection = json!({"filters": {"on_station_id": second}});
    let (status, body) = eighties
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": selection, "play_ratio": 2}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, done) = eighties
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": selection, "play_ratio": 2, "station_id": second}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["tracks"], 5);
    let (status, done) = eighties
        .send(
            Method::POST,
            "/tracks/enabled",
            json!({"selection": {"track_ids": [t[3]]}, "enabled": false, "station_id": second}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["changed"], 1);
    let (status, _) = eighties
        .send(
            Method::POST,
            "/tracks/enabled",
            json!({"selection": {"track_ids": [t[3]]}, "enabled": false, "station_id": first}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let detail = admin.get(&track(t[3])).await;
    assert_eq!(detail["play_ratio"], 10);
    let there = on(&detail, second);
    assert_eq!(
        (
            &there["enabled"],
            &there["play_ratio"],
            &there["disabled_by"]
        ),
        (&json!(false), &json!(2), &json!("eighties"))
    );
    assert_eq!(on(&detail, first)["ratio"], 10);

    // Disabled everywhere: who and when, on its page and in the list.
    let (status, detail) = admin
        .send(Method::PATCH, &track(t[4]), json!({"enabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["disabled_by"], "admin");
    assert!(detail["disabled_at_unix_ms"].as_i64().unwrap() > 0);
    assert_eq!(on(&detail, second)["ratio"], 0);
    let listed = admin.get("/tracks?filter=disabled").await;
    assert_eq!(listed["items"][0]["disabled_by"], "admin");
    let (status, detail) = admin
        .send(
            Method::PATCH,
            &track(t[4]),
            json!({"enabled": true, "play_ratio": 10}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["disabled_by"], Value::Null);

    // A station's counters, all reset to 10. Its queue manager may have
    // saved a counter of its own for a track it picked meanwhile.
    for (station_id, id, counter) in [(first, t[2], 4), (first, t[3], 10), (second, t[2], 7)] {
        sqlx::query(
            "INSERT INTO track_ratio_counters (station_id, track_id, counter) VALUES (?, ?, ?)
             ON DUPLICATE KEY UPDATE counter = VALUES(counter)",
        )
        .bind(station_id)
        .bind(id)
        .bind(counter)
        .execute(&station.db.pool)
        .await
        .unwrap();
    }
    let reset = |station_id: u64| format!("/stations/{station_id}/rotation/counters/reset");
    let (status, _) = eighties.send(Method::POST, &reset(first), json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Counters the queue managers saved meanwhile count too; those of the
    // other station stay.
    let (status, body) = admin.send(Method::POST, &reset(first), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["reset"].as_i64().unwrap() >= 1, "{body}");
    let detail = admin.get(&track(t[2])).await;
    assert_eq!(on(&detail, first)["counter"], 10);
    let kept: Option<u8> = sqlx::query_scalar(
        "SELECT counter FROM track_ratio_counters WHERE station_id = ? AND track_id = ?",
    )
    .bind(second)
    .bind(t[2])
    .fetch_optional(&station.db.pool)
    .await
    .unwrap();
    assert!(kept.is_some(), "the other station's counter stays");
    let (status, body) = eighties.send(Method::POST, &reset(second), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["reset"].as_i64().unwrap() >= 1, "{body}");

    // A CSV import: a dry run reports, then the values are written.
    let csv = "path;hits80;nope\n\
               Artist 1/Track 1.wav;0,5;x\n\
               Artist 2/Track 2.wav;0;\n\
               Gone/Gone.wav;0.3;\n\
               Artist 3/Track 3.wav;0.25;\n";
    let import = "/tracks/ratios/import";
    let (status, report) = eighties
        .post_bytes(&format!("{import}?dry_run=true"), csv.into(), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        (
            &report["rows"],
            &report["matched"],
            &report["values"],
            &report["changed"]
        ),
        (&json!(4), &json!(3), &json!(2), &json!(0))
    );
    assert_eq!(report["columns"], json!(["hits80"]));
    assert_eq!(report["unknown_columns"], json!(["nope"]));
    assert_eq!(
        report["unmatched"],
        json!([{"line": 4, "name": "Gone/Gone.wav", "reason": "not_found"}])
    );
    assert_eq!(
        report["invalid"],
        json!([{"line": 5, "column": "hits80", "value": "0.25"}])
    );
    assert_eq!(on(&admin.get(&track(t[0])).await, second)["ratio"], 2);
    let (status, report) = eighties.post_bytes(import, csv.into(), None).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["changed"], 2);
    assert_eq!(on(&admin.get(&track(t[0])).await, second)["ratio"], 5);
    let there = on(&admin.get(&track(t[1])).await, second);
    assert_eq!(
        (&there["enabled"], &there["ratio"]),
        (&json!(false), &json!(0))
    );
    // The default, and the first station: the shared role, and a role on
    // the first station.
    let csv = format!("filename,{first_code},default\ntrack 3.WAV,0.4,0.7\n");
    let (status, _) = eighties.post_bytes(import, csv.clone().into(), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, report) = admin.post_bytes(import, csv.into(), None).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["matched_by"], "filename");
    let detail = admin.get(&track(t[2])).await;
    assert_eq!(detail["play_ratio"], 7);
    assert_eq!(on(&detail, first)["ratio"], 4);
    assert_eq!(on(&detail, second)["ratio"], 2);
    let (status, body) = admin
        .post_bytes(import, "title,hits80\nA,1\n".into(), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_changes_reach_webhooks_and_the_public_queue() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let (mut received, receiver) = webhook_receiver().await;
    for (name, body) in [
        ("json", json!({})),
        (
            "next",
            json!({"content_type": "application/json",
                   "body_template": "{\"next\": \"{{next.title}}\", \"all\": {{upcoming_json}}}"}),
        ),
    ] {
        let mut webhook = json!({"name": name, "url": format!("http://{receiver}/{name}"),
                                 "events": ["queue.changed"]});
        webhook
            .as_object_mut()
            .unwrap()
            .extend(body.as_object().unwrap().clone());
        let (status, created) = admin.send(Method::POST, "/webhooks", webhook).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
    }

    // Autofill fills the queue and tracks start: queue.changed follows.
    let mut got = std::collections::HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while got.len() < 2 && Instant::now() < deadline {
        if let Ok(Some((_, name, headers, body))) =
            tokio::time::timeout(Duration::from_secs(1), received.recv()).await
        {
            assert_eq!(headers["x-heron-event"], "queue.changed");
            got.insert(name, body);
        }
    }
    let default: Value = serde_json::from_str(&got["json"]).unwrap();
    assert_eq!(default["event"], "queue.changed");
    let upcoming = default["upcoming"].as_array().unwrap();
    assert!(!upcoming.is_empty() && upcoming.len() <= 10, "{default}");
    for (position, track) in upcoming.iter().enumerate() {
        assert_eq!(track["position"], position);
        assert!(
            track["title"].as_str().unwrap().starts_with("Track "),
            "{track}"
        );
        assert!(track.get("track_id").is_none() && track.get("path").is_none());
    }
    let starts: Vec<u64> = upcoming
        .iter()
        .map(|track| track["planned_start_unix_ms"].as_u64().unwrap())
        .collect();
    assert!(
        starts.windows(2).all(|pair| pair[0] < pair[1]),
        "{starts:?}"
    );
    let templated: Value = serde_json::from_str(&got["next"]).unwrap();
    assert_eq!(templated["next"], templated["all"][0]["title"]);

    // The same list without signing in, from any origin.
    let anyone = reqwest::Client::new();
    let response = anyone
        .get(format!("http://{}/api/v1/public/queue", station.api))
        .header("origin", "https://example.org")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    let queue: Value = response.json().await.unwrap();
    assert!(!queue["upcoming"].as_array().unwrap().is_empty(), "{queue}");
    assert_eq!(queue["upcoming"][0]["position"], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn deck_eq_and_presets() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;

    // Flat and bypassed until set.
    let eq = admin.get("/settings/eq").await;
    assert_eq!(eq["music"]["enabled"], false);
    assert_eq!(eq["fx"]["gains_db"].as_array().unwrap().len(), 10);

    let settings = json!({
        "music": {"enabled": true, "gains_db": [3.0, 2.0, 0.0, 0.0, -1.5, 0.0, 0.0, 1.0, 1.0, 0.0]},
        "fx": {"enabled": false, "gains_db": vec![0.0; 10]}
    });
    let (status, _) = viewer
        .send(Method::PUT, "/settings/eq", settings.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let mut too_much = settings.clone();
    too_much["music"]["gains_db"][0] = json!(12.5);
    let (status, body) = admin.send(Method::PUT, "/settings/eq", too_much).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // Every live socket follows, and the engine has it at once.
    let url = format!("ws://{}/api/v1/live?levels=false", station.api);
    let mut request = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    request
        .headers_mut()
        .insert("cookie", viewer.cookie.as_deref().unwrap().parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let mut next = async || -> Value {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("live message")
            .unwrap()
            .unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    };
    assert_eq!(next().await["type"], "state");
    let (status, body) = admin
        .send(Method::PUT, "/settings/eq", settings.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut eq_message = None;
    for _ in 0..50 {
        let message = next().await;
        if message["type"] == "eq" {
            eq_message = Some(message);
            break;
        }
    }
    assert_eq!(eq_message.expect("an eq message")["settings"], settings);
    let config = station
        .state
        .stations
        .first()
        .engine
        .config()
        .await
        .unwrap();
    let engine_eq: Value = serde_json::from_str(&config.eq_settings_json).unwrap();
    assert_eq!(engine_eq, settings);
    assert_eq!(admin.get("/settings/eq").await, settings);

    // Presets: those of a fresh install are there; save, rename, delete.
    let presets = viewer.get("/eq-presets").await;
    // As a set: the database's collation decides how "R&B / soul" sorts.
    let mut names: Vec<&str> = presets
        .as_array()
        .unwrap()
        .iter()
        .map(|preset| preset["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    let mut expected = [
        "Acoustic",
        "Chill",
        "Classical",
        "Country",
        "Dance",
        "Flat",
        "Hip-hop",
        "Jazz",
        "Latin",
        "Pop",
        "Progressive metal",
        "Progressive rock",
        "R&B / soul",
        "Reggae",
        "Rock",
        "Soft rock",
        "Talk",
        "Voice (FX)",
    ];
    expected.sort_unstable();
    assert_eq!(names, expected);
    let flat = presets
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Flat")
        .unwrap();
    assert_eq!(flat["gains_db"], json!(vec![0.0; 10]));
    let rock = presets
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Rock")
        .unwrap();
    assert_eq!(rock["gains_db"][0], 5.0);
    let gains = settings["music"]["gains_db"].clone();
    let (status, _) = viewer
        .send(
            Method::POST,
            "/eq-presets",
            json!({"name": "Warm", "gains_db": gains}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, preset) = admin
        .send(
            Method::POST,
            "/eq-presets",
            json!({"name": "Warm", "gains_db": gains}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{preset}");
    assert_eq!(preset["gains_db"], settings["music"]["gains_db"]);
    let id = preset["id"].as_u64().unwrap();
    let (status, _) = admin
        .send(
            Method::POST,
            "/eq-presets",
            json!({"name": "flat", "gains_db": gains}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = admin
        .send(
            Method::POST,
            "/eq-presets",
            json!({"name": "Nine", "gains_db": vec![0; 9]}),
        )
        .await;
    assert!(status.is_client_error());
    let (status, renamed) = admin
        .send(
            Method::PUT,
            &format!("/eq-presets/{id}"),
            json!({"name": "Warm vinyl", "gains_db": gains}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Warm vinyl");
    let (status, _) = admin
        .request(Method::DELETE, &format!("/eq-presets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin
        .request(Method::DELETE, &format!("/eq-presets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The EQ keeps what was loaded.
    assert_eq!(admin.get("/settings/eq").await, settings);
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agc_chain_and_presets() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;

    // Bypassed until set, the final limiter on.
    let agc = admin.get("/settings/agc").await;
    assert_eq!(agc["bypass_all"], true);
    assert_eq!(agc["final_stage"]["enabled"], true);
    assert_eq!(agc["five_band"]["bands"].as_array().unwrap().len(), 5);

    let presets = viewer.get("/agc-presets").await;
    let names: Vec<&str> = presets
        .as_array()
        .unwrap()
        .iter()
        .map(|preset| preset["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Dense", "Gentle", "Pop", "Rock", "Talk"]);
    let pop = presets
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Pop")
        .unwrap();
    assert_eq!(pop["settings"]["five_band"]["enabled"], true);
    assert!(pop["settings"]["bypass_all"].is_null());

    // Loading a preset: its blocks into the chain, switched on.
    let mut settings = pop["settings"].clone();
    settings["bypass_all"] = json!(false);
    let (status, _) = viewer
        .send(Method::PUT, "/settings/agc", settings.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let mut backwards = settings.clone();
    backwards["five_band"]["crossovers_hz"] = json!([100, 400, 300, 6400]);
    let (status, body) = admin.send(Method::PUT, "/settings/agc", backwards).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let mut unknown = settings.clone();
    unknown["final_stage"]["mode"] = json!("brickwall");
    let (status, _) = admin.send(Method::PUT, "/settings/agc", unknown).await;
    assert!(status.is_client_error());

    // Every live socket follows, and the engine has it at once.
    let url = format!("ws://{}/api/v1/live", station.api);
    let mut request = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    request
        .headers_mut()
        .insert("cookie", viewer.cookie.as_deref().unwrap().parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let mut next = async || -> Value {
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("live message")
            .unwrap()
            .unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    };
    assert_eq!(next().await["type"], "state");
    let (status, saved) = admin
        .send(Method::PUT, "/settings/agc", settings.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let mut agc_message = None;
    let mut agc_levels = None;
    for _ in 0..100 {
        let message = next().await;
        if message["type"] == "agc" {
            agc_message = Some(message);
        } else if message["type"] == "levels" && message["levels"]["agc"].is_object() {
            agc_levels = Some(message);
        }
        if agc_message.is_some() && agc_levels.is_some() {
            break;
        }
    }
    assert_eq!(agc_message.expect("an agc message")["settings"], saved);
    let levels = agc_levels.expect("the chain's meters")["levels"]["agc"].clone();
    assert_eq!(levels["five_band_db"].as_array().unwrap().len(), 5);
    assert_eq!(levels["dual_band_db"].as_array().unwrap().len(), 2);
    assert!(levels["final_stage_db"].as_f64().unwrap() <= 0.0);
    assert!(levels["input"]["peak_left"].is_number());
    let config = station
        .state
        .stations
        .first()
        .engine
        .config()
        .await
        .unwrap();
    let engine_agc: heron_core::AgcSettings =
        serde_json::from_str(&config.agc_settings_json).unwrap();
    assert_eq!(serde_json::to_value(engine_agc).unwrap(), saved);
    assert_eq!(admin.get("/settings/agc").await, saved);

    // Presets: save, rename, delete; names are unique.
    let blocks = pop["settings"].clone();
    let (status, _) = viewer
        .send(
            Method::POST,
            "/agc-presets",
            json!({"name": "Morning", "settings": blocks}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, preset) = admin
        .send(
            Method::POST,
            "/agc-presets",
            json!({"name": "Morning", "settings": {"gated_agc": {"enabled": true}}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{preset}");
    assert_eq!(preset["settings"]["gated_agc"]["enabled"], true);
    assert_eq!(preset["settings"]["final_stage"]["ceiling_db"], -1.0);
    let id = preset["id"].as_u64().unwrap();
    let (status, _) = admin
        .send(
            Method::POST,
            "/agc-presets",
            json!({"name": "pop", "settings": blocks}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = admin
        .send(
            Method::POST,
            "/agc-presets",
            json!({"name": "Bypassed", "settings": {"bypass_all": true}}),
        )
        .await;
    assert!(status.is_client_error());
    let (status, renamed) = admin
        .send(
            Method::PUT,
            &format!("/agc-presets/{id}"),
            json!({"name": "Morning show", "settings": blocks}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Morning show");
    assert_eq!(renamed["settings"], blocks);
    let (status, _) = admin
        .request(Method::DELETE, &format!("/agc-presets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = admin
        .request(Method::DELETE, &format!("/agc-presets/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The chain keeps what was loaded.
    assert_eq!(admin.get("/settings/agc").await, saved);
    station.stop().await;
}

/// Two stations, each with its own engine, play at once from the same
/// library, each with its own clocks, queue and history; the routes of a
/// single station still serve the first.
#[tokio::test(flavor = "multi_thread")]
async fn two_stations_play_at_once() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let (_engine, _state_dir, second_url, grpc) = start_engine(station.library.path(), None).await;
    station.tasks.push(grpc);
    station.state.stations.set_engine("hits80", &second_url);
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;

    // The first station is the existing one, named from the deployment.
    let stations = admin.get("/stations").await;
    assert_eq!(stations[0]["code"], "main", "{stations}");
    assert_eq!(stations[0]["name"], "Test");
    assert_eq!(stations[0]["engine_variable"], "HERON_ENGINE_URL");
    assert_eq!(stations[0]["engine_configured"], true);

    // A station whose engine variable is not set stays off the air.
    let (status, waiting) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 90", "code": "hits90"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{waiting}");
    assert_eq!(waiting["engine_variable"], "HERON_ENGINE_URL_HITS90");
    assert_eq!(waiting["engine_configured"], false);
    let waiting = waiting["id"].as_u64().unwrap();
    let (status, _) = admin
        .request(Method::GET, &format!("/stations/{waiting}/player"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = admin
        .request(Method::DELETE, &format!("/stations/{waiting}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Nor does one whose engine is another station's: the same address,
    // its case and a trailing slash aside.
    let first_url = station.state.stations.first().engine_url().to_owned();
    station
        .state
        .stations
        .set_engine("hits70", &format!("{}/", first_url.to_uppercase()));
    let (status, sharing) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 70", "code": "hits70"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sharing}");
    assert_eq!(sharing["engine_configured"], true);
    assert_eq!(sharing["engine_taken_by"], "main", "{sharing}");
    let sharing = sharing["id"].as_u64().unwrap();
    let (status, _) = admin
        .request(Method::GET, &format!("/stations/{sharing}/player"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        admin.get(&format!("/stations/{sharing}")).await["engine_taken_by"],
        "main"
    );
    let (status, _) = admin
        .request(Method::DELETE, &format!("/stations/{sharing}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        admin.get("/stations").await[0]["engine_taken_by"],
        Value::Null
    );

    // Admins add a station with its own engine, set in the environment.
    let request = json!({"name": "Hits 80", "code": "hits80"});
    let (status, _) = viewer
        .send(Method::POST, "/stations", request.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for (bad, why) in [
        (json!({"name": "X", "code": "Hits 80"}), "code"),
        (json!({"name": "X", "code": "hits-80"}), "no - in codes"),
        (
            json!({"name": "X", "code": "x", "engine_url": "http://x:1"}),
            "no engine field",
        ),
        (json!({"name": "X", "code": "main"}), "taken code"),
    ] {
        let (status, body) = admin.send(Method::POST, "/stations", bad).await;
        assert!(status.is_client_error(), "{why}: {status} {body}");
    }
    let (status, created) = admin.send(Method::POST, "/stations", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["engine_configured"], true);
    let second = created["id"].as_u64().unwrap();
    let first = stations[0]["id"].as_u64().unwrap();

    // Its code names its engine's variable: it cannot change.
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/stations/{second}"),
            json!({"name": "Hits 80", "code": "hits8"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Both queues fill and both stations go on air.
    for id in [first, second] {
        eventually("both stations on air", Duration::from_secs(15), || async {
            let player = admin.get(&format!("/stations/{id}/player")).await;
            player["decks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|deck| deck["on_air"] == true)
                .then_some(())
        })
        .await;
    }
    // A track queued on one station; the single-station routes are the
    // first station's.
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/stations/{second}/queue"),
            json!({"track_ids": [station.tracks[0]], "position": 0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .send(
            Method::POST,
            "/queue",
            json!({"track_ids": [station.tracks[2]], "position": 0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let next = |queue: Value| queue[0]["track"]["track_id"].as_u64();
    assert_eq!(
        next(admin.get(&format!("/stations/{second}/queue")).await),
        Some(station.tracks[0])
    );
    assert_eq!(
        next(admin.get(&format!("/stations/{first}/queue")).await),
        Some(station.tracks[2])
    );
    assert_eq!(next(admin.get("/queue").await), Some(station.tracks[2]));

    // Each station has its own clocks and schedule.
    let (status, clock) = admin
        .send(
            Method::POST,
            &format!("/stations/{second}/clocks"),
            json!({"name": "Eighties", "slots": []}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    let clock_id = clock["id"].as_u64().unwrap();
    assert!(admin.get("/clocks").await.as_array().unwrap().is_empty());
    assert_eq!(
        admin.get(&format!("/stations/{second}/clocks")).await[0]["id"],
        clock_id
    );
    let (status, _) = admin
        .send(Method::GET, &format!("/clocks/{clock_id}"), Value::Null)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let hour = json!([{"day_of_week": 1, "hour": 6, "clock_id": clock_id}]);
    let (status, _) = admin.send(Method::PUT, "/schedule", hour.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = admin
        .send(Method::PUT, &format!("/stations/{second}/schedule"), hour)
        .await;
    assert_eq!(status, StatusCode::OK);

    // Settings of a station are its own; shared ones are not under a station.
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/stations/{second}/settings/queue"),
            json!({"autofill": true, "min_queued": 5}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(admin.get("/settings/queue").await["min_queued"], 5);
    let (status, _) = admin
        .request(
            Method::GET,
            &format!("/stations/{second}/settings/library"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = admin
        .request(Method::GET, &format!("/stations/{second}/tracks"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = admin
        .request(Method::GET, "/stations/999/queue", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Plays are recorded per station.
    let pool = &station.db.pool;
    eventually(
        "plays on both stations",
        Duration::from_secs(15),
        || async {
            let counts: Vec<(u64, i64)> = sqlx::query_as(
            "SELECT station_id, COUNT(*) FROM play_history GROUP BY station_id ORDER BY station_id",
        )
        .fetch_all(pool)
        .await
        .unwrap();
            (counts.len() == 2).then_some(())
        },
    )
    .await;
    let history = admin.get(&format!("/stations/{second}/history")).await;
    let in_db: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM play_history WHERE station_id = ?")
        .bind(second)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(history.as_array().unwrap().len() as i64 <= in_db);

    // Public routes per station, by code.
    let anonymous = station.client();
    anonymous.get("/public/stations/hits80/now-playing").await;
    anonymous.get("/public/stations/main/queue").await;
    let (status, _) = anonymous
        .request(Method::GET, "/public/stations/nope/now-playing", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A station is deleted only once nothing but its history is left.
    let (status, body) = admin
        .request(Method::DELETE, &format!("/stations/{second}"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = admin
        .request(Method::DELETE, &format!("/stations/{first}"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/stations/{second}/schedule"),
            json!([]),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .request(
            Method::DELETE,
            &format!("/stations/{second}/clocks/{clock_id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = admin
        .request(Method::DELETE, &format!("/stations/{second}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, _) = admin
        .request(Method::GET, &format!("/stations/{second}/queue"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(admin.get("/stations").await.as_array().unwrap().len(), 1);
    station.stop().await;
}

/// The track list on a station: the tracks of its categories, less those it
/// keeps out of its library; selections follow the same filter, and tracks
/// in no category show only without a station.
#[tokio::test(flavor = "multi_thread")]
async fn the_track_list_shows_a_station_s_library() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let (_engine, _state_dir, second_url, grpc) = start_engine(station.library.path(), None).await;
    station.tasks.push(grpc);
    station.state.stations.set_engine("hits80", &second_url);
    let admin = station.signed_in("admin").await;
    let first = admin.get("/stations").await[0]["id"].as_u64().unwrap();
    let (status, created) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 80", "code": "hits80"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let second = created["id"].as_u64().unwrap();

    // Shared, the first's, the second's, and one the second keeps out.
    let mut categories = Vec::new();
    for (name, stations) in [
        ("Shared", json!([first, second])),
        ("First", json!([first])),
        ("Second", json!([second])),
        ("Aside", json!([second])),
    ] {
        let (status, category) = admin
            .send(
                Method::POST,
                "/categories",
                json!({"name": name, "color": null, "station_ids": stations}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        categories.push(category["id"].as_u64().unwrap());
    }
    let [shared, firsts, seconds, aside] = categories[..] else {
        unreachable!()
    };
    let t = station.tracks.clone();
    for (track, in_categories) in [
        (t[0], vec![shared]),
        (t[1], vec![firsts]),
        (t[2], vec![seconds]),
        (t[3], vec![seconds, aside]),
    ] {
        let body: Vec<Value> = in_categories
            .iter()
            .map(|id| json!({"category_id": id}))
            .collect();
        let (status, answer) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{track}/categories"),
                Value::Array(body),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
    }
    let listed = |page: Value| -> Vec<u64> {
        let mut ids: Vec<u64> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|track| track["id"].as_u64().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(page["total"], ids.len(), "{page}");
        ids
    };
    assert_eq!(listed(admin.get("/tracks").await), t);
    assert_eq!(
        listed(admin.get(&format!("/tracks?on_station_id={first}")).await),
        [t[0], t[1]]
    );
    assert_eq!(
        listed(admin.get(&format!("/tracks?on_station_id={second}")).await),
        [t[0], t[2], t[3]]
    );
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/stations/{second}/settings/rotation"),
            json!({"kept_out_category_ids": [aside]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        listed(admin.get(&format!("/tracks?on_station_id={second}")).await),
        [t[0], t[2]]
    );
    // With other filters, and with the plays of another station.
    assert_eq!(
        listed(
            admin
                .get(&format!(
                    "/tracks?on_station_id={second}&station_id={first}&category_id={seconds}"
                ))
                .await
        ),
        [t[2]]
    );
    // No category: only without a station.
    assert_eq!(listed(admin.get("/tracks?no_category=true").await), [t[4]]);
    assert!(
        listed(
            admin
                .get(&format!("/tracks?no_category=true&on_station_id={first}"))
                .await
        )
        .is_empty()
    );

    // Every matching track of a selection follows the same filter.
    let (status, body) = admin
        .send(
            Method::POST,
            "/tracks/enabled",
            json!({"selection": {"filters": {"on_station_id": second}}, "enabled": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["tracks"], 2, "{body}");

    // The categories a station uses.
    let ids = |categories: Value| -> Vec<u64> {
        categories
            .as_array()
            .unwrap()
            .iter()
            .map(|category| category["id"].as_u64().unwrap())
            .collect()
    };
    let mut on_second = ids(admin.get(&format!("/categories?station_id={second}")).await);
    on_second.sort_unstable();
    assert_eq!(on_second, [shared, seconds, aside]);

    // A station that does not exist is not found.
    for path in [
        "/tracks?on_station_id=999".to_owned(),
        "/tracks?station_id=999".to_owned(),
        "/categories?station_id=999".to_owned(),
        "/medleys?station_id=999".to_owned(),
        format!("/tracks/{}/plays/grid?station_id=999", t[0]),
    ] {
        let (status, _) = admin.request(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = admin
        .send(
            Method::POST,
            "/tracks/enabled",
            json!({"selection": {"filters": {"on_station_id": 999}}, "enabled": true}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    station.stop().await;
}

/// Categories, restrictions and medleys are tagged with the stations they
/// are used on: a station only offers its own, untagging a station that
/// uses one is refused, and a medley's track follows its stations.
#[tokio::test(flavor = "multi_thread")]
async fn categories_restrictions_and_medleys_follow_their_stations() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let (_engine, _state_dir, second_url, grpc) = start_engine(station.library.path(), None).await;
    station.tasks.push(grpc);
    station.state.stations.set_engine("hits80", &second_url);
    let admin = station.signed_in("admin").await;
    let first = admin.get("/stations").await[0]["id"].as_u64().unwrap();
    let (status, created) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 80", "code": "hits80"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let second = created["id"].as_u64().unwrap();
    // Nothing queued but what the test queues.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/queue",
            json!({"autofill": false, "min_queued": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Categories: shared by both, the first station's (by default), the
    // second's.
    let mut ids = Vec::new();
    for body in [
        json!({"name": "80s", "color": null, "station_ids": [first, second]}),
        json!({"name": "Hits gold", "color": null}),
        json!({"name": "80s gold", "color": null, "station_ids": [second]}),
    ] {
        let (status, category) = admin.send(Method::POST, "/categories", body).await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        ids.push(category["id"].as_u64().unwrap());
    }
    let (eighties, gold, eighties_gold) = (ids[0], ids[1], ids[2]);
    assert_eq!(
        admin.get(&format!("/categories/{gold}")).await["station_ids"],
        json!([first])
    );
    let (status, _) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Nowhere", "color": null, "station_ids": []}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let offered = |categories: Value| -> Vec<u64> {
        categories
            .as_array()
            .unwrap()
            .iter()
            .map(|category| category["id"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(
        offered(admin.get(&format!("/categories?station_id={second}")).await),
        [eighties, eighties_gold]
    );

    // A station's clocks only take its categories.
    let clock = |category: u64| json!({"name": "Hour", "slots": [{"category_id": category}]});
    let (status, _) = admin
        .send(
            Method::POST,
            &format!("/stations/{second}/clocks"),
            clock(gold),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = admin
        .send(
            Method::POST,
            &format!("/stations/{second}/clocks"),
            clock(eighties_gold),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Untagging a station that uses the category is refused.
    let retag =
        |stations: Value| json!({"name": "80s gold", "color": null, "station_ids": stations});
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/categories/{eighties_gold}"),
            retag(json!([first])),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("clock Hour"),
        "{body}"
    );
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/categories/{eighties_gold}"),
            retag(json!([first, second])),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // A station only keeps its own categories out of its library, and
    // that uses them.
    let rotation = format!("/stations/{second}/settings/rotation");
    let (status, _) = admin
        .send(
            Method::PUT,
            &rotation,
            json!({"kept_out_category_ids": [gold]}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = admin
        .send(
            Method::PUT,
            &rotation,
            json!({"kept_out_category_ids": [eighties]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kept_out_category_ids"], json!([eighties]));
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/categories/{eighties}"),
            json!({"name": "80s", "color": null, "station_ids": [first]}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("keeps out"),
        "{body}"
    );
    let (status, _) = admin
        .send(Method::PUT, &rotation, json!({"kept_out_category_ids": []}))
        .await;
    assert_eq!(status, StatusCode::OK);

    // Restrictions apply to the stations they name, all by default.
    let (status, restriction) = admin
        .send(
            Method::POST,
            "/restrictions",
            json!({"target": {"kind": "category", "id": eighties},
                   "rule": {"kind": "daypart", "mode": "ban", "days": [1], "start_hour": 3,
                            "end_hour": 4},
                   "station_ids": [second]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{restriction}");
    assert_eq!(restriction["station_ids"], json!([second]));
    let applies = |station_id: u64| {
        let admin = &admin;
        async move {
            admin
                .get(&format!("/restrictions?station_id={station_id}"))
                .await
                .as_array()
                .unwrap()
                .len()
        }
    };
    assert_eq!((applies(first).await, applies(second).await), (0, 1));
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/categories/{eighties}"),
            json!({"name": "80s", "color": null, "station_ids": [first]}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Medleys: a station needs its medleys' category; each station has its own.
    let medley = |stations: Value| json!({"recipe": {"category_ids": [], "hooks": 2}, "station_ids": stations});
    let (status, _) = admin
        .send(Method::POST, "/medleys", medley(json!([second])))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/stations/{second}/settings/medleys"),
            json!({"category_id": gold}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "not the second's");
    for (path, category) in [
        (
            format!("/stations/{second}/settings/medleys"),
            eighties_gold,
        ),
        ("/settings/medleys".to_owned(), gold),
    ] {
        let (status, body) = admin
            .send(
                Method::PUT,
                &path,
                json!({"category_id": category, "name_from": "titles"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert_eq!(
        admin
            .get(&format!("/stations/{second}/settings/medleys"))
            .await["category_id"],
        eighties_gold
    );
    assert_eq!(admin.get("/settings/medleys").await["category_id"], gold);
    assert_eq!(
        admin
            .get(&format!("/stations/{second}/settings/medleys"))
            .await["name_from"],
        "titles"
    );
    let (status, _) = admin
        .send(
            Method::POST,
            "/medleys",
            json!({"recipe": {"category_ids": [gold], "hooks": 2}, "station_ids": [second]}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a category of another station"
    );
    let (status, made) = admin
        .send(Method::POST, "/medleys", medley(json!([second])))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let medley_id = made["id"].as_u64().unwrap();
    assert_eq!(made["station_ids"], json!([second]));
    let listed = |station_id: u64| {
        let admin = &admin;
        async move {
            admin
                .get(&format!("/medleys?station_id={station_id}"))
                .await
                .as_array()
                .unwrap()
                .len()
        }
    };
    assert_eq!((listed(first).await, listed(second).await), (0, 1));

    // The rendered track follows the medley's stations into their categories.
    let pool = &station.db.pool;
    let track = station.tracks[4];
    sqlx::query("UPDATE medleys SET track_id = ? WHERE id = ?")
        .bind(track)
        .bind(medley_id)
        .execute(pool)
        .await
        .unwrap();
    let categories = || async {
        sqlx::query_scalar::<_, u64>(
            "SELECT category_id FROM track_categories WHERE track_id = ? ORDER BY category_id",
        )
        .bind(track)
        .fetch_all(pool)
        .await
        .unwrap()
    };
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/medleys/{medley_id}"),
            medley(json!([second, first])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(categories().await, [gold, eighties_gold]);
    // Out of the first station: refused while it waits in its queue.
    let (status, _) = admin
        .send(
            Method::POST,
            "/queue",
            json!({"track_ids": [track], "position": 0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    eventually(
        "the medley in the queue",
        Duration::from_secs(5),
        || async {
            let queue = admin.get("/queue").await;
            waiting_tracks(&queue).contains(&track).then_some(())
        },
    )
    .await;
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/medleys/{medley_id}"),
            medley(json!([second])),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    eventually(
        "the medley out of the queue",
        Duration::from_secs(5),
        || async {
            let queue = admin.get("/queue").await;
            (!waiting_tracks(&queue).contains(&track)).then_some(())
        },
    )
    .await;
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/medleys/{medley_id}"),
            medley(json!([second])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(categories().await, [eighties_gold]);

    // A render: once the scanner found its file, the track goes into the
    // medleys' category of each of its stations.
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/medleys/{medley_id}"),
            medley(json!([second, first])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::query("DELETE FROM track_categories WHERE track_id = ?")
        .bind(track)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE medleys SET track_id = NULL, path = (SELECT path FROM tracks WHERE id = ?)
         WHERE id = ?",
    )
    .bind(track)
    .bind(medley_id)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(crate::medleys::link(pool).await.unwrap(), 1);
    assert_eq!(categories().await, [gold, eighties_gold]);

    // Its songs come from every category its stations can use.
    let (on_second, on_first) = (station.tracks[0], station.tracks[1]);
    for (song, category) in [(on_second, eighties_gold), (on_first, gold)] {
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{song}/categories"),
                json!([{"category_id": category}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{song}/hook"),
                json!({"hook_in_ms": 500, "hook_out_ms": 2_500}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    // Hooks of 2 s: the recipe takes them.
    let short = |stations: Value| json!({"recipe": {"category_ids": [], "hooks": 2, "hook_min_ms": 1_000}, "station_ids": stations});
    let (status, made) = admin
        .send(Method::POST, "/medleys", short(json!([second])))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let songs_of = |id: u64| {
        let admin = &admin;
        async move {
            let mut songs: Vec<u64> = admin
                .get(&format!("/medleys/{id}/candidates?kind=hook"))
                .await
                .as_array()
                .unwrap()
                .iter()
                .map(|part| part["track_id"].as_u64().unwrap())
                .collect();
            songs.sort_unstable();
            songs
        }
    };
    let id = made["id"].as_u64().unwrap();
    assert_eq!(songs_of(id).await, [on_second]);
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/medleys/{id}"),
            short(json!([second, first])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut both = vec![on_second, on_first];
    both.sort_unstable();
    assert_eq!(songs_of(id).await, both);

    // Each station has its own schedules.
    let (status, body) = admin
        .send(
            Method::POST,
            &format!("/stations/{second}/schedules"),
            json!({"name": "Christmas"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let names = |list: Value| -> Vec<String> {
        list["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|schedule| schedule["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(
        names(admin.get(&format!("/stations/{second}/schedules")).await)
            .contains(&"Christmas".to_owned())
    );
    assert!(
        !names(admin.get(&format!("/stations/{first}/schedules")).await)
            .contains(&"Christmas".to_owned())
    );
    assert!(!names(admin.get("/schedules").await).contains(&"Christmas".to_owned()));
    station.stop().await;
}

/// A station keeps the artists it plays apart from another station's plays
/// and queue, and says so in its rotation log; plays are counted per
/// station and in total.
#[tokio::test(flavor = "multi_thread")]
async fn rules_across_stations_and_plays_per_station() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let (_engine, _state_dir, second_url, grpc) = start_engine(station.library.path(), None).await;
    station.tasks.push(grpc);
    station.state.stations.set_engine("hits80", &second_url);
    let admin = station.signed_in("admin").await;
    let first = admin.get("/stations").await[0]["id"].as_u64().unwrap();
    // Before the second station starts: nothing it picks escapes the rule.
    let rotation = admin.get("/settings/rotation").await;
    let (status, created) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 80", "code": "hits80", "enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let second = created["id"].as_u64().unwrap();
    let mut across = rotation.clone();
    across["across"] = json!({"track_min": 30, "station_ids": [second]});
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/stations/{second}/settings/rotation"),
            across.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "not itself");
    across["across"] = json!({"track_min": 30});
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/stations/{second}/settings/rotation"),
            across,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let request = json!({"name": "Hits 80", "code": "hits80"});
    let (status, _) = admin
        .send(Method::PUT, &format!("/stations/{second}"), request)
        .await;
    assert_eq!(status, StatusCode::OK);

    // Five tracks, all played or queued by the first station within 30
    // minutes: the second station's picks are held back, then relaxed.
    let pick = eventually(
        "a pick held back by the first station",
        Duration::from_secs(20),
        || async {
            let picks = admin
                .get(&format!("/stations/{second}/rotation/picks"))
                .await;
            picks
                .as_array()
                .unwrap()
                .iter()
                .find(|pick| pick["explanation"]["held_elsewhere"].is_object())
                .cloned()
        },
    )
    .await;
    let explanation = &pick["explanation"];
    assert_eq!(
        explanation["held_elsewhere"]["station_id"], first,
        "{explanation}"
    );
    assert_eq!(explanation["held_elsewhere"]["rule"], "track");
    assert!(
        explanation["blocked"]["across"].as_u64() > Some(0),
        "{explanation}"
    );
    // Relaxed with the station's own track rule: halved at step 9.
    let relaxed = eventually("a relaxed pick", Duration::from_secs(20), || async {
        let picks = admin
            .get(&format!("/stations/{second}/rotation/picks"))
            .await;
        picks
            .as_array()
            .unwrap()
            .iter()
            .find(|pick| pick["explanation"]["step"].as_u64() >= Some(9))
            .cloned()
    })
    .await;
    assert!(
        relaxed["explanation"]["across"]["track_min"].as_u64() < Some(30),
        "{relaxed}"
    );
    // The first station looks at nobody: nothing is held there.
    let first_picks = admin
        .get(&format!("/stations/{first}/rotation/picks"))
        .await;
    assert!(
        first_picks
            .as_array()
            .unwrap()
            .iter()
            .all(|pick| pick["explanation"]["blocked"]["across"] == 0)
    );
    // A simulation of the second station is audited against the first's.
    let (status, simulation) = admin
        .send(
            Method::POST,
            &format!("/stations/{second}/rotation/simulate"),
            json!({"hours": 1, "seed": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{simulation}");
    assert_eq!(simulation["simulation"]["violations"], json!([]));

    // Plays per station, and in total.
    let track = eventually(
        "a track played on both",
        Duration::from_secs(30),
        || async {
            let rows: Vec<(u64,)> = sqlx::query_as(
                "SELECT track_id FROM play_history WHERE track_id IS NOT NULL
             GROUP BY track_id HAVING COUNT(DISTINCT station_id) = 2 LIMIT 1",
            )
            .fetch_all(&station.db.pool)
            .await
            .unwrap();
            rows.first().map(|row| row.0)
        },
    )
    .await;
    // Plays count once they are long enough (30 s by default): count them all.
    let mut programming = admin.get("/settings/programming").await;
    programming["min_play_s"] = json!(0);
    let (status, _) = admin
        .send(Method::PUT, "/settings/programming", programming)
        .await;
    assert_eq!(status, StatusCode::OK);
    let detail = admin.get(&format!("/tracks/{track}")).await;
    let plays = &detail["plays"];
    let stations: Vec<u64> = plays["stations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["station_id"].as_u64().unwrap())
        .collect();
    assert_eq!(stations, [first, second], "{plays}");
    let per_station: u64 = plays["stations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["play_count"].as_u64().unwrap())
        .sum();
    assert_eq!(plays["play_count"].as_u64(), Some(per_station));
    let count_on = |station_id: Option<u64>| {
        let admin = &admin;
        async move {
            let query = station_id.map_or_else(String::new, |id| format!("&station_id={id}"));
            let page = admin.get(&format!("/tracks?limit=50{query}")).await;
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"].as_u64() == Some(track))
                .map(|item| item["play_count"].as_u64().unwrap())
                .unwrap()
        }
    };
    let total = count_on(None).await;
    let on_second = count_on(Some(second)).await;
    assert!(on_second > 0 && on_second < total, "{on_second} of {total}");
    // When it played: one station's plays, or all of them added up.
    let grid_count = |query: String| {
        let admin = &admin;
        async move {
            let grid = admin
                .get(&format!("/tracks/{track}/plays/grid{query}"))
                .await;
            assert_eq!(grid["days"].as_array().unwrap().len(), 30, "{grid}");
            let by_day: usize = grid["days"]
                .as_array()
                .unwrap()
                .iter()
                .map(|day| day["plays"].as_array().unwrap().len())
                .sum();
            let by_hour: usize = grid["hours"]
                .as_array()
                .unwrap()
                .iter()
                .map(|cell| cell["plays"].as_array().unwrap().len())
                .sum();
            assert_eq!(by_day, by_hour, "{grid}");
            by_day as u64
        }
    };
    // The stations play on: as many plays as counted above, or more.
    let on_second_grid = grid_count(format!("?station_id={second}&days=90")).await;
    assert!(
        on_second_grid >= on_second,
        "{on_second_grid} < {on_second}"
    );
    let all = grid_count(String::new()).await;
    assert!(all >= total && all > on_second_grid, "{all} of {total}");
    assert_eq!(
        grid_count(format!("?station_id={second}&source=request")).await,
        0
    );
    let (status, _) = admin
        .send(
            Method::GET,
            &format!("/tracks/{track}/plays/grid?days=366"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = admin
        .send(
            Method::GET,
            &format!("/tracks/{track}/plays/grid?station_id=999"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    crate::artists::link_pending(&station.db.pool)
        .await
        .unwrap();
    let artist: u64 =
        sqlx::query_scalar("SELECT artist_id FROM track_artists WHERE track_id = ? LIMIT 1")
            .bind(track)
            .fetch_one(&station.db.pool)
            .await
            .unwrap();
    let artist = admin.get(&format!("/artists/{artist}")).await;
    assert_eq!(
        artist["plays"]["stations"].as_array().unwrap().len(),
        2,
        "{artist}"
    );
    station.stop().await;
}

/// Roles are given per station: a user with `none` on a station does not
/// see it; a viewer on one station and an operator on another can run only
/// the second.
#[tokio::test(flavor = "multi_thread")]
async fn roles_per_station() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let (_engine, _state_dir, second_url, grpc) = start_engine(station.library.path(), None).await;
    station.tasks.push(grpc);
    station.state.stations.set_engine("hits80", &second_url);
    let admin = station.signed_in("admin").await;
    let first = admin.get("/stations").await[0]["id"].as_u64().unwrap();
    let (status, created) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 80", "code": "hits80"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let second = created["id"].as_u64().unwrap();

    // Existing users keep their role on the first station only.
    let users = admin.get("/users").await;
    let viewer = users
        .as_array()
        .unwrap()
        .iter()
        .find(|user| user["username"] == "viewer")
        .unwrap();
    assert_eq!(
        viewer["station_roles"],
        json!([{"station_id": first, "role": "viewer"}])
    );

    let (status, body) = admin
        .send(
            Method::POST,
            "/users",
            json!({"username": "eighties", "password": "correct horse", "role": "viewer",
                   "station_roles": [{"station_id": first, "role": "viewer"},
                                     {"station_id": second, "role": "operator"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let operator = station.signed_in("eighties").await;
    assert_eq!(operator.get("/stations").await.as_array().unwrap().len(), 2);
    assert_eq!(
        operator.get("/auth/me").await["station_roles"][1]["role"],
        "operator"
    );
    let queue = json!({"autofill": true, "min_queued": 4});
    let (status, _) = operator
        .send(
            Method::PUT,
            &format!("/stations/{second}/settings/queue"),
            queue.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    for path in [
        "/settings/queue".to_owned(),
        format!("/stations/{first}/settings/queue"),
    ] {
        let (status, _) = operator.send(Method::PUT, &path, queue.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    // The shared role holds for what the stations share.
    let (status, _) = operator
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Nope", "color": null}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // `none` on the second station: it is not there.
    let viewer = station.signed_in("viewer").await;
    let stations = viewer.get("/stations").await;
    assert_eq!(stations.as_array().unwrap().len(), 1, "{stations}");
    // Nor on the shared routes that look at one station.
    let track = station.tracks[0];
    for path in [
        format!("/stations/{second}/queue"),
        format!("/stations/{second}"),
        format!("/tracks?station_id={second}"),
        format!("/tracks?on_station_id={second}"),
        format!("/categories?station_id={second}"),
        format!("/medleys?station_id={second}"),
        format!("/tracks/{track}/plays/grid?station_id={second}"),
    ] {
        let (status, _) = viewer.request(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    viewer.get("/queue").await;
    for path in [
        format!("/tracks?station_id={first}&on_station_id={first}"),
        format!("/categories?station_id={first}"),
        format!("/medleys?station_id={first}"),
        format!("/tracks/{track}/plays/grid?station_id={first}"),
    ] {
        viewer.get(&path).await;
    }

    // Roles change with the user; a shared admin sees every station.
    let id = admin
        .get("/users")
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|user| user["username"] == "viewer")
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let (status, body) = admin
        .send(
            Method::PATCH,
            &format!("/users/{id}"),
            json!({"station_roles": [{"station_id": second, "role": "viewer"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    viewer.get(&format!("/stations/{second}/queue")).await;
    let (status, _) = viewer.request(Method::GET, "/queue", None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "none on the first station now"
    );
    assert_eq!(admin.get("/stations").await.as_array().unwrap().len(), 2);
    station.stop().await;
}

#[tokio::test]
async fn schedules_play_during_their_periods() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let t = &station.tracks;
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/station",
            json!({"timezone": "Europe/Paris"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Two clocks, each calling its own category.
    let mut clocks = Vec::new();
    for (name, members) in [("Usual", &t[..3]), ("Christmas", &t[3..])] {
        let (status, category) = admin
            .send(Method::POST, "/categories", json!({"name": name}))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        for track in members {
            let (status, _) = admin
                .send(
                    Method::PUT,
                    &format!("/tracks/{track}/categories"),
                    json!([{"category_id": category["id"]}]),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, clock) = admin
            .send(
                Method::POST,
                "/clocks",
                json!({"name": name, "slots": [{"category_id": category["id"], "enforce_rules": false}]}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{clock}");
        clocks.push(clock["id"].as_u64().unwrap());
    }
    let week = |clock: u64, skip: Option<u8>| -> Value {
        json!(
            (1..=7u8)
                .flat_map(|day| (0..24u8).map(move |hour| (day, hour)))
                .filter(|(_, hour)| Some(*hour) != skip)
                .map(|(day, hour)| json!({"day_of_week": day, "hour": hour, "clock_id": clock}))
                .collect::<Vec<_>>()
        )
    };

    // The existing week is the default schedule's.
    let (status, _) = admin
        .send(Method::PUT, "/schedule", week(clocks[0], None))
        .await;
    assert_eq!(status, StatusCode::OK);
    let list = admin.get("/schedules").await;
    let default_id = list["schedules"][0]["id"].as_u64().unwrap();
    assert_eq!(list["schedules"].as_array().unwrap().len(), 1);
    assert_eq!(list["schedules"][0]["is_default"], true);
    assert_eq!(list["schedules"][0]["hours"], 168);
    assert_eq!(list["active_id"], default_id);
    assert!(list["next_change"].is_null());

    // A Christmas schedule, copied from the default one.
    let (status, _) = viewer
        .send(Method::POST, "/schedules", json!({"name": "Christmas"}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, christmas) = admin
        .send(
            Method::POST,
            "/schedules",
            json!({"name": "Christmas", "copy_from": default_id}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{christmas}");
    assert_eq!(christmas["hours"].as_array().unwrap().len(), 168);
    let christmas_id = christmas["id"].as_u64().unwrap();
    let (status, _) = admin
        .send(Method::POST, "/schedules", json!({"name": "Christmas"}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Periods: on the hour, not on the default schedule.
    let period =
        json!({"start": "2026-12-01T00:00:00", "end": "2027-01-01T00:00:00", "every_year": true});
    for (id, periods, expected) in [
        (
            default_id,
            json!([period]),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            christmas_id,
            json!([{"start": "2026-12-01T00:30:00", "end": "2027-01-01T00:00:00"}]),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            christmas_id,
            json!([{"start": "2027-01-01T00:00:00", "end": "2026-12-01T00:00:00"}]),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (status, body) = admin
            .send(
                Method::PUT,
                &format!("/schedules/{id}"),
                json!({"hours": [], "periods": periods}),
            )
            .await;
        assert_eq!(status, expected, "{body}");
    }
    // Christmas plays its clock, but leaves noon empty.
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/schedules/{christmas_id}"),
            json!({"name": "Noël", "hours": week(clocks[1], Some(12)), "periods": [period]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Noël");
    assert_eq!(body["periods"][0]["every_year"], true);
    assert_eq!(body["hours"].as_array().unwrap().len(), 161);
    let list = admin.get("/schedules").await;
    assert!(list["next_change"].is_object(), "{list}");

    // A simulation across 1 December: the change at midnight, Paris time
    // (23:00 UTC), and the empty noon rotates the whole library.
    let start = jiff::civil::date(2026, 11, 30)
        .at(22, 0, 0, 0)
        .in_tz("Europe/Paris")
        .unwrap()
        .timestamp()
        .as_millisecond();
    let midnight = start + 2 * 3_600_000;
    let noon = start + 14 * 3_600_000;
    let (status, report) = admin
        .send(
            Method::POST,
            "/rotation/simulate",
            json!({"hours": 16, "start_unix_ms": start, "seed": 7}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let picks = report["simulation"]["picks"].as_array().unwrap();
    assert!(!picks.is_empty());
    for pick in picks {
        let at = pick["start_unix_ms"].as_i64().unwrap();
        let explanation = &pick["explanation"];
        if at < midnight {
            assert_eq!(explanation["schedule_id"], default_id, "{pick}");
            assert_eq!(explanation["clock_id"], clocks[0], "{pick}");
        } else {
            assert_eq!(explanation["schedule_id"], christmas_id, "{pick}");
            assert_eq!(explanation["schedule_name"], "Noël", "{pick}");
            if (noon..noon + 3_600_000).contains(&at) {
                assert!(explanation["clock_id"].is_null(), "{pick}");
            } else {
                assert_eq!(explanation["clock_id"], clocks[1], "{pick}");
            }
        }
    }

    // The default schedule stays; the others go.
    let (status, _) = admin
        .request(Method::DELETE, &format!("/schedules/{default_id}"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = admin
        .request(Method::DELETE, &format!("/schedules/{christmas_id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        admin.get("/schedules").await["schedules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// What the API shows of the configuration, for comparisons.
async fn configuration(admin: &Client, tracks: &[u64]) -> Value {
    let mut shown = serde_json::Map::new();
    for path in [
        "/settings/rotation",
        "/settings/playout",
        "/categories",
        "/labels",
        "/clocks",
        "/schedules",
        "/restrictions",
        "/stream-servers",
        "/settings/stream-details",
        "/outputs",
        "/webhooks",
        "/users",
        "/eq-presets",
        "/settings/agc",
        "/agc-presets",
        "/stations",
        "/medleys",
        "/settings/medleys",
    ] {
        shown.insert(path.to_owned(), admin.get(path).await);
    }
    let ids: Vec<String> = tracks.iter().map(u64::to_string).collect();
    let path = format!("/tracks/categories?ids={}", ids.join(","));
    shown.insert(path.clone(), admin.get(&path).await);
    for track in tracks {
        let path = format!("/tracks/{track}");
        let mut detail = admin.get(&path).await;
        // What the scan and the plays give, not what was set; the play
        // ratio counters are state.
        for key in ["plays", "created_at_unix_ms", "updated_at_unix_ms"] {
            detail.as_object_mut().unwrap().remove(key);
        }
        for on_station in detail["station_ratios"].as_array_mut().unwrap() {
            on_station.as_object_mut().unwrap().remove("counter");
        }
        shown.insert(path, detail);
    }
    let mut shown = Value::Object(shown);
    // When things happened, not what was set.
    fn forget_times(value: &mut Value) {
        match value {
            Value::Object(object) => {
                object.retain(|key, _| {
                    !matches!(
                        key.as_str(),
                        "added_at_unix_ms"
                            | "analyzed_at_unix_ms"
                            | "created_at_unix_ms"
                            | "last_login_at_unix_ms"
                    )
                });
                object.values_mut().for_each(forget_times);
            }
            Value::Array(values) => values.iter_mut().for_each(forget_times),
            _ => {}
        }
    }
    forget_times(&mut shown);
    // Each output's live state, which the engine reports when it gets to
    // it: not configuration.
    for output in shown["/outputs"].as_array_mut().unwrap() {
        output.as_object_mut().unwrap().remove("status");
    }
    // A webhook's last delivery is what the station sent while the test
    // ran (a track started), not what was set.
    for webhook in shown["/webhooks"].as_array_mut().unwrap() {
        webhook.as_object_mut().unwrap().remove("last_delivery");
    }
    // An artist is found again by name, under the ID the scan gave it.
    for restriction in shown["/restrictions"].as_array_mut().unwrap() {
        if restriction["target"]["kind"] == "artist" {
            restriction["target"].as_object_mut().unwrap().remove("id");
        }
    }
    shown
}

/// The keys under which `a` and `b` differ.
fn differences(a: &Value, b: &Value, at: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a
                .keys()
                .chain(b.keys())
                .collect::<std::collections::BTreeSet<_>>()
            {
                differences(
                    a.get(key).unwrap_or(&Value::Null),
                    b.get(key).unwrap_or(&Value::Null),
                    &format!("{at}/{key}"),
                    out,
                );
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                differences(a, b, &format!("{at}[{index}]"), out);
            }
        }
        (a, b) if a != b => out.push(format!("{at}: {a} != {b}")),
        _ => {}
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_restores_onto_an_empty_database() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let t = station.tracks.clone();

    // Settings, shared and per station, and a second station.
    let mut rotation = admin.get("/settings/rotation").await;
    rotation["separation"]["artist_min"] = json!(45);
    rotation["daypart_count"] = json!(6);
    let (status, body) = admin
        .send(Method::PUT, "/settings/rotation", rotation)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/stations",
            json!({"name": "Hits 80", "code": "hits80"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Categories and what is set on tracks.
    let (_, gold) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Gold", "color": "#AA8800", "separation": {"dayparts": 3, "daypart_days": 2}}),
        )
        .await;
    let gold = gold["id"].as_u64().unwrap();
    // A medley, going into Gold.
    let (status, body) = admin
        .send(
            Method::PUT,
            "/settings/medleys",
            json!({"category_id": gold}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/medleys",
            json!({"recipe": {"category_ids": [gold], "hooks": 2}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{}/categories", t[0]),
            json!([{"category_id": gold, "weight": 2.5}]),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, live) = admin
        .send(Method::POST, "/labels", json!({"name": "live"}))
        .await;
    let (status, body) = admin
        .send(
            Method::POST,
            "/tracks/labels",
            json!({"selection": {"track_ids": [t[1]]}, "add": [live["id"]]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{}/overrides", t[0]),
            json!({"cue_in_ms": 500, "crossfade": {"mode": "fixed", "fixed_ms": 1_500}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{}/hook", t[1]),
            json!({"hook_in_ms": 500, "hook_out_ms": 2_500}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = admin
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": {"track_ids": [t[2]]}, "bpm": 128.0, "play_ratio": 4,
                   "dayparts": 0, "daypart_days": 2}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/tracks/enabled",
            json!({"selection": {"track_ids": [t[3]]}, "enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // A station's own ratio, and a track disabled on that station only.
    for (id, patch) in [
        (t[1], json!({"station_id": 1, "play_ratio": 3})),
        (t[0], json!({"station_id": 1, "play_ratio": 0})),
    ] {
        let (status, body) = admin
            .send(Method::PATCH, &format!("/tracks/{id}"), patch)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    // Programming.
    let (status, clock) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Hour", "slots": [{"category_id": gold, "method": "random"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    let (status, christmas) = admin
        .send(Method::POST, "/schedules", json!({"name": "Christmas"}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{christmas}");
    let (status, body) = admin
        .send(
            Method::PUT,
            &format!("/schedules/{}", christmas["id"]),
            json!({"name": "Christmas",
                   "hours": [{"day_of_week": 1, "hour": 8, "clock_id": clock["id"]}],
                   "periods": [{"start": "2026-12-01T00:00:00", "end": "2027-01-01T00:00:00", "every_year": true}]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let artist = sqlx::query("INSERT INTO artists (name) VALUES ('Artist 5')")
        .execute(&station.db.pool)
        .await
        .unwrap()
        .last_insert_id();
    for target in [
        json!({"kind": "track", "id": t[4]}),
        json!({"kind": "artist", "id": artist}),
    ] {
        let (status, body) = admin
            .send(
                Method::POST,
                "/restrictions",
                json!({"target": target,
                       "rule": {"kind": "daypart", "mode": "ban", "days": [1], "start_hour": 6, "end_hour": 9}}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    // Outputs, webhooks, users and their tokens.
    let profile = admin.get("/encoder-profiles").await[0]["id"].clone();
    let (status, server) = admin
        .send(
            Method::POST,
            "/stream-servers",
            json!({"name": "Local", "host": "127.0.0.1", "port": 9, "source_password": "hackme",
                   "admin_password": "adm1n"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{server}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/stream-servers",
            json!({"name": "Relay", "host": "127.0.0.2", "relay_of": server["id"],
                   "relay_mounts": [{"mount": "/live"}], "relay_subtract_one": true}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = admin
        .send(
            Method::PUT,
            "/settings/stream-details",
            json!({"name": "My station", "url": "https://example.org"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile, "enabled": false,
                   "server_id": server["id"], "config": {"mount": "/live"},
                   "details": {"genre": "Pop"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/webhooks",
            json!({"name": "Website", "url": "http://127.0.0.1:9/hook", "events": ["track.started"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = admin
        .send(
            Method::POST,
            "/users",
            json!({"username": "host", "password": "morning show", "role": "operator"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, token) = admin
        .send(Method::POST, "/tokens", json!({"name": "automation"}))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = token["token"].as_str().unwrap().to_owned();
    // The AGC chain and a preset of its own.
    let (status, body) = admin
        .send(
            Method::POST,
            "/agc-presets",
            json!({"name": "Morning", "settings": {"gated_agc": {"enabled": true, "max_gain_db": 9}}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let mut agc = admin.get("/settings/agc").await;
    agc["final_stage"]["ceiling_db"] = json!(-2.0);
    let (status, body) = admin.send(Method::PUT, "/settings/agc", agc).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // A play ratio counter: state, not in the file.
    sqlx::query(
        "INSERT INTO track_ratio_counters (station_id, track_id, counter) VALUES (1, ?, 6)
         ON DUPLICATE KEY UPDATE counter = 6",
    )
    .bind(t[2])
    .execute(&station.db.pool)
    .await
    .unwrap();
    let configured = configuration(&admin, &t).await;
    assert_eq!(configured[&format!("/tracks/{}", t[2])]["play_ratio"], 4);
    assert_eq!(configured[&format!("/tracks/{}", t[2])]["dayparts"], 0);
    assert_eq!(configured[&format!("/tracks/{}", t[2])]["daypart_days"], 2);

    // Backed up, then restored onto another Heron's empty database with
    // the same library.
    let viewer = station.signed_in("viewer").await;
    let (status, _) = viewer.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, backup) = admin.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{backup}");
    assert_eq!(backup["kind"], "manual");
    // The database dump goes with it.
    // `::error::` makes the reason an annotation of the CI run.
    assert_eq!(backup["database_error"], Value::Null, "\n::error::{backup}");
    assert!(
        backup["database_size"].as_u64().unwrap() > 1_000,
        "\n::error::{backup}"
    );
    let name = backup["name"].as_str().unwrap().to_owned();
    let mut file = admin.get(&format!("/backups/{name}/config")).await;
    assert_eq!(file["format"], 1);
    assert!(
        file["tables"]["track_categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["track"] == "Artist 1/Track 1.wav" && row["weight"] == 2.5),
        "{}",
        file["tables"]["track_categories"]
    );
    assert!(
        file["tables"]["track_programming"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["track"] == "Artist 3/Track 3.wav"
                && row["play_ratio"] == 4
                && row["dayparts"] == 0
                && row["daypart_days"] == 2),
        "{}",
        file["tables"]["track_programming"]
    );
    assert!(file["tables"].get("track_ratio_counters").is_none());
    let own = file["tables"]["track_station_ratios"].as_array().unwrap();
    assert_eq!(own.len(), 2, "{own:?}");
    assert!(
        own.iter().any(|row| row["track"] == "Artist 2/Track 2.wav"
            && row["play_ratio"] == 3
            && row["enabled"] == 1),
        "{own:?}"
    );
    assert!(
        own.iter().any(|row| row["track"] == "Artist 1/Track 1.wav"
            && row["enabled"] == 0
            && row["disabled_by"].is_u64()),
        "{own:?}"
    );
    assert!(
        file["disabled_by"]["Artist 4/Track 4.wav"]["user_id"].is_u64(),
        "{}",
        file["disabled_by"]
    );
    // What an older file restores: no ratios per station, no one who
    // disabled a track.
    let mut older = file.clone();
    older["tables"]
        .as_object_mut()
        .unwrap()
        .remove("track_station_ratios");
    older.as_object_mut().unwrap().remove("disabled_by");
    // Nor tracks' own daypart separation.
    for row in older["tables"]["track_programming"].as_array_mut().unwrap() {
        let row = row.as_object_mut().unwrap();
        row.remove("dayparts");
        row.remove("daypart_days");
    }
    station.stop().await;
    // A track the new library does not have.
    file["tables"]["track_categories"]
        .as_array_mut()
        .unwrap()
        .push(json!({"track": "Gone/Gone.wav", "category_id": gold, "weight": 1.0}));

    let Some(other) = Station::start().await else {
        return;
    };
    // The scan finds the artist again, under another ID.
    sqlx::query("INSERT INTO artists (name) VALUES ('Someone else'), ('Artist 5')")
        .execute(&other.db.pool)
        .await
        .unwrap();
    let admin = other.signed_in("admin").await;
    let (status, report) = admin.send(Method::POST, "/backups/restore", file).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["left_out"]["track_categories"], 1, "{report}");
    assert_eq!(report["missing_tracks"], json!(["Gone/Gone.wav"]));
    // Everyone signs in again.
    let (status, _) = admin.request(Method::GET, "/auth/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let admin = other.signed_in("admin").await;
    let restored = configuration(&admin, &other.tracks).await;
    let mut different = Vec::new();
    differences(&configured, &restored, "", &mut different);
    assert!(different.is_empty(), "{}", different.join("\n"));
    // The output's password and the token came back too.
    let mut scripted = other.client();
    scripted.bearer = Some(token);
    assert_eq!(scripted.get("/auth/me").await["username"], "admin");
    let host = other.client();
    let (status, _) = host
        .send(
            Method::POST,
            "/auth/login",
            json!({"username": "host", "password": "morning show"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // The restore is undone by the backup it took first.
    let before = report["before_restore"].as_str().unwrap();
    let (status, body) = admin
        .send(
            Method::POST,
            &format!("/backups/{before}/restore"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let admin = other.signed_in("admin").await;
    assert_eq!(admin.get("/categories").await, json!([]));
    let kinds: Vec<Value> = admin
        .get("/backups")
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|backup| backup["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        [
            json!("before_restore"),
            json!("before_restore"),
            json!("uploaded")
        ]
    );
    // A file from a newer Heron, or not a backup at all, changes nothing.
    let mut newer = admin.get(&format!("/backups/{before}/config")).await;
    newer["format"] = json!(2);
    let (status, body) = admin.send(Method::POST, "/backups/restore", newer).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, _) = admin
        .send(Method::POST, "/backups/restore", json!({"hello": "world"}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(admin.get("/backups").await.as_array().unwrap().len(), 3);

    // A restore that fails half-way, on a row the database refuses after
    // the earlier tables were written, changes nothing.
    let mut broken = admin.get(&format!("/backups/{before}/config")).await;
    broken["tables"]["track_categories"]
        .as_array_mut()
        .unwrap()
        .push(json!({"track": "Artist 1/Track 1.wav", "category_id": 999_999, "weight": 1.0}));
    let unchanged = configuration(&admin, &other.tracks).await;
    let (status, body) = admin.send(Method::POST, "/backups/restore", broken).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("track_categories"),
        "{body}"
    );
    let mut different = Vec::new();
    differences(
        &unchanged,
        &configuration(&admin, &other.tracks).await,
        "",
        &mut different,
    );
    assert!(different.is_empty(), "{}", different.join("\n"));

    // A file from before the ratios per station and the tracks' own
    // daypart separation restores: every track on its default ratio and
    // its categories' dayparts, disabled tracks with no one recorded.
    let (status, body) = admin.send(Method::POST, "/backups/restore", older).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let admin = other.signed_in("admin").await;
    let o = other.tracks.clone();
    let detail = admin.get(&format!("/tracks/{}", o[3])).await;
    assert_eq!(detail["enabled"], false);
    assert_eq!(detail["disabled_by"], Value::Null);
    let detail = admin.get(&format!("/tracks/{}", o[2])).await;
    assert_eq!(detail["play_ratio"], 4, "{detail}");
    assert_eq!(detail["dayparts"], Value::Null, "{detail}");
    assert_eq!(detail["daypart_days"], Value::Null, "{detail}");
    for id in [o[0], o[1]] {
        let detail = admin.get(&format!("/tracks/{id}")).await;
        assert!(
            detail["station_ratios"]
                .as_array()
                .unwrap()
                .iter()
                .all(|on| on["play_ratio"].is_null() && on["enabled"] == true),
            "{detail}"
        );
    }
    other.stop().await;
}

/// Waits for the backup test under way, and returns the last one.
async fn tested(admin: &Client) -> Value {
    eventually("the backup test", Duration::from_secs(60), || async {
        let tests = admin.get("/backups/tests").await;
        (tests["testing"].is_null() && !tests["last"].is_null()).then_some(tests)
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_encrypted_backup_opens_with_its_key_only() {
    let Some(station) = Station::start().await else {
        return;
    };
    let dir = station.state.backups.dir().to_owned();
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;

    // A backup in clear, with its checksums.
    let (status, clear) = admin.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{clear}");
    assert_eq!(clear["encrypted"], false);
    assert_eq!(clear["database_error"], Value::Null, "\n::error::{clear}");
    let clear = clear["name"].as_str().unwrap().to_owned();
    let checksums =
        std::fs::read_to_string(dir.join(format!("heron-checksums-{clear}.sha256"))).unwrap();
    assert!(
        checksums.contains(&format!("  heron-config-{clear}.json\n")),
        "{checksums}"
    );
    assert!(
        checksums.contains(&format!("  heron-database-{clear}.sql.gz\n")),
        "{checksums}"
    );
    let state = admin.get("/backups/state").await;
    assert_eq!(state["encryption"], "off");
    assert_eq!(state["check_due"], false);

    // Only admins set a key; it is shown once, then typed back.
    let (status, _) = viewer.send(Method::POST, "/backups/key", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, new) = admin.send(Method::POST, "/backups/key", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{new}");
    let key = new["key"].as_str().unwrap().to_owned();
    let recipient = new["recipient"].as_str().unwrap().to_owned();
    assert!(new["key_file"].as_str().unwrap().contains(&key));
    let view = admin.get("/backups/key").await;
    assert_eq!(view["state"], "pending");
    assert!(!view.to_string().contains(&key), "{view}");
    let (other_key, _) = crate::backups::generate_key();
    let (status, body) = admin
        .send(
            Method::POST,
            "/backups/key/confirm",
            json!({"key": other_key}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, confirmed) = admin
        .send(
            Method::POST,
            "/backups/key/confirm",
            json!({"key": key.to_lowercase()}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["key"]["state"], "on");
    assert_eq!(confirmed["encrypted"], json!([clear]));
    let with_key = confirmed["backup"].as_str().unwrap().to_owned();
    // The new backup is tested with the key just typed.
    let tests = tested(&admin).await;
    assert_eq!(tests["last"]["name"], with_key, "{tests}");
    assert_eq!(tests["last"]["ok"], true, "{tests}");

    // Encrypted in place: the files in clear are gone, the checksums follow.
    assert!(!dir.join(format!("heron-config-{clear}.json")).exists());
    assert!(!dir.join(format!("heron-database-{clear}.sql.gz")).exists());
    for name in [&clear, &with_key] {
        let config = std::fs::read(dir.join(format!("heron-config-{name}.json.age"))).unwrap();
        assert!(config.starts_with(b"age-encryption.org/v1\n"));
        assert!(
            dir.join(format!("heron-database-{name}.sql.gz.age"))
                .exists()
        );
        let checksums =
            std::fs::read_to_string(dir.join(format!("heron-checksums-{name}.sha256"))).unwrap();
        assert!(
            checksums.contains(&format!("  heron-config-{name}.json.age\n")),
            "{checksums}"
        );
        assert!(
            checksums.contains(&format!("  heron-database-{name}.sql.gz.age\n")),
            "{checksums}"
        );
    }
    // Nothing Heron keeps holds the secret key.
    let stored: Vec<String> = sqlx::query_scalar("SELECT CAST(value AS CHAR) FROM settings")
        .fetch_all(&station.db.pool)
        .await
        .unwrap();
    assert!(stored.iter().all(|value| !value.contains("AGE-SECRET-KEY")));
    assert!(stored.iter().any(|value| value.contains(&recipient)));
    for entry in std::fs::read_dir(&dir).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&bytes).contains("AGE-SECRET-KEY"));
    }
    let backups = admin.get("/backups").await;
    assert!(
        backups
            .as_array()
            .unwrap()
            .iter()
            .all(|backup| backup["encrypted"] == true
                && backup["key"] == "current"
                && backup["recipient"] == recipient.as_str()),
        "{backups}"
    );

    // A restore asks for the key; a wrong one changes nothing.
    let (status, body) = admin
        .send(
            Method::POST,
            &format!("/backups/{with_key}/restore"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("encrypted"),
        "{body}"
    );
    let (status, body) = admin
        .send(
            Method::POST,
            &format!("/backups/{with_key}/restore"),
            json!({"key": other_key}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        admin.get("/backups").await.as_array().unwrap().len(),
        2,
        "nothing taken"
    );
    let (status, report) = admin
        .send(
            Method::POST,
            &format!("/backups/{with_key}/restore"),
            json!({"key": key}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let admin = station.signed_in("admin").await;

    // An encrypted file uploaded, with its key in the header; kept as it is.
    let file = admin.get_bytes(&format!("/backups/{clear}/config")).await;
    let (status, body) = admin
        .post_bytes("/backups/restore", file.clone(), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, report) = admin
        .post_bytes("/backups/restore", file.clone(), Some(&key))
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let admin = station.signed_in("admin").await;
    let uploaded = admin
        .get("/backups")
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|backup| backup["kind"] == "uploaded")
        .cloned()
        .unwrap();
    assert_eq!(uploaded["key"], "unknown", "{uploaded}");
    let name = uploaded["name"].as_str().unwrap();
    assert_eq!(
        std::fs::read(dir.join(format!("heron-config-{name}.json.age"))).unwrap(),
        file
    );

    // A restore keeps the key, whatever the file says.
    let plain = crate::backups::decrypt_with(&key, &file);
    let mut plain: Value = serde_json::from_slice(&plain).unwrap();
    plain["tables"]["settings"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "backup_encryption", "value": {"current": null}}));
    let (status, report) = admin.send(Method::POST, "/backups/restore", plain).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let admin = station.signed_in("admin").await;
    let view = admin.get("/backups/key").await;
    assert_eq!(view["current"]["recipient"], recipient.as_str(), "{view}");

    // Each admin checks the key: a new one at once, then further away.
    let (status, body) = admin
        .send(
            Method::POST,
            "/users",
            json!({"username": "night", "password": "correct horse", "role": "admin"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let night = station.signed_in("night").await;
    assert_eq!(admin.get("/backups/state").await["check_due"], false);
    assert_eq!(night.get("/backups/state").await["check_due"], true);
    let (status, _) = night
        .send(
            Method::POST,
            "/backups/key/check",
            json!({"key": other_key}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        night.get("/backups/state").await["check_due"],
        true,
        "a wrong key changes nothing"
    );
    let (status, _) = night
        .send(Method::POST, "/backups/key/later", json!({}))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        night.get("/backups/state").await["check_due"],
        false,
        "asked again tomorrow"
    );
    let (status, checked) = night
        .send(Method::POST, "/backups/key/check", json!({"key": key}))
        .await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    let next = checked["next_check_unix_ms"].as_i64().unwrap();
    let in_a_day = jiff::Timestamp::now().as_millisecond() + 24 * 3600 * 1000;
    assert!((next - in_a_day).abs() < 60_000, "{checked}");
    let tests = tested(&night).await;
    assert_eq!(tests["last"]["by"], "night", "{tests}");
    assert_eq!(tests["last"]["ok"], true, "{tests}");

    // A file changed in the folder: damaged at the next check, and its test fails.
    let dump = dir.join(format!("heron-database-{clear}.sql.gz.age"));
    let mut bytes = std::fs::read(&dump).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 1;
    std::fs::write(&dump, bytes).unwrap();
    station.state.backups.verify_all().await.unwrap();
    let damaged = admin
        .get("/backups")
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|backup| backup["name"] == clear.as_str())
        .cloned()
        .unwrap();
    assert!(
        damaged["damaged"].as_str().unwrap().contains("changed"),
        "{damaged}"
    );
    assert_eq!(admin.get("/backups/state").await["damaged"], 1);
    let (status, body) = admin
        .send(Method::POST, &format!("/backups/{clear}/test"), json!({}))
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the key is needed: {body}"
    );
    let (status, body) = admin
        .send(
            Method::POST,
            &format!("/backups/{clear}/test"),
            json!({"key": key}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let tests = tested(&admin).await;
    assert_eq!(tests["last"]["ok"], false, "{tests}");
    assert!(
        tests["last"]["error"]
            .as_str()
            .unwrap()
            .contains(".sql.gz.age"),
        "{tests}"
    );
    assert_eq!(admin.get("/backups/state").await["test_due"], true);

    // A new key: the backup taken with it opens with it, the older ones
    // need the earlier key, even within the same second.
    let (status, second) = admin.send(Method::POST, "/backups/key", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let (status, confirmed) = admin
        .send(
            Method::POST,
            "/backups/key/confirm",
            json!({"key": second["key"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["encrypted"], json!([]), "nothing left in clear");
    let newest = confirmed["backup"].as_str().unwrap().to_owned();
    let backups = admin.get("/backups").await;
    for backup in backups.as_array().unwrap() {
        let (expected, public) = if backup["name"] == newest.as_str() {
            ("current", second["recipient"].clone())
        } else if backup["kind"] == "uploaded" {
            ("unknown", Value::Null)
        } else {
            ("earlier", json!(recipient))
        };
        assert_eq!(backup["key"], expected, "{backup}");
        assert_eq!(backup["recipient"], public, "{backup}");
    }
    tested(&admin).await;

    // Turned off: new backups in clear, the others stay encrypted.
    let (status, view) = admin.send(Method::DELETE, "/backups/key", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["state"], "off");
    assert_eq!(view["previous"][0]["recipient"], recipient.as_str());
    assert_eq!(view["previous"][1]["recipient"], second["recipient"]);
    let (status, backup) = admin.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{backup}");
    assert_eq!(backup["encrypted"], false);
    let backups = admin.get("/backups").await;
    let earlier = backups
        .as_array()
        .unwrap()
        .iter()
        .find(|backup| backup["name"] == with_key.as_str())
        .cloned()
        .unwrap();
    assert_eq!(earlier["key"], "earlier", "{earlier}");
    station.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tracks_have_their_lowest_and_highest_level() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let pool = &station.db.pool;
    let t = station.tracks.clone();
    // 50 ms points over the 3 s tracks: silence at both ends, the music
    // between the cue points (0 to 3000 ms in the test analysis).
    for (track, quietest, loudest) in [(t[0], 100u8, 250u8), (t[1], 50, 200)] {
        let mut peaks = vec![quietest; 60];
        peaks[30] = loudest;
        sqlx::query("INSERT INTO track_waveforms (track_id, interval_ms, peaks) VALUES (?, 50, ?)")
            .bind(track)
            .bind(peaks)
            .execute(pool)
            .await
            .unwrap();
    }
    assert_eq!(crate::levels::update(pool).await.unwrap(), 2);
    assert_eq!(crate::levels::update(pool).await.unwrap(), 0, "done once");
    let level = |byte: u8| (crate::levels::byte_db(byte) * 10.0).round() / 10.0;
    let detail = admin.get(&format!("/tracks/{}", t[0])).await;
    assert_eq!(detail["level_min_db"], json!(level(100)), "{detail}");
    assert_eq!(detail["level_max_db"], json!(level(250)));
    // A cue point moved: computed again, between the new points.
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{}/overrides", t[0]),
            json!({"cue_in_ms": 1_500, "cue_out_ms": 1_600}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(crate::levels::update(pool).await.unwrap(), 1);
    let detail = admin.get(&format!("/tracks/{}", t[0])).await;
    assert_eq!(detail["level_min_db"], json!(level(100)));
    assert_eq!(detail["level_max_db"], json!(level(250)));
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{}/overrides", t[0]),
            json!({"cue_in_ms": 0, "cue_out_ms": 1_000}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    crate::levels::update(pool).await.unwrap();
    let detail = admin.get(&format!("/tracks/{}", t[0])).await;
    assert_eq!(detail["level_max_db"], json!(level(100)), "the peak is out");
    // The list sorts by them, tracks without a level last.
    let page = admin.get("/tracks?sort=level_min").await;
    let order: Vec<u64> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| track["id"].as_u64().unwrap())
        .collect();
    assert_eq!(order[..2], [t[1], t[0]]);
    assert_eq!(page["items"][4]["level_min_db"], Value::Null);
    // The file changed: its analysis is gone, and so are its levels.
    sqlx::query("DELETE FROM track_analysis WHERE track_id = ?")
        .bind(t[1])
        .execute(pool)
        .await
        .unwrap();
    let detail = admin.get(&format!("/tracks/{}", t[1])).await;
    assert_eq!(detail["level_min_db"], Value::Null, "{detail}");
    assert_eq!(detail["level_max_db"], Value::Null);
    station.stop().await;
}

/// Daypart separation: the station's division of the day in the rotation
/// settings, the separation a category's own, never the station's or a
/// clock slot's; the simulation follows it and logs each pick's daypart.
#[tokio::test(flavor = "multi_thread")]
async fn daypart_separation_is_set_per_category() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;

    // 4 dayparts by default; the station has no separation of its own.
    let mut rotation = admin.get("/settings/rotation").await;
    assert_eq!(rotation["daypart_count"], 4, "{rotation}");
    assert!(
        rotation["separation"].get("dayparts").is_none(),
        "{rotation}"
    );
    // Only daypart separation, for the simulation below.
    for window in ["album_min", "artist_min", "title_min", "track_min"] {
        rotation["separation"][window] = json!(0);
    }
    let mut with_dayparts = rotation.clone();
    with_dayparts["separation"]["dayparts"] = json!(1);
    let (status, body) = admin
        .send(Method::PUT, "/settings/rotation", with_dayparts)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // 4 or 6 dayparts only.
    for (count, expected) in [(5, StatusCode::UNPROCESSABLE_ENTITY), (6, StatusCode::OK)] {
        rotation["daypart_count"] = json!(count);
        let (status, body) = admin
            .send(Method::PUT, "/settings/rotation", rotation.clone())
            .await;
        assert_eq!(status, expected, "{count} dayparts: {body}");
    }
    assert_eq!(admin.get("/settings/rotation").await["daypart_count"], 6);

    // A category's value: 0 to 10, off when left out.
    let (status, body) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Power", "separation": {"dayparts": 11}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, power) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Power", "separation": {"dayparts": 0}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{power}");
    assert_eq!(power["separation"], json!({"dayparts": 0}));
    let id = power["id"].as_u64().unwrap();
    let (status, power) = admin
        .send(
            Method::PUT,
            &format!("/categories/{id}"),
            json!({"name": "Power", "separation": {"track_min": 60}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{power}");
    assert!(power["separation"].get("dayparts").is_none(), "{power}");
    let (status, power) = admin
        .send(
            Method::PUT,
            &format!("/categories/{id}"),
            json!({"name": "Power", "separation": {"dayparts": 1}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{power}");
    assert_eq!(
        admin.get(&format!("/categories/{id}")).await["separation"],
        json!({"dayparts": 1})
    );

    // Never on a clock slot.
    let (status, body) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Hour", "slots": [
                {"category_id": id, "method": "random", "separation": {"dayparts": 1}},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, clock) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Hour", "slots": [{"category_id": id, "method": "random"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    let clock_id = clock["id"].as_u64().unwrap();

    // Eight hours of the clock, every track in Power: a handful of tracks,
    // so the rule holds them back and is relaxed, and the audit finds
    // nothing.
    for track in &station.tracks {
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{track}/categories"),
                json!([{"category_id": id}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let simulate = |clock: Option<u64>| {
        let admin = &admin;
        async move {
            let (status, report) = admin
                .send(
                    Method::POST,
                    "/rotation/simulate",
                    json!({"hours": 8, "seed": 4, "clock_id": clock}),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{report}");
            let simulation = report["simulation"].clone();
            assert!(
                simulation["violations"].as_array().unwrap().is_empty(),
                "{}",
                simulation["violations"]
            );
            simulation["picks"].as_array().unwrap().clone()
        }
    };
    let picks = simulate(Some(clock_id)).await;
    assert!(!picks.is_empty());
    for pick in &picks {
        let daypart = &pick["explanation"]["daypart"];
        assert_eq!(daypart["count"], 6, "{pick}");
        assert_eq!(daypart["separation"], 1);
        assert!(daypart["daypart"].as_u64().unwrap() < 6);
    }
    assert!(
        picks
            .iter()
            .any(|pick| pick["explanation"]["blocked"]["daypart"].as_u64() > Some(0)),
        "nothing held back by its daypart"
    );
    // The station's library, without a clock: no daypart separation.
    for pick in simulate(None).await {
        assert_eq!(pick["explanation"]["daypart"]["separation"], 0, "{pick}");
    }
    station.stop().await;
}

/// The same daypart on the following days: a category's own, 0 to 7,
/// never a clock slot's; logged with each pick, and kept by a
/// configuration backup.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_daypart_on_the_following_days_is_set_per_category() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let (status, body) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Power", "separation": {"daypart_days": 8}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // On its own, without daypart separation.
    let (status, power) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Power", "separation": {"daypart_days": 7}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{power}");
    let id = power["id"].as_u64().unwrap();
    let (status, power) = admin
        .send(
            Method::PUT,
            &format!("/categories/{id}"),
            json!({"name": "Power", "separation": {"dayparts": 1, "daypart_days": 2}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{power}");
    assert_eq!(
        admin.get(&format!("/categories/{id}")).await["separation"],
        json!({"dayparts": 1, "daypart_days": 2})
    );
    let (status, body) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Hour", "slots": [
                {"category_id": id, "method": "random", "separation": {"daypart_days": 1}},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, clock) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Hour", "slots": [{"category_id": id, "method": "random"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    for track in &station.tracks {
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{track}/categories"),
                json!([{"category_id": id}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    // Every track played a day ago, in the daypart of now: the history
    // reaches back that far, and the first pick finds them all held back,
    // until the rule is dropped (halved to 1 day, it still holds).
    for track in &station.tracks {
        sqlx::query(
            "INSERT INTO play_history (station_id, track_id, title, started_at, ended_at, played_ms,
                                       deck, source)
             VALUES (1, ?, 'x', NOW(6) - INTERVAL 1 DAY, NOW(6) - INTERVAL 1 DAY + INTERVAL 3 SECOND,
                     3000, 'a', 'rotation')",
        )
        .bind(track)
        .execute(&station.db.pool)
        .await
        .unwrap();
    }
    let (status, report) = admin
        .send(
            Method::POST,
            "/rotation/simulate",
            json!({"hours": 1, "seed": 4, "clock_id": clock["id"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let simulation = &report["simulation"];
    assert!(
        simulation["violations"].as_array().unwrap().is_empty(),
        "{}",
        simulation["violations"]
    );
    let first = &simulation["picks"][0]["explanation"];
    assert_eq!(first["daypart"]["days"], 2, "{first}");
    // The others by the track separation: what the station has just
    // played or queued.
    let blocked = &first["blocked"];
    assert!(blocked["daypart_days"].as_u64() >= Some(4), "{first}");
    assert_eq!(
        blocked["daypart_days"].as_u64().unwrap() + blocked["track"].as_u64().unwrap(),
        5,
        "{first}"
    );
    assert_eq!(blocked["daypart"], 0, "{first}");
    assert_eq!(first["step"], 4, "{first}");
    station.stop().await;
}

/// A track's own daypart separation: shared by the stations, set on a
/// selection (0 to 10, and 0 to 7 days), back to its categories' with
/// `null`, filtered on; it replaces its category's in rotation, and the
/// log says whose value applied.
#[tokio::test(flavor = "multi_thread")]
async fn a_track_s_own_daypart_separation_replaces_its_category_s() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let t = station.tracks.clone();
    let set = |body: Value| {
        let admin = &admin;
        async move { admin.send(Method::POST, "/tracks/programming", body).await }
    };
    for body in [
        json!({"selection": {"track_ids": [t[0]]}, "dayparts": 11}),
        json!({"selection": {"track_ids": [t[0]]}, "daypart_days": 8}),
    ] {
        let (status, answer) = set(body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    }
    // A viewer cannot set it.
    let viewer = station.signed_in("viewer").await;
    let (status, _) = viewer
        .send(
            Method::POST,
            "/tracks/programming",
            json!({"selection": {"track_ids": [t[0]]}, "dayparts": 1}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let own = |own: bool| {
        let admin = &admin;
        async move {
            admin.get(&format!("/tracks?own_dayparts={own}")).await["total"]
                .as_u64()
                .unwrap()
        }
    };
    let all = t.len() as u64;
    assert_eq!((own(true).await, own(false).await), (0, all));
    let (status, answer) = set(json!({"selection": {"track_ids": [t[0]]},
                                      "dayparts": 0, "daypart_days": 2}))
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let detail = admin.get(&format!("/tracks/{}", t[0])).await;
    assert_eq!(
        (&detail["dayparts"], &detail["daypart_days"]),
        (&json!(0), &json!(2))
    );
    assert_eq!((own(true).await, own(false).await), (1, all - 1));
    // Other programming details leave it as it is; `null` goes back to the
    // categories', one value at a time.
    let (status, _) = set(json!({"selection": {"track_ids": [t[0]]}, "bpm": 120.0})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = set(json!({"selection": {"track_ids": [t[0]]}, "dayparts": null})).await;
    assert_eq!(status, StatusCode::OK);
    let listed = admin.get("/tracks?own_dayparts=true").await;
    assert_eq!(listed["items"][0]["dayparts"], Value::Null, "{listed}");
    assert_eq!(listed["items"][0]["daypart_days"], 2, "{listed}");
    let (status, _) = set(json!({"selection": {"track_ids": [t[0]]}, "daypart_days": null})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(own(true).await, 0);

    // A category keeping its tracks out of the 3 dayparts after a play;
    // the first track's own 0 replaces it.
    let mut rotation = admin.get("/settings/rotation").await;
    for window in ["album_min", "artist_min", "title_min", "track_min"] {
        rotation["separation"][window] = json!(0);
    }
    let (status, body) = admin
        .send(Method::PUT, "/settings/rotation", rotation)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, power) = admin
        .send(
            Method::POST,
            "/categories",
            json!({"name": "Power", "separation": {"dayparts": 3}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{power}");
    let id = power["id"].as_u64().unwrap();
    let (status, clock) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Hour", "slots": [{"category_id": id, "method": "random"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    for track in &t {
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{track}/categories"),
                json!([{"category_id": id}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = set(json!({"selection": {"track_ids": [t[0]]}, "dayparts": 0})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, report) = admin
        .send(
            Method::POST,
            "/rotation/simulate",
            json!({"hours": 12, "seed": 4, "clock_id": clock["id"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let simulation = &report["simulation"];
    assert!(
        simulation["violations"].as_array().unwrap().is_empty(),
        "{}",
        simulation["violations"]
    );
    let picks = simulation["picks"].as_array().unwrap();
    let mut own_picks = 0;
    for pick in picks {
        let daypart = &pick["explanation"]["daypart"];
        if pick["track_id"] == t[0] {
            own_picks += 1;
            assert_eq!(daypart["separation"], 0, "{pick}");
            assert_eq!(daypart["separation_from"], "track", "{pick}");
        } else {
            assert_eq!(daypart["separation"], 3, "{pick}");
            assert_eq!(daypart["separation_from"], "category", "{pick}");
        }
        assert_eq!(daypart["days_from"], "category", "{pick}");
    }
    // Free of the category's rule, it plays in every daypart; held back
    // by it, the others are relaxed.
    assert!(own_picks > 0, "{picks:?}");
    station.stop().await;
}

/// A configuration file of a schema from before daypart separation was set
/// per category: the station's value goes onto its Song categories without
/// one when restored, as the migration does.
#[tokio::test(flavor = "multi_thread")]
async fn an_older_backup_moves_the_station_daypart_separation_to_its_categories() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let mut ids = Vec::new();
    for (name, type_id, separation) in [
        ("Hits", json!(1), json!({"track_min": 60})),
        ("Gold", json!(1), json!({"dayparts": 0})),
        ("Jingles", json!(2), json!({})),
        ("Untyped", Value::Null, json!({})),
    ] {
        let (status, category) = admin
            .send(
                Method::POST,
                "/categories",
                json!({"name": name, "type_id": type_id, "separation": separation}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        ids.push(category["id"].as_u64().unwrap());
    }
    let (status, backup) = admin.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{backup}");
    let name = backup["name"].as_str().unwrap();
    let mut file = admin.get(&format!("/backups/{name}/config")).await;
    file["schema"] = json!(20261128000000_i64);
    let settings = file["tables"]["station_settings"].as_array_mut().unwrap();
    settings.retain(|row| row["name"] != "rotation");
    settings.push(json!({"station_id": 1, "name": "rotation",
                         "value": {"separation": {"artist_min": 45, "dayparts": 2}}}));
    let (status, report) = admin.send(Method::POST, "/backups/restore", file).await;
    assert_eq!(status, StatusCode::OK, "{report}");

    let admin = station.signed_in("admin").await;
    let rotation = admin.get("/settings/rotation").await;
    assert_eq!(
        rotation["separation"],
        json!({"album_min": 60, "artist_min": 45, "title_min": 240, "track_min": 240}),
    );
    let mut separations = Vec::new();
    for id in &ids {
        separations.push(admin.get(&format!("/categories/{id}")).await["separation"].clone());
    }
    assert_eq!(
        separations,
        [
            json!({"track_min": 60, "dayparts": 2}),
            json!({"dayparts": 0}),
            json!({}),
            json!({}),
        ]
    );
    station.stop().await;
}

/// The whole queue on On air is kept by a configuration backup; a file
/// from before it restores it off.
#[tokio::test(flavor = "multi_thread")]
async fn a_backup_keeps_the_whole_queue_on_air_and_an_older_one_turns_it_off() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let (status, _) = admin
        .send(
            Method::PUT,
            "/auth/preferences",
            json!({"on_air_whole_queue": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, backup) = admin.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{backup}");
    let name = backup["name"].as_str().unwrap();
    let mut file = admin.get(&format!("/backups/{name}/config")).await;
    assert!(
        file["tables"]["users"]
            .as_array()
            .unwrap()
            .iter()
            .any(|user| user["username"] == "admin" && user["on_air_whole_queue"] == 1)
    );

    // Restored as it is, it stays on.
    let (status, report) = admin
        .send(Method::POST, "/backups/restore", file.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let admin = station.signed_in("admin").await;
    assert_eq!(admin.get("/auth/me").await["on_air_whole_queue"], true);

    // A file written before the column: off.
    file["schema"] = json!(20270702000000_i64);
    for user in file["tables"]["users"].as_array_mut().unwrap() {
        user.as_object_mut().unwrap().remove("on_air_whole_queue");
    }
    let (status, report) = admin.send(Method::POST, "/backups/restore", file).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let admin = station.signed_in("admin").await;
    for user in admin.get("/users").await.as_array().unwrap() {
        assert_eq!(user["on_air_whole_queue"], false, "{user}");
    }
    station.stop().await;
}

/// A configuration file from before the width choice: a user with the
/// whole width (`full_width`) gets it back as `full`.
#[tokio::test(flavor = "multi_thread")]
async fn an_older_backup_turns_full_width_into_a_width() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let (status, _) = admin
        .send(Method::PUT, "/auth/preferences", json!({"width": "wide"}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, backup) = admin.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{backup}");
    let name = backup["name"].as_str().unwrap();
    let mut file = admin.get(&format!("/backups/{name}/config")).await;
    assert!(
        file["tables"]["users"]
            .as_array()
            .unwrap()
            .iter()
            .any(|user| user["username"] == "admin" && user["width"] == "wide")
    );
    file["schema"] = json!(20270701000000_i64);
    for user in file["tables"]["users"].as_array_mut().unwrap() {
        let user = user.as_object_mut().unwrap();
        user.remove("width");
        // As backups write booleans: 1 or 0.
        let full = i64::from(user["username"] == "admin");
        user.insert("full_width".to_owned(), json!(full));
    }
    let (status, report) = admin.send(Method::POST, "/backups/restore", file).await;
    assert_eq!(status, StatusCode::OK, "{report}");

    let admin = station.signed_in("admin").await;
    let users = admin.get("/users").await;
    for user in users.as_array().unwrap() {
        let expected = if user["username"] == "admin" {
            "full"
        } else {
            "normal"
        };
        assert_eq!(user["width"], expected, "{user}");
    }
    station.stop().await;
}

/// A configuration file from before stream servers: each output holds its
/// server's settings. Restored, its outputs are grouped into servers as the
/// migration does, the station details lifted onto the station.
#[tokio::test(flavor = "multi_thread")]
async fn an_older_backup_groups_its_outputs_into_servers() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let (status, backup) = admin.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{backup}");
    let name = backup["name"].as_str().unwrap();
    let mut file = admin.get(&format!("/backups/{name}/config")).await;
    let profile = file["tables"]["encoder_profiles"][0]["id"].clone();
    file["schema"] = json!(20261228000000_i64);
    let tables = file["tables"].as_object_mut().unwrap();
    tables.remove("stream_servers");
    tables.insert(
        "outputs".to_owned(),
        json!([
            {"id": 11, "station_id": 1, "name": "MP3", "kind": "icecast",
             "encoder_profile_id": profile, "metadata_enabled": true, "enabled": false,
             "config": {"host": "127.0.0.1", "port": 9, "mount": "/mp3", "password": "hackme",
                        "name": "My station", "genre": "Pop"},
             "statistics": {"admin_password": "adm1n"}},
            {"id": 12, "station_id": 1, "name": "AAC", "kind": "icecast",
             "encoder_profile_id": profile, "metadata_enabled": true, "enabled": false,
             "config": {"host": "127.0.0.1", "port": 9, "mount": "/aac", "password": "hackme",
                        "username": "source", "name": "My station HD", "genre": "Pop",
                        "public_url": "https://stream.example.org"},
             "statistics": {"admin_username": "admin", "admin_password": "adm1n"}},
            {"id": 13, "station_id": 1, "name": "Other", "kind": "icecast",
             "encoder_profile_id": profile, "metadata_enabled": true, "enabled": false,
             "config": {"host": "127.0.0.1", "port": 9, "mount": "/other", "password": "else",
                        "name": "My station", "public": true},
             "statistics": null}
        ]),
    );
    let settings = tables["station_settings"].as_array_mut().unwrap();
    settings.retain(|row| row["name"] != "stream_details");
    let (status, report) = admin.send(Method::POST, "/backups/restore", file).await;
    assert_eq!(status, StatusCode::OK, "{report}");

    let admin = station.signed_in("admin").await;
    let servers = admin.get("/stream-servers").await;
    let servers = servers.as_array().unwrap();
    assert_eq!(servers.len(), 2, "{servers:?}");
    assert_eq!(servers[0]["name"], "127.0.0.1:9");
    assert_eq!(servers[0]["admin_password_set"], true);
    assert_eq!(servers[0]["admin_username"], "admin");
    // The first output's public address.
    assert_eq!(servers[0]["public_url"], Value::Null);
    assert_eq!(
        servers[0]["outputs"],
        json!([{"id": 12, "name": "AAC"}, {"id": 11, "name": "MP3"}])
    );
    assert_eq!(servers[1]["name"], "127.0.0.1:9 (2)");
    assert_eq!(servers[1]["admin_password_set"], false);
    assert_eq!(
        admin.get("/settings/stream-details").await,
        json!({"name": "My station", "description": null, "genre": "Pop", "url": null,
               "public": false})
    );
    let outputs = admin.get("/outputs").await;
    let shown: Vec<(Value, Value, Value)> = outputs
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            (
                o["config"].clone(),
                o["details"].clone(),
                o["server_id"].clone(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            (
                json!({"mount": "/mp3"}),
                json!({}),
                servers[0]["id"].clone()
            ),
            (
                json!({"mount": "/aac"}),
                json!({"name": "My station HD"}),
                servers[0]["id"].clone()
            ),
            (
                json!({"mount": "/other"}),
                json!({"genre": "", "public": true}),
                servers[1]["id"].clone()
            ),
        ]
    );
    // The engine gets what each output had.
    let config = eventually(
        "the outputs in the engine",
        Duration::from_secs(10),
        || async {
            let config = station.state.stations.first().engine.config().await.ok()?;
            let config: heron_core::OutputsConfig =
                serde_json::from_str(&config.outputs_json).ok()?;
            (config.outputs.len() == 3).then_some(config)
        },
    )
    .await;
    let other = config.outputs.iter().find(|o| o.id == 13).unwrap();
    assert_eq!(
        other.config,
        json!({"host": "127.0.0.1", "port": 9, "mount": "/other", "username": "source",
               "password": "else", "name": "My station", "public": true})
    );
    station.stop().await;
}

/// A base32 secret (RFC 4648, no padding) as apps read it.
fn from_base32(text: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for byte in text.bytes() {
        let value = alphabet.iter().position(|c| *c == byte).unwrap() as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

/// The app's code for a step, as typed.
fn totp_code(secret: &[u8], step: u64) -> String {
    format!("{:06}", crate::two_factor::totp::code(secret, step))
}

impl Station {
    /// Signs in with the password: the answer and the pending cookie.
    async fn password_step(&self, username: &str) -> (StatusCode, Value, Client) {
        let mut client = self.client();
        let response = client
            .http
            .post(format!("{}/auth/login", client.base))
            .json(&json!({"username": username, "password": "correct horse"}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let cookie = response
            .headers()
            .get("set-cookie")
            .map(|value| value.to_str().unwrap().to_owned());
        let body = response.json().await.unwrap_or(Value::Null);
        client.cookie = cookie.map(|cookie| cookie.split(';').next().unwrap().to_owned());
        (status, body, client)
    }

    /// Finishes a sign-in with a second factor: the signed-in client.
    async fn second_step(
        &self,
        pending: &Client,
        path: &str,
        code: &str,
    ) -> Result<Client, (StatusCode, Value)> {
        let mut request = pending
            .http
            .post(format!("{}{path}", pending.base))
            .json(&json!({"code": code}));
        if let Some(cookie) = &pending.cookie {
            request = request.header("cookie", cookie);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        if status != StatusCode::OK {
            return Err((status, response.json().await.unwrap_or(Value::Null)));
        }
        let cookies: Vec<String> = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect();
        assert!(
            cookies.iter().any(|c| c.starts_with("heron_pending=;")),
            "{cookies:?}"
        );
        let session = cookies
            .iter()
            .find(|c| c.starts_with("heron_session="))
            .unwrap();
        let mut client = self.client();
        client.cookie = Some(session.split(';').next().unwrap().to_owned());
        Ok(client)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_factor_authentication() {
    let Some(station) = Station::start().await else {
        return;
    };
    let pool = station.db.pool.clone();
    let admin = station.signed_in("admin").await;
    let elsewhere = station.signed_in("admin").await;
    let view = admin.get("/auth/two-factor").await;
    assert_eq!(view["totp"], Value::Null);
    assert_eq!(view["recovery_codes_left"], 0);
    // Nothing to regenerate without a factor.
    let (status, _) = admin
        .send(
            Method::POST,
            "/auth/two-factor/recovery-codes",
            json!({"password": "correct horse"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Setting up the app asks for the password, then a code.
    let (status, _) = admin
        .send(
            Method::POST,
            "/auth/two-factor/totp",
            json!({"password": "wrong"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, setup) = admin
        .send(
            Method::POST,
            "/auth/two-factor/totp",
            json!({"password": "correct horse"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{setup}");
    assert!(
        setup["uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/Heron:admin?secret=")
    );
    assert!(setup["qr_svg"].as_str().unwrap().contains("<svg"));
    let secret = from_base32(setup["secret"].as_str().unwrap());
    assert_eq!(secret.len(), 20);
    let now = crate::two_factor::now_step();
    let (status, _) = admin
        .send(
            Method::POST,
            "/auth/two-factor/totp/confirm",
            json!({"code": "000000"}),
        )
        .await;
    // (Unless 000000 happens to be the code.)
    if totp_code(&secret, now) != "000000" {
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (status, added) = admin
        .send(
            Method::POST,
            "/auth/two-factor/totp/confirm",
            json!({"code": totp_code(&secret, now)}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    let codes: Vec<String> = serde_json::from_value(added["recovery_codes"].clone()).unwrap();
    assert_eq!(codes.len(), 10);
    // The other session is signed out, this one stays.
    let (status, _) = elsewhere.request(Method::GET, "/auth/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let me = admin.get("/auth/me").await;
    assert_eq!(
        me["two_factor"],
        json!({"totp": true, "passkeys": 0, "required": false})
    );
    assert_eq!(
        admin.get("/auth/two-factor").await["recovery_codes_left"],
        10
    );

    // Signing in now asks for the second factor.
    let (status, body, pending) = station.password_step("admin").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body, json!({"second_factor": ["totp", "recovery"]}));
    let cookie = pending.cookie.clone().unwrap();
    assert!(cookie.starts_with("heron_pending="));
    // The pending cookie is not a session.
    let (status, _) = pending.request(Method::GET, "/auth/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Without it, the second step does not start.
    let (status, _) = station
        .second_step(
            &station.client(),
            "/auth/login/totp",
            &totp_code(&secret, now + 1),
        )
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The code used to set the app up cannot be used again.
    let (status, body) = station
        .second_step(&pending, "/auth/login/totp", &totp_code(&secret, now))
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let signed_in = station
        .second_step(&pending, "/auth/login/totp", &totp_code(&secret, now + 1))
        .await
        .unwrap();
    assert_eq!(signed_in.get("/auth/me").await["username"], "admin");
    // The pending sign-in is used up; so is the code.
    let (status, _) = station
        .second_step(&pending, "/auth/login/totp", &totp_code(&secret, now + 1))
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, _, pending) = station.password_step("admin").await;
    let (status, _) = station
        .second_step(&pending, "/auth/login/totp", &totp_code(&secret, now + 1))
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // A recovery code works once, typed in any case.
    let admin = station
        .second_step(&pending, "/auth/login/recovery", &codes[0].to_lowercase())
        .await
        .unwrap();
    assert_eq!(
        admin.get("/auth/two-factor").await["recovery_codes_left"],
        9
    );
    let (_, _, pending) = station.password_step("admin").await;
    let (status, _) = station
        .second_step(&pending, "/auth/login/recovery", &codes[0])
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // New codes void the old ones.
    let (status, fresh) = admin
        .send(
            Method::POST,
            "/auth/two-factor/recovery-codes",
            json!({"password": "correct horse"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    let (status, _) = station
        .second_step(&pending, "/auth/login/recovery", &codes[1])
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let admin = station
        .second_step(
            &pending,
            "/auth/login/recovery",
            fresh["recovery_codes"][3].as_str().unwrap(),
        )
        .await
        .unwrap();

    // Passkeys: the browser's address is the relying party; none to use yet.
    let response = admin
        .http
        .post(format!("{}/auth/two-factor/passkeys", admin.base))
        .header("cookie", admin.cookie.as_deref().unwrap())
        .header("x-requested-with", "test")
        .header("origin", "http://localhost:8000")
        .json(&json!({"password": "correct horse", "name": "Laptop"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let options: Value = response.json().await.unwrap();
    assert_eq!(options["publicKey"]["rp"]["id"], "localhost", "{options}");
    assert!(options["publicKey"]["challenge"].is_string());
    let (_, _, pending) = station.password_step("admin").await;
    let response = pending
        .http
        .post(format!("{}/auth/login/webauthn/start", pending.base))
        .header("cookie", pending.cookie.as_deref().unwrap())
        .header("origin", "http://localhost:8000")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Required for viewers: a viewer without a factor can only set one up.
    let viewer = station.signed_in("viewer").await;
    let (status, token) = viewer
        .send(Method::POST, "/tokens", json!({"name": "display"}))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = viewer
        .send(Method::PUT, "/settings/two-factor", json!({"viewer": true}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = admin
        .send(Method::PUT, "/settings/two-factor", json!({"viewer": true}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = viewer.request(Method::GET, "/queue", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "two_factor_required");
    assert_eq!(viewer.get("/auth/me").await["two_factor"]["required"], true);
    assert_eq!(
        viewer.get("/auth/two-factor").await["required_by"],
        "viewer"
    );
    // API tokens are not concerned.
    let mut scripted = station.client();
    scripted.bearer = Some(token["token"].as_str().unwrap().to_owned());
    scripted.get("/queue").await;
    let users = admin.get("/users").await;
    let state: Vec<(String, Value)> = users
        .as_array()
        .unwrap()
        .iter()
        .map(|user| {
            (
                user["username"].as_str().unwrap().to_owned(),
                user["two_factor"].clone(),
            )
        })
        .collect();
    assert_eq!(
        state,
        [
            (
                "admin".to_owned(),
                json!({"totp": true, "passkeys": 0, "required": false})
            ),
            (
                "viewer".to_owned(),
                json!({"totp": false, "passkeys": 0, "required": true})
            ),
        ]
    );
    // The viewer sets up an app: then everything is open again.
    let (_, setup) = viewer
        .send(
            Method::POST,
            "/auth/two-factor/totp",
            json!({"password": "correct horse"}),
        )
        .await;
    let viewer_secret = from_base32(setup["secret"].as_str().unwrap());
    let (status, _) = viewer
        .send(
            Method::POST,
            "/auth/two-factor/totp/confirm",
            json!({"code": totp_code(&viewer_secret, crate::two_factor::now_step())}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    viewer.get("/queue").await;
    // The last factor of a role that requires one cannot be removed.
    let (status, _) = viewer
        .send(
            Method::DELETE,
            "/auth/two-factor/totp",
            json!({"password": "correct horse"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // A backup keeps second factors and the requirement.
    let (status, backup) = admin.send(Method::POST, "/backups", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{backup}");
    let name = backup["name"].as_str().unwrap().to_owned();
    let file = admin.get(&format!("/backups/{name}/config")).await;
    assert_eq!(file["tables"]["user_totp"].as_array().unwrap().len(), 2);
    assert_eq!(
        file["tables"]["user_recovery_codes"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    let (status, _) = admin
        .send(
            Method::DELETE,
            "/auth/two-factor/totp",
            json!({"password": "correct horse"}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // The last factor gone, the codes go too.
    assert_eq!(
        admin.get("/auth/two-factor").await["recovery_codes_left"],
        0
    );
    let (status, body) = admin.send(Method::POST, "/backups/restore", file).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body, pending) = station.password_step("admin").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let admin = station
        .second_step(
            &pending,
            "/auth/login/recovery",
            fresh["recovery_codes"][5].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        admin.get("/settings/two-factor").await,
        json!({"admin": false, "operator": false, "viewer": true})
    );

    // The command line's reset removes everything: the password is enough.
    let viewer_id = users::user_id(&pool, "viewer").await.unwrap();
    assert!(crate::two_factor::reset(&pool, viewer_id).await.unwrap());
    assert!(!crate::two_factor::reset(&pool, viewer_id).await.unwrap());
    let (status, _, viewer) = station.password_step("viewer").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = viewer.request(Method::GET, "/queue", None).await;
    assert_eq!(
        (status, &body["error"]),
        (StatusCode::FORBIDDEN, &json!("two_factor_required"))
    );
    station.stop().await;
}

fn png(width: u32, height: u32, shade: u8) -> Vec<u8> {
    let picture = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        width,
        height,
        image::Rgb([shade, 100, 50]),
    ));
    let mut out = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

impl Client {
    /// Sends an image, as an upload does.
    async fn put_image(&self, path: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
        let mut request = self
            .http
            .put(format!("{}{path}", self.base))
            .header("content-type", "image/png")
            .body(bytes);
        if let Some(cookie) = &self.cookie {
            request = request
                .header("cookie", cookie)
                .header("x-requested-with", "test");
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let value = response.json().await.unwrap_or(Value::Null);
        (status, value)
    }
}

/// The lists of *To do*: the active songs (enabled, present, in a category
/// of the Song type) missing an intro, a hook or a cover, and their counts.
#[tokio::test(flavor = "multi_thread")]
async fn to_do_lists_the_songs_missing_an_intro_a_hook_or_a_cover() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let pool = &station.db.pool;
    let t = station.tracks.clone();

    // Songs: t0, t1, t2 (disabled) and t4 (also a jingle); t3 a jingle only.
    sqlx::query(
        "INSERT INTO categories (id, name, type_id) VALUES (101, 'Songs', 1), (102, 'Jingles', 2)",
    )
    .execute(pool)
    .await
    .unwrap();
    for (track, category) in [
        (t[0], 101),
        (t[1], 101),
        (t[2], 101),
        (t[3], 102),
        (t[4], 101),
        (t[4], 102),
    ] {
        sqlx::query("INSERT INTO track_categories (track_id, category_id) VALUES (?, ?)")
            .bind(track)
            .bind(category)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE tracks SET enabled = FALSE WHERE id = ?")
        .bind(t[2])
        .execute(pool)
        .await
        .unwrap();
    // t0 has everything; t4 an intro and half a hook.
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{}/intro", t[0]),
            json!({"intro_ms": 1000}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/tracks/{}/hook", t[0]),
            json!({"hook_in_ms": 500, "hook_out_ms": 1500}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .put_image(
            &format!("/tracks/{}/cover?scope=track", t[0]),
            png(50, 50, 1),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("INSERT INTO track_markers (track_id, intro_ms, hook_in_ms) VALUES (?, 800, 400)")
        .bind(t[4])
        .execute(pool)
        .await
        .unwrap();

    let ids = |page: &Value| -> Vec<u64> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|track| track["id"].as_u64().unwrap())
            .collect()
    };
    let songs = viewer.get("/tracks?active_songs=true").await;
    assert_eq!(ids(&songs), vec![t[0], t[1], t[4]]);
    let no_intro = viewer
        .get("/tracks?active_songs=true&filter=no_intro")
        .await;
    assert_eq!(ids(&no_intro), vec![t[1]]);
    let no_hook = viewer
        .get("/tracks?active_songs=true&filter=no_hook&sort=title&desc=true")
        .await;
    assert_eq!(ids(&no_hook), vec![t[4], t[1]]);
    assert_eq!(no_hook["total"], 2);
    let no_cover = viewer
        .get("/tracks?active_songs=true&filter=no_cover")
        .await;
    assert_eq!(ids(&no_cover), vec![t[1], t[4]]);
    // Without `active_songs`, every track.
    assert_eq!(viewer.get("/tracks?filter=no_intro").await["total"], 3);
    assert_eq!(viewer.get("/tracks?filter=no_hook").await["total"], 4);

    sqlx::query(
        "INSERT INTO track_identifications (track_id, status, reason) VALUES (?, 'needs_review', 'doubtful')",
    )
    .bind(t[3])
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(
        viewer.get("/library/todo").await,
        json!({"to_identify": 1, "no_intro": 1, "no_hook": 2, "no_cover": 2})
    );

    // A missing file, or a track in the trash, is not to do.
    sqlx::query("UPDATE tracks SET missing_since = NOW(6) WHERE id = ?")
        .bind(t[4])
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tracks SET deleted_at = NOW(6) WHERE id = ?")
        .bind(t[1])
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        viewer.get("/library/todo").await,
        json!({"to_identify": 1, "no_intro": 0, "no_hook": 0, "no_cover": 0})
    );

    // Without cover art, no count of covers.
    let (status, _) = admin
        .send(Method::PUT, "/modules", json!({"cover_art": false}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(viewer.get("/library/todo").await["no_cover"], Value::Null);
    station.stop().await;
}

/// Covers set by hand for an album or a track, served by hash with a long
/// cache, in track lists, history and the public side (the station logo
/// standing in), Cover Art Archive candidates, and backups.
#[tokio::test(flavor = "multi_thread")]
async fn covers_are_set_by_hand_shown_and_backed_up() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let pool = &station.db.pool;
    let t = station.tracks.clone();

    let (status, _) = viewer
        .put_image(&format!("/tracks/{}/cover", t[0]), png(800, 800, 1))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, error) = admin
        .put_image(&format!("/tracks/{}/cover", t[0]), b"not an image".to_vec())
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    let (status, cover) = admin
        .put_image(&format!("/tracks/{}/cover", t[0]), png(800, 400, 1))
        .await;
    assert_eq!(status, StatusCode::OK, "{cover}");
    assert_eq!(cover["source"], "hand");
    assert_eq!(cover["scope"], "album");
    assert_eq!(
        (cover["width"].as_u64(), cover["height"].as_u64()),
        (Some(800), Some(400))
    );
    let url = cover["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("/api/v1/public/covers/"), "{url}");

    // A track of its own.
    let (status, own) = admin
        .put_image(
            &format!("/tracks/{}/cover?scope=track", t[1]),
            png(50, 50, 2),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(own["scope"], "track");

    // In the track list, with a filter for the others.
    let page = viewer.get("/tracks").await;
    let urls: std::collections::HashMap<u64, Value> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| (track["id"].as_u64().unwrap(), track["cover_url"].clone()))
        .collect();
    assert_eq!(urls[&t[0]], json!(url));
    assert_eq!(urls[&t[1]], own["url"]);
    assert_eq!(urls[&t[2]], Value::Null);
    assert_eq!(viewer.get("/tracks?filter=no_cover").await["total"], 3);

    // Served by hash to anyone, cached for a year; by track to those signed in.
    let http = reqwest::Client::new();
    let response = http
        .get(format!("http://{}{url}?size=96", station.api))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/jpeg");
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    let etag = response.headers()["etag"].clone();
    let thumbnail = image::load_from_memory(&response.bytes().await.unwrap()).unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (96, 48));
    let again = http
        .get(format!("http://{}{url}?size=96", station.api))
        .header("if-none-match", etag)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
    let large = viewer.get_bytes(&format!("/tracks/{}/cover", t[0])).await;
    let large = image::load_from_memory(&large).unwrap();
    assert_eq!((large.width(), large.height()), (600, 300));
    let (status, _) = viewer
        .request(Method::GET, &format!("/tracks/{}/cover", t[2]), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The public side: full addresses, the logo for a track without a cover.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/station",
            json!({"timezone": "Europe/Paris", "public_url": "https://radio.example.org/"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/station",
            json!({"timezone": "Europe/Paris", "public_url": "ftp://radio.example.org"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = admin
        .put_image("/settings/station/logo", png(300, 300, 3))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for (track, minutes) in [(t[0], 10), (t[2], 5)] {
        sqlx::query(
            "INSERT INTO play_history (station_id, track_id, started_at, ended_at, deck, source)
             VALUES (1, ?, NOW(6) - INTERVAL ? MINUTE, NOW(6) - INTERVAL ? MINUTE, 'a', 'rotation')",
        )
        .bind(track)
        .bind(minutes)
        .bind(minutes - 1)
        .execute(pool)
        .await
        .unwrap();
    }
    let plays = station.client().get("/public/history").await;
    let logo_hash: Vec<u8> = sqlx::query_scalar("SELECT logo_hash FROM stations WHERE id = 1")
        .fetch_one(pool)
        .await
        .unwrap();
    let logo = format!(
        "https://radio.example.org/api/v1/public/covers/{}",
        heron_library::covers::hex(&logo_hash)
    );
    // The engine plays too: the plays are found by title.
    let cover_of = |plays: &Value, title: &str| {
        plays
            .as_array()
            .unwrap()
            .iter()
            .find(|play| play["title"] == title)
            .map(|play| play["cover_url"].clone())
            .unwrap()
    };
    assert_eq!(cover_of(&plays, "Track 3"), json!(logo), "{plays}");
    assert_eq!(
        cover_of(&plays, "Track 1"),
        json!(format!("https://radio.example.org{url}"))
    );
    // Heron's own history: the cover or nothing.
    let history = admin.get("/history").await;
    assert_eq!(cover_of(&history, "Track 3"), Value::Null);
    assert_eq!(cover_of(&history, "Track 1"), json!(url));

    // In backups: covers set by hand and the logo, with their images.
    let files = station.state.library.covers().clone();
    let file = crate::backups::file::export(
        pool,
        &files,
        crate::backups::file::Kind::Manual,
        "2026-12-15T00:00:00Z".to_owned(),
    )
    .await
    .unwrap();
    assert_eq!(file.tables["covers"].len(), 3);
    // The images, read from their files, as the database had them before
    // they were files: older backups restore the same way.
    for row in &file.tables["covers"] {
        let hash =
            heron_library::covers::parse_hex(&row["hash"].as_str().unwrap().to_ascii_lowercase())
                .unwrap();
        let image = files.read(&hash, 600).await.unwrap().unwrap();
        assert_eq!(
            row["image"].as_str().unwrap(),
            heron_library::covers::hex(&image).to_uppercase()
        );
        assert!(row["thumbnail"].as_str().is_some_and(|hex| !hex.is_empty()));
    }
    assert_eq!(file.tables["album_covers"].len(), 1);
    assert_eq!(
        file.tables["track_covers"][0]["track"],
        "Artist 2/Track 2.wav"
    );
    sqlx::query("UPDATE stations SET logo_hash = NULL")
        .execute(pool)
        .await
        .unwrap();
    for table in ["album_covers", "track_covers"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table}")))
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM covers")
        .execute(pool)
        .await
        .unwrap();
    // The covers folder lost too.
    std::fs::remove_dir_all(station.covers.path()).unwrap();
    let mut tx = pool.begin().await.unwrap();
    let mut report = crate::backups::file::RestoreReport::default();
    crate::backups::file::restore(&mut tx, &files, &file, &mut report)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let page = viewer.get("/tracks").await;
    let restored: Vec<&Value> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|track| &track["cover_url"])
        .collect();
    assert_eq!(restored.iter().filter(|url| !url.is_null()).count(), 2);
    let logo_back: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT logo_hash FROM stations WHERE id = 1")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(logo_back, Some(logo_hash.clone()));
    // Their images are files again.
    let back = viewer.get_bytes(&format!("/tracks/{}/cover", t[0])).await;
    assert_eq!(
        image::load_from_memory(&back).unwrap().width(),
        600,
        "restored from the backup"
    );
    let response = http
        .get(format!(
            "http://{}/api/v1/public/covers/{}?size=96",
            station.api,
            heron_library::covers::hex(&logo_hash)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // A file lost: not found, until the image is stored again.
    let logo_file = files.path(&logo_hash, 600);
    std::fs::remove_file(&logo_file).unwrap();
    let (status, _) = viewer
        .request(Method::GET, "/settings/station/logo", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = admin
        .put_image("/settings/station/logo", png(300, 300, 3))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(logo_file.is_file(), "written again");

    // Removed: the file has none, nothing comes back.
    let (status, now) = admin
        .request(Method::DELETE, &format!("/tracks/{}/cover", t[0]), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(now, Value::Null);

    // Cover Art Archive lookups are library tasks; unsure ones wait.
    let (status, task) = admin
        .send(
            Method::POST,
            "/covers/fetch",
            json!({"selection": {"track_ids": [t[2]]}}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{task}");
    assert_eq!(task["request"]["kind"], "fetch_covers");
    assert_eq!(task["request"]["track_ids"], json!([t[2]]));
    sqlx::query(
        "INSERT INTO cover_candidates (album_key, release_id, title, artist, score, thumbnail_url)
         SELECT album_key, '00000000-0000-4000-8000-000000000001', 'Album', 'Artist 3', 70,
                'https://coverartarchive.org/release/x/front-250'
         FROM tracks WHERE id = ?",
    )
    .bind(t[2])
    .execute(pool)
    .await
    .unwrap();
    let albums = viewer.get("/covers/candidates").await;
    assert_eq!(albums.as_array().unwrap().len(), 1, "{albums}");
    assert_eq!(albums[0]["track_id"], t[2]);
    assert_eq!(albums[0]["candidates"][0]["score"], 70);
    let detail = viewer.get(&format!("/tracks/{}", t[2])).await;
    assert_eq!(detail["cover_candidates"].as_array().unwrap().len(), 1);
    let candidate = albums[0]["candidates"][0]["id"].as_u64().unwrap();
    let (status, task) = admin
        .send(
            Method::POST,
            &format!("/covers/candidates/{candidate}/accept"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(task["request"]["kind"], "apply_cover");
    let (status, _) = admin
        .send(
            Method::POST,
            "/covers/candidates/refuse",
            json!({"album_key": albums[0]["album_key"]}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(viewer.get("/covers/candidates").await, json!([]));
    station.stop().await;
}

/// Modules turned off for the installation or a station: their routes
/// answer 404, their background work stops, and their data is back once
/// they are on again (docs/modules.md).
#[tokio::test(flavor = "multi_thread")]
async fn modules_are_turned_off_and_on_again() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let viewer = station.signed_in("viewer").await;
    let pool = &station.db.pool;
    let t = station.tracks.clone();
    let all_on = json!({"medleys": true, "cover_art": true, "listener_statistics": true,
                        "webhooks": true, "restrictions": true, "eq": true, "processing": true,
                        "trash": true, "review": true, "artists": true, "labels": true});

    // Everything is on after an upgrade.
    assert_eq!(viewer.get("/modules").await, all_on);
    let modules = viewer.get("/stations/1/modules").await;
    assert_eq!(modules["on"], all_on);
    assert_eq!(modules["station"], all_on);
    let (status, _) = viewer
        .send(Method::PUT, "/modules", json!({"webhooks": false}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = viewer
        .send(
            Method::PUT,
            "/stations/1/modules",
            json!({"webhooks": false}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = admin
        .request(Method::GET, "/stations/99/modules", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Data of each module.
    let (mut received, receiver) = webhook_receiver().await;
    let (status, created) = admin
        .send(
            Method::POST,
            "/webhooks",
            json!({"name": "Website", "url": format!("http://{receiver}/json"),
                   "events": ["track.started"]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, _) = admin
        .put_image(&format!("/tracks/{}/cover", t[1]), png(64, 64, 3))
        .await;
    assert_eq!(status, StatusCode::OK);
    let medley = sqlx::query("INSERT INTO medleys (name, recipe, track_id) VALUES ('M', '{}', ?)")
        .bind(t[0])
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id();
    sqlx::query("INSERT INTO medley_stations (medley_id, station_id, position) VALUES (?, 1, 0)")
        .bind(medley)
        .execute(pool)
        .await
        .unwrap();
    let profile = admin.get("/encoder-profiles").await[0]["id"]
        .as_u64()
        .unwrap();
    let (status, server) = admin
        .send(
            Method::POST,
            "/stream-servers",
            json!({"name": "Local", "host": "127.0.0.1", "port": 9, "source_password": "hackme"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{server}");
    let (status, output) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile,
                   "server_id": server["id"], "config": {"mount": "/live"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{output}");
    let collector = &station.state.listeners;
    collector.poll(rotation_now()).await.unwrap();
    assert_eq!(collector.last().len(), 1, "the output is read");
    // Webhooks are sent.
    tokio::time::timeout(Duration::from_secs(15), received.recv())
        .await
        .expect("a delivery")
        .unwrap();

    // Off for the station: webhooks and listener statistics.
    let (status, modules) = admin
        .send(
            Method::PUT,
            "/stations/1/modules",
            json!({"webhooks": false, "listener_statistics": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{modules}");
    assert_eq!(modules["installation"], all_on);
    assert_eq!(modules["on"]["webhooks"], false);
    assert_eq!(modules["on"]["medleys"], true);
    for path in ["/webhooks", "/stations/1/webhooks", "/listeners/now"] {
        let (status, body) = admin.request(Method::GET, path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body["error"], "module_off", "{path}");
    }
    // The statistics settings are the installation's.
    admin.get("/settings/statistics").await;
    collector.poll(rotation_now()).await.unwrap();
    assert!(
        collector.last().is_empty(),
        "the station's servers are not read"
    );
    // Nothing is sent: what was on its way fails.
    while tokio::time::timeout(Duration::from_millis(100), received.recv())
        .await
        .is_ok()
    {}
    tokio::time::sleep(Duration::from_secs(2 * TRACK_SECONDS as u64)).await;
    assert!(received.try_recv().is_err(), "a webhook was sent");
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_deliveries WHERE status = 'pending'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(pending, 0);

    // Off for the installation: medleys and cover art.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/modules",
            json!({"medleys": false, "cover_art": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let modules = viewer.get("/stations/1/modules").await;
    assert_eq!(modules["station"]["medleys"], true);
    assert_eq!(modules["on"]["medleys"], false);
    for path in [
        "/medleys".to_owned(),
        format!("/medleys/{medley}"),
        "/stations/1/settings/medleys".to_owned(),
        format!("/tracks/{}/cover", t[1]),
        "/covers/candidates".to_owned(),
        "/stations/1/settings/station/logo".to_owned(),
    ] {
        let (status, body) = viewer.request(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body["error"], "module_off", "{path}");
    }
    let page = viewer.get("/tracks").await;
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|track| track["cover_url"].is_null()),
        "{page}"
    );
    assert!(viewer.get(&format!("/tracks/{}", t[1])).await["cover"].is_null());
    assert!(viewer.get("/public/now-playing").await["now"]["cover_url"].is_null());
    // The medley never plays.
    let library = crate::rotation::load_library(pool, 1).await.unwrap();
    assert!(!library.tracks.contains_key(&t[0]));
    assert!(library.tracks.contains_key(&t[1]));
    let (status, refused) = admin
        .send(Method::POST, "/queue", json!({"track_ids": [t[0]]}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("medley"));

    // On again: everything is back as it was.
    let (status, _) = admin.send(Method::PUT, "/modules", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, modules) = admin
        .send(Method::PUT, "/stations/1/modules", json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(modules["on"], all_on);
    assert_eq!(admin.get("/webhooks").await[0]["name"], "Website");
    assert_eq!(admin.get("/medleys").await[0]["id"], medley);
    assert!(viewer.get(&format!("/tracks/{}", t[1])).await["cover"].is_object());
    let library = crate::rotation::load_library(pool, 1).await.unwrap();
    assert!(library.tracks.contains_key(&t[0]));
    collector.poll(rotation_now()).await.unwrap();
    assert_eq!(collector.last().len(), 1);
    tokio::time::timeout(Duration::from_secs(15), received.recv())
        .await
        .expect("a delivery once webhooks are on again")
        .unwrap();
    station.stop().await;
}

/// Restrictions, the equalizer and processing turned off for a station:
/// hidden and not applied (rotation ignores the restrictions, the engine
/// gets the EQ and the AGC chain bypassed), their settings kept. The
/// library's modules, off for the installation: their routes answer 404
/// (docs/modules.md).
#[tokio::test(flavor = "multi_thread")]
async fn more_modules_are_turned_off_and_on_again() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let pool = &station.db.pool;
    let t = station.tracks.clone();
    let engine = station.state.stations.first().engine.clone();

    // A restriction, an EQ and a chain in use.
    let (status, restriction) = admin
        .send(
            Method::POST,
            "/restrictions",
            json!({"target": {"kind": "track", "id": t[1]},
                   "rule": {"kind": "daypart", "mode": "ban", "days": [1, 2, 3, 4, 5, 6, 7],
                            "start_hour": 0, "end_hour": 24}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{restriction}");
    let eq = json!({
        "music": {"enabled": true, "gains_db": [3.0, 2.0, 0.0, 0.0, -1.5, 0.0, 0.0, 1.0, 1.0, 0.0]},
        "fx": {"enabled": true, "gains_db": [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]}
    });
    let (status, body) = admin.send(Method::PUT, "/settings/eq", eq.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut agc = admin.get("/settings/agc").await;
    agc["bypass_all"] = json!(false);
    let (status, agc) = admin.send(Method::PUT, "/settings/agc", agc).await;
    assert_eq!(status, StatusCode::OK, "{agc}");
    let library = crate::rotation::load_library(pool, 1).await.unwrap();
    assert_eq!(library.restrictions.len(), 1);

    // What the engine has, once the configuration sync has run.
    let engine_has = async |eq_enabled: bool, bypass_all: bool| {
        for _ in 0..100 {
            let config = engine.config().await.unwrap();
            let engine_eq: Value = serde_json::from_str(&config.eq_settings_json).unwrap();
            let engine_agc: Value = serde_json::from_str(&config.agc_settings_json).unwrap();
            if engine_eq["music"]["enabled"] == eq_enabled
                && engine_eq["fx"]["enabled"] == eq_enabled
                && engine_agc["bypass_all"] == bypass_all
            {
                return (engine_eq, engine_agc);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the engine never had eq enabled {eq_enabled}, bypass_all {bypass_all}");
    };
    let (engine_eq, _) = engine_has(true, false).await;
    assert_eq!(engine_eq, eq);

    // Off for the station.
    let (status, modules) = admin
        .send(
            Method::PUT,
            "/stations/1/modules",
            json!({"restrictions": false, "eq": false, "processing": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{modules}");
    assert_eq!(modules["on"]["eq"], false);
    for path in ["/settings/eq", "/stations/1/settings/agc"] {
        let (status, body) = admin.request(Method::GET, path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body["error"], "module_off", "{path}");
    }
    // Shared: they answer while the installation has them on.
    for path in ["/restrictions", "/eq-presets", "/agc-presets"] {
        admin.get(path).await;
    }
    let library = crate::rotation::load_library(pool, 1).await.unwrap();
    assert!(
        library.restrictions.is_empty(),
        "restrictions still applied"
    );
    // Bypassed on air, the gains and blocks kept.
    let (engine_eq, engine_agc) = engine_has(false, true).await;
    assert_eq!(engine_eq["music"]["gains_db"], eq["music"]["gains_db"]);
    assert_eq!(engine_agc["final_stage"], agc["final_stage"]);
    let stored: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT CAST(value AS CHAR) FROM station_settings WHERE station_id = 1 AND name = 'eq'",
        )
        .fetch_one(pool)
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stored, eq, "the station's EQ is kept");

    // Off for the installation, the shared routes too.
    let (status, _) = admin
        .send(
            Method::PUT,
            "/modules",
            json!({"restrictions": false, "eq": false, "processing": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    for path in [
        "/restrictions".to_owned(),
        format!("/restrictions/{}", restriction["id"]),
        "/eq-presets".to_owned(),
        "/agc-presets".to_owned(),
    ] {
        let (status, body) = admin.request(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body["error"], "module_off", "{path}");
    }

    // On again: applied as they were.
    let (status, _) = admin.send(Method::PUT, "/modules", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .send(Method::PUT, "/stations/1/modules", json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (engine_eq, engine_agc) = engine_has(true, false).await;
    assert_eq!(engine_eq, eq);
    assert_eq!(engine_agc, agc);
    assert_eq!(admin.get("/settings/eq").await, eq);
    let library = crate::rotation::load_library(pool, 1).await.unwrap();
    assert_eq!(library.restrictions.len(), 1);

    // The library's modules: a station cannot turn them off.
    let library_off = json!({"trash": false, "review": false, "artists": false, "labels": false});
    let (status, modules) = admin
        .send(Method::PUT, "/stations/1/modules", library_off.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{modules}");
    assert_eq!(modules["station"]["trash"], true);
    assert_eq!(modules["on"]["labels"], true);
    admin.get("/labels").await;
    // Off for the installation.
    let (status, _) = admin.send(Method::PUT, "/modules", library_off).await;
    assert_eq!(status, StatusCode::OK);
    let modules = admin.get("/stations/1/modules").await;
    assert_eq!(modules["on"]["trash"], false);
    for path in [
        "/library/trash".to_owned(),
        "/library/todo".to_owned(),
        "/library/review".to_owned(),
        "/library/batches".to_owned(),
        // Refused before the artist is looked for.
        "/artists/1".to_owned(),
        "/artist-suggestions".to_owned(),
        "/labels".to_owned(),
    ] {
        let (status, body) = admin.request(Method::GET, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body["error"], "module_off", "{path}");
    }
    let (status, body) = admin
        .send(
            Method::POST,
            "/tracks/labels",
            json!({"selection": {"track_ids": [t[0]]}, "add": [], "remove": []}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // The artist pickers' search still answers.
    let (status, _) = admin.request(Method::GET, "/artists", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin.send(Method::PUT, "/modules", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    admin.get("/library/trash").await;
    station.stop().await;
}

fn rotation_now() -> i64 {
    crate::rotation::now_ms()
}

/// A clock slot at an exact time: hard, the track on air is cut and the
/// slot starts then; soft, a track queued by hand that would end after the
/// window is taken out.
#[tokio::test(flavor = "multi_thread")]
async fn exact_times_cut_on_time_and_take_out_what_no_longer_fits() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let t = &station.tracks;
    let mut categories = Vec::new();
    // Songs repeat at once: they also fill the gaps before exact times.
    let free = json!({"track_min": 0, "title_min": 0, "artist_min": 0, "album_min": 0});
    for (name, members) in [("News", &t[4..]), ("Songs", &t[..4])] {
        let (status, category) = admin
            .send(
                Method::POST,
                "/categories",
                json!({"name": name, "separation": free}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        for track in members {
            let (status, _) = admin
                .send(
                    Method::PUT,
                    &format!("/tracks/{track}/categories"),
                    json!([{"category_id": category["id"]}]),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
        }
        categories.push(category["id"].as_u64().unwrap());
    }
    let (news, songs) = (categories[0], categories[1]);
    let mut rotation = admin.get("/settings/rotation").await;
    rotation["filler_category_id"] = json!(songs);
    let (status, body) = admin
        .send(Method::PUT, "/settings/rotation", rotation)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Exact times follow one another, and fit in the hour.
    for exact in [json!({"at_s": 3_600}), json!({"at_s": 10, "after_s": 301})] {
        let (status, body) = admin
            .send(
                Method::POST,
                "/clocks",
                json!({"name": "Bad", "slots": [{"category_id": news, "exact": exact}]}),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    // Keep a minute and a half from the end of the hour.
    while rotation_now().rem_euclid(3_600_000) > 3_500_000 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let hour_start = rotation_now() - rotation_now().rem_euclid(3_600_000);
    let hard_s = (rotation_now() - hour_start) / 1_000 + 20;
    let soft_s = hard_s + 40;
    let (status, clock) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Exact", "slots": [
                {"category_id": songs, "enforce_rules": false},
                {"category_id": news, "enforce_rules": false,
                 "exact": {"at_s": hard_s, "mode": "hard"}},
                {"category_id": songs, "enforce_rules": false},
                {"category_id": news, "enforce_rules": false,
                 "exact": {"at_s": soft_s, "mode": "soft", "before_s": 0, "after_s": 0}},
                {"category_id": songs, "enforce_rules": false},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    assert_eq!(clock["slots"][1]["exact"]["mode"], "hard");
    assert_eq!(clock["slots"][3]["exact"]["before_s"], 0);
    let clock_id = clock["id"].as_u64().unwrap();
    let week: Vec<Value> = (1..=7)
        .flat_map(|day| {
            (0..24).map(move |hour| json!({"day_of_week": day, "hour": hour, "clock_id": clock_id}))
        })
        .collect();
    let (status, _) = admin.send(Method::PUT, "/schedule", json!(week)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin
        .send(
            Method::PUT,
            "/settings/queue",
            json!({"autofill": true, "min_queued": 3}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Autofill played the whole library before the clock: those plays,
    // the news among them, do not count.
    let cleared_at = rotation_now();

    // Hard: queued with its start time, started then, the track before cut.
    let hard_at = hour_start + hard_s * 1_000;
    let queued = |at: i64| {
        let admin = &admin;
        async move {
            let queue = admin.get("/queue").await;
            queue
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["track"]["start_at_unix_ms"] == at)
                .cloned()
        }
    };
    let entry = eventually(
        "the hard exact slot queued",
        Duration::from_secs(30),
        || queued(hard_at),
    )
    .await;
    assert_eq!(entry["track"]["track_id"], t[4]);
    let play = eventually("the news on air", Duration::from_secs(40), || async {
        let history = admin.get("/history").await;
        history
            .as_array()
            .unwrap()
            .iter()
            .find(|play| {
                play["track_id"] == t[4] && play["started_at_unix_ms"].as_i64() > Some(cleared_at)
            })
            .cloned()
    })
    .await;
    let started = play["started_at_unix_ms"].as_i64().unwrap();
    if (started - hard_at).abs() >= 1_500 {
        let picks = admin.get("/rotation/picks?limit=30").await;
        let picks: Vec<Value> = picks
            .as_array()
            .unwrap()
            .iter()
            .map(|pick| {
                let e = &pick["explanation"];
                json!({"track": pick["track_id"], "planned": pick["planned_at_unix_ms"].as_i64().map(|at| at - hard_at),
                       "slot": e["slot"], "step": e["step"], "exact": e["exact"], "fill": e["fill"]})
            })
            .collect();
        let history = admin.get("/history").await;
        let history: Vec<Value> = history
            .as_array()
            .unwrap()
            .iter()
            .map(|play| json!({"track": play["track_id"], "start": play["started_at_unix_ms"].as_i64().map(|at| at - hard_at), "end": play["end_reason"]}))
            .collect();
        panic!(
            "started {} ms from its exact time\npicks: {}\nhistory: {}",
            started - hard_at,
            serde_json::to_string_pretty(&picks).unwrap(),
            serde_json::to_string(&history).unwrap()
        );
    }
    // The track before fades out over 1.5 s, skipped.
    eventually("the track before cut", Duration::from_secs(10), || async {
        let history = admin.get("/history").await;
        let before = history
            .as_array()
            .unwrap()
            .iter()
            .find(|other| other["started_at_unix_ms"].as_i64() < Some(started))
            .cloned()?;
        (before["end_reason"] == "skipped").then_some(())
    })
    .await;
    let picks = admin.get("/rotation/picks?limit=100").await;
    let exact = picks
        .as_array()
        .unwrap()
        .iter()
        .find(|pick| pick["explanation"]["exact"]["at_unix_ms"] == hard_at)
        .map(|pick| pick["explanation"]["exact"].clone())
        .unwrap();
    assert_eq!(exact["met"], "on_time", "{exact}");
    assert_eq!(exact["faded"], true, "{exact}");

    // Soft: a track queued by hand right before it, which would end after
    // the window, is taken out.
    let soft_at = hour_start + soft_s * 1_000;
    let entry = eventually(
        "the soft exact slot queued",
        Duration::from_secs(40),
        || queued(soft_at),
    )
    .await;
    let queue = admin.get("/queue").await;
    let position = queue
        .as_array()
        .unwrap()
        .iter()
        .position(|other| other["entry_id"] == entry["entry_id"])
        .unwrap();
    let (status, body) = admin
        .send(
            Method::POST,
            "/queue",
            json!({"track_ids": [t[0]], "position": position}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let manual = body["entry_ids"][0].as_u64().unwrap();
    eventually(
        "the manual track taken out",
        Duration::from_secs(20),
        || async {
            let queue = admin.get("/queue").await;
            (!queue
                .as_array()
                .unwrap()
                .iter()
                .any(|other| other["entry_id"] == manual))
            .then_some(())
        },
    )
    .await;
    station.stop().await;
}
