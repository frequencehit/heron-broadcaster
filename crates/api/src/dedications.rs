// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dedications (`docs/listener-interaction.md#dedications`): listeners ask
//! for a track with a message during the show's hours, through the
//! gateway; an operator accepts the request into the queue or refuses it.
//!
//! - A request names a requestable track (the search only finds those),
//!   with a first name, who it is for and a message. Limits per device and
//!   per opening range, one waiting at a time per device.
//! - Accepted, its track is queued with the source `request`, next, within
//!   the hour or at the best place for the separation rules; restrictions
//!   always apply. Its queue entry links it to its play when the track
//!   starts (`history.rs`), and the track carries the dedication on
//!   `now-playing` and `track.started`.
//! - Requests still waiting when the range closes are refused; requests
//!   are deleted after the station's retention, the device's hash when the
//!   range closes ([`tick`]).

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use heron_core::shows::{self as core_shows, Show};
use heron_core::{Module, PlaySource};
use heron_rotation::restrict::{Subject, forbidding};
use heron_rotation::{History, Library, Play};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::engine::{PlayerState, QueueEntry};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::gateway::GatewayClient;
use crate::settings::{self, StationSettings};
use crate::stations::{Station, StationRuntime, Stations};
use crate::webhooks::{WebhookEvent, Webhooks};

/// How often waiting requests of a closed range are refused, and old ones
/// deleted.
const TICK: Duration = Duration::from_secs(30);
const MAX_FROM: usize = 40;
const MAX_TO: usize = 60;
/// The gateway's own maximum for a message; the station's is lower.
pub const MAX_MESSAGE: usize = 500;
const MIN_QUERY: usize = 2;
const MAX_QUERY: usize = 60;
/// Tracks a search gives at most.
const SEARCH_RESULTS: u32 = 8;
const MAX_CATEGORIES: usize = 500;
/// Requests listed at most.
const MAX_LIST: u32 = 200;
/// An accepted request whose entry left the queue this long after it was
/// accepted, without playing, was taken out.
const REMOVED_AFTER_SECONDS: u32 = 60;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_dedication_settings, put_dedication_settings))
        .routes(routes!(list_dedications))
        .routes(routes!(accept_dedication))
        .routes(routes!(refuse_dedication))
        .routes(routes!(gateway_submit))
        .routes(routes!(gateway_search))
        .routes(routes!(gateway_follow))
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

fn seconds(at: Timestamp) -> f64 {
    at.as_millisecond() as f64 / 1_000.0
}

fn timestamp(ms: i64) -> ApiResult<Timestamp> {
    Timestamp::from_millisecond(ms).map_err(internal)
}

// ---------------------------------------------------------------------------
// Settings

/// A station's dedication settings (`station_settings`, `dedications`). Its
/// hours, website and notifications are its show settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DedicationSettings {
    /// Requests per opening range, every listener together, 1 to 1000.
    pub max_per_range: u32,
    /// Requests per device per opening range, 1 to 10.
    pub per_device: u32,
    /// Characters of a message at most, 20 to 500.
    pub message_max: u32,
    /// Days requests are kept, 1 to 365.
    pub retention_days: u32,
    /// The categories whose tracks can be requested; none: the station's
    /// music.
    pub category_ids: Vec<u64>,
}

impl Default for DedicationSettings {
    fn default() -> Self {
        Self {
            max_per_range: 30,
            per_device: 1,
            message_max: 140,
            retention_days: 30,
            category_ids: Vec::new(),
        }
    }
}

impl DedicationSettings {
    pub const SETTINGS_NAME: &str = "dedications";
    /// The most a device may send in a range, whatever the station says:
    /// the gateway's own limit.
    pub const MAX_PER_DEVICE: u32 = 10;

    fn check(&self) -> Result<(), String> {
        if !(1..=1_000).contains(&self.max_per_range) {
            return Err("max_per_range is 1 to 1000".to_owned());
        }
        if !(1..=Self::MAX_PER_DEVICE).contains(&self.per_device) {
            return Err(format!("per_device is 1 to {}", Self::MAX_PER_DEVICE));
        }
        if !(20..=MAX_MESSAGE as u32).contains(&self.message_max) {
            return Err(format!("message_max is 20 to {MAX_MESSAGE}"));
        }
        if !(1..=365).contains(&self.retention_days) {
            return Err("retention_days is 1 to 365".to_owned());
        }
        if self.category_ids.len() > MAX_CATEGORIES {
            return Err(format!("at most {MAX_CATEGORIES} categories"));
        }
        Ok(())
    }
}

pub async fn load_settings(pool: &MySqlPool, station_id: u64) -> ApiResult<DedicationSettings> {
    settings::load_for(pool, station_id, DedicationSettings::SETTINGS_NAME).await
}

async fn time_zone(pool: &MySqlPool, station_id: u64) -> ApiResult<TimeZone> {
    let station: StationSettings =
        settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await?;
    Ok(station.time_zone())
}

/// The Dedications' entry among the shows.
static DEDICATIONS: Show = core_shows::SHOWS[0];

fn dedications_show() -> &'static Show {
    Show::of(Module::Dedications).unwrap_or(&DEDICATIONS)
}

/// The station's dedication settings.
#[utoipa::path(get, path = "/dedications/settings", tag = "dedications",
    responses((status = 200, body = DedicationSettings), (status = "4XX", body = ErrorBody)))]
async fn get_dedication_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<DedicationSettings>> {
    Ok(Json(load_settings(&state.pool, station.id()).await?))
}

/// Replaces the station's dedication settings. The categories must be the
/// station's (`422`). Operators.
#[utoipa::path(put, path = "/dedications/settings", tag = "dedications",
    request_body = DedicationSettings,
    responses((status = 200, body = DedicationSettings), (status = "4XX", body = ErrorBody)))]
async fn put_dedication_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(mut settings): Json<DedicationSettings>,
) -> ApiResult<Json<DedicationSettings>> {
    user.require(Role::Operator)?;
    settings.check().map_err(ApiError::Invalid)?;
    settings.category_ids.sort_unstable();
    settings.category_ids.dedup();
    let pool = &state.pool;
    let ours: Vec<u64> =
        sqlx::query_scalar("SELECT category_id FROM category_stations WHERE station_id = ?")
            .bind(station.id())
            .fetch_all(pool)
            .await?;
    if let Some(id) = settings.category_ids.iter().find(|id| !ours.contains(id)) {
        return Err(ApiError::Invalid(format!(
            "category {id} is not one of the station's"
        )));
    }
    settings::save_for(
        pool,
        station.id(),
        DedicationSettings::SETTINGS_NAME,
        &settings,
    )
    .await?;
    tracing::info!(user = %user.username, station = station.id(), "dedication settings changed");
    Ok(Json(settings))
}

// ---------------------------------------------------------------------------
// Requestable tracks

/// The categories whose tracks can be requested; `None`: any track (a
/// station without categories rotates the whole library).
async fn requestable_categories(
    pool: &MySqlPool,
    station_id: u64,
    settings: &DedicationSettings,
) -> ApiResult<Option<Vec<u64>>> {
    if !settings.category_ids.is_empty() {
        return Ok(Some(settings.category_ids.clone()));
    }
    let rows: Vec<(u64, bool)> = sqlx::query_as(
        "SELECT c.id, COALESCE(ct.sends_metadata, TRUE) AS music
         FROM categories c
         JOIN category_stations cs ON cs.category_id = c.id AND cs.station_id = ?
         LEFT JOIN category_types ct ON ct.id = c.type_id",
    )
    .bind(station_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        rows.into_iter()
            .filter(|(_, music)| *music)
            .map(|(id, _)| id)
            .collect(),
    ))
}

/// The conditions of a requestable track `t`: one the station can play
/// now, in the requestable categories.
fn push_requestable(
    query: &mut QueryBuilder<sqlx::MySql>,
    station_id: u64,
    categories: &Option<Vec<u64>>,
) {
    query.push(
        " AND t.enabled AND t.missing_since IS NULL AND t.deleted_at IS NULL
          AND EXISTS (SELECT 1 FROM track_analysis a WHERE a.track_id = t.id)
          AND NOT EXISTS (SELECT 1 FROM medleys md WHERE md.track_id = t.id)
          AND NOT EXISTS (SELECT 1 FROM track_station_ratios r
                          WHERE r.track_id = t.id AND NOT r.enabled AND r.station_id = ",
    );
    query.push_bind(station_id).push(")");
    match categories {
        None => {}
        Some(ids) if ids.is_empty() => {
            query.push(" AND FALSE");
        }
        Some(ids) => {
            query.push(
                " AND EXISTS (SELECT 1 FROM track_categories tc
                              WHERE tc.track_id = t.id AND tc.category_id IN (",
            );
            let mut separated = query.separated(", ");
            for id in ids {
                separated.push_bind(*id);
            }
            query.push("))");
        }
    }
}

/// A track a listener can request.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct RequestableTrack {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
}

/// `text` for `LIKE`, its wildcards taken as they are.
fn like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 2);
    escaped.push('%');
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push('%');
    escaped
}

async fn search(
    pool: &MySqlPool,
    station_id: u64,
    settings: &DedicationSettings,
    text: &str,
) -> ApiResult<Vec<RequestableTrack>> {
    let categories = requestable_categories(pool, station_id, settings).await?;
    let pattern = like(text);
    let mut query = QueryBuilder::new(
        "SELECT t.id AS track_id, t.artist, t.title FROM tracks t
         WHERE (t.artist LIKE ",
    );
    query
        .push_bind(pattern.clone())
        .push(" OR t.title LIKE ")
        .push_bind(pattern)
        .push(")");
    push_requestable(&mut query, station_id, &categories);
    query
        .push(" ORDER BY t.artist, t.title, t.id LIMIT ")
        .push_bind(SEARCH_RESULTS);
    Ok(query.build_query_as().fetch_all(pool).await?)
}

async fn is_requestable(
    pool: &MySqlPool,
    station_id: u64,
    settings: &DedicationSettings,
    track_id: u64,
) -> ApiResult<bool> {
    let categories = requestable_categories(pool, station_id, settings).await?;
    let mut query = QueryBuilder::new("SELECT t.id FROM tracks t WHERE t.id = ");
    query.push_bind(track_id);
    push_requestable(&mut query, station_id, &categories);
    let found: Option<u64> = query.build_query_scalar().fetch_optional(pool).await?;
    Ok(found.is_some())
}

// ---------------------------------------------------------------------------
// Requests

/// Where a request stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DedicationStatus {
    Pending,
    Accepted,
    Played,
    Refused,
}

impl DedicationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Played => "played",
            Self::Refused => "refused",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "accepted" => Self::Accepted,
            "played" => Self::Played,
            "refused" => Self::Refused,
            _ => Self::Pending,
        }
    }
}

/// Why a request was refused, or did not play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DedicationReason {
    /// The track just played.
    JustPlayed,
    /// The track cannot play now.
    Unavailable,
    /// Never told the listener.
    Inappropriate,
    /// Still waiting when the range closed.
    Closed,
    /// Accepted, then taken out of the queue before it played.
    Removed,
}

impl DedicationReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::JustPlayed => "just_played",
            Self::Unavailable => "unavailable",
            Self::Inappropriate => "inappropriate",
            Self::Closed => "closed",
            Self::Removed => "removed",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "just_played" => Self::JustPlayed,
            "unavailable" => Self::Unavailable,
            "inappropriate" => Self::Inappropriate,
            "closed" => Self::Closed,
            "removed" => Self::Removed,
            _ => return None,
        })
    }
}

/// A request as it is stored.
#[derive(Clone, Debug, sqlx::FromRow)]
struct Row {
    id: u64,
    follow_key: String,
    track_id: Option<u64>,
    artist: Option<String>,
    title: Option<String>,
    from_name: String,
    to_name: String,
    message: String,
    status: String,
    reason: Option<String>,
    submitted_ms: i64,
    decided_ms: Option<i64>,
    decided_by: Option<String>,
    planned_ms: Option<i64>,
    played_ms: Option<i64>,
    play_id: Option<u64>,
}

const COLUMNS: &str = "d.id, d.follow_key, d.track_id, t.artist, t.title, d.from_name, d.to_name,
    d.message, CAST(d.status AS CHAR) AS status, CAST(d.reason AS CHAR) AS reason,
    CAST(UNIX_TIMESTAMP(d.submitted_at) * 1000 AS SIGNED) AS submitted_ms,
    CAST(UNIX_TIMESTAMP(d.decided_at) * 1000 AS SIGNED) AS decided_ms,
    u.username AS decided_by,
    CAST(UNIX_TIMESTAMP(d.planned_at) * 1000 AS SIGNED) AS planned_ms,
    CAST(UNIX_TIMESTAMP(d.played_at) * 1000 AS SIGNED) AS played_ms,
    d.play_id";
const FROM: &str = "FROM dedications d LEFT JOIN tracks t ON t.id = d.track_id
    LEFT JOIN users u ON u.id = d.decided_by";

/// What the rules say about a waiting request now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DedicationCheckKind {
    RulesKept,
    ArtistPlayed,
    TitlePlayed,
    TrackPlayed,
    InQueue,
    Restriction,
    Unplayable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct DedicationCheck {
    pub kind: DedicationCheckKind,
    /// How long ago it played (the separation rules).
    pub minutes_ago: Option<i64>,
    /// The rule's window.
    pub separation_min: Option<u32>,
    /// When the track queued already plays.
    pub at_unix_ms: Option<i64>,
    pub restriction_id: Option<u64>,
    /// It cannot be accepted: a restriction forbids it, or it cannot play.
    pub blocking: bool,
}

impl DedicationCheck {
    fn of(kind: DedicationCheckKind) -> Self {
        Self {
            kind,
            minutes_ago: None,
            separation_min: None,
            at_unix_ms: None,
            restriction_id: None,
            blocking: matches!(
                kind,
                DedicationCheckKind::Restriction | DedicationCheckKind::Unplayable
            ),
        }
    }
}

/// A request, for operators.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Dedication {
    pub id: u64,
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub from: String,
    pub to: String,
    pub message: String,
    pub status: DedicationStatus,
    pub reason: Option<DedicationReason>,
    pub submitted_at_unix_ms: i64,
    pub decided_at_unix_ms: Option<i64>,
    /// Who accepted or refused it.
    pub decided_by: Option<String>,
    pub planned_at_unix_ms: Option<i64>,
    pub played_at_unix_ms: Option<i64>,
    pub play_id: Option<u64>,
    /// While it waits: what the rules say about it now.
    pub checks: Vec<DedicationCheck>,
}

impl From<Row> for Dedication {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            track_id: row.track_id,
            artist: row.artist,
            title: row.title,
            from: row.from_name,
            to: row.to_name,
            message: row.message,
            status: DedicationStatus::parse(&row.status),
            reason: row.reason.as_deref().and_then(DedicationReason::parse),
            submitted_at_unix_ms: row.submitted_ms,
            decided_at_unix_ms: row.decided_ms,
            decided_by: row.decided_by,
            planned_at_unix_ms: row.planned_ms,
            played_at_unix_ms: row.played_ms,
            play_id: row.play_id,
            checks: Vec::new(),
        }
    }
}

/// A request, as its follow link gives it to the listener.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Followed {
    /// The key of the follow link.
    pub follow: String,
    pub status: DedicationStatus,
    /// `null` for no reason, or an inappropriate message.
    pub reason: Option<DedicationReason>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub from: String,
    pub to: String,
    pub message: String,
    pub submitted_at_unix_ms: i64,
    pub decided_at_unix_ms: Option<i64>,
    pub planned_at_unix_ms: Option<i64>,
    pub played_at_unix_ms: Option<i64>,
}

impl From<Row> for Followed {
    fn from(row: Row) -> Self {
        Self {
            follow: row.follow_key,
            status: DedicationStatus::parse(&row.status),
            reason: row
                .reason
                .as_deref()
                .and_then(DedicationReason::parse)
                .filter(|reason| *reason != DedicationReason::Inappropriate),
            artist: row.artist,
            title: row.title,
            from: row.from_name,
            to: row.to_name,
            message: row.message,
            submitted_at_unix_ms: row.submitted_ms,
            decided_at_unix_ms: row.decided_ms,
            planned_at_unix_ms: row.planned_ms,
            played_at_unix_ms: row.played_ms,
        }
    }
}

/// The dedication of a track on air.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema, sqlx::FromRow)]
pub struct OnAirDedication {
    pub from: String,
    pub to: String,
    pub message: String,
}

/// The opening range `now` is in: when it opened and closes. Hours open
/// all week long count a day at a time, from midnight.
fn opening(
    week: &core_shows::Week,
    tz: &TimeZone,
    now: Timestamp,
) -> Option<(Timestamp, Timestamp)> {
    if let Some(range) = core_shows::opening(week, tz, now) {
        return Some(range);
    }
    if !core_shows::status(week, tz, now).open {
        return None;
    }
    let today = now.to_zoned(tz.clone()).start_of_day().ok()?;
    let tomorrow = today.tomorrow().ok()?.start_of_day().ok()?;
    Some((today.timestamp(), tomorrow.timestamp()))
}

/// The opening range on at `now`, else the last one: when it opened and
/// closes. `None` for a show without hours.
fn range_of(
    settings: &core_shows::ShowSettings,
    tz: &TimeZone,
    now: Timestamp,
) -> Option<(Timestamp, Timestamp)> {
    let week = core_shows::Week::of(&settings.hours);
    if let Some(range) = opening(&week, tz, now) {
        return Some(range);
    }
    let start = core_shows::last_opening(&week, tz, now)?;
    opening(&week, tz, start)
}

// ---------------------------------------------------------------------------
// Rules

/// The first separation rule `track` breaks at `at_ms` against `plays`
/// (before and after it), with how long before `at_ms` the play was.
fn separation(
    library: &Library,
    plays: &[Play],
    track: u64,
    at_ms: i64,
) -> Option<DedicationCheck> {
    let found = library.tracks.get(&track)?;
    let windows = library.settings.separation;
    let mut broken: Option<(DedicationCheckKind, i64, u32)> = None;
    for play in plays {
        let gap = at_ms - play.start_ms;
        let within = |window: u32| window > 0 && gap.abs() < i64::from(window) * 60_000;
        let kind = if play.track == Some(track) && within(windows.track_min) {
            Some((DedicationCheckKind::TrackPlayed, windows.track_min))
        } else if found.title_key.is_some()
            && play.title_key == found.title_key
            && within(windows.title_min)
        {
            Some((DedicationCheckKind::TitlePlayed, windows.title_min))
        } else if within(windows.artist_min)
            && found.artists.iter().any(|artist| {
                play.artists
                    .iter()
                    .any(|other| library.same_artist(*artist, *other))
            })
        {
            Some((DedicationCheckKind::ArtistPlayed, windows.artist_min))
        } else {
            None
        };
        // The closest play breaking a rule.
        if let Some((kind, window)) = kind
            && broken.is_none_or(|(_, before, _)| gap.abs() < before.abs())
        {
            broken = Some((kind, gap, window));
        }
    }
    broken.map(|(kind, gap, window)| DedicationCheck {
        minutes_ago: Some(gap / 60_000),
        separation_min: Some(window),
        ..DedicationCheck::of(kind)
    })
}

/// The restriction that forbids `track` at `at_ms`, after a track of
/// `previous` categories.
fn restricted(
    library: &Library,
    track: u64,
    at_ms: i64,
    previous: Option<&[u64]>,
) -> Option<DedicationCheck> {
    let Some(found) = library.tracks.get(&track) else {
        return Some(DedicationCheck::of(DedicationCheckKind::Unplayable));
    };
    let subject = Subject {
        track,
        artists: &found.artists,
        categories: &found.categories,
    };
    forbidding(
        &library.restrictions,
        &subject,
        library.local(at_ms).datetime(),
        previous,
    )
    .map(|restriction| DedicationCheck {
        restriction_id: Some(restriction.id),
        ..DedicationCheck::of(DedicationCheckKind::Restriction)
    })
}

/// What the station knows to check requests: its library, what played, the
/// queue at its planned times.
struct Context {
    library: Library,
    /// What played.
    past: Vec<Play>,
    /// What played and what is queued, at its planned times.
    history: History,
    /// The waiting entries and when each starts; then when a track added
    /// at the end would.
    planned: Vec<(QueueEntry, i64)>,
    end_ms: i64,
    now_ms: i64,
}

impl Context {
    async fn load(
        pool: &MySqlPool,
        station_id: u64,
        state: Option<&PlayerState>,
    ) -> ApiResult<Self> {
        let library = crate::rotation::load_library(pool, station_id).await?;
        let now_ms = crate::rotation::now_ms();
        let history = crate::rotation::load_history(pool, station_id, &library, now_ms).await?;
        let past = history.plays().to_vec();
        let (planned, end_ms) = match state {
            Some(state) => {
                let (planned, end) = crate::rotation::planned_starts(state, now_ms);
                let history =
                    crate::rotation::with_queue(pool, &library, history.clone(), &planned).await?;
                let planned = planned
                    .into_iter()
                    .map(|(entry, at)| (entry.clone(), at))
                    .collect();
                return Ok(Self {
                    library,
                    past,
                    history,
                    planned,
                    end_ms: end,
                    now_ms,
                });
            }
            None => (Vec::new(), now_ms),
        };
        Ok(Self {
            library,
            past,
            history,
            planned,
            end_ms,
            now_ms,
        })
    }

    /// When a track put at `position` among the waiting ones would start.
    fn start_at(&self, position: usize) -> i64 {
        self.planned
            .get(position)
            .map_or(self.end_ms, |(_, at)| *at)
    }

    /// The categories of the track before `position`.
    fn previous(&self, position: usize) -> Option<&[u64]> {
        self.history
            .previous(self.start_at(position))
            .map(|play| play.categories.as_slice())
    }

    /// What the rules say about `track` now, as if it played next.
    fn checks(&self, track: u64) -> Vec<DedicationCheck> {
        let mut checks = Vec::new();
        if let Some(blocking) = restricted(&self.library, track, self.start_at(0), self.previous(0))
        {
            checks.push(blocking);
        }
        if let Some((_, at)) = self
            .planned
            .iter()
            .find(|(entry, _)| entry.track.track_id == Some(track))
        {
            checks.push(DedicationCheck {
                at_unix_ms: Some(*at),
                ..DedicationCheck::of(DedicationCheckKind::InQueue)
            });
        }
        if let Some(broken) = separation(&self.library, &self.past, track, self.now_ms) {
            checks.push(broken);
        }
        if checks.is_empty() {
            checks.push(DedicationCheck::of(DedicationCheckKind::RulesKept));
        }
        checks
    }
}

/// Where an accepted request goes in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DedicationPlace {
    /// Right after the track on air.
    Next,
    /// At the end of what is queued now, before the show closes.
    Hour,
    /// The first place where the separation rules hold, else within the
    /// hour.
    Best,
}

/// The position among the waiting tracks for `place`: 0 is next.
fn position(context: &Context, track: u64, place: DedicationPlace, closes_ms: i64) -> usize {
    let count = context.planned.len();
    let within = |position: usize| position == 0 || context.start_at(position) < closes_ms;
    let hour = (0..=count).rev().find(|p| within(*p)).unwrap_or(0);
    match place {
        DedicationPlace::Next => 0,
        DedicationPlace::Hour => hour,
        DedicationPlace::Best => (0..=count)
            .filter(|p| within(*p))
            .find(|p| {
                let at = context.start_at(*p);
                separation(&context.library, context.history.plays(), track, at).is_none()
                    && restricted(&context.library, track, at, context.previous(*p)).is_none()
            })
            .unwrap_or(hour),
    }
}

// ---------------------------------------------------------------------------
// Operators

#[derive(Clone, Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Only the requests of this status.
    pub status: Option<DedicationStatus>,
    /// 1 to 200, 100 by default.
    pub limit: Option<u32>,
}

/// The counts of the range's requests.
#[derive(Clone, Debug, Default, Serialize, ToSchema)]
pub struct DedicationCounts {
    pub pending: u64,
    pub accepted: u64,
    pub played: u64,
    pub refused: u64,
    pub received: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DedicationList {
    pub opens_at_unix_ms: Option<i64>,
    pub closes_at_unix_ms: Option<i64>,
    pub max_per_range: u32,
    pub counts: DedicationCounts,
    pub dedications: Vec<Dedication>,
    /// The dedication of the track playing.
    pub on_air: Option<OnAirDedication>,
}

/// The requests of the opening range on now, else of the last one, the
/// latest first; the waiting ones with what the rules say about them.
#[utoipa::path(get, path = "/dedications", tag = "dedications",
    params(ListQuery),
    responses((status = 200, body = DedicationList), (status = "4XX", body = ErrorBody)))]
async fn list_dedications(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<DedicationList>> {
    let pool = &state.pool;
    let settings = load_settings(pool, station.id()).await?;
    let show = crate::shows::load(pool, station.id(), dedications_show()).await?;
    let tz = time_zone(pool, station.id()).await?;
    let range = range_of(&show, &tz, Timestamp::now());
    let limit = query.limit.unwrap_or(100).clamp(1, MAX_LIST);
    let mut select = QueryBuilder::new(format!("SELECT {COLUMNS} {FROM} WHERE d.station_id = "));
    select.push_bind(station.id());
    if let Some((opens, _)) = range {
        select
            .push(" AND d.opens_at = FROM_UNIXTIME(")
            .push_bind(seconds(opens))
            .push(")");
    }
    if let Some(status) = query.status {
        select.push(" AND d.status = ").push_bind(status.as_str());
    }
    select
        .push(" ORDER BY d.submitted_at DESC, d.id DESC LIMIT ")
        .push_bind(limit);
    let rows: Vec<Row> = select.build_query_as().fetch_all(pool).await?;
    let mut dedications: Vec<Dedication> = rows.into_iter().map(Dedication::from).collect();
    if dedications
        .iter()
        .any(|d| d.status == DedicationStatus::Pending)
    {
        let player = station.engine.state().borrow().clone();
        let context = Context::load(pool, station.id(), player.as_deref()).await?;
        for dedication in &mut dedications {
            if let (DedicationStatus::Pending, Some(track)) =
                (dedication.status, dedication.track_id)
            {
                dedication.checks = context.checks(track);
            } else if dedication.status == DedicationStatus::Pending {
                dedication.checks = vec![DedicationCheck::of(DedicationCheckKind::Unplayable)];
            }
        }
    }
    let mut counts = DedicationCounts::default();
    if let Some((opens, _)) = range {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT CAST(status AS CHAR), COUNT(*) FROM dedications
             WHERE station_id = ? AND opens_at = FROM_UNIXTIME(?) GROUP BY status",
        )
        .bind(station.id())
        .bind(seconds(opens))
        .fetch_all(pool)
        .await?;
        for (status, count) in rows {
            let count = u64::try_from(count).unwrap_or(0);
            counts.received += count;
            match DedicationStatus::parse(&status) {
                DedicationStatus::Pending => counts.pending = count,
                DedicationStatus::Accepted => counts.accepted = count,
                DedicationStatus::Played => counts.played = count,
                DedicationStatus::Refused => counts.refused = count,
            }
        }
    }
    let player = station.engine.state().borrow().clone();
    let on_air = match player {
        Some(player) => on_air(pool, station.id(), &player).await?,
        None => None,
    };
    Ok(Json(DedicationList {
        opens_at_unix_ms: range.map(|(opens, _)| opens.as_millisecond()),
        closes_at_unix_ms: range.map(|(_, closes)| closes.as_millisecond()),
        max_per_range: settings.max_per_range,
        counts,
        dedications,
        on_air,
    }))
}

async fn row(pool: &MySqlPool, station_id: u64, id: u64) -> ApiResult<Row> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} {FROM} WHERE d.station_id = ? AND d.id = ?"
    )))
    .bind(station_id)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| ApiError::not_found(format!("dedication {id}")))
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptDedication {
    pub place: DedicationPlace,
}

/// Queues the request's track (source `request`) at the place asked and
/// answers the request. `409` for a request not waiting, or a track that
/// a restriction forbids then or that cannot play. Operators.
#[utoipa::path(post, path = "/dedications/{id}/accept", tag = "dedications",
    params(("id" = u64, Path, description = "The request's ID")),
    request_body = AcceptDedication,
    responses((status = 200, body = Dedication), (status = "4XX", body = ErrorBody),
              (status = 503, body = ErrorBody)))]
async fn accept_dedication(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(request): Json<AcceptDedication>,
) -> ApiResult<Json<Dedication>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let found = row(pool, station.id(), id).await?;
    if DedicationStatus::parse(&found.status) != DedicationStatus::Pending {
        return Err(ApiError::Conflict(format!(
            "dedication {id} is not waiting any more"
        )));
    }
    let track = found
        .track_id
        .ok_or_else(|| ApiError::Conflict("the track is not in the library any more".to_owned()))?;
    let player = station.engine.fetch_state().await?;
    let context = Context::load(pool, station.id(), Some(&player)).await?;
    let show = crate::shows::load(pool, station.id(), dedications_show()).await?;
    let tz = time_zone(pool, station.id()).await?;
    let closes_ms = range_of(&show, &tz, Timestamp::now())
        .map_or(context.end_ms, |(_, closes)| closes.as_millisecond());
    let position = position(&context, track, request.place, closes_ms);
    let at = context.start_at(position);
    if let Some(check) = restricted(&context.library, track, at, context.previous(position)) {
        return Err(ApiError::Conflict(match check.restriction_id {
            Some(restriction) => format!("restriction {restriction} forbids the track then"),
            None => "the track cannot play: disabled, missing or not analyzed".to_owned(),
        }));
    }
    // Taken first, so that two operators do not queue it twice.
    let claimed = sqlx::query(
        "UPDATE dedications SET status = 'accepted', decided_at = NOW(6), decided_by = ?,
                planned_at = FROM_UNIXTIME(?)
         WHERE id = ? AND station_id = ? AND status = 'pending'",
    )
    .bind(user.id)
    .bind(at as f64 / 1_000.0)
    .bind(id)
    .bind(station.id())
    .execute(pool)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Err(ApiError::Conflict(format!(
            "dedication {id} is not waiting any more"
        )));
    }
    let queued = async {
        let tracks = station.queue.tracks(&[track], PlaySource::Request).await?;
        let position = u32::try_from(position).unwrap_or(u32::MAX);
        station
            .engine
            .enqueue(tracks, Some(position), Vec::new())
            .await
    }
    .await;
    let entry = match queued {
        Ok(entries) => entries.first().copied(),
        Err(error) => {
            sqlx::query(
                "UPDATE dedications SET status = 'pending', decided_at = NULL, decided_by = NULL,
                        planned_at = NULL
                 WHERE id = ?",
            )
            .bind(id)
            .execute(pool)
            .await?;
            return Err(error);
        }
    };
    sqlx::query("UPDATE dedications SET engine_started_at = ?, entry_id = ? WHERE id = ?")
        .bind(player.engine_started_at_unix_ms)
        .bind(entry)
        .bind(id)
        .execute(pool)
        .await?;
    tracing::info!(user = %user.username, station = station.id(), dedication = id, track, position, "dedication accepted");
    let accepted = row(pool, station.id(), id).await?;
    let values = placeholders(&accepted, at, &tz);
    let values: Vec<(&str, String)> = values.iter().map(|(k, v)| (*k, v.clone())).collect();
    if let Err(error) = crate::shows::send_own_event(
        pool,
        &state.webhooks,
        station.id(),
        &station.code(),
        dedications_show(),
        WebhookEvent::DedicationAccepted,
        Timestamp::now(),
        &values,
    )
    .await
    {
        tracing::warn!(%error, "cannot send dedication.accepted");
    }
    Ok(Json(accepted.into()))
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefuseDedication {
    /// `null`: no reason. `closed` and `removed` are Heron's own.
    pub reason: Option<DedicationReason>,
}

/// Refuses a waiting request, with a reason or none. `409` for a request
/// not waiting. Operators.
#[utoipa::path(post, path = "/dedications/{id}/refuse", tag = "dedications",
    params(("id" = u64, Path, description = "The request's ID")),
    request_body = RefuseDedication,
    responses((status = 200, body = Dedication), (status = "4XX", body = ErrorBody)))]
async fn refuse_dedication(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(request): Json<RefuseDedication>,
) -> ApiResult<Json<Dedication>> {
    user.require(Role::Operator)?;
    if matches!(
        request.reason,
        Some(DedicationReason::Closed | DedicationReason::Removed)
    ) {
        return Err(ApiError::Invalid(
            "the reason is just_played, unavailable, inappropriate or null".to_owned(),
        ));
    }
    let pool = &state.pool;
    row(pool, station.id(), id).await?;
    let refused = sqlx::query(
        "UPDATE dedications SET status = 'refused', reason = ?, decided_at = NOW(6), decided_by = ?
         WHERE id = ? AND station_id = ? AND status = 'pending'",
    )
    .bind(request.reason.map(DedicationReason::as_str))
    .bind(user.id)
    .bind(id)
    .bind(station.id())
    .execute(pool)
    .await?
    .rows_affected();
    if refused == 0 {
        return Err(ApiError::Conflict(format!(
            "dedication {id} is not waiting any more"
        )));
    }
    tracing::info!(user = %user.username, station = station.id(), dedication = id, "dedication refused");
    Ok(Json(row(pool, station.id(), id).await?.into()))
}

/// The placeholders of a dedication's messages.
fn placeholders(row: &Row, at_ms: i64, tz: &TimeZone) -> Vec<(&'static str, String)> {
    let time = Timestamp::from_millisecond(at_ms)
        .map(|at| core_shows::hour_text(at, tz))
        .unwrap_or_default();
    vec![
        ("artist", row.artist.clone().unwrap_or_default()),
        ("title", row.title.clone().unwrap_or_default()),
        ("from", row.from_name.clone()),
        ("to", row.to_name.clone()),
        ("message", row.message.clone()),
        ("time", time),
    ]
}

// ---------------------------------------------------------------------------
// The gateway

/// What a listener sends.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DedicationFields {
    pub track_id: u64,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub message: Option<String>,
}

fn random_key() -> ApiResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(internal)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// A trimmed text of 1 to `max` characters (0 allowed when `empty`).
fn text(value: &str, name: &str, max: usize, empty: bool) -> ApiResult<String> {
    let value = value.trim();
    let count = value.chars().count();
    if (count == 0 && !empty) || count > max {
        return Err(ApiError::Invalid(if empty {
            format!("{name} is {max} characters at most")
        } else {
            format!("{name} is 1 to {max} characters")
        }));
    }
    if value.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ApiError::Invalid(format!("{name} has control characters")));
    }
    Ok(value.to_owned())
}

/// A listener's request, from the gateway, with the device's hash: one
/// waiting at a time per device, a few per device and a maximum per
/// opening range. Answers its follow link.
#[utoipa::path(post, path = "/gateway/stations/{code}/dedications", tag = "gateway", security(()),
    params(("code" = String, Path, description = "The station's code")),
    request_body = DedicationFields,
    responses((status = 201, body = Followed), (status = "4XX", body = ErrorBody)))]
async fn gateway_submit(
    gateway: GatewayClient,
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(fields): Json<DedicationFields>,
) -> ApiResult<(StatusCode, Json<Followed>)> {
    let (station, show) = crate::gateway::open_show(&state, &code, Module::Dedications).await?;
    let pool = &state.pool;
    let device = gateway
        .device
        .filter(|device| !device.is_empty())
        .ok_or_else(|| ApiError::Invalid("the device is missing".to_owned()))?;
    let settings = load_settings(pool, station.id()).await?;
    let from = text(&fields.from, "from", MAX_FROM, false)?;
    let to = text(&fields.to, "to", MAX_TO, false)?;
    let message = text(
        fields.message.as_deref().unwrap_or_default(),
        "message",
        settings.message_max as usize,
        true,
    )?;
    if !is_requestable(pool, station.id(), &settings, fields.track_id).await? {
        return Err(ApiError::Invalid(
            "this track cannot be requested".to_owned(),
        ));
    }
    let tz = time_zone(pool, station.id()).await?;
    let now = Timestamp::now();
    let (opens, closes) = opening(&show.open_week(), &tz, now)
        .ok_or_else(|| ApiError::Conflict("Dedications is closed".to_owned()))?;
    let (received, from_device, waiting): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), CAST(COALESCE(SUM(device = ?), 0) AS SIGNED),
                CAST(COALESCE(SUM(device = ? AND status = 'pending'), 0) AS SIGNED)
         FROM dedications WHERE station_id = ? AND opens_at = FROM_UNIXTIME(?)",
    )
    .bind(&device)
    .bind(&device)
    .bind(station.id())
    .bind(seconds(opens))
    .fetch_one(pool)
    .await?;
    if received >= i64::from(settings.max_per_range) {
        return Err(ApiError::Conflict(
            "full for today: no more requests in this range".to_owned(),
        ));
    }
    if waiting > 0 || from_device >= i64::from(settings.per_device) {
        return Err(ApiError::TooManyRequests);
    }
    let key = random_key()?;
    sqlx::query(
        "INSERT INTO dedications (station_id, follow_key, track_id, from_name, to_name, message,
                                  device, submitted_at, opens_at, closes_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, NOW(6), FROM_UNIXTIME(?), FROM_UNIXTIME(?))",
    )
    .bind(station.id())
    .bind(&key)
    .bind(fields.track_id)
    .bind(&from)
    .bind(&to)
    .bind(&message)
    .bind(&device)
    .bind(seconds(opens))
    .bind(seconds(closes))
    .execute(pool)
    .await?;
    tracing::info!(station = %code, track = fields.track_id, "dedication requested");
    Ok((
        StatusCode::CREATED,
        Json(followed(pool, station.id(), &key).await?),
    ))
}

async fn followed(pool: &MySqlPool, station_id: u64, key: &str) -> ApiResult<Followed> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} {FROM} WHERE d.station_id = ? AND d.follow_key = ?"
    )))
    .bind(station_id)
    .bind(key)
    .fetch_optional(pool)
    .await?;
    row.map(Followed::from)
        .ok_or_else(|| ApiError::not_found("dedication"))
}

/// An enabled station with Dedications on, open or not.
async fn station_on(state: &AppState, code: &str) -> ApiResult<Arc<StationRuntime>> {
    let station = state
        .stations
        .by_code(code)
        .filter(|station| station.info().enabled)
        .ok_or_else(|| ApiError::not_found(format!("station {code}")))?;
    if !crate::modules::is_on(&state.pool, Some(station.id()), Module::Dedications).await? {
        return Err(ApiError::ModuleOff(Module::Dedications));
    }
    Ok(station)
}

#[derive(Clone, Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// 2 to 60 characters of the artist or the title.
    pub q: String,
}

/// The requestable tracks whose artist or title has `q`, 8 at most. `409`
/// while the show is closed.
#[utoipa::path(get, path = "/gateway/stations/{code}/dedications/search", tag = "gateway", security(()),
    params(("code" = String, Path, description = "The station's code"), SearchQuery),
    responses((status = 200, body = Vec<RequestableTrack>), (status = "4XX", body = ErrorBody)))]
async fn gateway_search(
    _gateway: GatewayClient,
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Vec<RequestableTrack>>> {
    let (station, _) = crate::gateway::open_show(&state, &code, Module::Dedications).await?;
    let text = query.q.trim();
    if !(MIN_QUERY..=MAX_QUERY).contains(&text.chars().count()) {
        return Err(ApiError::Invalid(format!(
            "q is {MIN_QUERY} to {MAX_QUERY} characters"
        )));
    }
    let settings = load_settings(&state.pool, station.id()).await?;
    Ok(Json(
        search(&state.pool, station.id(), &settings, text).await?,
    ))
}

#[derive(Clone, Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FollowQuery {
    /// The key of the follow link.
    pub id: String,
}

/// A request, by the key of its follow link. `404` once deleted.
#[utoipa::path(get, path = "/gateway/stations/{code}/dedications/follow", tag = "gateway", security(()),
    params(("code" = String, Path, description = "The station's code"), FollowQuery),
    responses((status = 200, body = Followed), (status = "4XX", body = ErrorBody)))]
async fn gateway_follow(
    _gateway: GatewayClient,
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(query): Query<FollowQuery>,
) -> ApiResult<Json<Followed>> {
    let station = station_on(&state, &code).await?;
    if query.id.len() != 32 || !query.id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::not_found("dedication"));
    }
    Ok(Json(followed(&state.pool, station.id(), &query.id).await?))
}

// ---------------------------------------------------------------------------
// On air and on the website

/// The dedication of a queue entry: an accepted request queued as it.
pub async fn of_entry(
    pool: &MySqlPool,
    station_id: u64,
    entry_id: u64,
    track_id: Option<u64>,
) -> ApiResult<Option<(u64, OnAirDedication)>> {
    let found: Option<(u64, String, String, String)> = sqlx::query_as(
        "SELECT id, from_name, to_name, message FROM dedications
         WHERE station_id = ? AND entry_id = ? AND track_id <=> ?
           AND status IN ('accepted', 'played') AND decided_at >= NOW(6) - INTERVAL 1 DAY
         ORDER BY id DESC LIMIT 1",
    )
    .bind(station_id)
    .bind(entry_id)
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    Ok(found.map(|(id, from, to, message)| (id, OnAirDedication { from, to, message })))
}

/// The dedication of the track on air, while the module is on.
pub async fn on_air(
    pool: &MySqlPool,
    station_id: u64,
    player: &PlayerState,
) -> ApiResult<Option<OnAirDedication>> {
    let Some(entry) = player
        .decks
        .iter()
        .filter(|deck| deck.on_air)
        .find_map(|deck| deck.entry.as_ref())
        .filter(|entry| entry.track.source == PlaySource::Request)
    else {
        return Ok(None);
    };
    if !crate::modules::is_on(pool, Some(station_id), Module::Dedications).await? {
        return Ok(None);
    }
    Ok(
        of_entry(pool, station_id, entry.entry_id, entry.track.track_id)
            .await?
            .map(|(_, dedication)| dedication),
    )
}

/// Links an accepted request to the play of its queue entry: it played.
/// Returns the request's ID.
pub async fn link_play(
    pool: &MySqlPool,
    station_id: u64,
    engine_started_at: u64,
    entry_id: u64,
    play_id: u64,
) -> ApiResult<Option<u64>> {
    let id: Option<u64> = sqlx::query_scalar(
        "SELECT id FROM dedications
         WHERE station_id = ? AND engine_started_at = ? AND entry_id = ? AND status = 'accepted'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(station_id)
    .bind(engine_started_at)
    .bind(entry_id)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = id {
        sqlx::query(
            "UPDATE dedications d JOIN play_history h ON h.id = ?
             SET d.status = 'played', d.play_id = h.id, d.played_at = h.started_at
             WHERE d.id = ?",
        )
        .bind(play_id)
        .bind(id)
        .execute(pool)
        .await?;
    }
    Ok(id)
}

/// Sends `dedication.played` for a request whose track starts.
pub async fn notify_played(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station_id: u64,
    dedication_id: u64,
    at_ms: i64,
) -> ApiResult<usize> {
    let code: String = sqlx::query_scalar("SELECT code FROM stations WHERE id = ?")
        .bind(station_id)
        .fetch_one(pool)
        .await?;
    let found = row(pool, station_id, dedication_id).await?;
    let tz = time_zone(pool, station_id).await?;
    let values = placeholders(&found, at_ms, &tz);
    crate::shows::send_own_event(
        pool,
        webhooks,
        station_id,
        &code,
        dedications_show(),
        WebhookEvent::DedicationPlayed,
        timestamp(at_ms)?,
        &values,
    )
    .await
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
struct PublicDedication {
    artist: Option<String>,
    title: Option<String>,
    from: String,
    to: String,
    message: String,
    /// When it played, or is planned.
    at_unix_ms: Option<i64>,
    played: bool,
}

/// The show's live state for the gateway: the requests left in the range
/// and today's accepted dedications. Waiting and refused ones never show.
pub async fn public_state(
    pool: &MySqlPool,
    station_id: u64,
    show: &core_shows::ShowSettings,
    timezone: &str,
) -> ApiResult<serde_json::Value> {
    let settings = load_settings(pool, station_id).await?;
    let tz = TimeZone::get(timezone).unwrap_or(TimeZone::UTC);
    let now = Timestamp::now();
    let remaining = match opening(&show.open_week(), &tz, now) {
        Some((opens, _)) => {
            let received: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM dedications WHERE station_id = ? AND opens_at = FROM_UNIXTIME(?)",
            )
            .bind(station_id)
            .bind(seconds(opens))
            .fetch_one(pool)
            .await?;
            Some(
                i64::from(settings.max_per_range)
                    .saturating_sub(received)
                    .max(0),
            )
        }
        None => None,
    };
    let midnight = now
        .to_zoned(tz.clone())
        .start_of_day()
        .map_err(internal)?
        .timestamp();
    let today: Vec<PublicDedication> = sqlx::query_as(
        "SELECT t.artist, t.title, d.from_name AS `from`, d.to_name AS `to`, d.message,
                CAST(UNIX_TIMESTAMP(COALESCE(d.played_at, d.planned_at)) * 1000 AS SIGNED) AS at_unix_ms,
                d.status = 'played' AS played
         FROM dedications d LEFT JOIN tracks t ON t.id = d.track_id
         WHERE d.station_id = ? AND d.status IN ('accepted', 'played')
           AND COALESCE(d.played_at, d.planned_at, d.decided_at) >= FROM_UNIXTIME(?)
         ORDER BY COALESCE(d.played_at, d.planned_at), d.id
         LIMIT 50",
    )
    .bind(station_id)
    .bind(seconds(midnight))
    .fetch_all(pool)
    .await?;
    Ok(serde_json::json!({
        "remaining": remaining,
        "message_max": settings.message_max,
        "per_device": settings.per_device,
        "retention_days": settings.retention_days,
        "today": today,
    }))
}

// ---------------------------------------------------------------------------
// Background

/// Refuses the requests still waiting when their range closed, marks the
/// accepted ones taken out of the queue, clears the devices' hashes of
/// closed ranges and deletes the requests past their station's retention.
/// The privacy rules apply whether the module is on or not.
pub(crate) async fn tick(pool: &MySqlPool, stations: &Stations) -> ApiResult<()> {
    sqlx::query(
        "UPDATE dedications SET status = 'refused', reason = 'closed', decided_at = NOW(6)
         WHERE status = 'pending' AND closes_at <= NOW(6)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE dedications SET device = NULL WHERE device IS NOT NULL AND closes_at <= NOW(6)",
    )
    .execute(pool)
    .await?;
    for station in stations.all() {
        let settings = load_settings(pool, station.id()).await?;
        let purged = sqlx::query(
            "DELETE FROM dedications
             WHERE station_id = ? AND submitted_at < NOW(6) - INTERVAL ? DAY",
        )
        .bind(station.id())
        .bind(settings.retention_days)
        .execute(pool)
        .await?
        .rows_affected();
        if purged > 0 {
            tracing::info!(station = station.id(), purged, "old dedications deleted");
        }
        let player = station.engine.state().borrow().clone();
        if let Some(player) = player {
            removed(pool, station.id(), &player).await?;
        }
    }
    Ok(())
}

/// Accepted requests whose entry is neither queued nor on a deck any more,
/// without having played: taken out of the queue.
async fn removed(pool: &MySqlPool, station_id: u64, player: &PlayerState) -> ApiResult<()> {
    let accepted: Vec<(u64, Option<u64>, Option<u64>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, engine_started_at, entry_id FROM dedications
             WHERE station_id = ? AND status = 'accepted'
               AND decided_at < NOW(6) - INTERVAL {REMOVED_AFTER_SECONDS} SECOND"
        )))
        .bind(station_id)
        .fetch_all(pool)
        .await?;
    let present = |entry: u64| {
        player.waiting().any(|e| e.entry_id == entry)
            || player
                .decks
                .iter()
                .filter_map(|deck| deck.entry.as_ref())
                .any(|e| e.entry_id == entry)
            || player
                .pending_jingle
                .as_deref()
                .is_some_and(|e| e.entry_id == entry)
    };
    for (id, started, entry) in accepted {
        let gone = match (started, entry) {
            (Some(started), Some(entry)) => {
                started != player.engine_started_at_unix_ms || !present(entry)
            }
            _ => true,
        };
        if gone {
            sqlx::query(
                "UPDATE dedications SET status = 'refused', reason = 'removed'
                 WHERE id = ? AND status = 'accepted'",
            )
            .bind(id)
            .execute(pool)
            .await?;
            tracing::info!(
                station = station_id,
                dedication = id,
                "dedication taken out of the queue"
            );
        }
    }
    Ok(())
}

/// Runs [`tick`] every 30 seconds. Never returns.
pub async fn run(pool: MySqlPool, stations: Arc<Stations>) {
    let mut interval = tokio::time::interval(TICK);
    loop {
        interval.tick().await;
        if let Err(error) = tick(&pool, &stations).await {
            tracing::warn!(%error, "cannot tidy the dedications");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked() {
        assert_eq!(DedicationSettings::default().check(), Ok(()));
        for bad in [
            DedicationSettings {
                max_per_range: 0,
                ..DedicationSettings::default()
            },
            DedicationSettings {
                per_device: 11,
                ..DedicationSettings::default()
            },
            DedicationSettings {
                message_max: 501,
                ..DedicationSettings::default()
            },
            DedicationSettings {
                retention_days: 0,
                ..DedicationSettings::default()
            },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn searches_take_wildcards_as_they_are() {
        assert_eq!(like("neon"), "%neon%");
        assert_eq!(like("50%_off\\"), "%50\\%\\_off\\\\%");
    }

    #[test]
    fn texts_are_trimmed_and_measured_in_characters() {
        assert_eq!(text("  Zoë ", "from", 3, false).unwrap(), "Zoë");
        assert!(text("", "from", 3, false).is_err());
        assert_eq!(text(" ", "message", 3, true).unwrap(), "");
        assert!(text("four", "from", 3, false).is_err());
        assert!(text("a\u{7}", "from", 3, false).is_err());
        assert_eq!(random_key().unwrap().len(), 32);
    }

    #[test]
    fn an_inappropriate_message_is_not_told() {
        let row = Row {
            id: 1,
            follow_key: "k".to_owned(),
            track_id: Some(1),
            artist: None,
            title: None,
            from_name: "Marc".to_owned(),
            to_name: "Julie".to_owned(),
            message: String::new(),
            status: "refused".to_owned(),
            reason: Some("inappropriate".to_owned()),
            submitted_ms: 0,
            decided_ms: None,
            decided_by: None,
            planned_ms: None,
            played_ms: None,
            play_id: None,
        };
        let told = Followed::from(row.clone());
        assert_eq!(told.status, DedicationStatus::Refused);
        assert_eq!(told.reason, None);
        let operators = Dedication::from(row);
        assert_eq!(operators.reason, Some(DedicationReason::Inappropriate));
    }
}
