// SPDX-License-Identifier: AGPL-3.0-or-later

//! Process plumbing shared by the Heron binaries: logging, metrics, the ops
//! HTTP endpoints (`/healthz`, `/metrics`), graceful shutdown and the probe
//! used by container health checks.
//!
//! Nothing in here belongs on the audio path.

pub mod about;
pub mod clock;
#[cfg(feature = "mysql")]
pub mod db;
pub mod ops;
pub mod priority;
pub mod probe;
pub mod remote_write;
pub mod telemetry;

use std::net::SocketAddr;

pub use ops::{HealthCheck, ops_router};
pub use remote_write::MetricsArgs;
pub use telemetry::{LogArgs, LogFormat};

/// Errors raised while setting up or running a service.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to initialize logging: {0}")]
    Tracing(String),
    #[error("failed to install the metrics recorder: {0}")]
    Metrics(#[from] metrics_exporter_prometheus::BuildError),
    #[error("failed to bind or serve on {addr}: {source}")]
    Serve {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("metrics remote write: {0}")]
    RemoteWrite(String),
    #[error("health probe failed: {0}")]
    Probe(String),
    #[cfg(feature = "mysql")]
    #[error("database unavailable after {attempts} attempts: {source}")]
    Database { attempts: u32, source: sqlx::Error },
}

/// Binds `addr` and serves `router` until SIGINT or SIGTERM.
pub async fn serve(addr: SocketAddr, router: axum::Router) -> Result<(), Error> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| Error::Serve { addr, source })?;
    tracing::info!(%addr, "http server listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|source| Error::Serve { addr, source })
}

/// Resolves when the process receives SIGINT (Ctrl+C) or SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "cannot listen for Ctrl+C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown requested");
}
