// SPDX-License-Identifier: AGPL-3.0-or-later

//! Covers from the Cover Art Archive for albums without one
//! (docs/library.md#the-cover-art-archive), at one request a second.
//!
//! An album with a MusicBrainz release ID gets that release's front cover.
//! Otherwise MusicBrainz is searched for the album and its artist: a sure
//! result (score [`SURE_SCORE`] or more, with a cover) is applied; an
//! unsure search leaves its results as candidates for the review screen.

use heron_core::library::CoversReport;
use heron_library::covers::{self, Link, Source};
use sqlx::{MySqlPool, QueryBuilder};

use crate::identify::{Identifier, IdentifyError, ReleaseHit};

/// Lowest MusicBrainz score of a search result applied without review.
pub const SURE_SCORE: u8 = 90;
/// Lowest score of a result kept as a candidate.
pub const CANDIDATE_SCORE: u8 = 50;
pub const MAX_CANDIDATES: usize = 5;
/// Sure results tried for a cover, best first.
const SURE_TRIES: usize = 3;
/// Albums looked up per background pass.
pub const BACKGROUND_ALBUMS: usize = 30;

/// Which albums to look up.
pub enum Albums<'a> {
    /// The albums of these tracks (looked up even when they were before).
    Of(&'a [u64]),
    /// Every album without a cover.
    AllMissing,
    /// Albums without a cover never looked up, at most this many.
    NeverLookedUp(usize),
}

struct Album {
    key: String,
    release_id: Option<String>,
    album: Option<String>,
    artist: Option<String>,
}

enum Outcome {
    Found { release_id: String, jpeg: Vec<u8> },
    Candidates(Vec<ReleaseHit>),
    NotFound,
}

/// Looks albums up. With `candidates`, the results are left for an
/// operator to pick from, even when one is sure, and albums with a cover
/// are looked up too.
pub async fn look_up(
    pool: &MySqlPool,
    identifier: &Identifier,
    albums: Albums<'_>,
    candidates: bool,
) -> Result<CoversReport, sqlx::Error> {
    let mut report = CoversReport::default();
    for album in albums_to_look_up(pool, &albums, candidates).await? {
        report.albums += 1;
        match outcome(identifier, &album, candidates).await {
            Ok(Outcome::Found { release_id, jpeg }) => {
                let mut connection = pool.acquire().await?;
                match covers::store(&mut connection, jpeg).await {
                    Ok(hash) => {
                        let link = Link {
                            hash,
                            source: Source::CoverArtArchive,
                            detail: Some(release_id),
                        };
                        covers::set_album(&mut connection, &album.key, &link, None).await?;
                        record(pool, &album.key, "found", None).await?;
                        report.found += 1;
                    }
                    Err(covers::CoverError::Database(error)) => return Err(error),
                    Err(error) => {
                        tracing::warn!(album = album.key, %error, "cannot read the cover");
                        record(pool, &album.key, "failed", Some(&error.to_string())).await?;
                        report.failed += 1;
                    }
                }
            }
            Ok(Outcome::Candidates(hits)) => {
                save_candidates(pool, identifier, &album.key, &hits).await?;
                record(pool, &album.key, "candidates", None).await?;
                report.candidates += 1;
            }
            Ok(Outcome::NotFound) => {
                record(pool, &album.key, "not_found", None).await?;
                report.not_found += 1;
            }
            Err(error) => {
                tracing::warn!(album = album.key, %error, "cannot look the cover up");
                record(pool, &album.key, "failed", Some(&error.to_string())).await?;
                report.failed += 1;
            }
        }
    }
    if report.albums > 0 {
        tracing::info!(?report, "covers looked up on the Cover Art Archive");
    }
    Ok(report)
}

async fn albums_to_look_up(
    pool: &MySqlPool,
    albums: &Albums<'_>,
    candidates: bool,
) -> Result<Vec<Album>, sqlx::Error> {
    let mut query = QueryBuilder::new(
        "SELECT t.album_key, MAX(TRIM(t.musicbrainz_release_id)), MAX(t.album),
                MAX(COALESCE(NULLIF(TRIM(t.album_artist), ''), t.artist))
         FROM tracks t LEFT JOIN album_covers ac ON ac.album_key = t.album_key
         WHERE t.album_key IS NOT NULL AND t.deleted_at IS NULL",
    );
    if !candidates {
        query.push(" AND ac.album_key IS NULL");
    }
    let mut limit = None;
    match albums {
        Albums::Of(ids) => {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            query.push(" AND t.album_key IN (SELECT album_key FROM tracks WHERE id IN (");
            let mut separated = query.separated(", ");
            for id in *ids {
                separated.push_bind(*id);
            }
            query.push("))");
        }
        Albums::AllMissing => {}
        Albums::NeverLookedUp(most) => {
            query.push(
                " AND NOT EXISTS (SELECT 1 FROM cover_lookups l WHERE l.album_key = t.album_key)",
            );
            limit = Some(*most);
        }
    }
    query.push(" GROUP BY t.album_key ORDER BY t.album_key");
    if let Some(limit) = limit {
        query.push(" LIMIT ").push_bind(limit as u64);
    }
    let rows: Vec<(String, Option<String>, Option<String>, Option<String>)> =
        query.build_query_as().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(key, release_id, album, artist)| Album {
            key,
            release_id: release_id.filter(|id| !id.is_empty()),
            album,
            artist,
        })
        .collect())
}

async fn outcome(
    identifier: &Identifier,
    album: &Album,
    candidates: bool,
) -> Result<Outcome, IdentifyError> {
    if let (Some(release_id), false) = (&album.release_id, candidates)
        && let Some(jpeg) = identifier.cover(release_id).await?
    {
        return Ok(Outcome::Found {
            release_id: release_id.clone(),
            jpeg,
        });
    }
    let (Some(title), Some(artist)) = (&album.album, &album.artist) else {
        return Ok(Outcome::NotFound);
    };
    let hits = identifier.search_releases(title, artist).await?;
    if !candidates {
        for hit in hits
            .iter()
            .filter(|hit| hit.score >= SURE_SCORE)
            .take(SURE_TRIES)
        {
            if let Some(jpeg) = identifier.cover(&hit.id).await? {
                return Ok(Outcome::Found {
                    release_id: hit.id.clone(),
                    jpeg,
                });
            }
        }
    }
    let kept: Vec<ReleaseHit> = hits
        .into_iter()
        .filter(|hit| hit.score >= CANDIDATE_SCORE)
        .take(MAX_CANDIDATES)
        .collect();
    Ok(if kept.is_empty() {
        Outcome::NotFound
    } else {
        Outcome::Candidates(kept)
    })
}

async fn record(
    pool: &MySqlPool,
    album_key: &str,
    status: &str,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    let error = error.map(|error| error.chars().take(500).collect::<String>());
    sqlx::query(
        "INSERT INTO cover_lookups (album_key, status, error) VALUES (?, ?, ?)
         ON DUPLICATE KEY UPDATE status = VALUES(status), error = VALUES(error),
                                 looked_up_at = NOW(6)",
    )
    .bind(album_key)
    .bind(status)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// The results of an unsure search, pending again when they were refused
/// before (an operator asked again).
async fn save_candidates(
    pool: &MySqlPool,
    identifier: &Identifier,
    album_key: &str,
    hits: &[ReleaseHit],
) -> Result<(), sqlx::Error> {
    let cut = |text: &str, chars: usize| text.chars().take(chars).collect::<String>();
    for hit in hits {
        sqlx::query(
            "INSERT INTO cover_candidates
                 (album_key, release_id, title, artist, date, country, score, thumbnail_url)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON DUPLICATE KEY UPDATE title = VALUES(title), artist = VALUES(artist),
                 date = VALUES(date), country = VALUES(country), score = VALUES(score),
                 thumbnail_url = VALUES(thumbnail_url), status = 'pending'",
        )
        .bind(album_key)
        .bind(&hit.id)
        .bind(cut(&hit.title, 512))
        .bind(cut(&hit.artist, 512))
        .bind(hit.date.as_deref().map(|date| cut(date, 32)))
        .bind(hit.country.as_deref().map(|country| cut(country, 8)))
        .bind(hit.score)
        .bind(identifier.cover_thumbnail_url(&hit.id))
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Gives the albums of the tracks on `release_id` the release's cover,
/// fetched for an import, when they have none from a better source.
pub async fn keep_release_cover(
    pool: &MySqlPool,
    release_id: &str,
    jpeg: &[u8],
) -> Result<(), sqlx::Error> {
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT album_key FROM tracks
         WHERE TRIM(musicbrainz_release_id) = ? AND album_key IS NOT NULL",
    )
    .bind(release_id)
    .fetch_all(pool)
    .await?;
    if keys.is_empty() {
        return Ok(());
    }
    let mut connection = pool.acquire().await?;
    let hash = match covers::store(&mut connection, jpeg.to_vec()).await {
        Ok(hash) => hash,
        Err(covers::CoverError::Database(error)) => return Err(error),
        Err(error) => {
            tracing::warn!(release_id, %error, "cannot read the release's cover");
            return Ok(());
        }
    };
    let link = Link {
        hash,
        source: Source::CoverArtArchive,
        detail: Some(release_id.to_owned()),
    };
    for key in keys {
        if covers::album_cover(&mut *connection, &key).await?.is_none() {
            covers::set_album(&mut connection, &key, &link, None).await?;
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("candidate {0} not found")]
    NotFound(u64),
    #[error("the Cover Art Archive has no front cover for release {0}")]
    NoCover(String),
    #[error("cannot fetch the cover: {0}")]
    Identify(#[from] IdentifyError),
    #[error(transparent)]
    Cover(#[from] covers::CoverError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Fetches a candidate picked by an operator, for its album or for
/// `track_id` only; the album's other candidates are refused.
pub async fn apply(
    pool: &MySqlPool,
    identifier: &Identifier,
    candidate_id: u64,
    track_id: Option<u64>,
) -> Result<CoversReport, ApplyError> {
    let candidate: Option<(String, String)> =
        sqlx::query_as("SELECT album_key, release_id FROM cover_candidates WHERE id = ?")
            .bind(candidate_id)
            .fetch_optional(pool)
            .await?;
    let (album_key, release_id) = candidate.ok_or(ApplyError::NotFound(candidate_id))?;
    let jpeg = identifier
        .cover(&release_id)
        .await?
        .ok_or_else(|| ApplyError::NoCover(release_id.clone()))?;
    let mut connection = pool.acquire().await?;
    let hash = covers::store(&mut connection, jpeg).await?;
    let link = Link {
        hash,
        source: Source::CoverArtArchive,
        detail: Some(release_id),
    };
    match track_id {
        Some(track_id) => covers::set_track(&mut connection, track_id, &link, None).await?,
        None => covers::set_album(&mut connection, &album_key, &link, None).await?,
    }
    sqlx::query(
        "UPDATE cover_candidates SET status = IF(id = ?, 'accepted', 'refused')
         WHERE album_key = ? AND (status = 'pending' OR id = ?)",
    )
    .bind(candidate_id)
    .bind(&album_key)
    .bind(candidate_id)
    .execute(&mut *connection)
    .await?;
    record(pool, &album_key, "found", None).await?;
    Ok(CoversReport {
        albums: 1,
        found: 1,
        ..CoversReport::default()
    })
}
