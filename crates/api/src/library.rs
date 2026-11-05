// SPDX-License-Identifier: AGPL-3.0-or-later

//! Library scanner: walks the music library, reads tags, keeps `tracks` in
//! sync and queues analysis jobs.
//!
//! A scan only reads files that are new or whose size or mtime changed, so
//! re-scanning an unchanged library is cheap and re-analyzes nothing. Scans
//! run at startup, after file system events (debounced) and periodically,
//! as a safety net for file systems that do not report events (NFS, SMB).
//! The scanner never modifies files.
//!
//! `heron-librarian` moves files and rewrites tags, and records each change
//! in `tracks` itself. A scan holds the library lock, so it never sees a
//! change half done, and finds the files the librarian touched unchanged.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use heron_core::{FileStamp, LibraryPath};
use heron_library::details::{self, DETAILS_VERSION};
use heron_library::tags::FileTags;
use heron_library::{LibraryLock, is_audio, is_hidden};
use serde::Serialize;
use sqlx::{MySqlPool, Row};
use tokio::sync::mpsc;

/// Analysis jobs are queued every this many imported files during a scan.
const ENQUEUE_BATCH: u32 = 50;

/// Quiet time after the last file system event before rescanning.
const EVENT_DEBOUNCE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("the library directory {path} is not readable: {source}")]
    Root {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Analysis(#[from] heron_analysis::Error),
    #[error("scan task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Outcome of one scan, logged and served by `/api/v1/library/status`.
#[derive(Clone, Debug, Default, Serialize, utoipa::ToSchema)]
pub struct ScanReport {
    pub finished_at_unix: u64,
    pub duration_ms: u64,
    /// Audio files found on disk.
    pub files: u64,
    pub added: u64,
    pub updated: u64,
    /// Files found under a new path (same content as a missing track).
    pub moved: u64,
    pub unchanged: u64,
    /// Missing tracks whose file is back.
    pub restored: u64,
    /// Tracks newly marked missing.
    pub missing: u64,
    /// Files modified too recently (probably still being copied).
    pub deferred: u64,
    /// Unchanged files read again for details an older scanner did not
    /// read (bitrate, genre, …), without being analyzed again.
    pub refreshed: u64,
    /// Files that could not be read.
    pub failed: u64,
    /// Analysis jobs queued.
    pub enqueued: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct ScanOptions {
    /// Time between full scans when no event arrives.
    pub interval: Duration,
    /// Watch the library for changes.
    pub watch: bool,
    /// Files modified more recently than this are left for the next scan.
    pub settle: Duration,
}

/// The library scanner and the state it exposes.
pub struct Library {
    pool: MySqlPool,
    root: PathBuf,
    last_scan: Mutex<Option<ScanReport>>,
}

struct DiskFile {
    path: LibraryPath,
    stamp: FileStamp,
}

struct KnownTrack {
    id: u64,
    stamp: Option<FileStamp>,
    hash: Option<Vec<u8>>,
    missing: bool,
    /// Read by an older scanner: its details are read again.
    stale_details: bool,
}

/// What the scanner reads from a new or changed file.
struct FileInfo {
    hash: [u8; 32],
    tags: FileTags,
}

/// Library and analysis progress, served by `/api/v1/library/status`.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct LibraryStatus {
    /// Tracks whose file is present.
    pub tracks: u64,
    pub missing: u64,
    /// Present tracks with an up-to-date analysis.
    pub analyzed: u64,
    /// Analyzed tracks with nothing audible.
    pub silent: u64,
    pub analysis_queue: QueueStatus,
    pub last_scan: Option<ScanReport>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = AnalysisQueueStatus)]
pub struct QueueStatus {
    pub pending: u64,
    pub running: u64,
    pub failed: u64,
}

impl Library {
    pub async fn status(&self) -> Result<LibraryStatus, ScanError> {
        let settings = heron_analysis::jobs::load_settings(&self.pool).await?;
        let row = sqlx::query(
            "SELECT
                 CAST(COALESCE(SUM(t.missing_since IS NULL), 0) AS SIGNED) AS tracks,
                 CAST(COALESCE(SUM(t.missing_since IS NOT NULL), 0) AS SIGNED) AS missing,
                 CAST(COALESCE(SUM(t.missing_since IS NULL AND a.analysis_version = ?
                                   AND a.params_hash = ?), 0) AS SIGNED) AS analyzed,
                 CAST(COALESCE(SUM(t.missing_since IS NULL AND a.cue_out_ms <= a.cue_in_ms), 0)
                      AS SIGNED) AS silent
             FROM tracks t LEFT JOIN track_analysis a ON a.track_id = t.id
             WHERE t.deleted_at IS NULL",
        )
        .bind(heron_analysis::ALGORITHM_VERSION)
        .bind(settings.fingerprint().as_slice())
        .fetch_one(&self.pool)
        .await?;
        let count = |name: &str| -> Result<u64, sqlx::Error> {
            Ok(u64::try_from(row.try_get::<i64, _>(name)?).unwrap_or(0))
        };
        let queue = heron_analysis::jobs::counts(&self.pool).await?;
        Ok(LibraryStatus {
            tracks: count("tracks")?,
            missing: count("missing")?,
            analyzed: count("analyzed")?,
            silent: count("silent")?,
            analysis_queue: QueueStatus {
                pending: queue.pending,
                running: queue.running,
                failed: queue.failed,
            },
            last_scan: self.last_scan(),
        })
    }

    pub fn new(pool: MySqlPool, root: PathBuf) -> Self {
        Self {
            pool,
            root,
            last_scan: Mutex::new(None),
        }
    }

    /// The library root, as mounted in this container (read-only).
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn last_scan(&self) -> Option<ScanReport> {
        self.last_scan.lock().ok().and_then(|report| report.clone())
    }

    /// Scans, then rescans after file system events and every `interval`.
    /// Runs until the task is dropped.
    pub async fn run(self: Arc<Self>, options: ScanOptions) {
        let (events_tx, mut events) = mpsc::channel::<()>(1);
        let _watcher = if options.watch {
            self.watch(events_tx)
        } else {
            None
        };

        loop {
            let deferred = match self.scan(options.settle).await {
                Ok(report) => report.deferred,
                Err(error) => {
                    tracing::error!(%error, "library scan failed");
                    metrics::counter!("heron_library_scans_total", "result" => "error")
                        .increment(1);
                    0
                }
            };
            // Come back soon for files that were still being written.
            let wait = if deferred > 0 {
                options.settle + Duration::from_secs(1)
            } else {
                options.interval
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                Some(()) = events.recv() => {
                    // Debounce: wait for a quiet period, a copy sends many events.
                    while let Ok(Some(())) = tokio::time::timeout(EVENT_DEBOUNCE, events.recv()).await {}
                }
            }
        }
    }

    fn watch(&self, events: mpsc::Sender<()>) -> Option<notify::RecommendedWatcher> {
        let watcher = heron_library::watch::watch_dir(&self.root, move || {
            // A full channel already means "rescan pending".
            let _ = events.try_send(());
        });
        match watcher {
            Ok(watcher) => {
                tracing::info!(root = %self.root.display(), "watching the library for changes");
                Some(watcher)
            }
            Err(error) => {
                tracing::warn!(%error, "cannot watch the library, relying on periodic scans");
                None
            }
        }
    }

    /// One full scan, holding the library lock.
    pub async fn scan(&self, settle: Duration) -> Result<ScanReport, ScanError> {
        let lock = LibraryLock::acquire(&self.pool, "scanner").await?;
        let report = self.scan_locked(settle).await;
        lock.release().await?;
        report
    }

    async fn scan_locked(&self, settle: Duration) -> Result<ScanReport, ScanError> {
        let started = Instant::now();
        let mut known = self.known_tracks().await?;
        let in_flight = self.in_flight().await?;
        let root = self.root.clone();
        let files = tokio::task::spawn_blocking(move || walk(&root)).await??;
        let mut report = ScanReport {
            files: files.len() as u64,
            ..ScanReport::default()
        };

        let on_disk: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
        // Tracks whose file is gone, by content hash: candidates for moves.
        let mut vanished: HashMap<Vec<u8>, Vec<u64>> = HashMap::new();
        for (path, track) in &known {
            if let (false, Some(hash)) = (on_disk.contains(path.as_str()), &track.hash) {
                vanished.entry(hash.clone()).or_default().push(track.id);
            }
        }
        let mut moved_ids = HashSet::new();
        let settings = heron_analysis::jobs::load_settings(&self.pool).await?;
        // Known tracks first: after a settings change, re-analysis starts now.
        report.enqueued = heron_analysis::jobs::enqueue_stale(&self.pool, &settings).await?;
        let mut imported_since_enqueue = 0;
        let settle_before = now_us().saturating_sub(i64::try_from(settle.as_micros()).unwrap_or(0));

        for file in &files {
            // A move the librarian has not finished (it crashed): recovery
            // decides which of the two files is the track's.
            if in_flight.contains(file.path.as_str()) {
                tracing::debug!(path = %file.path, "being moved by the librarian, skipped");
                continue;
            }
            match known.get(file.path.as_str()) {
                // Including files the librarian just wrote: it recorded their stamp.
                Some(track) if track.stamp == Some(file.stamp) => {
                    if track.missing {
                        self.set_missing(track.id, false).await?;
                        report.restored += 1;
                    } else {
                        report.unchanged += 1;
                    }
                    if track.stale_details && self.refresh(track.id, &file.path).await? {
                        report.refreshed += 1;
                    }
                }
                _ if file.stamp.mtime_us > settle_before => report.deferred += 1,
                existing => {
                    let existing = existing.map(|track| track.id);
                    let Some(info) = self.read(&file.path).await? else {
                        report.failed += 1;
                        continue;
                    };
                    match existing {
                        Some(id) => {
                            self.update(id, file, &info, true).await?;
                            report.updated += 1;
                        }
                        None => {
                            let moved_from = vanished.get_mut(&info.hash[..]).and_then(Vec::pop);
                            if let Some(id) = moved_from {
                                self.update_path(id, file, &info).await?;
                                moved_ids.insert(id);
                                report.moved += 1;
                            } else {
                                self.insert(file, &info).await?;
                                report.added += 1;
                            }
                        }
                    }
                    // Feed the analyzers during long imports instead of at the end.
                    imported_since_enqueue += 1;
                    if imported_since_enqueue >= ENQUEUE_BATCH {
                        report.enqueued +=
                            heron_analysis::jobs::enqueue_stale(&self.pool, &settings).await?;
                        imported_since_enqueue = 0;
                    }
                }
            }
        }

        // Files that vanished are marked missing, unless the whole library
        // looks unmounted: an empty directory must not "lose" every track.
        known.retain(|path, track| {
            !on_disk.contains(path.as_str()) && !track.missing && !moved_ids.contains(&track.id)
        });
        if files.is_empty() && !known.is_empty() {
            tracing::error!(
                root = %self.root.display(),
                "the library directory is empty, not marking tracks missing (unmounted volume?)"
            );
        } else {
            for track in known.values() {
                self.set_missing(track.id, true).await?;
                report.missing += 1;
            }
        }

        report.enqueued += heron_analysis::jobs::enqueue_stale(&self.pool, &settings).await?;

        report.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        report.finished_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        tracing::info!(
            files = report.files,
            added = report.added,
            updated = report.updated,
            moved = report.moved,
            restored = report.restored,
            missing = report.missing,
            deferred = report.deferred,
            refreshed = report.refreshed,
            failed = report.failed,
            enqueued = report.enqueued,
            duration_ms = report.duration_ms,
            "library scan finished"
        );
        metrics::counter!("heron_library_scans_total", "result" => "ok").increment(1);
        metrics::gauge!("heron_library_scan_duration_seconds").set(started.elapsed().as_secs_f64());
        metrics::gauge!("heron_library_files").set(report.files as f64);
        if let Ok(mut last) = self.last_scan.lock() {
            *last = Some(report.clone());
        }
        Ok(report)
    }

    async fn known_tracks(&self) -> Result<HashMap<String, KnownTrack>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, path, file_size, file_mtime_us, file_hash,
                    missing_since IS NOT NULL AS missing, details_version < ? AS stale_details
             FROM tracks WHERE deleted_at IS NULL",
        )
        .bind(DETAILS_VERSION)
        .fetch_all(&self.pool)
        .await?;
        let mut tracks = HashMap::with_capacity(rows.len());
        for row in rows {
            let size: Option<u64> = row.try_get("file_size")?;
            let mtime_us: Option<i64> = row.try_get("file_mtime_us")?;
            tracks.insert(
                row.try_get("path")?,
                KnownTrack {
                    id: row.try_get("id")?,
                    stamp: size
                        .zip(mtime_us)
                        .map(|(size, mtime_us)| FileStamp { size, mtime_us }),
                    hash: row.try_get("file_hash")?,
                    missing: row.try_get("missing")?,
                    stale_details: row.try_get("stale_details")?,
                },
            );
        }
        Ok(tracks)
    }

    /// Paths of moves the librarian has not finished: the target of a move
    /// in progress, and the original of a copy not removed yet.
    async fn in_flight(&self) -> Result<HashSet<String>, sqlx::Error> {
        let paths: Vec<String> = sqlx::query_scalar(
            "SELECT to_path FROM library_moves WHERE status = 'pending'
             UNION SELECT from_path FROM library_moves
             WHERE status = 'moved' AND from_root = 'library'",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(paths.into_iter().collect())
    }

    /// Hashes the file and reads its tags. `None` if it cannot be read.
    async fn read(&self, path: &LibraryPath) -> Result<Option<FileInfo>, ScanError> {
        let absolute = path.resolve(&self.root);
        let info = tokio::task::spawn_blocking(move || read_file(&absolute)).await?;
        match info {
            Ok(info) => Ok(Some(info)),
            Err(error) => {
                tracing::warn!(%path, %error, "cannot read file");
                Ok(None)
            }
        }
    }

    async fn insert(&self, file: &DiskFile, info: &FileInfo) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let id = sqlx::query(
            "INSERT INTO tracks (path, file_hash, file_size, file_mtime_us) VALUES (?, ?, ?, ?)",
        )
        .bind(file.path.as_str())
        .bind(&info.hash[..])
        .bind(file.stamp.size)
        .bind(file.stamp.mtime_us)
        .execute(&mut *tx)
        .await?
        .last_insert_id();
        details::save(&mut tx, id, &info.tags).await?;
        tx.commit().await
    }

    async fn update(
        &self,
        id: u64,
        file: &DiskFile,
        info: &FileInfo,
        file_changed: bool,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE tracks
             SET file_hash = ?, file_size = ?, file_mtime_us = ?, missing_since = NULL
             WHERE id = ?",
        )
        .bind(&info.hash[..])
        .bind(file.stamp.size)
        .bind(file.stamp.mtime_us)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        details::save(&mut tx, id, &info.tags).await?;
        // The file changed: its cue points may be wrong, analyze it again.
        if file_changed {
            sqlx::query("DELETE FROM track_analysis WHERE track_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    /// Reads the details of an unchanged file again. Returns whether it
    /// could be read.
    async fn refresh(&self, id: u64, path: &LibraryPath) -> Result<bool, ScanError> {
        let absolute = path.resolve(&self.root);
        let read =
            tokio::task::spawn_blocking(move || heron_library::tags::read(&absolute)).await?;
        match read {
            Ok(read) => {
                let mut connection = self.pool.acquire().await?;
                details::save(&mut connection, id, &read).await?;
                Ok(true)
            }
            Err(error) => {
                tracing::warn!(%path, %error, "cannot read tags");
                // Not read again at every scan: the file is unreadable.
                sqlx::query("UPDATE tracks SET details_version = ? WHERE id = ?")
                    .bind(DETAILS_VERSION)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok(false)
            }
        }
    }

    async fn update_path(
        &self,
        id: u64,
        file: &DiskFile,
        info: &FileInfo,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE tracks SET path = ? WHERE id = ?")
            .bind(file.path.as_str())
            .bind(id)
            .execute(&self.pool)
            .await?;
        // Same file under a new name: the analysis stays valid.
        self.update(id, file, info, false).await
    }

    async fn set_missing(&self, id: u64, missing: bool) -> Result<(), sqlx::Error> {
        let query = if missing {
            "UPDATE tracks SET missing_since = NOW(6) WHERE id = ?"
        } else {
            "UPDATE tracks SET missing_since = NULL WHERE id = ?"
        };
        sqlx::query(query).bind(id).execute(&self.pool).await?;
        Ok(())
    }
}

/// Lists the audio files under `root`. Hidden files and directories
/// (including macOS `._` resource forks) are skipped.
fn walk(root: &Path) -> Result<Vec<DiskFile>, ScanError> {
    let root_error = |source| ScanError::Root {
        path: root.to_path_buf(),
        source,
    };
    let metadata = std::fs::metadata(root).map_err(root_error)?;
    if !metadata.is_dir() {
        return Err(root_error(std::io::Error::other("not a directory")));
    }

    let mut files = Vec::new();
    let entries = walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_hidden(entry.file_name()));
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(%error, "cannot read part of the library");
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_audio(entry.path()) {
            continue;
        }
        let Some(path) = LibraryPath::from_file(root, entry.path()) else {
            tracing::warn!(path = %entry.path().display(), "skipping file with a non UTF-8 name");
            continue;
        };
        match entry.metadata() {
            Ok(metadata) => files.push(DiskFile {
                path,
                stamp: FileStamp::from_metadata(&metadata),
            }),
            Err(error) => tracing::warn!(%path, %error, "cannot stat file"),
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn read_file(path: &Path) -> std::io::Result<FileInfo> {
    let hash = heron_library::content_hash(path)?;
    // Unreadable tags are not fatal: the track is still imported and analyzed.
    let read = heron_library::tags::read(path).unwrap_or_else(|error| {
        tracing::warn!(path = %path.display(), %error, "cannot read tags");
        FileTags::default()
    });
    Ok(FileInfo { hash, tags: read })
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use heron_service::db::ScratchDatabase;

    fn write_wav(path: &Path, seconds: f64, freq: f64) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for n in 0..(seconds * 8_000.0) as u32 {
            let t = f64::from(n) / 8_000.0;
            let sample = (0.3 * (std::f64::consts::TAU * freq * t).sin() * 32_767.0) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    async fn scan(library: &Library) -> ScanReport {
        library.scan(Duration::ZERO).await.unwrap()
    }

    async fn track_count(pool: &MySqlPool, condition: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM tracks WHERE {condition}"
        )))
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn imports_tracks_and_follows_changes() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_wav(&root.join("Artist/Album/01 One.wav"), 1.0, 440.0);
        write_wav(&root.join("Artist/Album/02 Two.WAV"), 1.0, 550.0);
        write_wav(&root.join("Artist/Album/._01 One.wav"), 0.1, 440.0);
        write_wav(&root.join(".hidden/secret.wav"), 0.1, 440.0);
        std::fs::write(root.join("Artist/Album/cover.jpg"), b"jpeg").unwrap();
        let library = Library::new(db.pool.clone(), root.to_path_buf());

        let first = scan(&library).await;
        assert_eq!(
            (first.files, first.added, first.enqueued),
            (2, 2, 2),
            "{first:?}"
        );

        let again = scan(&library).await;
        assert_eq!(
            (again.unchanged, again.added, again.enqueued),
            (2, 0, 0),
            "{again:?}"
        );

        write_wav(&root.join("Artist/Album/01 One.wav"), 2.0, 440.0);
        let changed = scan(&library).await;
        assert_eq!(changed.updated, 1, "{changed:?}");

        std::fs::rename(
            root.join("Artist/Album/02 Two.WAV"),
            root.join("Artist/Album/02 Two (renamed).WAV"),
        )
        .unwrap();
        let moved = scan(&library).await;
        assert_eq!(
            (moved.moved, moved.added, moved.missing),
            (1, 0, 0),
            "{moved:?}"
        );
        assert_eq!(track_count(&db.pool, "path LIKE '%(renamed)%'").await, 1);

        std::fs::remove_file(root.join("Artist/Album/01 One.wav")).unwrap();
        let removed = scan(&library).await;
        assert_eq!(removed.missing, 1, "{removed:?}");
        assert_eq!(track_count(&db.pool, "missing_since IS NOT NULL").await, 1);

        write_wav(&root.join("Artist/Album/01 One.wav"), 2.0, 440.0);
        let restored = scan(&library).await;
        assert_eq!(restored.restored + restored.updated, 1, "{restored:?}");
        assert_eq!(track_count(&db.pool, "missing_since IS NOT NULL").await, 0);
        db.drop().await;
    }

    #[tokio::test]
    async fn tracks_scanned_before_m5b_get_their_details_without_analysis() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        write_wav(&dir.path().join("a.wav"), 1.0, 440.0);
        let library = Library::new(db.pool.clone(), dir.path().to_path_buf());
        scan(&library).await;
        let details: (Option<String>, Option<u32>, Option<u32>, Option<u8>) =
            sqlx::query_as("SELECT codec, bitrate_kbps, sample_rate, channels FROM tracks")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            details,
            (Some("PCM".into()), Some(128), Some(8_000), Some(1))
        );

        // As an older scanner left it, analyzed.
        sqlx::query("UPDATE tracks SET codec = NULL, bitrate_kbps = NULL, details_version = 0")
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO track_analysis (track_id, analysis_version, params_hash, cue_in_ms, cue_out_ms)
             SELECT id, 1, '', 0, 1000 FROM tracks",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        let report = scan(&library).await;
        assert_eq!(
            (report.refreshed, report.unchanged, report.updated),
            (1, 1, 0),
            "{report:?}"
        );
        assert_eq!(
            track_count(&db.pool, "codec = 'PCM' AND bitrate_kbps = 128").await,
            1
        );
        let analyzed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_analysis")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(analyzed, 1, "not analyzed again");
        assert_eq!(scan(&library).await.refreshed, 0, "read once");
        db.drop().await;
    }

    #[tokio::test]
    async fn files_the_librarian_is_moving_are_left_alone() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_wav(&root.join("old/a.wav"), 1.0, 440.0);
        let library = Library::new(db.pool.clone(), root.to_path_buf());
        scan(&library).await;

        // A copy landed at the new path, then the librarian crashed.
        std::fs::create_dir_all(root.join("new")).unwrap();
        std::fs::copy(root.join("old/a.wav"), root.join("new/a.wav")).unwrap();
        sqlx::query("INSERT INTO library_batches (id, kind) VALUES (1, 'organize')")
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO library_moves (batch_id, track_id, from_root, from_path, to_path, method)
             SELECT 1, id, 'library', path, 'new/a.wav', 'copy' FROM tracks",
        )
        .execute(&db.pool)
        .await
        .unwrap();
        let report = scan(&library).await;
        assert_eq!((report.added, report.unchanged), (0, 1), "{report:?}");
        assert_eq!(track_count(&db.pool, "1").await, 1, "not imported twice");
        db.drop().await;
    }

    #[tokio::test]
    async fn an_empty_library_does_not_mark_everything_missing() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        write_wav(&dir.path().join("a.wav"), 0.5, 440.0);
        let library = Library::new(db.pool.clone(), dir.path().to_path_buf());
        scan(&library).await;

        std::fs::remove_file(dir.path().join("a.wav")).unwrap();
        let report = scan(&library).await;
        assert_eq!(report.missing, 0, "looks like an unmounted volume");
        db.drop().await;
    }

    #[tokio::test]
    async fn an_unreadable_root_is_an_error() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let library = Library::new(db.pool.clone(), PathBuf::from("/nonexistent/library"));
        assert!(matches!(
            library.scan(Duration::ZERO).await,
            Err(ScanError::Root { .. })
        ));
        db.drop().await;
    }
}
