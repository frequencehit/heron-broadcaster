// SPDX-License-Identifier: AGPL-3.0-or-later

//! Audience certification (`docs/certification.md`): the stream servers'
//! access logs, read once a minute from a file or an address from where
//! the last reading stopped, each line kept unchanged with what is parsed
//! from it, and exported per day, server and mount for the bodies that
//! certify online audience.
//!
//! The lines hold the listeners' addresses: only admins download them,
//! each download is logged, and they are deleted after the retention.

use std::path::{Component, Path as FsPath, PathBuf};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use heron_core::Module;
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::{self, StationSettings, some_or_null};
use crate::stations::Station;

mod delivery;
mod send;

pub use delivery::Worker as DeliveryWorker;
pub use send::Sender as DeliverySender;

/// How often the logs are read.
const READ_EVERY: Duration = Duration::from_secs(60);
/// The most read from a log at once; the rest waits for the next reading.
const MAX_CHUNK: u64 = 8 * 1024 * 1024;
/// Lines inserted per statement.
const BATCH: usize = 500;
/// The longest range of days `/certification/days` gives.
const MAX_DAYS: i64 = 62;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(
            get_certification_settings,
            put_certification_settings
        ))
        .routes(routes!(get_access_log, put_access_log, delete_access_log))
        .routes(routes!(list_certification_sources))
        .routes(routes!(certification_days))
        .routes(routes!(export_day))
        .merge(delivery::router())
}

fn seconds(ms: i64) -> f64 {
    ms as f64 / 1_000.0
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// How long the access logs' lines are kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CertificationSettings {
    /// 1 to 25 months, 13 by default.
    pub retention_months: u32,
}

impl CertificationSettings {
    pub const SETTINGS_NAME: &str = "certification";

    fn check(&self) -> Result<(), String> {
        if !(1..=25).contains(&self.retention_months) {
            return Err("retention_months is 1 to 25".to_owned());
        }
        Ok(())
    }
}

impl Default for CertificationSettings {
    fn default() -> Self {
        Self {
            retention_months: 13,
        }
    }
}

/// How long the access logs' lines are kept.
#[utoipa::path(get, path = "/settings/certification", tag = "certification",
    responses((status = 200, body = CertificationSettings), (status = "4XX", body = ErrorBody)))]
async fn get_certification_settings(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<CertificationSettings>> {
    Ok(Json(
        settings::load(&state.pool, CertificationSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the certification settings. Admins only.
#[utoipa::path(put, path = "/settings/certification", tag = "certification",
    request_body = CertificationSettings,
    responses((status = 200, body = CertificationSettings), (status = "4XX", body = ErrorBody)))]
async fn put_certification_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(new): Json<CertificationSettings>,
) -> ApiResult<Json<CertificationSettings>> {
    user.require(Role::Admin)?;
    new.check().map_err(ApiError::Invalid)?;
    settings::save(&state.pool, CertificationSettings::SETTINGS_NAME, &new).await?;
    tracing::info!(user = %user.username, ?new, "certification settings changed");
    Ok(Json(new))
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// What a line of an Icecast access log says (the combined format, with
/// the session's length in seconds at the end).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub address: String,
    /// The line's date: when the session ended.
    pub logged_at: Timestamp,
    pub method: String,
    /// The path, its query left out.
    pub path: String,
    pub status: Option<u16>,
    pub bytes: Option<u64>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    pub duration_s: Option<u32>,
}

impl Parsed {
    /// A listener's session: a mount listened to, not the source sending
    /// it, the admin, a status page or a file.
    pub fn is_session(&self) -> bool {
        const FILES: [&str; 9] = [
            ".xsl", ".html", ".htm", ".css", ".js", ".ico", ".png", ".json", ".xml",
        ];
        self.method == "GET"
            && self
                .status
                .is_some_and(|status| (200..300).contains(&status))
            && self.duration_s.is_some()
            && self.path.len() > 1
            && !self.path.starts_with("/admin")
            && !FILES
                .iter()
                .any(|ext| self.path.to_ascii_lowercase().ends_with(ext))
    }
}

/// `-` is none.
fn present(field: String) -> Option<String> {
    (field != "-" && !field.is_empty()).then_some(field)
}

/// The fields after the date: bare words, or quoted with `\"` inside.
fn fields(rest: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut field = String::new();
        if first == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(next) = chars.next() {
                            field.push(next);
                        }
                    }
                    '"' => break,
                    c => field.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                field.push(c);
                chars.next();
            }
        }
        fields.push(field);
    }
    fields
}

/// Parses a line: `None` when it is not one of an access log.
pub fn parse(line: &str) -> Option<Parsed> {
    let (address, rest) = line.split_once(' ')?;
    let open = rest.find('[')?;
    let close = open + rest[open..].find(']')?;
    let logged_at = Timestamp::strptime("%d/%b/%Y:%H:%M:%S %z", &rest[open + 1..close]).ok()?;
    let fields = fields(&rest[close + 1..]);
    let mut request = fields.first()?.split(' ');
    let method = request.next()?.to_owned();
    let target = request.next().unwrap_or("");
    let path = target.split('?').next().unwrap_or("").to_owned();
    let number = |at: usize| fields.get(at).and_then(|field| field.parse::<u64>().ok());
    Some(Parsed {
        address: address.to_owned(),
        logged_at,
        method,
        path,
        status: number(1).and_then(|status| u16::try_from(status).ok()),
        bytes: number(2),
        referer: fields.get(3).cloned().and_then(present),
        user_agent: fields.get(4).cloned().and_then(present),
        duration_s: number(5).and_then(|duration| u32::try_from(duration).ok()),
    })
}

// ---------------------------------------------------------------------------
// Where the logs are read from
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogSource {
    /// A file under the access logs folder.
    File,
    /// An address, read with range requests.
    Url,
}

impl LogSource {
    fn key(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Url => "url",
        }
    }

    fn of(key: &str) -> Self {
        if key == "url" { Self::Url } else { Self::File }
    }
}

/// A server's access log as stored, password included.
#[derive(Clone, Debug, sqlx::FromRow)]
struct StoredLog {
    server_id: u64,
    station_id: u64,
    source: String,
    path: Option<String>,
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    position: u64,
    read_at_ms: Option<i64>,
    read_ok_at_ms: Option<i64>,
    error: Option<String>,
    error_since_ms: Option<i64>,
}

const STORED: &str = "SELECT l.server_id, l.station_id, CAST(l.source AS CHAR) AS source, l.path, l.url, l.username, l.password,
        l.position,
        CAST(UNIX_TIMESTAMP(l.read_at) * 1000 AS SIGNED) AS read_at_ms,
        CAST(UNIX_TIMESTAMP(l.read_ok_at) * 1000 AS SIGNED) AS read_ok_at_ms,
        l.error,
        CAST(UNIX_TIMESTAMP(l.error_since) * 1000 AS SIGNED) AS error_since_ms
     FROM access_logs l";

/// A server's access log, its password never sent back, and how its
/// reading goes.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AccessLogView {
    pub server_id: u64,
    pub server_name: String,
    pub source: LogSource,
    pub path: Option<String>,
    pub url: Option<String>,
    pub username: Option<String>,
    pub password_set: bool,
    /// Bytes of the current file read.
    pub position: u64,
    pub read_at_unix_ms: Option<i64>,
    pub read_ok_at_unix_ms: Option<i64>,
    /// Why the last reading failed, and since when it does.
    pub error: Option<String>,
    pub error_since_unix_ms: Option<i64>,
}

impl AccessLogView {
    fn new(log: StoredLog, server_name: String) -> Self {
        Self {
            server_id: log.server_id,
            server_name,
            source: LogSource::of(&log.source),
            path: log.path,
            url: log.url,
            username: log.username,
            password_set: log.password.is_some_and(|password| !password.is_empty()),
            position: log.position,
            read_at_unix_ms: log.read_at_ms,
            read_ok_at_unix_ms: log.read_ok_at_ms,
            error: log.error,
            error_since_unix_ms: log.error_since_ms,
        }
    }
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessLogInput {
    pub source: LogSource,
    /// For a file: relative to the access logs folder (`main/access.log`).
    #[serde(default)]
    pub path: Option<String>,
    /// For an address: `https://…`.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    /// Left out, the stored one is kept; `null` removes it.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<String>)]
    pub password: Option<Option<String>>,
}

/// A path under the access logs folder: relative, without `..`.
fn check_path(path: Option<String>) -> ApiResult<String> {
    let path = path
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty())
        .ok_or_else(|| ApiError::Invalid("a file needs its path".to_owned()))?;
    if !FsPath::new(&path)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(ApiError::Invalid(format!(
            "the path is relative to the access logs folder, without .. (got {path:?})"
        )));
    }
    Ok(path)
}

fn check_url(url: Option<String>) -> ApiResult<String> {
    let url = url
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .ok_or_else(|| ApiError::Invalid("an address needs its URL".to_owned()))?;
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.contains(|c: char| c.is_whitespace() || c.is_control())
    {
        return Err(ApiError::Invalid(format!(
            "the URL must start with https:// or http:// (got {url:?})"
        )));
    }
    Ok(url)
}

/// The server's name, when it is one of the station's.
pub(crate) async fn server_name(
    pool: &MySqlPool,
    station_id: u64,
    server_id: u64,
) -> ApiResult<String> {
    sqlx::query_scalar("SELECT name FROM stream_servers WHERE id = ? AND station_id = ?")
        .bind(server_id)
        .bind(station_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::not_found("stream server"))
}

async fn stored(pool: &MySqlPool, server_id: u64) -> ApiResult<Option<StoredLog>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{STORED} WHERE l.server_id = ?"
    )))
    .bind(server_id)
    .fetch_optional(pool)
    .await?)
}

/// A server's access log and how its reading goes; `null` without one.
#[utoipa::path(get, path = "/stream-servers/{server_id}/access-log", tag = "certification",
    params(("server_id" = u64, Path)),
    responses((status = 200, body = Option<AccessLogView>), (status = "4XX", body = ErrorBody)))]
async fn get_access_log(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(server_id): Path<u64>,
) -> ApiResult<Json<Option<AccessLogView>>> {
    user.require(Role::Admin)?;
    let name = server_name(&state.pool, station.id(), server_id).await?;
    let log = stored(&state.pool, server_id).await?;
    Ok(Json(log.map(|log| AccessLogView::new(log, name))))
}

/// Sets where a server's access log is read from. A new place starts from
/// the beginning of the file. Admins only.
#[utoipa::path(put, path = "/stream-servers/{server_id}/access-log", tag = "certification",
    params(("server_id" = u64, Path)), request_body = AccessLogInput,
    responses((status = 200, body = AccessLogView), (status = "4XX", body = ErrorBody)))]
async fn put_access_log(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(server_id): Path<u64>,
    Json(input): Json<AccessLogInput>,
) -> ApiResult<Json<AccessLogView>> {
    user.require(Role::Admin)?;
    let name = server_name(&state.pool, station.id(), server_id).await?;
    let (path, url) = match input.source {
        LogSource::File => (Some(check_path(input.path)?), None),
        LogSource::Url => (None, Some(check_url(input.url)?)),
    };
    let username = input
        .username
        .map(|username| username.trim().to_owned())
        .filter(|username| !username.is_empty());
    let before = stored(&state.pool, server_id).await?;
    let password = match input.password {
        Some(password) => password.filter(|password| !password.is_empty()),
        None => before.as_ref().and_then(|log| log.password.clone()),
    };
    let moved = before.as_ref().is_none_or(|log| {
        LogSource::of(&log.source) != input.source || log.path != path || log.url != url
    });
    sqlx::query(
        "INSERT INTO access_logs (server_id, station_id, source, path, url, username, password)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE source = VALUES(source), path = VALUES(path), url = VALUES(url),
             username = VALUES(username), password = VALUES(password),
             position = IF(?, 0, position), error = IF(?, NULL, error),
             error_since = IF(?, NULL, error_since)",
    )
    .bind(server_id)
    .bind(station.id())
    .bind(input.source.key())
    .bind(&path)
    .bind(&url)
    .bind(&username)
    .bind(&password)
    .bind(moved)
    .bind(moved)
    .bind(moved)
    .execute(&state.pool)
    .await?;
    tracing::info!(user = %user.username, server = server_id, source = ?input.source, "access log set");
    let log = stored(&state.pool, server_id)
        .await?
        .ok_or_else(|| ApiError::Internal("the access log just saved is gone".to_owned()))?;
    Ok(Json(AccessLogView::new(log, name)))
}

/// Stops reading a server's access log. The lines kept stay until their
/// retention. Admins only.
#[utoipa::path(delete, path = "/stream-servers/{server_id}/access-log", tag = "certification",
    params(("server_id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_access_log(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(server_id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    server_name(&state.pool, station.id(), server_id).await?;
    sqlx::query("DELETE FROM access_logs WHERE server_id = ?")
        .bind(server_id)
        .execute(&state.pool)
        .await?;
    tracing::info!(user = %user.username, server = server_id, "access log removed");
    Ok(StatusCode::NO_CONTENT)
}

/// The station's servers with an access log, and how their reading goes.
#[utoipa::path(get, path = "/certification/sources", tag = "certification",
    responses((status = 200, body = Vec<AccessLogView>), (status = "4XX", body = ErrorBody)))]
async fn list_certification_sources(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Vec<AccessLogView>>> {
    user.require(Role::Admin)?;
    let logs: Vec<StoredLog> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{STORED} WHERE l.station_id = ? ORDER BY l.server_id"
    )))
    .bind(station.id())
    .fetch_all(&state.pool)
    .await?;
    let names: Vec<(u64, String)> =
        sqlx::query_as("SELECT id, name FROM stream_servers WHERE station_id = ?")
            .bind(station.id())
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(
        logs.into_iter()
            .map(|log| {
                let name = names
                    .iter()
                    .find(|(id, _)| *id == log.server_id)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_default();
                AccessLogView::new(log, name)
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// What a reading brought.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Chunk {
    /// The rest of the previous file, when the log was rotated and it is
    /// there: read whole, its last line too.
    pub previous: Option<Vec<u8>>,
    /// The log was rotated and its previous file could not be read: the
    /// lines written since the last reading are lost.
    pub lost: bool,
    /// Where `data` starts in the current file.
    pub base: u64,
    pub data: Vec<u8>,
    /// `data` is the whole rest of the file, not cut at `MAX_CHUNK`.
    pub whole: bool,
}

/// The full lines of `data`, and how many bytes they take; with `last`,
/// a line without its newline counts too.
pub fn lines(data: &[u8], last: bool) -> (Vec<String>, u64) {
    let end = if last {
        data.len()
    } else {
        data.iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |at| at + 1)
    };
    let text = String::from_utf8_lossy(&data[..end]);
    let lines = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect();
    (lines, end as u64)
}

/// Reads `at..at + MAX_CHUNK` of a file, and whether that is the rest of it.
fn read_range(path: &FsPath, at: u64) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(at.min(length)))?;
    let mut data = Vec::new();
    file.take(MAX_CHUNK).read_to_end(&mut data)?;
    Ok((data, at + MAX_CHUNK >= length))
}

/// Reads a log file under `dir` from `position`.
pub fn read_file(dir: &FsPath, path: &str, position: u64) -> Result<Chunk, String> {
    let full = dir.join(path);
    let length = std::fs::metadata(&full)
        .map_err(|error| format!("cannot read {path}: {error}"))?
        .len();
    let mut chunk = Chunk::default();
    let mut at = position;
    if length < position {
        // Rotated: the rest of the previous file first.
        let previous = full.with_file_name(format!(
            "{}.1",
            full.file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default()
        ));
        match std::fs::metadata(&previous) {
            Ok(meta) if meta.len() >= position => {
                let mut rest = Vec::new();
                let mut from = position;
                loop {
                    let (data, whole) = read_range(&previous, from)
                        .map_err(|error| format!("cannot read {path}.1: {error}"))?;
                    from += data.len() as u64;
                    rest.extend(data);
                    if whole {
                        break;
                    }
                }
                chunk.previous = Some(rest);
            }
            _ => chunk.lost = true,
        }
        at = 0;
    }
    let (data, whole) =
        read_range(&full, at).map_err(|error| format!("cannot read {path}: {error}"))?;
    chunk.base = at;
    chunk.data = data;
    chunk.whole = whole;
    Ok(chunk)
}

/// The total length in a `Content-Range` (`bytes 0-99/1234`, `bytes */1234`).
pub fn total_length(content_range: &str) -> Option<u64> {
    content_range.rsplit_once('/')?.1.trim().parse().ok()
}

/// Reads a log at an address from `position`, with range requests.
async fn read_url(client: &reqwest::Client, log: &StoredLog, url: &str) -> Result<Chunk, String> {
    let get = |url: String, from: u64| {
        let mut request = client.get(url).header(
            reqwest::header::RANGE,
            format!("bytes={from}-{}", from + MAX_CHUNK - 1),
        );
        if let Some(username) = &log.username {
            request = request.basic_auth(username, log.password.as_deref());
        }
        request.send()
    };
    let read = |response: reqwest::Response| async move {
        let status = response.status();
        let total = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(total_length);
        let body = response.bytes().await.map_err(|error| error.to_string())?;
        Ok::<_, String>((status, total, body.to_vec()))
    };
    let position = log.position;
    let response = get(url.to_owned(), position)
        .await
        .map_err(|error| format!("cannot reach {url}: {error}"))?;
    let (status, total, body) = read(response).await?;
    let mut chunk = Chunk::default();
    let rotated = match status.as_u16() {
        206 => {
            chunk.base = position;
            chunk.whole = total.is_none_or(|total| position + body.len() as u64 >= total);
            chunk.data = body;
            return Ok(chunk);
        }
        // Nothing new, or a shorter file: rotated.
        416 => total.is_some_and(|total| total < position),
        // The whole file: the range was not honoured.
        200 => {
            if (body.len() as u64) >= position {
                chunk.base = position;
                chunk.data = body[usize::try_from(position)
                    .unwrap_or(usize::MAX)
                    .min(body.len())..]
                    .to_vec();
                chunk.whole = true;
                return Ok(chunk);
            }
            chunk.data = body;
            chunk.whole = true;
            true
        }
        code => return Err(format!("{url} answered {code}")),
    };
    if !rotated {
        chunk.base = position;
        chunk.whole = true;
        return Ok(chunk);
    }
    // Rotated: the rest of the previous file, then the new one.
    match get(format!("{url}.1"), position).await {
        Ok(response) => match read(response).await? {
            (code, _, previous) if code.as_u16() == 206 => chunk.previous = Some(previous),
            _ => chunk.lost = true,
        },
        Err(_) => chunk.lost = true,
    }
    if chunk.data.is_empty() {
        let response = get(url.to_owned(), 0)
            .await
            .map_err(|error| format!("cannot reach {url}: {error}"))?;
        let (status, total, body) = read(response).await?;
        if !status.is_success() && status.as_u16() != 416 {
            return Err(format!("{url} answered {status}"));
        }
        chunk.whole =
            status.as_u16() != 206 || total.is_none_or(|total| body.len() as u64 >= total);
        chunk.data = body;
    }
    chunk.base = 0;
    Ok(chunk)
}

/// Reads the station's access logs once a minute, and deletes the lines
/// past their retention once an hour. Never returns.
pub async fn run(pool: MySqlPool, dir: PathBuf) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "cannot set up the access log reader");
            return;
        }
    };
    let mut interval = tokio::time::interval(READ_EVERY);
    let mut ticks: u64 = 0;
    loop {
        interval.tick().await;
        if let Err(error) = read_all(&pool, &client, &dir).await {
            tracing::warn!(%error, "cannot read the access logs");
        }
        if ticks.is_multiple_of(60) {
            match purge(&pool).await {
                Ok(0) => {}
                Ok(deleted) => {
                    tracing::info!(deleted, "access log lines past their retention deleted")
                }
                Err(error) => tracing::warn!(%error, "cannot delete the old access log lines"),
            }
        }
        ticks += 1;
    }
}

pub(crate) async fn read_all(
    pool: &MySqlPool,
    client: &reqwest::Client,
    dir: &FsPath,
) -> ApiResult<()> {
    let logs: Vec<StoredLog> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{STORED} ORDER BY l.server_id"
    )))
    .fetch_all(pool)
    .await?;
    for log in logs {
        if !crate::modules::is_on(pool, Some(log.station_id), Module::Certification).await? {
            continue;
        }
        let read = match (LogSource::of(&log.source), &log.path, &log.url) {
            (LogSource::File, Some(path), _) => {
                let (dir, path, position) = (dir.to_owned(), path.clone(), log.position);
                tokio::task::spawn_blocking(move || read_file(&dir, &path, position))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()))
            }
            (LogSource::Url, _, Some(url)) => read_url(client, &log, url).await,
            _ => Err("no path or address".to_owned()),
        };
        let done = match read {
            Ok(chunk) => keep(pool, &log, chunk).await,
            Err(error) => failed(pool, &log, &error).await,
        };
        if let Err(error) = done {
            tracing::warn!(server = log.server_id, %error, "cannot keep the access log's lines");
        }
    }
    Ok(())
}

/// A reading failed: the error and since when; the gap is written once
/// the log can be read again.
async fn failed(pool: &MySqlPool, log: &StoredLog, error: &str) -> ApiResult<()> {
    if log.error.as_deref() != Some(error) {
        tracing::warn!(server = log.server_id, %error, "cannot read the access log");
    }
    sqlx::query(
        "UPDATE access_logs SET read_at = NOW(6), error = ?,
             error_since = COALESCE(error_since, NOW(6))
         WHERE server_id = ?",
    )
    .bind(error)
    .bind(log.server_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Keeps a reading's lines and moves the position past them, together.
async fn keep(pool: &MySqlPool, log: &StoredLog, chunk: Chunk) -> ApiResult<()> {
    let mut kept = Vec::new();
    if let Some(previous) = &chunk.previous {
        kept.extend(lines(previous, true).0);
    }
    // A chunk cut at MAX_CHUNK without any newline is one huge line: kept
    // whole, so that the reading moves on.
    let (current, mut used) = lines(&chunk.data, false);
    kept.extend(current);
    if used == 0 && !chunk.whole && !chunk.data.is_empty() {
        kept.extend(lines(&chunk.data, true).0);
        used = chunk.data.len() as u64;
    }
    let now = crate::rotation::now_ms();
    let mut tx = pool.begin().await?;
    for batch in kept.chunks(BATCH) {
        let mut insert = QueryBuilder::new(
            "INSERT INTO access_log_lines (station_id, server_id, logged_at, line, session,
                 address, mount, status, bytes, referer, user_agent, duration_s) ",
        );
        insert.push_values(batch, |mut row, line| {
            let parsed = parse(line);
            let logged_ms = parsed
                .as_ref()
                .map_or(now, |parsed| parsed.logged_at.as_millisecond());
            let session = parsed.as_ref().is_some_and(Parsed::is_session);
            row.push_bind(log.station_id)
                .push_bind(log.server_id)
                .push("FROM_UNIXTIME(")
                .push_bind_unseparated(seconds(logged_ms))
                .push_unseparated(")")
                .push_bind(line.as_str())
                .push_bind(session)
                .push_bind(parsed.as_ref().map(|parsed| parsed.address.clone()))
                .push_bind(parsed.as_ref().map(|parsed| parsed.path.clone()))
                .push_bind(parsed.as_ref().and_then(|parsed| parsed.status))
                .push_bind(parsed.as_ref().and_then(|parsed| parsed.bytes))
                .push_bind(parsed.as_ref().and_then(|parsed| parsed.referer.clone()))
                .push_bind(parsed.as_ref().and_then(|parsed| parsed.user_agent.clone()))
                .push_bind(parsed.as_ref().and_then(|parsed| parsed.duration_s));
        });
        insert.build().execute(&mut *tx).await?;
    }
    // A gap: the hours the log could not be read, or the lines a rotation
    // without its previous file lost.
    let gap = if chunk.lost {
        Some("rotated without its previous file")
    } else if log.error.is_some() {
        Some("could not be read")
    } else {
        None
    };
    if let Some(reason) = gap {
        let from = log
            .read_ok_at_ms
            .or(log.error_since_ms)
            .or(log.read_at_ms)
            .unwrap_or(now);
        sqlx::query(
            "INSERT INTO access_log_gaps (station_id, server_id, from_at, to_at, reason)
             VALUES (?, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), ?)",
        )
        .bind(log.station_id)
        .bind(log.server_id)
        .bind(seconds(from))
        .bind(seconds(now))
        .bind(reason)
        .execute(&mut *tx)
        .await?;
        tracing::warn!(server = log.server_id, reason, "a gap in the access log");
    }
    sqlx::query(
        "UPDATE access_logs SET position = ?, read_at = NOW(6), read_ok_at = NOW(6),
             error = NULL, error_since = NULL
         WHERE server_id = ?",
    )
    .bind(chunk.base + used)
    .bind(log.server_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Deletes the lines, gaps and downloads past the retention.
pub async fn purge(pool: &MySqlPool) -> ApiResult<u64> {
    let settings: CertificationSettings =
        settings::load(pool, CertificationSettings::SETTINGS_NAME).await?;
    let months = settings.retention_months.clamp(1, 25);
    let mut deleted = 0;
    loop {
        let rows = sqlx::query(
            "DELETE FROM access_log_lines WHERE logged_at < NOW(6) - INTERVAL ? MONTH LIMIT 10000",
        )
        .bind(months)
        .execute(pool)
        .await?
        .rows_affected();
        deleted += rows;
        if rows < 10_000 {
            break;
        }
    }
    sqlx::query("DELETE FROM access_log_gaps WHERE to_at < NOW(6) - INTERVAL ? MONTH")
        .bind(months)
        .execute(pool)
        .await?;
    sqlx::query(
        "DELETE FROM certification_downloads WHERE downloaded_at < NOW(6) - INTERVAL ? MONTH",
    )
    .bind(months)
    .execute(pool)
    .await?;
    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Days and exports
// ---------------------------------------------------------------------------

/// The start and end of a day of the station.
pub(crate) fn day_range(day: Date, tz: &TimeZone) -> ApiResult<(i64, i64)> {
    let start = day
        .to_zoned(tz.clone())
        .map_err(|error| ApiError::Invalid(error.to_string()))?;
    let end = day
        .tomorrow()
        .and_then(|next| next.to_zoned(tz.clone()))
        .map_err(|error| ApiError::Invalid(error.to_string()))?;
    Ok((
        start.timestamp().as_millisecond(),
        end.timestamp().as_millisecond(),
    ))
}

pub(crate) fn parse_day(day: &str) -> ApiResult<Date> {
    day.parse()
        .map_err(|_| ApiError::Invalid(format!("a day is YYYY-MM-DD (got {day:?})")))
}

pub(crate) async fn time_zone(pool: &MySqlPool, station_id: u64) -> ApiResult<TimeZone> {
    let station: StationSettings =
        settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await?;
    Ok(station.time_zone())
}

/// When a log could not be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct GapView {
    pub server_id: u64,
    pub from_unix_ms: i64,
    pub to_unix_ms: i64,
    pub reason: String,
}

async fn gaps(pool: &MySqlPool, station_id: u64, from: i64, to: i64) -> ApiResult<Vec<GapView>> {
    let rows: Vec<(u64, i64, i64, String)> = sqlx::query_as(
        "SELECT server_id, CAST(UNIX_TIMESTAMP(from_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(to_at) * 1000 AS SIGNED), reason
         FROM access_log_gaps
         WHERE station_id = ? AND from_at < FROM_UNIXTIME(?) AND to_at > FROM_UNIXTIME(?)
         ORDER BY from_at",
    )
    .bind(station_id)
    .bind(seconds(to))
    .bind(seconds(from))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(server_id, from_unix_ms, to_unix_ms, reason)| GapView {
            server_id,
            from_unix_ms,
            to_unix_ms,
            reason,
        })
        .collect())
}

#[derive(Clone, Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DaysQuery {
    /// The first day, `YYYY-MM-DD` in the station's time zone.
    pub from: String,
    /// The last day, included; 62 days at most.
    pub to: String,
}

/// A day of a server's log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct CertificationDay {
    /// `YYYY-MM-DD`, in the station's time zone.
    pub day: String,
    pub server_id: u64,
    pub lines: u64,
    pub sessions: u64,
    pub gaps: Vec<GapView>,
}

/// Per day and server: the lines kept, the sessions among them, and the
/// gaps. Admins only.
#[utoipa::path(get, path = "/certification/days", tag = "certification",
    params(DaysQuery),
    responses((status = 200, body = Vec<CertificationDay>), (status = "4XX", body = ErrorBody)))]
async fn certification_days(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<DaysQuery>,
) -> ApiResult<Json<Vec<CertificationDay>>> {
    user.require(Role::Admin)?;
    let (first, last) = (parse_day(&query.from)?, parse_day(&query.to)?);
    let span = (last - first).get_days();
    if !(0..MAX_DAYS).contains(&i64::from(span)) {
        return Err(ApiError::Invalid(format!(
            "from is before to, {MAX_DAYS} days at most"
        )));
    }
    let tz = time_zone(&state.pool, station.id()).await?;
    let (from, _) = day_range(first, &tz)?;
    let (_, to) = day_range(last, &tz)?;
    // Per hour, then each hour to its day in the station's time zone.
    let hours: Vec<(u64, i64, i64, i64)> = sqlx::query_as(
        "SELECT server_id,
                CAST(UNIX_TIMESTAMP(DATE_FORMAT(logged_at, '%Y-%m-%d %H:00:00')) * 1000 AS SIGNED),
                COUNT(*), CAST(SUM(session) AS SIGNED)
         FROM access_log_lines
         WHERE station_id = ? AND logged_at >= FROM_UNIXTIME(?) AND logged_at < FROM_UNIXTIME(?)
         GROUP BY server_id, DATE_FORMAT(logged_at, '%Y-%m-%d %H:00:00')",
    )
    .bind(station.id())
    .bind(seconds(from))
    .bind(seconds(to))
    .fetch_all(&state.pool)
    .await?;
    let gaps = gaps(&state.pool, station.id(), from, to).await?;
    let local_day = |ms: i64| {
        Timestamp::from_millisecond(ms)
            .map(|at| at.to_zoned(tz.clone()).date().to_string())
            .unwrap_or_default()
    };
    let mut out: Vec<CertificationDay> = Vec::new();
    for (server_id, hour, lines, sessions) in hours {
        let row = day_row(&mut out, local_day(hour), server_id);
        row.lines += u64::try_from(lines).unwrap_or(0);
        row.sessions += u64::try_from(sessions).unwrap_or(0);
    }
    // A gap shows on each day it touches.
    let mut day = first;
    while day <= last {
        let (start, end) = day_range(day, &tz)?;
        for gap in gaps
            .iter()
            .filter(|gap| gap.from_unix_ms < end && gap.to_unix_ms > start)
        {
            day_row(&mut out, day.to_string(), gap.server_id)
                .gaps
                .push(gap.clone());
        }
        day = day
            .tomorrow()
            .map_err(|error| ApiError::Internal(error.to_string()))?;
    }
    out.sort_by(|a, b| b.day.cmp(&a.day).then(a.server_id.cmp(&b.server_id)));
    Ok(Json(out))
}

/// The row of a day and a server, added when missing.
fn day_row(out: &mut Vec<CertificationDay>, day: String, server_id: u64) -> &mut CertificationDay {
    let at = match out
        .iter()
        .position(|row| row.day == day && row.server_id == server_id)
    {
        Some(at) => at,
        None => {
            out.push(CertificationDay {
                day,
                server_id,
                lines: 0,
                sessions: 0,
                gaps: Vec::new(),
            });
            out.len() - 1
        }
    };
    &mut out[at]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// The server's lines, unchanged.
    Raw,
    /// The sessions' lines.
    Combined,
    Csv,
    Json,
}

impl ExportFormat {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Combined => "combined",
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }

    pub(crate) fn of(key: &str) -> Self {
        match key {
            "combined" => Self::Combined,
            "csv" => Self::Csv,
            "json" => Self::Json,
            _ => Self::Raw,
        }
    }

    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Raw | Self::Combined => "log",
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Raw | Self::Combined => "text/plain; charset=utf-8",
            Self::Csv => "text/csv; charset=utf-8",
            Self::Json => "application/json",
        }
    }
}

#[derive(Clone, Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExportQuery {
    /// `YYYY-MM-DD`, in the station's time zone: the lines logged that day.
    pub day: String,
    pub server_id: u64,
    /// Only this mount (`/live`).
    pub mount: Option<String>,
    #[param(inline)]
    pub format: ExportFormat,
}

/// A session, as the CSV and JSON exports give it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionRow {
    pub server: String,
    pub mount: Option<String>,
    pub address: Option<String>,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
    pub status: Option<u16>,
    pub bytes: Option<u64>,
    pub started_at: Option<String>,
    pub ended_at: String,
    pub duration_s: Option<u32>,
}

type LineRow = (
    String,
    bool,
    Option<String>,
    Option<String>,
    Option<u16>,
    Option<u64>,
    Option<String>,
    Option<String>,
    Option<u32>,
    i64,
);

pub(crate) fn iso(ms: i64) -> String {
    Timestamp::from_millisecond(ms)
        .map(|at| at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// One CSV field, quoted when it needs to be.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

pub fn csv(rows: &[SessionRow]) -> String {
    let mut out = String::from(
        "server,mount,address,user_agent,referer,status,bytes,started_at,ended_at,duration_s\n",
    );
    for row in rows {
        let fields = [
            row.server.clone(),
            row.mount.clone().unwrap_or_default(),
            row.address.clone().unwrap_or_default(),
            row.user_agent.clone().unwrap_or_default(),
            row.referer.clone().unwrap_or_default(),
            row.status
                .map(|status| status.to_string())
                .unwrap_or_default(),
            row.bytes.map(|bytes| bytes.to_string()).unwrap_or_default(),
            row.started_at.clone().unwrap_or_default(),
            row.ended_at.clone(),
            row.duration_s
                .map(|duration| duration.to_string())
                .unwrap_or_default(),
        ];
        let line: Vec<String> = fields.iter().map(|field| csv_field(field)).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

/// A day of a server's log, as a file. Admins only; each download is
/// logged. `X-Heron-Gaps` lists the gaps of the day (ISO 8601 intervals,
/// UTC).
#[utoipa::path(get, path = "/certification/export", tag = "certification",
    params(ExportQuery),
    responses((status = 200, content_type = "text/plain", body = String),
              (status = "4XX", body = ErrorBody)))]
async fn export_day(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<ExportQuery>,
) -> ApiResult<Response> {
    user.require(Role::Admin)?;
    let server = server_name(&state.pool, station.id(), query.server_id).await?;
    let day = parse_day(&query.day)?;
    let tz = time_zone(&state.pool, station.id()).await?;
    let mount = query
        .mount
        .map(|mount| mount.trim().to_owned())
        .filter(|mount| !mount.is_empty());
    let (body, gaps) = export(
        &state.pool,
        station.id(),
        query.server_id,
        &server,
        day,
        &tz,
        mount.as_deref(),
        query.format,
    )
    .await?;
    let gaps: Vec<String> = gaps
        .iter()
        .map(|gap| format!("{}/{}", iso(gap.from_unix_ms), iso(gap.to_unix_ms)))
        .collect();
    sqlx::query(
        "INSERT INTO certification_downloads
             (station_id, user_id, username, server_id, day, mount, format, downloaded_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, NOW(6))",
    )
    .bind(station.id())
    .bind(user.id)
    .bind(&user.username)
    .bind(query.server_id)
    .bind(day.to_string())
    .bind(&mount)
    .bind(query.format.key())
    .execute(&state.pool)
    .await?;
    tracing::info!(
        user = %user.username,
        server = query.server_id,
        day = %day,
        mount = ?mount,
        format = query.format.key(),
        "certification export downloaded"
    );
    let file = format!(
        "{}-{}-{day}.{}",
        safe_name(&station.code()),
        safe_name(&server),
        query.format.extension()
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(query.format.content_type()),
    );
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{file}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    if let Ok(value) = HeaderValue::from_str(&gaps.join(", ")) {
        headers.insert("x-heron-gaps", value);
    }
    Ok((StatusCode::OK, headers, body).into_response())
}

/// A day of a server's log in a format, and the gaps of the day.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn export(
    pool: &MySqlPool,
    station_id: u64,
    server_id: u64,
    server: &str,
    day: Date,
    tz: &TimeZone,
    mount: Option<&str>,
    format: ExportFormat,
) -> ApiResult<(String, Vec<GapView>)> {
    let (from, to) = day_range(day, tz)?;
    let rows: Vec<LineRow> = sqlx::query_as(
        "SELECT line, session, address, mount, status, bytes, referer, user_agent, duration_s,
                CAST(UNIX_TIMESTAMP(logged_at) * 1000 AS SIGNED)
         FROM access_log_lines
         WHERE server_id = ? AND station_id = ?
           AND logged_at >= FROM_UNIXTIME(?) AND logged_at < FROM_UNIXTIME(?)
           AND (? IS NULL OR mount = ?)
           AND (? OR session)
         ORDER BY id",
    )
    .bind(server_id)
    .bind(station_id)
    .bind(seconds(from))
    .bind(seconds(to))
    .bind(mount)
    .bind(mount)
    .bind(format == ExportFormat::Raw)
    .fetch_all(pool)
    .await?;
    let sessions = || {
        rows.iter()
            .map(|row| SessionRow {
                server: server.to_owned(),
                mount: row.3.clone(),
                address: row.2.clone(),
                user_agent: row.7.clone(),
                referer: row.6.clone(),
                status: row.4,
                bytes: row.5,
                started_at: row
                    .8
                    .map(|duration| iso(row.9 - i64::from(duration) * 1_000)),
                ended_at: iso(row.9),
                duration_s: row.8,
            })
            .collect::<Vec<_>>()
    };
    let body = match format {
        ExportFormat::Raw | ExportFormat::Combined => {
            let mut text = rows
                .iter()
                .map(|row| row.0.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                text.push('\n');
            }
            text
        }
        ExportFormat::Csv => csv(&sessions()),
        ExportFormat::Json => serde_json::to_string(&sessions())
            .map_err(|error| ApiError::Internal(error.to_string()))?,
    };
    let gaps: Vec<GapView> = gaps(pool, station_id, from, to)
        .await?
        .into_iter()
        .filter(|gap| gap.server_id == server_id)
        .collect();
    Ok((body, gaps))
}

/// A name for a file: letters, digits, `-` and `_`.
pub(crate) fn safe_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    safe.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTENER: &str = r#"203.0.113.7 - - [07/Nov/2026:20:41:05 +0100] "GET /live HTTP/1.1" 200 5242880 "https://radio.example.org/" "VLC/3.0.20 LibVLC/3.0.20" 1342"#;

    #[test]
    fn a_listener_line_is_a_session() {
        let parsed = parse(LISTENER).unwrap();
        assert_eq!(parsed.address, "203.0.113.7");
        assert_eq!(
            parsed.logged_at,
            "2026-11-07T19:41:05Z".parse::<Timestamp>().unwrap()
        );
        assert_eq!(
            (parsed.method.as_str(), parsed.path.as_str()),
            ("GET", "/live")
        );
        assert_eq!((parsed.status, parsed.bytes), (Some(200), Some(5_242_880)));
        assert_eq!(
            parsed.referer.as_deref(),
            Some("https://radio.example.org/")
        );
        assert_eq!(
            parsed.user_agent.as_deref(),
            Some("VLC/3.0.20 LibVLC/3.0.20")
        );
        assert_eq!(parsed.duration_s, Some(1342));
        assert!(parsed.is_session());
    }

    #[test]
    fn other_lines_are_not_sessions() {
        for line in [
            r#"198.51.100.2 - source [07/Nov/2026:20:00:00 +0000] "SOURCE /live HTTP/1.0" 200 0 "-" "heron" 3600"#,
            r#"198.51.100.2 - admin [07/Nov/2026:20:00:00 +0000] "GET /admin/stats HTTP/1.1" 200 812 "-" "curl/8" 0"#,
            r#"203.0.113.9 - - [07/Nov/2026:20:00:00 +0000] "GET /status-json.xsl HTTP/1.1" 200 812 "-" "Mozilla/5.0" 0"#,
            r#"203.0.113.9 - - [07/Nov/2026:20:00:00 +0000] "GET /missing HTTP/1.1" 404 0 "-" "Mozilla/5.0" 0"#,
        ] {
            let parsed = parse(line).unwrap();
            assert!(!parsed.is_session(), "{line}");
        }
    }

    #[test]
    fn odd_lines_parse_as_far_as_they_go() {
        assert_eq!(parse("not a log line"), None);
        assert_eq!(parse(""), None);
        // A query, a quote inside the User-Agent, no referer, no length.
        let parsed = parse(
            r#"2001:db8::1 - - [01/Jan/2027:00:00:30 -0500] "GET /aac?token=x HTTP/1.1" 200 10 "-" "Player \"X\" 1.0""#,
        )
        .unwrap();
        assert_eq!(parsed.address, "2001:db8::1");
        assert_eq!(parsed.path, "/aac");
        assert_eq!(parsed.referer, None);
        assert_eq!(parsed.user_agent.as_deref(), Some(r#"Player "X" 1.0"#));
        assert_eq!(parsed.duration_s, None);
        assert!(!parsed.is_session(), "no length: not a full session line");
        assert_eq!(
            parsed.logged_at,
            "2027-01-01T05:00:30Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn only_full_lines_are_taken() {
        let (taken, used) = lines(b"one\ntwo\r\nthree", false);
        assert_eq!(taken, ["one", "two"]);
        assert_eq!(used, 9);
        let (taken, used) = lines(b"one\nlast", true);
        assert_eq!(taken, ["one", "last"]);
        assert_eq!(used, 8);
        assert_eq!(lines(b"partial", false), (Vec::new(), 0));
    }

    #[test]
    fn paths_stay_in_the_folder() {
        assert_eq!(
            check_path(Some(" main/access.log ".to_owned())).unwrap(),
            "main/access.log"
        );
        for bad in [
            "/var/log/icecast/access.log",
            "../etc/passwd",
            "a/../../b",
            "",
            "./a",
        ] {
            assert!(check_path(Some(bad.to_owned())).is_err(), "{bad}");
        }
        assert!(
            check_url(Some(
                "https://stream.example.org/logs/access.log".to_owned()
            ))
            .is_ok()
        );
        assert!(check_url(Some("ftp://stream.example.org/access.log".to_owned())).is_err());
    }

    #[test]
    fn content_ranges_give_the_length() {
        assert_eq!(total_length("bytes 0-99/1234"), Some(1234));
        assert_eq!(total_length("bytes */1234"), Some(1234));
        assert_eq!(total_length("bytes 0-99/*"), None);
    }

    /// A folder of its own under the temporary folder.
    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("heron-certification-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("main")).unwrap();
        dir
    }

    #[test]
    fn a_file_is_read_from_its_position_and_follows_rotation() {
        let dir = folder("rotation");
        let log = dir.join("main/access.log");
        std::fs::write(&log, "a\nb\nc").unwrap();
        let chunk = read_file(&dir, "main/access.log", 0).unwrap();
        let (taken, used) = lines(&chunk.data, false);
        assert_eq!(
            (taken.as_slice(), used),
            (["a".to_owned(), "b".to_owned()].as_slice(), 4)
        );
        // More written: read from the position, nothing twice.
        std::fs::write(&log, "a\nb\nc\nd\n").unwrap();
        let chunk = read_file(&dir, "main/access.log", 4).unwrap();
        assert_eq!(chunk.base, 4);
        assert_eq!(lines(&chunk.data, false).0, ["c", "d"]);
        // Rotated: the rest of the previous file, then the new one.
        std::fs::write(&log, "e\n").unwrap();
        std::fs::write(dir.join("main/access.log.1"), "a\nb\nc\nd\nlate\n").unwrap();
        let chunk = read_file(&dir, "main/access.log", 8).unwrap();
        assert!(!chunk.lost);
        assert_eq!(lines(chunk.previous.as_deref().unwrap(), true).0, ["late"]);
        assert_eq!(
            (chunk.base, lines(&chunk.data, false).0),
            (0, vec!["e".to_owned()])
        );
        // Rotated without the previous file: the lines since are lost.
        std::fs::remove_file(dir.join("main/access.log.1")).unwrap();
        std::fs::write(&log, "").unwrap();
        let chunk = read_file(&dir, "main/access.log", 2).unwrap();
        assert!(chunk.lost && chunk.previous.is_none());
        assert!(read_file(&dir, "main/missing.log", 0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exports_quote_what_needs_it() {
        let row = SessionRow {
            server: "Main".to_owned(),
            mount: Some("/live".to_owned()),
            address: Some("203.0.113.7".to_owned()),
            user_agent: Some("Player, \"X\"".to_owned()),
            referer: None,
            status: Some(200),
            bytes: Some(10),
            started_at: Some("2026-11-07T19:18:43Z".to_owned()),
            ended_at: "2026-11-07T19:41:05Z".to_owned(),
            duration_s: Some(1342),
        };
        assert_eq!(
            csv(&[row]),
            "server,mount,address,user_agent,referer,status,bytes,started_at,ended_at,duration_s\n\
             Main,/live,203.0.113.7,\"Player, \"\"X\"\"\",,200,10,2026-11-07T19:18:43Z,2026-11-07T19:41:05Z,1342\n"
        );
        assert_eq!(safe_name("Main server/1"), "Main-server-1");
    }

    #[test]
    fn days_follow_the_station_time_zone() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let (from, to) = day_range(parse_day("2026-11-07").unwrap(), &paris).unwrap();
        assert_eq!(iso(from), "2026-11-06T23:00:00Z");
        assert_eq!(iso(to), "2026-11-07T23:00:00Z");
        assert!(parse_day("07/11/2026").is_err());
    }
}
