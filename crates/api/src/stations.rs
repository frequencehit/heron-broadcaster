// SPDX-License-Identifier: AGPL-3.0-or-later

//! Several stations in one Heron (`docs/stations.md`).
//!
//! Each station has its own engine, reached over gRPC, and its own queue
//! manager, configuration sync and history recorder: a [`StationRuntime`].
//! [`Stations`] keeps one per station and restarts it when an admin
//! changes the station.
//!
//! What is per station is served under `/api/v1/stations/{id}/…`. The
//! routes of a single station (`/api/v1/queue`…) keep answering for the
//! first station: [`rewrite`] turns the first form into the second, with
//! the station in a request extension that the [`Station`] extractor reads.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use axum::Json;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{Request, StatusCode, Uri};
use heron_core::EqSettings;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::engine::EngineLink;
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate};
use crate::history::HistoryRecorder;
use crate::queue::QueueManager;
use crate::sync::ConfigSync;
use crate::webhooks::Webhooks;

/// The name the first station gets when it has none (an upgraded Heron).
const DEFAULT_NAME: &str = "Heron";
const MAX_CODE_LENGTH: usize = 32;

/// The first path segments, under `/api/v1`, of what is per station.
/// `settings` is per station only for the groups in [`STATION_SETTINGS`].
const PER_STATION: [&str; 14] = [
    "engine",
    "live",
    "playout",
    "player",
    "queue",
    "history",
    "clocks",
    "schedule",
    "schedules",
    "rotation",
    "outputs",
    "webhooks",
    "webhook-deliveries",
    "listeners",
];
/// The settings groups served per station (`/settings/{group}`).
const STATION_SETTINGS: [&str; 6] = ["playout", "eq", "queue", "rotation", "station", "medleys"];
/// The public routes per station (`/public/{route}`).
const PUBLIC: [&str; 3] = ["now-playing", "queue", "history"];

/// A station as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema, sqlx::FromRow)]
pub struct StationInfo {
    pub id: u64,
    pub name: String,
    /// Short and unique: lowercase letters and digits. Set once for the
    /// stations after the first: it names their engine's variable.
    pub code: String,
    /// A disabled station is not fed: its engine plays what it has
    /// queued, then its fallback directory.
    pub enabled: bool,
    pub position: u32,
    /// The variable of `heron-api`'s environment that holds its engine's
    /// gRPC address: `HERON_ENGINE_URL` for the first station,
    /// `HERON_ENGINE_URL_<CODE>` for the others.
    #[sqlx(skip)]
    pub engine_variable: String,
    /// Whether that variable is set: a station without it is off the air.
    #[sqlx(skip)]
    pub engine_configured: bool,
}

const COLUMNS: &str = "id, name, code, enabled, position";

/// The first station's engine variable.
const FIRST_ENGINE_VARIABLE: &str = "HERON_ENGINE_URL";

/// The engine variable of a station after the first.
pub fn engine_variable(code: &str) -> String {
    format!("{FIRST_ENGINE_VARIABLE}_{}", code.to_ascii_uppercase())
}

/// The engines of the stations after the first, from the environment:
/// `HERON_ENGINE_URL_<CODE>` to its address.
pub fn engines_from_env() -> BTreeMap<String, String> {
    let prefix = format!("{FIRST_ENGINE_VARIABLE}_");
    std::env::vars()
        .filter(|(name, url)| name.starts_with(&prefix) && !url.trim().is_empty())
        .map(|(name, url)| (name, url.trim().to_owned()))
        .collect()
}

/// What runs for one station.
pub struct StationRuntime {
    id: u64,
    info: RwLock<StationInfo>,
    pub engine: Arc<EngineLink>,
    pub queue: Arc<QueueManager>,
    pub sync: Arc<ConfigSync>,
    /// Deck EQ settings as they change, for the live WebSockets.
    pub eq_changes: broadcast::Sender<EqSettings>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl StationRuntime {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn info(&self) -> StationInfo {
        self.info
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn code(&self) -> String {
        self.info
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .code
            .clone()
    }

    /// Stops its tasks: the station was deleted, enabled or disabled.
    fn stop(&self) {
        for task in self
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            task.abort();
        }
    }
}

/// Every station's runtime.
pub struct Stations {
    /// For the queue managers, which look at the other stations' queues.
    me: std::sync::Weak<Stations>,
    pool: MySqlPool,
    /// The first station's engine (`HERON_ENGINE_URL`).
    default_engine_url: String,
    /// The other stations' engines, by variable (`HERON_ENGINE_URL_<CODE>`).
    engines: RwLock<BTreeMap<String, String>>,
    /// The first station: the lowest ID, which never changes.
    first_id: u64,
    webhooks: Arc<Webhooks>,
    running: RwLock<BTreeMap<u64, Arc<StationRuntime>>>,
}

impl Stations {
    /// Names the first station if it has no name yet, and starts every
    /// station.
    pub async fn start(
        pool: MySqlPool,
        default_engine_url: String,
        engines: BTreeMap<String, String>,
        default_name: Option<String>,
        webhooks: Arc<Webhooks>,
    ) -> ApiResult<Arc<Self>> {
        let name = default_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| DEFAULT_NAME.to_owned());
        sqlx::query("UPDATE stations SET name = ? WHERE name = ''")
            .bind(name)
            .execute(&pool)
            .await?;
        let all = list_by_id(&pool).await?;
        let first_id = all.first().map_or(1, |station| station.id);
        let stations = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            pool,
            default_engine_url,
            engines: RwLock::new(engines),
            first_id,
            webhooks,
            running: RwLock::new(BTreeMap::new()),
        });
        // A station whose engine is not set or invalid does not stop the
        // others; the first station's (HERON_ENGINE_URL) does.
        for info in all {
            let id = info.id;
            match stations.runtime(info) {
                Ok(Some(runtime)) => {
                    stations
                        .running
                        .write()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(id, runtime);
                }
                Ok(None) => {}
                Err(error) if id == first_id => return Err(error),
                Err(error) => tracing::error!(%error, "cannot start the station"),
            }
        }
        Ok(stations)
    }

    /// Sets the engine of the station with that code, as its variable would
    /// when `heron-api` starts: for tests.
    #[cfg(test)]
    pub fn set_engine(&self, code: &str, url: &str) {
        self.engines
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(engine_variable(code), url.to_owned());
    }

    /// `info` with its engine variable, and whether it is set.
    pub fn describe(&self, mut info: StationInfo) -> StationInfo {
        if info.id == self.first_id {
            info.engine_variable = FIRST_ENGINE_VARIABLE.to_owned();
            info.engine_configured = true;
        } else {
            info.engine_variable = engine_variable(&info.code);
            info.engine_configured = self
                .engines
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&info.engine_variable);
        }
        info
    }

    /// What runs for a station; none while its engine variable is not set.
    fn runtime(&self, info: StationInfo) -> ApiResult<Option<Arc<StationRuntime>>> {
        let info = self.describe(info);
        let url = if info.id == self.first_id {
            self.default_engine_url.clone()
        } else {
            let engines = self.engines.read().unwrap_or_else(PoisonError::into_inner);
            let Some(url) = engines.get(&info.engine_variable) else {
                tracing::warn!(
                    station = %info.code,
                    variable = %info.engine_variable,
                    "the station is off the air: its engine variable is not set"
                );
                return Ok(None);
            };
            url.clone()
        };
        let engine = EngineLink::new(&url).map_err(|error| {
            ApiError::Invalid(format!(
                "{}: invalid engine address {url}: {error}",
                info.engine_variable
            ))
        })?;
        let queue = QueueManager::new(
            self.pool.clone(),
            Arc::clone(&engine),
            info.id,
            self.me.clone(),
        );
        let sync = ConfigSync::new(self.pool.clone(), Arc::clone(&engine), info.id);
        let mut tasks = engine.spawn();
        tasks.push(tokio::spawn(
            HistoryRecorder::new(self.pool.clone(), info.id).run(engine.events()),
        ));
        tasks.push(tokio::spawn(
            Arc::clone(&self.webhooks).dispatch(info.id, engine.events()),
        ));
        tasks.push(tokio::spawn(
            Arc::clone(&self.webhooks).watch_queue(info.id, engine.state()),
        ));
        tasks.push(tokio::spawn(Arc::clone(&sync).run()));
        if info.enabled {
            tasks.push(tokio::spawn(Arc::clone(&queue).run()));
        }
        tracing::info!(station = %info.code, engine = %url, enabled = info.enabled, "station started");
        Ok(Some(Arc::new(StationRuntime {
            id: info.id,
            info: RwLock::new(info),
            engine,
            queue,
            sync,
            eq_changes: broadcast::channel(16).0,
            tasks: Mutex::new(tasks),
        })))
    }

    /// Restarts a station after it was enabled or disabled, starts a new
    /// one, or stops a deleted one. A station whose engine cannot start is
    /// left off the air, with the reason logged.
    pub async fn reload(&self, id: u64) -> ApiResult<()> {
        let info: Option<StationInfo> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM stations WHERE id = ?"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let mut running = self.running.write().unwrap_or_else(PoisonError::into_inner);
        let Some(info) = info else {
            if let Some(old) = running.remove(&id) {
                old.stop();
            }
            return Ok(());
        };
        if let Some(current) = running.get(&id) {
            // A rename, the first station's new code or a position keeps
            // what runs.
            if current.info().enabled == info.enabled {
                *current.info.write().unwrap_or_else(PoisonError::into_inner) = self.describe(info);
                return Ok(());
            }
        }
        let code = info.code.clone();
        let runtime = match self.runtime(info) {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(station = %code, %error, "cannot start the station");
                None
            }
        };
        let old = match runtime {
            Some(runtime) => running.insert(id, runtime),
            None => running.remove(&id),
        };
        if let Some(old) = old {
            old.stop();
        }
        Ok(())
    }

    pub fn get(&self, id: u64) -> Option<Arc<StationRuntime>> {
        self.running
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
    }

    pub fn by_code(&self, code: &str) -> Option<Arc<StationRuntime>> {
        self.all()
            .into_iter()
            .find(|station| station.code() == code)
    }

    /// The first station: the one the single-station routes serve.
    // It always runs: `start` fails without it, and it cannot be deleted.
    #[allow(clippy::expect_used)]
    pub fn first(&self) -> Arc<StationRuntime> {
        self.running
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .next()
            .cloned()
            .expect("there is always a station")
    }

    /// Every station, by ID.
    pub fn all(&self) -> Vec<Arc<StationRuntime>> {
        self.running
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Checks every station's queue now (the library or rotation changed).
    pub fn wake_all(&self) {
        for station in self.all() {
            station.queue.wake();
        }
    }
}

async fn list_by_id(pool: &MySqlPool) -> ApiResult<Vec<StationInfo>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM stations ORDER BY id"
    )))
    .fetch_all(pool)
    .await?)
}

async fn list(pool: &MySqlPool) -> ApiResult<Vec<StationInfo>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM stations ORDER BY position, id"
    )))
    .fetch_all(pool)
    .await?)
}

/// The station a request is for, set by [`rewrite`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct StationId(pub(crate) u64);

/// The station a request is for: the one in `/stations/{id}/…`, or the
/// first station on the single-station routes and the shared ones.
#[derive(Clone)]
pub struct Station(pub Arc<StationRuntime>);

impl Deref for Station {
    type Target = StationRuntime;

    fn deref(&self) -> &StationRuntime {
        &self.0
    }
}

impl FromRequestParts<AppState> for Station {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        match parts.extensions.get::<StationId>() {
            Some(StationId(id)) => state
                .stations
                .get(*id)
                .map(Self)
                .ok_or_else(|| ApiError::NotFound(format!("station {id} not found"))),
            None => Ok(Self(state.stations.first())),
        }
    }
}

/// Serves `/api/v1/stations/{id}/<route>` as `/api/v1/<route>` for that
/// station, and `/api/v1/public/stations/{code}/<route>` as
/// `/api/v1/public/<route>`. Other paths are left alone: an unknown
/// station's routes answer 404, as do shared routes under a station.
pub fn rewrite<B>(stations: &Stations, mut request: Request<B>) -> Request<B> {
    let path = request.uri().path();
    let found = if let Some(rest) = path.strip_prefix("/api/v1/stations/") {
        rest.split_once('/').and_then(|(id, rest)| {
            let id: u64 = id.parse().ok()?;
            per_station(rest).then(|| (id, format!("/api/v1/{rest}")))
        })
    } else if let Some(rest) = path.strip_prefix("/api/v1/public/stations/") {
        rest.split_once('/').and_then(|(code, rest)| {
            let station = stations.by_code(code)?;
            PUBLIC
                .contains(&rest)
                .then(|| (station.id(), format!("/api/v1/public/{rest}")))
        })
    } else {
        // A single-station route: the first station's.
        if path.strip_prefix("/api/v1/").is_some_and(per_station) {
            let first = stations.first().id();
            request.extensions_mut().insert(StationId(first));
        }
        None
    };
    let Some((id, path)) = found else {
        return request;
    };
    let path_and_query = match request.uri().query() {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    let mut parts = request.uri().clone().into_parts();
    let Ok(path_and_query) = path_and_query.parse() else {
        return request;
    };
    parts.path_and_query = Some(path_and_query);
    if let Ok(uri) = Uri::from_parts(parts) {
        *request.uri_mut() = uri;
        request.extensions_mut().insert(StationId(id));
    }
    request
}

/// Whether a path under `/api/v1` is one of a station's.
pub fn per_station(rest: &str) -> bool {
    let mut segments = rest.split('/');
    match segments.next() {
        Some("settings") => segments
            .next()
            .is_some_and(|group| STATION_SETTINGS.contains(&group)),
        Some(first) => PER_STATION.contains(&first),
        None => false,
    }
}

/// Whether an OpenAPI path (`/api/v1/…`) is per station.
pub fn per_station_path(path: &str) -> bool {
    path.strip_prefix("/api/v1/").is_some_and(per_station)
}

/// Whether an OpenAPI path is a public route served per station.
pub fn public_path(path: &str) -> Option<&str> {
    path.strip_prefix("/api/v1/public/")
        .filter(|rest| PUBLIC.contains(rest))
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_stations, create_station))
        .routes(routes!(get_station_info, update_station, delete_station))
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StationRequest {
    pub name: String,
    pub code: String,
    /// Set once for the stations after the first: their engine's variable
    /// is `HERON_ENGINE_URL_<CODE>`.
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub position: u32,
}

fn enabled() -> bool {
    true
}

fn check(request: &StationRequest) -> ApiResult<(String, String)> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::Invalid("a station needs a name".to_owned()));
    }
    let code = request.code.trim();
    let valid = !code.is_empty()
        && code.len() <= MAX_CODE_LENGTH
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !valid {
        return Err(ApiError::Invalid(format!(
            "a station code is 1 to {MAX_CODE_LENGTH} lowercase letters and digits"
        )));
    }
    Ok((name.to_owned(), code.to_owned()))
}

fn conflict(error: sqlx::Error, code: &str) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict(format!("a station already has the code {code}"))
    } else {
        error.into()
    }
}

/// Every station.
#[utoipa::path(get, path = "/stations", tag = "stations",
    responses((status = 200, body = Vec<StationInfo>), (status = "4XX", body = ErrorBody)))]
async fn list_stations(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<StationInfo>>> {
    let mut stations: Vec<StationInfo> = list(&state.pool)
        .await?
        .into_iter()
        .map(|station| state.stations.describe(station))
        .collect();
    if user.role != Role::Admin {
        let seen: Vec<u64> = sqlx::query_scalar(
            "SELECT station_id FROM user_station_roles WHERE user_id = ? AND role <> 'none'",
        )
        .bind(user.id)
        .fetch_all(&state.pool)
        .await?;
        stations.retain(|station| seen.contains(&station.id));
    }
    Ok(Json(stations))
}

/// Adds a station (admins). Its engine is a new service of the deployment,
/// whose address is `HERON_ENGINE_URL_<CODE>` in `heron-api`'s environment.
#[utoipa::path(post, path = "/stations", tag = "stations",
    request_body = StationRequest,
    responses((status = 201, body = StationInfo), (status = "4XX", body = ErrorBody)))]
async fn create_station(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<StationRequest>,
) -> ApiResult<(StatusCode, Json<StationInfo>)> {
    user.require(Role::Admin)?;
    let (name, code) = check(&request)?;
    let id =
        sqlx::query("INSERT INTO stations (name, code, enabled, position) VALUES (?, ?, ?, ?)")
            .bind(&name)
            .bind(&code)
            .bind(request.enabled)
            .bind(request.position)
            .execute(&state.pool)
            .await
            .map_err(|error| conflict(error, &code))?
            .last_insert_id();
    crate::schedules::default_id(&state.pool, id).await?;
    state.stations.reload(id).await?;
    tracing::info!(station = %code, by = %user.username, "station added");
    let station = state.stations.describe(fetch(&state.pool, id).await?);
    Ok((StatusCode::CREATED, Json(station)))
}

async fn fetch(pool: &MySqlPool, id: u64) -> ApiResult<StationInfo> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM stations WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("station {id} not found")))
}

#[utoipa::path(get, path = "/stations/{id}", tag = "stations",
    params(("id" = u64, Path)),
    responses((status = 200, body = StationInfo), (status = "4XX", body = ErrorBody)))]
async fn get_station_info(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<StationInfo>> {
    if user.role != Role::Admin
        && crate::auth::station_role(&state.pool, user.id, id)
            .await?
            .is_none()
    {
        return Err(ApiError::NotFound(format!("station {id} not found")));
    }
    Ok(Json(state.stations.describe(fetch(&state.pool, id).await?)))
}

/// Renames a station, changes its position, enables or disables it
/// (admins). Only the first station's code can change: the others' names
/// their engine's variable.
#[utoipa::path(put, path = "/stations/{id}", tag = "stations",
    params(("id" = u64, Path)), request_body = StationRequest,
    responses((status = 200, body = StationInfo), (status = "4XX", body = ErrorBody)))]
async fn update_station(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(request): Json<StationRequest>,
) -> ApiResult<Json<StationInfo>> {
    user.require(Role::Admin)?;
    let (name, code) = check(&request)?;
    let first = state.stations.first().id() == id;
    let before = fetch(&state.pool, id).await?;
    if !first && before.code != code {
        return Err(ApiError::Conflict(format!(
            "the code of {} cannot change: it names its engine's variable, {}",
            before.code,
            engine_variable(&before.code)
        )));
    }
    if !request.enabled && first {
        return Err(ApiError::Invalid(
            "the first station cannot be disabled".to_owned(),
        ));
    }
    let updated = sqlx::query(
        "UPDATE stations SET name = ?, code = ?, enabled = ?, position = ? WHERE id = ?",
    )
    .bind(&name)
    .bind(&code)
    .bind(request.enabled)
    .bind(request.position)
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(|error| conflict(error, &code))?
    .rows_affected();
    if updated > 0 {
        state.stations.reload(id).await?;
    }
    Ok(Json(state.stations.describe(fetch(&state.pool, id).await?)))
}

/// Deletes a station and its history and statistics (admins). Refused
/// while it has clocks, a schedule, outputs or webhooks, and for the first
/// station.
#[utoipa::path(delete, path = "/stations/{id}", tag = "stations",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_station(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    let station = fetch(&state.pool, id).await?;
    if state.stations.first().id() == id {
        return Err(ApiError::Conflict(
            "the first station cannot be deleted".to_owned(),
        ));
    }
    let uses: Vec<(String, i64)> = sqlx::query_as(
        "SELECT 'clocks', COUNT(*) FROM clocks WHERE station_id = ?
         UNION ALL SELECT 'scheduled hours', COUNT(*) FROM clock_schedule cs
             JOIN schedules s ON s.id = cs.schedule_id WHERE s.station_id = ?
         UNION ALL SELECT 'schedules besides the default one', COUNT(*) FROM schedules
             WHERE station_id = ? AND NOT is_default
         UNION ALL SELECT 'outputs', COUNT(*) FROM outputs WHERE station_id = ?
         UNION ALL SELECT 'webhooks', COUNT(*) FROM webhooks WHERE station_id = ?",
    )
    .bind(id)
    .bind(id)
    .bind(id)
    .bind(id)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let used: Vec<String> = uses
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(what, count)| format!("{count} {what}"))
        .collect();
    if !used.is_empty() {
        return Err(ApiError::Conflict(format!(
            "{} still has {}: remove them first",
            station.code,
            used.join(", ")
        )));
    }
    sqlx::query("DELETE FROM stations WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    state.stations.reload(id).await?;
    tracing::info!(station = %station.code, by = %user.username, "station deleted");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_station_routes() {
        assert!(per_station("queue"));
        assert!(per_station("queue/12"));
        assert!(per_station("settings/playout"));
        assert!(per_station("webhook-deliveries/3/retry"));
        assert!(!per_station("settings/library"));
        assert!(per_station("settings/medleys"));
        assert!(!per_station("tracks/1"));
        assert!(!per_station("stations"));
    }

    #[test]
    fn engine_variables() {
        assert_eq!(engine_variable("hits80"), "HERON_ENGINE_URL_HITS80");
        assert_eq!(engine_variable("fh90"), "HERON_ENGINE_URL_FH90");
    }
}
