// SPDX-License-Identifier: AGPL-3.0-or-later

//! Library work that `heron-api` asks of `heron-librarian` through the
//! `library_tasks` table (the API cannot write files), and its results.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A task, as stored in `library_tasks.params`. The tag is also its `kind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskRequest {
    /// Dry run of an organization: lists the moves, changes nothing.
    OrganizePlan,
    /// Carries out the moves of a dry run, as one batch.
    OrganizeApply { plan_task_id: u64 },
    /// Reverts a batch.
    Undo { batch_id: u64 },
    /// Applies a release picked on the review screen to a track: its tags
    /// are written and it is filed where the layout says, as one batch.
    ReviewApply {
        track_id: u64,
        release_id: String,
        recording_id: String,
        disc: Option<u32>,
        track: Option<u32>,
    },
    /// Looks up the MusicBrainz relationships between the station's
    /// artists ("member of", "is person", …) and suggests the links.
    ArtistRelations,
    /// Writes tags into the files of `track_ids`, as one batch: the fields
    /// of `set` take the same value on every track (`null` removes the
    /// field), the fields of `from_details` each track's programming
    /// details (ISRC, record label, BPM, mood). Other fields are kept.
    EditTags {
        track_ids: Vec<u64>,
        #[serde(default)]
        #[cfg_attr(feature = "openapi", schema(value_type = std::collections::HashMap<String, Option<String>>))]
        set: FieldValues,
        #[serde(default)]
        from_details: Vec<TagField>,
    },
    /// Fetches the programming details of identified tracks from
    /// MusicBrainz (album, record label, ISRC, first release year, genres):
    /// every one, or only those never fetched.
    MusicbrainzDetails {
        #[serde(default)]
        all: bool,
    },
    /// Imports files of the buffer zone into the library, as one batch.
    StagingImport { file_ids: Vec<u64> },
    /// Moves tracks to the trash, as one batch (undone to restore them).
    Delete { track_ids: Vec<u64> },
    /// Purges the whole trash now.
    EmptyTrash,
    /// Mixes a medley's parts into its file (`docs/medleys.md`).
    RenderMedley { medley_id: u64 },
    /// Mixes `parts` as a render would, into the medley's preview, outside
    /// the library (`docs/medleys.md`).
    PreviewMedley {
        medley_id: u64,
        parts: Vec<crate::medley::Part>,
    },
    /// Looks the albums of `track_ids` up on the Cover Art Archive (every
    /// album without a cover when empty); albums with a cover are left
    /// out. With `candidates`, the results wait for an operator even when
    /// one is sure.
    FetchCovers {
        #[serde(default)]
        track_ids: Vec<u64>,
        #[serde(default)]
        candidates: bool,
    },
    /// Fetches the cover of a candidate picked by an operator, for its
    /// album, or for `track_id` only.
    ApplyCover {
        candidate_id: u64,
        #[serde(default)]
        track_id: Option<u64>,
    },
}

impl TaskRequest {
    /// The value of `library_tasks.kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::OrganizePlan => "organize_plan",
            Self::OrganizeApply { .. } => "organize_apply",
            Self::Undo { .. } => "undo",
            Self::ReviewApply { .. } => "review_apply",
            Self::ArtistRelations => "artist_relations",
            Self::EditTags { .. } => "edit_tags",
            Self::MusicbrainzDetails { .. } => "musicbrainz_details",
            Self::StagingImport { .. } => "staging_import",
            Self::Delete { .. } => "delete",
            Self::EmptyTrash => "empty_trash",
            Self::RenderMedley { .. } => "render_medley",
            Self::PreviewMedley { .. } => "preview_medley",
            Self::FetchCovers { .. } => "fetch_covers",
            Self::ApplyCover { .. } => "apply_cover",
        }
    }
}

/// A tag Heron edits in the files, besides those identification writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TagField {
    Title,
    Artist,
    Album,
    AlbumArtist,
    /// Written as the recording date (`1997`), replacing a full date.
    Year,
    Genre,
    Track,
    Disc,
    Isrc,
    RecordLabel,
    Bpm,
    Mood,
}

impl TagField {
    pub const ALL: [Self; 12] = [
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::AlbumArtist,
        Self::Year,
        Self::Genre,
        Self::Track,
        Self::Disc,
        Self::Isrc,
        Self::RecordLabel,
        Self::Bpm,
        Self::Mood,
    ];

    /// The programming details that can be written into the files.
    pub fn is_detail(self) -> bool {
        matches!(
            self,
            Self::Isrc | Self::RecordLabel | Self::Bpm | Self::Mood
        )
    }

    /// The name used in the API and in `track_tag_overrides.field`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::AlbumArtist => "album_artist",
            Self::Year => "year",
            Self::Genre => "genre",
            Self::Track => "track",
            Self::Disc => "disc",
            Self::Isrc => "isrc",
            Self::RecordLabel => "record_label",
            Self::Bpm => "bpm",
            Self::Mood => "mood",
        }
    }

    /// The field named `name` (see [`TagField::as_str`]).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.as_str() == name)
    }

    /// Checks a value for this field and puts it in the form written into
    /// the files: trimmed text, `1997`, `3`, `120.5`, `USRC17607839`.
    pub fn normalize(self, value: &str) -> Result<String, String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("empty value: send null to remove the field".to_owned());
        }
        match self {
            Self::Year => match value.parse::<u16>() {
                Ok(year) if (1000..=2999).contains(&year) => Ok(year.to_string()),
                _ => Err(format!("{value:?} is not a year")),
            },
            Self::Track | Self::Disc => match value.parse::<u16>() {
                Ok(number) if number >= 1 => Ok(number.to_string()),
                _ => Err(format!("{value:?} is not a number from 1")),
            },
            Self::Bpm => match value.parse::<f64>() {
                Ok(bpm) if (20.0..=400.0).contains(&bpm) => {
                    let rounded = (bpm * 10.0).round() / 10.0;
                    Ok(if rounded.fract() == 0.0 {
                        format!("{rounded:.0}")
                    } else {
                        format!("{rounded:.1}")
                    })
                }
                _ => Err(format!("{value:?} is not a tempo (20 to 400 BPM)")),
            },
            Self::Isrc => {
                let isrc: String = value
                    .chars()
                    .filter(|c| !matches!(c, '-' | ' '))
                    .collect::<String>()
                    .to_ascii_uppercase();
                let valid = isrc.is_ascii()
                    && isrc.len() == 12
                    && isrc[..2].bytes().all(|b| b.is_ascii_uppercase())
                    && isrc[2..5].bytes().all(|b| b.is_ascii_alphanumeric())
                    && isrc[5..].bytes().all(|b| b.is_ascii_digit());
                if valid {
                    Ok(isrc)
                } else {
                    Err(format!("{value:?} is not an ISRC (CC-XXX-YY-NNNNN)"))
                }
            }
            _ => Ok(value.chars().take(MAX_FIELD_CHARS).collect()),
        }
    }
}

/// Longest text value written into a tag (the width of the `tracks` columns).
pub const MAX_FIELD_CHARS: usize = 512;

/// Values of tag fields; `None` removes the field.
pub type FieldValues = BTreeMap<TagField, Option<String>>;

/// What a Cover Art Archive lookup did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CoversReport {
    /// Albums looked up.
    pub albums: u64,
    /// Albums given a cover.
    pub found: u64,
    /// Albums whose search was unsure: their candidates wait for review.
    pub candidates: u64,
    pub not_found: u64,
    /// Lookups that failed (MusicBrainz or the Cover Art Archive unreachable).
    pub failed: u64,
}

/// What a MusicBrainz details lookup did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DetailsReport {
    /// Releases looked up (one request each).
    pub releases: u64,
    /// Tracks whose details were saved.
    pub tracks: u64,
    /// Lookups that failed (tried again next time).
    pub failed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PlannedMove {
    pub track_id: u64,
    /// Relative to the library root.
    pub from: String,
    pub to: String,
}

/// The dry run of an organization.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OrganizePlan {
    pub moves: Vec<PlannedMove>,
    /// Tracks already where the layout puts them.
    pub unchanged: u64,
    /// Tracks whose file cannot be read (left where they are).
    pub unreadable: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

/// What a batch of file operations did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BatchReport {
    pub batch_id: u64,
    pub done: u64,
    /// Files that needed no change (their tags already had the values).
    #[serde(default)]
    pub unchanged: u64,
    /// Operations not done, with the reason.
    pub skipped: Vec<SkippedFile>,
}

/// Why a file of the buffer zone should be looked at before it is imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum StagedFlag {
    /// The same file, or the same audio, is in the library.
    DuplicateInLibrary,
    /// The same file, or the same audio, is in the buffer zone.
    DuplicateInBuffer,
    /// The library has a track of the same MusicBrainz recording.
    SameRecordingInLibrary,
    /// The file cannot be decoded: broken, or not audio.
    Undecodable,
    /// Too short to be fingerprinted (and identified).
    TooShort,
    /// An audio format Heron cannot play.
    UnsupportedFormat,
}

/// The outcome of identifying a file of the buffer zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum StagedIdentificationStatus {
    /// A confident match on one release.
    Identified,
    /// Not confident: pick a candidate, or keep the file's own tags.
    NeedsReview,
    /// Identification is off (no AcoustID key): the file's own tags.
    Off,
}

/// As stored in `staged_files.identification`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StagedIdentification {
    pub status: StagedIdentificationStatus,
    /// Why it needs review.
    pub reason: Option<String>,
    pub score: Option<f64>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    /// Releases that matched, best first (the review screen's candidates).
    #[serde(default)]
    pub candidates: Vec<serde_json::Value>,
}

/// What a file of the buffer zone is imported as, when not its
/// identification (as stored in `staged_files.choice`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StagedChoice {
    /// One of its candidate releases.
    Release {
        release_id: String,
        recording_id: String,
        disc: Option<u32>,
        track: Option<u32>,
    },
    /// The file's own tags.
    Own,
}

/// Where uploads are written in a group's folder of the buffer zone until
/// they are complete. Hidden, so they never show as files.
pub fn upload_part_name(file_id: u64) -> String {
    format!(".upload-{file_id}.part")
}

/// What a purge of the trash did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PurgeReport {
    /// `None` when there was nothing to purge.
    pub batch_id: Option<u64>,
    /// Tracks removed for good.
    pub tracks: u64,
    /// Files removed from the trash (tracks' files and the files that
    /// went with them).
    pub files: u64,
}

/// Where deleted files wait, at the root of the library. Hidden, so the
/// scanner skips it.
pub const TRASH_DIR: &str = ".heron-trash";

/// Whether a library path is in the trash.
pub fn in_trash(path: &str) -> bool {
    path.strip_prefix(TRASH_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// What a MusicBrainz relationships lookup found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RelationsReport {
    /// Artists with a MusicBrainz ID looked up.
    pub artists: u64,
    /// New suggestions, between two artists of the station.
    pub suggestions: u64,
    /// Lookups that failed (tried again next time).
    pub failed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_tagged_by_kind() {
        let request = TaskRequest::Undo { batch_id: 4 };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "undo", "batch_id": 4}));
        assert_eq!(json["kind"], request.kind());
        let plan: TaskRequest = serde_json::from_str(r#"{"kind": "organize_plan"}"#).unwrap();
        assert_eq!(plan, TaskRequest::OrganizePlan);
        let edit: TaskRequest = serde_json::from_str(
            r#"{"kind": "edit_tags", "track_ids": [1], "set": {"genre": "Jazz", "year": null}}"#,
        )
        .unwrap();
        let TaskRequest::EditTags { set, .. } = edit else {
            panic!("{edit:?}");
        };
        assert_eq!(set[&TagField::Genre].as_deref(), Some("Jazz"));
        assert_eq!(set[&TagField::Year], None);
    }

    #[test]
    fn field_values_are_checked_and_normalized() {
        assert_eq!(TagField::Year.normalize(" 1997 ").unwrap(), "1997");
        assert!(TagField::Year.normalize("97x").is_err());
        assert_eq!(TagField::Track.normalize("03").unwrap(), "3");
        assert!(TagField::Disc.normalize("0").is_err());
        assert_eq!(TagField::Bpm.normalize("120.04").unwrap(), "120");
        assert_eq!(TagField::Bpm.normalize("87.46").unwrap(), "87.5");
        assert!(TagField::Bpm.normalize("2000").is_err());
        assert_eq!(
            TagField::Isrc.normalize("us-rc1-76-07839").unwrap(),
            "USRC17607839"
        );
        assert!(TagField::Isrc.normalize("USRC1760783").is_err());
        assert!(TagField::Title.normalize("  ").is_err());
        assert_eq!(TagField::Genre.normalize(" Jazz ").unwrap(), "Jazz");
    }

    #[test]
    fn field_names_are_the_api_names() {
        for field in TagField::ALL {
            let json = serde_json::to_string(&field).unwrap();
            assert_eq!(json, format!("\"{}\"", field.as_str()));
            assert_eq!(TagField::from_name(field.as_str()), Some(field));
        }
        assert_eq!(TagField::from_name("composer"), None);
    }

    #[test]
    fn trash_paths() {
        assert!(in_trash(".heron-trash/4/a/b.mp3"));
        assert!(!in_trash(".heron-trashy/b.mp3"));
        assert!(!in_trash("a/.heron-trash/b.mp3"));
    }
}
