// SPDX-License-Identifier: AGPL-3.0-or-later

//! The running engine: the audio thread, the player, the outputs, and the
//! task keeping them in step and saving the queue.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use heron_core::{AgcSettings, EqSettings, OutputsConfig, PlayoutSettings};
use heron_outputs::{ConfigError, NowPlaying, Outputs};
use heron_service::HealthCheck;
use tokio::sync::{broadcast, watch};

use crate::clock::{AudioThread, ClockStatus};
use crate::config::{ConfigFileError, EngineConfig};
use crate::fallback::Fallback;
use crate::mixer::{BLOCK_FRAMES, CHANNELS, Mixer, MixerStatus, SAMPLE_RATE};
use crate::player::{Halted, HandoverPoint, Player, PlayerEvent, SavedQueue};
use crate::queue_file::{self, QueueWriter};

/// Audio waiting between the audio thread and the encoders, at most.
const MASTER_BUFFER_SECONDS: usize = 2;
/// How often the player handles the mixer's events.
const TICK: Duration = Duration::from_millis(10);
/// The queue is saved when it changes, and this often for the position.
const SAVE_EVERY: Duration = Duration::from_secs(5);
/// With the audio thread stuck this long, the process exits so Docker
/// restarts it, resuming the saved queue.
const STUCK_EXIT: Duration = Duration::from_secs(10);

pub struct EngineOptions {
    pub library_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Tracks played when the queue runs dry. `None`: silence.
    pub fallback_dir: Option<PathBuf>,
    /// Queue entries whose file is read into memory ahead of time.
    pub preload_tracks: usize,
    /// Exit the process when the audio thread is stuck (production).
    pub exit_when_stuck: bool,
    /// Play the master bus on the local sound card (`monitor` feature).
    pub monitor: bool,
    /// The relay network outputs connect through (`heron-engine relay`), so
    /// their connections can be handed over to an engine in another container.
    pub relay_socket: Option<PathBuf>,
    /// Start as a standby, to take over from a running engine: silent, no
    /// outputs, the saved state neither read nor written, until
    /// [`Engine::complete_takeover`] or [`Engine::cold_start`].
    pub standby: bool,
}

impl EngineOptions {
    /// Options for an engine without fallback directory.
    pub fn new(library_dir: PathBuf, state_dir: PathBuf) -> Self {
        Self {
            library_dir,
            state_dir,
            fallback_dir: None,
            preload_tracks: crate::player::PRELOAD_TRACKS,
            exit_when_stuck: false,
            monitor: false,
            relay_socket: None,
            standby: false,
        }
    }
}

pub struct Engine {
    player: Mutex<Player>,
    outputs: Mutex<Outputs>,
    state_dir: PathBuf,
    events: broadcast::Sender<PlayerEvent>,
    status: Arc<MixerStatus>,
    clock: Arc<ClockStatus>,
    queue_writer: QueueWriter,
    /// Set when shutting down: the last save is made, no other may follow.
    /// Set too while a standby: the state belongs to the active engine.
    closing: AtomicBool,
    standby: AtomicBool,
    /// Outputs to start when taking over ([`Engine::prepare_takeover`]).
    takeover_outputs: Mutex<Option<OutputsConfig>>,
    started: Instant,
    started_at: SystemTime,
    #[cfg(feature = "monitor")]
    _monitor: Option<crate::monitor::Monitor>,
    _audio: AudioThread,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    ConfigFile(#[from] ConfigFileError),
    #[error("cannot start a thread: {0}")]
    Thread(#[from] std::io::Error),
    #[error(transparent)]
    Player(#[from] crate::player::PlayerError),
}

impl Engine {
    /// Starts playing (silence until tracks are queued) and streaming to the
    /// saved outputs.
    pub fn start(
        options: EngineOptions,
        runtime: tokio::runtime::Handle,
    ) -> Result<Arc<Self>, EngineError> {
        // Present from the start, so dashboards and alerts see them before
        // the first event.
        for counter in [
            "heron_engine_silence_skips_total",
            "heron_engine_fallback_tracks_total",
            "heron_engine_preload_failures_total",
            "heron_engine_underrun_tracks_total",
            "heron_engine_stalled_tracks_total",
        ] {
            metrics::counter!(counter).absolute(0);
        }
        metrics::gauge!("heron_engine_dead_air").set(0.0);
        let config = if options.standby {
            // Handed over by the active engine.
            EngineConfig::default()
        } else {
            load_config(&options.state_dir)
        };

        let (mixer, handle) = Mixer::new(BLOCK_FRAMES);
        let status = Arc::clone(&handle.status);
        let (master_tx, master_rx) =
            rtrb::RingBuffer::new(MASTER_BUFFER_SECONDS * SAMPLE_RATE as usize * CHANNELS);
        let (now_playing_tx, now_playing_rx) = watch::channel(NowPlaying::default());
        let (events, _) = broadcast::channel(256);

        let mut outputs = Outputs::new(master_rx, now_playing_rx, runtime.clone())?;
        outputs.set_relay(options.relay_socket.clone());
        if let Err(error) = outputs.apply(&config.outputs) {
            tracing::error!(%error, "cannot start the saved outputs");
        }
        #[cfg(feature = "monitor")]
        let monitor = if options.monitor {
            crate::monitor::Monitor::start(outputs.tap())
                .inspect_err(|error| tracing::error!(%error, "cannot start the monitor output"))
                .ok()
        } else {
            None
        };
        #[cfg(not(feature = "monitor"))]
        if options.monitor {
            tracing::warn!("built without the monitor feature: no monitor output");
        }
        let mut player = Player::new(
            options.library_dir,
            config.playout,
            handle,
            events.clone(),
            now_playing_tx,
        )
        .with_preload(options.preload_tracks);
        if let Some(dir) = options.fallback_dir {
            player = player.with_fallback(Fallback::start(dir, &options.state_dir));
        }
        // Before anything plays: the first sample has the EQ and the AGC chain.
        player.set_eq(config.eq, false)?;
        player.set_agc(config.agc, false)?;
        if options.standby {
            player.hold().map_err(std::io::Error::other)?;
        } else {
            player.restore(queue_file::load(&options.state_dir));
        }
        let queue_writer = QueueWriter::start(options.state_dir.clone())?;
        let clock = Arc::new(ClockStatus::default());
        let audio = AudioThread::spawn(mixer, master_tx, Arc::clone(&clock))?;
        if options.exit_when_stuck {
            exit_when_stuck(Arc::clone(&status))?;
        }
        tracing::info!(
            outputs = config.outputs.outputs.len(),
            standby = options.standby,
            "audio engine started"
        );

        let engine = Arc::new(Self {
            player: Mutex::new(player),
            outputs: Mutex::new(outputs),
            state_dir: options.state_dir,
            events,
            status,
            clock,
            queue_writer,
            closing: AtomicBool::new(options.standby),
            standby: AtomicBool::new(options.standby),
            takeover_outputs: Mutex::new(None),
            started: Instant::now(),
            started_at: SystemTime::now(),
            #[cfg(feature = "monitor")]
            _monitor: monitor,
            _audio: audio,
        });
        runtime.spawn(run_player(Arc::downgrade(&engine)));
        Ok(engine)
    }

    pub fn player(&self) -> MutexGuard<'_, Player> {
        self.player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn outputs(&self) -> MutexGuard<'_, Outputs> {
        self.outputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// When the engine started.
    pub fn started_at(&self) -> SystemTime {
        self.started_at
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PlayerEvent> {
        self.events.subscribe()
    }

    pub fn status(&self) -> &MixerStatus {
        &self.status
    }

    pub fn status_handle(&self) -> Arc<MixerStatus> {
        Arc::clone(&self.status)
    }

    pub fn config(&self) -> EngineConfig {
        let (playout, eq, agc) = {
            let player = self.player();
            (*player.settings(), *player.eq(), *player.agc())
        };
        EngineConfig {
            playout,
            outputs: self.outputs().config().clone(),
            eq,
            agc,
        }
    }

    pub fn set_playout_settings(&self, settings: PlayoutSettings) -> Result<(), EngineError> {
        self.player().set_settings(settings)?;
        self.save()
    }

    /// New deck EQ settings, gliding in.
    pub fn set_eq(&self, eq: EqSettings) -> Result<(), EngineError> {
        self.player().set_eq(eq, true)?;
        self.save()
    }

    /// New AGC chain settings, gliding in.
    pub fn set_agc(&self, agc: AgcSettings) -> Result<(), EngineError> {
        self.player().set_agc(agc, true)?;
        self.save()
    }

    /// Frames by which the AGC chain delays the master bus now.
    pub fn agc_latency_frames(&self) -> u64 {
        self.status.agc.latency_frames.load(Ordering::Relaxed)
    }

    /// The gated AGC's gain now, while it is on.
    pub fn agc_gain_db(&self) -> Option<f32> {
        self.status.agc.agc_gain_db()
    }

    /// Applies new outputs; an invalid configuration changes nothing.
    pub fn set_outputs(&self, config: &OutputsConfig) -> Result<(), EngineError> {
        self.outputs().apply(config)?;
        self.save()
    }

    fn save(&self) -> Result<(), EngineError> {
        self.config().save(&self.state_dir)?;
        Ok(())
    }

    /// Saves the queue a last time and waits for it to be on disk (when
    /// shutting down). No periodic save follows it.
    pub fn save_queue(&self) {
        // Under the player's lock, like periodic saves: none can slip in
        // after this one with an older queue.
        let (number, waiting) = {
            let player = self.player();
            self.closing.store(true, Ordering::Relaxed);
            let queue = player.saved();
            let waiting = queue.waiting.len();
            (self.queue_writer.submit(queue), waiting)
        };
        if self
            .queue_writer
            .wait_written(number, Duration::from_secs(2))
        {
            tracing::info!(waiting, "queue saved");
        } else {
            tracing::error!("the queue could not be saved in time");
        }
    }

    /// Waiting to take over from another engine.
    pub fn is_standby(&self) -> bool {
        self.standby.load(Ordering::Relaxed)
    }

    /// Standby: gets ready to go on with `queue` and `config`, handed over
    /// by the active engine. The track on air is opened where the active
    /// engine will stop it; nothing plays yet. May be called again, with a
    /// later stopping point.
    pub fn prepare_takeover(&self, config: EngineConfig, queue: SavedQueue) {
        let mut player = self.player();
        if let Err(error) = player.set_settings(config.playout) {
            tracing::warn!(%error, "cannot set the playout settings handed over");
        }
        // Held, nothing plays: the EQ and the AGC chain are there from the
        // first sample.
        if let Err(error) = player.set_eq(config.eq, false) {
            tracing::warn!(%error, "cannot set the deck EQ handed over");
        }
        if let Err(error) = player.set_agc(config.agc, false) {
            tracing::warn!(%error, "cannot set the AGC chain handed over");
        }
        if let Err(error) = player.reset() {
            tracing::warn!(%error, "cannot forget the queue prepared before");
        }
        player.take_over(queue);
        *self
            .takeover_outputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config.outputs);
    }

    /// Standby: the track to go on with has enough audio decoded.
    pub fn takeover_ready(&self) -> bool {
        self.player().cued_ready()
    }

    /// Standby: goes on air, streaming to the outputs handed over
    /// (`connections`, by output ID; the others connect), the gated AGC at
    /// the old engine's gain, and becomes the active engine, saving its state.
    pub fn complete_takeover(
        &self,
        connections: HashMap<u64, heron_outputs::HandedConnection>,
        agc_gain_db: Option<f32>,
    ) {
        let outputs = self
            .takeover_outputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or_default();
        if let Err(error) = self.outputs().apply_adopting(&outputs, connections) {
            tracing::error!(%error, "cannot start the outputs handed over");
        }
        let mut player = self.player();
        if let Some(gain_db) = agc_gain_db
            && let Err(error) = player.start_agc_at(gain_db)
        {
            tracing::warn!(%error, "cannot start the AGC at the gain handed over");
        }
        if let Err(error) = player.unhold() {
            tracing::error!(%error, "cannot start playing");
        }
        drop(player);
        self.activate();
    }

    /// Standby: no engine handed over; starts like a restarted engine, from
    /// the saved configuration and queue.
    pub fn cold_start(&self) {
        let config = load_config(&self.state_dir);
        {
            let mut player = self.player();
            if let Err(error) = player.set_settings(config.playout) {
                tracing::warn!(%error, "cannot set the saved playout settings");
            }
            // Held, nothing plays yet.
            if let Err(error) = player.set_eq(config.eq, false) {
                tracing::warn!(%error, "cannot set the saved deck EQ");
            }
            if let Err(error) = player.set_agc(config.agc, false) {
                tracing::warn!(%error, "cannot set the saved AGC chain");
            }
            if let Err(error) = player.reset() {
                tracing::warn!(%error, "cannot forget the queue prepared before");
            }
            player.restore(queue_file::load(&self.state_dir));
            if let Err(error) = player.unhold() {
                tracing::error!(%error, "cannot start playing");
            }
        }
        if let Err(error) = self.outputs().apply(&config.outputs) {
            tracing::error!(%error, "cannot start the saved outputs");
        }
        self.activate();
    }

    fn activate(&self) {
        self.standby.store(false, Ordering::Relaxed);
        {
            let player = self.player();
            self.closing.store(false, Ordering::Relaxed);
            self.queue_writer.submit(player.saved());
        }
        if let Err(error) = self.save() {
            tracing::error!(%error, "cannot save the configuration");
        }
    }

    /// Where the track on air can stop to hand over, `lead_ms` from now.
    pub fn handover_point(&self, lead_ms: u32) -> HandoverPoint {
        self.player().handover_point(lead_ms)
    }

    /// Holds the mixer silent at once (nothing on air to stop).
    pub fn hold(&self) -> Result<(), crate::player::PlayerError> {
        self.player().hold()
    }

    /// Stops the track on air at `position` (frames from its cue-in).
    pub fn halt_at(&self, position: u64) -> Result<(), crate::player::PlayerError> {
        self.player().halt_at(position)
    }

    pub fn take_halted(&self) -> Option<Halted> {
        self.player().take_halted()
    }

    /// Plays on after a handover given up.
    pub fn unhold(&self) -> Result<(), crate::player::PlayerError> {
        self.player().unhold()
    }

    /// Position of the track on air, in frames from its cue-in.
    pub fn on_air_position(&self) -> Option<u64> {
        let status = &self.status;
        let on_air = status.on_air.load(Ordering::Relaxed);
        status.decks[..2]
            .iter()
            .find(|deck| on_air != 0 && deck.entry_id.load(Ordering::Relaxed) == on_air)
            .map(|deck| deck.position.load(Ordering::Relaxed))
    }

    /// The queue to hand over, the track on air stopping at `position_ms`.
    pub fn handed_over_queue(&self, position_ms: Option<u64>) -> SavedQueue {
        self.player().handed_over(position_ms)
    }

    /// Handing over: no more audio for the outputs after master frame
    /// `frame`, then every output gives up its connection.
    pub fn release_outputs(
        &self,
        after_frame: Option<u64>,
    ) -> Vec<(
        u64,
        tokio::sync::oneshot::Receiver<Option<heron_outputs::HandedConnection>>,
    )> {
        let mut outputs = self.outputs();
        if let Some(frame) = after_frame {
            outputs.stop_after(frame);
        }
        outputs.release()
    }

    /// Handed over: the state now belongs to the new engine.
    pub fn stop_saving(&self) {
        let _player = self.player();
        self.closing.store(true, Ordering::Relaxed);
    }

    /// Problems listed in `/healthz` without making the engine unhealthy.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(warning) = heron_service::clock::status().warning {
            warnings.push(warning);
        }
        if self.player().dead_air() {
            warnings.push(
                "nothing to play: the queue is empty and the fallback directory has no track"
                    .to_owned(),
            );
        }
        warnings
    }

    fn record_metrics(&self) {
        let status = &self.status;
        let rendered = status.frame.load(Ordering::Relaxed) as f64 / f64::from(SAMPLE_RATE);
        metrics::gauge!("heron_engine_rendered_seconds").set(rendered);
        // Audio rendered minus time elapsed: constant (the render lead) when
        // the clock keeps time; it moves only when the clock resets.
        metrics::gauge!("heron_engine_clock_drift_seconds")
            .set(rendered - self.started.elapsed().as_secs_f64());
        let late_us = self.clock.max_late_us.swap(0, Ordering::Relaxed);
        metrics::gauge!("heron_engine_clock_max_late_seconds").set(late_us as f64 / 1e6);
        metrics::counter!("heron_engine_clock_resets_total")
            .absolute(self.clock.resets.load(Ordering::Relaxed));
        metrics::counter!("heron_engine_dropped_blocks_total")
            .absolute(self.clock.dropped_blocks.load(Ordering::Relaxed));
        metrics::counter!("heron_engine_underruns_total")
            .absolute(status.underruns.load(Ordering::Relaxed));
        metrics::counter!("heron_engine_mixer_overflows_total")
            .absolute(status.overflows.load(Ordering::Relaxed));
        metrics::gauge!("heron_engine_paused")
            .set(f64::from(u8::from(status.paused.load(Ordering::Relaxed))));
        let (peak, _) = status.master.read();
        metrics::gauge!("heron_engine_master_peak").set(f64::from(peak[0].max(peak[1])));
        metrics::gauge!("heron_engine_queue_length")
            .set(self.player().snapshot().queue.len() as f64);
        metrics::gauge!("heron_engine_silent_seconds")
            .set(status.silent_frames.load(Ordering::Relaxed) as f64 / f64::from(SAMPLE_RATE));
        heron_service::clock::status().record_metrics();
    }
}

fn load_config(dir: &std::path::Path) -> EngineConfig {
    EngineConfig::load(dir).unwrap_or_else(|error| {
        // Better on air with defaults than off air.
        tracing::error!(%error, "ignoring the saved configuration");
        EngineConfig::default()
    })
}

/// Exits the process when the audio thread stops rendering: Docker's
/// restart policy brings the engine back, with the saved queue.
fn exit_when_stuck(status: Arc<MixerStatus>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("heron-watchdog".to_owned())
        .spawn(move || {
            let mut last = status.frame.load(Ordering::Relaxed);
            let mut since = Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let frame = status.frame.load(Ordering::Relaxed);
                if frame != last {
                    last = frame;
                    since = Instant::now();
                } else if since.elapsed() >= STUCK_EXIT {
                    tracing::error!(
                        seconds = STUCK_EXIT.as_secs(),
                        "the audio thread is stuck: exiting to be restarted"
                    );
                    std::process::exit(1);
                }
            }
        })?;
    Ok(())
}

/// Ticks the player until the engine is dropped.
async fn run_player(engine: std::sync::Weak<Engine>) {
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks = 0_u64;
    let mut saved_at = Instant::now();
    loop {
        interval.tick().await;
        let Some(engine) = engine.upgrade() else {
            return;
        };
        {
            let mut player = engine.player();
            player.tick();
            let due = player.take_changed() || saved_at.elapsed() >= SAVE_EVERY;
            // Under the player's lock: see `save_queue`.
            if due && !engine.closing.load(Ordering::Relaxed) {
                engine.queue_writer.submit(player.saved());
                saved_at = Instant::now();
            }
        }
        ticks += 1;
        if ticks.is_multiple_of(100) {
            engine.record_metrics();
        }
    }
}

/// `/healthz`: healthy while the audio thread renders. Warns when the clock
/// is not synchronized or there is nothing to play.
#[derive(Clone)]
pub struct EngineHealth(pub Arc<Engine>);

impl HealthCheck for EngineHealth {
    async fn check(&self) -> Result<(), String> {
        let status = self.0.status();
        let before = status.frame.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(100)).await;
        if status.frame.load(Ordering::Relaxed) > before {
            Ok(())
        } else {
            Err("the audio thread is stuck".to_owned())
        }
    }

    async fn warnings(&self) -> Vec<String> {
        self.0.warnings()
    }

    async fn details(&self) -> serde_json::Map<String, serde_json::Value> {
        let role = if self.0.is_standby() {
            "standby"
        } else {
            "active"
        };
        serde_json::Map::from_iter([("role".to_owned(), role.into())])
    }
}
