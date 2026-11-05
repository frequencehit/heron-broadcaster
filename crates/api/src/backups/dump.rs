// SPDX-License-Identifier: AGPL-3.0-or-later

//! The database dump: `mariadb-dump --single-transaction`, the station
//! playing on, compressed (gzip) and, with a key set, encrypted.

use std::io::{BufWriter, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use age::x25519::Recipient;
use flate2::Compression;
use flate2::write::GzEncoder;

use super::check::{Checksum, HashingWriter, copy_dump};
use super::crypto;

/// The parts of `HERON_DATABASE_URL` (`mysql://user:password@host:port/db`).
#[derive(Debug, PartialEq, Eq)]
struct Target {
    user: String,
    password: String,
    host: String,
    port: u16,
    database: String,
}

/// `%XX` escapes decoded.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = text
                .get(index + 1..index + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse(url: &str) -> Result<Target, String> {
    let rest = url
        .strip_prefix("mysql://")
        .or_else(|| url.strip_prefix("mariadb://"))
        .ok_or("the database URL is not mysql://…")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (credentials, location) = rest.rsplit_once('@').unwrap_or(("", rest));
    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
    let (address, database) = location
        .split_once('/')
        .ok_or("the database URL names no database")?;
    let port = |port: &str| {
        port.parse::<u16>()
            .map_err(|_| format!("invalid port in the database URL: {port}"))
    };
    // An IPv6 address is in brackets, its port after them.
    let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
        let (host, after) = bracketed
            .split_once(']')
            .ok_or("invalid IPv6 address in the database URL")?;
        match after.strip_prefix(':') {
            Some(after) => (host, port(after)?),
            None if after.is_empty() => (host, 3306),
            None => return Err(format!("invalid port in the database URL: {after}")),
        }
    } else {
        match address.rsplit_once(':') {
            Some((host, after)) => (host, port(after)?),
            None => (address, 3306),
        }
    };
    if database.is_empty() {
        return Err("the database URL names no database".to_owned());
    }
    Ok(Target {
        user: decode(user),
        password: decode(password),
        host: host.to_owned(),
        port,
        database: decode(database),
    })
}

/// An option file quoting `value`, for `[client]`.
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Dumps the database to `target`, compressed, encrypted for `recipient`
/// when there is one, written whole or not at all: the checksum of what
/// was written.
pub async fn dump(
    database_url: &str,
    target: &Path,
    recipient: Option<Recipient>,
) -> Result<Checksum, String> {
    let database = parse(database_url)?;
    let target = target.to_owned();
    tokio::task::spawn_blocking(move || dump_blocking(&database, &target, recipient.as_ref()))
        .await
        .map_err(|error| format!("the dump stopped: {error}"))?
}

fn dump_blocking(
    database: &Target,
    target: &Path,
    recipient: Option<&Recipient>,
) -> Result<Checksum, String> {
    let dir = target.parent().ok_or("no backup folder")?;
    // The password goes in an option file, not on the command line. TCP,
    // as SQLx: for `localhost`, the client would take a socket instead,
    // and leave the port out.
    let options = tempfile::Builder::new()
        .prefix(".heron-dump-")
        .tempfile_in(dir)
        .map_err(|error| format!("cannot write the dump's options: {error}"))?;
    {
        let mut file = options.as_file();
        writeln!(
            file,
            "[client]\nuser={}\npassword={}\nhost={}\nport={}\nprotocol=TCP",
            quoted(&database.user),
            quoted(&database.password),
            quoted(&database.host),
            database.port
        )
        .map_err(|error| format!("cannot write the dump's options: {error}"))?;
    }
    let name = target
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("dump");
    let temporary = target.with_file_name(format!(".{name}.tmp"));
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    let result = (|| {
        let mut child = Command::new("mariadb-dump")
            .arg(format!(
                "--defaults-extra-file={}",
                options.path().display()
            ))
            .args([
                "--single-transaction",
                "--quick",
                "--routines",
                "--triggers",
                "--no-tablespaces",
                "--hex-blob",
                "--default-character-set=utf8mb4",
            ])
            .arg(&database.database)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run mariadb-dump: {error}"))?;
        // Read aside: a full pipe of errors would stop the dump.
        let stderr = child.stderr.take();
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut stderr) = stderr {
                let _ = stderr.read_to_string(&mut text);
            }
            text
        });
        let written = match child.stdout.take() {
            Some(mut stdout) => write(&mut stdout, output, recipient),
            None => Err(std::io::Error::other("no output from mariadb-dump")),
        };
        if written.is_err() {
            let _ = child.kill();
        }
        let status = child
            .wait()
            .map_err(|error| format!("mariadb-dump: {error}"))?;
        let errors = errors.join().unwrap_or_default();
        if !status.success() {
            return Err(format!("mariadb-dump failed: {}", errors.trim()));
        }
        let (file, checksum, whole) =
            written.map_err(|error| format!("cannot write the dump: {error}"))?;
        if !whole {
            return Err(
                "the dump is cut short: mariadb-dump stopped before its end (no \"-- Dump completed\")"
                    .to_owned(),
            );
        }
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, target).map_err(|error| error.to_string())?;
        std::fs::File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(checksum)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Compresses the dump read from `input` into `output`, encrypted for
/// `recipient`: the file, the checksum of what was written, and whether
/// the dump is whole.
fn write(
    input: &mut impl Read,
    output: std::fs::File,
    recipient: Option<&Recipient>,
) -> std::io::Result<(std::fs::File, Checksum, bool)> {
    let hashing = HashingWriter::new(BufWriter::new(output));
    let (hashing, whole) = match recipient {
        Some(recipient) => {
            let encrypted = crypto::encrypt_to(recipient, hashing)?;
            let mut compressed = GzEncoder::new(encrypted, Compression::default());
            let whole = copy_dump(input, &mut compressed)?;
            (compressed.finish()?.finish()?, whole)
        }
        None => {
            let mut compressed = GzEncoder::new(hashing, Compression::default());
            let whole = copy_dump(input, &mut compressed)?;
            (compressed.finish()?, whole)
        }
    };
    let (buffered, checksum) = hashing.finish();
    let file = buffered.into_inner().map_err(|error| error.into_error())?;
    Ok((file, checksum, whole))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_database_url() {
        assert_eq!(
            parse(&format!(
                "mysql://heron:{}@mariadb:3307/heron?ssl-mode=disabled",
                "p%40ss%3Aword"
            ))
            .unwrap(),
            Target {
                user: "heron".to_owned(),
                password: "p@ss:word".to_owned(),
                host: "mariadb".to_owned(),
                port: 3307,
                database: "heron".to_owned(),
            }
        );
        let target = parse("mysql://root@127.0.0.1/heron").unwrap();
        assert_eq!((target.port, target.password.as_str()), (3306, ""));
        let target = parse("mysql://heron:pass@[::1]/heron").unwrap();
        assert_eq!((target.host.as_str(), target.port), ("::1", 3306));
        let target = parse("mysql://heron:pass@[2001:db8::5]:3307/heron").unwrap();
        assert_eq!((target.host.as_str(), target.port), ("2001:db8::5", 3307));
        assert!(parse("mysql://heron@[::1/heron").is_err());
        assert!(parse("mysql://heron@[::1]x/heron").is_err());
        assert!(parse("postgres://x/y").is_err());
        assert!(parse("mysql://x@host").is_err());
    }

    #[test]
    fn quotes_option_values() {
        assert_eq!(quoted(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
