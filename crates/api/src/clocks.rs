// SPDX-License-Identifier: AGPL-3.0-or-later

//! Clocks (clockwheels); the schedules are in `schedules`.
//!
//! A clock is an ordered list of slots; each slot calls a category, with a
//! selection method, an enforce-rules flag and separation overrides. The
//! schedule says which clock runs at each hour of the week, in the
//! station's time zone. Hours without a clock rotate the station's library.
//! A clock starts over at each hour, unless it loops: then it carries on
//! from its last slot across hours, like SAM Broadcaster's clockwheel.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use heron_rotation::{Method, SeparationOverrides};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::categories::separation_json;
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate, is_still_used};
use crate::rotation::{self, overrides};
use crate::stations::Station;

const MAX_SLOTS: usize = 100;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_clocks, create_clock))
        .routes(routes!(get_clock, update_clock, delete_clock))
        .routes(routes!(preview_clock))
}

/// How a slot calls its category.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClockSlot {
    pub category_id: u64,
    #[serde(default)]
    pub method: Method,
    /// Off, the slot skips the separation rules (restrictions still apply).
    #[serde(default = "yes")]
    pub enforce_rules: bool,
    /// Windows this slot overrides, over its category's.
    #[serde(default)]
    pub separation: SeparationOverrides,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ClockView {
    pub id: u64,
    pub name: String,
    /// Carries on from its last slot across hours instead of starting over.
    pub looping: bool,
    pub slots: Vec<ClockSlot>,
    /// Hours of the week it is scheduled at.
    pub scheduled_hours: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClockInput {
    pub name: String,
    /// Off by default: the clock starts over at each hour.
    #[serde(default)]
    pub looping: bool,
    pub slots: Vec<ClockSlot>,
}

impl ClockInput {
    fn check(&self) -> ApiResult<String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 255 {
            return Err(ApiError::Invalid(
                "names are 1 to 255 characters".to_owned(),
            ));
        }
        if self.slots.len() > MAX_SLOTS {
            return Err(ApiError::Invalid(format!(
                "a clock has at most {MAX_SLOTS} slots"
            )));
        }
        Ok(name.to_owned())
    }
}

async fn clocks(state: &AppState, station: &Station, id: Option<u64>) -> ApiResult<Vec<ClockView>> {
    let rows: Vec<(u64, String, bool, i64)> = sqlx::query_as(
        "SELECT c.id, c.name, c.looping,
                (SELECT COUNT(*) FROM clock_schedule s WHERE s.clock_id = c.id)
         FROM clocks c WHERE c.station_id = ? AND (? IS NULL OR c.id = ?) ORDER BY c.name",
    )
    .bind(station.id())
    .bind(id)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let slots: Vec<(u64, u64, String, bool, Option<String>)> = sqlx::query_as(
        "SELECT s.clock_id, s.category_id, CAST(s.selection_method AS CHAR), s.enforce_rules,
                CAST(s.separation AS CHAR)
         FROM clock_slots s JOIN clocks c ON c.id = s.clock_id
         WHERE c.station_id = ? AND (? IS NULL OR s.clock_id = ?)
         ORDER BY s.clock_id, s.position",
    )
    .bind(station.id())
    .bind(id)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let mut by_clock: HashMap<u64, Vec<ClockSlot>> = HashMap::new();
    for (clock, category_id, method, enforce_rules, separation) in slots {
        by_clock.entry(clock).or_default().push(ClockSlot {
            category_id,
            method: Method::parse(&method).unwrap_or_default(),
            enforce_rules,
            separation: overrides(separation.as_deref()),
        });
    }
    Ok(rows
        .into_iter()
        .map(|(id, name, looping, scheduled_hours)| ClockView {
            id,
            name,
            looping,
            slots: by_clock.remove(&id).unwrap_or_default(),
            scheduled_hours,
        })
        .collect())
}

#[utoipa::path(get, path = "/clocks", tag = "rotation",
    responses((status = 200, body = Vec<ClockView>), (status = "4XX", body = ErrorBody)))]
async fn list_clocks(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Vec<ClockView>>> {
    clocks(&state, &station, None).await.map(Json)
}

#[utoipa::path(get, path = "/clocks/{id}", tag = "rotation",
    params(("id" = u64, Path)),
    responses((status = 200, body = ClockView), (status = "4XX", body = ErrorBody)))]
async fn get_clock(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<ClockView>> {
    clocks(&state, &station, Some(id))
        .await?
        .pop()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("clock"))
}

fn conflict(error: sqlx::Error) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict("another clock has this name".to_owned())
    } else {
        error.into()
    }
}

/// Refuses a slot calling a category the station does not use.
async fn check_slots(state: &AppState, station: &Station, slots: &[ClockSlot]) -> ApiResult<()> {
    let ids: std::collections::BTreeSet<u64> = slots.iter().map(|slot| slot.category_id).collect();
    for id in ids {
        crate::categories::check_offered(&state.pool, station.id(), id).await?;
    }
    Ok(())
}

async fn write_slots(
    tx: &mut sqlx::MySqlConnection,
    clock: u64,
    slots: &[ClockSlot],
) -> ApiResult<()> {
    sqlx::query("DELETE FROM clock_slots WHERE clock_id = ?")
        .bind(clock)
        .execute(&mut *tx)
        .await?;
    for (position, slot) in slots.iter().enumerate() {
        let separation = separation_json(&slot.separation)?;
        sqlx::query(
            "INSERT INTO clock_slots
                 (clock_id, position, category_id, selection_method, enforce_rules, separation)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(clock)
        .bind(u32::try_from(position).unwrap_or(u32::MAX))
        .bind(slot.category_id)
        .bind(slot.method.as_str())
        .bind(slot.enforce_rules)
        .bind(separation)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if is_still_used(&error) {
                ApiError::Invalid(format!("no category {}", slot.category_id))
            } else {
                error.into()
            }
        })?;
    }
    Ok(())
}

/// Operators and admins.
#[utoipa::path(post, path = "/clocks", tag = "rotation",
    request_body = ClockInput,
    responses((status = 201, body = ClockView), (status = "4XX", body = ErrorBody)))]
async fn create_clock(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(input): Json<ClockInput>,
) -> ApiResult<(StatusCode, Json<ClockView>)> {
    user.require(Role::Operator)?;
    let name = input.check()?;
    let mut tx = state.pool.begin().await?;
    let id = sqlx::query("INSERT INTO clocks (station_id, name, looping) VALUES (?, ?, ?)")
        .bind(station.id())
        .bind(name)
        .bind(input.looping)
        .execute(&mut *tx)
        .await
        .map_err(conflict)?
        .last_insert_id();
    check_slots(&state, &station, &input.slots).await?;
    write_slots(&mut tx, id, &input.slots).await?;
    tx.commit().await?;
    let Json(clock) = get_clock(user, State(state), station, Path(id)).await?;
    Ok((StatusCode::CREATED, Json(clock)))
}

/// Renames a clock and replaces its slots. Operators and admins.
#[utoipa::path(put, path = "/clocks/{id}", tag = "rotation",
    params(("id" = u64, Path)), request_body = ClockInput,
    responses((status = 200, body = ClockView), (status = "4XX", body = ErrorBody)))]
async fn update_clock(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(input): Json<ClockInput>,
) -> ApiResult<Json<ClockView>> {
    user.require(Role::Operator)?;
    let name = input.check()?;
    let mut tx = state.pool.begin().await?;
    let exists: Option<u64> =
        sqlx::query_scalar("SELECT id FROM clocks WHERE id = ? AND station_id = ? FOR UPDATE")
            .bind(id)
            .bind(station.id())
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Err(ApiError::not_found("clock"));
    }
    sqlx::query("UPDATE clocks SET name = ?, looping = ? WHERE id = ?")
        .bind(name)
        .bind(input.looping)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(conflict)?;
    check_slots(&state, &station, &input.slots).await?;
    write_slots(&mut tx, id, &input.slots).await?;
    tx.commit().await?;
    tracing::info!(user = %user.username, clock = id, slots = input.slots.len(), "clock changed");
    get_clock(user, State(state), station, Path(id)).await
}

/// Deletes a clock; the hours it was scheduled at rotate the station's library
/// until another clock is scheduled. Operators and admins.
#[utoipa::path(delete, path = "/clocks/{id}", tag = "rotation",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_clock(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let deleted = sqlx::query("DELETE FROM clocks WHERE id = ? AND station_id = ?")
        .bind(id)
        .bind(station.id())
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found("clock"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// A slot as it will sound.
#[derive(Debug, Serialize, ToSchema)]
pub struct SlotPreview {
    pub category_id: u64,
    pub category_name: Option<String>,
    pub color: Option<String>,
    /// Tracks of the category rotation may pick (playable).
    pub tracks: u32,
    /// Their average playable length; missing for an empty category.
    pub average_ms: Option<u32>,
    /// When the slot is expected to start, from the top of the hour.
    pub expected_start_ms: u64,
    /// Expected to start after the hour ends: it will not play (never for
    /// a looping clock).
    pub beyond_hour: bool,
    /// Windows the slot applies (rotation settings, category, slot).
    pub windows: heron_rotation::Windows,
    /// Times an hour of this clock calls the category.
    pub calls_per_hour: f64,
    /// Tracks the category needs so its tracks are not repeated within
    /// the track window without relaxation.
    pub needed_tracks: u32,
    /// The category has fewer tracks than it needs: expect relaxations.
    pub short: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ClockPreview {
    pub slots: Vec<SlotPreview>,
    /// Expected length of the whole clock.
    pub duration_ms: u64,
    /// How many times the clock runs in an hour: more than 1 when it is
    /// shorter than the hour; under 1 for a looping clock longer than it.
    pub runs_per_hour: f64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    /// The slots as edited, saved or not.
    pub slots: Vec<ClockSlot>,
    /// Whether the clock loops across hours.
    #[serde(default)]
    pub looping: bool,
}

/// A clock as it will sound, from its slots (saved or not): expected
/// starts and lengths, and the categories running short.
#[utoipa::path(post, path = "/clocks/preview", tag = "rotation",
    request_body = PreviewRequest,
    responses((status = 200, body = ClockPreview), (status = "4XX", body = ErrorBody)))]
async fn preview_clock(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(request): Json<PreviewRequest>,
) -> ApiResult<Json<ClockPreview>> {
    if request.slots.len() > MAX_SLOTS {
        return Err(ApiError::Invalid(format!(
            "a clock has at most {MAX_SLOTS} slots"
        )));
    }
    let library = rotation::load_library(&state.pool, station.id()).await?;
    let clock = heron_rotation::Clock {
        id: 0,
        name: String::new(),
        slots: request
            .slots
            .iter()
            .map(|slot| heron_rotation::Slot {
                category: slot.category_id,
                method: slot.method,
                enforce_rules: slot.enforce_rules,
                separation: slot.separation,
            })
            .collect(),
        looping: request.looping,
    };
    let clock = &clock;
    let colors: HashMap<u64, Option<String>> =
        sqlx::query_as::<_, (u64, Option<String>)>("SELECT id, color FROM categories")
            .fetch_all(&state.pool)
            .await?
            .into_iter()
            .collect();
    const HOUR_MS: u64 = 3_600_000;
    let mut at = 0_u64;
    let mut slots = Vec::with_capacity(clock.slots.len());
    for slot in &clock.slots {
        let category = library.categories.get(&slot.category);
        let lengths: Vec<u64> = category
            .into_iter()
            .flat_map(|c| &c.members)
            .filter_map(|m| library.tracks.get(&m.track))
            .map(|t| u64::from(t.length_ms))
            .collect();
        let average_ms =
            (!lengths.is_empty()).then(|| lengths.iter().sum::<u64>() / lengths.len() as u64);
        let windows = library
            .settings
            .separation
            .with(&category.map(|c| c.separation).unwrap_or_default())
            .with(&slot.separation);
        slots.push(SlotPreview {
            category_id: slot.category,
            category_name: category.map(|c| c.name.clone()),
            color: colors.get(&slot.category).cloned().flatten(),
            tracks: u32::try_from(lengths.len()).unwrap_or(u32::MAX),
            average_ms: average_ms.map(|ms| u32::try_from(ms).unwrap_or(u32::MAX)),
            expected_start_ms: at,
            beyond_hour: !clock.looping && at >= HOUR_MS,
            windows,
            calls_per_hour: 0.0,
            needed_tracks: 0,
            short: false,
        });
        // An empty category falls back: count an average track.
        at += average_ms.unwrap_or(210_000);
    }
    let duration_ms = at;
    let runs_per_hour = if duration_ms > 0 && (duration_ms < HOUR_MS || clock.looping) {
        HOUR_MS as f64 / duration_ms as f64
    } else {
        1.0
    };
    let mut calls: HashMap<u64, f64> = HashMap::new();
    for slot in slots.iter().filter(|s| !s.beyond_hour) {
        *calls.entry(slot.category_id).or_default() += runs_per_hour;
    }
    for (preview, slot) in slots.iter_mut().zip(&clock.slots) {
        preview.calls_per_hour = calls.get(&preview.category_id).copied().unwrap_or(0.0);
        let window_hours = f64::from(preview.windows.track_min) / 60.0;
        preview.needed_tracks = if slot.enforce_rules {
            (preview.calls_per_hour * window_hours).ceil() as u32 + 1
        } else {
            1
        };
        preview.short = preview.tracks < preview.needed_tracks;
    }
    Ok(Json(ClockPreview {
        slots,
        duration_ms,
        runs_per_hour,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_default_to_least_recently_played_with_rules() {
        let slot: ClockSlot = serde_json::from_str(r#"{"category_id": 3}"#).unwrap();
        assert_eq!(slot.method, Method::LeastRecentlyPlayed);
        assert!(slot.enforce_rules);
        assert!(slot.separation.is_empty());
        let input = ClockInput {
            name: " ".to_owned(),
            looping: false,
            slots: vec![],
        };
        assert!(input.check().is_err());
    }
}
