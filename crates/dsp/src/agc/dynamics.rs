// SPDX-License-Identifier: AGPL-3.0-or-later

//! The dynamics of one band: a compressor, an expander or a limiter.
//!
//! A gain computer with a soft knee turns the level into a gain in dB (0 or
//! less); the gain follows it with the attack as it falls and the release as
//! it rises, after the hold (the smooth branching detector of Giannoulis,
//! Massberg and Reiss, *Digital Dynamic Range Compressor Design*, 2012).
//! Threshold and ratio glide; after a change of mode, the gain moves to the
//! new one with the attack or the release, as it does when the level changes.

use super::{GLIDE_SECONDS, Smoothed, gain_to_db, share};

/// Width of the soft knee, in dB. The limiter has none.
const KNEE_DB: f32 = 6.0;
/// The expander lowers by this much at most.
const MAX_EXPANSION_DB: f32 = 40.0;

/// What a band does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BandMode {
    /// Over the threshold, rises by 1 dB for each `ratio` dB.
    #[default]
    Compressor,
    /// Under the threshold, falls by `ratio` dB for each dB.
    Expander,
    /// Never rises over the threshold.
    Limiter,
}

/// Settings of one band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandParams {
    pub mode: BandMode,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub hold_ms: f32,
    /// After the dynamics.
    pub gain_db: f32,
}

impl Default for BandParams {
    fn default() -> Self {
        Self {
            mode: BandMode::Compressor,
            threshold_db: -20.0,
            ratio: 3.0,
            attack_ms: 20.0,
            release_ms: 250.0,
            hold_ms: 0.0,
            gain_db: 0.0,
        }
    }
}

/// How far `over` dB over the threshold counts, through a knee `width` wide.
fn knee(over: f32, width: f32) -> f32 {
    if over <= -width / 2.0 {
        0.0
    } else if over >= width / 2.0 || width <= 0.0 {
        over.max(0.0)
    } else {
        (over + width / 2.0).powi(2) / (2.0 * width)
    }
}

/// The gain (dB, 0 or less) of `mode` for a level `over` dB over the threshold.
pub(crate) fn static_gain_db(mode: BandMode, ratio: f32, over: f32) -> f32 {
    match mode {
        BandMode::Compressor => -(1.0 - 1.0 / ratio) * knee(over, KNEE_DB),
        BandMode::Limiter => -knee(over, 0.0),
        BandMode::Expander => (-(ratio - 1.0) * knee(-over, KNEE_DB)).max(-MAX_EXPANSION_DB),
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Dynamics {
    sample_rate: f64,
    mode: BandMode,
    threshold_db: Smoothed,
    ratio: Smoothed,
    attack_share: f32,
    release_share: f32,
    hold_frames: u32,
    hold_left: u32,
    glide_share: f32,
    /// The gain, in dB.
    gain_db: f32,
}

impl Dynamics {
    pub fn new(sample_rate: f64) -> Self {
        let defaults = BandParams::default();
        let mut dynamics = Self {
            sample_rate,
            mode: defaults.mode,
            threshold_db: Smoothed::new(defaults.threshold_db),
            ratio: Smoothed::new(defaults.ratio),
            attack_share: 1.0,
            release_share: 1.0,
            hold_frames: 0,
            hold_left: 0,
            glide_share: share(GLIDE_SECONDS, 1, sample_rate),
            gain_db: 0.0,
        };
        dynamics.set(&defaults);
        dynamics
    }

    pub fn set(&mut self, params: &BandParams) {
        self.mode = params.mode;
        self.threshold_db.target = params.threshold_db.clamp(-60.0, 0.0);
        self.ratio.target = params.ratio.clamp(1.0, 20.0);
        let ms = |value: f32, low: f32, high: f32| f64::from(value.clamp(low, high)) / 1000.0;
        self.attack_share = share(ms(params.attack_ms, 0.5, 200.0), 1, self.sample_rate);
        self.release_share = share(ms(params.release_ms, 10.0, 2000.0), 1, self.sample_rate);
        self.hold_frames = (ms(params.hold_ms, 0.0, 500.0) * self.sample_rate) as u32;
    }

    /// Settings at their targets at once, the gain kept.
    pub fn settle(&mut self) {
        self.threshold_db.settle();
        self.ratio.settle();
    }

    pub fn reset(&mut self) {
        self.settle();
        self.gain_db = 0.0;
        self.hold_left = 0;
    }

    /// One frame: the gain in dB (0 or less) for the band's peak `level`.
    pub fn step(&mut self, level: f32) -> f32 {
        let threshold = self.threshold_db.step(self.glide_share);
        let ratio = self.ratio.step(self.glide_share);
        let wanted = static_gain_db(self.mode, ratio, gain_to_db(level) - threshold);
        if wanted < self.gain_db {
            self.gain_db += (wanted - self.gain_db) * self.attack_share;
            self.hold_left = self.hold_frames;
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        } else {
            self.gain_db += (wanted - self.gain_db) * self.release_share;
        }
        self.gain_db
    }
}
