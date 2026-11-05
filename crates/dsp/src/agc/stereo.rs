// SPDX-License-Identifier: AGPL-3.0-or-later

//! The stereo expander: boosts the side signal (L − R) of programmes close to
//! mono. The width is the side's level against the mid's (L + R), over
//! 300 ms; the boost is `depth` × (−6 dB − width), between 0 and `level` ×
//! 6 dB. Nothing is boosted under the threshold, nor below 150 Hz.

use super::crossover::{Butterworth, State};
use super::{Block, CHANNELS, CONTROL_FRAMES, GLIDE_SECONDS, Smoothed, db_to_gain, share};

/// Most boost, at `level` 100 %.
const MAX_BOOST_DB: f32 = 6.0;
/// A side within this of the mid is wide enough.
const WIDE_DB: f32 = -6.0;
/// Time constant of the width and level measures.
const MEASURE_SECONDS: f64 = 0.3;
/// The side is boosted above this.
const BASS_HZ: f64 = 150.0;

/// Settings of the stereo expander.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoExpanderParams {
    pub enabled: bool,
    /// 0 to 100 %.
    pub level: f32,
    /// 0 to 100 %.
    pub depth: f32,
    /// Programme level (dBFS RMS) under which nothing is boosted.
    pub threshold_db: f32,
}

impl Default for StereoExpanderParams {
    fn default() -> Self {
        Self {
            enabled: false,
            level: 50.0,
            depth: 50.0,
            threshold_db: -40.0,
        }
    }
}

pub struct StereoExpander {
    params: StereoExpanderParams,
    high_pass: Butterworth,
    side_state: State,
    measure_share: f32,
    glide_share: f32,
    mid_power: f32,
    side_power: f32,
    power: f32,
    boost_db: Smoothed,
    /// The side added, `gain − 1`, gliding linearly between updates.
    extra: f32,
    step: f32,
    until_update: usize,
}

impl StereoExpander {
    pub fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        Self {
            params: StereoExpanderParams::default(),
            high_pass: Butterworth::new(BASS_HZ, rate),
            side_state: [0.0; 2],
            measure_share: share(MEASURE_SECONDS, 1, rate),
            glide_share: share(GLIDE_SECONDS, CONTROL_FRAMES, rate),
            mid_power: 0.0,
            side_power: 0.0,
            power: 0.0,
            boost_db: Smoothed::new(0.0),
            extra: 0.0,
            step: 0.0,
            until_update: 0,
        }
    }

    pub fn set(&mut self, params: &StereoExpanderParams) {
        self.params = *params;
    }

    /// The boost the measures call for, in dB.
    fn wanted_db(&self) -> f32 {
        let level_db = 10.0 * self.power.max(1e-20).log10();
        if level_db < self.params.threshold_db {
            return 0.0;
        }
        let width_db = 10.0 * (self.side_power.max(1e-20) / self.mid_power.max(1e-20)).log10();
        let most = MAX_BOOST_DB * self.params.level.clamp(0.0, 100.0) / 100.0;
        (self.params.depth.clamp(0.0, 100.0) / 100.0 * (WIDE_DB - width_db)).clamp(0.0, most)
    }

    fn update(&mut self) {
        self.boost_db.target = self.wanted_db();
        let boost = self.boost_db.step(self.glide_share);
        let next = if boost == 0.0 {
            0.0
        } else {
            db_to_gain(boost) - 1.0
        };
        self.step = (next - self.extra) / CONTROL_FRAMES as f32;
        self.until_update = CONTROL_FRAMES;
    }
}

impl Block for StereoExpander {
    fn process(&mut self, samples: &mut [f32]) {
        for frame in samples.as_chunks_mut::<CHANNELS>().0 {
            let [left, right] = *frame;
            let mid = (left + right) * 0.5;
            let side = (left - right) * 0.5;
            let a = self.measure_share;
            self.mid_power += (mid * mid - self.mid_power) * a;
            self.side_power += (side * side - self.side_power) * a;
            self.power += ((left * left + right * right) * 0.5 - self.power) * a;
            if self.until_update == 0 {
                self.update();
            }
            self.until_update -= 1;
            self.extra += self.step;
            let high = self
                .high_pass
                .high_pass(&mut self.side_state, f64::from(side)) as f32;
            let side = side + self.extra * high;
            *frame = [mid + side, mid - side];
        }
        super::flush_denormals(&mut self.side_state);
    }

    fn settle(&mut self) {
        self.boost_db.target = self.wanted_db();
        self.boost_db.settle();
        self.extra = if self.boost_db.value == 0.0 {
            0.0
        } else {
            db_to_gain(self.boost_db.value) - 1.0
        };
        self.step = 0.0;
        self.until_update = CONTROL_FRAMES;
    }

    fn reset(&mut self) {
        self.side_state = [0.0; 2];
        self.mid_power = 0.0;
        self.side_power = 0.0;
        self.power = 0.0;
        self.boost_db = Smoothed::new(0.0);
        self.extra = 0.0;
        self.step = 0.0;
        self.until_update = 0;
    }
}
