// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-analyzer`: track analysis worker pool.
//!
//! Workers claim jobs from the `analysis_jobs` queue filled by the scanner in
//! `heron-api`, decode and analyze the file, and store the result. Several
//! replicas can run side by side. It never applies migrations: `heron-api`
//! owns the schema.

mod worker;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use heron_service::about::ConfigItem;
use heron_service::db::{self, DatabaseHealth};
use heron_service::{LogArgs, MetricsArgs};
use worker::WorkerConfig;

const SERVICE: &str = "heron-analyzer";

#[derive(Debug, Parser)]
#[command(name = SERVICE, version, about = "Heron Broadcaster analysis worker")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Address of the ops HTTP server (`/healthz`, `/metrics`).
    #[arg(
        long,
        env = "HERON_HTTP_ADDR",
        default_value = "0.0.0.0:8082",
        global = true
    )]
    http_addr: SocketAddr,

    /// MariaDB URL, e.g. `mysql://heron:secret@mariadb:3306/heron`.
    #[arg(long, env = "HERON_DATABASE_URL", hide_env_values = true)]
    database_url: Option<String>,

    /// Root of the music library, as mounted in this container.
    #[arg(long, env = "HERON_LIBRARY_DIR", default_value = "/music")]
    library_dir: PathBuf,

    /// Number of tracks analyzed in parallel. Defaults to the number of CPUs
    /// minus one, so the engine keeps a core on small machines (Raspberry Pi).
    #[arg(long, env = "HERON_ANALYZER_WORKERS")]
    workers: Option<usize>,

    /// Name of this instance in the job queue. Defaults to the host name,
    /// which Docker sets to the container ID.
    #[arg(long, env = "HERON_ANALYZER_ID")]
    instance: Option<String>,

    /// Niceness, 0 to 19 (0 leaves it as is). The workers yield the CPU and
    /// the disk to the engine; they also get the lowest best-effort I/O priority.
    #[arg(long, env = "HERON_NICE", default_value = "10", global = true)]
    nice: String,

    #[command(flatten)]
    log: LogArgs,

    #[command(flatten)]
    metrics: MetricsArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Exit 0 if the running worker answers `/healthz` (used by Docker).
    Healthcheck,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Healthcheck) => {
            heron_service::probe::probe(cli.http_addr, "/healthz", Duration::from_secs(3))?;
            Ok(())
        }
        None => {
            // Before the runtime starts: its threads inherit the priority.
            let lowered = heron_service::priority::lower_current_thread_to(&cli.nice);
            run(cli, lowered)
        }
    }
}

#[tokio::main]
async fn run(cli: Cli, lowered: Result<(), String>) -> anyhow::Result<()> {
    let _telemetry = heron_service::telemetry::init_tracing(&cli.log, SERVICE)?;
    match lowered {
        Ok(()) => {
            tracing::info!(nice = ?heron_service::priority::current_nice(), "priority lowered")
        }
        Err(problem) => {
            tracing::warn!(nice = ?heron_service::priority::current_nice(), "priority: {problem}")
        }
    }
    let metrics = heron_service::ops::install_metrics(SERVICE, &cli.metrics)?;
    let database_url = cli
        .database_url
        .context("HERON_DATABASE_URL (or --database-url) is required")?;

    let workers = cli
        .workers
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(|cpus| usize::from(cpus).saturating_sub(1))
        })
        .unwrap_or(1)
        .max(1);
    let instance = cli
        .instance
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| SERVICE.to_owned());

    let options = db::ConnectOptions {
        max_connections: u32::try_from(workers + 2).unwrap_or(u32::MAX),
        ..db::ConnectOptions::default()
    };
    let pool = db::connect(&database_url, options).await?;
    let pool_for_workers = pool.clone();
    let pool_for_release = pool.clone();

    let library_dir = cli.library_dir.clone();
    let config = WorkerConfig::new(cli.library_dir, instance.clone());
    // Jobs left running by a previous run of this instance (crash, kill).
    let released = heron_analysis::jobs::release(&pool, &instance).await?;
    if released > 0 {
        tracing::info!(released, "released jobs from a previous run");
    }
    tracing::info!(workers, %instance, "analysis workers starting");

    let (stop, stopped) = tokio::sync::watch::channel(false);
    // Says on the About page that the analyzer runs, and how.
    let mut about = heron_service::about::common_config(&cli.log, &cli.metrics, cli.http_addr);
    about.extend([
        ConfigItem::text("HERON_LIBRARY_DIR", library_dir.display()),
        ConfigItem::text("HERON_ANALYZER_WORKERS", workers),
        ConfigItem::text("HERON_ANALYZER_ID", &instance),
        ConfigItem::text("HERON_NICE", &cli.nice),
    ]);
    tokio::spawn(heron_service::about::run_heartbeat(
        pool.clone(),
        SERVICE,
        instance.clone(),
        about,
        stopped.clone(),
    ));
    let pool_handle = worker::spawn(pool_for_workers, config, workers, stopped);

    let router = heron_service::ops_router(SERVICE, metrics, DatabaseHealth(pool.clone()));
    let served = heron_service::serve(cli.http_addr, router).await;

    // Stop claiming, give running analyses a moment, then put back what is left.
    let _ = stop.send(true);
    if tokio::time::timeout(Duration::from_secs(5), pool_handle)
        .await
        .is_err()
    {
        tracing::info!("analyses still running at shutdown, releasing their jobs");
    }
    heron_analysis::jobs::release(&pool_for_release, &instance).await?;
    pool.close().await;
    Ok(served?)
}
