// SPDX-License-Identifier: AGPL-3.0-or-later

//! The routes of `heron-public`, the shows' public gateway
//! (`docs/listener-interaction.md`), under `/api/v1/gateway/…`.
//!
//! They accept only the gateway token (`HERON_PUBLIC_TOKEN`), which is no
//! user's and reaches nothing else: [`GatewayClient`] checks it. A show's
//! writes go under `/gateway/stations/{code}/{module}/…`, and start with
//! [`open_show`], which checks again that the show is on and open.

use std::sync::Arc;
use std::time::SystemTime;

use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use heron_core::Module;
use heron_core::shows::{Show, ShowPublic, ShowSettings};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::engine::unix_ms;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::public::NowPlaying;
use crate::stations::{Station, StationRuntime};

/// The header carrying the device's hash, from the gateway: a keyed hash of
/// the listener's address and browser, never the address.
pub const DEVICE_HEADER: &str = "x-heron-device";

/// The hash of the gateway token, compared with what is sent.
pub type GatewayToken = Option<Arc<blake3::Hash>>;

/// The gateway token from the environment; `None` when unset.
pub fn token_from(value: Option<&str>) -> GatewayToken {
    value
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| Arc::new(blake3::hash(token.as_bytes())))
}

/// `heron-public`, authenticated with the gateway token. `401` otherwise.
#[derive(Clone, Debug)]
pub struct GatewayClient {
    /// The device's hash, on a listener's write.
    // Read by the shows' writes sent on one by one (#131).
    #[allow(dead_code)]
    pub device: Option<String>,
}

impl FromRequestParts<AppState> for GatewayClient {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let expected = state.gateway_token.as_ref().ok_or(ApiError::Unauthorized)?;
        let sent = crate::auth::bearer_token(&parts.headers).ok_or(ApiError::Unauthorized)?;
        // `blake3::Hash` compares in constant time.
        if blake3::hash(sent.as_bytes()) != **expected {
            return Err(ApiError::Unauthorized);
        }
        let device = parts
            .headers
            .get(DEVICE_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 128)
            .map(str::to_owned);
        Ok(Self { device })
    }
}

/// An enabled station's show, on and open now: what a show's write needs.
/// `404` for an unknown station, a show that is not one or is off, `409`
/// while it is closed.
pub async fn open_show(
    state: &AppState,
    code: &str,
    module: Module,
) -> ApiResult<(Arc<StationRuntime>, ShowSettings)> {
    let station = state
        .stations
        .by_code(code)
        .filter(|station| station.info().enabled)
        .ok_or_else(|| ApiError::not_found(format!("station {code}")))?;
    let show = Show::of(module).ok_or_else(|| ApiError::not_found(module.name()))?;
    if !crate::modules::is_on(&state.pool, Some(station.id()), module).await? {
        return Err(ApiError::ModuleOff(module));
    }
    let settings = crate::shows::load(&state.pool, station.id(), show).await?;
    let timezone: crate::settings::StationSettings = crate::settings::load_for(
        &state.pool,
        station.id(),
        crate::settings::StationSettings::SETTINGS_NAME,
    )
    .await?;
    let status =
        ShowPublic::new(module, &settings, &timezone.timezone).status(jiff::Timestamp::now());
    if !status.open {
        return Err(ApiError::Conflict(format!("{} is closed", show.name)));
    }
    Ok((station, settings))
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(gateway_state))
}

/// A station as the gateway serves it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct GatewayStation {
    pub code: String,
    pub name: String,
    /// IANA time zone, which the shows' hours follow.
    pub timezone: String,
    /// As `GET /public/now-playing` gives it.
    pub now_playing: NowPlaying,
    /// The shows on for the station.
    pub shows: Vec<ShowPublic>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct GatewayState {
    /// The enabled stations.
    pub stations: Vec<GatewayStation>,
    pub server_time_unix_ms: u64,
}

/// What the gateway serves listeners: every enabled station, its now
/// playing and its shows. Read once a second by `heron-public`, with the
/// gateway token only.
#[utoipa::path(get, path = "/gateway/state", tag = "gateway", security(()),
    responses((status = 200, body = GatewayState), (status = "4XX", body = ErrorBody)))]
async fn gateway_state(
    _gateway: GatewayClient,
    State(state): State<AppState>,
) -> ApiResult<Json<GatewayState>> {
    let mut stations = Vec::new();
    for runtime in state.stations.all() {
        let info = runtime.info();
        if !info.enabled {
            continue;
        }
        let timezone: crate::settings::StationSettings = crate::settings::load_for(
            &state.pool,
            runtime.id(),
            crate::settings::StationSettings::SETTINGS_NAME,
        )
        .await?;
        let station = Station(Arc::clone(&runtime));
        stations.push(GatewayStation {
            code: info.code,
            name: info.name,
            timezone: timezone.timezone,
            now_playing: crate::public::now_playing_of(&state, &station).await,
            shows: crate::shows::public_shows(&state.pool, runtime.id()).await?,
        });
    }
    Ok(Json(GatewayState {
        stations,
        server_time_unix_ms: unix_ms(SystemTime::now()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_token_is_no_token() {
        assert!(token_from(None).is_none());
        assert!(token_from(Some("  ")).is_none());
        let token = token_from(Some(" secret ")).unwrap();
        assert_eq!(*token, blake3::hash(b"secret"));
    }
}
