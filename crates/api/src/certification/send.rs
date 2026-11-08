// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sending a file to a certification body's server by FTPS: explicit TLS,
//! the certificate checked with the system's authorities. SFTP comes next
//! (docs/certification.md#limitations).

use std::sync::Arc;
use std::time::Duration;

use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsFtpStream};
use suppaftp::tokio_rustls::TlsConnector;
use suppaftp::tokio_rustls::rustls::ClientConfig;
use suppaftp::types::FileType;

/// The longest a connection, a login and a transfer take together.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Where a file goes, and how to log in.
#[derive(Clone, Debug)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    /// Empty: the login's folder.
    pub remote_dir: String,
}

/// Sends files; holds the TLS settings.
#[derive(Clone)]
pub struct Sender {
    tls: Arc<ClientConfig>,
    timeout: Duration,
}

impl Sender {
    /// Checks FTPS certificates with the system's authorities.
    pub fn new() -> Result<Self, String> {
        use rustls_platform_verifier::ConfigVerifierExt;
        let tls = ClientConfig::with_platform_verifier().map_err(|error| error.to_string())?;
        Ok(Self::with_tls(tls))
    }

    /// With TLS settings of its own (a test's authority).
    pub fn with_tls(tls: ClientConfig) -> Self {
        Self {
            tls: Arc::new(tls),
            timeout: TIMEOUT,
        }
    }

    /// Connects, logs in and opens the folder; writes nothing.
    pub async fn test(&self, target: &Target) -> Result<(), String> {
        self.within(self.ftps(target, None)).await
    }

    /// Writes `data` to the file `name` of the folder, replacing it.
    pub async fn upload(&self, target: &Target, name: &str, data: &[u8]) -> Result<(), String> {
        self.within(self.ftps(target, Some((name, data)))).await
    }

    async fn within(&self, work: impl Future<Output = Result<(), String>>) -> Result<(), String> {
        tokio::time::timeout(self.timeout, work)
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "no answer within {} seconds",
                    self.timeout.as_secs()
                ))
            })
    }

    async fn ftps(&self, target: &Target, file: Option<(&str, &[u8])>) -> Result<(), String> {
        let at = format!("{}:{}", target.host, target.port);
        let ftp = AsyncRustlsFtpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|error| format!("cannot connect to {at}: {error}"))?;
        let connector = AsyncRustlsConnector::from(TlsConnector::from(Arc::clone(&self.tls)));
        let mut ftp = ftp
            .into_secure(connector, &target.host)
            .await
            .map_err(|error| format!("TLS refused by {at}: {error}"))?;
        // A server behind NAT may give its private address for passive
        // transfers: the control connection's address is used instead.
        ftp.set_passive_nat_workaround(true);
        ftp.login(
            target.username.as_str(),
            target.password.as_deref().unwrap_or(""),
        )
        .await
        .map_err(|error| format!("login refused: {error}"))?;
        ftp.transfer_type(FileType::Binary)
            .await
            .map_err(|error| error.to_string())?;
        if !target.remote_dir.is_empty() {
            ftp.cwd(&target.remote_dir).await.map_err(|error| {
                format!("cannot open the folder {}: {error}", target.remote_dir)
            })?;
        }
        if let Some((name, data)) = file {
            let mut reader = data;
            ftp.put_file(name, &mut reader)
                .await
                .map_err(|error| format!("cannot write {name}: {error}"))?;
        }
        let _ = ftp.quit().await;
        Ok(())
    }
}
