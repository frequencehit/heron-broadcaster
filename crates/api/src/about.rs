// SPDX-License-Identifier: AGPL-3.0-or-later

//! The About page: which build runs, on what machine and how it is set up
//! (`docs/roadmap/m17-about-page.md`).
//!
//! Everyone gets the build; the machine, the dependencies and the
//! configuration are for admins. The configuration is whatever each service
//! lists itself (`heron_service::about::ConfigItem`): a secret is only ever
//! "set" or not.

use std::time::SystemTime;

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use heron_service::about::{ConfigItem, HEARTBEAT_STALE, ProcessInfo, common_config, unix_ms};

use crate::app::{AppState, SERVICE};
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiResult, ErrorBody};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(about))
}

/// What the API knows of itself from its start.
pub struct Own {
    pub started: SystemTime,
    pub config: Vec<ConfigItem>,
}

impl Own {
    pub fn new(config: Vec<ConfigItem>) -> Self {
        Self {
            started: SystemTime::now(),
            config,
        }
    }

    /// For the API's own settings: the common ones plus `specific`.
    pub fn with_common(
        log: &heron_service::LogArgs,
        metrics: &heron_service::MetricsArgs,
        http_addr: std::net::SocketAddr,
        specific: impl IntoIterator<Item = ConfigItem>,
    ) -> Self {
        let mut config = common_config(log, metrics, http_addr);
        config.extend(specific);
        Self::new(config)
    }
}

/// A Heron service and whether it runs.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceInfo {
    /// `heron-api`, `heron-engine`, `heron-analyzer` or `heron-librarian`.
    pub service: String,
    /// The station's code for an engine, the host name for the others.
    pub instance: Option<String>,
    /// Missing when the service was never seen or does not say.
    pub version: Option<String>,
    pub revision: Option<String>,
    pub build_date: Option<String>,
    /// Answering now: the engine reachable, a worker's heartbeat recent.
    pub running: bool,
}

/// The machine a service runs on (admins).
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceHost {
    pub service: String,
    pub instance: Option<String>,
    /// How long it has been running, in seconds.
    pub uptime_seconds: Option<i64>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub arch: Option<String>,
    pub container: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Runtime {
    pub hosts: Vec<ServiceHost>,
    /// The `TZ` of the API's environment.
    pub timezone: Option<String>,
    pub clock: heron_service::clock::ClockStatus,
    /// Logical cores of the machine the API runs on.
    pub cpu_cores: u32,
    /// Its memory, in MiB.
    pub memory_mib: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Database {
    /// `SELECT VERSION()`, e.g. `12.3.1-MariaDB`.
    pub version: Option<String>,
    /// Database migrations applied.
    pub migrations: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Dependencies {
    pub database: Database,
    /// The first line of `ffmpeg -version` of an engine with the external
    /// encoder; missing when the built-in encoders are used.
    pub ffmpeg: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceConfig {
    pub service: String,
    pub instance: Option<String>,
    pub items: Vec<ConfigItem>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct About {
    pub services: Vec<ServiceInfo>,
    /// Admins only.
    pub runtime: Option<Runtime>,
    /// Admins only.
    pub dependencies: Option<Dependencies>,
    /// Admins only.
    pub configuration: Option<Vec<ServiceConfig>>,
}

/// One heartbeat row.
#[derive(sqlx::FromRow)]
struct Heartbeat {
    service: String,
    instance: String,
    version: String,
    revision: Option<String>,
    build_date: Option<String>,
    uptime_seconds: i64,
    age_seconds: i64,
    os: Option<String>,
    kernel: Option<String>,
    arch: Option<String>,
    container: bool,
    config: String,
}

async fn heartbeats(pool: &MySqlPool) -> Vec<Heartbeat> {
    sqlx::query_as(
        "SELECT service, instance, version, revision, build_date,
                CAST(TIMESTAMPDIFF(SECOND, started_at, UTC_TIMESTAMP(6)) AS SIGNED) AS uptime_seconds,
                CAST(TIMESTAMPDIFF(SECOND, seen_at, UTC_TIMESTAMP(6)) AS SIGNED) AS age_seconds,
                os, kernel, arch, container, CAST(config AS CHAR) AS config
         FROM service_heartbeats ORDER BY service, instance",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(%error, "cannot read the services' heartbeats");
        Vec::new()
    })
}

/// The build part of an engine's or the API's own report.
fn service_info(
    service: &str,
    instance: Option<String>,
    process: Option<&ProcessInfo>,
) -> ServiceInfo {
    ServiceInfo {
        service: service.to_owned(),
        instance,
        version: process.map(|process| process.version.clone()),
        revision: process.and_then(|process| process.revision.clone()),
        build_date: process.and_then(|process| process.build_date.clone()),
        running: process.is_some(),
    }
}

fn service_host(service: &str, instance: Option<String>, process: &ProcessInfo) -> ServiceHost {
    let now = unix_ms(SystemTime::now());
    ServiceHost {
        service: service.to_owned(),
        instance,
        uptime_seconds: Some(((now - process.started_unix_ms) / 1000).max(0)),
        os: process.os.clone(),
        kernel: process.kernel.clone(),
        arch: Some(process.arch.clone()),
        container: process.container,
    }
}

/// The build, and for admins the machine, the dependencies and the
/// configuration.
#[utoipa::path(get, path = "/about", tag = "system",
    responses((status = 200, body = About), (status = "4XX", body = ErrorBody)))]
async fn about(user: CurrentUser, State(state): State<AppState>) -> ApiResult<Json<About>> {
    let admin = user.role >= Role::Admin;

    let own_process = ProcessInfo::read(state.about.started);
    let mut services = vec![service_info(SERVICE, None, Some(&own_process))];

    // The engines, all at once: each answers or times out on its own.
    let mut asking = tokio::task::JoinSet::new();
    for station in state.stations.all() {
        asking.spawn(async move {
            let report = station.engine.fetch_about().await;
            let reachable = station.engine.is_connected();
            (station.code(), reachable, report)
        });
    }
    let mut engines = Vec::new();
    while let Some(Ok(engine)) = asking.join_next().await {
        engines.push(engine);
    }
    engines.sort_by(|a, b| a.0.cmp(&b.0));

    let beats = heartbeats(&state.pool).await;
    let now_running = |beat: &Heartbeat| {
        u64::try_from(beat.age_seconds).is_ok_and(|age| age <= HEARTBEAT_STALE.as_secs())
    };

    let mut hosts = vec![service_host(SERVICE, None, &own_process)];
    let mut configuration = vec![ServiceConfig {
        service: SERVICE.to_owned(),
        instance: None,
        items: state.about.config.clone(),
    }];
    let mut ffmpeg = None;

    for (code, reachable, report) in engines {
        let mut info = service_info(
            "heron-engine",
            Some(code.clone()),
            report.as_ref().map(|r| &r.process),
        );
        // An engine too old to report still answers: its version is unknown.
        info.running = reachable;
        services.push(info);
        if let Some(report) = report {
            hosts.push(service_host(
                "heron-engine",
                Some(code.clone()),
                &report.process,
            ));
            ffmpeg = ffmpeg.or(report.ffmpeg);
            configuration.push(ServiceConfig {
                service: "heron-engine".to_owned(),
                instance: Some(code),
                items: report.config,
            });
        }
    }

    for worker in ["heron-analyzer", "heron-librarian"] {
        let mut seen = false;
        for beat in beats.iter().filter(|beat| beat.service == worker) {
            seen = true;
            services.push(ServiceInfo {
                service: beat.service.clone(),
                instance: Some(beat.instance.clone()),
                version: Some(beat.version.clone()),
                revision: beat.revision.clone(),
                build_date: beat.build_date.clone(),
                running: now_running(beat),
            });
            hosts.push(ServiceHost {
                service: beat.service.clone(),
                instance: Some(beat.instance.clone()),
                uptime_seconds: now_running(beat).then_some(beat.uptime_seconds.max(0)),
                os: beat.os.clone(),
                kernel: beat.kernel.clone(),
                arch: beat.arch.clone(),
                container: beat.container,
            });
            configuration.push(ServiceConfig {
                service: beat.service.clone(),
                instance: Some(beat.instance.clone()),
                items: serde_json::from_str(&beat.config).unwrap_or_default(),
            });
        }
        if !seen {
            services.push(service_info(worker, None, None));
        }
    }

    if !admin {
        return Ok(Json(About {
            services,
            runtime: None,
            dependencies: None,
            configuration: None,
        }));
    }

    let memory_mib = sysinfo::System::new().map_or(0, |mut system| {
        system.refresh_memory();
        system.total_memory() / (1024 * 1024)
    });
    let runtime = Runtime {
        hosts,
        timezone: std::env::var("TZ").ok().filter(|tz| !tz.trim().is_empty()),
        clock: heron_service::clock::status(),
        cpu_cores: std::thread::available_parallelism()
            .map_or(1, |cores| u32::try_from(cores.get()).unwrap_or(u32::MAX)),
        memory_mib,
    };
    let database = Database {
        version: sqlx::query_scalar("SELECT VERSION()")
            .fetch_one(&state.pool)
            .await
            .ok(),
        migrations: sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&state.pool)
            .await
            .ok(),
    };
    Ok(Json(About {
        services,
        runtime: Some(runtime),
        dependencies: Some(Dependencies { database, ffmpeg }),
        configuration: Some(configuration),
    }))
}
