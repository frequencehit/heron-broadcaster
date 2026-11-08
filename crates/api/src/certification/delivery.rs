// SPDX-License-Identifier: AGPL-3.0-or-later

//! Certification deliveries (`docs/certification.md#automatic-delivery`): a
//! day's export sent by FTPS to a certification body every day at a
//! set time, retried, logged and alerted on failure.
//!
//! A day to send is a row of `certification_delivery_runs`, made at the
//! delivery's time (once per day: `scheduled_day` is unique) or by *Send
//! now*; the worker sends the rows that are due, records each attempt, and
//! keeps the last 30 per delivery. The rows survive a restart.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use heron_core::Module;
use jiff::Timestamp;
use jiff::civil::{Date, Time};
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::send::{Sender, Target};
use super::{ExportFormat, day_range, export, iso, parse_day, safe_name, server_name, time_zone};
use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::some_or_null;
use crate::stations::Station;
use crate::webhooks::{Payload, WebhookEvent, Webhooks};

/// How often the worker looks for days to send.
const TICK: Duration = Duration::from_secs(15);
/// Attempts before a day has failed.
pub const MAX_ATTEMPTS: u16 = 5;
/// The days sent kept per delivery.
const KEEP_RUNS: u32 = 30;
/// A day being sent is taken again after this long, if the API stopped.
const CLAIM_MINUTES: u32 = 10;
const ERROR_CHARS: usize = 1_000;
pub const DEFAULT_FILE_NAME: &str = "{station}-{server}-{date}.{ext}";

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(
            list_certification_deliveries,
            create_certification_delivery
        ))
        .routes(routes!(
            get_certification_delivery,
            update_certification_delivery,
            delete_certification_delivery
        ))
        .routes(routes!(test_certification_delivery))
        .routes(routes!(send_certification_day))
        .routes(routes!(list_certification_runs))
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryProtocol {
    /// FTP with explicit TLS (`AUTH TLS`), passive mode. SFTP comes next.
    Ftps,
}

impl DeliveryProtocol {
    fn key(self) -> &'static str {
        match self {
            Self::Ftps => "ftps",
        }
    }

    fn of(_key: &str) -> Self {
        Self::Ftps
    }

    fn default_port(self) -> u16 {
        match self {
            Self::Ftps => 21,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryCompression {
    #[default]
    None,
    Gzip,
}

impl DeliveryCompression {
    fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
        }
    }

    fn of(key: &str) -> Self {
        if key == "gzip" {
            Self::Gzip
        } else {
            Self::None
        }
    }
}

/// A delivery as stored, secrets included.
#[derive(Clone, Debug, sqlx::FromRow)]
struct Stored {
    id: u64,
    station_id: u64,
    name: String,
    enabled: bool,
    server_id: u64,
    mount: Option<String>,
    protocol: String,
    host: String,
    port: u16,
    username: String,
    password: Option<String>,
    remote_dir: String,
    file_name: String,
    format: String,
    compression: String,
    send_at_min: u16,
    days_back: u8,
}

const STORED: &str = "SELECT id, station_id, name, enabled, server_id, mount,
        CAST(protocol AS CHAR) AS protocol, host, port, username, password, remote_dir, file_name, format, CAST(compression AS CHAR) AS compression, send_at_min,
        days_back
     FROM certification_deliveries";

impl Stored {
    fn target(&self) -> Target {
        Target {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: self
                .password
                .clone()
                .filter(|password| !password.is_empty()),
            remote_dir: self.remote_dir.clone(),
        }
    }

    fn format(&self) -> ExportFormat {
        ExportFormat::of(&self.format)
    }

    fn compression(&self) -> DeliveryCompression {
        DeliveryCompression::of(&self.compression)
    }
}

fn hh_mm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// A delivery to a certification body, its secrets never sent back.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CertificationDelivery {
    pub id: u64,
    /// The body it sends to.
    pub name: String,
    pub enabled: bool,
    pub server_id: u64,
    pub server_name: String,
    /// Only this mount; `null` for every mount.
    pub mount: Option<String>,
    pub protocol: DeliveryProtocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password_set: bool,
    /// Empty: the login's folder.
    pub remote_dir: String,
    /// The file name's pattern (`{station}-{server}-{date}.{ext}`).
    pub file_name: String,
    pub format: ExportFormat,
    pub compression: DeliveryCompression,
    /// `HH:MM`, in the station's time zone.
    pub send_at: String,
    /// The day sent: 1 the day before, up to 7.
    pub days_back: u8,
    /// The last day sent.
    pub last: Option<DeliveryRun>,
}

impl CertificationDelivery {
    fn new(stored: Stored, server_name: String, last: Option<DeliveryRun>) -> Self {
        Self {
            id: stored.id,
            format: stored.format(),
            compression: stored.compression(),
            protocol: DeliveryProtocol::of(&stored.protocol),
            name: stored.name,
            enabled: stored.enabled,
            server_id: stored.server_id,
            server_name,
            mount: stored.mount,
            host: stored.host,
            port: stored.port,
            username: stored.username,
            password_set: stored.password.is_some_and(|password| !password.is_empty()),
            remote_dir: stored.remote_dir,
            file_name: stored.file_name,
            send_at: hh_mm(stored.send_at_min),
            days_back: stored.days_back,
            last,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CertificationDeliveryInput {
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub server_id: u64,
    #[serde(default)]
    pub mount: Option<String>,
    pub protocol: DeliveryProtocol,
    pub host: String,
    /// 21 when left out.
    #[serde(default)]
    pub port: Option<u16>,
    pub username: String,
    /// Left out, the stored one is kept; `null` removes it.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<String>)]
    pub password: Option<Option<String>>,
    #[serde(default)]
    pub remote_dir: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub format: Option<ExportFormat>,
    #[serde(default)]
    pub compression: Option<DeliveryCompression>,
    /// `HH:MM`, `04:00` when left out.
    #[serde(default)]
    pub send_at: Option<String>,
    /// 1 (the day before, when left out) to 7.
    #[serde(default)]
    pub days_back: Option<u8>,
}

/// What a file name's tokens are.
pub struct Names<'a> {
    pub station: &'a str,
    pub server: &'a str,
    pub mount: Option<&'a str>,
    pub day: Date,
    /// `log`, `csv` or `json`, `.gz` added when compressed.
    pub ext: &'a str,
}

/// The file name a pattern gives.
pub fn file_name(pattern: &str, names: &Names<'_>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| "a { without its }".to_owned())?;
        let value = match &after[..close] {
            "station" => safe_name(names.station),
            "server" => safe_name(names.server),
            "mount" => names
                .mount
                .map(safe_name)
                .filter(|mount| !mount.is_empty())
                .unwrap_or_else(|| "all".to_owned()),
            "date" => names.day.to_string(),
            "yyyy" => format!("{:04}", names.day.year()),
            "mm" => format!("{:02}", names.day.month()),
            "dd" => format!("{:02}", names.day.day()),
            "ext" => names.ext.to_owned(),
            token => return Err(format!("unknown token {{{token}}}")),
        };
        out.push_str(&value);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    let out = out.trim().to_owned();
    if out.is_empty() || out == "." || out == ".." {
        return Err("the file name is empty".to_owned());
    }
    if out.contains(['/', '\\']) || out.contains(|c: char| c.is_control()) {
        return Err("the file name has no / and no control character".to_owned());
    }
    if out.len() > 255 {
        return Err("the file name is longer than 255 bytes".to_owned());
    }
    Ok(out)
}

fn extension(format: ExportFormat, compression: DeliveryCompression) -> String {
    match compression {
        DeliveryCompression::None => format.extension().to_owned(),
        DeliveryCompression::Gzip => format!("{}.gz", format.extension()),
    }
}

fn parse_time(text: &str) -> Result<u16, String> {
    let time: Time = text
        .trim()
        .parse()
        .map_err(|_| format!("send_at is HH:MM (got {text:?})"))?;
    Ok(u16::try_from(time.hour()).unwrap_or(0) * 60 + u16::try_from(time.minute()).unwrap_or(0))
}

/// What is saved, checked.
struct Checked {
    name: String,
    enabled: bool,
    server_id: u64,
    mount: Option<String>,
    protocol: DeliveryProtocol,
    host: String,
    port: u16,
    username: String,
    password: Option<String>,
    remote_dir: String,
    file_name: String,
    format: ExportFormat,
    compression: DeliveryCompression,
    send_at_min: u16,
    days_back: u8,
}

fn check(input: CertificationDeliveryInput, before: Option<&Stored>) -> Result<Checked, String> {
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 255 {
        return Err("the name is 1 to 255 characters".to_owned());
    }
    let host = input.host.trim().to_owned();
    if host.is_empty()
        || host.len() > 255
        || host.contains(|c: char| c.is_whitespace() || c == '/' || c.is_control())
    {
        return Err("the host is a name or an address, without spaces or /".to_owned());
    }
    let port = input
        .port
        .filter(|port| *port != 0)
        .unwrap_or_else(|| input.protocol.default_port());
    let username = input.username.trim().to_owned();
    if username.is_empty() || username.len() > 255 {
        return Err("the login is 1 to 255 characters".to_owned());
    }
    let mount = input
        .mount
        .map(|mount| mount.trim().to_owned())
        .filter(|mount| !mount.is_empty());
    if mount.as_ref().is_some_and(|mount| mount.len() > 255) {
        return Err("the mount is 255 characters at most".to_owned());
    }
    let keep = |new: Option<Option<String>>, old: Option<String>| match new {
        Some(value) => value.filter(|value| !value.is_empty()),
        None => old,
    };
    let password = keep(input.password, before.and_then(|d| d.password.clone()));
    if password
        .as_ref()
        .is_some_and(|password| password.len() > 255)
    {
        return Err("the password is 255 characters at most".to_owned());
    }
    if password.is_none() {
        return Err("FTPS needs a password".to_owned());
    }
    let remote_dir = input.remote_dir.unwrap_or_default().trim().to_owned();
    if remote_dir.len() > 1_024 || remote_dir.contains(|c: char| c.is_control()) {
        return Err("the folder is 1024 characters at most".to_owned());
    }
    let pattern = input
        .file_name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_FILE_NAME.to_owned());
    let format = input.format.unwrap_or(ExportFormat::Raw);
    let compression = input.compression.unwrap_or_default();
    file_name(
        &pattern,
        &Names {
            station: "station",
            server: "server",
            mount: mount.as_deref(),
            day: Date::constant(2026, 11, 7),
            ext: &extension(format, compression),
        },
    )?;
    let send_at_min = parse_time(input.send_at.as_deref().unwrap_or("04:00"))?;
    let days_back = input.days_back.unwrap_or(1);
    if !(1..=7).contains(&days_back) {
        return Err("days_back is 1 to 7".to_owned());
    }
    Ok(Checked {
        name,
        enabled: input.enabled,
        server_id: input.server_id,
        mount,
        protocol: input.protocol,
        host,
        port,
        username,
        password,
        remote_dir,
        file_name: pattern,
        format,
        compression,
        send_at_min,
        days_back,
    })
}

async fn stored(pool: &MySqlPool, station_id: u64, id: u64) -> ApiResult<Stored> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{STORED} WHERE id = ? AND station_id = ?"
    )))
    .bind(id)
    .bind(station_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("certification delivery"))
}

async fn view(pool: &MySqlPool, stored: Stored) -> ApiResult<CertificationDelivery> {
    let server: String = sqlx::query_scalar("SELECT name FROM stream_servers WHERE id = ?")
        .bind(stored.server_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_default();
    let last = runs(pool, stored.id, 1).await?.into_iter().next();
    Ok(CertificationDelivery::new(stored, server, last))
}

/// The station's deliveries, each with its last day sent. Admins only.
#[utoipa::path(get, path = "/certification/deliveries", tag = "certification",
    responses((status = 200, body = Vec<CertificationDelivery>), (status = "4XX", body = ErrorBody)))]
async fn list_certification_deliveries(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Vec<CertificationDelivery>>> {
    user.require(Role::Admin)?;
    let all: Vec<Stored> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{STORED} WHERE station_id = ? ORDER BY name, id"
    )))
    .bind(station.id())
    .fetch_all(&state.pool)
    .await?;
    let mut out = Vec::with_capacity(all.len());
    for stored in all {
        out.push(view(&state.pool, stored).await?);
    }
    Ok(Json(out))
}

/// A delivery. Admins only.
#[utoipa::path(get, path = "/certification/deliveries/{id}", tag = "certification",
    params(("id" = u64, Path)),
    responses((status = 200, body = CertificationDelivery), (status = "4XX", body = ErrorBody)))]
async fn get_certification_delivery(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<CertificationDelivery>> {
    user.require(Role::Admin)?;
    let stored = stored(&state.pool, station.id(), id).await?;
    Ok(Json(view(&state.pool, stored).await?))
}

async fn save(
    pool: &MySqlPool,
    station_id: u64,
    id: Option<u64>,
    checked: &Checked,
) -> ApiResult<u64> {
    let query = match id {
        None => {
            "INSERT INTO certification_deliveries (name, enabled, server_id, mount, protocol, host,
                 port, username, password, remote_dir, file_name, format, compression,
                 send_at_min, days_back, station_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        }
        Some(_) => {
            "UPDATE certification_deliveries SET name = ?, enabled = ?, server_id = ?, mount = ?,
                 protocol = ?, host = ?, port = ?, username = ?, password = ?,
                 remote_dir = ?, file_name = ?, format = ?, compression = ?,
                 send_at_min = ?, days_back = ?
             WHERE station_id = ? AND id = ?"
        }
    };
    let mut query = sqlx::query(query)
        .bind(&checked.name)
        .bind(checked.enabled)
        .bind(checked.server_id)
        .bind(&checked.mount)
        .bind(checked.protocol.key())
        .bind(&checked.host)
        .bind(checked.port)
        .bind(&checked.username)
        .bind(&checked.password)
        .bind(&checked.remote_dir)
        .bind(&checked.file_name)
        .bind(checked.format.key())
        .bind(checked.compression.key())
        .bind(checked.send_at_min)
        .bind(checked.days_back)
        .bind(station_id);
    if let Some(id) = id {
        query = query.bind(id);
    }
    let done = query.execute(pool).await?;
    Ok(id.unwrap_or_else(|| done.last_insert_id()))
}

/// Adds a delivery. Admins only.
#[utoipa::path(post, path = "/certification/deliveries", tag = "certification",
    request_body = CertificationDeliveryInput,
    responses((status = 201, body = CertificationDelivery), (status = "4XX", body = ErrorBody)))]
async fn create_certification_delivery(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(input): Json<CertificationDeliveryInput>,
) -> ApiResult<(StatusCode, Json<CertificationDelivery>)> {
    user.require(Role::Admin)?;
    server_name(&state.pool, station.id(), input.server_id).await?;
    let checked = check(input, None).map_err(ApiError::Invalid)?;
    let id = save(&state.pool, station.id(), None, &checked).await?;
    tracing::info!(user = %user.username, delivery = id, name = %checked.name, "certification delivery added");
    let stored = stored(&state.pool, station.id(), id).await?;
    Ok((StatusCode::CREATED, Json(view(&state.pool, stored).await?)))
}

/// Replaces a delivery; the secrets left out are kept. Admins only.
#[utoipa::path(put, path = "/certification/deliveries/{id}", tag = "certification",
    params(("id" = u64, Path)), request_body = CertificationDeliveryInput,
    responses((status = 200, body = CertificationDelivery), (status = "4XX", body = ErrorBody)))]
async fn update_certification_delivery(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(input): Json<CertificationDeliveryInput>,
) -> ApiResult<Json<CertificationDelivery>> {
    user.require(Role::Admin)?;
    let before = stored(&state.pool, station.id(), id).await?;
    server_name(&state.pool, station.id(), input.server_id).await?;
    let checked = check(input, Some(&before)).map_err(ApiError::Invalid)?;
    save(&state.pool, station.id(), Some(id), &checked).await?;
    tracing::info!(user = %user.username, delivery = id, "certification delivery changed");
    let stored = stored(&state.pool, station.id(), id).await?;
    Ok(Json(view(&state.pool, stored).await?))
}

/// Deletes a delivery and its log. Admins only.
#[utoipa::path(delete, path = "/certification/deliveries/{id}", tag = "certification",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_certification_delivery(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    stored(&state.pool, station.id(), id).await?;
    sqlx::query("DELETE FROM certification_deliveries WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    tracing::info!(user = %user.username, delivery = id, "certification delivery deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// What *Test connection* found.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DeliveryTest {
    pub ok: bool,
    pub error: Option<String>,
}

/// Connects, logs in and opens the folder, writing nothing. Admins only.
#[utoipa::path(post, path = "/certification/deliveries/{id}/test", tag = "certification",
    params(("id" = u64, Path)),
    responses((status = 200, body = DeliveryTest), (status = "4XX", body = ErrorBody)))]
async fn test_certification_delivery(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<DeliveryTest>> {
    user.require(Role::Admin)?;
    let stored = stored(&state.pool, station.id(), id).await?;
    let sender = Sender::new().map_err(ApiError::Internal)?;
    let answer = match sender.test(&stored.target()).await {
        Ok(()) => DeliveryTest {
            ok: true,
            error: None,
        },
        Err(error) => DeliveryTest {
            ok: false,
            error: Some(error),
        },
    };
    tracing::info!(user = %user.username, delivery = id, ok = answer.ok, "certification delivery tested");
    Ok(Json(answer))
}

// ---------------------------------------------------------------------------
// The days sent
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRunState {
    /// Waiting for its first or next attempt.
    Pending,
    Sending,
    Delivered,
    /// Every attempt failed.
    Failed,
}

impl DeliveryRunState {
    fn of(key: &str) -> Self {
        match key {
            "sending" => Self::Sending,
            "delivered" => Self::Delivered,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// An attempt to send a day.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DeliveryAttempt {
    pub at_unix_ms: i64,
    pub ok: bool,
    pub error: Option<String>,
    pub file_name: Option<String>,
    pub bytes: Option<u64>,
}

/// A day sent, at its time or by *Send now*.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DeliveryRun {
    pub id: u64,
    /// `YYYY-MM-DD`, in the station's time zone.
    pub day: String,
    /// Sent by *Send now*, not at its time.
    pub manual: bool,
    pub state: DeliveryRunState,
    pub file_name: Option<String>,
    pub bytes: Option<u64>,
    /// The day's gaps, ISO 8601 intervals in UTC: the day was sent with them.
    pub gaps: Vec<String>,
    /// The last attempt's error.
    pub error: Option<String>,
    pub created_at_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
    /// While pending: when the next attempt is.
    pub next_attempt_unix_ms: Option<i64>,
    /// Oldest first.
    pub attempts: Vec<DeliveryAttempt>,
}

type RunRow = (
    u64,
    String,
    bool,
    String,
    Option<String>,
    Option<u64>,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
    Option<i64>,
);

type AttemptRow = (u64, i64, bool, Option<String>, Option<String>, Option<u64>);

/// A delivery's last days sent, newest first.
async fn runs(pool: &MySqlPool, delivery_id: u64, limit: u32) -> ApiResult<Vec<DeliveryRun>> {
    let rows: Vec<RunRow> = sqlx::query_as(
        "SELECT id, DATE_FORMAT(day, '%Y-%m-%d'), scheduled_day IS NULL, CAST(state AS CHAR),
                file_name, bytes, gaps, error,
                CAST(UNIX_TIMESTAMP(created_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(finished_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(next_attempt_at) * 1000 AS SIGNED)
         FROM certification_delivery_runs WHERE delivery_id = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(delivery_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let ids: Vec<u64> = rows.iter().map(|row| row.0).collect();
    let mut attempts: Vec<(u64, DeliveryAttempt)> = Vec::new();
    if !ids.is_empty() {
        let mut query = sqlx::QueryBuilder::new(
            "SELECT run_id, CAST(UNIX_TIMESTAMP(attempted_at) * 1000 AS SIGNED), ok, error,
                    file_name, bytes
             FROM certification_delivery_attempts WHERE run_id IN (",
        );
        let mut list = query.separated(", ");
        for id in &ids {
            list.push_bind(*id);
        }
        query.push(") ORDER BY id");
        let found: Vec<AttemptRow> = query.build_query_as().fetch_all(pool).await?;
        attempts = found
            .into_iter()
            .map(|(run, at_unix_ms, ok, error, file_name, bytes)| {
                (
                    run,
                    DeliveryAttempt {
                        at_unix_ms,
                        ok,
                        error,
                        file_name,
                        bytes,
                    },
                )
            })
            .collect();
    }
    Ok(rows
        .into_iter()
        .map(|row| {
            let state = DeliveryRunState::of(&row.3);
            DeliveryRun {
                id: row.0,
                day: row.1,
                manual: row.2,
                state,
                file_name: row.4,
                bytes: row.5,
                gaps: row
                    .6
                    .map(|gaps| {
                        gaps.split(',')
                            .map(|gap| gap.trim().to_owned())
                            .filter(|gap| !gap.is_empty())
                            .collect()
                    })
                    .unwrap_or_default(),
                error: row.7,
                created_at_unix_ms: row.8,
                finished_at_unix_ms: row.9,
                next_attempt_unix_ms: row.10.filter(|_| state == DeliveryRunState::Pending),
                attempts: attempts
                    .iter()
                    .filter(|(run, _)| *run == row.0)
                    .map(|(_, attempt)| attempt.clone())
                    .collect(),
            }
        })
        .collect())
}

/// A delivery's last 30 days sent, newest first, with their attempts.
/// Admins only.
#[utoipa::path(get, path = "/certification/deliveries/{id}/runs", tag = "certification",
    params(("id" = u64, Path)),
    responses((status = 200, body = Vec<DeliveryRun>), (status = "4XX", body = ErrorBody)))]
async fn list_certification_runs(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<Vec<DeliveryRun>>> {
    user.require(Role::Admin)?;
    stored(&state.pool, station.id(), id).await?;
    Ok(Json(runs(&state.pool, id, KEEP_RUNS).await?))
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SendDay {
    /// `YYYY-MM-DD`, in the station's time zone, today at the latest.
    pub day: String,
}

/// Keeps a delivery's last 30 days sent.
async fn prune(pool: &MySqlPool, delivery_id: u64) -> ApiResult<()> {
    sqlx::query(
        "DELETE FROM certification_delivery_runs WHERE delivery_id = ? AND id NOT IN (
             SELECT id FROM (SELECT id FROM certification_delivery_runs WHERE delivery_id = ?
                             ORDER BY id DESC LIMIT ?) kept)",
    )
    .bind(delivery_id)
    .bind(delivery_id)
    .bind(KEEP_RUNS)
    .execute(pool)
    .await?;
    Ok(())
}

/// Sends a day now, with the same retries, even when the delivery is off.
/// Admins only.
#[utoipa::path(post, path = "/certification/deliveries/{id}/send", tag = "certification",
    params(("id" = u64, Path)), request_body = SendDay,
    responses((status = 202, body = DeliveryRun), (status = "4XX", body = ErrorBody)))]
async fn send_certification_day(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(input): Json<SendDay>,
) -> ApiResult<(StatusCode, Json<DeliveryRun>)> {
    user.require(Role::Admin)?;
    stored(&state.pool, station.id(), id).await?;
    let day = parse_day(&input.day)?;
    let tz = time_zone(&state.pool, station.id()).await?;
    if day > Timestamp::now().to_zoned(tz).date() {
        return Err(ApiError::Invalid(
            "the day is today at the latest".to_owned(),
        ));
    }
    let run = sqlx::query(
        "INSERT INTO certification_delivery_runs (delivery_id, day, next_attempt_at)
         VALUES (?, ?, NOW(6))",
    )
    .bind(id)
    .bind(day.to_string())
    .execute(&state.pool)
    .await?
    .last_insert_id();
    prune(&state.pool, id).await?;
    tracing::info!(user = %user.username, delivery = id, day = %day, "certification day sent now");
    let run = runs(&state.pool, id, 1)
        .await?
        .into_iter()
        .find(|found| found.id == run)
        .ok_or_else(|| ApiError::Internal("the day just queued is gone".to_owned()))?;
    Ok((StatusCode::ACCEPTED, Json(run)))
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

/// The waits after each failed attempt: 5 attempts over about 2 hours.
pub fn default_backoff() -> Vec<Duration> {
    [300, 900, 1_800, 3_600]
        .into_iter()
        .map(Duration::from_secs)
        .collect()
}

fn gzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

fn truncate(mut text: String) -> String {
    if let Some((index, _)) = text.char_indices().nth(ERROR_CHARS) {
        text.truncate(index);
    }
    text
}

/// A day ready to send.
struct File {
    name: String,
    data: Vec<u8>,
    gaps: Vec<String>,
}

/// Sends the days due, at their time, and retries them.
pub struct Worker {
    pool: MySqlPool,
    webhooks: Arc<Webhooks>,
    sender: Sender,
    backoff: Vec<Duration>,
}

impl Worker {
    pub fn new(pool: MySqlPool, webhooks: Arc<Webhooks>, sender: Sender) -> Self {
        Self {
            pool,
            webhooks,
            sender,
            backoff: default_backoff(),
        }
    }

    /// With waits of its own between attempts.
    #[cfg(test)]
    pub fn with_backoff(mut self, backoff: Vec<Duration>) -> Self {
        self.backoff = backoff;
        self
    }

    /// Looks for days to send every 15 seconds. Never returns.
    pub async fn run(self) {
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            if let Err(error) = self.tick(Timestamp::now()).await {
                tracing::warn!(%error, "cannot send the certification deliveries");
            }
        }
    }

    /// Makes the days due at `now`, then sends those due.
    pub async fn tick(&self, now: Timestamp) -> ApiResult<()> {
        self.schedule(now).await?;
        self.send_due().await?;
        let failing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM certification_deliveries d
             WHERE d.enabled AND (SELECT CAST(r.state AS CHAR) FROM certification_delivery_runs r
                                  WHERE r.delivery_id = d.id AND r.state IN ('delivered', 'failed')
                                  ORDER BY r.id DESC LIMIT 1) = 'failed'",
        )
        .fetch_one(&self.pool)
        .await?;
        metrics::gauge!("heron_api_certification_deliveries_failing").set(failing as f64);
        Ok(())
    }

    /// The day of each enabled delivery whose time has come today, once.
    async fn schedule(&self, now: Timestamp) -> ApiResult<()> {
        let all: Vec<(u64, u64, u16, u8, i64)> = sqlx::query_as(
            "SELECT id, station_id, send_at_min, days_back,
                    CAST(UNIX_TIMESTAMP(created_at) * 1000 AS SIGNED)
             FROM certification_deliveries WHERE enabled",
        )
        .fetch_all(&self.pool)
        .await?;
        for (id, station_id, send_at_min, days_back, created_ms) in all {
            if !crate::modules::is_on(&self.pool, Some(station_id), Module::Certification).await? {
                continue;
            }
            let tz = time_zone(&self.pool, station_id).await?;
            let today = now.to_zoned(tz.clone()).date();
            let time = Time::new(
                i8::try_from(send_at_min / 60).unwrap_or(0).min(23),
                i8::try_from(send_at_min % 60).unwrap_or(0),
                0,
                0,
            )
            .map_err(|error| ApiError::Internal(error.to_string()))?;
            let due = today
                .to_datetime(time)
                .to_zoned(tz)
                .map_err(|error| ApiError::Internal(error.to_string()))?
                .timestamp()
                .as_millisecond();
            // Not yet, or added after today's time: tomorrow first.
            if now.as_millisecond() < due || created_ms >= due {
                continue;
            }
            let day = today
                .checked_sub(jiff::Span::new().days(i64::from(days_back)))
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            let made = sqlx::query(
                "INSERT IGNORE INTO certification_delivery_runs
                     (delivery_id, day, scheduled_day, next_attempt_at)
                 VALUES (?, ?, ?, NOW(6))",
            )
            .bind(id)
            .bind(day.to_string())
            .bind(day.to_string())
            .execute(&self.pool)
            .await?
            .rows_affected();
            if made > 0 {
                tracing::info!(delivery = id, day = %day, "certification day to send");
                prune(&self.pool, id).await?;
            }
        }
        Ok(())
    }

    /// Sends each day due once: a failed attempt waits for its retry.
    async fn send_due(&self) -> ApiResult<()> {
        const DUE: &str = "((r.state = 'pending' AND r.next_attempt_at <= NOW(6))
                            OR (r.state = 'sending' AND r.claimed_until < NOW(6)))";
        let due: Vec<(u64, u64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT r.id, d.station_id FROM certification_delivery_runs r
                 JOIN certification_deliveries d ON d.id = r.delivery_id
             WHERE {DUE} ORDER BY r.next_attempt_at, r.id"
        )))
        .fetch_all(&self.pool)
        .await?;
        for (run_id, station_id) in due {
            if !crate::modules::is_on(&self.pool, Some(station_id), Module::Certification).await? {
                continue;
            }
            let claimed = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE certification_delivery_runs r SET r.state = 'sending',
                     r.claimed_until = NOW(6) + INTERVAL {CLAIM_MINUTES} MINUTE
                 WHERE r.id = ? AND {DUE}"
            )))
            .bind(run_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
            if claimed == 1
                && let Err(error) = self.attempt(run_id).await
            {
                tracing::warn!(run = run_id, %error, "cannot send a certification day");
            }
        }
        Ok(())
    }

    /// The file of a day: the export, compressed when asked, named.
    async fn prepare(&self, delivery: &Stored, day: Date) -> ApiResult<File> {
        let station: String = sqlx::query_scalar("SELECT code FROM stations WHERE id = ?")
            .bind(delivery.station_id)
            .fetch_one(&self.pool)
            .await?;
        let server: String = sqlx::query_scalar("SELECT name FROM stream_servers WHERE id = ?")
            .bind(delivery.server_id)
            .fetch_one(&self.pool)
            .await?;
        let tz = time_zone(&self.pool, delivery.station_id).await?;
        let format = delivery.format();
        let (body, gaps) = export(
            &self.pool,
            delivery.station_id,
            delivery.server_id,
            &server,
            day,
            &tz,
            delivery.mount.as_deref(),
            format,
        )
        .await?;
        let mut gaps: Vec<String> = gaps
            .iter()
            .map(|gap| format!("{}/{}", iso(gap.from_unix_ms), iso(gap.to_unix_ms)))
            .collect();
        // A log that cannot be read now: its gap is not written yet.
        let end = day_range(day, &tz)?.1;
        let since: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT CAST(UNIX_TIMESTAMP(error_since) * 1000 AS SIGNED) FROM access_logs
             WHERE server_id = ? AND error IS NOT NULL",
        )
        .bind(delivery.server_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(Some(since)) = since
            && since < end
        {
            let to = crate::rotation::now_ms().min(end);
            gaps.push(format!("{}/{}", iso(since), iso(to)));
        }
        let compression = delivery.compression();
        let data = match compression {
            DeliveryCompression::None => body.into_bytes(),
            DeliveryCompression::Gzip => {
                gzip(body.as_bytes()).map_err(|error| ApiError::Internal(error.to_string()))?
            }
        };
        let name = file_name(
            &delivery.file_name,
            &Names {
                station: &station,
                server: &server,
                mount: delivery.mount.as_deref(),
                day,
                ext: &extension(format, compression),
            },
        )
        .map_err(ApiError::Invalid)?;
        Ok(File { name, data, gaps })
    }

    async fn attempt(&self, run_id: u64) -> ApiResult<()> {
        let (delivery_id, day, attempts): (u64, String, u16) = sqlx::query_as(
            "SELECT delivery_id, DATE_FORMAT(day, '%Y-%m-%d'), attempts
             FROM certification_delivery_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_one(&self.pool)
        .await?;
        let delivery: Stored =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{STORED} WHERE id = ?")))
                .bind(delivery_id)
                .fetch_one(&self.pool)
                .await?;
        let day = parse_day(&day)?;
        let file = self.prepare(&delivery, day).await;
        let (name, bytes, gaps) = match &file {
            Ok(file) => (
                Some(file.name.clone()),
                Some(file.data.len() as u64),
                Some(file.gaps.join(", ")).filter(|gaps| !gaps.is_empty()),
            ),
            Err(_) => (None, None, None),
        };
        let sent = match &file {
            Ok(file) => {
                self.sender
                    .upload(&delivery.target(), &file.name, &file.data)
                    .await
            }
            Err(error) => Err(error.to_string()),
        };
        let attempts = attempts.saturating_add(1);
        let error = sent.as_ref().err().map(|error| truncate(error.clone()));
        sqlx::query(
            "INSERT INTO certification_delivery_attempts
                 (run_id, attempted_at, ok, error, file_name, bytes)
             VALUES (?, NOW(6), ?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(sent.is_ok())
        .bind(&error)
        .bind(&name)
        .bind(bytes)
        .execute(&self.pool)
        .await?;
        match sent {
            Ok(()) => {
                sqlx::query(
                    "UPDATE certification_delivery_runs SET state = 'delivered', attempts = ?,
                         file_name = ?, bytes = ?, gaps = ?, error = NULL, claimed_until = NULL,
                         next_attempt_at = NULL, finished_at = NOW(6)
                     WHERE id = ?",
                )
                .bind(attempts)
                .bind(&name)
                .bind(bytes)
                .bind(&gaps)
                .bind(run_id)
                .execute(&self.pool)
                .await?;
                metrics::counter!("heron_api_certification_deliveries_total", "result" => "delivered")
                    .increment(1);
                tracing::info!(
                    delivery = delivery.id,
                    day = %day,
                    file = name.as_deref().unwrap_or_default(),
                    bytes,
                    gaps = gaps.as_deref().unwrap_or_default(),
                    "certification day delivered"
                );
            }
            Err(error) => {
                let wait = self.backoff.get(usize::from(attempts) - 1).copied();
                match wait.filter(|_| attempts < MAX_ATTEMPTS) {
                    Some(wait) => {
                        sqlx::query(
                            "UPDATE certification_delivery_runs SET state = 'pending', attempts = ?,
                                 file_name = ?, bytes = ?, gaps = ?, error = ?, claimed_until = NULL,
                                 next_attempt_at = NOW(6) + INTERVAL ? MICROSECOND
                             WHERE id = ?",
                        )
                        .bind(attempts)
                        .bind(&name)
                        .bind(bytes)
                        .bind(&gaps)
                        .bind(truncate(error.clone()))
                        .bind(u64::try_from(wait.as_micros()).unwrap_or(u64::MAX))
                        .bind(run_id)
                        .execute(&self.pool)
                        .await?;
                        metrics::counter!("heron_api_certification_deliveries_total", "result" => "retrying")
                            .increment(1);
                        tracing::warn!(delivery = delivery.id, day = %day, attempts, %error, "certification delivery failed, to retry");
                    }
                    None => {
                        sqlx::query(
                            "UPDATE certification_delivery_runs SET state = 'failed', attempts = ?,
                                 file_name = ?, bytes = ?, gaps = ?, error = ?, claimed_until = NULL,
                                 next_attempt_at = NULL, finished_at = NOW(6)
                             WHERE id = ?",
                        )
                        .bind(attempts)
                        .bind(&name)
                        .bind(bytes)
                        .bind(&gaps)
                        .bind(truncate(error.clone()))
                        .bind(run_id)
                        .execute(&self.pool)
                        .await?;
                        metrics::counter!("heron_api_certification_deliveries_total", "result" => "failed")
                            .increment(1);
                        tracing::error!(delivery = delivery.id, day = %day, attempts, %error, "certification delivery failed");
                        let message = format!(
                            "{}: the delivery of {day} failed after {attempts} attempts: {error}",
                            delivery.name
                        );
                        self.webhooks
                            .queue_for(
                                delivery.station_id,
                                Payload::message(WebhookEvent::CertificationFailed, message),
                            )
                            .await?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(mount: Option<&str>) -> Names<'_> {
        Names {
            station: "main",
            server: "Main server",
            mount,
            day: Date::constant(2026, 11, 7),
            ext: "log.gz",
        }
    }

    #[test]
    fn file_names_follow_their_pattern() {
        assert_eq!(
            file_name(DEFAULT_FILE_NAME, &names(None)).unwrap(),
            "main-Main-server-2026-11-07.log.gz"
        );
        assert_eq!(
            file_name("{yyyy}{mm}{dd}_{mount}.{ext}", &names(Some("/live"))).unwrap(),
            "20261107_live.log.gz"
        );
        assert_eq!(
            file_name("radio_{mount}", &names(None)).unwrap(),
            "radio_all"
        );
        for bad in ["{day}.log", "a/{date}", "{date", "  ", "..", "{station"] {
            assert!(file_name(bad, &names(None)).is_err(), "{bad}");
        }
    }

    #[test]
    fn times_are_checked() {
        assert_eq!(parse_time("04:00"), Ok(240));
        assert_eq!(parse_time("23:59"), Ok(1_439));
        assert!(parse_time("24:00").is_err());
        assert!(parse_time("4h").is_err());
        assert_eq!(hh_mm(65), "01:05");
    }

    fn input() -> CertificationDeliveryInput {
        serde_json::from_value(serde_json::json!({
            "name": "Certifier", "server_id": 1, "protocol": "ftps",
            "host": "ftp.example.org", "username": "station", "password": "secret"
        }))
        .unwrap()
    }

    #[test]
    fn deliveries_are_checked() {
        let checked = check(input(), None).unwrap();
        assert_eq!(
            (checked.port, checked.send_at_min, checked.days_back),
            (21, 240, 1)
        );
        assert_eq!(checked.file_name, DEFAULT_FILE_NAME);
        let mut no_secret = input();
        no_secret.password = None;
        assert!(check(no_secret, None).is_err());
        // Left out, the password kept stays.
        let before = check(input(), None).unwrap();
        let mut kept = input();
        kept.password = None;
        let stored = Stored {
            id: 1,
            station_id: 1,
            name: before.name,
            enabled: true,
            server_id: 1,
            mount: None,
            protocol: "ftps".to_owned(),
            host: before.host,
            port: before.port,
            username: before.username,
            password: before.password,
            remote_dir: String::new(),
            file_name: before.file_name,
            format: "raw".to_owned(),
            compression: "none".to_owned(),
            send_at_min: 240,
            days_back: 1,
        };
        assert_eq!(
            check(kept, Some(&stored)).unwrap().password.as_deref(),
            Some("secret")
        );
        let mut bad = input();
        bad.host = "ftp.example.org/in".to_owned();
        assert!(check(bad, None).is_err());
        let mut bad = input();
        bad.days_back = Some(8);
        assert!(check(bad, None).is_err());
        let mut bad = input();
        bad.file_name = Some("{nope}".to_owned());
        assert!(check(bad, None).is_err());
    }

    #[test]
    fn gzip_round_trips() {
        let packed = gzip(b"line\n").unwrap();
        let mut out = String::new();
        std::io::Read::read_to_string(
            &mut flate2::read::GzDecoder::new(packed.as_slice()),
            &mut out,
        )
        .unwrap();
        assert_eq!(out, "line\n");
    }
}
