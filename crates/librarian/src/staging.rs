// SPDX-License-Identifier: AGPL-3.0-or-later

//! The buffer zone (`HERON_STAGING_DIR`): new files, uploaded through the
//! API or moved from the incoming folder, wait there until someone imports
//! them.
//!
//! [`Stager`] reads each complete file's tags and duration, fingerprints
//! it, identifies it with its group (AcoustID, then MusicBrainz), flags
//! duplicates and files that cannot be played, and works out the tags it
//! would get and where the layout files it. Files are then `ready`. When
//! someone picks another release, or keeps the file's own tags, the file
//! goes back to `identifying` and the stager works the tags and path out
//! again.
//!
//! [`import`] files ready files into the library as one `import` batch,
//! through the journal, like the incoming folder did before M5c.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use heron_analysis::Chromaprint;
use heron_core::library::{
    BatchReport, FieldValues, SkippedFile, StagedChoice, StagedFlag, StagedIdentification,
    StagedIdentificationStatus, TagField,
};
use heron_core::{LibraryPath, LibrarySettings};
use heron_library::tags::{self, FileTags};
use heron_library::{LibraryLock, Tags};
use sqlx::{MySqlPool, Row};

use crate::identify::{Endpoints, FileToIdentify, Identified, Identifier};
use crate::journal::{
    self, BatchKind, Identification, IdentificationStatus, NewTrack, OpError, Operation, Origin,
    Retag, RetagCover, Roots, Subject, TagChange,
};
use crate::layout::{FileFacts, Layout};
use crate::organize::{self, Error};
use crate::{fsops, import};

/// Lookups of a group are tried this many times before its files are
/// marked as needing review.
const LOOKUP_ATTEMPTS: u32 = 3;
/// Wait before trying a group again, doubled after each failure.
const RETRY_DELAY: Duration = Duration::from_secs(60);
/// A group is identified once none of its uploads has made progress for
/// this long (an upload in progress, or one given up on).
const UPLOAD_QUIET_MINUTES: u32 = 10;
/// Uploads that made no progress for this long are given up.
const ABANDONED_UPLOAD_HOURS: u32 = 24;
/// How often abandoned uploads and leftover folders are cleaned up.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(3600);
/// Files imported per library lock, so scans can run in between.
const LOCK_CHUNK: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// What one look at the buffer zone did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StagingReport {
    /// Files identified and checked, now ready.
    pub ready: u64,
    /// Files whose group waits (uploads still arriving, lookups failing).
    pub waiting: u64,
}

struct Retry {
    attempts: u32,
    at: Instant,
}

/// A file already checked in this pass: its ID, content hash and fingerprint.
type Seen = (u64, Option<[u8; 32]>, Option<Vec<u8>>);

/// Identifies and checks the files of the buffer zone.
pub struct Stager {
    pool: MySqlPool,
    roots: Roots,
    endpoints: Endpoints,
    retries: HashMap<u64, Retry>,
    last_cleanup: Option<Instant>,
}

/// A row of `staged_files` waiting for the stager.
struct Waiting {
    id: u64,
    staged_path: LibraryPath,
    /// The file's own tags, once read.
    own: Option<Tags>,
    identification: Option<StagedIdentification>,
    choice: Option<StagedChoice>,
    /// Its tags before the edits, once worked out.
    base: Option<Tags>,
    /// Fields edited by hand, as stored (to tell when they change).
    edits: Option<String>,
}

impl Waiting {
    fn edits(&self) -> FieldValues {
        self.edits
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default()
    }
}

/// `tags` with the fields edited by hand: a value replaces the field,
/// `None` removes it. The year replaces the date.
pub fn apply_edits(tags: &Tags, edits: &FieldValues) -> Tags {
    let mut tags = tags.clone();
    let number = |value: &Option<String>| value.as_deref().and_then(|n| n.parse().ok());
    for (field, value) in edits {
        match field {
            TagField::Title => tags.title.clone_from(value),
            TagField::Artist => tags.artist.clone_from(value),
            TagField::Album => tags.album.clone_from(value),
            TagField::AlbumArtist => tags.album_artist.clone_from(value),
            TagField::Year => tags.date.clone_from(value),
            TagField::Track => tags.track = number(value),
            TagField::Disc => tags.disc = number(value),
            // Not edited in the buffer zone.
            _ => {}
        }
    }
    tags
}

/// A file read and fingerprinted.
struct Read {
    file: FileTags,
    content_hash: Option<[u8; 32]>,
    chromaprint: Option<Chromaprint>,
    flags: Vec<StagedFlag>,
}

impl Stager {
    pub fn new(pool: MySqlPool, roots: Roots, endpoints: Endpoints) -> Self {
        Self {
            pool,
            roots,
            endpoints,
            retries: HashMap::new(),
            last_cleanup: None,
        }
    }

    /// Identifies the groups whose files are all complete.
    pub async fn identify_once(&mut self) -> Result<StagingReport, StagingError> {
        if self
            .last_cleanup
            .is_none_or(|at| at.elapsed() >= CLEANUP_INTERVAL)
        {
            self.last_cleanup = Some(Instant::now());
            cleanup(&self.pool, &self.roots).await?;
        }
        let groups: Vec<u64> = sqlx::query_scalar(
            "SELECT DISTINCT f.group_id FROM staged_files f
             WHERE f.status = 'identifying'
               AND NOT EXISTS (
                   SELECT 1 FROM staged_files r
                   WHERE r.group_id = f.group_id AND r.status = 'receiving'
                     AND r.updated_at > NOW(6) - INTERVAL ? MINUTE)
             ORDER BY f.group_id",
        )
        .bind(UPLOAD_QUIET_MINUTES)
        .fetch_all(&self.pool)
        .await?;
        let mut report = StagingReport::default();
        if groups.is_empty() {
            return Ok(report);
        }
        let settings = import::load_settings(&self.pool).await?;
        let layout = Layout::from_settings(&settings);
        // Looking a release up needs no AcoustID key.
        let identifier = match Identifier::new(
            self.endpoints.clone(),
            settings.acoustid_key.clone().unwrap_or_default(),
            settings.min_score,
        ) {
            Ok(identifier) => identifier,
            Err(error) => {
                tracing::error!(%error, "cannot set up identification");
                return Ok(report);
            }
        };
        for group in groups {
            if self
                .retries
                .get(&group)
                .is_some_and(|retry| retry.at > Instant::now())
            {
                report.waiting += 1;
                continue;
            }
            let waiting = self.waiting(group).await?;
            let (fresh, chosen): (Vec<Waiting>, Vec<Waiting>) = waiting
                .into_iter()
                .partition(|file| file.identification.is_none());
            let mut planned = HashSet::new();
            if !fresh.is_empty() {
                let key = settings.acoustid_key.is_some().then_some(&identifier);
                match self
                    .identify_group(group, fresh, key, &layout, &mut planned)
                    .await?
                {
                    Some(ready) => report.ready += ready,
                    None => report.waiting += 1,
                }
            }
            for file in chosen {
                let ready = if file.base.is_some() {
                    self.apply_edits(file, &layout, &mut planned).await?
                } else {
                    self.apply_choice(group, file, &identifier, &layout, &mut planned)
                        .await?
                };
                if ready {
                    report.ready += 1;
                } else {
                    report.waiting += 1;
                }
            }
        }
        if report.ready > 0 {
            tracing::info!(ready = report.ready, "files of the buffer zone identified");
        }
        Ok(report)
    }

    async fn waiting(&self, group: u64) -> Result<Vec<Waiting>, StagingError> {
        let rows = sqlx::query(
            "SELECT id, staged_path, CAST(tags AS CHAR) AS tags, duration_ms, has_cover,
                    CAST(identification AS CHAR) AS identification,
                    CAST(choice AS CHAR) AS choice, CAST(base_tags AS CHAR) AS base,
                    CAST(tag_edits AS CHAR) AS edits
             FROM staged_files WHERE group_id = ? AND status = 'identifying'
             ORDER BY original_name, id",
        )
        .bind(group)
        .fetch_all(&self.pool)
        .await?;
        let mut files = Vec::with_capacity(rows.len());
        for row in rows {
            let tags: Option<String> = row.try_get("tags")?;
            let identification: Option<String> = row.try_get("identification")?;
            let choice: Option<String> = row.try_get("choice")?;
            let base: Option<String> = row.try_get("base")?;
            files.push(Waiting {
                id: row.try_get("id")?,
                staged_path: LibraryPath::from_stored(row.try_get("staged_path")?),
                own: tags.and_then(|json| serde_json::from_str(&json).ok()),
                identification: identification.and_then(|json| serde_json::from_str(&json).ok()),
                choice: choice.and_then(|json| serde_json::from_str(&json).ok()),
                base: base.and_then(|json| serde_json::from_str(&json).ok()),
                edits: row.try_get("edits")?,
            });
        }
        Ok(files)
    }

    /// Identifies new files of a group together. Returns how many are
    /// ready, or `None` when the lookups failed and the group waits.
    async fn identify_group(
        &mut self,
        group: u64,
        files: Vec<Waiting>,
        identifier: Option<&Identifier>,
        layout: &Layout,
        planned: &mut HashSet<String>,
    ) -> Result<Option<u64>, StagingError> {
        let root = self.roots.staging.clone();
        let paths: Vec<LibraryPath> = files.iter().map(|file| file.staged_path.clone()).collect();
        let reads = tokio::task::spawn_blocking(move || {
            paths
                .iter()
                .map(|path| read_file(&path.resolve(&root)))
                .collect::<Vec<_>>()
        })
        .await?;

        let identified = match identifier {
            None => vec![None; reads.len()],
            Some(identifier) => {
                let lookups: Vec<FileToIdentify> = reads
                    .iter()
                    .map(|read| FileToIdentify {
                        chromaprint: read.chromaprint.clone(),
                        duration_s: read.file.duration_ms.map(|ms| (ms + 500) / 1000),
                        album_tag: read.file.tags.album.clone(),
                    })
                    .collect();
                match identifier.identify(&lookups).await {
                    Ok(identified) => {
                        self.retries.remove(&group);
                        identified.into_iter().map(Some).collect()
                    }
                    Err(error) => {
                        let retry = self.retries.entry(group).or_insert(Retry {
                            attempts: 0,
                            at: Instant::now(),
                        });
                        retry.attempts += 1;
                        if retry.attempts < LOOKUP_ATTEMPTS {
                            retry.at = Instant::now() + RETRY_DELAY * 2_u32.pow(retry.attempts - 1);
                            tracing::warn!(group, %error, attempt = retry.attempts, "identification failed, will retry");
                            return Ok(None);
                        }
                        tracing::warn!(group, %error, "identification keeps failing, the files need review");
                        self.retries.remove(&group);
                        let reason = format!("identification failed: {error}");
                        vec![
                            Some(Identified::Review {
                                reason,
                                candidates: Vec::new()
                            });
                            reads.len()
                        ]
                    }
                }
            }
        };

        let mut seen: Vec<Seen> = Vec::new();
        let mut ready = 0;
        for ((file, mut read), identified) in files.into_iter().zip(reads).zip(identified) {
            let own = read.file.tags.clone();
            let edits = file.edits();
            let (identification, base) = match identified {
                None => (
                    StagedIdentification {
                        status: StagedIdentificationStatus::Off,
                        reason: None,
                        score: None,
                        recording_mbid: None,
                        release_mbid: None,
                        candidates: Vec::new(),
                    },
                    own.clone(),
                ),
                Some(Identified::Match {
                    tags,
                    score,
                    recording_mbid,
                    release_mbid,
                    candidates,
                }) => (
                    StagedIdentification {
                        status: StagedIdentificationStatus::Identified,
                        reason: None,
                        score: Some(score),
                        recording_mbid: Some(recording_mbid),
                        release_mbid: Some(release_mbid),
                        candidates: candidates_list(&candidates),
                    },
                    import::merge(*tags, &own),
                ),
                Some(Identified::Review { reason, candidates }) => (
                    StagedIdentification {
                        status: StagedIdentificationStatus::NeedsReview,
                        reason: Some(reason),
                        score: candidates.iter().map(|c| c.score).reduce(f64::max),
                        recording_mbid: None,
                        release_mbid: None,
                        candidates: candidates_list(&candidates),
                    },
                    own.clone(),
                ),
            };

            let chromaprint = read.chromaprint.as_ref().map(|c| c.as_bytes().to_vec());
            let duplicates = self
                .duplicates(
                    file.id,
                    read.content_hash,
                    chromaprint.as_deref(),
                    identification.recording_mbid.as_deref(),
                    &seen,
                )
                .await?;
            read.flags.extend(duplicates.flags);
            seen.push((file.id, read.content_hash, chromaprint.clone()));
            let applied = apply_edits(&base, &edits);
            let planned_path = self
                .plan_path(layout, &applied, &file.staged_path, planned)
                .await?;

            let updated = sqlx::query(
                "UPDATE staged_files SET status = 'ready', tags = ?, duration_ms = ?,
                     has_cover = ?, chromaprint = ?, content_hash = ?, identification = ?,
                     base_tags = ?, applied_tags = ?, planned_path = ?, flags = ?,
                     duplicate_of_track_id = ?, duplicate_of_staged_id = ?, error = NULL
                 WHERE id = ? AND status = 'identifying' AND tag_edits <=> ?",
            )
            .bind(json(&own))
            .bind(read.file.duration_ms)
            .bind(read.file.has_cover)
            .bind(chromaprint)
            .bind(read.content_hash.map(|hash| hash.to_vec()))
            .bind(json(&identification))
            .bind(json(&base))
            .bind(json(&applied))
            .bind(&planned_path)
            .bind((!read.flags.is_empty()).then(|| json(&read.flags)))
            .bind(duplicates.track)
            .bind(duplicates.staged)
            .bind(file.id)
            // Edited meanwhile: worked out again next time.
            .bind(&file.edits)
            .execute(&self.pool)
            .await?
            .rows_affected();
            ready += updated;
        }
        Ok(Some(ready))
    }

    /// Works out the tags and path of a file whose release was picked by
    /// hand, or that keeps its own tags. Returns whether it is ready.
    async fn apply_choice(
        &mut self,
        group: u64,
        file: Waiting,
        identifier: &Identifier,
        layout: &Layout,
        planned: &mut HashSet<String>,
    ) -> Result<bool, StagingError> {
        let own = file.own.clone().unwrap_or_default();
        let (applied, error) = match &file.choice {
            None | Some(StagedChoice::Own) => (Some(own.clone()), None),
            Some(StagedChoice::Release {
                release_id,
                recording_id,
                disc,
                track,
            }) => match identifier
                .release_tags(release_id, recording_id, *disc, *track)
                .await
            {
                Ok(Some(tags)) => (Some(import::merge(tags, &own)), None),
                Ok(None) => (
                    None,
                    Some("MusicBrainz does not list this recording on the release".to_owned()),
                ),
                Err(error) => {
                    let retry = self.retries.entry(group).or_insert(Retry {
                        attempts: 0,
                        at: Instant::now(),
                    });
                    retry.attempts += 1;
                    if retry.attempts < LOOKUP_ATTEMPTS {
                        retry.at = Instant::now() + RETRY_DELAY * 2_u32.pow(retry.attempts - 1);
                        return Ok(false);
                    }
                    self.retries.remove(&group);
                    (None, Some(format!("cannot look the release up: {error}")))
                }
            },
        };
        let updated = match applied {
            Some(base) => {
                let applied = apply_edits(&base, &file.edits());
                let planned_path = self
                    .plan_path(layout, &applied, &file.staged_path, planned)
                    .await?;
                sqlx::query(
                    "UPDATE staged_files SET status = 'ready', base_tags = ?, applied_tags = ?,
                         planned_path = ?, error = NULL
                     WHERE id = ? AND status = 'identifying' AND tag_edits <=> ?",
                )
                .bind(json(&base))
                .bind(json(&applied))
                .bind(planned_path)
                .bind(file.id)
                .bind(&file.edits)
                .execute(&self.pool)
                .await?
            }
            // The previous tags and path stand.
            None => {
                sqlx::query(
                    "UPDATE staged_files SET status = 'ready', choice = NULL, error = ?
                     WHERE id = ? AND status = 'identifying'",
                )
                .bind(error)
                .bind(file.id)
                .execute(&self.pool)
                .await?
            }
        };
        Ok(updated.rows_affected() > 0)
    }

    /// Works out the tags and path of a file whose tags were edited, from
    /// the tags it had before: the release is not looked up again.
    async fn apply_edits(
        &self,
        file: Waiting,
        layout: &Layout,
        planned: &mut HashSet<String>,
    ) -> Result<bool, StagingError> {
        let base = file.base.clone().unwrap_or_default();
        let applied = apply_edits(&base, &file.edits());
        let planned_path = self
            .plan_path(layout, &applied, &file.staged_path, planned)
            .await?;
        let updated = sqlx::query(
            "UPDATE staged_files SET status = 'ready', applied_tags = ?, planned_path = ?,
                 error = NULL
             WHERE id = ? AND status = 'identifying' AND tag_edits <=> ?",
        )
        .bind(json(&applied))
        .bind(planned_path)
        .bind(file.id)
        .bind(&file.edits)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() > 0)
    }

    /// Where the layout would file a file today. The path is taken again
    /// on import, as the library may have changed.
    async fn plan_path(
        &self,
        layout: &Layout,
        tags: &Tags,
        staged_path: &LibraryPath,
        planned: &mut HashSet<String>,
    ) -> Result<String, sqlx::Error> {
        plan_path(&self.pool, &self.roots, layout, tags, staged_path, planned)
            .await
            .map(|path| path.to_string())
    }

    async fn duplicates(
        &self,
        file_id: u64,
        content_hash: Option<[u8; 32]>,
        chromaprint: Option<&[u8]>,
        recording_mbid: Option<&str>,
        seen: &[Seen],
    ) -> Result<Duplicates, sqlx::Error> {
        let mut found = Duplicates::default();
        if let Some(hash) = content_hash {
            found.track = sqlx::query_scalar(
                "SELECT id FROM tracks WHERE file_hash = ? AND deleted_at IS NULL LIMIT 1",
            )
            .bind(&hash[..])
            .fetch_optional(&self.pool)
            .await?;
        }
        if found.track.is_none()
            && let Some(chromaprint) = chromaprint
        {
            found.track = sqlx::query_scalar(
                "SELECT f.track_id FROM track_fingerprints f
                 JOIN tracks t ON t.id = f.track_id AND t.deleted_at IS NULL
                 WHERE f.chromaprint_hash = UNHEX(MD5(?)) AND f.chromaprint = ? LIMIT 1",
            )
            .bind(chromaprint)
            .bind(chromaprint)
            .fetch_optional(&self.pool)
            .await?;
        }
        if found.track.is_some() {
            found.flags.push(StagedFlag::DuplicateInLibrary);
        } else if let Some(recording) = recording_mbid {
            let same: Option<u64> = sqlx::query_scalar(
                "SELECT i.track_id FROM track_identifications i
                 JOIN tracks t ON t.id = i.track_id AND t.deleted_at IS NULL
                 WHERE i.recording_mbid = ? LIMIT 1",
            )
            .bind(recording)
            .fetch_optional(&self.pool)
            .await?;
            if same.is_some() {
                found.track = same;
                found.flags.push(StagedFlag::SameRecordingInLibrary);
            }
        }

        // Earlier files of this pass, then the rest of the buffer zone.
        found.staged = seen
            .iter()
            .find(|(_, hash, print)| {
                (content_hash.is_some() && *hash == content_hash)
                    || (chromaprint.is_some() && print.as_deref() == chromaprint)
            })
            .map(|(id, _, _)| *id);
        if found.staged.is_none() && (content_hash.is_some() || chromaprint.is_some()) {
            found.staged = sqlx::query_scalar(
                "SELECT id FROM staged_files
                 WHERE id <> ? AND status IN ('ready', 'importing')
                   AND (content_hash = ? OR chromaprint = ?)
                 ORDER BY id LIMIT 1",
            )
            .bind(file_id)
            .bind(content_hash.map(|hash| hash.to_vec()))
            .bind(chromaprint)
            .fetch_optional(&self.pool)
            .await?;
        }
        if found.staged.is_some() {
            found.flags.push(StagedFlag::DuplicateInBuffer);
        }
        Ok(found)
    }
}

#[derive(Default)]
struct Duplicates {
    flags: Vec<StagedFlag>,
    track: Option<u64>,
    staged: Option<u64>,
}

/// Reads a file's tags, content hash and fingerprint, and flags what
/// cannot be played. Blocking.
fn read_file(path: &std::path::Path) -> Read {
    let mut flags = Vec::new();
    let file = tags::read(path).unwrap_or_else(|error| {
        tracing::warn!(path = %path.display(), %error, "cannot read tags");
        FileTags::default()
    });
    let content_hash = heron_library::content_hash(path).ok();
    let chromaprint = match heron_analysis::fingerprint_file(path) {
        Ok(Some(chromaprint)) => Some(chromaprint),
        Ok(None) => {
            flags.push(StagedFlag::TooShort);
            None
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "cannot decode");
            flags.push(if error.is_unsupported_format() {
                StagedFlag::UnsupportedFormat
            } else {
                StagedFlag::Undecodable
            });
            None
        }
    };
    Read {
        file,
        content_hash,
        chromaprint,
        flags,
    }
}

fn candidates_list(candidates: &[crate::identify::matching::Candidate]) -> Vec<serde_json::Value> {
    match import::candidates_json(candidates) {
        Some(serde_json::Value::Array(list)) => list,
        _ => Vec::new(),
    }
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// The first free path the layout gives `tags` in the library.
async fn plan_path(
    pool: &MySqlPool,
    roots: &Roots,
    layout: &Layout,
    tags: &Tags,
    staged_path: &LibraryPath,
    planned: &mut HashSet<String>,
) -> Result<LibraryPath, sqlx::Error> {
    let (stem, extension) = organize::stem_and_extension(&staged_path.resolve(&roots.staging));
    let desired = layout.render(&FileFacts {
        tags,
        original_stem: &stem,
        extension: &extension,
    });
    loop {
        let candidate = organize::free_path(&desired, None, planned, |path| {
            std::fs::symlink_metadata(roots.library.join(path)).is_ok()
        });
        let Some(candidate) = candidate.map(LibraryPath::from_stored) else {
            continue;
        };
        if journal::is_free(pool, roots, &candidate).await? {
            return Ok(candidate);
        }
    }
}

/// A file being imported.
struct ToImport {
    id: u64,
    group_id: u64,
    staged_path: LibraryPath,
    own: Tags,
    applied: Tags,
    duration_ms: Option<u32>,
    has_cover: bool,
    chromaprint: Option<Vec<u8>>,
    identification: Option<StagedIdentification>,
    choice: Option<StagedChoice>,
}

impl ToImport {
    /// The release it is filed as, if any.
    fn release(&self) -> Option<&String> {
        match (&self.choice, &self.identification) {
            (Some(StagedChoice::Release { release_id, .. }), _) => Some(release_id),
            (None, Some(identification))
                if identification.status == StagedIdentificationStatus::Identified =>
            {
                identification.release_mbid.as_ref()
            }
            _ => None,
        }
    }

    /// What the track's identification becomes.
    fn track_identification(&self) -> Option<Identification> {
        let identification = self.identification.as_ref()?;
        if identification.status == StagedIdentificationStatus::Off
            && !matches!(self.choice, Some(StagedChoice::Release { .. }))
        {
            return None;
        }
        let candidates = (!identification.candidates.is_empty())
            .then(|| serde_json::Value::Array(identification.candidates.clone()));
        let (status, reason, recording, release) = match &self.choice {
            Some(StagedChoice::Release {
                release_id,
                recording_id,
                ..
            }) => (
                IdentificationStatus::Identified,
                None,
                Some(recording_id.clone()),
                Some(release_id.clone()),
            ),
            Some(StagedChoice::Own) => (IdentificationStatus::Kept, None, None, None),
            None => match identification.status {
                StagedIdentificationStatus::Identified => (
                    IdentificationStatus::Identified,
                    None,
                    identification.recording_mbid.clone(),
                    identification.release_mbid.clone(),
                ),
                _ => (
                    IdentificationStatus::NeedsReview,
                    identification.reason.clone(),
                    None,
                    None,
                ),
            },
        };
        Some(Identification {
            status,
            reason,
            score: identification.score,
            recording_mbid: recording,
            release_mbid: release,
            candidates,
        })
    }
}

/// Imports ready files of the buffer zone into the library, as one
/// `import` batch: their tags are written, they are filed where the layout
/// says, covers are saved, and they are analyzed like any new track.
/// Files that are not ready are skipped.
pub async fn import(
    pool: &MySqlPool,
    roots: &Roots,
    endpoints: &Endpoints,
    settings: &LibrarySettings,
    file_ids: &[u64],
) -> Result<BatchReport, Error> {
    let batch_id = journal::create_batch(pool, BatchKind::Import, None).await?;
    let mut report = BatchReport {
        batch_id,
        ..BatchReport::default()
    };
    let mut files = Vec::new();
    for &id in file_ids {
        let claimed = sqlx::query(
            "UPDATE staged_files SET status = 'importing' WHERE id = ? AND status = 'ready'",
        )
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
        if claimed == 0 {
            let name: Option<(String, String)> = sqlx::query_as(
                "SELECT original_name, CAST(status AS CHAR) FROM staged_files WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            report.skipped.push(match name {
                Some((name, status)) => SkippedFile {
                    path: name,
                    reason: format!("not ready to import ({status})"),
                },
                None => SkippedFile {
                    path: format!("file {id}"),
                    reason: "not in the buffer zone".into(),
                },
            });
            continue;
        }
        files.push(load_claimed(pool, id).await?);
    }

    let releases: HashSet<&String> = files.iter().filter_map(ToImport::release).collect();
    let covers = if (settings.save_cover || settings.embed_cover) && !releases.is_empty() {
        match Identifier::new(endpoints.clone(), String::new(), settings.min_score) {
            Ok(identifier) => import::fetch_covers(&identifier, releases).await,
            Err(error) => {
                tracing::warn!(%error, "cannot fetch covers");
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };

    let layout = Layout::from_settings(settings);
    let mut planned = HashSet::new();
    let mut album_dirs: HashMap<String, std::path::PathBuf> = HashMap::new();
    let mut group_dirs = HashSet::new();
    for chunk in files.chunks(LOCK_CHUNK) {
        let lock = LibraryLock::acquire(pool, "librarian").await?;
        for file in chunk {
            group_dirs.insert(file.group_id);
            let release = file.release().cloned();
            let cover = release
                .as_ref()
                .and_then(|release| covers.get(release))
                .filter(|_| settings.embed_cover && !file.has_cover);
            let retag = (file.applied != file.own || cover.is_some()).then(|| Retag {
                change: TagChange::Tags {
                    previous: file.own.clone(),
                    applied: file.applied.clone(),
                },
                restore: None,
                cover: cover.map_or(RetagCover::Keep, |jpeg| RetagCover::Embed(jpeg.clone())),
            });
            let to = plan_path(
                pool,
                roots,
                &layout,
                &file.applied,
                &file.staged_path,
                &mut planned,
            )
            .await?;
            let operation = Operation {
                batch_id,
                subject: Subject::New(Box::new(NewTrack {
                    tags: file.applied.clone(),
                    duration_ms: file.duration_ms,
                    chromaprint: file.chromaprint.clone().map(Chromaprint::from_compressed),
                    identification: file.track_identification(),
                    prepared_from: Some(file.id),
                })),
                from: Origin::Staging(file.staged_path.clone()),
                to: to.clone(),
                retag,
                expected_stamp: None,
            };
            let outcome = journal::execute(pool, roots, operation).await;
            let (status, track_id, error) = match &outcome {
                Ok(track_id) => ("imported", Some(*track_id), None),
                Err(OpError::Database(_)) => ("importing", None, None),
                Err(OpError::SourceMissing) => {
                    ("failed", None, Some("the file is gone".to_owned()))
                }
                Err(error) => ("ready", None, Some(error.to_string())),
            };
            if let Err(OpError::Database(error)) = outcome {
                // Recovery settles the file on restart.
                return Err(error.into());
            }
            sqlx::query("UPDATE staged_files SET status = ?, track_id = ?, error = ? WHERE id = ?")
                .bind(status)
                .bind(track_id)
                .bind(&error)
                .bind(file.id)
                .execute(pool)
                .await?;
            match error {
                None => {
                    report.done += 1;
                    tracing::info!(from = %file.staged_path, %to, "imported from the buffer zone");
                    if let (Some(release), Some(parent)) =
                        (release, to.resolve(&roots.library).parent())
                    {
                        album_dirs
                            .entry(release)
                            .or_insert_with(|| parent.to_path_buf());
                    }
                }
                Some(reason) => report.skipped.push(SkippedFile {
                    path: file.staged_path.to_string(),
                    reason,
                }),
            }
        }
        lock.release().await?;
    }

    // Kept in Heron too, for the albums that have no better cover.
    for (release, jpeg) in &covers {
        crate::covers::keep_release_cover(pool, &roots.covers, release, jpeg).await?;
    }
    if settings.save_cover {
        for (release, dir) in album_dirs {
            if let Some(jpeg) = covers.get(&release) {
                import::save_cover(&dir, jpeg);
            }
        }
    }
    let dirs: Vec<_> = group_dirs
        .into_iter()
        .map(|group| roots.staging.join(group.to_string()))
        .collect();
    tokio::task::spawn_blocking(move || dirs.iter().for_each(|dir| fsops::prune_empty_dirs(dir)))
        .await?;
    journal::finish_batch(pool, batch_id).await?;
    if report.done > 0 {
        let analysis = heron_analysis::jobs::load_settings(pool).await?;
        heron_analysis::jobs::enqueue_stale(pool, &analysis).await?;
    }
    metrics::counter!("heron_librarian_imports_total", "result" => "imported")
        .increment(report.done);
    tracing::info!(
        batch_id,
        done = report.done,
        skipped = report.skipped.len(),
        "files imported from the buffer zone"
    );
    Ok(report)
}

async fn load_claimed(pool: &MySqlPool, id: u64) -> Result<ToImport, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, group_id, staged_path, CAST(tags AS CHAR) AS tags,
                CAST(applied_tags AS CHAR) AS applied, duration_ms, has_cover, chromaprint,
                CAST(identification AS CHAR) AS identification, CAST(choice AS CHAR) AS choice
         FROM staged_files WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    let parse = |column: &str| -> Option<String> { row.try_get(column).ok().flatten() };
    let own: Tags = parse("tags")
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let applied = parse("applied")
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_else(|| own.clone());
    Ok(ToImport {
        id: row.try_get("id")?,
        group_id: row.try_get("group_id")?,
        staged_path: LibraryPath::from_stored(row.try_get("staged_path")?),
        own,
        applied,
        duration_ms: row.try_get("duration_ms")?,
        has_cover: row.try_get("has_cover")?,
        chromaprint: row.try_get("chromaprint")?,
        identification: parse("identification").and_then(|json| serde_json::from_str(&json).ok()),
        choice: parse("choice").and_then(|json| serde_json::from_str(&json).ok()),
    })
}

/// What [`recover`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StagingRecovery {
    /// Moves from the incoming folder finished.
    pub moved: u64,
    /// Moves from the incoming folder rolled back (the file stayed there).
    pub abandoned: u64,
    /// Imports settled from the journal.
    pub imports: u64,
}

/// Finishes or rolls back moves into the buffer zone and imports out of it
/// that a crash interrupted. Run it at startup, after
/// [`journal::recover`], holding the library lock.
pub async fn recover(pool: &MySqlPool, roots: &Roots) -> Result<StagingRecovery, sqlx::Error> {
    let mut recovery = StagingRecovery::default();
    let moves: Vec<(u64, u64, String, String)> = sqlx::query_as(
        "SELECT f.id, f.group_id, f.staged_path, f.original_name FROM staged_files f
         JOIN staged_groups g ON g.id = f.group_id
         WHERE f.status = 'receiving' AND g.source = 'incoming'",
    )
    .fetch_all(pool)
    .await?;
    for (id, group, staged, original) in moves {
        let to = LibraryPath::from_stored(staged).resolve(&roots.staging);
        let from = LibraryPath::from_stored(original).resolve(&roots.incoming);
        let _ = std::fs::remove_file(
            LibraryPath::from_stored(format!("{group}/.heron-{id}")).resolve(&roots.staging),
        );
        if to.is_file() {
            // A copy is renamed into place only once verified: the original
            // can go, unless it changed meanwhile (then both stay).
            if from.is_file()
                && (fsops::same_file(&from, &to).unwrap_or(false)
                    || matches!(
                        (fsops::full_hash(&from), fsops::full_hash(&to)),
                        (Ok(a), Ok(b)) if a == b
                    ))
            {
                let _ = std::fs::remove_file(&from);
            }
            sqlx::query("UPDATE staged_files SET status = 'identifying' WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
            recovery.moved += 1;
        } else {
            sqlx::query("DELETE FROM staged_files WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
            recovery.abandoned += 1;
        }
    }

    let importing: Vec<(u64, String)> =
        sqlx::query_as("SELECT id, staged_path FROM staged_files WHERE status = 'importing'")
            .fetch_all(pool)
            .await?;
    for (id, staged) in importing {
        let journaled: Option<(Option<u64>, String)> = sqlx::query_as(
            "SELECT track_id, CAST(status AS CHAR) FROM library_moves
             WHERE from_root = 'staging' AND from_path = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(&staged)
        .fetch_optional(pool)
        .await?;
        let (status, track_id, error) = match journaled {
            Some((track_id, status)) if status == "done" || status == "moved" => {
                ("imported", track_id, None)
            }
            _ if LibraryPath::from_stored(staged.clone())
                .resolve(&roots.staging)
                .is_file() =>
            {
                ("ready", None, Some("interrupted: import it again"))
            }
            _ => ("failed", None, Some("the file is gone")),
        };
        sqlx::query("UPDATE staged_files SET status = ?, track_id = ?, error = ? WHERE id = ?")
            .bind(status)
            .bind(track_id)
            .bind(error)
            .bind(id)
            .execute(pool)
            .await?;
        recovery.imports += 1;
    }
    sqlx::query(
        "UPDATE library_tasks SET status = 'failed', finished_at = NOW(6),
             error = 'interrupted: heron-librarian stopped while it ran'
         WHERE status = 'running' AND kind = 'staging_import'",
    )
    .execute(pool)
    .await?;
    if recovery != StagingRecovery::default() {
        tracing::info!(?recovery, "recovered interrupted buffer zone operations");
    }
    Ok(recovery)
}

/// Drops what a discarded file was prepared with.
async fn forget_preparation(pool: &MySqlPool, id: u64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM staged_file_categories WHERE staged_file_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM staged_file_labels WHERE staged_file_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE staged_files SET mood = NULL, sound_code = NULL WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Gives up uploads that made no progress for a day, and removes the
/// folders of groups with nothing left to import (files discarded or
/// imported).
pub async fn cleanup(pool: &MySqlPool, roots: &Roots) -> Result<(), sqlx::Error> {
    let abandoned: Vec<(u64, u64)> = sqlx::query_as(
        "SELECT f.id, f.group_id FROM staged_files f
         JOIN staged_groups g ON g.id = f.group_id
         WHERE f.status = 'receiving' AND g.source = 'upload'
           AND f.updated_at < NOW(6) - INTERVAL ? HOUR",
    )
    .bind(ABANDONED_UPLOAD_HOURS)
    .fetch_all(pool)
    .await?;
    for (id, group) in abandoned {
        let part = roots
            .staging
            .join(group.to_string())
            .join(heron_core::library::upload_part_name(id));
        let _ = std::fs::remove_file(part);
        let given_up = sqlx::query(
            "UPDATE staged_files SET status = 'discarded', error = 'upload abandoned'
             WHERE id = ? AND status = 'receiving'",
        )
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
        if given_up > 0 {
            forget_preparation(pool, id).await?;
        }
    }
    let active: HashSet<u64> = sqlx::query_scalar(
        "SELECT DISTINCT group_id FROM staged_files
         WHERE status IN ('receiving', 'identifying', 'ready', 'importing', 'failed')",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let staging = roots.staging.clone();
    let folders = tokio::task::spawn_blocking(move || fsops::numbered_dirs(&staging))
        .await
        .unwrap_or_default();
    for (group, folder) in folders {
        if active.contains(&group) {
            continue;
        }
        // A group created a moment ago, whose first upload has no row yet.
        let known: Option<u64> = sqlx::query_scalar(
            "SELECT id FROM staged_groups WHERE id = ? AND created_at > NOW(6) - INTERVAL 1 HOUR",
        )
        .bind(group)
        .fetch_optional(pool)
        .await?;
        if known.is_some() {
            continue;
        }
        let removed = tokio::task::spawn_blocking(move || fsops::remove_tree(&folder)).await;
        if let Ok(Err(error)) = removed {
            tracing::warn!(group, %error, "cannot remove a folder of the buffer zone");
        }
    }
    Ok(())
}
