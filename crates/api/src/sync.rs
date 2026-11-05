// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keeps the engine's configuration (playout settings, encoder profiles and
//! outputs, deck EQ, AGC chain) equal to the database's.
//!
//! The engine keeps its own copy on disk and plays on with it while the API
//! or the database are down. This task pushes the database's version when
//! it changes, when the engine (re)starts or reconnects, and once a minute.
//!
//! The first time, the API adopts the engine's configuration instead of
//! replacing it (stations configured with `heron-engine ctl` before M4):
//! playout settings, deck EQ and AGC chain never saved through the API and
//! outputs, when the database has none, are copied from the engine.
//!
//! Engines from before the deck EQ or the AGC chain send no settings for
//! them: none are pushed to them.

use std::sync::Arc;
use std::time::Duration;

use heron_core::{AgcSettings, EqSettings, OutputsConfig, PlayoutSettings};
use serde::Serialize;
use sqlx::MySqlPool;
use tokio::sync::{Notify, watch};
use utoipa::ToSchema;

use crate::engine::{EngineLink, unix_ms};
use crate::error::{ApiError, ApiResult};
use crate::outputs;
use crate::settings::{self, EngineManagement};

const RECHECK: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// Not checked yet.
    Unknown,
    /// The engine has the configuration of the database.
    Synced,
    /// Changes wait for the engine to be reachable.
    Pending,
    /// The engine refused the configuration (see `error`).
    Failed,
}

/// Whether the engine runs with the configuration stored in the database.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct SyncStatus {
    pub state: SyncState,
    pub error: Option<String>,
    pub checked_at_unix_ms: Option<u64>,
}

pub struct ConfigSync {
    pool: MySqlPool,
    engine: Arc<EngineLink>,
    station_id: u64,
    wake: Notify,
    status: watch::Sender<SyncStatus>,
}

/// Resolves when the engine becomes reachable or restarts (a different
/// start time than `started_at`).
async fn new_engine(
    engine: &mut watch::Receiver<Option<Arc<crate::engine::PlayerState>>>,
    started_at: Option<u64>,
) {
    loop {
        if engine.changed().await.is_err() {
            return std::future::pending().await;
        }
        let now = engine
            .borrow()
            .as_ref()
            .map(|state| state.engine_started_at_unix_ms);
        if now.is_some() && now != started_at {
            return;
        }
    }
}

impl ConfigSync {
    pub fn new(pool: MySqlPool, engine: Arc<EngineLink>, station_id: u64) -> Arc<Self> {
        Arc::new(Self {
            pool,
            engine,
            station_id,
            wake: Notify::new(),
            status: watch::channel(SyncStatus {
                state: SyncState::Unknown,
                error: None,
                checked_at_unix_ms: None,
            })
            .0,
        })
    }

    /// The configuration changed in the database: push it.
    pub fn changed(&self) {
        self.wake.notify_one();
    }

    pub fn status(&self) -> SyncStatus {
        self.status.borrow().clone()
    }

    pub async fn run(self: Arc<Self>) {
        let mut engine = self.engine.state();
        loop {
            let started_at = engine
                .borrow_and_update()
                .as_ref()
                .map(|state| state.engine_started_at_unix_ms);
            if started_at.is_some() {
                let result = self.sync().await;
                self.record(result);
            } else {
                self.status.send_modify(|status| {
                    if status.state == SyncState::Synced {
                        status.state = SyncState::Pending;
                    }
                });
            }
            tokio::select! {
                () = self.wake.notified() => {}
                () = tokio::time::sleep(RECHECK) => {}
                () = new_engine(&mut engine, started_at) => {}
            }
        }
    }

    fn record(&self, result: ApiResult<()>) {
        let checked_at_unix_ms = Some(unix_ms(std::time::SystemTime::now()));
        let (state, error) = match result {
            Ok(()) => (SyncState::Synced, None),
            Err(ApiError::EngineUnavailable) => (SyncState::Pending, None),
            Err(error) => {
                tracing::error!(%error, "cannot apply the configuration to the engine");
                (SyncState::Failed, Some(error.to_string()))
            }
        };
        self.status.send_replace(SyncStatus {
            state,
            error,
            checked_at_unix_ms,
        });
    }

    async fn sync(&self) -> ApiResult<()> {
        let config = self.engine.config().await?;
        let engine_playout: PlayoutSettings =
            serde_json::from_str(&config.playout_settings_json)
                .map_err(|error| ApiError::Internal(format!("engine playout settings: {error}")))?;
        let engine_outputs: OutputsConfig = serde_json::from_str(&config.outputs_json)
            .map_err(|error| ApiError::Internal(format!("engine outputs: {error}")))?;
        let engine_eq: Option<EqSettings> = optional(&config.eq_settings_json, "EQ")?;
        let engine_agc: Option<AgcSettings> = optional(&config.agc_settings_json, "AGC")?;

        let management: EngineManagement =
            settings::load_for(&self.pool, self.station_id, EngineManagement::SETTINGS_NAME)
                .await?;
        if !management.managed {
            self.adopt(&engine_playout, engine_eq.as_ref(), &engine_outputs)
                .await?;
            settings::save_for(
                &self.pool,
                self.station_id,
                EngineManagement::SETTINGS_NAME,
                &EngineManagement { managed: true },
            )
            .await?;
        }

        let playout: PlayoutSettings =
            settings::load_for(&self.pool, self.station_id, PlayoutSettings::SETTINGS_NAME).await?;
        if playout != engine_playout {
            let json = serde_json::to_string(&playout)
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            self.engine.set_playout_settings(json).await?;
            tracing::info!("playout settings applied to the engine");
        }

        if let Some(engine_eq) = engine_eq {
            let eq: EqSettings =
                settings::load_for(&self.pool, self.station_id, EqSettings::SETTINGS_NAME).await?;
            if eq != engine_eq {
                let json = serde_json::to_string(&eq)
                    .map_err(|error| ApiError::Internal(error.to_string()))?;
                self.engine.set_eq_settings(json).await?;
                tracing::info!("deck EQ applied to the engine");
            }
        }

        if let Some(engine_agc) = engine_agc {
            // A station managed before M10 has no AGC chain saved yet: the
            // engine's is kept (set with `ctl set-agc`, or bypassed).
            if !settings::exists_for(&self.pool, self.station_id, AgcSettings::SETTINGS_NAME)
                .await?
            {
                settings::save_for(
                    &self.pool,
                    self.station_id,
                    AgcSettings::SETTINGS_NAME,
                    &engine_agc,
                )
                .await?;
                tracing::info!("copied the engine's AGC chain into the database");
            }
            let agc: AgcSettings =
                settings::load_for(&self.pool, self.station_id, AgcSettings::SETTINGS_NAME).await?;
            if agc != engine_agc {
                let json = serde_json::to_string(&agc)
                    .map_err(|error| ApiError::Internal(error.to_string()))?;
                self.engine.set_agc_settings(json).await?;
                tracing::info!("AGC chain applied to the engine");
            }
        }

        let mut connection = self.pool.acquire().await?;
        let outputs = outputs::load_config(&mut connection, Some(self.station_id)).await?;
        drop(connection);
        if outputs != engine_outputs {
            let json = serde_json::to_string(&outputs)
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            self.engine.set_outputs(json).await?;
            tracing::info!(
                outputs = outputs.outputs.len(),
                "outputs applied to the engine"
            );
        }
        Ok(())
    }

    async fn adopt(
        &self,
        playout: &PlayoutSettings,
        eq: Option<&EqSettings>,
        outputs: &OutputsConfig,
    ) -> ApiResult<()> {
        if !settings::exists_for(&self.pool, self.station_id, PlayoutSettings::SETTINGS_NAME)
            .await?
        {
            settings::save_for(
                &self.pool,
                self.station_id,
                PlayoutSettings::SETTINGS_NAME,
                playout,
            )
            .await?;
            tracing::info!("copied the engine's playout settings into the database");
        }
        if let Some(eq) = eq
            && !settings::exists_for(&self.pool, self.station_id, EqSettings::SETTINGS_NAME).await?
        {
            settings::save_for(&self.pool, self.station_id, EqSettings::SETTINGS_NAME, eq).await?;
            tracing::info!("copied the engine's deck EQ into the database");
        }
        outputs::adopt(&self.pool, self.station_id, outputs).await?;
        Ok(())
    }
}

/// Settings an engine may not know yet: empty from engines before them.
fn optional<T: serde::de::DeserializeOwned>(json: &str, what: &str) -> ApiResult<Option<T>> {
    if json.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(json)
        .map(Some)
        .map_err(|error| ApiError::Internal(format!("engine {what} settings: {error}")))
}
