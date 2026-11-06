// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cover art through the API (docs/api.md#cover-art): a track's cover,
//! set by hand or removed, Cover Art Archive lookups and their candidates,
//! the station logo, and the cover addresses of the public side.
//!
//! Covers are found by the scanner and `heron-librarian`
//! (`heron_library::covers`); images are served by their hash from
//! `/public/covers/{hash}`, an address that changes with the image.

use std::collections::HashMap;
use std::time::Duration;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use heron_core::library::TaskRequest;
use heron_library::covers::{self, CoverError, Link, Source};
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::librarian::{LibraryTask, ask};
use crate::selection::TrackSelection;
use crate::settings::{self, StationSettings};
use crate::stations::Station;

/// Where public cover images are served, by hash.
pub const PUBLIC_PATH: &str = "/api/v1/public/covers/";
/// Tracks in one lookup asked for, at most.
const MAX_FETCH: usize = 50_000;
/// An image fetched from an address, the request with its body.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// Cached for a year: the address changes with the image.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Revalidated: the address stays the same when the image changes.
const REVALIDATE: &str = "private, no-cache";

pub fn router() -> OpenApiRouter<AppState> {
    let images = OpenApiRouter::new()
        .routes(routes!(
            get_track_cover,
            put_track_cover,
            delete_track_cover
        ))
        .routes(routes!(
            get_station_logo,
            put_station_logo,
            delete_station_logo
        ))
        .layer(DefaultBodyLimit::max(covers::MAX_BYTES + 64 * 1024));
    OpenApiRouter::new()
        .merge(images)
        .routes(routes!(fetch_covers))
        .routes(routes!(list_cover_candidates))
        .routes(routes!(accept_cover_candidate))
        .routes(routes!(refuse_cover_candidates))
}

/// The public route, with the public endpoints (any origin).
pub fn public_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(public_cover))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverSource {
    /// Uploaded, or fetched from an address, by an operator.
    Hand,
    /// Embedded in the file.
    Embedded,
    /// An image beside the file (`cover.jpg`).
    File,
    CoverArtArchive,
}

impl From<Source> for CoverSource {
    fn from(source: Source) -> Self {
        match source {
            Source::Hand => Self::Hand,
            Source::Embedded => Self::Embedded,
            Source::File => Self::File,
            Source::CoverArtArchive => Self::CoverArtArchive,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverScope {
    /// The cover of the track's album, shared by its tracks.
    #[default]
    Album,
    /// The track's own, in place of its album's.
    Track,
}

/// A track's cover, as its page shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct TrackCover {
    pub source: CoverSource,
    /// The file's name (`file`), the release ID (`cover_art_archive`).
    pub detail: Option<String>,
    pub scope: CoverScope,
    /// Of the image as found.
    pub width: u32,
    pub height: u32,
    /// `/api/v1/public/covers/…`; `?size=96` for the thumbnail.
    pub url: String,
}

/// A release proposed by an unsure search.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema, sqlx::FromRow)]
pub struct CoverCandidate {
    pub id: u64,
    pub release_id: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub date: Option<String>,
    pub country: Option<String>,
    /// MusicBrainz's, 0 to 100.
    pub score: u8,
    /// The release's front cover (250 px) on the Cover Art Archive.
    pub thumbnail_url: String,
}

/// An album whose search was unsure.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CandidateAlbum {
    pub album_key: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    /// One of its tracks.
    pub track_id: u64,
    /// Its tracks.
    pub tracks: u64,
    /// Best first.
    pub candidates: Vec<CoverCandidate>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SizeQuery {
    /// 600 (the default) or 96.
    pub size: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ScopeQuery {
    /// The album's cover (the default), or the track's own.
    #[param(inline)]
    pub scope: Option<CoverScope>,
}

/// An image to fetch from an address.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CoverAddress {
    /// `http` or `https`.
    pub url: String,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchRequest {
    /// The tracks whose albums to look up; every album without a cover
    /// when left out.
    pub selection: Option<TrackSelection>,
    /// Shows the results as candidates even when one is sure, and looks up
    /// albums that have a cover too.
    #[serde(default)]
    pub candidates: bool,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptRequest {
    /// For this track only; for its album when left out.
    pub track_id: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefuseRequest {
    pub album_key: String,
}

/// The public address of a cover.
pub fn public_path(hash: &[u8]) -> String {
    format!("{PUBLIC_PATH}{}", covers::hex(hash))
}

fn large(size: Option<u32>) -> ApiResult<bool> {
    match size {
        None | Some(covers::LARGE) => Ok(true),
        Some(covers::SMALL) => Ok(false),
        Some(_) => Err(ApiError::BadRequest(format!(
            "size is {} or {}",
            covers::LARGE,
            covers::SMALL
        ))),
    }
}

/// The image of `hash`, with its `ETag`; `304` when the client has it.
async fn image(
    pool: &MySqlPool,
    hash: &[u8],
    large: bool,
    cache: &'static str,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    let etag = format!(
        "\"{}-{}\"",
        covers::hex(hash),
        if large { covers::LARGE } else { covers::SMALL }
    );
    let cache_headers = |response: &mut Response| {
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
        if let Ok(value) = HeaderValue::from_str(&etag) {
            headers.insert(header::ETAG, value);
        }
    };
    let known = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|tag| tag.trim() == etag));
    if known {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        cache_headers(&mut response);
        return Ok(response);
    }
    let column = if large { "image" } else { "thumbnail" };
    let bytes: Option<Vec<u8>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT {column} FROM covers WHERE hash = ?"
    )))
    .bind(hash)
    .fetch_optional(pool)
    .await?;
    let bytes = bytes.ok_or_else(|| ApiError::not_found("cover"))?;
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    cache_headers(&mut response);
    Ok(response)
}

/// The cover a track shows: its own, else its album's.
pub async fn track_cover(pool: &MySqlPool, track_id: u64) -> ApiResult<Option<TrackCover>> {
    type Row = (Vec<u8>, String, Option<String>, String, u32, u32);
    let row: Option<Row> = sqlx::query_as(
        "SELECT c.hash, CAST(l.source AS CHAR), l.detail, l.scope, c.width, c.height
         FROM (SELECT cover_hash, source, detail, 'track' AS scope, 0 AS rank
               FROM track_covers WHERE track_id = ?
               UNION ALL
               SELECT a.cover_hash, a.source, a.detail, 'album', 1
               FROM album_covers a JOIN tracks t ON t.album_key = a.album_key
               WHERE t.id = ?) l
         JOIN covers c ON c.hash = l.cover_hash
         ORDER BY l.rank LIMIT 1",
    )
    .bind(track_id)
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    Ok(
        row.and_then(|(hash, source, detail, scope, width, height)| {
            Some(TrackCover {
                source: Source::parse(&source)?.into(),
                detail,
                scope: if scope == "track" {
                    CoverScope::Track
                } else {
                    CoverScope::Album
                },
                width,
                height,
                url: public_path(&hash),
            })
        }),
    )
}

/// The pending candidates of a track's album.
pub async fn track_candidates(pool: &MySqlPool, track_id: u64) -> ApiResult<Vec<CoverCandidate>> {
    Ok(sqlx::query_as(
        "SELECT c.id, c.release_id, c.title, c.artist, c.date, c.country,
                CAST(c.score AS UNSIGNED) AS score, c.thumbnail_url
         FROM cover_candidates c JOIN tracks t ON t.album_key = c.album_key
         WHERE t.id = ? AND c.status = 'pending'
         ORDER BY c.score DESC, c.id",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?)
}

/// The covers of the public side: a track's cover, else the station's
/// logo; full addresses when the station has its public address.
#[derive(Clone, Debug, Default)]
pub struct PublicCovers {
    base: String,
    logo: Option<String>,
    tracks: HashMap<u64, String>,
}

impl PublicCovers {
    /// None while cover art is turned off for the station.
    pub async fn load(pool: &MySqlPool, station_id: u64, track_ids: &[u64]) -> ApiResult<Self> {
        if !crate::modules::on_for(pool, station_id).await?.cover_art {
            return Ok(Self::default());
        }
        let settings: StationSettings =
            settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await?;
        let base = settings
            .public_url
            .as_deref()
            .map(|url| url.trim_end_matches('/').to_owned())
            .unwrap_or_default();
        let logo: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT logo_hash FROM stations WHERE id = ?")
                .bind(station_id)
                .fetch_optional(pool)
                .await?
                .flatten();
        let mut tracks = HashMap::new();
        if !track_ids.is_empty() {
            let mut query = QueryBuilder::new(
                "SELECT t.id, COALESCE(tc.cover_hash, ac.cover_hash) FROM tracks t
                 LEFT JOIN track_covers tc ON tc.track_id = t.id
                 LEFT JOIN album_covers ac ON ac.album_key = t.album_key
                 WHERE COALESCE(tc.cover_hash, ac.cover_hash) IS NOT NULL AND t.id IN (",
            );
            let mut separated = query.separated(", ");
            for id in track_ids {
                separated.push_bind(*id);
            }
            query.push(")");
            let rows: Vec<(u64, Vec<u8>)> = query.build_query_as().fetch_all(pool).await?;
            tracks = rows
                .into_iter()
                .map(|(id, hash)| (id, public_path(&hash)))
                .collect();
        }
        Ok(Self {
            logo: logo.map(|hash| public_path(&hash)),
            base,
            tracks,
        })
    }

    /// The cover address of a track (or of a file outside the library).
    pub fn url(&self, track_id: Option<u64>) -> Option<String> {
        let path = track_id
            .and_then(|id| self.tracks.get(&id))
            .or(self.logo.as_ref())?;
        Some(format!("{}{path}", self.base))
    }
}

/// The track's cover as JPEG, 404 when it has none. Its address stays the
/// same when the cover changes: it is revalidated (`ETag`).
#[utoipa::path(get, path = "/tracks/{id}/cover", tag = "library",
    params(("id" = u64, Path), SizeQuery),
    responses((status = 200, content_type = "image/jpeg", body = Vec<u8>),
              (status = 304), (status = "4XX", body = ErrorBody)))]
async fn get_track_cover(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<SizeQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let large = large(query.size)?;
    let hash: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT COALESCE(tc.cover_hash, ac.cover_hash) FROM tracks t
         LEFT JOIN track_covers tc ON tc.track_id = t.id
         LEFT JOIN album_covers ac ON ac.album_key = t.album_key
         WHERE t.id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    let hash = hash.ok_or_else(|| ApiError::not_found("cover"))?;
    image(&state.pool, &hash, large, REVALIDATE, &headers).await
}

/// A cover or a station logo, by its hash. No sign-in; any origin. Its
/// address changes with the image: it is cached for a year.
#[utoipa::path(get, path = "/public/covers/{hash}", tag = "public", security(()),
    params(("hash" = String, Path), SizeQuery),
    responses((status = 200, content_type = "image/jpeg", body = Vec<u8>),
              (status = 304), (status = "4XX", body = ErrorBody)))]
async fn public_cover(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    Query(query): Query<SizeQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let large = large(query.size)?;
    let hash = covers::parse_hex(&hash).ok_or_else(|| ApiError::not_found("cover"))?;
    image(&state.pool, &hash, large, IMMUTABLE, &headers).await
}

/// The image of a request: its body, or the image at the address of a
/// JSON body (`{"url"}`).
async fn uploaded(headers: &HeaderMap, body: Bytes) -> ApiResult<Vec<u8>> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if content_type.starts_with("application/json") {
        let address: CoverAddress = serde_json::from_slice(&body)
            .map_err(|error| ApiError::BadRequest(format!("invalid body: {error}")))?;
        return fetch(&address.url).await;
    }
    if body.is_empty() {
        return Err(ApiError::Invalid("send an image".to_owned()));
    }
    Ok(body.to_vec())
}

/// Fetches an image from an address, at most [`covers::MAX_BYTES`]. What
/// it answers is only kept if it reads as an image.
async fn fetch(url: &str) -> ApiResult<Vec<u8>> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|_| ApiError::Invalid(format!("{url:?} is not an address")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::Invalid(
            "the address starts with http:// or https://".to_owned(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("HeronBroadcaster/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let unreachable = |error: reqwest::Error| {
        ApiError::Invalid(format!("cannot fetch the image: {}", error.without_url()))
    };
    let mut response = client.get(parsed).send().await.map_err(unreachable)?;
    if !response.status().is_success() {
        return Err(ApiError::Invalid(format!(
            "cannot fetch the image: the address answered {}",
            response.status()
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
        if bytes.len() + chunk.len() > covers::MAX_BYTES {
            return Err(ApiError::Invalid(CoverError::TooLarge.to_string()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Stores an image; what cannot be read is the request's fault.
async fn store(state: &AppState, bytes: Vec<u8>) -> ApiResult<covers::Hash> {
    let mut connection = state.pool.acquire().await?;
    covers::store(&mut connection, bytes)
        .await
        .map_err(|error| match error {
            CoverError::Database(error) => error.into(),
            CoverError::Task(error) => ApiError::Internal(error.to_string()),
            other => ApiError::Invalid(other.to_string()),
        })
}

async fn album_key_of(state: &AppState, id: u64) -> ApiResult<Option<String>> {
    let found: Option<Option<String>> =
        sqlx::query_scalar("SELECT album_key FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    found.ok_or_else(|| ApiError::not_found("track"))
}

fn no_album() -> ApiError {
    ApiError::Invalid(
        "this track has no album: set or remove its own cover (scope=track)".to_owned(),
    )
}

/// Sets the track's cover by hand, for its album (the default) or for the
/// track only. The body is the image (JPEG, PNG or WebP, at most 10 MB),
/// or `{"url"}`: the API fetches it. Operators and admins.
#[utoipa::path(put, path = "/tracks/{id}/cover", tag = "library",
    params(("id" = u64, Path), ScopeQuery),
    request_body(content(
        (Vec<u8> = "image/jpeg"), (Vec<u8> = "image/png"), (Vec<u8> = "image/webp"),
        (CoverAddress = "application/json"))),
    responses((status = 200, body = TrackCover), (status = "4XX", body = ErrorBody)))]
async fn put_track_cover(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<ScopeQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<TrackCover>> {
    user.require(Role::Operator)?;
    let album_key = album_key_of(&state, id).await?;
    let scope = query.scope.unwrap_or_default();
    if scope == CoverScope::Album && album_key.is_none() {
        return Err(no_album());
    }
    let bytes = uploaded(&headers, body).await?;
    let hash = store(&state, bytes).await?;
    let link = Link {
        hash,
        source: Source::Hand,
        detail: None,
    };
    let mut connection = state.pool.acquire().await?;
    match (scope, &album_key) {
        (CoverScope::Album, Some(key)) => {
            covers::set_album(&mut connection, key, &link, Some(user.id)).await?;
            // The track shows the album's now.
            sqlx::query("DELETE FROM track_covers WHERE track_id = ?")
                .bind(id)
                .execute(&mut *connection)
                .await?;
        }
        _ => covers::set_track(&mut connection, id, &link, Some(user.id)).await?,
    }
    tracing::info!(user = %user.username, track = id, ?scope, "cover set by hand");
    track_cover(&state.pool, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::Internal("the cover was not saved".to_owned()))
}

/// Removes the album's cover (the default) or the track's own, whatever
/// its source; the file and its folder are looked at again for the next
/// source. Answers the cover the track shows now. Operators and admins.
#[utoipa::path(delete, path = "/tracks/{id}/cover", tag = "library",
    params(("id" = u64, Path), ScopeQuery),
    responses((status = 200, body = Option<TrackCover>), (status = "4XX", body = ErrorBody)))]
async fn delete_track_cover(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<ScopeQuery>,
) -> ApiResult<Json<Option<TrackCover>>> {
    user.require(Role::Operator)?;
    let album_key = album_key_of(&state, id).await?;
    match (query.scope.unwrap_or_default(), &album_key) {
        (CoverScope::Album, Some(key)) => {
            sqlx::query("DELETE FROM album_covers WHERE album_key = ?")
                .bind(key)
                .execute(&state.pool)
                .await?;
        }
        (CoverScope::Album, None) => return Err(no_album()),
        (CoverScope::Track, _) => {
            sqlx::query("DELETE FROM track_covers WHERE track_id = ?")
                .bind(id)
                .execute(&state.pool)
                .await?;
        }
    }
    tracing::info!(user = %user.username, track = id, scope = ?query.scope, "cover removed");
    look_again(&state, id).await;
    Ok(Json(track_cover(&state.pool, id).await?))
}

/// Looks for a track's covers in its file and its folder again.
async fn look_again(state: &AppState, id: u64) {
    let path: Option<String> = match sqlx::query_scalar(
        "SELECT path FROM tracks WHERE id = ? AND missing_since IS NULL AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(track = id, %error, "cannot look for the next cover");
            return;
        }
    };
    let Some(path) = path else {
        return;
    };
    let absolute = heron_core::LibraryPath::from_stored(path).resolve(state.library.root());
    let file = absolute.clone();
    let embedded = tokio::task::spawn_blocking(move || {
        heron_library::tags::read(&absolute)
            .ok()
            .and_then(|read| read.cover)
    })
    .await
    .ok()
    .flatten();
    let found = async {
        let mut connection = state.pool.acquire().await?;
        covers::from_file(&mut connection, id, Some(&file), embedded.as_deref()).await
    };
    if let Err(error) = found.await {
        tracing::warn!(track = id, %error, "cannot look for the next cover");
    }
}

/// Looks the albums of a selection up on the Cover Art Archive (every
/// album without a cover when there is none), as a library task.
/// Operators and admins.
#[utoipa::path(post, path = "/covers/fetch", tag = "library",
    request_body = FetchRequest,
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn fetch_covers(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<FetchRequest>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let track_ids = match &request.selection {
        Some(selection) => {
            let ids = selection.resolve(&state, MAX_FETCH).await?;
            if ids.is_empty() {
                return Err(ApiError::Invalid("the selection is empty".to_owned()));
            }
            ids
        }
        None => Vec::new(),
    };
    let task = ask(
        &state,
        &user,
        TaskRequest::FetchCovers {
            track_ids,
            candidates: request.candidates,
        },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

/// The albums whose Cover Art Archive search was unsure and that have no
/// cover, with their candidates.
#[utoipa::path(get, path = "/covers/candidates", tag = "library",
    responses((status = 200, body = Vec<CandidateAlbum>), (status = "4XX", body = ErrorBody)))]
async fn list_cover_candidates(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<CandidateAlbum>>> {
    type Row = (String, u64, u64, Option<String>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT t.album_key, MIN(t.id), CAST(COUNT(*) AS UNSIGNED), MAX(t.album),
                MAX(COALESCE(NULLIF(TRIM(t.album_artist), ''), t.artist))
         FROM tracks t
         WHERE t.deleted_at IS NULL
           AND t.album_key IN (SELECT c.album_key FROM cover_candidates c
                               WHERE c.status = 'pending')
           AND NOT EXISTS (SELECT 1 FROM album_covers a WHERE a.album_key = t.album_key)
         GROUP BY t.album_key
         ORDER BY MAX(COALESCE(NULLIF(TRIM(t.album_artist), ''), t.artist)), MAX(t.album)
         LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut albums = Vec::with_capacity(rows.len());
    for (album_key, track_id, tracks, album, artist) in rows {
        albums.push(CandidateAlbum {
            candidates: track_candidates(&state.pool, track_id).await?,
            album_key,
            album,
            artist,
            track_id,
            tracks,
        });
    }
    Ok(Json(albums))
}

/// Picks a candidate: a library task fetches it, for its album or for one
/// track, and refuses the album's other candidates. Operators and admins.
#[utoipa::path(post, path = "/covers/candidates/{id}/accept", tag = "library",
    params(("id" = u64, Path)), request_body = AcceptRequest,
    responses((status = 202, body = LibraryTask), (status = "4XX", body = ErrorBody)))]
async fn accept_cover_candidate(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(request): Json<AcceptRequest>,
) -> ApiResult<(StatusCode, Json<LibraryTask>)> {
    user.require(Role::Operator)?;
    let found: Option<i32> = sqlx::query_scalar("SELECT 1 FROM cover_candidates WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    found.ok_or_else(|| ApiError::not_found("candidate"))?;
    if let Some(track_id) = request.track_id {
        album_key_of(&state, track_id).await?;
    }
    let task = ask(
        &state,
        &user,
        TaskRequest::ApplyCover {
            candidate_id: id,
            track_id: request.track_id,
        },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

/// Refuses every candidate of an album: it keeps no cover. Operators and
/// admins.
#[utoipa::path(post, path = "/covers/candidates/refuse", tag = "library",
    request_body = RefuseRequest,
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn refuse_cover_candidates(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<RefuseRequest>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    sqlx::query(
        "UPDATE cover_candidates SET status = 'refused'
         WHERE album_key = ? AND status = 'pending'",
    )
    .bind(&request.album_key)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The station's logo as JPEG, 404 when it has none.
#[utoipa::path(get, path = "/settings/station/logo", tag = "settings",
    params(SizeQuery),
    responses((status = 200, content_type = "image/jpeg", body = Vec<u8>),
              (status = 304), (status = "4XX", body = ErrorBody)))]
async fn get_station_logo(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<SizeQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let large = large(query.size)?;
    let hash: Option<Vec<u8>> = sqlx::query_scalar("SELECT logo_hash FROM stations WHERE id = ?")
        .bind(station.id())
        .fetch_optional(&state.pool)
        .await?
        .flatten();
    let hash = hash.ok_or_else(|| ApiError::not_found("logo"))?;
    image(&state.pool, &hash, large, REVALIDATE, &headers).await
}

/// Sets the station's logo (JPEG, PNG or WebP, at most 10 MB): it stands in
/// for a missing cover on the public side. Admins only.
#[utoipa::path(put, path = "/settings/station/logo", tag = "settings",
    request_body(content(
        (Vec<u8> = "image/jpeg"), (Vec<u8> = "image/png"), (Vec<u8> = "image/webp"))),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn put_station_logo(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    let bytes = uploaded(&headers, body).await?;
    let hash = store(&state, bytes).await?;
    sqlx::query("UPDATE stations SET logo_hash = ? WHERE id = ?")
        .bind(&hash[..])
        .bind(station.id())
        .execute(&state.pool)
        .await?;
    tracing::info!(user = %user.username, station = station.id(), "station logo set");
    Ok(StatusCode::NO_CONTENT)
}

/// Removes the station's logo. Admins only.
#[utoipa::path(delete, path = "/settings/station/logo", tag = "settings",
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_station_logo(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    sqlx::query("UPDATE stations SET logo_hash = NULL WHERE id = ?")
        .bind(station.id())
        .execute(&state.pool)
        .await?;
    tracing::info!(user = %user.username, station = station.id(), "station logo removed");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_600_or_96() {
        assert!(large(None).unwrap());
        assert!(large(Some(600)).unwrap());
        assert!(!large(Some(96)).unwrap());
        assert!(large(Some(300)).is_err());
    }

    #[test]
    fn public_covers_fall_back_on_the_logo() {
        let covers = PublicCovers {
            base: "https://radio.example.org".into(),
            logo: Some("/api/v1/public/covers/logo".into()),
            tracks: HashMap::from([(1, "/api/v1/public/covers/one".into())]),
        };
        assert_eq!(
            covers.url(Some(1)).as_deref(),
            Some("https://radio.example.org/api/v1/public/covers/one")
        );
        assert_eq!(
            covers.url(Some(2)).as_deref(),
            Some("https://radio.example.org/api/v1/public/covers/logo")
        );
        assert_eq!(PublicCovers::default().url(Some(1)), None);
    }
}
