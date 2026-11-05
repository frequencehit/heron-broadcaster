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
//! - `system` every few seconds, to operators and admins only.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::engine::{DeckName, Levels, LinkEvent, PlayerState, TrackEvent, unix_ms};
use crate::error::ErrorBody;
use crate::stations::Station;
use crate::system::{SystemLoad, SystemWatch};

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
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LiveQuery {
    /// Send levels ten times a second (default true).
    pub levels: Option<bool>,
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
    let system = (user.role >= Role::Operator).then_some(state.system);
    upgrade.on_upgrade(move |socket| async move {
        metrics::gauge!("heron_api_live_clients").increment(1.0);
        if let Err(error) = serve(socket, station, levels, system).await {
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
            incoming = socket.recv() => match incoming {
                // Clients have nothing to say; pings are answered by axum.
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Ok(()),
                Some(Ok(_)) => {}
            },
        }
    }
}
