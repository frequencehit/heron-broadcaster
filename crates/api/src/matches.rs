// SPDX-License-Identifier: AGPL-3.0-or-later

//! The match (`docs/listener-interaction.md#match`): every N tracks in the
//! show's hours, listeners vote between two tracks of one category for the
//! next one.
//!
//! - As rotation fills the queue ([`planner`], from `queue.rs`), the track
//!   picked when a match is due carries the vote: an announcement jingle is
//!   linked to it, and the two tracks, picked by rotation from the match's
//!   category, are queued after it, both read ahead by the engine.
//! - [`tick`], every second, opens the vote when that track starts on air,
//!   and closes it a few seconds before the next transition: the loser
//!   leaves the queue, the winner plays next.
//! - Each device's vote comes from the gateway, its last one every second;
//!   the votes are counted per match, the devices' votes deleted an hour
//!   after it closes.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use heron_core::Module;
use heron_core::shows::{self as core_shows, Show, ShowSettings, Week};
use heron_rotation::{History, Library, Pick, Play, Rng};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::engine::PlayerState;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::gateway::GatewayClient;
use crate::settings::{self, StationSettings};
use crate::stations::{Station, StationRuntime, Stations};
use crate::webhooks::{WebhookEvent, Webhooks};

/// How often the matches are opened and closed.
const TICK: Duration = Duration::from_secs(1);
/// How often the devices' votes are deleted, in ticks.
const PURGE_EVERY_TICKS: u32 = 600;
/// The close waits this long for the gateway's batch on its way.
const GRACE_MS: i64 = 1_000;
/// A track shorter than this does not carry a vote.
const MIN_TRACK_MS: i64 = 60_000;
/// The shortest vote: a track shorter than this plus the seconds of the
/// close does not carry a vote.
const MIN_VOTE_MS: i64 = 30_000;
/// Results on the website: today's at most.
const TODAY: u32 = 20;
const MAX_HISTORY: u32 = 200;
/// Rows written per statement.
const CHUNK: usize = 1_000;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_match_settings, put_match_settings))
        .routes(routes!(current_match))
        .routes(routes!(close_match))
        .routes(routes!(cancel_match))
        .routes(routes!(match_history))
        .routes(routes!(gateway_match_votes))
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

fn seconds(ms: i64) -> f64 {
    ms as f64 / 1_000.0
}

// ---------------------------------------------------------------------------
// Settings

/// A station's match settings (`station_settings`, `match`). Its hours,
/// website and notifications are its show settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MatchSettings {
    /// A winner every N tracks, 2 to 20: the vote runs during the track
    /// before it.
    pub every_tracks: u32,
    /// The category the two tracks come from, the station's; none: no
    /// match.
    pub category_id: Option<u64>,
    /// The announcement over the intro of the vote's track; none: no
    /// jingle.
    pub jingle_category_id: Option<u64>,
    /// Seconds before the next transition the vote closes, 5 to 60.
    pub close_before_s: u32,
    /// The votes on the website as they come; off: the result only.
    pub live_results: bool,
    /// A loser can be one of the two tracks again the same day.
    pub loser_same_day: bool,
}

impl Default for MatchSettings {
    fn default() -> Self {
        Self {
            every_tracks: 4,
            category_id: None,
            jingle_category_id: None,
            close_before_s: 10,
            live_results: true,
            loser_same_day: false,
        }
    }
}

impl MatchSettings {
    pub const SETTINGS_NAME: &str = "match";

    fn check(&self) -> Result<(), String> {
        if !(2..=20).contains(&self.every_tracks) {
            return Err("every_tracks is 2 to 20".to_owned());
        }
        if !(5..=60).contains(&self.close_before_s) {
            return Err("close_before_s is 5 to 60".to_owned());
        }
        Ok(())
    }

    /// The shortest track that carries a vote.
    fn min_track_ms(&self) -> i64 {
        MIN_TRACK_MS.max(i64::from(self.close_before_s) * 1_000 + MIN_VOTE_MS)
    }
}

pub async fn load_settings(pool: &MySqlPool, station_id: u64) -> ApiResult<MatchSettings> {
    settings::load_for(pool, station_id, MatchSettings::SETTINGS_NAME).await
}

async fn time_zone(pool: &MySqlPool, station_id: u64) -> ApiResult<TimeZone> {
    let station: StationSettings =
        settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await?;
    Ok(station.time_zone())
}

/// The Match's entry among the shows.
static MATCH: Show = core_shows::SHOWS[1];

fn match_show() -> &'static Show {
    Show::of(Module::Match).unwrap_or(&MATCH)
}

async fn show_settings(pool: &MySqlPool, station_id: u64) -> ApiResult<ShowSettings> {
    crate::shows::load(pool, station_id, match_show()).await
}

/// The show's name on the website: its title, else its own.
fn show_name(show: &ShowSettings) -> String {
    Some(show.website.title.trim())
        .filter(|title| !title.is_empty())
        .unwrap_or(match_show().name)
        .to_owned()
}

/// The station's match settings.
#[utoipa::path(get, path = "/match/settings", tag = "match",
    responses((status = 200, body = MatchSettings), (status = "4XX", body = ErrorBody)))]
async fn get_match_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<MatchSettings>> {
    Ok(Json(load_settings(&state.pool, station.id()).await?))
}

/// Replaces the station's match settings. The categories must be the
/// station's (`422`). Operators.
#[utoipa::path(put, path = "/match/settings", tag = "match",
    request_body = MatchSettings,
    responses((status = 200, body = MatchSettings), (status = "4XX", body = ErrorBody)))]
async fn put_match_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(settings): Json<MatchSettings>,
) -> ApiResult<Json<MatchSettings>> {
    user.require(Role::Operator)?;
    settings.check().map_err(ApiError::Invalid)?;
    let pool = &state.pool;
    let ours: Vec<u64> =
        sqlx::query_scalar("SELECT category_id FROM category_stations WHERE station_id = ?")
            .bind(station.id())
            .fetch_all(pool)
            .await?;
    for id in [settings.category_id, settings.jingle_category_id]
        .into_iter()
        .flatten()
    {
        if !ours.contains(&id) {
            return Err(ApiError::Invalid(format!(
                "category {id} is not one of the station's"
            )));
        }
    }
    settings::save_for(pool, station.id(), MatchSettings::SETTINGS_NAME, &settings).await?;
    tracing::info!(user = %user.username, station = station.id(), "match settings changed");
    station.queue.wake();
    Ok(Json(settings))
}

// ---------------------------------------------------------------------------
// Planning, as rotation fills the queue

/// What the queue manager needs to plan a match while it fills the queue:
/// see [`planner`].
pub struct Planner {
    settings: MatchSettings,
    category: u64,
    week: Week,
    tz: TimeZone,
    /// When the last match's winner was planned to start.
    last_planned_ms: Option<i64>,
    /// Music tracks rotation queued since then.
    since: u32,
    /// Tracks that cannot be one of the two: today's losers.
    excluded: Vec<u64>,
    /// Categories whose type sends no metadata (jingles).
    silent: HashSet<u64>,
}

/// A match planned among rotation's picks: where its tracks are in them.
#[derive(Clone, Debug)]
pub struct Planned {
    pub category: u64,
    pub vote: usize,
    pub jingle: Option<usize>,
    pub a: usize,
    pub b: usize,
    /// When the winner should start (Unix ms).
    pub planned_at_ms: i64,
}

/// The opening range `at` is in: when it opened and closes. Hours open all
/// week long count a day at a time, from midnight.
fn opening(week: &Week, tz: &TimeZone, at: Timestamp) -> Option<(Timestamp, Timestamp)> {
    if let Some(range) = core_shows::opening(week, tz, at) {
        return Some(range);
    }
    if !core_shows::status(week, tz, at).open {
        return None;
    }
    let today = at.to_zoned(tz.clone()).start_of_day().ok()?;
    let tomorrow = today.tomorrow().ok()?.start_of_day().ok()?;
    Some((today.timestamp(), tomorrow.timestamp()))
}

/// Music: in no category whose type sends no metadata.
const MUSIC: &str = "NOT EXISTS (SELECT 1 FROM track_categories tc
                     JOIN categories c ON c.id = tc.category_id
                     JOIN category_types ct ON ct.id = c.type_id
                     WHERE tc.track_id = p.track_id AND NOT ct.sends_metadata)";

/// The planner of the station's matches, when one may be queued: the
/// module on, the show enabled with a category, and no match waiting or
/// open.
pub async fn planner(pool: &MySqlPool, station_id: u64) -> ApiResult<Option<Planner>> {
    if !crate::modules::is_on(pool, Some(station_id), Module::Match).await? {
        return Ok(None);
    }
    let settings = load_settings(pool, station_id).await?;
    let Some(category) = settings.category_id else {
        return Ok(None);
    };
    let show = show_settings(pool, station_id).await?;
    let week = show.open_week();
    if week.is_empty() {
        return Ok(None);
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM matches WHERE station_id = ? AND status IN ('queued', 'open')",
    )
    .bind(station_id)
    .fetch_one(pool)
    .await?;
    if pending > 0 {
        return Ok(None);
    }
    let tz = time_zone(pool, station_id).await?;
    let last_planned_ms: Option<i64> = sqlx::query_scalar(
        "SELECT CAST(UNIX_TIMESTAMP(planned_at) * 1000 AS SIGNED) FROM matches
         WHERE station_id = ? ORDER BY planned_at DESC, id DESC LIMIT 1",
    )
    .bind(station_id)
    .fetch_optional(pool)
    .await?;
    let since: i64 = match last_planned_ms {
        Some(last) => {
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM rotation_picks p
                 WHERE p.station_id = ? AND p.planned_at > FROM_UNIXTIME(?) AND {MUSIC}"
            )))
            .bind(station_id)
            .bind(seconds(last))
            .fetch_one(pool)
            .await?
        }
        None => 0,
    };
    let excluded = if settings.loser_same_day {
        Vec::new()
    } else {
        let midnight = Timestamp::now()
            .to_zoned(tz.clone())
            .start_of_day()
            .map_err(internal)?
            .timestamp();
        sqlx::query_scalar::<_, Option<u64>>(
            "SELECT IF(winner = 'a', track_b_id, track_a_id) FROM matches
             WHERE station_id = ? AND status = 'closed' AND closed_at >= FROM_UNIXTIME(?)",
        )
        .bind(station_id)
        .bind(seconds(midnight.as_millisecond()))
        .fetch_all(pool)
        .await?
        .into_iter()
        .flatten()
        .collect()
    };
    let silent: Vec<u64> = sqlx::query_scalar(
        "SELECT c.id FROM categories c JOIN category_types ct ON ct.id = c.type_id
         WHERE NOT ct.sends_metadata",
    )
    .fetch_all(pool)
    .await?;
    Ok(Some(Planner {
        settings,
        category,
        week,
        tz,
        last_planned_ms,
        since: u32::try_from(since).unwrap_or(u32::MAX),
        excluded,
        silent: silent.into_iter().collect(),
    }))
}

impl Planner {
    /// Whether a match is due for a vote's track starting at `start_ms`
    /// and lasting `length_ms`: in the hours, the whole track in its range,
    /// long enough, and every N tracks (or the first of the range).
    fn due(&self, start_ms: i64, length_ms: i64) -> bool {
        let Ok(start) = Timestamp::from_millisecond(start_ms) else {
            return false;
        };
        let Some((opens, closes)) = opening(&self.week, &self.tz, start) else {
            return false;
        };
        let first = self
            .last_planned_ms
            .is_none_or(|last| last < opens.as_millisecond());
        let counted = first || self.since + 2 >= self.settings.every_tracks;
        counted
            && length_ms >= self.settings.min_track_ms()
            && start_ms + length_ms <= closes.as_millisecond()
    }

    /// After rotation's last pick (the end of `picks`, planned to end at
    /// `at`): when it carries a match, links the announcement to it and
    /// adds the two tracks after it. Otherwise counts it.
    pub fn after(
        &mut self,
        library: &Library,
        history: &History,
        at: i64,
        picks: &mut Vec<Pick>,
        linked: &mut Vec<u32>,
        rng: &mut Rng,
    ) -> Option<Planned> {
        let vote = picks.last()?;
        let track = library.tracks.get(&vote.track)?;
        if track.categories.iter().any(|c| self.silent.contains(c)) {
            return None;
        }
        let length = i64::from(track.length_ms);
        if !self.due(at - length, length) {
            self.since = self.since.saturating_add(1);
            return None;
        }
        let a = heron_rotation::from_category(
            library,
            history,
            at,
            self.category,
            &self.excluded,
            true,
            rng,
        )?;
        let mut with_a = history.clone();
        if let Some(track) = library.tracks.get(&a.track) {
            with_a.push(Play::of(track, at));
        }
        let mut excluded = self.excluded.clone();
        excluded.push(a.track);
        let b = heron_rotation::from_category(
            library,
            &with_a,
            at,
            self.category,
            &excluded,
            true,
            rng,
        )?;
        // The announcement, in turn and without separation rules, takes the
        // place of a linked slot's jingle.
        let announcement = self.settings.jingle_category_id.and_then(|jingles| {
            heron_rotation::from_category(library, history, at - length, jingles, &[], false, rng)
        });
        let mut vote = picks.len() - 1;
        let had_jingle = vote > 0 && linked.contains(&u32::try_from(vote - 1).unwrap_or(u32::MAX));
        let jingle = match (announcement, had_jingle) {
            (Some(mut announcement), true) => {
                announcement.explanation.linked = true;
                picks[vote - 1] = announcement;
                Some(vote - 1)
            }
            (Some(mut announcement), false) => {
                announcement.explanation.linked = true;
                picks.insert(vote, announcement);
                linked.push(u32::try_from(vote).unwrap_or(u32::MAX));
                vote += 1;
                Some(vote - 1)
            }
            (None, true) => Some(vote - 1),
            (None, false) => None,
        };
        picks.push(a);
        picks.push(b);
        Some(Planned {
            category: self.category,
            vote,
            jingle,
            a: vote + 1,
            b: vote + 2,
            planned_at_ms: at,
        })
    }
}

/// Records a match just queued: its picks became `entries`, logged as
/// `log_ids`.
pub async fn queued(
    pool: &MySqlPool,
    station_id: u64,
    planned: &Planned,
    picks: &[Pick],
    engine_started_at: u64,
    entries: &[u64],
    log_ids: &[u64],
) -> ApiResult<u64> {
    let entry = |index: usize| entries.get(index).copied();
    let (Some(vote_entry), Some(entry_a), Some(entry_b)) =
        (entry(planned.vote), entry(planned.a), entry(planned.b))
    else {
        return Err(internal("a match's tracks were not all queued"));
    };
    let id = sqlx::query(
        "INSERT INTO matches
             (station_id, status, category_id, vote_track_id, jingle_track_id, track_a_id,
              track_b_id, engine_started_at, vote_entry_id, entry_a_id, entry_b_id,
              pick_a_id, pick_b_id, planned_at, created_at)
         VALUES (?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, FROM_UNIXTIME(?), NOW(6))",
    )
    .bind(station_id)
    .bind(planned.category)
    .bind(picks[planned.vote].track)
    .bind(planned.jingle.map(|index| picks[index].track))
    .bind(picks[planned.a].track)
    .bind(picks[planned.b].track)
    .bind(engine_started_at)
    .bind(vote_entry)
    .bind(entry_a)
    .bind(entry_b)
    .bind(log_ids.get(planned.a).copied())
    .bind(log_ids.get(planned.b).copied())
    .bind(seconds(planned.planned_at_ms))
    .execute(pool)
    .await?
    .last_insert_id();
    tracing::info!(
        station = station_id,
        r#match = id,
        a = picks[planned.a].track,
        b = picks[planned.b].track,
        "match queued"
    );
    Ok(id)
}

// ---------------------------------------------------------------------------
// The matches

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchStatus {
    /// Its tracks are queued; the vote opens when the vote's track starts.
    Queued,
    Open,
    Closed,
    Cancelled,
}

impl MatchStatus {
    fn parse(text: &str) -> Self {
        match text {
            "open" => Self::Open,
            "closed" => Self::Closed,
            "cancelled" => Self::Cancelled,
            _ => Self::Queued,
        }
    }
}

/// One of the two tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchSide {
    A,
    B,
}

impl MatchSide {
    fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "a" => Some(Self::A),
            "b" => Some(Self::B),
            _ => None,
        }
    }

    fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

/// How the winner was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchDecision {
    /// The most votes.
    Votes,
    /// A tie: the least recently played of the two.
    Tie,
    /// No vote: the least recently played of the two.
    NoVotes,
    /// An operator closed the vote early, with the votes so far.
    Early,
}

impl MatchDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Votes => "votes",
            Self::Tie => "tie",
            Self::NoVotes => "no_votes",
            Self::Early => "early",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "votes" => Some(Self::Votes),
            "tie" => Some(Self::Tie),
            "no_votes" => Some(Self::NoVotes),
            "early" => Some(Self::Early),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct Row {
    id: u64,
    station_id: u64,
    status: String,
    category_id: Option<u64>,
    category_name: Option<String>,
    vote_track_id: Option<u64>,
    vote_artist: Option<String>,
    vote_title: Option<String>,
    jingle_track_id: Option<u64>,
    jingle_artist: Option<String>,
    jingle_title: Option<String>,
    track_a_id: Option<u64>,
    a_artist: Option<String>,
    a_title: Option<String>,
    track_b_id: Option<u64>,
    b_artist: Option<String>,
    b_title: Option<String>,
    engine_started_at: u64,
    vote_entry_id: u64,
    entry_a_id: u64,
    entry_b_id: u64,
    pick_a_id: Option<u64>,
    pick_b_id: Option<u64>,
    opened_ms: Option<i64>,
    closes_ms: Option<i64>,
    transition_ms: Option<i64>,
    closed_ms: Option<i64>,
    votes_a: u32,
    votes_b: u32,
    winner: Option<String>,
    decided_by: Option<String>,
}

const COLUMNS: &str = "m.id, m.station_id, CAST(m.status AS CHAR) AS status, m.category_id,
    c.name AS category_name,
    m.vote_track_id, tv.artist AS vote_artist, tv.title AS vote_title,
    m.jingle_track_id, tj.artist AS jingle_artist, tj.title AS jingle_title,
    m.track_a_id, ta.artist AS a_artist, ta.title AS a_title,
    m.track_b_id, tb.artist AS b_artist, tb.title AS b_title,
    m.engine_started_at, m.vote_entry_id, m.entry_a_id, m.entry_b_id, m.pick_a_id, m.pick_b_id,
    CAST(UNIX_TIMESTAMP(m.opened_at) * 1000 AS SIGNED) AS opened_ms,
    CAST(UNIX_TIMESTAMP(m.closes_at) * 1000 AS SIGNED) AS closes_ms,
    CAST(UNIX_TIMESTAMP(m.transition_at) * 1000 AS SIGNED) AS transition_ms,
    CAST(UNIX_TIMESTAMP(m.closed_at) * 1000 AS SIGNED) AS closed_ms,
    m.votes_a, m.votes_b, CAST(m.winner AS CHAR) AS winner,
    CAST(m.decided_by AS CHAR) AS decided_by";

const FROM: &str = "FROM matches m
    LEFT JOIN categories c ON c.id = m.category_id
    LEFT JOIN tracks tv ON tv.id = m.vote_track_id
    LEFT JOIN tracks tj ON tj.id = m.jingle_track_id
    LEFT JOIN tracks ta ON ta.id = m.track_a_id
    LEFT JOIN tracks tb ON tb.id = m.track_b_id";

impl Row {
    fn status(&self) -> MatchStatus {
        MatchStatus::parse(&self.status)
    }

    fn winner(&self) -> Option<MatchSide> {
        self.winner.as_deref().and_then(MatchSide::parse)
    }

    fn entry(&self, choice: MatchSide) -> u64 {
        match choice {
            MatchSide::A => self.entry_a_id,
            MatchSide::B => self.entry_b_id,
        }
    }

    fn track_id(&self, choice: MatchSide) -> Option<u64> {
        match choice {
            MatchSide::A => self.track_a_id,
            MatchSide::B => self.track_b_id,
        }
    }

    fn votes(&self, choice: MatchSide) -> u32 {
        match choice {
            MatchSide::A => self.votes_a,
            MatchSide::B => self.votes_b,
        }
    }

    fn pick(&self, choice: MatchSide) -> Option<u64> {
        match choice {
            MatchSide::A => self.pick_a_id,
            MatchSide::B => self.pick_b_id,
        }
    }

    fn track(&self, choice: MatchSide) -> MatchTrack {
        match choice {
            MatchSide::A => MatchTrack {
                track_id: self.track_a_id,
                artist: self.a_artist.clone(),
                title: self.a_title.clone(),
            },
            MatchSide::B => MatchTrack {
                track_id: self.track_b_id,
                artist: self.b_artist.clone(),
                title: self.b_title.clone(),
            },
        }
    }

    fn total(&self) -> u32 {
        self.votes_a + self.votes_b
    }

    fn choice(&self, choice: MatchSide) -> MatchChoice {
        let track = self.track(choice);
        MatchChoice {
            choice,
            track_id: track.track_id,
            artist: track.artist,
            title: track.title,
            votes: self.votes(choice),
            percent: percent(self.votes(choice), self.total()),
        }
    }
}

/// `part` of `total` in whole percents, 0 without votes.
fn percent(part: u32, total: u32) -> u32 {
    if total == 0 {
        0
    } else {
        ((f64::from(part) * 100.0 / f64::from(total)).round()) as u32
    }
}

async fn rows(pool: &MySqlPool, condition: &str, station_id: u64) -> ApiResult<Vec<Row>> {
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} {FROM} WHERE m.station_id = ? AND {condition} ORDER BY m.id DESC"
    )))
    .bind(station_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// The station's match waiting for its vote or open, if any.
async fn pending(pool: &MySqlPool, station_id: u64) -> ApiResult<Option<Row>> {
    Ok(rows(pool, "m.status IN ('queued', 'open')", station_id)
        .await?
        .into_iter()
        .next())
}

async fn row(pool: &MySqlPool, id: u64) -> ApiResult<Row> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} {FROM} WHERE m.id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("match {id} not found")))
}

/// A track of a match.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MatchTrack {
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
}

/// One of the two tracks, with its votes.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MatchChoice {
    pub choice: MatchSide,
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub votes: u32,
    /// Its share of the votes, 0 to 100.
    pub percent: u32,
}

/// A match waiting for its vote or open, for operators.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MatchView {
    pub id: u64,
    pub status: MatchStatus,
    pub category_id: Option<u64>,
    pub category_name: Option<String>,
    /// The track the vote runs during.
    pub vote_track: MatchTrack,
    /// The announcement over its intro.
    pub jingle: Option<MatchTrack>,
    /// A, then B.
    pub choices: Vec<MatchChoice>,
    pub votes: u32,
    /// When the vote's track started.
    pub opened_at_unix_ms: Option<i64>,
    pub closes_at_unix_ms: Option<i64>,
    /// The next transition, as planned when the vote opened.
    pub transition_at_unix_ms: Option<i64>,
    /// Once decided.
    pub winner: Option<MatchSide>,
    pub decided_by: Option<MatchDecision>,
    pub closed_at_unix_ms: Option<i64>,
}

impl From<&Row> for MatchView {
    fn from(row: &Row) -> Self {
        Self {
            id: row.id,
            status: row.status(),
            category_id: row.category_id,
            category_name: row.category_name.clone(),
            vote_track: MatchTrack {
                track_id: row.vote_track_id,
                artist: row.vote_artist.clone(),
                title: row.vote_title.clone(),
            },
            jingle: row.jingle_track_id.map(|id| MatchTrack {
                track_id: Some(id),
                artist: row.jingle_artist.clone(),
                title: row.jingle_title.clone(),
            }),
            choices: vec![row.choice(MatchSide::A), row.choice(MatchSide::B)],
            votes: row.total(),
            opened_at_unix_ms: row.opened_ms,
            closes_at_unix_ms: row.closes_ms,
            transition_at_unix_ms: row.transition_ms,
            winner: row.winner(),
            decided_by: row.decided_by.as_deref().and_then(MatchDecision::parse),
            closed_at_unix_ms: row.closed_ms,
        }
    }
}

/// The match waiting for its vote or open, else `null`.
#[utoipa::path(get, path = "/match/current", tag = "match",
    responses((status = 200, body = Option<MatchView>), (status = "4XX", body = ErrorBody)))]
async fn current_match(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Option<MatchView>>> {
    let found = pending(&state.pool, station.id()).await?;
    Ok(Json(found.as_ref().map(MatchView::from)))
}

/// Closes the vote now and decides with the votes so far. `409` while the
/// vote is not open. Operators.
#[utoipa::path(post, path = "/match/current/close", tag = "match",
    responses((status = 200, body = MatchView), (status = "4XX", body = ErrorBody)))]
async fn close_match(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<MatchView>> {
    user.require(Role::Operator)?;
    let found = pending(&state.pool, station.id())
        .await?
        .ok_or_else(|| ApiError::NotFound("no match in progress".to_owned()))?;
    if found.status() != MatchStatus::Open {
        return Err(ApiError::Conflict("the vote is not open yet".to_owned()));
    }
    let player = station
        .engine
        .state()
        .borrow()
        .clone()
        .ok_or(ApiError::EngineUnavailable)?;
    close(
        &state.pool,
        &state.webhooks,
        &station,
        &player,
        &found,
        Some(user.id),
    )
    .await?;
    tracing::info!(user = %user.username, station = station.id(), r#match = found.id, "match closed early");
    Ok(Json(MatchView::from(&row(&state.pool, found.id).await?)))
}

/// Cancels the match: the first track stays in the queue as an ordinary
/// track, the second leaves it. Operators.
#[utoipa::path(post, path = "/match/current/cancel", tag = "match",
    responses((status = 200, body = MatchView), (status = "4XX", body = ErrorBody)))]
async fn cancel_match(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<MatchView>> {
    user.require(Role::Operator)?;
    let found = pending(&state.pool, station.id())
        .await?
        .ok_or_else(|| ApiError::NotFound("no match in progress".to_owned()))?;
    if !cancel(&state.pool, &station, &found, Some(user.id), true).await? {
        return Err(ApiError::Conflict(
            "the match is already decided".to_owned(),
        ));
    }
    tracing::info!(user = %user.username, station = station.id(), r#match = found.id, "match cancelled");
    Ok(Json(MatchView::from(&row(&state.pool, found.id).await?)))
}

#[derive(Clone, Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HistoryQuery {
    /// 1 to 200, 50 by default.
    pub limit: Option<u32>,
    /// The matches before this ID: the next page.
    pub before: Option<u64>,
}

/// The week's matches and votes.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MatchWeek {
    /// Monday 00:00, in the station's time zone.
    pub from_unix_ms: i64,
    pub matches: u64,
    pub votes: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MatchHistory {
    /// Decided or cancelled, the latest first.
    pub matches: Vec<MatchView>,
    pub week: MatchWeek,
}

/// The matches decided or cancelled, the latest first, and the week's
/// matches and votes.
#[utoipa::path(get, path = "/match/history", tag = "match",
    params(HistoryQuery),
    responses((status = 200, body = MatchHistory), (status = "4XX", body = ErrorBody)))]
async fn match_history(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<MatchHistory>> {
    let pool = &state.pool;
    let limit = query.limit.unwrap_or(50);
    if !(1..=MAX_HISTORY).contains(&limit) {
        return Err(ApiError::Invalid(format!("limit is 1 to {MAX_HISTORY}")));
    }
    let found: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} {FROM}
         WHERE m.station_id = ? AND m.status IN ('closed', 'cancelled') AND m.id < ?
         ORDER BY m.id DESC LIMIT ?"
    )))
    .bind(station.id())
    .bind(query.before.unwrap_or(u64::MAX))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let tz = time_zone(pool, station.id()).await?;
    let from = crate::likes::week_start(Timestamp::now(), &tz)?;
    let (matches, votes): (i64, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), CAST(SUM(votes_a + votes_b) AS SIGNED) FROM matches
         WHERE station_id = ? AND status = 'closed' AND closed_at >= FROM_UNIXTIME(?)",
    )
    .bind(station.id())
    .bind(seconds(from.as_millisecond()))
    .fetch_one(pool)
    .await?;
    Ok(Json(MatchHistory {
        matches: found.iter().map(MatchView::from).collect(),
        week: MatchWeek {
            from_unix_ms: from.as_millisecond(),
            matches: u64::try_from(matches).unwrap_or(0),
            votes: votes.and_then(|v| u64::try_from(v).ok()).unwrap_or(0),
        },
    }))
}

// ---------------------------------------------------------------------------
// Votes, through the gateway

/// What a listener sent.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct MatchVoteFields {
    pub match_id: u64,
    pub choice: MatchSide,
}

/// A device's last vote.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct MatchDeviceVote {
    /// The gateway's hash of the device.
    pub device: String,
    pub fields: MatchVoteFields,
}

/// Each device's last vote since the last batch.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct MatchVoteBatch {
    pub writes: Vec<MatchDeviceVote>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct MatchVotesTaken {
    /// The votes counted.
    pub counted: u64,
    /// Those dropped: for a match that is not open, or another station's.
    pub dropped: u64,
}

/// Each device's last vote, from the gateway, every second. A vote counts
/// while its match is open; a device's vote replaces its earlier one.
#[utoipa::path(post, path = "/gateway/stations/{code}/match/vote", tag = "gateway", security(()),
    params(("code" = String, Path, description = "The station's code")),
    request_body = MatchVoteBatch,
    responses((status = 200, body = MatchVotesTaken), (status = "4XX", body = ErrorBody)))]
async fn gateway_match_votes(
    _gateway: GatewayClient,
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(batch): Json<MatchVoteBatch>,
) -> ApiResult<Json<MatchVotesTaken>> {
    let (station, _) = crate::gateway::open_show(&state, &code, Module::Match).await?;
    Ok(Json(
        count_votes(&state.pool, station.id(), &batch.writes).await?,
    ))
}

/// Keeps each device's vote on the open match and counts its votes again.
pub async fn count_votes(
    pool: &MySqlPool,
    station_id: u64,
    writes: &[MatchDeviceVote],
) -> ApiResult<MatchVotesTaken> {
    let open: Option<u64> = sqlx::query_scalar(
        "SELECT id FROM matches WHERE station_id = ? AND status = 'open' ORDER BY id DESC LIMIT 1",
    )
    .bind(station_id)
    .fetch_optional(pool)
    .await?;
    let valid = |device: &str| {
        !device.is_empty() && device.len() <= 128 && device.bytes().all(|b| b.is_ascii_graphic())
    };
    let counted: Vec<&MatchDeviceVote> = writes
        .iter()
        .filter(|write| Some(write.fields.match_id) == open && valid(&write.device))
        .collect();
    let dropped = (writes.len() - counted.len()) as u64;
    let Some(id) = open.filter(|_| !counted.is_empty()) else {
        return Ok(MatchVotesTaken {
            counted: 0,
            dropped,
        });
    };
    let mut tx = pool.begin().await?;
    for chunk in counted.chunks(CHUNK) {
        let mut query =
            QueryBuilder::new("INSERT INTO match_votes (match_id, device, choice, voted_at) ");
        query.push_values(chunk, |mut row, write| {
            row.push_bind(id)
                .push_bind(&write.device)
                .push_bind(write.fields.choice.as_str())
                .push("NOW(6)");
        });
        query.push(" ON DUPLICATE KEY UPDATE choice = VALUES(choice), voted_at = VALUES(voted_at)");
        query.build().execute(&mut *tx).await?;
    }
    recount(&mut *tx, id).await?;
    tx.commit().await?;
    Ok(MatchVotesTaken {
        counted: counted.len() as u64,
        dropped,
    })
}

/// Counts a match's votes from its devices' votes.
async fn recount(executor: impl sqlx::MySqlExecutor<'_>, id: u64) -> ApiResult<()> {
    sqlx::query(
        "UPDATE matches m SET
             votes_a = (SELECT COUNT(*) FROM match_votes v WHERE v.match_id = m.id AND v.choice = 'a'),
             votes_b = (SELECT COUNT(*) FROM match_votes v WHERE v.match_id = m.id AND v.choice = 'b')
         WHERE m.id = ? AND m.status = 'open'",
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(())
}

/// Deletes the devices' votes once they cannot change any more: an hour
/// after their match closes, a day after the vote at the latest. Whether
/// the module is on or not: it is a privacy rule.
pub async fn purge_devices(pool: &MySqlPool) -> ApiResult<u64> {
    let closed = sqlx::query(
        "DELETE v FROM match_votes v JOIN matches m ON m.id = v.match_id
         WHERE m.status IN ('closed', 'cancelled') AND m.closed_at < NOW(6) - INTERVAL 1 HOUR",
    )
    .execute(pool)
    .await?
    .rows_affected();
    let old = sqlx::query("DELETE FROM match_votes WHERE voted_at < NOW(6) - INTERVAL 1 DAY")
        .execute(pool)
        .await?
        .rows_affected();
    Ok(closed + old)
}

// ---------------------------------------------------------------------------
// Opening and closing

/// Whether a queue entry of the engine started at `started_at` is still
/// there: waiting, or on a deck.
fn present(player: &PlayerState, started_at: u64, entry: u64) -> bool {
    player.engine_started_at_unix_ms == started_at
        && (player.waiting().any(|e| e.entry_id == entry)
            || player
                .decks
                .iter()
                .filter_map(|deck| deck.entry.as_ref())
                .any(|e| e.entry_id == entry))
}

fn waiting(player: &PlayerState, started_at: u64, entry: u64) -> bool {
    player.engine_started_at_unix_ms == started_at && player.waiting().any(|e| e.entry_id == entry)
}

/// The deck playing the vote's track on air, if it does.
fn on_air<'p>(player: &'p PlayerState, row: &Row) -> Option<&'p crate::engine::DeckState> {
    if player.engine_started_at_unix_ms != row.engine_started_at {
        return None;
    }
    player.decks.iter().find(|deck| {
        deck.on_air
            && deck
                .entry
                .as_ref()
                .is_some_and(|entry| entry.entry_id == row.vote_entry_id)
    })
}

/// Opens, closes or cancels the stations' matches as their time comes, and
/// deletes the devices' votes once in a while.
pub(crate) async fn tick(
    pool: &MySqlPool,
    stations: &Stations,
    webhooks: &Webhooks,
) -> ApiResult<()> {
    let active: Vec<u64> = sqlx::query_scalar(
        "SELECT DISTINCT station_id FROM matches WHERE status IN ('queued', 'open')",
    )
    .fetch_all(pool)
    .await?;
    for station_id in active {
        let Some(station) = stations.get(station_id) else {
            continue;
        };
        if let Err(error) = tick_station(pool, webhooks, &station).await {
            tracing::warn!(station = station_id, %error, "cannot open or close the match");
        }
    }
    Ok(())
}

async fn tick_station(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station: &StationRuntime,
) -> ApiResult<()> {
    let Some(found) = pending(pool, station.id()).await? else {
        return Ok(());
    };
    let player = station.engine.state().borrow().clone();
    let Some(player) = player else {
        return Ok(());
    };
    if !crate::modules::is_on(pool, Some(station.id()), Module::Match).await? {
        cancel(pool, station, &found, None, true).await?;
        return Ok(());
    }
    if player.engine_started_at_unix_ms != found.engine_started_at {
        // A new engine: its entries are not ours any more.
        cancel(pool, station, &found, None, false).await?;
        return Ok(());
    }
    let now = crate::rotation::now_ms();
    match found.status() {
        MatchStatus::Queued => {
            if let Some(deck) = on_air(&player, &found) {
                open(pool, webhooks, station, &found, deck, now).await?;
            } else if !present(&player, found.engine_started_at, found.vote_entry_id) {
                // The vote's track was taken out, or ended without us.
                cancel(pool, station, &found, None, true).await?;
            }
        }
        MatchStatus::Open => {
            if on_air(&player, &found).is_none() {
                // Skipped: the next track has started already.
                cancel(pool, station, &found, None, true).await?;
            } else if found
                .closes_ms
                .is_none_or(|closes| now >= closes + GRACE_MS)
            {
                close(pool, webhooks, station, &player, &found, None).await?;
            }
        }
        MatchStatus::Closed | MatchStatus::Cancelled => {}
    }
    Ok(())
}

/// Opens the vote: the vote's track is on air on `deck`. It closes the
/// set seconds before the next transition, at the track's mix point.
async fn open(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station: &StationRuntime,
    found: &Row,
    deck: &crate::engine::DeckState,
    now: i64,
) -> ApiResult<()> {
    let settings = load_settings(pool, station.id()).await?;
    // The position as the engine has it now, not as last reported.
    let fresh = station.engine.fetch_state().await.ok();
    let deck = fresh
        .as_ref()
        .and_then(|player| on_air(player, found))
        .unwrap_or(deck);
    let now = if fresh.is_some() {
        crate::rotation::now_ms()
    } else {
        now
    };
    let length = i64::try_from(deck.length_ms).unwrap_or(i64::MAX);
    let mix = match found.vote_track_id {
        Some(id) => match station.queue.playout(id, None).await {
            Ok(playout) => playout
                .mix_point_ms
                .filter(|mix| *mix > playout.cue_in_ms)
                .map(|mix| i64::from(mix - playout.cue_in_ms))
                .filter(|mix| *mix < length),
            Err(_) => None,
        },
        None => None,
    };
    let position = i64::try_from(deck.position_ms).unwrap_or(0);
    let transition = now + (mix.unwrap_or(length) - position).max(0);
    let closes = transition - i64::from(settings.close_before_s) * 1_000;
    let started = now - position;
    let opened = sqlx::query(
        "UPDATE matches SET status = 'open', opened_at = FROM_UNIXTIME(?),
             closes_at = FROM_UNIXTIME(?), transition_at = FROM_UNIXTIME(?)
         WHERE id = ? AND status = 'queued'",
    )
    .bind(seconds(started))
    .bind(seconds(closes))
    .bind(seconds(transition))
    .bind(found.id)
    .execute(pool)
    .await?
    .rows_affected();
    if opened == 0 {
        return Ok(());
    }
    tracing::info!(station = station.id(), r#match = found.id, "match open");
    let opened = row(pool, found.id).await?;
    let values = placeholders(&opened, None, &time_zone(pool, station.id()).await?, "en");
    notify(pool, webhooks, station, WebhookEvent::MatchStarted, values).await;
    Ok(())
}

/// The least recently played of the two on the station (never played
/// first), else one at random.
async fn least_recently_played(pool: &MySqlPool, found: &Row) -> ApiResult<MatchSide> {
    let last = |track: Option<u64>| async move {
        let Some(track) = track else {
            return Ok::<_, ApiError>(None);
        };
        let at: Option<i64> = sqlx::query_scalar(
            "SELECT CAST(UNIX_TIMESTAMP(MAX(started_at)) * 1000 AS SIGNED) FROM play_history
             WHERE station_id = ? AND track_id = ?",
        )
        .bind(found.station_id)
        .bind(track)
        .fetch_one(pool)
        .await?;
        Ok(at)
    };
    let a = last(found.track_a_id).await?;
    let b = last(found.track_b_id).await?;
    Ok(tie_break(a, b, crate::rotation::seed()))
}

/// The least recently played of two (`None`: never played), else `seed`'s
/// pick.
fn tie_break(a: Option<i64>, b: Option<i64>, seed: u64) -> MatchSide {
    match a.unwrap_or(i64::MIN).cmp(&b.unwrap_or(i64::MIN)) {
        std::cmp::Ordering::Less => MatchSide::A,
        std::cmp::Ordering::Greater => MatchSide::B,
        std::cmp::Ordering::Equal if seed.is_multiple_of(2) => MatchSide::A,
        std::cmp::Ordering::Equal => MatchSide::B,
    }
}

/// Decides the match with its votes and takes the loser out of the queue;
/// `user`: an operator closing it early. A track taken out of the queue
/// meanwhile loses; both: the match is cancelled.
async fn close(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station: &StationRuntime,
    player: &PlayerState,
    found: &Row,
    user: Option<u64>,
) -> ApiResult<()> {
    let mut tx = pool.begin().await?;
    recount(&mut *tx, found.id).await?;
    tx.commit().await?;
    let counted = row(pool, found.id).await?;
    let a_there = waiting(player, counted.engine_started_at, counted.entry_a_id);
    let b_there = waiting(player, counted.engine_started_at, counted.entry_b_id);
    let (votes_a, votes_b) = (counted.votes_a, counted.votes_b);
    let (winner, decided_by) = match (a_there, b_there) {
        (false, false) => {
            cancel(pool, station, &counted, user, false).await?;
            return Ok(());
        }
        (true, false) => (MatchSide::A, MatchDecision::Votes),
        (false, true) => (MatchSide::B, MatchDecision::Votes),
        (true, true) if votes_a > votes_b => (MatchSide::A, MatchDecision::Votes),
        (true, true) if votes_b > votes_a => (MatchSide::B, MatchDecision::Votes),
        (true, true) => (
            least_recently_played(pool, &counted).await?,
            if votes_a == 0 {
                MatchDecision::NoVotes
            } else {
                MatchDecision::Tie
            },
        ),
    };
    let decided_by = if user.is_some() {
        MatchDecision::Early
    } else {
        decided_by
    };
    let closed = sqlx::query(
        "UPDATE matches SET status = 'closed', closed_at = NOW(6), winner = ?, decided_by = ?,
             closed_by = ?
         WHERE id = ? AND status = 'open'",
    )
    .bind(winner.as_str())
    .bind(decided_by.as_str())
    .bind(user)
    .bind(found.id)
    .execute(pool)
    .await?
    .rows_affected();
    if closed == 0 {
        return Ok(());
    }
    let loser = winner.other();
    if waiting(player, counted.engine_started_at, counted.entry(loser)) {
        take_out(station, counted.entry(loser)).await;
    }
    forget_pick(pool, counted.pick(loser)).await?;
    tracing::info!(
        station = station.id(),
        r#match = found.id,
        winner = ?counted.track_id(winner),
        votes_a,
        votes_b,
        decided_by = decided_by.as_str(),
        "match closed"
    );
    let closed = row(pool, found.id).await?;
    let values = placeholders(
        &closed,
        Some(winner),
        &time_zone(pool, station.id()).await?,
        &show_settings(pool, station.id())
            .await?
            .notifications
            .language,
    );
    notify(pool, webhooks, station, WebhookEvent::MatchClosed, values).await;
    station.queue.wake();
    Ok(())
}

/// Cancels a match waiting or open; with `take_out`, its second track
/// leaves the queue, the first one staying as an ordinary track. False when
/// it was decided meanwhile.
async fn cancel(
    pool: &MySqlPool,
    station: &StationRuntime,
    found: &Row,
    user: Option<u64>,
    take_b_out: bool,
) -> ApiResult<bool> {
    let cancelled = sqlx::query(
        "UPDATE matches SET status = 'cancelled', closed_at = NOW(6), closed_by = ?
         WHERE id = ? AND status IN ('queued', 'open')",
    )
    .bind(user)
    .bind(found.id)
    .execute(pool)
    .await?
    .rows_affected();
    if cancelled == 0 {
        return Ok(false);
    }
    if take_b_out {
        let player = station.engine.state().borrow().clone();
        if player.is_some_and(|player| waiting(&player, found.engine_started_at, found.entry_b_id))
        {
            take_out(station, found.entry_b_id).await;
        }
        forget_pick(pool, found.pick_b_id).await?;
    }
    tracing::info!(
        station = station.id(),
        r#match = found.id,
        "match cancelled"
    );
    station.queue.wake();
    Ok(true)
}

/// Takes a queue entry out; logged when the engine refuses (it started).
async fn take_out(station: &StationRuntime, entry: u64) {
    match station.engine.remove(entry).await {
        Ok(()) => {}
        Err(ApiError::NotFound(_)) => {}
        Err(error) => {
            tracing::warn!(entry, %error, "cannot take the match's loser out of the queue")
        }
    }
}

/// Deletes a pick that will not play from the rotation log.
async fn forget_pick(pool: &MySqlPool, pick: Option<u64>) -> ApiResult<()> {
    if let Some(pick) = pick {
        sqlx::query("DELETE FROM rotation_picks WHERE id = ?")
            .bind(pick)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// The share of the votes as the language writes it: `58%`, `58 %`.
fn percent_text(percent: u32, language: &str) -> String {
    if language == "en" {
        format!("{percent}%")
    } else {
        format!("{percent}\u{a0}%")
    }
}

/// A match's own placeholders; with `winner` once decided.
fn placeholders(
    found: &Row,
    winner: Option<MatchSide>,
    tz: &TimeZone,
    language: &str,
) -> Vec<(&'static str, String)> {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let close = found
        .closes_ms
        .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        .map(|at| core_shows::hour_text(at, tz))
        .unwrap_or_default();
    let mut values = vec![
        ("title1", text(&found.a_title)),
        ("artist1", text(&found.a_artist)),
        ("title2", text(&found.b_title)),
        ("artist2", text(&found.b_artist)),
        ("close", close),
    ];
    if let Some(winner) = winner {
        values.push(("winner", text(&found.track(winner).title)));
        values.push(("loser", text(&found.track(winner.other()).title)));
        values.push((
            "percent",
            percent_text(percent(found.votes(winner), found.total()), language),
        ));
        values.push(("votes", found.total().to_string()));
    }
    values
}

async fn notify(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station: &StationRuntime,
    event: WebhookEvent,
    values: Vec<(&'static str, String)>,
) {
    let sent = crate::shows::send_own_event(
        pool,
        webhooks,
        station.id(),
        &station.code(),
        match_show(),
        event,
        Timestamp::now(),
        &values,
    )
    .await;
    if let Err(error) = sent {
        tracing::warn!(event = event.as_str(), %error, "cannot send the match's notification");
    }
}

/// Runs [`tick`] every second. Never returns.
pub async fn run(pool: MySqlPool, stations: Arc<Stations>, webhooks: Arc<Webhooks>) {
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks = 0u32;
    loop {
        interval.tick().await;
        if let Err(error) = tick(&pool, &stations, &webhooks).await {
            tracing::warn!(%error, "cannot open or close the matches");
        }
        ticks = ticks.wrapping_add(1);
        if ticks % PURGE_EVERY_TICKS == 1 {
            match purge_devices(&pool).await {
                Ok(0) => {}
                Ok(purged) => tracing::debug!(purged, "devices' match votes deleted"),
                Err(error) => tracing::warn!(%error, "cannot delete the devices' match votes"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// On air and on the website

/// What `now-playing` says of a track the listeners chose.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct OnAirMatch {
    /// The show's name on the website.
    pub name: String,
    /// Its share of the votes, 0 to 100.
    pub percent: u32,
    pub votes: u32,
}

/// The match the track on air won, while the module is on.
pub async fn on_air_winner(
    pool: &MySqlPool,
    station_id: u64,
    player: &PlayerState,
) -> ApiResult<Option<OnAirMatch>> {
    let Some(entry) = player
        .decks
        .iter()
        .filter(|deck| deck.on_air)
        .find_map(|deck| deck.entry.as_ref())
    else {
        return Ok(None);
    };
    let found: Option<(u32, u32)> = sqlx::query_as(
        "SELECT IF(winner = 'a', votes_a, votes_b), votes_a + votes_b FROM matches
         WHERE station_id = ? AND engine_started_at = ? AND status = 'closed'
           AND ((winner = 'a' AND entry_a_id = ?) OR (winner = 'b' AND entry_b_id = ?))
           AND closed_at >= NOW(6) - INTERVAL 1 DAY
         ORDER BY id DESC LIMIT 1",
    )
    .bind(station_id)
    .bind(player.engine_started_at_unix_ms)
    .bind(entry.entry_id)
    .bind(entry.entry_id)
    .fetch_optional(pool)
    .await?;
    let Some((won, votes)) = found else {
        return Ok(None);
    };
    if !crate::modules::is_on(pool, Some(station_id), Module::Match).await? {
        return Ok(None);
    }
    let show = show_settings(pool, station_id).await?;
    Ok(Some(OnAirMatch {
        name: show_name(&show),
        percent: percent(won, votes),
        votes,
    }))
}

/// A track on the website.
#[derive(Clone, Debug, Serialize)]
struct PublicChoice {
    choice: MatchSide,
    artist: Option<String>,
    title: Option<String>,
    cover_url: Option<String>,
    /// `null` without live results.
    votes: Option<u32>,
    percent: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
struct PublicResult {
    winner: PublicChoice,
    loser: PublicChoice,
    /// The winner's share.
    percent: u32,
    votes: u32,
    decided_by: Option<MatchDecision>,
    closed_at_unix_ms: Option<i64>,
}

fn public_choice(
    found: &Row,
    choice: MatchSide,
    covers: &crate::covers::PublicCovers,
    live: bool,
) -> PublicChoice {
    let track = found.track(choice);
    PublicChoice {
        choice,
        artist: track.artist,
        title: track.title,
        cover_url: covers.url(track.track_id),
        votes: live.then(|| found.votes(choice)),
        percent: live.then(|| percent(found.votes(choice), found.total())),
    }
}

fn public_result(found: &Row, covers: &crate::covers::PublicCovers) -> Option<PublicResult> {
    let winner = found.winner()?;
    Some(PublicResult {
        winner: public_choice(found, winner, covers, true),
        loser: public_choice(found, winner.other(), covers, true),
        percent: percent(found.votes(winner), found.total()),
        votes: found.total(),
        decided_by: found.decided_by.as_deref().and_then(MatchDecision::parse),
        closed_at_unix_ms: found.closed_ms,
    })
}

/// The show's live state for the gateway: the match open to votes, the last
/// result and today's.
pub async fn public_state(
    pool: &MySqlPool,
    station_id: u64,
    timezone: &str,
) -> ApiResult<serde_json::Value> {
    let settings = load_settings(pool, station_id).await?;
    let tz = TimeZone::get(timezone).unwrap_or(TimeZone::UTC);
    let open = rows(pool, "m.status = 'open'", station_id)
        .await?
        .into_iter()
        .next();
    let midnight = Timestamp::now()
        .to_zoned(tz)
        .start_of_day()
        .map_err(internal)?
        .timestamp();
    let today: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} {FROM}
         WHERE m.station_id = ? AND m.status = 'closed' AND m.closed_at >= FROM_UNIXTIME(?)
         ORDER BY m.id DESC LIMIT ?"
    )))
    .bind(station_id)
    .bind(seconds(midnight.as_millisecond()))
    .bind(TODAY)
    .fetch_all(pool)
    .await?;
    let last: Option<Row> = match today.first() {
        Some(_) => None,
        None => rows(pool, "m.status = 'closed'", station_id)
            .await?
            .into_iter()
            .next(),
    };
    let mut ids = Vec::new();
    for found in open.iter().chain(&today).chain(&last) {
        ids.extend(found.track_a_id);
        ids.extend(found.track_b_id);
    }
    let covers = crate::covers::PublicCovers::load(pool, station_id, &ids).await?;
    let live = settings.live_results;
    let current = open.as_ref().map(|found| {
        serde_json::json!({
            "match_id": found.id,
            "closes_at_unix_ms": found.closes_ms,
            "choices": [
                public_choice(found, MatchSide::A, &covers, live),
                public_choice(found, MatchSide::B, &covers, live),
            ],
            "votes": live.then(|| found.total()),
        })
    });
    let results: Vec<PublicResult> = today
        .iter()
        .filter_map(|found| public_result(found, &covers))
        .collect();
    let last = results.first().cloned().or_else(|| {
        last.as_ref()
            .and_then(|found| public_result(found, &covers))
    });
    let mut state = serde_json::json!({
        "match_id": null,
        "closes_at_unix_ms": null,
        "choices": [],
        "votes": null,
        "live_results": live,
        "last": last,
        "today": results,
    });
    if let (Some(current), Some(object)) = (current, state.as_object_mut())
        && let Some(fields) = current.as_object()
    {
        for (key, value) in fields {
            object.insert(key.clone(), value.clone());
        }
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked() {
        assert_eq!(MatchSettings::default().check(), Ok(()));
        for bad in [
            MatchSettings {
                every_tracks: 1,
                ..MatchSettings::default()
            },
            MatchSettings {
                every_tracks: 21,
                ..MatchSettings::default()
            },
            MatchSettings {
                close_before_s: 4,
                ..MatchSettings::default()
            },
            MatchSettings {
                close_before_s: 61,
                ..MatchSettings::default()
            },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        // A vote of 30 seconds at least, on a track of a minute at least.
        assert_eq!(MatchSettings::default().min_track_ms(), 60_000);
        let late = MatchSettings {
            close_before_s: 45,
            ..MatchSettings::default()
        };
        assert_eq!(late.min_track_ms(), 75_000);
    }

    #[test]
    fn percents_are_whole_and_zero_without_votes() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(46, 79), 58);
        assert_eq!(percent(33, 79), 42);
        assert_eq!(percent_text(58, "en"), "58%");
        assert_eq!(percent_text(58, "fr"), "58\u{a0}%");
    }

    #[test]
    fn a_tie_goes_to_the_least_recently_played() {
        assert_eq!(tie_break(Some(10), Some(20), 0), MatchSide::A);
        assert_eq!(tie_break(Some(30), Some(20), 0), MatchSide::B);
        // Never played first.
        assert_eq!(tie_break(Some(10), None, 0), MatchSide::B);
        // Both never played: at random.
        assert_eq!(tie_break(None, None, 2), MatchSide::A);
        assert_eq!(tie_break(None, None, 3), MatchSide::B);
    }

    fn planner(every: u32, since: u32, last: Option<i64>) -> Planner {
        Planner {
            settings: MatchSettings {
                every_tracks: every,
                category_id: Some(1),
                ..MatchSettings::default()
            },
            category: 1,
            // Mondays, 18:00 to 19:00, UTC.
            week: Week::of(&[core_shows::HourRange {
                days: vec![1],
                start_hour: 18,
                end_hour: 19,
            }]),
            tz: TimeZone::UTC,
            last_planned_ms: last,
            since,
            excluded: Vec::new(),
            silent: HashSet::new(),
        }
    }

    fn at(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    #[test]
    fn a_match_is_due_every_n_tracks_in_the_hours_only() {
        let minutes = |n: i64| n * 60_000;
        let start = at("2026-11-09T18:10:00Z");
        // The first track of the range carries the vote.
        assert!(planner(4, 0, None).due(start, minutes(3)));
        assert!(planner(4, 9, Some(at("2026-11-02T18:50:00Z"))).due(start, minutes(3)));
        // Then the winner every 4 tracks: two tracks, then the vote's.
        let last = Some(at("2026-11-09T18:05:00Z"));
        assert!(!planner(4, 0, last).due(start, minutes(3)));
        assert!(!planner(4, 1, last).due(start, minutes(3)));
        assert!(planner(4, 2, last).due(start, minutes(3)));
        // Not outside the hours, nor past their end, nor on a short track.
        assert!(!planner(4, 0, None).due(at("2026-11-09T17:58:00Z"), minutes(3)));
        assert!(!planner(4, 0, None).due(at("2026-11-10T18:10:00Z"), minutes(3)));
        assert!(!planner(4, 0, None).due(at("2026-11-09T18:58:00Z"), minutes(3)));
        assert!(!planner(4, 0, None).due(start, 59_000));
    }
}
