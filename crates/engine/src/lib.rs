// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-engine`: real-time playout. Decks, mixer, crossfades, pacing clock,
//! encoders and outputs. See `docs/audio-engine.md`.
//!
//! The engine never connects to the database and must keep playing when
//! every other service is down.

pub mod agc;
pub mod clock;
pub mod config;
pub mod decoder;
pub mod engine;
pub mod fallback;
pub mod grpc;
pub mod handover;
pub mod mixer;
#[cfg(feature = "monitor")]
pub mod monitor;
pub mod player;
pub mod preload;
pub mod queue_file;
