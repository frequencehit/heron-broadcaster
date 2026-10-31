// SPDX-License-Identifier: AGPL-3.0-or-later

//! The database dump: `mariadb-dump --single-transaction`, compressed by
//! `gzip`, the station playing on.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Stdio;

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
    let (host, port) = match address.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse()
                .map_err(|_| format!("invalid port in the database URL: {port}"))?,
        ),
        None => (address, 3306),
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

/// Dumps the database to `target`, written whole or not at all.
pub async fn dump(database_url: &str, target: &Path) -> Result<(), String> {
    let database = parse(database_url)?;
    let dir = target.parent().ok_or("no backup folder")?;
    // The password goes in an option file, not on the command line.
    let options = tempfile::Builder::new()
        .prefix(".heron-dump-")
        .tempfile_in(dir)
        .map_err(|error| format!("cannot write the dump's options: {error}"))?;
    {
        let mut file = options.as_file();
        writeln!(
            file,
            "[client]\nuser={}\npassword={}\nhost={}\nport={}",
            quoted(&database.user),
            quoted(&database.password),
            quoted(&database.host),
            database.port
        )
        .map_err(|error| format!("cannot write the dump's options: {error}"))?;
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("dump");
    let temporary = target.with_file_name(format!(".{name}.tmp"));
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    let result = async {
        let mut dump = tokio::process::Command::new("mariadb-dump")
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
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("cannot run mariadb-dump: {error}"))?;
        let stdout: Stdio = dump
            .stdout
            .take()
            .ok_or("no output from mariadb-dump")?
            .try_into()
            .map_err(|error| format!("cannot read mariadb-dump: {error}"))?;
        let gzip = tokio::process::Command::new("gzip")
            .arg("-c")
            .stdin(stdout)
            .stdout(output)
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("cannot run gzip: {error}"))?;
        let (dumped, zipped) = tokio::join!(dump.wait_with_output(), gzip.wait_with_output());
        let dumped = dumped.map_err(|error| format!("mariadb-dump: {error}"))?;
        if !dumped.status.success() {
            return Err(format!(
                "mariadb-dump failed: {}",
                String::from_utf8_lossy(&dumped.stderr).trim()
            ));
        }
        let zipped = zipped.map_err(|error| format!("gzip: {error}"))?;
        if !zipped.status.success() {
            return Err(format!(
                "gzip failed: {}",
                String::from_utf8_lossy(&zipped.stderr).trim()
            ));
        }
        let file = std::fs::File::open(&temporary).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, target).map_err(|error| error.to_string())?;
        std::fs::File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_database_url() {
        assert_eq!(
            parse("mysql://heron:p%40ss%3Aword@mariadb:3307/heron?ssl-mode=disabled").unwrap(),
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
        assert!(parse("postgres://x/y").is_err());
        assert!(parse("mysql://x@host").is_err());
    }

    #[test]
    fn quotes_option_values() {
        assert_eq!(quoted(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
