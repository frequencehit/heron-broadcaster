// SPDX-License-Identifier: AGPL-3.0-or-later

//! The master AGC chain: gated AGC, stereo expander, bass EQ, 5-band and
//! dual-band processors, and a final limiter or clipper, in that order.
//!
//! Every block works in place on interleaved stereo `f32`. Their state is
//! allocated in `new`: `process` never allocates, so it runs on the audio
//! thread. Settings glide to their new values in about 50 ms, and switching a
//! block or the whole chain crossfades ([`Chain`]).
//!
//! See `docs/audio-engine.md#master-agc-chain`.

mod bass;
mod chain;
mod crossover;
mod dynamics;
mod final_stage;
mod gate;
mod multiband;
mod stereo;

#[cfg(test)]
mod tests;

pub use bass::{BassEq, BassEqParams, BassShape};
pub use chain::{Chain, ChainMeters, ChainParams};
pub use dynamics::{BandMode, BandParams};
pub use final_stage::{FinalMode, FinalStage, FinalStageParams};
pub use gate::{GatedAgc, GatedAgcParams};
pub use multiband::{DualBandParams, FiveBandParams, Multiband};
pub use stereo::{StereoExpander, StereoExpanderParams};

/// Interleaved stereo.
const CHANNELS: usize = 2;

/// Time constant of the glide of settings: there in about 50 ms.
const GLIDE_SECONDS: f64 = 0.015;

/// Frames between two updates of the settings that are costly to apply
/// (filter coefficients, logarithms).
const CONTROL_FRAMES: usize = 16;

/// Filter states under this are flushed to 0: denormal numbers are slow.
const DENORMAL_FLOOR: f64 = 1e-150;

pub(crate) fn db_to_gain(db: f32) -> f32 {
    (db * (std::f32::consts::LN_10 / 20.0)).exp()
}

/// The level of a linear gain or peak, in dB; −180 dB for silence.
pub(crate) fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-9).log10()
}

/// The share of the way to its target a one-pole smoother of time constant
/// `seconds` covers in `frames` frames.
pub(crate) fn share(seconds: f64, frames: usize, sample_rate: f64) -> f32 {
    if seconds <= 0.0 {
        return 1.0;
    }
    (1.0 - (-(frames as f64) / (seconds * sample_rate)).exp()) as f32
}

pub(crate) fn flush_denormals(values: &mut [f64]) {
    for value in values {
        if value.abs() < DENORMAL_FLOOR {
            *value = 0.0;
        }
    }
}

/// A setting gliding towards its target.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Smoothed {
    pub value: f32,
    pub target: f32,
}

impl Smoothed {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
        }
    }

    /// Moves `share` of the way to the target, and lands on it when close.
    pub fn step(&mut self, share: f32) -> f32 {
        if self.value != self.target {
            self.value += (self.target - self.value) * share;
            if (self.target - self.value).abs() <= 1e-5 * self.target.abs().max(1.0) {
                self.value = self.target;
            }
        }
        self.value
    }

    pub fn is_settled(&self) -> bool {
        self.value == self.target
    }

    pub fn settle(&mut self) {
        self.value = self.target;
    }
}

/// A block of the chain.
pub(crate) trait Block {
    /// Processes interleaved stereo in place.
    fn process(&mut self, samples: &mut [f32]);
    /// Settings at their targets at once, the sound in the filters kept.
    fn settle(&mut self);
    /// Back to rest: no sound left in the filters, settings at their targets.
    fn reset(&mut self);
}
