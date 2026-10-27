// SPDX-License-Identifier: AGPL-3.0-or-later

//! The player: the control side of the mixer.
//!
//! It owns the queue (in memory: the engine never reads the database),
//! loads the next track onto a free deck as soon as the current one starts,
//! plans the transition between them, and turns the mixer's events into
//! track changes for the outputs and the API. It runs outside the audio
//! thread and may allocate, lock and log.
//!
//! It also keeps the air on: the files of the next tracks are read into
//! memory ahead of time ([`Preloader`]), a track from the fallback directory
//! is cued when the queue runs dry, the silence watchdog skips tracks that
//! stay silent, and the queue is saved ([`SavedQueue`]) so a restart resumes it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use heron_core::playout::{TrackPlayout, Transition, db_to_gain, plan_transition};
use heron_core::{CrossfadeSettings, FadeCurve, PlaySource, PlayoutSettings};
use heron_outputs::NowPlaying;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::decoder::{DecodedSource, SourceFile};
use crate::fallback::Fallback;
use crate::mixer::{
    AudioSource, Command, Deck, EndReason, Event, FrameRamp, FrameTransition, LoadedTrack,
    MixerHandle, MixerStatus, frames_to_ms, ms_to_frames,
};
use crate::preload::Preloader;

/// Queue entries whose file is read into memory ahead of time, by default.
pub const PRELOAD_TRACKS: usize = 3;
/// With nothing waiting, a fallback track is cued when the track on air has
/// less than this left, so it starts with the planned transition.
const FALLBACK_LEAD_MS: u32 = 30_000;
/// With nothing on air, a cued track that has not started after this
/// (its file hangs on an unreachable share) is given up. Long enough for a
/// track resumed far into its file on a Raspberry Pi.
const STALLED_CUE_MS: u32 = 10_000;
/// A track resumed after a restart fades in over this.
const RESUME_FADE_MS: u32 = 500;
/// A track this close to its end when the engine stopped is not resumed.
const RESUME_MIN_LEFT_MS: u64 = 2_000;
/// Nothing to play for this long is dead air: shorter gaps happen while
/// the engine starts (before the API queues tracks or the fallback
/// directory is scanned).
const DEAD_AIR_AFTER_MS: u32 = 5_000;

/// Opens the audio of a file between its cue points (in ms).
pub type SourceLoader = fn(SourceFile, u32, u32) -> std::io::Result<Box<dyn AudioSource>>;

/// Decodes on a thread of its own, ahead of the deck.
fn spawn_decoder(
    file: SourceFile,
    cue_in_ms: u32,
    cue_out_ms: u32,
) -> std::io::Result<Box<dyn AudioSource>> {
    Ok(Box::new(DecodedSource::spawn(file, cue_in_ms, cue_out_ms)?))
}

/// A track to play, as sent by the API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackRequest {
    pub track_id: Option<u64>,
    /// Relative to the library root, with `/` separators.
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playout: TrackPlayout,
    /// Carried back in the events, for the play history.
    pub source: PlaySource,
    /// Sends no metadata (a jingle): the stream keeps the last track's.
    #[serde(default)]
    pub no_metadata: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub entry_id: u64,
    pub track: TrackRequest,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerEvent {
    Started {
        deck: Deck,
        entry: QueueEntry,
        at: SystemTime,
    },
    Ended {
        deck: Deck,
        entry: QueueEntry,
        played_ms: u64,
        reason: EndReason,
        at: SystemTime,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("invalid path {0:?}: it must be relative to the library, without `..`")]
    InvalidPath(String),
    #[error("no queue entry {0}")]
    NotFound(u64),
    #[error("entry {0} is on air: skip it instead")]
    OnAir(u64),
    #[error("the engine is busy, try again")]
    Busy,
    #[error("cannot start decoding: {0}")]
    Spawn(#[from] std::io::Error),
}

/// One deck as seen from outside.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckSnapshot {
    pub deck: Deck,
    pub entry: Option<QueueEntry>,
    pub position_ms: u64,
    pub length_ms: u64,
    pub on_air: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSnapshot {
    pub paused: bool,
    pub decks: Vec<DeckSnapshot>,
    pub cued: Option<QueueEntry>,
    pub queue: Vec<QueueEntry>,
    pub rendered_ms: u64,
    pub underruns: u64,
}

/// The queue as saved on disk, to resume it after a restart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedQueue {
    pub next_entry_id: u64,
    pub paused: bool,
    /// The track on air and where it was.
    pub on_air: Option<SavedTrack>,
    /// The cued track, then the queue.
    pub waiting: Vec<QueueEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTrack {
    pub entry: QueueEntry,
    /// From the cue-in.
    pub position_ms: u64,
    /// A track of the fallback directory (handed over to another engine;
    /// never saved on disk).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fallback: bool,
}

/// Where an engine handing over can stop the track on air
/// ([`Player::handover_point`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoverPoint {
    /// Nothing plays (paused, or nothing on air): hold now.
    Now,
    /// The track on air stops at this position (frames from its cue-in).
    At { position: u64 },
    /// A transition or a jingle is playing, or the track on air ends too
    /// soon: ask again later.
    Wait,
}

/// Where the mixer held after [`Player::halt_at`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Halted {
    pub entry_id: u64,
    /// Frames from the cue-in of the track on air.
    pub position: u64,
    /// Master bus frame of the stop: silence after it.
    pub at_frame: u64,
}

pub struct Player {
    library_dir: PathBuf,
    settings: PlayoutSettings,
    mixer: MixerHandle,
    queue: VecDeque<QueueEntry>,
    /// Entries handed to the mixer (cued, on air, fading out, FX).
    loaded: HashMap<u64, QueueEntry>,
    cued: Option<u64>,
    /// Entries the player uncued whose `Uncued` event has not arrived yet.
    /// They are not cued again before it does: the mixer may have started
    /// them instead.
    uncueing: HashSet<u64>,
    /// The track on air: the last one started on Deck A or B.
    on_air: Option<u64>,
    paused: bool,
    next_entry_id: u64,
    load_source: SourceLoader,
    events: broadcast::Sender<PlayerEvent>,
    now_playing: watch::Sender<NowPlaying>,
    preloader: Preloader,
    fallback: Option<Fallback>,
    /// Loaded entries from the fallback directory. They are never in the
    /// queue: anything queued replaces a cued one.
    fallback_entries: HashSet<u64>,
    /// Nothing on air, nothing to play: the stream carries silence.
    dead_air: bool,
    /// Master frame since which there has been nothing to play.
    idle_since: Option<u64>,
    /// The silence watchdog counts silence from this master frame on (the
    /// last track start, skip or resume).
    watch_from: u64,
    /// Tracks the watchdog skipped since sound was last heard.
    silence_skips: u32,
    /// Master frame since which the cued track has been waiting to start
    /// with nothing on air.
    stalled_since: Option<u64>,
    /// Something to save changed since the last [`Self::take_changed`].
    changed: bool,
    /// Held by [`Self::hold`]: waiting to take over from another engine.
    held: bool,
    halted: Option<Halted>,
}

impl Player {
    pub fn new(
        library_dir: PathBuf,
        settings: PlayoutSettings,
        mixer: MixerHandle,
        events: broadcast::Sender<PlayerEvent>,
        now_playing: watch::Sender<NowPlaying>,
    ) -> Self {
        Self {
            library_dir,
            settings,
            mixer,
            queue: VecDeque::new(),
            loaded: HashMap::new(),
            cued: None,
            uncueing: HashSet::new(),
            on_air: None,
            paused: false,
            next_entry_id: 1,
            load_source: spawn_decoder,
            events,
            now_playing,
            preloader: Preloader::new(PRELOAD_TRACKS),
            fallback: None,
            fallback_entries: HashSet::new(),
            dead_air: false,
            idle_since: None,
            watch_from: 0,
            silence_skips: 0,
            stalled_since: None,
            changed: false,
            held: false,
            halted: None,
        }
    }

    /// Replaces how tracks are opened (decode threads by default).
    pub fn with_source_loader(mut self, load_source: SourceLoader) -> Self {
        self.load_source = load_source;
        self
    }

    /// Reads the files of the next `tracks` queue entries into memory ahead of time.
    pub fn with_preload(mut self, tracks: usize) -> Self {
        self.preloader = Preloader::new(tracks);
        self
    }

    /// Plays tracks from the fallback directory when the queue runs dry.
    pub fn with_fallback(mut self, fallback: Fallback) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// Nothing is on air and there is nothing to play.
    pub fn dead_air(&self) -> bool {
        self.dead_air
    }

    /// Whether the queue changed since the last call: it needs saving.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    pub fn status(&self) -> &MixerStatus {
        &self.mixer.status
    }

    /// Adds tracks at `position` among the waiting tracks (0 is next, before
    /// the cued one), or at the end.
    pub fn enqueue(
        &mut self,
        tracks: Vec<TrackRequest>,
        position: Option<usize>,
    ) -> Result<Vec<u64>, PlayerError> {
        for track in &tracks {
            self.resolve(&track.path)?;
        }
        let entries: Vec<QueueEntry> = tracks
            .into_iter()
            .map(|track| {
                let entry_id = self.next_entry_id;
                self.next_entry_id += 1;
                QueueEntry { entry_id, track }
            })
            .collect();
        let ids = entries.iter().map(|entry| entry.entry_id).collect();
        let position = position.unwrap_or(usize::MAX);
        let fallback_cued = self
            .cued
            .is_some_and(|id| self.fallback_entries.contains(&id));
        if position == 0 || fallback_cued {
            // The new tracks go before the one already cued; a fallback
            // track is dropped.
            self.uncue()?;
        }
        let at = self.queue_index(position);
        for (offset, entry) in entries.into_iter().enumerate() {
            self.queue.insert(at + offset, entry);
        }
        self.changed = true;
        self.ensure_cued();
        Ok(ids)
    }

    pub fn remove(&mut self, entry_id: u64) -> Result<(), PlayerError> {
        if self.on_air == Some(entry_id) {
            return Err(PlayerError::OnAir(entry_id));
        }
        if self.cued == Some(entry_id) {
            self.uncue()?;
            if self.fallback_entries.contains(&entry_id) {
                return Ok(());
            }
        }
        let before = self.queue.len();
        self.queue.retain(|entry| entry.entry_id != entry_id);
        if self.queue.len() == before {
            return Err(PlayerError::NotFound(entry_id));
        }
        self.changed = true;
        self.ensure_cued();
        Ok(())
    }

    /// Moves a waiting entry (cued or queued) to `position` (0 is next).
    pub fn move_entry(&mut self, entry_id: u64, position: usize) -> Result<(), PlayerError> {
        if self.on_air == Some(entry_id) || self.is_playing(entry_id) {
            return Err(PlayerError::OnAir(entry_id));
        }
        if self.cued == Some(entry_id) && position == 0 {
            return Ok(());
        }
        if self.cued.is_some() && (self.cued == Some(entry_id) || position == 0) {
            // The cued track goes back to the front of the queue.
            self.uncue()?;
        }
        let Some(from) = self.queue.iter().position(|e| e.entry_id == entry_id) else {
            self.ensure_cued();
            return Err(PlayerError::NotFound(entry_id));
        };
        if let Some(entry) = self.queue.remove(from) {
            let at = self.queue_index(position);
            self.queue.insert(at, entry);
        }
        self.changed = true;
        self.ensure_cued();
        Ok(())
    }

    /// Replaces the track of a waiting entry (cued or queued), keeping its
    /// place and ID. A cued entry is loaded again.
    pub fn replace(&mut self, entry_id: u64, track: TrackRequest) -> Result<(), PlayerError> {
        self.resolve(&track.path)?;
        if self.on_air == Some(entry_id) || self.is_playing(entry_id) {
            return Err(PlayerError::OnAir(entry_id));
        }
        if self.cued == Some(entry_id) {
            self.uncue()?;
        }
        let Some(entry) = self.queue.iter_mut().find(|e| e.entry_id == entry_id) else {
            return Err(PlayerError::NotFound(entry_id));
        };
        entry.track = track;
        self.changed = true;
        self.ensure_cued();
        Ok(())
    }

    /// Where `position` in the waiting tracks (the cued one, then the queue)
    /// falls in the queue.
    fn queue_index(&self, position: usize) -> usize {
        let cued = usize::from(self.cued.is_some());
        position.saturating_sub(cued).min(self.queue.len())
    }

    /// Whether the entry is on a deck (on air, fading out or on the FX player).
    fn is_playing(&self, entry_id: u64) -> bool {
        self.cued != Some(entry_id)
            && self.loaded.contains_key(&entry_id)
            && !self.queue.iter().any(|e| e.entry_id == entry_id)
    }

    /// Empties the queue, including the cued track. The track on air plays on.
    pub fn clear(&mut self) -> Result<(), PlayerError> {
        self.uncue()?;
        self.queue.clear();
        self.changed = true;
        Ok(())
    }

    /// Fades out the track on air and starts the next one.
    pub fn skip(&mut self) -> Result<(), PlayerError> {
        let crossfade = &self.settings.crossfade;
        self.send(Command::Skip {
            fade_frames: ms_to_frames(crossfade.skip_fade_ms),
            curve: crossfade.fade_out.curve,
        })
    }

    pub fn pause(&mut self) -> Result<(), PlayerError> {
        self.send(Command::Pause)?;
        self.paused = true;
        self.changed = true;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), PlayerError> {
        self.send(Command::Resume)?;
        self.paused = false;
        self.changed = true;
        self.watch_from = self.mixer.status.frame.load(Ordering::Relaxed);
        Ok(())
    }

    pub fn play_fx(&mut self, track: TrackRequest) -> Result<u64, PlayerError> {
        let entry = QueueEntry {
            entry_id: self.next_entry_id,
            track,
        };
        self.next_entry_id += 1;
        let loaded = self.load(&entry, None, 0)?;
        self.send(Command::PlayFx(loaded))?;
        let entry_id = entry.entry_id;
        self.loaded.insert(entry_id, entry);
        Ok(entry_id)
    }

    pub fn stop_fx(&mut self) -> Result<(), PlayerError> {
        self.send(Command::StopFx)
    }

    /// Holds the mixer silent at once, until [`Self::unhold`]: an engine
    /// waiting to take over. Tracks are cued but do not start.
    pub fn hold(&mut self) -> Result<(), PlayerError> {
        self.send(Command::Hold)?;
        self.held = true;
        Ok(())
    }

    /// Plays on from where the mixer was held (unless paused), fading in.
    pub fn unhold(&mut self) -> Result<(), PlayerError> {
        if !self.paused {
            self.send(Command::Resume)?;
        }
        self.held = false;
        self.watch_from = self.mixer.status.frame.load(Ordering::Relaxed);
        self.stalled_since = None;
        self.idle_since = None;
        Ok(())
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Whether the cued track (if any) has enough audio decoded to start.
    pub fn cued_ready(&self) -> bool {
        let status = &self.mixer.status;
        self.cued.is_none()
            || (status.cued.load(Ordering::Relaxed) == self.cued.unwrap_or(0)
                && status.cued_ready.load(Ordering::Relaxed))
    }

    /// Where the track on air can stop for a handover, at least `lead_ms`
    /// from now: never in a transition or under a jingle, and at least 1 s
    /// before the next transition. On a millisecond, as saved queues are.
    pub fn handover_point(&self, lead_ms: u32) -> HandoverPoint {
        if self.paused || self.held {
            return HandoverPoint::Now;
        }
        let status = &self.mixer.status;
        if status.decks[Deck::Fx.slot()]
            .entry_id
            .load(Ordering::Relaxed)
            != 0
        {
            return HandoverPoint::Wait;
        }
        let on_air = status.on_air.load(Ordering::Relaxed);
        let busy: Vec<&crate::mixer::DeckStatus> = status.decks[..2]
            .iter()
            .filter(|deck| deck.entry_id.load(Ordering::Relaxed) != 0)
            .collect();
        let Some(deck) = busy
            .iter()
            .find(|deck| on_air != 0 && deck.entry_id.load(Ordering::Relaxed) == on_air)
        else {
            return if busy.is_empty() {
                HandoverPoint::Now
            } else {
                HandoverPoint::Wait
            };
        };
        if busy.len() > 1 {
            // The previous track is still fading out.
            return HandoverPoint::Wait;
        }
        let position = deck.position.load(Ordering::Relaxed);
        let length = deck.length.load(Ordering::Relaxed);
        let next_start = self
            .cued
            .and_then(|cued| self.loaded.get(&cued))
            .map_or_else(
                // Nothing cued yet: one may be, with a transition before the end.
                || length.saturating_sub(ms_to_frames(FALLBACK_LEAD_MS / 2)),
                |entry| {
                    let transition = self.transition_into(&entry.track.playout);
                    self.frame_transition(transition.as_ref())
                        .next_start
                        .min(length)
                },
            );
        let stop = ms_to_frames(
            u32::try_from(frames_to_ms(position))
                .unwrap_or(u32::MAX)
                .saturating_add(lead_ms),
        );
        if stop.saturating_add(ms_to_frames(1_000)) > next_start {
            return HandoverPoint::Wait;
        }
        HandoverPoint::At { position: stop }
    }

    /// Stops the track on air at `position` (frames from its cue-in) and
    /// holds the mixer: reported by [`Self::take_halted`].
    pub fn halt_at(&mut self, position: u64) -> Result<(), PlayerError> {
        self.halted = None;
        self.send(Command::HaltAt { position })?;
        self.held = true;
        Ok(())
    }

    pub fn take_halted(&mut self) -> Option<Halted> {
        self.halted.take()
    }

    /// Forgets the queue and the cued track (not what is on air), for a
    /// handed-over queue to replace them.
    pub fn reset(&mut self) -> Result<(), PlayerError> {
        self.uncue_with(false)?;
        self.queue.clear();
        self.halted = None;
        Ok(())
    }

    pub fn settings(&self) -> &PlayoutSettings {
        &self.settings
    }

    /// New settings apply to the next transitions planned.
    pub fn set_settings(&mut self, settings: PlayoutSettings) {
        self.settings = settings;
    }

    /// Handles what the mixer reported and keeps the next track cued. Call
    /// it every few milliseconds.
    pub fn tick(&mut self) {
        while let Ok(track) = self.mixer.trash.pop() {
            // Stops its decode thread.
            drop(track);
        }
        while let Ok(event) = self.mixer.events.pop() {
            self.handle(event);
        }
        self.watch_silence();
        self.watch_stalled_cue();
        self.ensure_cued();
        self.preload();
    }

    /// Reads the files of the next queued tracks into memory.
    fn preload(&mut self) {
        let upcoming: Vec<(u64, PathBuf)> = self
            .queue
            .iter()
            .filter(|entry| !self.uncueing.contains(&entry.entry_id))
            .filter_map(|entry| Some((entry.entry_id, self.resolve(&entry.track.path).ok()?)))
            .collect();
        self.preloader.keep(upcoming);
    }

    /// Skips a track that stays silent, then goes to the fallback directory
    /// if the next one is silent too.
    fn watch_silence(&mut self) {
        if self.held {
            return;
        }
        let watchdog = self.settings.silence_watchdog;
        let status = &self.mixer.status;
        let frame = status.frame.load(Ordering::Relaxed);
        let silent = status
            .silent_frames
            .load(Ordering::Relaxed)
            .min(frame.saturating_sub(self.watch_from));
        if silent < frame.saturating_sub(self.watch_from) {
            // Sound was heard since the last skip.
            self.silence_skips = 0;
        }
        if !watchdog.enabled || self.paused || silent < ms_to_frames(watchdog.timeout_ms) {
            return;
        }
        let Some(on_air) = self.on_air else {
            return;
        };
        let path = self
            .loaded
            .get(&on_air)
            .map(|entry| entry.track.path.clone());
        let to_fallback = self.silence_skips > 0 && self.cue_fallback_now();
        tracing::warn!(
            entry_id = on_air,
            ?path,
            seconds = frames_to_ms(silent) / 1_000,
            to_fallback,
            "silence on air: skipping the track"
        );
        metrics::counter!("heron_engine_silence_skips_total").increment(1);
        if self.skip().is_ok() {
            self.silence_skips += 1;
            self.watch_from = frame;
        }
    }

    /// Gives up a cued track that cannot start while nothing plays: its
    /// decoder is stuck (a file on an unreachable share). It is dropped, not
    /// put back in the queue, and a fallback track plays instead.
    fn watch_stalled_cue(&mut self) {
        let frame = self.mixer.status.frame.load(Ordering::Relaxed);
        let Some(cued) = self
            .cued
            .filter(|_| self.on_air.is_none() && !self.paused && !self.held)
        else {
            self.stalled_since = None;
            return;
        };
        let since = *self.stalled_since.get_or_insert(frame);
        if frame.saturating_sub(since) < ms_to_frames(STALLED_CUE_MS) {
            return;
        }
        self.stalled_since = None;
        let path = self.loaded.get(&cued).map(|entry| entry.track.path.clone());
        if self.uncue_with(false).is_err() {
            return;
        }
        tracing::warn!(
            entry_id = cued,
            ?path,
            "the cued track could not start (is its file reachable?): skipped"
        );
        metrics::counter!("heron_engine_stalled_tracks_total").increment(1);
        self.changed = true;
        self.cue_fallback();
    }

    /// Cues a fallback track in place of the cued one (put back in the queue).
    fn cue_fallback_now(&mut self) -> bool {
        if self
            .fallback
            .as_ref()
            .is_none_or(|fallback| !fallback.is_available())
            || self.uncue().is_err()
        {
            return false;
        }
        self.cue_fallback()
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Started { deck, entry_id, .. } => {
                // The mixer may start a track the player just tried to uncue.
                self.uncueing.remove(&entry_id);
                self.queue.retain(|entry| entry.entry_id != entry_id);
                if self.cued == Some(entry_id) {
                    self.cued = None;
                }
                let Some(entry) = self.loaded.get(&entry_id).cloned() else {
                    return;
                };
                self.changed = true;
                if deck != Deck::Fx {
                    self.on_air = Some(entry_id);
                    self.watch_from = self.mixer.status.frame.load(Ordering::Relaxed);
                    if self.fallback_entries.contains(&entry_id) {
                        metrics::counter!("heron_engine_fallback_tracks_total").increment(1);
                    }
                    // A jingle leaves the stream on the track before.
                    if !entry.track.no_metadata {
                        self.now_playing.send_replace(NowPlaying {
                            artist: entry.track.artist.clone(),
                            title: entry.track.title.clone(),
                        });
                    }
                    self.retarget_cued();
                }
                tracing::info!(
                    ?deck,
                    entry_id,
                    artist = entry.track.artist.as_deref().unwrap_or_default(),
                    title = entry.track.title.as_deref().unwrap_or_default(),
                    "track started"
                );
                let _ = self.events.send(PlayerEvent::Started {
                    deck,
                    entry,
                    at: SystemTime::now(),
                });
            }
            Event::Ended {
                deck,
                entry_id,
                played_frames,
                reason,
                ..
            } => {
                if self.on_air == Some(entry_id) {
                    self.on_air = None;
                }
                self.changed = true;
                self.fallback_entries.remove(&entry_id);
                if let Some(entry) = self.loaded.remove(&entry_id) {
                    let _ = self.events.send(PlayerEvent::Ended {
                        deck,
                        entry,
                        played_ms: frames_to_ms(played_frames),
                        reason,
                        at: SystemTime::now(),
                    });
                }
            }
            Event::Uncued { entry_id } => {
                if self.uncueing.remove(&entry_id) {
                    // Uncued by the player: back in the queue.
                    if self.cued != Some(entry_id) {
                        self.loaded.remove(&entry_id);
                        self.fallback_entries.remove(&entry_id);
                    }
                    return;
                }
                // Dropped by the mixer, its audio having ended before it started.
                if self.cued == Some(entry_id) {
                    let path = self.loaded.get(&entry_id).map(|e| e.track.path.clone());
                    tracing::warn!(entry_id, ?path, "skipping a track that cannot be decoded");
                    self.cued = None;
                    self.changed = true;
                }
                self.loaded.remove(&entry_id);
                self.fallback_entries.remove(&entry_id);
            }
            Event::Halted {
                entry_id,
                position,
                at_frame,
            } => {
                self.halted = Some(Halted {
                    entry_id,
                    position,
                    at_frame,
                });
            }
            Event::Underrun { deck, entry_id } => {
                metrics::counter!("heron_engine_underrun_tracks_total").increment(1);
                tracing::warn!(?deck, entry_id, "decoding fell behind: silence played");
            }
        }
    }

    /// Loads the next queue entry onto the mixer when nothing is cued, or
    /// a fallback track when the queue is empty.
    fn ensure_cued(&mut self) {
        while self.cued.is_none() {
            if self
                .queue
                .front()
                .is_some_and(|entry| self.uncueing.contains(&entry.entry_id))
            {
                return;
            }
            let Some(entry) = self.queue.pop_front() else {
                if self.uncueing.is_empty() && !self.paused && !self.held && self.fallback_due() {
                    self.cue_fallback();
                }
                self.update_dead_air();
                return;
            };
            let transition = self.transition_into(&entry.track.playout);
            match self.load(&entry, transition.as_ref(), 0) {
                Ok(track) => {
                    let frame_transition = self.frame_transition(transition.as_ref());
                    match self.mixer.commands.push(Command::Cue {
                        track,
                        transition: frame_transition,
                    }) {
                        Ok(()) => {
                            self.cued = Some(entry.entry_id);
                            self.loaded.insert(entry.entry_id, entry);
                        }
                        Err(_) => {
                            // Try again on the next tick.
                            self.queue.push_front(entry);
                            return;
                        }
                    }
                }
                Err(error) => {
                    tracing::error!(entry_id = entry.entry_id, path = %entry.track.path, %error, "cannot load track");
                    self.changed = true;
                }
            }
        }
        self.update_dead_air();
    }

    /// Whether the queue running dry calls for a fallback track now: nothing
    /// on air, or the track on air ends soon.
    fn fallback_due(&self) -> bool {
        let Some(on_air) = self.on_air else {
            return true;
        };
        let status = &self.mixer.status;
        status.decks[..2]
            .iter()
            .find(|deck| deck.entry_id.load(Ordering::Relaxed) == on_air)
            .is_none_or(|deck| {
                let left = deck
                    .length
                    .load(Ordering::Relaxed)
                    .saturating_sub(deck.position.load(Ordering::Relaxed));
                left < ms_to_frames(FALLBACK_LEAD_MS)
            })
    }

    /// Cues the next track of the fallback directory. Returns whether one was cued.
    fn cue_fallback(&mut self) -> bool {
        let Some(track) = self.fallback.as_mut().and_then(Fallback::pick) else {
            return false;
        };
        let entry = QueueEntry {
            entry_id: self.next_entry_id,
            track,
        };
        self.next_entry_id += 1;
        self.fallback_entries.insert(entry.entry_id);
        let transition = self.transition_into(&entry.track.playout);
        let track = match self.load(&entry, transition.as_ref(), 0) {
            Ok(track) => track,
            Err(error) => {
                tracing::error!(path = %entry.track.path, %error, "cannot load a fallback track");
                self.fallback_entries.remove(&entry.entry_id);
                return false;
            }
        };
        let transition = self.frame_transition(transition.as_ref());
        if self
            .mixer
            .commands
            .push(Command::Cue { track, transition })
            .is_err()
        {
            self.fallback_entries.remove(&entry.entry_id);
            return false;
        }
        tracing::info!(
            entry_id = entry.entry_id,
            path = %entry.track.path,
            "nothing queued: playing from the fallback directory"
        );
        self.cued = Some(entry.entry_id);
        self.loaded.insert(entry.entry_id, entry);
        true
    }

    fn update_dead_air(&mut self) {
        let frame = self.mixer.status.frame.load(Ordering::Relaxed);
        let idle = self.on_air.is_none() && self.cued.is_none() && !self.paused && !self.held;
        self.idle_since = idle.then(|| self.idle_since.unwrap_or(frame));
        let dead_air = self
            .idle_since
            .is_some_and(|since| frame.saturating_sub(since) >= ms_to_frames(DEAD_AIR_AFTER_MS));
        if dead_air != self.dead_air {
            self.dead_air = dead_air;
            if dead_air {
                tracing::error!(
                    "nothing to play: streaming silence (the queue is empty and \
                     there is no track in the fallback directory)"
                );
            } else {
                tracing::info!("back on air");
            }
            metrics::gauge!("heron_engine_dead_air").set(f64::from(u8::from(dead_air)));
        }
    }

    /// Removes the cued track from the mixer and puts it back in front of the queue.
    fn uncue(&mut self) -> Result<(), PlayerError> {
        self.uncue_with(true)
    }

    /// Removes the cued track from the mixer; puts it back in front of the
    /// queue if `requeue` (never a fallback track).
    fn uncue_with(&mut self, requeue: bool) -> Result<(), PlayerError> {
        let Some(cued) = self.cued.take() else {
            return Ok(());
        };
        if let Err(error) = self.send(Command::Uncue) {
            self.cued = Some(cued);
            return Err(error);
        }
        self.uncueing.insert(cued);
        if requeue
            && !self.fallback_entries.contains(&cued)
            && let Some(entry) = self.loaded.get(&cued).cloned()
        {
            self.queue.push_front(entry);
        }
        Ok(())
    }

    /// Replans the cued track's transition against the track now on air.
    fn retarget_cued(&mut self) {
        let Some(cued) = self.cued else {
            return;
        };
        let Some(entry) = self.loaded.get(&cued) else {
            return;
        };
        let transition = self.transition_into(&entry.track.playout);
        let fade_in = self.fade_in(entry, transition.as_ref());
        let command = Command::Retarget {
            transition: self.frame_transition(transition.as_ref()),
            fade_in,
        };
        let _ = self.send(command);
    }

    /// The transition from the track on air into `incoming`; `None` when
    /// nothing is on air.
    fn transition_into(&self, incoming: &TrackPlayout) -> Option<Transition> {
        let outgoing = self.loaded.get(&self.on_air?)?;
        Some(plan_transition(
            &outgoing.track.playout,
            incoming,
            &self.crossfade(),
        ))
    }

    fn frame_transition(&self, transition: Option<&Transition>) -> FrameTransition {
        let outgoing = self.on_air.and_then(|entry_id| self.loaded.get(&entry_id));
        match (transition, outgoing) {
            (Some(transition), Some(outgoing)) => {
                FrameTransition::new(transition, &outgoing.track.playout)
            }
            _ => FrameTransition::back_to_back(),
        }
    }

    fn fade_in(&self, entry: &QueueEntry, transition: Option<&Transition>) -> Option<FrameRamp> {
        transition
            .and_then(|transition| transition.fade_in)
            .map(|ramp| FrameRamp::from_ramp(&ramp, entry.track.playout.cue_in_ms))
    }

    fn crossfade(&self) -> CrossfadeSettings {
        self.settings.crossfade
    }

    /// Opens the track of `entry`, from `start_ms` after its cue-in.
    fn load(
        &mut self,
        entry: &QueueEntry,
        transition: Option<&Transition>,
        start_ms: u32,
    ) -> Result<Box<LoadedTrack>, PlayerError> {
        self.load_with(entry, transition, start_ms, RESUME_FADE_MS)
    }

    /// [`Self::load`], fading in over `resume_fade_ms` when starting after
    /// the cue-in (none: 0).
    fn load_with(
        &mut self,
        entry: &QueueEntry,
        transition: Option<&Transition>,
        start_ms: u32,
        resume_fade_ms: u32,
    ) -> Result<Box<LoadedTrack>, PlayerError> {
        let playout = &entry.track.playout;
        let path = if self.fallback_entries.contains(&entry.entry_id) {
            let dir = self.fallback.as_ref().map(|fallback| fallback.dir());
            dir.and_then(|dir| resolve_library_path(dir, &entry.track.path))
                .ok_or_else(|| PlayerError::InvalidPath(entry.track.path.clone()))?
        } else {
            self.resolve(&entry.track.path)?
        };
        let file = self.preloader.take(entry.entry_id, path);
        let cue_in_ms = playout.cue_in_ms.saturating_add(start_ms);
        let source = (self.load_source)(file, cue_in_ms, playout.cue_out_ms)?;
        let start = ms_to_frames(start_ms);
        let fade_in = if start > 0 {
            (resume_fade_ms > 0).then(|| FrameRamp {
                start,
                length: ms_to_frames(resume_fade_ms),
                curve: FadeCurve::Linear,
                level: 0.0,
            })
        } else {
            self.fade_in(entry, transition)
        };
        Ok(Box::new(LoadedTrack {
            entry_id: entry.entry_id,
            source,
            length: ms_to_frames(playout.playable_ms()),
            start,
            gain: db_to_gain(playout.gain_db(&self.settings.loudness)),
            fade_in,
        }))
    }

    /// The queue to save: the track on air with its position, and the
    /// waiting tracks. Tracks from the fallback directory are left out.
    pub fn saved(&self) -> SavedQueue {
        self.saved_queue(false)
    }

    /// The queue to hand over to another engine: as [`Self::saved`], with
    /// the track on air even from the fallback directory, and stopping at
    /// `position_ms` when given.
    pub fn handed_over(&self, position_ms: Option<u64>) -> SavedQueue {
        let mut queue = self.saved_queue(true);
        if let (Some(on_air), Some(position_ms)) = (queue.on_air.as_mut(), position_ms) {
            on_air.position_ms = position_ms;
        }
        queue
    }

    fn saved_queue(&self, fallback_on_air: bool) -> SavedQueue {
        let snapshot = self.snapshot();
        let on_air_deck = snapshot.decks.into_iter().find(|deck| deck.on_air);
        // The audio thread may have started the cued track since the last
        // tick: it is then on air and still cued here. Saved once, on air.
        let on_air_id = on_air_deck
            .as_ref()
            .and_then(|deck| deck.entry.as_ref())
            .map(|entry| entry.entry_id);
        let on_air = on_air_deck.and_then(|deck| {
            let entry = deck.entry?;
            let fallback = self.fallback_entries.contains(&entry.entry_id);
            (!fallback || fallback_on_air).then_some(SavedTrack {
                entry,
                position_ms: deck.position_ms,
                fallback,
            })
        });
        let waiting = snapshot
            .cued
            .into_iter()
            .chain(snapshot.queue)
            .filter(|entry| {
                Some(entry.entry_id) != on_air_id
                    && !self.fallback_entries.contains(&entry.entry_id)
            })
            .collect();
        SavedQueue {
            next_entry_id: self.next_entry_id,
            paused: snapshot.paused,
            on_air,
            waiting,
        }
    }

    /// Resumes a saved queue: the track on air where it was, then the
    /// waiting tracks. Call it on a new player, before anything is queued.
    pub fn restore(&mut self, saved: SavedQueue) {
        self.restore_with(saved, RESUME_FADE_MS);
    }

    /// Goes on with a queue handed over by another engine, after
    /// [`Self::reset`]: the track on air exactly where the other engine
    /// stopped it, without a fade of its own (the mixer fades in from
    /// [`Self::hold`]).
    pub fn take_over(&mut self, queue: SavedQueue) {
        self.restore_with(queue, 0);
    }

    fn restore_with(&mut self, saved: SavedQueue, resume_fade_ms: u32) {
        self.next_entry_id = self.next_entry_id.max(saved.next_entry_id);
        if saved.paused != self.paused {
            let restored = if saved.paused {
                self.pause()
            } else {
                self.resume()
            };
            if restored.is_err() {
                tracing::warn!("cannot restore the pause");
            }
        }
        if let Some(on_air) = saved.on_air {
            let left = u64::from(on_air.entry.track.playout.playable_ms())
                .saturating_sub(on_air.position_ms);
            if on_air.fallback && self.fallback.is_some() {
                self.fallback_entries.insert(on_air.entry.entry_id);
            }
            if left > RESUME_MIN_LEFT_MS && (!on_air.fallback || self.fallback.is_some()) {
                let start_ms = u32::try_from(on_air.position_ms).unwrap_or(u32::MAX);
                let entry = on_air.entry;
                match self.load_with(&entry, None, start_ms, resume_fade_ms) {
                    Ok(track) => {
                        let transition = FrameTransition::back_to_back();
                        if self
                            .mixer
                            .commands
                            .push(Command::Cue { track, transition })
                            .is_ok()
                        {
                            tracing::info!(
                                entry_id = entry.entry_id,
                                path = %entry.track.path,
                                position_ms = on_air.position_ms,
                                "resuming the track on air"
                            );
                            self.cued = Some(entry.entry_id);
                            self.loaded.insert(entry.entry_id, entry);
                        }
                    }
                    Err(error) => {
                        self.fallback_entries.remove(&entry.entry_id);
                        tracing::warn!(path = %entry.track.path, %error, "cannot resume the track on air");
                    }
                }
            }
        }
        let waiting: Vec<QueueEntry> = saved
            .waiting
            .into_iter()
            .filter(|entry| self.resolve(&entry.track.path).is_ok())
            .collect();
        let waiting_count = waiting.len();
        self.queue.extend(waiting);
        tracing::info!(waiting = waiting_count, "queue restored");
        self.ensure_cued();
    }

    fn send(&mut self, command: Command) -> Result<(), PlayerError> {
        self.mixer
            .commands
            .push(command)
            .map_err(|_| PlayerError::Busy)
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, PlayerError> {
        resolve_library_path(&self.library_dir, path)
            .ok_or_else(|| PlayerError::InvalidPath(path.to_owned()))
    }

    pub fn snapshot(&self) -> PlayerSnapshot {
        let status = &self.mixer.status;
        let on_air = status.on_air.load(Ordering::Relaxed);
        let decks = Deck::ALL
            .iter()
            .map(|&deck| {
                let deck_status = &status.decks[deck.slot()];
                let entry_id = deck_status.entry_id.load(Ordering::Relaxed);
                DeckSnapshot {
                    deck,
                    entry: self.loaded.get(&entry_id).cloned(),
                    position_ms: frames_to_ms(deck_status.position.load(Ordering::Relaxed)),
                    length_ms: frames_to_ms(deck_status.length.load(Ordering::Relaxed)),
                    on_air: entry_id != 0 && entry_id == on_air,
                }
            })
            .collect();
        PlayerSnapshot {
            paused: self.paused,
            decks,
            cued: self.cued.and_then(|cued| self.loaded.get(&cued).cloned()),
            queue: self.queue.iter().cloned().collect(),
            rendered_ms: frames_to_ms(status.frame.load(Ordering::Relaxed)),
            underruns: status.underruns.load(Ordering::Relaxed),
        }
    }
}

/// Resolves a library path, refusing anything that could leave the library.
pub fn resolve_library_path(root: &Path, path: &str) -> Option<PathBuf> {
    let mut resolved = root.to_path_buf();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return None;
        }
        resolved.push(part);
    }
    Some(resolved)
}

#[cfg(test)]
mod tests;
