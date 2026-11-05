// SPDX-License-Identifier: AGPL-3.0-or-later

//! The master AGC chain of each station: its settings, pushed to the
//! engine as soon as they change, and the presets the stations share.
//! Presets stay here: loading one is the client copying its blocks into a
//! chain. See `docs/api.md#agc`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use heron_core::AgcSettings;
use heron_core::agc::{
    BassEqSettings, DualBandSettings, FinalStageSettings, FiveBandSettings, GatedAgcSettings,
    StereoExpanderSettings,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody, is_duplicate};
use crate::settings;
use crate::stations::Station;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_agc, put_agc))
        .routes(routes!(list_agc_presets, create_agc_preset))
        .routes(routes!(update_agc_preset, delete_agc_preset))
}

/// The station's AGC chain.
#[utoipa::path(get, path = "/settings/agc", tag = "settings",
    responses((status = 200, body = AgcSettings), (status = "4XX", body = ErrorBody)))]
async fn get_agc(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<AgcSettings>> {
    Ok(Json(
        settings::load_for(&state.pool, station.id(), AgcSettings::SETTINGS_NAME).await?,
    ))
}

/// Replaces the station's AGC chain (missing fields take the defaults) and
/// pushes it to the engine at once, where it glides in. Every open live
/// WebSocket gets an `agc` message. Operators and admins.
#[utoipa::path(put, path = "/settings/agc", tag = "settings",
    request_body = AgcSettings,
    responses((status = 200, body = AgcSettings), (status = "4XX", body = ErrorBody)))]
async fn put_agc(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(agc): Json<AgcSettings>,
) -> ApiResult<Json<AgcSettings>> {
    user.require(Role::Operator)?;
    agc.check().map_err(ApiError::Invalid)?;
    settings::save_for(&state.pool, station.id(), AgcSettings::SETTINGS_NAME, &agc).await?;
    // Nobody listening is fine.
    let _ = station.agc_changes.send(agc);
    let json =
        serde_json::to_string(&agc).map_err(|error| ApiError::Internal(error.to_string()))?;
    // Saved either way: the configuration sync pushes it when the engine
    // is back, and reports a refusal.
    match station.engine.set_agc_settings(json).await {
        Ok(()) | Err(ApiError::EngineUnavailable) => {}
        Err(error) => {
            tracing::warn!(%error, "the engine refused the AGC settings");
            station.sync.changed();
        }
    }
    Ok(Json(agc))
}

/// The blocks of an AGC chain, as a preset keeps them: the chain's
/// settings without `bypass_all`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AgcPresetSettings {
    pub gated_agc: GatedAgcSettings,
    pub stereo_expander: StereoExpanderSettings,
    pub bass_eq: BassEqSettings,
    pub five_band: FiveBandSettings,
    pub dual_band: DualBandSettings,
    pub final_stage: FinalStageSettings,
}

impl AgcPresetSettings {
    /// The chain with these blocks.
    pub fn chain(self, bypass_all: bool) -> AgcSettings {
        AgcSettings {
            bypass_all,
            gated_agc: self.gated_agc,
            stereo_expander: self.stereo_expander,
            bass_eq: self.bass_eq,
            five_band: self.five_band,
            dual_band: self.dual_band,
            final_stage: self.final_stage,
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AgcPreset {
    pub id: u64,
    pub name: String,
    pub settings: AgcPresetSettings,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgcPresetInput {
    /// Unique, ignoring case.
    pub name: String,
    /// Missing fields take the defaults.
    pub settings: AgcPresetSettings,
}

impl AgcPresetInput {
    fn check(&self) -> ApiResult<(String, String)> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(ApiError::Invalid(
                "preset names are 1 to 100 characters".to_owned(),
            ));
        }
        self.settings
            .chain(false)
            .check()
            .map_err(ApiError::Invalid)?;
        let settings = serde_json::to_string(&self.settings)
            .map_err(|error| ApiError::Internal(error.to_string()))?;
        Ok((name.to_owned(), settings))
    }
}

#[derive(sqlx::FromRow)]
struct PresetRow {
    id: u64,
    name: String,
    settings: String,
}

impl TryFrom<PresetRow> for AgcPreset {
    type Error = ApiError;

    fn try_from(row: PresetRow) -> ApiResult<Self> {
        let settings = serde_json::from_str(&row.settings).map_err(|error| {
            ApiError::Internal(format!("stored AGC preset {}: {error}", row.id))
        })?;
        Ok(Self {
            id: row.id,
            name: row.name,
            settings,
        })
    }
}

const SELECT: &str = "SELECT id, name, CAST(settings AS CHAR) AS settings FROM agc_presets";

fn conflict(error: sqlx::Error) -> ApiError {
    if is_duplicate(&error) {
        ApiError::Conflict("another preset has this name".to_owned())
    } else {
        error.into()
    }
}

async fn find(state: &AppState, id: u64) -> ApiResult<AgcPreset> {
    sqlx::query_as::<_, PresetRow>(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = ?")))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("AGC preset"))?
        .try_into()
}

/// Every AGC preset, by name.
#[utoipa::path(get, path = "/agc-presets", tag = "settings",
    responses((status = 200, body = Vec<AgcPreset>), (status = "4XX", body = ErrorBody)))]
async fn list_agc_presets(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<AgcPreset>>> {
    let rows: Vec<PresetRow> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} ORDER BY name")))
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(
        rows.into_iter()
            .map(AgcPreset::try_from)
            .collect::<ApiResult<_>>()?,
    ))
}

/// Saves a chain's blocks as a new preset. Operators and admins.
#[utoipa::path(post, path = "/agc-presets", tag = "settings",
    request_body = AgcPresetInput,
    responses((status = 201, body = AgcPreset), (status = "4XX", body = ErrorBody)))]
async fn create_agc_preset(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<AgcPresetInput>,
) -> ApiResult<(StatusCode, Json<AgcPreset>)> {
    user.require(Role::Operator)?;
    let (name, settings) = input.check()?;
    let id = sqlx::query("INSERT INTO agc_presets (name, settings) VALUES (?, ?)")
        .bind(name)
        .bind(settings)
        .execute(&state.pool)
        .await
        .map_err(conflict)?
        .last_insert_id();
    Ok((StatusCode::CREATED, Json(find(&state, id).await?)))
}

/// Renames a preset or saves new blocks in it. The chains it was loaded
/// into keep their settings. Operators and admins.
#[utoipa::path(put, path = "/agc-presets/{id}", tag = "settings",
    params(("id" = u64, Path)), request_body = AgcPresetInput,
    responses((status = 200, body = AgcPreset), (status = "4XX", body = ErrorBody)))]
async fn update_agc_preset(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(input): Json<AgcPresetInput>,
) -> ApiResult<Json<AgcPreset>> {
    user.require(Role::Operator)?;
    let (name, settings) = input.check()?;
    sqlx::query("UPDATE agc_presets SET name = ?, settings = ? WHERE id = ?")
        .bind(name)
        .bind(settings)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(conflict)?;
    Ok(Json(find(&state, id).await?))
}

/// Deletes a preset. The chains it was loaded into keep their settings.
/// Operators and admins.
#[utoipa::path(delete, path = "/agc-presets/{id}", tag = "settings",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_agc_preset(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let deleted = sqlx::query("DELETE FROM agc_presets WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found("AGC preset"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The presets a fresh install has, as the migration writes them.
    fn seeded() -> Vec<(String, serde_json::Value)> {
        let sql = include_str!("../../../migrations/20261123000000_agc_presets.sql");
        let values = &sql[sql.find("VALUES").unwrap()..];
        values
            .split("    ('")
            .skip(1)
            .map(|row| {
                let (name, rest) = row.split_once("', '").unwrap();
                let json = &rest[..rest.rfind("}'").unwrap() + 1];
                (name.to_owned(), serde_json::from_str(json).unwrap())
            })
            .collect()
    }

    /// Equal, numbers compared as numbers (`-45` and `-45.0`).
    fn same(a: &serde_json::Value, b: &serde_json::Value) -> bool {
        use serde_json::Value;
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, a)| b.get(key).is_some_and(|b| same(a, b)))
            }
            _ => a == b,
        }
    }

    #[test]
    fn the_seeded_presets_are_complete_and_valid() {
        let presets = seeded();
        let names: Vec<&str> = presets.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Gentle", "Pop", "Dense", "Rock", "Talk"]);
        for (name, json) in presets {
            let settings: AgcPresetSettings = serde_json::from_value(json.clone())
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(settings.chain(false).check(), Ok(()), "{name}");
            // Every field written out: a later change of a default does not
            // change a preset.
            assert!(
                same(&serde_json::to_value(settings).unwrap(), &json),
                "{name}: some field is missing"
            );
            assert!(settings.gated_agc.enabled, "{name}");
            assert!(settings.final_stage.enabled, "{name}");
        }
    }

    #[test]
    fn presets_have_no_bypass() {
        let input: Result<AgcPresetInput, _> =
            serde_json::from_str(r#"{"name": "x", "settings": {"bypass_all": false}}"#);
        assert!(input.is_err());
        let input: AgcPresetInput =
            serde_json::from_str(r#"{"name": " Mine ", "settings": {}}"#).unwrap();
        let (name, settings) = input.check().unwrap();
        assert_eq!(name, "Mine");
        let settings: AgcPresetSettings = serde_json::from_str(&settings).unwrap();
        assert_eq!(settings.chain(true), AgcSettings::default());
    }

    #[test]
    fn presets_are_checked() {
        let mut input = AgcPresetInput {
            name: "Loud".to_owned(),
            settings: AgcPresetSettings::default(),
        };
        input.settings.five_band.crossovers_hz = [100.0, 90.0, 1_600.0, 6_400.0];
        assert!(input.check().is_err());
        input.settings = AgcPresetSettings::default();
        input.name = "  ".to_owned();
        assert!(input.check().is_err());
    }
}
