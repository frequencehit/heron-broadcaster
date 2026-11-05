// SPDX-License-Identifier: AGPL-3.0-or-later

//! Station settings shared between services. Stored as JSON in the
//! `settings` table, one row per group.

use serde::{Deserialize, Serialize};

/// Parameters of the import-time analysis (Gap Killer and crossfade point
/// detection). Stored under the `analysis` settings name.
///
/// Levels are in dB **relative to the track's integrated loudness**, so the
/// same thresholds work for quiet and loud masters. See `docs/analysis.md`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct AnalysisSettings {
    /// Audio quieter than this at the start of a track is trimmed.
    pub start_threshold_db: f64,
    /// Audio quieter than this at the end of a track is trimmed.
    pub end_threshold_db: f64,
    /// Absolute ceiling (dBFS) for what counts as silence: quiet but audible
    /// passages above it are never trimmed, whatever the track's loudness.
    pub silence_ceiling_dbfs: f64,
    /// Bursts of sound this short, surrounded by silence, are ignored.
    pub blip_max_ms: u32,
    /// Silence longer than this inside a track ends the track there.
    pub max_internal_silence_ms: u32,
    /// The crossfade starts after the last moment the track is louder than this.
    pub fade_trigger_db: f64,
    /// Shortest distance between the crossfade point and the end of the track.
    pub fade_min_ms: u32,
    /// Longest distance between the crossfade point and the end of the track.
    pub fade_max_ms: u32,
}

impl AnalysisSettings {
    /// Name of the row in the `settings` table.
    pub const SETTINGS_NAME: &str = "analysis";

    /// Fingerprint of these settings. Stored with each analysis, so changing a
    /// setting marks every track for re-analysis.
    pub fn fingerprint(&self) -> [u8; 32] {
        // Struct fields serialize in a fixed order, so the JSON is canonical.
        let json = serde_json::to_vec(self).unwrap_or_default();
        *blake3::hash(&json).as_bytes()
    }
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            start_threshold_db: -20.0,
            end_threshold_db: -24.0,
            silence_ceiling_dbfs: -50.0,
            blip_max_ms: 10,
            max_internal_silence_ms: 15_000,
            fade_trigger_db: -15.0,
            fade_min_ms: 150,
            fade_max_ms: 2_500,
        }
    }
}

/// Library management: identification, folder layout, uploads and the
/// trash. Stored under the `library` settings name. See `docs/library.md`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LibrarySettings {
    /// The station's AcoustID application key. Identification is off
    /// without one: incoming files are filed using their own tags.
    pub acoustid_key: Option<String>,
    /// Where files go, relative to the library root. See `docs/library.md`.
    pub layout: String,
    /// Lowest AcoustID score (0 to 1) accepted as a match.
    pub min_score: f64,
    /// Save the release's front cover as `cover.jpg` in the album folder.
    pub save_cover: bool,
    /// Also embed the front cover in the files.
    pub embed_cover: bool,
    /// Largest file accepted by upload, in MiB.
    pub max_upload_mb: u64,
    /// Uploads are refused when they would leave less free space than
    /// this on the buffer zone's disk, in MiB.
    pub min_free_mb: u64,
    /// Days deleted tracks stay in the trash before it is purged.
    pub trash_days: u32,
    /// Tags edited from the browser are written into the files. Off, they
    /// are kept in Heron's database (`track_tag_overrides`) and the files
    /// are left as they are.
    pub write_tags: bool,
}

impl LibrarySettings {
    /// Name of the row in the `settings` table.
    pub const SETTINGS_NAME: &str = "library";
    pub const DEFAULT_LAYOUT: &str = "{album_artist}/{album}/{disc-}{track:02} {title}.{ext}";
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            acoustid_key: None,
            layout: Self::DEFAULT_LAYOUT.to_owned(),
            min_score: 0.9,
            save_cover: true,
            embed_cover: false,
            max_upload_mb: 2048,
            min_free_mb: 5120,
            trash_days: 30,
            write_tags: true,
        }
    }
}

// Keeps the AcoustID key out of logs.
impl std::fmt::Debug for LibrarySettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibrarySettings")
            .field("acoustid_key", &self.acoustid_key.as_ref().map(|_| "<set>"))
            .field("layout", &self.layout)
            .field("min_score", &self.min_score)
            .field("save_cover", &self.save_cover)
            .field("embed_cover", &self.embed_cover)
            .field("max_upload_mb", &self.max_upload_mb)
            .field("min_free_mb", &self.min_free_mb)
            .field("trash_days", &self.trash_days)
            .field("write_tags", &self.write_tags)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let settings: AnalysisSettings = serde_json::from_str(r#"{"blip_max_ms": 20}"#).unwrap();
        assert_eq!(settings.blip_max_ms, 20);
        assert_eq!(settings.end_threshold_db, -24.0);
    }

    #[test]
    fn fingerprint_changes_with_settings() {
        let default = AnalysisSettings::default();
        let changed = AnalysisSettings {
            fade_trigger_db: -12.0,
            ..AnalysisSettings::default()
        };
        assert_eq!(
            default.fingerprint(),
            AnalysisSettings::default().fingerprint()
        );
        assert_ne!(default.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn the_acoustid_key_is_not_logged() {
        let settings: LibrarySettings =
            serde_json::from_str(r#"{"acoustid_key": "s3cret"}"#).unwrap();
        assert_eq!(settings.acoustid_key.as_deref(), Some("s3cret"));
        assert_eq!(settings.layout, LibrarySettings::DEFAULT_LAYOUT);
        assert!(!format!("{settings:?}").contains("s3cret"));
    }

    #[test]
    fn tags_are_written_into_the_files_unless_turned_off() {
        // Settings saved before `write_tags` existed keep writing the files.
        let saved: LibrarySettings = serde_json::from_str(r#"{"trash_days": 7}"#).unwrap();
        assert!(saved.write_tags);
        let off: LibrarySettings = serde_json::from_str(r#"{"write_tags": false}"#).unwrap();
        assert!(!off.write_tags);
    }
}
