// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tags Heron reads from files and writes after identification, and
//! the fields edited from the browser.
//!
//! Only these fields are written; every other tag in the file (comments,
//! lyrics, pictures, ReplayGain, …) is left alone. Writing a [`Tags`] whose
//! field is `None` removes that field, so writing back the tags read before
//! a change undoes it. [`write_fields`] writes only the fields it is given.

use std::path::Path;

use heron_core::library::{FieldValues, TagField};

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, Tag};
use serde::{Deserialize, Serialize};

use crate::properties::AudioProperties;

#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("cannot read the tags: {0}")]
    Read(#[from] lofty::error::FileParseError),
    #[error("cannot write the tags: {0}")]
    Write(#[from] lofty::error::FileEncodingError),
}

/// Longest tag value kept (the width of the `tracks` columns).
pub const MAX_TAG_CHARS: usize = 512;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
    pub disc_total: Option<u32>,
    /// As written in the file: `1997`, `1997-05-21`, …
    pub date: Option<String>,
    /// Part of a various-artists compilation.
    pub compilation: bool,
    pub musicbrainz: MusicBrainzIds,
}

/// MusicBrainz identifiers, named after the MusicBrainz entities (the tag
/// names differ: Picard's `MUSICBRAINZ_TRACKID` holds the recording ID).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicBrainzIds {
    pub recording: Option<String>,
    pub release_track: Option<String>,
    pub release: Option<String>,
    pub release_group: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
}

impl MusicBrainzIds {
    /// The first artist ID: the tag may hold several (`id1; id2`, `id1/id2`).
    pub fn first_artist(&self) -> Option<String> {
        let first = self
            .artist
            .as_deref()?
            .split([';', '/', ','])
            .next()?
            .trim()
            .to_ascii_lowercase();
        (first.len() == 36).then_some(first)
    }
}

impl Tags {
    /// The year of [`Tags::date`], when it starts with one.
    pub fn year(&self) -> Option<u16> {
        let date = self.date.as_deref()?;
        let year = date.get(..4)?;
        year.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| year.parse().ok())
            .flatten()
    }
}

/// What [`read`] finds in a file.
#[derive(Clone, Debug, Default)]
pub struct FileTags {
    pub tags: Tags,
    pub genre: Option<String>,
    pub duration_ms: Option<u32>,
    pub properties: AudioProperties,
    /// The file has embedded cover art.
    pub has_cover: bool,
    /// The embedded cover: the front cover, else the first picture (at
    /// most [`crate::covers::MAX_BYTES`]).
    pub cover: Option<Vec<u8>>,
}

/// Reads the tags, duration and audio properties of `path`.
pub fn read(path: &Path) -> Result<FileTags, TagError> {
    let tagged = lofty::read_from_path(path)?;
    let duration_ms = u32::try_from(tagged.properties().duration().as_millis())
        .ok()
        .filter(|&ms| ms > 0);
    let properties = crate::properties::of(path, &tagged);
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(FileTags {
            duration_ms,
            properties,
            ..FileTags::default()
        });
    };
    let text = |key: ItemKey| tag.get_string(key).and_then(clean);
    let tags = Tags {
        title: tag.title().and_then(|value| clean(&value)),
        artist: tag.artist().and_then(|value| clean(&value)),
        album_artist: text(ItemKey::AlbumArtist),
        album: tag.album().and_then(|value| clean(&value)),
        track: tag.track(),
        track_total: tag.track_total(),
        disc: tag.disk(),
        disc_total: tag.disk_total(),
        date: text(ItemKey::RecordingDate).or_else(|| text(ItemKey::Year)),
        compilation: tag
            .get_string(ItemKey::FlagCompilation)
            .is_some_and(|flag| flag.trim() == "1"),
        musicbrainz: MusicBrainzIds {
            recording: text(ItemKey::MusicBrainzRecordingId),
            release_track: text(ItemKey::MusicBrainzTrackId),
            release: text(ItemKey::MusicBrainzReleaseId),
            release_group: text(ItemKey::MusicBrainzReleaseGroupId),
            artist: text(ItemKey::MusicBrainzArtistId),
            album_artist: text(ItemKey::MusicBrainzReleaseArtistId),
        },
    };
    Ok(FileTags {
        tags,
        genre: tag.genre().and_then(|value| clean(&value)),
        duration_ms,
        properties,
        has_cover: tag.pictures().iter().any(|picture| {
            picture.pic_type() == PictureType::CoverFront
                || picture.pic_type() == PictureType::Other
        }),
        cover: embedded_cover(tag),
    })
}

/// The front cover, else the first picture.
fn embedded_cover(tag: &Tag) -> Option<Vec<u8>> {
    let pictures = tag.pictures();
    let usable = |picture: &&Picture| {
        !picture.data().is_empty() && picture.data().len() <= crate::covers::MAX_BYTES
    };
    pictures
        .iter()
        .filter(usable)
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.iter().find(usable))
        .map(|picture| picture.data().to_vec())
}

/// What to do with embedded cover art when writing tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover<'a> {
    Keep,
    /// Embed this JPEG as the front cover, replacing any front cover.
    Embed(&'a [u8]),
    /// Remove the embedded front cover (undoing an [`Cover::Embed`]).
    Remove,
}

/// Writes `tags` into `path` in place. Callers write into a copy and swap
/// it in, so an interruption never leaves a half-written file in the library.
pub fn write(path: &Path, tags: &Tags, cover: Cover<'_>) -> Result<(), TagError> {
    let mut tagged = lofty::read_from_path(path)?;
    let tag_type = tagged.primary_tag_type();
    if tagged.tag(tag_type).is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let Some(tag) = tagged.tag_mut(tag_type) else {
        return Ok(());
    };

    let set_text = |tag: &mut Tag, key: ItemKey, value: &Option<String>| match value {
        Some(value) => {
            tag.insert_text(key, value.clone());
        }
        None => tag.remove_key(key),
    };
    set_text(tag, ItemKey::TrackTitle, &tags.title);
    set_text(tag, ItemKey::TrackArtist, &tags.artist);
    set_text(tag, ItemKey::AlbumArtist, &tags.album_artist);
    set_text(tag, ItemKey::AlbumTitle, &tags.album);
    tag.remove_key(ItemKey::Year);
    set_text(tag, ItemKey::RecordingDate, &tags.date);
    let compilation = tags.compilation.then(|| "1".to_owned());
    set_text(tag, ItemKey::FlagCompilation, &compilation);
    let ids = &tags.musicbrainz;
    set_text(tag, ItemKey::MusicBrainzRecordingId, &ids.recording);
    set_text(tag, ItemKey::MusicBrainzTrackId, &ids.release_track);
    set_text(tag, ItemKey::MusicBrainzReleaseId, &ids.release);
    set_text(tag, ItemKey::MusicBrainzReleaseGroupId, &ids.release_group);
    set_text(tag, ItemKey::MusicBrainzArtistId, &ids.artist);
    set_text(tag, ItemKey::MusicBrainzReleaseArtistId, &ids.album_artist);

    match tags.track {
        Some(track) => tag.set_track(track),
        None => tag.remove_track(),
    }
    match tags.track_total {
        Some(total) => tag.set_track_total(total),
        None => tag.remove_track_total(),
    }
    match tags.disc {
        Some(disc) => tag.set_disk(disc),
        None => tag.remove_disk(),
    }
    match tags.disc_total {
        Some(total) => tag.set_disk_total(total),
        None => tag.remove_disk_total(),
    }

    match cover {
        Cover::Keep => {}
        Cover::Embed(jpeg) => {
            tag.remove_picture_type(PictureType::CoverFront);
            tag.push_picture(
                Picture::unchecked(jpeg.to_vec())
                    .pic_type(PictureType::CoverFront)
                    .mime_type(MimeType::Jpeg)
                    .build(),
            );
        }
        Cover::Remove => tag.remove_picture_type(PictureType::CoverFront),
    }

    Ok(tagged.save_to_path(path, WriteOptions::default())?)
}

/// Reads `fields` from `path`, as written in the file (a full date for
/// [`TagField::Year`]): what [`write_fields`] needs to put them back.
pub fn read_fields(
    path: &Path,
    fields: impl IntoIterator<Item = TagField>,
) -> Result<FieldValues, TagError> {
    let tagged = lofty::read_from_path(path)?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    Ok(fields
        .into_iter()
        .map(|field| (field, tag.and_then(|tag| field_value(tag, field))))
        .collect())
}

fn field_value(tag: &Tag, field: TagField) -> Option<String> {
    let text = |key: ItemKey| tag.get_string(key).and_then(clean);
    match field {
        TagField::Title => tag.title().and_then(|value| clean(&value)),
        TagField::Artist => tag.artist().and_then(|value| clean(&value)),
        TagField::Album => tag.album().and_then(|value| clean(&value)),
        TagField::AlbumArtist => text(ItemKey::AlbumArtist),
        TagField::Year => text(ItemKey::RecordingDate).or_else(|| text(ItemKey::Year)),
        TagField::Genre => tag.genre().and_then(|value| clean(&value)),
        TagField::Track => tag.track().map(|track| track.to_string()),
        TagField::Disc => tag.disk().map(|disc| disc.to_string()),
        TagField::Isrc => text(ItemKey::Isrc),
        TagField::RecordLabel => text(label_key(tag)),
        TagField::Bpm => text(ItemKey::Bpm).or_else(|| text(ItemKey::IntegerBpm)),
        TagField::Mood => text(ItemKey::Mood),
    }
}

/// The record label: ID3v2 has only a publisher frame (`TPUB`), the other
/// formats a label of their own.
fn label_key(tag: &Tag) -> ItemKey {
    if tag.tag_type() == lofty::tag::TagType::Id3v2 {
        ItemKey::Publisher
    } else {
        ItemKey::Label
    }
}

/// Writes `values` into `path` in place, leaving every other tag alone; a
/// `None` value removes the field. Callers write into a copy and swap it
/// in, like [`write`].
pub fn write_fields(path: &Path, values: &FieldValues) -> Result<(), TagError> {
    let mut tagged = lofty::read_from_path(path)?;
    let tag_type = tagged.primary_tag_type();
    if tagged.tag(tag_type).is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let Some(tag) = tagged.tag_mut(tag_type) else {
        return Ok(());
    };
    let set_text = |tag: &mut Tag, key: ItemKey, value: &Option<String>| {
        tag.remove_key(key);
        if let Some(value) = value {
            tag.insert_text(key, value.clone());
        }
    };
    let number = |value: &Option<String>| value.as_deref().and_then(|value| value.parse().ok());
    for (&field, value) in values {
        match field {
            TagField::Title => set_text(tag, ItemKey::TrackTitle, value),
            TagField::Artist => set_text(tag, ItemKey::TrackArtist, value),
            TagField::Album => set_text(tag, ItemKey::AlbumTitle, value),
            TagField::AlbumArtist => set_text(tag, ItemKey::AlbumArtist, value),
            TagField::Year => {
                tag.remove_key(ItemKey::Year);
                set_text(tag, ItemKey::RecordingDate, value);
            }
            TagField::Genre => set_text(tag, ItemKey::Genre, value),
            TagField::Track => match number(value) {
                Some(track) => tag.set_track(track),
                None => tag.remove_track(),
            },
            TagField::Disc => match number(value) {
                Some(disc) => tag.set_disk(disc),
                None => tag.remove_disk(),
            },
            TagField::Isrc => set_text(tag, ItemKey::Isrc, value),
            TagField::RecordLabel => {
                let key = label_key(tag);
                set_text(tag, key, value);
            }
            TagField::Bpm => {
                tag.remove_key(ItemKey::Bpm);
                tag.remove_key(ItemKey::IntegerBpm);
                if let Some(bpm) = value {
                    // ID3v2 and MP4 only have whole tempos; Vorbis comments
                    // keep the decimal.
                    let whole = bpm
                        .parse::<f64>()
                        .map_or_else(|_| bpm.clone(), |bpm| format!("{:.0}", bpm.round()));
                    if !tag.insert_text(ItemKey::IntegerBpm, whole) {
                        tag.insert_text(ItemKey::Bpm, bpm.clone());
                    }
                }
            }
            TagField::Mood => set_text(tag, ItemKey::Mood, value),
        }
    }
    Ok(tagged.save_to_path(path, WriteOptions::default())?)
}

/// Trims a tag value and caps its length. Empty values count as missing.
pub fn clean(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('\0').trim();
    (!value.is_empty()).then(|| value.chars().take(MAX_TAG_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_artist_id() {
        let ids = |artist: &str| MusicBrainzIds {
            artist: Some(artist.to_owned()),
            ..MusicBrainzIds::default()
        };
        let id = "83d91898-7763-47d7-b03b-b92132375c47";
        assert_eq!(ids(id).first_artist().as_deref(), Some(id));
        let two = format!(
            "{}; 7a3b1c2d-0000-4000-8000-000000000005",
            id.to_uppercase()
        );
        assert_eq!(ids(&two).first_artist().as_deref(), Some(id));
        assert_eq!(ids("not an id").first_artist(), None);
        assert_eq!(MusicBrainzIds::default().first_artist(), None);
    }

    fn write_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for n in 0..8_000 {
            writer.write_sample((n % 100) as i16 * 100).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn full_tags() -> Tags {
        Tags {
            title: Some("So What".into()),
            artist: Some("Miles Davis".into()),
            album_artist: Some("Miles Davis".into()),
            album: Some("Kind of Blue".into()),
            track: Some(1),
            track_total: Some(5),
            disc: Some(1),
            disc_total: Some(1),
            date: Some("1959-08-17".into()),
            compilation: false,
            musicbrainz: MusicBrainzIds {
                recording: Some("7a3b1c2d-0000-4000-8000-000000000001".into()),
                release_track: Some("7a3b1c2d-0000-4000-8000-000000000002".into()),
                release: Some("7a3b1c2d-0000-4000-8000-000000000003".into()),
                release_group: Some("7a3b1c2d-0000-4000-8000-000000000004".into()),
                artist: Some("7a3b1c2d-0000-4000-8000-000000000005".into()),
                album_artist: Some("7a3b1c2d-0000-4000-8000-000000000005".into()),
            },
        }
    }

    #[test]
    fn written_tags_read_back() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.wav", "b.mp3"] {
            let path = dir.path().join(name);
            if name.ends_with("wav") {
                write_wav(&path);
            } else {
                std::fs::copy(
                    concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../analysis/tests/fixtures/silence-1s-sine-3s.mp3"
                    ),
                    &path,
                )
                .unwrap();
            }
            write(&path, &full_tags(), Cover::Keep).unwrap();
            let read = read(&path).unwrap();
            assert_eq!(read.tags, full_tags(), "{name}");
            assert_eq!(read.tags.year(), Some(1959));
            assert!(read.duration_ms.is_some());
        }
    }

    #[test]
    fn writing_previous_tags_undoes_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_wav(&path);
        let previous = Tags {
            title: Some("track 1".into()),
            ..Tags::default()
        };
        write(&path, &previous, Cover::Keep).unwrap();
        write(&path, &full_tags(), Cover::Embed(b"\xff\xd8\xff\xe0fake")).unwrap();
        let embedded = read(&path).unwrap();
        assert!(embedded.has_cover);
        assert_eq!(embedded.cover.as_deref(), Some(&b"\xff\xd8\xff\xe0fake"[..]));

        write(&path, &previous, Cover::Remove).unwrap();
        let read = read(&path).unwrap();
        assert_eq!(read.tags, previous);
        assert!(!read.has_cover);
        assert!(read.cover.is_none());
    }

    #[test]
    fn flac_pictures_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("cover.png");
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "quiet", "-y", "-f", "lavfi", "-i", "color=c=red:s=32x32", "-frames:v", "1"])
            .arg(&png)
            .status()
            .is_ok_and(|status| status.success());
        let Some(flac) = flac_fixture().filter(|_| made) else {
            return;
        };
        let path = dir.path().join("with-cover.flac");
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "quiet", "-y", "-i"])
            .arg(&flac)
            .arg("-i")
            .arg(&png)
            .args(["-map", "0", "-map", "1", "-c:a", "copy", "-c:v", "png"])
            .args(["-disposition:v", "attached_pic"])
            .arg(&path)
            .status()
            .is_ok_and(|status| status.success());
        assert!(made);
        let cover = read(&path).unwrap().cover.expect("the FLAC picture");
        assert_eq!(cover, std::fs::read(&png).unwrap());
    }

    #[test]
    fn fields_are_written_alone_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.wav", "b.mp3", "c.flac"] {
            let path = dir.path().join(name);
            match name {
                "a.wav" => write_wav(&path),
                "b.mp3" => {
                    std::fs::copy(
                        concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/../analysis/tests/fixtures/silence-1s-sine-3s.mp3"
                        ),
                        &path,
                    )
                    .unwrap();
                }
                _ => {
                    let Some(flac) = flac_fixture() else { continue };
                    std::fs::copy(flac, &path).unwrap();
                }
            }
            write(&path, &full_tags(), Cover::Keep).unwrap();
            let values: FieldValues = [
                (TagField::Genre, Some("Jazz".to_owned())),
                (TagField::Year, Some("1960".to_owned())),
                (TagField::Isrc, Some("USRC17607839".to_owned())),
                (TagField::RecordLabel, Some("Columbia".to_owned())),
                (TagField::Bpm, Some("136".to_owned())),
                (TagField::Mood, Some("mellow".to_owned())),
                (TagField::Track, Some("2".to_owned())),
                (TagField::Album, None),
            ]
            .into_iter()
            .collect();
            write_fields(&path, &values).unwrap();
            let read_back = read_fields(&path, values.keys().copied()).unwrap();
            assert_eq!(read_back, values, "{name}");
            let all = read(&path).unwrap();
            assert_eq!(all.genre.as_deref(), Some("Jazz"), "{name}");
            // Fields left alone keep their value.
            assert_eq!(all.tags.title, full_tags().title, "{name}");
            assert_eq!(all.tags.track_total, Some(5), "{name}");
            assert_eq!(all.tags.musicbrainz, full_tags().musicbrainz, "{name}");
            assert_eq!(all.tags.album, None, "{name}");
        }
    }

    /// A FLAC file, when ffmpeg is installed.
    fn flac_fixture() -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join(format!("heron-library-flac-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("fixture.flac");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v", "quiet", "-y", "-f", "lavfi", "-i", "sine=d=1", "-c:a", "flac",
            ])
            .arg(&path)
            .status()
            .is_ok_and(|status| status.success());
        made.then_some(path)
    }

    #[test]
    fn audio_properties_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("a.wav");
        write_wav(&wav);
        let properties = read(&wav).unwrap().properties;
        assert_eq!(properties.codec.as_deref(), Some("PCM"));
        assert_eq!(properties.sample_rate, Some(8_000));
        assert_eq!(properties.channels, Some(1));
        assert_eq!(properties.bitrate_kbps, Some(128));
        assert_eq!(
            properties.bitrate_mode,
            Some(crate::properties::BitrateMode::Lossless)
        );

        let mp3 = read(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../analysis/tests/fixtures/silence-1s-sine-3s.mp3"
        )))
        .unwrap()
        .properties;
        assert_eq!(mp3.codec.as_deref(), Some("MP3"));
        assert!(mp3.bitrate_mode.is_some(), "{mp3:?}");
        assert!(mp3.bitrate_kbps.is_some(), "{mp3:?}");
    }

    #[test]
    fn years_come_from_dates() {
        let tags = |date: &str| Tags {
            date: Some(date.into()),
            ..Tags::default()
        };
        assert_eq!(tags("1997-05-21").year(), Some(1997));
        assert_eq!(tags("1997").year(), Some(1997));
        assert_eq!(tags("May 97").year(), None);
    }
}
