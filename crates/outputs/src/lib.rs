// SPDX-License-Identifier: AGPL-3.0-or-later

//! Encoders and output sinks: the `OutputSink` trait, the MP3 and AAC-LC
//! encoders, the external FFmpeg encoder and the Icecast sink, and the pipeline feeding
//! several outputs at once. Shoutcast, HLS, file recording, pipes and more
//! built-in encoders arrive in M11.
//!
//! See `docs/outputs.md`.

pub mod aac;
pub mod connection;
pub mod encoder;
pub mod external;
pub mod icecast;
pub mod pipeline;
pub mod sink;

pub use connection::{ConnectionKind, HandedConnection};
pub use pipeline::{OutputSnapshot, Outputs};
pub use sink::{NowPlaying, OutputSink, OutputState};

/// A configuration the engine cannot apply.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("encoder profile {profile:?}: {reason}")]
    Profile { profile: String, reason: String },
    #[error("output {output:?}: {reason}")]
    Output { output: String, reason: String },
}
