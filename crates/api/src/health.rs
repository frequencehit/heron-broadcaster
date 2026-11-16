// SPDX-License-Identifier: AGPL-3.0-or-later

//! The API's `/healthz`: unhealthy without its database; its warnings say
//! what keeps a station from being fed as it should
//! (docs/monitoring.md#health-checks).

use std::sync::Arc;

use heron_service::HealthCheck;
use heron_service::db::DatabaseHealth;

use crate::stations::Stations;

#[derive(Clone)]
pub struct ApiHealth {
    pub database: DatabaseHealth,
    pub stations: Arc<Stations>,
}

impl HealthCheck for ApiHealth {
    async fn check(&self) -> Result<(), String> {
        self.database.check().await
    }

    /// Settings read with a part left out, and each station's queue:
    /// empty for a while with autofill on, or its loop restarted lately.
    async fn warnings(&self) -> Vec<String> {
        let mut warnings = crate::settings::left_out();
        for station in self.stations.all() {
            warnings.extend(station.queue.warnings());
        }
        warnings
    }
}
