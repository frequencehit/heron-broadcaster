// SPDX-License-Identifier: AGPL-3.0-or-later

//! Live state over a WebSocket (`/api/v1/live`): now playing, deck
//! positions, levels and queue changes, pushed to the web UI.
//!
//! Every message is a JSON [`LiveMessage`]:
//! - `state` on connection, then whenever the decks, queue, outputs or the
//!   engine's reachability change;
//! - `positions` twice a second;
//! - `levels` ten times a second (unless `?levels=false`);
//! - `track` for each track change reported by the engine;
//! - `eq` whenever the deck EQ settings change;
//! - `agc` whenever the AGC settings change;
//! - `system` every few seconds, to operators and admins only;
//! - `stations` (with `?stations=true`): the other stations the user sees,
//!   in short, for the status bar (`docs/web.md#status-bar`).

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::engine::{
    DeckName, Levels, LinkEvent, OutputConnection, PlayerState, TrackEvent, unix_ms,
};
use crate::error::{ApiResult, ErrorBody};
use crate::stations::{Station, StationInfo, Stations};
use crate::system::{SystemLoad, SystemWatch};

/// How often the other stations are looked at, and at most sent.
const STATIONS_TICK: Duration = Duration::from_secs(1);
/// How often they are sent at least, for the time left.
const STATIONS_REFRESH: Duration = Duration::from_secs(5);

/// Another station, in short: what its status bar needs.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StationSummary {
    pub id: u64,
    pub code: String,
    /// Its engine answers.
    pub reachable: bool,
    pub paused: bool,
    /// The track listeners hear; `null` with nothing on air.
    pub on_air: Option<OnAirSummary>,
    /// The names of its enabled outputs that are not connected.
    pub outputs_down: Vec<String>,
}

/// The deck on air of another station.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct OnAirSummary {
    pub deck: DeckName,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub path: String,
    /// From the cue-in, at the message's `at_unix_ms`.
    pub position_ms: u64,
    /// Where the next track starts, from the cue-in; the track's length
    /// when the engine does not say.
    pub next_start_ms: u64,
    #[serde(skip)]
    entry_id: u64,
}

impl StationSummary {
    /// A station's summary from its engine's state (`None`: unreachable).
    pub fn of(id: u64, code: String, player: Option<&PlayerState>) -> Self {
        let Some(player) = player else {
            return Self {
                id,
                code,
                reachable: false,
                paused: false,
                on_air: None,
                outputs_down: Vec::new(),
            };
        };
        let on_air = player.decks.iter().find_map(|deck| {
            let entry = deck.entry.as_ref().filter(|_| deck.on_air)?;
            Some(OnAirSummary {
                deck: deck.deck,
                artist: entry.track.artist.clone(),
                title: entry.track.title.clone(),
                path: entry.track.path.clone(),
                position_ms: deck.position_ms,
                next_start_ms: entry.next_start_ms.map_or(deck.length_ms, u64::from),
                entry_id: entry.entry_id,
            })
        });
        Self {
            id,
            code,
            reachable: true,
            paused: player.paused,
            on_air,
            outputs_down: player
                .outputs
                .iter()
                .filter(|output| output.enabled && output.state != OutputConnection::Connected)
                .map(|output| output.name.clone())
                .collect(),
        }
    }

    /// What changes it, the time aside: sent at once when it does.
    fn shape(&self) -> impl PartialEq + use<'_> {
        (
            self.id,
            self.reachable,
            self.paused,
            self.on_air
                .as_ref()
                .map(|on_air| (on_air.entry_id, on_air.deck)),
            &self.outputs_down,
        )
    }
}

/// The other stations `user` has a role on, in the stations' order, as
/// they were at connection.
async fn others(state: &AppState, user: &CurrentUser, current: u64) -> ApiResult<Vec<StationInfo>> {
    let seen: Option<Vec<u64>> = if user.role == Role::Admin {
        None
    } else {
        Some(
            sqlx::query_scalar(
                "SELECT station_id FROM user_station_roles WHERE user_id = ? AND role <> 'none'",
            )
            .bind(user.id)
            .fetch_all(&state.pool)
            .await?,
        )
    };
    Ok(crate::stations::list(&state.pool)
        .await?
        .into_iter()
        .filter(|station| station.id != current)
        .filter(|station| seen.as_ref().is_none_or(|seen| seen.contains(&station.id)))
        .collect())
}

/// The summaries of `stations` now, from what runs: a station that does
/// not run (its engine not set) is unreachable; a disabled one is left out.
fn summaries(running: &Stations, stations: &[StationInfo]) -> Vec<StationSummary> {
    stations
        .iter()
        .filter_map(|station| match running.get(station.id) {
            Some(runtime) => {
                let info = runtime.info();
                let player = runtime.engine.state().borrow().clone();
                info.enabled
                    .then(|| StationSummary::of(info.id, info.code, player.as_deref()))
            }
            None => station
                .enabled
                .then(|| StationSummary::of(station.id, station.code.clone(), None)),
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DeckPosition {
    pub deck: DeckName,
    pub position_ms: u64,
    pub length_ms: u64,
}

/// A message on the live WebSocket.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveMessage {
    State {
        /// Missing while the engine is unreachable.
        player: Option<PlayerState>,
    },
    Positions {
        at_unix_ms: u64,
        decks: Vec<DeckPosition>,
    },
    Levels {
        levels: Levels,
    },
    Track {
        event: TrackEvent,
    },
    Eq {
        settings: heron_core::EqSettings,
    },
    Agc {
        settings: heron_core::AgcSettings,
    },
    /// The machine's load. Only operators and admins receive it.
    System(SystemLoad),
    /// The other stations the user sees, in short (`?stations=true`).
    Stations {
        at_unix_ms: u64,
        stations: Vec<StationSummary>,
    },
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LiveQuery {
    /// Send levels ten times a second (default true).
    pub levels: Option<bool>,
    /// Send the other stations the user sees, in short (default false).
    pub stations: Option<bool>,
}

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(live))
}

/// Opens the live WebSocket. Messages are [`LiveMessage`]s.
#[utoipa::path(get, path = "/live", tag = "live",
    params(LiveQuery),
    responses((status = 101, description = "WebSocket of LiveMessage", body = LiveMessage),
              (status = "4XX", body = ErrorBody)))]
async fn live(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Query(query): Query<LiveQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let levels = query.levels.unwrap_or(true);
    let system = (user.role >= Role::Operator).then_some(state.system.clone());
    let others = if query.stations.unwrap_or(false) {
        match others(&state, &user, station.id()).await {
            Ok(others) => others,
            Err(error) => return error.into_response(),
        }
    } else {
        Vec::new()
    };
    upgrade.on_upgrade(move |socket| async move {
        metrics::gauge!("heron_api_live_clients").increment(1.0);
        let others = (!others.is_empty()).then_some((state.stations, others));
        if let Err(error) = serve(socket, station, levels, system, others).await {
            tracing::debug!(%error, "live client gone");
        }
        metrics::gauge!("heron_api_live_clients").decrement(1.0);
    })
}

async fn send(socket: &mut WebSocket, message: &LiveMessage) -> Result<(), axum::Error> {
    let json = serde_json::to_string(message).map_err(axum::Error::new)?;
    socket.send(Message::Text(json.into())).await
}

fn positions(player: &PlayerState) -> LiveMessage {
    LiveMessage::Positions {
        at_unix_ms: unix_ms(std::time::SystemTime::now()),
        decks: player
            .decks
            .iter()
            .map(|deck| DeckPosition {
                deck: deck.deck,
                position_ms: deck.position_ms,
                length_ms: deck.length_ms,
            })
            .collect(),
    }
}

async fn serve(
    mut socket: WebSocket,
    station: Station,
    with_levels: bool,
    mut system: Option<SystemWatch>,
    others: Option<(Arc<Stations>, Vec<StationInfo>)>,
) -> Result<(), axum::Error> {
    let mut player = station.engine.state();
    let mut events = station.engine.events();
    let mut eq_changes = station.eq_changes.subscribe();
    let mut agc_changes = station.agc_changes.subscribe();
    let mut levels = with_levels.then(|| station.engine.levels());

    let current = player.borrow_and_update().clone();
    let mut shown = current.as_ref().map(|player| player.structure());
    send(
        &mut socket,
        &LiveMessage::State {
            player: current.as_deref().cloned(),
        },
    )
    .await?;
    if let Some(system) = &mut system {
        let value = *system.borrow_and_update();
        if let Some(value) = value {
            send(&mut socket, &LiveMessage::System(value)).await?;
        }
    }
    // The other stations: looked at every second, sent when they change
    // and every few seconds for the time left.
    let mut stations_tick = others.is_some().then(|| {
        let mut tick = tokio::time::interval(STATIONS_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick
    });
    let mut stations_sent: Option<(Vec<StationSummary>, Instant)> = None;

    loop {
        tokio::select! {
            changed = player.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let current = player.borrow_and_update().clone();
                let structure = current.as_ref().map(|player| player.structure());
                if structure != shown {
                    shown = structure;
                    send(&mut socket, &LiveMessage::State { player: current.as_deref().cloned() }).await?;
                } else if let Some(current) = &current {
                    send(&mut socket, &positions(current)).await?;
                }
            }
            event = events.recv() => match event {
                Ok(LinkEvent::Track(event)) => {
                    send(&mut socket, &LiveMessage::Track { event }).await?;
                }
                Ok(LinkEvent::Connected(_)) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            eq = eq_changes.recv() => match eq {
                Ok(settings) => send(&mut socket, &LiveMessage::Eq { settings }).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            agc = agc_changes.recv() => match agc {
                Ok(settings) => send(&mut socket, &LiveMessage::Agc { settings }).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            load = async {
                match &mut system {
                    Some(system) => system.changed().await.map(|()| *system.borrow_and_update()),
                    None => std::future::pending().await,
                }
            } => match load {
                Ok(Some(value)) => send(&mut socket, &LiveMessage::System(value)).await?,
                Ok(None) => {}
                // The sampler is gone: carry on without the system part.
                Err(_) => system = None,
            },
            level = async {
                match &mut levels {
                    Some(levels) => levels.recv().await,
                    None => std::future::pending().await,
                }
            } => match level {
                Ok(levels) => send(&mut socket, &LiveMessage::Levels { levels }).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            _ = async {
                match &mut stations_tick {
                    Some(tick) => tick.tick().await,
                    None => std::future::pending().await,
                }
            } => {
                let Some((running, others)) = &others else { continue };
                let now = summaries(running, others);
                let due = stations_sent.as_ref().is_none_or(|(sent, at)| {
                    at.elapsed() >= STATIONS_REFRESH
                        || sent.len() != now.len()
                        || sent.iter().zip(&now).any(|(a, b)| a.shape() != b.shape())
                });
                if due {
                    send(&mut socket, &LiveMessage::Stations {
                        at_unix_ms: unix_ms(std::time::SystemTime::now()),
                        stations: now.clone(),
                    }).await?;
                    stations_sent = Some((now, Instant::now()));
                }
            }
            incoming = socket.recv() => match incoming {
                // Clients have nothing to say; pings are answered by axum.
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Ok(()),
                Some(Ok(_)) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{DeckState, OutputStatus, QueueEntry, QueuedTrack};
    use heron_core::PlaySource;

    fn entry(entry_id: u64, next_start_ms: Option<u32>) -> QueueEntry {
        QueueEntry {
            entry_id,
            track: QueuedTrack {
                track_id: Some(1),
                path: "a/b.mp3".to_owned(),
                title: Some("Supernature".to_owned()),
                artist: Some("Cerrone".to_owned()),
                album: None,
                duration_ms: 300_000,
                cue_in_ms: 0,
                cue_out_ms: 300_000,
                source: PlaySource::Rotation,
                no_metadata: false,
                over_intro: false,
                intro_ms: None,
                start_at_unix_ms: None,
                continues: false,
            },
            linked_to: None,
            next_start_ms,
        }
    }

    fn deck(deck: DeckName, on_air: bool, entry: Option<QueueEntry>) -> DeckState {
        DeckState {
            deck,
            entry,
            position_ms: 61_000,
            length_ms: 300_000,
            on_air,
        }
    }

    fn output(name: &str, enabled: bool, state: OutputConnection) -> OutputStatus {
        OutputStatus {
            id: 1,
            name: name.to_owned(),
            enabled,
            state,
            error: None,
            bytes_sent: 0,
            connected_since_unix_ms: None,
            dropped_chunks: 0,
        }
    }

    fn player(decks: Vec<DeckState>, outputs: Vec<OutputStatus>) -> PlayerState {
        PlayerState {
            paused: false,
            decks,
            cued: None,
            cued_jingle: None,
            pending_jingle: None,
            queue: Vec::new(),
            outputs,
            rendered_ms: 0,
            underruns: 0,
            engine_started_at_unix_ms: 0,
        }
    }

    #[test]
    fn a_station_in_short() {
        let state = player(
            vec![
                deck(DeckName::A, false, Some(entry(1, None))),
                deck(DeckName::B, true, Some(entry(2, Some(290_000)))),
            ],
            vec![
                output("MP3", true, OutputConnection::Connected),
                output("AAC", true, OutputConnection::Reconnecting),
                output("Old", false, OutputConnection::Stopped),
            ],
        );
        let summary = StationSummary::of(2, "fh90".to_owned(), Some(&state));
        assert!(summary.reachable);
        assert_eq!(summary.outputs_down, ["AAC"]);
        let on_air = summary.on_air.unwrap();
        assert_eq!(
            (
                on_air.deck,
                on_air.position_ms,
                on_air.next_start_ms,
                on_air.entry_id
            ),
            (DeckName::B, 61_000, 290_000, 2)
        );
        assert_eq!(on_air.title.as_deref(), Some("Supernature"));
    }

    #[test]
    fn the_end_of_the_track_without_a_transition_and_nothing_on_air() {
        let state = player(
            vec![deck(DeckName::A, true, Some(entry(1, None)))],
            Vec::new(),
        );
        let summary = StationSummary::of(2, "fh90".to_owned(), Some(&state));
        assert_eq!(summary.on_air.unwrap().next_start_ms, 300_000);
        let empty = player(vec![deck(DeckName::A, true, None)], Vec::new());
        assert!(
            StationSummary::of(2, "fh90".to_owned(), Some(&empty))
                .on_air
                .is_none()
        );
    }

    #[test]
    fn an_engine_unreachable() {
        let summary = StationSummary::of(2, "fh90".to_owned(), None);
        assert!(!summary.reachable && summary.on_air.is_none());
    }

    #[test]
    fn the_time_alone_is_no_change() {
        let state = |position_ms| {
            let mut state = player(
                vec![deck(DeckName::A, true, Some(entry(1, None)))],
                Vec::new(),
            );
            state.decks[0].position_ms = position_ms;
            state
        };
        let a = StationSummary::of(2, "x".to_owned(), Some(&state(1_000)));
        let b = StationSummary::of(2, "x".to_owned(), Some(&state(9_000)));
        assert!(a.shape() == b.shape());
        let mut c = b.clone();
        c.paused = true;
        assert!(a.shape() != c.shape());
    }
}
