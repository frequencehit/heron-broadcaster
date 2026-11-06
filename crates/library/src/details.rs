// SPDX-License-Identifier: AGPL-3.0-or-later

//! The track columns read from a file: the tags shown and edited in the
//! library, and the audio properties. Saved by the scanner for new and
//! changed files, and by the librarian for the files it writes.
//!
//! Tags edited while the library setting `write_tags` is off are kept in
//! `track_tag_overrides` instead of the files: [`save`] applies them on top
//! of what the file says, so they survive a rescan.

use std::collections::HashMap;
use std::path::Path;

use heron_core::library::{FieldValues, TagField};
use sqlx::{MySqlConnection, MySqlPool, Row};

use crate::covers;
use crate::tags::FileTags;

/// What the scanner reads from files, stored in `tracks.details_version`.
/// Tracks read by an older version are read again at the next scan,
/// without being analyzed again.
///
/// - 1: M5b, album artist, genre, year, track and disc numbers, MusicBrainz
///   recording and release IDs, codec, bitrate, sample rate, channels.
/// - 2: cover art, embedded or beside the file.
pub const DETAILS_VERSION: u8 = 2;

/// Saves the tags and properties of `read` to the track `id`, with the
/// track's overrides applied on top, then its covers ([`covers::from_file`]):
/// the one embedded in it, and one beside `file` (its absolute path) when
/// given. A cover that cannot be read is logged, and left out.
pub async fn save(
    connection: &mut MySqlConnection,
    id: u64,
    read: &FileTags,
    file: Option<&Path>,
) -> Result<(), sqlx::Error> {
    let overrides = overrides(&mut *connection, id).await?;
    let mut read = read.clone();
    apply_overrides(&mut read, &overrides);
    let read = &read;
    let tags = &read.tags;
    let properties = &read.properties;
    let small = |number: Option<u32>| number.and_then(|number| u16::try_from(number).ok());
    let mbid = |id: &Option<String>| {
        id.as_deref()
            .map(|id| id.trim().to_ascii_lowercase())
            .filter(|id| id.len() == 36)
    };
    sqlx::query(
        "UPDATE tracks
         SET title = ?, artist = ?, album = ?, album_artist = ?, genre = ?, year = ?,
             track_number = ?, disc_number = ?, musicbrainz_artist_id = ?,
             musicbrainz_recording_id = ?, musicbrainz_release_id = ?,
             duration_ms = COALESCE(?, duration_ms), codec = ?, bitrate_kbps = ?,
             bitrate_mode = ?, sample_rate = ?, channels = ?, details_version = ?
         WHERE id = ?",
    )
    .bind(&tags.title)
    .bind(&tags.artist)
    .bind(&tags.album)
    .bind(&tags.album_artist)
    .bind(&read.genre)
    .bind(tags.year())
    .bind(small(tags.track))
    .bind(small(tags.disc))
    .bind(tags.musicbrainz.first_artist())
    .bind(mbid(&tags.musicbrainz.recording))
    .bind(mbid(&tags.musicbrainz.release))
    .bind(read.duration_ms)
    .bind(&properties.codec)
    .bind(properties.bitrate_kbps)
    .bind(properties.bitrate_mode.map(|mode| mode.as_str()))
    .bind(properties.sample_rate)
    .bind(properties.channels)
    .bind(DETAILS_VERSION)
    .bind(id)
    .execute(&mut *connection)
    .await?;
    match covers::from_file(connection, id, file, read.cover.as_deref()).await {
        Ok(()) => Ok(()),
        Err(covers::CoverError::Database(error)) => Err(error),
        Err(error) => {
            tracing::warn!(track_id = id, %error, "cannot read the track's covers");
            Ok(())
        }
    }
}

/// The tags kept in Heron's database for the track `id`.
pub async fn overrides(
    executor: impl sqlx::MySqlExecutor<'_>,
    id: u64,
) -> Result<FieldValues, sqlx::Error> {
    let rows = sqlx::query("SELECT field, value FROM track_tag_overrides WHERE track_id = ?")
        .bind(id)
        .fetch_all(executor)
        .await?;
    let mut overrides = FieldValues::new();
    for row in rows {
        if let Some((field, value)) = override_row(&row)? {
            overrides.insert(field, value);
        }
    }
    Ok(overrides)
}

/// The overrides of every track that has some, for work on the whole
/// library (organizing it).
pub async fn all_overrides(pool: &MySqlPool) -> Result<HashMap<u64, FieldValues>, sqlx::Error> {
    let rows = sqlx::query("SELECT track_id, field, value FROM track_tag_overrides")
        .fetch_all(pool)
        .await?;
    let mut all: HashMap<u64, FieldValues> = HashMap::new();
    for row in rows {
        if let Some((field, value)) = override_row(&row)? {
            all.entry(row.try_get("track_id")?)
                .or_default()
                .insert(field, value);
        }
    }
    Ok(all)
}

fn override_row(
    row: &sqlx::mysql::MySqlRow,
) -> Result<Option<(TagField, Option<String>)>, sqlx::Error> {
    let name: String = row.try_get("field")?;
    let Some(field) = TagField::from_name(&name) else {
        tracing::warn!(field = name, "unknown tag override, ignored");
        return Ok(None);
    };
    Ok(Some((field, row.try_get("value")?)))
}

/// Puts `overrides` in place of what the file says. `None` removes the
/// field. Programming details are not tags of the `tracks` table, and are
/// left alone.
pub fn apply_overrides(read: &mut FileTags, overrides: &FieldValues) {
    for (&field, value) in overrides {
        let number = || value.as_deref().and_then(|value| value.parse().ok());
        let tags = &mut read.tags;
        match field {
            TagField::Title => tags.title.clone_from(value),
            TagField::Artist => tags.artist.clone_from(value),
            TagField::Album => tags.album.clone_from(value),
            TagField::AlbumArtist => tags.album_artist.clone_from(value),
            TagField::Year => tags.date.clone_from(value),
            TagField::Genre => read.genre.clone_from(value),
            TagField::Track => tags.track = number(),
            TagField::Disc => tags.disc = number(),
            TagField::Isrc | TagField::RecordLabel | TagField::Bpm | TagField::Mood => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tags;

    #[test]
    fn overrides_replace_the_files_tags() {
        let mut read = FileTags {
            tags: Tags {
                title: Some("Tilte".into()),
                artist: Some("Nina Simone".into()),
                album: Some("Pastel Blues".into()),
                date: Some("1965-10-01".into()),
                track: Some(4),
                disc: Some(1),
                ..Tags::default()
            },
            genre: Some("Jazz".into()),
            ..FileTags::default()
        };
        let overrides = FieldValues::from([
            (TagField::Title, Some("Sinnerman".to_owned())),
            (TagField::Year, Some("1965".to_owned())),
            (TagField::Track, Some("9".to_owned())),
            (TagField::Disc, None),
            (TagField::Genre, None),
            (TagField::Isrc, Some("USRC17607839".to_owned())),
        ]);
        apply_overrides(&mut read, &overrides);
        assert_eq!(read.tags.title.as_deref(), Some("Sinnerman"));
        assert_eq!(read.tags.year(), Some(1965));
        assert_eq!(read.tags.track, Some(9));
        assert_eq!(read.tags.disc, None);
        assert_eq!(read.genre, None);
        // Untouched.
        assert_eq!(read.tags.artist.as_deref(), Some("Nina Simone"));
        assert_eq!(read.tags.album.as_deref(), Some("Pastel Blues"));
    }
}
