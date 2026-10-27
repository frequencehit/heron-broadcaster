// SPDX-License-Identifier: AGPL-3.0-or-later

//! The gRPC control interface (`heron.engine.v1.Engine`), used by heron-api.

use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use heron_core::playout::TrackPlayout;
use heron_core::{OutputsConfig, PlaySource, PlayoutSettings};
use heron_outputs::OutputState;
use heron_proto::engine as pb;
use heron_proto::engine::engine_server::{Engine as EngineRpc, EngineServer};
use tokio_stream::{Stream, StreamExt, wrappers::BroadcastStream};
use tonic::{Request, Response, Status};

use crate::engine::{Engine, EngineError};
use crate::mixer::{Deck, EndReason, MeterStatus};
use crate::player::{PlayerError, PlayerEvent, QueueEntry, TrackRequest};

pub struct EngineService {
    engine: Arc<Engine>,
}

impl EngineService {
    pub fn server(engine: Arc<Engine>) -> EngineServer<Self> {
        EngineServer::new(Self { engine })
    }
}

type Reply<T> = Result<Response<T>, Status>;

#[tonic::async_trait]
impl EngineRpc for EngineService {
    async fn enqueue(&self, request: Request<pb::EnqueueRequest>) -> Reply<pb::EnqueueResponse> {
        let request = request.into_inner();
        let tracks = request
            .tracks
            .into_iter()
            .map(track_from_proto)
            .collect::<Result<Vec<_>, _>>()?;
        let position = request.position.map(|position| position as usize);
        let entry_ids = self
            .engine
            .player()
            .enqueue(tracks, position)
            .map_err(player_status)?;
        Ok(Response::new(pb::EnqueueResponse { entry_ids }))
    }

    async fn remove_from_queue(
        &self,
        request: Request<pb::RemoveFromQueueRequest>,
    ) -> Reply<pb::RemoveFromQueueResponse> {
        self.engine
            .player()
            .remove(request.into_inner().entry_id)
            .map_err(player_status)?;
        Ok(Response::new(pb::RemoveFromQueueResponse {}))
    }

    async fn clear_queue(
        &self,
        _: Request<pb::ClearQueueRequest>,
    ) -> Reply<pb::ClearQueueResponse> {
        self.engine.player().clear().map_err(player_status)?;
        Ok(Response::new(pb::ClearQueueResponse {}))
    }

    async fn move_queue_entry(
        &self,
        request: Request<pb::MoveQueueEntryRequest>,
    ) -> Reply<pb::MoveQueueEntryResponse> {
        let request = request.into_inner();
        self.engine
            .player()
            .move_entry(request.entry_id, request.position as usize)
            .map_err(player_status)?;
        Ok(Response::new(pb::MoveQueueEntryResponse {}))
    }

    async fn replace_queue_entry(
        &self,
        request: Request<pb::ReplaceQueueEntryRequest>,
    ) -> Reply<pb::ReplaceQueueEntryResponse> {
        let request = request.into_inner();
        let track = request
            .track
            .ok_or_else(|| Status::invalid_argument("missing track"))?;
        self.engine
            .player()
            .replace(request.entry_id, track_from_proto(track)?)
            .map_err(player_status)?;
        Ok(Response::new(pb::ReplaceQueueEntryResponse {}))
    }

    async fn skip(&self, _: Request<pb::SkipRequest>) -> Reply<pb::SkipResponse> {
        self.engine.player().skip().map_err(player_status)?;
        Ok(Response::new(pb::SkipResponse {}))
    }

    async fn pause(&self, _: Request<pb::PauseRequest>) -> Reply<pb::PauseResponse> {
        self.engine.player().pause().map_err(player_status)?;
        Ok(Response::new(pb::PauseResponse {}))
    }

    async fn resume(&self, _: Request<pb::ResumeRequest>) -> Reply<pb::ResumeResponse> {
        self.engine.player().resume().map_err(player_status)?;
        Ok(Response::new(pb::ResumeResponse {}))
    }

    async fn play_fx(&self, request: Request<pb::PlayFxRequest>) -> Reply<pb::PlayFxResponse> {
        let track = request
            .into_inner()
            .track
            .ok_or_else(|| Status::invalid_argument("missing track"))?;
        let entry_id = self
            .engine
            .player()
            .play_fx(track_from_proto(track)?)
            .map_err(player_status)?;
        Ok(Response::new(pb::PlayFxResponse { entry_id }))
    }

    async fn stop_fx(&self, _: Request<pb::StopFxRequest>) -> Reply<pb::StopFxResponse> {
        self.engine.player().stop_fx().map_err(player_status)?;
        Ok(Response::new(pb::StopFxResponse {}))
    }

    async fn get_state(&self, _: Request<pb::GetStateRequest>) -> Reply<pb::EngineState> {
        let snapshot = self.engine.player().snapshot();
        let outputs = self
            .engine
            .outputs()
            .snapshot()
            .into_iter()
            .map(|output| {
                let (state, error, connected_since) = match output.state {
                    None => ("stopped", None, None),
                    Some(OutputState::Connecting) => ("connecting", None, None),
                    Some(OutputState::Connected { since }) => (
                        "connected",
                        None,
                        Some(heron_outputs::pipeline::unix_seconds(since)),
                    ),
                    Some(OutputState::Reconnecting { error, .. }) => {
                        ("reconnecting", Some(error), None)
                    }
                };
                pb::OutputState {
                    id: output.id,
                    name: output.name,
                    enabled: output.enabled,
                    state: state.to_owned(),
                    error,
                    bytes_sent: output.bytes_sent,
                    connected_since_unix: connected_since,
                    dropped_chunks: output.dropped_chunks,
                }
            })
            .collect();
        Ok(Response::new(pb::EngineState {
            paused: snapshot.paused,
            decks: snapshot
                .decks
                .into_iter()
                .map(|deck| pb::DeckState {
                    deck: deck_to_proto(deck.deck).into(),
                    entry: deck.entry.map(entry_to_proto),
                    position_ms: deck.position_ms,
                    length_ms: deck.length_ms,
                    on_air: deck.on_air,
                })
                .collect(),
            cued: snapshot.cued.map(entry_to_proto),
            queue: snapshot.queue.into_iter().map(entry_to_proto).collect(),
            outputs,
            rendered_ms: snapshot.rendered_ms,
            underruns: snapshot.underruns,
            started_unix_ms: unix_ms(self.engine.started_at()),
        }))
    }

    async fn get_levels(&self, _: Request<pb::GetLevelsRequest>) -> Reply<pb::Levels> {
        let status = self.engine.status();
        Ok(Response::new(pb::Levels {
            master: Some(levels(&status.master)),
            deck_a: Some(levels(&status.decks[Deck::A.slot()].meter)),
            deck_b: Some(levels(&status.decks[Deck::B.slot()].meter)),
            fx: Some(levels(&status.decks[Deck::Fx.slot()].meter)),
        }))
    }

    type WatchEventsStream = Pin<Box<dyn Stream<Item = Result<pb::EngineEvent, Status>> + Send>>;

    async fn watch_events(
        &self,
        _: Request<pb::WatchEventsRequest>,
    ) -> Reply<Self::WatchEventsStream> {
        let stream = BroadcastStream::new(self.engine.subscribe())
            // A client too slow to keep up misses events rather than stalling the engine.
            .filter_map(|event| event.ok())
            .map(|event| Ok(event_to_proto(event)));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_config(&self, _: Request<pb::GetConfigRequest>) -> Reply<pb::EngineConfig> {
        let config = self.engine.config();
        Ok(Response::new(pb::EngineConfig {
            playout_settings_json: to_json(&config.playout)?,
            outputs_json: to_json(&config.outputs)?,
        }))
    }

    async fn set_playout_settings(
        &self,
        request: Request<pb::SetPlayoutSettingsRequest>,
    ) -> Reply<pb::SetPlayoutSettingsResponse> {
        let settings: PlayoutSettings =
            serde_json::from_str(&request.into_inner().settings_json)
                .map_err(|error| Status::invalid_argument(format!("invalid settings: {error}")))?;
        self.engine
            .set_playout_settings(settings)
            .map_err(engine_status)?;
        Ok(Response::new(pb::SetPlayoutSettingsResponse {}))
    }

    async fn set_outputs(
        &self,
        request: Request<pb::SetOutputsRequest>,
    ) -> Reply<pb::SetOutputsResponse> {
        let config: OutputsConfig = serde_json::from_str(&request.into_inner().outputs_json)
            .map_err(|error| Status::invalid_argument(format!("invalid outputs: {error}")))?;
        let engine = Arc::clone(&self.engine);
        // Stopping encoders joins their threads.
        tokio::task::spawn_blocking(move || engine.set_outputs(&config))
            .await
            .map_err(|error| Status::internal(error.to_string()))?
            .map_err(engine_status)?;
        Ok(Response::new(pb::SetOutputsResponse {}))
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, Status> {
    serde_json::to_string(value).map_err(|error| Status::internal(error.to_string()))
}

fn player_status(error: PlayerError) -> Status {
    match error {
        PlayerError::InvalidPath(_) => Status::invalid_argument(error.to_string()),
        PlayerError::NotFound(_) => Status::not_found(error.to_string()),
        PlayerError::OnAir(_) => Status::failed_precondition(error.to_string()),
        PlayerError::Busy => Status::unavailable(error.to_string()),
        PlayerError::Spawn(_) => Status::internal(error.to_string()),
    }
}

fn engine_status(error: EngineError) -> Status {
    match error {
        EngineError::Config(error) => Status::invalid_argument(error.to_string()),
        error => Status::internal(error.to_string()),
    }
}

/// Reads a track sent by the API. Cue points of 0 and 0 mean "not analyzed":
/// the whole file plays.
pub fn track_from_proto(track: pb::Track) -> Result<TrackRequest, Status> {
    let crossfade = if track.crossfade_overrides_json.trim().is_empty() {
        Default::default()
    } else {
        serde_json::from_str(&track.crossfade_overrides_json).map_err(|error| {
            Status::invalid_argument(format!("invalid crossfade overrides: {error}"))
        })?
    };
    let source = source_from_proto(track.source());
    let mut playout = TrackPlayout {
        duration_ms: track.duration_ms,
        cue_in_ms: track.cue_in_ms,
        cue_out_ms: track.cue_out_ms,
        mix_point_ms: track.mix_point_ms,
        integrated_lufs: track.integrated_lufs,
        true_peak_db: track.true_peak_db,
        crossfade,
    };
    if playout.cue_in_ms == 0 && playout.cue_out_ms == 0 {
        playout.cue_out_ms = if track.duration_ms > 0 {
            track.duration_ms
        } else {
            u32::MAX
        };
    }
    if playout.cue_in_ms > playout.cue_out_ms {
        return Err(Status::invalid_argument(format!(
            "{}: cue-in {} ms is after cue-out {} ms",
            track.path, playout.cue_in_ms, playout.cue_out_ms
        )));
    }
    Ok(TrackRequest {
        track_id: track.track_id,
        path: track.path,
        title: track.title,
        artist: track.artist,
        album: track.album,
        playout,
        source,
        no_metadata: track.no_metadata,
    })
}

fn track_to_proto(track: TrackRequest) -> pb::Track {
    let playout = track.playout;
    pb::Track {
        track_id: track.track_id,
        path: track.path,
        title: track.title,
        artist: track.artist,
        album: track.album,
        duration_ms: playout.duration_ms,
        cue_in_ms: playout.cue_in_ms,
        cue_out_ms: playout.cue_out_ms,
        mix_point_ms: playout.mix_point_ms,
        integrated_lufs: playout.integrated_lufs,
        true_peak_db: playout.true_peak_db,
        crossfade_overrides_json: if playout.crossfade == Default::default() {
            String::new()
        } else {
            serde_json::to_string(&playout.crossfade).unwrap_or_default()
        },
        source: source_to_proto(track.source).into(),
        no_metadata: track.no_metadata,
    }
}

/// Tracks from older clients count as queued by hand.
fn source_from_proto(source: pb::PlaySource) -> PlaySource {
    match source {
        pb::PlaySource::Rotation => PlaySource::Rotation,
        pb::PlaySource::Request => PlaySource::Request,
        pb::PlaySource::Fallback => PlaySource::Fallback,
        pb::PlaySource::Manual | pb::PlaySource::Unspecified => PlaySource::Manual,
    }
}

fn source_to_proto(source: PlaySource) -> pb::PlaySource {
    match source {
        PlaySource::Rotation => pb::PlaySource::Rotation,
        PlaySource::Request => pb::PlaySource::Request,
        PlaySource::Fallback => pb::PlaySource::Fallback,
        PlaySource::Manual => pb::PlaySource::Manual,
    }
}

fn entry_to_proto(entry: QueueEntry) -> pb::QueueEntry {
    pb::QueueEntry {
        entry_id: entry.entry_id,
        track: Some(track_to_proto(entry.track)),
    }
}

fn deck_to_proto(deck: Deck) -> pb::Deck {
    match deck {
        Deck::A => pb::Deck::A,
        Deck::B => pb::Deck::B,
        Deck::Fx => pb::Deck::Fx,
    }
}

fn reason_to_proto(reason: EndReason) -> pb::EndReason {
    match reason {
        EndReason::Finished => pb::EndReason::Finished,
        EndReason::Skipped => pb::EndReason::Skipped,
        EndReason::Stopped => pb::EndReason::Stopped,
        EndReason::Replaced => pb::EndReason::Replaced,
    }
}

fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

fn event_to_proto(event: PlayerEvent) -> pb::EngineEvent {
    match event {
        PlayerEvent::Started { deck, entry, at } => pb::EngineEvent {
            timestamp_unix_ms: unix_ms(at),
            event: Some(pb::engine_event::Event::TrackStarted(pb::TrackStarted {
                deck: deck_to_proto(deck).into(),
                entry: Some(entry_to_proto(entry)),
            })),
        },
        PlayerEvent::Ended {
            deck,
            entry,
            played_ms,
            reason,
            at,
        } => pb::EngineEvent {
            timestamp_unix_ms: unix_ms(at),
            event: Some(pb::engine_event::Event::TrackEnded(pb::TrackEnded {
                deck: deck_to_proto(deck).into(),
                entry: Some(entry_to_proto(entry)),
                played_ms,
                reason: reason_to_proto(reason).into(),
            })),
        },
    }
}

fn to_dbfs(linear: f32) -> f32 {
    if linear > 1e-6 {
        20.0 * linear.log10()
    } else {
        -120.0
    }
}

fn levels(meter: &MeterStatus) -> pb::ChannelLevels {
    let (peak, rms) = meter.read();
    pb::ChannelLevels {
        peak_left: to_dbfs(peak[0]),
        peak_right: to_dbfs(peak[1]),
        rms_left: to_dbfs(rms[0]),
        rms_right: to_dbfs(rms[1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unanalyzed_tracks_play_whole() {
        let track = track_from_proto(pb::Track {
            path: "a.mp3".into(),
            duration_ms: 1_000,
            ..pb::Track::default()
        })
        .unwrap();
        assert_eq!(track.playout.cue_out_ms, 1_000);
        let unknown = track_from_proto(pb::Track {
            path: "a.mp3".into(),
            ..pb::Track::default()
        })
        .unwrap();
        assert_eq!(unknown.playout.cue_out_ms, u32::MAX);
    }

    #[test]
    fn rejects_inconsistent_tracks() {
        let inverted = pb::Track {
            path: "a.mp3".into(),
            cue_in_ms: 5_000,
            cue_out_ms: 1_000,
            ..pb::Track::default()
        };
        assert!(track_from_proto(inverted).is_err());
        let bad_json = pb::Track {
            path: "a.mp3".into(),
            crossfade_overrides_json: "{\"nope\": 1}".into(),
            ..pb::Track::default()
        };
        assert!(track_from_proto(bad_json).is_err());
    }

    #[test]
    fn tracks_round_trip() {
        let original = pb::Track {
            track_id: Some(42),
            path: "Jazz/So What.flac".into(),
            title: Some("So What".into()),
            artist: Some("Miles Davis".into()),
            album: None,
            duration_ms: 545_000,
            cue_in_ms: 120,
            cue_out_ms: 544_000,
            mix_point_ms: Some(542_000),
            integrated_lufs: Some(-18.5),
            true_peak_db: Some(-1.2),
            crossfade_overrides_json: r#"{"fade_out":null,"fade_in":null,"mode":"fixed","fixed_ms":3000,"crossfade":null}"#.into(),
            source: pb::PlaySource::Rotation.into(),
            no_metadata: false,
        };
        let back = track_to_proto(track_from_proto(original.clone()).unwrap());
        assert_eq!(back, original);
    }
}
