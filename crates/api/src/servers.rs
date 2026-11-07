// SPDX-License-Identifier: AGPL-3.0-or-later

//! A station's stream servers, which its outputs pick, and relay servers,
//! only read for the listener statistics; the station details its outputs
//! send to them. See `docs/outputs.md#stream-servers`.
//!
//! The engine never sees a server: `outputs::load_config` joins each
//! output's server and details into the output's own settings, as the
//! engine has always had them. A change to a server or to the station
//! details is checked as a whole configuration and pushes the outputs.
//! Passwords are never sent back; an update without them keeps them.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Transaction};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate};
use crate::settings::{self, some_or_null};
use crate::stations::Station;

pub const DEFAULT_PORT: u16 = 8000;
pub const DEFAULT_SOURCE: &str = "source";

/// What the station tells the stream servers about itself, shown in stream
/// directories. Each output can override each of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct StreamDetails {
    pub name: Option<String>,
    pub description: Option<String>,
    pub genre: Option<String>,
    /// The station's website.
    pub url: Option<String>,
    /// List the streams in public directories.
    pub public: bool,
}

impl StreamDetails {
    pub const SETTINGS_NAME: &str = "stream_details";
}

/// The station details an output sends instead of the station's: a field
/// left out follows the station's; an empty text sends none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DetailOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<bool>,
}

impl DetailOverrides {
    pub fn parse(json: Option<&str>) -> Self {
        json.and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default()
    }

    /// The details the output sends: its own, then the station's. Empty
    /// texts are none.
    pub fn over(&self, station: &StreamDetails) -> StreamDetails {
        let pick = |own: &Option<String>, station: &Option<String>| {
            own.as_ref()
                .or(station.as_ref())
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
        };
        StreamDetails {
            name: pick(&self.name, &station.name),
            description: pick(&self.description, &station.description),
            genre: pick(&self.genre, &station.genre),
            url: pick(&self.url, &station.url),
            public: self.public.unwrap_or(station.public),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    #[default]
    Icecast,
}

/// A mount a relay repeats.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RelayMount {
    /// The mount on the master.
    pub mount: String,
    /// Its name on the relay, when it differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_mount: Option<String>,
}

/// A server as stored, secrets included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredServer {
    pub id: u64,
    pub station_id: u64,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub source_username: Option<String>,
    pub source_password: Option<String>,
    pub admin_username: Option<String>,
    pub admin_password: Option<String>,
    pub public_url: Option<String>,
    pub relay_of: Option<u64>,
    /// `None`: all of the master's mounts.
    pub relay_mounts: Option<Vec<RelayMount>>,
    pub relay_subtract_one: bool,
}

impl StoredServer {
    /// The admin credentials, when a password is set.
    pub fn admin_credentials(&self) -> Option<(String, String)> {
        let password = self.admin_password.clone().filter(|p| !p.is_empty())?;
        let username = self
            .admin_username
            .clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "admin".to_owned());
        Some((username, password))
    }

    /// The name on this relay of a master's mount, if it relays it.
    pub fn relayed(&self, mount: &str) -> Option<String> {
        match &self.relay_mounts {
            None => Some(mount.to_owned()),
            Some(mounts) => mounts
                .iter()
                .find(|relayed| relayed.mount == mount)
                .map(|relayed| {
                    relayed
                        .relay_mount
                        .clone()
                        .unwrap_or_else(|| mount.to_owned())
                }),
        }
    }
}

type ServerRow = (
    u64,
    u64,
    String,
    String,
    u16,
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<u64>,
    Option<String>,
    bool,
);

/// The servers of a station, or of every station with `None`.
pub async fn load(
    executor: &mut sqlx::MySqlConnection,
    station_id: Option<u64>,
) -> ApiResult<Vec<StoredServer>> {
    let rows: Vec<ServerRow> = sqlx::query_as(
        "SELECT id, station_id, name, host, port, tls, source_username, source_password,
                admin_username, admin_password, public_url, relay_of,
                CAST(relay_mounts AS CHAR), relay_subtract_one
         FROM stream_servers WHERE station_id = ? OR ? IS NULL ORDER BY id",
    )
    .bind(station_id)
    .bind(station_id)
    .fetch_all(&mut *executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                station_id,
                name,
                host,
                port,
                tls,
                source_username,
                source_password,
                admin_username,
                admin_password,
                public_url,
                relay_of,
                relay_mounts,
                relay_subtract_one,
            )| StoredServer {
                id,
                station_id,
                name,
                host,
                port,
                tls,
                source_username,
                source_password,
                admin_username,
                admin_password,
                public_url,
                relay_of,
                relay_mounts: relay_mounts.and_then(|json| serde_json::from_str(&json).ok()),
                relay_subtract_one,
            },
        )
        .collect())
}

/// An output or a relay using a server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct ServerUse {
    pub id: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StreamServerView {
    pub id: u64,
    pub name: String,
    pub kind: ServerKind,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// `source` when missing.
    pub source_username: Option<String>,
    /// Never sent back.
    pub source_password_set: bool,
    /// For the listener statistics; `admin` when missing.
    pub admin_username: Option<String>,
    /// Never sent back: without it, only concurrent listeners are counted.
    pub admin_password_set: bool,
    /// As listeners reach the server; `http://host:port` when missing.
    pub public_url: Option<String>,
    /// The master server, for a relay.
    pub relay_of: Option<u64>,
    /// The mounts a relay repeats; missing: all of the master's.
    pub relay_mounts: Option<Vec<RelayMount>>,
    /// The relay's connection to the master counts as 1 listener per mount.
    pub relay_subtract_one: bool,
    /// Outputs streaming to it.
    pub outputs: Vec<ServerUse>,
    /// Relays repeating it.
    pub relays: Vec<ServerUse>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StreamServerInput {
    pub name: String,
    #[serde(default)]
    pub kind: ServerKind,
    pub host: String,
    /// 8000 when left out.
    #[serde(default)]
    pub port: Option<u16>,
    /// Relays only for now: the engine does not send over TLS yet.
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub source_username: Option<String>,
    /// Left out, the stored one is kept; `null` removes it. Required for a
    /// server outputs send to.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<String>)]
    pub source_password: Option<Option<String>>,
    #[serde(default)]
    pub admin_username: Option<String>,
    /// Left out, the stored one is kept; `null` removes it.
    #[serde(default, deserialize_with = "some_or_null")]
    #[schema(value_type = Option<String>)]
    pub admin_password: Option<Option<String>>,
    #[serde(default)]
    pub public_url: Option<String>,
    /// Makes it a relay of this server of the station.
    #[serde(default)]
    pub relay_of: Option<u64>,
    #[serde(default)]
    pub relay_mounts: Option<Vec<RelayMount>>,
    #[serde(default)]
    pub relay_subtract_one: bool,
}

fn blank(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn check_mount(mount: &str) -> ApiResult<()> {
    if !mount.starts_with('/')
        || mount.len() < 2
        || mount.contains(|c: char| c.is_whitespace() || c.is_control() || c == '?')
    {
        return Err(ApiError::Invalid(format!(
            "invalid mount {mount:?}: it starts with / and has no spaces"
        )));
    }
    Ok(())
}

fn check_public_url(url: Option<String>) -> ApiResult<Option<String>> {
    let Some(url) = blank(url) else {
        return Ok(None);
    };
    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.contains(|c: char| c.is_whitespace() || c.is_control())
    {
        return Err(ApiError::Invalid(format!(
            "the public URL must start with http:// or https:// (got {url:?})"
        )));
    }
    Ok(Some(url))
}

/// Checks an input against what is stored, and gives the row to write.
async fn checked(
    tx: &mut Transaction<'_, MySql>,
    station: &Station,
    id: Option<u64>,
    input: StreamServerInput,
    stored: Option<&StoredServer>,
) -> ApiResult<StoredServer> {
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(ApiError::Invalid(
            "names are 1 to 255 characters".to_owned(),
        ));
    }
    let host = input.host.trim().to_owned();
    if host.is_empty() || host.contains(|c: char| c.is_whitespace() || c == '/') {
        return Err(ApiError::Invalid(format!(
            "invalid host {host:?}: give a name or an address"
        )));
    }
    // Icecast only for now: Shoutcast with its sinks (M11).
    let ServerKind::Icecast = input.kind;
    let port = input.port.unwrap_or(DEFAULT_PORT);
    if port == 0 {
        return Err(ApiError::Invalid("the port is 1 to 65535".to_owned()));
    }
    let keep = |input: Option<Option<String>>, stored: Option<String>| match input {
        None => stored,
        Some(value) => value.filter(|value| !value.is_empty()),
    };
    let source_password = keep(
        input.source_password,
        stored.and_then(|s| s.source_password.clone()),
    );
    let admin_password = keep(
        input.admin_password,
        stored.and_then(|s| s.admin_password.clone()),
    );
    if let Some(master) = input.relay_of {
        if Some(master) == id {
            return Err(ApiError::Invalid("a server cannot relay itself".to_owned()));
        }
        let row: Option<(u64, Option<u64>)> =
            sqlx::query_as("SELECT station_id, relay_of FROM stream_servers WHERE id = ?")
                .bind(master)
                .fetch_optional(&mut **tx)
                .await?;
        match row {
            Some((station_id, None)) if station_id == station.id() => {}
            Some((station_id, Some(_))) if station_id == station.id() => {
                return Err(ApiError::Invalid(
                    "a relay's master cannot be a relay itself".to_owned(),
                ));
            }
            _ => {
                return Err(ApiError::Invalid(format!(
                    "no stream server {master} on this station"
                )));
            }
        }
        if let Some(id) = id {
            let outputs = users_of(tx, id).await?;
            if !outputs.is_empty() {
                return Err(ApiError::Conflict(format!(
                    "outputs stream to this server, so it cannot be a relay: {}",
                    names(&outputs)
                )));
            }
            let relays = relays_of(tx, id).await?;
            if !relays.is_empty() {
                return Err(ApiError::Conflict(format!(
                    "this server has relays, so it cannot be a relay: {}",
                    names(&relays)
                )));
            }
        }
        if let Some(mounts) = &input.relay_mounts {
            let mut seen = std::collections::HashSet::new();
            for relayed in mounts {
                check_mount(&relayed.mount)?;
                if let Some(other) = &relayed.relay_mount {
                    check_mount(other)?;
                }
                if !seen.insert(relayed.mount.as_str()) {
                    return Err(ApiError::Invalid(format!(
                        "the mount {} is listed twice",
                        relayed.mount
                    )));
                }
            }
        }
    } else {
        if input.tls {
            return Err(ApiError::Invalid(
                "TLS is not supported yet for the servers outputs send to: only for relays"
                    .to_owned(),
            ));
        }
        if source_password.is_none() {
            return Err(ApiError::Invalid(
                "the source password is missing: outputs need it to connect".to_owned(),
            ));
        }
    }
    let relay = input.relay_of.is_some();
    Ok(StoredServer {
        id: id.unwrap_or(0),
        station_id: station.id(),
        name,
        host,
        port,
        tls: input.tls,
        source_username: blank(input.source_username).filter(|_| !relay),
        source_password: source_password.filter(|_| !relay),
        admin_username: blank(input.admin_username),
        admin_password,
        public_url: check_public_url(input.public_url)?,
        relay_of: input.relay_of,
        relay_mounts: input.relay_mounts.filter(|_| relay),
        relay_subtract_one: relay && input.relay_subtract_one,
    })
}

fn names(uses: &[ServerUse]) -> String {
    uses.iter()
        .map(|used| used.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

async fn users_of(tx: &mut Transaction<'_, MySql>, id: u64) -> ApiResult<Vec<ServerUse>> {
    let rows: Vec<(u64, String)> =
        sqlx::query_as("SELECT id, name FROM outputs WHERE server_id = ? ORDER BY name")
            .bind(id)
            .fetch_all(&mut **tx)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| ServerUse { id, name })
        .collect())
}

async fn relays_of(tx: &mut Transaction<'_, MySql>, id: u64) -> ApiResult<Vec<ServerUse>> {
    let rows: Vec<(u64, String)> =
        sqlx::query_as("SELECT id, name FROM stream_servers WHERE relay_of = ? ORDER BY name")
            .bind(id)
            .fetch_all(&mut **tx)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| ServerUse { id, name })
        .collect())
}

fn mounts_json(mounts: Option<&Vec<RelayMount>>) -> ApiResult<Option<String>> {
    mounts
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| ApiError::Internal(error.to_string()))
}

fn duplicate_or(error: sqlx::Error) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict("another stream server of the station has this name".to_owned())
    } else {
        error.into()
    }
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_servers, create_server))
        .routes(routes!(get_server, update_server, delete_server))
        .routes(routes!(get_stream_details, put_stream_details))
}

async fn views(state: &AppState, station: &Station) -> ApiResult<Vec<StreamServerView>> {
    let mut connection = state.pool.acquire().await?;
    let servers = load(&mut connection, Some(station.id())).await?;
    let outputs: Vec<(u64, String, u64)> = sqlx::query_as(
        "SELECT id, name, server_id FROM outputs
         WHERE station_id = ? AND server_id IS NOT NULL ORDER BY name",
    )
    .bind(station.id())
    .fetch_all(&mut *connection)
    .await?;
    Ok(servers
        .iter()
        .map(|server| StreamServerView {
            id: server.id,
            name: server.name.clone(),
            kind: ServerKind::Icecast,
            host: server.host.clone(),
            port: server.port,
            tls: server.tls,
            source_username: server.source_username.clone(),
            source_password_set: server.source_password.is_some(),
            admin_username: server.admin_username.clone(),
            admin_password_set: server.admin_password.is_some(),
            public_url: server.public_url.clone(),
            relay_of: server.relay_of,
            relay_mounts: server.relay_mounts.clone(),
            relay_subtract_one: server.relay_subtract_one,
            outputs: outputs
                .iter()
                .filter(|(_, _, server_id)| *server_id == server.id)
                .map(|(id, name, _)| ServerUse {
                    id: *id,
                    name: name.clone(),
                })
                .collect(),
            relays: servers
                .iter()
                .filter(|relay| relay.relay_of == Some(server.id))
                .map(|relay| ServerUse {
                    id: relay.id,
                    name: relay.name.clone(),
                })
                .collect(),
        })
        .collect())
}

/// The station's stream servers and relays, without their passwords.
#[utoipa::path(get, path = "/stream-servers", tag = "outputs",
    responses((status = 200, body = Vec<StreamServerView>), (status = "4XX", body = ErrorBody)))]
async fn list_servers(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Vec<StreamServerView>>> {
    Ok(Json(views(&state, &station).await?))
}

#[utoipa::path(get, path = "/stream-servers/{id}", tag = "outputs",
    params(("id" = u64, Path)),
    responses((status = 200, body = StreamServerView), (status = "4XX", body = ErrorBody)))]
async fn get_server(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<StreamServerView>> {
    views(&state, &station)
        .await?
        .into_iter()
        .find(|server| server.id == id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("stream server"))
}

/// Adds a stream server, or a relay of one. Admins only.
#[utoipa::path(post, path = "/stream-servers", tag = "outputs",
    request_body = StreamServerInput,
    responses((status = 201, body = StreamServerView), (status = "4XX", body = ErrorBody)))]
async fn create_server(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(input): Json<StreamServerInput>,
) -> ApiResult<(StatusCode, Json<StreamServerView>)> {
    user.require(Role::Admin)?;
    let mut tx = state.pool.begin().await?;
    let server = checked(&mut tx, &station, None, input, None).await?;
    let id = sqlx::query(
        "INSERT INTO stream_servers (station_id, name, kind, host, port, tls, source_username,
             source_password, admin_username, admin_password, public_url, relay_of, relay_mounts,
             relay_subtract_one)
         VALUES (?, ?, 'icecast', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(server.station_id)
    .bind(&server.name)
    .bind(&server.host)
    .bind(server.port)
    .bind(server.tls)
    .bind(&server.source_username)
    .bind(&server.source_password)
    .bind(&server.admin_username)
    .bind(&server.admin_password)
    .bind(&server.public_url)
    .bind(server.relay_of)
    .bind(mounts_json(server.relay_mounts.as_ref())?)
    .bind(server.relay_subtract_one)
    .execute(&mut *tx)
    .await
    .map_err(duplicate_or)?
    .last_insert_id();
    tx.commit().await?;
    tracing::info!(user = %user.username, id, name = %server.name, station = %station.code(), "stream server created");
    let Json(view) = get_server(user, State(state), station, Path(id)).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// Replaces a stream server. Passwords left out are kept. Outputs using it
/// are pushed again: those whose settings changed reconnect. Admins only.
#[utoipa::path(put, path = "/stream-servers/{id}", tag = "outputs",
    params(("id" = u64, Path)), request_body = StreamServerInput,
    responses((status = 200, body = StreamServerView), (status = "4XX", body = ErrorBody)))]
async fn update_server(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(input): Json<StreamServerInput>,
) -> ApiResult<Json<StreamServerView>> {
    user.require(Role::Admin)?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT id FROM stream_servers WHERE id = ? FOR UPDATE")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let stored = load(&mut tx, Some(station.id()))
        .await?
        .into_iter()
        .find(|server| server.id == id)
        .ok_or_else(|| ApiError::not_found("stream server"))?;
    let server = checked(&mut tx, &station, Some(id), input, Some(&stored)).await?;
    sqlx::query(
        "UPDATE stream_servers SET name = ?, host = ?, port = ?, tls = ?, source_username = ?,
                source_password = ?, admin_username = ?, admin_password = ?, public_url = ?,
                relay_of = ?, relay_mounts = ?, relay_subtract_one = ?
         WHERE id = ?",
    )
    .bind(&server.name)
    .bind(&server.host)
    .bind(server.port)
    .bind(server.tls)
    .bind(&server.source_username)
    .bind(&server.source_password)
    .bind(&server.admin_username)
    .bind(&server.admin_password)
    .bind(&server.public_url)
    .bind(server.relay_of)
    .bind(mounts_json(server.relay_mounts.as_ref())?)
    .bind(server.relay_subtract_one)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(duplicate_or)?;
    crate::outputs::commit(&state, Some(&station), tx).await?;
    tracing::info!(user = %user.username, id, name = %server.name, "stream server changed");
    get_server(user, State(state), station, Path(id)).await
}

/// Deletes a stream server no output uses and no relay repeats: `409`
/// otherwise, naming them. Admins only.
#[utoipa::path(delete, path = "/stream-servers/{id}", tag = "outputs",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_server(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    let mut tx = state.pool.begin().await?;
    let exists: Option<u64> = sqlx::query_scalar(
        "SELECT id FROM stream_servers WHERE id = ? AND station_id = ? FOR UPDATE",
    )
    .bind(id)
    .bind(station.id())
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        return Err(ApiError::not_found("stream server"));
    }
    let outputs = users_of(&mut tx, id).await?;
    if !outputs.is_empty() {
        return Err(ApiError::Conflict(format!(
            "outputs still use this server: {}",
            names(&outputs)
        )));
    }
    let relays = relays_of(&mut tx, id).await?;
    if !relays.is_empty() {
        return Err(ApiError::Conflict(format!(
            "relays still repeat this server: {}",
            names(&relays)
        )));
    }
    sqlx::query("DELETE FROM stream_servers WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(user = %user.username, id, "stream server deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// The station details sent to the stream servers.
#[utoipa::path(get, path = "/settings/stream-details", tag = "settings",
    responses((status = 200, body = StreamDetails), (status = "4XX", body = ErrorBody)))]
async fn get_stream_details(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<StreamDetails>> {
    Ok(Json(
        settings::load_for(&state.pool, station.id(), StreamDetails::SETTINGS_NAME).await?,
    ))
}

/// Replaces the station details. The outputs that follow them are pushed
/// again, and reconnect. Admins only.
#[utoipa::path(put, path = "/settings/stream-details", tag = "settings",
    request_body = StreamDetails,
    responses((status = 200, body = StreamDetails), (status = "4XX", body = ErrorBody)))]
async fn put_stream_details(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(details): Json<StreamDetails>,
) -> ApiResult<Json<StreamDetails>> {
    user.require(Role::Admin)?;
    let details = StreamDetails {
        name: blank(details.name),
        description: blank(details.description),
        genre: blank(details.genre),
        url: blank(details.url),
        public: details.public,
    };
    let mut tx = state.pool.begin().await?;
    settings::save_for(
        &mut *tx,
        station.id(),
        StreamDetails::SETTINGS_NAME,
        &details,
    )
    .await?;
    crate::outputs::commit(&state, Some(&station), tx).await?;
    tracing::info!(user = %user.username, station = %station.code(), "station details changed");
    Ok(Json(details))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_win_and_empty_texts_send_nothing() {
        let station = StreamDetails {
            name: Some("Station".to_owned()),
            description: Some("Hits".to_owned()),
            genre: None,
            url: Some("https://example.org".to_owned()),
            public: true,
        };
        let own = DetailOverrides {
            name: Some("Station HD".to_owned()),
            description: Some(String::new()),
            public: Some(false),
            ..DetailOverrides::default()
        };
        let sent = own.over(&station);
        assert_eq!(sent.name.as_deref(), Some("Station HD"));
        assert_eq!(sent.description, None);
        assert_eq!(sent.url.as_deref(), Some("https://example.org"));
        assert!(!sent.public);
        assert_eq!(DetailOverrides::default().over(&station), station);
        assert_eq!(
            serde_json::to_value(&own).unwrap(),
            serde_json::json!({"name": "Station HD", "description": "", "public": false})
        );
    }

    #[test]
    fn relays_repeat_all_mounts_or_a_list() {
        let mut relay = StoredServer {
            id: 2,
            station_id: 1,
            name: "Relay".to_owned(),
            host: "relay".to_owned(),
            port: 8000,
            tls: false,
            source_username: None,
            source_password: None,
            admin_username: None,
            admin_password: Some("pw".to_owned()),
            public_url: None,
            relay_of: Some(1),
            relay_mounts: None,
            relay_subtract_one: false,
        };
        assert_eq!(relay.relayed("/live").as_deref(), Some("/live"));
        assert_eq!(
            relay.admin_credentials(),
            Some(("admin".to_owned(), "pw".to_owned()))
        );
        relay.relay_mounts = Some(vec![
            RelayMount {
                mount: "/live".to_owned(),
                relay_mount: Some("/r".to_owned()),
            },
            RelayMount {
                mount: "/aac".to_owned(),
                relay_mount: None,
            },
        ]);
        assert_eq!(relay.relayed("/live").as_deref(), Some("/r"));
        assert_eq!(relay.relayed("/aac").as_deref(), Some("/aac"));
        assert_eq!(relay.relayed("/other"), None);
    }
}
