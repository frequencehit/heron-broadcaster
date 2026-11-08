// SPDX-License-Identifier: AGPL-3.0-or-later

//! The attempts page (`docs/rotation.md#the-attempts-page`): each time
//! the queue manager built fills before an exact time, a calculation with
//! every fill as an attempt; a light row each time a pick followed the
//! fill kept; an attempt chosen by hand, and back to automatic. Kept 7
//! days.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use heron_rotation::{Attempt, AttemptEnding, Calculation, ExactLog, ExactMode, Explanation, Stop};
use jiff::Timestamp;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::rotation::{TrackLabel, track_labels};
use crate::settings::{self, StationSettings};
use crate::stations::Station;

/// How long calculations are kept.
const KEEP_DAYS: u32 = 7;
/// How often the old ones are deleted.
const PURGE_EVERY: Duration = Duration::from_secs(3_600);

/// Why fills were built, or that a pick followed the fill kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// The first pick for this exact time (after the exact time before it).
    First,
    /// No fill kept, but this exact time had calculations: the API
    /// restarted.
    Restart,
    /// The fill kept no longer landed: the queue changed (a track added,
    /// removed or moved, a skip, a match, a rule that no longer passes).
    QueueChanged,
    /// The fill chosen by hand no longer landed: rotation chooses by
    /// itself again.
    HandDropped,
    /// Back to automatic: the choice by hand was dropped.
    Automatic,
    /// The pick followed the fill kept: no fill built.
    Followed,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Restart => "restart",
            Self::QueueChanged => "queue_changed",
            Self::HandDropped => "hand_dropped",
            Self::Automatic => "automatic",
            Self::Followed => "followed",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "first" => Self::First,
            "restart" => Self::Restart,
            "queue_changed" => Self::QueueChanged,
            "hand_dropped" => Self::HandDropped,
            "automatic" => Self::Automatic,
            _ => Self::Followed,
        }
    }
}

fn stop_str(stop: Stop) -> &'static str {
    match stop {
        Stop::Followed => "followed",
        Stop::Landed => "landed",
        Stop::MaxFills => "max_fills",
        Stop::Time => "time",
    }
}

fn parse_stop(text: &str) -> Stop {
    match text {
        "landed" => Stop::Landed,
        "max_fills" => Stop::MaxFills,
        "time" => Stop::Time,
        _ => Stop::Followed,
    }
}

fn mode_str(mode: ExactMode) -> &'static str {
    match mode {
        ExactMode::Hard => "hard",
        ExactMode::Soft => "soft",
    }
}

/// What the queue manager keeps between two fills of the queue before an
/// exact time: the fill kept, the calculation it comes from with the
/// picks queued from it, and the attempt chosen by hand.
#[derive(Default)]
pub struct Kept {
    pub plan: Option<heron_rotation::Plan>,
    pub calculation: Option<KeptCalculation>,
    pub choice: Option<HandChoice>,
    /// Why the next fills are built, when the queue manager knows it
    /// (back to automatic).
    pub next_trigger: Option<Trigger>,
}

/// The calculation whose fill the queue follows.
pub struct KeptCalculation {
    pub id: u64,
    pub exact_at_ms: i64,
    /// The picks queued from it since (its own first, then those that
    /// followed it), in order.
    pub queued: Vec<QueuedPick>,
    /// A match was planned with them: its tracks are locked, no attempt
    /// can replace them.
    pub blocked: bool,
}

/// A pick queued from a calculation: its queue entries and tracks.
#[derive(Clone, Debug)]
pub struct QueuedPick {
    pub entries: Vec<u64>,
    pub tracks: Vec<u64>,
}

/// An attempt chosen by hand that the queue follows.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct HandChoice {
    pub calculation_id: u64,
    /// The attempt's number.
    pub attempt: u32,
    pub exact_at_unix_ms: i64,
    pub user_id: u64,
    pub username: String,
    pub at_unix_ms: i64,
    /// How the chosen attempt ends, as when it was chosen.
    pub ending: AttemptEnding,
}

/// The tracks of `attempt`, pick by pick.
pub fn attempt_picks(attempt: &Attempt) -> Vec<Vec<u64>> {
    let mut picks: Vec<Vec<u64>> = Vec::new();
    let mut last = None;
    for track in &attempt.tracks {
        match picks.last_mut() {
            Some(pick) if last == Some(track.pick) => pick.push(track.track_id),
            _ => picks.push(vec![track.track_id]),
        }
        last = Some(track.pick);
    }
    picks
}

/// Whether an attempt may be chosen by hand: it lands in its window
/// (its gap filled or not) and fades nothing out.
pub fn choosable(ending: &AttemptEnding) -> bool {
    ending.in_window && !ending.faded
}

/// Stores what a pick before an exact time did: a calculation with its
/// attempts, or a light row for a pick that followed the fill kept
/// (`follows`: the calculation it comes from). Returns its ID.
pub async fn store(
    pool: &MySqlPool,
    station_id: u64,
    calculation: &Calculation,
    trigger: Trigger,
    follows: Option<u64>,
) -> ApiResult<u64> {
    let attempts = if calculation.attempts.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&calculation.attempts)
                .map_err(|error| ApiError::Internal(error.to_string()))?,
        )
    };
    let ending = &calculation.ending;
    let id = sqlx::query(
        "INSERT INTO exact_calculations
             (station_id, exact_at_ms, window_from_ms, window_cut_ms, mode, clock_id, exact_slot,
              from_ms, reason, follows_id, fills, batches, stop, duration_ms, kept_attempt,
              offset_ms, in_window, faded, filling_ms, relaxed_steps, age_ms, by_hand, attempts)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(station_id)
    .bind(calculation.exact_at_unix_ms)
    .bind(calculation.window_from_unix_ms)
    .bind(calculation.window_cut_unix_ms)
    .bind(mode_str(calculation.mode))
    .bind(calculation.exact_clock_id)
    .bind(calculation.exact_slot)
    .bind(calculation.from_unix_ms)
    .bind(trigger.as_str())
    .bind(follows)
    .bind(u16::try_from(calculation.fills).unwrap_or(u16::MAX))
    .bind(u16::try_from(calculation.batches).unwrap_or(u16::MAX))
    .bind(stop_str(calculation.stop))
    .bind(calculation.duration_ms)
    .bind(
        calculation
            .kept
            .map(|kept| u16::try_from(kept).unwrap_or(u16::MAX)),
    )
    .bind(ending.offset_ms)
    .bind(ending.in_window)
    .bind(ending.faded)
    .bind(ending.filling_ms)
    .bind(ending.relaxed_steps)
    .bind(ending.age_ms)
    .bind(calculation.by_hand)
    .bind(attempts)
    .execute(pool)
    .await?
    .last_insert_id();
    Ok(id)
}

/// Whether the station has calculations for the exact time at
/// `exact_at_ms` already.
pub async fn calculated_before(
    pool: &MySqlPool,
    station_id: u64,
    exact_at_ms: i64,
) -> ApiResult<bool> {
    let found: Option<u64> = sqlx::query_scalar(
        "SELECT id FROM exact_calculations WHERE station_id = ? AND exact_at_ms = ? LIMIT 1",
    )
    .bind(station_id)
    .bind(exact_at_ms)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

/// Deletes the calculations of more than [`KEEP_DAYS`] days.
pub async fn purge_old(pool: &MySqlPool) -> ApiResult<u64> {
    Ok(
        sqlx::query("DELETE FROM exact_calculations WHERE computed_at < NOW(6) - INTERVAL ? DAY")
            .bind(KEEP_DAYS)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// Runs [`purge_old`] every hour. Never returns.
pub async fn purge(pool: MySqlPool) {
    let mut interval = tokio::time::interval(PURGE_EVERY);
    loop {
        interval.tick().await;
        match purge_old(&pool).await {
            Ok(0) => {}
            Ok(purged) => tracing::debug!(purged, "old exact time calculations deleted"),
            Err(error) => tracing::warn!(%error, "cannot delete old exact time calculations"),
        }
    }
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_exact_times))
        .routes(routes!(get_exact_calculation))
        .routes(routes!(use_exact_attempt))
        .routes(routes!(drop_exact_choice))
}

/// An attempt chosen by hand, as kept with its calculation.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ChosenAttempt {
    pub attempt: u32,
    /// Missing once the user is deleted.
    pub username: Option<String>,
    pub at_unix_ms: i64,
}

/// A calculation, in the list of an exact time's.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CalculationSummary {
    pub id: u64,
    pub computed_at_unix_ms: i64,
    pub trigger: Trigger,
    /// The calculation a followed pick follows.
    pub follows_id: Option<u64>,
    /// New fills built, in batches of 10, and why building stopped.
    pub fills: u32,
    pub batches: u32,
    pub stop: Stop,
    pub duration_ms: u32,
    /// The attempt kept, by its number.
    pub kept: Option<u32>,
    /// How the fill kept (or followed) ends.
    pub ending: AttemptEnding,
    /// The fill followed was chosen by hand.
    pub by_hand: bool,
    pub chosen: Option<ChosenAttempt>,
}

/// An exact time of the day.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ExactTimeOfDay {
    pub at_unix_ms: i64,
    pub mode: ExactMode,
    /// Its window (Unix ms): the earliest start, and the cut.
    pub window_from_unix_ms: i64,
    pub window_cut_unix_ms: i64,
    /// Its clock and slot (from 0).
    pub clock_name: Option<String>,
    pub slot: Option<u32>,
    /// How it was met, once its slot was picked.
    pub met: Option<ExactLog>,
    /// Not reached yet.
    pub upcoming: bool,
    /// Its calculations, in order.
    pub calculations: Vec<CalculationSummary>,
}

/// The station's exact times of a day.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ExactTimesDay {
    /// `2026-11-08`, in the station's time zone.
    pub date: String,
    pub today: String,
    pub time_zone: String,
    pub exact_times: Vec<ExactTimeOfDay>,
    /// The attempt chosen by hand the queue follows now, if any.
    pub choice: Option<HandChoice>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExactTimesQuery {
    /// `2026-11-08`; today, in the station's time zone, by default.
    pub day: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: u64,
    exact_at_ms: i64,
    window_from_ms: i64,
    window_cut_ms: i64,
    mode: String,
    clock_name: Option<String>,
    exact_slot: u32,
    from_ms: i64,
    computed_at_ms: i64,
    reason: String,
    follows_id: Option<u64>,
    fills: u16,
    batches: u16,
    stop: String,
    duration_ms: u32,
    kept_attempt: Option<u16>,
    offset_ms: Option<i64>,
    in_window: bool,
    faded: bool,
    filling_ms: u32,
    relaxed_steps: u32,
    age_ms: i64,
    by_hand: bool,
    chosen_attempt: Option<u16>,
    chosen_by: Option<String>,
    chosen_at_ms: Option<i64>,
}

const SUMMARY_COLUMNS: &str = "
    e.id, e.exact_at_ms, e.window_from_ms, e.window_cut_ms, CAST(e.mode AS CHAR) AS mode,
    c.name AS clock_name, e.exact_slot, e.from_ms,
    CAST(UNIX_TIMESTAMP(e.computed_at) * 1000 AS SIGNED) AS computed_at_ms,
    CAST(e.reason AS CHAR) AS reason, e.follows_id, e.fills, e.batches,
    CAST(e.stop AS CHAR) AS stop, e.duration_ms, e.kept_attempt, e.offset_ms, e.in_window,
    e.faded, e.filling_ms, e.relaxed_steps, e.age_ms, e.by_hand, e.chosen_attempt,
    u.username AS chosen_by,
    CAST(UNIX_TIMESTAMP(e.chosen_at) * 1000 AS SIGNED) AS chosen_at_ms
    FROM exact_calculations e
    LEFT JOIN clocks c ON c.id = e.clock_id
    LEFT JOIN users u ON u.id = e.chosen_by";

impl SummaryRow {
    fn mode(&self) -> ExactMode {
        if self.mode == "soft" {
            ExactMode::Soft
        } else {
            ExactMode::Hard
        }
    }

    fn summary(&self) -> CalculationSummary {
        CalculationSummary {
            id: self.id,
            computed_at_unix_ms: self.computed_at_ms,
            trigger: Trigger::parse(&self.reason),
            follows_id: self.follows_id,
            fills: u32::from(self.fills),
            batches: u32::from(self.batches),
            stop: parse_stop(&self.stop),
            duration_ms: self.duration_ms,
            kept: self.kept_attempt.map(u32::from),
            ending: AttemptEnding {
                offset_ms: self.offset_ms,
                in_window: self.in_window,
                faded: self.faded,
                filling_ms: self.filling_ms,
                relaxed_steps: self.relaxed_steps,
                age_ms: self.age_ms,
            },
            by_hand: self.by_hand,
            chosen: self.chosen_attempt.map(|attempt| ChosenAttempt {
                attempt: u32::from(attempt),
                username: self.chosen_by.clone(),
                at_unix_ms: self.chosen_at_ms.unwrap_or_default(),
            }),
        }
    }
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

/// The station's exact times of a day, each with how it was met and its
/// calculations. Every role.
#[utoipa::path(get, path = "/exact-times", tag = "rotation",
    params(ExactTimesQuery),
    responses((status = 200, body = ExactTimesDay), (status = "4XX", body = ErrorBody)))]
async fn list_exact_times(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<ExactTimesQuery>,
) -> ApiResult<Json<ExactTimesDay>> {
    let settings: StationSettings =
        settings::load_for(&state.pool, station.id(), StationSettings::SETTINGS_NAME).await?;
    let zone = settings.time_zone();
    let now = crate::rotation::now_ms();
    let today = Timestamp::from_millisecond(now)
        .map_err(internal)?
        .to_zoned(zone.clone())
        .date();
    let date = match query.day.as_deref() {
        Some(day) => day
            .parse::<Date>()
            .map_err(|_| ApiError::BadRequest(format!("{day}: a day is YYYY-MM-DD")))?,
        None => today,
    };
    let start = date
        .to_zoned(zone.clone())
        .map_err(internal)?
        .timestamp()
        .as_millisecond();
    let end = date
        .tomorrow()
        .map_err(internal)?
        .to_zoned(zone.clone())
        .map_err(internal)?
        .timestamp()
        .as_millisecond();

    let mut items: BTreeMap<i64, ExactTimeOfDay> = BTreeMap::new();
    let mut sql = QueryBuilder::new(format!("SELECT {SUMMARY_COLUMNS} WHERE e.station_id = "));
    sql.push_bind(station.id())
        .push(" AND e.exact_at_ms >= ")
        .push_bind(start)
        .push(" AND e.exact_at_ms < ")
        .push_bind(end)
        .push(" ORDER BY e.exact_at_ms, e.id");
    let rows: Vec<SummaryRow> = sql.build_query_as().fetch_all(&state.pool).await?;
    for row in rows {
        let item = items
            .entry(row.exact_at_ms)
            .or_insert_with(|| ExactTimeOfDay {
                at_unix_ms: row.exact_at_ms,
                mode: row.mode(),
                window_from_unix_ms: row.window_from_ms,
                window_cut_unix_ms: row.window_cut_ms,
                clock_name: row.clock_name.clone(),
                slot: Some(row.exact_slot),
                met: None,
                upcoming: row.exact_at_ms > now,
                calculations: Vec::new(),
            });
        item.calculations.push(row.summary());
    }

    // How each was met: its slot's pick (the latest when picked again).
    // Its planned start is in its window, at most 5 minutes from it.
    let margin = 5 * 60_000;
    let picks: Vec<(u64, String)> = sqlx::query_as(
        "SELECT id, CAST(explanation AS CHAR) FROM rotation_picks
         WHERE station_id = ? AND planned_at >= FROM_UNIXTIME(?) AND planned_at < FROM_UNIXTIME(?)
           AND JSON_VALUE(explanation, '$.exact.at_unix_ms') IS NOT NULL
         ORDER BY id",
    )
    .bind(station.id())
    .bind((start - margin) as f64 / 1_000.0)
    .bind((end + margin) as f64 / 1_000.0)
    .fetch_all(&state.pool)
    .await?;
    for (id, explanation) in picks {
        let explanation: Explanation = serde_json::from_str(&explanation)
            .map_err(|error| ApiError::Internal(format!("pick {id}: {error}")))?;
        let Some(exact) = explanation.exact else {
            continue;
        };
        if !(start..end).contains(&exact.at_unix_ms) {
            continue;
        }
        let item = items
            .entry(exact.at_unix_ms)
            .or_insert_with(|| ExactTimeOfDay {
                at_unix_ms: exact.at_unix_ms,
                mode: exact.mode,
                window_from_unix_ms: exact.at_unix_ms - i64::from(exact.before_s) * 1_000,
                window_cut_unix_ms: exact.cut_at_unix_ms,
                clock_name: None,
                slot: None,
                met: None,
                upcoming: false,
                calculations: Vec::new(),
            });
        item.met = Some(exact);
        item.upcoming = false;
        item.clock_name = explanation.clock_name.clone().or(item.clock_name.take());
        item.slot = explanation.slot.or(item.slot);
    }
    Ok(Json(ExactTimesDay {
        date: date.to_string(),
        today: today.to_string(),
        time_zone: settings.timezone.clone(),
        exact_times: items.into_values().collect(),
        choice: station.queue.choice(),
    }))
}

/// A category of an attempt's tracks.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CategoryLabel {
    pub id: u64,
    pub name: String,
    pub color: Option<String>,
}

/// A calculation with its attempts.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CalculationDetail {
    #[serde(flatten)]
    pub summary: CalculationSummary,
    pub exact_at_unix_ms: i64,
    pub mode: ExactMode,
    pub window_from_unix_ms: i64,
    pub window_cut_unix_ms: i64,
    pub clock_name: Option<String>,
    pub exact_slot: u32,
    /// Where the fills start: the end of the queue (Unix ms).
    pub from_unix_ms: i64,
    /// The latest calculation of its exact time (followed picks aside).
    pub latest: bool,
    /// Its attempts landing in the window can be chosen by hand: it is the
    /// latest of the upcoming exact time, and the queue follows it.
    pub can_choose: bool,
    /// Its attempts, best first; none for a pick that followed the fill
    /// kept (see `follows_id`).
    pub attempts: Vec<Attempt>,
    /// The tracks and categories of the attempts.
    pub tracks: Vec<TrackLabel>,
    pub categories: Vec<CategoryLabel>,
}

/// A calculation's row and attempts, on the station.
async fn load(pool: &MySqlPool, station_id: u64, id: u64) -> ApiResult<(SummaryRow, Vec<Attempt>)> {
    let mut query = QueryBuilder::new(format!("SELECT {SUMMARY_COLUMNS} WHERE e.id = "));
    query
        .push_bind(id)
        .push(" AND e.station_id = ")
        .push_bind(station_id);
    let row: Option<SummaryRow> = query.build_query_as().fetch_optional(pool).await?;
    let row = row.ok_or_else(|| ApiError::not_found(format!("calculation {id}")))?;
    let attempts: Option<String> =
        sqlx::query_scalar("SELECT CAST(attempts AS CHAR) FROM exact_calculations WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
    let mut attempts: Vec<Attempt> = match attempts {
        Some(json) => serde_json::from_str(&json)
            .map_err(|error| ApiError::Internal(format!("calculation {id}: {error}")))?,
        None => Vec::new(),
    };
    attempts.sort_by_key(|attempt| attempt.rank);
    Ok((row, attempts))
}

/// Whether `row` is the latest calculation that built fills for its
/// exact time.
async fn is_latest(pool: &MySqlPool, row: &SummaryRow) -> ApiResult<bool> {
    let later: Option<u64> = sqlx::query_scalar(
        "SELECT id FROM exact_calculations
         WHERE station_id = (SELECT station_id FROM exact_calculations WHERE id = ?)
           AND exact_at_ms = ? AND id > ? AND reason <> 'followed' LIMIT 1",
    )
    .bind(row.id)
    .bind(row.exact_at_ms)
    .bind(row.id)
    .fetch_optional(pool)
    .await?;
    Ok(row.reason != "followed" && later.is_none())
}

/// A calculation's attempts, best first, with their tracks and endings.
/// Every role.
#[utoipa::path(get, path = "/exact-times/calculations/{id}", tag = "rotation",
    params(("id" = u64, Path, description = "The calculation's ID")),
    responses((status = 200, body = CalculationDetail), (status = "4XX", body = ErrorBody)))]
async fn get_exact_calculation(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<CalculationDetail>> {
    let (row, attempts) = load(&state.pool, station.id(), id).await?;
    let latest = is_latest(&state.pool, &row).await?;
    let can_choose = latest
        && row.exact_at_ms > crate::rotation::now_ms()
        && station.queue.kept_calculation() == Some(id);
    let track_ids: HashSet<u64> = attempts
        .iter()
        .flat_map(|attempt| &attempt.tracks)
        .map(|track| track.track_id)
        .collect();
    let tracks = track_labels(&state.pool, &track_ids.into_iter().collect::<Vec<_>>()).await?;
    let category_ids: HashSet<u64> = attempts
        .iter()
        .flat_map(|attempt| &attempt.tracks)
        .filter_map(|track| track.category_id)
        .collect();
    let categories = if category_ids.is_empty() {
        Vec::new()
    } else {
        let mut query = QueryBuilder::new("SELECT id, name, color FROM categories WHERE id IN (");
        let mut separated = query.separated(", ");
        for id in &category_ids {
            separated.push_bind(*id);
        }
        query.push(") ORDER BY id");
        let rows: Vec<(u64, String, Option<String>)> =
            query.build_query_as().fetch_all(&state.pool).await?;
        rows.into_iter()
            .map(|(id, name, color)| CategoryLabel { id, name, color })
            .collect()
    };
    Ok(Json(CalculationDetail {
        summary: row.summary(),
        exact_at_unix_ms: row.exact_at_ms,
        mode: row.mode(),
        window_from_unix_ms: row.window_from_ms,
        window_cut_unix_ms: row.window_cut_ms,
        clock_name: row.clock_name.clone(),
        exact_slot: row.exact_slot,
        from_unix_ms: row.from_ms,
        latest,
        can_choose,
        attempts,
        tracks,
        categories,
    }))
}

/// Uses an attempt instead of the fill kept: the tracks rotation queued
/// from the calculation that are not in it are taken out, and the next
/// picks follow it while it lands. Operators and admins; `409` unless the
/// calculation is the latest of the upcoming exact time, the queue still
/// follows it, and the attempt lands in its window, fading nothing out.
#[utoipa::path(post, path = "/exact-times/calculations/{id}/attempts/{n}/use", tag = "rotation",
    params(("id" = u64, Path, description = "The calculation's ID"),
           ("n" = u32, Path, description = "The attempt's number")),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn use_exact_attempt(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path((id, n)): Path<(u64, u32)>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let (row, attempts) = load(&state.pool, station.id(), id).await?;
    let attempt = attempts
        .iter()
        .find(|attempt| attempt.number == n)
        .ok_or_else(|| ApiError::not_found(format!("attempt {n} of calculation {id}")))?;
    if row.exact_at_ms <= crate::rotation::now_ms() {
        return Err(ApiError::Conflict("its exact time has passed".to_owned()));
    }
    if !is_latest(&state.pool, &row).await? {
        return Err(ApiError::Conflict(
            "only the attempts of the latest calculation of the upcoming exact time can be used"
                .to_owned(),
        ));
    }
    if !choosable(&attempt.ending) {
        return Err(ApiError::Conflict(
            "this attempt does not land in the window, or fades a track out".to_owned(),
        ));
    }
    station
        .queue
        .use_attempt(id, row.exact_at_ms, attempt, &user)
        .await?;
    sqlx::query(
        "UPDATE exact_calculations SET chosen_attempt = ?, chosen_by = ?, chosen_at = NOW(6)
         WHERE id = ?",
    )
    .bind(u16::try_from(n).unwrap_or(u16::MAX))
    .bind(user.id)
    .bind(id)
    .execute(&state.pool)
    .await?;
    tracing::info!(
        station = station.id(),
        calculation = id,
        attempt = n,
        user = %user.username,
        "exact time: an attempt chosen by hand"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Back to automatic: the attempt chosen by hand is dropped, and the next
/// pick builds new fills. Operators and admins.
#[utoipa::path(delete, path = "/exact-times/choice", tag = "rotation",
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn drop_exact_choice(user: CurrentUser, station: Station) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    if station.queue.back_to_automatic().await {
        tracing::info!(
            station = station.id(),
            user = %user.username,
            "exact time: back to automatic"
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use heron_rotation::{AttemptTrack, Chosen};

    use super::*;

    fn track(track_id: u64, pick: u32) -> AttemptTrack {
        AttemptTrack {
            track_id,
            pick,
            slot: Some(pick + 1),
            category_id: None,
            start_unix_ms: 0,
            next_ms: 0,
            chosen: Chosen::Slot,
            step: 0,
            hooks_left_out: 0,
            parts_left_out: Vec::new(),
        }
    }

    #[test]
    fn an_attempt_goes_pick_by_pick() {
        let attempt = Attempt {
            number: 3,
            batch: 1,
            previous: false,
            seed: 7,
            rank: 2,
            tracks: vec![track(10, 0), track(11, 1), track(12, 1), track(13, 2)],
            ending: AttemptEnding::default(),
            search: None,
        };
        assert_eq!(
            attempt_picks(&attempt),
            vec![vec![10], vec![11, 12], vec![13]]
        );
    }

    #[test]
    fn only_attempts_in_their_window_can_be_chosen() {
        let landing = AttemptEnding {
            offset_ms: Some(-3_000),
            in_window: true,
            filling_ms: 4_000,
            ..AttemptEnding::default()
        };
        assert!(choosable(&landing));
        let early = AttemptEnding {
            in_window: false,
            ..landing
        };
        assert!(!choosable(&early));
        let faded = AttemptEnding {
            faded: true,
            ..landing
        };
        assert!(!choosable(&faded));
    }

    #[test]
    fn triggers_and_stops_round_trip() {
        for trigger in [
            Trigger::First,
            Trigger::Restart,
            Trigger::QueueChanged,
            Trigger::HandDropped,
            Trigger::Automatic,
            Trigger::Followed,
        ] {
            assert_eq!(Trigger::parse(trigger.as_str()), trigger);
        }
        for stop in [Stop::Followed, Stop::Landed, Stop::MaxFills, Stop::Time] {
            assert_eq!(parse_stop(stop_str(stop)), stop);
        }
    }

    #[test]
    fn seeds_are_strings() {
        let attempt = Attempt {
            number: 1,
            batch: 1,
            previous: false,
            seed: u64::MAX,
            rank: 1,
            tracks: Vec::new(),
            ending: AttemptEnding::default(),
            search: None,
        };
        let json = serde_json::to_value(&attempt).unwrap();
        assert_eq!(json["seed"], "18446744073709551615");
        let back: Attempt = serde_json::from_value(json).unwrap();
        assert_eq!(back, attempt);
    }
}
