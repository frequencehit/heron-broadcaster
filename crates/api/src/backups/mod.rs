// SPDX-License-Identifier: AGPL-3.0-or-later

//! Backups (docs/backups.md): every day, and on request, the configuration
//! file (everything set by hand) and a dump of the database, in the backup
//! folder, encrypted when a key is set and checked as they are written;
//! restoring a configuration file; testing backups.

mod check;
mod crypto;
mod dump;
pub mod file;
mod key;

use std::collections::BTreeMap;
use std::io::{BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::StationSettings;
use crate::stations::Stations;
use check::Checksum;
pub use file::{Kind, RestoreReport};
pub use key::{Encryption, KeyState, TestRecord, Tests};

/// A backup key, `AGE-SECRET-KEY-1…`, alone or in its file.
pub fn parse_key(text: &str) -> ApiResult<age::x25519::Identity> {
    crypto::parse_identity(text)
}

/// A new key: the secret key and its public key.
#[cfg(test)]
pub fn generate_key() -> (String, String) {
    crypto::generate()
}

/// Decrypts a backup's file.
#[cfg(test)]
pub fn decrypt_with(key: &str, bytes: &[u8]) -> Vec<u8> {
    crypto::decrypt(&crypto::parse_identity(key).unwrap(), bytes).unwrap()
}

/// How many backups taken before a restore are kept.
const KEEP_BEFORE_RESTORE: usize = 7;
/// The largest configuration file uploaded.
const MAX_UPLOAD: usize = 512 * 1024 * 1024;
/// Locked by a backup or restore, in the backup folder: one at a time
/// across processes, the API and `heron-api backup`.
const LOCK_FILE: &str = ".heron-backup.lock";
/// The header an uploaded backup's key comes in.
const KEY_HEADER: &str = "x-heron-backup-key";
/// How often every backup is checked against its checksums.
const VERIFY_EVERY: Duration = Duration::from_secs(24 * 3600);

/// The `backups` settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct BackupSettings {
    /// The daily backup.
    pub enabled: bool,
    /// `HH:MM`, in the first station's time zone.
    pub time: String,
    /// Daily backups kept: 1 to 90.
    pub keep_days: u32,
    /// A database dump with each backup.
    pub database: bool,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            time: "03:30".to_owned(),
            keep_days: 7,
            database: true,
        }
    }
}

impl BackupSettings {
    pub const SETTINGS_NAME: &str = "backups";

    fn at(&self) -> Option<(i8, i8)> {
        let (hour, minute) = self.time.split_once(':')?;
        let hour: i8 = hour.parse().ok()?;
        let minute: i8 = minute.parse().ok()?;
        ((0..24).contains(&hour) && (0..60).contains(&minute) && self.time.len() == 5)
            .then_some((hour, minute))
    }

    pub fn check(&self) -> ApiResult<()> {
        if self.at().is_none() {
            return Err(ApiError::Invalid(
                "time is HH:MM, 00:00 to 23:59".to_owned(),
            ));
        }
        if !(1..=90).contains(&self.keep_days) {
            return Err(ApiError::Invalid("keep_days is 1 to 90".to_owned()));
        }
        Ok(())
    }
}

/// A backup in the folder.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BackupView {
    /// Its time and kind: `2026-11-03T03-30-00Z-daily`.
    pub name: String,
    pub kind: Kind,
    pub created_at_unix_ms: i64,
    /// Bytes.
    pub config_size: u64,
    /// Bytes; missing without a dump.
    pub database_size: Option<u64>,
    /// Why the dump failed.
    pub database_error: Option<String>,
    /// Its files are encrypted.
    pub encrypted: bool,
    /// Which key opens it, when it is encrypted.
    pub key: Option<BackupKey>,
    /// The public key of that key, `age1…`, when it is known: the key typed
    /// for it must match.
    pub recipient: Option<String>,
    /// What fails its checksums, at the last check: a file missing, cut
    /// short or changed.
    pub damaged: Option<String>,
}

/// Which key opens an encrypted backup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupKey {
    /// The key backups are encrypted with now.
    Current,
    /// A key used before.
    Earlier,
    /// Not known: an encrypted file uploaded.
    Unknown,
}

/// What keeps backups from being written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct FolderProblem {
    /// The system's error (`Permission denied`, `No space left on device`…).
    pub error: String,
    /// The folder is not the API's user's: `chown` fixes it.
    pub permission_denied: bool,
    /// The user and group the API runs as: the backup folder must be
    /// theirs.
    pub uid: u32,
    pub gid: u32,
}

/// The backup folder, and what backs up the database.
pub struct Backups {
    dir: PathBuf,
    database_url: String,
    /// One backup or restore at a time in this process; `LOCK_FILE` for
    /// the others.
    lock: tokio::sync::Mutex<()>,
    /// Whether the folder could be written, when last checked.
    problem: std::sync::Mutex<Option<FolderProblem>>,
    /// The backups that failed their checksums, at the last check, with
    /// what failed; when that check was.
    damaged: std::sync::Mutex<(BTreeMap<String, String>, Option<Instant>)>,
    /// The backup being tested.
    testing: std::sync::Mutex<Option<String>>,
}

/// An error writing to the backup folder, saying how to fix it.
fn folder_error(dir: &Path, error: &std::io::Error) -> ApiError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        let (uid, gid) = ids();
        ApiError::Invalid(format!(
            "cannot write in the backup folder ({}): it is not the API's user's ({uid}:{gid}). \
             On the host, give it to that user: chown {uid}:{gid} on HERON_BACKUP_FOLDER \
             (./data/backups by default)",
            dir.display()
        ))
    } else {
        ApiError::Invalid(format!(
            "cannot write in the backup folder ({}): {error}",
            dir.display()
        ))
    }
}

fn ids() -> (u32, u32) {
    (
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw(),
    )
}

fn age(encrypted: bool) -> &'static str {
    if encrypted { ".age" } else { "" }
}

fn config_file(name: &str, encrypted: bool) -> String {
    format!("heron-config-{name}.json{}", age(encrypted))
}

fn database_file(name: &str, encrypted: bool) -> String {
    format!("heron-database-{name}.sql.gz{}", age(encrypted))
}

fn failed_file(name: &str) -> String {
    format!("heron-database-{name}.failed")
}

fn checksums_file(name: &str) -> String {
    format!("heron-checksums-{name}.sha256")
}

/// Every file a backup may have.
fn files_of(name: &str) -> [String; 6] {
    [
        config_file(name, false),
        config_file(name, true),
        database_file(name, false),
        database_file(name, true),
        failed_file(name),
        checksums_file(name),
    ]
}

/// A backup's name, from one of its files that says it is there.
fn name_of(file: &str) -> Option<&str> {
    let name = None
        .or_else(|| file.strip_prefix("heron-config-")?.strip_suffix(".json"))
        .or_else(|| {
            file.strip_prefix("heron-config-")?
                .strip_suffix(".json.age")
        })
        .or_else(|| {
            file.strip_prefix("heron-checksums-")?
                .strip_suffix(".sha256")
        })?;
    parse_name(name).map(|_| name)
}

/// A backup's files, as they are in the folder.
struct Files {
    /// The configuration file, and whether it is encrypted.
    config: Option<(String, bool)>,
    database: Option<(String, bool)>,
}

impl Files {
    fn of(dir: &Path, name: &str) -> Self {
        let find = |names: [String; 2]| {
            names
                .into_iter()
                .zip([false, true])
                .find(|(file, _)| dir.join(file).exists())
        };
        Self {
            config: find([config_file(name, false), config_file(name, true)]),
            database: find([database_file(name, false), database_file(name, true)]),
        }
    }

    fn encrypted(&self) -> bool {
        self.config
            .iter()
            .chain(&self.database)
            .any(|(_, encrypted)| *encrypted)
    }
}

/// Whether `name` is a backup's name (`<time>-<kind>`): nothing else is
/// read or deleted.
fn parse_name(name: &str) -> Option<(Timestamp, Kind)> {
    // 2026-11-03T03-30-00Z
    let (time, kind) = (name.get(..20)?, name.get(21..)?);
    if name.as_bytes().get(20) != Some(&b'-') {
        return None;
    }
    let kind = Kind::parse(kind)?;
    let time = jiff::fmt::strtime::parse("%Y-%m-%dT%H-%M-%SZ", time)
        .ok()?
        .to_datetime()
        .ok()?
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()?
        .timestamp();
    Some((time, kind))
}

/// Writes `bytes` to `path` whole, readable by this user only.
fn write_atomically(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("backup");
    let temporary = path.with_file_name(format!(".{name}.tmp"));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        write(&mut file)?;
        file.flush()?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        if let Some(dir) = path.parent() {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn sha256(bytes: &[u8]) -> Checksum {
    let mut writer = check::HashingWriter::new(std::io::sink());
    let _ = writer.write_all(bytes);
    writer.finish().1
}

/// The checksums a backup's checksums file lists, by file; none without
/// one (a backup from before M15). Only the backup's own files are read.
fn checksums_of(dir: &Path, name: &str) -> Option<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(dir.join(checksums_file(name))).ok()?;
    let own = files_of(name);
    Some(
        check::parse_checksums(&text)
            .into_iter()
            .filter(|(file, _)| own.contains(file))
            .collect(),
    )
}

/// What fails a backup's checksums: a file missing, cut short or changed.
fn verify(dir: &Path, name: &str) -> Option<String> {
    let checksums = checksums_of(dir, name)?;
    let problems: Vec<String> = checksums
        .iter()
        .filter_map(|(file, expected)| match check::hash_file(&dir.join(file)) {
            Ok(checksum) if check::hex(&checksum) == *expected => None,
            Ok(_) => Some(format!("{file}: changed or damaged (its checksum differs)")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Some(format!("{file}: missing"))
            }
            Err(error) => Some(format!("{file}: cannot be read ({error})")),
        })
        .collect();
    (!problems.is_empty()).then(|| problems.join("; "))
}

/// Encrypts a backup's files in clear for `recipient`: each written
/// encrypted and synced, the checksums written again, and only then the
/// files in clear deleted.
fn encrypt_backup(
    dir: &Path,
    name: &str,
    recipient: &age::x25519::Recipient,
) -> std::io::Result<()> {
    let files = Files::of(dir, name);
    let mut checksums = Vec::new();
    let mut in_clear = Vec::new();
    for (file, encrypted, encrypted_name) in [
        files
            .config
            .map(|(file, encrypted)| (file, encrypted, config_file(name, true))),
        files
            .database
            .map(|(file, encrypted)| (file, encrypted, database_file(name, true))),
    ]
    .into_iter()
    .flatten()
    {
        if encrypted {
            checksums.push((file.clone(), check::hash_file(&dir.join(&file))?));
            continue;
        }
        let mut input = BufReader::new(std::fs::File::open(dir.join(&file))?);
        let mut checksum = None;
        write_atomically(&dir.join(&encrypted_name), |output| {
            let hashing = check::HashingWriter::new(std::io::BufWriter::new(output));
            let mut writer = crypto::encrypt_to(recipient, hashing)?;
            std::io::copy(&mut input, &mut writer)?;
            let (buffered, sum) = writer.finish()?.finish();
            buffered.into_inner().map_err(|error| error.into_error())?;
            checksum = Some(sum);
            Ok(())
        })?;
        if let Some(checksum) = checksum {
            checksums.push((encrypted_name, checksum));
        }
        in_clear.push(file);
    }
    let text = check::checksums_text(&checksums);
    write_atomically(&dir.join(checksums_file(name)), |file| {
        file.write_all(text.as_bytes())
    })?;
    for file in in_clear {
        std::fs::remove_file(dir.join(file))?;
    }
    Ok(())
}

/// Reads a backup's file whole: what it holds, decrypted.
fn read_file(
    dir: &Path,
    file: &str,
    encrypted: bool,
    identity: Option<&age::x25519::Identity>,
    expected: Option<&String>,
    read: impl FnOnce(&mut dyn Read) -> Result<(), String>,
) -> Result<(), String> {
    let mut hashing = check::HashingReader::new(
        std::fs::File::open(dir.join(file)).map_err(|error| format!("{file}: {error}"))?,
    );
    let result = {
        let buffered = BufReader::new(&mut hashing);
        if encrypted {
            let identity = identity.ok_or_else(|| {
                "this backup is encrypted: its key is needed to test it".to_owned()
            })?;
            crypto::decrypt_from(identity, buffered)
                .map_err(|error| error.to_string())
                .and_then(|mut reader| read(&mut reader))
        } else {
            let mut buffered = buffered;
            read(&mut buffered)
        }
    };
    // What was not read, for its checksum: a changed file says so first.
    std::io::copy(&mut hashing, &mut std::io::sink())
        .map_err(|error| format!("{file}: {error}"))?;
    if expected.is_some_and(|expected| *expected != check::hex(&hashing.checksum())) {
        return Err(format!("{file}: changed or damaged (its checksum differs)"));
    }
    result.map_err(|error| format!("{file}: {error}"))
}

/// Reads a backup back to its end (`Backups::test`).
fn test_files(
    dir: &Path,
    name: &str,
    identity: Option<&age::x25519::Identity>,
    schema: i64,
) -> Result<(), String> {
    let files = Files::of(dir, name);
    let checksums = checksums_of(dir, name).unwrap_or_default();
    let Some((config, encrypted)) = &files.config else {
        return Err(format!("{}: missing", config_file(name, false)));
    };
    read_file(
        dir,
        config,
        *encrypted,
        identity,
        checksums.get(config),
        |reader| {
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .map_err(|error| format!("cannot be decrypted ({error})"))?;
            check::config(&bytes, schema)
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
    )?;
    if let Some((database, encrypted)) = &files.database {
        read_file(
            dir,
            database,
            *encrypted,
            identity,
            checksums.get(database),
            |reader| check::read_dump(reader),
        )?;
    }
    // A file the checksums list that is gone.
    for file in checksums.keys() {
        if !dir.join(file).exists() {
            return Err(format!("{file}: missing"));
        }
    }
    Ok(())
}

impl Backups {
    pub fn new(dir: PathBuf, database_url: String) -> Arc<Self> {
        Arc::new(Self {
            dir,
            database_url,
            lock: tokio::sync::Mutex::new(()),
            problem: std::sync::Mutex::new(None),
            damaged: std::sync::Mutex::new((BTreeMap::new(), None)),
            testing: std::sync::Mutex::new(None),
        })
    }

    /// What kept the folder from being written when last checked.
    pub fn problem(&self) -> Option<FolderProblem> {
        self.problem
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Writes and removes a file in the folder.
    fn probe(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        // A file of its own: another process checking at the same time
        // does not remove it.
        tempfile::Builder::new()
            .prefix(".heron-write-check-")
            .tempfile_in(&self.dir)?
            .close()
    }

    /// Whether the folder can be written, remembered for `/status`.
    pub fn check_folder(&self) -> std::io::Result<()> {
        let result = self.probe();
        let problem = result.as_ref().err().map(|error| {
            let (uid, gid) = ids();
            FolderProblem {
                error: error.to_string(),
                permission_denied: error.kind() == std::io::ErrorKind::PermissionDenied,
                uid,
                gid,
            }
        });
        match &problem {
            Some(problem) if problem.permission_denied => tracing::error!(
                dir = %self.dir.display(),
                error = %problem.error,
                "the backup folder cannot be written: give it to {}:{} on the host",
                problem.uid,
                problem.gid
            ),
            Some(problem) => tracing::error!(
                dir = %self.dir.display(),
                error = %problem.error,
                "the backup folder cannot be written"
            ),
            None => {}
        }
        let mut current = self
            .problem
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.is_some() && problem.is_none() {
            tracing::info!(dir = %self.dir.display(), "the backup folder can be written again");
        }
        current.clone_from(&problem);
        result
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The backups, newest first.
    pub fn list(&self) -> ApiResult<Vec<BackupView>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(folder_error(&self.dir, &error)),
        };
        let names: std::collections::BTreeSet<String> = entries
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(name_of)
                    .map(str::to_owned)
            })
            .collect();
        let damaged = self.lock_damaged().0.clone();
        let mut backups = Vec::new();
        for name in names.into_iter().rev() {
            let Some((time, kind)) = parse_name(&name) else {
                continue;
            };
            let files = Files::of(&self.dir, &name);
            let size = |file: &Option<(String, bool)>| {
                file.as_ref()
                    .and_then(|(file, _)| std::fs::metadata(self.dir.join(file)).ok())
                    .map(|metadata| metadata.len())
            };
            backups.push(BackupView {
                kind,
                created_at_unix_ms: time.as_millisecond(),
                config_size: size(&files.config).unwrap_or(0),
                database_size: size(&files.database),
                database_error: std::fs::read_to_string(self.dir.join(failed_file(&name)))
                    .ok()
                    .map(|error| error.trim().to_owned()),
                encrypted: files.encrypted(),
                key: None,
                recipient: None,
                damaged: damaged.get(&name).cloned(),
                name,
            });
        }
        Ok(backups)
    }

    /// The backups, newest first, with the key that opens each.
    pub async fn views(&self, pool: &MySqlPool) -> ApiResult<Vec<BackupView>> {
        let encryption = Encryption::load(pool).await?;
        let mut backups = self.list()?;
        for backup in &mut backups {
            if backup.encrypted {
                let (key, recipient) = encryption.key_of(backup.created_at_unix_ms, backup.kind);
                backup.key = Some(key);
                backup.recipient = recipient;
            }
        }
        Ok(backups)
    }

    fn lock_damaged(
        &self,
    ) -> std::sync::MutexGuard<'_, (BTreeMap<String, String>, Option<Instant>)> {
        self.damaged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn find(&self, name: &str) -> ApiResult<BackupView> {
        self.list()?
            .into_iter()
            .find(|backup| backup.name == name)
            .ok_or_else(|| ApiError::not_found(format!("backup {name}")))
    }

    /// Takes a backup: the configuration file, then the database dump.
    /// Waits for the backup or restore under way, here or in another
    /// process; the next one waits until both are dropped.
    async fn exclusive(
        &self,
    ) -> ApiResult<(tokio::sync::MutexGuard<'_, ()>, Option<std::fs::File>)> {
        let guard = self.lock.lock().await;
        self.check_folder()
            .map_err(|error| folder_error(&self.dir, &error))?;
        let path = self.dir.join(LOCK_FILE);
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .await
            .map_err(|error| folder_error(&self.dir, &error))?
            .into_std()
            .await;
        // Tried again and again rather than waited for in a thread: a task
        // stopped while it waits (the API shutting down) stops waiting.
        loop {
            match file.try_lock() {
                Ok(()) => return Ok((guard, Some(file))),
                Err(std::fs::TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                // A share without locks: the API still takes one at a time.
                Err(std::fs::TryLockError::Error(error)) => {
                    tracing::warn!(%error, "cannot lock {}: backups taken on the command line may run at the same time", path.display());
                    return Ok((guard, None));
                }
            }
        }
    }

    pub async fn create(&self, pool: &MySqlPool, kind: Kind) -> ApiResult<BackupView> {
        let _exclusive = self.exclusive().await?;
        self.create_locked(pool, kind, None).await
    }

    async fn create_locked(
        &self,
        pool: &MySqlPool,
        kind: Kind,
        upload: Option<&[u8]>,
    ) -> ApiResult<BackupView> {
        self.check_folder()
            .map_err(|error| folder_error(&self.dir, &error))?;
        let encryption = Encryption::load(pool).await?;
        let recipient = encryption.recipient()?;
        let mut now = Timestamp::now();
        // Named after the last change of encryption, which tells which key
        // a backup has.
        if let Ok(changed) = Timestamp::from_millisecond(encryption.changed_unix_ms)
            && changed > now
        {
            now = changed;
        }
        // Each backup has its own second, so that names sort by time.
        let taken: Vec<String> = self.list()?.into_iter().map(|backup| backup.name).collect();
        let name = loop {
            let time = now.strftime("%Y-%m-%dT%H-%M-%SZ").to_string();
            if !taken.iter().any(|name| name.starts_with(&time)) {
                break format!("{time}-{}", kind.as_str());
            }
            now += Duration::from_secs(1);
        };
        let bytes = match upload {
            Some(bytes) => bytes.to_vec(),
            None => {
                let file = file::export(pool, kind, now.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
                    .await?;
                let bytes = serde_json::to_vec_pretty(&file)
                    .map_err(|error| ApiError::Internal(error.to_string()))?;
                // Read back as a restore would read it: a file it would
                // refuse fails the backup.
                check::config(&bytes, file.schema).map_err(|error| {
                    ApiError::Internal(format!(
                        "the configuration file does not read back: {error}"
                    ))
                })?;
                bytes
            }
        };
        // An encrypted upload is kept as it is.
        let (bytes, encrypted) = match &recipient {
            _ if crypto::is_encrypted(&bytes) => (bytes, true),
            Some(recipient) => (
                crypto::encrypt(recipient, &bytes)
                    .map_err(|error| ApiError::Internal(format!("cannot encrypt: {error}")))?,
                true,
            ),
            None => (bytes, false),
        };
        let config = config_file(&name, encrypted);
        let mut checksums = vec![(config.clone(), sha256(&bytes))];
        let path = self.dir.join(&config);
        tokio::task::spawn_blocking(move || write_atomically(&path, |file| file.write_all(&bytes)))
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?
            .map_err(|error| folder_error(&self.dir, &error))?;
        let settings: BackupSettings =
            crate::settings::load(pool, BackupSettings::SETTINGS_NAME).await?;
        let mut whole = true;
        if settings.database && kind != Kind::Uploaded {
            let database = database_file(&name, recipient.is_some());
            match dump::dump(&self.database_url, &self.dir.join(&database), recipient).await {
                Ok(checksum) => checksums.push((database, checksum)),
                Err(error) => {
                    whole = false;
                    tracing::error!(backup = %name, %error, "the database dump failed");
                    let failed = self.dir.join(failed_file(&name));
                    let _ = write_atomically(&failed, |file| file.write_all(error.as_bytes()));
                }
            }
        }
        let text = check::checksums_text(&checksums);
        let path = self.dir.join(checksums_file(&name));
        tokio::task::spawn_blocking(move || {
            write_atomically(&path, |file| file.write_all(text.as_bytes()))
        })
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?
        .map_err(|error| folder_error(&self.dir, &error))?;
        if whole {
            metrics::gauge!("heron_api_backup_last_success_timestamp_seconds")
                .set(now.as_second() as f64);
        }
        tracing::info!(backup = %name, encrypted, "backed up");
        self.find(&name)
    }

    /// Deletes a backup's files.
    pub fn delete(&self, name: &str) -> ApiResult<()> {
        self.find(name)?;
        for file in files_of(name) {
            match std::fs::remove_file(self.dir.join(&file)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ApiError::Internal(format!("cannot delete {file}: {error}")));
                }
            }
        }
        self.lock_damaged().0.remove(name);
        Ok(())
    }

    /// Deletes the daily backups beyond `keep_days`, and the backups taken
    /// before restores beyond the last few.
    fn prune(&self, keep_days: u32) -> ApiResult<()> {
        let backups = self.list()?;
        let old = |kind: Kind, keep: usize| {
            backups
                .iter()
                .filter(move |backup| backup.kind == kind)
                .skip(keep)
        };
        for backup in old(Kind::Daily, keep_days as usize)
            .chain(old(Kind::BeforeRestore, KEEP_BEFORE_RESTORE))
        {
            self.delete(&backup.name)?;
            tracing::info!(backup = %backup.name, "old backup deleted");
        }
        Ok(())
    }

    /// A backup's configuration file, as it is in the folder.
    pub fn read(&self, name: &str) -> ApiResult<Vec<u8>> {
        self.find(name)?;
        let (file, _) = Files::of(&self.dir, name)
            .config
            .ok_or_else(|| ApiError::not_found(format!("the configuration file of {name}")))?;
        std::fs::read(self.dir.join(file))
            .map_err(|error| ApiError::Internal(format!("cannot read the backup: {error}")))
    }

    /// Restores a configuration file, after backing up the configuration.
    /// An encrypted file needs its key.
    pub async fn restore(
        &self,
        pool: &MySqlPool,
        stations: Option<&Stations>,
        bytes: &[u8],
        upload: bool,
        identity: Option<&age::x25519::Identity>,
    ) -> ApiResult<RestoreReport> {
        let plain = if crypto::is_encrypted(bytes) {
            let identity = identity.ok_or_else(|| {
                ApiError::Invalid("this backup is encrypted: give its key to restore it".to_owned())
            })?;
            crypto::decrypt(identity, bytes)?
        } else {
            bytes.to_vec()
        };
        let _exclusive = self.exclusive().await?;
        let schema: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success",
        )
        .fetch_one(pool)
        .await?;
        let file = check::config(&plain, schema)?;
        if upload {
            self.create_locked(pool, Kind::Uploaded, Some(bytes))
                .await?;
        }
        let before = self.create_locked(pool, Kind::BeforeRestore, None).await?;
        let mut report = RestoreReport {
            before_restore: before.name,
            ..RestoreReport::default()
        };
        // Read committed: what other tasks change meanwhile (a webhook's
        // deliveries, retried in the background) is read as it is now. In
        // repeatable read, deleting a row changed since the transaction's
        // snapshot fails ("Record has changed since last read").
        let mut connection = pool.acquire().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *connection)
            .await?;
        let mut tx = sqlx::Connection::begin(&mut *connection).await?;
        file::restore(&mut tx, &file, &mut report).await?;
        // Everyone signs in again, with the restored users.
        sqlx::query("DELETE FROM sessions")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(before = %report.before_restore, "configuration restored");
        if let Some(stations) = stations {
            let ids: Vec<u64> = sqlx::query_scalar("SELECT id FROM stations ORDER BY id")
                .fetch_all(pool)
                .await?;
            for id in ids {
                if let Err(error) = stations.reload(id).await {
                    tracing::error!(station = id, %error, "cannot restart the station after the restore");
                }
            }
            for station in stations.all() {
                station.sync.changed();
            }
        }
        let settings: BackupSettings =
            crate::settings::load(pool, BackupSettings::SETTINGS_NAME).await?;
        self.prune(settings.keep_days)?;
        Ok(report)
    }

    /// Encrypts the backups in clear for `recipient`: their names, and
    /// those left in clear (failing their checksums, or not written).
    pub async fn encrypt_existing(
        &self,
        recipient: &age::x25519::Recipient,
    ) -> ApiResult<(Vec<String>, Vec<String>)> {
        let _exclusive = self.exclusive().await?;
        let names: Vec<String> = self
            .list()?
            .into_iter()
            .filter(|backup| {
                let files = Files::of(&self.dir, &backup.name);
                files
                    .config
                    .iter()
                    .chain(&files.database)
                    .any(|(_, encrypted)| !encrypted)
            })
            .map(|backup| backup.name)
            .collect();
        let dir = self.dir.clone();
        let recipient = recipient.clone();
        let (encrypted, left) = tokio::task::spawn_blocking(move || {
            let mut encrypted = Vec::new();
            let mut left = Vec::new();
            for name in names {
                if let Some(damage) = verify(&dir, &name) {
                    tracing::warn!(backup = %name, %damage, "left in clear: it fails its checksums");
                    left.push(name);
                    continue;
                }
                match encrypt_backup(&dir, &name, &recipient) {
                    Ok(()) => encrypted.push(name),
                    Err(error) => {
                        tracing::error!(backup = %name, %error, "cannot encrypt the backup");
                        left.push(name);
                    }
                }
            }
            (encrypted, left)
        })
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
        tracing::info!(
            encrypted = encrypted.len(),
            left_in_clear = left.len(),
            "backups in clear encrypted"
        );
        Ok((encrypted, left))
    }

    /// Checks every backup against its checksums, now and then.
    pub(crate) async fn verify_all(&self) -> ApiResult<()> {
        let names: Vec<String> = self.list()?.into_iter().map(|backup| backup.name).collect();
        let dir = self.dir.clone();
        let damaged: BTreeMap<String, String> = tokio::task::spawn_blocking(move || {
            names
                .into_iter()
                .filter_map(|name| verify(&dir, &name).map(|damage| (name, damage)))
                .collect()
        })
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
        for (name, damage) in &damaged {
            tracing::error!(backup = %name, %damage, "a backup fails its checksums");
        }
        metrics::gauge!("heron_api_backups_damaged").set(damaged.len() as f64);
        *self.lock_damaged() = (damaged, Some(Instant::now()));
        Ok(())
    }

    /// The backup being tested.
    pub fn testing(&self) -> Option<String> {
        self.testing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Marks `name` as being tested: one test at a time.
    fn claim_test(&self, name: &str) -> ApiResult<()> {
        let mut testing = self
            .testing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(other) = &*testing {
            return Err(ApiError::Conflict(format!("{other} is being tested")));
        }
        *testing = Some(name.to_owned());
        Ok(())
    }

    /// Tests a backup in the background (`test`), marked as being tested
    /// at once.
    pub fn spawn_test(
        self: &Arc<Self>,
        pool: &MySqlPool,
        name: String,
        identity: Option<age::x25519::Identity>,
        by: Option<String>,
    ) -> ApiResult<()> {
        self.find(&name)?;
        self.claim_test(&name)?;
        let backups = Arc::clone(self);
        let pool = pool.clone();
        tokio::spawn(async move {
            if let Err(error) = backups.run_test(&pool, &name, identity.as_ref(), by).await {
                tracing::error!(backup = %name, %error, "cannot test the backup");
            }
        });
        Ok(())
    }

    /// Reads a backup back to its end, with its key when it is encrypted:
    /// its checksums, its configuration file checked as a restore would,
    /// its dump whole. Nothing is written but the result.
    pub async fn test(
        &self,
        pool: &MySqlPool,
        name: &str,
        identity: Option<&age::x25519::Identity>,
        by: Option<String>,
    ) -> ApiResult<TestRecord> {
        self.find(name)?;
        self.claim_test(name)?;
        self.run_test(pool, name, identity, by).await
    }

    async fn run_test(
        &self,
        pool: &MySqlPool,
        name: &str,
        identity: Option<&age::x25519::Identity>,
        by: Option<String>,
    ) -> ApiResult<TestRecord> {
        let result = self.test_claimed(pool, name, identity).await;
        let now = Timestamp::now();
        let test = TestRecord {
            name: name.to_owned(),
            at_unix_ms: now.as_millisecond(),
            by,
            ok: result.is_ok(),
            error: result.err(),
        };
        match &test.error {
            None => {
                tracing::info!(backup = %name, "backup tested: it reads back whole");
                metrics::gauge!("heron_api_backup_last_test_success_timestamp_seconds")
                    .set(now.as_second() as f64);
            }
            Some(error) => tracing::error!(backup = %name, %error, "backup test failed"),
        }
        // Saved before the test is over: whoever waits for it reads it.
        let recorded = Tests::record(pool, test.clone()).await;
        *self
            .testing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        recorded.map(|()| test)
    }

    async fn test_claimed(
        &self,
        pool: &MySqlPool,
        name: &str,
        identity: Option<&age::x25519::Identity>,
    ) -> Result<(), String> {
        let _exclusive = self.exclusive().await.map_err(|error| error.to_string())?;
        let schema: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success",
        )
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
        let dir = self.dir.clone();
        let owned = name.to_owned();
        let identity = identity.cloned();
        tokio::task::spawn_blocking(move || test_files(&dir, &owned, identity.as_ref(), schema))
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result)
    }

    /// Takes the daily backup at its time, and catches up a day missed.
    pub async fn run_daily(self: Arc<Self>, pool: MySqlPool, stations: Arc<Stations>) {
        loop {
            if let Err(error) = self.daily(&pool, &stations).await {
                tracing::error!(%error, "the daily backup failed");
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }

    async fn daily(&self, pool: &MySqlPool, stations: &Stations) -> ApiResult<()> {
        // Checked every minute: the banner goes once the folder is fixed.
        if self.check_folder().is_err() {
            return Ok(());
        }
        // When the API starts, then every day: what fails its checksums,
        // and the newest backup read back when it is in clear.
        if self
            .lock_damaged()
            .1
            .is_none_or(|at| at.elapsed() >= VERIFY_EVERY)
        {
            self.verify_all().await?;
            if let Some(newest) = self
                .list()?
                .into_iter()
                .next()
                .filter(|backup| !backup.encrypted)
                && let Err(error) = self.test(pool, &newest.name, None, None).await
            {
                tracing::error!(backup = %newest.name, %error, "cannot test the backup");
            }
        }
        let settings: BackupSettings =
            crate::settings::load(pool, BackupSettings::SETTINGS_NAME).await?;
        let Some((hour, minute)) = settings.at().filter(|_| settings.enabled) else {
            return Ok(());
        };
        let zone = crate::settings::load_for::<StationSettings>(
            pool,
            stations.first().id(),
            StationSettings::SETTINGS_NAME,
        )
        .await?
        .time_zone();
        let due = last_due(&Timestamp::now().to_zoned(zone), hour, minute)
            .map_err(|error| ApiError::Internal(error.to_string()))?;
        let last = self
            .list()?
            .into_iter()
            .find(|backup| backup.kind == Kind::Daily)
            .map(|backup| backup.created_at_unix_ms);
        if last.is_some_and(|last| last >= due.timestamp().as_millisecond()) {
            return Ok(());
        }
        let backup = self.create(pool, Kind::Daily).await?;
        self.prune(settings.keep_days)?;
        // In clear, it is read back at once: no key is needed.
        if !backup.encrypted
            && let Err(error) = self.test(pool, &backup.name, None, None).await
        {
            tracing::error!(backup = %backup.name, %error, "cannot test the backup");
        }
        Ok(())
    }
}

/// When the daily backup was last due: today at its time, or yesterday
/// before it. A daily backup older than that is missing.
fn last_due(now: &jiff::Zoned, hour: i8, minute: i8) -> Result<jiff::Zoned, jiff::Error> {
    let today = now
        .with()
        .hour(hour)
        .minute(minute)
        .second(0)
        .subsec_nanosecond(0)
        .build()?;
    if today <= *now {
        Ok(today)
    } else {
        today.yesterday()
    }
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_backups, create_backup))
        .routes(routes!(delete_backup))
        .routes(routes!(download_config))
        .routes(routes!(download_database))
        .routes(routes!(restore_backup))
        .routes(routes!(upload_and_restore))
        .routes(routes!(get_backup_settings, put_backup_settings))
        .routes(routes!(test_backup))
        .routes(routes!(get_tests))
        .routes(routes!(get_state))
        .merge(key::router())
        // A configuration file with every track's settings is large.
        .layer(axum::extract::DefaultBodyLimit::max(MAX_UPLOAD))
}

/// The backups, newest first. Admins only.
#[utoipa::path(get, path = "/backups", tag = "backups",
    responses((status = 200, body = Vec<BackupView>), (status = "4XX", body = ErrorBody)))]
async fn list_backups(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<BackupView>>> {
    user.require(Role::Admin)?;
    state.backups.views(&state.pool).await.map(Json)
}

/// Backs up now: the configuration file and the database dump. Admins only.
#[utoipa::path(post, path = "/backups", tag = "backups",
    responses((status = 201, body = BackupView), (status = "4XX", body = ErrorBody)))]
async fn create_backup(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<(StatusCode, Json<BackupView>)> {
    user.require(Role::Admin)?;
    let backup = state.backups.create(&state.pool, Kind::Manual).await?;
    tracing::info!(user = %user.username, backup = %backup.name, "backed up on request");
    Ok((StatusCode::CREATED, Json(backup)))
}

/// Deletes a backup's files. Admins only.
#[utoipa::path(delete, path = "/backups/{name}", tag = "backups", params(("name" = String, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_backup(
    user: CurrentUser,
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> ApiResult<StatusCode> {
    user.require(Role::Admin)?;
    state.backups.delete(&name)?;
    tracing::info!(user = %user.username, backup = %name, "backup deleted");
    Ok(StatusCode::NO_CONTENT)
}

fn download(path: PathBuf, file_name: String, content_type: &'static str) -> ApiResult<Response> {
    let bytes = std::fs::read(&path)
        .map_err(|error| ApiError::Internal(format!("cannot read {file_name}: {error}")))?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file_name}\""),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// Downloads a backup's configuration file, encrypted when it is. Admins
/// only: it holds secrets.
#[utoipa::path(get, path = "/backups/{name}/config", tag = "backups", params(("name" = String, Path)),
    responses((status = 200, content_type = "application/json"), (status = "4XX", body = ErrorBody)))]
async fn download_config(
    user: CurrentUser,
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> ApiResult<Response> {
    user.require(Role::Admin)?;
    state.backups.find(&name)?;
    let (file, encrypted) = Files::of(state.backups.dir(), &name)
        .config
        .ok_or_else(|| ApiError::not_found(format!("the configuration file of {name}")))?;
    let content_type = if encrypted {
        "application/octet-stream"
    } else {
        "application/json"
    };
    download(state.backups.dir().join(&file), file, content_type)
}

/// Downloads a backup's database dump, encrypted when it is. Admins only:
/// it holds secrets.
#[utoipa::path(get, path = "/backups/{name}/database", tag = "backups", params(("name" = String, Path)),
    responses((status = 200, content_type = "application/gzip"), (status = "4XX", body = ErrorBody)))]
async fn download_database(
    user: CurrentUser,
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> ApiResult<Response> {
    user.require(Role::Admin)?;
    state.backups.find(&name)?;
    let (file, encrypted) = Files::of(state.backups.dir(), &name)
        .database
        .ok_or_else(|| ApiError::not_found(format!("the database dump of {name}")))?;
    let content_type = if encrypted {
        "application/octet-stream"
    } else {
        "application/gzip"
    };
    download(state.backups.dir().join(&file), file, content_type)
}

/// The key of an encrypted backup.
#[derive(Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WithKey {
    /// The secret key, `AGE-SECRET-KEY-1…`, or the contents of its file.
    #[serde(default)]
    pub key: Option<String>,
}

impl WithKey {
    /// The key in a request's body: none when the body is empty, as
    /// before encryption.
    fn identity(body: &[u8]) -> ApiResult<Option<age::x25519::Identity>> {
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        let body: Self = serde_json::from_slice(body)
            .map_err(|error| ApiError::BadRequest(format!("invalid body: {error}")))?;
        Self::key(body.key.as_deref())
    }

    fn key(key: Option<&str>) -> ApiResult<Option<age::x25519::Identity>> {
        key.filter(|key| !key.trim().is_empty())
            .map(crypto::parse_identity)
            .transpose()
    }
}

/// Restores a backup's configuration file, after backing up the current
/// configuration. Everyone signs in again. Admins only.
#[utoipa::path(post, path = "/backups/{name}/restore", tag = "backups", params(("name" = String, Path)),
    request_body(content = Option<WithKey>, description = "The key of an encrypted backup"),
    responses((status = 200, body = RestoreReport), (status = "4XX", body = ErrorBody)))]
async fn restore_backup(
    user: CurrentUser,
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Json<RestoreReport>> {
    user.require(Role::Admin)?;
    let identity = WithKey::identity(&body)?;
    let bytes = state.backups.read(&name)?;
    let report = state
        .backups
        .restore(
            &state.pool,
            Some(&state.stations),
            &bytes,
            false,
            identity.as_ref(),
        )
        .await?;
    tracing::info!(user = %user.username, backup = %name, "backup restored");
    Ok(Json(report))
}

/// Restores an uploaded configuration file (the request's body), kept as an
/// `uploaded` backup. An encrypted one needs its key, in the
/// `X-Heron-Backup-Key` header. Admins only.
#[utoipa::path(post, path = "/backups/restore", tag = "backups",
    params(("X-Heron-Backup-Key" = Option<String>, Header, description = "The key of an encrypted file")),
    request_body(content = String, content_type = "application/octet-stream", description = "A configuration file, `.json` or `.json.age`"),
    responses((status = 200, body = RestoreReport), (status = "4XX", body = ErrorBody)))]
async fn upload_and_restore(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<RestoreReport>> {
    user.require(Role::Admin)?;
    let key = headers
        .get(KEY_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| ApiError::Invalid("the backup key is not text".to_owned()))
        })
        .transpose()?;
    let identity = WithKey::key(key)?;
    let report = state
        .backups
        .restore(
            &state.pool,
            Some(&state.stations),
            &body,
            true,
            identity.as_ref(),
        )
        .await?;
    tracing::info!(user = %user.username, "uploaded backup restored");
    Ok(Json(report))
}

#[derive(Serialize, ToSchema)]
pub struct Testing {
    /// The backup being tested.
    pub testing: String,
}

/// Tests a backup in the background: read back to its end with its key
/// when it is encrypted, nothing restored. Its result is in
/// `/backups/tests`. Admins only.
#[utoipa::path(post, path = "/backups/{name}/test", tag = "backups", params(("name" = String, Path)),
    request_body(content = Option<WithKey>, description = "The key of an encrypted backup"),
    responses((status = 202, body = Testing), (status = "4XX", body = ErrorBody)))]
async fn test_backup(
    user: CurrentUser,
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Testing>)> {
    user.require(Role::Admin)?;
    let identity = WithKey::identity(&body)?;
    let backup = state.backups.find(&name)?;
    if backup.encrypted && identity.is_none() {
        return Err(ApiError::Invalid(
            "this backup is encrypted: give its key to test it".to_owned(),
        ));
    }
    state.backups.spawn_test(
        &state.pool,
        name.clone(),
        identity,
        Some(user.username.clone()),
    )?;
    Ok((StatusCode::ACCEPTED, Json(Testing { testing: name })))
}

#[derive(Serialize, ToSchema)]
pub struct TestsView {
    /// The backup being tested.
    pub testing: Option<String>,
    pub last: Option<TestRecord>,
    /// The last test that passed.
    pub last_ok: Option<TestRecord>,
}

/// The last backup test, the last one that passed, and the one under way.
/// Admins only.
#[utoipa::path(get, path = "/backups/tests", tag = "backups",
    responses((status = 200, body = TestsView), (status = "4XX", body = ErrorBody)))]
async fn get_tests(user: CurrentUser, State(state): State<AppState>) -> ApiResult<Json<TestsView>> {
    user.require(Role::Admin)?;
    let tests = Tests::load(&state.pool).await?;
    Ok(Json(TestsView {
        testing: state.backups.testing(),
        last: tests.last,
        last_ok: tests.last_ok,
    }))
}

/// What the banners say about backups, for the admin asking.
#[derive(Serialize, ToSchema)]
pub struct BackupState {
    pub encryption: KeyState,
    /// This admin's key check is due.
    pub check_due: bool,
    /// Backups that fail their checksums.
    pub damaged: usize,
    /// The last test failed, or none passed for 30 days.
    pub test_due: bool,
}

/// What the banners and the key check need, for the admin asking. Admins
/// only.
#[utoipa::path(get, path = "/backups/state", tag = "backups",
    responses((status = 200, body = BackupState), (status = "4XX", body = ErrorBody)))]
async fn get_state(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<BackupState>> {
    user.require(Role::Admin)?;
    let encryption = Encryption::load(&state.pool).await?;
    let on = encryption.current.is_some();
    let backups = state.backups.list()?;
    let tests = Tests::load(&state.pool).await?;
    Ok(Json(BackupState {
        encryption: if encryption.pending.is_some() {
            KeyState::Pending
        } else if on {
            KeyState::On
        } else {
            KeyState::Off
        },
        check_due: on && key::check_due(&state.pool, user.id).await?,
        damaged: backups
            .iter()
            .filter(|backup| backup.damaged.is_some())
            .count(),
        test_due: !backups.is_empty()
            && state.backups.testing().is_none()
            && tests.due(Timestamp::now().as_millisecond()),
    }))
}

#[utoipa::path(get, path = "/settings/backups", tag = "settings",
    responses((status = 200, body = BackupSettings), (status = "4XX", body = ErrorBody)))]
async fn get_backup_settings(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<BackupSettings>> {
    user.require(Role::Admin)?;
    crate::settings::load(&state.pool, BackupSettings::SETTINGS_NAME)
        .await
        .map(Json)
}

#[utoipa::path(put, path = "/settings/backups", tag = "settings", request_body = BackupSettings,
    responses((status = 200, body = BackupSettings), (status = "4XX", body = ErrorBody)))]
async fn put_backup_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(settings): Json<BackupSettings>,
) -> ApiResult<Json<BackupSettings>> {
    user.require(Role::Admin)?;
    settings.check()?;
    crate::settings::save(&state.pool, BackupSettings::SETTINGS_NAME, &settings).await?;
    tracing::info!(user = %user.username, ?settings, "backup settings changed");
    Ok(Json(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_carry_the_time_and_kind() {
        let (time, kind) = parse_name("2026-11-03T03-30-00Z-before_restore").unwrap();
        assert_eq!(kind, Kind::BeforeRestore);
        assert_eq!(time.to_string(), "2026-11-03T03:30:00Z");
        assert!(parse_name("2026-11-03T03-30-00Z-weekly").is_none());
        assert!(parse_name("../../etc/passwd").is_none());
        assert!(parse_name("2026-11-03T03-30-00Z").is_none());
    }

    #[test]
    fn the_daily_backup_is_due_at_its_time_or_caught_up() {
        let zone = jiff::tz::TimeZone::get("Europe/Paris").unwrap();
        let at = |time: &str| {
            jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M", time)
                .unwrap()
                .to_zoned(zone.clone())
                .unwrap()
        };
        assert_eq!(
            last_due(&at("2026-11-03 04:00"), 3, 30).unwrap(),
            at("2026-11-03 03:30")
        );
        assert_eq!(
            last_due(&at("2026-11-03 03:30"), 3, 30).unwrap(),
            at("2026-11-03 03:30")
        );
        // Before its time: yesterday's, caught up if it was missed.
        assert_eq!(
            last_due(&at("2026-11-03 02:00"), 3, 30).unwrap(),
            at("2026-11-02 03:30")
        );
    }

    #[test]
    fn old_daily_and_before_restore_backups_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let backups = Backups::new(dir.path().to_path_buf(), String::new());
        for day in 1..=9 {
            for kind in ["daily", "before_restore", "manual"] {
                let name = format!("2026-11-{day:02}T03-30-00Z-{kind}");
                std::fs::write(dir.path().join(config_file(&name, false)), "{}").unwrap();
                std::fs::write(dir.path().join(database_file(&name, false)), "").unwrap();
            }
        }
        backups.prune(7).unwrap();
        let left = backups.list().unwrap();
        let days = |kind: Kind| -> Vec<String> {
            left.iter()
                .filter(|backup| backup.kind == kind)
                .map(|backup| backup.name[8..10].to_owned())
                .collect()
        };
        assert_eq!(
            days(Kind::Daily),
            ["09", "08", "07", "06", "05", "04", "03"]
        );
        assert_eq!(days(Kind::BeforeRestore).len(), KEEP_BEFORE_RESTORE);
        assert_eq!(days(Kind::Manual).len(), 9);
        // A backup's dump goes with it.
        assert!(
            !dir.path()
                .join(database_file("2026-11-01T03-30-00Z-daily", false))
                .exists()
        );
        assert!(
            dir.path()
                .join(database_file("2026-11-01T03-30-00Z-manual", false))
                .exists()
        );
    }

    #[test]
    fn a_folder_that_cannot_be_written_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the folder should be: it cannot be created.
        let taken = dir.path().join("backups");
        std::fs::write(&taken, "").unwrap();
        let backups = Backups::new(taken.clone(), String::new());
        assert!(backups.check_folder().is_err());
        let problem = backups.problem().unwrap();
        assert_eq!((problem.uid, problem.gid), ids());
        assert!(!problem.permission_denied, "not a matter of owner");
        // Fixed: the problem goes at the next check.
        std::fs::remove_file(&taken).unwrap();
        assert!(backups.check_folder().is_ok());
        assert_eq!(backups.problem(), None);
        assert_eq!(
            std::fs::read_dir(&taken).unwrap().count(),
            0,
            "the probe is gone"
        );
    }

    #[test]
    fn permission_errors_say_how_to_fix_them() {
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let message = folder_error(Path::new("/var/lib/heron/backups"), &denied).to_string();
        let (uid, gid) = ids();
        assert!(message.contains(&format!("chown {uid}:{gid}")), "{message}");
        assert!(message.contains("HERON_BACKUP_FOLDER"), "{message}");
    }

    #[test]
    fn settings_are_checked() {
        assert!(BackupSettings::default().check().is_ok());
        for time in ["3:30", "24:00", "03:60", "03:30:00"] {
            let settings = BackupSettings {
                time: time.to_owned(),
                ..BackupSettings::default()
            };
            assert!(settings.check().is_err(), "{time}");
        }
        let settings = BackupSettings {
            keep_days: 0,
            ..BackupSettings::default()
        };
        assert!(settings.check().is_err());
    }

    #[test]
    fn files_are_written_whole_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("heron-config-x.json");
        write_atomically(&path, |file| file.write_all(b"{}")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        let mode = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        // A failed write leaves nothing under the name, nor its temporary file.
        let failed = write_atomically(&dir.path().join("other.json"), |_| {
            Err(std::io::Error::other("disk full"))
        });
        assert!(failed.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn one_backup_at_a_time_across_processes() {
        let dir = tempfile::tempdir().unwrap();
        // Two processes: each its own `Backups`, the folder in common.
        let api = Backups::new(dir.path().to_owned(), String::new());
        let command_line = Backups::new(dir.path().to_owned(), String::new());
        let held = api.exclusive().await.unwrap();
        assert!(held.1.is_some(), "locked");
        let waiting = tokio::spawn(async move { command_line.exclusive().await.map(|_| ()) });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!waiting.is_finished(), "waits for the other");
        // Stopped while it waits (the API shutting down): it stops at once.
        let stopped = Backups::new(dir.path().to_owned(), String::new());
        let gone = tokio::time::timeout(Duration::from_millis(300), stopped.exclusive()).await;
        assert!(gone.is_err(), "still waiting when stopped");
        drop(held);
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("goes on once the other is done")
            .unwrap()
            .unwrap();
    }
}
