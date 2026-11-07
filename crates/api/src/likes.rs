// SPDX-License-Identifier: AGPL-3.0-or-later

//! Likes (`docs/listener-interaction.md#likes`): listeners like or dislike
//! the track playing, through the gateway.
//!
//! Each device's vote on a play is kept (`play_vote_devices`) so that it
//! can change, and the play's likes and dislikes are counted from them
//! (`play_votes`), with the play's station, track and start: a period's
//! ranking adds the plays up. The devices' votes go an hour after their
//! play ends.

use std::collections::BTreeSet;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use heron_core::Module;
use jiff::{Timestamp, ToSpan, tz::TimeZone};
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::gateway::GatewayClient;
use crate::settings::{self, StationSettings};
use crate::stations::Station;

/// Votes a track needs in a period to be ranked.
pub const MIN_VOTES: u64 = 10;
/// How long after its play ends a vote still counts: the gateway's batch
/// on its way.
const GRACE_SECONDS: u32 = 10;
/// How many tracks each end of the week's ranking gives.
const RANKED: u32 = 10;
/// The latest plays with votes on a track's page.
const LATEST_PLAYS: u32 = 20;
/// Rows written per statement.
const CHUNK: usize = 1_000;
/// How often the devices' votes are deleted.
const PURGE_EVERY: Duration = Duration::from_secs(600);

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(current))
        .routes(routes!(week))
        .routes(routes!(track_likes))
        .routes(routes!(gateway_votes))
}

/// Which plays can be voted on: music (decks A and B, a track of the
/// library that sends its metadata, not a medley), not from the fallback
/// directory.
const VOTABLE: &str = "h.deck <> 'fx' AND h.source <> 'fallback' AND h.track_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM track_categories tc
                    JOIN categories c ON c.id = tc.category_id
                    JOIN category_types ct ON ct.id = c.type_id
                    WHERE tc.track_id = h.track_id AND NOT ct.sends_metadata)
    AND NOT EXISTS (SELECT 1 FROM medleys md WHERE md.track_id = h.track_id)";

/// The play listeners can vote on now: the latest music on air.
pub async fn current_play(pool: &MySqlPool, station_id: u64) -> ApiResult<Option<(u64, u64)>> {
    let play = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT h.id, h.track_id FROM play_history h
         WHERE h.station_id = ? AND h.ended_at IS NULL AND {VOTABLE}
         ORDER BY h.started_at DESC, h.id DESC LIMIT 1"
    )))
    .bind(station_id)
    .fetch_optional(pool)
    .await?;
    Ok(play)
}

/// The show's live state for the gateway: the play to vote on, or `null`.
pub async fn public_state(pool: &MySqlPool, station_id: u64) -> ApiResult<serde_json::Value> {
    Ok(match current_play(pool, station_id).await? {
        Some((play_id, _)) => serde_json::json!({ "play_id": play_id }),
        None => serde_json::Value::Null,
    })
}

/// Likes and dislikes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, ToSchema, sqlx::FromRow)]
pub struct Votes {
    pub likes: u64,
    pub dislikes: u64,
}

/// The play on air that listeners can vote on.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LikesNow {
    /// None while nothing can be voted on (a jingle, nothing on air).
    pub play_id: Option<u64>,
    pub track_id: Option<u64>,
    pub likes: u64,
    pub dislikes: u64,
}

async fn play_votes(pool: &MySqlPool, play_id: u64) -> ApiResult<Votes> {
    let votes = sqlx::query_as("SELECT likes, dislikes FROM play_votes WHERE play_id = ?")
        .bind(play_id)
        .fetch_optional(pool)
        .await?;
    Ok(votes.unwrap_or_default())
}

/// The votes on the track playing, as they come.
#[utoipa::path(get, path = "/likes/current", tag = "likes",
    responses((status = 200, body = LikesNow), (status = "4XX", body = ErrorBody)))]
async fn current(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<LikesNow>> {
    let Some((play_id, track_id)) = current_play(&state.pool, station.id()).await? else {
        return Ok(Json(LikesNow {
            play_id: None,
            track_id: None,
            likes: 0,
            dislikes: 0,
        }));
    };
    let votes = play_votes(&state.pool, play_id).await?;
    Ok(Json(LikesNow {
        play_id: Some(play_id),
        track_id: Some(track_id),
        likes: votes.likes,
        dislikes: votes.dislikes,
    }))
}

/// A track in the week's ranking.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct RankedTrack {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub likes: u64,
    pub dislikes: u64,
    /// The share of likes, 0 to 1.
    #[sqlx(skip)]
    pub share: f64,
    /// Its plays with votes.
    pub plays: u64,
}

/// The week's most and least liked tracks.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WeekLikes {
    /// Monday 00:00, in the station's time zone.
    pub from_unix_ms: i64,
    /// Votes a track needs to be ranked.
    pub min_votes: u64,
    pub most_liked: Vec<RankedTrack>,
    pub least_liked: Vec<RankedTrack>,
}

/// The start of the week of `now`: Monday 00:00 in `tz`.
pub fn week_start(now: Timestamp, tz: &TimeZone) -> ApiResult<Timestamp> {
    let today = now.to_zoned(tz.clone()).date();
    let monday = today
        .checked_sub(i64::from(today.weekday().to_monday_zero_offset()).days())
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let start = monday
        .to_zoned(tz.clone())
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    Ok(start.timestamp())
}

/// The tracks of a station with at least `min_votes` votes on the plays
/// that started from `from`, by share of likes (highest first, or lowest
/// with `ascending`), then by votes. What the Top 30 ranks too (#134).
pub async fn ranking(
    pool: &MySqlPool,
    station_id: u64,
    from: Timestamp,
    to: Timestamp,
    min_votes: u64,
    ascending: bool,
    limit: u32,
) -> ApiResult<Vec<RankedTrack>> {
    let direction = if ascending { "" } else { " DESC" };
    let mut tracks: Vec<RankedTrack> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT v.track_id, t.artist, t.title,
                CAST(SUM(v.likes) AS UNSIGNED) AS likes,
                CAST(SUM(v.dislikes) AS UNSIGNED) AS dislikes,
                CAST(COUNT(*) AS UNSIGNED) AS plays
         FROM play_votes v JOIN tracks t ON t.id = v.track_id
         WHERE v.station_id = ? AND v.started_at >= FROM_UNIXTIME(?)
           AND v.started_at < FROM_UNIXTIME(?) AND t.deleted_at IS NULL
         GROUP BY v.track_id, t.artist, t.title
         HAVING SUM(v.likes) + SUM(v.dislikes) >= ? AND SUM(v.likes) + SUM(v.dislikes) > 0
         ORDER BY SUM(v.likes) / (SUM(v.likes) + SUM(v.dislikes)){direction},
                  SUM(v.likes) + SUM(v.dislikes) DESC, v.track_id
         LIMIT ?"
    )))
    .bind(station_id)
    .bind(from.as_millisecond() as f64 / 1_000.0)
    .bind(to.as_millisecond() as f64 / 1_000.0)
    .bind(min_votes)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    for track in &mut tracks {
        track.share = share(track.likes, track.dislikes);
    }
    Ok(tracks)
}

/// The share of likes, 0 to 1; 0 without votes.
pub fn share(likes: u64, dislikes: u64) -> f64 {
    let votes = likes + dislikes;
    if votes == 0 {
        0.0
    } else {
        likes as f64 / votes as f64
    }
}

/// The most liked and least liked tracks of the week so far, from
/// Monday 00:00 in the station's time zone, among those with at least
/// `min_votes` votes.
#[utoipa::path(get, path = "/likes/week", tag = "likes",
    responses((status = 200, body = WeekLikes), (status = "4XX", body = ErrorBody)))]
async fn week(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<WeekLikes>> {
    let pool = &state.pool;
    let station_settings: StationSettings =
        settings::load_for(pool, station.id(), StationSettings::SETTINGS_NAME).await?;
    let now = Timestamp::now();
    let from = week_start(now, &station_settings.time_zone())?;
    // Plays still to come do not count yet: up to a minute ahead.
    let to = now
        .checked_add(jiff::SignedDuration::from_mins(1))
        .unwrap_or(now);
    let ranked = |ascending| ranking(pool, station.id(), from, to, MIN_VOTES, ascending, RANKED);
    Ok(Json(WeekLikes {
        from_unix_ms: from.as_millisecond(),
        min_votes: MIN_VOTES,
        most_liked: ranked(false).await?,
        least_liked: ranked(true).await?,
    }))
}

/// A play's votes.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct PlayVotes {
    pub play_id: u64,
    pub started_at_unix_ms: i64,
    pub likes: u64,
    pub dislikes: u64,
}

/// A track's votes on a station.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TrackLikes {
    /// Since Monday 00:00, in the station's time zone.
    pub week: Votes,
    /// The last 30 days.
    pub days_30: Votes,
    pub total: Votes,
    /// Votes a track needs to be ranked.
    pub min_votes: u64,
    /// Its latest plays with votes, the latest first.
    pub plays: Vec<PlayVotes>,
}

/// A track's likes and dislikes on the station: this week, 30 days and
/// always, and its latest plays with votes.
#[utoipa::path(get, path = "/likes/tracks/{id}", tag = "likes",
    params(("id" = u64, Path, description = "The track's ID")),
    responses((status = 200, body = TrackLikes), (status = "4XX", body = ErrorBody)))]
async fn track_likes(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(track_id): Path<u64>,
) -> ApiResult<Json<TrackLikes>> {
    let pool = &state.pool;
    let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM tracks WHERE id = ?")
        .bind(track_id)
        .fetch_optional(pool)
        .await?;
    exists.ok_or_else(|| ApiError::NotFound(format!("track {track_id} not found")))?;
    let station_settings: StationSettings =
        settings::load_for(pool, station.id(), StationSettings::SETTINGS_NAME).await?;
    let now = Timestamp::now();
    let week = week_start(now, &station_settings.time_zone())?;
    let days_30 = now
        .checked_sub(jiff::SignedDuration::from_hours(24 * 30))
        .unwrap_or(now);
    let seconds = |at: Timestamp| at.as_millisecond() as f64 / 1_000.0;
    let row: (u64, u64, u64, u64, u64, u64) = sqlx::query_as(
        "SELECT CAST(COALESCE(SUM(IF(started_at >= FROM_UNIXTIME(?), likes, 0)), 0) AS UNSIGNED),
                CAST(COALESCE(SUM(IF(started_at >= FROM_UNIXTIME(?), dislikes, 0)), 0) AS UNSIGNED),
                CAST(COALESCE(SUM(IF(started_at >= FROM_UNIXTIME(?), likes, 0)), 0) AS UNSIGNED),
                CAST(COALESCE(SUM(IF(started_at >= FROM_UNIXTIME(?), dislikes, 0)), 0) AS UNSIGNED),
                CAST(COALESCE(SUM(likes), 0) AS UNSIGNED),
                CAST(COALESCE(SUM(dislikes), 0) AS UNSIGNED)
         FROM play_votes WHERE track_id = ? AND station_id = ?",
    )
    .bind(seconds(week))
    .bind(seconds(week))
    .bind(seconds(days_30))
    .bind(seconds(days_30))
    .bind(track_id)
    .bind(station.id())
    .fetch_one(pool)
    .await?;
    let plays = sqlx::query_as(
        "SELECT play_id, CAST(UNIX_TIMESTAMP(started_at) * 1000 AS SIGNED) AS started_at_unix_ms,
                CAST(likes AS UNSIGNED) AS likes, CAST(dislikes AS UNSIGNED) AS dislikes
         FROM play_votes WHERE track_id = ? AND station_id = ?
         ORDER BY started_at DESC, play_id DESC LIMIT ?",
    )
    .bind(track_id)
    .bind(station.id())
    .bind(LATEST_PLAYS)
    .fetch_all(pool)
    .await?;
    Ok(Json(TrackLikes {
        week: Votes {
            likes: row.0,
            dislikes: row.1,
        },
        days_30: Votes {
            likes: row.2,
            dislikes: row.3,
        },
        total: Votes {
            likes: row.4,
            dislikes: row.5,
        },
        min_votes: MIN_VOTES,
        plays,
    }))
}

/// Like or dislike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Vote {
    Like,
    Dislike,
}

/// What a listener sent.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct LikeFields {
    pub play_id: u64,
    pub vote: Vote,
}

/// A device's last vote.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct DeviceVote {
    /// The gateway's hash of the device.
    pub device: String,
    pub fields: LikeFields,
}

/// Each device's last vote since the last batch.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct VoteBatch {
    pub writes: Vec<DeviceVote>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct VotesTaken {
    /// The votes counted.
    pub counted: u64,
    /// Those dropped: their play ended, is not music, or is another
    /// station's.
    pub dropped: u64,
}

/// Each device's last like or dislike, from the gateway, every second. A
/// vote counts while its play is on air and up to 10 seconds after; a
/// device's vote on a play replaces its earlier one.
#[utoipa::path(post, path = "/gateway/stations/{code}/likes/vote", tag = "gateway", security(()),
    params(("code" = String, Path, description = "The station's code")),
    request_body = VoteBatch,
    responses((status = 200, body = VotesTaken), (status = "4XX", body = ErrorBody)))]
async fn gateway_votes(
    _gateway: GatewayClient,
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(batch): Json<VoteBatch>,
) -> ApiResult<Json<VotesTaken>> {
    let (station, _) = crate::gateway::open_show(&state, &code, Module::Likes).await?;
    let taken = count_votes(&state.pool, station.id(), &batch.writes).await?;
    Ok(Json(taken))
}

/// Keeps each device's vote and counts the plays' votes again.
pub async fn count_votes(
    pool: &MySqlPool,
    station_id: u64,
    writes: &[DeviceVote],
) -> ApiResult<VotesTaken> {
    let asked: BTreeSet<u64> = writes.iter().map(|write| write.fields.play_id).collect();
    let mut open = BTreeSet::new();
    for ids in asked.iter().copied().collect::<Vec<_>>().chunks(CHUNK) {
        let mut query = QueryBuilder::new("SELECT h.id FROM play_history h WHERE h.station_id = ");
        query.push_bind(station_id).push(format!(
            " AND (h.ended_at IS NULL OR h.ended_at >= NOW(6) - INTERVAL {GRACE_SECONDS} SECOND)
              AND {VOTABLE} AND h.id IN ("
        ));
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        query.push(")");
        let found: Vec<u64> = query.build_query_scalar().fetch_all(pool).await?;
        open.extend(found);
    }
    let valid = |device: &str| {
        !device.is_empty() && device.len() <= 128 && device.bytes().all(|b| b.is_ascii_graphic())
    };
    let counted: Vec<&DeviceVote> = writes
        .iter()
        .filter(|write| open.contains(&write.fields.play_id) && valid(&write.device))
        .collect();
    let dropped = (writes.len() - counted.len()) as u64;
    if counted.is_empty() {
        return Ok(VotesTaken {
            counted: 0,
            dropped,
        });
    }
    let mut tx = pool.begin().await?;
    for chunk in counted.chunks(CHUNK) {
        let mut query =
            QueryBuilder::new("INSERT INTO play_vote_devices (play_id, device, vote, voted_at) ");
        query.push_values(chunk, |mut row, write| {
            row.push_bind(write.fields.play_id)
                .push_bind(&write.device)
                .push_bind(match write.fields.vote {
                    Vote::Like => "like",
                    Vote::Dislike => "dislike",
                })
                .push("NOW(6)");
        });
        query.push(" ON DUPLICATE KEY UPDATE vote = VALUES(vote), voted_at = VALUES(voted_at)");
        query.build().execute(&mut *tx).await?;
    }
    let plays: BTreeSet<u64> = counted.iter().map(|write| write.fields.play_id).collect();
    for ids in plays.into_iter().collect::<Vec<_>>().chunks(CHUNK) {
        let mut query = QueryBuilder::new(
            "INSERT INTO play_votes (play_id, station_id, track_id, started_at, likes, dislikes)
             SELECT h.id, h.station_id, h.track_id, h.started_at,
                    SUM(d.vote = 'like'), SUM(d.vote = 'dislike')
             FROM play_history h JOIN play_vote_devices d ON d.play_id = h.id
             WHERE h.id IN (",
        );
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        query.push(
            ") GROUP BY h.id, h.station_id, h.track_id, h.started_at
             ON DUPLICATE KEY UPDATE likes = VALUES(likes), dislikes = VALUES(dislikes)",
        );
        query.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(VotesTaken {
        counted: counted.len() as u64,
        dropped,
    })
}

/// Deletes the devices' votes once they cannot change any more: an hour
/// after their play ends, a day after the vote at the latest. Whether the
/// module is on or not: it is a privacy rule.
pub async fn purge_devices(pool: &MySqlPool) -> ApiResult<u64> {
    let ended = sqlx::query(
        "DELETE d FROM play_vote_devices d JOIN play_history h ON h.id = d.play_id
         WHERE h.ended_at < NOW(6) - INTERVAL 1 HOUR",
    )
    .execute(pool)
    .await?
    .rows_affected();
    let old = sqlx::query("DELETE FROM play_vote_devices WHERE voted_at < NOW(6) - INTERVAL 1 DAY")
        .execute(pool)
        .await?
        .rows_affected();
    Ok(ended + old)
}

/// Runs [`purge_devices`] every 10 minutes. Never returns.
pub async fn purge(pool: MySqlPool) {
    let mut interval = tokio::time::interval(PURGE_EVERY);
    loop {
        interval.tick().await;
        match purge_devices(&pool).await {
            Ok(0) => {}
            Ok(purged) => tracing::debug!(purged, "devices' votes deleted"),
            Err(error) => tracing::warn!(%error, "cannot delete the devices' votes"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weeks_start_on_monday_in_the_stations_time_zone() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        // Sunday 8 November 2026, 23:30 in Paris.
        let sunday: Timestamp = "2026-11-08T22:30:00Z".parse().unwrap();
        assert_eq!(
            week_start(sunday, &paris).unwrap(),
            "2026-11-01T23:00:00Z".parse::<Timestamp>().unwrap()
        );
        // Monday 00:30 in Paris is the next week.
        let monday: Timestamp = "2026-11-08T23:30:00Z".parse().unwrap();
        assert_eq!(
            week_start(monday, &paris).unwrap(),
            "2026-11-08T23:00:00Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn the_share_is_of_the_votes() {
        assert_eq!(share(0, 0), 0.0);
        assert_eq!(share(3, 1), 0.75);
    }
}
