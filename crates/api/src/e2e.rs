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
use crate::{engine, history, queue, sync, users, webhooks};

/// Tracks short enough to see several play during a test.
const TRACK_SECONDS: f64 = 3.0;

struct Station {
    db: ScratchDatabase,
    library: TempDir,
    _state_dir: TempDir,
    staging: TempDir,
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

        let state_dir = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        if let Some(config) = engine_config {
            config.save(state_dir.path()).unwrap();
        }
        let engine = Engine::start(
            EngineOptions::new(library.path().to_path_buf(), state_dir.path().to_path_buf()),
            tokio::runtime::Handle::current(),
        )
        .unwrap();
        let grpc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let grpc_addr = grpc.local_addr().unwrap();
        let service = EngineService::server(Arc::clone(&engine));
        let mut tasks = vec![tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(TcpListenerStream::new(grpc))
                .await;
        })];

        let link = engine::EngineLink::new(&format!("http://{grpc_addr}")).unwrap();
        let queue = queue::QueueManager::new(pool.clone(), Arc::clone(&link));
        let sync = sync::ConfigSync::new(pool.clone(), Arc::clone(&link));
        let history_events = link.events();
        let webhook_events = link.events();
        let webhooks = webhooks::Webhooks::with_backoff(
            pool.clone(),
            webhooks::Backoff(vec![Duration::from_millis(200); 2]),
        )
        .unwrap();
        tasks.extend(link.spawn());
        tasks.push(tokio::spawn(
            history::HistoryRecorder::new(pool.clone()).run(history_events),
        ));
        tasks.push(tokio::spawn(Arc::clone(&webhooks).dispatch(webhook_events)));
        tasks.push(tokio::spawn(Arc::clone(&webhooks).send_loop()));
        tasks.push(tokio::spawn(Arc::clone(&queue).run()));
        tasks.push(tokio::spawn(Arc::clone(&sync).run()));

        let state = AppState {
            pool: pool.clone(),
            health: DatabaseHealth(pool.clone()),
            library: Arc::new(Library::new(pool.clone(), library.path().to_path_buf())),
            engine: link,
            queue,
            sync,
            passwords: Arc::new(Passwords::default()),
            staging: Arc::new(staging.path().to_path_buf()),
            webhooks,
            // Driven by the tests, minute by minute.
            listeners: crate::listeners::Collector::new(pool.clone()).unwrap(),
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
            let config = station.state.engine.config().await.ok()?;
            config
                .playout_settings_json
                .contains("\"min_track_ms\":30000")
                .then_some(())
        },
    )
    .await;

    // An output: validated, stored without showing its password, pushed.
    let profiles = admin.get("/encoder-profiles").await;
    let profile = profiles[0]["id"].as_u64().unwrap();
    let (status, body) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Bad", "kind": "icecast", "encoder_profile_id": profile,
                   "config": {"host": "127.0.0.1", "mount": "live", "password": "x"}}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, output) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile,
                   "config": {"host": "127.0.0.1", "port": 9, "mount": "/live", "password": "hackme"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{output}");
    assert!(output["config"].get("password").is_none());
    assert_eq!(output["secrets_set"], json!(["password"]));
    assert_eq!(output["listen_url"], "http://127.0.0.1:9/live");
    let id = output["id"].as_u64().unwrap();
    eventually(
        "the output in the engine",
        Duration::from_secs(10),
        || async {
            let config = station.state.engine.config().await.ok()?;
            config.outputs_json.contains("hackme").then_some(())
        },
    )
    .await;

    // Updating without the password keeps it.
    let (status, _) = admin
        .send(
            Method::PUT,
            &format!("/outputs/{id}"),
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile,
                   "config": {"host": "127.0.0.1", "port": 9, "mount": "/other",
                              "public_url": "https://stream.example.org/"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    eventually(
        "the changed output in the engine",
        Duration::from_secs(10),
        || async {
            let config = station.state.engine.config().await.ok()?;
            (config.outputs_json.contains("/other") && config.outputs_json.contains("hackme"))
                .then_some(())
        },
    )
    .await;
    let outputs = eventually("the output's state", Duration::from_secs(10), || async {
        let outputs = admin.get("/outputs").await;
        outputs[0]["status"].is_object().then_some(outputs)
    })
    .await;
    assert_eq!(outputs[0]["listen_url"], "https://stream.example.org/other");
    assert_ne!(outputs[0]["status"]["state"], "connected");

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
            let config = station.state.engine.config().await.ok()?;
            (!config.outputs_json.contains("hackme")).then_some(())
        },
    )
    .await;

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
    let config = json!({"host": "127.0.0.1", "port": 9, "mount": "/live", "password": "hackme"});
    let (status, output) = admin
        .send(
            Method::POST,
            "/outputs",
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile,
                   "config": config, "statistics": {"admin_password": "adm1n-pw"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{output}");
    assert_eq!(
        output["statistics"],
        json!({"admin_username": null, "admin_password_set": true})
    );
    assert!(!output.to_string().contains("adm1n-pw"));
    let id = output["id"].as_u64().unwrap();
    let pushed = eventually(
        "the output in the engine",
        Duration::from_secs(10),
        || async {
            let config = station.state.engine.config().await.ok()?;
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
            &format!("/outputs/{id}"),
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile,
                   "config": {"host": "127.0.0.1", "port": 9, "mount": "/live"},
                   "statistics": {"admin_username": "ops"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(
        changed["statistics"],
        json!({"admin_username": "ops", "admin_password_set": true})
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        station.state.engine.config().await.unwrap().outputs_json,
        pushed
    );
    let (_, removed) = admin
        .send(
            Method::PUT,
            &format!("/outputs/{id}"),
            json!({"name": "Icecast", "kind": "icecast", "encoder_profile_id": profile,
                   "config": {"host": "127.0.0.1", "port": 9, "mount": "/live"},
                   "statistics": {"admin_password": null}}),
        )
        .await;
    assert_eq!(removed["statistics"]["admin_password_set"], false);

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
            "INSERT INTO listener_totals (output_id, day, hour, minutes, peak, average,
                 listening_hours, sessions, session_s, distinct_listeners)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        json!([{"hour": 8, "days": 1, "average": 9.0, "peak": 12, "listening_hours": 9.0}])
    );

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
        json!({"outputs": [], "total": null})
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
    ] {
        let (status, _) = viewer.request(Method::GET, bad, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    for path in [
        format!("/listeners/samples?from_unix_ms=0&to_unix_ms=60000&output_id={id}"),
        format!("/listeners/players?from=2026-10-19&to=2026-10-25&output_id={id}&limit=5"),
        "/listeners/tracks?from=2026-10-19&to=2026-10-25&min_plays=1&limit=5".to_owned(),
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
        "hour,days,average,peak,listening_hours\n8,1,9.00,12,9.00\n"
    );
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
    let playout = admin.get("/settings/playout").await;
    assert_eq!(playout["crossfade"]["min_track_ms"], 25000);

    // Same IDs: the engine kept its configuration as it was.
    let config = station.state.engine.config().await.unwrap();
    let outputs: heron_core::OutputsConfig = serde_json::from_str(&config.outputs_json).unwrap();
    assert_eq!(outputs.outputs, engine_config.outputs.outputs);
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

    // And the width: a centred column unless the user wants the whole screen.
    assert_eq!(me["full_width"], false);
    let (status, me) = admin
        .send(
            Method::PUT,
            "/auth/preferences",
            json!({"full_width": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(
        (&me["full_width"], &me["language"], &me["theme"]),
        (&json!(true), &json!("fr"), &json!("dark"))
    );
    assert_eq!(viewer.get("/auth/me").await["full_width"], false);
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

    // Two categories, a clock calling them in turn, all week.
    let mut categories = Vec::new();
    for (name, members) in [("A", &t[..3]), ("B", &t[3..])] {
        let (status, category) = admin
            .send(
                Method::POST,
                "/categories",
                json!({"name": name, "separation": {"track_min": 1}}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{category}");
        assert_eq!(category["separation"]["track_min"], 1);
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
                 "separation": {"artist_min": 0}},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    let clock_id = clock["id"].as_u64().unwrap();
    assert_eq!(clock["slots"][1]["separation"]["artist_min"], 0);
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
            "INSERT INTO play_history (track_id, title, started_at, ended_at, played_ms, deck, source)
             VALUES (?, 'x', NOW(6) - INTERVAL ? SECOND,
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
    heron_library::details::save(&mut connection, track, &read)
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
    let mut events = station.state.engine.events();
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

    // The delivery log, a test ping and a retry.
    let log = admin.get(&format!("/webhooks/{webhook}/deliveries")).await;
    let log = log.as_array().unwrap();
    assert!(log.len() >= expected.len());
    assert!(
        log.iter()
            .all(|delivery| delivery["status"] == "delivered" && delivery["attempts"] == 1)
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
    let delivered = log[0]["id"].as_u64().unwrap();
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
        (&jingle_type["name"], &jingle_type["sends_metadata"]),
        (&json!("Jingle"), &json!(false))
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
            (deck["on_air"] == true && track["no_metadata"] == true).then(|| track["title"].clone())
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
