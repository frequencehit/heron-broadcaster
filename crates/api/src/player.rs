// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decks, transport, FX player and queue: the engine's actions, for
//! operators. Reading is open to every role.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use heron_core::PlaySource;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::engine::{Levels, PlayerState, QueueEntry};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::matches;
use crate::stations::Station;
use crate::sync::SyncStatus;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(engine_status))
        .routes(routes!(get_player))
        .routes(routes!(get_levels))
        .routes(routes!(skip))
        .routes(routes!(pause))
        .routes(routes!(resume))
        .routes(routes!(play_fx, stop_fx))
        .routes(routes!(get_queue, enqueue, clear_queue))
        .routes(routes!(remove_entry))
        .routes(routes!(move_entry))
}

/// Whether the engine is reachable and runs the stored configuration.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EngineStatus {
    pub connected: bool,
    pub started_at_unix_ms: Option<u64>,
    /// Playout settings and outputs.
    pub config: SyncStatus,
}

#[utoipa::path(get, path = "/engine", tag = "player",
    responses((status = 200, body = EngineStatus), (status = "4XX", body = ErrorBody)))]
async fn engine_status(_user: CurrentUser, station: Station) -> Json<EngineStatus> {
    let started_at = station
        .engine
        .state()
        .borrow()
        .as_ref()
        .map(|player| player.engine_started_at_unix_ms);
    Json(EngineStatus {
        connected: started_at.is_some(),
        started_at_unix_ms: started_at,
        config: station.sync.status(),
    })
}

/// Decks, queue and outputs, read from the engine now.
#[utoipa::path(get, path = "/player", tag = "player",
    responses((status = 200, body = PlayerState), (status = "4XX", body = ErrorBody),
              (status = 503, body = ErrorBody, description = "Engine unreachable")))]
async fn get_player(_user: CurrentUser, station: Station) -> ApiResult<Json<PlayerState>> {
    Ok(Json(station.engine.fetch_state().await?))
}

/// Peak and RMS levels (dBFS) of the master bus and each deck.
#[utoipa::path(get, path = "/player/levels", tag = "player",
    responses((status = 200, body = Levels), (status = "4XX", body = ErrorBody),
              (status = 503, body = ErrorBody, description = "Engine unreachable")))]
async fn get_levels(_user: CurrentUser, station: Station) -> ApiResult<Json<Levels>> {
    Ok(Json(station.engine.fetch_levels().await?))
}

/// Fades out the track on air and starts the next one. The vote's track
/// of a match whose vote is open: the vote closes first, with the votes so
/// far, and the winner starts.
#[utoipa::path(post, path = "/player/skip", tag = "player",
    responses((status = 204), (status = "4XX", body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn skip(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    matches::close_before_skip(&state.pool, &state.webhooks, &station, user.id).await?;
    station.engine.skip().await?;
    tracing::info!(user = %user.username, "skip");
    Ok(StatusCode::NO_CONTENT)
}

/// Fades out and holds the decks; the stream goes on with silence.
#[utoipa::path(post, path = "/player/pause", tag = "player",
    responses((status = 204), (status = "4XX", body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn pause(user: CurrentUser, station: Station) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    station.engine.pause().await?;
    tracing::info!(user = %user.username, "pause");
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/player/resume", tag = "player",
    responses((status = 204), (status = "4XX", body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn resume(user: CurrentUser, station: Station) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    station.engine.resume().await?;
    tracing::info!(user = %user.username, "resume");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayFxRequest {
    pub track_id: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PlayFxResponse {
    pub entry_id: u64,
}

/// Plays a library track on the FX player, over the music. Replaces the
/// FX playing, if any.
#[utoipa::path(post, path = "/player/fx", tag = "player",
    request_body = PlayFxRequest,
    responses((status = 200, body = PlayFxResponse), (status = "4XX", body = ErrorBody),
              (status = 503, body = ErrorBody)))]
async fn play_fx(
    user: CurrentUser,
    station: Station,
    Json(request): Json<PlayFxRequest>,
) -> ApiResult<Json<PlayFxResponse>> {
    user.require(Role::Operator)?;
    let mut tracks = station
        .queue
        .tracks(&[request.track_id], PlaySource::Manual)
        .await?;
    let track = tracks.pop().ok_or_else(|| ApiError::not_found("track"))?;
    let entry_id = station.engine.play_fx(track).await?;
    Ok(Json(PlayFxResponse { entry_id }))
}

/// Fades out the FX player.
#[utoipa::path(delete, path = "/player/fx", tag = "player",
    responses((status = 204), (status = "4XX", body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn stop_fx(user: CurrentUser, station: Station) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    station.engine.stop_fx().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The tracks waiting to play, in order: the cued one first.
#[utoipa::path(get, path = "/queue", tag = "queue",
    responses((status = 200, body = Vec<QueueEntry>), (status = "4XX", body = ErrorBody),
              (status = 503, body = ErrorBody)))]
async fn get_queue(_user: CurrentUser, station: Station) -> ApiResult<Json<Vec<QueueEntry>>> {
    let player = station.engine.fetch_state().await?;
    Ok(Json(player.waiting().cloned().collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EnqueueRequest {
    /// Library tracks, played in this order.
    pub track_ids: Vec<u64>,
    /// Where among the waiting tracks, 0 being next. At the end when missing.
    pub position: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EnqueueResponse {
    pub entry_ids: Vec<u64>,
}

/// Queues library tracks. Missing files and tracks with nothing audible are
/// refused, and so is a position among a match's tracks (`409`).
#[utoipa::path(post, path = "/queue", tag = "queue",
    request_body = EnqueueRequest,
    responses((status = 200, body = EnqueueResponse), (status = "4XX", body = ErrorBody),
              (status = 503, body = ErrorBody)))]
async fn enqueue(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(request): Json<EnqueueRequest>,
) -> ApiResult<Json<EnqueueResponse>> {
    user.require(Role::Operator)?;
    if request.track_ids.is_empty() || request.track_ids.len() > 500 {
        return Err(ApiError::BadRequest(
            "queue 1 to 500 tracks at a time".to_owned(),
        ));
    }
    if let Some(position) = request.position {
        let player = station.engine.fetch_state().await?;
        let lock = matches::lock(&state.pool, station.id(), &player).await?;
        if lock.is_some_and(|lock| lock.refuses(position, None)) {
            return Err(matches::between());
        }
    }
    let tracks = station
        .queue
        .tracks(&request.track_ids, PlaySource::Manual)
        .await?;
    let entry_ids = station
        .engine
        .enqueue(tracks, request.position, Vec::new())
        .await?;
    tracing::info!(user = %user.username, tracks = ?request.track_ids, "queued");
    Ok(Json(EnqueueResponse { entry_ids }))
}

/// Empties the queue, cued track included. The track on air plays on (and
/// autofill, when on, queues new tracks). The tracks of a match waiting or
/// open stay, and the match goes on.
#[utoipa::path(delete, path = "/queue", tag = "queue",
    responses((status = 204), (status = "4XX", body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn clear_queue(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let player = station.engine.fetch_state().await?;
    let Some(lock) = matches::lock(&state.pool, station.id(), &player).await? else {
        station.engine.clear().await?;
        tracing::info!(user = %user.username, "queue cleared");
        return Ok(StatusCode::NO_CONTENT);
    };
    for entry in player.waiting().filter(|entry| !lock.holds(entry.entry_id)) {
        match station.engine.remove(entry.entry_id).await {
            // Gone with its linked track, or started meanwhile.
            Ok(()) | Err(ApiError::NotFound(_) | ApiError::Invalid(_)) => {}
            Err(error) => return Err(error),
        }
    }
    tracing::info!(user = %user.username, "queue cleared, the match's tracks kept");
    Ok(StatusCode::NO_CONTENT)
}

/// Takes an entry out of the queue. The track on air cannot be removed: skip
/// it. A match's track is refused while the match waits or its vote is
/// open (`409`): cancel the match first.
#[utoipa::path(delete, path = "/queue/{entry_id}", tag = "queue",
    params(("entry_id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn remove_entry(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(entry_id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let player = station.engine.fetch_state().await?;
    let lock = matches::lock(&state.pool, station.id(), &player).await?;
    if lock.is_some_and(|lock| lock.holds(entry_id)) {
        return Err(matches::locked());
    }
    station.engine.remove(entry_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    /// New place among the waiting tracks, 0 being next.
    pub position: u32,
}

/// Moves a waiting entry. A match's track, or a place among them, is
/// refused while the match waits or its vote is open (`409`).
#[utoipa::path(put, path = "/queue/{entry_id}/position", tag = "queue",
    params(("entry_id" = u64, Path)), request_body = MoveRequest,
    responses((status = 204), (status = "4XX", body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn move_entry(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(entry_id): Path<u64>,
    Json(request): Json<MoveRequest>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let player = station.engine.fetch_state().await?;
    if let Some(lock) = matches::lock(&state.pool, station.id(), &player).await? {
        if lock.holds(entry_id) {
            return Err(matches::locked());
        }
        if lock.refuses(request.position, Some(entry_id)) {
            return Err(matches::between());
        }
    }
    station
        .engine
        .move_entry(entry_id, request.position)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
