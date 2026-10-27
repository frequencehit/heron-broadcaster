// SPDX-License-Identifier: AGPL-3.0-or-later

//! Category types: what a category holds. Song, Jingle and Ad exist by
//! default; the station can rename them, add its own (Liner, Promo…) and
//! delete those no category uses. See `docs/rotation.md#category-types`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate, is_still_used};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_types, create_type))
        .routes(routes!(update_type, delete_type))
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct CategoryType {
    pub id: u64,
    pub name: String,
    /// Never two tracks of this type in a row: rotation passes a slot
    /// calling it over right after one.
    pub no_two_in_a_row: bool,
    /// Its tracks send metadata to the stream servers. Off (jingles, ads):
    /// the stream and the public now-playing keep the last song's, and
    /// webhooks send them only when they include the FX player.
    pub sends_metadata: bool,
    /// Categories of this type.
    pub categories: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryTypeInput {
    pub name: String,
    /// Kept as it is when missing (off for a new type).
    #[serde(default)]
    pub no_two_in_a_row: Option<bool>,
    /// Kept as it is when missing (on for a new type).
    #[serde(default)]
    pub sends_metadata: Option<bool>,
}

impl CategoryTypeInput {
    fn name(&self) -> ApiResult<String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(ApiError::Invalid("names are 1 to 64 characters".to_owned()));
        }
        Ok(name.to_owned())
    }
}

const SELECT: &str = "SELECT t.id, t.name, t.no_two_in_a_row, t.sends_metadata,
           (SELECT COUNT(*) FROM categories c WHERE c.type_id = t.id) AS categories
    FROM category_types t";

fn conflict(error: sqlx::Error) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict("another type has this name".to_owned())
    } else {
        error.into()
    }
}

async fn find(state: &AppState, id: u64) -> ApiResult<CategoryType> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = ?")))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("category type"))
}

/// Every category type, in the order they were created.
#[utoipa::path(get, path = "/category-types", tag = "categories",
    responses((status = 200, body = Vec<CategoryType>), (status = "4XX", body = ErrorBody)))]
async fn list_types(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<CategoryType>>> {
    let types = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} ORDER BY t.id")))
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(types))
}

/// Operators and admins.
#[utoipa::path(post, path = "/category-types", tag = "categories",
    request_body = CategoryTypeInput,
    responses((status = 201, body = CategoryType), (status = "4XX", body = ErrorBody)))]
async fn create_type(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<CategoryTypeInput>,
) -> ApiResult<(StatusCode, Json<CategoryType>)> {
    user.require(Role::Operator)?;
    let id = sqlx::query(
        "INSERT INTO category_types (name, no_two_in_a_row, sends_metadata) VALUES (?, ?, ?)",
    )
    .bind(input.name()?)
    .bind(input.no_two_in_a_row.unwrap_or(false))
    .bind(input.sends_metadata.unwrap_or(true))
    .execute(&state.pool)
    .await
    .map_err(conflict)?
    .last_insert_id();
    Ok((StatusCode::CREATED, Json(find(&state, id).await?)))
}

/// Renames a type, or changes its rules. Operators and admins.
#[utoipa::path(put, path = "/category-types/{id}", tag = "categories",
    params(("id" = u64, Path)), request_body = CategoryTypeInput,
    responses((status = 200, body = CategoryType), (status = "4XX", body = ErrorBody)))]
async fn update_type(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(input): Json<CategoryTypeInput>,
) -> ApiResult<Json<CategoryType>> {
    user.require(Role::Operator)?;
    let name = input.name()?;
    find(&state, id).await?;
    sqlx::query(
        "UPDATE category_types SET name = ?, no_two_in_a_row = COALESCE(?, no_two_in_a_row),
             sends_metadata = COALESCE(?, sends_metadata)
         WHERE id = ?",
    )
    .bind(name)
    .bind(input.no_two_in_a_row)
    .bind(input.sends_metadata)
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(conflict)?;
    Ok(Json(find(&state, id).await?))
}

/// Deletes a type no category uses. Operators and admins.
#[utoipa::path(delete, path = "/category-types/{id}", tag = "categories",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_type(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let deleted = sqlx::query("DELETE FROM category_types WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|error| {
            if is_still_used(&error) {
                ApiError::Conflict("categories still have this type".to_owned())
            } else {
                error.into()
            }
        })?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found("category type"));
    }
    Ok(StatusCode::NO_CONTENT)
}
