// SPDX-License-Identifier: AGPL-3.0-or-later

//! Schedules: which clock runs at each hour of the week, in station time
//! (`docs/rotation.md#scheduling`).
//!
//! A station has a default schedule and may have others, each playing
//! during its periods; when periods overlap, the one that started last
//! plays. Hours a schedule leaves empty rotate the station's library.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use heron_rotation::schedule::{self, Period, Schedule};
use jiff::civil::DateTime;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate, is_still_used};
use crate::settings::{self, StationSettings};
use crate::stations::Station;

const MAX_PERIODS: usize = 100;
/// How far ahead the next change is looked for.
const NEXT_CHANGE_HOURS: i64 = 400 * 24;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_default, put_default))
        .routes(routes!(list_schedules, create_schedule))
        .routes(routes!(get_schedule, update_schedule, delete_schedule))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledHour {
    /// ISO 8601: 1 = Monday … 7 = Sunday.
    pub day_of_week: u8,
    /// 0 to 23, station time.
    pub hour: u8,
    pub clock_id: u64,
}

/// A schedule, without its hours.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ScheduleSummary {
    pub id: u64,
    /// Empty for the default schedule.
    pub name: String,
    pub is_default: bool,
    /// When it plays; empty for the default schedule.
    pub periods: Vec<Period>,
    /// Hours of the week it paints with a clock.
    pub hours: u32,
}

/// When the schedule playing changes next.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NextChange {
    /// Station time.
    #[schema(value_type = String, example = "2026-12-01T00:00:00")]
    pub at: DateTime,
    pub schedule_id: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Schedules {
    /// The default schedule first, then the others by name.
    pub schedules: Vec<ScheduleSummary>,
    /// The schedule playing now.
    pub active_id: u64,
    /// Within the next 400 days; missing when nothing changes.
    pub next_change: Option<NextChange>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ScheduleView {
    pub id: u64,
    pub name: String,
    pub is_default: bool,
    pub periods: Vec<Period>,
    pub hours: Vec<ScheduledHour>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewSchedule {
    pub name: String,
    /// A schedule of the station whose hours the new one starts with.
    #[serde(default)]
    pub copy_from: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleUpdate {
    /// Ignored for the default schedule.
    #[serde(default)]
    pub name: Option<String>,
    pub hours: Vec<ScheduledHour>,
    /// Refused on the default schedule.
    #[serde(default)]
    pub periods: Vec<Period>,
}

/// The station's default schedule, created when missing.
pub async fn default_id(pool: &MySqlPool, station_id: u64) -> ApiResult<u64> {
    let found: Option<u64> =
        sqlx::query_scalar("SELECT id FROM schedules WHERE station_id = ? AND is_default")
            .bind(station_id)
            .fetch_optional(pool)
            .await?;
    if let Some(id) = found {
        return Ok(id);
    }
    let result =
        sqlx::query("INSERT INTO schedules (station_id, name, is_default) VALUES (?, '', TRUE)")
            .bind(station_id)
            .execute(pool)
            .await;
    match result {
        Ok(done) => Ok(done.last_insert_id()),
        // Created at the same time by another request.
        Err(error) if is_duplicate(&error) => {
            sqlx::query_scalar("SELECT id FROM schedules WHERE station_id = ? AND is_default")
                .bind(station_id)
                .fetch_one(pool)
                .await
                .map_err(Into::into)
        }
        Err(error) => Err(error.into()),
    }
}

/// The station's default schedule and the others, for rotation.
pub async fn load(pool: &MySqlPool, station_id: u64) -> ApiResult<(Schedule, Vec<Schedule>)> {
    let default = default_id(pool, station_id).await?;
    let rows: Vec<(u64, String)> =
        sqlx::query_as("SELECT id, name FROM schedules WHERE station_id = ? ORDER BY id")
            .bind(station_id)
            .fetch_all(pool)
            .await?;
    let mut schedules: HashMap<u64, Schedule> = rows
        .into_iter()
        .map(|(id, name)| {
            (
                id,
                Schedule {
                    id,
                    name,
                    ..Schedule::default()
                },
            )
        })
        .collect();
    let rows: Vec<(u64, u8, u8, u64)> = sqlx::query_as(
        "SELECT cs.schedule_id, cs.day_of_week, cs.hour, cs.clock_id
         FROM clock_schedule cs JOIN schedules s ON s.id = cs.schedule_id
         WHERE s.station_id = ?",
    )
    .bind(station_id)
    .fetch_all(pool)
    .await?;
    for (id, day, hour, clock) in rows {
        if let Some(schedule) = schedules.get_mut(&id) {
            schedule.hours.insert((day, hour), clock);
        }
    }
    for (id, period) in periods(pool, station_id, None).await? {
        if let Some(schedule) = schedules.get_mut(&id) {
            schedule.periods.push(period);
        }
    }
    let default = schedules
        .remove(&default)
        .ok_or_else(|| ApiError::Internal("the default schedule went away".to_owned()))?;
    let mut others: Vec<Schedule> = schedules.into_values().collect();
    others.sort_by_key(|schedule| schedule.id);
    Ok((default, others))
}

/// The periods of the station's schedules (or of one), by schedule.
async fn periods(
    pool: &MySqlPool,
    station_id: u64,
    schedule_id: Option<u64>,
) -> ApiResult<Vec<(u64, Period)>> {
    let rows: Vec<(u64, String, String, bool)> = sqlx::query_as(
        "SELECT p.schedule_id, DATE_FORMAT(p.starts_at, '%Y-%m-%dT%H:%i:%s'),
                DATE_FORMAT(p.ends_at, '%Y-%m-%dT%H:%i:%s'), p.every_year
         FROM schedule_periods p JOIN schedules s ON s.id = p.schedule_id
         WHERE s.station_id = ? AND (? IS NULL OR s.id = ?)
         ORDER BY p.schedule_id, p.starts_at, p.id",
    )
    .bind(station_id)
    .bind(schedule_id)
    .bind(schedule_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|(id, start, end, every_year)| {
            let parse = |text: &str| {
                text.parse::<DateTime>()
                    .map_err(|error| ApiError::Internal(format!("schedule {id}: {error}")))
            };
            Ok((
                id,
                Period {
                    start: parse(&start)?,
                    end: parse(&end)?,
                    every_year,
                },
            ))
        })
        .collect()
}

/// Lists the station's schedules, the one playing now and the next change.
#[utoipa::path(get, path = "/schedules", tag = "rotation",
    responses((status = 200, body = Schedules), (status = "4XX", body = ErrorBody)))]
async fn list_schedules(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Schedules>> {
    let (default, mut others) = load(&state.pool, station.id()).await?;
    let settings: StationSettings =
        settings::load_for(&state.pool, station.id(), StationSettings::SETTINGS_NAME).await?;
    let now = jiff::Zoned::now()
        .with_time_zone(settings.time_zone())
        .datetime();
    let active_id = schedule::playing(&default, &others, now).id;
    let next_change =
        schedule::next_change(&default, &others, now, NEXT_CHANGE_HOURS).map(|(at, schedule)| {
            NextChange {
                at,
                schedule_id: schedule.id,
            }
        });
    others.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    let summary = |schedule: Schedule, is_default: bool| ScheduleSummary {
        id: schedule.id,
        name: schedule.name,
        is_default,
        periods: schedule.periods,
        hours: u32::try_from(schedule.hours.len()).unwrap_or(u32::MAX),
    };
    let schedules = std::iter::once(summary(default, true))
        .chain(others.into_iter().map(|schedule| summary(schedule, false)))
        .collect();
    Ok(Json(Schedules {
        schedules,
        active_id,
        next_change,
    }))
}

async fn view(pool: &MySqlPool, station_id: u64, id: u64) -> ApiResult<ScheduleView> {
    let (name, is_default): (String, bool) =
        sqlx::query_as("SELECT name, is_default FROM schedules WHERE id = ? AND station_id = ?")
            .bind(id)
            .bind(station_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| ApiError::not_found("schedule"))?;
    let rows: Vec<(u8, u8, u64)> = sqlx::query_as(
        "SELECT day_of_week, hour, clock_id FROM clock_schedule WHERE schedule_id = ?
         ORDER BY day_of_week, hour",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(ScheduleView {
        id,
        name,
        is_default,
        periods: periods(pool, station_id, Some(id))
            .await?
            .into_iter()
            .map(|(_, period)| period)
            .collect(),
        hours: rows
            .into_iter()
            .map(|(day_of_week, hour, clock_id)| ScheduledHour {
                day_of_week,
                hour,
                clock_id,
            })
            .collect(),
    })
}

#[utoipa::path(get, path = "/schedules/{id}", tag = "rotation",
    params(("id" = u64, Path)),
    responses((status = 200, body = ScheduleView), (status = "4XX", body = ErrorBody)))]
async fn get_schedule(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<ScheduleView>> {
    view(&state.pool, station.id(), id).await.map(Json)
}

fn check_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(ApiError::Invalid(
            "a name is 1 to 255 characters".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

fn conflict(error: sqlx::Error) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict("another schedule has this name".to_owned())
    } else {
        error.into()
    }
}

/// Adds a schedule, empty or a copy of another's hours. Operators and admins.
#[utoipa::path(post, path = "/schedules", tag = "rotation",
    request_body = NewSchedule,
    responses((status = 201, body = ScheduleView), (status = "4XX", body = ErrorBody)))]
async fn create_schedule(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(request): Json<NewSchedule>,
) -> ApiResult<(StatusCode, Json<ScheduleView>)> {
    user.require(Role::Operator)?;
    let name = check_name(&request.name)?;
    default_id(&state.pool, station.id()).await?;
    let mut tx = state.pool.begin().await?;
    if let Some(from) = request.copy_from {
        let found: Option<u64> =
            sqlx::query_scalar("SELECT id FROM schedules WHERE id = ? AND station_id = ?")
                .bind(from)
                .bind(station.id())
                .fetch_optional(&mut *tx)
                .await?;
        if found.is_none() {
            return Err(ApiError::Invalid(format!("no schedule {from}")));
        }
    }
    let id =
        sqlx::query("INSERT INTO schedules (station_id, name, is_default) VALUES (?, ?, FALSE)")
            .bind(station.id())
            .bind(&name)
            .execute(&mut *tx)
            .await
            .map_err(conflict)?
            .last_insert_id();
    if let Some(from) = request.copy_from {
        sqlx::query(
            "INSERT INTO clock_schedule (schedule_id, day_of_week, hour, clock_id)
             SELECT ?, day_of_week, hour, clock_id FROM clock_schedule WHERE schedule_id = ?",
        )
        .bind(id)
        .bind(from)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    tracing::info!(user = %user.username, schedule = %name, "schedule added");
    Ok((
        StatusCode::CREATED,
        Json(view(&state.pool, station.id(), id).await?),
    ))
}

async fn check_hours(pool: &MySqlPool, station_id: u64, hours: &[ScheduledHour]) -> ApiResult<()> {
    let mut seen = HashSet::new();
    for hour in hours {
        if !(1..=7).contains(&hour.day_of_week) || hour.hour > 23 {
            return Err(ApiError::Invalid(
                "days are 1 to 7 and hours 0 to 23".to_owned(),
            ));
        }
        if !seen.insert((hour.day_of_week, hour.hour)) {
            return Err(ApiError::Invalid(format!(
                "day {} hour {} is scheduled twice",
                hour.day_of_week, hour.hour
            )));
        }
    }
    let clocks: HashSet<u64> = sqlx::query_scalar("SELECT id FROM clocks WHERE station_id = ?")
        .bind(station_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    if let Some(hour) = hours.iter().find(|hour| !clocks.contains(&hour.clock_id)) {
        return Err(ApiError::Invalid(format!("no clock {}", hour.clock_id)));
    }
    Ok(())
}

/// Replaces a schedule's hours and periods, and renames it. Operators and
/// admins.
#[utoipa::path(put, path = "/schedules/{id}", tag = "rotation",
    params(("id" = u64, Path)),
    request_body = ScheduleUpdate,
    responses((status = 200, body = ScheduleView), (status = "4XX", body = ErrorBody)))]
async fn update_schedule(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(request): Json<ScheduleUpdate>,
) -> ApiResult<Json<ScheduleView>> {
    user.require(Role::Operator)?;
    let current = view(&state.pool, station.id(), id).await?;
    if current.is_default && !request.periods.is_empty() {
        return Err(ApiError::Invalid(
            "the default schedule plays outside the other schedules' periods: it has none"
                .to_owned(),
        ));
    }
    if request.periods.len() > MAX_PERIODS {
        return Err(ApiError::Invalid(format!(
            "a schedule has at most {MAX_PERIODS} periods"
        )));
    }
    for period in &request.periods {
        period.check().map_err(ApiError::Invalid)?;
    }
    let name = match &request.name {
        Some(name) if !current.is_default => Some(check_name(name)?),
        _ => None,
    };
    check_hours(&state.pool, station.id(), &request.hours).await?;
    let mut tx = state.pool.begin().await?;
    if let Some(name) = &name {
        sqlx::query("UPDATE schedules SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(conflict)?;
    }
    write_hours(&mut tx, id, &request.hours).await?;
    sqlx::query("DELETE FROM schedule_periods WHERE schedule_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for period in &request.periods {
        sqlx::query(
            "INSERT INTO schedule_periods (schedule_id, starts_at, ends_at, every_year)
             VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(sql_datetime(period.start))
        .bind(sql_datetime(period.end))
        .bind(period.every_year)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    tracing::info!(
        user = %user.username,
        schedule = id,
        hours = request.hours.len(),
        periods = request.periods.len(),
        "schedule changed"
    );
    view(&state.pool, station.id(), id).await.map(Json)
}

fn sql_datetime(at: DateTime) -> String {
    at.strftime("%Y-%m-%d %H:%M:%S").to_string()
}

async fn write_hours(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    id: u64,
    hours: &[ScheduledHour],
) -> ApiResult<()> {
    sqlx::query("DELETE FROM clock_schedule WHERE schedule_id = ?")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    for hour in hours {
        sqlx::query(
            "INSERT INTO clock_schedule (schedule_id, day_of_week, hour, clock_id) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(hour.day_of_week)
        .bind(hour.hour)
        .bind(hour.clock_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| {
            if is_still_used(&error) {
                ApiError::Invalid(format!("no clock {}", hour.clock_id))
            } else {
                error.into()
            }
        })?;
    }
    Ok(())
}

/// Deletes a schedule other than the default one. Operators and admins.
#[utoipa::path(delete, path = "/schedules/{id}", tag = "rotation",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_schedule(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let current = view(&state.pool, station.id(), id).await?;
    if current.is_default {
        return Err(ApiError::Conflict(
            "the default schedule cannot be deleted".to_owned(),
        ));
    }
    sqlx::query("DELETE FROM schedules WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    tracing::info!(user = %user.username, schedule = %current.name, "schedule deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// The default schedule's hours; hours left out rotate the station's library.
#[utoipa::path(get, path = "/schedule", tag = "rotation",
    responses((status = 200, body = Vec<ScheduledHour>), (status = "4XX", body = ErrorBody)))]
async fn get_default(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Vec<ScheduledHour>>> {
    let id = default_id(&state.pool, station.id()).await?;
    Ok(Json(view(&state.pool, station.id(), id).await?.hours))
}

/// Replaces the default schedule's whole week. Operators and admins.
#[utoipa::path(put, path = "/schedule", tag = "rotation",
    request_body = Vec<ScheduledHour>,
    responses((status = 200, body = Vec<ScheduledHour>), (status = "4XX", body = ErrorBody)))]
async fn put_default(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(hours): Json<Vec<ScheduledHour>>,
) -> ApiResult<Json<Vec<ScheduledHour>>> {
    user.require(Role::Operator)?;
    check_hours(&state.pool, station.id(), &hours).await?;
    let id = default_id(&state.pool, station.id()).await?;
    let mut tx = state.pool.begin().await?;
    write_hours(&mut tx, id, &hours).await?;
    tx.commit().await?;
    tracing::info!(user = %user.username, hours = hours.len(), "schedule changed");
    Ok(Json(view(&state.pool, station.id(), id).await?.hours))
}
