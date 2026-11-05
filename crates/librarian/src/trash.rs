// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deleting tracks through the trash: [`delete`] moves their files to
//! `.heron-trash/<batch>/…` at the library root (a rename, journaled as a
//! `delete` batch), [`restore`] brings them back by undoing the batch, and
//! [`purge`] removes them for good once they have been in the trash for
//! `trash_days`.
//!
//! A deleted track keeps its analysis, categories, overrides and history
//! until it is purged, so restoring it brings it back whole. A purged
//! track's plays keep their artist and title.

use std::collections::{BTreeSet, HashSet};
use std::io::ErrorKind;

use heron_core::LibraryPath;
use heron_core::library::{BatchReport, PurgeReport, SkippedFile, TRASH_DIR, in_trash};
use heron_library::{LibraryLock, is_audio};
use sqlx::{MySqlPool, Row};

use crate::fsops;
use crate::journal::{self, BatchKind, OpError, Operation, Origin, Roots, Subject};
use crate::organize::Error;

/// Tracks handled per library lock, so scans can run in between.
const LOCK_CHUNK: usize = 50;

/// Where a file of a `delete` batch goes in the trash.
fn trash_path(batch_id: u64, path: &LibraryPath) -> LibraryPath {
    LibraryPath::from_stored(format!("{TRASH_DIR}/{batch_id}/{path}"))
}

/// Moves tracks to the trash, as one `delete` batch. Tracks whose file is
/// missing are deleted too. Files that are not tracks (`cover.jpg`) follow
/// the last track of their folder, and emptied folders are removed.
pub async fn delete(
    pool: &MySqlPool,
    roots: &Roots,
    track_ids: &[u64],
) -> Result<BatchReport, Error> {
    let batch_id = journal::create_batch(pool, BatchKind::Delete, None).await?;
    let mut report = BatchReport {
        batch_id,
        ..BatchReport::default()
    };
    let mut folders = BTreeSet::new();
    for chunk in track_ids.chunks(LOCK_CHUNK) {
        let lock = LibraryLock::acquire(pool, "librarian").await?;
        for &track_id in chunk {
            let row: Option<(String, bool)> =
                sqlx::query_as("SELECT path, deleted_at IS NOT NULL FROM tracks WHERE id = ?")
                    .bind(track_id)
                    .fetch_optional(pool)
                    .await?;
            let Some((path, deleted)) = row else {
                report.skipped.push(SkippedFile {
                    path: format!("track {track_id}"),
                    reason: "no such track".into(),
                });
                continue;
            };
            if deleted {
                report.skipped.push(SkippedFile {
                    path,
                    reason: "already in the trash".into(),
                });
                continue;
            }
            let from = LibraryPath::from_stored(path);
            let to = trash_path(batch_id, &from);
            // A file back at its path (not scanned yet) goes to the trash too.
            let present = from.resolve(&roots.library).is_file();
            let outcome = if present {
                let operation = Operation {
                    batch_id,
                    subject: Subject::Existing(track_id),
                    from: Origin::Library(from.clone()),
                    to,
                    retag: None,
                    expected_stamp: None,
                };
                journal::execute(pool, roots, operation).await
            } else {
                delete_record(pool, batch_id, track_id, &from, &to)
                    .await
                    .map(|()| track_id)
                    .map_err(OpError::from)
            };
            if outcome.is_ok()
                && let Some((folder, _)) = from.as_str().rsplit_once('/')
            {
                folders.insert(folder.to_owned());
            }
            record_outcome(&mut report, from.as_str(), outcome)?;
        }
        lock.release().await?;
    }

    if !folders.is_empty() {
        let lock = LibraryLock::acquire(pool, "librarian").await?;
        let result = sweep_folders(pool, roots, batch_id, folders).await;
        lock.release().await?;
        result?;
    }
    journal::finish_batch(pool, batch_id).await?;
    tracing::info!(
        batch_id,
        done = report.done,
        skipped = report.skipped.len(),
        "tracks moved to the trash"
    );
    Ok(report)
}

/// Deletes a track that has no file: only its record moves to the trash.
async fn delete_record(
    pool: &MySqlPool,
    batch_id: u64,
    track_id: u64,
    from: &LibraryPath,
    to: &LibraryPath,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO library_moves (batch_id, track_id, from_root, from_path, to_path, method, status)
         VALUES (?, ?, 'library', ?, ?, 'rename', 'done')",
    )
    .bind(batch_id)
    .bind(track_id)
    .bind(from.as_str())
    .bind(to.as_str())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE tracks SET path = ?, deleted_at = NOW(6),
             missing_since = COALESCE(missing_since, NOW(6))
         WHERE id = ?",
    )
    .bind(to.as_str())
    .bind(track_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

fn record_outcome(
    report: &mut BatchReport,
    path: &str,
    outcome: Result<u64, OpError>,
) -> Result<(), Error> {
    match outcome {
        Ok(_) => report.done += 1,
        // The database is unreachable: stop, recovery finishes what started.
        Err(OpError::Database(error)) => return Err(error.into()),
        Err(error) => {
            tracing::warn!(path, %error, "file operation skipped");
            report.skipped.push(SkippedFile {
                path: path.to_owned(),
                reason: error.to_string(),
            });
        }
    }
    Ok(())
}

/// Moves the files left in folders that no longer hold audio (covers,
/// playlists) to the trash, then removes the emptied folders, walking up
/// as long as folders empty out. Hold the library lock.
async fn sweep_folders(
    pool: &MySqlPool,
    roots: &Roots,
    batch_id: u64,
    mut folders: BTreeSet<String>,
) -> Result<(), Error> {
    let mut seen = HashSet::new();
    // Deepest first, so a parent is looked at once its children are done.
    while let Some(folder) = folders
        .iter()
        .max_by_key(|f| f.matches('/').count())
        .cloned()
    {
        folders.remove(&folder);
        if !seen.insert(folder.clone()) {
            continue;
        }
        let absolute = LibraryPath::from_stored(folder.clone()).resolve(&roots.library);
        let Some(loose) = loose_files(&absolute) else {
            continue;
        };
        for name in loose {
            let from = LibraryPath::from_stored(format!("{folder}/{name}"));
            let to = trash_path(batch_id, &from);
            match journal::move_loose_file(pool, roots, batch_id, &from, &to).await {
                Ok(()) => {}
                Err(OpError::Database(error)) => return Err(error.into()),
                Err(error) => tracing::warn!(path = %from, %error, "cannot move to the trash"),
            }
        }
        fsops::remove_empty_dirs(&absolute, &roots.library);
        if !absolute.exists()
            && let Some((parent, _)) = folder.rsplit_once('/')
        {
            folders.insert(parent.to_owned());
        }
    }
    Ok(())
}

/// The files of a folder that holds neither audio nor subfolders, or
/// `None` when it still holds some (or cannot be read).
fn loose_files(dir: &std::path::Path) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let file_type = entry.file_type().ok()?;
        let name = entry.file_name().into_string().ok()?;
        // Heron's own temporary files belong to operations in progress.
        if name.starts_with(".heron-") {
            continue;
        }
        if file_type.is_dir() || is_audio(&entry.path()) {
            return None;
        }
        if file_type.is_file() {
            names.push(name);
        }
    }
    Some(names)
}

/// Restores the tracks of a `delete` batch, as the undo batch `undo_batch`.
/// Files go back to their paths; a path taken since is reported and the
/// track stays in the trash. Tracks purged since are gone for good.
pub(crate) async fn restore(
    pool: &MySqlPool,
    roots: &Roots,
    batch: u64,
    undo_batch: u64,
) -> Result<BatchReport, Error> {
    let rows = sqlx::query(
        "SELECT m.id, m.track_id, m.from_path, m.to_path, t.path AS track_path,
                t.deleted_at IS NOT NULL AS deleted
         FROM library_moves m LEFT JOIN tracks t ON t.id = m.track_id
         WHERE m.batch_id = ? AND m.status = 'done'
         ORDER BY m.id",
    )
    .bind(batch)
    .fetch_all(pool)
    .await?;
    let mut report = BatchReport {
        batch_id: undo_batch,
        ..BatchReport::default()
    };
    for chunk in rows.chunks(LOCK_CHUNK) {
        let lock = LibraryLock::acquire(pool, "librarian").await?;
        for row in chunk {
            let move_id: u64 = row.try_get("id")?;
            let track_id: Option<u64> = row.try_get("track_id")?;
            let original = LibraryPath::from_stored(row.try_get("from_path")?);
            let in_bin = LibraryPath::from_stored(row.try_get("to_path")?);
            let current: Option<String> = row.try_get("track_path")?;
            let deleted: Option<bool> = row.try_get("deleted")?;
            let outcome = match (track_id, current) {
                // Restored already, by a restore interrupted before recording it.
                (Some(_), Some(_)) if deleted != Some(true) => Ok(()),
                (Some(track_id), Some(current)) => {
                    restore_track(pool, roots, undo_batch, track_id, current, &original)
                        .await
                        .map(|_| ())
                }
                // A file that is not a track, or a track purged since.
                _ if !in_bin.resolve(&roots.library).exists() => Err(OpError::SourceMissing),
                _ => journal::move_loose_file(pool, roots, undo_batch, &in_bin, &original).await,
            };
            let succeeded = outcome.is_ok();
            record_outcome(&mut report, original.as_str(), outcome.map(|()| 0))?;
            if succeeded {
                sqlx::query("UPDATE library_moves SET status = 'undone' WHERE id = ?")
                    .bind(move_id)
                    .execute(pool)
                    .await?;
            }
        }
        lock.release().await?;
    }
    fsops::remove_empty_dirs(
        &LibraryPath::from_stored(format!("{TRASH_DIR}/{batch}")).resolve(&roots.library),
        &roots.library,
    );
    tracing::info!(
        batch,
        undo_batch,
        done = report.done,
        skipped = report.skipped.len(),
        "tracks restored from the trash"
    );
    Ok(report)
}

async fn restore_track(
    pool: &MySqlPool,
    roots: &Roots,
    undo_batch: u64,
    track_id: u64,
    current: String,
    original: &LibraryPath,
) -> Result<u64, OpError> {
    let current = LibraryPath::from_stored(current);
    if current.resolve(&roots.library).is_file() {
        let operation = Operation {
            batch_id: undo_batch,
            subject: Subject::Existing(track_id),
            from: Origin::Library(current),
            to: original.clone(),
            retag: None,
            expected_stamp: None,
        };
        return journal::execute(pool, roots, operation).await;
    }
    // It had no file when it was deleted (or its file left the trash): it
    // comes back missing.
    if !journal::is_free(pool, roots, original).await? {
        return Err(OpError::TargetTaken(original.clone()));
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO library_moves (batch_id, track_id, from_root, from_path, to_path, method, status)
         VALUES (?, ?, 'library', ?, ?, 'rename', 'done')",
    )
    .bind(undo_batch)
    .bind(track_id)
    .bind(current.as_str())
    .bind(original.as_str())
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE tracks SET path = ?, deleted_at = NULL WHERE id = ?")
        .bind(original.as_str())
        .bind(track_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(track_id)
}

/// Removes for good the tracks in the trash for more than `older_than_days`
/// (all of them with `None`), with their files, as a `purge` batch. Their
/// plays keep their artist and title. Trash folders left without tracks
/// (the covers that went with them) are removed too.
///
/// Each track is deleted before its file, so an interruption never leaves a
/// track without its file: at worst a file stays in the trash until the
/// next purge.
pub async fn purge(
    pool: &MySqlPool,
    roots: &Roots,
    older_than_days: Option<u32>,
) -> Result<PurgeReport, Error> {
    let tracks: Vec<(u64, String)> = sqlx::query_as(
        "SELECT id, path FROM tracks
         WHERE deleted_at IS NOT NULL AND (? IS NULL OR deleted_at < NOW(6) - INTERVAL ? DAY)
         ORDER BY id",
    )
    .bind(older_than_days)
    .bind(older_than_days)
    .fetch_all(pool)
    .await?;
    let expired_batches: HashSet<u64> = sqlx::query_scalar(
        "SELECT id FROM library_batches
         WHERE kind = 'delete' AND (? IS NULL OR created_at < NOW(6) - INTERVAL ? DAY)",
    )
    .bind(older_than_days)
    .bind(older_than_days)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let trash = roots.library.join(TRASH_DIR);
    let folders = tokio::task::spawn_blocking(move || fsops::numbered_dirs(&trash)).await?;
    if tracks.is_empty() && folders.is_empty() {
        return Ok(PurgeReport::default());
    }

    let mut report = PurgeReport::default();
    let mut batch_id = None;
    for chunk in tracks.chunks(LOCK_CHUNK) {
        let batch = match batch_id {
            Some(id) => id,
            None => *batch_id.insert(journal::create_batch(pool, BatchKind::Purge, None).await?),
        };
        let lock = LibraryLock::acquire(pool, "librarian").await?;
        for (track_id, path) in chunk {
            let mut tx = pool.begin().await?;
            sqlx::query("DELETE FROM restrictions WHERE target = 'track' AND target_id = ?")
                .bind(track_id)
                .execute(&mut *tx)
                .await?;
            let deleted = sqlx::query("DELETE FROM tracks WHERE id = ? AND deleted_at IS NOT NULL")
                .bind(track_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            // A record of what went, for the batch list.
            sqlx::query(
                "INSERT INTO library_moves
                     (batch_id, from_root, from_path, to_path, method, status)
                 VALUES (?, 'library', ?, ?, 'rename', 'done')",
            )
            .bind(batch)
            .bind(path)
            .bind(path)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            if deleted == 0 {
                continue;
            }
            report.tracks += 1;
            journal::crash_point("purged");
            if in_trash(path) {
                match tokio::fs::remove_file(
                    LibraryPath::from_stored(path.clone()).resolve(&roots.library),
                )
                .await
                {
                    Ok(()) => report.files += 1,
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => tracing::warn!(path, %error, "cannot remove from the trash"),
                }
            }
        }
        lock.release().await?;
    }

    // Folders of the trash whose tracks are all gone (purged or restored).
    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT path FROM tracks WHERE deleted_at IS NOT NULL")
            .fetch_all(pool)
            .await?;
    let lock = LibraryLock::acquire(pool, "librarian").await?;
    for (batch, folder) in folders {
        let prefix = format!("{TRASH_DIR}/{batch}/");
        if !expired_batches.contains(&batch) || remaining.iter().any(|p| p.starts_with(&prefix)) {
            continue;
        }
        let removing = folder.clone();
        match tokio::task::spawn_blocking(move || fsops::remove_tree(&removing)).await? {
            Ok(files) => report.files += files,
            Err(error) => {
                tracing::warn!(folder = %folder.display(), %error, "cannot empty the trash");
            }
        }
    }
    lock.release().await?;
    if report.files > 0 && batch_id.is_none() {
        batch_id = Some(journal::create_batch(pool, BatchKind::Purge, None).await?);
    }
    if let Some(batch) = batch_id {
        journal::finish_batch(pool, batch).await?;
    }
    report.batch_id = batch_id;
    if report != PurgeReport::default() {
        tracing::info!(?report, "trash purged");
    }
    Ok(report)
}

/// Whether a batch can be restored by [`restore`].
pub(crate) async fn is_delete(pool: &MySqlPool, batch: u64) -> Result<bool, sqlx::Error> {
    let kind: Option<String> =
        sqlx::query_scalar("SELECT CAST(kind AS CHAR) FROM library_batches WHERE id = ?")
            .bind(batch)
            .fetch_optional(pool)
            .await?;
    Ok(kind.as_deref() == Some("delete"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_paths_keep_the_original_path() {
        let path = LibraryPath::from_stored("Band/Album/01 Song.mp3".into());
        assert_eq!(
            trash_path(7, &path).as_str(),
            ".heron-trash/7/Band/Album/01 Song.mp3"
        );
        assert!(in_trash(trash_path(7, &path).as_str()));
    }

    #[test]
    fn only_folders_without_audio_or_subfolders_give_up_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/sub")).unwrap();
        std::fs::write(root.join("a/cover.jpg"), b"x").unwrap();
        assert_eq!(loose_files(&root.join("a")), None, "a subfolder");
        std::fs::remove_dir(root.join("a/sub")).unwrap();
        std::fs::write(root.join("a/song.mp3"), b"x").unwrap();
        assert_eq!(loose_files(&root.join("a")), None, "audio");
        std::fs::remove_file(root.join("a/song.mp3")).unwrap();
        std::fs::write(root.join("a/.heron-9.mp3"), b"x").unwrap();
        assert_eq!(loose_files(&root.join("a")), Some(vec!["cover.jpg".into()]));
    }
}
