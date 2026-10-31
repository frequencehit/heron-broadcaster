// SPDX-License-Identifier: AGPL-3.0-or-later

//! Categories: groups of tracks the clockwheels call. A track can be in
//! several categories. A category can override the separation windows of
//! the rotation settings (a power rotation repeats its titles sooner).
//! Categories for the FX player (liners, power intros, voice tracks) are
//! what its picker opens on.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use heron_rotation::SeparationOverrides;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate, is_still_used};
use crate::stations::Station;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_categories, create_category))
        .routes(routes!(get_category, update_category, delete_category))
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Category {
    pub id: u64,
    pub name: String,
    /// `#rrggbb`.
    pub color: Option<String>,
    /// What it holds (`GET /category-types`); none when unset.
    pub type_id: Option<u64>,
    pub type_name: Option<String>,
    /// Liners, power intros, voice tracks: the FX player's picker opens on
    /// these categories. Clocks still call them like any other.
    pub fx_player: bool,
    pub tracks: i64,
    /// How long its tracks last together, in ms: what is played of each
    /// (cue-in to cue-out, overrides applied), or the file's length for a
    /// track not analyzed yet. Over the same tracks as `tracks`.
    pub total_ms: i64,
    /// Windows this category overrides; the others follow the rotation
    /// settings (clock slots can override them again).
    pub separation: SeparationOverrides,
    /// The stations it is used on, by ID.
    pub station_ids: Vec<u64>,
}

type CategoryRow = (
    u64,
    String,
    Option<String>,
    Option<u64>,
    Option<String>,
    bool,
    i64,
    i64,
    Option<String>,
    Option<String>,
);

fn category(
    (id, name, color, type_id, type_name, fx_player, tracks, total_ms, separation, stations): CategoryRow,
) -> Category {
    Category {
        id,
        name,
        color,
        type_id,
        type_name,
        fx_player,
        tracks,
        total_ms,
        separation: crate::rotation::overrides(separation.as_deref()),
        station_ids: stations
            .unwrap_or_default()
            .split(',')
            .filter_map(|id| id.parse().ok())
            .collect(),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryInput {
    pub name: String,
    /// `#rrggbb`, or none.
    pub color: Option<String>,
    /// A category type; none by default.
    #[serde(default)]
    pub type_id: Option<u64>,
    /// Not for the FX player by default.
    #[serde(default)]
    pub fx_player: bool,
    /// None overridden by default.
    #[serde(default)]
    pub separation: SeparationOverrides,
    /// The stations it is used on, at least one: the station it is made on
    /// when left out on creation, kept when left out on a change.
    #[serde(default)]
    pub station_ids: Option<Vec<u64>>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CategoriesQuery {
    /// Only the categories this station can use.
    pub station_id: Option<u64>,
}

/// Overrides as stored: `NULL` for none.
pub fn separation_json(overrides: &SeparationOverrides) -> ApiResult<Option<String>> {
    let windows = heron_rotation::Windows::default().with(overrides);
    windows.check().map_err(ApiError::Invalid)?;
    if overrides.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(overrides)
        .map(Some)
        .map_err(|error| ApiError::Internal(error.to_string()))
}

impl CategoryInput {
    fn check(&self) -> ApiResult<(String, Option<String>)> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 255 {
            return Err(ApiError::Invalid(
                "names are 1 to 255 characters".to_owned(),
            ));
        }
        Ok((name.to_owned(), check_color(self.color.as_deref())?))
    }
}

/// A color as stored: `#rrggbb`, or none.
pub fn check_color(color: Option<&str>) -> ApiResult<Option<String>> {
    match color.map(str::trim) {
        None | Some("") => Ok(None),
        Some(color)
            if color.len() == 7
                && color.starts_with('#')
                && color[1..].chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            Ok(Some(color.to_ascii_lowercase()))
        }
        Some(_) => Err(ApiError::Invalid("colors are written #rrggbb".to_owned())),
    }
}

/// `total_ms` follows `TrackPlayout::with_overrides`: the analyzed cue
/// points (the whole file when not analyzed or the Gap Killer is off), then
/// the cue overrides, the cue-out kept within the file.
const SELECT: &str = "SELECT c.id, c.name, c.color, c.type_id, ct.name, c.fx_player,
           (SELECT COUNT(*) FROM track_categories tc JOIN tracks t ON t.id = tc.track_id
            WHERE tc.category_id = c.id AND t.deleted_at IS NULL) AS tracks,
           (SELECT CAST(COALESCE(SUM(GREATEST(0,
                   CAST(LEAST(COALESCE(o.cue_out_ms, IF(a.cue_in_ms IS NULL OR a.cue_out_ms IS NULL OR o.gap_killer_enabled = FALSE,
                                                        GREATEST(COALESCE(t.duration_ms, 0), COALESCE(a.cue_out_ms, 0)),
                                                        a.cue_out_ms)),
                              GREATEST(COALESCE(t.duration_ms, 0), COALESCE(a.cue_out_ms, 0))) AS SIGNED)
                   - CAST(COALESCE(o.cue_in_ms, IF(a.cue_in_ms IS NULL OR a.cue_out_ms IS NULL OR o.gap_killer_enabled = FALSE,
                                                   0, a.cue_in_ms)) AS SIGNED))), 0) AS SIGNED)
            FROM track_categories tc JOIN tracks t ON t.id = tc.track_id
            LEFT JOIN track_analysis a ON a.track_id = t.id
            LEFT JOIN track_overrides o ON o.track_id = t.id
            WHERE tc.category_id = c.id AND t.deleted_at IS NULL) AS total_ms,
           CAST(c.separation AS CHAR),
           (SELECT CAST(GROUP_CONCAT(cs.station_id ORDER BY cs.station_id) AS CHAR)
            FROM category_stations cs WHERE cs.category_id = c.id)
    FROM categories c LEFT JOIN category_types ct ON ct.id = c.type_id";

fn conflict(error: sqlx::Error) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict("another category has this name".to_owned())
    } else if is_still_used(&error) {
        ApiError::Invalid("no such category type".to_owned())
    } else {
        error.into()
    }
}

#[utoipa::path(get, path = "/categories", tag = "categories", params(CategoriesQuery),
    responses((status = 200, body = Vec<Category>), (status = "4XX", body = ErrorBody)))]
async fn list_categories(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<CategoriesQuery>,
) -> ApiResult<Json<Vec<Category>>> {
    let rows: Vec<CategoryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT}
         WHERE ? IS NULL OR EXISTS (SELECT 1 FROM category_stations cs
                                    WHERE cs.category_id = c.id AND cs.station_id = ?)
         ORDER BY c.name"
    )))
    .bind(query.station_id)
    .bind(query.station_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(category).collect()))
}

#[utoipa::path(get, path = "/categories/{id}", tag = "categories",
    params(("id" = u64, Path)),
    responses((status = 200, body = Category), (status = "4XX", body = ErrorBody)))]
async fn get_category(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<Category>> {
    let row: Option<CategoryRow> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE c.id = ?")))
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    row.map(|row| Json(category(row)))
        .ok_or_else(|| ApiError::not_found("category"))
}

/// Operators and admins.
#[utoipa::path(post, path = "/categories", tag = "categories",
    request_body = CategoryInput,
    responses((status = 201, body = Category), (status = "4XX", body = ErrorBody)))]
async fn create_category(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(input): Json<CategoryInput>,
) -> ApiResult<(StatusCode, Json<Category>)> {
    user.require(Role::Operator)?;
    let (name, color) = input.check()?;
    let separation = separation_json(&input.separation)?;
    let station_ids = input
        .station_ids
        .clone()
        .unwrap_or_else(|| vec![station.id()]);
    check_stations(&state, &station_ids)?;
    let mut tx = state.pool.begin().await?;
    let id = sqlx::query(
        "INSERT INTO categories (name, color, type_id, fx_player, separation) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(color)
    .bind(input.type_id)
    .bind(input.fx_player)
    .bind(separation)
    .execute(&mut *tx)
    .await
    .map_err(conflict)?
    .last_insert_id();
    write_stations(&mut tx, id, &station_ids).await?;
    tx.commit().await?;
    state.stations.wake_all();
    let Json(category) = get_category(user, State(state), Path(id)).await?;
    Ok((StatusCode::CREATED, Json(category)))
}

/// Renames, recolors a category, changes its type, its separation, its
/// stations or whether the FX player offers it. Untagging a station that
/// uses it is refused, listing what uses it. Operators and admins.
#[utoipa::path(put, path = "/categories/{id}", tag = "categories",
    params(("id" = u64, Path)), request_body = CategoryInput,
    responses((status = 200, body = Category), (status = "4XX", body = ErrorBody)))]
async fn update_category(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(input): Json<CategoryInput>,
) -> ApiResult<Json<Category>> {
    user.require(Role::Operator)?;
    let (name, color) = input.check()?;
    let separation = separation_json(&input.separation)?;
    let before: Vec<u64> =
        sqlx::query_scalar("SELECT station_id FROM category_stations WHERE category_id = ?")
            .bind(id)
            .fetch_all(&state.pool)
            .await?;
    if let Some(station_ids) = &input.station_ids {
        check_stations(&state, station_ids)?;
        for removed in before.iter().filter(|s| !station_ids.contains(s)) {
            let uses = uses(&state.pool, *removed, id).await?;
            if !uses.is_empty() {
                let code = state
                    .stations
                    .get(*removed)
                    .map_or_else(|| removed.to_string(), |station| station.code());
                return Err(ApiError::Conflict(format!(
                    "{code} still uses this category: {}",
                    uses.join(", ")
                )));
            }
        }
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "UPDATE categories SET name = ?, color = ?, type_id = ?, fx_player = ?, separation = ?
         WHERE id = ?",
    )
    .bind(name)
    .bind(color)
    .bind(input.type_id)
    .bind(input.fx_player)
    .bind(separation)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(conflict)?;
    if let Some(station_ids) = &input.station_ids {
        write_stations(&mut tx, id, station_ids).await?;
    }
    tx.commit().await?;
    if input.station_ids.is_some() {
        state.stations.wake_all();
    }
    get_category(user, State(state), Path(id)).await
}

fn check_stations(state: &AppState, station_ids: &[u64]) -> ApiResult<()> {
    if station_ids.is_empty() {
        return Err(ApiError::Invalid(
            "a category is used on at least one station".to_owned(),
        ));
    }
    if let Some(id) = station_ids
        .iter()
        .find(|id| state.stations.get(**id).is_none())
    {
        return Err(ApiError::Invalid(format!("no station {id}")));
    }
    Ok(())
}

async fn write_stations(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    category_id: u64,
    station_ids: &[u64],
) -> ApiResult<()> {
    sqlx::query("DELETE FROM category_stations WHERE category_id = ?")
        .bind(category_id)
        .execute(&mut **tx)
        .await?;
    for station_id in station_ids {
        sqlx::query("INSERT IGNORE INTO category_stations (category_id, station_id) VALUES (?, ?)")
            .bind(category_id)
            .bind(station_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Refuses a category a station cannot use: one that does not exist or is
/// not tagged with it.
pub async fn check_offered(pool: &MySqlPool, station_id: u64, category_id: u64) -> ApiResult<()> {
    let exists: Option<bool> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM category_stations
                        WHERE category_id = c.id AND station_id = ?) <> 0
         FROM categories c WHERE c.id = ?",
    )
    .bind(station_id)
    .bind(category_id)
    .fetch_optional(pool)
    .await?;
    match exists {
        None => Err(ApiError::Invalid(format!("no category {category_id}"))),
        Some(false) => Err(ApiError::Invalid(format!(
            "category {category_id} is not used on this station"
        ))),
        Some(true) => Ok(()),
    }
}

/// What makes a station use a category: its clocks, the restrictions that
/// apply to it, its fallback category, its medley categories, the recipes
/// of its medleys.
pub async fn uses(pool: &MySqlPool, station_id: u64, category_id: u64) -> ApiResult<Vec<String>> {
    let mut uses = Vec::new();
    let clocks: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT c.name FROM clock_slots s JOIN clocks c ON c.id = s.clock_id
         WHERE c.station_id = ? AND s.category_id = ? ORDER BY c.name",
    )
    .bind(station_id)
    .bind(category_id)
    .fetch_all(pool)
    .await?;
    uses.extend(clocks.into_iter().map(|name| format!("clock {name}")));
    let restrictions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM restrictions r
         WHERE ((r.target = 'category' AND r.target_id = ?)
                OR (JSON_VALUE(r.rule, '$.kind') = 'not_after'
                    AND JSON_VALUE(r.rule, '$.category_id') = ?))
           AND (NOT EXISTS (SELECT 1 FROM restriction_stations rs WHERE rs.restriction_id = r.id)
                OR EXISTS (SELECT 1 FROM restriction_stations rs
                           WHERE rs.restriction_id = r.id AND rs.station_id = ?))",
    )
    .bind(category_id)
    .bind(category_id)
    .bind(station_id)
    .fetch_one(pool)
    .await?;
    if restrictions > 0 {
        uses.push(format!("{restrictions} restriction(s)"));
    }
    let rotation: heron_rotation::RotationSettings = crate::settings::load_for(
        pool,
        station_id,
        heron_rotation::RotationSettings::SETTINGS_NAME,
    )
    .await?;
    if rotation.fallback_category_id == Some(category_id) {
        uses.push("the fallback category".to_owned());
    }
    if rotation.kept_out_category_ids.contains(&category_id) {
        uses.push("the categories its rotation keeps out".to_owned());
    }
    let medleys = crate::medleys::categories_of(pool, station_id).await?;
    if medleys.ids().any(|id| id == category_id) {
        uses.push("the medley settings".to_owned());
    }
    let recipes: Vec<(u64, Option<String>)> = sqlx::query_as(
        "SELECT m.id, m.name FROM medleys m JOIN medley_stations ms ON ms.medley_id = m.id
         WHERE ms.station_id = ? AND JSON_CONTAINS(m.recipe, ?, '$.category_ids')
         ORDER BY m.id",
    )
    .bind(station_id)
    .bind(category_id.to_string())
    .fetch_all(pool)
    .await?;
    uses.extend(recipes.into_iter().map(|(id, name)| match name {
        Some(name) => format!("medley {name}"),
        None => format!("medley {id}"),
    }));
    Ok(uses)
}

/// Deletes a category and its restrictions; its tracks stay in the
/// library. Refused while a clock uses it. Operators and admins.
#[utoipa::path(delete, path = "/categories/{id}", tag = "categories",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_category(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM restrictions WHERE target = 'category' AND target_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let deleted = sqlx::query("DELETE FROM categories WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if is_still_used(&error) {
                ApiError::Conflict("clocks still use this category".to_owned())
            } else {
                error.into()
            }
        })?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found("category"));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_names_and_colors() {
        let input = |name: &str, color: Option<&str>| CategoryInput {
            name: name.to_owned(),
            color: color.map(str::to_owned),
            type_id: None,
            fx_player: false,
            separation: SeparationOverrides::default(),
            station_ids: None,
        };
        assert_eq!(
            input(" Gold ", Some("#FFAA00")).check().unwrap(),
            ("Gold".to_owned(), Some("#ffaa00".to_owned()))
        );
        assert!(input("", None).check().is_err());
        assert!(input("Gold", Some("orange")).check().is_err());
        assert_eq!(input("Gold", Some("")).check().unwrap().1, None);
    }

    #[test]
    fn stores_only_overridden_windows() {
        assert_eq!(
            separation_json(&SeparationOverrides::default()).unwrap(),
            None
        );
        let overrides = SeparationOverrides {
            track_min: Some(90),
            ..SeparationOverrides::default()
        };
        let json = separation_json(&overrides).unwrap().unwrap();
        assert_eq!(crate::rotation::overrides(Some(&json)), overrides);
        let too_long = SeparationOverrides {
            title_min: Some(20_000),
            ..SeparationOverrides::default()
        };
        assert!(separation_json(&too_long).is_err());
    }
}
