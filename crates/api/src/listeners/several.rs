// SPDX-License-Identifier: AGPL-3.0-or-later

//! The statistics of several stations at once: each station's figures and
//! the stations added up, counted like a station's outputs (minute by
//! minute, session by session), among the stations the user sees. See
//! `docs/statistics.md#several-stations`.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::{
    Filter, Format, Grouping, HourOfDay, MAX_SAMPLE_RANGE_MS, MinuteSample, PeriodTotals, TotalRow,
    audience, audience_csv, audience_of, bounds_s, breakdown, breakdown_csv, counted, csv_response,
    days_csv, field, group_totals, hours_csv, hours_from, period, previous_period, push_stations,
    total_rows, totals, with_tunes,
};
use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::modules::{self, Module};
use crate::settings::{self, StationSettings, StatisticsSettings};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(stations_now))
        .routes(routes!(stations_samples))
        .routes(routes!(stations_audience))
        .routes(routes!(stations_days))
        .routes(routes!(stations_hours_of_day))
        .routes(routes!(stations_breakdown))
}

/// A station of the answer.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationRef {
    pub station_id: u64,
    pub code: String,
    pub name: String,
}

/// A station chosen, with its time zone.
struct Chosen {
    station: StationRef,
    tz: TimeZone,
}

fn ids(chosen: &[Chosen]) -> Vec<u64> {
    chosen.iter().map(|c| c.station.station_id).collect()
}

/// The time zone of the stations added up: the first one's.
fn zone(chosen: &[Chosen]) -> TimeZone {
    chosen.first().map_or(TimeZone::UTC, |c| c.tz.clone())
}

/// The stations of `stations` (`all`, or IDs such as `1,2`), in the
/// stations' order: only those the user sees, `404` for another one, as
/// its own routes answer.
async fn chosen(state: &AppState, user: &CurrentUser, stations: &str) -> ApiResult<Vec<Chosen>> {
    let wanted: Option<Vec<u64>> = match stations.trim() {
        "all" => None,
        list => Some(
            list.split(',')
                .map(|id| id.trim().parse::<u64>())
                .collect::<Result<_, _>>()
                .map_err(|_| {
                    ApiError::BadRequest(
                        "stations is all or station IDs separated by commas".to_owned(),
                    )
                })?,
        ),
    };
    let rows: Vec<(u64, String, String)> =
        sqlx::query_as("SELECT id, code, name FROM stations ORDER BY position, id")
            .fetch_all(&state.pool)
            .await?;
    let seen: Option<Vec<u64>> = if user.role == Role::Admin {
        None
    } else {
        Some(
            sqlx::query_scalar(
                "SELECT station_id FROM user_station_roles WHERE user_id = ? AND role <> 'none'",
            )
            .bind(user.id)
            .fetch_all(&state.pool)
            .await?,
        )
    };
    let visible = |id: u64| seen.as_ref().is_none_or(|seen| seen.contains(&id));
    if let Some(wanted) = &wanted {
        for &id in wanted {
            if !visible(id) || !rows.iter().any(|(station, _, _)| *station == id) {
                return Err(ApiError::NotFound(format!("station {id} not found")));
            }
        }
    }
    let mut chosen = Vec::new();
    for (id, code, name) in rows {
        if !visible(id) || wanted.as_ref().is_some_and(|wanted| !wanted.contains(&id)) {
            continue;
        }
        if !modules::on_for(&state.pool, id)
            .await?
            .is_on(Module::ListenerStatistics)
        {
            if wanted.is_some() {
                return Err(ApiError::ModuleOff(Module::ListenerStatistics));
            }
            continue;
        }
        let settings: StationSettings =
            settings::load_for(&state.pool, id, StationSettings::SETTINGS_NAME).await?;
        chosen.push(Chosen {
            station: StationRef {
                station_id: id,
                code,
                name,
            },
            tz: settings.time_zone(),
        });
    }
    Ok(chosen)
}

/// Each station's CSV with a first `station` column (its code), then the
/// stations added up (an empty code).
fn with_station(parts: &[(&str, String)]) -> String {
    let mut csv = String::new();
    for (n, (code, part)) in parts.iter().enumerate() {
        let mut lines = part.lines();
        let header = lines.next().unwrap_or_default();
        if n == 0 {
            csv.push_str("station,");
            csv.push_str(header);
            csv.push('\n');
        }
        for line in lines {
            csv.push_str(&field(code));
            csv.push(',');
            csv.push_str(line);
            csv.push('\n');
        }
    }
    csv
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NowQuery {
    /// `all` (every station the user sees), or station IDs (`1,2`).
    pub stations: String,
}

/// A station's listeners at the last minute.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationNow {
    #[serde(flatten)]
    pub station: StationRef,
    /// Its outputs' listeners added up; missing when none could be read.
    pub total: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationsNow {
    pub stations: Vec<StationNow>,
    /// The stations' listeners added up; missing when none could be read.
    pub total: Option<u32>,
}

/// Each station's listeners at the last minute, and their total.
#[utoipa::path(get, path = "/stations/listeners/now", tag = "listeners",
    params(NowQuery),
    responses((status = 200, body = StationsNow), (status = "4XX", body = ErrorBody)))]
async fn stations_now(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<NowQuery>,
) -> ApiResult<Json<StationsNow>> {
    let chosen = chosen(&state, &user, &query.stations).await?;
    let outputs: Vec<(u64, u64)> = sqlx::query_as("SELECT id, station_id FROM outputs")
        .fetch_all(&state.pool)
        .await?;
    let last = state.listeners.last();
    let stations: Vec<StationNow> = chosen
        .into_iter()
        .map(|c| {
            let read: Vec<u32> = last
                .iter()
                .filter(|o| outputs.contains(&(o.output_id, c.station.station_id)))
                .filter_map(|o| o.listeners)
                .collect();
            StationNow {
                station: c.station,
                total: (!read.is_empty()).then(|| read.iter().sum()),
            }
        })
        .collect();
    let read: Vec<u32> = stations.iter().filter_map(|s| s.total).collect();
    Ok(Json(StationsNow {
        total: (!read.is_empty()).then(|| read.iter().sum()),
        stations,
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SamplesQuery {
    /// `all` (every station the user sees), or station IDs (`1,2`).
    pub stations: String,
    /// From this time (Unix ms).
    pub from_unix_ms: i64,
    /// Until this time (Unix ms), at most 8 days after `from_unix_ms`.
    pub to_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationSamples {
    #[serde(flatten)]
    pub station: StationRef,
    pub samples: Vec<MinuteSample>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationsSamples {
    pub stations: Vec<StationSamples>,
    /// The stations' listeners added up, minute by minute.
    pub total: Vec<MinuteSample>,
}

/// Each station's listeners minute by minute, and the stations' added up.
#[utoipa::path(get, path = "/stations/listeners/samples", tag = "listeners",
    params(SamplesQuery),
    responses((status = 200, body = StationsSamples), (status = "4XX", body = ErrorBody)))]
async fn stations_samples(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<SamplesQuery>,
) -> ApiResult<Json<StationsSamples>> {
    if query.to_unix_ms < query.from_unix_ms
        || query.to_unix_ms - query.from_unix_ms > MAX_SAMPLE_RANGE_MS
    {
        return Err(ApiError::BadRequest(
            "to_unix_ms must follow from_unix_ms by at most 8 days".to_owned(),
        ));
    }
    let chosen = chosen(&state, &user, &query.stations).await?;
    let mut sql = sqlx::QueryBuilder::new(
        "SELECT o.station_id, CAST(UNIX_TIMESTAMP(s.sampled_at) * 1000 AS SIGNED),
                CAST(SUM(s.listeners) AS UNSIGNED)
         FROM listener_samples s JOIN outputs o ON o.id = s.output_id
         WHERE ",
    );
    push_stations(&mut sql, &ids(&chosen));
    sql.push(" AND s.sampled_at >= FROM_UNIXTIME(")
        .push_bind(query.from_unix_ms as f64 / 1_000.0)
        .push(") AND s.sampled_at <= FROM_UNIXTIME(")
        .push_bind(query.to_unix_ms as f64 / 1_000.0)
        .push(") GROUP BY o.station_id, s.sampled_at ORDER BY s.sampled_at");
    let rows: Vec<(u64, i64, u64)> = sql.build_query_as().fetch_all(&state.pool).await?;
    let mut total: BTreeMap<i64, u64> = BTreeMap::new();
    for &(_, at, listeners) in &rows {
        *total.entry(at).or_default() += listeners;
    }
    let sample = |at_unix_ms: i64, listeners: u64| MinuteSample {
        at_unix_ms,
        listeners: u32::try_from(listeners).unwrap_or(u32::MAX),
    };
    Ok(Json(StationsSamples {
        stations: chosen
            .into_iter()
            .map(|c| StationSamples {
                samples: rows
                    .iter()
                    .filter(|(id, _, _)| *id == c.station.station_id)
                    .map(|&(_, at, listeners)| sample(at, listeners))
                    .collect(),
                station: c.station,
            })
            .collect(),
        total: total
            .into_iter()
            .map(|(at, listeners)| sample(at, listeners))
            .collect(),
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PeriodQuery {
    /// `all` (every station the user sees), or station IDs (`1,2`).
    pub stations: String,
    /// First day (`YYYY-MM-DD`, station time).
    pub from: String,
    /// Last day, included.
    pub to: String,
    #[param(inline)]
    pub format: Option<Format>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationAudience {
    #[serde(flatten)]
    pub station: StationRef,
    pub current: audience::Audience,
    pub previous: audience::Audience,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationsAudience {
    pub stations: Vec<StationAudience>,
    /// The stations together: the peak and AQH on their minutes added up,
    /// a listener of two stations once.
    pub total: audience::AudienceComparison,
}

/// Each station's audience figures and the previous period's, and the
/// stations' together.
#[utoipa::path(get, path = "/stations/listeners/audience", tag = "listeners",
    params(PeriodQuery),
    responses(
        (status = 200, body = StationsAudience, content_type = "application/json"),
        (status = "4XX", body = ErrorBody)))]
async fn stations_audience(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<PeriodQuery>,
) -> ApiResult<Response> {
    let (from, to) = period(&query.from, &query.to)?;
    let chosen = chosen(&state, &user, &query.stations).await?;
    let pool = &state.pool;
    let all = Filter::default();
    let before = previous_period((from, to));
    let mut stations = Vec::new();
    for c in &chosen {
        let id = [c.station.station_id];
        stations.push(StationAudience {
            station: c.station.clone(),
            current: audience_of(pool, &id, (from, to), &c.tz, all).await?,
            previous: audience_of(pool, &id, before, &c.tz, all).await?,
        });
    }
    let (ids, tz) = (ids(&chosen), zone(&chosen));
    let total = audience::AudienceComparison {
        current: audience_of(pool, &ids, (from, to), &tz, all).await?,
        previous: audience_of(pool, &ids, before, &tz, all).await?,
    };
    Ok(match query.format.unwrap_or_default() {
        Format::Json => Json(StationsAudience { stations, total }).into_response(),
        Format::Csv => {
            let mut parts: Vec<(&str, String)> = stations
                .iter()
                .zip(&chosen)
                .map(|(s, c)| {
                    let figures = audience::AudienceComparison {
                        current: s.current.clone(),
                        previous: s.previous.clone(),
                    };
                    (s.station.code.as_str(), audience_csv(&figures, &c.tz))
                })
                .collect();
            parts.push(("", audience_csv(&total, &tz)));
            csv_response(
                &format!("listeners-audience-stations-{from}-{to}.csv"),
                &with_station(&parts),
            )
        }
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DaysQuery {
    /// `all` (every station the user sees), or station IDs (`1,2`).
    pub stations: String,
    /// First day (`YYYY-MM-DD`, station time).
    pub from: String,
    /// Last day, included.
    pub to: String,
    /// Rows per hour, day (by default), week or month.
    #[param(inline)]
    pub group: Option<Grouping>,
    #[param(inline)]
    pub format: Option<Format>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationDays {
    #[serde(flatten)]
    pub station: StationRef,
    pub periods: Vec<PeriodTotals>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationsDays {
    pub stations: Vec<StationDays>,
    /// The stations together, in the first one's time zone.
    pub total: Vec<PeriodTotals>,
}

/// The stations' totals added up, per day or hour: from their minutes and
/// sessions together while they are kept (the peak is the highest minute
/// of the stations together, a listener of two stations counts once), from
/// each station's totals for the days purged.
async fn combined_rows(
    state: &AppState,
    chosen: &[Chosen],
    (from, to): (Date, Date),
    hourly: bool,
) -> ApiResult<Vec<TotalRow>> {
    if chosen.is_empty() {
        return Ok(Vec::new());
    }
    let (ids, tz) = (ids(chosen), zone(chosen));
    let statistics: StatisticsSettings =
        settings::load(&state.pool, StatisticsSettings::SETTINGS_NAME).await?;

    // Each station's totals, added up: what is left of the days purged.
    #[derive(Default)]
    struct Added {
        minutes: u32,
        peak: u32,
        listened: f64,
        sessions: Option<u32>,
        session_s: Option<u64>,
        distinct: Option<u32>,
    }
    let mut added: BTreeMap<(String, Option<u8>), Added> = BTreeMap::new();
    for &id in &ids {
        for (day, hour, minutes, peak, _, hours, sessions, session_s, distinct) in
            total_rows(state, id, from, to, Filter::default(), hourly).await?
        {
            let sum = added.entry((day, hour)).or_default();
            sum.minutes = sum.minutes.max(minutes);
            sum.peak = sum.peak.max(peak);
            sum.listened += hours * 60.0;
            for (total, value) in [(&mut sum.sessions, sessions), (&mut sum.distinct, distinct)] {
                if let Some(value) = value {
                    *total.get_or_insert(0) += value;
                }
            }
            if let Some(value) = session_s {
                *sum.session_s.get_or_insert(0) += value;
            }
        }
    }

    // The minutes and sessions of the stations together, day by day.
    let (start, end) = bounds_s((from, to), &tz);
    let mut sql = sqlx::QueryBuilder::new(
        "SELECT CAST(UNIX_TIMESTAMP(s.sampled_at) * 1000 AS SIGNED),
                CAST(SUM(s.listeners) AS UNSIGNED), CAST(MAX(s.sessions) AS SIGNED)
         FROM listener_samples s JOIN outputs o ON o.id = s.output_id
         WHERE ",
    );
    push_stations(&mut sql, &ids);
    sql.push(" AND s.sampled_at >= FROM_UNIXTIME(")
        .push_bind(start)
        .push(") AND s.sampled_at < FROM_UNIXTIME(")
        .push_bind(end)
        .push(") GROUP BY s.sampled_at");
    let minutes: Vec<(i64, u64, i64)> = sql.build_query_as().fetch_all(&state.pool).await?;
    let mut days: BTreeMap<Date, (Vec<totals::Sample>, Vec<totals::Span>)> = BTreeMap::new();
    for (minute_ms, listeners, sessions) in minutes {
        days.entry(totals::day_of(minute_ms, &tz))
            .or_default()
            .0
            .push(totals::Sample {
                output_id: 0,
                server_id: 0,
                minute_ms,
                listeners: u32::try_from(listeners).unwrap_or(u32::MAX),
                sessions: sessions == 1,
            });
    }
    let mut sql = sqlx::QueryBuilder::new(
        "SELECT CAST(s.day AS CHAR), s.listener_key,
                CAST(UNIX_TIMESTAMP(s.started_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(s.last_seen_at) * 1000 AS SIGNED), s.duration_s
         FROM listener_sessions s JOIN outputs o ON o.id = s.output_id
         WHERE ",
    );
    push_stations(&mut sql, &ids);
    sql.push(" AND s.day >= ")
        .push_bind(from.to_string())
        .push(" AND s.day <= ")
        .push_bind(to.to_string())
        .push(" AND s.duration_s >= ")
        .push_bind(statistics.min_session_s);
    let spans: Vec<(String, Vec<u8>, i64, i64, u32)> =
        sql.build_query_as().fetch_all(&state.pool).await?;
    for (day, key, started_ms, last_seen_ms, duration_s) in spans {
        let (Ok(day), Ok(key)) = (day.parse::<Date>(), key.try_into()) else {
            continue;
        };
        // Only the days with minutes: the others keep their totals.
        if let Some((_, spans)) = days.get_mut(&day) {
            spans.push(totals::Span {
                output_id: 0,
                server_id: 0,
                key,
                started_ms,
                last_seen_ms,
                duration_s,
            });
        }
    }
    let mut rows: BTreeMap<(String, Option<u8>), TotalRow> = added
        .into_iter()
        .map(|((day, hour), sum)| {
            let average = if sum.minutes == 0 {
                0.0
            } else {
                sum.listened / f64::from(sum.minutes)
            };
            let row = (
                day.clone(),
                hour,
                sum.minutes,
                sum.peak,
                average,
                sum.listened / 60.0,
                sum.sessions,
                sum.session_s,
                sum.distinct,
            );
            ((day, hour), row)
        })
        .collect();
    for (day, (samples, spans)) in days {
        let day = day.to_string();
        rows.retain(|(d, _), _| *d != day);
        for total in totals::scope_totals(&samples, &spans, &tz, statistics.min_session_s) {
            if total.output_id.is_some() || total.hour.is_some() != hourly {
                continue;
            }
            rows.insert(
                (day.clone(), total.hour),
                (
                    day.clone(),
                    total.hour,
                    total.minutes,
                    total.peak,
                    total.average,
                    total.listening_hours,
                    total.sessions,
                    total.session_s,
                    total.distinct_listeners,
                ),
            );
        }
    }
    Ok(rows.into_values().collect())
}

/// Each station's totals per hour, day, week or month, and the stations'
/// together.
#[utoipa::path(get, path = "/stations/listeners/days", tag = "listeners",
    params(DaysQuery),
    responses(
        (status = 200, body = StationsDays, content_type = "application/json"),
        (status = "4XX", body = ErrorBody)))]
async fn stations_days(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<DaysQuery>,
) -> ApiResult<Response> {
    let (from, to) = period(&query.from, &query.to)?;
    let grouping = query.group.unwrap_or_default();
    let hourly = grouping == Grouping::Hour;
    let chosen = chosen(&state, &user, &query.stations).await?;
    let mut stations = Vec::new();
    for c in &chosen {
        let rows = total_rows(
            &state,
            c.station.station_id,
            from,
            to,
            Filter::default(),
            hourly,
        )
        .await?;
        stations.push(StationDays {
            station: c.station.clone(),
            periods: group_totals(rows, grouping),
        });
    }
    let total = group_totals(
        combined_rows(&state, &chosen, (from, to), hourly).await?,
        grouping,
    );
    Ok(match query.format.unwrap_or_default() {
        Format::Json => Json(StationsDays { stations, total }).into_response(),
        Format::Csv => {
            let mut parts: Vec<(&str, String)> = stations
                .iter()
                .map(|s| (s.station.code.as_str(), days_csv(&s.periods)))
                .collect();
            parts.push(("", days_csv(&total)));
            csv_response(
                &format!("listeners-stations-{from}-{to}.csv"),
                &with_station(&parts),
            )
        }
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationHours {
    #[serde(flatten)]
    pub station: StationRef,
    pub hours: Vec<HourOfDay>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationsHours {
    pub stations: Vec<StationHours>,
    /// The stations together, in the first one's time zone.
    pub total: Vec<HourOfDay>,
}

/// Each station's hours of the day, and the stations' together.
#[utoipa::path(get, path = "/stations/listeners/hours-of-day", tag = "listeners",
    params(PeriodQuery),
    responses(
        (status = 200, body = StationsHours, content_type = "application/json"),
        (status = "4XX", body = ErrorBody)))]
async fn stations_hours_of_day(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<PeriodQuery>,
) -> ApiResult<Response> {
    let (from, to) = period(&query.from, &query.to)?;
    let chosen = chosen(&state, &user, &query.stations).await?;
    let all = Filter::default();
    let mut stations = Vec::new();
    for c in &chosen {
        let id = c.station.station_id;
        let rows = total_rows(&state, id, from, to, all, true).await?;
        stations.push(StationHours {
            station: c.station.clone(),
            hours: with_tunes(&state.pool, hours_from(rows), &[id], (from, to), &c.tz, all).await?,
        });
    }
    let rows = combined_rows(&state, &chosen, (from, to), true).await?;
    let total = with_tunes(
        &state.pool,
        hours_from(rows),
        &ids(&chosen),
        (from, to),
        &zone(&chosen),
        all,
    )
    .await?;
    Ok(match query.format.unwrap_or_default() {
        Format::Json => Json(StationsHours { stations, total }).into_response(),
        Format::Csv => {
            let mut parts: Vec<(&str, String)> = stations
                .iter()
                .map(|s| (s.station.code.as_str(), hours_csv(&s.hours)))
                .collect();
            parts.push(("", hours_csv(&total)));
            csv_response(
                &format!("listeners-hours-stations-{from}-{to}.csv"),
                &with_station(&parts),
            )
        }
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BreakdownQuery {
    /// `all` (every station the user sees), or station IDs (`1,2`).
    pub stations: String,
    /// First day (`YYYY-MM-DD`, station time).
    pub from: String,
    /// Last day, included.
    pub to: String,
    /// By country (by default), device or player.
    #[param(inline)]
    pub by: Option<breakdown::By>,
    #[param(inline)]
    pub format: Option<Format>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationShares {
    #[serde(flatten)]
    pub station: StationRef,
    pub groups: Vec<breakdown::Share>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationsBreakdown {
    /// The GeoIP country database's type when countries are on, `null`
    /// otherwise.
    pub geoip: Option<String>,
    pub stations: Vec<StationShares>,
    /// The stations together: a listener of two stations once.
    pub total: Vec<breakdown::Share>,
}

/// Each station's shares by country, device or player, and the stations'
/// together.
#[utoipa::path(get, path = "/stations/listeners/breakdown", tag = "listeners",
    params(BreakdownQuery),
    responses(
        (status = 200, body = StationsBreakdown, content_type = "application/json"),
        (status = "4XX", body = ErrorBody)))]
async fn stations_breakdown(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<BreakdownQuery>,
) -> ApiResult<Response> {
    let (from, to) = period(&query.from, &query.to)?;
    let by = query.by.unwrap_or_default();
    let chosen = chosen(&state, &user, &query.stations).await?;
    let all = Filter::default();
    let mut stations = Vec::new();
    let mut together = Vec::new();
    for c in &chosen {
        let sessions = counted(&state.pool, &[c.station.station_id], (from, to), all, by).await?;
        stations.push(StationShares {
            station: c.station.clone(),
            groups: breakdown::shares(&sessions, by.min_listeners()),
        });
        together.extend(sessions);
    }
    let figures = StationsBreakdown {
        geoip: state.listeners.geoip_database(),
        stations,
        total: breakdown::shares(&together, by.min_listeners()),
    };
    Ok(match query.format.unwrap_or_default() {
        Format::Json => Json(figures).into_response(),
        Format::Csv => {
            let name = match by {
                breakdown::By::Country => "country",
                breakdown::By::Device => "device",
                breakdown::By::Player => "player",
            };
            let mut parts: Vec<(&str, String)> = figures
                .stations
                .iter()
                .map(|s| (s.station.code.as_str(), breakdown_csv(name, &s.groups)))
                .collect();
            parts.push(("", breakdown_csv(name, &figures.total)));
            csv_response(
                &format!("listeners-{name}-stations-{from}-{to}.csv"),
                &with_station(&parts),
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_gets_a_station_column() {
        let csv = with_station(&[
            ("hits", "hour,peak\n8,3\n9,4\n".to_owned()),
            ("", "hour,peak\n8,5\n".to_owned()),
        ]);
        assert_eq!(csv, "station,hour,peak\nhits,8,3\nhits,9,4\n,8,5\n");
    }
}
