// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-engine`: real-time playout (decks, mixer, encoders, outputs).
//!
//! It never connects to the database and must keep running when every other
//! service is down. `heron-engine ctl` is a command-line client for its gRPC
//! control interface.

mod ctl;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use heron_engine::engine::{Engine, EngineHealth, EngineOptions};
use heron_engine::grpc::EngineService;
use heron_engine::handover;
use heron_service::{LogArgs, MetricsArgs};

const SERVICE: &str = "heron-engine";
/// How long a shutdown waits for open connections to close.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Parser)]
#[command(name = SERVICE, version, about = "Heron Broadcaster playout engine")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Address of the ops HTTP server (`/healthz`, `/metrics`).
    #[arg(
        long,
        env = "HERON_HTTP_ADDR",
        default_value = "0.0.0.0:8081",
        global = true
    )]
    http_addr: SocketAddr,

    /// Address of the gRPC control interface, used by heron-api.
    #[arg(long, env = "HERON_GRPC_ADDR", default_value = "0.0.0.0:50051")]
    grpc_addr: SocketAddr,

    /// Root of the music library; queued track paths are relative to it.
    #[arg(
        long,
        env = "HERON_LIBRARY_DIR",
        default_value = "/music",
        global = true
    )]
    library_dir: PathBuf,

    /// Directory of tracks played when the queue runs dry or the API is unreachable.
    #[arg(
        long,
        env = "HERON_FALLBACK_DIR",
        default_value = "/var/lib/heron/fallback"
    )]
    fallback_dir: PathBuf,

    /// Queue entries whose file is read into memory ahead of time, so a slow
    /// or unreachable library does not interrupt the air.
    #[arg(long, env = "HERON_PRELOAD_TRACKS", default_value_t = heron_engine::player::PRELOAD_TRACKS)]
    preload_tracks: usize,

    /// Play the master bus on the local sound card (builds with the `monitor` feature).
    #[arg(long, env = "HERON_MONITOR")]
    monitor: bool,

    /// Socket of the relay (`heron-engine relay`) network outputs connect
    /// through, so an upgrade hands them over without a gap. Direct
    /// connections when unset or unreachable.
    #[arg(long, env = "HERON_RELAY_SOCKET", global = true)]
    relay_socket: Option<PathBuf>,

    /// Directory where the engine persists its state (configuration, queue) between restarts.
    #[arg(long, env = "HERON_STATE_DIR", default_value = "/var/lib/heron/engine")]
    state_dir: PathBuf,

    #[command(flatten)]
    log: LogArgs,

    #[command(flatten)]
    metrics: MetricsArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Exit 0 if the running engine answers `/healthz` (used by Docker).
    Healthcheck,
    /// Control a running engine.
    Ctl(ctl::Args),
    /// Hold the engine's connections to stream servers, so they outlive
    /// the engine's container (`docs/handover.md`). Listens on
    /// `--relay-socket`.
    Relay {
        /// Exit 0 if a relay listens (used by Docker).
        #[arg(long)]
        check: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Healthcheck) => {
            heron_service::probe::probe(cli.http_addr, "/healthz", Duration::from_secs(3))?;
            Ok(())
        }
        Some(Command::Ctl(args)) => ctl::run(args, &cli.library_dir),
        Some(Command::Relay { check }) => {
            let socket = cli
                .relay_socket
                .clone()
                .context("the relay needs --relay-socket (HERON_RELAY_SOCKET)")?;
            if check {
                std::os::unix::net::UnixStream::connect(&socket)
                    .with_context(|| format!("no relay on {}", socket.display()))?;
                return Ok(());
            }
            relay(&cli, socket)
        }
        None => run(cli),
    }
}

#[tokio::main]
async fn relay(cli: &Cli, socket: PathBuf) -> anyhow::Result<()> {
    let _telemetry = heron_service::telemetry::init_tracing(&cli.log, "heron-relay")?;
    if let Some(dir) = socket.parent().map(Path::to_path_buf) {
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&dir))
            .await?
            .context("cannot create the relay's directory")?;
    }
    tokio::select! {
        served = heron_outputs::connection::serve_relay(socket) => served.context("relay failed"),
        () = heron_service::shutdown_signal() => Ok(()),
    }
}

#[tokio::main]
async fn run(cli: Cli) -> anyhow::Result<()> {
    let _telemetry = heron_service::telemetry::init_tracing(&cli.log, SERVICE)?;
    let metrics = heron_service::ops::install_metrics(SERVICE, &cli.metrics)?;

    std::fs::create_dir_all(&cli.state_dir).with_context(|| {
        format!(
            "cannot create the state directory {}",
            cli.state_dir.display()
        )
    })?;
    // An engine running already hands over to this one (an upgrade).
    let state_dir = cli.state_dir.clone();
    let start = tokio::task::spawn_blocking(move || handover::start(&state_dir)).await?;
    let standby = start.standby;
    let engine = Engine::start(
        EngineOptions {
            fallback_dir: Some(cli.fallback_dir.clone()),
            preload_tracks: cli.preload_tracks,
            exit_when_stuck: true,
            monitor: cli.monitor,
            relay_socket: cli.relay_socket.clone(),
            standby,
            ..EngineOptions::new(cli.library_dir.clone(), cli.state_dir.clone())
        },
        tokio::runtime::Handle::current(),
    )?;
    tracing::info!(
        library_dir = %cli.library_dir.display(),
        state_dir = %cli.state_dir.display(),
        "engine ready"
    );

    let health = EngineHealth(std::sync::Arc::clone(&engine));
    let router = heron_service::ops_router(SERVICE, metrics, health);
    let http = tokio::spawn(heron_service::serve(cli.http_addr, router));

    // A standby goes on air once the active engine hands over, or stops.
    let takeover = async {
        if standby {
            let following = std::sync::Arc::clone(&engine);
            let state_dir = cli.state_dir.clone();
            tokio::task::spawn_blocking(move || {
                handover::go_on_air(&following, start.follow, &state_dir)
            })
            .await?;
        }
        anyhow::Ok(
            handover::Listener::bind(&cli.state_dir)
                .context("cannot listen for a standby engine")?,
        )
    };
    let listener = tokio::select! {
        listener = takeover => listener?,
        () = heron_service::shutdown_signal() => {
            // A standby stops without touching the state: it is not its own.
            tracing::info!("stopping before taking over");
            return Ok(());
        }
    };

    let about = about_report(&cli).await;
    let grpc = tonic::transport::Server::builder()
        .add_service(EngineService::server_with_about(
            std::sync::Arc::clone(&engine),
            about,
        ))
        .serve_with_shutdown(cli.grpc_addr, heron_service::shutdown_signal());
    tracing::info!(addr = %cli.grpc_addr, "grpc server listening");

    let servers = async {
        let (grpc, http) = tokio::join!(grpc, http);
        grpc.context("grpc server failed")?;
        http??;
        anyhow::Ok(())
    };
    tokio::pin!(servers);
    tokio::select! {
        result = &mut servers => {
            engine.save_queue();
            return result;
        }
        () = heron_service::shutdown_signal() => {}
    }
    let stopping = std::sync::Arc::clone(&engine);
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || handover::stop(&stopping, &listener, &runtime)).await?;
    // Streams (the API's WatchEvents) never end on their own: do not wait
    // for them, or Docker kills the engine after 10 s.
    match tokio::time::timeout(SHUTDOWN_GRACE, &mut servers).await {
        Ok(result) => result,
        Err(_) => {
            tracing::info!("closing the connections still open");
            Ok(())
        }
    }
}

/// What the About page shows of this engine, as JSON.
async fn about_report(cli: &Cli) -> String {
    use heron_service::about::{AboutReport, ConfigItem, ProcessInfo, common_config};

    let external =
        std::env::var_os(heron_outputs::external::FFMPEG_ENV).is_some_and(|v| !v.is_empty());
    let program = heron_outputs::external::configured_program();
    // The binary is only run when the external encoder is set up.
    let ffmpeg = if external {
        tokio::task::spawn_blocking(move || heron_outputs::external::version(&program))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let mut config = common_config(&cli.log, &cli.metrics, cli.http_addr);
    config.extend([
        ConfigItem::text("HERON_GRPC_ADDR", cli.grpc_addr),
        ConfigItem::text("HERON_LIBRARY_DIR", cli.library_dir.display()),
        ConfigItem::text("HERON_FALLBACK_DIR", cli.fallback_dir.display()),
        ConfigItem::text("HERON_STATE_DIR", cli.state_dir.display()),
        ConfigItem::text("HERON_PRELOAD_TRACKS", cli.preload_tracks),
        ConfigItem::flag("HERON_MONITOR", cli.monitor),
        ConfigItem::flag("HERON_FFMPEG", external),
        ConfigItem::flag("HERON_RELAY_SOCKET", cli.relay_socket.is_some()),
    ]);
    let report = AboutReport {
        process: ProcessInfo::read(std::time::SystemTime::now()),
        config,
        ffmpeg,
    };
    serde_json::to_string(&report).unwrap_or_default()
}
