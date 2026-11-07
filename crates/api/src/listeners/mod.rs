// SPDX-License-Identifier: AGPL-3.0-or-later

//! Listener statistics, read from the stream servers (`collector`), and
//! the routes that show them. See `docs/statistics.md`.

pub mod collector;
mod icecast;
mod sessions;
pub mod totals;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub use collector::{Collector, OutputListeners};

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::{self, StationSettings, StatisticsSettings};
use crate::stations::Station;

/// The longest range of minute samples in one request.
const MAX_SAMPLE_RANGE_MS: i64 = 8 * 86_400_000;
/// The longest period, in days, of the other routes.
const MAX_DAYS: i32 = 3_660;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(now))
        .routes(routes!(samples))
        .routes(routes!(days))
        .routes(routes!(hours_of_day))
        .routes(routes!(tracks))
        .routes(routes!(players))
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ListenersNow {
    /// Each enabled output read at the last minute.
    pub outputs: Vec<OutputListeners>,
    /// The outputs' listeners added up; missing before the first reading or
    /// when no server could be read.
    pub total: Option<u32>,
    /// Each server's listeners (master and relays), its outputs' parts
    /// added up; missing when it could not be read.
    pub servers: Vec<ServerListeners>,
}

/// A stream server's listeners at the last minute.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ServerListeners {
    pub server_id: u64,
    pub name: String,
    /// The master it relays, for a relay.
    pub relay_of: Option<u64>,
    pub listeners: Option<u32>,
    pub error: Option<String>,
}

/// Each server's listeners, from the outputs' parts.
fn servers_of(outputs: &[OutputListeners]) -> Vec<ServerListeners> {
    let mut servers: Vec<ServerListeners> = Vec::new();
    for part in outputs.iter().flat_map(|output| &output.servers) {
        match servers.iter_mut().find(|s| s.server_id == part.server_id) {
            Some(server) => {
                server.listeners = match (server.listeners, part.listeners) {
                    (Some(a), Some(b)) => Some(a + b),
                    (known, None) | (None, known) => known,
                };
                server.error = server.error.take().or_else(|| part.error.clone());
            }
            None => servers.push(ServerListeners {
                server_id: part.server_id,
                name: part.name.clone(),
                relay_of: part.relay_of,
                listeners: part.listeners,
                error: part.error.clone(),
            }),
        }
    }
    servers
}

/// Each output's listeners at the last minute, and the station's.
#[utoipa::path(get, path = "/listeners/now", tag = "listeners",
    responses((status = 200, body = ListenersNow), (status = "4XX", body = ErrorBody)))]
async fn now(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<ListenersNow>> {
    let ids: Vec<u64> = sqlx::query_scalar("SELECT id FROM outputs WHERE station_id = ?")
        .bind(station.id())
        .fetch_all(&state.pool)
        .await?;
    let outputs: Vec<OutputListeners> = state
        .listeners
        .last()
        .into_iter()
        .filter(|output| ids.contains(&output.output_id))
        .collect();
    let read: Vec<u32> = outputs.iter().filter_map(|o| o.listeners).collect();
    Ok(Json(ListenersNow {
        total: (!read.is_empty()).then(|| read.iter().sum()),
        servers: servers_of(&outputs),
        outputs,
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SamplesQuery {
    /// From this time (Unix ms).
    pub from_unix_ms: i64,
    /// Until this time (Unix ms), at most 8 days after `from_unix_ms`.
    pub to_unix_ms: i64,
    /// Only this output; the whole station otherwise.
    pub output_id: Option<u64>,
    /// Only this server's part (the master or a relay); every server's
    /// otherwise.
    pub server_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema, sqlx::FromRow)]
pub struct MinuteSample {
    /// The start of the minute.
    pub at_unix_ms: i64,
    pub listeners: u32,
}

/// Listeners minute by minute. Minutes no server could be read are
/// missing.
#[utoipa::path(get, path = "/listeners/samples", tag = "listeners",
    params(SamplesQuery),
    responses((status = 200, body = Vec<MinuteSample>), (status = "4XX", body = ErrorBody)))]
async fn samples(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<SamplesQuery>,
) -> ApiResult<Json<Vec<MinuteSample>>> {
    if query.to_unix_ms < query.from_unix_ms
        || query.to_unix_ms - query.from_unix_ms > MAX_SAMPLE_RANGE_MS
    {
        return Err(ApiError::BadRequest(
            "to_unix_ms must follow from_unix_ms by at most 8 days".to_owned(),
        ));
    }
    let mut sql = sqlx::QueryBuilder::new(
        "SELECT CAST(UNIX_TIMESTAMP(sampled_at) * 1000 AS SIGNED) AS at_unix_ms,
                CAST(SUM(listeners) AS UNSIGNED) AS listeners
         FROM listener_samples
         WHERE output_id IN (SELECT id FROM outputs WHERE station_id = ",
    );
    sql.push_bind(station.id())
        .push(") AND sampled_at >= FROM_UNIXTIME(")
        .push_bind(query.from_unix_ms as f64 / 1_000.0)
        .push(") AND sampled_at <= FROM_UNIXTIME(")
        .push_bind(query.to_unix_ms as f64 / 1_000.0)
        .push(")");
    if let Some(output_id) = query.output_id {
        sql.push(" AND output_id = ").push_bind(output_id);
    }
    if let Some(server_id) = query.server_id {
        sql.push(" AND server_id = ").push_bind(server_id);
    }
    sql.push(" GROUP BY sampled_at ORDER BY sampled_at");
    Ok(Json(sql.build_query_as().fetch_all(&state.pool).await?))
}

/// JSON, or CSV for spreadsheets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Json,
    Csv,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    Hour,
    #[default]
    Day,
    /// ISO weeks, from Monday.
    Week,
    Month,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PeriodQuery {
    /// First day (`YYYY-MM-DD`, station time).
    pub from: String,
    /// Last day, included.
    pub to: String,
    /// Only this output; the whole station otherwise.
    pub output_id: Option<u64>,
    /// Rows per hour, day (by default), week or month.
    #[param(inline)]
    pub group: Option<Grouping>,
    #[param(inline)]
    pub format: Option<Format>,
}

fn parse_day(text: &str, what: &str) -> ApiResult<Date> {
    text.parse()
        .map_err(|_| ApiError::BadRequest(format!("{what} must be a date such as 2026-10-25")))
}

fn period(from: &str, to: &str) -> ApiResult<(Date, Date)> {
    let (from, to) = (parse_day(from, "from")?, parse_day(to, "to")?);
    if to < from || (to - from).get_days() > MAX_DAYS {
        return Err(ApiError::BadRequest(
            "to must follow from, by at most 10 years".to_owned(),
        ));
    }
    Ok((from, to))
}

/// The totals of a period.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PeriodTotals {
    /// The first day of the period (`YYYY-MM-DD`).
    pub day: String,
    /// The hour (0 to 23), with `group=hour`.
    pub hour: Option<u8>,
    /// Days with a minute measured.
    pub days: u32,
    /// Minutes measured.
    pub minutes: u32,
    pub peak: u32,
    /// Mean of the listeners over the minutes measured.
    pub average: f64,
    /// Total listening hours.
    pub listening_hours: f64,
    /// Sessions of the minimum length started in the period; missing
    /// without listener lists.
    pub sessions: Option<u32>,
    pub average_session_s: Option<f64>,
    /// Distinct listeners; for weeks and months, the average per day.
    pub distinct_listeners: Option<f64>,
    /// Distinct listeners of each day, added up.
    pub listener_days: Option<u32>,
}

type TotalRow = (
    String,
    Option<u8>,
    u32,
    u32,
    f64,
    f64,
    Option<u32>,
    Option<u64>,
    Option<u32>,
);

async fn total_rows(
    state: &AppState,
    station: &Station,
    from: Date,
    to: Date,
    output_id: Option<u64>,
    hourly: bool,
) -> ApiResult<Vec<TotalRow>> {
    let mut sql = sqlx::QueryBuilder::new(
        "SELECT CAST(day AS CHAR), hour, minutes, peak, average, listening_hours, sessions,
                session_s, distinct_listeners
         FROM listener_totals WHERE station_id = ",
    );
    sql.push_bind(station.id())
        .push(" AND day >= ")
        .push_bind(from.to_string())
        .push(" AND day <= ")
        .push_bind(to.to_string());
    match output_id {
        Some(id) => sql.push(" AND output_id = ").push_bind(id),
        None => sql.push(" AND output_id IS NULL"),
    };
    sql.push(if hourly {
        " AND hour IS NOT NULL"
    } else {
        " AND hour IS NULL"
    });
    sql.push(" ORDER BY day, hour");
    Ok(sql.build_query_as().fetch_all(&state.pool).await?)
}

/// The first day of the week or month of a day.
fn period_start(day: Date, grouping: Grouping) -> Date {
    match grouping {
        Grouping::Hour | Grouping::Day => day,
        Grouping::Week => {
            let back = i64::from(day.weekday().to_monday_zero_offset());
            day.checked_sub(jiff::Span::new().days(back)).unwrap_or(day)
        }
        Grouping::Month => day.first_of_month(),
    }
}

/// Adds up the rows of `listener_totals` into periods.
fn group_totals(rows: Vec<TotalRow>, grouping: Grouping) -> Vec<PeriodTotals> {
    #[derive(Default)]
    struct Sum {
        days: u32,
        minutes: u32,
        peak: u32,
        listened: f64,
        sessions: Option<u32>,
        session_s: Option<u64>,
        distinct: Vec<u32>,
    }
    let mut periods: BTreeMap<(Date, Option<u8>), Sum> = BTreeMap::new();
    for (day, hour, minutes, peak, average, _, sessions, session_s, distinct) in rows {
        let Ok(day) = day.parse::<Date>() else {
            continue;
        };
        let sum = periods
            .entry((period_start(day, grouping), hour))
            .or_default();
        sum.days += 1;
        sum.minutes += minutes;
        sum.peak = sum.peak.max(peak);
        sum.listened += average * f64::from(minutes);
        if let Some(sessions) = sessions {
            *sum.sessions.get_or_insert(0) += sessions;
        }
        if let Some(session_s) = session_s {
            *sum.session_s.get_or_insert(0) += session_s;
        }
        sum.distinct.extend(distinct);
    }
    periods
        .into_iter()
        .map(|((day, hour), sum)| {
            let listener_days: Option<u32> =
                (!sum.distinct.is_empty()).then(|| sum.distinct.iter().sum());
            PeriodTotals {
                day: day.to_string(),
                hour,
                days: sum.days,
                minutes: sum.minutes,
                peak: sum.peak,
                average: if sum.minutes == 0 {
                    0.0
                } else {
                    sum.listened / f64::from(sum.minutes)
                },
                listening_hours: sum.listened / 60.0,
                sessions: sum.sessions,
                average_session_s: match (sum.sessions, sum.session_s) {
                    (Some(sessions), Some(seconds)) if sessions > 0 => {
                        Some(seconds as f64 / f64::from(sessions))
                    }
                    _ => None,
                },
                distinct_listeners: listener_days
                    .map(|total| f64::from(total) / sum.distinct.len() as f64),
                listener_days,
            }
        })
        .collect()
}

/// Totals per hour, day, week or month: distinct listeners, peak, average,
/// listening hours, sessions. Today's change until midnight.
#[utoipa::path(get, path = "/listeners/days", tag = "listeners",
    params(PeriodQuery),
    responses(
        (status = 200, body = Vec<PeriodTotals>, content_type = "application/json"),
        (status = "4XX", body = ErrorBody)))]
async fn days(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<PeriodQuery>,
) -> ApiResult<Response> {
    let (from, to) = period(&query.from, &query.to)?;
    let grouping = query.group.unwrap_or_default();
    let rows = total_rows(
        &state,
        &station,
        from,
        to,
        query.output_id,
        grouping == Grouping::Hour,
    )
    .await?;
    let totals = group_totals(rows, grouping);
    Ok(match query.format.unwrap_or_default() {
        Format::Json => Json(totals).into_response(),
        Format::Csv => csv_response(&format!("listeners-{from}-{to}.csv"), &days_csv(&totals)),
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HoursQuery {
    /// First day (`YYYY-MM-DD`, station time).
    pub from: String,
    /// Last day, included.
    pub to: String,
    /// Only this output; the whole station otherwise.
    pub output_id: Option<u64>,
    #[param(inline)]
    pub format: Option<Format>,
}

/// An hour of the day, over the days of a period.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct HourOfDay {
    /// 0 to 23, station time.
    pub hour: u8,
    /// Days with a minute measured in that hour.
    pub days: u32,
    /// Mean of the listeners over the minutes measured.
    pub average: f64,
    /// The highest peak.
    pub peak: u32,
    /// Listening hours, on average per day.
    pub listening_hours: f64,
}

fn hours_from(rows: Vec<TotalRow>) -> Vec<HourOfDay> {
    let mut hours: BTreeMap<u8, (u32, u32, u32, f64)> = BTreeMap::new();
    for (_, hour, minutes, peak, average, _, _, _, _) in rows {
        let Some(hour) = hour else { continue };
        let entry = hours.entry(hour).or_default();
        entry.0 += 1;
        entry.1 += minutes;
        entry.2 = entry.2.max(peak);
        entry.3 += average * f64::from(minutes);
    }
    hours
        .into_iter()
        .map(|(hour, (days, minutes, peak, listened))| HourOfDay {
            hour,
            days,
            average: if minutes == 0 {
                0.0
            } else {
                listened / f64::from(minutes)
            },
            peak,
            listening_hours: listened / 60.0 / f64::from(days.max(1)),
        })
        .collect()
}

/// Each hour of the day (station time), averaged over the days of the
/// period.
#[utoipa::path(get, path = "/listeners/hours-of-day", tag = "listeners",
    params(HoursQuery),
    responses(
        (status = 200, body = Vec<HourOfDay>, content_type = "application/json"),
        (status = "4XX", body = ErrorBody)))]
async fn hours_of_day(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<HoursQuery>,
) -> ApiResult<Response> {
    let (from, to) = period(&query.from, &query.to)?;
    let hours = hours_from(total_rows(&state, &station, from, to, query.output_id, true).await?);
    Ok(match query.format.unwrap_or_default() {
        Format::Json => Json(hours).into_response(),
        Format::Csv => csv_response(
            &format!("listeners-hours-{from}-{to}.csv"),
            &hours_csv(&hours),
        ),
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TracksQuery {
    /// First day (`YYYY-MM-DD`, station time).
    pub from: String,
    /// Last day, included.
    pub to: String,
    /// Tracks played fewer times are left out, 3 by default.
    pub min_plays: Option<u32>,
    /// 1 to 500, 50 by default.
    pub limit: Option<u32>,
    #[param(inline)]
    pub format: Option<Format>,
}

/// A track's listeners over its plays of a period.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct TrackListeners {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// Plays with listeners measured when they started.
    pub plays: u32,
    /// The station's listeners when it started, on average.
    pub listeners_start: f64,
    /// When it ended, on average.
    pub listeners_end: f64,
    /// Sessions of the minimum length that ended during its plays; missing
    /// without listener lists.
    pub departures: Option<u32>,
    /// Departures over the listeners at the start.
    pub departure_rate: Option<f64>,
}

type TrackRow = (
    u64,
    Option<String>,
    Option<String>,
    i64,
    f64,
    f64,
    i64,
    i64,
    Option<i64>,
);

/// Tracks of the period, those listeners left the most first (by
/// departure rate, or by the change of listeners without sessions).
#[utoipa::path(get, path = "/listeners/tracks", tag = "listeners",
    params(TracksQuery),
    responses(
        (status = 200, body = Vec<TrackListeners>, content_type = "application/json"),
        (status = "4XX", body = ErrorBody)))]
async fn tracks(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<TracksQuery>,
) -> ApiResult<Response> {
    let (from, to) = period(&query.from, &query.to)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=500).contains(&limit) {
        return Err(ApiError::BadRequest("limit is 1 to 500".to_owned()));
    }
    let settings: StationSettings =
        settings::load_for(&state.pool, station.id(), StationSettings::SETTINGS_NAME).await?;
    let tz = settings.time_zone();
    let statistics: StatisticsSettings =
        settings::load(&state.pool, StatisticsSettings::SETTINGS_NAME).await?;
    let (start, _) = totals::day_bounds(from, &tz);
    let (_, end) = totals::day_bounds(to, &tz);
    let mut tracks = track_listeners(
        &state.pool,
        station.id(),
        (start, end),
        statistics.min_session_s,
        query.min_plays.unwrap_or(3),
    )
    .await?;
    tracks.truncate(limit as usize);
    Ok(match query.format.unwrap_or_default() {
        Format::Json => Json(tracks).into_response(),
        Format::Csv => csv_response(
            &format!("listeners-tracks-{from}-{to}.csv"),
            &tracks_csv(&tracks),
        ),
    })
}

/// The tracks played between two times (Unix ms), by departure rate.
pub async fn track_listeners(
    pool: &sqlx::MySqlPool,
    station_id: u64,
    (start, end): (i64, i64),
    min_session_s: u32,
    min_plays: u32,
) -> ApiResult<Vec<TrackListeners>> {
    // A session ends when its listener was last seen: they left within the
    // next minute, half a minute later on average.
    let rows: Vec<TrackRow> = sqlx::query_as(
        "SELECT p.track_id, MAX(t.artist), MAX(t.title), COUNT(*),
                CAST(AVG(p.start_l) AS DOUBLE), CAST(AVG(COALESCE(p.end_l, 0)) AS DOUBLE),
                CAST(SUM(p.start_l) AS SIGNED), CAST(SUM(p.dep) AS SIGNED),
                CAST(MAX(p.lists) AS SIGNED)
         FROM (SELECT h.track_id,
                 (SELECT SUM(s.listeners) FROM listener_samples s JOIN outputs o ON o.id = s.output_id
                  WHERE o.station_id = h.station_id
                    AND s.sampled_at = TIMESTAMP(DATE_FORMAT(h.started_at, '%Y-%m-%d %H:%i:00')))
                   AS start_l,
                 (SELECT SUM(s.listeners) FROM listener_samples s JOIN outputs o ON o.id = s.output_id
                  WHERE o.station_id = h.station_id
                    AND s.sampled_at = TIMESTAMP(DATE_FORMAT(h.ended_at, '%Y-%m-%d %H:%i:00')))
                   AS end_l,
                 (SELECT MAX(s.sessions) FROM listener_samples s JOIN outputs o ON o.id = s.output_id
                  WHERE o.station_id = h.station_id
                    AND s.sampled_at = TIMESTAMP(DATE_FORMAT(h.started_at, '%Y-%m-%d %H:%i:00')))
                   AS lists,
                 (SELECT COUNT(*) FROM listener_sessions x JOIN outputs o ON o.id = x.output_id
                  WHERE o.station_id = h.station_id
                    AND x.ended_at >= h.started_at - INTERVAL 30 SECOND
                    AND x.ended_at < h.ended_at - INTERVAL 30 SECOND
                    AND x.duration_s >= ? AND NOT x.split) AS dep
               FROM play_history h
               WHERE h.station_id = ? AND h.deck <> 'fx' AND h.track_id IS NOT NULL
                 AND h.ended_at IS NOT NULL
                 AND h.started_at >= FROM_UNIXTIME(?) AND h.started_at < FROM_UNIXTIME(?)) p
         JOIN tracks t ON t.id = p.track_id
         WHERE p.start_l IS NOT NULL
         GROUP BY p.track_id
         HAVING COUNT(*) >= ?",
    )
    .bind(min_session_s)
    .bind(station_id)
    .bind(start as f64 / 1_000.0)
    .bind(end as f64 / 1_000.0)
    .bind(min_plays)
    .fetch_all(pool)
    .await?;
    let mut tracks: Vec<TrackListeners> = rows
        .into_iter()
        .map(
            |(track_id, artist, title, plays, start, end, start_sum, departures, lists)| {
                let departures = (lists == Some(1)).then(|| u32::try_from(departures).unwrap_or(0));
                TrackListeners {
                    track_id,
                    artist,
                    title,
                    plays: u32::try_from(plays).unwrap_or(0),
                    listeners_start: start,
                    listeners_end: end,
                    departures,
                    departure_rate: departures
                        .filter(|_| start_sum > 0)
                        .map(|departures| f64::from(departures) / start_sum as f64),
                }
            },
        )
        .collect();
    tracks.sort_by(|a, b| {
        let change =
            |t: &TrackListeners| (t.listeners_end - t.listeners_start) / t.listeners_start.max(1.0);
        b.departure_rate
            .unwrap_or(-1.0)
            .total_cmp(&a.departure_rate.unwrap_or(-1.0))
            .then(change(a).total_cmp(&change(b)))
            .then(a.track_id.cmp(&b.track_id))
    });
    Ok(tracks)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PlayersQuery {
    /// First day (`YYYY-MM-DD`, station time).
    pub from: String,
    /// Last day, included.
    pub to: String,
    /// Only this output; the whole station otherwise.
    pub output_id: Option<u64>,
    /// 1 to 100, 20 by default.
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema, sqlx::FromRow)]
pub struct PlayerListening {
    /// As the player sends it.
    pub user_agent: String,
    /// Sessions of the minimum length.
    pub sessions: i64,
    pub listening_hours: f64,
}

/// Sessions and listening hours by player (user agent), most listened
/// first.
#[utoipa::path(get, path = "/listeners/players", tag = "listeners",
    params(PlayersQuery),
    responses((status = 200, body = Vec<PlayerListening>), (status = "4XX", body = ErrorBody)))]
async fn players(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<PlayersQuery>,
) -> ApiResult<Json<Vec<PlayerListening>>> {
    let (from, to) = period(&query.from, &query.to)?;
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::BadRequest("limit is 1 to 100".to_owned()));
    }
    let statistics: StatisticsSettings =
        settings::load(&state.pool, StatisticsSettings::SETTINGS_NAME).await?;
    let mut sql = sqlx::QueryBuilder::new(
        "SELECT user_agent, COUNT(*) AS sessions,
                CAST(SUM(duration_s) / 3600 AS DOUBLE) AS listening_hours
         FROM listener_sessions
         WHERE output_id IN (SELECT id FROM outputs WHERE station_id = ",
    );
    sql.push_bind(station.id())
        .push(") AND day >= ")
        .push_bind(from.to_string())
        .push(" AND day <= ")
        .push_bind(to.to_string())
        .push(" AND duration_s >= ")
        .push_bind(statistics.min_session_s);
    if let Some(output_id) = query.output_id {
        sql.push(" AND output_id = ").push_bind(output_id);
    }
    sql.push(" GROUP BY user_agent ORDER BY listening_hours DESC, sessions DESC LIMIT ")
        .push_bind(limit);
    Ok(Json(sql.build_query_as().fetch_all(&state.pool).await?))
}

fn csv_response(filename: &str, body: &str) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body.to_owned(),
    )
        .into_response()
}

/// A CSV field, quoted when needed. Fields starting like a formula are
/// prefixed with `'` so that spreadsheets do not run them.
fn field(text: &str) -> String {
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_owned()
    };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn number(value: f64) -> String {
    format!("{value:.2}")
}

fn optional<T: ToString>(value: Option<T>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

fn days_csv(totals: &[PeriodTotals]) -> String {
    let mut csv = String::from(
        "day,hour,days,minutes,peak,average,listening_hours,sessions,average_session_s,\
         distinct_listeners,listener_days\n",
    );
    for t in totals {
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{},{}",
            t.day,
            optional(t.hour),
            t.days,
            t.minutes,
            t.peak,
            number(t.average),
            number(t.listening_hours),
            optional(t.sessions),
            optional(t.average_session_s.map(number)),
            optional(t.distinct_listeners.map(number)),
            optional(t.listener_days),
        );
    }
    csv
}

fn hours_csv(hours: &[HourOfDay]) -> String {
    let mut csv = String::from("hour,days,average,peak,listening_hours\n");
    for h in hours {
        let _ = writeln!(
            csv,
            "{},{},{},{},{}",
            h.hour,
            h.days,
            number(h.average),
            h.peak,
            number(h.listening_hours)
        );
    }
    csv
}

fn tracks_csv(tracks: &[TrackListeners]) -> String {
    let mut csv = String::from(
        "track_id,artist,title,plays,listeners_start,listeners_end,departures,departure_rate\n",
    );
    for t in tracks {
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{}",
            t.track_id,
            field(t.artist.as_deref().unwrap_or_default()),
            field(t.title.as_deref().unwrap_or_default()),
            t.plays,
            number(t.listeners_start),
            number(t.listeners_end),
            optional(t.departures),
            optional(t.departure_rate.map(|rate| format!("{rate:.4}"))),
        );
    }
    csv
}

#[cfg(test)]
mod simulation;

#[cfg(test)]
mod tests {
    use super::*;

    fn day_row(
        day: &str,
        minutes: u32,
        peak: u32,
        average: f64,
        distinct: Option<u32>,
    ) -> TotalRow {
        (
            day.to_owned(),
            None,
            minutes,
            peak,
            average,
            average * f64::from(minutes) / 60.0,
            distinct.map(|d| d * 2),
            distinct.map(|d| u64::from(d) * 600),
            distinct,
        )
    }

    #[test]
    fn weeks_average_their_days() {
        // Monday 2026-10-19 to Sunday 2026-10-25, then Monday 2026-10-26.
        let rows = vec![
            day_row("2026-10-19", 1_440, 10, 4.0, Some(30)),
            day_row("2026-10-25", 720, 20, 8.0, Some(50)),
            day_row("2026-10-26", 1_440, 5, 2.0, None),
        ];
        let weeks = group_totals(rows, Grouping::Week);
        assert_eq!(weeks.len(), 2);
        let first = &weeks[0];
        assert_eq!(first.day, "2026-10-19");
        assert_eq!((first.days, first.minutes, first.peak), (2, 2_160, 20));
        assert!((first.average - (4.0 * 1_440.0 + 8.0 * 720.0) / 2_160.0).abs() < 1e-9);
        assert!((first.listening_hours - (96.0 + 96.0)).abs() < 1e-9);
        assert_eq!(first.sessions, Some(160));
        assert_eq!(first.average_session_s, Some(300.0));
        assert_eq!(first.distinct_listeners, Some(40.0));
        assert_eq!(first.listener_days, Some(80));
        let second = &weeks[1];
        assert_eq!(second.day, "2026-10-26");
        assert_eq!(
            (
                second.sessions,
                second.distinct_listeners,
                second.listener_days
            ),
            (None, None, None)
        );
        let months = group_totals(
            vec![day_row("2026-10-19", 60, 1, 1.0, Some(1))],
            Grouping::Month,
        );
        assert_eq!(months[0].day, "2026-10-01");
    }

    #[test]
    fn hours_of_the_day_average_over_days() {
        let hour = |day: &str, hour: u8, minutes: u32, average: f64| -> TotalRow {
            let mut row = day_row(day, minutes, 3, average, None);
            row.1 = Some(hour);
            row
        };
        let hours = hours_from(vec![
            hour("2026-10-24", 8, 60, 2.0),
            hour("2026-10-25", 8, 60, 4.0),
            hour("2026-10-25", 9, 30, 1.0),
        ]);
        assert_eq!(hours.len(), 2);
        assert_eq!((hours[0].hour, hours[0].days), (8, 2));
        assert!((hours[0].average - 3.0).abs() < 1e-9);
        assert!((hours[0].listening_hours - 3.0).abs() < 1e-9);
        assert!((hours[1].listening_hours - 0.5).abs() < 1e-9);
    }

    #[test]
    fn csv_fields_are_safe() {
        assert_eq!(field("Simple"), "Simple");
        assert_eq!(field("Earth, Wind & Fire"), "\"Earth, Wind & Fire\"");
        assert_eq!(field("Say \"hi\""), "\"Say \"\"hi\"\"\"");
        assert_eq!(field("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(field("-M-"), "'-M-");
        let csv = tracks_csv(&[TrackListeners {
            track_id: 1,
            artist: Some("A, B".to_owned()),
            title: None,
            plays: 3,
            listeners_start: 10.0,
            listeners_end: 8.5,
            departures: Some(2),
            departure_rate: Some(2.0 / 30.0),
        }]);
        assert_eq!(
            csv.lines().nth(1).unwrap(),
            "1,\"A, B\",,3,10.00,8.50,2,0.0667"
        );
    }
}
