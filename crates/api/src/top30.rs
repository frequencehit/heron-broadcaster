// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Top 30 (`docs/listener-interaction.md#top-30`): each week, the
//! tracks the listeners liked most, frozen when the week closes, then
//! played on air in the show's hours, from the last rank to number 1, then
//! a new release.
//!
//! - A week runs from Monday 00:00 to its close, in the station's time
//!   zone; its chart is ranked from the likes of the plays that started in
//!   it (`play_votes`), and kept when it closes (`top30_charts`,
//!   `top30_entries`).
//! - A show's programme is built when the queue manager first needs a
//!   track planned in its hours ([`due`]), kept (`top30_shows`) and queued
//!   in order: no separation rules, the restrictions checked.
//! - The gateway's state hides the ranks still to come while the show
//!   plays ([`public_state`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use heron_core::Module;
use heron_core::shows::{self as core_shows, Show, ShowSettings};
use heron_rotation::Library;
use heron_rotation::restrict::{Subject, forbidding};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan};
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::{self, StationSettings};
use crate::stations::{Station, Stations};
use crate::webhooks::{WebhookEvent, Webhooks};

/// How often the background work looks for a week that closed and ranks
/// that started playing.
const TICK: Duration = Duration::from_secs(15);
const MAX_SIZE: u32 = 50;
const MAX_MIN_VOTES: u64 = 1_000;
const MAX_CATEGORIES: usize = 500;
/// A programme started keeps playing this long after its slot at most.
const OVERRUN_SECONDS: u32 = 3_600;
/// Past charts listed per page by default, and at most.
const CHARTS_PAGE: u32 = 20;
const MAX_CHARTS_PAGE: u32 = 100;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_top30_settings, put_top30_settings))
        .routes(routes!(get_top30_chart))
        .routes(routes!(list_top30_charts))
        .routes(routes!(remove_track, restore_track))
        .routes(routes!(choose_new_release))
        .routes(routes!(get_top30_show))
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

fn seconds(at: Timestamp) -> f64 {
    at.as_millisecond() as f64 / 1_000.0
}

// ---------------------------------------------------------------------------
// Settings

/// A station's Top 30 settings (`station_settings`, `top30`). Its hours,
/// website and notifications are its show settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Top30Settings {
    /// The day the week closes, 1 (Monday) to 7.
    pub close_day: u8,
    /// The hour it closes, 0 to 24.
    pub close_hour: u8,
    /// Votes a track needs over the week to be ranked, 1 to 1000.
    pub min_votes: u64,
    /// Tracks in the chart, 1 to 50.
    pub size: u32,
    /// The categories whose tracks can enter the chart; none: the
    /// station's music.
    pub category_ids: Vec<u64>,
    /// The jingles between the tracks of the show.
    pub jingle_category_id: Option<u64>,
    /// Where the new release at the end of the show comes from.
    pub new_release_category_id: Option<u64>,
}

impl Default for Top30Settings {
    fn default() -> Self {
        Self {
            close_day: 7,
            close_hour: 17,
            min_votes: crate::likes::MIN_VOTES,
            size: 30,
            category_ids: Vec::new(),
            jingle_category_id: None,
            new_release_category_id: None,
        }
    }
}

impl Top30Settings {
    pub const SETTINGS_NAME: &str = "top30";

    fn check(&self) -> Result<(), String> {
        if !(1..=7).contains(&self.close_day) || self.close_hour > 24 {
            return Err("the week closes on a day 1 (Monday) to 7, at an hour 0 to 24".to_owned());
        }
        if self.close_day == 1 && self.close_hour == 0 {
            return Err("the week starts on Monday 00:00: it cannot close then".to_owned());
        }
        if !(1..=MAX_MIN_VOTES).contains(&self.min_votes) {
            return Err(format!("min_votes is 1 to {MAX_MIN_VOTES}"));
        }
        if !(1..=MAX_SIZE).contains(&self.size) {
            return Err(format!("size is 1 to {MAX_SIZE}"));
        }
        if self.category_ids.len() > MAX_CATEGORIES {
            return Err(format!("at most {MAX_CATEGORIES} categories"));
        }
        Ok(())
    }
}

async fn load_settings(pool: &MySqlPool, station_id: u64) -> ApiResult<Top30Settings> {
    settings::load_for(pool, station_id, Top30Settings::SETTINGS_NAME).await
}

async fn time_zone(pool: &MySqlPool, station_id: u64) -> ApiResult<TimeZone> {
    let station: StationSettings =
        settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await?;
    Ok(station.time_zone())
}

/// The Top 30's entry among the shows.
static TOP30: Show = core_shows::SHOWS[2];

fn top30_show() -> &'static Show {
    Show::of(Module::Top30).unwrap_or(&TOP30)
}

/// The station's Top 30 settings.
#[utoipa::path(get, path = "/top30/settings", tag = "top30",
    responses((status = 200, body = Top30Settings), (status = "4XX", body = ErrorBody)))]
async fn get_top30_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Top30Settings>> {
    Ok(Json(load_settings(&state.pool, station.id()).await?))
}

/// Replaces the station's Top 30 settings. The categories must be the
/// station's (`422`). Operators.
#[utoipa::path(put, path = "/top30/settings", tag = "top30",
    request_body = Top30Settings,
    responses((status = 200, body = Top30Settings), (status = "4XX", body = ErrorBody)))]
async fn put_top30_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(mut settings): Json<Top30Settings>,
) -> ApiResult<Json<Top30Settings>> {
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
    let named = settings
        .category_ids
        .iter()
        .chain(settings.jingle_category_id.iter())
        .chain(settings.new_release_category_id.iter());
    for id in named {
        if !ours.contains(id) {
            return Err(ApiError::Invalid(format!(
                "category {id} is not one of the station's"
            )));
        }
    }
    settings::save_for(pool, station.id(), Top30Settings::SETTINGS_NAME, &settings).await?;
    tracing::info!(user = %user.username, station = station.id(), "top 30 settings changed");
    Ok(Json(settings))
}

// ---------------------------------------------------------------------------
// Weeks

/// A chart's week: from Monday 00:00 to its close, in the station's time
/// zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChartWeek {
    pub monday: Date,
    pub from: Timestamp,
    pub closes_at: Timestamp,
}

impl ChartWeek {
    pub fn of(monday: Date, settings: &Top30Settings, tz: &TimeZone) -> ApiResult<Self> {
        let from = monday.to_zoned(tz.clone()).map_err(internal)?.timestamp();
        let day = monday
            .checked_add(i64::from(settings.close_day.clamp(1, 7) - 1).days())
            .map_err(internal)?;
        let closes_at = if settings.close_hour >= 24 {
            day.checked_add(1.day())
                .map_err(internal)?
                .to_zoned(tz.clone())
                .map_err(internal)?
        } else {
            day.at(settings.close_hour as i8, 0, 0, 0)
                .to_zoned(tz.clone())
                .map_err(internal)?
        };
        Ok(Self {
            monday,
            from,
            closes_at: closes_at.timestamp(),
        })
    }

    /// The week `at` is in.
    pub fn containing(at: Timestamp, settings: &Top30Settings, tz: &TimeZone) -> ApiResult<Self> {
        let today = at.to_zoned(tz.clone()).date();
        let monday = today
            .checked_sub(i64::from(today.weekday().to_monday_zero_offset()).days())
            .map_err(internal)?;
        Self::of(monday, settings, tz)
    }

    /// The latest week closed at `at`.
    pub fn latest_closed(
        at: Timestamp,
        settings: &Top30Settings,
        tz: &TimeZone,
    ) -> ApiResult<Self> {
        let week = Self::containing(at, settings, tz)?;
        if week.closes_at <= at {
            Ok(week)
        } else {
            week.previous(settings, tz)
        }
    }

    pub fn previous(&self, settings: &Top30Settings, tz: &TimeZone) -> ApiResult<Self> {
        let monday = self.monday.checked_sub(7.days()).map_err(internal)?;
        Self::of(monday, settings, tz)
    }

    /// `2026-11-02`, its Monday.
    pub fn key(&self) -> String {
        self.monday.to_string()
    }
}

/// A week as a path names it, by its Monday.
fn parse_week(text: &str) -> ApiResult<Date> {
    let monday: Date = text
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("{text:?} is not a date: 2026-11-02")))?;
    if monday.weekday() != jiff::civil::Weekday::Monday {
        return Err(ApiError::BadRequest(format!(
            "{text} is not a Monday: a week is named by its Monday"
        )));
    }
    Ok(monday)
}

// ---------------------------------------------------------------------------
// The chart

/// How a track moved against the chart of the week before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Movement {
    Up,
    Down,
    Same,
    /// Never in a chart before.
    New,
    /// In an earlier chart, not last week's.
    Reentry,
}

impl Movement {
    fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Same => "same",
            Self::New => "new",
            Self::Reentry => "reentry",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "up" => Self::Up,
            "down" => Self::Down,
            "same" => Self::Same,
            "reentry" => Self::Reentry,
            _ => Self::New,
        }
    }
}

/// How a track at `rank` moved: its rank last week, and how many earlier
/// charts it was in.
pub fn movement(rank: u32, previous: Option<u32>, earlier_weeks: u32) -> Movement {
    match previous {
        Some(previous) if previous > rank => Movement::Up,
        Some(previous) if previous < rank => Movement::Down,
        Some(_) => Movement::Same,
        None if earlier_weeks > 0 => Movement::Reentry,
        None => Movement::New,
    }
}

/// A rank of a chart.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ChartEntry {
    pub rank: u32,
    /// Missing once the track is deleted.
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub likes: u64,
    pub dislikes: u64,
    /// The share of likes, 0 to 1.
    pub share: f64,
    /// Its plays with votes over the week.
    pub plays: u64,
    pub movement: Movement,
    /// Its rank in last week's chart.
    pub previous_rank: Option<u32>,
    /// The charts it was in, this one included.
    pub weeks: u32,
}

/// A track taken out of a chart.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct Removed {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// Who took it out; missing once the user is deleted.
    pub removed_by: Option<String>,
    pub removed_at_unix_ms: i64,
}

/// The new release at the end of the show.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NewRelease {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// Proposed by Heron, not chosen by hand.
    pub proposed: bool,
}

/// A week's chart.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Chart {
    /// Its Monday: `2026-11-02`.
    pub week: String,
    pub from_unix_ms: i64,
    pub closes_at_unix_ms: i64,
    /// Closed and kept; else the chart as it stands now.
    pub frozen: bool,
    /// A show was built from it: it does not change any more.
    pub locked: bool,
    /// The votes on the eligible tracks over the week.
    pub votes: u64,
    pub entries: Vec<ChartEntry>,
    pub removed: Vec<Removed>,
    /// `null` without a new release category.
    pub new_release: Option<NewRelease>,
}

/// A track's votes over a week.
#[derive(Clone, Debug, sqlx::FromRow)]
struct Ranked {
    track_id: u64,
    artist: Option<String>,
    title: Option<String>,
    likes: u64,
    dislikes: u64,
    plays: u64,
}

/// The categories whose tracks can enter the chart; `None`: every track
/// (a station without categories rotates the whole library).
async fn eligible(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
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

/// The plays of a week's eligible tracks, not taken out, in a query.
fn push_week_filter(
    query: &mut QueryBuilder<sqlx::MySql>,
    station_id: u64,
    week: &ChartWeek,
    to: Timestamp,
    categories: Option<&[u64]>,
) {
    query
        .push(" FROM play_votes v JOIN tracks t ON t.id = v.track_id WHERE v.station_id = ")
        .push_bind(station_id)
        .push(" AND v.started_at >= FROM_UNIXTIME(")
        .push_bind(seconds(week.from))
        .push(") AND v.started_at < FROM_UNIXTIME(")
        .push_bind(seconds(to.min(week.closes_at)))
        .push(") AND t.deleted_at IS NULL");
    if let Some(categories) = categories {
        query.push(
            " AND EXISTS (SELECT 1 FROM track_categories tc
                          WHERE tc.track_id = v.track_id AND tc.category_id IN (",
        );
        let mut ids = query.separated(", ");
        for id in categories {
            ids.push_bind(*id);
        }
        query.push("))");
    }
}

/// The week's ranking up to `to` (at most its close): the eligible tracks
/// with enough votes, not taken out, by share of likes, then votes, then
/// the oldest track; and the week's votes.
async fn rank_week(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    week: &ChartWeek,
    to: Timestamp,
) -> ApiResult<(Vec<Ranked>, u64)> {
    let categories = eligible(pool, station_id, settings).await?;
    if categories.as_ref().is_some_and(Vec::is_empty) {
        return Ok((Vec::new(), 0));
    }
    let categories = categories.as_deref();
    let mut query = QueryBuilder::new(
        "SELECT v.track_id, t.artist, t.title,
                CAST(SUM(v.likes) AS UNSIGNED) AS likes,
                CAST(SUM(v.dislikes) AS UNSIGNED) AS dislikes,
                CAST(COUNT(*) AS UNSIGNED) AS plays",
    );
    push_week_filter(&mut query, station_id, week, to, categories);
    query
        .push(
            " AND NOT EXISTS (SELECT 1 FROM top30_removals r
                              WHERE r.station_id = v.station_id AND r.track_id = v.track_id
                                AND r.week = ",
        )
        .push_bind(week.key())
        .push(
            ") GROUP BY v.track_id, t.artist, t.title
             HAVING SUM(v.likes) + SUM(v.dislikes) >= ",
        )
        .push_bind(settings.min_votes.max(1))
        .push(
            " ORDER BY SUM(v.likes) / (SUM(v.likes) + SUM(v.dislikes)) DESC,
                       SUM(v.likes) + SUM(v.dislikes) DESC, v.track_id
             LIMIT ",
        )
        .push_bind(settings.size.clamp(1, MAX_SIZE));
    let ranked: Vec<Ranked> = query.build_query_as().fetch_all(pool).await?;
    let mut query =
        QueryBuilder::new("SELECT CAST(COALESCE(SUM(v.likes + v.dislikes), 0) AS UNSIGNED)");
    push_week_filter(&mut query, station_id, week, to, categories);
    let votes: u64 = query.build_query_scalar().fetch_one(pool).await?;
    Ok((ranked, votes))
}

/// The ranking with each track's movement against the week before and its
/// weeks in the chart, from the charts kept.
async fn with_movements(
    pool: &MySqlPool,
    station_id: u64,
    week: &ChartWeek,
    ranked: Vec<Ranked>,
) -> ApiResult<Vec<ChartEntry>> {
    let before = week.monday.checked_sub(7.days()).map_err(internal)?;
    let previous: Vec<(u64, u64)> = sqlx::query_as(
        "SELECT CAST(`rank` AS UNSIGNED), track_id FROM top30_entries
         WHERE station_id = ? AND week = ? AND track_id IS NOT NULL",
    )
    .bind(station_id)
    .bind(before.to_string())
    .fetch_all(pool)
    .await?;
    let earlier: HashMap<u64, u64> = sqlx::query_as::<_, (u64, u64)>(
        "SELECT track_id, CAST(COUNT(*) AS UNSIGNED) FROM top30_entries
         WHERE station_id = ? AND week < ? AND track_id IS NOT NULL
         GROUP BY track_id",
    )
    .bind(station_id)
    .bind(week.key())
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    Ok(ranked
        .into_iter()
        .enumerate()
        .map(|(index, track)| {
            let rank = index as u32 + 1;
            let previous_rank = previous
                .iter()
                .find(|(_, id)| *id == track.track_id)
                .map(|(rank, _)| *rank as u32);
            let weeks_before = earlier.get(&track.track_id).copied().unwrap_or(0) as u32;
            ChartEntry {
                rank,
                track_id: Some(track.track_id),
                artist: track.artist,
                title: track.title,
                likes: track.likes,
                dislikes: track.dislikes,
                share: crate::likes::share(track.likes, track.dislikes),
                plays: track.plays,
                movement: movement(rank, previous_rank, weeks_before),
                previous_rank,
                weeks: weeks_before + 1,
            }
        })
        .collect())
}

/// Ranks a closed week and keeps its chart, in place of the one kept
/// (after a track was taken out). Returns its entries and votes.
pub async fn freeze(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    week: &ChartWeek,
) -> ApiResult<(Vec<ChartEntry>, u64)> {
    let (ranked, votes) = rank_week(pool, station_id, settings, week, week.closes_at).await?;
    let entries = with_movements(pool, station_id, week, ranked).await?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO top30_charts (station_id, week, from_at, closes_at, frozen_at, votes)
         VALUES (?, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), NOW(6), ?)
         ON DUPLICATE KEY UPDATE from_at = VALUES(from_at), closes_at = VALUES(closes_at),
                                 frozen_at = VALUES(frozen_at), votes = VALUES(votes)",
    )
    .bind(station_id)
    .bind(week.key())
    .bind(seconds(week.from))
    .bind(seconds(week.closes_at))
    .bind(votes)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM top30_entries WHERE station_id = ? AND week = ?")
        .bind(station_id)
        .bind(week.key())
        .execute(&mut *tx)
        .await?;
    if !entries.is_empty() {
        let mut query = QueryBuilder::new(
            "INSERT INTO top30_entries (station_id, week, `rank`, track_id, artist, title,
                                        likes, dislikes, plays, movement, previous_rank, weeks) ",
        );
        query.push_values(&entries, |mut row, entry| {
            row.push_bind(station_id)
                .push_bind(week.key())
                .push_bind(entry.rank)
                .push_bind(entry.track_id)
                .push_bind(&entry.artist)
                .push_bind(&entry.title)
                .push_bind(entry.likes)
                .push_bind(entry.dislikes)
                .push_bind(entry.plays)
                .push_bind(entry.movement.as_str())
                .push_bind(entry.previous_rank)
                .push_bind(entry.weeks);
        });
        query.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    tracing::info!(
        station = station_id,
        week = %week.key(),
        tracks = entries.len(),
        "top 30 frozen"
    );
    Ok((entries, votes))
}

/// The votes of a frozen week; `None` while it is not frozen.
async fn frozen_votes(pool: &MySqlPool, station_id: u64, monday: Date) -> ApiResult<Option<u64>> {
    let votes: Option<Option<u64>> = sqlx::query_scalar(
        "SELECT CAST(IF(frozen_at IS NULL, NULL, votes) AS UNSIGNED) FROM top30_charts
         WHERE station_id = ? AND week = ?",
    )
    .bind(station_id)
    .bind(monday.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(votes.flatten())
}

/// Whether a show was built from the week's chart.
async fn locked(pool: &MySqlPool, station_id: u64, monday: Date) -> ApiResult<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM top30_shows WHERE station_id = ? AND week = ? LIMIT 1")
            .bind(station_id)
            .bind(monday.to_string())
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

#[derive(sqlx::FromRow)]
struct StoredEntry {
    rank: u64,
    track_id: Option<u64>,
    artist: Option<String>,
    title: Option<String>,
    likes: u64,
    dislikes: u64,
    plays: u64,
    movement: String,
    previous_rank: Option<u64>,
    weeks: u64,
}

/// A frozen chart's entries.
async fn stored_entries(
    pool: &MySqlPool,
    station_id: u64,
    monday: Date,
) -> ApiResult<Vec<ChartEntry>> {
    let rows: Vec<StoredEntry> = sqlx::query_as(
        "SELECT CAST(`rank` AS UNSIGNED) AS `rank`, track_id, artist, title, likes, dislikes,
                plays, CAST(movement AS CHAR) AS movement,
                CAST(previous_rank AS UNSIGNED) AS previous_rank,
                CAST(weeks AS UNSIGNED) AS weeks
         FROM top30_entries WHERE station_id = ? AND week = ? ORDER BY `rank`",
    )
    .bind(station_id)
    .bind(monday.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ChartEntry {
            rank: row.rank as u32,
            track_id: row.track_id,
            artist: row.artist,
            title: row.title,
            likes: row.likes,
            dislikes: row.dislikes,
            share: crate::likes::share(row.likes, row.dislikes),
            plays: row.plays,
            movement: Movement::parse(&row.movement),
            previous_rank: row.previous_rank.map(|rank| rank as u32),
            weeks: row.weeks as u32,
        })
        .collect())
}

/// A week's entries: kept once it is frozen (frozen now when it closed
/// and was not yet), as they stand now before. With whether they are
/// frozen, and the week's votes.
async fn entries_of(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    week: &ChartWeek,
    now: Timestamp,
) -> ApiResult<(Vec<ChartEntry>, bool, u64)> {
    if let Some(votes) = frozen_votes(pool, station_id, week.monday).await? {
        return Ok((
            stored_entries(pool, station_id, week.monday).await?,
            true,
            votes,
        ));
    }
    if week.closes_at <= now {
        let (entries, votes) = freeze(pool, station_id, settings, week).await?;
        return Ok((entries, true, votes));
    }
    // Plays still to come do not count yet: up to a minute ahead.
    let to = now.checked_add(SignedDuration::from_mins(1)).unwrap_or(now);
    let (ranked, votes) = rank_week(pool, station_id, settings, week, to).await?;
    let entries = with_movements(pool, station_id, week, ranked).await?;
    Ok((entries, false, votes))
}

type Label = (u64, Option<String>, Option<String>);

/// ` AND {column} NOT IN (ids)`, nothing without IDs.
fn not_in(query: &mut QueryBuilder<sqlx::MySql>, column: &str, ids: &[u64]) {
    if ids.is_empty() {
        return;
    }
    query.push(format!(" AND {column} NOT IN ("));
    let mut separated = query.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
    query.push(")");
}

/// The new release of a week: chosen by hand, else the most liked track of
/// the new release category over the week that is not in `chart`, else
/// the track of the category added last.
async fn new_release(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    week: &ChartWeek,
    chart: &[u64],
) -> ApiResult<Option<NewRelease>> {
    let Some(category) = settings.new_release_category_id else {
        return Ok(None);
    };
    let chosen: Option<Option<u64>> = sqlx::query_scalar(
        "SELECT new_release_track_id FROM top30_charts WHERE station_id = ? AND week = ?",
    )
    .bind(station_id)
    .bind(week.key())
    .fetch_optional(pool)
    .await?;
    if let Some(id) = chosen.flatten() {
        let found: Option<Label> = sqlx::query_as(
            "SELECT id, artist, title FROM tracks WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        if let Some((track_id, artist, title)) = found {
            return Ok(Some(NewRelease {
                track_id,
                artist,
                title,
                proposed: false,
            }));
        }
    }
    let mut query = QueryBuilder::new(
        "SELECT v.track_id, t.artist, t.title
         FROM play_votes v JOIN tracks t ON t.id = v.track_id
         JOIN track_categories tc ON tc.track_id = v.track_id AND tc.category_id = ",
    );
    query
        .push_bind(category)
        .push(" WHERE v.station_id = ")
        .push_bind(station_id)
        .push(" AND v.started_at >= FROM_UNIXTIME(")
        .push_bind(seconds(week.from))
        .push(") AND v.started_at < FROM_UNIXTIME(")
        .push_bind(seconds(week.closes_at))
        .push(") AND t.deleted_at IS NULL");
    not_in(&mut query, "v.track_id", chart);
    query.push(
        " GROUP BY v.track_id, t.artist, t.title
         HAVING SUM(v.likes) + SUM(v.dislikes) > 0
         ORDER BY SUM(v.likes) / (SUM(v.likes) + SUM(v.dislikes)) DESC,
                  SUM(v.likes) + SUM(v.dislikes) DESC, v.track_id
         LIMIT 1",
    );
    let mut found: Option<Label> = query.build_query_as().fetch_optional(pool).await?;
    if found.is_none() {
        let mut query = QueryBuilder::new(
            "SELECT t.id, t.artist, t.title FROM tracks t
             JOIN track_categories tc ON tc.track_id = t.id AND tc.category_id = ",
        );
        query
            .push_bind(category)
            .push(" WHERE t.deleted_at IS NULL AND t.enabled");
        not_in(&mut query, "t.id", chart);
        query.push(" ORDER BY t.created_at DESC, t.id DESC LIMIT 1");
        found = query.build_query_as().fetch_optional(pool).await?;
    }
    Ok(found.map(|(track_id, artist, title)| NewRelease {
        track_id,
        artist,
        title,
        proposed: true,
    }))
}

async fn removed(pool: &MySqlPool, station_id: u64, monday: Date) -> ApiResult<Vec<Removed>> {
    Ok(sqlx::query_as(
        "SELECT r.track_id, t.artist, t.title, u.username AS removed_by,
                CAST(UNIX_TIMESTAMP(r.removed_at) * 1000 AS SIGNED) AS removed_at_unix_ms
         FROM top30_removals r JOIN tracks t ON t.id = r.track_id
         LEFT JOIN users u ON u.id = r.removed_by
         WHERE r.station_id = ? AND r.week = ? ORDER BY r.removed_at, r.track_id",
    )
    .bind(station_id)
    .bind(monday.to_string())
    .fetch_all(pool)
    .await?)
}

/// A week's chart as the pages show it.
async fn chart(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    week: &ChartWeek,
    now: Timestamp,
) -> ApiResult<Chart> {
    let (entries, frozen, votes) = entries_of(pool, station_id, settings, week, now).await?;
    let ids: Vec<u64> = entries.iter().filter_map(|entry| entry.track_id).collect();
    Ok(Chart {
        week: week.key(),
        from_unix_ms: week.from.as_millisecond(),
        closes_at_unix_ms: week.closes_at.as_millisecond(),
        frozen,
        locked: locked(pool, station_id, week.monday).await?,
        votes,
        new_release: new_release(pool, station_id, settings, week, &ids).await?,
        removed: removed(pool, station_id, week.monday).await?,
        entries,
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChartQuery {
    /// A week, by its Monday (`2026-11-02`); this week by default.
    pub week: Option<String>,
}

/// The week as a request names it, this week by default; `404` for a week
/// to come.
async fn requested_week(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    week: Option<&str>,
    now: Timestamp,
) -> ApiResult<ChartWeek> {
    let tz = time_zone(pool, station_id).await?;
    let this = ChartWeek::containing(now, settings, &tz)?;
    let Some(week) = week else {
        return Ok(this);
    };
    let monday = parse_week(week)?;
    if monday > this.monday {
        return Err(ApiError::not_found(format!("the week of {monday}")));
    }
    ChartWeek::of(monday, settings, &tz)
}

/// A week's chart: this week's as it stands now by default, frozen once
/// the week closed.
#[utoipa::path(get, path = "/top30/chart", tag = "top30",
    params(ChartQuery),
    responses((status = 200, body = Chart), (status = "4XX", body = ErrorBody)))]
async fn get_top30_chart(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<ChartQuery>,
) -> ApiResult<Json<Chart>> {
    let pool = &state.pool;
    let settings = load_settings(pool, station.id()).await?;
    let now = Timestamp::now();
    let week = requested_week(pool, station.id(), &settings, query.week.as_deref(), now).await?;
    Ok(Json(
        chart(pool, station.id(), &settings, &week, now).await?,
    ))
}

/// A frozen chart in the list of past weeks.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PastChart {
    pub week: String,
    pub from_unix_ms: i64,
    pub closes_at_unix_ms: i64,
    /// Its tracks.
    pub size: u32,
    pub number_one: Option<PastNumberOne>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PastNumberOne {
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChartsQuery {
    /// Weeks before this one (its Monday), for the next page.
    pub before: Option<String>,
    /// 1 to 100, 20 by default.
    pub limit: Option<u32>,
}

/// The frozen charts, the latest first.
#[utoipa::path(get, path = "/top30/charts", tag = "top30",
    params(ChartsQuery),
    responses((status = 200, body = Vec<PastChart>), (status = "4XX", body = ErrorBody)))]
async fn list_top30_charts(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<ChartsQuery>,
) -> ApiResult<Json<Vec<PastChart>>> {
    let limit = query.limit.unwrap_or(CHARTS_PAGE);
    if !(1..=MAX_CHARTS_PAGE).contains(&limit) {
        return Err(ApiError::BadRequest(format!(
            "limit is 1 to {MAX_CHARTS_PAGE}"
        )));
    }
    let mut sql = QueryBuilder::new(
        "SELECT CAST(c.week AS CHAR),
                CAST(UNIX_TIMESTAMP(c.from_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(c.closes_at) * 1000 AS SIGNED),
                CAST((SELECT COUNT(*) FROM top30_entries e
                      WHERE e.station_id = c.station_id AND e.week = c.week) AS UNSIGNED),
                CAST(o.`rank` AS UNSIGNED), o.track_id, o.artist, o.title
         FROM top30_charts c
         LEFT JOIN top30_entries o ON o.station_id = c.station_id AND o.week = c.week
                                  AND o.`rank` = 1
         WHERE c.frozen_at IS NOT NULL AND c.station_id = ",
    );
    sql.push_bind(station.id());
    if let Some(before) = &query.before {
        sql.push(" AND c.week < ")
            .push_bind(parse_week(before)?.to_string());
    }
    sql.push(" ORDER BY c.week DESC LIMIT ").push_bind(limit);
    type Row = (
        String,
        i64,
        i64,
        u64,
        Option<u64>,
        Option<u64>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sql.build_query_as().fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(week, from, closes, size, one, track_id, artist, title)| PastChart {
                    week,
                    from_unix_ms: from,
                    closes_at_unix_ms: closes,
                    size: size as u32,
                    number_one: one.map(|_| PastNumberOne {
                        track_id,
                        artist,
                        title,
                    }),
                },
            )
            .collect(),
    ))
}

/// A week that can still change: not to come, no show built from it.
async fn changeable(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    week: &str,
    now: Timestamp,
) -> ApiResult<ChartWeek> {
    let week = requested_week(pool, station_id, settings, Some(week), now).await?;
    if locked(pool, station_id, week.monday).await? {
        return Err(ApiError::Conflict(
            "the show was built from this chart: it does not change any more".to_owned(),
        ));
    }
    Ok(week)
}

/// Freezes the week again when it was frozen: its ranks follow the change.
async fn refreeze(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    week: &ChartWeek,
) -> ApiResult<()> {
    if frozen_votes(pool, station_id, week.monday).await?.is_some() {
        freeze(pool, station_id, settings, week).await?;
    }
    Ok(())
}

/// Takes a track out of a week's chart: the tracks after it move up. `409`
/// once a show was built from the chart. Operators.
#[utoipa::path(post, path = "/top30/charts/{week}/removals/{track_id}", tag = "top30",
    params(("week" = String, Path, description = "The week's Monday"),
           ("track_id" = u64, Path, description = "The track")),
    responses((status = 200, body = Chart), (status = "4XX", body = ErrorBody)))]
async fn remove_track(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path((week, track_id)): Path<(String, u64)>,
) -> ApiResult<Json<Chart>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let settings = load_settings(pool, station.id()).await?;
    let now = Timestamp::now();
    let week = changeable(pool, station.id(), &settings, &week, now).await?;
    let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM tracks WHERE id = ?")
        .bind(track_id)
        .fetch_optional(pool)
        .await?;
    exists.ok_or_else(|| ApiError::not_found(format!("track {track_id}")))?;
    sqlx::query(
        "INSERT IGNORE INTO top30_removals (station_id, week, track_id, removed_by, removed_at)
         VALUES (?, ?, ?, ?, NOW(6))",
    )
    .bind(station.id())
    .bind(week.key())
    .bind(track_id)
    .bind(user.id)
    .execute(pool)
    .await?;
    refreeze(pool, station.id(), &settings, &week).await?;
    tracing::info!(user = %user.username, station = station.id(), week = %week.key(), track = track_id, "taken out of the top 30");
    Ok(Json(
        chart(pool, station.id(), &settings, &week, now).await?,
    ))
}

/// Puts back a track taken out of a week's chart. `409` once a show was
/// built from the chart. Operators.
#[utoipa::path(delete, path = "/top30/charts/{week}/removals/{track_id}", tag = "top30",
    params(("week" = String, Path, description = "The week's Monday"),
           ("track_id" = u64, Path, description = "The track")),
    responses((status = 200, body = Chart), (status = "4XX", body = ErrorBody)))]
async fn restore_track(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path((week, track_id)): Path<(String, u64)>,
) -> ApiResult<Json<Chart>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let settings = load_settings(pool, station.id()).await?;
    let now = Timestamp::now();
    let week = changeable(pool, station.id(), &settings, &week, now).await?;
    let deleted = sqlx::query(
        "DELETE FROM top30_removals WHERE station_id = ? AND week = ? AND track_id = ?",
    )
    .bind(station.id())
    .bind(week.key())
    .bind(track_id)
    .execute(pool)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found(format!(
            "track {track_id} taken out of that week"
        )));
    }
    refreeze(pool, station.id(), &settings, &week).await?;
    tracing::info!(user = %user.username, station = station.id(), week = %week.key(), track = track_id, "put back in the top 30");
    Ok(Json(
        chart(pool, station.id(), &settings, &week, now).await?,
    ))
}

/// The new release chosen by hand; `null` goes back to the proposed one.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewReleaseChoice {
    pub track_id: Option<u64>,
}

/// Chooses a week's new release among the tracks of the new release
/// category, or goes back to the proposed one. `409` once a show was built
/// from the chart. Operators.
#[utoipa::path(put, path = "/top30/charts/{week}/new-release", tag = "top30",
    params(("week" = String, Path, description = "The week's Monday")),
    request_body = NewReleaseChoice,
    responses((status = 200, body = Chart), (status = "4XX", body = ErrorBody)))]
async fn choose_new_release(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(week): Path<String>,
    Json(choice): Json<NewReleaseChoice>,
) -> ApiResult<Json<Chart>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let settings = load_settings(pool, station.id()).await?;
    let now = Timestamp::now();
    let week = changeable(pool, station.id(), &settings, &week, now).await?;
    if let Some(track_id) = choice.track_id {
        let Some(category) = settings.new_release_category_id else {
            return Err(ApiError::Invalid(
                "the Top 30 has no new release category".to_owned(),
            ));
        };
        let found: Option<u64> = sqlx::query_scalar(
            "SELECT t.id FROM tracks t
             JOIN track_categories tc ON tc.track_id = t.id AND tc.category_id = ?
             WHERE t.id = ? AND t.deleted_at IS NULL",
        )
        .bind(category)
        .bind(track_id)
        .fetch_optional(pool)
        .await?;
        if found.is_none() {
            return Err(ApiError::Invalid(format!(
                "track {track_id} is not in the new release category"
            )));
        }
    }
    sqlx::query(
        "INSERT INTO top30_charts (station_id, week, from_at, closes_at, new_release_track_id)
         VALUES (?, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), ?)
         ON DUPLICATE KEY UPDATE new_release_track_id = VALUES(new_release_track_id)",
    )
    .bind(station.id())
    .bind(week.key())
    .bind(seconds(week.from))
    .bind(seconds(week.closes_at))
    .bind(choice.track_id)
    .execute(pool)
    .await?;
    tracing::info!(user = %user.username, station = station.id(), week = %week.key(), track = ?choice.track_id, "top 30 new release chosen");
    Ok(Json(
        chart(pool, station.id(), &settings, &week, now).await?,
    ))
}

// ---------------------------------------------------------------------------
// The show

/// What an item of the show is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Jingle,
    Track,
    NewRelease,
}

/// An item of a show's programme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Item {
    pub kind: ItemKind,
    /// The rank of a track, and of the jingle before it.
    pub rank: Option<u32>,
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// What is played of it.
    pub length_ms: u32,
    pub planned_at_unix_ms: i64,
    /// Why it is passed over (a restriction, a track that cannot play).
    pub skipped: Option<String>,
    /// Sent to the queue.
    #[serde(default)]
    pub queued: bool,
}

/// A show's programme: its items in order, and the ranks left out to fit
/// its slot.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Programme {
    pub items: Vec<Item>,
    pub left_out: Vec<u32>,
}

/// The jingles of the show: one per rank when the category has them, and
/// the others in turn, the least recently played first.
#[derive(Clone, Debug, Default)]
pub struct Jingles {
    pub by_rank: HashMap<u32, u64>,
    pub others: Vec<u64>,
}

/// The rank a jingle's title ends with, after a dash or `#`: `Top 30 – 12`
/// and `Top 30 #12` are 12's; `Top 30` is no rank's.
pub fn rank_of_title(title: &str) -> Option<u32> {
    let title = title.trim();
    let (before, after) = title.rsplit_once(['-', '–', '—', '#'])?;
    if before.trim().is_empty() {
        return None;
    }
    let rank: u32 = after.trim().parse().ok()?;
    (rank > 0).then_some(rank)
}

/// Why `track` cannot play at `at_ms` after a track of `previous`
/// categories; `None` when it can.
fn passed_over(
    library: &Library,
    track: u64,
    at_ms: i64,
    previous: Option<&[u64]>,
) -> Option<String> {
    let Some(found) = library.tracks.get(&track) else {
        return Some("cannot play: disabled, missing or not analyzed".to_owned());
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
    .map(|restriction| format!("restriction {} forbids it then", restriction.id))
}

fn length(library: &Library, track: u64) -> u32 {
    library
        .tracks
        .get(&track)
        .map_or(0, |track| track.length_ms)
}

/// The items for `ranks` (rank and track, in rank order), from the last
/// rank to number 1, each after its jingle, then the new release after a
/// jingle, from `opens_ms`. A track passed over goes without its jingle.
fn sequence(
    library: &Library,
    ranks: &[(u32, u64)],
    jingles: &Jingles,
    new_release: Option<u64>,
    opens_ms: i64,
) -> Vec<Item> {
    let mut items = Vec::new();
    let mut at = opens_ms;
    let mut previous: Option<Vec<u64>> = None;
    // The next of the other jingles that may play.
    let mut turn = 0usize;
    let order = ranks
        .iter()
        .rev()
        .map(|(rank, track)| (ItemKind::Track, Some(*rank), *track))
        .chain(new_release.map(|track| (ItemKind::NewRelease, None, track)));
    for (kind, rank, track) in order {
        // The rank's own jingle, else the next other one restrictions
        // allow; it plays when its track does.
        let allowed =
            |jingle: &u64| passed_over(library, *jingle, at, previous.as_deref()).is_none();
        let others = jingles.others.len();
        let jingle = rank
            .and_then(|rank| jingles.by_rank.get(&rank).copied())
            .filter(allowed)
            .map(|jingle| (jingle, None))
            .or_else(|| {
                (0..others)
                    .map(|step| (turn + step) % others)
                    .map(|index| (jingles.others[index], Some(index)))
                    .find(|(jingle, _)| allowed(jingle))
            });
        let (track_at, track_previous) = match jingle {
            Some((jingle, _)) => (
                at + i64::from(length(library, jingle)),
                library.tracks.get(&jingle).map(|j| j.categories.clone()),
            ),
            None => (at, previous.clone()),
        };
        if let Some(reason) = passed_over(library, track, track_at, track_previous.as_deref()) {
            items.push(Item {
                kind,
                rank,
                track_id: track,
                artist: None,
                title: None,
                length_ms: length(library, track),
                planned_at_unix_ms: at,
                skipped: Some(reason),
                queued: false,
            });
            continue;
        }
        if let Some((jingle, other)) = jingle {
            if let Some(index) = other {
                turn = index + 1;
            }
            let length_ms = length(library, jingle);
            items.push(Item {
                kind: ItemKind::Jingle,
                rank,
                track_id: jingle,
                artist: None,
                title: None,
                length_ms,
                planned_at_unix_ms: at,
                skipped: None,
                queued: false,
            });
            at += i64::from(length_ms);
        }
        let length_ms = length(library, track);
        items.push(Item {
            kind,
            rank,
            track_id: track,
            artist: None,
            title: None,
            length_ms,
            planned_at_unix_ms: at,
            skipped: None,
            queued: false,
        });
        at += i64::from(length_ms);
        previous = library.tracks.get(&track).map(|t| t.categories.clone());
    }
    items
}

/// Plans a show from `ranks` (rank and track): from the last rank to
/// number 1 with their jingles, then the new release, fitted to its slot
/// by leaving out the lowest ranks. Separation rules do not apply; the
/// restrictions do.
pub fn plan(
    library: &Library,
    ranks: &[(u32, u64)],
    jingles: &Jingles,
    new_release: Option<u64>,
    opens_ms: i64,
    closes_ms: i64,
) -> Programme {
    let mut ranks = ranks.to_vec();
    ranks.sort_by_key(|(rank, _)| *rank);
    let mut keep = ranks.len();
    loop {
        let items = sequence(library, &ranks[..keep], jingles, new_release, opens_ms);
        let total: i64 = items
            .iter()
            .filter(|item| item.skipped.is_none())
            .map(|item| i64::from(item.length_ms))
            .sum();
        if opens_ms + total <= closes_ms || keep <= 1 {
            return Programme {
                items,
                left_out: ranks[keep..].iter().map(|(rank, _)| *rank).collect(),
            };
        }
        keep -= 1;
    }
}

/// The jingles of the jingle category, playable on the station.
async fn load_jingles(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    library: &Library,
) -> ApiResult<Jingles> {
    let Some(category) = settings.jingle_category_id else {
        return Ok(Jingles::default());
    };
    // Track, title, when it last played on the station (ms).
    let rows: Vec<(u64, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT t.id, t.title,
                (SELECT CAST(UNIX_TIMESTAMP(MAX(h.started_at)) * 1000 AS SIGNED)
                 FROM play_history h WHERE h.track_id = t.id AND h.station_id = ?)
         FROM tracks t JOIN track_categories tc ON tc.track_id = t.id AND tc.category_id = ?
         WHERE t.deleted_at IS NULL ORDER BY t.id",
    )
    .bind(station_id)
    .bind(category)
    .fetch_all(pool)
    .await?;
    let mut jingles = Jingles::default();
    let mut others = Vec::new();
    for (id, title, last) in rows {
        if !library.tracks.contains_key(&id) {
            continue;
        }
        match title.as_deref().and_then(rank_of_title) {
            Some(rank) => {
                jingles.by_rank.entry(rank).or_insert(id);
            }
            None => others.push((last.unwrap_or(i64::MIN), id)),
        }
    }
    others.sort();
    jingles.others = others.into_iter().map(|(_, id)| id).collect();
    Ok(jingles)
}

/// Writes the artist and title of each item.
async fn label(pool: &MySqlPool, items: &mut [Item]) -> ApiResult<()> {
    let ids: HashSet<u64> = items.iter().map(|item| item.track_id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::new("SELECT id, artist, title FROM tracks WHERE id IN (");
    let mut separated = query.separated(", ");
    for id in &ids {
        separated.push_bind(*id);
    }
    query.push(")");
    let rows: Vec<Label> = query.build_query_as().fetch_all(pool).await?;
    let labels: HashMap<u64, (Option<String>, Option<String>)> = rows
        .into_iter()
        .map(|(id, artist, title)| (id, (artist, title)))
        .collect();
    for item in items {
        if let Some((artist, title)) = labels.get(&item.track_id) {
            item.artist.clone_from(artist);
            item.title.clone_from(title);
        }
    }
    Ok(())
}

/// Builds the programme of the show opening at `opens`, from the latest
/// chart closed then (frozen now if it was not yet).
async fn build(
    pool: &MySqlPool,
    station_id: u64,
    settings: &Top30Settings,
    tz: &TimeZone,
    opens: Timestamp,
    closes: Timestamp,
) -> ApiResult<(ChartWeek, Programme)> {
    let week = ChartWeek::latest_closed(opens, settings, tz)?;
    let now = Timestamp::now();
    let (entries, _, _) = entries_of(pool, station_id, settings, &week, now).await?;
    let ranks: Vec<(u32, u64)> = entries
        .iter()
        .filter_map(|entry| entry.track_id.map(|track| (entry.rank, track)))
        .collect();
    if ranks.is_empty() {
        return Ok((week, Programme::default()));
    }
    let library = crate::rotation::load_library(pool, station_id).await?;
    let jingles = load_jingles(pool, station_id, settings, &library).await?;
    let chart: Vec<u64> = ranks.iter().map(|(_, track)| *track).collect();
    let new_release = new_release(pool, station_id, settings, &week, &chart)
        .await?
        .map(|release| release.track_id);
    let mut programme = plan(
        &library,
        &ranks,
        &jingles,
        new_release,
        opens.as_millisecond(),
        closes.as_millisecond(),
    );
    label(pool, &mut programme.items).await?;
    Ok((week, programme))
}

/// A show kept: when it opens, the chart it plays, its programme and how
/// many items are queued.
#[derive(sqlx::FromRow)]
struct Kept {
    opens_ms: i64,
    closes_ms: i64,
    week: Option<String>,
    programme: String,
    next_item: u64,
}

impl Kept {
    fn programme(&self) -> ApiResult<Programme> {
        serde_json::from_str(&self.programme).map_err(internal)
    }
}

const KEPT_COLUMNS: &str = "CAST(UNIX_TIMESTAMP(opens_at) * 1000 AS SIGNED) AS opens_ms,
    CAST(UNIX_TIMESTAMP(closes_at) * 1000 AS SIGNED) AS closes_ms,
    CAST(week AS CHAR) AS week, CAST(programme AS CHAR) AS programme,
    CAST(next_item AS UNSIGNED) AS next_item";

async fn kept_at(pool: &MySqlPool, station_id: u64, opens: Timestamp) -> ApiResult<Option<Kept>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {KEPT_COLUMNS} FROM top30_shows WHERE station_id = ? AND opens_at = FROM_UNIXTIME(?)"
    )))
    .bind(station_id)
    .bind(seconds(opens))
    .fetch_optional(pool)
    .await?)
}

/// The items of the programme to queue next, from `next`: `wanted` that
/// play at most (a jingle comes with its track), and the index after them.
pub fn take(items: &[Item], next: usize, wanted: usize) -> (Vec<Item>, usize) {
    let mut taken: Vec<Item> = Vec::new();
    let mut index = next;
    while index < items.len() && taken.len() < wanted {
        let item = &items[index];
        index += 1;
        if item.skipped.is_none() {
            taken.push(item.clone());
        }
    }
    while taken
        .last()
        .is_some_and(|item| item.kind == ItemKind::Jingle)
        && index < items.len()
    {
        let item = &items[index];
        index += 1;
        if item.skipped.is_none() {
            taken.push(item.clone());
        }
    }
    (taken, index)
}

/// What the queue manager queues of the show now.
pub struct Due {
    pub opens_ms: i64,
    pub items: Vec<Item>,
    /// `next_item` before and after them.
    pub from: usize,
    pub next: usize,
}

/// The show's items the queue needs, for a track planned at `at_ms`: the
/// programme playing on, else the one of the show `at_ms` is in, built
/// now. `None`: rotation picks (the module or the show off, outside the
/// hours, the programme played, an empty chart).
pub async fn due(
    pool: &MySqlPool,
    station_id: u64,
    at_ms: i64,
    wanted: usize,
) -> ApiResult<Option<Due>> {
    if wanted == 0 || !crate::modules::is_on(pool, Some(station_id), Module::Top30).await? {
        return Ok(None);
    }
    let show_settings = crate::shows::load(pool, station_id, top30_show()).await?;
    if !show_settings.enabled {
        return Ok(None);
    }
    let at = Timestamp::from_millisecond(at_ms).map_err(internal)?;
    let tz = time_zone(pool, station_id).await?;
    let opening = core_shows::opening(&show_settings.open_week(), &tz, at);
    let playing: Option<Kept> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {KEPT_COLUMNS} FROM top30_shows
         WHERE station_id = ? AND opens_at <= FROM_UNIXTIME(?)
           AND closes_at > FROM_UNIXTIME(?) - INTERVAL {OVERRUN_SECONDS} SECOND
         ORDER BY opens_at DESC LIMIT 1"
    )))
    .bind(station_id)
    .bind(seconds(at))
    .bind(seconds(at))
    .fetch_optional(pool)
    .await?;
    let taken = |kept: &Kept| -> ApiResult<Option<Due>> {
        let programme = kept.programme()?;
        let from = kept.next_item as usize;
        let (items, next) = take(&programme.items, from, wanted);
        Ok((!items.is_empty()).then_some(Due {
            opens_ms: kept.opens_ms,
            items,
            from,
            next,
        }))
    };
    if let Some(kept) = &playing {
        if let Some(due) = taken(kept)? {
            return Ok(Some(due));
        }
        if opening.is_some_and(|(opens, _)| opens.as_millisecond() == kept.opens_ms) {
            // Played: rotation fills the rest of the hours.
            return Ok(None);
        }
    }
    let Some((opens, closes)) = opening else {
        return Ok(None);
    };
    if kept_at(pool, station_id, opens).await?.is_none() {
        let settings = load_settings(pool, station_id).await?;
        let (week, programme) = build(pool, station_id, &settings, &tz, opens, closes).await?;
        let json = serde_json::to_string(&programme).map_err(internal)?;
        sqlx::query(
            "INSERT IGNORE INTO top30_shows (station_id, opens_at, closes_at, week, programme, built_at)
             VALUES (?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), ?, ?, NOW(6))",
        )
        .bind(station_id)
        .bind(seconds(opens))
        .bind(seconds(closes))
        .bind(week.key())
        .bind(json)
        .execute(pool)
        .await?;
        tracing::info!(
            station = station_id,
            week = %week.key(),
            items = programme.items.len(),
            left_out = ?programme.left_out,
            "top 30 show built"
        );
    }
    match kept_at(pool, station_id, opens).await? {
        Some(kept) => taken(&kept),
        None => Ok(None),
    }
}

/// Records that the queue took the items before `next`, unless another
/// call did meanwhile.
pub async fn queued(
    pool: &MySqlPool,
    station_id: u64,
    opens_ms: i64,
    from: usize,
    next: usize,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE top30_shows SET next_item = ?
         WHERE station_id = ? AND opens_at = FROM_UNIXTIME(?) AND next_item = ?",
    )
    .bind(next as u32)
    .bind(station_id)
    .bind(opens_ms as f64 / 1_000.0)
    .bind(from as u32)
    .execute(pool)
    .await?;
    Ok(())
}

/// The show on now, or the next one.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ShowPlan {
    pub opens_at_unix_ms: i64,
    pub closes_at_unix_ms: i64,
    /// The chart it plays; `null` without one.
    pub week: Option<String>,
    /// Its programme is built: the one on air. Else what would be built now.
    pub built: bool,
    pub items: Vec<Item>,
    /// The ranks left out to fit the slot.
    pub left_out: Vec<u32>,
    pub music_ms: i64,
    pub jingles_ms: i64,
    pub new_release_ms: i64,
    pub total_ms: i64,
}

fn show_plan(
    opens: i64,
    closes: i64,
    week: Option<String>,
    built: bool,
    programme: Programme,
) -> ShowPlan {
    let sum = |kind: ItemKind| -> i64 {
        programme
            .items
            .iter()
            .filter(|item| item.kind == kind && item.skipped.is_none())
            .map(|item| i64::from(item.length_ms))
            .sum()
    };
    let (music_ms, jingles_ms, new_release_ms) = (
        sum(ItemKind::Track),
        sum(ItemKind::Jingle),
        sum(ItemKind::NewRelease),
    );
    ShowPlan {
        opens_at_unix_ms: opens,
        closes_at_unix_ms: closes,
        week,
        built,
        items: programme.items,
        left_out: programme.left_out,
        music_ms,
        jingles_ms,
        new_release_ms,
        total_ms: music_ms + jingles_ms + new_release_ms,
    }
}

/// The show on now, else the next one within a week, with its programme:
/// the one built, or what would be built now. `null` without hours.
#[utoipa::path(get, path = "/top30/show", tag = "top30",
    responses((status = 200, body = Option<ShowPlan>), (status = "4XX", body = ErrorBody)))]
async fn get_top30_show(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Option<ShowPlan>>> {
    let pool = &state.pool;
    let show_settings = crate::shows::load(pool, station.id(), top30_show()).await?;
    let tz = time_zone(pool, station.id()).await?;
    let now = Timestamp::now();
    let week_hours = show_settings.open_week();
    let Some((opens, closes)) = core_shows::opening(&week_hours, &tz, now)
        .or_else(|| core_shows::next_opening(&week_hours, &tz, now))
    else {
        return Ok(Json(None));
    };
    if let Some(kept) = kept_at(pool, station.id(), opens).await? {
        let mut programme = kept.programme()?;
        for (index, item) in programme.items.iter_mut().enumerate() {
            item.queued = index < kept.next_item as usize;
        }
        return Ok(Json(Some(show_plan(
            kept.opens_ms,
            kept.closes_ms,
            kept.week,
            true,
            programme,
        ))));
    }
    let settings = load_settings(pool, station.id()).await?;
    let (week, programme) = build(pool, station.id(), &settings, &tz, opens, closes).await?;
    Ok(Json(Some(show_plan(
        opens.as_millisecond(),
        closes.as_millisecond(),
        Some(week.key()),
        false,
        programme,
    ))))
}

// ---------------------------------------------------------------------------
// On the website

/// What has played of a show: the highest rank played (the ranks from it
/// down are revealed), the rank on air, and whether the new release
/// played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reveal {
    pub from_rank: Option<u32>,
    pub on_air_rank: Option<u32>,
    pub new_release: bool,
}

/// What `played` (the tracks started since the show opened, the one on
/// air included) reveals of `items`.
pub fn reveal(items: &[Item], played: &HashSet<u64>, on_air: Option<u64>) -> Reveal {
    let mut found = Reveal::default();
    for item in items.iter().filter(|item| item.skipped.is_none()) {
        match item.kind {
            ItemKind::Track if played.contains(&item.track_id) => {
                if let Some(rank) = item.rank {
                    found.from_rank = Some(found.from_rank.map_or(rank, |from| from.min(rank)));
                    if on_air == Some(item.track_id) {
                        found.on_air_rank = Some(rank);
                    }
                }
            }
            ItemKind::NewRelease if played.contains(&item.track_id) => found.new_release = true,
            _ => {}
        }
    }
    found
}

#[derive(Clone, Debug, Serialize)]
struct PublicEntry {
    rank: u32,
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    movement: Option<Movement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_rank: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weeks: Option<u32>,
    on_air: bool,
}

#[derive(Clone, Debug, Serialize)]
struct PublicNewRelease {
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PublicChart {
    week: String,
    entries: Vec<PublicEntry>,
}

#[derive(Clone, Debug, Serialize)]
struct PublicState {
    chart: Option<PublicChart>,
    new_release: Option<PublicNewRelease>,
    live: bool,
    on_air_rank: Option<u32>,
    votes_count_for_unix_ms: Option<i64>,
}

/// The show's live state for the gateway: the chart of the last show that
/// started, its ranks still to come hidden while it plays (their tracks are
/// not in it), and the show the votes of now count for.
pub async fn public_state(
    pool: &MySqlPool,
    station_id: u64,
    show_settings: &ShowSettings,
    timezone: &str,
) -> ApiResult<serde_json::Value> {
    let tz = TimeZone::get(timezone).unwrap_or(TimeZone::UTC);
    let settings = load_settings(pool, station_id).await?;
    let now = Timestamp::now();
    let hours = show_settings.open_week();
    let this = ChartWeek::containing(now, &settings, &tz)?;
    let votes_count_for = (now < this.closes_at)
        .then(|| core_shows::next_opening(&hours, &tz, this.closes_at))
        .flatten()
        .map(|(opens, _)| opens.as_millisecond());
    let mut public = PublicState {
        chart: None,
        new_release: None,
        live: false,
        on_air_rank: None,
        votes_count_for_unix_ms: votes_count_for,
    };
    let last: Option<Kept> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {KEPT_COLUMNS} FROM top30_shows
         WHERE station_id = ? AND opens_at <= NOW(6) AND week IS NOT NULL
           AND JSON_LENGTH(programme, '$.items') > 0
         ORDER BY opens_at DESC LIMIT 1"
    )))
    .bind(station_id)
    .fetch_optional(pool)
    .await?;
    let Some(kept) = last else {
        return serde_json::to_value(public).map_err(internal);
    };
    let Some(monday) = kept
        .week
        .as_deref()
        .and_then(|week| week.parse::<Date>().ok())
    else {
        return serde_json::to_value(public).map_err(internal);
    };
    let programme = kept.programme()?;
    let entries = stored_entries(pool, station_id, monday).await?;
    let live = show_settings.enabled
        && core_shows::opening(&hours, &tz, now)
            .is_some_and(|(opens, _)| opens.as_millisecond() == kept.opens_ms);
    let found = if live {
        let played: HashSet<u64> = sqlx::query_scalar::<_, u64>(
            "SELECT DISTINCT track_id FROM play_history
             WHERE station_id = ? AND started_at >= FROM_UNIXTIME(?) AND deck <> 'fx'
               AND track_id IS NOT NULL",
        )
        .bind(station_id)
        .bind(kept.opens_ms as f64 / 1_000.0)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
        let on_air: Option<u64> = sqlx::query_scalar(
            "SELECT track_id FROM play_history
             WHERE station_id = ? AND ended_at IS NULL AND deck <> 'fx' AND track_id IS NOT NULL
             ORDER BY started_at DESC, id DESC LIMIT 1",
        )
        .bind(station_id)
        .fetch_optional(pool)
        .await?;
        reveal(&programme.items, &played, on_air)
    } else {
        Reveal {
            from_rank: Some(1),
            on_air_rank: None,
            new_release: true,
        }
    };
    let shown = |rank: u32| found.from_rank.is_some_and(|from| rank >= from);
    public.live = live;
    public.on_air_rank = found.on_air_rank;
    public.chart = Some(PublicChart {
        week: monday.to_string(),
        entries: entries
            .into_iter()
            .map(|entry| {
                if shown(entry.rank) {
                    PublicEntry {
                        rank: entry.rank,
                        hidden: false,
                        artist: entry.artist,
                        title: entry.title,
                        movement: Some(entry.movement),
                        previous_rank: entry.previous_rank,
                        weeks: Some(entry.weeks),
                        on_air: found.on_air_rank == Some(entry.rank),
                    }
                } else {
                    PublicEntry {
                        rank: entry.rank,
                        hidden: true,
                        artist: None,
                        title: None,
                        movement: None,
                        previous_rank: None,
                        weeks: None,
                        on_air: false,
                    }
                }
            })
            .collect(),
    });
    public.new_release = programme
        .items
        .iter()
        .find(|item| item.kind == ItemKind::NewRelease && item.skipped.is_none())
        .map(|item| {
            if found.new_release {
                PublicNewRelease {
                    hidden: false,
                    artist: item.artist.clone(),
                    title: item.title.clone(),
                }
            } else {
                PublicNewRelease {
                    hidden: true,
                    artist: None,
                    title: None,
                }
            }
        });
    serde_json::to_value(public).map_err(internal)
}

// ---------------------------------------------------------------------------
// In the background

/// Freezes each station's week as it closes, sending `top.published`, and
/// sends `top.rank` as ranks start playing. Times that passed while the
/// API was stopped send nothing. Never returns.
pub async fn run(pool: MySqlPool, stations: Arc<Stations>, webhooks: Arc<Webhooks>) {
    let mut last = Timestamp::now();
    loop {
        tokio::time::sleep(TICK).await;
        let now = Timestamp::now();
        for station in stations.all() {
            let code = station.code();
            if let Err(error) = tick(&pool, &webhooks, station.id(), &code, last, now).await {
                tracing::warn!(station = %code, %error, "top 30: cannot freeze or notify");
            }
        }
        last = now;
    }
}

/// `1. Artist – Title, 2. …`, the first three.
fn top3(entries: &[ChartEntry]) -> String {
    entries
        .iter()
        .take(3)
        .map(|entry| {
            format!(
                "{}. {}",
                entry.rank,
                track_text(&entry.artist, &entry.title)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn track_text(artist: &Option<String>, title: &Option<String>) -> String {
    [artist.as_deref(), title.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" – ")
}

/// The background work of one station between `from` and `to`.
pub(crate) async fn tick(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station_id: u64,
    code: &str,
    from: Timestamp,
    to: Timestamp,
) -> ApiResult<()> {
    if !crate::modules::is_on(pool, Some(station_id), Module::Top30).await? {
        return Ok(());
    }
    let settings = load_settings(pool, station_id).await?;
    let tz = time_zone(pool, station_id).await?;
    let show = top30_show();
    let week = ChartWeek::latest_closed(to, &settings, &tz)?;
    if frozen_votes(pool, station_id, week.monday).await?.is_none() {
        freeze(pool, station_id, &settings, &week).await?;
    }
    // Whoever froze it (a page read at the close), the week closed now.
    if from < week.closes_at && week.closes_at <= to {
        let entries = stored_entries(pool, station_id, week.monday).await?;
        if let Some(first) = entries.first() {
            let values = [
                ("artist", first.artist.clone().unwrap_or_default()),
                ("title", first.title.clone().unwrap_or_default()),
                ("top3", top3(&entries)),
            ];
            crate::shows::send_own_event(
                pool,
                webhooks,
                station_id,
                code,
                show,
                WebhookEvent::TopPublished,
                week.closes_at,
                &values,
            )
            .await?;
        }
    }
    // Ranks that started playing.
    let playing: Option<Kept> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {KEPT_COLUMNS} FROM top30_shows
         WHERE station_id = ? AND opens_at <= FROM_UNIXTIME(?)
           AND closes_at > FROM_UNIXTIME(?) - INTERVAL {OVERRUN_SECONDS} SECOND
         ORDER BY opens_at DESC LIMIT 1"
    )))
    .bind(station_id)
    .bind(seconds(to))
    .bind(seconds(from))
    .fetch_optional(pool)
    .await?;
    let Some(kept) = playing else {
        return Ok(());
    };
    let started: Vec<(u64, i64)> = sqlx::query_as(
        "SELECT track_id, CAST(UNIX_TIMESTAMP(started_at) * 1000 AS SIGNED) FROM play_history
         WHERE station_id = ? AND deck <> 'fx' AND track_id IS NOT NULL
           AND started_at > FROM_UNIXTIME(?) AND started_at <= FROM_UNIXTIME(?)
           AND started_at >= FROM_UNIXTIME(?)
         ORDER BY started_at, id",
    )
    .bind(station_id)
    .bind(seconds(from))
    .bind(seconds(to))
    .bind(kept.opens_ms as f64 / 1_000.0)
    .fetch_all(pool)
    .await?;
    if started.is_empty() {
        return Ok(());
    }
    let programme = kept.programme()?;
    for (track, at_ms) in started {
        let Some(item) = programme.items.iter().find(|item| {
            item.kind == ItemKind::Track && item.track_id == track && item.skipped.is_none()
        }) else {
            continue;
        };
        let values = [
            ("rank", item.rank.unwrap_or_default().to_string()),
            ("artist", item.artist.clone().unwrap_or_default()),
            ("title", item.title.clone().unwrap_or_default()),
        ];
        let at = Timestamp::from_millisecond(at_ms).unwrap_or(to);
        crate::shows::send_own_event(
            pool,
            webhooks,
            station_id,
            code,
            show,
            WebhookEvent::TopRank,
            at,
            &values,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use heron_rotation::{
        Category, LibraryParts, Member, Mode, Restriction, RotationSettings, Rule, Target, Track,
    };

    fn paris() -> TimeZone {
        TimeZone::get("Europe/Paris").unwrap()
    }

    fn at(text: &str) -> Timestamp {
        format!("{text}[Europe/Paris]")
            .parse::<jiff::Zoned>()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn a_week_runs_from_monday_to_its_close() {
        let settings = Top30Settings::default();
        let tz = paris();
        // Sunday 8 November 2026, 16:59: the week of the 2nd, still open.
        let week = ChartWeek::containing(at("2026-11-08T16:59"), &settings, &tz).unwrap();
        assert_eq!(week.key(), "2026-11-02");
        assert_eq!(week.from, at("2026-11-02T00:00"));
        assert_eq!(week.closes_at, at("2026-11-08T17:00"));
        let closed = ChartWeek::latest_closed(at("2026-11-08T16:59"), &settings, &tz).unwrap();
        assert_eq!(closed.key(), "2026-10-26");
        // From 17:00, it is closed: the show at 18:00 plays it.
        let closed = ChartWeek::latest_closed(at("2026-11-08T18:00"), &settings, &tz).unwrap();
        assert_eq!(closed.key(), "2026-11-02");
        // Closing at 24:00 on Sunday: the whole week.
        let midnight = Top30Settings {
            close_hour: 24,
            ..Top30Settings::default()
        };
        let week = ChartWeek::containing(at("2026-11-08T23:00"), &midnight, &tz).unwrap();
        assert_eq!(week.closes_at, at("2026-11-09T00:00"));
        // Across the end of summer time: 25 October 2026.
        let week = ChartWeek::containing(at("2026-10-25T12:00"), &settings, &tz).unwrap();
        assert_eq!(week.closes_at, at("2026-10-25T17:00"));
    }

    #[test]
    fn settings_are_checked() {
        assert!(Top30Settings::default().check().is_ok());
        let bad = |settings: Top30Settings| settings.check().is_err();
        assert!(bad(Top30Settings {
            close_day: 1,
            close_hour: 0,
            ..Top30Settings::default()
        }));
        assert!(bad(Top30Settings {
            size: 51,
            ..Top30Settings::default()
        }));
        assert!(bad(Top30Settings {
            min_votes: 0,
            ..Top30Settings::default()
        }));
        assert!(bad(Top30Settings {
            close_hour: 25,
            ..Top30Settings::default()
        }));
        assert!(parse_week("2026-11-02").is_ok());
        assert!(parse_week("2026-11-03").is_err());
    }

    #[test]
    fn movements_against_last_week() {
        assert_eq!(movement(2, Some(3), 1), Movement::Up);
        assert_eq!(movement(3, Some(1), 1), Movement::Down);
        assert_eq!(movement(1, Some(1), 4), Movement::Same);
        assert_eq!(movement(9, None, 2), Movement::Reentry);
        assert_eq!(movement(6, None, 0), Movement::New);
    }

    #[test]
    fn jingles_of_a_rank_end_with_it() {
        assert_eq!(rank_of_title("Top 30 – 12"), Some(12));
        assert_eq!(rank_of_title("Top 30 #7"), Some(7));
        assert_eq!(rank_of_title("Top 30 - 1 "), Some(1));
        assert_eq!(rank_of_title("Top 30"), None);
        assert_eq!(rank_of_title("Top 30 – opening"), None);
        assert_eq!(rank_of_title("#12"), None);
    }

    /// Tracks 1 to 40 of 3 minutes (jingles 101 to 103 of 10 seconds), in
    /// category 1 (music) or 2 (jingles).
    fn library(restrictions: Vec<Restriction>) -> Library {
        let track = |id: u64, length_ms: u32| Track {
            id,
            title_key: Some(format!("t{id}")),
            album_key: None,
            artists: vec![id],
            categories: Vec::new(),
            length_ms,
            mix_ms: None,
            ratio: 10,
            dayparts: None,
            daypart_days: None,
        };
        let mut tracks: Vec<Track> = (1..=40).map(|id| track(id, 180_000)).collect();
        tracks.extend((101..=103).map(|id| track(id, 10_000)));
        let members = |ids: Vec<u64>| -> Vec<Member> {
            ids.into_iter()
                .map(|track| Member { track, weight: 1.0 })
                .collect()
        };
        Library::new(
            LibraryParts {
                tracks,
                categories: vec![
                    Category {
                        id: 1,
                        name: "Music".to_owned(),
                        separation: Default::default(),
                        members: members((1..=40).collect()),
                    },
                    Category {
                        id: 2,
                        name: "Jingles".to_owned(),
                        separation: Default::default(),
                        members: members((101..=103).collect()),
                    },
                ],
                similar: Vec::new(),
                restrictions,
                clocks: Vec::new(),
                schedule: Default::default(),
                schedules: Vec::new(),
                settings: RotationSettings::default(),
                medley_category: None,
                medleys: Vec::new(),
                no_two_in_a_row: Vec::new(),
                counters: HashMap::new(),
                station_ratios: HashMap::new(),
                modules_off: Vec::new(),
            },
            paris(),
        )
    }

    fn kinds(programme: &Programme) -> Vec<(ItemKind, Option<u32>, u64)> {
        programme
            .items
            .iter()
            .filter(|item| item.skipped.is_none())
            .map(|item| (item.kind, item.rank, item.track_id))
            .collect()
    }

    #[test]
    fn the_show_counts_down_then_plays_the_new_release() {
        let library = library(Vec::new());
        let jingles = Jingles {
            by_rank: HashMap::from([(1, 103)]),
            others: vec![101, 102],
        };
        let opens = at("2026-11-08T18:00").as_millisecond();
        let closes = at("2026-11-08T20:00").as_millisecond();
        // Three ranked tracks: fewer than 30, the show counts down from 3.
        let ranks = [(1, 7), (2, 8), (3, 9)];
        let programme = plan(&library, &ranks, &jingles, Some(40), opens, closes);
        use ItemKind::{Jingle, NewRelease, Track as T};
        assert_eq!(
            kinds(&programme),
            [
                (Jingle, Some(3), 101),
                (T, Some(3), 9),
                (Jingle, Some(2), 102),
                (T, Some(2), 8),
                (Jingle, Some(1), 103),
                (T, Some(1), 7),
                (Jingle, None, 101),
                (NewRelease, None, 40),
            ]
        );
        assert!(programme.left_out.is_empty());
        // Planned one after the other.
        assert_eq!(programme.items[0].planned_at_unix_ms, opens);
        assert_eq!(programme.items[1].planned_at_unix_ms, opens + 10_000);
        assert_eq!(programme.items[2].planned_at_unix_ms, opens + 190_000);
        // Without jingles, the tracks follow one another.
        let programme = plan(&library, &ranks, &Jingles::default(), None, opens, closes);
        assert_eq!(
            kinds(&programme),
            [(T, Some(3), 9), (T, Some(2), 8), (T, Some(1), 7)]
        );
    }

    #[test]
    fn the_lowest_ranks_are_left_out_to_fit_the_slot() {
        let library = library(Vec::new());
        let jingles = Jingles {
            by_rank: HashMap::new(),
            others: vec![101],
        };
        let opens = at("2026-11-08T18:00").as_millisecond();
        let closes = at("2026-11-08T19:00").as_millisecond();
        // 30 tracks of 3 minutes do not fit in an hour.
        let ranks: Vec<(u32, u64)> = (1..=30).map(|rank| (rank, u64::from(rank))).collect();
        let programme = plan(&library, &ranks, &jingles, Some(40), opens, closes);
        // 17 tracks and the new release, each after 10 s of jingle: 57 min;
        // 18: 60 min 10 s.
        assert_eq!(programme.left_out, (18..=30).collect::<Vec<u32>>());
        let first = &programme.items[1];
        assert_eq!((first.kind, first.rank), (ItemKind::Track, Some(17)));
        let last = programme.items.last().unwrap();
        assert_eq!(last.kind, ItemKind::NewRelease);
        let end = last.planned_at_unix_ms + i64::from(last.length_ms);
        assert!(end <= closes);
    }

    #[test]
    fn restrictions_apply_in_the_show() {
        // Track 8 is banned on Sundays, jingle 102 too.
        let ban = |id: u64, track: u64| Restriction {
            id,
            target: Target::Track(track),
            rule: Rule::Daypart {
                mode: Mode::Ban,
                days: vec![7],
                start_hour: 0,
                end_hour: 24,
            },
        };
        let library = library(vec![ban(1, 8), ban(2, 102)]);
        let jingles = Jingles {
            by_rank: HashMap::new(),
            others: vec![101, 102],
        };
        let opens = at("2026-11-08T18:00").as_millisecond();
        let closes = at("2026-11-08T20:00").as_millisecond();
        let ranks = [(1, 7), (2, 8), (3, 9), (4, 999)];
        let programme = plan(&library, &ranks, &jingles, None, opens, closes);
        use ItemKind::{Jingle, Track as T};
        assert_eq!(
            kinds(&programme),
            [
                (Jingle, Some(3), 101),
                (T, Some(3), 9),
                (Jingle, Some(1), 101),
                (T, Some(1), 7),
            ]
        );
        let skipped: Vec<(Option<u32>, &str)> = programme
            .items
            .iter()
            .filter_map(|item| item.skipped.as_deref().map(|why| (item.rank, why)))
            .collect();
        assert_eq!(
            skipped,
            [
                (Some(4), "cannot play: disabled, missing or not analyzed"),
                (Some(2), "restriction 1 forbids it then"),
            ]
        );
    }

    fn item(kind: ItemKind, rank: Option<u32>, track_id: u64) -> Item {
        Item {
            kind,
            rank,
            track_id,
            artist: None,
            title: None,
            length_ms: 1_000,
            planned_at_unix_ms: 0,
            skipped: None,
            queued: false,
        }
    }

    #[test]
    fn the_queue_takes_a_jingle_with_its_track() {
        use ItemKind::{Jingle, NewRelease, Track as T};
        let mut items = vec![
            item(Jingle, Some(3), 101),
            item(T, Some(3), 9),
            item(T, Some(2), 8),
            item(Jingle, Some(1), 101),
            item(T, Some(1), 7),
            item(NewRelease, None, 40),
        ];
        items[2].skipped = Some("restriction".to_owned());
        let (taken, next) = take(&items, 0, 1);
        assert_eq!(
            taken.iter().map(|i| i.track_id).collect::<Vec<_>>(),
            [101, 9]
        );
        assert_eq!(next, 2);
        // The track passed over is not queued.
        let (taken, next) = take(&items, next, 2);
        assert_eq!(
            taken.iter().map(|i| i.track_id).collect::<Vec<_>>(),
            [101, 7]
        );
        assert_eq!(next, 5);
        let (taken, next) = take(&items, next, 3);
        assert_eq!(taken.iter().map(|i| i.track_id).collect::<Vec<_>>(), [40]);
        assert_eq!(next, 6);
        assert!(take(&items, next, 3).0.is_empty());
    }

    #[test]
    fn ranks_show_as_they_play() {
        use ItemKind::{Jingle, NewRelease, Track as T};
        let mut items = vec![
            item(Jingle, Some(4), 101),
            item(T, Some(4), 10),
            item(T, Some(3), 9),
            item(Jingle, Some(2), 101),
            item(T, Some(2), 8),
            item(T, Some(1), 7),
            item(NewRelease, None, 40),
        ];
        items[2].skipped = Some("restriction".to_owned());
        assert_eq!(reveal(&items, &HashSet::new(), None), Reveal::default());
        let played = HashSet::from([101, 10]);
        assert_eq!(
            reveal(&items, &played, Some(10)),
            Reveal {
                from_rank: Some(4),
                on_air_rank: Some(4),
                new_release: false
            }
        );
        // Number 3 was passed over: it shows once number 2 plays.
        let played = HashSet::from([101, 10, 8]);
        assert_eq!(reveal(&items, &played, Some(101)).from_rank, Some(2));
        let played = HashSet::from([101, 10, 8, 7, 40]);
        let found = reveal(&items, &played, Some(40));
        assert_eq!(found.from_rank, Some(1));
        assert_eq!(found.on_air_rank, None);
        assert!(found.new_release);
    }
}
