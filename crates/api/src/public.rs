// SPDX-License-Identifier: AGPL-3.0-or-later

//! Public now-playing and history (`/api/v1/public/…`), for station
//! websites, apps and displays: no sign-in, any origin (CORS), the music
//! only (decks A and B, not the FX player, and not tracks whose category
//! type sends no metadata, such as jingles), without IDs or paths.

use std::time::SystemTime;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{Method, header};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::engine::{DeckName, unix_ms};
use crate::error::{ApiError, ApiResult, ErrorBody};

/// What plays on air.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PublicTrack {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub started_at_unix_ms: u64,
    /// From the cue-in to the cue-out.
    pub duration_ms: u64,
    pub position_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct NowPlaying {
    /// Missing while nothing plays or the engine is unreachable.
    pub now: Option<PublicTrack>,
    /// To tell how far the track has gone since.
    pub server_time_unix_ms: u64,
}

/// One play.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct PublicPlay {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub started_at_unix_ms: i64,
    /// Missing while the track plays.
    pub ended_at_unix_ms: Option<i64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PublicHistoryQuery {
    /// Plays started before this time (Unix ms), for the next page.
    pub before: Option<i64>,
    /// 1 to 50, 20 by default.
    pub limit: Option<u32>,
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(now_playing))
        .routes(routes!(public_history))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET]),
        )
}

/// Tracks of these categories send no metadata (jingles, ads).
const NO_METADATA: &str = "EXISTS (SELECT 1 FROM track_categories tc
        JOIN categories c ON c.id = tc.category_id
        JOIN category_types ct ON ct.id = c.type_id
        WHERE tc.track_id = h.track_id AND NOT ct.sends_metadata)";

/// The track on air; while a track that sends no metadata (a jingle) is on
/// air, the last one that does. No sign-in needed; any website may read it.
#[utoipa::path(get, path = "/public/now-playing", tag = "public", security(()),
    responses((status = 200, body = NowPlaying)))]
async fn now_playing(
    State(state): State<AppState>,
) -> ([(header::HeaderName, &'static str); 1], Json<NowPlaying>) {
    let now = unix_ms(SystemTime::now());
    let mut jingle = false;
    let track = state.engine.state().borrow().as_ref().and_then(|player| {
        player
            .decks
            .iter()
            .filter(|deck| deck.deck != DeckName::Fx && deck.on_air)
            .find_map(|deck| {
                let track = &deck.entry.as_ref()?.track;
                if track.no_metadata {
                    jingle = true;
                    return None;
                }
                Some(PublicTrack {
                    artist: track.artist.clone(),
                    title: track.title.clone(),
                    album: track.album.clone(),
                    started_at_unix_ms: now.saturating_sub(deck.position_ms),
                    duration_ms: deck.length_ms,
                    position_ms: deck.position_ms,
                })
            })
    });
    let track = match track {
        Some(track) => Some(track),
        None if jingle => last_song(&state, now).await.unwrap_or_else(|error| {
            tracing::warn!(%error, "cannot read the last song");
            None
        }),
        None => None,
    };
    (
        [(header::CACHE_CONTROL, "public, max-age=5")],
        Json(NowPlaying {
            now: track,
            server_time_unix_ms: now,
        }),
    )
}

/// What was played, most recent first, the FX player and tracks that send no
/// metadata (jingles) left out. No sign-in needed; any website may read it.
#[utoipa::path(get, path = "/public/history", tag = "public", security(()),
    params(PublicHistoryQuery),
    responses((status = 200, body = Vec<PublicPlay>), (status = "4XX", body = ErrorBody)))]
async fn public_history(
    State(state): State<AppState>,
    Query(query): Query<PublicHistoryQuery>,
) -> ApiResult<(
    [(header::HeaderName, &'static str); 1],
    Json<Vec<PublicPlay>>,
)> {
    let limit = query.limit.unwrap_or(20);
    if !(1..=50).contains(&limit) {
        return Err(ApiError::BadRequest("limit is 1 to 50".to_owned()));
    }
    let mut sql = sqlx::QueryBuilder::new(
        "SELECT COALESCE(h.artist, t.artist) AS artist, COALESCE(h.title, t.title) AS title,
                t.album,
                CAST(UNIX_TIMESTAMP(h.started_at) * 1000 AS SIGNED) AS started_at_unix_ms,
                CAST(UNIX_TIMESTAMP(h.ended_at) * 1000 AS SIGNED) AS ended_at_unix_ms
         FROM play_history h LEFT JOIN tracks t ON t.id = h.track_id
         WHERE h.deck <> 'fx' AND NOT ",
    );
    sql.push(NO_METADATA);
    if let Some(before) = query.before {
        sql.push(" AND h.started_at < FROM_UNIXTIME(")
            .push_bind(before as f64 / 1_000.0)
            .push(")");
    }
    sql.push(" ORDER BY h.started_at DESC, h.id DESC LIMIT ")
        .push_bind(limit);
    let plays = sql.build_query_as().fetch_all(&state.pool).await?;
    Ok(([(header::CACHE_CONTROL, "public, max-age=15")], Json(plays)))
}

/// The last song (a track that sends metadata) that played on the decks,
/// shown while a jingle plays. Its position may be past its end: it ended.
async fn last_song(state: &AppState, now: u64) -> ApiResult<Option<PublicTrack>> {
    type Row = (
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        Option<u32>,
    );
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT COALESCE(h.artist, t.artist), COALESCE(h.title, t.title), t.album,
                    CAST(UNIX_TIMESTAMP(h.started_at) * 1000 AS SIGNED), t.duration_ms
             FROM play_history h LEFT JOIN tracks t ON t.id = h.track_id
             WHERE h.deck <> 'fx' AND NOT {NO_METADATA}
             ORDER BY h.started_at DESC, h.id DESC LIMIT 1"
    )))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.map(|(artist, title, album, started, duration_ms)| {
        let started = u64::try_from(started).unwrap_or(0);
        PublicTrack {
            artist,
            title,
            album,
            started_at_unix_ms: started,
            duration_ms: u64::from(duration_ms.unwrap_or(0)),
            position_ms: now.saturating_sub(started),
        }
    }))
}
