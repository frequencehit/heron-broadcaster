// SPDX-License-Identifier: AGPL-3.0-or-later

//! Public now-playing, queue and history (`/api/v1/public/…`), for station
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
use crate::engine::{DeckName, PlayerState, unix_ms};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::stations::Station;

/// What plays on air.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PublicTrack {
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    /// Its cover, else the station's logo (docs/api.md#public-endpoints).
    pub cover_url: Option<String>,
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
    #[serde(skip)]
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    /// Its cover, else the station's logo.
    #[sqlx(skip)]
    pub cover_url: Option<String>,
    pub started_at_unix_ms: i64,
    /// Missing while the track plays.
    pub ended_at_unix_ms: Option<i64>,
}

/// A track waiting in the queue.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct UpcomingTrack {
    /// 0 for the next one.
    pub position: u32,
    #[serde(skip)]
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    /// Its cover, else the station's logo; filled by [`fill_covers`].
    pub cover_url: Option<String>,
    /// From the cue-in to the cue-out.
    pub duration_ms: u64,
    /// When it should start, without crossfades, as the Queue page shows it.
    pub planned_start_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PublicQueue {
    /// In airing order; empty while the engine is unreachable.
    pub upcoming: Vec<UpcomingTrack>,
    pub server_time_unix_ms: u64,
}

/// The most upcoming tracks sent or shown.
pub const MAX_UPCOMING: usize = 10;

/// The tracks waiting, in airing order: the music only (no tracks that send
/// no metadata), at most [`MAX_UPCOMING`], with their planned start. Each
/// comes with its queue entry ID, to tell a change.
pub fn upcoming(player: &PlayerState, now_ms: u64) -> Vec<(u64, UpcomingTrack)> {
    let now = i64::try_from(now_ms).unwrap_or(i64::MAX);
    let (planned, _) = crate::rotation::planned_starts(player, now);
    planned
        .into_iter()
        .filter(|(entry, _)| !entry.track.no_metadata)
        .take(MAX_UPCOMING)
        .enumerate()
        .map(|(position, (entry, at))| {
            let track = &entry.track;
            (
                entry.entry_id,
                UpcomingTrack {
                    position: position as u32,
                    track_id: track.track_id,
                    artist: track.artist.clone(),
                    title: track.title.clone(),
                    album: track.album.clone(),
                    cover_url: None,
                    duration_ms: u64::try_from(crate::rotation::planned_length_ms(entry))
                        .unwrap_or(0),
                    planned_start_unix_ms: u64::try_from(at).unwrap_or(0),
                },
            )
        })
        .collect()
}

/// Gives upcoming tracks their cover addresses. Without them when the
/// database cannot be read: the list matters more.
pub async fn fill_covers(pool: &sqlx::MySqlPool, station_id: u64, tracks: &mut [UpcomingTrack]) {
    let ids: Vec<u64> = tracks.iter().filter_map(|track| track.track_id).collect();
    match crate::covers::PublicCovers::load(pool, station_id, &ids).await {
        Ok(covers) => {
            for track in tracks {
                track.cover_url = covers.url(track.track_id);
            }
        }
        Err(error) => tracing::warn!(%error, "cannot read the covers"),
    }
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
        .routes(routes!(public_queue))
        .merge(crate::covers::public_router())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET]),
        )
}

/// Tracks of these categories send no metadata (jingles, ads).
const NO_METADATA: &str = "(EXISTS (SELECT 1 FROM track_categories tc
        JOIN categories c ON c.id = tc.category_id
        JOIN category_types ct ON ct.id = c.type_id
        WHERE tc.track_id = h.track_id AND NOT ct.sends_metadata)
    OR EXISTS (SELECT 1 FROM medleys md WHERE md.track_id = h.track_id))";

/// The track on air; while a track that sends no metadata (a jingle) is on
/// air, the last one that does. No sign-in needed; any website may read it.
#[utoipa::path(get, path = "/public/now-playing", tag = "public", security(()),
    responses((status = 200, body = NowPlaying)))]
async fn now_playing(
    State(state): State<AppState>,
    station: Station,
) -> ([(header::HeaderName, &'static str); 1], Json<NowPlaying>) {
    let now = unix_ms(SystemTime::now());
    let mut jingle = false;
    let mut track_id = None;
    let track = station.engine.state().borrow().as_ref().and_then(|player| {
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
                track_id = track.track_id;
                Some(PublicTrack {
                    artist: track.artist.clone(),
                    title: track.title.clone(),
                    album: track.album.clone(),
                    cover_url: None,
                    started_at_unix_ms: now.saturating_sub(deck.position_ms),
                    duration_ms: deck.length_ms,
                    position_ms: deck.position_ms,
                })
            })
    });
    let mut track = match track {
        Some(track) => Some(track),
        None if jingle => match last_song(&state, &station, now).await {
            Ok(Some((id, track))) => {
                track_id = id;
                Some(track)
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, "cannot read the last song");
                None
            }
        },
        None => None,
    };
    if let Some(track) = &mut track {
        let ids: Vec<u64> = track_id.into_iter().collect();
        match crate::covers::PublicCovers::load(&state.pool, station.id(), &ids).await {
            Ok(covers) => track.cover_url = covers.url(track_id),
            Err(error) => tracing::warn!(%error, "cannot read the cover"),
        }
    }
    (
        [(header::CACHE_CONTROL, "public, max-age=5")],
        Json(NowPlaying {
            now: track,
            server_time_unix_ms: now,
        }),
    )
}

/// What comes next, in airing order: the music only (no FX player, no
/// tracks that send no metadata), at most 10. No sign-in needed; any
/// website may read it.
#[utoipa::path(get, path = "/public/queue", tag = "public", security(()),
    responses((status = 200, body = PublicQueue)))]
async fn public_queue(
    State(state): State<AppState>,
    station: Station,
) -> ([(header::HeaderName, &'static str); 1], Json<PublicQueue>) {
    let now = unix_ms(SystemTime::now());
    let mut upcoming: Vec<UpcomingTrack> = station
        .engine
        .state()
        .borrow()
        .as_ref()
        .map(|player| {
            upcoming(player, now)
                .into_iter()
                .map(|(_, track)| track)
                .collect()
        })
        .unwrap_or_default();
    fill_covers(&state.pool, station.id(), &mut upcoming).await;
    (
        [(header::CACHE_CONTROL, "public, max-age=5")],
        Json(PublicQueue {
            upcoming,
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
    station: Station,
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
        "SELECT h.track_id, COALESCE(h.artist, t.artist) AS artist,
                COALESCE(h.title, t.title) AS title, t.album,
                CAST(UNIX_TIMESTAMP(h.started_at) * 1000 AS SIGNED) AS started_at_unix_ms,
                CAST(UNIX_TIMESTAMP(h.ended_at) * 1000 AS SIGNED) AS ended_at_unix_ms
         FROM play_history h LEFT JOIN tracks t ON t.id = h.track_id
         WHERE h.deck <> 'fx' AND h.station_id = ",
    );
    sql.push_bind(station.id())
        .push(" AND NOT ")
        .push(NO_METADATA);
    if let Some(before) = query.before {
        sql.push(" AND h.started_at < FROM_UNIXTIME(")
            .push_bind(before as f64 / 1_000.0)
            .push(")");
    }
    sql.push(" ORDER BY h.started_at DESC, h.id DESC LIMIT ")
        .push_bind(limit);
    let mut plays: Vec<PublicPlay> = sql.build_query_as().fetch_all(&state.pool).await?;
    let ids: Vec<u64> = plays.iter().filter_map(|play| play.track_id).collect();
    let covers = crate::covers::PublicCovers::load(&state.pool, station.id(), &ids).await?;
    for play in &mut plays {
        play.cover_url = covers.url(play.track_id);
    }
    Ok(([(header::CACHE_CONTROL, "public, max-age=15")], Json(plays)))
}

/// The last song (a track that sends metadata) that played on the decks,
/// shown while a jingle plays, with its track. Its position may be past
/// its end: it ended.
async fn last_song(
    state: &AppState,
    station: &Station,
    now: u64,
) -> ApiResult<Option<(Option<u64>, PublicTrack)>> {
    type Row = (
        Option<u64>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        Option<u32>,
    );
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT h.track_id, COALESCE(h.artist, t.artist), COALESCE(h.title, t.title), t.album,
                    CAST(UNIX_TIMESTAMP(h.started_at) * 1000 AS SIGNED), t.duration_ms
             FROM play_history h LEFT JOIN tracks t ON t.id = h.track_id
             WHERE h.deck <> 'fx' AND h.station_id = ? AND NOT {NO_METADATA}
             ORDER BY h.started_at DESC, h.id DESC LIMIT 1"
    )))
    .bind(station.id())
    .fetch_optional(&state.pool)
    .await?;
    Ok(
        row.map(|(track_id, artist, title, album, started, duration_ms)| {
            let started = u64::try_from(started).unwrap_or(0);
            let track = PublicTrack {
                artist,
                title,
                album,
                cover_url: None,
                started_at_unix_ms: started,
                duration_ms: u64::from(duration_ms.unwrap_or(0)),
                position_ms: now.saturating_sub(started),
            };
            (track_id, track)
        }),
    )
}

#[cfg(test)]
mod tests {
    use heron_core::PlaySource;

    use super::*;
    use crate::engine::{DeckState, QueueEntry, QueuedTrack};

    fn entry(entry_id: u64, title: &str, no_metadata: bool) -> QueueEntry {
        QueueEntry {
            entry_id,
            track: QueuedTrack {
                track_id: Some(entry_id),
                path: format!("{title}.mp3"),
                title: Some(title.to_owned()),
                artist: Some("Artist".to_owned()),
                album: None,
                duration_ms: 200_000,
                cue_in_ms: 1_000,
                cue_out_ms: 181_000,
                source: PlaySource::Rotation,
                no_metadata,
                over_intro: false,
                intro_ms: None,
                start_at_unix_ms: None,
                continues: false,
            },
            linked_to: None,
        }
    }

    #[test]
    fn the_upcoming_music_comes_in_airing_order_with_its_planned_start() {
        let mut queue: Vec<QueueEntry> = (3..20)
            .map(|id| entry(id, &format!("Song {id}"), false))
            .collect();
        queue.insert(0, entry(2, "Jingle", true));
        let player = PlayerState {
            paused: false,
            decks: vec![DeckState {
                deck: DeckName::A,
                entry: Some(entry(1, "On air", false)),
                position_ms: 100_000,
                length_ms: 180_000,
                on_air: true,
            }],
            cued: None,
            cued_jingle: None,
            pending_jingle: None,
            queue,
            outputs: Vec::new(),
            rendered_ms: 0,
            underruns: 0,
            engine_started_at_unix_ms: 0,
        };
        let upcoming = upcoming(&player, 1_000_000);
        assert_eq!(upcoming.len(), MAX_UPCOMING);
        // The jingle is left out; positions count the music only.
        let (id, first) = &upcoming[0];
        assert_eq!(
            (*id, first.position, first.title.as_deref()),
            (3, 0, Some("Song 3"))
        );
        assert_eq!(first.duration_ms, 180_000);
        // After what the deck still plays, and the jingle before it.
        assert_eq!(first.planned_start_unix_ms, 1_000_000 + 80_000 + 180_000);
        assert_eq!(
            upcoming[1].1.planned_start_unix_ms,
            first.planned_start_unix_ms + 180_000
        );
    }
}
