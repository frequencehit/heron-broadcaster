// SPDX-License-Identifier: AGPL-3.0-or-later

//! Encoder profiles and outputs, stored in MariaDB and pushed to the engine
//! (see `sync`). See `docs/outputs.md`.
//!
//! Every change is checked as a whole configuration, the way the engine
//! would, before it is saved: an invalid change saves nothing.
//! An Icecast output picks a stream server (`servers`) and keeps only its
//! mount in `config`, and the station details it overrides in `details`:
//! the engine gets the output's full settings, joined from them
//! (`load_config`). Credentials in an output's settings (`password`, of
//! the kinds still holding their own) are never sent back; an update
//! without them keeps the stored ones.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use heron_core::{EncoderKind, EncoderProfile, OutputConfig, OutputKind, OutputsConfig};
use heron_outputs::icecast::IcecastConfig;
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Transaction};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::engine::OutputStatus;
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate, is_still_used};
use crate::servers::{self, DetailOverrides, StoredServer, StreamDetails};
use crate::stations::Station;

/// Settings never sent back to clients.
const SECRET_KEYS: [&str; 1] = ["password"];
/// What an Icecast output keeps of its own; the rest is its server's.
const ICECAST_OWN: [&str; 1] = ["mount"];
/// Groups outputs holding their server's settings into stream servers.
pub const GROUP_INTO_SERVERS: &str =
    include_str!("../../../migrations/20270301000001_stream_servers_from_outputs.sql");

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct EncoderProfileView {
    pub id: u64,
    pub name: String,
    pub encoder: EncoderKind,
    pub bitrate_kbps: Option<u16>,
    pub sample_rate: u32,
    pub channels: u8,
    /// Encoder-specific options, e.g. MP3: `{"mode": "cbr", "quality": 3}`.
    #[schema(value_type = Object)]
    pub options: serde_json::Value,
    /// Outputs using this profile.
    pub outputs: u32,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EncoderProfileInput {
    pub name: String,
    pub encoder: EncoderKind,
    pub bitrate_kbps: Option<u16>,
    pub sample_rate: u32,
    pub channels: u8,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub options: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct OutputView {
    pub id: u64,
    pub name: String,
    pub kind: OutputKind,
    /// The stream server it sends to (Icecast).
    pub server_id: Option<u64>,
    pub encoder_profile_id: u64,
    /// Its own settings (Icecast: the mount), without credentials.
    #[schema(value_type = Object)]
    pub config: serde_json::Value,
    /// The station details it sends instead of the station's.
    pub details: DetailOverrides,
    /// Credentials set in the settings (e.g. `password`), never sent back.
    pub secrets_set: Vec<String>,
    pub metadata_enabled: bool,
    pub enabled: bool,
    /// Where listeners can play the stream (Icecast).
    pub listen_url: Option<String>,
    /// Live state from the engine; missing while it is unreachable or
    /// before the output reaches it.
    pub status: Option<OutputStatus>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputInput {
    pub name: String,
    pub kind: OutputKind,
    /// The stream server, required for Icecast: one of the station's, not
    /// a relay.
    #[serde(default)]
    pub server_id: Option<u64>,
    pub encoder_profile_id: u64,
    /// Its own settings: Icecast's `mount`. Credentials left out keep
    /// their stored value.
    #[schema(value_type = Object)]
    pub config: serde_json::Value,
    /// The station details it overrides; left out, it follows the
    /// station's.
    #[serde(default)]
    pub details: DetailOverrides,
    #[serde(default = "yes")]
    pub metadata_enabled: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

fn enum_text<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn parse_enum<T: serde::de::DeserializeOwned>(text: String) -> ApiResult<T> {
    serde_json::from_value(serde_json::Value::String(text.clone()))
        .map_err(|_| ApiError::Internal(format!("unknown value {text:?} in the database")))
}

fn parse_json(text: Option<String>) -> serde_json::Value {
    text.and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null)
}

type ProfileRow = (u64, String, String, Option<u16>, u32, u8, Option<String>);
type OutputRow = (
    u64,
    u64,
    String,
    String,
    Option<u64>,
    u64,
    String,
    Option<String>,
    bool,
    bool,
);

/// An output as stored: its own settings only.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredOutput {
    pub id: u64,
    pub station_id: u64,
    pub name: String,
    pub kind: OutputKind,
    pub server_id: Option<u64>,
    pub encoder_profile_id: u64,
    pub config: serde_json::Value,
    pub details: DetailOverrides,
    pub metadata_enabled: bool,
    pub enabled: bool,
}

/// The outputs of one station, or of all with `None`.
pub async fn load_outputs(
    executor: &mut sqlx::MySqlConnection,
    station_id: Option<u64>,
) -> ApiResult<Vec<StoredOutput>> {
    let rows: Vec<OutputRow> = sqlx::query_as(
        "SELECT id, station_id, name, CAST(kind AS CHAR), server_id, encoder_profile_id,
                CAST(config AS CHAR), CAST(details AS CHAR), metadata_enabled, enabled
         FROM outputs WHERE station_id = ? OR ? IS NULL ORDER BY id",
    )
    .bind(station_id)
    .bind(station_id)
    .fetch_all(&mut *executor)
    .await?;
    rows.into_iter()
        .map(
            |(
                id,
                station_id,
                name,
                kind,
                server_id,
                encoder_profile_id,
                config,
                details,
                metadata_enabled,
                enabled,
            )| {
                Ok(StoredOutput {
                    id,
                    station_id,
                    name,
                    kind: parse_enum(kind)?,
                    server_id,
                    encoder_profile_id,
                    config: parse_json(Some(config)),
                    details: DetailOverrides::parse(details.as_deref()),
                    metadata_enabled,
                    enabled,
                })
            },
        )
        .collect()
}

/// The station details of each station that set them.
async fn station_details(
    executor: &mut sqlx::MySqlConnection,
) -> ApiResult<HashMap<u64, StreamDetails>> {
    let rows: Vec<(u64, String)> = sqlx::query_as(
        "SELECT station_id, CAST(value AS CHAR) FROM station_settings WHERE name = ?",
    )
    .bind(StreamDetails::SETTINGS_NAME)
    .fetch_all(&mut *executor)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(station, json)| Some((station, serde_json::from_str(&json).ok()?)))
        .collect())
}

/// An Icecast output's settings as the engine takes them: its server's
/// address and source credentials, its mount, and the station details
/// with its overrides.
pub fn icecast_settings(
    server: &StoredServer,
    own: &serde_json::Value,
    details: &StreamDetails,
) -> serde_json::Value {
    let mut config = serde_json::Map::new();
    config.insert("host".to_owned(), server.host.clone().into());
    config.insert("port".to_owned(), server.port.into());
    if let Some(mount) = own.get("mount") {
        config.insert("mount".to_owned(), mount.clone());
    }
    config.insert(
        "username".to_owned(),
        server
            .source_username
            .clone()
            .unwrap_or_else(|| servers::DEFAULT_SOURCE.to_owned())
            .into(),
    );
    if let Some(password) = &server.source_password {
        config.insert("password".to_owned(), password.clone().into());
    }
    for (key, value) in [
        ("name", &details.name),
        ("description", &details.description),
        ("genre", &details.genre),
        ("url", &details.url),
    ] {
        if let Some(value) = value {
            config.insert(key.to_owned(), value.clone().into());
        }
    }
    config.insert("public".to_owned(), details.public.into());
    if let Some(url) = &server.public_url {
        config.insert("public_url".to_owned(), url.clone().into());
    }
    serde_json::Value::Object(config)
}

/// An output as the engine takes it.
fn engine_output(
    output: &StoredOutput,
    servers: &[StoredServer],
    details: &HashMap<u64, StreamDetails>,
) -> OutputConfig {
    let server = output
        .server_id
        .and_then(|id| servers.iter().find(|server| server.id == id));
    let config = match (output.kind, server) {
        (OutputKind::Icecast, Some(server)) => {
            let station = details.get(&output.station_id).cloned().unwrap_or_default();
            icecast_settings(server, &output.config, &output.details.over(&station))
        }
        _ => output.config.clone(),
    };
    OutputConfig {
        id: output.id,
        name: output.name.clone(),
        kind: output.kind,
        encoder_profile_id: output.encoder_profile_id,
        config,
        metadata_enabled: output.metadata_enabled,
        enabled: output.enabled,
    }
}

/// The configuration in the shape the engine takes: every encoder profile,
/// and the outputs of one station, or of all with `None`, with their full
/// settings.
pub async fn load_config(
    executor: &mut sqlx::MySqlConnection,
    station_id: Option<u64>,
) -> ApiResult<OutputsConfig> {
    let profiles: Vec<ProfileRow> = sqlx::query_as(
        "SELECT id, name, CAST(encoder AS CHAR), bitrate_kbps, sample_rate, channels,
                CAST(options AS CHAR)
         FROM encoder_profiles ORDER BY id",
    )
    .fetch_all(&mut *executor)
    .await?;
    let encoder_profiles = profiles
        .into_iter()
        .map(
            |(id, name, encoder, bitrate_kbps, sample_rate, channels, options)| {
                Ok(EncoderProfile {
                    id,
                    name,
                    encoder: parse_enum(encoder)?,
                    bitrate_kbps,
                    sample_rate,
                    channels,
                    options: match parse_json(options) {
                        serde_json::Value::Null => serde_json::json!({}),
                        options => options,
                    },
                })
            },
        )
        .collect::<ApiResult<Vec<_>>>()?;
    let outputs = load_outputs(&mut *executor, station_id).await?;
    let servers = servers::load(&mut *executor, station_id).await?;
    let details = station_details(&mut *executor).await?;
    let outputs = outputs
        .iter()
        .map(|output| engine_output(output, &servers, &details))
        .collect();
    Ok(OutputsConfig {
        encoder_profiles,
        outputs,
    })
}

/// An Icecast output's settings, empty texts as none, without the public
/// address (where listeners play it, which the engine does not use): two
/// that give the same are the same stream to the engine.
fn icecast_meaning(name: &str, config: &serde_json::Value) -> Option<IcecastConfig> {
    let mut config = IcecastConfig::parse(name, config).ok()?;
    config.public_url = None;
    for text in [
        &mut config.name,
        &mut config.description,
        &mut config.genre,
        &mut config.url,
    ] {
        *text = text.take().filter(|text| !text.is_empty());
    }
    Some(config)
}

/// Keeps the engine's own settings of an output where they mean the same
/// as the new ones (written differently: a default left out, an empty
/// text), so that it does not reconnect for nothing, after the upgrade
/// that moved servers out of the outputs in particular.
pub fn keep_engine_shape(new: &mut OutputsConfig, engine: &OutputsConfig) {
    for output in &mut new.outputs {
        let Some(running) = engine.outputs.iter().find(|o| o.id == output.id) else {
            continue;
        };
        if running.config == output.config
            || running.kind != OutputKind::Icecast
            || output.kind != OutputKind::Icecast
        {
            continue;
        }
        let same = icecast_meaning(&output.name, &output.config).is_some_and(|meaning| {
            icecast_meaning(&running.name, &running.config) == Some(meaning)
        });
        if same {
            output.config = running.config.clone();
        }
    }
}

/// Checks the configuration as the engine will.
fn validate(config: &OutputsConfig) -> ApiResult<()> {
    heron_outputs::Outputs::validate(config).map_err(|error| ApiError::Invalid(error.to_string()))
}

/// Saves a change made in `tx` if the resulting configuration of each
/// station is valid, and tells the engines: one station's for a change to
/// its outputs, servers or details, every station's for a change to the
/// encoder profiles.
pub async fn commit(
    state: &AppState,
    station: Option<&Station>,
    mut tx: Transaction<'_, MySql>,
) -> ApiResult<()> {
    let stations = match station {
        Some(station) => vec![Arc::clone(&station.0)],
        None => state.stations.all(),
    };
    for station in &stations {
        let config = load_config(&mut tx, Some(station.id())).await?;
        validate(&config)?;
    }
    tx.commit().await?;
    for station in stations {
        station.sync.changed();
    }
    Ok(())
}

fn redact(config: &serde_json::Value) -> (serde_json::Value, Vec<String>) {
    let mut config = config.clone();
    let mut secrets = Vec::new();
    if let Some(object) = config.as_object_mut() {
        for key in SECRET_KEYS {
            if object.remove(key).is_some() {
                secrets.push(key.to_owned());
            }
        }
    }
    (config, secrets)
}

/// Keeps the stored credentials an update leaves out.
fn keep_secrets(new: &mut serde_json::Value, old: &serde_json::Value) {
    let (Some(new), Some(old)) = (new.as_object_mut(), old.as_object()) else {
        return;
    };
    for key in SECRET_KEYS {
        if !new.contains_key(key)
            && let Some(value) = old.get(key)
        {
            new.insert(key.to_owned(), value.clone());
        }
    }
}

fn output_view(
    output: StoredOutput,
    engine: &OutputConfig,
    status: Option<OutputStatus>,
) -> OutputView {
    let listen_url = match output.kind {
        OutputKind::Icecast => IcecastConfig::parse(&engine.name, &engine.config)
            .ok()
            .map(|config| config.listen_url()),
        _ => None,
    };
    let (config, secrets_set) = redact(&output.config);
    OutputView {
        id: output.id,
        name: output.name,
        kind: output.kind,
        server_id: output.server_id,
        encoder_profile_id: output.encoder_profile_id,
        config,
        details: output.details,
        secrets_set,
        metadata_enabled: output.metadata_enabled,
        enabled: output.enabled,
        listen_url,
        status,
    }
}

/// Checks what an output keeps of its own, and its server.
async fn check_output(
    tx: &mut Transaction<'_, MySql>,
    station: &Station,
    input: &OutputInput,
) -> ApiResult<()> {
    let Some(config) = input.config.as_object() else {
        return Err(ApiError::Invalid("config must be an object".to_owned()));
    };
    if input.kind != OutputKind::Icecast {
        if input.server_id.is_some() {
            return Err(ApiError::Invalid(
                "only Icecast outputs pick a stream server".to_owned(),
            ));
        }
        return Ok(());
    }
    if let Some(key) = config
        .keys()
        .find(|key| !ICECAST_OWN.contains(&key.as_str()))
    {
        return Err(ApiError::Invalid(format!(
            "{key} is a setting of the stream server: an Icecast output's config has only its mount"
        )));
    }
    let Some(server_id) = input.server_id else {
        return Err(ApiError::Invalid(
            "an Icecast output needs a stream server (server_id)".to_owned(),
        ));
    };
    let server: Option<Option<u64>> =
        sqlx::query_scalar("SELECT relay_of FROM stream_servers WHERE id = ? AND station_id = ?")
            .bind(server_id)
            .bind(station.id())
            .fetch_optional(&mut **tx)
            .await?;
    match server {
        None => Err(ApiError::Invalid(format!(
            "no stream server {server_id} on this station"
        ))),
        Some(Some(_)) => Err(ApiError::Invalid(
            "a relay server is only read for the statistics: outputs cannot send to it".to_owned(),
        )),
        Some(None) => Ok(()),
    }
}

fn details_json(details: &DetailOverrides) -> ApiResult<Option<String>> {
    if *details == DetailOverrides::default() {
        return Ok(None);
    }
    serde_json::to_string(details)
        .map(Some)
        .map_err(|error| ApiError::Internal(error.to_string()))
}

fn profile_view(profile: EncoderProfile, config: &OutputsConfig) -> EncoderProfileView {
    let outputs = config
        .outputs
        .iter()
        .filter(|output| output.encoder_profile_id == profile.id)
        .count();
    EncoderProfileView {
        id: profile.id,
        name: profile.name,
        encoder: profile.encoder,
        bitrate_kbps: profile.bitrate_kbps,
        sample_rate: profile.sample_rate,
        channels: profile.channels,
        options: profile.options,
        outputs: u32::try_from(outputs).unwrap_or(u32::MAX),
    }
}

fn check_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 255 {
        return Err(ApiError::Invalid(
            "names are 1 to 255 characters".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

fn duplicate_or(error: sqlx::Error, what: &str) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict(format!("another {what} has this name"))
    } else {
        error.into()
    }
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_profiles, create_profile))
        .routes(routes!(get_profile, update_profile, delete_profile))
        .routes(routes!(list_outputs, create_output))
        .routes(routes!(get_output, update_output, delete_output))
        .routes(routes!(duplicate_output))
}

async fn current(state: &AppState, station: Option<&Station>) -> ApiResult<OutputsConfig> {
    let mut connection = state.pool.acquire().await?;
    load_config(&mut connection, station.map(|station| station.id())).await
}

#[utoipa::path(get, path = "/encoder-profiles", tag = "outputs",
    responses((status = 200, body = Vec<EncoderProfileView>), (status = "4XX", body = ErrorBody)))]
async fn list_profiles(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<EncoderProfileView>>> {
    let config = current(&state, None).await?;
    let profiles = config
        .encoder_profiles
        .iter()
        .cloned()
        .map(|profile| profile_view(profile, &config))
        .collect();
    Ok(Json(profiles))
}

#[utoipa::path(get, path = "/encoder-profiles/{id}", tag = "outputs",
    params(("id" = u64, Path)),
    responses((status = 200, body = EncoderProfileView), (status = "4XX", body = ErrorBody)))]
async fn get_profile(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<EncoderProfileView>> {
    let config = current(&state, None).await?;
    let profile = config
        .encoder_profiles
        .iter()
        .find(|profile| profile.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("encoder profile"))?;
    Ok(Json(profile_view(profile, &config)))
}

/// Adds an encoder profile. Admins only.
#[utoipa::path(post, path = "/encoder-profiles", tag = "outputs",
    request_body = EncoderProfileInput,
    responses((status = 201, body = EncoderProfileView), (status = "4XX", body = ErrorBody)))]
async fn create_profile(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<EncoderProfileInput>,
) -> ApiResult<(StatusCode, Json<EncoderProfileView>)> {
    user.require(Role::Admin)?;
    let name = check_name(&input.name)?;
    let mut tx = state.pool.begin().await?;
    let id = sqlx::query(
        "INSERT INTO encoder_profiles (name, encoder, bitrate_kbps, sample_rate, channels, options)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&name)
    .bind(enum_text(input.encoder))
    .bind(input.bitrate_kbps)
    .bind(input.sample_rate)
    .bind(input.channels)
    .bind(input.options.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|error| duplicate_or(error, "encoder profile"))?
    .last_insert_id();
    commit(&state, None, tx).await?;
    tracing::info!(user = %user.username, id, %name, "encoder profile created");
    let Json(view) = get_profile(user, State(state), Path(id)).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// Replaces an encoder profile. Outputs using it restart with the new
/// encoding. Admins only.
#[utoipa::path(put, path = "/encoder-profiles/{id}", tag = "outputs",
    params(("id" = u64, Path)), request_body = EncoderProfileInput,
    responses((status = 200, body = EncoderProfileView), (status = "4XX", body = ErrorBody)))]
async fn update_profile(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(input): Json<EncoderProfileInput>,
) -> ApiResult<Json<EncoderProfileView>> {
    user.require(Role::Admin)?;
    let name = check_name(&input.name)?;
    let mut tx = state.pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE encoder_profiles SET name = ?, encoder = ?, bitrate_kbps = ?, sample_rate = ?,
                channels = ?, options = ?
         WHERE id = ?",
    )
    .bind(&name)
    .bind(enum_text(input.encoder))
    .bind(input.bitrate_kbps)
    .bind(input.sample_rate)
    .bind(input.channels)
    .bind(input.options.to_string())
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|error| duplicate_or(error, "encoder profile"))?;
    if updated.rows_affected() == 0 && !row_exists(&mut tx, "encoder_profiles", id).await? {
        return Err(ApiError::not_found("encoder profile"));
    }
    commit(&state, None, tx).await?;
    tracing::info!(user = %user.username, id, %name, "encoder profile changed");
    get_profile(user, State(state), Path(id)).await
}

/// Deletes an encoder profile no output uses. Admins only.
#[utoipa::path(delete, path = "/encoder-profiles/{id}", tag = "outputs",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_profile(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    let mut tx = state.pool.begin().await?;
    let deleted = sqlx::query("DELETE FROM encoder_profiles WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if is_still_used(&error) {
                ApiError::Conflict("outputs still use this encoder profile".to_owned())
            } else {
                error.into()
            }
        })?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found("encoder profile"));
    }
    commit(&state, None, tx).await?;
    tracing::info!(user = %user.username, id, "encoder profile deleted");
    Ok(StatusCode::NO_CONTENT)
}

fn statuses(station: &Station) -> Vec<OutputStatus> {
    station
        .engine
        .state()
        .borrow()
        .as_ref()
        .map(|player| player.outputs.clone())
        .unwrap_or_default()
}

/// A station's outputs, as stored and as the engine gets them.
async fn outputs_of(
    state: &AppState,
    station: &Station,
) -> ApiResult<(Vec<StoredOutput>, OutputsConfig)> {
    let mut connection = state.pool.acquire().await?;
    let stored = load_outputs(&mut connection, Some(station.id())).await?;
    let config = load_config(&mut connection, Some(station.id())).await?;
    Ok((stored, config))
}

fn views(
    stored: Vec<StoredOutput>,
    config: &OutputsConfig,
    statuses: &[OutputStatus],
) -> Vec<OutputView> {
    stored
        .into_iter()
        .filter_map(|output| {
            let engine = config.outputs.iter().find(|o| o.id == output.id)?;
            let status = statuses.iter().find(|s| s.id == output.id).cloned();
            Some(output_view(output, engine, status))
        })
        .collect()
}

/// Outputs with their live state.
#[utoipa::path(get, path = "/outputs", tag = "outputs",
    responses((status = 200, body = Vec<OutputView>), (status = "4XX", body = ErrorBody)))]
async fn list_outputs(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Vec<OutputView>>> {
    let (stored, config) = outputs_of(&state, &station).await?;
    Ok(Json(views(stored, &config, &statuses(&station))))
}

#[utoipa::path(get, path = "/outputs/{id}", tag = "outputs",
    params(("id" = u64, Path)),
    responses((status = 200, body = OutputView), (status = "4XX", body = ErrorBody)))]
async fn get_output(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<OutputView>> {
    let (stored, config) = outputs_of(&state, &station).await?;
    let stored = stored
        .into_iter()
        .filter(|output| output.id == id)
        .collect();
    views(stored, &config, &statuses(&station))
        .pop()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("output"))
}

fn write_error(error: sqlx::Error, profile: u64) -> ApiError {
    if is_still_used(&error) {
        ApiError::Invalid(format!("no encoder profile {profile}"))
    } else {
        duplicate_or(error, "output")
    }
}

/// Adds an output; the engine starts streaming to it if it is enabled.
/// Admins only.
#[utoipa::path(post, path = "/outputs", tag = "outputs",
    request_body = OutputInput,
    responses((status = 201, body = OutputView), (status = "4XX", body = ErrorBody)))]
async fn create_output(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(input): Json<OutputInput>,
) -> ApiResult<(StatusCode, Json<OutputView>)> {
    user.require(Role::Admin)?;
    let name = check_name(&input.name)?;
    let mut tx = state.pool.begin().await?;
    check_output(&mut tx, &station, &input).await?;
    let id = sqlx::query(
        "INSERT INTO outputs (station_id, name, kind, server_id, encoder_profile_id, config,
                              details, metadata_enabled, enabled)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(station.id())
    .bind(&name)
    .bind(enum_text(input.kind))
    .bind(input.server_id)
    .bind(input.encoder_profile_id)
    .bind(input.config.to_string())
    .bind(details_json(&input.details)?)
    .bind(input.metadata_enabled)
    .bind(input.enabled)
    .execute(&mut *tx)
    .await
    .map_err(|error| write_error(error, input.encoder_profile_id))?
    .last_insert_id();
    commit(&state, Some(&station), tx).await?;
    tracing::info!(user = %user.username, id, %name, station = %station.code(), "output created");
    let Json(view) = get_output(user, State(state), station, Path(id)).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// Replaces an output. Credentials left out of `config` are kept. Admins
/// only.
#[utoipa::path(put, path = "/outputs/{id}", tag = "outputs",
    params(("id" = u64, Path)), request_body = OutputInput,
    responses((status = 200, body = OutputView), (status = "4XX", body = ErrorBody)))]
async fn update_output(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(mut input): Json<OutputInput>,
) -> ApiResult<Json<OutputView>> {
    user.require(Role::Admin)?;
    let name = check_name(&input.name)?;
    let mut tx = state.pool.begin().await?;
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT CAST(config AS CHAR) FROM outputs WHERE id = ? AND station_id = ? FOR UPDATE",
    )
    .bind(id)
    .bind(station.id())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(stored) = stored else {
        return Err(ApiError::not_found("output"));
    };
    keep_secrets(&mut input.config, &parse_json(Some(stored)));
    check_output(&mut tx, &station, &input).await?;
    sqlx::query(
        "UPDATE outputs SET name = ?, kind = ?, server_id = ?, encoder_profile_id = ?, config = ?,
                details = ?, statistics = NULL, metadata_enabled = ?, enabled = ?
         WHERE id = ?",
    )
    .bind(&name)
    .bind(enum_text(input.kind))
    .bind(input.server_id)
    .bind(input.encoder_profile_id)
    .bind(input.config.to_string())
    .bind(details_json(&input.details)?)
    .bind(input.metadata_enabled)
    .bind(input.enabled)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|error| write_error(error, input.encoder_profile_id))?;
    commit(&state, Some(&station), tx).await?;
    tracing::info!(user = %user.username, id, %name, "output changed");
    get_output(user, State(state), station, Path(id)).await
}

/// A name or mount not taken yet: `base`, then `base` and a number.
fn free(base: &str, taken: &[String], numbered: impl Fn(u32) -> String) -> String {
    if !taken.iter().any(|name| name == base) {
        return base.to_owned();
    }
    (2..)
        .map(numbered)
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| base.to_owned())
}

/// Copies an output: the same server, profile and details, its name
/// followed by `(copy)`, its mount by `-copy`, and disabled, so that two
/// outputs never send to one mount. Admins only.
#[utoipa::path(post, path = "/outputs/{id}/duplicate", tag = "outputs",
    params(("id" = u64, Path)),
    responses((status = 201, body = OutputView), (status = "4XX", body = ErrorBody)))]
async fn duplicate_output(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<(StatusCode, Json<OutputView>)> {
    user.require(Role::Admin)?;
    let mut tx = state.pool.begin().await?;
    let outputs = load_outputs(&mut tx, Some(station.id())).await?;
    let original = outputs
        .iter()
        .find(|output| output.id == id)
        .ok_or_else(|| ApiError::not_found("output"))?;
    let names: Vec<String> = outputs.iter().map(|output| output.name.clone()).collect();
    let base = format!("{} (copy)", original.name);
    let name = free(&base, &names, |n| format!("{} (copy {n})", original.name));
    let mut config = original.config.clone();
    if let Some(mount) = config.get("mount").and_then(serde_json::Value::as_str) {
        // Mounts on the same server.
        let mounts: Vec<String> = outputs
            .iter()
            .filter(|output| output.server_id == original.server_id)
            .filter_map(|output| output.config.get("mount")?.as_str().map(str::to_owned))
            .collect();
        let copy = free(&format!("{mount}-copy"), &mounts, |n| {
            format!("{mount}-copy-{n}")
        });
        config["mount"] = copy.into();
    }
    let new_id = sqlx::query(
        "INSERT INTO outputs (station_id, name, kind, server_id, encoder_profile_id, config,
                              details, metadata_enabled, enabled)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, FALSE)",
    )
    .bind(station.id())
    .bind(&name)
    .bind(enum_text(original.kind))
    .bind(original.server_id)
    .bind(original.encoder_profile_id)
    .bind(config.to_string())
    .bind(details_json(&original.details)?)
    .bind(original.metadata_enabled)
    .execute(&mut *tx)
    .await
    .map_err(|error| write_error(error, original.encoder_profile_id))?
    .last_insert_id();
    commit(&state, Some(&station), tx).await?;
    tracing::info!(user = %user.username, id = new_id, from = id, %name, "output duplicated");
    let Json(view) = get_output(user, State(state), station, Path(new_id)).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// Deletes an output; the engine stops streaming to it. Admins only.
#[utoipa::path(delete, path = "/outputs/{id}", tag = "outputs",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_output(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    let mut tx = state.pool.begin().await?;
    let deleted = sqlx::query("DELETE FROM outputs WHERE id = ? AND station_id = ?")
        .bind(id)
        .bind(station.id())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found("output"));
    }
    commit(&state, Some(&station), tx).await?;
    tracing::info!(user = %user.username, id, "output deleted");
    Ok(StatusCode::NO_CONTENT)
}

async fn row_exists(
    tx: &mut Transaction<'_, MySql>,
    table: &'static str,
    id: u64,
) -> ApiResult<bool> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE id = ?");
    let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(count > 0)
}

/// Copies the engine's own outputs into an empty database, the first time
/// the API manages the engine (stations set up with `heron-engine ctl`
/// before M4). Profile IDs are kept when they are free or identical, so
/// outputs keep streaming; they are grouped into stream servers. Returns whether anything was copied.
pub async fn adopt(
    pool: &sqlx::MySqlPool,
    station_id: u64,
    engine: &OutputsConfig,
) -> ApiResult<bool> {
    let mut tx = pool.begin().await?;
    let stored = load_config(&mut tx, Some(station_id)).await?;
    if !stored.outputs.is_empty() || engine.outputs.is_empty() {
        return Ok(false);
    }
    let mut profile_ids = std::collections::HashMap::new();
    for profile in &engine.encoder_profiles {
        let same = |stored: &EncoderProfile| {
            stored.name == profile.name
                && stored.encoder == profile.encoder
                && stored.bitrate_kbps == profile.bitrate_kbps
                && stored.sample_rate == profile.sample_rate
                && stored.channels == profile.channels
        };
        if let Some(existing) = stored.encoder_profiles.iter().find(|p| same(p)) {
            profile_ids.insert(profile.id, existing.id);
            continue;
        }
        let id_taken = stored.encoder_profiles.iter().any(|p| p.id == profile.id);
        let name_taken = stored
            .encoder_profiles
            .iter()
            .any(|p| p.name == profile.name);
        let name = if name_taken {
            format!("{} (engine)", profile.name)
        } else {
            profile.name.clone()
        };
        let id = sqlx::query(
            "INSERT INTO encoder_profiles (id, name, encoder, bitrate_kbps, sample_rate, channels, options)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind((!id_taken).then_some(profile.id))
        .bind(name)
        .bind(enum_text(profile.encoder))
        .bind(profile.bitrate_kbps)
        .bind(profile.sample_rate)
        .bind(profile.channels)
        .bind(profile.options.to_string())
        .execute(&mut *tx)
        .await?
        .last_insert_id();
        profile_ids.insert(profile.id, id);
    }
    for output in &engine.outputs {
        let Some(profile_id) = profile_ids.get(&output.encoder_profile_id) else {
            continue;
        };
        // Another station's output may have the same ID.
        let id_taken = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM outputs WHERE id = ?")
            .bind(output.id)
            .fetch_one(&mut *tx)
            .await?
            > 0;
        sqlx::query(
            "INSERT INTO outputs (id, station_id, name, kind, encoder_profile_id, config,
                                  metadata_enabled, enabled)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind((!id_taken).then_some(output.id))
        .bind(station_id)
        .bind(&output.name)
        .bind(enum_text(output.kind))
        .bind(profile_id)
        .bind(output.config.to_string())
        .bind(output.metadata_enabled)
        .bind(output.enabled)
        .execute(&mut *tx)
        .await?;
    }
    // They hold their server's settings, as before stream servers.
    sqlx::raw_sql(GROUP_INTO_SERVERS).execute(&mut *tx).await?;
    tx.commit().await?;
    tracing::info!(
        outputs = engine.outputs.len(),
        "copied the engine's outputs into the database"
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_hidden_and_kept() {
        let stored = serde_json::json!({"directory": "/hls", "password": "hackme"});
        let (shown, secrets) = redact(&stored);
        assert_eq!(shown, serde_json::json!({"directory": "/hls"}));
        assert_eq!(secrets, ["password"]);

        let mut update = serde_json::json!({"directory": "/other"});
        keep_secrets(&mut update, &stored);
        assert_eq!(update["password"], "hackme");
        let mut replaced = serde_json::json!({"directory": "/other", "password": "new"});
        keep_secrets(&mut replaced, &stored);
        assert_eq!(replaced["password"], "new");
    }

    #[test]
    fn enums_match_the_database() {
        assert_eq!(enum_text(EncoderKind::ExternalFfmpeg), "external_ffmpeg");
        assert_eq!(enum_text(OutputKind::ShoutcastV2), "shoutcast_v2");
        let kind: OutputKind = parse_enum("icecast".to_owned()).unwrap();
        assert_eq!(kind, OutputKind::Icecast);
    }

    fn server() -> StoredServer {
        StoredServer {
            id: 1,
            station_id: 1,
            name: "icecast:8000".to_owned(),
            host: "icecast".to_owned(),
            port: 8000,
            tls: false,
            source_username: None,
            source_password: Some("hackme".to_owned()),
            admin_username: None,
            admin_password: Some("admin-secret".to_owned()),
            public_url: Some("https://stream.example.org".to_owned()),
            relay_of: None,
            relay_mounts: None,
            relay_subtract_one: false,
        }
    }

    #[test]
    fn the_engine_gets_the_full_settings() {
        let details = StreamDetails {
            name: Some("Station HD".to_owned()),
            genre: Some("Pop".to_owned()),
            ..StreamDetails::default()
        };
        let settings = icecast_settings(&server(), &serde_json::json!({"mount": "/hd"}), &details);
        assert_eq!(
            settings,
            serde_json::json!({"host": "icecast", "port": 8000, "mount": "/hd",
                "username": "source", "password": "hackme", "name": "Station HD",
                "genre": "Pop", "public": false, "public_url": "https://stream.example.org"})
        );
        // The admin credentials never reach the engine.
        assert!(!settings.to_string().contains("admin-secret"));
        let parsed = IcecastConfig::parse("HD", &settings).unwrap();
        assert_eq!(parsed.listen_url(), "https://stream.example.org/hd");
    }

    #[test]
    fn settings_meaning_the_same_keep_the_engine_s() {
        let output = |config: serde_json::Value| OutputConfig {
            id: 3,
            name: "Live".to_owned(),
            kind: OutputKind::Icecast,
            encoder_profile_id: 1,
            config,
            metadata_enabled: true,
            enabled: true,
        };
        // As written by an earlier Heron: defaults left out, an empty name.
        let legacy = serde_json::json!({"host": "icecast", "mount": "/live", "password": "hackme",
            "name": ""});
        let engine = OutputsConfig {
            encoder_profiles: Vec::new(),
            outputs: vec![output(legacy.clone())],
        };
        let joined = icecast_settings(
            &StoredServer {
                public_url: None,
                ..server()
            },
            &serde_json::json!({"mount": "/live"}),
            &StreamDetails::default(),
        );
        let mut new = OutputsConfig {
            encoder_profiles: Vec::new(),
            outputs: vec![output(joined.clone())],
        };
        keep_engine_shape(&mut new, &engine);
        assert_eq!(new, engine);

        // A new password is a change.
        let mut changed = joined;
        changed["password"] = "new".into();
        let mut new = OutputsConfig {
            encoder_profiles: Vec::new(),
            outputs: vec![output(changed.clone())],
        };
        keep_engine_shape(&mut new, &engine);
        assert_eq!(new.outputs[0].config, changed);
    }

    #[test]
    fn copies_take_free_names() {
        let taken = vec!["/live".to_owned(), "/live-copy".to_owned()];
        assert_eq!(
            free("/live-copy", &taken, |n| format!("/live-copy-{n}")),
            "/live-copy-2"
        );
        assert_eq!(
            free("/aac-copy", &taken, |n| format!("/aac-copy-{n}")),
            "/aac-copy"
        );
    }
}
