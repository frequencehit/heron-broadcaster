// SPDX-License-Identifier: AGPL-3.0-or-later

//! The link to `heron-engine` over gRPC.
//!
//! The channel reconnects on its own. Three tasks keep the API up to date:
//! a state poller (twice a second), a levels poller (ten times a second, only
//! while someone watches the levels) and the engine's track-change stream,
//! resubscribed after each disconnection. The engine plays on whatever
//! happens here: it never waits for the API.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use heron_core::PlaySource;
use heron_proto::engine as pb;
use heron_proto::engine::engine_client::EngineClient;
use serde::Serialize;
use tokio::sync::{broadcast, watch};
use tonic::transport::{Channel, Endpoint};
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult};

/// How long a unary call may take before the engine counts as unreachable.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const STATE_INTERVAL: Duration = Duration::from_millis(500);
const LEVELS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeckName {
    A,
    B,
    Fx,
}

impl DeckName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::Fx => "fx",
        }
    }
}

/// A track as the engine holds it.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct QueuedTrack {
    /// Library track, when the track comes from the library.
    pub track_id: Option<u64>,
    /// Relative to the library root.
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: u32,
    /// Cue points the engine plays between, overrides applied.
    pub cue_in_ms: u32,
    pub cue_out_ms: u32,
    pub source: PlaySource,
    /// Sends no metadata (a jingle): the stream keeps the last track's.
    pub no_metadata: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct QueueEntry {
    /// Assigned by the engine. Never reused: a restarted engine resumes its
    /// saved queue with the same IDs and goes on from there.
    pub entry_id: u64,
    pub track: QueuedTrack,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct DeckState {
    pub deck: DeckName,
    /// Missing when the deck is empty.
    pub entry: Option<QueueEntry>,
    /// From the cue-in.
    pub position_ms: u64,
    pub length_ms: u64,
    /// The track listeners hear as "now playing".
    pub on_air: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputConnection {
    Stopped,
    Connecting,
    Connected,
    Reconnecting,
}

/// The live state of an output, as the engine reports it.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct OutputStatus {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub state: OutputConnection,
    /// Why the last connection failed, while reconnecting.
    pub error: Option<String>,
    pub bytes_sent: u64,
    pub connected_since_unix_ms: Option<u64>,
    pub dropped_chunks: u64,
}

/// Decks, queue and outputs of the engine.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PlayerState {
    pub paused: bool,
    pub decks: Vec<DeckState>,
    /// Loaded and waiting for its transition.
    pub cued: Option<QueueEntry>,
    /// After the cued track.
    pub queue: Vec<QueueEntry>,
    pub outputs: Vec<OutputStatus>,
    /// Audio rendered since the engine started.
    pub rendered_ms: u64,
    pub underruns: u64,
    pub engine_started_at_unix_ms: u64,
}

impl PlayerState {
    /// The tracks waiting to play: the cued one, then the queue.
    pub fn waiting(&self) -> impl Iterator<Item = &QueueEntry> {
        self.cued.iter().chain(&self.queue)
    }

    /// The same state without what changes all the time (positions,
    /// counters), to tell when something worth showing changed.
    pub fn structure(&self) -> Self {
        let mut state = self.clone();
        for deck in &mut state.decks {
            deck.position_ms = 0;
        }
        for output in &mut state.outputs {
            output.bytes_sent = 0;
            output.dropped_chunks = 0;
        }
        state.rendered_ms = 0;
        state.underruns = 0;
        state
    }
}

/// dBFS, −120 for silence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, ToSchema)]
pub struct ChannelLevels {
    pub peak_left: f32,
    pub peak_right: f32,
    pub rms_left: f32,
    pub rms_right: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, ToSchema)]
pub struct Levels {
    pub master: ChannelLevels,
    pub deck_a: ChannelLevels,
    pub deck_b: ChannelLevels,
    pub fx: ChannelLevels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// Played to its end (or its transition).
    Finished,
    Skipped,
    /// The FX player was stopped.
    Stopped,
    /// Another FX started.
    Replaced,
}

impl EndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Finished => "finished",
            Self::Skipped => "skipped",
            Self::Stopped => "stopped",
            Self::Replaced => "replaced",
        }
    }
}

/// A track change reported by the engine.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrackEvent {
    Started {
        deck: DeckName,
        entry: QueueEntry,
        at_unix_ms: u64,
    },
    Ended {
        deck: DeckName,
        entry: QueueEntry,
        at_unix_ms: u64,
        played_ms: u64,
        reason: EndReason,
    },
}

/// What the event stream tells the rest of the API.
#[derive(Clone, Debug)]
pub enum LinkEvent {
    /// The stream (re)connected; the state was read right after subscribing,
    /// so events that follow may already be reflected in it.
    Connected(Arc<PlayerState>),
    Track(TrackEvent),
}

pub struct EngineLink {
    client: EngineClient<Channel>,
    state: watch::Sender<Option<Arc<PlayerState>>>,
    events: broadcast::Sender<LinkEvent>,
    levels: broadcast::Sender<Levels>,
}

impl EngineLink {
    /// Prepares the channel; it connects on first use, and again after failures.
    pub fn new(url: &str) -> Result<Arc<Self>, tonic::transport::Error> {
        let channel = Endpoint::from_shared(url.to_owned())?
            .connect_timeout(Duration::from_secs(2))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .http2_keep_alive_interval(Duration::from_secs(15))
            .keep_alive_while_idle(true)
            .connect_lazy();
        Ok(Arc::new(Self {
            client: EngineClient::new(channel),
            state: watch::channel(None).0,
            events: broadcast::channel(256).0,
            levels: broadcast::channel(4).0,
        }))
    }

    /// Starts the pollers and the event stream.
    pub fn spawn(self: &Arc<Self>) -> Vec<tokio::task::JoinHandle<()>> {
        vec![
            tokio::spawn(Arc::clone(self).poll_state()),
            tokio::spawn(Arc::clone(self).poll_levels()),
            tokio::spawn(Arc::clone(self).watch_events()),
        ]
    }

    /// The last state read; `None` while the engine is unreachable.
    pub fn state(&self) -> watch::Receiver<Option<Arc<PlayerState>>> {
        self.state.subscribe()
    }

    pub fn is_connected(&self) -> bool {
        self.state.borrow().is_some()
    }

    pub fn events(&self) -> broadcast::Receiver<LinkEvent> {
        self.events.subscribe()
    }

    /// Levels ten times a second, while there are subscribers.
    pub fn levels(&self) -> broadcast::Receiver<Levels> {
        self.levels.subscribe()
    }

    async fn poll_state(self: Arc<Self>) {
        let mut interval = tokio::time::interval(STATE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match self.fetch_state().await {
                Ok(state) => {
                    if self.state.borrow().is_none() {
                        tracing::info!("engine reachable");
                    }
                    self.state.send_replace(Some(Arc::new(state)));
                }
                Err(error) => {
                    if self.state.borrow().is_some() {
                        tracing::warn!(%error, "engine unreachable");
                        metrics::counter!("heron_api_engine_disconnections_total").increment(1);
                    }
                    self.state.send_replace(None);
                }
            }
            metrics::gauge!("heron_api_engine_connected").set(if self.is_connected() {
                1.0
            } else {
                0.0
            });
        }
    }

    async fn poll_levels(self: Arc<Self>) {
        let mut interval = tokio::time::interval(LEVELS_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if self.levels.receiver_count() == 0 || !self.is_connected() {
                continue;
            }
            if let Ok(levels) = self.fetch_levels().await {
                let _ = self.levels.send(levels);
            }
        }
    }

    async fn watch_events(self: Arc<Self>) {
        let mut delay = Duration::from_millis(500);
        loop {
            match self.stream_events().await {
                Ok(()) => tracing::warn!("engine event stream ended"),
                Err(error) => tracing::debug!(%error, "engine event stream unavailable"),
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(5));
            if self.is_connected() {
                delay = Duration::from_millis(500);
            }
        }
    }

    async fn stream_events(&self) -> Result<(), tonic::Status> {
        let mut stream = self
            .client
            .clone()
            .watch_events(pb::WatchEventsRequest {})
            .await?
            .into_inner();
        // Read after subscribing, so no change falls between the two.
        let state = self.fetch_state().await.map_err(|error| match error {
            ApiError::EngineUnavailable => tonic::Status::unavailable("engine unreachable"),
            other => tonic::Status::internal(other.to_string()),
        })?;
        tracing::info!("following the engine's track changes");
        let _ = self.events.send(LinkEvent::Connected(Arc::new(state)));
        while let Some(event) = stream.message().await? {
            if let Some(event) = track_event(event) {
                let _ = self.events.send(LinkEvent::Track(event));
            }
        }
        Ok(())
    }

    async fn call<T>(
        &self,
        call: impl Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    ) -> ApiResult<T> {
        match tokio::time::timeout(CALL_TIMEOUT, call).await {
            Ok(Ok(response)) => Ok(response.into_inner()),
            Ok(Err(status)) => Err(status.into()),
            Err(_) => Err(ApiError::EngineUnavailable),
        }
    }

    pub async fn fetch_state(&self) -> ApiResult<PlayerState> {
        let mut client = self.client.clone();
        let state = self.call(client.get_state(pb::GetStateRequest {})).await?;
        Ok(player_state(state))
    }

    pub async fn fetch_levels(&self) -> ApiResult<Levels> {
        let mut client = self.client.clone();
        let levels = self
            .call(client.get_levels(pb::GetLevelsRequest {}))
            .await?;
        let channel = |levels: Option<pb::ChannelLevels>| {
            let levels = levels.unwrap_or_default();
            ChannelLevels {
                peak_left: levels.peak_left,
                peak_right: levels.peak_right,
                rms_left: levels.rms_left,
                rms_right: levels.rms_right,
            }
        };
        Ok(Levels {
            master: channel(levels.master),
            deck_a: channel(levels.deck_a),
            deck_b: channel(levels.deck_b),
            fx: channel(levels.fx),
        })
    }

    pub async fn enqueue(
        &self,
        tracks: Vec<pb::Track>,
        position: Option<u32>,
    ) -> ApiResult<Vec<u64>> {
        let mut client = self.client.clone();
        let response = self
            .call(client.enqueue(pb::EnqueueRequest { tracks, position }))
            .await?;
        self.refresh().await;
        Ok(response.entry_ids)
    }

    pub async fn remove(&self, entry_id: u64) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.remove_from_queue(pb::RemoveFromQueueRequest { entry_id }))
            .await?;
        self.refresh().await;
        Ok(())
    }

    pub async fn clear(&self) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.clear_queue(pb::ClearQueueRequest {}))
            .await?;
        self.refresh().await;
        Ok(())
    }

    pub async fn move_entry(&self, entry_id: u64, position: u32) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.move_queue_entry(pb::MoveQueueEntryRequest { entry_id, position }))
            .await?;
        self.refresh().await;
        Ok(())
    }

    pub async fn replace(&self, entry_id: u64, track: pb::Track) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.replace_queue_entry(pb::ReplaceQueueEntryRequest {
            entry_id,
            track: Some(track),
        }))
        .await?;
        self.refresh().await;
        Ok(())
    }

    pub async fn skip(&self) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.skip(pb::SkipRequest {})).await?;
        Ok(())
    }

    pub async fn pause(&self) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.pause(pb::PauseRequest {})).await?;
        self.refresh().await;
        Ok(())
    }

    pub async fn resume(&self) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.resume(pb::ResumeRequest {})).await?;
        self.refresh().await;
        Ok(())
    }

    pub async fn play_fx(&self, track: pb::Track) -> ApiResult<u64> {
        let mut client = self.client.clone();
        let response = self
            .call(client.play_fx(pb::PlayFxRequest { track: Some(track) }))
            .await?;
        Ok(response.entry_id)
    }

    pub async fn stop_fx(&self) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.stop_fx(pb::StopFxRequest {})).await?;
        Ok(())
    }

    pub async fn config(&self) -> ApiResult<pb::EngineConfig> {
        let mut client = self.client.clone();
        self.call(client.get_config(pb::GetConfigRequest {})).await
    }

    pub async fn set_playout_settings(&self, settings_json: String) -> ApiResult<()> {
        let mut client = self.client.clone();
        self.call(client.set_playout_settings(pb::SetPlayoutSettingsRequest { settings_json }))
            .await?;
        Ok(())
    }

    pub async fn set_outputs(&self, outputs_json: String) -> ApiResult<()> {
        let mut client = self.client.clone();
        // Stopping encoders joins their threads: give it time.
        let call = client.set_outputs(pb::SetOutputsRequest { outputs_json });
        match tokio::time::timeout(CALL_TIMEOUT * 4, call).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(status)) => Err(status.into()),
            Err(_) => Err(ApiError::EngineUnavailable),
        }
    }

    /// Reads the state again after a change, so watchers see it at once.
    async fn refresh(&self) {
        if let Ok(state) = self.fetch_state().await {
            self.state.send_replace(Some(Arc::new(state)));
        }
    }
}

pub fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

pub fn source_to_proto(source: PlaySource) -> pb::PlaySource {
    match source {
        PlaySource::Rotation => pb::PlaySource::Rotation,
        PlaySource::Request => pb::PlaySource::Request,
        PlaySource::Fallback => pb::PlaySource::Fallback,
        PlaySource::Manual => pb::PlaySource::Manual,
    }
}

fn source_from_proto(source: pb::PlaySource) -> PlaySource {
    match source {
        pb::PlaySource::Rotation => PlaySource::Rotation,
        pb::PlaySource::Request => PlaySource::Request,
        pb::PlaySource::Fallback => PlaySource::Fallback,
        pb::PlaySource::Manual | pb::PlaySource::Unspecified => PlaySource::Manual,
    }
}

fn deck(deck: pb::Deck) -> DeckName {
    match deck {
        pb::Deck::B => DeckName::B,
        pb::Deck::Fx => DeckName::Fx,
        pb::Deck::A | pb::Deck::Unspecified => DeckName::A,
    }
}

fn entry(entry: pb::QueueEntry) -> QueueEntry {
    let track = entry.track.unwrap_or_default();
    QueueEntry {
        entry_id: entry.entry_id,
        track: QueuedTrack {
            source: source_from_proto(track.source()),
            track_id: track.track_id,
            path: track.path,
            title: track.title,
            artist: track.artist,
            album: track.album,
            duration_ms: track.duration_ms,
            cue_in_ms: track.cue_in_ms,
            cue_out_ms: track.cue_out_ms,
            no_metadata: track.no_metadata,
        },
    }
}

pub fn player_state(state: pb::EngineState) -> PlayerState {
    PlayerState {
        paused: state.paused,
        decks: state
            .decks
            .into_iter()
            .map(|deck_state| DeckState {
                deck: deck(deck_state.deck()),
                entry: deck_state.entry.map(entry),
                position_ms: deck_state.position_ms,
                length_ms: deck_state.length_ms,
                on_air: deck_state.on_air,
            })
            .collect(),
        cued: state.cued.map(entry),
        queue: state.queue.into_iter().map(entry).collect(),
        outputs: state
            .outputs
            .into_iter()
            .map(|output| OutputStatus {
                id: output.id,
                name: output.name,
                enabled: output.enabled,
                state: match output.state.as_str() {
                    "connecting" => OutputConnection::Connecting,
                    "connected" => OutputConnection::Connected,
                    "reconnecting" => OutputConnection::Reconnecting,
                    _ => OutputConnection::Stopped,
                },
                error: output.error,
                bytes_sent: output.bytes_sent,
                connected_since_unix_ms: output.connected_since_unix.map(|s| s * 1_000),
                dropped_chunks: output.dropped_chunks,
            })
            .collect(),
        rendered_ms: state.rendered_ms,
        underruns: state.underruns,
        engine_started_at_unix_ms: state.started_unix_ms,
    }
}

fn track_event(event: pb::EngineEvent) -> Option<TrackEvent> {
    let at_unix_ms = event.timestamp_unix_ms;
    match event.event? {
        pb::engine_event::Event::TrackStarted(started) => Some(TrackEvent::Started {
            deck: deck(started.deck()),
            entry: entry(started.entry?),
            at_unix_ms,
        }),
        pb::engine_event::Event::TrackEnded(ended) => Some(TrackEvent::Ended {
            deck: deck(ended.deck()),
            reason: match ended.reason() {
                pb::EndReason::Skipped => EndReason::Skipped,
                pb::EndReason::Stopped => EndReason::Stopped,
                pb::EndReason::Replaced => EndReason::Replaced,
                pb::EndReason::Finished | pb::EndReason::Unspecified => EndReason::Finished,
            },
            played_ms: ended.played_ms,
            entry: entry(ended.entry?),
            at_unix_ms,
        }),
    }
}
