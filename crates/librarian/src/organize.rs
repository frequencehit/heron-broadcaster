// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reorganizing an existing library, on request only: [`plan`] lists every
//! move without touching anything (the dry run), [`apply`] carries it out,
//! and [`undo`] reverts any batch from the journal.

use std::collections::HashSet;
use std::path::Path;

use heron_core::LibraryPath;
use heron_core::library::{BatchReport, FieldValues, OrganizePlan, PlannedMove, SkippedFile};
use heron_library::tags;
use heron_library::{LibraryLock, Tags};
use sqlx::{MySqlPool, Row};

use crate::fsops;
use crate::journal::{
    self, BatchKind, OpError, Operation, Origin, Retag, RetagCover, Roots, Subject, TagChange,
};
use crate::layout::{self, FileFacts, Layout};
use crate::restore::Delta;
use crate::trash;

/// Operations run per library lock, so scans can run in between.
pub(crate) const LOCK_CHUNK: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Operation(#[from] OpError),
    #[error("batch {0} does not exist")]
    NoSuchBatch(u64),
    #[error("batch {batch} was already undone by batch {by}")]
    AlreadyUndone { batch: u64, by: u64 },
    #[error("task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Analysis(#[from] heron_analysis::Error),
}

/// The dry run of an organization.
pub type Plan = OrganizePlan;
/// What a batch did.
pub type Report = BatchReport;

/// Lists the moves that would put every present track where `layout` says.
/// Reads tags, with the tags kept in Heron's database on top, and touches
/// nothing.
pub async fn plan(pool: &MySqlPool, roots: &Roots, layout: &Layout) -> Result<Plan, Error> {
    let rows =
        sqlx::query("SELECT id, path, missing_since IS NULL AS present FROM tracks ORDER BY path")
            .fetch_all(pool)
            .await?;
    let mut tracks = Vec::new();
    // Every recorded path is taken, including those of missing tracks.
    let mut taken = HashSet::new();
    for row in rows {
        let path: String = row.try_get("path")?;
        taken.insert(path.to_lowercase());
        if row.try_get::<bool, _>("present")? {
            tracks.push((row.try_get::<u64, _>("id")?, LibraryPath::from_stored(path)));
        }
    }

    // Tags kept in Heron's database file tracks like their files' tags.
    let overrides = heron_library::details::all_overrides(pool).await?;

    let library = roots.library.clone();
    let layout = layout.clone();
    let plan = tokio::task::spawn_blocking(move || {
        let mut plan = Plan::default();
        for (track_id, current) in tracks {
            let absolute = current.resolve(&library);
            let mut read = match tags::read(&absolute) {
                Ok(read) => read,
                Err(error) => {
                    tracing::warn!(path = %current, %error, "cannot read tags, not moving");
                    plan.unreadable.push(current.to_string());
                    continue;
                }
            };
            if let Some(overrides) = overrides.get(&track_id) {
                heron_library::details::apply_overrides(&mut read, overrides);
            }
            let (stem, extension) = stem_and_extension(&absolute);
            let desired = layout.render(&FileFacts {
                tags: &read.tags,
                original_stem: &stem,
                extension: &extension,
            });
            match free_path(&desired, Some(current.as_str()), &mut taken, |path| {
                std::fs::symlink_metadata(library.join(path)).is_ok()
            }) {
                None => plan.unchanged += 1,
                Some(to) => plan.moves.push(PlannedMove {
                    track_id,
                    from: current.to_string(),
                    to,
                }),
            }
        }
        plan
    })
    .await?;
    Ok(plan)
}

/// The first of `desired`, `desired (2)`, … that is free, or `None` when the
/// file is already there (`own`, compared ignoring case, as case-insensitive
/// file systems do). The chosen path is added to `taken`.
pub fn free_path(
    desired: &str,
    own: Option<&str>,
    taken: &mut HashSet<String>,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    for suffix in 1.. {
        let candidate = if suffix == 1 {
            desired.to_owned()
        } else {
            layout::with_suffix(desired, suffix)
        };
        if own.is_some_and(|own| {
            own.eq_ignore_ascii_case(&candidate) || own.to_lowercase() == candidate.to_lowercase()
        }) {
            return None;
        }
        let key = candidate.to_lowercase();
        if !taken.contains(&key) && !exists(&candidate) {
            taken.insert(key);
            return Some(candidate);
        }
    }
    None
}

pub fn stem_and_extension(path: &Path) -> (String, String) {
    let stem = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("track")
        .to_owned();
    let extension = path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("")
        .to_owned();
    (stem, extension)
}

/// Carries out a plan as one batch, which [`undo`] can revert.
pub async fn apply(pool: &MySqlPool, roots: &Roots, plan: &Plan) -> Result<Report, Error> {
    let batch_id = journal::create_batch(pool, BatchKind::Organize, None).await?;
    let mut report = Report {
        batch_id,
        ..Report::default()
    };
    for chunk in plan.moves.chunks(LOCK_CHUNK) {
        let lock = LibraryLock::acquire(pool, "librarian").await?;
        for planned in chunk {
            let current: Option<String> =
                sqlx::query_scalar("SELECT path FROM tracks WHERE id = ?")
                    .bind(planned.track_id)
                    .fetch_optional(pool)
                    .await?;
            if current.as_deref() != Some(planned.from.as_str()) {
                report.skipped.push(SkippedFile {
                    path: planned.from.clone(),
                    reason: "the track moved since the plan".into(),
                });
                continue;
            }
            let operation = Operation {
                batch_id,
                subject: Subject::Existing(planned.track_id),
                from: Origin::Library(LibraryPath::from_stored(planned.from.clone())),
                to: LibraryPath::from_stored(planned.to.clone()),
                retag: None,
                expected_stamp: None,
            };
            record_outcome(
                &mut report,
                &planned.from,
                journal::execute(pool, roots, operation).await,
            )?;
        }
        lock.release().await?;
    }
    journal::finish_batch(pool, batch_id).await?;
    tracing::info!(
        batch_id,
        done = report.done,
        skipped = report.skipped.len(),
        "library organized"
    );
    Ok(report)
}

pub(crate) fn record_outcome(
    report: &mut Report,
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

/// Reverts a batch: files go back to their previous paths and get their
/// previous tags back. Files imported from the incoming folder stay in the
/// library (they get their own tags back). Nothing is overwritten: a file
/// whose previous path is taken is left where it is and reported.
pub async fn undo(pool: &MySqlPool, roots: &Roots, batch: u64) -> Result<Report, Error> {
    let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM library_batches WHERE id = ?")
        .bind(batch)
        .fetch_optional(pool)
        .await?;
    if exists.is_none() {
        return Err(Error::NoSuchBatch(batch));
    }
    let undone_by: Option<(u64, bool)> =
        sqlx::query_as("SELECT id, finished_at IS NOT NULL FROM library_batches WHERE undoes = ?")
            .bind(batch)
            .fetch_optional(pool)
            .await?;
    // An undo that was interrupted is resumed.
    let resumed = match undone_by {
        Some((by, true)) => return Err(Error::AlreadyUndone { batch, by }),
        Some((by, false)) => Some(by),
        None => None,
    };

    let rows = sqlx::query(
        "SELECT m.id, m.track_id, CAST(m.from_root AS CHAR) AS from_root, m.from_path,
                m.to_path, m.content_hash, t.path AS track_path, c.id AS change_id,
                CAST(c.kind AS CHAR) AS kind, CAST(c.previous AS CHAR) AS previous,
                c.cover_embedded, c.restore
         FROM library_moves m
         JOIN tracks t ON t.id = m.track_id
         LEFT JOIN track_tag_changes c ON c.move_id = m.id AND c.undone_at IS NULL
         WHERE m.batch_id = ? AND m.status = 'done'
         ORDER BY m.id DESC",
    )
    .bind(batch)
    .fetch_all(pool)
    .await?;

    let undo_batch = match resumed {
        Some(id) => id,
        None => journal::create_batch(pool, BatchKind::Undo, Some(batch)).await?,
    };
    // Undoing a deletion restores the tracks from the trash.
    if trash::is_delete(pool, batch).await? {
        let report = trash::restore(pool, roots, batch, undo_batch).await?;
        journal::finish_batch(pool, undo_batch).await?;
        return Ok(report);
    }
    let mut report = Report {
        batch_id: undo_batch,
        ..Report::default()
    };
    for chunk in rows.chunks(LOCK_CHUNK) {
        let lock = LibraryLock::acquire(pool, "librarian").await?;
        for row in chunk {
            let move_id: u64 = row.try_get("id")?;
            let track_id: u64 = row.try_get("track_id")?;
            let current = LibraryPath::from_stored(row.try_get("track_path")?);
            let relocated = row.try_get::<String, _>("from_root")? == "library"
                && row.try_get::<String, _>("from_path")? != row.try_get::<String, _>("to_path")?;
            let change_id: Option<u64> = row.try_get("change_id")?;
            if !relocated && change_id.is_none() {
                continue;
            }
            let to = if relocated {
                LibraryPath::from_stored(row.try_get("from_path")?)
            } else {
                current.clone()
            };
            let retag = match change_id {
                None => None,
                Some(_) => {
                    let change = RecordedChange {
                        kind: row.try_get("kind")?,
                        previous: row.try_get("previous")?,
                        cover_embedded: row.try_get("cover_embedded")?,
                        restore: row.try_get("restore")?,
                        written: row.try_get("content_hash")?,
                    };
                    let path = current.resolve(&roots.library);
                    tokio::task::spawn_blocking(move || change.undo(&path)).await?
                }
            };
            let outcome = if to == current && retag.is_none() {
                Ok(track_id)
            } else {
                let operation = Operation {
                    batch_id: undo_batch,
                    subject: Subject::Existing(track_id),
                    from: Origin::Library(current.clone()),
                    to,
                    retag,
                    expected_stamp: None,
                };
                journal::execute(pool, roots, operation).await
            };
            let succeeded = outcome.is_ok();
            record_outcome(&mut report, current.as_str(), outcome)?;
            if succeeded {
                if relocated {
                    sqlx::query("UPDATE library_moves SET status = 'undone' WHERE id = ?")
                        .bind(move_id)
                        .execute(pool)
                        .await?;
                }
                if let Some(change_id) = change_id {
                    sqlx::query("UPDATE track_tag_changes SET undone_at = NOW(6) WHERE id = ?")
                        .bind(change_id)
                        .execute(pool)
                        .await?;
                }
            }
        }
        lock.release().await?;
    }
    // A release picked on the review screen is taken back: the track
    // needs review again.
    sqlx::query(
        "UPDATE track_identifications i
         JOIN library_moves m ON m.track_id = i.track_id AND m.batch_id = ? AND m.status <> 'abandoned'
         JOIN library_batches b ON b.id = m.batch_id AND b.kind = 'review'
         SET i.status = 'needs_review', i.recording_mbid = NULL, i.release_mbid = NULL",
    )
    .bind(batch)
    .execute(pool)
    .await?;
    journal::finish_batch(pool, undo_batch).await?;
    tracing::info!(
        batch,
        undo_batch,
        done = report.done,
        skipped = report.skipped.len(),
        "batch undone"
    );
    Ok(report)
}

/// A tag change as recorded, to undo it.
struct RecordedChange {
    kind: String,
    previous: String,
    cover_embedded: bool,
    restore: Option<Vec<u8>>,
    /// Hash of the file the change wrote.
    written: Option<Vec<u8>>,
}

impl RecordedChange {
    /// What undoing the change writes into the file at `path`: the file as
    /// it was, byte for byte, when it is still the one the change wrote;
    /// otherwise the previous tags. `None` when it is undone already.
    fn undo(self, path: &Path) -> Option<Retag> {
        let delta = self
            .restore
            .as_deref()
            .and_then(|bytes| Delta::from_bytes(bytes).ok());
        let change = if self.kind == "fields" {
            let previous: FieldValues = serde_json::from_str(&self.previous).unwrap_or_default();
            let now = tags::read_fields(path, previous.keys().copied()).unwrap_or_default();
            TagChange::Fields {
                previous: now,
                applied: previous,
            }
        } else {
            let previous: Tags = serde_json::from_str(&self.previous).unwrap_or_default();
            let now = tags::read(path).map(|read| read.tags).unwrap_or_default();
            TagChange::Tags {
                previous: now,
                applied: previous,
            }
        };
        if let Some(delta) = delta
            && let Ok(hash) = fsops::full_hash(path)
        {
            // Done already, by an undo interrupted before recording it.
            if hash == delta.original_hash() {
                return None;
            }
            if self.written.as_deref() == Some(&hash[..]) {
                return Some(Retag {
                    change,
                    cover: RetagCover::Keep,
                    restore: Some(Box::new(delta)),
                });
            }
        }
        // Changed again since (or recorded before M5b): the previous tags
        // are written back, which may not give the same bytes.
        let done = match &change {
            TagChange::Tags { previous, applied } => previous == applied,
            TagChange::Fields { previous, applied } => previous == applied,
        };
        let cover = if self.cover_embedded {
            RetagCover::Remove
        } else {
            RetagCover::Keep
        };
        (!done).then_some(Retag {
            change,
            cover,
            restore: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_paths_skip_taken_ones_and_recognize_the_own_path() {
        let mut taken = HashSet::from(["a/x.mp3".to_owned()]);
        let on_disk = |path: &str| path == "a/x (2).mp3";
        assert_eq!(
            free_path("a/x.mp3", None, &mut taken, on_disk),
            Some("a/x (3).mp3".into())
        );
        assert_eq!(
            free_path("a/x.mp3", None, &mut taken, on_disk),
            Some("a/x (4).mp3".into()),
            "(3) is now planned"
        );
        assert_eq!(
            free_path("A/X (5).mp3", Some("a/x (5).mp3"), &mut taken, on_disk),
            None
        );
        assert_eq!(
            free_path("a/x.mp3", Some("a/x (5).mp3"), &mut taken, on_disk),
            None
        );
    }
}
