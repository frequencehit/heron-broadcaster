// SPDX-License-Identifier: AGPL-3.0-or-later

//! Actions on a selection of tracks, given by IDs or by the filters of the
//! track list (every matching track, on every page): labels, categories,
//! mood, sound code, tempo and play ratio, enabled or not, and the files'
//! tags: written by
//! `heron-librarian`, as a library task, or kept in Heron's database when
//! the library setting `write_tags` is off.

use axum::Json;
use axum::extract::{Query, State};
use heron_core::LibrarySettings;
use heron_core::library::{FieldValues, TagField, TaskRequest};
use serde::{Deserialize, Serialize};
use sqlx::{MySql, QueryBuilder, Transaction};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::librarian::{self, LibraryTask};
use crate::settings::{self, ProgrammingSettings, some_or_null};
use crate::tracks::{ListContext, TrackQuery};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(apply_labels))
        .routes(routes!(apply_categories, track_categories))
        .routes(routes!(apply_programming))
        .routes(routes!(edit_tags))
        .routes(routes!(override_tags))
        .routes(routes!(clear_tag_overrides))
        .routes(routes!(set_enabled))
}

/// Tracks in one selection, at most.
pub const MAX_SELECTION: usize = 100_000;
/// Files in one tag edit, at most.
pub const MAX_TAG_EDIT: usize = 10_000;
/// IDs per statement.
const CHUNK: usize = 1_000;

/// Some tracks: these IDs, or every track matching the filters.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackSelection {
    pub track_ids: Option<Vec<u64>>,
    /// The filters of `GET /tracks` (sorting and pages aside).
    pub filters: Option<TrackQuery>,
}

impl TrackSelection {
    /// The selected tracks that exist, at most `max`. Filters on a station
    /// the user does not see answer 404.
    pub async fn resolve(
        &self,
        state: &AppState,
        user: &CurrentUser,
        max: usize,
    ) -> ApiResult<Vec<u64>> {
        match (&self.track_ids, &self.filters) {
            (Some(ids), None) => {
                if ids.len() > max {
                    return Err(ApiError::Invalid(format!(
                        "the selection has more than {max} tracks"
                    )));
                }
                let mut existing = Vec::with_capacity(ids.len());
                for chunk in ids.chunks(CHUNK) {
                    let mut query = QueryBuilder::new("SELECT id FROM tracks WHERE id IN (");
                    let mut separated = query.separated(", ");
                    for id in chunk {
                        separated.push_bind(*id);
                    }
                    query.push(")");
                    existing.extend(
                        query
                            .build_query_scalar::<u64>()
                            .fetch_all(&state.pool)
                            .await?,
                    );
                }
                existing.sort_unstable();
                existing.dedup();
                Ok(existing)
            }
            (None, Some(filters)) => {
                filters.require_stations(&state.pool, user).await?;
                ListContext::load(&state.pool)
                    .await?
                    .ids(&state.pool, filters, max)
                    .await
            }
            _ => Err(ApiError::BadRequest(
                "select tracks with either track_ids or filters".to_owned(),
            )),
        }
    }
}

/// What an action on a selection did.
#[derive(Debug, Serialize, ToSchema)]
pub struct SelectionResult {
    /// Tracks selected.
    pub tracks: u64,
    /// Changes made (a label added to a track counts once).
    pub changed: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EnabledChange {
    pub selection: TrackSelection,
    /// Off: the tracks do not play at all (rotation, clocks, medleys,
    /// queued by hand), and leave the queue.
    pub enabled: bool,
    /// When enabling: the play ratio of every selected track, in tenths, 1
    /// to 10. Missing: each keeps its own.
    pub play_ratio: Option<u8>,
    /// Puts their play ratio counters back to 10 on every station.
    #[serde(default)]
    pub reset_counters: bool,
}

/// Whether a change of `enabled` and `play_ratio` enables the tracks,
/// disables them, or leaves them: a ratio of 0 disables.
pub fn check_ratio(enabled: Option<bool>, play_ratio: Option<u8>) -> ApiResult<Option<bool>> {
    match (enabled, play_ratio) {
        (_, Some(ratio)) if ratio > 10 => Err(ApiError::Invalid(
            "play_ratio is 0 to 10 (tenths)".to_owned(),
        )),
        (Some(true), Some(0)) => Err(ApiError::Invalid(
            "a play ratio of 0 disables the track".to_owned(),
        )),
        (_, Some(0)) => Ok(Some(false)),
        (enabled, _) => Ok(enabled),
    }
}

/// Sets the play ratio (tenths, 1 to 10) of `ids`; answers how many changed.
pub async fn set_play_ratio(
    tx: &mut Transaction<'_, MySql>,
    ids: &[u64],
    ratio: u8,
) -> ApiResult<u64> {
    let mut changed = 0;
    for chunk in ids.chunks(CHUNK) {
        let mut query = QueryBuilder::new("INSERT INTO track_programming (track_id, play_ratio) ");
        query.push_values(chunk, |mut row, id| {
            row.push_bind(*id).push_bind(ratio);
        });
        query.push(" ON DUPLICATE KEY UPDATE play_ratio = VALUES(play_ratio)");
        let result = query.build().execute(&mut **tx).await?;
        // 1 a row added, 2 a row changed, 0 unchanged.
        changed += result.rows_affected().min(chunk.len() as u64);
    }
    Ok(changed)
}

/// Puts the play ratio counters of `ids` back to 10 on every station.
pub async fn reset_counters(tx: &mut Transaction<'_, MySql>, ids: &[u64]) -> ApiResult<()> {
    for chunk in ids.chunks(CHUNK) {
        let mut query = QueryBuilder::new("DELETE FROM track_ratio_counters WHERE track_id IN (");
        let mut separated = query.separated(", ");
        for id in chunk {
            separated.push_bind(*id);
        }
        query.push(")");
        query.build().execute(&mut **tx).await?;
    }
    Ok(())
}

/// Sets `enabled` on `ids`; answers how many changed.
async fn set_enabled_on(
    tx: &mut Transaction<'_, MySql>,
    ids: &[u64],
    enabled: bool,
) -> ApiResult<u64> {
    let mut changed = 0;
    for chunk in ids.chunks(CHUNK) {
        let mut query = QueryBuilder::new("UPDATE tracks SET enabled = ");
        query
            .push_bind(enabled)
            .push(" WHERE enabled <> ")
            .push_bind(enabled)
            .push(" AND id IN (");
        let mut separated = query.separated(", ");
        for id in chunk {
            separated.push_bind(*id);
        }
        query.push(")");
        changed += query.build().execute(&mut **tx).await?.rows_affected();
    }
    Ok(changed)
}

/// Disables or enables every selected track, enabled ones at the play ratio
/// given. Disabled tracks leave the queue. Operators and admins.
#[utoipa::path(post, path = "/tracks/enabled", tag = "library",
    request_body = EnabledChange,
    responses((status = 200, body = SelectionResult), (status = "4XX", body = ErrorBody)))]
async fn set_enabled(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(change): Json<EnabledChange>,
) -> ApiResult<Json<SelectionResult>> {
    user.require(Role::Operator)?;
    if let Some(ratio) = change.play_ratio
        && !(change.enabled && (1..=10).contains(&ratio))
    {
        return Err(ApiError::Invalid(format!(
            "play_ratio {ratio}: enabled tracks get a ratio of 1 to 10 (tenths)"
        )));
    }
    let ids = change
        .selection
        .resolve(&state, &user, MAX_SELECTION)
        .await?;
    let mut tx = state.pool.begin().await?;
    let mut changed = set_enabled_on(&mut tx, &ids, change.enabled).await?;
    if let Some(ratio) = change.play_ratio {
        changed = changed.max(set_play_ratio(&mut tx, &ids, ratio).await?);
    }
    if change.reset_counters {
        reset_counters(&mut tx, &ids).await?;
    }
    tx.commit().await?;
    tracing::info!(user = %user.username, tracks = ids.len(), changed, enabled = change.enabled,
        "tracks enabled or disabled");
    Ok(Json(SelectionResult {
        tracks: ids.len() as u64,
        changed,
    }))
}

/// Refuses labels that do not exist.
pub async fn check_labels(pool: &sqlx::MySqlPool, add: &[u64], remove: &[u64]) -> ApiResult<()> {
    for &id in add.iter().chain(remove) {
        let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM labels WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        if exists.is_none() {
            return Err(ApiError::Invalid(format!("no label {id}")));
        }
    }
    Ok(())
}

/// Refuses categories that do not exist, and weights that are not above 0.
pub async fn check_categories(
    pool: &sqlx::MySqlPool,
    add: &[CategoryShare],
    remove: &[u64],
) -> ApiResult<()> {
    if add
        .iter()
        .any(|share| !share.weight.is_finite() || share.weight <= 0.0)
    {
        return Err(ApiError::Invalid("weights must be above 0".to_owned()));
    }
    for id in add
        .iter()
        .map(|share| share.category_id)
        .chain(remove.iter().copied())
    {
        let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM categories WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        if exists.is_none() {
            return Err(ApiError::Invalid(format!("no category {id}")));
        }
    }
    Ok(())
}

/// Refuses a mood or a sound code that is not on the station's scale.
pub async fn check_scales(
    pool: &sqlx::MySqlPool,
    mood: &Option<Option<String>>,
    sound_code: &Option<Option<String>>,
) -> ApiResult<()> {
    let settings: ProgrammingSettings =
        settings::load(pool, ProgrammingSettings::SETTINGS_NAME).await?;
    let on_scale = |what: &str, value: &Option<Option<String>>, scale: &[String]| match value {
        Some(Some(value)) if !scale.contains(value) => Err(ApiError::Invalid(format!(
            "{value:?} is not on the {what} scale"
        ))),
        _ => Ok(()),
    };
    on_scale("mood", mood, &settings.moods)?;
    on_scale("sound code", sound_code, &settings.sound_codes)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelsChange {
    pub selection: TrackSelection,
    /// Labels added to every selected track.
    #[serde(default)]
    pub add: Vec<u64>,
    /// Labels removed from every selected track.
    #[serde(default)]
    pub remove: Vec<u64>,
}

/// Adds labels to, and removes labels from, every selected track.
/// Operators and admins.
#[utoipa::path(post, path = "/tracks/labels", tag = "labels",
    request_body = LabelsChange,
    responses((status = 200, body = SelectionResult), (status = "4XX", body = ErrorBody)))]
async fn apply_labels(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(change): Json<LabelsChange>,
) -> ApiResult<Json<SelectionResult>> {
    user.require(Role::Operator)?;
    if change.add.is_empty() && change.remove.is_empty() {
        return Err(ApiError::Invalid("no label to add or remove".to_owned()));
    }
    if change.add.iter().any(|id| change.remove.contains(id)) {
        return Err(ApiError::Invalid(
            "a label cannot be added and removed at once".to_owned(),
        ));
    }
    check_labels(&state.pool, &change.add, &change.remove).await?;
    let ids = change
        .selection
        .resolve(&state, &user, MAX_SELECTION)
        .await?;
    let mut tx = state.pool.begin().await?;
    let mut changed = 0;
    for chunk in ids.chunks(CHUNK) {
        for &label in &change.add {
            let mut query =
                QueryBuilder::new("INSERT IGNORE INTO track_labels (track_id, label_id) ");
            query.push_values(chunk, |mut row, id| {
                row.push_bind(*id).push_bind(label);
            });
            changed += query.build().execute(&mut *tx).await?.rows_affected();
        }
        if !change.remove.is_empty() {
            let mut query = QueryBuilder::new("DELETE FROM track_labels WHERE label_id IN (");
            let mut separated = query.separated(", ");
            for label in &change.remove {
                separated.push_bind(*label);
            }
            query.push(") AND track_id IN (");
            let mut separated = query.separated(", ");
            for id in chunk {
                separated.push_bind(*id);
            }
            query.push(")");
            changed += query.build().execute(&mut *tx).await?.rows_affected();
        }
    }
    tx.commit().await?;
    tracing::info!(user = %user.username, tracks = ids.len(), changed, "labels changed");
    Ok(Json(SelectionResult {
        tracks: ids.len() as u64,
        changed,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryShare {
    pub category_id: u64,
    /// Relative chance of being picked within the category, 1 by default.
    #[serde(default = "one")]
    pub weight: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoriesChange {
    pub selection: TrackSelection,
    /// Categories every selected track is put in (with this weight, also
    /// for tracks already in them).
    #[serde(default)]
    pub add: Vec<CategoryShare>,
    /// Categories every selected track is taken out of.
    #[serde(default)]
    pub remove: Vec<u64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TrackCategoriesQuery {
    /// Track IDs, comma-separated: at most 200.
    pub ids: String,
}

/// A category, as the decks and the queue show it.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct CategoryRef {
    pub id: u64,
    pub name: String,
    /// `#rrggbb`.
    pub color: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct TrackCategories {
    pub track_id: u64,
    /// By name; empty for none.
    pub categories: Vec<CategoryRef>,
}

/// Each track's categories, for up to 200 tracks: what the decks and the
/// queue show. Unknown tracks are left out.
#[utoipa::path(get, path = "/tracks/categories", tag = "categories", params(TrackCategoriesQuery),
    responses((status = 200, body = Vec<TrackCategories>), (status = "4XX", body = ErrorBody)))]
async fn track_categories(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<TrackCategoriesQuery>,
) -> ApiResult<Json<Vec<TrackCategories>>> {
    let mut ids: Vec<u64> = query
        .ids
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| {
            id.parse()
                .map_err(|_| ApiError::BadRequest(format!("ids: {id:?} is not an ID")))
        })
        .collect::<ApiResult<_>>()?;
    ids.sort_unstable();
    ids.dedup();
    if ids.len() > 200 {
        return Err(ApiError::BadRequest("ids: at most 200 tracks".to_owned()));
    }
    if ids.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let mut sql = QueryBuilder::new(
        "SELECT t.id, c.id, c.name, c.color FROM tracks t
         LEFT JOIN track_categories tc ON tc.track_id = t.id
         LEFT JOIN categories c ON c.id = tc.category_id
         WHERE t.id IN (",
    );
    let mut separated = sql.separated(", ");
    for id in &ids {
        separated.push_bind(*id);
    }
    sql.push(") ORDER BY t.id, c.name");
    // Track, category, name, color.
    type Row = (u64, Option<u64>, Option<String>, Option<String>);
    let rows: Vec<Row> = sql.build_query_as().fetch_all(&state.pool).await?;
    let mut result: Vec<TrackCategories> = Vec::new();
    for (track_id, category_id, name, color) in rows {
        if result.last().is_none_or(|last| last.track_id != track_id) {
            result.push(TrackCategories {
                track_id,
                categories: Vec::new(),
            });
        }
        if let (Some(id), Some(name), Some(last)) = (category_id, name, result.last_mut()) {
            last.categories.push(CategoryRef { id, name, color });
        }
    }
    Ok(Json(result))
}

/// Puts every selected track in categories, or takes them out. Filling a
/// category from a label: select with `filters.labels_any`. Operators and
/// admins.
#[utoipa::path(post, path = "/tracks/categories", tag = "categories",
    request_body = CategoriesChange,
    responses((status = 200, body = SelectionResult), (status = "4XX", body = ErrorBody)))]
async fn apply_categories(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(change): Json<CategoriesChange>,
) -> ApiResult<Json<SelectionResult>> {
    user.require(Role::Operator)?;
    if change.add.is_empty() && change.remove.is_empty() {
        return Err(ApiError::Invalid("no category to add or remove".to_owned()));
    }
    check_categories(&state.pool, &change.add, &change.remove).await?;
    let ids = change
        .selection
        .resolve(&state, &user, MAX_SELECTION)
        .await?;
    let mut tx = state.pool.begin().await?;
    let mut changed = 0;
    for chunk in ids.chunks(CHUNK) {
        for share in &change.add {
            let mut query =
                QueryBuilder::new("INSERT INTO track_categories (track_id, category_id, weight) ");
            query.push_values(chunk, |mut row, id| {
                row.push_bind(*id)
                    .push_bind(share.category_id)
                    .push_bind(share.weight);
            });
            query.push(" ON DUPLICATE KEY UPDATE weight = VALUES(weight)");
            changed += query.build().execute(&mut *tx).await?.rows_affected();
        }
        if !change.remove.is_empty() {
            let mut query =
                QueryBuilder::new("DELETE FROM track_categories WHERE category_id IN (");
            let mut separated = query.separated(", ");
            for category in &change.remove {
                separated.push_bind(*category);
            }
            query.push(") AND track_id IN (");
            let mut separated = query.separated(", ");
            for id in chunk {
                separated.push_bind(*id);
            }
            query.push(")");
            changed += query.build().execute(&mut *tx).await?.rows_affected();
        }
    }
    tx.commit().await?;
    tracing::info!(user = %user.username, tracks = ids.len(), changed, "categories changed");
    Ok(Json(SelectionResult {
        tracks: ids.len() as u64,
        changed,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProgrammingChange {
    pub selection: TrackSelection,
    /// A value of the mood scale, or `null` to clear it. Missing: kept.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<String>)]
    pub mood: Option<Option<String>>,
    /// A value of the sound code scale, or `null` to clear it.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<String>)]
    pub sound_code: Option<Option<String>>,
    /// The tempo, replacing the measured one; `null` goes back to it.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<f64>)]
    pub bpm: Option<Option<f64>>,
    /// The play ratio, in tenths, 1 to 10; 0 disables the tracks, which keep
    /// their ratio.
    pub play_ratio: Option<u8>,
}

/// Sets the mood, sound code, tempo or play ratio of every selected track.
/// Operators and admins.
#[utoipa::path(post, path = "/tracks/programming", tag = "library",
    request_body = ProgrammingChange,
    responses((status = 200, body = SelectionResult), (status = "4XX", body = ErrorBody)))]
async fn apply_programming(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(change): Json<ProgrammingChange>,
) -> ApiResult<Json<SelectionResult>> {
    user.require(Role::Operator)?;
    check_scales(&state.pool, &change.mood, &change.sound_code).await?;
    let bpm = match change.bpm {
        Some(Some(bpm)) => {
            let bpm = TagField::Bpm
                .normalize(&bpm.to_string())
                .map_err(ApiError::Invalid)?;
            Some(Some(
                bpm.parse::<f64>()
                    .map_err(|error| ApiError::Internal(error.to_string()))?,
            ))
        }
        other => other,
    };
    let mut columns: Vec<(&str, Option<String>, Option<f64>)> = Vec::new();
    if let Some(mood) = &change.mood {
        columns.push(("mood", mood.clone(), None));
    }
    if let Some(sound_code) = &change.sound_code {
        columns.push(("sound_code", sound_code.clone(), None));
    }
    if let Some(bpm) = bpm {
        columns.push(("bpm", None, bpm));
    }
    let disable = check_ratio(None, change.play_ratio)? == Some(false);
    if columns.is_empty() && change.play_ratio.is_none() {
        return Err(ApiError::Invalid("nothing to set".to_owned()));
    }
    let ids = change
        .selection
        .resolve(&state, &user, MAX_SELECTION)
        .await?;
    let mut tx = state.pool.begin().await?;
    let mut changed = 0;
    match change.play_ratio {
        Some(_) if disable => changed += set_enabled_on(&mut tx, &ids, false).await?,
        Some(ratio) => changed += set_play_ratio(&mut tx, &ids, ratio).await?,
        None => {}
    }
    for chunk in ids.chunks(CHUNK) {
        for (column, text, number) in &columns {
            let mut query = QueryBuilder::new(format!(
                "INSERT INTO track_programming (track_id, {column}) "
            ));
            query.push_values(chunk, |mut row, id| {
                row.push_bind(*id);
                if *column == "bpm" {
                    row.push_bind(*number);
                } else {
                    row.push_bind(text.clone());
                }
            });
            query.push(format!(
                " ON DUPLICATE KEY UPDATE {column} = VALUES({column})"
            ));
            let result = query.build().execute(&mut *tx).await?;
            // 1 a row added, 2 a row changed, 0 unchanged.
            changed += result.rows_affected().min(chunk.len() as u64);
        }
    }
    tx.commit().await?;
    tracing::info!(user = %user.username, tracks = ids.len(), "programming details set");
    Ok(Json(SelectionResult {
        tracks: ids.len() as u64,
        changed,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TagEdit {
    pub selection: TrackSelection,
    /// Fields set to the same value on every track; `null` removes the
    /// field. Fields left out keep each track's value.
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, Option<String>>)]
    pub set: FieldValues,
    /// Programming details written from each track's own values (ISRC,
    /// record label, BPM, mood); tracks without the value keep the file's.
    #[serde(default)]
    pub from_details: Vec<TagField>,
}

/// Writes tags into the files of the selected tracks (at most 10,000), as
/// one batch that can be undone: queues a library task for
/// heron-librarian. Files are not moved. Operators and admins.
#[utoipa::path(post, path = "/tracks/tags", tag = "library",
    request_body = TagEdit,
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn edit_tags(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(edit): Json<TagEdit>,
) -> ApiResult<(axum::http::StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let library: LibrarySettings =
        settings::load(&state.pool, LibrarySettings::SETTINGS_NAME).await?;
    if !library.write_tags {
        return Err(ApiError::Conflict(
            "tags are kept in Heron's database (the library setting write_tags is off): \
             POST /tracks/tag-overrides"
                .to_owned(),
        ));
    }
    let set = normalize_set(&edit.set)?;
    if let Some(field) = edit.from_details.iter().find(|field| !field.is_detail()) {
        return Err(ApiError::Invalid(format!(
            "{field:?} is not a programming detail"
        )));
    }
    if set.is_empty() && edit.from_details.is_empty() {
        return Err(ApiError::Invalid("no field to write".to_owned()));
    }
    if edit
        .from_details
        .iter()
        .any(|field| set.contains_key(field))
    {
        return Err(ApiError::Invalid(
            "a field is set and written from the details at once".to_owned(),
        ));
    }
    let track_ids = edit.selection.resolve(&state, &user, MAX_TAG_EDIT).await?;
    if track_ids.is_empty() {
        return Err(ApiError::Invalid("no track selected".to_owned()));
    }
    let request = TaskRequest::EditTags {
        track_ids,
        set,
        from_details: edit.from_details,
    };
    let task = librarian::ask(&state, &user, request).await?;
    Ok((axum::http::StatusCode::ACCEPTED, Json(task)))
}

/// Checks the values of `set` and puts them in the form they are kept in.
fn normalize_set(set: &FieldValues) -> ApiResult<FieldValues> {
    let mut normalized = FieldValues::new();
    for (field, value) in set {
        let value = match value {
            None => None,
            Some(value) => Some(
                field
                    .normalize(value)
                    .map_err(|error| ApiError::Invalid(format!("{field:?}: {error}")))?,
            ),
        };
        normalized.insert(*field, value);
    }
    Ok(normalized)
}

/// The `tracks` column of a tag field; `None` for programming details.
fn column(field: TagField) -> Option<&'static str> {
    Some(match field {
        TagField::Title => "title",
        TagField::Artist => "artist",
        TagField::Album => "album",
        TagField::AlbumArtist => "album_artist",
        TagField::Genre => "genre",
        TagField::Year => "year",
        TagField::Track => "track_number",
        TagField::Disc => "disc_number",
        TagField::Isrc | TagField::RecordLabel | TagField::Bpm | TagField::Mood => return None,
    })
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TagOverrides {
    pub selection: TrackSelection,
    /// Fields set to the same value on every track; `null` removes the
    /// field. Fields left out keep each track's value. Title, artist,
    /// album, album artist, year, genre, track and disc only.
    #[schema(value_type = std::collections::HashMap<String, Option<String>>)]
    pub set: FieldValues,
}

/// Edits the tags of the selected tracks in Heron's database only, when
/// the library setting `write_tags` is off: the files are not changed.
/// The values are shown instead of the files' tags, and survive a rescan,
/// until they are cleared. Operators and admins.
#[utoipa::path(post, path = "/tracks/tag-overrides", tag = "library",
    request_body = TagOverrides,
    responses((status = 200, body = SelectionResult), (status = "4XX", body = ErrorBody)))]
async fn override_tags(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(edit): Json<TagOverrides>,
) -> ApiResult<Json<SelectionResult>> {
    user.require(Role::Operator)?;
    let library: LibrarySettings =
        settings::load(&state.pool, LibrarySettings::SETTINGS_NAME).await?;
    if library.write_tags {
        return Err(ApiError::Conflict(
            "tags are written into the files (the library setting write_tags is on): \
             POST /tracks/tags"
                .to_owned(),
        ));
    }
    let set = normalize_set(&edit.set)?;
    if let Some(field) = set.keys().find(|field| column(**field).is_none()) {
        return Err(ApiError::Invalid(format!(
            "{field:?} is a programming detail, kept in Heron already"
        )));
    }
    if set.is_empty() {
        return Err(ApiError::Invalid("no field to set".to_owned()));
    }
    let ids = edit.selection.resolve(&state, &user, MAX_SELECTION).await?;
    let mut tx = state.pool.begin().await?;
    let mut changed = 0;
    for chunk in ids.chunks(CHUNK) {
        for (field, value) in &set {
            let mut query = QueryBuilder::new(
                "INSERT INTO track_tag_overrides (track_id, field, value, updated_by) ",
            );
            query.push_values(chunk, |mut row, id| {
                row.push_bind(*id)
                    .push_bind(field.as_str())
                    .push_bind(value.as_deref())
                    .push_bind(user.id);
            });
            query.push(
                " ON DUPLICATE KEY UPDATE updated_by = IF(value <=> VALUES(value), updated_by, \
                 VALUES(updated_by)), value = VALUES(value)",
            );
            // 1 a row added, 2 a row changed, 0 unchanged.
            let result = query.build().execute(&mut *tx).await?;
            changed += result.rows_affected().min(chunk.len() as u64);

            // What the library shows, until the scanner reads the file again
            // (and applies the overrides on top).
            let column = column(*field).unwrap_or_default();
            let mut query = QueryBuilder::new(format!("UPDATE tracks SET {column} = "));
            match field {
                TagField::Year | TagField::Track | TagField::Disc => {
                    query.push_bind(value.as_deref().and_then(|value| value.parse::<u16>().ok()))
                }
                _ => query.push_bind(value.as_deref()),
            };
            query.push(" WHERE id IN (");
            let mut separated = query.separated(", ");
            for id in chunk {
                separated.push_bind(*id);
            }
            query.push(")");
            query.build().execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    tracing::info!(user = %user.username, tracks = ids.len(), changed, "tags kept in the database");
    Ok(Json(SelectionResult {
        tracks: ids.len() as u64,
        changed,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TagOverridesClear {
    pub selection: TrackSelection,
    /// The fields to give back to the files; every field when empty.
    #[serde(default)]
    pub fields: Vec<TagField>,
}

/// Drops the tags kept in Heron's database for the selected tracks (at most
/// 10,000): they show their files' tags again. Works whatever `write_tags`
/// says. Operators and admins.
#[utoipa::path(post, path = "/tracks/tag-overrides/clear", tag = "library",
    request_body = TagOverridesClear,
    responses((status = 200, body = SelectionResult), (status = "4XX", body = ErrorBody)))]
async fn clear_tag_overrides(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(clear): Json<TagOverridesClear>,
) -> ApiResult<Json<SelectionResult>> {
    user.require(Role::Operator)?;
    let ids = clear.selection.resolve(&state, &user, MAX_TAG_EDIT).await?;
    let mut changed = 0;
    for chunk in ids.chunks(CHUNK) {
        let mut query = QueryBuilder::new(
            "SELECT DISTINCT o.track_id, t.path FROM track_tag_overrides o \
             JOIN tracks t ON t.id = o.track_id WHERE o.track_id IN (",
        );
        let mut separated = query.separated(", ");
        for id in chunk {
            separated.push_bind(*id);
        }
        query.push(")");
        push_fields(&mut query, &clear.fields);
        let tracks: Vec<(u64, String)> = query.build_query_as().fetch_all(&state.pool).await?;
        if tracks.is_empty() {
            continue;
        }

        let mut query = QueryBuilder::new("DELETE FROM track_tag_overrides WHERE track_id IN (");
        let mut separated = query.separated(", ");
        for (id, _) in &tracks {
            separated.push_bind(*id);
        }
        query.push(")");
        push_fields(&mut query, &clear.fields);
        changed += query.build().execute(&state.pool).await?.rows_affected();

        // The files' tags, with the overrides left on top.
        let root = state.library.root().to_path_buf();
        let read = tokio::task::spawn_blocking(move || {
            tracks
                .into_iter()
                .map(|(id, path)| {
                    let absolute = heron_core::LibraryPath::from_stored(path).resolve(&root);
                    let read = heron_library::tags::read(&absolute);
                    (id, absolute, read)
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
        let mut connection = state.pool.acquire().await?;
        for (id, absolute, read) in read {
            match read {
                Ok(read) => {
                    heron_library::details::save(
                        &mut connection,
                        state.library.covers(),
                        id,
                        &read,
                        Some(&absolute),
                    )
                    .await?
                }
                Err(error) => {
                    tracing::warn!(track = id, %error, "cannot read tags, read at the next scan");
                    sqlx::query("UPDATE tracks SET details_version = 0 WHERE id = ?")
                        .bind(id)
                        .execute(&mut *connection)
                        .await?;
                }
            }
        }
    }
    tracing::info!(user = %user.username, tracks = ids.len(), changed, "tag overrides cleared");
    Ok(Json(SelectionResult {
        tracks: ids.len() as u64,
        changed,
    }))
}

/// ` AND field IN (…)`, unless every field is meant.
fn push_fields(query: &mut QueryBuilder<sqlx::MySql>, fields: &[TagField]) {
    if fields.is_empty() {
        return;
    }
    query.push(" AND field IN (");
    let mut separated = query.separated(", ");
    for field in fields {
        separated.push_bind(field.as_str());
    }
    query.push(")");
}
