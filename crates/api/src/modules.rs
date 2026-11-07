// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modules turned on and off, for the installation and per station
//! (`docs/modules.md`).
//!
//! The switches are settings: `modules` (shared) and `station_modules`
//! (per station). [`guard`] answers `404` on the routes of a module that
//! is off; the background work of each module asks [`is_on`] itself.

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
pub use heron_core::{Module, Modules};
use serde::Serialize;
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings;
use crate::stations::StationId;

/// The installation's switches.
pub async fn installation(pool: &MySqlPool) -> ApiResult<Modules> {
    settings::load(pool, Modules::SETTINGS_NAME).await
}

/// A station's own switches: what it leaves out is on.
async fn own(pool: &MySqlPool, station_id: u64) -> ApiResult<Modules> {
    let stored: serde_json::Value =
        settings::load_for(pool, station_id, Modules::STATION_SETTINGS_NAME).await?;
    Ok(Modules::over(Modules::ALL_ON, &stored))
}

/// What is on for a station: on for the installation and for the station.
pub async fn on_for(pool: &MySqlPool, station_id: u64) -> ApiResult<Modules> {
    Ok(installation(pool).await?.and(own(pool, station_id).await?))
}

/// Whether `module` is on for a station, or for the installation.
pub async fn is_on(pool: &MySqlPool, station_id: Option<u64>, module: Module) -> ApiResult<bool> {
    let modules = match station_id {
        Some(id) => on_for(pool, id).await?,
        None => installation(pool).await?,
    };
    Ok(modules.is_on(module))
}

/// The module a route (`/api/v1/…`, after the station's prefix is taken
/// off) belongs to.
pub fn module_of(path: &str) -> Option<Module> {
    let rest = path.strip_prefix("/api/v1/")?;
    let segments: Vec<&str> = rest.split('/').collect();
    let module = match segments.as_slice() {
        ["medleys", ..] | ["settings", "medleys"] => Module::Medleys,
        ["covers", ..]
        | ["tracks", _, "cover"]
        | ["public", "covers", ..]
        | ["settings", "station", "logo"] => Module::CoverArt,
        ["listeners", ..] | ["settings", "statistics"] => Module::ListenerStatistics,
        ["webhooks" | "webhook-deliveries", ..] => Module::Webhooks,
        ["restrictions", ..] => Module::Restrictions,
        ["settings", "eq"] | ["eq-presets", ..] => Module::Eq,
        ["settings", "agc"] | ["agc-presets", ..] => Module::Processing,
        ["library", "trash", ..] => Module::Trash,
        ["library", "review" | "organize" | "batches", ..] | ["tracks", _, "review", ..] => {
            Module::Review
        }
        // `GET /artists` is the search of the artist pickers: it answers.
        ["artists", _, ..] | ["artist-names" | "artist-suggestions", ..] => Module::Artists,
        ["labels", ..] | ["tracks", "labels"] => Module::Labels,
        ["shows", key, ..] => Module::from_key(key).filter(|module| module.is_show())?,
        ["likes", ..] => Module::Likes,
        _ => return None,
    };
    Some(module)
}

/// Answers `404` (`module_off`) on the routes of a module turned off: a
/// station's routes when it is off for that station, the shared ones when
/// it is off for the installation.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(module) = module_of(request.uri().path()) else {
        return next.run(request).await;
    };
    let station = request.extensions().get::<StationId>().map(|id| id.0);
    match is_on(&state.pool, station, module).await {
        Ok(true) => next.run(request).await,
        Ok(false) => ApiError::ModuleOff(module).into_response(),
        Err(error) => error.into_response(),
    }
}

/// What follows the modules: the queue (medleys, restrictions) and what
/// the engine has (the deck EQ and the AGC chain, bypassed while off).
fn changed(state: &AppState) {
    state.stations.wake_all();
    for station in state.stations.all() {
        station.sync.changed();
    }
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_modules, put_modules))
        .routes(routes!(get_station_modules, put_station_modules))
}

/// A station's modules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct StationModules {
    /// The installation's switches: off here is off for every station.
    pub installation: Modules,
    /// The station's own switches.
    pub station: Modules,
    /// What is on for the station: on at both levels.
    pub on: Modules,
}

async fn station_modules(pool: &MySqlPool, station_id: u64) -> ApiResult<StationModules> {
    let installation = installation(pool).await?;
    let station = own(pool, station_id).await?;
    Ok(StationModules {
        installation,
        station,
        on: installation.and(station),
    })
}

/// The user's role on a station that exists; `404` for a station they
/// have no role on, as on the station's other routes.
async fn role_on(pool: &MySqlPool, user: &CurrentUser, station_id: u64) -> ApiResult<Role> {
    let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM stations WHERE id = ?")
        .bind(station_id)
        .fetch_optional(pool)
        .await?;
    let not_found = || ApiError::NotFound(format!("station {station_id} not found"));
    exists.ok_or_else(not_found)?;
    if user.role == Role::Admin {
        return Ok(Role::Admin);
    }
    crate::auth::station_role(pool, user.id, station_id)
        .await?
        .ok_or_else(not_found)
}

/// The installation's modules.
#[utoipa::path(get, path = "/modules", tag = "settings",
    responses((status = 200, body = Modules), (status = "4XX", body = ErrorBody)))]
async fn get_modules(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Modules>> {
    Ok(Json(installation(&state.pool).await?))
}

/// Turns modules on or off for the installation (a module left out takes
/// its default: off for a show). Admins only.
#[utoipa::path(put, path = "/modules", tag = "settings", request_body = Modules,
    responses((status = 200, body = Modules), (status = "4XX", body = ErrorBody)))]
async fn put_modules(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(modules): Json<Modules>,
) -> ApiResult<Json<Modules>> {
    user.require(Role::Admin)?;
    settings::save(&state.pool, Modules::SETTINGS_NAME, &modules).await?;
    tracing::info!(user = %user.username, ?modules, "installation modules changed");
    changed(&state);
    Ok(Json(modules))
}

/// A station's modules: the installation's, its own, and what is on.
#[utoipa::path(get, path = "/stations/{id}/modules", tag = "stations",
    params(("id" = u64, Path)),
    responses((status = 200, body = StationModules), (status = "4XX", body = ErrorBody)))]
async fn get_station_modules(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<StationModules>> {
    role_on(&state.pool, &user, id).await?;
    Ok(Json(station_modules(&state.pool, id).await?))
}

/// Turns modules on or off for a station (a module left out is on). A
/// module off for the installation stays off. Admins of the station.
#[utoipa::path(put, path = "/stations/{id}/modules", tag = "stations",
    params(("id" = u64, Path)), request_body = Modules,
    responses((status = 200, body = StationModules), (status = "4XX", body = ErrorBody)))]
async fn put_station_modules(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<Json<StationModules>> {
    if role_on(&state.pool, &user, id).await? < Role::Admin {
        return Err(ApiError::Forbidden);
    }
    if !body.is_object() {
        return Err(ApiError::Invalid(
            "give the modules as an object".to_owned(),
        ));
    }
    // What the station leaves out stays on.
    let modules = Modules::over(Modules::ALL_ON, &body).for_station();
    settings::save_for(&state.pool, id, Modules::STATION_SETTINGS_NAME, &modules).await?;
    tracing::info!(user = %user.username, station = id, ?modules, "station modules changed");
    changed(&state);
    Ok(Json(station_modules(&state.pool, id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_belong_to_their_module() {
        for (path, module) in [
            ("/api/v1/medleys", Some(Module::Medleys)),
            ("/api/v1/medleys/3/render", Some(Module::Medleys)),
            ("/api/v1/settings/medleys", Some(Module::Medleys)),
            ("/api/v1/tracks/12/cover", Some(Module::CoverArt)),
            ("/api/v1/covers/candidates/4/accept", Some(Module::CoverArt)),
            ("/api/v1/public/covers/abcd", Some(Module::CoverArt)),
            ("/api/v1/settings/station/logo", Some(Module::CoverArt)),
            ("/api/v1/listeners/days", Some(Module::ListenerStatistics)),
            (
                "/api/v1/settings/statistics",
                Some(Module::ListenerStatistics),
            ),
            ("/api/v1/webhooks/2/test", Some(Module::Webhooks)),
            ("/api/v1/webhook-deliveries/9/retry", Some(Module::Webhooks)),
            ("/api/v1/restrictions/4", Some(Module::Restrictions)),
            ("/api/v1/settings/eq", Some(Module::Eq)),
            ("/api/v1/eq-presets/2", Some(Module::Eq)),
            ("/api/v1/settings/agc", Some(Module::Processing)),
            ("/api/v1/agc-presets", Some(Module::Processing)),
            ("/api/v1/library/trash/7/restore", Some(Module::Trash)),
            ("/api/v1/library/review", Some(Module::Review)),
            ("/api/v1/library/organize/3/apply", Some(Module::Review)),
            ("/api/v1/library/batches/3/undo", Some(Module::Review)),
            ("/api/v1/tracks/12/review/keep", Some(Module::Review)),
            ("/api/v1/artists/5/merge", Some(Module::Artists)),
            ("/api/v1/artist-names/5", Some(Module::Artists)),
            (
                "/api/v1/artist-suggestions/musicbrainz",
                Some(Module::Artists),
            ),
            ("/api/v1/artists", None),
            ("/api/v1/labels/3/merge", Some(Module::Labels)),
            ("/api/v1/tracks/labels", Some(Module::Labels)),
            ("/api/v1/library/tasks", None),
            ("/api/v1/library/import", None),
            ("/api/v1/shows/dedications", Some(Module::Dedications)),
            ("/api/v1/shows/match", Some(Module::Match)),
            ("/api/v1/shows/likes", Some(Module::Likes)),
            ("/api/v1/likes/week", Some(Module::Likes)),
            ("/api/v1/likes/tracks/12", Some(Module::Likes)),
            ("/api/v1/shows/medleys", None),
            ("/api/v1/shows", None),
            ("/api/v1/tracks/12", None),
            ("/api/v1/tracks/categories", None),
            ("/api/v1/settings/station", None),
            ("/api/v1/public/now-playing", None),
            ("/api/v1/modules", None),
            ("/api/v1/stations/2/modules", None),
            ("/medleys", None),
        ] {
            assert_eq!(module_of(path), module, "{path}");
        }
    }
}
