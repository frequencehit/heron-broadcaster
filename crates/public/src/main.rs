// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-public`: the public gateway of the shows. See
//! `docs/listener-interaction.md`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::http::HeaderName;
use clap::{Parser, Subcommand};
use heron_public::{Config, Gateway, SERVICE};
use heron_service::{HealthCheck, LogArgs, MetricsArgs};

#[derive(Debug, Parser)]
#[command(name = SERVICE, version, about = "Heron Broadcaster public gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// The public address: the website's requests.
    #[arg(long, env = "HERON_HTTP_ADDR", default_value = "0.0.0.0:8084")]
    http_addr: SocketAddr,

    /// `/healthz` and `/metrics`, kept off the public address.
    #[arg(
        long,
        env = "HERON_OPS_ADDR",
        default_value = "0.0.0.0:8085",
        global = true
    )]
    ops_addr: SocketAddr,

    /// Where the API is reached.
    #[arg(
        long,
        env = "HERON_PUBLIC_API_URL",
        default_value = "http://heron-api:8080"
    )]
    api_url: String,

    /// The gateway token, the same as `heron-api`'s.
    #[arg(long, env = "HERON_PUBLIC_TOKEN", hide_env_values = true)]
    token: Option<String>,

    /// The websites that may send writes, comma-separated; empty: any.
    #[arg(long, env = "HERON_PUBLIC_ALLOWED_ORIGINS", default_value = "")]
    allowed_origins: String,

    /// The header a reverse proxy puts the listener's address in.
    #[arg(long, env = "HERON_PUBLIC_CLIENT_IP_HEADER", default_value = "")]
    client_ip_header: String,

    #[arg(long, env = "HERON_PUBLIC_READS_PER_MINUTE", default_value_t = 300)]
    reads_per_minute: u32,

    #[arg(long, env = "HERON_PUBLIC_WRITES_PER_MINUTE", default_value_t = 20)]
    writes_per_minute: u32,

    #[arg(long, env = "HERON_PUBLIC_WRITES_PER_SECOND", default_value_t = 50)]
    writes_per_second: u32,

    #[arg(long, env = "HERON_PUBLIC_STREAMS_PER_ADDRESS", default_value_t = 10)]
    streams_per_address: u32,

    #[arg(long, env = "HERON_PUBLIC_MAX_STREAMS", default_value_t = 10_000)]
    max_streams: usize,

    /// Difficulty of the proof of work, 0 (off) to 24.
    #[arg(long, env = "HERON_PUBLIC_PROOF_BITS", default_value_t = 16,
          value_parser = clap::value_parser!(u8).range(0..=24))]
    proof_bits: u8,

    #[command(flatten)]
    log: LogArgs,

    #[command(flatten)]
    metrics: MetricsArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Exit 0 if the running gateway answers `/healthz` (used by Docker).
    Healthcheck,
}

/// Healthy while it runs: it serves what it last read when the API does
/// not answer, and says so.
#[derive(Clone)]
struct Health(Arc<Gateway>);

impl HealthCheck for Health {
    async fn check(&self) -> Result<(), String> {
        Ok(())
    }

    async fn warnings(&self) -> Vec<String> {
        if self.0.config.token.is_none() {
            return vec!["HERON_PUBLIC_TOKEN is not set: the shows are not served".to_owned()];
        }
        let age = self.0.state_age();
        if age > Duration::from_secs(5) {
            return vec![format!("the API has not answered for {} s", age.as_secs())];
        }
        Vec::new()
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Healthcheck) => {
            heron_service::probe::probe(cli.ops_addr, "/healthz", Duration::from_secs(3))?;
            Ok(())
        }
        None => run(cli),
    }
}

#[tokio::main]
async fn run(cli: Cli) -> anyhow::Result<()> {
    let _telemetry = heron_service::telemetry::init_tracing(&cli.log, SERVICE)?;
    let metrics = heron_service::ops::install_metrics(SERVICE, &cli.metrics)?;
    let client_ip_header = Some(cli.client_ip_header.trim())
        .filter(|name| !name.is_empty())
        .map(|name| HeaderName::try_from(name.to_ascii_lowercase()))
        .transpose()
        .context("HERON_PUBLIC_CLIENT_IP_HEADER is not a header name")?;
    let config = Config {
        api_url: cli.api_url,
        token: cli.token.filter(|token| !token.trim().is_empty()),
        allowed_origins: cli
            .allowed_origins
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_owned)
            .collect(),
        client_ip_header,
        reads_per_minute: cli.reads_per_minute,
        writes_per_minute: cli.writes_per_minute,
        writes_per_second: cli.writes_per_second,
        streams_per_address: cli.streams_per_address,
        max_streams: cli.max_streams,
        proof_bits: cli.proof_bits,
        poll: Duration::from_secs(1),
    };
    if config.token.is_none() {
        tracing::error!("HERON_PUBLIC_TOKEN is not set: the gateway answers 503 until it is");
    }
    let gateway = Gateway::new(config, heron_public::actions::ACTIONS.to_vec())?;
    let poller = tokio::spawn(Arc::clone(&gateway).run());

    let ops = heron_service::ops_router(SERVICE, metrics, Health(Arc::clone(&gateway)));
    let ops_task = tokio::spawn(heron_service::serve(cli.ops_addr, ops));

    let listener = tokio::net::TcpListener::bind(cli.http_addr)
        .await
        .with_context(|| format!("cannot listen on {}", cli.http_addr))?;
    tracing::info!(addr = %cli.http_addr, "public gateway listening");
    let app =
        heron_public::app::router(gateway).into_make_service_with_connect_info::<SocketAddr>();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(heron_service::shutdown_signal())
        .await;
    poller.abort();
    ops_task.abort();
    Ok(served?)
}
