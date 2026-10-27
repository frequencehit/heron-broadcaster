// SPDX-License-Identifier: AGPL-3.0-or-later

//! The fallback directory: local tracks played when the queue runs dry,
//! because the API is unreachable or rotation has nothing to offer.
//!
//! A thread of its own, at the lowest priority, scans the directory every
//! minute and analyzes new files once, in the background, the way the analyzer analyzes library
//! tracks at import (cue points, crossfade point, loudness). Results are
//! kept in `fallback.json` in the state directory, so a restart does not
//! analyze them again. A file not analyzed yet plays whole, at its own level.
//! Files that cannot be decoded, and silent files, are never picked.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use heron_core::playout::TrackPlayout;
use heron_core::{AnalysisSettings, PlaySource};
use lofty::file::TaggedFileExt;
use lofty::tag::Accessor;
use serde::{Deserialize, Serialize};

use crate::player::TrackRequest;

const CACHE_FILE: &str = "fallback.json";
const RESCAN: Duration = Duration::from_secs(60);
const AUDIO_EXTENSIONS: [&str; 11] = [
    "mp3", "m4a", "mp4", "aac", "flac", "ogg", "oga", "opus", "wav", "aif", "aiff",
];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct FileAnalysis {
    duration_ms: u32,
    cue_in_ms: u32,
    cue_out_ms: u32,
    mix_point_ms: Option<u32>,
    integrated_lufs: Option<f64>,
    true_peak_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FallbackFile {
    size: u64,
    modified_unix_ms: u64,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    /// `None` until analyzed.
    analysis: Option<FileAnalysis>,
    /// Cannot be decoded, or silent: never picked, until the file changes.
    unplayable: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Catalog {
    /// By path relative to the fallback directory, with `/` separators.
    files: BTreeMap<String, FallbackFile>,
}

pub struct Fallback {
    dir: PathBuf,
    catalog: Arc<Mutex<Catalog>>,
    stop: Arc<AtomicBool>,
    /// Paths still to play before any repeats, picked from the end.
    bag: Vec<String>,
    last: Option<String>,
    rng: u64,
}

impl Fallback {
    /// Starts watching `dir`, keeping analyses in `state_dir`.
    pub fn start(dir: PathBuf, state_dir: &Path) -> Self {
        let cache = state_dir.join(CACHE_FILE);
        let catalog = match std::fs::read(&cache) {
            Ok(json) => serde_json::from_slice(&json).unwrap_or_else(|error| {
                tracing::warn!(%error, "ignoring the fallback analyses saved");
                Catalog::default()
            }),
            Err(_) => Catalog::default(),
        };
        let catalog = Arc::new(Mutex::new(catalog));
        let stop = Arc::new(AtomicBool::new(false));
        let scanner = Scanner {
            dir: dir.clone(),
            cache,
            catalog: Arc::clone(&catalog),
            stop: Arc::clone(&stop),
        };
        if let Err(error) = std::thread::Builder::new()
            .name("heron-fallback".to_owned())
            .spawn(move || scanner.run())
        {
            tracing::error!(%error, "cannot start watching the fallback directory");
        }
        let rng = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(1, |since| since.as_nanos() as u64)
            | 1;
        Self {
            dir,
            catalog,
            stop,
            bag: Vec::new(),
            last: None,
            rng,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether there is anything to play.
    pub fn is_available(&self) -> bool {
        self.lock().files.values().any(|file| !file.unplayable)
    }

    /// Files found, playable or not.
    pub fn len(&self) -> usize {
        self.lock().files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Files analyzed or found unplayable.
    pub fn analyzed(&self) -> usize {
        self.lock()
            .files
            .values()
            .filter(|file| file.analysis.is_some() || file.unplayable)
            .count()
    }

    /// The next track to play: every playable file once, in random order,
    /// before any plays again.
    pub fn pick(&mut self) -> Option<TrackRequest> {
        let catalog = Arc::clone(&self.catalog);
        let catalog = catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for _ in 0..2 {
            if self.bag.is_empty() {
                self.refill(&catalog);
            }
            while let Some(path) = self.bag.pop() {
                if let Some(file) = catalog.files.get(&path)
                    && !file.unplayable
                {
                    let track = track_request(&path, file);
                    self.last = Some(path);
                    return Some(track);
                }
            }
        }
        None
    }

    fn refill(&mut self, catalog: &Catalog) {
        self.bag = catalog
            .files
            .iter()
            .filter(|(_, file)| !file.unplayable)
            .map(|(path, _)| path.clone())
            .collect();
        // Fisher-Yates, with xorshift: no need for a random number crate.
        for index in (1..self.bag.len()).rev() {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let other = (self.rng % (index as u64 + 1)) as usize;
            self.bag.swap(index, other);
        }
        // Never the same track twice in a row.
        if self.bag.len() > 1 && self.bag.last() == self.last.as_ref() {
            let last = self.bag.len() - 1;
            self.bag.swap(0, last);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Catalog> {
        self.catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for Fallback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn track_request(path: &str, file: &FallbackFile) -> TrackRequest {
    let playout = match file.analysis {
        Some(analysis) => TrackPlayout {
            duration_ms: analysis.duration_ms,
            cue_in_ms: analysis.cue_in_ms,
            cue_out_ms: analysis.cue_out_ms,
            mix_point_ms: analysis.mix_point_ms,
            integrated_lufs: analysis.integrated_lufs,
            true_peak_db: analysis.true_peak_db,
            crossfade: Default::default(),
        },
        // Whole, whatever its length.
        None => TrackPlayout::unanalyzed(u32::MAX),
    };
    let title = file.title.clone().or_else(|| {
        Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
    });
    TrackRequest {
        track_id: None,
        path: path.to_owned(),
        title,
        artist: file.artist.clone(),
        album: file.album.clone(),
        playout,
        source: PlaySource::Fallback,
        no_metadata: false,
    }
}

struct Scanner {
    dir: PathBuf,
    cache: PathBuf,
    catalog: Arc<Mutex<Catalog>>,
    stop: Arc<AtomicBool>,
}

impl Scanner {
    fn run(self) {
        // Analysis is background work: it must yield the CPU and the disk
        // to the audio, decode and encoder threads.
        if let Err(error) = heron_service::priority::lower_current_thread(19) {
            tracing::warn!(%error, "cannot lower the fallback scanner's priority");
        }
        let mut reported = None;
        while !self.stop.load(Ordering::Relaxed) {
            let found = self.scan();
            if reported != Some(found) {
                if found == 0 {
                    tracing::warn!(dir = %self.dir.display(), "no tracks in the fallback directory");
                } else {
                    tracing::info!(dir = %self.dir.display(), files = found, "fallback directory scanned");
                }
                reported = Some(found);
            }
            self.analyze_pending();
            let mut waited = Duration::ZERO;
            while waited < RESCAN && !self.stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(1));
                waited += Duration::from_secs(1);
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Catalog> {
        self.catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Brings the catalog in line with the directory; returns the number of files.
    fn scan(&self) -> usize {
        let mut found = Vec::new();
        list_audio(&self.dir, &self.dir, &mut found);
        let (removed, new) = {
            let mut catalog = self.lock();
            let before = catalog.files.len();
            catalog
                .files
                .retain(|path, _| found.iter().any(|(found, _, _)| found == path));
            let new: Vec<(String, u64, u64)> = found
                .iter()
                .filter(|(path, size, modified)| {
                    catalog
                        .files
                        .get(path)
                        .is_none_or(|file| file.size != *size || file.modified_unix_ms != *modified)
                })
                .cloned()
                .collect();
            (catalog.files.len() != before, new)
        };
        // Tags are read without holding the catalog: the player picks from
        // it, and a slow file must not hold the air up.
        let added: Vec<(String, FallbackFile)> = new
            .into_iter()
            .map(|(path, size, modified_unix_ms)| {
                let (title, artist, album) = read_tags(&self.dir.join(&path));
                let file = FallbackFile {
                    size,
                    modified_unix_ms,
                    title,
                    artist,
                    album,
                    analysis: None,
                    unplayable: false,
                };
                (path, file)
            })
            .collect();
        let changed = removed || !added.is_empty();
        self.lock().files.extend(added);
        if changed {
            self.save();
        }
        found.len()
    }

    /// Analyzes the files not analyzed yet, one at a time.
    fn analyze_pending(&self) {
        while !self.stop.load(Ordering::Relaxed) {
            let next = self
                .lock()
                .files
                .iter()
                .find(|(_, file)| file.analysis.is_none() && !file.unplayable)
                .map(|(path, file)| (path.clone(), file.size, file.modified_unix_ms));
            let Some((path, size, modified)) = next else {
                return;
            };
            let result =
                heron_analysis::analyze_file(&self.dir.join(&path), &AnalysisSettings::default());
            let mut catalog = self.lock();
            // Changed or removed meanwhile: the next scan sees it again.
            let Some(file) = catalog
                .files
                .get_mut(&path)
                .filter(|file| file.size == size && file.modified_unix_ms == modified)
            else {
                continue;
            };
            match result {
                Ok(analysis) if analysis.is_silent() => {
                    tracing::warn!(%path, "fallback track is silent: never played");
                    file.unplayable = true;
                }
                Ok(analysis) => {
                    file.analysis = Some(FileAnalysis {
                        duration_ms: analysis.duration_ms,
                        cue_in_ms: analysis.cue_in_ms,
                        cue_out_ms: analysis.cue_out_ms,
                        mix_point_ms: analysis.fade_out_start_ms,
                        integrated_lufs: analysis.integrated_lufs,
                        true_peak_db: analysis.true_peak_db,
                    });
                }
                Err(error) => {
                    tracing::warn!(%path, %error, "fallback track cannot be decoded: never played");
                    file.unplayable = true;
                }
            }
            drop(catalog);
            self.save();
        }
    }

    fn save(&self) {
        let json = match serde_json::to_vec(&*self.lock()) {
            Ok(json) => json,
            Err(error) => {
                tracing::warn!(%error, "cannot save the fallback analyses");
                return;
            }
        };
        let Some(dir) = self.cache.parent() else {
            return;
        };
        if let Err(error) = crate::config::write_atomically(dir, CACHE_FILE, &json) {
            tracing::warn!(%error, "cannot save the fallback analyses");
        }
    }
}

/// Audio files under `dir`: path relative to `root`, size and modification time.
fn list_audio(root: &Path, dir: &Path, files: &mut Vec<(String, u64, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            list_audio(root, &path, files);
            continue;
        }
        let audio = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| AUDIO_EXTENSIONS.contains(&extension.to_lowercase().as_str()));
        let Some(relative) = path.strip_prefix(root).ok().and_then(|relative| {
            let parts: Option<Vec<&str>> = relative.iter().map(|part| part.to_str()).collect();
            parts.map(|parts| parts.join("/"))
        }) else {
            continue;
        };
        if !audio {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis() as u64);
        files.push((relative, metadata.len(), modified));
    }
}

fn read_tags(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(tagged) = lofty::read_from_path(path) else {
        return (None, None, None);
    };
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return (None, None, None);
    };
    (
        tag.title().map(|value| value.into_owned()),
        tag.artist().map(|value| value.into_owned()),
        tag.album().map(|value| value.into_owned()),
    )
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn write_tone(path: &Path, seconds: f64, amplitude: f64) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for n in 0..(seconds * 44_100.0) as u64 {
            let value = (n as f64 * 0.06).sin() * amplitude * f64::from(i16::MAX);
            writer.write_sample(value as i16).unwrap();
            writer.write_sample(value as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn wait_analyzed(fallback: &Fallback, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while fallback.analyzed() < count {
            assert!(Instant::now() < deadline, "fallback files never analyzed");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn analyzes_and_picks_every_playable_file_before_repeating() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("jingles")).unwrap();
        write_tone(&dir.path().join("one.wav"), 2.0, 0.3);
        write_tone(&dir.path().join("jingles/two.wav"), 2.0, 0.3);
        write_tone(&dir.path().join("silent.wav"), 2.0, 0.0);
        std::fs::write(dir.path().join("broken.mp3"), b"not audio").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"not a track").unwrap();

        let mut fallback = Fallback::start(dir.path().to_path_buf(), state.path());
        wait_analyzed(&fallback, 4);
        assert_eq!(fallback.len(), 4);
        assert!(fallback.is_available());

        let mut picked: Vec<String> = (0..4)
            .map(|_| {
                let track = fallback.pick().unwrap();
                assert_eq!(track.source, PlaySource::Fallback);
                assert!(track.playout.integrated_lufs.is_some());
                assert!(track.playout.cue_out_ms <= 2_000);
                track.path
            })
            .collect();
        assert_ne!(picked[0], picked[1]);
        assert_ne!(picked[1], picked[2]);
        picked[..2].sort();
        assert_eq!(picked[..2], ["jingles/two.wav", "one.wav"]);

        // Kept across restarts.
        drop(fallback);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // The last analysis may still be on its way to the disk.
            let saved = std::fs::read_to_string(state.path().join(CACHE_FILE)).unwrap();
            if saved.matches("\"cue_in_ms\"").count() == 2
                && saved.matches("\"unplayable\":true").count() == 2
            {
                break;
            }
            assert!(Instant::now() < deadline, "analyses never saved: {saved}");
            std::thread::sleep(Duration::from_millis(10));
        }
        let fallback = Fallback::start(dir.path().to_path_buf(), state.path());
        assert_eq!(fallback.analyzed(), 4);
    }

    #[test]
    fn nothing_to_pick_in_an_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut fallback = Fallback::start(dir.path().join("missing"), dir.path());
        assert!(!fallback.is_available());
        assert_eq!(fallback.pick(), None);
    }
}
