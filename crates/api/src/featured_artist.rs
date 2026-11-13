// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Featured artist (`docs/listener-interaction.md#featured-artist`): at
//! set minutes of its hours, a track of the artist of the week, or of the
//! day, at a soft exact time outside the clocks, after a jingle if any.
//!
//! Its hours, website and notifications are a show's (`shows.rs`); its own
//! settings are the `featured_artist` settings of the station, and its
//! planning the `featured_artists` table. Rotation plays its times
//! ([`heron_rotation::ShowTime`]): the queue manager gives them to it with
//! the library ([`show_times`]).

use std::collections::HashMap;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use heron_core::Module;
use heron_core::shows::{Show, ShowSettings, Week};
use heron_rotation::ShowTime;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::{self, StationSettings};
use crate::stations::Station;

/// The minutes of an hour it can play at.
pub const MINUTES: [u8; 4] = [0, 15, 30, 45];
const MAX_WINDOW_S: u32 = 300;
/// The planning listed by default: this week and the next 12, or today
/// and the next 31 days.
const WEEKS_LISTED: i64 = 12;
const DAYS_LISTED: i64 = 31;
/// The longest planning listed at once.
const MAX_LISTED_DAYS: i64 = 400;
/// A week or day over for this long is deleted.
const KEEP_DAYS: i64 = 365;
const PURGE_EVERY: Duration = Duration::from_secs(6 * 3_600);
/// The times given to rotation: from an hour back (a time due now) to a
/// few hours ahead (the queue's look ahead).
const BEHIND_MS: i64 = 3_600_000;
const AHEAD_MS: i64 = 4 * 3_600_000;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_featured_settings, put_featured_settings))
        .routes(routes!(get_featured_planning))
        .routes(routes!(put_featured_entry, delete_featured_entry))
        .routes(routes!(get_featured_day))
}

/// An artist per week, from Monday to Sunday, or per day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum By {
    #[default]
    Week,
    Day,
}

impl By {
    fn as_str(self) -> &'static str {
        match self {
            Self::Week => "week",
            Self::Day => "day",
        }
    }

    /// The planning's entry `date` falls in: the Monday of its week, or
    /// the day.
    pub fn key(self, date: Date) -> Date {
        match self {
            Self::Week => {
                let back = i64::from(date.weekday().to_monday_zero_offset());
                date.checked_sub(back.days()).unwrap_or(date)
            }
            Self::Day => date,
        }
    }
}

/// The show's own settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FeaturedSettings {
    /// Of each hour of its hours: some of 0, 15, 30 and 45.
    pub minutes: Vec<u8>,
    pub by: By,
    /// How early and how late the jingle (or the track) may start, in
    /// seconds, 0 to 300.
    pub window_before_s: u32,
    pub window_after_s: u32,
    /// The jingle before the track, a category of the station; `None`:
    /// none.
    pub jingle_category_id: Option<u64>,
}

impl Default for FeaturedSettings {
    fn default() -> Self {
        Self {
            minutes: vec![0],
            by: By::Week,
            window_before_s: 10,
            window_after_s: 30,
            jingle_category_id: None,
        }
    }
}

impl FeaturedSettings {
    pub const SETTINGS_NAME: &str = "featured_artist";

    fn check(&mut self) -> Result<(), String> {
        if self.minutes.is_empty() || self.minutes.iter().any(|m| !MINUTES.contains(m)) {
            return Err("minutes are some of 0, 15, 30 and 45, one at least".to_owned());
        }
        self.minutes.sort_unstable();
        self.minutes.dedup();
        if self.window_before_s > MAX_WINDOW_S || self.window_after_s > MAX_WINDOW_S {
            return Err(format!(
                "the window reaches at most {MAX_WINDOW_S} seconds before or after its time"
            ));
        }
        Ok(())
    }
}

/// The Featured artist's entry among the shows.
static FEATURED: Show = heron_core::shows::SHOWS[4];

pub(crate) fn featured_show() -> &'static Show {
    Show::of(Module::FeaturedArtist).unwrap_or(&FEATURED)
}

async fn load_settings(pool: &MySqlPool, station_id: u64) -> ApiResult<FeaturedSettings> {
    settings::load_for(pool, station_id, FeaturedSettings::SETTINGS_NAME).await
}

async fn time_zone(pool: &MySqlPool, station_id: u64) -> ApiResult<TimeZone> {
    let station: StationSettings =
        settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await?;
    Ok(station.time_zone())
}

/// Its times from `from` (included) to `to` (excluded): the minutes of each
/// hour of `week`, in `tz`.
pub fn times(
    week: &Week,
    minutes: &[u8],
    tz: &TimeZone,
    from: Timestamp,
    to: Timestamp,
) -> Vec<Timestamp> {
    let mut times = Vec::new();
    let mut date = from.to_zoned(tz.clone()).date();
    let last = to.to_zoned(tz.clone()).date();
    while date <= last {
        let day = date.weekday().to_monday_one_offset() as u8;
        for hour in 0..24u8 {
            if !week.is_open(day, hour) {
                continue;
            }
            for minute in minutes {
                let Ok(at) = date
                    .at(hour as i8, *minute as i8, 0, 0)
                    .to_zoned(tz.clone())
                else {
                    continue;
                };
                // A time the clocks skip moves to after the gap: not ours.
                if at.hour() != hour as i8 {
                    continue;
                }
                let at = at.timestamp();
                if at >= from && at < to {
                    times.push(at);
                }
            }
        }
        let Ok(next) = date.tomorrow() else {
            break;
        };
        date = next;
    }
    times.sort_unstable();
    times.dedup();
    times
}

fn date_of(text: &str) -> ApiResult<Date> {
    text.parse::<Date>()
        .map_err(|_| ApiError::Invalid(format!("{text:?} is not a date (YYYY-MM-DD)")))
}

fn seconds(ms: i64) -> f64 {
    ms as f64 / 1_000.0
}

/// The planning's artists from `from` to `to` (keys, included), by key.
async fn planned(
    pool: &MySqlPool,
    station_id: u64,
    by: By,
    from: Date,
    to: Date,
) -> ApiResult<HashMap<Date, (u64, String)>> {
    let rows: Vec<(String, u64, String)> = sqlx::query_as(
        "SELECT CAST(f.day AS CHAR), f.artist_id, a.name FROM featured_artists f
         JOIN artists a ON a.id = f.artist_id
         WHERE f.station_id = ? AND f.period = ? AND f.day BETWEEN ? AND ?",
    )
    .bind(station_id)
    .bind(by.as_str())
    .bind(from.to_string())
    .bind(to.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(day, id, name)| Some((day.parse::<Date>().ok()?, (id, name))))
        .collect())
}

/// The artist of the week or the day of `at`, if one is planned.
pub(crate) async fn artist_at(
    pool: &MySqlPool,
    station_id: u64,
    at: Timestamp,
) -> ApiResult<Option<(u64, String)>> {
    let settings = load_settings(pool, station_id).await?;
    let tz = time_zone(pool, station_id).await?;
    let key = settings.by.key(at.to_zoned(tz).date());
    Ok(planned(pool, station_id, settings.by, key, key)
        .await?
        .remove(&key))
}

/// The artist's music tracks: its own and those it is featured on, in no
/// category whose type sends no metadata.
async fn tracks_of(pool: &MySqlPool, artist_id: u64) -> ApiResult<Vec<u64>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT ta.track_id FROM track_artists ta
         WHERE ta.artist_id = ?
           AND NOT EXISTS (SELECT 1 FROM track_categories tc
                           JOIN categories c ON c.id = tc.category_id
                           JOIN category_types ct ON ct.id = c.type_id
                           WHERE tc.track_id = ta.track_id AND NOT ct.sends_metadata)
         ORDER BY ta.track_id",
    )
    .bind(artist_id)
    .fetch_all(pool)
    .await?)
}

/// The show's picks of the station planned from `from_ms` to `to_ms`:
/// for each time, its picks (`track_id`, a jingle, planned at, the play
/// it became and when it started).
type ShowPick = (Option<u64>, bool, i64, Option<i64>);

async fn show_picks(
    pool: &MySqlPool,
    station_id: u64,
    from_ms: i64,
    to_ms: i64,
) -> ApiResult<HashMap<i64, Vec<ShowPick>>> {
    // Its time, the track, `linked` (a jingle), planned at, started at.
    type Row = (i64, Option<u64>, Option<String>, i64, Option<i64>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT CAST(JSON_VALUE(p.explanation, '$.show_at_unix_ms') AS SIGNED),
                p.track_id, JSON_VALUE(p.explanation, '$.linked'),
                CAST(UNIX_TIMESTAMP(p.planned_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(h.started_at) * 1000 AS SIGNED)
         FROM rotation_picks p LEFT JOIN play_history h ON h.id = p.play_id
         WHERE p.station_id = ? AND p.planned_at >= FROM_UNIXTIME(?)
           AND p.planned_at < FROM_UNIXTIME(?)
           AND JSON_VALUE(p.explanation, '$.show_at_unix_ms') IS NOT NULL
         ORDER BY p.planned_at, p.id",
    )
    .bind(station_id)
    .bind(seconds(from_ms))
    .bind(seconds(to_ms))
    .fetch_all(pool)
    .await?;
    let mut picks: HashMap<i64, Vec<ShowPick>> = HashMap::new();
    for (at, track, linked, planned, started) in rows {
        let jingle = matches!(linked.as_deref(), Some("true" | "1"));
        picks
            .entry(at)
            .or_default()
            .push((track, jingle, planned, started));
    }
    Ok(picks)
}

/// The show's settings and own settings, when it plays on the station: the
/// module on and the show enabled.
async fn playing(
    pool: &MySqlPool,
    station_id: u64,
) -> ApiResult<Option<(ShowSettings, FeaturedSettings)>> {
    if !crate::modules::is_on(pool, Some(station_id), Module::FeaturedArtist).await? {
        return Ok(None);
    }
    let show = crate::shows::load(pool, station_id, featured_show()).await?;
    if !show.enabled {
        return Ok(None);
    }
    Ok(Some((show, load_settings(pool, station_id).await?)))
}

/// The show's times around `now_ms` for rotation: those with an artist
/// planned and tracks of it in `library`, each marked done once the
/// rotation log has its pick. Empty while the show does not play.
pub async fn show_times(
    pool: &MySqlPool,
    station_id: u64,
    library: &heron_rotation::Library,
    now_ms: i64,
) -> ApiResult<Vec<ShowTime>> {
    let Some((show, settings)) = playing(pool, station_id).await? else {
        return Ok(Vec::new());
    };
    let tz = time_zone(pool, station_id).await?;
    let from = Timestamp::from_millisecond(now_ms - BEHIND_MS)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let to = Timestamp::from_millisecond(now_ms + AHEAD_MS)
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let times = times(&show.open_week(), &settings.minutes, &tz, from, to);
    if times.is_empty() {
        return Ok(Vec::new());
    }
    let first = settings.by.key(times[0].to_zoned(tz.clone()).date());
    let last = settings
        .by
        .key(times[times.len() - 1].to_zoned(tz.clone()).date());
    let artists = planned(pool, station_id, settings.by, first, last).await?;
    let picks = show_picks(
        pool,
        station_id,
        now_ms - 2 * BEHIND_MS,
        now_ms + 2 * AHEAD_MS,
    )
    .await?;
    let mut tracks: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut shown = Vec::new();
    for at in times {
        let key = settings.by.key(at.to_zoned(tz.clone()).date());
        let Some((artist, _)) = artists.get(&key) else {
            continue;
        };
        if !tracks.contains_key(artist) {
            let theirs = tracks_of(pool, *artist).await?;
            tracks.insert(*artist, theirs);
        }
        let theirs: Vec<u64> = tracks[artist]
            .iter()
            .copied()
            .filter(|track| library.tracks.contains_key(track))
            .collect();
        if theirs.is_empty() {
            continue;
        }
        let at_ms = at.as_millisecond();
        shown.push(ShowTime {
            at_ms,
            before_s: settings.window_before_s,
            after_s: settings.window_after_s,
            tracks: theirs,
            jingle_category: settings.jingle_category_id,
            done: picks.contains_key(&at_ms),
        });
    }
    Ok(shown)
}

/// For the widget: the artist of now, and the show's next time today.
pub async fn public_state(
    pool: &MySqlPool,
    station_id: u64,
    show: &ShowSettings,
    timezone: &str,
) -> ApiResult<serde_json::Value> {
    let settings = load_settings(pool, station_id).await?;
    let tz = TimeZone::get(timezone).unwrap_or(TimeZone::UTC);
    let now = Timestamp::now();
    let key = settings.by.key(now.to_zoned(tz.clone()).date());
    let artist = planned(pool, station_id, settings.by, key, key)
        .await?
        .remove(&key)
        .map(|(_, name)| name);
    let end = now
        .to_zoned(tz.clone())
        .date()
        .tomorrow()
        .ok()
        .and_then(|day| day.to_zoned(tz.clone()).ok())
        .map_or(now, |day| day.timestamp());
    let next = times(&show.open_week(), &settings.minutes, &tz, now, end)
        .first()
        .map(|at| at.as_millisecond());
    Ok(serde_json::json!({
        "by": settings.by,
        "artist": artist,
        "next_at_unix_ms": artist.as_ref().and(next),
    }))
}

/// Deletes the planning a year after its week or day. Never returns.
pub async fn purge(pool: MySqlPool) {
    let mut interval = tokio::time::interval(PURGE_EVERY);
    loop {
        interval.tick().await;
        let deleted = sqlx::query(
            "DELETE FROM featured_artists
             WHERE day < CURDATE() - INTERVAL ? DAY - INTERVAL IF(period = 'week', 6, 0) DAY",
        )
        .bind(KEEP_DAYS)
        .execute(&pool)
        .await;
        match deleted {
            Ok(done) if done.rows_affected() > 0 => {
                tracing::debug!(
                    deleted = done.rows_affected(),
                    "old featured artists deleted"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "cannot delete old featured artists"),
        }
    }
}

// ---------------------------------------------------------------------------
// Routes

/// The station's Featured artist settings.
#[utoipa::path(get, path = "/featured-artist/settings", tag = "featured_artist",
    responses((status = 200, body = FeaturedSettings), (status = "4XX", body = ErrorBody)))]
async fn get_featured_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<FeaturedSettings>> {
    Ok(Json(load_settings(&state.pool, station.id()).await?))
}

/// Replaces them. The jingle category must be the station's (`422`).
/// Operators.
#[utoipa::path(put, path = "/featured-artist/settings", tag = "featured_artist",
    request_body = FeaturedSettings,
    responses((status = 200, body = FeaturedSettings), (status = "4XX", body = ErrorBody)))]
async fn put_featured_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(mut settings): Json<FeaturedSettings>,
) -> ApiResult<Json<FeaturedSettings>> {
    user.require(Role::Operator)?;
    settings.check().map_err(ApiError::Invalid)?;
    let pool = &state.pool;
    if let Some(id) = settings.jingle_category_id {
        let ours: Option<u64> = sqlx::query_scalar(
            "SELECT category_id FROM category_stations WHERE station_id = ? AND category_id = ?",
        )
        .bind(station.id())
        .bind(id)
        .fetch_optional(pool)
        .await?;
        if ours.is_none() {
            return Err(ApiError::Invalid(format!(
                "category {id} is not one of the station's"
            )));
        }
    }
    settings::save_for(
        pool,
        station.id(),
        FeaturedSettings::SETTINGS_NAME,
        &settings,
    )
    .await?;
    tracing::info!(user = %user.username, station = station.id(), "featured artist settings changed");
    station.queue.wake();
    Ok(Json(settings))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PlanningQuery {
    /// The first date (`YYYY-MM-DD`): this week, or today, by default.
    pub from: Option<String>,
    /// The last date: 12 weeks, or 31 days, after the first by default.
    pub to: Option<String>,
}

/// A week or a day of the planning.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PlanningEntry {
    /// The Monday of the week, or the day (`YYYY-MM-DD`).
    pub date: String,
    pub artist_id: Option<u64>,
    pub artist: Option<String>,
    /// The week or day of today.
    pub current: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Planning {
    pub by: By,
    pub entries: Vec<PlanningEntry>,
}

async fn today(pool: &MySqlPool, station_id: u64) -> ApiResult<Date> {
    let tz = time_zone(pool, station_id).await?;
    Ok(Timestamp::now().to_zoned(tz).date())
}

/// The planning: each week (or day) from `from` to `to`, with its artist
/// or none.
#[utoipa::path(get, path = "/featured-artist/planning", tag = "featured_artist",
    params(PlanningQuery),
    responses((status = 200, body = Planning), (status = "4XX", body = ErrorBody)))]
async fn get_featured_planning(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<PlanningQuery>,
) -> ApiResult<Json<Planning>> {
    let pool = &state.pool;
    let settings = load_settings(pool, station.id()).await?;
    let by = settings.by;
    let now = by.key(today(pool, station.id()).await?);
    let from = match &query.from {
        Some(text) => by.key(date_of(text)?),
        None => now,
    };
    let to = match &query.to {
        Some(text) => by.key(date_of(text)?),
        None => match by {
            By::Week => from.checked_add((WEEKS_LISTED * 7).days()),
            By::Day => from.checked_add(DAYS_LISTED.days()),
        }
        .map_err(|error| ApiError::Invalid(error.to_string()))?,
    };
    if to < from || (to - from).get_days() > MAX_LISTED_DAYS as i32 {
        return Err(ApiError::Invalid(format!(
            "from is before to, {MAX_LISTED_DAYS} days apart at most"
        )));
    }
    let mut artists = planned(pool, station.id(), by, from, to).await?;
    let step = match by {
        By::Week => 7,
        By::Day => 1,
    };
    let mut entries = Vec::new();
    let mut date = from;
    while date <= to {
        let artist = artists.remove(&date);
        entries.push(PlanningEntry {
            date: date.to_string(),
            artist_id: artist.as_ref().map(|(id, _)| *id),
            artist: artist.map(|(_, name)| name),
            current: date == now,
        });
        let Ok(next) = date.checked_add(step.days()) else {
            break;
        };
        date = next;
    }
    Ok(Json(Planning { by, entries }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryInput {
    pub artist_id: u64,
}

/// The planning's key for `date`, which must be one (a Monday by week)
/// and not over yet (`409`).
async fn entry_key(pool: &MySqlPool, station_id: u64, by: By, date: &str) -> ApiResult<Date> {
    let date = date_of(date)?;
    if by.key(date) != date {
        return Err(ApiError::Invalid(
            "an artist of the week is planned on its Monday".to_owned(),
        ));
    }
    let now = by.key(today(pool, station_id).await?);
    if date < now {
        return Err(ApiError::Conflict(format!(
            "the {} of {date} is over",
            by.as_str()
        )));
    }
    Ok(date)
}

/// Plans the artist of the week of that Monday, or of that day. Operators.
#[utoipa::path(put, path = "/featured-artist/planning/{date}", tag = "featured_artist",
    params(("date" = String, Path, description = "The Monday of the week, or the day (YYYY-MM-DD)")),
    request_body = EntryInput,
    responses((status = 200, body = PlanningEntry), (status = "4XX", body = ErrorBody)))]
async fn put_featured_entry(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(date): Path<String>,
    Json(input): Json<EntryInput>,
) -> ApiResult<Json<PlanningEntry>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let by = load_settings(pool, station.id()).await?.by;
    let key = entry_key(pool, station.id(), by, &date).await?;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM artists WHERE id = ?")
        .bind(input.artist_id)
        .fetch_optional(pool)
        .await?;
    let Some(name) = name else {
        return Err(ApiError::Invalid(format!(
            "there is no artist {}",
            input.artist_id
        )));
    };
    sqlx::query(
        "INSERT INTO featured_artists (station_id, period, day, artist_id) VALUES (?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE artist_id = VALUES(artist_id)",
    )
    .bind(station.id())
    .bind(by.as_str())
    .bind(key.to_string())
    .bind(input.artist_id)
    .execute(pool)
    .await?;
    tracing::info!(user = %user.username, station = station.id(), %key, artist = input.artist_id, "featured artist planned");
    station.queue.wake();
    let now = by.key(today(pool, station.id()).await?);
    Ok(Json(PlanningEntry {
        date: key.to_string(),
        artist_id: Some(input.artist_id),
        artist: Some(name),
        current: key == now,
    }))
}

/// Takes the artist out of the week or the day. Operators.
#[utoipa::path(delete, path = "/featured-artist/planning/{date}", tag = "featured_artist",
    params(("date" = String, Path, description = "The Monday of the week, or the day (YYYY-MM-DD)")),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_featured_entry(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(date): Path<String>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let by = load_settings(pool, station.id()).await?.by;
    let key = entry_key(pool, station.id(), by, &date).await?;
    sqlx::query("DELETE FROM featured_artists WHERE station_id = ? AND period = ? AND day = ?")
        .bind(station.id())
        .bind(by.as_str())
        .bind(key.to_string())
        .execute(pool)
        .await?;
    tracing::info!(user = %user.username, station = station.id(), %key, "featured artist taken out");
    station.queue.wake();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DayQuery {
    /// `YYYY-MM-DD`; today by default.
    pub date: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeStatus {
    /// To come: its track is the one that would be picked now.
    Planned,
    Queued,
    Played,
    /// Its time is over and nothing played for it.
    PassedOver,
    /// No artist, or no track of the artist to play.
    None,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DayTrack {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub length_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DayTime {
    pub at_unix_ms: i64,
    pub status: TimeStatus,
    /// Why it was passed over, or plays nothing.
    pub reason: Option<String>,
    pub jingle: Option<DayTrack>,
    pub track: Option<DayTrack>,
    pub started_at_unix_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Day {
    pub date: String,
    pub artist_id: Option<u64>,
    pub artist: Option<String>,
    pub times: Vec<DayTime>,
}

async fn day_tracks(pool: &MySqlPool, ids: &[u64]) -> ApiResult<HashMap<u64, DayTrack>> {
    let mut tracks = HashMap::new();
    for id in ids {
        let row: Option<(Option<String>, Option<String>, Option<u64>)> = sqlx::query_as(
            "SELECT t.artist, t.title,
                    CAST(GREATEST(COALESCE(o.cue_out_ms, a.cue_out_ms) - COALESCE(o.cue_in_ms, a.cue_in_ms), 0) AS UNSIGNED)
             FROM tracks t
             LEFT JOIN track_analysis a ON a.track_id = t.id
             LEFT JOIN track_overrides o ON o.track_id = t.id
             WHERE t.id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        if let Some((artist, title, length_ms)) = row {
            tracks.insert(
                *id,
                DayTrack {
                    track_id: *id,
                    artist,
                    title,
                    length_ms,
                },
            );
        }
    }
    Ok(tracks)
}

/// The day's times, each with its jingle and track (planned, queued or
/// played), or why nothing played.
#[utoipa::path(get, path = "/featured-artist/day", tag = "featured_artist",
    params(DayQuery),
    responses((status = 200, body = Day), (status = "4XX", body = ErrorBody)))]
async fn get_featured_day(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<DayQuery>,
) -> ApiResult<Json<Day>> {
    let pool = &state.pool;
    let station_id = station.id();
    let tz = time_zone(pool, station_id).await?;
    let date = match &query.date {
        Some(text) => date_of(text)?,
        None => today(pool, station_id).await?,
    };
    let settings = load_settings(pool, station_id).await?;
    let show = crate::shows::load(pool, station_id, featured_show()).await?;
    let start = date
        .to_zoned(tz.clone())
        .map_err(|error| ApiError::Internal(error.to_string()))?
        .timestamp();
    let end = date
        .tomorrow()
        .and_then(|day| day.to_zoned(tz.clone()))
        .map_err(|error| ApiError::Internal(error.to_string()))?
        .timestamp();
    let key = settings.by.key(date);
    let artist = planned(pool, station_id, settings.by, key, key)
        .await?
        .remove(&key);
    let at = times(&show.open_week(), &settings.minutes, &tz, start, end);
    let picks = show_picks(
        pool,
        station_id,
        start.as_millisecond() - BEHIND_MS,
        end.as_millisecond() + BEHIND_MS,
    )
    .await?;
    let candidates = match &artist {
        Some((id, _)) => tracks_of(pool, *id).await?,
        None => Vec::new(),
    };
    // The tracks to come, the least recently played first: one per time.
    let mut coming: Vec<(Option<i64>, u64)> = Vec::new();
    for track in &candidates {
        let last: Option<i64> = sqlx::query_scalar(
            "SELECT CAST(UNIX_TIMESTAMP(MAX(started_at)) * 1000 AS SIGNED) FROM play_history
             WHERE station_id = ? AND track_id = ?",
        )
        .bind(station_id)
        .bind(track)
        .fetch_one(pool)
        .await?;
        // A track queued counts as played at its planned time.
        let queued = picks
            .values()
            .flatten()
            .filter(|(id, _, _, _)| *id == Some(*track))
            .map(|(_, _, planned, _)| *planned)
            .max();
        coming.push((last.max(queued), *track));
    }
    // Each time to come plays the least recently played, which then goes
    // last: they follow one another in this order.
    coming.sort();
    let mut next_coming = 0;
    let now = Timestamp::now().as_millisecond();
    let mut ids: Vec<u64> = candidates.clone();
    ids.extend(
        picks
            .values()
            .flatten()
            .filter_map(|(track, _, _, _)| *track),
    );
    ids.sort_unstable();
    ids.dedup();
    let tracks = day_tracks(pool, &ids).await?;
    let mut times_out = Vec::new();
    for at in at {
        let at_ms = at.as_millisecond();
        let mine = picks.get(&at_ms);
        let jingle = mine
            .and_then(|picks| picks.iter().find(|(_, jingle, _, _)| *jingle))
            .and_then(|(track, _, _, _)| track.and_then(|id| tracks.get(&id).cloned()));
        let track_pick = mine.and_then(|picks| picks.iter().find(|(_, jingle, _, _)| !jingle));
        let (status, reason, track, started) =
            if let Some((track, _, planned, started)) = track_pick {
                let track = track.and_then(|id| tracks.get(&id).cloned());
                match started {
                    Some(started) => (TimeStatus::Played, None, track, Some(*started)),
                    None if *planned + 60_000 >= now => (TimeStatus::Queued, None, track, None),
                    None => (
                        TimeStatus::PassedOver,
                        Some("taken out of the queue before it played".to_owned()),
                        track,
                        None,
                    ),
                }
            } else if artist.is_none() {
                (
                    TimeStatus::None,
                    Some("no artist planned".to_owned()),
                    None,
                    None,
                )
            } else if candidates.is_empty() {
                (
                    TimeStatus::None,
                    Some("the artist has no music track".to_owned()),
                    None,
                    None,
                )
            } else if at_ms + i64::from(settings.window_after_s) * 1_000 < now {
                (
                    TimeStatus::PassedOver,
                    Some(
                        "nothing could play at that time (restrictions, a track that cannot play, \
                     the show or the module off)"
                            .to_owned(),
                    ),
                    None,
                    None,
                )
            } else {
                let next = coming
                    .get(next_coming % coming.len().max(1))
                    .map(|(_, track)| *track);
                next_coming += 1;
                (
                    TimeStatus::Planned,
                    None,
                    next.and_then(|id| tracks.get(&id).cloned()),
                    None,
                )
            };
        times_out.push(DayTime {
            at_unix_ms: at_ms,
            status,
            reason,
            jingle,
            track,
            started_at_unix_ms: started,
        });
    }
    Ok(Json(Day {
        date: date.to_string(),
        artist_id: artist.as_ref().map(|(id, _)| *id),
        artist: artist.map(|(_, name)| name),
        times: times_out,
    }))
}

#[cfg(test)]
mod tests {
    use heron_core::shows::HourRange;

    use super::*;

    fn at(text: &str) -> Timestamp {
        format!("{text}[Europe/Paris]")
            .parse::<jiff::Zoned>()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn its_times_are_the_minutes_of_its_hours() {
        let tz = TimeZone::get("Europe/Paris").unwrap();
        // Monday to Friday, 12:00 to 16:00.
        let week = Week::of(&[HourRange {
            days: vec![1, 2, 3, 4, 5],
            start_hour: 12,
            end_hour: 16,
        }]);
        // Friday 13 November 2026, then the weekend.
        let from = at("2026-11-13T00:00");
        let to = at("2026-11-16T00:00");
        let hourly = times(&week, &[0], &tz, from, to);
        assert_eq!(
            hourly,
            ["12:00", "13:00", "14:00", "15:00"].map(|time| at(&format!("2026-11-13T{time}")))
        );
        let quarters = times(&week, &[15, 30, 45], &tz, from, to);
        assert_eq!(quarters.len(), 12);
        assert_eq!(quarters[0], at("2026-11-13T12:15"));
        assert_eq!(quarters[11], at("2026-11-13T15:45"));
        // From a time on: those after it only.
        assert_eq!(times(&week, &[0], &tz, at("2026-11-13T13:00"), to).len(), 3);
    }

    #[test]
    fn a_week_is_planned_on_its_monday() {
        let friday: Date = "2026-11-13".parse().unwrap();
        assert_eq!(By::Week.key(friday).to_string(), "2026-11-09");
        assert_eq!(By::Day.key(friday), friday);
        let monday: Date = "2026-11-16".parse().unwrap();
        assert_eq!(By::Week.key(monday), monday);
        let sunday: Date = "2026-11-22".parse().unwrap();
        assert_eq!(By::Week.key(sunday), monday);
    }

    #[test]
    fn the_settings_are_checked() {
        let mut settings = FeaturedSettings {
            minutes: vec![45, 15, 15],
            ..FeaturedSettings::default()
        };
        settings.check().unwrap();
        assert_eq!(settings.minutes, [15, 45]);
        for minutes in [vec![], vec![10]] {
            let mut settings = FeaturedSettings {
                minutes,
                ..FeaturedSettings::default()
            };
            assert!(settings.check().is_err());
        }
        let mut settings = FeaturedSettings {
            window_after_s: 301,
            ..FeaturedSettings::default()
        };
        assert!(settings.check().is_err());
    }
}
