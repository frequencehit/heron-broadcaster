// SPDX-License-Identifier: AGPL-3.0-or-later

//! Station settings, stored as JSON in the `settings` table, one row per group.
//!
//! - `playout`: crossfades and loudness, pushed to the engine.
//! - `analysis`: Gap Killer and crossfade point detection. Changing them
//!   re-analyzes the library.
//! - `library`: identification and layout, read by `heron-librarian`.
//! - `queue`: the queue manager.
//! - `rotation`: separation rules, fallback category and weights.
//! - `station`: the time zone clocks and restrictions follow.
//! - `programming`: what counts as a play, eras, and the mood and sound
//!   code scales.
//! - `statistics`: listener statistics (the minimum session, retention).

use axum::Json;
use axum::extract::State;
use heron_core::{AnalysisSettings, LibrarySettings, PlayoutSettings};
use heron_library::layout::Layout;
use heron_rotation::RotationSettings;
use jiff::tz::TimeZone;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::stations::Station;

/// The queue manager. Until rotation and clockwheels (M6), it keeps the
/// engine's queue filled with the least recently played tracks.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct QueueSettings {
    /// Add tracks when the queue runs low. Off, the station plays silence
    /// once the queue is empty.
    pub autofill: bool,
    /// Tracks kept waiting (cued and queued), 1 to 50.
    pub min_queued: u32,
}

impl QueueSettings {
    pub const SETTINGS_NAME: &'static str = "queue";
}

impl Default for QueueSettings {
    fn default() -> Self {
        Self {
            autofill: true,
            min_queued: 3,
        }
    }
}

/// The station itself.
// No `serde(default)`: the default time zone is the server's, and the
// OpenAPI document must not depend on the machine that generates it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StationSettings {
    /// IANA time zone the clocks and restrictions follow, e.g.
    /// `Europe/Paris`. By default the server's (`TZ`), or UTC.
    pub timezone: String,
}

impl StationSettings {
    pub const SETTINGS_NAME: &'static str = "station";

    /// The time zone; UTC if the stored one is unknown.
    pub fn time_zone(&self) -> TimeZone {
        TimeZone::get(&self.timezone).unwrap_or_else(|error| {
            tracing::warn!(timezone = %self.timezone, %error, "unknown time zone: using UTC");
            TimeZone::UTC
        })
    }
}

impl Default for StationSettings {
    fn default() -> Self {
        let from_env = std::env::var("TZ")
            .ok()
            .map(|tz| tz.trim_start_matches(':').to_owned())
            .filter(|tz| TimeZone::get(tz).is_ok());
        let system = || {
            TimeZone::try_system()
                .ok()
                .and_then(|tz| tz.iana_name().map(str::to_owned))
        };
        Self {
            timezone: from_env.or_else(system).unwrap_or_else(|| "UTC".to_owned()),
        }
    }
}

/// Listener statistics. See `docs/statistics.md#settings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct StatisticsSettings {
    /// Shortest session that counts as a listener, in seconds (0 counts
    /// every one), at most 3600.
    pub min_session_s: u32,
    /// How long sessions and minute samples are kept, 7 to 3650 days.
    pub retention_days: u32,
}

impl StatisticsSettings {
    pub const SETTINGS_NAME: &str = "statistics";

    fn check(&self) -> Result<(), String> {
        if self.min_session_s > 3_600 {
            return Err("min_session_s is at most 3600".to_owned());
        }
        if !(7..=3_650).contains(&self.retention_days) {
            return Err("retention_days is 7 to 3650".to_owned());
        }
        Ok(())
    }
}

impl Default for StatisticsSettings {
    fn default() -> Self {
        Self {
            min_session_s: 60,
            retention_days: 90,
        }
    }
}

/// What music programmers sort and separate tracks by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ProgrammingSettings {
    /// A play counts (play counts, last played) when it lasted at least
    /// this long, so skipped tracks do not count. 0 counts every start.
    pub min_play_s: u32,
    /// Eras by year of first release, the first matching one applies.
    /// Empty: decades (`70s`, `80s`, `2000s`).
    pub eras: Vec<Era>,
    /// The mood scale, in order (an energy from 1 to 5 by default).
    pub moods: Vec<String>,
    /// The sound code scale, in order.
    pub sound_codes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Era {
    pub name: String,
    /// First year, open-ended when missing.
    pub from_year: Option<u16>,
    /// Last year, open-ended when missing.
    pub to_year: Option<u16>,
}

impl ProgrammingSettings {
    pub const SETTINGS_NAME: &'static str = "programming";

    fn check(&self) -> Result<(), String> {
        if self.min_play_s > 3_600 {
            return Err("min_play_s is at most 3600".to_owned());
        }
        let names = |what: &str, names: &mut dyn Iterator<Item = &String>| -> Result<(), String> {
            let mut seen = std::collections::HashSet::new();
            for name in names {
                let trimmed = name.trim();
                if trimmed.is_empty() || trimmed.chars().count() > 64 || trimmed != name {
                    return Err(format!(
                        "{what} are 1 to 64 characters, without spaces around"
                    ));
                }
                if !seen.insert(name.to_lowercase()) {
                    return Err(format!("{what}: {name:?} appears twice"));
                }
            }
            if seen.len() > 50 {
                return Err(format!("at most 50 {what}"));
            }
            Ok(())
        };
        names("eras", &mut self.eras.iter().map(|era| &era.name))?;
        names("moods", &mut self.moods.iter())?;
        names("sound codes", &mut self.sound_codes.iter())?;
        for era in &self.eras {
            if let (Some(from), Some(to)) = (era.from_year, era.to_year)
                && from > to
            {
                return Err(format!("era {:?} ends before it starts", era.name));
            }
        }
        Ok(())
    }
}

impl Default for ProgrammingSettings {
    fn default() -> Self {
        Self {
            min_play_s: 30,
            eras: Vec::new(),
            moods: (1..=5).map(|energy| energy.to_string()).collect(),
            sound_codes: ["soft", "medium", "hard"].map(str::to_owned).to_vec(),
        }
    }
}

/// Library settings as shown: the AcoustID key is never sent back.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct LibrarySettingsView {
    /// Whether the station has set its AcoustID key (identification is off without one).
    pub acoustid_key_set: bool,
    pub layout: String,
    pub min_score: f64,
    pub save_cover: bool,
    pub embed_cover: bool,
    /// Largest file accepted by upload, in MiB.
    pub max_upload_mb: u64,
    /// Uploads are refused when they would leave less free space on the
    /// buffer zone's disk, in MiB.
    pub min_free_mb: u64,
    /// Days deleted tracks stay in the trash.
    pub trash_days: u32,
    /// Tags edited from the browser are written into the files; off, they
    /// are kept in Heron's database and the files are left alone.
    pub write_tags: bool,
}

impl From<LibrarySettings> for LibrarySettingsView {
    fn from(settings: LibrarySettings) -> Self {
        Self {
            acoustid_key_set: settings.acoustid_key.is_some(),
            layout: settings.layout,
            min_score: settings.min_score,
            save_cover: settings.save_cover,
            embed_cover: settings.embed_cover,
            max_upload_mb: settings.max_upload_mb,
            min_free_mb: settings.min_free_mb,
            trash_days: settings.trash_days,
            write_tags: settings.write_tags,
        }
    }
}

/// Changes to the library settings: missing fields are kept.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LibrarySettingsPatch {
    /// The station's AcoustID application key; `null` removes it.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<String>)]
    pub acoustid_key: Option<Option<String>>,
    /// See `docs/library.md#layout`.
    pub layout: Option<String>,
    /// Lowest AcoustID score (0 to 1) accepted as a match.
    pub min_score: Option<f64>,
    pub save_cover: Option<bool>,
    pub embed_cover: Option<bool>,
    /// 1 or more.
    pub max_upload_mb: Option<u64>,
    pub min_free_mb: Option<u64>,
    /// 1 to 3650.
    pub trash_days: Option<u32>,
    /// Write tags edited from the browser into the files (true), or keep
    /// them in Heron's database (false).
    pub write_tags: Option<bool>,
}

/// Tells a field set to `null` (`Some(None)`) from a missing one (`None`).
pub(crate) fn some_or_null<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Marks the engine's configuration as managed by the API (see `sync`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineManagement {
    pub managed: bool,
}

impl EngineManagement {
    pub const SETTINGS_NAME: &'static str = "engine";
}

/// The settings groups each station has its own of, in `station_settings`
/// (docs/stations.md#settings). The others are shared, in `settings`.
pub const PER_STATION: [&str; 7] = [
    "station",
    "playout",
    "eq",
    "queue",
    "engine",
    "rotation",
    "medley_categories",
];

fn shared(name: &str) {
    debug_assert!(
        !PER_STATION.contains(&name),
        "{name} settings belong to a station: use load_for and save_for"
    );
}

fn per_station(name: &str) {
    debug_assert!(
        PER_STATION.contains(&name),
        "{name} settings are shared: use load and save"
    );
}

fn parse<T: DeserializeOwned + Default>(name: &str, value: Option<String>) -> ApiResult<T> {
    match value {
        None => Ok(T::default()),
        Some(json) => serde_json::from_str(&json)
            .map_err(|error| ApiError::Internal(format!("stored {name} settings: {error}"))),
    }
}

/// Reads a station's settings group; its defaults when it was never saved.
pub async fn load_for<T: DeserializeOwned + Default>(
    executor: impl sqlx::MySqlExecutor<'_>,
    station_id: u64,
    name: &str,
) -> ApiResult<T> {
    per_station(name);
    let value: Option<String> = sqlx::query_scalar(
        "SELECT CAST(value AS CHAR) FROM station_settings WHERE station_id = ? AND name = ?",
    )
    .bind(station_id)
    .bind(name)
    .fetch_optional(executor)
    .await?;
    parse(name, value)
}

/// Whether a station's settings group was ever saved.
pub async fn exists_for(pool: &MySqlPool, station_id: u64, name: &str) -> ApiResult<bool> {
    per_station(name);
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM station_settings WHERE station_id = ? AND name = ?",
    )
    .bind(station_id)
    .bind(name)
    .fetch_one(pool)
    .await?
        > 0)
}

pub async fn save_for<T: Serialize>(
    executor: impl sqlx::MySqlExecutor<'_>,
    station_id: u64,
    name: &str,
    value: &T,
) -> ApiResult<()> {
    per_station(name);
    let json = serde_json::to_string(value)
        .map_err(|error| ApiError::Internal(format!("{name} settings: {error}")))?;
    sqlx::query(
        "INSERT INTO station_settings (station_id, name, value) VALUES (?, ?, ?)
         ON DUPLICATE KEY UPDATE value = VALUES(value)",
    )
    .bind(station_id)
    .bind(name)
    .bind(json)
    .execute(executor)
    .await?;
    Ok(())
}

/// Reads a shared settings group; its defaults when it was never saved.
pub async fn load<T: DeserializeOwned + Default>(
    executor: impl sqlx::MySqlExecutor<'_>,
    name: &str,
) -> ApiResult<T> {
    shared(name);
    let value: Option<String> =
        sqlx::query_scalar("SELECT CAST(value AS CHAR) FROM settings WHERE name = ?")
            .bind(name)
            .fetch_optional(executor)
            .await?;
    parse(name, value)
}

pub async fn save<T: Serialize>(
    executor: impl sqlx::MySqlExecutor<'_>,
    name: &str,
    value: &T,
) -> ApiResult<()> {
    shared(name);
    let json = serde_json::to_string(value)
        .map_err(|error| ApiError::Internal(format!("{name} settings: {error}")))?;
    sqlx::query(
        "INSERT INTO settings (name, value) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE value = VALUES(value)",
    )
    .bind(name)
    .bind(json)
    .execute(executor)
    .await?;
    Ok(())
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_playout, put_playout))
        .routes(routes!(get_analysis, put_analysis))
        .routes(routes!(get_library, patch_library))
        .routes(routes!(get_queue_settings, put_queue_settings))
        .routes(routes!(get_rotation_settings, put_rotation_settings))
        .routes(routes!(get_station, put_station))
        .routes(routes!(get_programming, put_programming))
        .routes(routes!(get_statistics, put_statistics))
}

/// Crossfades and loudness normalization.
#[utoipa::path(get, path = "/settings/playout", tag = "settings",
    responses((status = 200, body = PlayoutSettings), (status = "4XX", body = ErrorBody)))]
async fn get_playout(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<PlayoutSettings>> {
    Ok(Json(
        load_for(&state.pool, station.id(), PlayoutSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the playout settings (missing fields take the defaults) and
/// pushes them to the engine. Admins only.
#[utoipa::path(put, path = "/settings/playout", tag = "settings",
    request_body = PlayoutSettings,
    responses((status = 200, body = PlayoutSettings), (status = "4XX", body = ErrorBody)))]
async fn put_playout(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(settings): Json<PlayoutSettings>,
) -> ApiResult<Json<PlayoutSettings>> {
    user.require(Role::Admin)?;
    check_playout(&settings)?;
    save_for(
        &state.pool,
        station.id(),
        PlayoutSettings::SETTINGS_NAME,
        &settings,
    )
    .await?;
    tracing::info!(user = %user.username, "playout settings changed");
    station.sync.changed();
    Ok(Json(settings))
}

fn check_playout(settings: &PlayoutSettings) -> ApiResult<()> {
    let crossfade = &settings.crossfade;
    for (name, fade) in [
        ("fade_out", crossfade.fade_out),
        ("fade_in", crossfade.fade_in),
    ] {
        if fade.level_pct > 100 {
            return Err(ApiError::Invalid(format!(
                "{name}.level_pct is a percentage, 0 to 100"
            )));
        }
        if fade.time_ms > 60_000 {
            return Err(ApiError::Invalid(format!(
                "{name}.time_ms is at most 60,000"
            )));
        }
    }
    let loudness = &settings.loudness;
    if !(-70.0..=0.0).contains(&loudness.target_lufs) {
        return Err(ApiError::Invalid(
            "loudness.target_lufs must be between -70 and 0".to_owned(),
        ));
    }
    if !(0.0..=40.0).contains(&loudness.max_gain_db) {
        return Err(ApiError::Invalid(
            "loudness.max_gain_db must be between 0 and 40".to_owned(),
        ));
    }
    if !(1_000..=600_000).contains(&settings.silence_watchdog.timeout_ms) {
        return Err(ApiError::Invalid(
            "silence_watchdog.timeout_ms must be between 1,000 and 600,000".to_owned(),
        ));
    }
    Ok(())
}

/// Gap Killer and crossfade point detection, used at import.
#[utoipa::path(get, path = "/settings/analysis", tag = "settings",
    responses((status = 200, body = AnalysisSettings), (status = "4XX", body = ErrorBody)))]
async fn get_analysis(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<AnalysisSettings>> {
    Ok(Json(
        load(&state.pool, AnalysisSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the analysis settings. Every track is analyzed again with the
/// new settings, in the background. Admins only.
#[utoipa::path(put, path = "/settings/analysis", tag = "settings",
    request_body = AnalysisSettings,
    responses((status = 200, body = AnalysisSettings), (status = "4XX", body = ErrorBody)))]
async fn put_analysis(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(settings): Json<AnalysisSettings>,
) -> ApiResult<Json<AnalysisSettings>> {
    user.require(Role::Admin)?;
    if settings.fade_min_ms > settings.fade_max_ms {
        return Err(ApiError::Invalid(
            "fade_min_ms must not be above fade_max_ms".to_owned(),
        ));
    }
    let previous: AnalysisSettings = load(&state.pool, AnalysisSettings::SETTINGS_NAME).await?;
    save(&state.pool, AnalysisSettings::SETTINGS_NAME, &settings).await?;
    if previous != settings {
        let queued = heron_analysis::jobs::enqueue_stale(&state.pool, &settings)
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?;
        tracing::info!(user = %user.username, queued, "analysis settings changed: re-analyzing");
    }
    Ok(Json(settings))
}

/// Identification and folder layout, used by `heron-librarian`.
#[utoipa::path(get, path = "/settings/library", tag = "settings",
    responses((status = 200, body = LibrarySettingsView), (status = "4XX", body = ErrorBody)))]
async fn get_library(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<LibrarySettingsView>> {
    let settings: LibrarySettings = load(&state.pool, LibrarySettings::SETTINGS_NAME).await?;
    Ok(Json(settings.into()))
}

/// Changes some library settings; the others are kept. Admins only.
#[utoipa::path(patch, path = "/settings/library", tag = "settings",
    request_body = LibrarySettingsPatch,
    responses((status = 200, body = LibrarySettingsView), (status = "4XX", body = ErrorBody)))]
async fn patch_library(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(patch): Json<LibrarySettingsPatch>,
) -> ApiResult<Json<LibrarySettingsView>> {
    user.require(Role::Admin)?;
    let mut settings: LibrarySettings = load(&state.pool, LibrarySettings::SETTINGS_NAME).await?;
    if let Some(key) = patch.acoustid_key {
        settings.acoustid_key = key
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty());
    }
    if let Some(layout) = patch.layout {
        Layout::parse(&layout).map_err(|error| ApiError::Invalid(format!("layout: {error}")))?;
        settings.layout = layout;
    }
    if let Some(min_score) = patch.min_score {
        if !(0.0..=1.0).contains(&min_score) {
            return Err(ApiError::Invalid("min_score is between 0 and 1".to_owned()));
        }
        settings.min_score = min_score;
    }
    if let Some(max) = patch.max_upload_mb {
        if max == 0 {
            return Err(ApiError::Invalid("max_upload_mb is at least 1".to_owned()));
        }
        settings.max_upload_mb = max;
    }
    settings.min_free_mb = patch.min_free_mb.unwrap_or(settings.min_free_mb);
    if let Some(days) = patch.trash_days {
        if !(1..=3650).contains(&days) {
            return Err(ApiError::Invalid("trash_days is 1 to 3650".to_owned()));
        }
        settings.trash_days = days;
    }
    settings.save_cover = patch.save_cover.unwrap_or(settings.save_cover);
    settings.embed_cover = patch.embed_cover.unwrap_or(settings.embed_cover);
    settings.write_tags = patch.write_tags.unwrap_or(settings.write_tags);
    save(&state.pool, LibrarySettings::SETTINGS_NAME, &settings).await?;
    tracing::info!(user = %user.username, ?settings, "library settings changed");
    Ok(Json(settings.into()))
}

#[utoipa::path(get, path = "/settings/queue", tag = "settings",
    responses((status = 200, body = QueueSettings), (status = "4XX", body = ErrorBody)))]
async fn get_queue_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<QueueSettings>> {
    Ok(Json(
        load_for(&state.pool, station.id(), QueueSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the queue manager settings. Operators and admins.
#[utoipa::path(put, path = "/settings/queue", tag = "settings",
    request_body = QueueSettings,
    responses((status = 200, body = QueueSettings), (status = "4XX", body = ErrorBody)))]
async fn put_queue_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(settings): Json<QueueSettings>,
) -> ApiResult<Json<QueueSettings>> {
    user.require(Role::Operator)?;
    if !(1..=50).contains(&settings.min_queued) {
        return Err(ApiError::Invalid(
            "min_queued is between 1 and 50".to_owned(),
        ));
    }
    save_for(
        &state.pool,
        station.id(),
        QueueSettings::SETTINGS_NAME,
        &settings,
    )
    .await?;
    station.queue.wake();
    Ok(Json(settings))
}

/// Separation rules, the fallback category and weights.
#[utoipa::path(get, path = "/settings/rotation", tag = "settings",
    responses((status = 200, body = RotationSettings), (status = "4XX", body = ErrorBody)))]
async fn get_rotation_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<RotationSettings>> {
    Ok(Json(
        load_for(&state.pool, station.id(), RotationSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the rotation settings (missing fields take the defaults).
/// Operators and admins.
#[utoipa::path(put, path = "/settings/rotation", tag = "settings",
    request_body = RotationSettings,
    responses((status = 200, body = RotationSettings), (status = "4XX", body = ErrorBody)))]
async fn put_rotation_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(settings): Json<RotationSettings>,
) -> ApiResult<Json<RotationSettings>> {
    user.require(Role::Operator)?;
    settings.check().map_err(ApiError::Invalid)?;
    if let Some(id) = settings
        .across
        .station_ids
        .iter()
        .find(|id| **id == station.id() || state.stations.get(**id).is_none())
    {
        return Err(ApiError::Invalid(format!(
            "station {id} is not another station"
        )));
    }
    for id in settings
        .fallback_category_id
        .iter()
        .chain(&settings.kept_out_category_ids)
    {
        crate::categories::check_offered(&state.pool, station.id(), *id).await?;
    }
    save_for(
        &state.pool,
        station.id(),
        RotationSettings::SETTINGS_NAME,
        &settings,
    )
    .await?;
    tracing::info!(user = %user.username, ?settings, "rotation settings changed");
    Ok(Json(settings))
}

#[utoipa::path(get, path = "/settings/station", tag = "settings",
    responses((status = 200, body = StationSettings), (status = "4XX", body = ErrorBody)))]
async fn get_station(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<StationSettings>> {
    Ok(Json(
        load_for(&state.pool, station.id(), StationSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the station settings. Admins only.
#[utoipa::path(put, path = "/settings/station", tag = "settings",
    request_body = StationSettings,
    responses((status = 200, body = StationSettings), (status = "4XX", body = ErrorBody)))]
async fn put_station(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(settings): Json<StationSettings>,
) -> ApiResult<Json<StationSettings>> {
    user.require(Role::Admin)?;
    TimeZone::get(&settings.timezone).map_err(|_| {
        ApiError::Invalid(format!(
            "unknown time zone {:?}: use an IANA name such as Europe/Paris",
            settings.timezone
        ))
    })?;
    save_for(
        &state.pool,
        station.id(),
        StationSettings::SETTINGS_NAME,
        &settings,
    )
    .await?;
    tracing::info!(user = %user.username, timezone = %settings.timezone, "station settings changed");
    Ok(Json(settings))
}

/// What counts as a play, eras, and the mood and sound code scales.
#[utoipa::path(get, path = "/settings/programming", tag = "settings",
    responses((status = 200, body = ProgrammingSettings), (status = "4XX", body = ErrorBody)))]
async fn get_programming(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<ProgrammingSettings>> {
    Ok(Json(
        load(&state.pool, ProgrammingSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the programming settings (missing fields take the defaults).
/// Tracks keep their mood and sound code when a scale changes. Operators
/// and admins.
#[utoipa::path(put, path = "/settings/programming", tag = "settings",
    request_body = ProgrammingSettings,
    responses((status = 200, body = ProgrammingSettings), (status = "4XX", body = ErrorBody)))]
async fn put_programming(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(settings): Json<ProgrammingSettings>,
) -> ApiResult<Json<ProgrammingSettings>> {
    user.require(Role::Operator)?;
    settings.check().map_err(ApiError::Invalid)?;
    save(&state.pool, ProgrammingSettings::SETTINGS_NAME, &settings).await?;
    tracing::info!(user = %user.username, ?settings, "programming settings changed");
    Ok(Json(settings))
}

/// Listener statistics: the minimum session, how long sessions are kept.
#[utoipa::path(get, path = "/settings/statistics", tag = "settings",
    responses((status = 200, body = StatisticsSettings), (status = "4XX", body = ErrorBody)))]
async fn get_statistics(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<StatisticsSettings>> {
    Ok(Json(
        load(&state.pool, StatisticsSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the listener statistics settings (missing fields take the
/// defaults). A new minimum session recomputes the totals the sessions
/// kept still cover. Admins only.
#[utoipa::path(put, path = "/settings/statistics", tag = "settings",
    request_body = StatisticsSettings,
    responses((status = 200, body = StatisticsSettings), (status = "4XX", body = ErrorBody)))]
async fn put_statistics(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(settings): Json<StatisticsSettings>,
) -> ApiResult<Json<StatisticsSettings>> {
    user.require(Role::Admin)?;
    settings.check().map_err(ApiError::Invalid)?;
    let before: StatisticsSettings = load(&state.pool, StatisticsSettings::SETTINGS_NAME).await?;
    save(&state.pool, StatisticsSettings::SETTINGS_NAME, &settings).await?;
    if before.min_session_s != settings.min_session_s {
        state.listeners.settings_changed();
    }
    tracing::info!(user = %user.username, ?settings, "statistics settings changed");
    Ok(Json(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_patch_tells_null_from_missing() {
        let clear: LibrarySettingsPatch =
            serde_json::from_str(r#"{"acoustid_key": null}"#).unwrap();
        assert_eq!(clear.acoustid_key, Some(None));
        let keep: LibrarySettingsPatch =
            serde_json::from_str(r#"{"layout": "{title}.{ext}"}"#).unwrap();
        assert_eq!(keep.acoustid_key, None);
        let set: LibrarySettingsPatch = serde_json::from_str(r#"{"acoustid_key": "k"}"#).unwrap();
        assert_eq!(set.acoustid_key, Some(Some("k".to_owned())));
    }

    #[test]
    fn statistics_settings_are_checked() {
        assert!(StatisticsSettings::default().check().is_ok());
        let partial: StatisticsSettings = serde_json::from_str(r#"{"min_session_s": 0}"#).unwrap();
        assert_eq!(partial.retention_days, 90);
        assert!(partial.check().is_ok());
        for (min_session_s, retention_days) in [(3_601, 90), (60, 6), (60, 3_651)] {
            let settings = StatisticsSettings {
                min_session_s,
                retention_days,
            };
            assert!(settings.check().is_err());
        }
    }

    #[test]
    fn programming_settings_are_checked() {
        let defaults = ProgrammingSettings::default();
        assert_eq!(defaults.check(), Ok(()));
        let twice = ProgrammingSettings {
            moods: vec!["calm".into(), "Calm".into()],
            ..ProgrammingSettings::default()
        };
        assert!(twice.check().is_err());
        let backwards = ProgrammingSettings {
            eras: vec![Era {
                name: "currents".into(),
                from_year: Some(2024),
                to_year: Some(2020),
            }],
            ..ProgrammingSettings::default()
        };
        assert!(backwards.check().is_err());
        let partial: ProgrammingSettings = serde_json::from_str(r#"{"min_play_s": 0}"#).unwrap();
        assert_eq!(partial.sound_codes, defaults.sound_codes);
    }

    #[test]
    fn rejects_odd_playout_settings() {
        let mut settings = PlayoutSettings::default();
        check_playout(&settings).unwrap();
        settings.crossfade.fade_out.level_pct = 150;
        assert!(check_playout(&settings).is_err());
    }
}
