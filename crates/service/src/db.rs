// SPDX-License-Identifier: AGPL-3.0-or-later

//! MariaDB connection helpers. Never used by `heron-engine`.

use std::time::Duration;

use sqlx::mysql::{MySqlPool, MySqlPoolOptions};

use crate::{Error, HealthCheck};

/// How long to keep retrying the first connection, and how many connections to open.
#[derive(Clone, Copy, Debug)]
pub struct ConnectOptions {
    pub max_connections: u32,
    pub max_attempts: u32,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            max_connections: 10,
            max_attempts: 30,
        }
    }
}

/// Connects to MariaDB, retrying with exponential backoff (capped at 10 s)
/// so services can start before the database is ready.
///
/// The URL is never logged: it contains the password.
pub async fn connect(url: &str, options: ConnectOptions) -> Result<MySqlPool, Error> {
    let mut delay = Duration::from_millis(500);
    let mut attempt = 1;
    loop {
        let result = MySqlPoolOptions::new()
            .max_connections(options.max_connections)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await;
        match result {
            Ok(pool) => {
                tracing::info!(attempt, "connected to the database");
                return Ok(pool);
            }
            Err(source) if attempt >= options.max_attempts => {
                return Err(Error::Database {
                    attempts: attempt,
                    source,
                });
            }
            Err(error) => {
                tracing::warn!(attempt, %error, retry_in = ?delay, "database not reachable yet");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(10));
                attempt += 1;
            }
        }
    }
}

/// Health check that pings the database.
#[derive(Clone, Debug)]
pub struct DatabaseHealth(pub MySqlPool);

impl HealthCheck for DatabaseHealth {
    async fn check(&self) -> Result<(), String> {
        sqlx::query("SELECT 1")
            .execute(&self.0)
            .await
            .map(|_| ())
            .map_err(|error| format!("database: {error}"))
    }
}

/// The schema migrations in `migrations/`, applied by `heron-api` on startup.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// A throwaway database for integration tests, created from an admin URL
/// (one allowed to create databases, without a database name) and migrated.
#[cfg(feature = "test-util")]
pub struct ScratchDatabase {
    pub pool: MySqlPool,
    admin_url: String,
    name: String,
}

#[cfg(feature = "test-util")]
impl ScratchDatabase {
    /// Uses `HERON_TEST_DATABASE_URL`, e.g. `mysql://root:root@127.0.0.1:3306`.
    /// Returns `None` (and the test should return early) when it is not set,
    /// and panics when it is set but unusable, so CI cannot skip silently.
    #[allow(clippy::expect_used)]
    pub async fn create() -> Option<Self> {
        let Ok(admin_url) = std::env::var("HERON_TEST_DATABASE_URL") else {
            eprintln!("HERON_TEST_DATABASE_URL not set, skipping");
            return None;
        };
        let admin_url = admin_url.trim_end_matches('/').to_owned();
        let name = format!(
            "heron_test_{}_{}",
            std::process::id(),
            SCRATCH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let admin = MySqlPool::connect(&admin_url)
            .await
            .expect("cannot connect to HERON_TEST_DATABASE_URL");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE `{name}`")))
            .execute(&admin)
            .await
            .expect("cannot create the scratch database");
        admin.close().await;
        let pool = MySqlPool::connect(&format!("{admin_url}/{name}"))
            .await
            .expect("cannot connect to the scratch database");
        MIGRATOR
            .run(&pool)
            .await
            .expect("cannot migrate the scratch database");
        Some(Self {
            pool,
            admin_url,
            name,
        })
    }

    /// URL of the scratch database, for child processes.
    pub fn url(&self) -> String {
        format!("{}/{}", self.admin_url, self.name)
    }

    /// Drops the database. Call it at the end of the test.
    pub async fn drop(self) {
        self.pool.close().await;
        if let Ok(admin) = MySqlPool::connect(&self.admin_url).await {
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE `{}`",
                self.name
            )))
            .execute(&admin)
            .await;
        }
    }
}

#[cfg(feature = "test-util")]
static SCRATCH_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(test)]
mod tests {
    use super::MIGRATOR;

    /// Two migrations with the same version break every upgrade: the
    /// database keeps the checksum of the one applied first, and the API
    /// then refuses to start ("previously applied but has been modified").
    #[test]
    fn migration_versions_are_unique() {
        let mut seen = std::collections::HashMap::new();
        for migration in MIGRATOR.iter() {
            if let Some(other) = seen.insert(migration.version, &migration.description) {
                panic!(
                    "migrations {} share version {}: give one of them the next free version",
                    if other < &migration.description {
                        format!("{other} and {}", migration.description)
                    } else {
                        format!("{} and {other}", migration.description)
                    },
                    migration.version
                );
            }
        }
    }
}
