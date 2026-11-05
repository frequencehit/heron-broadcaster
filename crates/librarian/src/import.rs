// SPDX-License-Identifier: AGPL-3.0-or-later

//! The incoming folder: files dropped there are moved into the buffer zone
//! once they have not changed for the settle time, like the scanner does,
//! and wait there with uploads until someone imports them (see
//! [`crate::staging`]). The files of one folder form a group, so siblings
//! are identified together and agree on their release.
//!
//! Each move is recorded in `staged_files` (`receiving`) before the file is
//! touched, and finished or rolled back by [`crate::staging::recover`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use heron_core::{FileStamp, LibraryPath, LibrarySettings};
use heron_library::{Tags, is_audio, is_hidden};
use serde::Serialize;
use sqlx::MySqlPool;
use tokio::sync::{mpsc, watch};

use crate::fsops;
use crate::identify::Identifier;
use crate::identify::matching::Candidate;
use crate::journal::Roots;
use crate::staging::Stager;

/// Candidates kept for the review screen.
pub(crate) const MAX_CANDIDATES: usize = 20;
/// Quiet time after the last file system event before looking.
const EVENT_DEBOUNCE: Duration = Duration::from_secs(2);
/// Name of the cover saved in album folders.
const COVER_FILE: &str = "cover.jpg";
/// How often the buffer zone is looked at for files to identify.
const STAGING_POLL: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("the incoming directory {path} is not readable: {source}")]
    Root {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Analysis(#[from] heron_analysis::Error),
    #[error("task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Outcome of one pass over the incoming folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    /// Files moved into the buffer zone.
    pub staged: u64,
    /// Files still changing.
    pub deferred: u64,
    pub failed: u64,
}

pub struct Importer {
    pool: MySqlPool,
    roots: Roots,
    settle: Duration,
}

impl Importer {
    pub fn new(pool: MySqlPool, roots: Roots, settle: Duration) -> Self {
        Self {
            pool,
            roots,
            settle,
        }
    }

    /// Moves incoming files into the buffer zone after file system events
    /// and every `interval`, and has `stager` identify what waits there,
    /// until `stopped` turns true.
    pub async fn run(
        self,
        mut stager: Stager,
        interval: Duration,
        watch: bool,
        mut stopped: watch::Receiver<bool>,
    ) {
        let (events_tx, mut events) = mpsc::channel::<()>(1);
        let _watcher = if watch {
            watch_dir(&self.roots.incoming, events_tx)
        } else {
            None
        };
        let mut next_look = tokio::time::Instant::now();
        while !*stopped.borrow() {
            if tokio::time::Instant::now() >= next_look {
                let wait = match self.import_once().await {
                    Ok(report) if report.deferred > 0 => self.settle + Duration::from_secs(1),
                    Ok(_) => interval,
                    Err(error) => {
                        tracing::error!(%error, "cannot look at the incoming folder");
                        interval
                    }
                };
                next_look = tokio::time::Instant::now() + wait;
            }
            if let Err(error) = stager.identify_once().await {
                tracing::error!(%error, "cannot identify the files of the buffer zone");
            }
            let wake = next_look.min(tokio::time::Instant::now() + STAGING_POLL);
            tokio::select! {
                () = tokio::time::sleep_until(wake) => {}
                Some(()) = events.recv() => {
                    while let Ok(Some(())) = tokio::time::timeout(EVENT_DEBOUNCE, events.recv()).await {}
                    next_look = tokio::time::Instant::now();
                }
                _ = stopped.changed() => {}
            }
        }
    }

    /// One pass over the incoming folder: settled files move into the
    /// buffer zone, one group per folder.
    pub async fn import_once(&self) -> Result<ImportReport, ImportError> {
        let root = self.roots.incoming.clone();
        let files = tokio::task::spawn_blocking(move || walk(&root)).await??;
        let mut report = ImportReport::default();
        let settled_before =
            now_us().saturating_sub(i64::try_from(self.settle.as_micros()).unwrap_or(0));
        let mut groups: BTreeMap<String, Vec<(LibraryPath, FileStamp)>> = BTreeMap::new();
        for (path, stamp) in files {
            if stamp.mtime_us > settled_before {
                report.deferred += 1;
                continue;
            }
            let dir = path
                .as_str()
                .rsplit_once('/')
                .map_or("", |(dir, _)| dir)
                .to_owned();
            groups.entry(dir).or_default().push((path, stamp));
        }

        for (dir, files) in groups {
            let name = if dir.is_empty() { "incoming" } else { &dir };
            let group_id =
                sqlx::query("INSERT INTO staged_groups (source, name) VALUES ('incoming', ?)")
                    .bind(truncate(name, 512))
                    .execute(&self.pool)
                    .await?
                    .last_insert_id();
            for (path, stamp) in files {
                match self.stage(group_id, &path, stamp).await {
                    Ok(true) => report.staged += 1,
                    Ok(false) => report.deferred += 1,
                    Err(StageError::Database(error)) => return Err(error.into()),
                    Err(StageError::Io(error)) => {
                        tracing::warn!(%path, %error, "cannot move into the buffer zone");
                        report.failed += 1;
                    }
                }
            }
            fsops::remove_empty_dirs(
                &LibraryPath::from_stored(dir).resolve(&self.roots.incoming),
                &self.roots.incoming,
            );
        }
        // Folders whose files all failed or changed leave an empty group.
        sqlx::query(
            "DELETE FROM staged_groups
             WHERE source = 'incoming'
               AND NOT EXISTS (SELECT 1 FROM staged_files f WHERE f.group_id = staged_groups.id)",
        )
        .execute(&self.pool)
        .await?;
        if report.staged > 0 {
            tracing::info!(?report, "incoming files moved into the buffer zone");
        }
        metrics::counter!("heron_librarian_staged_total", "source" => "incoming")
            .increment(report.staged);
        Ok(report)
    }

    /// Moves one incoming file into its group's folder of the buffer zone.
    /// Returns `false` when it changed meanwhile (it is tried again later).
    async fn stage(
        &self,
        group_id: u64,
        path: &LibraryPath,
        stamp: FileStamp,
    ) -> Result<bool, StageError> {
        let name = path
            .as_str()
            .rsplit_once('/')
            .map_or(path.as_str(), |(_, name)| name);
        let staged = LibraryPath::from_stored(format!("{group_id}/{name}"));
        let file_id = sqlx::query(
            "INSERT INTO staged_files (group_id, staged_path, original_name, size, status)
             VALUES (?, ?, ?, ?, 'receiving')",
        )
        .bind(group_id)
        .bind(staged.as_str())
        .bind(path.as_str())
        .bind(stamp.size)
        .execute(&self.pool)
        .await?
        .last_insert_id();
        crate::journal::crash_point("staging-recorded");

        let from = path.resolve(&self.roots.incoming);
        let to = staged.resolve(&self.roots.staging);
        let temp = LibraryPath::from_stored(format!("{group_id}/.heron-{file_id}"))
            .resolve(&self.roots.staging);
        let moved = tokio::task::spawn_blocking(move || move_in(&from, &to, &temp, stamp))
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        match moved {
            Ok(true) => {
                sqlx::query("UPDATE staged_files SET status = 'identifying' WHERE id = ?")
                    .bind(file_id)
                    .execute(&self.pool)
                    .await?;
                tracing::info!(from = %path, to = %staged, "moved into the buffer zone");
                Ok(true)
            }
            Ok(false) | Err(_) => {
                sqlx::query("DELETE FROM staged_files WHERE id = ?")
                    .bind(file_id)
                    .execute(&self.pool)
                    .await?;
                moved.map_err(StageError::Io)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum StageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
}

/// Moves `from` to `to` without replacing anything: a rename, or across
/// file systems a verified copy into `temp`, renamed into place, before
/// the original is removed. Returns `false` (nothing moved) when the file
/// changed since it was looked at.
fn move_in(from: &Path, to: &Path, temp: &Path, stamp: FileStamp) -> std::io::Result<bool> {
    if FileStamp::from_metadata(&std::fs::metadata(from)?) != stamp {
        return Ok(false);
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match fsops::rename_no_replace(from, to) {
        Ok(()) => {
            if let Some(parent) = to.parent() {
                fsops::sync_dir(parent);
            }
            return Ok(true);
        }
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::CrossesDevices | ErrorKind::Unsupported
            ) => {}
        Err(error) => return Err(error),
    }
    fsops::copy_verified(from, temp)?;
    if FileStamp::from_metadata(&std::fs::metadata(from)?) != stamp {
        let _ = std::fs::remove_file(temp);
        return Ok(false);
    }
    if let Err(error) = fsops::rename_no_replace(temp, to) {
        let _ = std::fs::remove_file(temp);
        return Err(error);
    }
    if let Some(parent) = to.parent() {
        fsops::sync_dir(parent);
    }
    crate::journal::crash_point("staging-copied");
    std::fs::remove_file(from)?;
    Ok(true)
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// Front covers of releases, from the Cover Art Archive.
pub(crate) async fn fetch_covers(
    identifier: &Identifier,
    releases: HashSet<&String>,
) -> HashMap<String, Vec<u8>> {
    let mut covers = HashMap::new();
    for release in releases {
        match identifier.cover(release).await {
            Ok(Some(jpeg)) => {
                covers.insert(release.clone(), jpeg);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(release, %error, "cannot fetch the cover"),
        }
    }
    covers
}

/// Saves `cover.jpg` in an album folder, unless there is one already.
pub(crate) fn save_cover(dir: &Path, jpeg: &[u8]) {
    let target = dir.join(COVER_FILE);
    if target.exists() {
        return;
    }
    let temp = dir.join(format!(".heron-{COVER_FILE}"));
    let written = std::fs::write(&temp, jpeg)
        .and_then(|()| fsops::sync_file(&temp))
        .and_then(|()| fsops::rename_no_replace(&temp, &target));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        tracing::warn!(dir = %dir.display(), %error, "cannot save the cover");
    }
}

/// The identified tags, keeping the file's own values for fields
/// MusicBrainz left empty.
pub(crate) fn merge(identified: Tags, own: &Tags) -> Tags {
    Tags {
        title: identified.title.or_else(|| own.title.clone()),
        artist: identified.artist.or_else(|| own.artist.clone()),
        album_artist: identified.album_artist.or_else(|| own.album_artist.clone()),
        album: identified.album.or_else(|| own.album.clone()),
        track: identified.track.or(own.track),
        track_total: identified.track_total.or(own.track_total),
        disc: identified.disc.or(own.disc),
        disc_total: identified.disc_total.or(own.disc_total),
        date: identified.date.or_else(|| own.date.clone()),
        compilation: identified.compilation,
        musicbrainz: identified.musicbrainz,
    }
}

pub(crate) fn candidates_json(candidates: &[Candidate]) -> Option<serde_json::Value> {
    if candidates.is_empty() {
        return None;
    }
    let mut sorted: Vec<&Candidate> = candidates.iter().collect();
    sorted.sort_by(|a, b| b.score.total_cmp(&a.score));
    serde_json::to_value(sorted.into_iter().take(MAX_CANDIDATES).collect::<Vec<_>>()).ok()
}

/// Reads the library settings from the `settings` table, falling back to
/// the defaults when the row is missing or invalid.
pub async fn load_settings(pool: &MySqlPool) -> Result<LibrarySettings, sqlx::Error> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT CAST(value AS CHAR) FROM settings WHERE name = ?")
            .bind(LibrarySettings::SETTINGS_NAME)
            .fetch_optional(pool)
            .await?;
    let Some(value) = value else {
        return Ok(LibrarySettings::default());
    };
    match serde_json::from_str(&value) {
        Ok(settings) => Ok(settings),
        Err(error) => {
            tracing::error!(%error, "invalid library settings, using the defaults");
            Ok(LibrarySettings::default())
        }
    }
}

/// Audio files in the incoming folder, with their stamps.
pub(crate) fn walk(root: &Path) -> Result<Vec<(LibraryPath, FileStamp)>, ImportError> {
    let root_error = |source| ImportError::Root {
        path: root.to_path_buf(),
        source,
    };
    if !std::fs::metadata(root).map_err(root_error)?.is_dir() {
        return Err(root_error(std::io::Error::other("not a directory")));
    }
    let mut files = Vec::new();
    let entries = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_hidden(entry.file_name()));
    for entry in entries.flatten() {
        if !entry.file_type().is_file() || !is_audio(entry.path()) {
            continue;
        }
        let Some(path) = LibraryPath::from_file(root, entry.path()) else {
            tracing::warn!(path = %entry.path().display(), "skipping file with a non UTF-8 name");
            continue;
        };
        if let Ok(metadata) = entry.metadata() {
            files.push((path, FileStamp::from_metadata(&metadata)));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn watch_dir(dir: &Path, events: mpsc::Sender<()>) -> Option<notify::RecommendedWatcher> {
    let watcher = heron_library::watch::watch_dir(dir, move || {
        let _ = events.try_send(());
    });
    match watcher {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            tracing::warn!(%error, "cannot watch the incoming folder, relying on periodic checks");
            None
        }
    }
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
        })
}
