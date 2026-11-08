// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sending a file to a certification body's server: SFTP (a password or a
//! private key, the host key trusted on first use) or FTPS (explicit TLS,
//! the certificate checked with the system's authorities).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use suppaftp::tokio_rustls::TlsConnector;
use suppaftp::tokio_rustls::rustls::ClientConfig;
use suppaftp::types::FileType;
use suppaftp::{AsyncRustlsConnector, AsyncRustlsFtpStream};
use tokio::io::AsyncWriteExt;

/// The longest a connection, a login and a transfer take together.
const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Sftp,
    Ftps,
}

/// Where a file goes, and how to log in.
#[derive(Clone, Debug)]
pub struct Target {
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    /// SFTP: an OpenSSH private key.
    pub private_key: Option<String>,
    /// SFTP: the host key's fingerprint expected; `None` trusts the first.
    pub host_key: Option<String>,
    /// Empty: the login's folder.
    pub remote_dir: String,
}

impl Target {
    /// The path of a file in the folder.
    pub fn path(&self, name: &str) -> String {
        let dir = self.remote_dir.trim_end_matches('/');
        if dir.is_empty() {
            if self.remote_dir.starts_with('/') {
                format!("/{name}")
            } else {
                name.to_owned()
            }
        } else {
            format!("{dir}/{name}")
        }
    }
}

/// A connection that logged in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connected {
    /// SFTP: the host key's fingerprint, `SHA256:…`.
    pub host_key: Option<String>,
}

/// Sends files; holds the TLS settings of FTPS.
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
    pub async fn test(&self, target: &Target) -> Result<Connected, String> {
        self.within(self.go(target, None)).await
    }

    /// Writes `data` to the file `name` of the folder, replacing it.
    pub async fn upload(
        &self,
        target: &Target,
        name: &str,
        data: &[u8],
    ) -> Result<Connected, String> {
        self.within(self.go(target, Some((name, data)))).await
    }

    async fn within(
        &self,
        work: impl Future<Output = Result<Connected, String>>,
    ) -> Result<Connected, String> {
        tokio::time::timeout(self.timeout, work)
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "no answer within {} seconds",
                    self.timeout.as_secs()
                ))
            })
    }

    async fn go(&self, target: &Target, file: Option<(&str, &[u8])>) -> Result<Connected, String> {
        match target.protocol {
            Protocol::Sftp => sftp(target, file).await,
            Protocol::Ftps => self.ftps(target, file).await,
        }
    }

    async fn ftps(&self, target: &Target, file: Option<(&str, &[u8])>) -> Result<Connected, String> {
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
            ftp.cwd(&target.remote_dir)
                .await
                .map_err(|error| format!("cannot open the folder {}: {error}", target.remote_dir))?;
        }
        if let Some((name, data)) = file {
            let mut reader = data;
            ftp.put_file(name, &mut reader)
                .await
                .map_err(|error| format!("cannot write {name}: {error}"))?;
        }
        let _ = ftp.quit().await;
        Ok(Connected { host_key: None })
    }
}

/// The SSH side of an SFTP connection: checks the host key.
struct Ssh {
    expected: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Ssh {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256),
            PublicKeyOrCertificate::Certificate(certificate) => {
                certificate.public_key().fingerprint(HashAlg::Sha256)
            }
        }
        .to_string();
        let accepted = self
            .expected
            .as_deref()
            .is_none_or(|expected| expected == fingerprint);
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some(fingerprint);
        }
        Ok(accepted)
    }
}

async fn sftp(target: &Target, file: Option<(&str, &[u8])>) -> Result<Connected, String> {
    let at = format!("{}:{}", target.host, target.port);
    let seen = Arc::new(Mutex::new(None));
    let handler = Ssh {
        expected: target.host_key.clone(),
        seen: Arc::clone(&seen),
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    });
    let seen_key = || seen.lock().ok().and_then(|seen| seen.clone());
    let mut session = match client::connect(config, (target.host.as_str(), target.port), handler)
        .await
    {
        Ok(session) => session,
        Err(russh::Error::UnknownKey) => {
            return Err(format!(
                "the host key of {at} changed: {} was kept, the server has {}",
                target.host_key.as_deref().unwrap_or("?"),
                seen_key().as_deref().unwrap_or("?")
            ));
        }
        Err(error) => return Err(format!("cannot connect to {at}: {error}")),
    };
    let mut logged_in = false;
    if let Some(key) = target.private_key.as_deref().filter(|key| !key.is_empty()) {
        let key = russh::keys::decode_secret_key(key, None)
            .map_err(|error| format!("the private key cannot be read: {error}"))?;
        let hash = session
            .best_supported_rsa_hash()
            .await
            .map_err(|error| error.to_string())?
            .flatten();
        logged_in = session
            .authenticate_publickey(
                target.username.as_str(),
                PrivateKeyWithHashAlg::new(Arc::new(key), hash),
            )
            .await
            .map_err(|error| format!("login failed: {error}"))?
            .success();
    }
    if !logged_in && let Some(password) = target.password.as_deref() {
        logged_in = session
            .authenticate_password(target.username.as_str(), password)
            .await
            .map_err(|error| format!("login failed: {error}"))?
            .success();
    }
    if !logged_in {
        return Err(format!("login refused for {}", target.username));
    }
    let channel = session
        .channel_open_session()
        .await
        .map_err(|error| error.to_string())?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| error.to_string())?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|error| format!("no SFTP on {at}: {error}"))?;
    let dir = if target.remote_dir.is_empty() {
        "."
    } else {
        target.remote_dir.as_str()
    };
    let is_dir = sftp
        .metadata(dir)
        .await
        .map(|meta| meta.is_dir())
        .map_err(|error| format!("cannot open the folder {dir}: {error}"))?;
    if !is_dir {
        return Err(format!("{dir} is not a folder"));
    }
    if let Some((name, data)) = file {
        let path = target.path(name);
        let mut remote = sftp
            .create(path.as_str())
            .await
            .map_err(|error| format!("cannot write {path}: {error}"))?;
        remote
            .write_all(data)
            .await
            .map_err(|error| format!("cannot write {path}: {error}"))?;
        remote
            .shutdown()
            .await
            .map_err(|error| format!("cannot write {path}: {error}"))?;
    }
    let _ = sftp.close().await;
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;
    Ok(Connected {
        host_key: seen_key(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(dir: &str) -> Target {
        Target {
            protocol: Protocol::Sftp,
            host: "sftp.example.org".to_owned(),
            port: 22,
            username: "station".to_owned(),
            password: None,
            private_key: None,
            host_key: None,
            remote_dir: dir.to_owned(),
        }
    }

    #[test]
    fn files_go_in_the_folder() {
        assert_eq!(target("").path("a.log"), "a.log");
        assert_eq!(target("/").path("a.log"), "/a.log");
        assert_eq!(target("/incoming/").path("a.log"), "/incoming/a.log");
        assert_eq!(target("in").path("a.log"), "in/a.log");
    }
}
