// SPDX-License-Identifier: AGPL-3.0-or-later

//! The mixer: Deck A, Deck B and the FX player summed on the master bus.
//!
//! This is the code of the audio thread. It never allocates, locks, does I/O
//! or logs: it talks to the rest of the engine through lock-free ring buffers
//! (commands in, events and finished tracks out) and atomics (status, levels).
//! Tracks are dropped by the control thread, never here.
//!
//! Each deck slot has its EQ ([`GraphicEq`]): Decks A and B the music EQ's
//! settings, the FX player its own. The master bus goes through the AGC chain
//! ([`Chain`]) after the pause, then is clipped at full scale.
//!
//! Transitions are decided here, sample-accurately, from a plan computed in
//! advance ([`FrameTransition`]). The same code renders to a buffer offline,
//! which is how crossfades are tested.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use heron_core::playout::{Ramp, TrackPlayout, Transition};
use heron_core::{EQ_BANDS, FadeCurve};
use heron_dsp::agc::ChainParams;
use heron_dsp::{Chain, GraphicEq};

/// Internal sample rate.
pub const SAMPLE_RATE: u32 = 48_000;
/// Internal channel count (interleaved stereo).
pub const CHANNELS: usize = 2;
/// Frames rendered per block by the pacing clock (10 ms).
pub const BLOCK_FRAMES: usize = 480;

/// Fade applied when pausing, resuming or stopping the FX player.
const SHORT_FADE_FRAMES: u64 = 480;
/// Peak meters fall back by this much per second.
const PEAK_DECAY_DB_PER_S: f32 = 20.0;
/// Time constant of the RMS meters.
const RMS_TAU_S: f32 = 0.3;
/// The master bus is silent below this peak level: −50 dBFS, the analysis'
/// ceiling for silence.
const SILENCE_LEVEL: f32 = 0.003_162_3;

pub fn ms_to_frames(ms: u32) -> u64 {
    u64::from(ms) * u64::from(SAMPLE_RATE) / 1_000
}

pub fn frames_to_ms(frames: u64) -> u64 {
    frames * 1_000 / u64::from(SAMPLE_RATE)
}

/// Decoded audio for one track, at the internal format.
pub trait AudioSource: Send {
    /// Reads up to `out.len() / 2` frames of interleaved stereo into `out`
    /// and returns the number of frames read. Must not block or allocate.
    fn read(&mut self, out: &mut [f32]) -> usize;
    /// No more audio will come: the track ended, early or not.
    fn is_finished(&self) -> bool;
    /// Enough audio is decoded to start playing (or the source is finished).
    /// A cued track waits for this, so it never starts with an underrun.
    fn is_ready(&self) -> bool {
        true
    }
}

/// Audio held in memory. Used for offline rendering and tests.
pub struct VecSource {
    samples: Vec<f32>,
    cursor: usize,
}

impl VecSource {
    pub fn new(samples: Vec<f32>) -> Self {
        Self { samples, cursor: 0 }
    }
}

impl AudioSource for VecSource {
    fn read(&mut self, out: &mut [f32]) -> usize {
        let len = out.len().min(self.samples.len() - self.cursor);
        let len = len - len % CHANNELS;
        out[..len].copy_from_slice(&self.samples[self.cursor..self.cursor + len]);
        self.cursor += len;
        len / CHANNELS
    }

    fn is_finished(&self) -> bool {
        self.cursor >= self.samples.len()
    }
}

/// A gain ramp in frames, on a track's timeline (frame 0 is the cue-in).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRamp {
    pub start: u64,
    pub length: u64,
    pub curve: FadeCurve,
    /// Level at the quiet end of the ramp.
    pub level: f32,
}

impl FrameRamp {
    /// Converts a ramp planned in milliseconds on the file's timeline.
    pub fn from_ramp(ramp: &Ramp, cue_in_ms: u32) -> Self {
        Self {
            start: ms_to_frames(ramp.start_ms.saturating_sub(cue_in_ms)),
            length: ms_to_frames(ramp.length_ms).max(1),
            curve: ramp.curve,
            level: ramp.level,
        }
    }

    fn progress(&self, position: u64) -> f32 {
        (position.saturating_sub(self.start) as f64 / self.length as f64).min(1.0) as f32
    }

    pub fn fade_in_gain(&self, position: u64) -> f32 {
        if position < self.start {
            return self.level;
        }
        self.level + (1.0 - self.level) * self.curve.fade_in(self.progress(position))
    }

    pub fn fade_out_gain(&self, position: u64) -> f32 {
        if position < self.start {
            return 1.0;
        }
        self.level + (1.0 - self.level) * self.curve.fade_in(1.0 - self.progress(position))
    }
}

/// A transition from the track on air to the next one, in frames on the
/// outgoing track's timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameTransition {
    /// When the next track starts.
    pub next_start: u64,
    pub fade_out: Option<FrameRamp>,
}

impl FrameTransition {
    pub fn new(transition: &Transition, outgoing: &TrackPlayout) -> Self {
        Self {
            next_start: ms_to_frames(transition.next_start_ms.saturating_sub(outgoing.cue_in_ms)),
            fade_out: transition
                .fade_out
                .map(|ramp| FrameRamp::from_ramp(&ramp, outgoing.cue_in_ms)),
        }
    }

    /// The next track starts as soon as the current one ends.
    pub const fn back_to_back() -> Self {
        Self {
            next_start: u64::MAX,
            fade_out: None,
        }
    }
}

/// A track ready to play.
pub struct LoadedTrack {
    /// Identifies the track in events and status (queue entry ID). Not 0.
    pub entry_id: u64,
    pub source: Box<dyn AudioSource>,
    /// Frames from the cue-in to the cue-out.
    pub length: u64,
    /// Frame (from the cue-in) where playback starts: 0, or where the track
    /// was when the engine stopped. The source starts there.
    pub start: u64,
    /// Linear gain (loudness normalization).
    pub gain: f32,
    pub fade_in: Option<FrameRamp>,
}

/// A jingle played on the FX player over the intro of a track: it starts
/// when that track reaches `at`, so it ends where the vocals start.
pub struct Overlay {
    pub track: Box<LoadedTrack>,
    /// Frames from the cue-in of the track it plays over.
    pub at: u64,
}

/// Commands are moved through a ring buffer of fixed size: their size only
/// costs memory there. A large one is never boxed, since the box would be
/// freed on the audio thread.
#[expect(
    clippy::large_enum_variant,
    reason = "SetAgc carries the chain's settings inline: no allocation to free on the audio thread"
)]
pub enum Command {
    /// Plays `track` next. It starts at `transition.next_start` on the track
    /// on air, when the track on air ends, or at once when nothing is on air.
    /// Replaces a track already cued.
    Cue {
        track: Box<LoadedTrack>,
        transition: FrameTransition,
    },
    /// [`Command::Cue`], with a jingle on the FX player over the track's
    /// intro. Skipping the track before the jingle starts drops the jingle,
    /// skipping it while the jingle plays fades it out too.
    CueOver {
        track: Box<LoadedTrack>,
        transition: FrameTransition,
        overlay: Overlay,
    },
    /// Replans the transition into the cued track, when the track on air
    /// is not the one it was planned against.
    Retarget {
        transition: FrameTransition,
        fade_in: Option<FrameRamp>,
    },
    /// Removes the cued track (and its jingle) if it has not started.
    Uncue,
    /// Fades out the track on air and starts the cued track now.
    Skip {
        fade_frames: u64,
        curve: FadeCurve,
    },
    Pause,
    Resume,
    /// Pauses at once, without a fade: nothing plays until `Resume`, which
    /// fades in. For an engine waiting to take over from another.
    Hold,
    /// Fades out the track on air and holds the mixer as it reaches
    /// `position` (frames from its cue-in), then reports [`Event::Halted`].
    /// For an engine handing over to another, which goes on from there.
    HaltAt {
        position: u64,
    },
    /// Plays a jingle on the FX player, on top of the decks. Replaces the one playing.
    PlayFx(Box<LoadedTrack>),
    StopFx,
    /// New settings for a deck EQ: its filter gains, solved from the
    /// sliders by the control thread ([`heron_dsp::eq::filter_gains`]). They
    /// glide in, or with `glide: false` apply at once with the filters at
    /// rest: only while nothing plays.
    SetEq {
        eq: EqTarget,
        enabled: bool,
        filter_gains_db: [f32; EQ_BANDS],
        glide: bool,
    },
    /// New settings for the AGC chain. They glide in, or with `glide: false`
    /// apply at once, from rest: only while nothing plays. Not boxed: the
    /// box would be freed here, on the audio thread.
    SetAgc {
        params: ChainParams,
        glide: bool,
    },
    /// The gated AGC starts from this gain: the gain of the engine handing
    /// over, sent while nothing plays.
    StartAgcAt {
        gain_db: f32,
    },
}

/// Which deck EQ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EqTarget {
    /// Decks A and B.
    Music,
    Fx,
}

impl EqTarget {
    /// The deck slots it applies to.
    fn slots(self) -> &'static [usize] {
        match self {
            Self::Music => &[0, 1],
            Self::Fx => &[2],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deck {
    A,
    B,
    Fx,
}

impl Deck {
    pub const ALL: [Deck; 3] = [Deck::A, Deck::B, Deck::Fx];

    fn from_slot(slot: usize) -> Self {
        match slot {
            0 => Self::A,
            1 => Self::B,
            _ => Self::Fx,
        }
    }

    pub fn slot(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
            Self::Fx => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// Reached its cue-out, or the end of the audio.
    Finished,
    Skipped,
    /// The FX player was stopped.
    Stopped,
    /// A new track needed the deck.
    Replaced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Started {
        deck: Deck,
        entry_id: u64,
        /// Master frame where the track starts.
        at_frame: u64,
    },
    Ended {
        deck: Deck,
        entry_id: u64,
        played_frames: u64,
        at_frame: u64,
        reason: EndReason,
    },
    /// The track's audio was not decoded in time; silence was played.
    Underrun { deck: Deck, entry_id: u64 },
    /// A cued track was removed before it started: uncued, replaced, or
    /// its audio ended before it could start.
    Uncued { entry_id: u64 },
    /// Held after [`Command::HaltAt`]: the track on air stopped at `position`,
    /// and the master bus at `at_frame` (silence after it).
    Halted {
        entry_id: u64,
        position: u64,
        at_frame: u64,
    },
}

/// Levels of one meter point, as linear values (f32 bits).
#[derive(Debug, Default)]
pub struct MeterStatus {
    peak: [AtomicU32; CHANNELS],
    rms: [AtomicU32; CHANNELS],
}

impl MeterStatus {
    /// Peak (falling back 20 dB/s) and RMS (300 ms) per channel, linear.
    pub fn read(&self) -> ([f32; CHANNELS], [f32; CHANNELS]) {
        let load = |value: &AtomicU32| f32::from_bits(value.load(Ordering::Relaxed));
        (
            [load(&self.peak[0]), load(&self.peak[1])],
            [load(&self.rms[0]), load(&self.rms[1])],
        )
    }
}

/// The AGC chain's meters. Gains in dB as f32 bits; gain reductions hold
/// their deepest value and come back by 20 dB/s, as the peak meters.
#[derive(Debug, Default)]
pub struct AgcStatus {
    /// The chain is computed: on, or fading out.
    pub active: AtomicBool,
    /// The chain's input: the master bus before it.
    pub input: MeterStatus,
    /// The gated AGC is on, and its gain.
    pub agc_on: AtomicBool,
    pub agc_gain_db: AtomicU32,
    pub five_band_db: [AtomicU32; 5],
    pub dual_band_db: [AtomicU32; 2],
    pub final_stage_db: AtomicU32,
    /// Frames by which the chain delays the master bus now.
    pub latency_frames: AtomicU64,
}

impl AgcStatus {
    pub fn agc_gain_db(&self) -> Option<f32> {
        self.agc_on
            .load(Ordering::Relaxed)
            .then(|| f32::from_bits(self.agc_gain_db.load(Ordering::Relaxed)))
    }

    /// The gain reductions of the 5 bands, the 2 bands and the final stage.
    pub fn reductions_db(&self) -> ([f32; 5], [f32; 2], f32) {
        let load = |value: &AtomicU32| f32::from_bits(value.load(Ordering::Relaxed));
        (
            self.five_band_db.each_ref().map(load),
            self.dual_band_db.each_ref().map(load),
            load(&self.final_stage_db),
        )
    }
}

/// A gain reduction meter: the deepest of this block, or the last one coming
/// back by `rise_db`.
fn publish_reduction(meter: &AtomicU32, block_db: f32, rise_db: f32) {
    let previous = f32::from_bits(meter.load(Ordering::Relaxed));
    let value = block_db.min((previous + rise_db).min(0.0));
    meter.store(value.to_bits(), Ordering::Relaxed);
}

#[derive(Debug, Default)]
pub struct DeckStatus {
    /// 0 when the deck is empty.
    pub entry_id: AtomicU64,
    pub position: AtomicU64,
    pub length: AtomicU64,
    pub meter: MeterStatus,
}

/// What the audio thread publishes after every block.
#[derive(Debug, Default)]
pub struct MixerStatus {
    /// Frames rendered since the start.
    pub frame: AtomicU64,
    pub paused: AtomicBool,
    /// Entry ID of the track on air (the last one started on Deck A or B), 0 for none.
    pub on_air: AtomicU64,
    /// Entry ID of the cued track, 0 for none.
    pub cued: AtomicU64,
    /// The cued track has enough audio decoded to start.
    pub cued_ready: AtomicBool,
    /// Entry ID of a jingle waiting to start over the intro of the track on
    /// air, 0 for none.
    pub overlay: AtomicU64,
    pub decks: [DeckStatus; 3],
    /// The master bus, after the AGC chain.
    pub master: MeterStatus,
    pub agc: AgcStatus,
    pub underruns: AtomicU64,
    /// Frames of silence (below −50 dBFS) on the master bus before the AGC
    /// chain up to now, 0 when the last block had sound. Read by the silence
    /// watchdog.
    pub silent_frames: AtomicU64,
    /// Events or finished tracks that could not be handed to the control
    /// thread because its queues were full.
    pub overflows: AtomicU64,
}

/// The control thread's side of the mixer.
pub struct MixerHandle {
    pub commands: rtrb::Producer<Command>,
    pub events: rtrb::Consumer<Event>,
    /// Finished tracks, to be dropped outside the audio thread.
    pub trash: rtrb::Consumer<Box<LoadedTrack>>,
    pub status: Arc<MixerStatus>,
}

struct Voice {
    track: Box<LoadedTrack>,
    position: u64,
    fade_out: Option<FrameRamp>,
    stop_at: u64,
    end_reason: EndReason,
    underrun_reported: bool,
}

impl Voice {
    fn new(track: Box<LoadedTrack>) -> Self {
        let stop_at = track.length;
        Self {
            position: track.start.min(stop_at),
            track,
            fade_out: None,
            stop_at,
            end_reason: EndReason::Finished,
            underrun_reported: false,
        }
    }

    /// Fades out from the current position and stops at the end of the fade.
    fn fade_out_now(&mut self, frames: u64, curve: FadeCurve, reason: EndReason) {
        let frames = frames.max(1);
        let current = self.gain_at(self.position);
        self.fade_out = Some(FrameRamp {
            start: self.position,
            length: frames,
            curve,
            level: 0.0,
        });
        // Keep the current fade-out level: scale the new fade from it.
        self.track.gain *= current;
        self.track.fade_in = None;
        self.stop_at = self.stop_at.min(self.position + frames);
        self.end_reason = reason;
    }

    fn gain_at(&self, position: u64) -> f32 {
        let fade_in = self
            .track
            .fade_in
            .map_or(1.0, |ramp| ramp.fade_in_gain(position));
        let fade_out = self
            .fade_out
            .map_or(1.0, |ramp| ramp.fade_out_gain(position));
        fade_in * fade_out
    }
}

#[derive(Clone, Copy, Default)]
struct BlockMeter {
    peak: [f32; CHANNELS],
    sum_squares: [f32; CHANNELS],
}

impl BlockMeter {
    fn add(&mut self, left: f32, right: f32) {
        self.peak[0] = self.peak[0].max(left.abs());
        self.peak[1] = self.peak[1].max(right.abs());
        self.sum_squares[0] += left * left;
        self.sum_squares[1] += right * right;
    }

    fn publish(&self, frames: usize, meter: &MeterStatus, decay: f32, alpha: f32) {
        for channel in 0..CHANNELS {
            let previous_peak = f32::from_bits(meter.peak[channel].load(Ordering::Relaxed));
            let peak = self.peak[channel].max(previous_peak * decay);
            meter.peak[channel].store(peak.to_bits(), Ordering::Relaxed);

            let previous = f32::from_bits(meter.rms[channel].load(Ordering::Relaxed));
            let mean_square = self.sum_squares[channel] / frames.max(1) as f32;
            let previous_square = previous * previous;
            let rms = (previous_square + alpha * (mean_square - previous_square)).sqrt();
            meter.rms[channel].store(rms.to_bits(), Ordering::Relaxed);
        }
    }
}

pub struct Mixer {
    /// Deck A, Deck B, FX.
    voices: [Option<Voice>; 3],
    /// Slot of the track on air (0 or 1).
    main: Option<usize>,
    /// Slot of the last track started on a deck: decks alternate.
    last_main: usize,
    cued: Option<(Box<LoadedTrack>, FrameTransition)>,
    /// The jingle to play over the cued track's intro.
    cued_overlay: Option<Overlay>,
    /// A jingle waiting to start over the intro of the track in this slot.
    overlay: Option<(usize, Overlay)>,
    /// Slot of the track the jingle on the FX player plays over.
    fx_over: Option<usize>,
    frame: u64,
    silent_frames: u64,
    pause_gain: f32,
    paused: bool,
    frozen: bool,
    /// Where the track on air stops for a handover ([`Command::HaltAt`]).
    halt_at: Option<u64>,
    /// Slot whose fade-out was set by `HaltAt`, removed on `Resume`.
    halt_fade: Option<usize>,
    scratch: Vec<f32>,
    /// The EQ of each deck slot.
    eqs: [GraphicEq; 3],
    meters: [BlockMeter; 3],
    /// The AGC chain on the master bus.
    chain: Chain,
    commands: rtrb::Consumer<Command>,
    events: rtrb::Producer<Event>,
    trash: rtrb::Producer<Box<LoadedTrack>>,
    status: Arc<MixerStatus>,
}

impl Mixer {
    /// Creates a mixer rendering blocks of at most `max_block_frames`.
    pub fn new(max_block_frames: usize) -> (Self, MixerHandle) {
        let (command_tx, command_rx) = rtrb::RingBuffer::new(64);
        let (event_tx, event_rx) = rtrb::RingBuffer::new(256);
        let (trash_tx, trash_rx) = rtrb::RingBuffer::new(64);
        let status = Arc::new(MixerStatus::default());
        let mixer = Self {
            voices: [None, None, None],
            main: None,
            last_main: 1,
            cued: None,
            cued_overlay: None,
            overlay: None,
            fx_over: None,
            frame: 0,
            silent_frames: 0,
            pause_gain: 1.0,
            paused: false,
            frozen: false,
            halt_at: None,
            halt_fade: None,
            scratch: vec![0.0; max_block_frames * CHANNELS],
            eqs: std::array::from_fn(|_| GraphicEq::new(SAMPLE_RATE)),
            meters: [BlockMeter::default(); 3],
            chain: Chain::new(SAMPLE_RATE),
            commands: command_rx,
            events: event_tx,
            trash: trash_tx,
            status: Arc::clone(&status),
        };
        let handle = MixerHandle {
            commands: command_tx,
            events: event_rx,
            trash: trash_rx,
            status,
        };
        (mixer, handle)
    }

    /// Renders the next `out.len() / 2` frames of the master bus.
    pub fn render(&mut self, out: &mut [f32]) {
        let max = self.scratch.len();
        for block in out.chunks_mut(max) {
            self.render_block(block);
        }
    }

    fn render_block(&mut self, out: &mut [f32]) {
        let frames = out.len() / CHANNELS;
        let out = &mut out[..frames * CHANNELS];
        out.fill(0.0);
        self.apply_commands();
        self.meters = [BlockMeter::default(); 3];

        if !self.frozen {
            let mut offset = 0;
            let mut halted = false;
            while offset < frames {
                self.retire_finished();
                self.start_cued_if_due();
                self.start_overlay_if_due();
                let mut segment = self.next_segment(frames - offset);
                // The track on air first: when its audio ends early, the
                // segment ends there, so the next track starts on that frame.
                let main = self.main;
                if let Some(slot) = main {
                    let range = offset * CHANNELS..(offset + segment) * CHANNELS;
                    let read = self.render_voice(slot, &mut out[range]);
                    if self.voices[slot]
                        .as_ref()
                        .is_some_and(|voice| voice.position >= voice.stop_at)
                    {
                        segment = read;
                    }
                }
                for slot in (0..self.voices.len()).filter(|&slot| Some(slot) != main) {
                    let range = offset * CHANNELS..(offset + segment) * CHANNELS;
                    self.render_voice(slot, &mut out[range]);
                }
                offset += segment;
                self.frame += segment as u64;
                if self.halt_due() {
                    self.halt();
                    // The rest of the block stays silent.
                    self.frame += (frames - offset) as u64;
                    halted = true;
                    break;
                }
            }
            if !halted {
                self.retire_finished();
                self.start_cued_if_due();
                self.start_overlay_if_due();
                self.apply_pause(out);
            }
        } else {
            self.frame += frames as u64;
        }

        // Silence is what the decks play: the AGC chain's gain never hides it.
        let mut input = BlockMeter::default();
        for frame in out.as_chunks::<CHANNELS>().0 {
            input.add(frame[0], frame[1]);
        }
        if input.peak[0].max(input.peak[1]) < SILENCE_LEVEL {
            self.silent_frames += frames as u64;
        } else {
            self.silent_frames = 0;
        }
        self.chain.process(out);
        let mut master = BlockMeter::default();
        for frame in out.as_chunks_mut::<CHANNELS>().0 {
            // A last safety: never let anything over full scale out.
            frame[0] = frame[0].clamp(-1.0, 1.0);
            frame[1] = frame[1].clamp(-1.0, 1.0);
            master.add(frame[0], frame[1]);
        }
        self.publish(frames, &master, &input);
    }

    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Cue { track, transition } => {
                    self.drop_cued();
                    self.cued = Some((track, transition));
                }
                Command::CueOver {
                    track,
                    transition,
                    overlay,
                } => {
                    self.drop_cued();
                    self.cued = Some((track, transition));
                    self.cued_overlay = Some(overlay);
                }
                Command::Retarget {
                    transition,
                    fade_in,
                } => {
                    if let Some((track, planned)) = &mut self.cued {
                        *planned = transition;
                        track.fade_in = fade_in;
                    }
                }
                Command::Uncue => self.drop_cued(),
                Command::Skip { fade_frames, curve } => self.skip(fade_frames, curve),
                Command::Pause => self.paused = true,
                Command::Resume => {
                    self.paused = false;
                    self.frozen = false;
                    // A handover given up: the track on air plays on.
                    if let Some(slot) = self.halt_fade.take()
                        && let Some(voice) = &mut self.voices[slot]
                        && voice.end_reason == EndReason::Finished
                    {
                        voice.fade_out = None;
                    }
                }
                Command::Hold => {
                    self.paused = true;
                    self.pause_gain = 0.0;
                    self.frozen = true;
                }
                Command::HaltAt { position } => self.halt_at_position(position),
                Command::PlayFx(track) => {
                    self.stop_slot(2, EndReason::Replaced);
                    self.start_voice(2, track);
                }
                Command::StopFx => {
                    if let Some(voice) = &mut self.voices[2] {
                        voice.fade_out_now(
                            SHORT_FADE_FRAMES,
                            FadeCurve::Linear,
                            EndReason::Stopped,
                        );
                    }
                }
                Command::SetEq {
                    eq,
                    enabled,
                    filter_gains_db,
                    glide,
                } => {
                    for &slot in eq.slots() {
                        if glide {
                            self.eqs[slot].set(enabled, filter_gains_db);
                        } else {
                            self.eqs[slot].restart(enabled, filter_gains_db);
                        }
                    }
                }
                Command::SetAgc { params, glide } => {
                    if glide {
                        self.chain.set(&params);
                    } else {
                        self.chain.restart(&params);
                    }
                }
                Command::StartAgcAt { gain_db } => self.chain.start_agc_at(gain_db),
            }
        }
    }

    /// Removes the cued track and its jingle, if any.
    fn drop_cued(&mut self) {
        if let Some((track, _)) = self.cued.take() {
            self.emit(Event::Uncued {
                entry_id: track.entry_id,
            });
            self.discard(track);
        }
        if let Some(overlay) = self.cued_overlay.take() {
            self.drop_overlay(overlay);
        }
    }

    /// A jingle that will not play: reported as uncued.
    fn drop_overlay(&mut self, overlay: Overlay) {
        self.emit(Event::Uncued {
            entry_id: overlay.track.entry_id,
        });
        self.discard(overlay.track);
    }

    /// Starts the jingle waiting for the intro of the track it plays over.
    fn start_overlay_if_due(&mut self) {
        let due = self.overlay.as_ref().is_some_and(|(slot, overlay)| {
            self.voices[*slot]
                .as_ref()
                .is_some_and(|voice| voice.position >= overlay.at)
        });
        if due && let Some((slot, overlay)) = self.overlay.take() {
            self.stop_slot(2, EndReason::Replaced);
            self.start_voice(2, overlay.track);
            self.fx_over = Some(slot);
        }
    }

    fn halt_at_position(&mut self, position: u64) {
        if self.frozen {
            // Paused already: nothing more will play.
            self.halt();
            return;
        }
        let main = self.main;
        let Some(voice) = main.and_then(|slot| self.voices[slot].as_mut()) else {
            // Nothing on air: hold now.
            self.halt_at = Some(0);
            return;
        };
        let position = position.max(voice.position);
        let start = position
            .saturating_sub(SHORT_FADE_FRAMES)
            .max(voice.position);
        if voice.fade_out.is_none() {
            voice.fade_out = Some(FrameRamp {
                start,
                length: (position - start).max(1),
                curve: FadeCurve::Linear,
                level: 0.0,
            });
            self.halt_fade = main;
        }
        self.halt_at = Some(position);
    }

    /// The track on air reached the handover point.
    fn halt_due(&self) -> bool {
        self.halt_at.is_some_and(|position| {
            self.main_voice()
                .is_none_or(|voice| voice.position >= position)
        })
    }

    fn halt(&mut self) {
        self.halt_at = None;
        self.paused = true;
        self.pause_gain = 0.0;
        self.frozen = true;
        let (entry_id, position) = self
            .main_voice()
            .map_or((0, 0), |voice| (voice.track.entry_id, voice.position));
        self.emit(Event::Halted {
            entry_id,
            position,
            at_frame: self.frame,
        });
    }

    fn skip(&mut self, fade_frames: u64, curve: FadeCurve) {
        if let Some(main) = self.main
            && let Some(voice) = &mut self.voices[main]
        {
            voice.fade_out_now(fade_frames, curve, EndReason::Skipped);
            // Its jingle goes with it.
            if let Some((slot, _)) = &self.overlay
                && *slot == main
                && let Some((_, overlay)) = self.overlay.take()
            {
                self.drop_overlay(overlay);
            }
            if self.fx_over == Some(main)
                && let Some(fx) = &mut self.voices[2]
            {
                fx.fade_out_now(fade_frames, curve, EndReason::Skipped);
            }
        }
        if let Some((track, _)) = self.cued.take() {
            self.start_main(track, None);
        }
    }

    /// Starts the cued track when the track on air reaches the transition
    /// point or has ended.
    fn start_cued_if_due(&mut self) {
        // Nothing to play (the file could not be decoded): never on air.
        if self
            .cued
            .as_ref()
            .is_some_and(|(track, _)| track.source.is_finished())
            && let Some((track, _)) = self.cued.take()
        {
            self.emit(Event::Uncued {
                entry_id: track.entry_id,
            });
            self.discard(track);
        }
        let due = match (&self.cued, self.main_voice()) {
            (None, _) => false,
            (Some((track, _)), None) => track.source.is_ready(),
            (Some((track, transition)), Some(voice)) => {
                voice.position >= transition.next_start && track.source.is_ready()
            }
        };
        if due && let Some((track, transition)) = self.cued.take() {
            self.start_main(track, transition.fade_out);
        }
    }

    fn start_main(&mut self, track: Box<LoadedTrack>, fade_out: Option<FrameRamp>) {
        let slot = match self.main {
            Some(main) => {
                if let Some(voice) = &mut self.voices[main]
                    && voice.end_reason == EndReason::Finished
                {
                    voice.fade_out = fade_out;
                }
                1 - main
            }
            None => {
                let other = 1 - self.last_main;
                if self.voices[other].is_none() || self.voices[self.last_main].is_some() {
                    other
                } else {
                    self.last_main
                }
            }
        };
        // Still busy with an older track's tail: it has to go.
        self.stop_slot(slot, EndReason::Replaced);
        self.start_voice(slot, track);
        self.main = Some(slot);
        self.last_main = slot;
        if let Some(overlay) = self.cued_overlay.take() {
            if let Some((_, previous)) = self.overlay.take() {
                self.drop_overlay(previous);
            }
            self.overlay = Some((slot, overlay));
        }
    }

    fn start_voice(&mut self, slot: usize, track: Box<LoadedTrack>) {
        self.emit(Event::Started {
            deck: Deck::from_slot(slot),
            entry_id: track.entry_id,
            at_frame: self.frame,
        });
        self.voices[slot] = Some(Voice::new(track));
        // Nothing of the previous track rings into this one.
        self.eqs[slot].restart_track();
    }

    fn main_voice(&self) -> Option<&Voice> {
        self.main.and_then(|slot| self.voices[slot].as_ref())
    }

    /// Frames until the next thing that must happen at an exact frame.
    /// Frames until the next thing that must happen at an exact frame: a
    /// track ending, or the cued track starting.
    fn next_segment(&self, remaining: usize) -> usize {
        let mut until = self
            .voices
            .iter()
            .flatten()
            .map(|voice| voice.stop_at.saturating_sub(voice.position))
            .min()
            .unwrap_or(u64::MAX);
        if let Some((_, transition)) = &self.cued
            && let Some(voice) = self.main_voice()
        {
            until = until.min(transition.next_start.saturating_sub(voice.position));
        }
        if let Some((slot, overlay)) = &self.overlay
            && let Some(voice) = &self.voices[*slot]
        {
            until = until.min(overlay.at.saturating_sub(voice.position));
        }
        if let Some(position) = self.halt_at
            && let Some(voice) = self.main_voice()
        {
            until = until.min(position.saturating_sub(voice.position));
        }
        usize::try_from(until)
            .unwrap_or(usize::MAX)
            .clamp(1, remaining)
    }

    /// Adds the voice in `slot` to `out` and returns the frames it played.
    fn render_voice(&mut self, slot: usize, out: &mut [f32]) -> usize {
        let Some(voice) = &mut self.voices[slot] else {
            return 0;
        };
        let wanted = (out.len() / CHANNELS).min(
            usize::try_from(voice.stop_at.saturating_sub(voice.position)).unwrap_or(usize::MAX),
        );
        if wanted == 0 {
            return 0;
        }
        let scratch = &mut self.scratch[..wanted * CHANNELS];
        let read = voice.track.source.read(scratch);
        let played = &mut scratch[..read * CHANNELS];
        for (index, frame) in played.as_chunks_mut::<CHANNELS>().0.iter_mut().enumerate() {
            let gain = voice.track.gain * voice.gain_at(voice.position + index as u64);
            frame[0] *= gain;
            frame[1] *= gain;
        }
        self.eqs[slot].process(played);
        let meter = &mut self.meters[slot];
        for (input, output) in played
            .as_chunks::<CHANNELS>()
            .0
            .iter()
            .zip(out.as_chunks_mut::<CHANNELS>().0)
        {
            output[0] += input[0];
            output[1] += input[1];
            meter.add(input[0], input[1]);
        }
        voice.position += read as u64;
        if read < wanted {
            if voice.track.source.is_finished() {
                // The audio ended before the cue-out.
                voice.stop_at = voice.position;
            } else {
                self.status.underruns.fetch_add(1, Ordering::Relaxed);
                if !voice.underrun_reported {
                    voice.underrun_reported = true;
                    let entry_id = voice.track.entry_id;
                    self.emit(Event::Underrun {
                        deck: Deck::from_slot(slot),
                        entry_id,
                    });
                }
            }
        }
        read
    }

    fn retire_finished(&mut self) {
        for slot in 0..self.voices.len() {
            // Sources that end early set `stop_at` when they are read.
            if let Some(voice) = &self.voices[slot]
                && voice.position >= voice.stop_at
            {
                let reason = voice.end_reason;
                self.stop_slot(slot, reason);
            }
        }
    }

    fn stop_slot(&mut self, slot: usize, reason: EndReason) {
        let Some(voice) = self.voices[slot].take() else {
            return;
        };
        if self.main == Some(slot) {
            self.main = None;
        }
        // The jingle over this track's intro, or the track under it.
        if slot == 2 || self.fx_over == Some(slot) {
            self.fx_over = None;
        }
        // The track a jingle waits for ended before its intro.
        if let Some((waiting, _)) = &self.overlay
            && *waiting == slot
            && let Some((_, overlay)) = self.overlay.take()
        {
            self.drop_overlay(overlay);
        }
        self.emit(Event::Ended {
            deck: Deck::from_slot(slot),
            entry_id: voice.track.entry_id,
            played_frames: voice.position,
            at_frame: self.frame,
            reason,
        });
        self.discard(voice.track);
    }

    fn apply_pause(&mut self, out: &mut [f32]) {
        let target = if self.paused { 0.0 } else { 1.0 };
        if self.pause_gain == target {
            if self.paused {
                out.fill(0.0);
                self.frozen = true;
            }
            return;
        }
        let step = 1.0 / SHORT_FADE_FRAMES as f32;
        for frame in out.as_chunks_mut::<CHANNELS>().0 {
            self.pause_gain = if self.paused {
                (self.pause_gain - step).max(0.0)
            } else {
                (self.pause_gain + step).min(1.0)
            };
            frame[0] *= self.pause_gain;
            frame[1] *= self.pause_gain;
        }
        if self.paused && self.pause_gain == 0.0 {
            self.frozen = true;
        }
    }

    fn emit(&mut self, event: Event) {
        if self.events.push(event).is_err() {
            self.status.overflows.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Hands a track to the control thread, which drops it.
    fn discard(&mut self, track: Box<LoadedTrack>) {
        if let Err(rtrb::PushError::Full(track)) = self.trash.push(track) {
            self.status.overflows.fetch_add(1, Ordering::Relaxed);
            // Never expected: the control thread empties the trash every few ms.
            // Leaking is better than freeing memory on the audio thread.
            std::mem::forget(track);
        }
    }

    fn publish(&mut self, frames: usize, master: &BlockMeter, input: &BlockMeter) {
        let seconds = frames as f32 / SAMPLE_RATE as f32;
        let decay = 10f32.powf(-PEAK_DECAY_DB_PER_S * seconds / 20.0);
        let alpha = 1.0 - (-seconds / RMS_TAU_S).exp();
        let status = &self.status;
        status.frame.store(self.frame, Ordering::Relaxed);
        status
            .silent_frames
            .store(self.silent_frames, Ordering::Relaxed);
        status.paused.store(self.paused, Ordering::Relaxed);
        let main_id = self.main_voice().map_or(0, |voice| voice.track.entry_id);
        status.on_air.store(main_id, Ordering::Relaxed);
        let cued_id = self.cued.as_ref().map_or(0, |(track, _)| track.entry_id);
        status.cued.store(cued_id, Ordering::Relaxed);
        let cued_ready = self
            .cued
            .as_ref()
            .is_some_and(|(track, _)| track.source.is_ready());
        status.cued_ready.store(cued_ready, Ordering::Relaxed);
        let overlay = self
            .overlay
            .as_ref()
            .map_or(0, |(_, overlay)| overlay.track.entry_id);
        status.overlay.store(overlay, Ordering::Relaxed);
        for (slot, deck) in status.decks.iter().enumerate() {
            let (entry_id, position, length) =
                self.voices[slot].as_ref().map_or((0, 0, 0), |voice| {
                    (voice.track.entry_id, voice.position, voice.track.length)
                });
            deck.entry_id.store(entry_id, Ordering::Relaxed);
            deck.position.store(position, Ordering::Relaxed);
            deck.length.store(length, Ordering::Relaxed);
            self.meters[slot].publish(frames, &deck.meter, decay, alpha);
        }
        master.publish(frames, &status.master, decay, alpha);

        let agc = &status.agc;
        input.publish(frames, &agc.input, decay, alpha);
        let meters = self.chain.take_meters();
        let rise = PEAK_DECAY_DB_PER_S * seconds;
        for (meter, &block) in agc.five_band_db.iter().zip(&meters.five_band_db) {
            publish_reduction(meter, block, rise);
        }
        for (meter, &block) in agc.dual_band_db.iter().zip(&meters.dual_band_db) {
            publish_reduction(meter, block, rise);
        }
        publish_reduction(&agc.final_stage_db, meters.final_stage_db, rise);
        agc.agc_on
            .store(meters.agc_gain_db.is_some(), Ordering::Relaxed);
        agc.agc_gain_db.store(
            meters.agc_gain_db.unwrap_or_default().to_bits(),
            Ordering::Relaxed,
        );
        agc.active.store(self.chain.is_active(), Ordering::Relaxed);
        agc.latency_frames
            .store(self.chain.latency() as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests;
