// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tags edited from the browser: the fields are written into the files in
//! place, as one batch that can be undone byte for byte. Files are not
//! moved (organizing the library files them by their new tags) and keep
//! their track: ID, analysis, categories and history.
//!
//! Tags kept in Heron's database (`track_tag_overrides`, while the library
//! setting `write_tags` was off) for the fields written are dropped: the
//! file has the values now.

use heron_core::library::{BatchReport, FieldValues, SkippedFile, TagField};
use heron_core::{FileStamp, LibraryPath};
use heron_library::LibraryLock;
use heron_library::tags;
use sqlx::{MySqlPool, Row};

use crate::journal::{
    self, BatchKind, Operation, Origin, Retag, RetagCover, Roots, Subject, TagChange,
};
use crate::organize::{self, Error, LOCK_CHUNK};

/// Writes `set`, and each track's own `from_details`, into the files of
/// `track_ids`.
pub async fn apply(
    pool: &MySqlPool,
    roots: &Roots,
    track_ids: &[u64],
    set: &FieldValues,
    from_details: &[TagField],
) -> Result<BatchReport, Error> {
    let batch_id = journal::create_batch(pool, BatchKind::Edit, None).await?;
    let mut report = BatchReport {
        batch_id,
        ..BatchReport::default()
    };
    for chunk in track_ids.chunks(LOCK_CHUNK) {
        let lock = LibraryLock::acquire(pool, "librarian").await?;
        for &track_id in chunk {
            edit_one(
                pool,
                roots,
                batch_id,
                track_id,
                set,
                from_details,
                &mut report,
            )
            .await?;
        }
        lock.release().await?;
    }
    journal::finish_batch(pool, batch_id).await?;
    tracing::info!(
        batch_id,
        done = report.done,
        unchanged = report.unchanged,
        skipped = report.skipped.len(),
        "tags edited"
    );
    Ok(report)
}

async fn edit_one(
    pool: &MySqlPool,
    roots: &Roots,
    batch_id: u64,
    track_id: u64,
    set: &FieldValues,
    from_details: &[TagField],
    report: &mut BatchReport,
) -> Result<(), Error> {
    let row = sqlx::query(
        "SELECT t.path, t.missing_since IS NOT NULL AS missing, t.file_size, t.file_mtime_us,
                mb.isrc, mb.record_label, CAST(COALESCE(p.bpm, a.bpm) AS DOUBLE) AS bpm, p.mood
         FROM tracks t
         LEFT JOIN track_musicbrainz mb ON mb.track_id = t.id
         LEFT JOIN track_programming p ON p.track_id = t.id
         LEFT JOIN track_analysis a ON a.track_id = t.id
         WHERE t.id = ?",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        report.skipped.push(SkippedFile {
            path: format!("track {track_id}"),
            reason: "the track no longer exists".into(),
        });
        return Ok(());
    };
    let path = LibraryPath::from_stored(row.try_get("path")?);
    if row.try_get::<bool, _>("missing")? {
        report.skipped.push(SkippedFile {
            path: path.to_string(),
            reason: "the file is missing".into(),
        });
        return Ok(());
    }

    let mut applied = set.clone();
    for &field in from_details {
        let value = match field {
            TagField::Isrc => row.try_get::<Option<String>, _>("isrc")?,
            TagField::RecordLabel => row.try_get("record_label")?,
            TagField::Bpm => row
                .try_get::<Option<f64>, _>("bpm")?
                .and_then(|bpm| field.normalize(&bpm.to_string()).ok()),
            TagField::Mood => row.try_get("mood")?,
            _ => None,
        };
        // A detail Heron does not have leaves the file's value alone.
        if let Some(value) = value {
            applied.insert(field, Some(value));
        }
    }
    if applied.is_empty() {
        report.unchanged += 1;
        return Ok(());
    }

    let absolute = path.resolve(&roots.library);
    let fields: Vec<TagField> = applied.keys().copied().collect();
    let previous =
        tokio::task::spawn_blocking(move || tags::read_fields(&absolute, fields)).await?;
    let previous = match previous {
        Ok(previous) => previous,
        Err(error) => {
            report.skipped.push(SkippedFile {
                path: path.to_string(),
                reason: error.to_string(),
            });
            return Ok(());
        }
    };
    if previous == applied {
        drop_overrides(pool, roots, track_id, &path, &fields_written(&applied)).await?;
        report.unchanged += 1;
        return Ok(());
    }
    // A file changed since the last scan is left alone: the scanner reads it first.
    let size: Option<u64> = row.try_get("file_size")?;
    let mtime_us: Option<i64> = row.try_get("file_mtime_us")?;
    let written = fields_written(&applied);
    let operation = Operation {
        batch_id,
        subject: Subject::Existing(track_id),
        from: Origin::Library(path.clone()),
        to: path.clone(),
        retag: Some(Retag {
            change: TagChange::Fields { previous, applied },
            cover: RetagCover::Keep,
            restore: None,
        }),
        expected_stamp: size
            .zip(mtime_us)
            .map(|(size, mtime_us)| FileStamp { size, mtime_us }),
    };
    let outcome = journal::execute(pool, roots, operation).await;
    if outcome.is_ok() {
        drop_overrides(pool, roots, track_id, &path, &written).await?;
    }
    organize::record_outcome(report, path.as_str(), outcome)
}

fn fields_written(applied: &FieldValues) -> Vec<TagField> {
    applied.keys().copied().collect()
}

/// Drops the track's overrides of `fields`, which the file has now, and
/// reads the file's tags back into the track if there were any.
async fn drop_overrides(
    pool: &MySqlPool,
    roots: &Roots,
    track_id: u64,
    path: &LibraryPath,
    fields: &[TagField],
) -> Result<(), Error> {
    let mut dropped = 0;
    for field in fields {
        dropped += sqlx::query("DELETE FROM track_tag_overrides WHERE track_id = ? AND field = ?")
            .bind(track_id)
            .bind(field.as_str())
            .execute(pool)
            .await?
            .rows_affected();
    }
    if dropped == 0 {
        return Ok(());
    }
    let absolute = path.resolve(&roots.library);
    let file = absolute.clone();
    match tokio::task::spawn_blocking(move || tags::read(&absolute)).await? {
        Ok(read) => {
            let mut connection = pool.acquire().await?;
            heron_library::details::save(
                &mut connection,
                &roots.covers,
                track_id,
                &read,
                Some(&file),
            )
            .await?;
        }
        // The scanner reads it again when the file changes.
        Err(error) => tracing::warn!(%path, %error, "cannot read the tags back"),
    }
    Ok(())
}
