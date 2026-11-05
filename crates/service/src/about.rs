// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a service says of itself for the About page: its build, the machine
//! it runs on and its configuration. See `docs/roadmap/m17-about-page.md`.
//!
//! The configuration is built value by value by each binary, never from a
//! dump of its environment, so a secret added later is not shown by default.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sysinfo::System;

/// A configuration value worth showing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConfigItem {
    /// The setting, as its environment variable.
    pub name: String,
    #[serde(flatten)]
    pub value: ConfigValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ConfigValue {
    /// A value that is not secret.
    Text(String),
    /// A feature that is on or off.
    Flag(bool),
    /// A secret: only whether it is set.
    Secret(bool),
}

impl ConfigItem {
    pub fn text(name: &str, value: impl ToString) -> Self {
        Self {
            name: name.to_owned(),
            value: ConfigValue::Text(value.to_string()),
        }
    }

    /// `value` is shown when set, `unset` otherwise.
    pub fn optional_text(name: &str, value: Option<&str>, unset: &str) -> Self {
        Self::text(
            name,
            value
                .filter(|text| !text.trim().is_empty())
                .unwrap_or(unset),
        )
    }

    pub fn flag(name: &str, on: bool) -> Self {
        Self {
            name: name.to_owned(),
            value: ConfigValue::Flag(on),
        }
    }

    /// A secret: whether it is set, never what it is.
    pub fn secret(name: &str, value: Option<&str>) -> Self {
        Self {
            name: name.to_owned(),
            value: ConfigValue::Secret(value.is_some_and(|text| !text.trim().is_empty())),
        }
    }
}

/// What every service shows: error reporting and metrics, as flags and
/// secrets.
pub fn common_config(
    log: &crate::LogArgs,
    metrics: &crate::MetricsArgs,
    http_addr: std::net::SocketAddr,
) -> Vec<ConfigItem> {
    vec![
        ConfigItem::text("HERON_HTTP_ADDR", http_addr),
        ConfigItem::text(
            "HERON_LOG_FORMAT",
            format!("{:?}", log.log_format).to_lowercase(),
        ),
        ConfigItem::secret("HERON_SENTRY_DSN", log.sentry_dsn.as_deref()),
        ConfigItem::flag(
            "HERON_METRICS_REMOTE_WRITE_URL",
            metrics
                .remote_write_url
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty()),
        ),
        ConfigItem::secret(
            "HERON_METRICS_REMOTE_WRITE_PASSWORD",
            metrics.remote_write_password.as_deref(),
        ),
    ]
}

/// The build and the machine of the running process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub version: String,
    /// The commit the image was built from (`HERON_REVISION`).
    pub revision: Option<String>,
    /// When the image was built (`HERON_BUILD_DATE`).
    pub build_date: Option<String>,
    /// Unix time, in ms, the process started.
    pub started_unix_ms: i64,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub arch: String,
    /// Running in a container (Docker or Kubernetes).
    pub container: bool,
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Whether the process runs in a container: Docker's marker file, or the
/// variable Kubernetes sets.
fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::env::var_os("KUBERNETES_SERVICE_HOST").is_some()
}

impl ProcessInfo {
    /// Reads the build and the machine; `started` is when the process began.
    pub fn read(started: SystemTime) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            revision: env_nonempty("HERON_REVISION"),
            build_date: env_nonempty("HERON_BUILD_DATE"),
            started_unix_ms: unix_ms(started),
            os: System::long_os_version().or_else(|_| System::name()).ok(),
            kernel: System::kernel_version().ok(),
            arch: std::env::consts::ARCH.to_owned(),
            container: in_container(),
        }
    }
}

/// What a service answers the About page: its build and machine, and its
/// configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AboutReport {
    pub process: ProcessInfo,
    pub config: Vec<ConfigItem>,
    /// The first line of `ffmpeg -version`, for the engine when it has one.
    #[serde(default)]
    pub ffmpeg: Option<String>,
}

/// `time` as Unix milliseconds (0 before 1970).
pub fn unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
    })
}

/// How often a service says it is alive.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// A heartbeat older than this means the service is not running.
pub const HEARTBEAT_STALE: Duration = Duration::from_secs(90);

#[cfg(feature = "mysql")]
pub use heartbeat::run as run_heartbeat;

#[cfg(feature = "mysql")]
mod heartbeat {
    use super::{ConfigItem, HEARTBEAT_INTERVAL, ProcessInfo};
    use std::time::SystemTime;
    use tokio::sync::watch;

    /// Says every [`HEARTBEAT_INTERVAL`] that the service `service` (the
    /// instance `instance`) runs, until `stop` changes. A failure is logged:
    /// the About page is never worth stopping a service for.
    pub async fn run(
        pool: sqlx::MySqlPool,
        service: &'static str,
        instance: String,
        config: Vec<ConfigItem>,
        mut stop: watch::Receiver<bool>,
    ) {
        let info = ProcessInfo::read(SystemTime::now());
        let config = serde_json::to_string(&config).unwrap_or_else(|_| "[]".to_owned());
        let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = stop.changed() => return,
            }
            if let Err(error) = beat(&pool, service, &instance, &info, &config).await {
                tracing::warn!(%error, "cannot record the heartbeat");
            }
        }
    }

    pub(super) async fn beat(
        pool: &sqlx::MySqlPool,
        service: &str,
        instance: &str,
        info: &ProcessInfo,
        config: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO service_heartbeats
                 (service, instance, version, revision, build_date, started_at, seen_at,
                  os, kernel, arch, container, config)
             VALUES (?, ?, ?, ?, ?, FROM_UNIXTIME(? / 1000), UTC_TIMESTAMP(6), ?, ?, ?, ?, ?)
             ON DUPLICATE KEY UPDATE
                 version = VALUES(version), revision = VALUES(revision),
                 build_date = VALUES(build_date), started_at = VALUES(started_at),
                 seen_at = VALUES(seen_at), os = VALUES(os), kernel = VALUES(kernel),
                 arch = VALUES(arch), container = VALUES(container), config = VALUES(config)",
        )
        .bind(service)
        .bind(instance)
        .bind(&info.version)
        .bind(&info.revision)
        .bind(&info.build_date)
        .bind(info.started_unix_ms)
        .bind(&info.os)
        .bind(&info.kernel)
        .bind(&info.arch)
        .bind(info.container)
        .bind(config)
        .execute(pool)
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_shows_only_whether_it_is_set() {
        let item = ConfigItem::secret("HERON_SENTRY_DSN", Some("https://key@example.com/1"));
        let json = serde_json::to_string(&item).unwrap();
        assert!(!json.contains("example.com"));
        assert_eq!(item.value, ConfigValue::Secret(true));
        assert_eq!(
            ConfigItem::secret("X", Some("  ")).value,
            ConfigValue::Secret(false)
        );
        assert_eq!(
            ConfigItem::secret("X", None).value,
            ConfigValue::Secret(false)
        );
    }

    #[test]
    fn an_empty_value_shows_what_stands_for_it() {
        let shown = |value: Option<&str>| ConfigItem::optional_text("HERON_X", value, "none").value;
        assert_eq!(
            shown(Some("/music")),
            ConfigValue::Text("/music".to_owned())
        );
        assert_eq!(shown(Some("  ")), ConfigValue::Text("none".to_owned()));
        assert_eq!(shown(None), ConfigValue::Text("none".to_owned()));
    }

    #[test]
    fn items_serialize_with_their_kind() {
        let json = serde_json::to_value(ConfigItem::flag("HERON_LIBRARY_WATCH", true)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"name": "HERON_LIBRARY_WATCH", "kind": "flag", "value": true})
        );
    }

    #[test]
    fn the_process_reports_its_build() {
        let info = ProcessInfo::read(SystemTime::now());
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.arch.is_empty());
    }

    /// The heartbeat writes a row, updates it and keeps the start time.
    #[cfg(feature = "test-util")]
    #[tokio::test]
    async fn a_heartbeat_is_recorded_and_updated() {
        let Some(db) = crate::db::ScratchDatabase::create().await else {
            return;
        };
        let info = ProcessInfo::read(SystemTime::now() - Duration::from_secs(3_600));
        let config = serde_json::to_string(&[ConfigItem::flag("HERON_MONITOR", true)]).unwrap();
        for _ in 0..2 {
            super::heartbeat::beat(&db.pool, "heron-analyzer", "box", &info, &config)
                .await
                .unwrap();
        }
        let (count, uptime, age, stored): (i64, i64, i64, String) = sqlx::query_as(
            "SELECT COUNT(*),
                    CAST(TIMESTAMPDIFF(SECOND, MIN(started_at), UTC_TIMESTAMP(6)) AS SIGNED),
                    CAST(TIMESTAMPDIFF(SECOND, MIN(seen_at), UTC_TIMESTAMP(6)) AS SIGNED),
                    CAST(MIN(config) AS CHAR)
             FROM service_heartbeats",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        assert!((3_599..=3_605).contains(&uptime), "uptime {uptime}");
        assert!((0..=5).contains(&age), "age {age}");
        assert!(stored.contains("HERON_MONITOR"));
        db.drop().await;
    }
}
