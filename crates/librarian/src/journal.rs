// SPDX-License-Identifier: AGPL-3.0-or-later

//! Journaled file operations: every move and tag write is recorded in
//! `library_moves` before the file is touched, and the track is updated in
//! the same transaction that marks the file in place. An interrupted
//! operation is finished or rolled back by [`recover`] on restart.
//!
//! One operation moves a file to a new path in the library, writes new tags
//! into it, or both:
//!
//! 1. Record the operation (`pending`). An imported file also gets its
//!    track, marked missing until the file is in place.
//! 2. Move the file without ever replacing another one. A rename when only
//!    the path changes; otherwise (tags to write, or another file system) a
//!    copy into a hidden temporary file next to the target, verified, then
//!    renamed into place.
//! 3. Point the track at the new file and record its stamp (`moved`, or
//!    `done` after a rename).
//! 4. Remove the original of a copy (`done`).
//!
//! A move into the trash (`.heron-trash/` at the library root) marks the
//! track deleted in step 3, and a move out of it restores the track, so a
//! track is never in the trash without its file, or out of it with its
//! file still there.
//!
//! Callers hold the [`heron_library::LibraryLock`] around each operation.
//!
//! A tag write also records how to rebuild the file as it was
//! ([`crate::restore`]), so undoing it gives back the same bytes.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use heron_analysis::Chromaprint;
use heron_core::library::{FieldValues, in_trash};
use heron_core::{FileStamp, LibraryPath};
use heron_library::Tags;
use heron_library::tags::{self, Cover};
use serde::Serialize;
use sqlx::{MySqlPool, Row};

use crate::fsops;
use crate::restore::{self, Delta};

/// The library, the incoming folder and the buffer zone, as mounted in
/// this container.
#[derive(Clone, Debug)]
pub struct Roots {
    pub library: PathBuf,
    pub incoming: PathBuf,
    pub staging: PathBuf,
}

/// Where a file comes from. Paths are relative to their root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Incoming(LibraryPath),
    Library(LibraryPath),
    /// The buffer zone.
    Staging(LibraryPath),
}

impl Origin {
    fn root_name(&self) -> &'static str {
        match self {
            Self::Incoming(_) => "incoming",
            Self::Library(_) => "library",
            Self::Staging(_) => "staging",
        }
    }

    fn from_stored(root: &str, path: LibraryPath) -> Self {
        match root {
            "incoming" => Self::Incoming(path),
            "staging" => Self::Staging(path),
            _ => Self::Library(path),
        }
    }

    fn path(&self) -> &LibraryPath {
        match self {
            Self::Incoming(path) | Self::Library(path) | Self::Staging(path) => path,
        }
    }

    fn resolve(&self, roots: &Roots) -> PathBuf {
        self.path().resolve(self.root(roots))
    }

    fn root<'a>(&self, roots: &'a Roots) -> &'a Path {
        match self {
            Self::Incoming(_) => &roots.incoming,
            Self::Library(_) => &roots.library,
            Self::Staging(_) => &roots.staging,
        }
    }
}

/// The track an operation applies to.
#[derive(Clone, Debug)]
pub enum Subject {
    Existing(u64),
    /// A file imported from the buffer zone: its track is created.
    New(Box<NewTrack>),
}

#[derive(Clone, Debug)]
pub struct NewTrack {
    /// The tags the file has once imported.
    pub tags: Tags,
    pub duration_ms: Option<u32>,
    pub chromaprint: Option<Chromaprint>,
    pub identification: Option<Identification>,
    /// The buffer file whose categories, labels, mood and sound code the
    /// track gets.
    pub prepared_from: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentificationStatus {
    Identified,
    NeedsReview,
    /// Reviewed: the file keeps its own tags.
    Kept,
}

impl IdentificationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Identified => "identified",
            Self::NeedsReview => "needs_review",
            Self::Kept => "kept",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Identification {
    pub status: IdentificationStatus,
    pub reason: Option<String>,
    pub score: Option<f64>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub candidates: Option<serde_json::Value>,
}

/// Tags to write, with the ones they replace (kept to undo the change).
#[derive(Clone, Debug)]
pub struct Retag {
    pub change: TagChange,
    pub cover: RetagCover,
    /// Undoing a change: the file is rebuilt as it was before it, byte for
    /// byte, instead of having the tags written.
    pub restore: Option<Box<Delta>>,
}

// One per file operation: not worth boxing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagChange {
    /// The tags identification writes ([`Tags`]).
    Tags { previous: Tags, applied: Tags },
    /// Fields edited from the browser: only these are written.
    Fields {
        previous: FieldValues,
        applied: FieldValues,
    },
}

impl TagChange {
    /// The value of `track_tag_changes.kind`.
    fn kind(&self) -> &'static str {
        match self {
            Self::Tags { .. } => "tags",
            Self::Fields { .. } => "fields",
        }
    }

    /// The `previous` and `applied` columns.
    fn json(&self) -> (String, String) {
        let json = |value: Result<String, serde_json::Error>| value.unwrap_or_default();
        match self {
            Self::Tags { previous, applied } => (
                json(serde_json::to_string(previous)),
                json(serde_json::to_string(applied)),
            ),
            Self::Fields { previous, applied } => (
                json(serde_json::to_string(previous)),
                json(serde_json::to_string(applied)),
            ),
        }
    }

    fn write(&self, path: &Path, cover: Cover<'_>) -> Result<(), tags::TagError> {
        match self {
            Self::Tags { applied, .. } => tags::write(path, applied, cover),
            Self::Fields { applied, .. } => tags::write_fields(path, applied),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetagCover {
    Keep,
    /// Embed this JPEG (only done when the file has no cover).
    Embed(Vec<u8>),
    /// Remove a cover embedded by an earlier change (undo).
    Remove,
}

#[derive(Clone, Debug)]
pub struct Operation {
    pub batch_id: u64,
    pub subject: Subject,
    pub from: Origin,
    pub to: LibraryPath,
    pub retag: Option<Retag>,
    /// Stamp of the source when it was read: the operation is refused if
    /// the file changed since.
    pub expected_stamp: Option<FileStamp>,
}

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("{0} already exists")]
    TargetTaken(LibraryPath),
    #[error("the file changed since it was read")]
    SourceChanged,
    #[error("the file is gone")]
    SourceMissing,
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Tags(#[from] tags::TagError),
}

/// Kinds of batches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchKind {
    Import,
    Organize,
    Undo,
    /// A release picked on the review screen.
    Review,
    /// Tags edited from the browser.
    Edit,
    /// Tracks moved to the trash.
    Delete,
    /// Tracks removed from the trash for good.
    Purge,
}

impl BatchKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Organize => "organize",
            Self::Undo => "undo",
            Self::Review => "review",
            Self::Edit => "edit",
            Self::Delete => "delete",
            Self::Purge => "purge",
        }
    }
}

pub async fn create_batch(
    pool: &MySqlPool,
    kind: BatchKind,
    undoes: Option<u64>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("INSERT INTO library_batches (kind, undoes) VALUES (?, ?)")
        .bind(kind.as_str())
        .bind(undoes)
        .execute(pool)
        .await?;
    Ok(result.last_insert_id())
}

pub async fn finish_batch(pool: &MySqlPool, id: u64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE library_batches SET finished_at = NOW(6) WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Whether `path` is free in the library: no file there, and no track
/// (even a missing one) recorded there.
pub async fn is_free(
    pool: &MySqlPool,
    roots: &Roots,
    path: &LibraryPath,
) -> Result<bool, sqlx::Error> {
    if std::fs::symlink_metadata(path.resolve(&roots.library)).is_ok() {
        return Ok(false);
    }
    let taken: Option<i32> = sqlx::query_scalar("SELECT 1 FROM tracks WHERE path = ? LIMIT 1")
        .bind(path.as_str())
        .fetch_optional(pool)
        .await?;
    Ok(taken.is_none())
}

/// Runs one operation. Returns the track's ID. Hold the library lock.
pub async fn execute(pool: &MySqlPool, roots: &Roots, op: Operation) -> Result<u64, OpError> {
    let from = op.from.resolve(roots);
    let to = op.to.resolve(&roots.library);
    let in_place = op.from == Origin::Library(op.to.clone());

    let source = match std::fs::metadata(&from) {
        Ok(metadata) => FileStamp::from_metadata(&metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => return Err(OpError::SourceMissing),
        Err(error) => return Err(error.into()),
    };
    if op.expected_stamp.is_some_and(|expected| expected != source) {
        return Err(OpError::SourceChanged);
    }
    if !in_place && !is_free(pool, roots, &op.to).await? {
        return Err(OpError::TargetTaken(op.to));
    }

    let copy = op.retag.is_some();
    let (move_id, track_id) = record(pool, &op, copy).await?;
    let temp = temp_path(&op.to, move_id);
    crash_point("recorded");

    let transferred = transfer(
        pool, roots, &op, move_id, &from, &to, &temp, source, in_place,
    )
    .await;
    if transferred.is_ok() {
        crash_point("transferred");
    }
    let read_details = op.retag.is_some() || matches!(op.subject, Subject::New(_));
    match transferred {
        Ok(Transfer::Renamed) => {
            finish(pool, move_id, track_id, &op.to, &to, read_details, "done").await?;
        }
        Ok(Transfer::Copied) => {
            if in_place {
                finish(pool, move_id, track_id, &op.to, &to, read_details, "done").await?;
            } else {
                finish(pool, move_id, track_id, &op.to, &to, read_details, "moved").await?;
                crash_point("finished");
                remove_original(&from)?;
                crash_point("removed");
                set_status(pool, move_id, "done", None).await?;
            }
        }
        Err(error) => {
            let _ = std::fs::remove_file(temp.resolve(&roots.library));
            abandon(pool, move_id, &error.to_string()).await?;
            return Err(error);
        }
    }
    if !in_place && let Some(parent) = from.parent() {
        fsops::remove_empty_dirs(parent, op.from.root(roots));
    }
    Ok(track_id)
}

/// The hidden file a copy is written to, next to its target.
fn temp_path(to: &LibraryPath, move_id: u64) -> LibraryPath {
    let extension = to
        .as_str()
        .rsplit_once('.')
        .map_or("tmp", |(_, extension)| extension);
    let name = format!(".heron-{move_id}.{extension}");
    LibraryPath::from_stored(match to.as_str().rsplit_once('/') {
        Some((dirs, _)) => format!("{dirs}/{name}"),
        None => name,
    })
}

/// Step 1: journal the operation (and create an imported file's track).
async fn record(pool: &MySqlPool, op: &Operation, copy: bool) -> Result<(u64, u64), OpError> {
    let mut tx = pool.begin().await?;
    let existing = match &op.subject {
        Subject::Existing(id) => Some(*id),
        Subject::New(_) => None,
    };
    let move_id = sqlx::query(
        "INSERT INTO library_moves (batch_id, track_id, from_root, from_path, to_path, method)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(op.batch_id)
    .bind(existing)
    .bind(op.from.root_name())
    .bind(op.from.path().as_str())
    .bind(op.to.as_str())
    .bind(if copy { "copy" } else { "rename" })
    .execute(&mut *tx)
    .await?
    .last_insert_id();
    if copy {
        sqlx::query("UPDATE library_moves SET temp_path = ? WHERE id = ?")
            .bind(temp_path(&op.to, move_id).as_str())
            .bind(move_id)
            .execute(&mut *tx)
            .await?;
    }

    let track_id = match &op.subject {
        Subject::Existing(id) => *id,
        Subject::New(track) => {
            // Missing until the file is in place: never played, never analyzed.
            let id = sqlx::query(
                "INSERT INTO tracks
                     (path, title, artist, album, musicbrainz_artist_id, duration_ms, missing_since)
                 VALUES (?, ?, ?, ?, ?, ?, NOW(6))",
            )
            .bind(op.to.as_str())
            .bind(&track.tags.title)
            .bind(&track.tags.artist)
            .bind(&track.tags.album)
            .bind(track.tags.musicbrainz.first_artist())
            .bind(track.duration_ms)
            .execute(&mut *tx)
            .await?
            .last_insert_id();
            sqlx::query("UPDATE library_moves SET track_id = ? WHERE id = ?")
                .bind(id)
                .bind(move_id)
                .execute(&mut *tx)
                .await?;
            if let Some(chromaprint) = &track.chromaprint {
                sqlx::query("INSERT INTO track_fingerprints (track_id, chromaprint) VALUES (?, ?)")
                    .bind(id)
                    .bind(chromaprint.as_bytes())
                    .execute(&mut *tx)
                    .await?;
            }
            if let Some(identification) = &track.identification {
                save_identification(&mut tx, id, identification).await?;
            }
            if let Some(file_id) = track.prepared_from {
                give_preparation(&mut tx, id, file_id).await?;
            }
            id
        }
    };

    if let Some(retag) = &op.retag {
        let (previous, applied) = retag.change.json();
        sqlx::query(
            "INSERT INTO track_tag_changes
                 (track_id, move_id, kind, previous, applied, cover_embedded)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(track_id)
        .bind(move_id)
        .bind(retag.change.kind())
        .bind(previous)
        .bind(applied)
        .bind(matches!(retag.cover, RetagCover::Embed(_)))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok((move_id, track_id))
}

pub async fn save_identification(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    track_id: u64,
    identification: &Identification,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO track_identifications
             (track_id, status, reason, score, recording_mbid, release_mbid, candidates)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE status = VALUES(status), reason = VALUES(reason),
             score = VALUES(score), recording_mbid = VALUES(recording_mbid),
             release_mbid = VALUES(release_mbid), candidates = VALUES(candidates)",
    )
    .bind(track_id)
    .bind(identification.status.as_str())
    .bind(identification.reason.as_deref().map(|reason| {
        let reason: String = reason.chars().take(255).collect();
        reason
    }))
    .bind(identification.score)
    .bind(&identification.recording_mbid)
    .bind(&identification.release_mbid)
    .bind(
        identification
            .candidates
            .as_ref()
            .map(serde_json::Value::to_string),
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

enum Transfer {
    Renamed,
    Copied,
}

/// Step 2: put the file at its new path (or its new tags into it).
#[allow(clippy::too_many_arguments)]
async fn transfer(
    pool: &MySqlPool,
    roots: &Roots,
    op: &Operation,
    move_id: u64,
    from: &Path,
    to: &Path,
    temp: &LibraryPath,
    source: FileStamp,
    in_place: bool,
) -> Result<Transfer, OpError> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if op.retag.is_none() {
        let (from_owned, to_owned) = (from.to_path_buf(), to.to_path_buf());
        let renamed = blocking(move || fsops::rename_no_replace(&from_owned, &to_owned)).await;
        match renamed {
            Ok(()) => {
                if let Some(parent) = to.parent() {
                    fsops::sync_dir(parent);
                }
                return Ok(Transfer::Renamed);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::CrossesDevices | ErrorKind::Unsupported
                ) =>
            {
                // Another file system: copy, verify, then remove the original.
                sqlx::query("UPDATE library_moves SET method = 'copy', temp_path = ? WHERE id = ?")
                    .bind(temp.as_str())
                    .bind(move_id)
                    .execute(pool)
                    .await?;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return Err(OpError::TargetTaken(op.to.clone()));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(OpError::SourceMissing);
            }
            Err(error) => return Err(error.into()),
        }
    }

    let temp_abs = temp.resolve(&roots.library);
    let retag = op.retag.clone();
    let (from_owned, temp_owned) = (from.to_path_buf(), temp_abs.clone());
    let (content_hash, delta) =
        blocking(move || write_copy(&from_owned, &temp_owned, retag.as_ref())).await?;
    sqlx::query("UPDATE library_moves SET content_hash = ? WHERE id = ?")
        .bind(&content_hash[..])
        .bind(move_id)
        .execute(pool)
        .await?;
    if let Some(delta) = delta {
        sqlx::query("UPDATE track_tag_changes SET restore = ? WHERE move_id = ?")
            .bind(delta)
            .bind(move_id)
            .execute(pool)
            .await?;
    }

    // The original changed while it was copied: the copy is not the file.
    if FileStamp::from_metadata(&std::fs::metadata(from)?) != source {
        return Err(OpError::SourceChanged);
    }
    if in_place {
        // The one replacement Heron does: the file by its own re-tagged copy.
        std::fs::rename(&temp_abs, to)?;
    } else {
        match fsops::rename_no_replace(&temp_abs, to) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return Err(OpError::TargetTaken(op.to.clone()));
            }
            Err(error) => return Err(error.into()),
        }
    }
    if let Some(parent) = to.parent() {
        fsops::sync_dir(parent);
    }
    Ok(Transfer::Copied)
}

/// Writes the copy of `from` at `temp`: the file with its new tags, or
/// rebuilt as it was (undo). Returns the copy's hash, and the delta that
/// rebuilds `from` from it, when the tags changed and the files are small
/// enough.
fn write_copy(
    from: &Path,
    temp: &Path,
    retag: Option<&Retag>,
) -> Result<([u8; 32], Option<Vec<u8>>), OpError> {
    let Some(retag) = retag else {
        return Ok((fsops::copy_verified(from, temp)?, None));
    };
    let small = std::fs::metadata(from)?.len() <= restore::MAX_FILE_BYTES;
    match &retag.restore {
        Some(delta) => {
            let current = std::fs::read(from)?;
            let rebuilt = delta
                .apply(&current)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            write_new(temp, &rebuilt)?;
        }
        None => {
            fsops::copy_verified(from, temp)?;
            let cover = match &retag.cover {
                RetagCover::Keep => Cover::Keep,
                RetagCover::Embed(jpeg) => Cover::Embed(jpeg),
                RetagCover::Remove => Cover::Remove,
            };
            retag.change.write(temp, cover)?;
        }
    }
    fsops::sync_file(temp)?;
    let written = fsops::full_hash(temp)?;
    let delta = if small && std::fs::metadata(temp)?.len() <= restore::MAX_FILE_BYTES {
        let (original, new) = (std::fs::read(from)?, std::fs::read(temp)?);
        let delta = Delta::compute(&original, &new).to_bytes();
        if delta.len() <= restore::MAX_DELTA_BYTES {
            Some(delta)
        } else {
            tracing::warn!(path = %from.display(), bytes = delta.len(),
                "the tag change rewrote too much of the file: undoing it will write the tags back");
            None
        }
    } else {
        None
    };
    Ok((written, delta))
}

/// Writes `bytes` into the new file `path` (which must not exist).
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Step 3: the track points at the file in place, with its new stamp, and
/// the tags and properties read from it when they may have changed.
async fn finish(
    pool: &MySqlPool,
    move_id: u64,
    track_id: u64,
    to: &LibraryPath,
    to_abs: &Path,
    read_details: bool,
    status: &str,
) -> Result<(), OpError> {
    let stamp = FileStamp::from_metadata(&std::fs::metadata(to_abs)?);
    let path = to_abs.to_path_buf();
    let hash = blocking(move || heron_library::content_hash(&path)).await?;
    let details = if read_details {
        let path = to_abs.to_path_buf();
        let read = blocking(move || Ok::<_, std::io::Error>(tags::read(&path))).await?;
        read.map_err(|error| tracing::warn!(path = %to, %error, "cannot read the tags back"))
            .ok()
    } else {
        None
    };
    let mut tx = pool.begin().await?;
    // In the trash, a track is deleted and nothing plays it; out of it, it
    // is back.
    let query = if in_trash(to.as_str()) {
        "UPDATE tracks SET path = ?, file_hash = ?, file_size = ?, file_mtime_us = ?,
             missing_since = COALESCE(missing_since, NOW(6)),
             deleted_at = COALESCE(deleted_at, NOW(6))
         WHERE id = ?"
    } else {
        "UPDATE tracks SET path = ?, file_hash = ?, file_size = ?, file_mtime_us = ?,
             missing_since = NULL, deleted_at = NULL
         WHERE id = ?"
    };
    sqlx::query(query)
        .bind(to.as_str())
        .bind(&hash[..])
        .bind(stamp.size)
        .bind(stamp.mtime_us)
        .bind(track_id)
        .execute(&mut *tx)
        .await?;
    if let Some(details) = &details {
        // In the trash, a file's folder is not its album's.
        let folder = (!in_trash(to.as_str())).then_some(to_abs);
        heron_library::details::save(&mut tx, track_id, details, folder).await?;
    }
    sqlx::query("UPDATE library_moves SET status = ?, error = NULL WHERE id = ?")
        .bind(status)
        .bind(move_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Step 4, after a verified copy.
fn remove_original(from: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(from) {
        Ok(()) => {
            if let Some(parent) = from.parent() {
                fsops::sync_dir(parent);
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn set_status(
    pool: &MySqlPool,
    move_id: u64,
    status: &str,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE library_moves SET status = ?, error = ? WHERE id = ?")
        .bind(status)
        .bind(error)
        .bind(move_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Nothing changed on disk: forget the operation, and the track it created.
async fn abandon(pool: &MySqlPool, move_id: u64, reason: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT m.track_id, CAST(m.from_root AS CHAR) AS from_root,
                t.file_size IS NULL AS created
         FROM library_moves m LEFT JOIN tracks t ON t.id = m.track_id
         WHERE m.id = ? FOR UPDATE",
    )
    .bind(move_id)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM track_tag_changes WHERE move_id = ?")
        .bind(move_id)
        .execute(&mut *tx)
        .await?;
    if let Some(row) = row {
        let track_id: Option<u64> = row.try_get("track_id")?;
        let from_root: String = row.try_get("from_root")?;
        let created: Option<bool> = row.try_get("created")?;
        // An import's track never had a file: it never existed.
        if let (Some(track_id), "incoming" | "staging", Some(true)) =
            (track_id, from_root.as_str(), created)
        {
            sqlx::query("DELETE FROM tracks WHERE id = ?")
                .bind(track_id)
                .execute(&mut *tx)
                .await?;
        }
    }
    let reason: String = reason.chars().take(1_000).collect();
    sqlx::query("UPDATE library_moves SET status = 'abandoned', error = ? WHERE id = ?")
        .bind(reason)
        .bind(move_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Gives a new track what its buffer file was prepared with, in the
/// transaction that creates it. Categories and labels deleted since are
/// gone from the file already.
async fn give_preparation(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    track_id: u64,
    file_id: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO track_categories (track_id, category_id, weight)
         SELECT ?, category_id, weight FROM staged_file_categories WHERE staged_file_id = ?",
    )
    .bind(track_id)
    .bind(file_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO track_labels (track_id, label_id)
         SELECT ?, label_id FROM staged_file_labels WHERE staged_file_id = ?",
    )
    .bind(track_id)
    .bind(file_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO track_programming (track_id, mood, sound_code)
         SELECT ?, mood, sound_code FROM staged_files
         WHERE id = ? AND (mood IS NOT NULL OR sound_code IS NOT NULL)",
    )
    .bind(track_id)
    .bind(file_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Moves a file that is not a track (a cover, a playlist) within the
/// library, journaled like a track's. Never replaces a file. Hold the
/// library lock.
pub async fn move_loose_file(
    pool: &MySqlPool,
    roots: &Roots,
    batch_id: u64,
    from: &LibraryPath,
    to: &LibraryPath,
) -> Result<(), OpError> {
    let (from_abs, to_abs) = (from.resolve(&roots.library), to.resolve(&roots.library));
    if !is_free(pool, roots, to).await? {
        return Err(OpError::TargetTaken(to.clone()));
    }
    let move_id = sqlx::query(
        "INSERT INTO library_moves (batch_id, from_root, from_path, to_path, method)
         VALUES (?, 'library', ?, ?, 'rename')",
    )
    .bind(batch_id)
    .bind(from.as_str())
    .bind(to.as_str())
    .execute(pool)
    .await?
    .last_insert_id();
    if let Some(parent) = to_abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let renamed = {
        let (from_abs, to_abs) = (from_abs.clone(), to_abs.clone());
        blocking(move || fsops::rename_no_replace(&from_abs, &to_abs)).await
    };
    match renamed {
        Ok(()) => {
            if let Some(parent) = to_abs.parent() {
                fsops::sync_dir(parent);
            }
            set_status(pool, move_id, "done", None).await?;
            Ok(())
        }
        Err(error) => {
            abandon(pool, move_id, &error.to_string()).await?;
            Err(match error.kind() {
                ErrorKind::AlreadyExists => OpError::TargetTaken(to.clone()),
                ErrorKind::NotFound => OpError::SourceMissing,
                _ => error.into(),
            })
        }
    }
}

/// Fault injection for the chaos tests: with `HERON_LIBRARIAN_CRASH_AT` set
/// to `step:n`, the process aborts the `n`th time it reaches `step`, as a
/// power cut would stop it. Never set outside tests.
pub(crate) fn crash_point(step: &str) {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    static PLAN: OnceLock<Option<(String, u64)>> = OnceLock::new();
    static REACHED: AtomicU64 = AtomicU64::new(0);
    let plan = PLAN.get_or_init(|| {
        let value = std::env::var("HERON_LIBRARIAN_CRASH_AT").ok()?;
        let (step, n) = value.split_once(':')?;
        Some((step.to_owned(), n.parse().ok()?))
    });
    if let Some((crash_step, n)) = plan
        && crash_step == step
        && REACHED.fetch_add(1, Ordering::SeqCst) + 1 == *n
    {
        eprintln!("HERON_LIBRARIAN_CRASH_AT: crashing at {step}");
        std::process::abort();
    }
}

/// What [`recover`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Recovery {
    /// Interrupted operations finished.
    pub finished: u64,
    /// Interrupted operations rolled back (the file had not moved).
    pub abandoned: u64,
}

/// Finishes or rolls back operations interrupted by a crash. Run it at
/// startup, holding the library lock, before any other operation.
pub async fn recover(pool: &MySqlPool, roots: &Roots) -> Result<Recovery, OpError> {
    let rows = sqlx::query(
        "SELECT id, track_id, CAST(from_root AS CHAR) AS from_root, from_path, to_path,
                CAST(method AS CHAR) AS method, temp_path, content_hash,
                CAST(status AS CHAR) AS status
         FROM library_moves WHERE status IN ('pending', 'moved') ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let mut recovery = Recovery::default();
    for row in rows {
        let move_id: u64 = row.try_get("id")?;
        let track_id: Option<u64> = row.try_get("track_id")?;
        let from_path = LibraryPath::from_stored(row.try_get("from_path")?);
        let origin = Origin::from_stored(&row.try_get::<String, _>("from_root")?, from_path);
        let to = LibraryPath::from_stored(row.try_get("to_path")?);
        let method: String = row.try_get("method")?;
        let temp: Option<String> = row.try_get("temp_path")?;
        let content_hash: Option<Vec<u8>> = row.try_get("content_hash")?;
        let status: String = row.try_get("status")?;

        let from_abs = origin.resolve(roots);
        let to_abs = to.resolve(&roots.library);
        let in_place = origin == Origin::Library(to.clone());
        let read_details = matches!(origin, Origin::Incoming(_)) || retagged(pool, move_id).await?;
        let matches_copy = |path: &Path| match &content_hash {
            Some(hash) => fsops::full_hash(path).is_ok_and(|actual| actual[..] == hash[..]),
            None => false,
        };

        if status == "moved" {
            // The track already points at the copy: remove the original.
            if !in_place && from_abs.exists() && (matches_copy(&to_abs) || same(&from_abs, &to_abs))
            {
                remove_original(&from_abs)?;
            }
            set_status(pool, move_id, "done", None).await?;
            recovery.finished += 1;
            continue;
        }

        if let Some(temp) = temp {
            let _ = std::fs::remove_file(LibraryPath::from_stored(temp).resolve(&roots.library));
        }
        let (from_exists, to_exists) = (from_abs.exists(), to_abs.exists());
        let landed = if in_place {
            matches_copy(&to_abs)
        } else if !to_exists {
            false
        } else if from_exists {
            same(&from_abs, &to_abs) || matches_copy(&to_abs)
        } else {
            method == "rename" || matches_copy(&to_abs)
        };
        match (landed, track_id) {
            (true, Some(track_id)) => {
                let leftover = !in_place && from_exists;
                let status = if leftover { "moved" } else { "done" };
                finish(pool, move_id, track_id, &to, &to_abs, read_details, status).await?;
                if leftover {
                    remove_original(&from_abs)?;
                    set_status(pool, move_id, "done", None).await?;
                }
                recovery.finished += 1;
            }
            // A file that is not a track (a cover moved to the trash).
            (true, None) => {
                set_status(pool, move_id, "done", None).await?;
                recovery.finished += 1;
            }
            _ => {
                if !from_exists && !in_place {
                    tracing::error!(from = %from_abs.display(), to = %to, "an interrupted move left no file at either path");
                }
                abandon(pool, move_id, "interrupted before the file moved").await?;
                recovery.abandoned += 1;
            }
        }
    }
    if recovery != Recovery::default() {
        tracing::info!(?recovery, "recovered interrupted library operations");
    }
    Ok(recovery)
}

fn same(a: &Path, b: &Path) -> bool {
    fsops::same_file(a, b).unwrap_or(false)
}

/// Whether the move wrote tags.
async fn retagged(pool: &MySqlPool, move_id: u64) -> Result<bool, sqlx::Error> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM track_tag_changes WHERE move_id = ?")
            .bind(move_id)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// Runs file I/O on a blocking thread.
async fn blocking<T, E>(task: impl FnOnce() -> Result<T, E> + Send + 'static) -> Result<T, E>
where
    T: Send + 'static,
    E: From<std::io::Error> + Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result,
        Err(error) => Err(std::io::Error::other(format!("file task failed: {error}")).into()),
    }
}
