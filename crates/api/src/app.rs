// SPDX-License-Identifier: AGPL-3.0-or-later

//! The HTTP API: shared state, routes and the OpenAPI document.
//!
//! Every route lives under `/api/v1`. The OpenAPI document is generated
//! from the handlers and served at `/api/v1/openapi.json`; the web client's
//! types are generated from it (`web/src/api/schema.ts`).

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use heron_service::HealthCheck;
use heron_service::db::DatabaseHealth;
use serde::Serialize;
use sqlx::MySqlPool;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{Passwords, SESSION_COOKIE};
use crate::backups::Backups;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::library::{Library, LibraryStatus};
use crate::listeners::Collector;
use crate::stations::Stations;
use crate::webhooks::Webhooks;
use crate::{
    artists, categories, category_types, clocks, eq, history, import, labels, librarian, listeners,
    live, medleys, outputs, player, public, restrictions, rotation, schedules, selection, settings,
    staging, stations, tracks, users, webhooks,
};

pub const SERVICE: &str = "heron-api";

#[derive(Clone)]
pub struct AppState {
    pub pool: MySqlPool,
    pub health: DatabaseHealth,
    pub library: Arc<Library>,
    /// Every station's engine, queue manager and configuration sync.
    pub stations: Arc<Stations>,
    pub passwords: Arc<Passwords>,
    /// The buffer zone, as mounted in this container (read-write).
    pub staging: Arc<std::path::PathBuf>,
    pub webhooks: Arc<Webhooks>,
    /// Listener statistics from the stream servers.
    pub listeners: Arc<Collector>,
    /// The backup folder.
    pub backups: Arc<Backups>,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Heron Broadcaster API",
        description = "Control plane of Heron Broadcaster. Sign in with `POST /api/v1/auth/login` \
                       (session cookie, plus an `X-Requested-With` header on requests that change \
                       something) or send an API token as `Authorization: Bearer heron_…`.",
        license(name = "AGPL-3.0-or-later", identifier = "AGPL-3.0-or-later"),
    ),
    modifiers(&Security),
    security(("session" = []), ("token" = [])),
    components(schemas(ErrorBody)),
    tags(
        (name = "system", description = "Status and the OpenAPI document"),
        (name = "auth", description = "Signing in, your account and your API tokens"),
        (name = "users", description = "User management (admins)"),
        (name = "player", description = "Decks, transport, FX player and the engine"),
        (name = "queue", description = "The tracks waiting to play"),
        (name = "library", description = "Tracks, their analysis and overrides; review, organization, undo, deletion and the trash through heron-librarian"),
        (name = "buffer", description = "The buffer zone: uploads, and new files waiting to be imported"),
        (name = "categories", description = "Groups of tracks"),
        (name = "labels", description = "Keywords on tracks, to sort the library"),
        (name = "medleys", description = "Medleys built from song hooks and liners"),
        (name = "artists", description = "Artists, their names and similar artists"),
        (name = "rotation", description = "Clocks, the schedule, restrictions, the selection log and simulations"),
        (name = "history", description = "What was played"),
        (name = "stations", description = "The stations, each with its own engine and programming"),
        (name = "settings", description = "Settings: shared ones, and each station's own"),
        (name = "outputs", description = "Encoder profiles and outputs"),
        (name = "listeners", description = "Listener statistics, from the stream servers"),
        (name = "live", description = "Live state over a WebSocket"),
        (name = "webhooks", description = "Webhooks on track changes, and their deliveries (admins)"),
        (name = "public", description = "Now playing and history for station websites: no sign-in, any origin"),
    )
)]
struct ApiDoc;

struct Security;

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE))),
        );
        components.add_security_scheme(
            "token",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}

fn api() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(library_status))
        .merge(users::router())
        .merge(crate::backups::router())
        .merge(stations::router())
        .merge(player::router())
        .merge(tracks::router())
        .merge(selection::router())
        .merge(labels::router())
        .merge(librarian::router())
        .merge(import::router())
        .merge(staging::router())
        .merge(categories::router())
        .merge(category_types::router())
        .merge(medleys::router())
        .merge(artists::router())
        .merge(clocks::router())
        .merge(schedules::router())
        .merge(restrictions::router())
        .merge(rotation::router())
        .merge(history::router())
        .merge(settings::router())
        .merge(eq::router())
        .merge(outputs::router())
        .merge(listeners::router())
        .merge(live::router())
        .merge(webhooks::router())
        .merge(public::router())
}

/// The OpenAPI document, as served.
///
/// Each route of a station is documented twice: under
/// `/api/v1/stations/{station_id}/…`, and as the single-station route that
/// serves the first station (`/api/v1/queue`…). The public routes are also
/// documented under `/api/v1/public/stations/{code}/…`.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::openapi::path::{Operation, ParameterBuilder, ParameterIn};
    use utoipa::openapi::{ObjectBuilder, Required, Type};

    let (_, mut openapi) = OpenApiRouter::<AppState>::with_openapi(ApiDoc::openapi())
        .nest("/api/v1", api())
        .split_for_parts();
    openapi.info.version = env!("CARGO_PKG_VERSION").to_owned();
    let mut added = Vec::new();
    for (path, item) in &openapi.paths.paths {
        let (station_path, name, kind, description) = if stations::per_station_path(path) {
            let rest = path.trim_start_matches("/api/v1/");
            (
                format!("/api/v1/stations/{{station_id}}/{rest}"),
                "station_id",
                Type::Integer,
                "The station's ID",
            )
        } else if let Some(rest) = stations::public_path(path) {
            (
                format!("/api/v1/public/stations/{{code}}/{rest}"),
                "code",
                Type::String,
                "The station's code",
            )
        } else {
            continue;
        };
        let parameter = ParameterBuilder::new()
            .name(name)
            .parameter_in(ParameterIn::Path)
            .required(Required::True)
            .description(Some(description))
            .schema(Some(ObjectBuilder::new().schema_type(kind)))
            .build();
        let mut item = item.clone();
        let operations: [&mut Option<Operation>; 8] = [
            &mut item.get,
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.options,
            &mut item.head,
            &mut item.patch,
            &mut item.trace,
        ];
        for operation in operations.into_iter().flatten() {
            if let Some(id) = &mut operation.operation_id {
                id.push_str("_on_station");
            }
            operation
                .parameters
                .get_or_insert_with(Vec::new)
                .insert(0, parameter.clone().into());
        }
        added.push((station_path, item));
    }
    openapi.paths.paths.extend(added);
    openapi
}

pub fn router(state: AppState) -> Router {
    let (router, _) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest("/api/v1", api())
        .split_for_parts();
    let document = Arc::new(openapi());
    let stations = Arc::clone(&state.stations);
    let router = router
        .route(
            "/api/v1/openapi.json",
            axum::routing::get(move || {
                let document = Arc::clone(&document);
                async move { Json(document.as_ref().clone()) }
            }),
        )
        .with_state(state)
        .layer(axum::middleware::map_response(
            crate::error::json_client_errors,
        ));
    // Routing happens after the station's prefix is taken off the path.
    let rewritten = tower::ServiceExt::map_request(router, move |request| {
        stations::rewrite(&stations, request)
    });
    Router::new().fallback_service(rewritten)
}

#[derive(Serialize, ToSchema)]
pub struct Status {
    pub service: &'static str,
    pub version: &'static str,
    /// The commit the published image was built from (`HERON_REVISION`);
    /// missing in a local build.
    pub revision: Option<String>,
    /// `ok` or `unavailable`.
    pub database: &'static str,
    /// `ok` or `unavailable`.
    pub engine: &'static str,
    /// The server clock's synchronization (the host's: containers share it).
    pub clock: heron_service::clock::ClockStatus,
    /// The server's time, to compare with the browser's.
    pub server_time_unix_ms: i64,
}

/// Whether the API, its database and the engine are up, and whether the
/// server clock can be trusted. No sign-in needed.
#[utoipa::path(get, path = "/status", tag = "system", security(()),
    responses((status = 200, body = Status)))]
async fn status(State(state): State<AppState>) -> Json<Status> {
    let database = match state.health.check().await {
        Ok(()) => "ok",
        Err(_) => "unavailable",
    };
    Json(Status {
        service: SERVICE,
        version: env!("CARGO_PKG_VERSION"),
        revision: std::env::var("HERON_REVISION")
            .ok()
            .filter(|revision| !revision.trim().is_empty()),
        database,
        engine: if state.stations.first().engine.is_connected() {
            "ok"
        } else {
            "unavailable"
        },
        clock: heron_service::clock::status(),
        server_time_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            }),
    })
}

/// Import and analysis progress. No sign-in needed.
#[utoipa::path(get, path = "/library/status", tag = "library", security(()),
    responses((status = 200, body = LibraryStatus), (status = "5XX", body = ErrorBody)))]
async fn library_status(State(state): State<AppState>) -> ApiResult<Json<LibraryStatus>> {
    state.library.status().await.map(Json).map_err(|error| {
        tracing::warn!(%error, "cannot read the library status");
        ApiError::Internal(error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `$ref` points at a schema of the document: clients are
    /// generated from it.
    #[test]
    fn the_openapi_document_is_complete() {
        let document = serde_json::to_value(openapi()).unwrap();
        let schemas = document["components"]["schemas"].as_object().unwrap();
        let mut stack = vec![&document];
        let mut refs = 0;
        while let Some(value) = stack.pop() {
            match value {
                serde_json::Value::Object(object) => {
                    if let Some(serde_json::Value::String(target)) = object.get("$ref") {
                        refs += 1;
                        let name = target.trim_start_matches("#/components/schemas/");
                        assert!(schemas.contains_key(name), "dangling {target}");
                    }
                    stack.extend(object.values());
                }
                serde_json::Value::Array(values) => stack.extend(values),
                _ => {}
            }
        }
        assert!(refs > 50);
        let mut operations = std::collections::HashSet::new();
        for path in document["paths"].as_object().unwrap().values() {
            for operation in path.as_object().unwrap().values() {
                let id = operation["operationId"].as_str().unwrap();
                assert!(
                    operations.insert(id.to_owned()),
                    "operationId {id} used twice"
                );
            }
        }
        assert!(document["paths"]["/api/v1/live"]["get"].is_object());
        assert!(document["paths"]["/api/v1/stations/{station_id}/queue"]["get"].is_object());
        assert!(document["paths"]["/api/v1/public/stations/{code}/now-playing"]["get"].is_object());
        assert!(document["paths"]["/api/v1/stations/{station_id}/tracks"].is_null());
    }
}
