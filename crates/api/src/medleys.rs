// SPDX-License-Identifier: AGPL-3.0-or-later

//! Medleys: song hooks and liners picked from a recipe, then mixed into a
//! library file by heron-librarian (the API cannot write files). Picking is
//! [`heron_core::medley::plan`]; the rendered file becomes a track like any
//! other, linked back to its medley by [`run_linker`]. See `docs/medleys.md`.

use std::collections::HashSet;
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use heron_core::library::TaskRequest;
use heron_core::medley::{
    self, Candidate, DuckPlace, FadeCurve, LinerMode, LinerSlot, Liners, Part, PartKind, Recipe,
};
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate};
use crate::librarian::{self, LibraryTask};
use crate::settings::{self, StationSettings};
use crate::stations::Station;

const LINK_INTERVAL: Duration = Duration::from_secs(10);
/// The longest part of a medley.
const MAX_PART_MS: u32 = 120_000;
/// The most parts in a medley: 30 hooks and a liner after each, and one before.
const MAX_PARTS: usize = 61;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_medleys, create_medley))
        .routes(routes!(get_medley, update_medley, delete_medley))
        .routes(routes!(plan_medley))
        .routes(routes!(duplicate_medley))
        .routes(routes!(pick_part))
        .routes(routes!(list_candidates))
        .routes(routes!(preview_medley, get_preview))
        .routes(routes!(render_medley))
        .routes(routes!(get_settings, put_settings))
}

/// A station's medley settings: its own categories, and what every medley
/// shares (naming and the recipe of a new one).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MedleySettings {
    /// The category rendered medleys go in (their track's), usually a
    /// Jingle-type one.
    pub category_id: Option<u64>,
    /// Where the liners come from.
    pub liner_category_id: Option<u64>,
    /// Where the liner at the start comes from; the liner category when unset.
    pub opening_liner_category_id: Option<u64>,
    /// Where the liner at the end comes from; the liner category when unset.
    pub closing_liner_category_id: Option<u64>,
    /// Medleys are named after their hooks' artists, or titles.
    pub name_from: NameFrom,
    /// The recipe of a new medley; each medley can change it.
    pub defaults: MedleyDefaults,
}

/// What every medley shares (the `medleys` settings).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SharedMedleySettings {
    name_from: NameFrom,
    defaults: MedleyDefaults,
}

impl SharedMedleySettings {
    const SETTINGS_NAME: &str = "medleys";
}

/// A station's medley categories (its `medley_categories` settings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MedleyCategories {
    pub category_id: Option<u64>,
    pub liner_category_id: Option<u64>,
    pub opening_liner_category_id: Option<u64>,
    pub closing_liner_category_id: Option<u64>,
}

impl MedleyCategories {
    pub const SETTINGS_NAME: &str = "medley_categories";

    pub fn ids(self) -> impl Iterator<Item = u64> {
        [
            self.category_id,
            self.liner_category_id,
            self.opening_liner_category_id,
            self.closing_liner_category_id,
        ]
        .into_iter()
        .flatten()
    }
}

/// A station's medley categories.
pub async fn categories_of(pool: &MySqlPool, station_id: u64) -> ApiResult<MedleyCategories> {
    settings::load_for(pool, station_id, MedleyCategories::SETTINGS_NAME).await
}

impl Default for MedleySettings {
    fn default() -> Self {
        Self {
            category_id: None,
            liner_category_id: None,
            opening_liner_category_id: None,
            closing_liner_category_id: None,
            name_from: NameFrom::Artists,
            defaults: MedleyDefaults::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NameFrom {
    #[default]
    Artists,
    Titles,
}

/// A recipe without its songs: the defaults of a new medley.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MedleyDefaults {
    pub hooks: u32,
    pub hook_min_ms: u32,
    pub hook_max_ms: u32,
    pub crossfade_ms: u32,
    pub liner_every: u32,
    pub liner_at_start: bool,
    pub liner_at_end: bool,
    pub liner_mode: LinerMode,
    pub duck_db: f64,
    /// The music down over 0.3 s from a liner's start, and back up over
    /// 0.3 s across its end, by default.
    pub duck_attack_ms: u32,
    pub duck_attack_at: DuckPlace,
    pub duck_release_ms: u32,
    pub duck_release_at: DuckPlace,
    /// No fade-in by default.
    pub fade_in_ms: u32,
    pub fade_in_curve: FadeCurve,
    /// No fade-out by default.
    pub fade_out_ms: u32,
    pub fade_out_curve: FadeCurve,
}

impl Default for MedleyDefaults {
    fn default() -> Self {
        let recipe = Recipe::default();
        Self {
            hooks: recipe.hooks,
            hook_min_ms: recipe.hook_min_ms,
            hook_max_ms: recipe.hook_max_ms,
            crossfade_ms: recipe.crossfade_ms,
            liner_every: recipe.liner_every,
            liner_at_start: recipe.liner_at_start,
            liner_at_end: recipe.liner_at_end,
            liner_mode: recipe.liner_mode,
            duck_db: recipe.duck_db,
            duck_attack_ms: recipe.duck_attack_ms,
            duck_attack_at: recipe.duck_attack_at,
            duck_release_ms: recipe.duck_release_ms,
            duck_release_at: recipe.duck_release_at,
            fade_in_ms: recipe.fade_in_ms,
            fade_in_curve: recipe.fade_in_curve,
            fade_out_ms: recipe.fade_out_ms,
            fade_out_curve: recipe.fade_out_curve,
        }
    }
}

impl MedleyDefaults {
    fn check(&self) -> Result<(), String> {
        Recipe {
            hooks: self.hooks,
            hook_min_ms: self.hook_min_ms,
            hook_max_ms: self.hook_max_ms,
            crossfade_ms: self.crossfade_ms,
            liner_every: self.liner_every,
            liner_at_start: self.liner_at_start,
            liner_at_end: self.liner_at_end,
            liner_mode: self.liner_mode,
            duck_db: self.duck_db,
            duck_attack_ms: self.duck_attack_ms,
            duck_attack_at: self.duck_attack_at,
            duck_release_ms: self.duck_release_ms,
            duck_release_at: self.duck_release_at,
            fade_in_ms: self.fade_in_ms,
            fade_in_curve: self.fade_in_curve,
            fade_out_ms: self.fade_out_ms,
            fade_out_curve: self.fade_out_curve,
            ..Recipe::default()
        }
        .check_settings()
    }
}

async fn shared_settings(pool: &MySqlPool) -> ApiResult<SharedMedleySettings> {
    settings::load(pool, SharedMedleySettings::SETTINGS_NAME).await
}

async fn load_settings(pool: &MySqlPool, station_id: u64) -> ApiResult<MedleySettings> {
    let shared = shared_settings(pool).await?;
    let categories = categories_of(pool, station_id).await?;
    Ok(MedleySettings {
        category_id: categories.category_id,
        liner_category_id: categories.liner_category_id,
        opening_liner_category_id: categories.opening_liner_category_id,
        closing_liner_category_id: categories.closing_liner_category_id,
        name_from: shared.name_from,
        defaults: shared.defaults,
    })
}

/// A station's medley settings: its categories, and the shared naming and
/// defaults.
#[utoipa::path(get, path = "/settings/medleys", tag = "settings",
    responses((status = 200, body = MedleySettings), (status = "4XX", body = ErrorBody)))]
async fn get_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<MedleySettings>> {
    Ok(Json(load_settings(&state.pool, station.id()).await?))
}

/// Replaces a station's medley categories and the shared naming and
/// defaults (missing fields take the defaults). Its categories must be
/// tagged with the station. Tracks already rendered keep their categories.
/// Operators and admins.
#[utoipa::path(put, path = "/settings/medleys", tag = "settings",
    request_body = MedleySettings,
    responses((status = 200, body = MedleySettings), (status = "4XX", body = ErrorBody)))]
async fn put_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(settings): Json<MedleySettings>,
) -> ApiResult<Json<MedleySettings>> {
    user.require(Role::Operator)?;
    settings.defaults.check().map_err(ApiError::Invalid)?;
    for id in [
        settings.category_id,
        settings.liner_category_id,
        settings.opening_liner_category_id,
        settings.closing_liner_category_id,
    ]
    .into_iter()
    .flatten()
    {
        crate::categories::check_offered(&state.pool, station.id(), id).await?;
    }
    let categories = MedleyCategories {
        category_id: settings.category_id,
        liner_category_id: settings.liner_category_id,
        opening_liner_category_id: settings.opening_liner_category_id,
        closing_liner_category_id: settings.closing_liner_category_id,
    };
    let shared = SharedMedleySettings {
        name_from: settings.name_from,
        defaults: settings.defaults.clone(),
    };
    let mut tx = state.pool.begin().await?;
    settings::save(&mut *tx, SharedMedleySettings::SETTINGS_NAME, &shared).await?;
    settings::save_for(
        &mut *tx,
        station.id(),
        MedleyCategories::SETTINGS_NAME,
        &categories,
    )
    .await?;
    tx.commit().await?;
    tracing::info!(user = %user.username, ?settings, "medley settings changed");
    Ok(Json(settings))
}

/// A part, with what its track is.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PartView {
    pub kind: PartKind,
    pub track_id: u64,
    /// On the file's timeline.
    pub from_ms: u32,
    pub to_ms: u32,
    /// A liner over the music: how its gap is shared, see `Part`.
    pub shift_ms: i32,
    /// How far it was moved in the medley, see `Part`.
    pub move_ms: i32,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub path: String,
    /// The file's length, to keep the part within it.
    pub duration_ms: Option<u32>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Medley {
    pub id: u64,
    /// Given at each render, from its hooks; none until rendered.
    pub name: Option<String>,
    pub recipe: Recipe,
    /// Its current version, in order (empty until rendered once).
    pub parts: Vec<PartView>,
    /// The rendered file, relative to the library root.
    pub path: Option<String>,
    /// The rendered file's track, once the scanner found it.
    pub track_id: Option<u64>,
    /// The last render asked: its library task.
    pub task_id: Option<u64>,
    /// `pending`, `running`, `done` or `failed`.
    pub task_status: Option<String>,
    pub task_error: Option<String>,
    pub rendered_at_unix_ms: Option<i64>,
    /// The layout of the file rendered last, part by part; none for a medley
    /// rendered before it was saved.
    pub rendered_layout: Option<medley::Layout>,
    /// The stations it plays on; the first is the one its liners come from.
    pub station_ids: Vec<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MedleyInput {
    #[serde(default)]
    pub recipe: Recipe,
    /// The stations it plays on, the first one first: the station it is
    /// made on when left out on creation, kept when left out on a change.
    #[serde(default)]
    pub station_ids: Option<Vec<u64>>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MedleysQuery {
    /// Only the medleys of this station.
    pub station_id: Option<u64>,
}

impl MedleyInput {
    fn check(&self) -> ApiResult<()> {
        self.recipe.check().map_err(ApiError::Invalid)
    }
}

/// Parts picked from the recipe, not saved.
#[derive(Debug, Serialize, ToSchema)]
pub struct MedleyPlan {
    pub parts: Vec<PartView>,
    /// Songs that can give a hook: fewer than the recipe's hooks means a
    /// shorter medley.
    pub qualified: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PickRequest {
    pub kind: PartKind,
    /// Tracks not to pick (those already in the plan).
    #[serde(default)]
    pub exclude: Vec<u64>,
    /// Where the liner goes, for a liner.
    #[serde(default)]
    pub slot: LinerSlot,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CandidatesQuery {
    pub kind: PartKind,
    /// Where the liner goes, for a liner: `between` by default.
    #[serde(default)]
    #[param(inline)]
    pub slot: LinerSlot,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderRequest {
    pub parts: Vec<Part>,
}

type MedleyRow = (
    u64,
    Option<String>,
    String,
    Option<String>,
    Option<u64>,
    Option<u64>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
);

const SELECT: &str = "SELECT m.id, m.name, CAST(m.recipe AS CHAR), m.path,
        m.track_id, m.task_id, CAST(lt.status AS CHAR), lt.error,
        CAST(UNIX_TIMESTAMP(m.rendered_at) * 1000 AS SIGNED), CAST(m.rendered_layout AS CHAR)
    FROM medleys m
    LEFT JOIN library_tasks lt ON lt.id = m.task_id";

async fn load(pool: &MySqlPool, id: Option<u64>) -> ApiResult<Vec<Medley>> {
    let rows: Vec<MedleyRow> = match id {
        Some(id) => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE m.id = ?")))
                .bind(id)
                .fetch_all(pool)
                .await?
        }
        None => {
            // Not rendered yet (no name) first, the newest first.
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "{SELECT} ORDER BY m.name IS NOT NULL, m.name, m.id DESC"
            )))
            .fetch_all(pool)
            .await?
        }
    };
    let mut medleys = Vec::with_capacity(rows.len());
    for (id, name, recipe, path, track_id, task_id, status, error, rendered, layout) in rows {
        let recipe = serde_json::from_str(&recipe)
            .map_err(|error| ApiError::Internal(format!("medley {id}: {error}")))?;
        let parts: Vec<Part> = saved_parts(pool, id).await?;
        let station_ids = sqlx::query_scalar(
            "SELECT station_id FROM medley_stations WHERE medley_id = ? ORDER BY position",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;
        medleys.push(Medley {
            id,
            name,
            recipe,
            parts: views(pool, &parts).await?,
            path,
            track_id,
            task_id,
            task_status: status,
            task_error: error,
            rendered_at_unix_ms: rendered,
            // A layout this version cannot read is left out, not an error.
            rendered_layout: layout.and_then(|json| serde_json::from_str(&json).ok()),
            station_ids,
        });
    }
    Ok(medleys)
}

async fn find(pool: &MySqlPool, id: u64) -> ApiResult<Medley> {
    load(pool, Some(id))
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("medley"))
}

async fn saved_parts(pool: &MySqlPool, id: u64) -> ApiResult<Vec<Part>> {
    let rows: Vec<(String, u64, u32, u32, i32, i32)> = sqlx::query_as(
        "SELECT CAST(kind AS CHAR), track_id, from_ms, to_ms, shift_ms, move_ms FROM medley_parts
         WHERE medley_id = ? ORDER BY position",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, track_id, from_ms, to_ms, shift_ms, move_ms)| Part {
            kind: if kind == "liner" {
                PartKind::Liner
            } else {
                PartKind::Hook
            },
            track_id,
            from_ms,
            to_ms,
            shift_ms,
            move_ms,
        })
        .collect())
}

/// The parts with their tracks' title, artist and path.
async fn views(pool: &MySqlPool, parts: &[Part]) -> ApiResult<Vec<PartView>> {
    if parts.is_empty() {
        return Ok(Vec::new());
    }
    let mut query =
        QueryBuilder::new("SELECT id, title, artist, path, duration_ms FROM tracks WHERE id IN (");
    let mut separated = query.separated(", ");
    for part in parts {
        separated.push_bind(part.track_id);
    }
    query.push(")");
    type Row = (u64, Option<String>, Option<String>, String, Option<u32>);
    let tracks: Vec<Row> = query.build_query_as().fetch_all(pool).await?;
    Ok(parts
        .iter()
        .filter_map(|part| {
            let (_, title, artist, path, duration_ms) =
                tracks.iter().find(|track| track.0 == part.track_id)?;
            Some(PartView {
                kind: part.kind,
                track_id: part.track_id,
                from_ms: part.from_ms,
                to_ms: part.to_ms,
                shift_ms: part.shift_ms,
                move_ms: part.move_ms,
                title: title.clone(),
                artist: artist.clone(),
                path: path.clone(),
                duration_ms: *duration_ms,
            })
        })
        .collect())
}

/// The songs that can give a hook to a medley: from its recipe's
/// categories, or every category its stations can use.
async fn songs(pool: &MySqlPool, medley: &Medley) -> ApiResult<Vec<Candidate>> {
    let recipe = &medley.recipe;
    let mut query = QueryBuilder::new(
        "SELECT t.id, m.hook_in_ms, m.hook_out_ms,
                CAST(COALESCE(mb.first_release_year, t.year) AS UNSIGNED),
                CAST(COALESCE(p.bpm, a.bpm) AS DOUBLE)
         FROM tracks t
         JOIN track_markers m ON m.track_id = t.id
         JOIN track_analysis a ON a.track_id = t.id
         LEFT JOIN track_musicbrainz mb ON mb.track_id = t.id
         LEFT JOIN track_programming p ON p.track_id = t.id
         WHERE t.deleted_at IS NULL AND t.enabled AND t.missing_since IS NULL
           AND a.cue_out_ms > a.cue_in_ms
           AND m.hook_in_ms IS NOT NULL AND m.hook_out_ms > m.hook_in_ms
           AND NOT EXISTS (SELECT 1 FROM medleys md WHERE md.track_id = t.id)
           AND EXISTS (SELECT 1 FROM track_categories tc
                       WHERE tc.track_id = t.id AND tc.category_id IN (",
    );
    if recipe.category_ids.is_empty() {
        query.push("SELECT cs.category_id FROM category_stations cs WHERE cs.station_id IN (");
        let mut separated = query.separated(", ");
        // No station: no song.
        separated.push_bind(0u64);
        for id in &medley.station_ids {
            separated.push_bind(*id);
        }
        query.push(")");
    } else {
        let mut separated = query.separated(", ");
        for id in &recipe.category_ids {
            separated.push_bind(*id);
        }
    }
    query.push("))");
    if let Some(label_id) = recipe.label_id {
        query
            .push(" AND EXISTS (SELECT 1 FROM track_labels tl WHERE tl.track_id = t.id AND tl.label_id = ")
            .push_bind(label_id)
            .push(")");
    }
    // Track, hook in and out, year, BPM.
    type Row = (u64, u32, u32, Option<u64>, Option<f64>);
    let rows: Vec<Row> = query.build_query_as().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(track_id, from_ms, to_ms, year, bpm)| Candidate {
            track_id,
            from_ms,
            to_ms,
            year: year.and_then(|year| u16::try_from(year).ok()),
            bpm,
        })
        .collect())
}

/// The liners of a medley's first station, by where they go; none when its
/// recipe has its liners off.
async fn liners(pool: &MySqlPool, medley: &Medley) -> ApiResult<Liners> {
    let Some(first) = medley.station_ids.first() else {
        return Ok(Liners::default());
    };
    if !medley.recipe.liners {
        return Ok(Liners::default());
    }
    let settings = categories_of(pool, *first).await?;
    Ok(Liners {
        between: liner_tracks(pool, settings.liner_category_id).await?,
        start: liner_tracks(pool, settings.opening_liner_category_id).await?,
        end: liner_tracks(pool, settings.closing_liner_category_id).await?,
    })
}

/// The tracks of a liner category, from cue-in to cue-out; never a medley,
/// even when medleys go in that category.
async fn liner_tracks(pool: &MySqlPool, category_id: Option<u64>) -> ApiResult<Vec<Candidate>> {
    let Some(category_id) = category_id else {
        return Ok(Vec::new());
    };
    let rows: Vec<(u64, u32, u32)> = sqlx::query_as(
        "SELECT t.id, COALESCE(o.cue_in_ms, a.cue_in_ms), COALESCE(o.cue_out_ms, a.cue_out_ms)
         FROM tracks t
         JOIN track_categories tc ON tc.track_id = t.id AND tc.category_id = ?
         JOIN track_analysis a ON a.track_id = t.id
         LEFT JOIN track_overrides o ON o.track_id = t.id
         WHERE t.deleted_at IS NULL AND t.enabled AND t.missing_since IS NULL
           AND a.cue_out_ms > a.cue_in_ms
           AND NOT EXISTS (SELECT 1 FROM medleys md WHERE md.track_id = t.id)",
    )
    .bind(category_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, from_ms, to_ms)| to_ms > from_ms)
        .map(|(track_id, from_ms, to_ms)| Candidate {
            track_id,
            from_ms,
            to_ms: to_ms.min(from_ms.saturating_add(MAX_PART_MS)),
            year: None,
            bpm: None,
        })
        .collect())
}

/// The tracks of the medley's current version: new picks avoid them.
async fn current_tracks(pool: &MySqlPool, id: u64) -> ApiResult<HashSet<u64>> {
    Ok(saved_parts(pool, id)
        .await?
        .into_iter()
        .map(|part| part.track_id)
        .collect())
}

/// Every medley, by name.
#[utoipa::path(get, path = "/medleys", tag = "medleys", params(MedleysQuery),
    responses((status = 200, body = Vec<Medley>), (status = "4XX", body = ErrorBody)))]
async fn list_medleys(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<MedleysQuery>,
) -> ApiResult<Json<Vec<Medley>>> {
    let mut medleys = load(&state.pool, None).await?;
    if let Some(station_id) = query.station_id {
        medleys.retain(|medley| medley.station_ids.contains(&station_id));
    }
    Ok(Json(medleys))
}

#[utoipa::path(get, path = "/medleys/{id}", tag = "medleys", params(("id" = u64, Path)),
    responses((status = 200, body = Medley), (status = "4XX", body = ErrorBody)))]
async fn get_medley(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<Medley>> {
    Ok(Json(find(&state.pool, id).await?))
}

fn recipe_json(recipe: &Recipe) -> ApiResult<String> {
    serde_json::to_string(recipe).map_err(|error| ApiError::Internal(error.to_string()))
}

/// Operators and admins.
#[utoipa::path(post, path = "/medleys", tag = "medleys", request_body = MedleyInput,
    responses((status = 201, body = Medley), (status = "4XX", body = ErrorBody)))]
async fn create_medley(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(input): Json<MedleyInput>,
) -> ApiResult<(StatusCode, Json<Medley>)> {
    user.require(Role::Operator)?;
    input.check()?;
    let station_ids = input
        .station_ids
        .clone()
        .unwrap_or_else(|| vec![station.id()]);
    check_stations(&state, &station_ids, &station_ids, &input.recipe).await?;
    let mut tx = state.pool.begin().await?;
    let id = sqlx::query("INSERT INTO medleys (recipe) VALUES (?)")
        .bind(recipe_json(&input.recipe)?)
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    write_stations(&mut tx, id, &station_ids).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(find(&state.pool, id).await?)))
}

/// Refuses stations that do not exist, new ones (`added`) without a
/// medleys' category, and a recipe category none of them can use.
async fn check_stations(
    state: &AppState,
    station_ids: &[u64],
    added: &[u64],
    recipe: &Recipe,
) -> ApiResult<()> {
    if station_ids.is_empty() {
        return Err(ApiError::Invalid(
            "a medley plays on at least one station".to_owned(),
        ));
    }
    let unique: HashSet<u64> = station_ids.iter().copied().collect();
    if unique.len() != station_ids.len() {
        return Err(ApiError::Invalid("a station is listed twice".to_owned()));
    }
    for id in station_ids {
        let station = state
            .stations
            .get(*id)
            .ok_or_else(|| ApiError::Invalid(format!("no station {id}")))?;
        if added.contains(id) && categories_of(&state.pool, *id).await?.category_id.is_none() {
            return Err(ApiError::Invalid(format!(
                "{} has no medleys' category: set it in its medley settings first",
                station.code()
            )));
        }
    }
    for category_id in &recipe.category_ids {
        let offered: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM category_stations WHERE category_id = ? AND station_id IN ({})",
            station_ids
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )))
        .bind(category_id)
        .fetch_one(&state.pool)
        .await?;
        if offered == 0 {
            return Err(ApiError::Invalid(format!(
                "category {category_id} is not used on the medley's stations"
            )));
        }
    }
    Ok(())
}

async fn write_stations(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    medley_id: u64,
    station_ids: &[u64],
) -> ApiResult<()> {
    sqlx::query("DELETE FROM medley_stations WHERE medley_id = ?")
        .bind(medley_id)
        .execute(&mut **tx)
        .await?;
    for (position, station_id) in station_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO medley_stations (medley_id, station_id, position) VALUES (?, ?, ?)",
        )
        .bind(medley_id)
        .bind(station_id)
        .bind(u32::try_from(position).unwrap_or(u32::MAX))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Moves a rendered medley's track into the categories of the stations it
/// now plays on, and out of those it no longer does (unless another of its
/// stations shares the category).
async fn follow_stations(pool: &MySqlPool, medley: &Medley, before: &[u64]) -> ApiResult<()> {
    let Some(track_id) = medley.track_id else {
        return Ok(());
    };
    let mut keep = HashSet::new();
    for id in &medley.station_ids {
        if let Some(category_id) = categories_of(pool, *id).await?.category_id {
            keep.insert(category_id);
            put_in_category(pool, track_id, category_id).await?;
        }
    }
    for id in before.iter().filter(|id| !medley.station_ids.contains(id)) {
        if let Some(category_id) = categories_of(pool, *id).await?.category_id
            && !keep.contains(&category_id)
        {
            sqlx::query("DELETE FROM track_categories WHERE track_id = ? AND category_id = ?")
                .bind(track_id)
                .bind(category_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// Changes a medley's recipe (applied on its next build). Operators and
/// admins.
#[utoipa::path(put, path = "/medleys/{id}", tag = "medleys", params(("id" = u64, Path)),
    request_body = MedleyInput,
    responses((status = 200, body = Medley), (status = "4XX", body = ErrorBody)))]
async fn update_medley(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(input): Json<MedleyInput>,
) -> ApiResult<Json<Medley>> {
    user.require(Role::Operator)?;
    input.check()?;
    let before = find(&state.pool, id).await?;
    let station_ids = input
        .station_ids
        .clone()
        .unwrap_or_else(|| before.station_ids.clone());
    let added: Vec<u64> = station_ids
        .iter()
        .copied()
        .filter(|id| !before.station_ids.contains(id))
        .collect();
    check_stations(&state, &station_ids, &added, &input.recipe).await?;
    if let Some(track_id) = before.track_id {
        for removed in before
            .station_ids
            .iter()
            .filter(|s| !station_ids.contains(s))
        {
            let Some(station) = state.stations.get(*removed) else {
                continue;
            };
            let waiting = station
                .engine
                .state()
                .borrow()
                .as_ref()
                .is_some_and(|player| {
                    player
                        .waiting()
                        .any(|entry| entry.track.track_id == Some(track_id))
                });
            if waiting {
                return Err(ApiError::Conflict(format!(
                    "the medley waits in {}'s queue: take it out first",
                    station.code()
                )));
            }
        }
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE medleys SET recipe = ? WHERE id = ?")
        .bind(recipe_json(&input.recipe)?)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    write_stations(&mut tx, id, &station_ids).await?;
    tx.commit().await?;
    let medley = find(&state.pool, id).await?;
    follow_stations(&state.pool, &medley, &before.station_ids).await?;
    Ok(Json(medley))
}

/// Deletes the recipe; the rendered track stays in the library. Operators
/// and admins.
#[utoipa::path(delete, path = "/medleys/{id}", tag = "medleys", params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_medley(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let deleted = sqlx::query("DELETE FROM medleys WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found("medley"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// A new medley with the same recipe and parts, not rendered (no name
/// until it is). Operators and admins.
#[utoipa::path(post, path = "/medleys/{id}/duplicate", tag = "medleys", params(("id" = u64, Path)),
    responses((status = 201, body = Medley), (status = "4XX", body = ErrorBody)))]
async fn duplicate_medley(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<(StatusCode, Json<Medley>)> {
    user.require(Role::Operator)?;
    let original = find(&state.pool, id).await?;
    let mut tx = state.pool.begin().await?;
    let copy = sqlx::query("INSERT INTO medleys (recipe) VALUES (?)")
        .bind(recipe_json(&original.recipe)?)
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    write_stations(&mut tx, copy, &original.station_ids).await?;
    sqlx::query(
        "INSERT INTO medley_parts (medley_id, position, kind, track_id, from_ms, to_ms, shift_ms, move_ms)
         SELECT ?, position, kind, track_id, from_ms, to_ms, shift_ms, move_ms
         FROM medley_parts WHERE medley_id = ?",
    )
    .bind(copy)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(find(&state.pool, copy).await?)))
}

/// Picks the parts from the recipe, without saving them. Operators and admins.
#[utoipa::path(post, path = "/medleys/{id}/plan", tag = "medleys", params(("id" = u64, Path)),
    responses((status = 200, body = MedleyPlan), (status = "4XX", body = ErrorBody)))]
async fn plan_medley(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<MedleyPlan>> {
    user.require(Role::Operator)?;
    let medley = find(&state.pool, id).await?;
    if medley.rendered_at_unix_ms.is_some() {
        return Err(ApiError::Conflict(
            "a rendered medley is edited, not built anew: duplicate it for a new one".to_owned(),
        ));
    }
    let recipe = &medley.recipe;
    let songs = songs(&state.pool, &medley).await?;
    let liners = liners(&state.pool, &medley).await?;
    let avoid = current_tracks(&state.pool, id).await?;
    let parts = medley::plan(recipe, &songs, &liners, &avoid, crate::rotation::seed());
    Ok(Json(MedleyPlan {
        parts: views(&state.pool, &parts).await?,
        qualified: medley::qualified(recipe, &songs) as u64,
    }))
}

/// Another hook or liner, none of `exclude`, to replace one in a plan.
/// Operators and admins.
#[utoipa::path(post, path = "/medleys/{id}/pick", tag = "medleys", params(("id" = u64, Path)),
    request_body = PickRequest,
    responses((status = 200, body = PartView), (status = "4XX", body = ErrorBody)))]
async fn pick_part(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(request): Json<PickRequest>,
) -> ApiResult<Json<PartView>> {
    user.require(Role::Operator)?;
    let medley = find(&state.pool, id).await?;
    let candidates = match request.kind {
        PartKind::Hook => songs(&state.pool, &medley).await?,
        PartKind::Liner => liners(&state.pool, &medley)
            .await?
            .at(request.slot)
            .to_vec(),
    };
    let exclude: HashSet<u64> = request.exclude.into_iter().collect();
    let avoid = current_tracks(&state.pool, id).await?;
    let part = medley::pick_one(
        &medley.recipe,
        request.kind,
        &candidates,
        &exclude,
        &avoid,
        crate::rotation::seed(),
    )
    .ok_or_else(|| ApiError::Conflict("no other track to pick".to_owned()))?;
    views(&state.pool, &[part])
        .await?
        .pop()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("track"))
}

/// Every track that can be a part of `kind`, with the span it would take,
/// by artist and title: to choose one. Operators and admins.
#[utoipa::path(get, path = "/medleys/{id}/candidates", tag = "medleys",
    params(("id" = u64, Path), CandidatesQuery),
    responses((status = 200, body = Vec<PartView>), (status = "4XX", body = ErrorBody)))]
async fn list_candidates(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<CandidatesQuery>,
) -> ApiResult<Json<Vec<PartView>>> {
    user.require(Role::Operator)?;
    let medley = find(&state.pool, id).await?;
    let candidates = match query.kind {
        PartKind::Hook => songs(&state.pool, &medley).await?,
        PartKind::Liner => liners(&state.pool, &medley).await?.at(query.slot).to_vec(),
    };
    let parts = medley::choices(&medley.recipe, query.kind, &candidates);
    let mut views = views(&state.pool, &parts).await?;
    views.sort_by_cached_key(|view| {
        (
            view.artist.clone().unwrap_or_default().to_lowercase(),
            view.title
                .clone()
                .unwrap_or_else(|| view.path.clone())
                .to_lowercase(),
        )
    });
    Ok(Json(views))
}

/// Saves the parts as the medley's version, names the medley after its
/// hooks and asks heron-librarian to mix them into its file. Operators and
/// admins.
#[utoipa::path(post, path = "/medleys/{id}/render", tag = "medleys", params(("id" = u64, Path)),
    request_body = RenderRequest,
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn render_medley(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(request): Json<RenderRequest>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let medley = find(&state.pool, id).await?;
    let parts = request.parts;
    check_parts(&state.pool, &parts).await?;
    let name = name_for(&state.pool, &medley, &parts).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE medleys SET name = ? WHERE id = ?")
        .bind(&name)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if is_duplicate(&error) {
                ApiError::Conflict("another medley took this name: try again".to_owned())
            } else {
                error.into()
            }
        })?;
    sqlx::query("DELETE FROM medley_parts WHERE medley_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let mut query = QueryBuilder::new(
        "INSERT INTO medley_parts
         (medley_id, position, kind, track_id, from_ms, to_ms, shift_ms, move_ms) ",
    );
    query.push_values(parts.iter().enumerate(), |mut row, (position, part)| {
        row.push_bind(id)
            .push_bind(position as u32)
            .push_bind(match part.kind {
                PartKind::Hook => "hook",
                PartKind::Liner => "liner",
            })
            .push_bind(part.track_id)
            .push_bind(part.from_ms)
            .push_bind(part.to_ms)
            .push_bind(part.shift_ms)
            .push_bind(part.move_ms);
    });
    query.build().execute(&mut *tx).await?;
    tx.commit().await?;
    if let Some(track_id) = medley.track_id
        && medley.name.as_deref() != Some(&name)
    {
        name_track(&state.pool, track_id, &name).await?;
    }
    let task = librarian::ask(&state, &user, TaskRequest::RenderMedley { medley_id: id }).await?;
    sqlx::query("UPDATE medleys SET task_id = ? WHERE id = ?")
        .bind(task.id)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

/// The name a medley gets for these parts: after its hooks' artists or
/// titles (as set), the day of now in its first station's time zone, with
/// ` (2)`, ` (3)`… when another medley has it.
async fn name_for(pool: &MySqlPool, medley: &Medley, parts: &[Part]) -> ApiResult<String> {
    let id = medley.id;
    let settings = shared_settings(pool).await?;
    let station: StationSettings = match medley.station_ids.first() {
        Some(first) => settings::load_for(pool, *first, StationSettings::SETTINGS_NAME).await?,
        None => StationSettings::default(),
    };
    let date = jiff::Timestamp::now()
        .to_zoned(station.time_zone())
        .strftime("%Y-%m-%d")
        .to_string();
    let hooks: Vec<Part> = parts
        .iter()
        .filter(|part| part.kind == PartKind::Hook)
        .copied()
        .collect();
    let views = views(pool, &hooks).await?;
    let names: Vec<&str> = views
        .iter()
        .filter_map(|view| match settings.name_from {
            NameFrom::Artists => view.artist.as_deref(),
            NameFrom::Titles => view.title.as_deref(),
        })
        .collect();
    let base = medley::name(&date, &names);
    let mut name = base.clone();
    for number in 2.. {
        let taken: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM medleys WHERE id <> ? AND name = ?")
                .bind(id)
                .bind(&name)
                .fetch_one(pool)
                .await?;
        if taken == 0 {
            break;
        }
        name = format!("{base} ({number})");
    }
    Ok(name)
}

/// Why these parts cannot make a medley, if they cannot.
async fn check_parts(pool: &MySqlPool, parts: &[Part]) -> ApiResult<()> {
    if parts.is_empty() || parts.len() > MAX_PARTS {
        return Err(ApiError::Invalid(format!(
            "a medley has 1 to {MAX_PARTS} parts"
        )));
    }
    if !parts.iter().any(|part| part.kind == PartKind::Hook) {
        return Err(ApiError::Invalid(
            "a medley has at least one hook".to_owned(),
        ));
    }
    if parts.iter().any(|part| {
        part.from_ms >= part.to_ms
            || part.length_ms() > MAX_PART_MS
            || part.shift_ms.unsigned_abs() > MAX_PART_MS
            || part.move_ms.unsigned_abs() > MAX_PART_MS
    }) {
        return Err(ApiError::Invalid(format!(
            "each part starts before it ends, lasts at most {} s and moves at most as much",
            MAX_PART_MS / 1_000
        )));
    }
    let tracks = views(pool, parts).await?;
    if tracks.len() != parts.len() {
        return Err(ApiError::Invalid(
            "a part's track does not exist".to_owned(),
        ));
    }
    if tracks
        .iter()
        .any(|part| part.duration_ms.is_some_and(|length| part.to_ms > length))
    {
        return Err(ApiError::Invalid("a part ends after its file".to_owned()));
    }
    Ok(())
}

/// Mixes the parts into the medley's preview, outside the library: nothing
/// is saved. Operators and admins.
#[utoipa::path(post, path = "/medleys/{id}/preview", tag = "medleys", params(("id" = u64, Path)),
    request_body = RenderRequest,
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn preview_medley(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(request): Json<RenderRequest>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    find(&state.pool, id).await?;
    check_parts(&state.pool, &request.parts).await?;
    let task = librarian::ask(
        &state,
        &user,
        TaskRequest::PreviewMedley {
            medley_id: id,
            parts: request.parts,
        },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

/// The last preview's audio (WAV), with range requests.
#[utoipa::path(get, path = "/medleys/{id}/preview", tag = "medleys", params(("id" = u64, Path)),
    responses((status = 200, content_type = "audio/wav"), (status = "4XX", body = ErrorBody)))]
async fn get_preview(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    request: Request,
) -> ApiResult<Response> {
    let file = state
        .library
        .root()
        .join(heron_core::medley::preview_path(id));
    let service = tower_http::services::ServeFile::new(file);
    let (parts, _) = request.into_parts();
    let request = Request::from_parts(parts, Body::empty());
    let response = tower::ServiceExt::oneshot(service, request)
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(ApiError::not_found("preview"));
    }
    Ok(response.map(Body::new))
}

/// Shows the medley's name as its track's title, kept in Heron's database
/// (a WAV file has no tags to read it from).
async fn name_track(pool: &MySqlPool, track_id: u64, name: &str) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO track_tag_overrides (track_id, field, value) VALUES (?, 'title', ?)
         ON DUPLICATE KEY UPDATE value = VALUES(value)",
    )
    .bind(track_id)
    .bind(name)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE tracks SET title = ? WHERE id = ?")
        .bind(name)
        .bind(track_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Puts a track in a category, if the category still exists.
async fn put_in_category(pool: &MySqlPool, track_id: u64, category_id: u64) -> ApiResult<()> {
    sqlx::query(
        "INSERT IGNORE INTO track_categories (track_id, category_id)
         SELECT ?, id FROM categories WHERE id = ?",
    )
    .bind(track_id)
    .bind(category_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Links the medleys rendered since to their tracks, once the scanner found
/// their files: the track gets the medley's name and the medleys' category
/// of each of its stations.
pub async fn link(pool: &MySqlPool) -> ApiResult<usize> {
    let rows: Vec<(u64, Option<String>, u64)> = sqlx::query_as(
        "SELECT m.id, m.name, t.id FROM medleys m
         JOIN tracks t ON t.path = m.path AND t.deleted_at IS NULL
         WHERE m.track_id IS NULL OR m.track_id <> t.id",
    )
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    for (medley_id, name, track_id) in &rows {
        sqlx::query("UPDATE medleys SET track_id = ? WHERE id = ?")
            .bind(track_id)
            .bind(medley_id)
            .execute(pool)
            .await?;
        if let Some(name) = name {
            name_track(pool, *track_id, name).await?;
        }
        let stations: Vec<u64> =
            sqlx::query_scalar("SELECT station_id FROM medley_stations WHERE medley_id = ?")
                .bind(medley_id)
                .fetch_all(pool)
                .await?;
        for station_id in stations {
            if let Some(category_id) = categories_of(pool, station_id).await?.category_id {
                put_in_category(pool, *track_id, category_id).await?;
            }
        }
    }
    Ok(rows.len())
}

/// Links rendered medleys to their tracks every few seconds.
pub async fn run_linker(pool: MySqlPool) {
    loop {
        match link(&pool).await {
            Ok(0) => {}
            Ok(linked) => tracing::info!(medleys = linked, "medleys linked to their tracks"),
            Err(error) => tracing::warn!(%error, "cannot link medleys to their tracks"),
        }
        tokio::time::sleep(LINK_INTERVAL).await;
    }
}
