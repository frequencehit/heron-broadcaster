// SPDX-License-Identifier: AGPL-3.0-or-later

//! Test helpers: generated audio files, library snapshots, and a mock of
//! AcoustID, MusicBrainz and the Cover Art Archive.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::Router;
use axum::extract::{Form, Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use heron_core::{FileStamp, LibraryPath};
use heron_library::Tags;
use serde_json::{Value, json};
use sqlx::MySqlPool;

/// A short mono WAV whose notes depend on `seed`, so every seed gets its
/// own Chromaprint fingerprint.
pub fn write_wav(path: &Path, seed: u64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 11_025,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    let mut state = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    let mut notes = Vec::new();
    for _ in 0..16 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        notes.push(220.0 * 2_f64.powf(((state >> 33) % 36) as f64 / 12.0));
    }
    for n in 0..(8 * 11_025) {
        let t = f64::from(n) / 11_025.0;
        let freq = notes[(t * 2.0) as usize % notes.len()];
        let sample = 0.3 * (std::f64::consts::TAU * freq * t).sin();
        writer.write_sample((sample * 32_767.0) as i16).unwrap();
    }
    writer.finalize().unwrap();
}

pub fn write_tagged(path: &Path, seed: u64, tags: &Tags) {
    write_wav(path, seed);
    heron_library::tags::write(path, tags, heron_library::tags::Cover::Keep).unwrap();
}

/// Every file under `dir` (relative path → BLAKE3 of the content).
pub fn snapshot(dir: &Path) -> BTreeMap<String, [u8; 32]> {
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if entry.file_type().is_file() {
            let path = LibraryPath::from_file(dir, entry.path()).unwrap();
            let hash = *blake3::hash(&std::fs::read(entry.path()).unwrap()).as_bytes();
            files.insert(path.to_string(), hash);
        }
    }
    files
}

/// Records a library file as a track, like the scanner does.
pub async fn insert_track(pool: &MySqlPool, library: &Path, path: &str) -> u64 {
    let absolute = LibraryPath::from_stored(path.to_owned()).resolve(library);
    let stamp = FileStamp::from_metadata(&std::fs::metadata(&absolute).unwrap());
    let hash = heron_library::content_hash(&absolute).unwrap();
    sqlx::query(
        "INSERT INTO tracks (path, file_hash, file_size, file_mtime_us, title) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(path)
    .bind(&hash[..])
    .bind(stamp.size)
    .bind(stamp.mtime_us)
    .bind(path)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_id()
}

/// Every track's path exists and its recorded stamp matches the file.
pub async fn assert_tracks_match_files(pool: &MySqlPool, library: &Path) {
    let rows: Vec<(String, Option<u64>, Option<i64>)> = sqlx::query_as(
        "SELECT path, file_size, file_mtime_us FROM tracks WHERE missing_since IS NULL",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    for (path, size, mtime_us) in rows {
        let absolute = LibraryPath::from_stored(path.clone()).resolve(library);
        let metadata = std::fs::metadata(&absolute)
            .unwrap_or_else(|error| panic!("track {path} points at a missing file: {error}"));
        let stamp = FileStamp::from_metadata(&metadata);
        assert_eq!(
            (Some(stamp.size), Some(stamp.mtime_us)),
            (size, mtime_us),
            "stale stamp for {path}"
        );
    }
    let missing: i64 =
        // Tracks in the trash are marked missing on purpose.
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM tracks WHERE missing_since IS NOT NULL AND deleted_at IS NULL",
        )
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(missing, 0, "no track is left missing");
}

// Mock web services ---------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    AcoustId,
    MusicBrainz,
    CoverArt,
}

#[derive(Default)]
pub struct MockState {
    /// Encoded fingerprint → AcoustID `results`.
    pub acoustid: HashMap<String, Value>,
    /// Release MBID → MusicBrainz release.
    pub releases: HashMap<String, Value>,
    /// Release MBIDs with a front cover.
    pub covers: HashMap<String, Vec<u8>>,
    /// Artist MBID → MusicBrainz artist with its relationships.
    pub artists: HashMap<String, Value>,
    /// Release searches: a word of the query → MusicBrainz `releases`.
    pub searches: HashMap<String, Value>,
    pub requests: Vec<(Service, Instant)>,
    /// User-Agent of every request.
    pub user_agents: Vec<String>,
}

pub struct Mock {
    pub url: String,
    pub state: Arc<Mutex<MockState>>,
}

impl Mock {
    pub async fn start(state: MockState) -> Self {
        let state = Arc::new(Mutex::new(state));
        let router = Router::new()
            .route("/v2/lookup", post(acoustid))
            .route("/ws/2/release", get(search))
            .route("/ws/2/release/{id}", get(release))
            .route("/ws/2/artist/{id}", get(artist))
            .route("/caa/release/{id}/front-500", get(cover))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        Self { url, state }
    }

    pub fn endpoints(&self) -> heron_librarian::identify::Endpoints {
        heron_librarian::identify::Endpoints {
            acoustid: format!("{}/v2/lookup", self.url),
            musicbrainz: format!("{}/ws/2", self.url),
            cover_art: format!("{}/caa", self.url),
        }
    }

    pub fn requests(&self, service: Service) -> Vec<Instant> {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(s, _)| *s == service)
            .map(|(_, at)| *at)
            .collect()
    }
}

type Shared = State<Arc<Mutex<MockState>>>;

fn log(state: &Shared, service: Service, headers: &axum::http::HeaderMap) {
    let mut state = state.lock().unwrap();
    state.requests.push((service, Instant::now()));
    let agent = headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    state.user_agents.push(agent);
}

async fn acoustid(
    state: Shared,
    headers: axum::http::HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    log(&state, Service::AcoustId, &headers);
    if form.get("client").map(String::as_str) != Some("test-key") {
        return axum::Json(
            json!({"status": "error", "error": {"code": 4, "message": "invalid API key"}}),
        )
        .into_response();
    }
    let results = form
        .get("fingerprint")
        .and_then(|fp| state.lock().unwrap().acoustid.get(fp).cloned())
        .unwrap_or_else(|| json!([]));
    axum::Json(json!({"status": "ok", "results": results})).into_response()
}

async fn release(
    state: Shared,
    headers: axum::http::HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Response {
    log(&state, Service::MusicBrainz, &headers);
    match state.lock().unwrap().releases.get(&id) {
        Some(release) => axum::Json(release.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn search(
    state: Shared,
    headers: axum::http::HeaderMap,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    log(&state, Service::MusicBrainz, &headers);
    let asked = query.get("query").cloned().unwrap_or_default();
    let releases = state
        .lock()
        .unwrap()
        .searches
        .iter()
        .find(|(word, _)| asked.contains(word.as_str()))
        .map(|(_, releases)| releases.clone())
        .unwrap_or_else(|| json!([]));
    axum::Json(json!({"count": 1, "offset": 0, "releases": releases})).into_response()
}

async fn artist(
    state: Shared,
    headers: axum::http::HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Response {
    log(&state, Service::MusicBrainz, &headers);
    match state.lock().unwrap().artists.get(&id) {
        Some(artist) => axum::Json(artist.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn cover(
    state: Shared,
    headers: axum::http::HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Response {
    log(&state, Service::CoverArt, &headers);
    match state.lock().unwrap().covers.get(&id) {
        Some(jpeg) => jpeg.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// Releases described for the mock -------------------------------------------

pub const VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

pub struct Album {
    pub release_id: String,
    pub group_id: String,
    pub title: String,
    pub artist: String,
    pub artist_id: String,
    pub year: u32,
    /// Recording ID and title of each track (artist: the album artist,
    /// or `Artist n` on a compilation).
    pub tracks: Vec<(String, String)>,
    pub various: bool,
}

impl Album {
    pub fn new(index: u32, tracks: u32, various: bool) -> Self {
        let id = |kind: u32| format!("{kind:08x}-0000-4000-8000-{index:012x}");
        Self {
            release_id: id(1),
            group_id: id(2),
            title: format!("Album {index}"),
            artist: if various {
                "Various Artists".into()
            } else {
                format!("Band {index}")
            },
            artist_id: if various {
                VARIOUS_ARTISTS_ID.into()
            } else {
                id(3)
            },
            year: 1990 + index,
            tracks: (1..=tracks)
                .map(|n| {
                    (
                        format!("{:08x}-0000-4000-8000-{index:06x}{n:06x}", 4),
                        format!("Song {n} of {index}"),
                    )
                })
                .collect(),
            various,
        }
    }

    pub fn track_artist(&self, track: usize) -> String {
        if self.various {
            format!("Artist {track}")
        } else {
            self.artist.clone()
        }
    }

    /// The AcoustID `releasegroups` entry placing track `index` (0-based).
    pub fn acoustid_group(&self, index: usize) -> Value {
        json!({
            "id": self.group_id,
            "title": self.title,
            "type": "Album",
            "releases": [{
                "id": self.release_id,
                "title": self.title,
                "country": "GB",
                "date": {"year": self.year},
                "mediums": [{"position": 1, "track_count": self.tracks.len(), "tracks": [{"position": index + 1}]}],
            }],
        })
    }

    pub fn musicbrainz(&self) -> Value {
        let credit =
            |name: &str, id: &str| json!([{"name": name, "joinphrase": "", "artist": {"id": id}}]);
        json!({
            "id": self.release_id,
            "title": self.title,
            "date": format!("{}-03-01", self.year + 20),
            "artist-credit": credit(&self.artist, &self.artist_id),
            "release-group": {"id": self.group_id, "first-release-date": self.year.to_string()},
            "media": [{
                "position": 1,
                "track-count": self.tracks.len(),
                "tracks": self.tracks.iter().enumerate().map(|(n, (recording, title))| json!({
                    "id": format!("{recording}-t"),
                    "position": n + 1,
                    "title": title,
                    "artist-credit": credit(&self.track_artist(n + 1), &format!("artist-{n}")),
                    "recording": {"id": recording},
                })).collect::<Vec<_>>(),
            }],
        })
    }
}

/// An AcoustID result for a recording on the given release groups.
pub fn acoustid_result(score: f64, recording: &str, groups: Vec<Value>) -> Value {
    json!([{"id": "acoustid-id", "score": score, "recordings": [{"id": recording, "releasegroups": groups}]}])
}

pub fn fingerprint(path: &Path) -> String {
    heron_analysis::fingerprint_file(path)
        .unwrap()
        .unwrap()
        .encoded()
}

pub fn temp_roots() -> (tempfile::TempDir, heron_librarian::journal::Roots) {
    let dir = tempfile::tempdir().unwrap();
    let roots = heron_librarian::journal::Roots {
        library: dir.path().join("library"),
        incoming: dir.path().join("incoming"),
        staging: dir.path().join("staging"),
    };
    std::fs::create_dir_all(&roots.library).unwrap();
    std::fs::create_dir_all(&roots.incoming).unwrap();
    std::fs::create_dir_all(&roots.staging).unwrap();
    (dir, roots)
}

pub fn path_buf(root: &Path, relative: &str) -> PathBuf {
    LibraryPath::from_stored(relative.to_owned()).resolve(root)
}

/// Moves the incoming files into the buffer zone, identifies them, then
/// imports every file that is ready, as the web UI's "import" would.
pub async fn stage_and_import(
    pool: &MySqlPool,
    roots: &heron_librarian::journal::Roots,
    endpoints: heron_librarian::identify::Endpoints,
    settle: std::time::Duration,
) -> (
    heron_librarian::import::ImportReport,
    heron_core::library::BatchReport,
) {
    let staged = stage(pool, roots, endpoints.clone(), settle).await;
    let report = import_ready(pool, roots, endpoints).await;
    (staged, report)
}

/// Moves the incoming files into the buffer zone and identifies them.
pub async fn stage(
    pool: &MySqlPool,
    roots: &heron_librarian::journal::Roots,
    endpoints: heron_librarian::identify::Endpoints,
    settle: std::time::Duration,
) -> heron_librarian::import::ImportReport {
    let importer = heron_librarian::import::Importer::new(pool.clone(), roots.clone(), settle);
    let staged = importer.import_once().await.unwrap();
    let mut stager = heron_librarian::staging::Stager::new(pool.clone(), roots.clone(), endpoints);
    stager.identify_once().await.unwrap();
    staged
}

/// Imports every file of the buffer zone that is ready.
pub async fn import_ready(
    pool: &MySqlPool,
    roots: &heron_librarian::journal::Roots,
    endpoints: heron_librarian::identify::Endpoints,
) -> heron_core::library::BatchReport {
    let ready: Vec<u64> =
        sqlx::query_scalar("SELECT id FROM staged_files WHERE status = 'ready' ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
    let settings = heron_librarian::import::load_settings(pool).await.unwrap();
    heron_librarian::staging::import(pool, roots, &endpoints, &settings, &ready)
        .await
        .unwrap()
}
