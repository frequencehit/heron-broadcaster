// SPDX-License-Identifier: AGPL-3.0-or-later

//! The engine's own copy of its configuration (playout settings, encoder
//! profiles and outputs, deck EQ, AGC chain). The API pushes it over gRPC; the engine keeps it on
//! disk so it restarts with its outputs even when the API and the database
//! are down.

use std::io::Write;
use std::path::{Path, PathBuf};

use heron_core::{AgcSettings, EqSettings, OutputsConfig, PlayoutSettings};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "config.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    pub playout: PlayoutSettings,
    pub outputs: OutputsConfig,
    /// Ignored by engines from before the deck EQ: their own struct does not
    /// deny unknown fields, so they can still read this file.
    pub eq: EqSettings,
    /// Ignored by engines from before the AGC chain, as `eq`.
    pub agc: AgcSettings,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigFileError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl EngineConfig {
    /// Reads the configuration saved in `dir`; the defaults when there is none.
    pub fn load(dir: &Path) -> Result<Self, ConfigFileError> {
        let path = dir.join(FILE_NAME);
        let json = match std::fs::read(&path) {
            Ok(json) => json,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => return Err(ConfigFileError::Read { path, source }),
        };
        serde_json::from_slice(&json).map_err(|source| ConfigFileError::Parse { path, source })
    }

    /// Saves the configuration in `dir`, atomically: a crash never leaves a
    /// half-written file.
    pub fn save(&self, dir: &Path) -> Result<(), ConfigFileError> {
        let write = || -> std::io::Result<()> {
            let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
            write_atomically(dir, FILE_NAME, &json)
        };
        write().map_err(|source| ConfigFileError::Write {
            path: dir.join(FILE_NAME),
            source,
        })
    }
}

/// Replaces `dir/name` with `contents`: written to a temporary file, synced,
/// then renamed over it, so a crash never leaves a half-written file.
pub fn write_atomically(dir: &Path, name: &str, contents: &[u8]) -> std::io::Result<()> {
    let temporary = dir.join(format!(".{name}.tmp"));
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(&temporary, dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            EngineConfig::load(dir.path()).unwrap(),
            EngineConfig::default()
        );
        let mut config = EngineConfig::default();
        config.playout.crossfade.min_track_ms = 30_000;
        config.eq.music.enabled = true;
        config.eq.music.gains_db[0] = 4.5;
        config.agc.bypass_all = false;
        config.agc.five_band.enabled = true;
        config.agc.five_band.bands[1].ratio = 6.0;
        config.save(dir.path()).unwrap();
        assert_eq!(EngineConfig::load(dir.path()).unwrap(), config);
    }

    #[test]
    fn files_from_before_the_eq_load_without_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE_NAME),
            br#"{"playout": {}, "outputs": {}}"#,
        )
        .unwrap();
        let config = EngineConfig::load(dir.path()).unwrap();
        assert_eq!(config.eq, EqSettings::default());
        assert_eq!(config.agc, AgcSettings::default());
    }

    #[test]
    fn reports_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), b"{not json").unwrap();
        assert!(matches!(
            EngineConfig::load(dir.path()),
            Err(ConfigFileError::Parse { .. })
        ));
    }
}
