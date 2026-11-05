// SPDX-License-Identifier: AGPL-3.0-or-later

//! The bass EQ: a low shelf or a peak, one state-variable filter of the deck
//! EQ's kind, so its gain and frequency glide without clicks. A change of
//! shape glides the gain to 0 dB, where both shapes pass the signal as it is,
//! switches, then glides to the gain.

use std::f64::consts::PI;

use super::{Block, CHANNELS, GLIDE_SECONDS, Smoothed, share};
use crate::eq::{SHELF_Q, Shape, Svf};

/// Q of the peak: about an octave and a third wide.
const PEAK_Q: f64 = 1.0;
pub const MIN_FREQUENCY_HZ: f32 = 40.0;
pub const MAX_FREQUENCY_HZ: f32 = 250.0;
pub const MAX_GAIN_DB: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BassShape {
    #[default]
    Shelf,
    Peak,
}

/// Settings of the bass EQ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BassEqParams {
    pub enabled: bool,
    pub shape: BassShape,
    pub frequency_hz: f32,
    pub gain_db: f32,
}

impl Default for BassEqParams {
    fn default() -> Self {
        Self {
            enabled: false,
            shape: BassShape::Shelf,
            frequency_hz: 100.0,
            gain_db: 0.0,
        }
    }
}

pub struct BassEq {
    sample_rate: f64,
    shape: BassShape,
    wanted_shape: BassShape,
    wanted_gain_db: f32,
    gain_db: Smoothed,
    frequency: Smoothed,
    glide_share: f32,
    filter: Svf,
    state: [[f64; 2]; CHANNELS],
}

impl BassEq {
    pub fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        let defaults = BassEqParams::default();
        let mut eq = Self {
            sample_rate: rate,
            shape: defaults.shape,
            wanted_shape: defaults.shape,
            wanted_gain_db: defaults.gain_db,
            gain_db: Smoothed::new(defaults.gain_db),
            frequency: Smoothed::new(defaults.frequency_hz),
            glide_share: share(GLIDE_SECONDS, 1, rate),
            filter: Svf::new(Shape::LowShelf, 0.0, SHELF_Q, 0.0),
            state: [[0.0; 2]; CHANNELS],
        };
        eq.design();
        eq
    }

    pub fn set(&mut self, params: &BassEqParams) {
        self.wanted_shape = params.shape;
        self.wanted_gain_db = params.gain_db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        self.frequency.target = params
            .frequency_hz
            .clamp(MIN_FREQUENCY_HZ, MAX_FREQUENCY_HZ);
        self.aim();
    }

    /// While the shape is to change, the gain goes to 0 dB first.
    fn aim(&mut self) {
        self.gain_db.target = if self.shape == self.wanted_shape {
            self.wanted_gain_db
        } else {
            0.0
        };
    }

    fn design(&mut self) {
        let warped = (PI * f64::from(self.frequency.value) / self.sample_rate).tan();
        let (shape, q) = match self.shape {
            BassShape::Shelf => (Shape::LowShelf, SHELF_Q),
            BassShape::Peak => (Shape::Peaking, PEAK_Q),
        };
        self.filter = Svf::new(shape, warped, q, f64::from(self.gain_db.value));
    }

    /// While gliding, the filter is designed again at every sample: in steps,
    /// it would whine at the rate of the steps.
    fn glide(&mut self) {
        if self.shape != self.wanted_shape && self.gain_db.value == 0.0 {
            self.shape = self.wanted_shape;
            self.aim();
        }
        if !self.gain_db.is_settled() || !self.frequency.is_settled() {
            self.gain_db.step(self.glide_share);
            // In octaves.
            let ratio = self.frequency.target / self.frequency.value;
            self.frequency.value *= ratio.powf(self.glide_share);
            if (ratio - 1.0).abs() < 1e-4 {
                self.frequency.settle();
            }
            self.design();
        }
    }
}

impl Block for BassEq {
    fn process(&mut self, samples: &mut [f32]) {
        for frame in samples.as_chunks_mut::<CHANNELS>().0 {
            self.glide();
            for (sample, state) in frame.iter_mut().zip(&mut self.state) {
                *sample = self.filter.tick(state, f64::from(*sample)) as f32;
            }
        }
        super::flush_denormals(self.state.as_flattened_mut());
    }

    fn settle(&mut self) {
        self.shape = self.wanted_shape;
        self.aim();
        self.gain_db.settle();
        self.frequency.settle();
        self.design();
    }

    fn reset(&mut self) {
        self.state = [[0.0; 2]; CHANNELS];
        self.settle();
    }
}
