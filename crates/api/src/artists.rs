// SPDX-License-Identifier: AGPL-3.0-or-later

//! Artists: their names, similar artists and the links suggested for review.
//!
//! - The linker turns tags into artists: the artist tag split on
//!   "featuring", plus artists featured in the title. Each name is matched
//!   case-insensitively, ignoring accents and spaces; an unknown name
//!   becomes a new artist. Tracks whose tags changed are linked again.
//! - Anything less obvious is linked by hand: a name moved to another
//!   artist, two artists merged, two artists marked similar (separation
//!   then treats them as one).
//! - Suggestions are never applied without review: names such as `A & B`
//!   when A and B are known artists, and MusicBrainz relationships looked
//!   up by heron-librarian.

use std::collections::HashMap;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use heron_core::library::TaskRequest;
use heron_rotation::names::{artist_names, normalize};
use serde::{Deserialize, Serialize};
use sqlx::{MySqlConnection, MySqlPool};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate};
use crate::librarian::{self, LibraryTask};

const LINK_INTERVAL: Duration = Duration::from_secs(10);
const LINK_BATCH: u32 = 500;

/// The key a track's artists were linked from, in SQL, from its artist and
/// title (two placeholders, or two columns).
fn artists_key(artist: &str, title: &str) -> String {
    format!("UNHEX(SHA2(CONCAT_WS(CHAR(31), COALESCE({artist}, ''), COALESCE({title}, '')), 256))")
}

/// Links the tracks whose tags changed, until none is left, every few seconds.
pub async fn run_linker(pool: MySqlPool, stations: std::sync::Arc<crate::stations::Stations>) {
    loop {
        match link_pending(&pool).await {
            Ok(0) => {}
            Ok(linked) => {
                tracing::info!(tracks = linked, "linked tracks to their artists");
                stations.wake_all();
                if linked as u32 == LINK_BATCH {
                    continue;
                }
            }
            Err(error) => tracing::warn!(%error, "cannot link tracks to their artists"),
        }
        tokio::time::sleep(LINK_INTERVAL).await;
    }
}

/// Links a batch of tracks whose tags changed since they were linked.
pub async fn link_pending(pool: &MySqlPool) -> ApiResult<usize> {
    type Row = (u64, Option<String>, Option<String>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, artist, title, musicbrainz_artist_id FROM tracks
             WHERE NOT (artists_key <=> {}) ORDER BY id LIMIT ?",
        artists_key("artist", "title")
    )))
    .bind(LINK_BATCH)
    .fetch_all(pool)
    .await?;
    let mut created = false;
    for (id, artist, title, musicbrainz_id) in &rows {
        let mut tx = pool.begin().await?;
        let names = artist_names(artist.as_deref(), title.as_deref());
        let mut artists = Vec::with_capacity(names.len());
        for name in &names {
            let (artist, new) = find_or_create(&mut tx, name).await?;
            created |= new;
            if !artists.contains(&artist) {
                artists.push(artist);
            }
        }
        sqlx::query("DELETE FROM track_artists WHERE track_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (position, artist) in artists.iter().enumerate() {
            sqlx::query(
                "INSERT INTO track_artists (track_id, artist_id, position) VALUES (?, ?, ?)",
            )
            .bind(id)
            .bind(artist)
            .bind(u8::try_from(position).unwrap_or(u8::MAX))
            .execute(&mut *tx)
            .await?;
        }
        // A single artist with a MusicBrainz ID: the artist gets it.
        if let ([artist], Some(mbid)) = (&artists[..], musicbrainz_id) {
            let result = sqlx::query(
                "UPDATE artists SET musicbrainz_id = ? WHERE id = ? AND musicbrainz_id IS NULL",
            )
            .bind(mbid)
            .bind(artist)
            .execute(&mut *tx)
            .await;
            match result {
                // Another artist has it: they may be the same, left for review.
                Err(error) if is_duplicate(&error) => {}
                other => {
                    other?;
                }
            }
        }
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE tracks SET artists_key = {} WHERE id = ?",
            artists_key("?", "?")
        )))
        .bind(artist)
        .bind(title)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }
    if created {
        suggest_from_names(pool).await?;
    }
    if !rows.is_empty() {
        forget_unused(pool).await?;
    }
    Ok(rows.len())
}

/// Deletes the artists nothing refers to any more: no track, nothing done
/// by hand (names moved, similar artists, restrictions, MusicBrainz ID).
async fn forget_unused(pool: &MySqlPool) -> ApiResult<()> {
    let forgotten = sqlx::query(
        "DELETE FROM artists
         WHERE musicbrainz_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM track_artists ta WHERE ta.artist_id = artists.id)
           AND NOT EXISTS (SELECT 1 FROM artist_names n WHERE n.artist_id = artists.id AND n.by_hand)
           AND NOT EXISTS (SELECT 1 FROM artist_similar s
                           WHERE s.artist_id = artists.id OR s.similar_id = artists.id)
           AND NOT EXISTS (SELECT 1 FROM restrictions r
                           WHERE r.target = 'artist' AND r.target_id = artists.id)
           AND NOT EXISTS (SELECT 1 FROM featured_artists f WHERE f.artist_id = artists.id)",
    )
    .execute(pool)
    .await?
    .rows_affected();
    if forgotten > 0 {
        tracing::info!(artists = forgotten, "forgot artists no track uses any more");
    }
    Ok(())
}

/// The artist a name belongs to, created with it if the name is new.
async fn find_or_create(tx: &mut MySqlConnection, name: &str) -> ApiResult<(u64, bool)> {
    let normalized = normalize(name);
    let find = "SELECT artist_id FROM artist_names WHERE normalized_hash = UNHEX(SHA2(?, 256))";
    if let Some(artist) = sqlx::query_scalar(find)
        .bind(&normalized)
        .fetch_optional(&mut *tx)
        .await?
    {
        return Ok((artist, false));
    }
    let artist = sqlx::query("INSERT INTO artists (name) VALUES (?)")
        .bind(name)
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    sqlx::query("INSERT INTO artist_names (artist_id, name, normalized) VALUES (?, ?, ?)")
        .bind(artist)
        .bind(name)
        .bind(&normalized)
        .execute(&mut *tx)
        .await?;
    Ok((artist, true))
}

/// What a name such as `A & B` or `A, B and C` is made of, normalized.
fn name_parts(normalized: &str) -> Vec<&str> {
    const SEPARATORS: [&str; 9] = [
        " & ", " and ", ", ", " / ", " x ", " + ", " vs. ", " vs ", " with ",
    ];
    let mut parts = vec![normalized];
    for separator in SEPARATORS {
        parts = parts
            .into_iter()
            .flat_map(|part| part.split(separator))
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
    }
    if parts.len() < 2 {
        return Vec::new();
    }
    parts
}

/// Suggests linking artists whose name is made of other artists' names.
async fn suggest_from_names(pool: &MySqlPool) -> ApiResult<()> {
    let names: Vec<(u64, String, String)> =
        sqlx::query_as("SELECT artist_id, name, normalized FROM artist_names")
            .fetch_all(pool)
            .await?;
    let by_name: HashMap<&str, (u64, &str)> = names
        .iter()
        .map(|(artist, name, normalized)| (normalized.as_str(), (*artist, name.as_str())))
        .collect();
    for (artist, name, normalized) in &names {
        for part in name_parts(normalized) {
            let Some(&(other, other_name)) = by_name.get(part) else {
                continue;
            };
            if other == *artist {
                continue;
            }
            sqlx::query(
                "INSERT IGNORE INTO artist_suggestions (artist_id, other_id, source, reason)
                 VALUES (?, ?, 'names', ?)",
            )
            .bind(other.min(*artist))
            .bind(other.max(*artist))
            .bind(format!("“{name}” names {other_name}"))
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_artists))
        .routes(routes!(get_artist, patch_artist))
        .routes(routes!(merge_artist))
        .routes(routes!(move_name))
        .routes(routes!(add_similar, remove_similar))
        .routes(routes!(list_suggestions))
        .routes(routes!(review_suggestion))
        .routes(routes!(look_up_relations))
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct ArtistRef {
    pub id: u64,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ArtistName {
    pub id: u64,
    pub name: String,
    /// Moved to this artist by hand.
    pub by_hand: bool,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Artist {
    pub id: u64,
    pub name: String,
    pub musicbrainz_id: Option<String>,
    pub tracks: i64,
    pub names: Vec<ArtistName>,
    /// Separation treats these as the same artist.
    pub similar: Vec<ArtistRef>,
    /// Plays and last play in total and per station; only on
    /// `GET /artists/{id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plays: Option<crate::history::Plays>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ArtistPage {
    pub artists: Vec<Artist>,
    pub total: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ArtistQuery {
    /// Matches any of the artist's names.
    pub q: Option<String>,
    /// 1 to 200, 50 by default.
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Appends `(id, id, …)` to a query.
fn push_ids(query: &mut sqlx::QueryBuilder<sqlx::MySql>, ids: &[u64]) {
    query.push("(");
    let mut separated = query.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
    query.push(")");
}

async fn artists(pool: &MySqlPool, ids: &[u64]) -> ApiResult<Vec<Artist>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::new(
        "SELECT a.id, a.name, a.musicbrainz_id,
                (SELECT COUNT(*) FROM track_artists ta JOIN tracks t ON t.id = ta.track_id
                 WHERE ta.artist_id = a.id AND t.deleted_at IS NULL)
         FROM artists a WHERE a.id IN ",
    );
    push_ids(&mut query, ids);
    let rows: Vec<(u64, String, Option<String>, i64)> =
        query.build_query_as().fetch_all(pool).await?;
    let mut query = sqlx::QueryBuilder::new(
        "SELECT artist_id, id, name, by_hand FROM artist_names WHERE artist_id IN ",
    );
    push_ids(&mut query, ids);
    query.push(" ORDER BY by_hand, id");
    let names: Vec<(u64, u64, String, bool)> = query.build_query_as().fetch_all(pool).await?;
    let mut query = sqlx::QueryBuilder::new(
        "SELECT s.artist_id, a.id, a.name FROM artist_similar s JOIN artists a ON a.id = s.similar_id
         WHERE s.artist_id IN ",
    );
    push_ids(&mut query, ids);
    query.push(
        " UNION ALL SELECT s.similar_id, a.id, a.name FROM artist_similar s
         JOIN artists a ON a.id = s.artist_id WHERE s.similar_id IN ",
    );
    push_ids(&mut query, ids);
    let similar: Vec<(u64, u64, String)> = query.build_query_as().fetch_all(pool).await?;
    let mut by_id: HashMap<u64, Artist> = rows
        .into_iter()
        .map(|(id, name, musicbrainz_id, tracks)| {
            (
                id,
                Artist {
                    id,
                    name,
                    musicbrainz_id,
                    tracks,
                    names: Vec::new(),
                    similar: Vec::new(),
                    plays: None,
                },
            )
        })
        .collect();
    for (artist, id, name, by_hand) in names {
        if let Some(artist) = by_id.get_mut(&artist) {
            artist.names.push(ArtistName { id, name, by_hand });
        }
    }
    for (artist, id, name) in similar {
        if let Some(artist) = by_id.get_mut(&artist) {
            artist.similar.push(ArtistRef { id, name });
        }
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

async fn artist(pool: &MySqlPool, id: u64) -> ApiResult<Artist> {
    artists(pool, &[id])
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("artist"))
}

/// Artists, by name.
#[utoipa::path(get, path = "/artists", tag = "artists",
    params(ArtistQuery),
    responses((status = 200, body = ArtistPage), (status = "4XX", body = ErrorBody)))]
async fn list_artists(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<ArtistQuery>,
) -> ApiResult<Json<ArtistPage>> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::BadRequest("limit is 1 to 200".to_owned()));
    }
    let pattern = query
        .q
        .as_deref()
        .map(normalize)
        .filter(|q| !q.is_empty())
        .map(|q| {
            format!(
                "%{}%",
                q.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
    let filter = "(? IS NULL OR EXISTS (SELECT 1 FROM artist_names n
                   WHERE n.artist_id = a.id AND n.normalized LIKE ?))";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM artists a WHERE {filter}"
    )))
    .bind(&pattern)
    .bind(&pattern)
    .fetch_one(&state.pool)
    .await?;
    let ids: Vec<u64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT a.id FROM artists a WHERE {filter} ORDER BY a.name, a.id LIMIT ? OFFSET ?"
    )))
    .bind(&pattern)
    .bind(&pattern)
    .bind(limit)
    .bind(query.offset.unwrap_or(0))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(ArtistPage {
        artists: artists(&state.pool, &ids).await?,
        total,
    }))
}

#[utoipa::path(get, path = "/artists/{id}", tag = "artists",
    params(("id" = u64, Path)),
    responses((status = 200, body = Artist), (status = "4XX", body = ErrorBody)))]
async fn get_artist(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<Artist>> {
    let mut artist = artist(&state.pool, id).await?;
    artist.plays =
        Some(crate::history::plays(&state.pool, crate::history::PlaysOf::Artist(id)).await?);
    Ok(Json(artist))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtistPatch {
    /// The name shown.
    pub name: Option<String>,
    /// `null` removes it.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<String>)]
    pub musicbrainz_id: Option<Option<String>>,
}

fn some_or_null<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

fn is_mbid(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// Renames an artist, or sets its MusicBrainz ID. Operators and admins.
#[utoipa::path(patch, path = "/artists/{id}", tag = "artists",
    params(("id" = u64, Path)), request_body = ArtistPatch,
    responses((status = 200, body = Artist), (status = "4XX", body = ErrorBody)))]
async fn patch_artist(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(patch): Json<ArtistPatch>,
) -> ApiResult<Json<Artist>> {
    user.require(Role::Operator)?;
    artist(&state.pool, id).await?;
    if let Some(name) = &patch.name {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 512 {
            return Err(ApiError::Invalid(
                "names are 1 to 512 characters".to_owned(),
            ));
        }
        sqlx::query("UPDATE artists SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(&state.pool)
            .await?;
    }
    if let Some(mbid) = &patch.musicbrainz_id {
        let mbid = mbid.as_deref().map(|m| m.trim().to_ascii_lowercase());
        if mbid.as_deref().is_some_and(|m| !is_mbid(m)) {
            return Err(ApiError::Invalid("MusicBrainz IDs are UUIDs".to_owned()));
        }
        sqlx::query("UPDATE artists SET musicbrainz_id = ? WHERE id = ?")
            .bind(mbid)
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(|error| {
                if is_duplicate(&error) {
                    ApiError::Conflict(
                        "another artist has this MusicBrainz ID: merge them".to_owned(),
                    )
                } else {
                    error.into()
                }
            })?;
    }
    artist(&state.pool, id).await.map(Json)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeRequest {
    /// The artist merged into this one, then deleted.
    pub artist_id: u64,
}

/// Merges another artist into this one: its names, tracks, similar
/// artists and restrictions. Operators and admins.
#[utoipa::path(post, path = "/artists/{id}/merge", tag = "artists",
    params(("id" = u64, Path)), request_body = MergeRequest,
    responses((status = 200, body = Artist), (status = "4XX", body = ErrorBody)))]
async fn merge_artist(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(keep): Path<u64>,
    Json(request): Json<MergeRequest>,
) -> ApiResult<Json<Artist>> {
    user.require(Role::Operator)?;
    let gone = request.artist_id;
    if gone == keep {
        return Err(ApiError::Invalid(
            "an artist cannot be merged into itself".to_owned(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    let found: Vec<(u64, Option<String>)> =
        sqlx::query_as("SELECT id, musicbrainz_id FROM artists WHERE id IN (?, ?) FOR UPDATE")
            .bind(keep)
            .bind(gone)
            .fetch_all(&mut *tx)
            .await?;
    if found.len() < 2 {
        return Err(ApiError::not_found("artist"));
    }
    let mbid = |id: u64| {
        found
            .iter()
            .find(|(a, _)| *a == id)
            .and_then(|(_, m)| m.clone())
    };
    let (kept_mbid, gone_mbid) = (mbid(keep), mbid(gone));
    sqlx::query("UPDATE artist_names SET artist_id = ?, by_hand = TRUE WHERE artist_id = ?")
        .bind(keep)
        .bind(gone)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT IGNORE INTO track_artists (track_id, artist_id, position)
         SELECT track_id, ?, position FROM track_artists WHERE artist_id = ?",
    )
    .bind(keep)
    .bind(gone)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT IGNORE INTO artist_similar (artist_id, similar_id)
         SELECT LEAST(?, other), GREATEST(?, other) FROM (
             SELECT similar_id AS other FROM artist_similar WHERE artist_id = ?
             UNION SELECT artist_id FROM artist_similar WHERE similar_id = ?) linked
         WHERE other <> ?",
    )
    .bind(keep)
    .bind(keep)
    .bind(gone)
    .bind(gone)
    .bind(keep)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE restrictions SET target_id = ? WHERE target = 'artist' AND target_id = ?")
        .bind(keep)
        .bind(gone)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE featured_artists SET artist_id = ? WHERE artist_id = ?")
        .bind(keep)
        .bind(gone)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM artists WHERE id = ?")
        .bind(gone)
        .execute(&mut *tx)
        .await?;
    if kept_mbid.is_none() && gone_mbid.is_some() {
        sqlx::query("UPDATE artists SET musicbrainz_id = ? WHERE id = ?")
            .bind(gone_mbid)
            .bind(keep)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    tracing::info!(user = %user.username, keep, gone, "artists merged");
    state.stations.wake_all();
    artist(&state.pool, keep).await.map(Json)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveName {
    /// The artist the name now belongs to; missing to make it an artist
    /// of its own.
    pub artist_id: Option<u64>,
}

/// Moves a name to another artist (or to a new one): the tracks tagged
/// with it follow. Operators and admins.
#[utoipa::path(put, path = "/artist-names/{id}", tag = "artists",
    params(("id" = u64, Path)), request_body = MoveName,
    responses((status = 200, body = Artist), (status = "4XX", body = ErrorBody)))]
async fn move_name(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(request): Json<MoveName>,
) -> ApiResult<Json<Artist>> {
    user.require(Role::Operator)?;
    let mut tx = state.pool.begin().await?;
    let found: Option<(u64, String)> =
        sqlx::query_as("SELECT artist_id, name FROM artist_names WHERE id = ? FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let (from, name) = found.ok_or_else(|| ApiError::not_found("name"))?;
    let to = match request.artist_id {
        Some(to) => {
            let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM artists WHERE id = ?")
                .bind(to)
                .fetch_optional(&mut *tx)
                .await?;
            exists.ok_or_else(|| ApiError::Invalid(format!("no artist {to}")))?
        }
        None => sqlx::query("INSERT INTO artists (name) VALUES (?)")
            .bind(&name)
            .execute(&mut *tx)
            .await?
            .last_insert_id(),
    };
    sqlx::query("UPDATE artist_names SET artist_id = ?, by_hand = TRUE WHERE id = ?")
        .bind(to)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    // The tracks of the artist it left are linked again, by their names.
    sqlx::query(
        "UPDATE tracks t JOIN track_artists ta ON ta.track_id = t.id
         SET t.artists_key = NULL WHERE ta.artist_id = ?",
    )
    .bind(from)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    while link_pending(&state.pool).await? as u32 == LINK_BATCH {}
    // Nothing left of the artist: it goes.
    sqlx::query(
        "DELETE FROM artists WHERE id = ?
           AND NOT EXISTS (SELECT 1 FROM artist_names WHERE artist_id = ?)
           AND NOT EXISTS (SELECT 1 FROM track_artists WHERE artist_id = ?)
           AND NOT EXISTS (SELECT 1 FROM restrictions WHERE target = 'artist' AND target_id = ?)
           AND NOT EXISTS (SELECT 1 FROM featured_artists WHERE artist_id = ?)",
    )
    .bind(from)
    .bind(from)
    .bind(from)
    .bind(from)
    .bind(from)
    .execute(&state.pool)
    .await?;
    tracing::info!(user = %user.username, name = %name, from, to, "artist name moved");
    state.stations.wake_all();
    artist(&state.pool, to).await.map(Json)
}

async fn link_similar(pool: &MySqlPool, a: u64, b: u64, linked: bool) -> ApiResult<()> {
    if a == b {
        return Err(ApiError::Invalid(
            "an artist is always the same as itself".to_owned(),
        ));
    }
    let (low, high) = (a.min(b), a.max(b));
    if linked {
        sqlx::query("INSERT IGNORE INTO artist_similar (artist_id, similar_id) VALUES (?, ?)")
            .bind(low)
            .bind(high)
            .execute(pool)
            .await
            .map_err(|error| {
                if crate::error::is_still_used(&error) {
                    ApiError::not_found("artist")
                } else {
                    error.into()
                }
            })?;
    } else {
        sqlx::query("DELETE FROM artist_similar WHERE artist_id = ? AND similar_id = ?")
            .bind(low)
            .bind(high)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Marks two artists similar: separation treats them as the same artist.
/// Operators and admins.
#[utoipa::path(put, path = "/artists/{id}/similar/{other}", tag = "artists",
    params(("id" = u64, Path), ("other" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn add_similar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((id, other)): Path<(u64, u64)>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    link_similar(&state.pool, id, other, true).await?;
    state.stations.wake_all();
    Ok(StatusCode::NO_CONTENT)
}

/// Operators and admins.
#[utoipa::path(delete, path = "/artists/{id}/similar/{other}", tag = "artists",
    params(("id" = u64, Path), ("other" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn remove_similar(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((id, other)): Path<(u64, u64)>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    link_similar(&state.pool, id, other, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ArtistSuggestion {
    pub id: u64,
    pub artist: ArtistRef,
    pub other: ArtistRef,
    /// `names` or `musicbrainz`.
    pub source: String,
    pub reason: String,
}

/// Links waiting for review.
#[utoipa::path(get, path = "/artist-suggestions", tag = "artists",
    responses((status = 200, body = Vec<ArtistSuggestion>), (status = "4XX", body = ErrorBody)))]
async fn list_suggestions(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ArtistSuggestion>>> {
    let rows: Vec<(u64, u64, String, u64, String, String, String)> = sqlx::query_as(
        "SELECT s.id, a.id, a.name, o.id, o.name, CAST(s.source AS CHAR), s.reason
         FROM artist_suggestions s
         JOIN artists a ON a.id = s.artist_id JOIN artists o ON o.id = s.other_id
         WHERE s.status = 'pending'
           AND NOT EXISTS (SELECT 1 FROM artist_similar l
                           WHERE l.artist_id = s.artist_id AND l.similar_id = s.other_id)
         ORDER BY s.id LIMIT 500",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, a, a_name, o, o_name, source, reason)| ArtistSuggestion {
                    id,
                    artist: ArtistRef {
                        id: a,
                        name: a_name,
                    },
                    other: ArtistRef {
                        id: o,
                        name: o_name,
                    },
                    source,
                    reason,
                },
            )
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestionReview {
    /// `true` marks the artists similar; `false` dismisses the suggestion.
    pub accept: bool,
}

/// Accepts or dismisses a suggestion. Operators and admins.
#[utoipa::path(post, path = "/artist-suggestions/{id}", tag = "artists",
    params(("id" = u64, Path)), request_body = SuggestionReview,
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn review_suggestion(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(review): Json<SuggestionReview>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let found: Option<(u64, u64)> =
        sqlx::query_as("SELECT artist_id, other_id FROM artist_suggestions WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let (artist, other) = found.ok_or_else(|| ApiError::not_found("suggestion"))?;
    if review.accept {
        link_similar(&state.pool, artist, other, true).await?;
        state.stations.wake_all();
    }
    sqlx::query("UPDATE artist_suggestions SET status = ? WHERE id = ?")
        .bind(if review.accept {
            "accepted"
        } else {
            "dismissed"
        })
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Asks heron-librarian to look up the MusicBrainz relationships ("member
/// of", "is person", …) between the artists that have a MusicBrainz ID;
/// they become suggestions. Operators and admins.
#[utoipa::path(post, path = "/artist-suggestions/musicbrainz", tag = "artists",
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn look_up_relations(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let task = librarian::ask(&state, &user, TaskRequest::ArtistRelations).await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_made_of_names() {
        assert_eq!(name_parts("simon & garfunkel"), ["simon", "garfunkel"]);
        assert_eq!(
            name_parts("crosby, stills, nash and young"),
            ["crosby", "stills", "nash", "young"]
        );
        assert!(name_parts("pink floyd").is_empty());
    }

    #[test]
    fn checks_musicbrainz_ids() {
        assert!(is_mbid("83d91898-7763-47d7-b03b-b92132375c47"));
        assert!(!is_mbid("83d91898776347d7b03bb92132375c47"));
        assert!(!is_mbid("83d91898-7763-47d7-b03b-b92132375c4g"));
    }
}
