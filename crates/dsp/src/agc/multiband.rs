// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 5-band and dual-band processors. Linkwitz-Riley crossovers split the
//! sound into bands, each band goes through its dynamics and its gain, and
//! the bands add up again. Each band also goes through the all-pass of the
//! crossovers above it that do not split it, so flat bands add up to the
//! input's magnitude exactly. Linked adjacent bands apply the deeper of their
//! gain reductions; changing the links glides each band from the gain it had
//! to the new grouping's over 50 ms, also when the links change again during
//! a glide.

use super::crossover::{Crossover, State};
use super::dynamics::Dynamics;
use super::{BandParams, Block, CHANNELS, GLIDE_SECONDS, Smoothed, db_to_gain, share};

/// Most bands.
const MAX_BANDS: usize = 5;
/// Crossovers are kept at least a third of an octave apart.
const MIN_RATIO: f32 = 1.259_921;
pub const MIN_CROSSOVER_HZ: f32 = 40.0;
pub const MAX_CROSSOVER_HZ: f32 = 16_000.0;
pub const MAX_OVERDRIVE_DB: f32 = 12.0;
pub const MAX_BAND_GAIN_DB: f32 = 12.0;
/// How long a change of links crossfades.
const LINK_SECONDS: f64 = 0.05;

/// Settings of the 5-band processor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiveBandParams {
    pub enabled: bool,
    pub overdrive_db: f32,
    pub crossovers_hz: [f32; 4],
    /// `links[i]` links band `i` and band `i + 1`.
    pub links: [bool; 4],
    pub bands: [BandParams; 5],
}

impl Default for FiveBandParams {
    fn default() -> Self {
        Self {
            enabled: false,
            overdrive_db: 0.0,
            crossovers_hz: [100.0, 400.0, 1_600.0, 6_400.0],
            links: [false; 4],
            bands: [BandParams::default(); 5],
        }
    }
}

/// Settings of the dual-band processor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DualBandParams {
    pub enabled: bool,
    pub crossover_hz: f32,
    pub link: bool,
    /// Low, then high.
    pub bands: [BandParams; 2],
}

impl Default for DualBandParams {
    fn default() -> Self {
        Self {
            enabled: false,
            crossover_hz: 200.0,
            link: false,
            bands: [BandParams::default(); 2],
        }
    }
}

/// A processor of 2 to 5 bands.
pub struct Multiband {
    sample_rate: f64,
    bands: usize,
    crossovers: [Crossover; MAX_BANDS - 1],
    frequencies: [Smoothed; MAX_BANDS - 1],
    /// All-pass states: per band, per crossover, per channel.
    all_pass: [[[State; CHANNELS]; MAX_BANDS - 1]; MAX_BANDS],
    dynamics: [Dynamics; MAX_BANDS],
    band_gain: [Smoothed; MAX_BANDS],
    links: [bool; MAX_BANDS - 1],
    /// What each band's gain was off the new links' when they changed, in
    /// dB: faded out from there.
    link_offset_db: [f32; MAX_BANDS],
    /// 1 to 0: how much of the offset is left.
    link_fade: f32,
    /// The bands' gains last computed, before and after the links, in dB.
    last_gains_db: [f32; MAX_BANDS],
    last_linked_db: [f32; MAX_BANDS],
    link_step: f32,
    drive: Smoothed,
    share: f32,
    /// The deepest gain of each band since the meters were read, in dB.
    deepest_db: [f32; MAX_BANDS],
}

impl Multiband {
    /// A processor of `bands` bands, 2 to 5.
    pub fn new(sample_rate: u32, bands: usize) -> Self {
        let rate = f64::from(sample_rate);
        let bands = bands.clamp(2, MAX_BANDS);
        let defaults = FiveBandParams::default().crossovers_hz;
        Self {
            sample_rate: rate,
            bands,
            crossovers: defaults.map(|hz| Crossover::new(f64::from(hz), rate)),
            frequencies: defaults.map(Smoothed::new),
            all_pass: [[[[0.0; 2]; CHANNELS]; MAX_BANDS - 1]; MAX_BANDS],
            dynamics: [Dynamics::new(rate); MAX_BANDS],
            band_gain: [Smoothed::new(1.0); MAX_BANDS],
            links: [false; MAX_BANDS - 1],
            link_offset_db: [0.0; MAX_BANDS],
            link_fade: 0.0,
            last_gains_db: [0.0; MAX_BANDS],
            last_linked_db: [0.0; MAX_BANDS],
            link_step: (1.0 / (LINK_SECONDS * rate)) as f32,
            drive: Smoothed::new(1.0),
            share: share(GLIDE_SECONDS, 1, rate),
            deepest_db: [0.0; MAX_BANDS],
        }
    }

    pub fn set_five(&mut self, params: &FiveBandParams) {
        self.set(
            params.overdrive_db,
            &params.crossovers_hz,
            &params.links,
            &params.bands,
        );
    }

    pub fn set_dual(&mut self, params: &DualBandParams) {
        self.set(0.0, &[params.crossover_hz], &[params.link], &params.bands);
    }

    fn set(&mut self, overdrive_db: f32, crossovers: &[f32], links: &[bool], bands: &[BandParams]) {
        self.drive.target = db_to_gain(overdrive_db.clamp(0.0, MAX_OVERDRIVE_DB));
        // Rising, and apart: what validation guarantees, kept here too.
        let mut lowest = MIN_CROSSOVER_HZ;
        for (frequency, &wanted) in self.frequencies.iter_mut().zip(crossovers) {
            frequency.target = wanted.clamp(lowest, MAX_CROSSOVER_HZ);
            lowest = frequency.target * MIN_RATIO;
        }
        let mut wanted = [false; MAX_BANDS - 1];
        for (link, &on) in wanted.iter_mut().zip(links) {
            *link = on;
        }
        if wanted != self.links {
            // From the gains the bands have now, mid-glide or not.
            self.links = wanted;
            let mut target = self.last_gains_db;
            Self::link(self.bands, &self.links, &mut target);
            for ((offset, &now), target) in self
                .link_offset_db
                .iter_mut()
                .zip(&self.last_linked_db)
                .zip(target)
            {
                *offset = now - target;
            }
            self.link_fade = 1.0;
        }
        for ((dynamics, gain), params) in
            self.dynamics.iter_mut().zip(&mut self.band_gain).zip(bands)
        {
            dynamics.set(params);
            gain.target = db_to_gain(params.gain_db.clamp(-MAX_BAND_GAIN_DB, MAX_BAND_GAIN_DB));
        }
    }

    /// The deepest gain of each band since the last call, in dB (0 or less).
    pub fn take_deepest_db(&mut self) -> [f32; MAX_BANDS] {
        std::mem::replace(&mut self.deepest_db, [0.0; MAX_BANDS])
    }

    /// The crossovers glide in octaves, tuned again at every sample.
    fn glide(&mut self) {
        for (crossover, frequency) in self.crossovers.iter_mut().zip(&mut self.frequencies) {
            if !frequency.is_settled() {
                let ratio = frequency.target / frequency.value;
                frequency.value *= ratio.powf(self.share);
                if (ratio - 1.0).abs() < 1e-4 {
                    frequency.settle();
                }
                crossover.tune(f64::from(frequency.value), self.sample_rate);
            }
        }
    }

    /// The bands' gains with the links, gliding from where they were when
    /// the links changed.
    pub(super) fn linked(&mut self, gains: &[f32; MAX_BANDS]) -> [f32; MAX_BANDS] {
        let mut now = *gains;
        Self::link(self.bands, &self.links, &mut now);
        if self.link_fade > 0.0 {
            for (now, offset) in now.iter_mut().zip(self.link_offset_db) {
                *now += offset * self.link_fade;
            }
            self.link_fade = (self.link_fade - self.link_step).max(0.0);
        }
        self.last_gains_db = *gains;
        self.last_linked_db = now;
        now
    }

    /// Linked bands take the deepest gain among them.
    pub(super) fn link(bands: usize, links: &[bool; MAX_BANDS - 1], gains: &mut [f32; MAX_BANDS]) {
        let mut start = 0;
        while start < bands {
            let mut end = start;
            while end + 1 < bands && links[end] {
                end += 1;
            }
            let deepest = gains[start..=end].iter().copied().fold(0.0, f32::min);
            gains[start..=end].fill(deepest);
            start = end + 1;
        }
    }
}

impl Block for Multiband {
    fn process(&mut self, samples: &mut [f32]) {
        let count = self.bands;
        let splits = count - 1;
        for frame in samples.as_chunks_mut::<CHANNELS>().0 {
            self.glide();
            let drive = f64::from(self.drive.step(self.share));
            let mut signals = [[0.0f64; CHANNELS]; MAX_BANDS];
            for (channel, &sample) in frame.iter().enumerate() {
                let mut rest = f64::from(sample) * drive;
                for (band, crossover) in self.crossovers[..splits].iter_mut().enumerate() {
                    let (low, high) = crossover.split(channel, rest);
                    signals[band][channel] = low;
                    rest = high;
                }
                signals[splits][channel] = rest;
                for (band, (signal, all_pass)) in signals
                    .iter_mut()
                    .zip(&mut self.all_pass)
                    .enumerate()
                    .take(splits)
                {
                    let above = band + 1..splits;
                    for (crossover, state) in self.crossovers[above.clone()]
                        .iter()
                        .zip(&mut all_pass[above])
                    {
                        signal[channel] = crossover
                            .filter
                            .all_pass(&mut state[channel], signal[channel]);
                    }
                }
            }
            let mut gains = [0.0f32; MAX_BANDS];
            for ((gain, dynamics), signal) in gains
                .iter_mut()
                .zip(&mut self.dynamics)
                .zip(&signals)
                .take(count)
            {
                *gain = dynamics.step(signal[0].abs().max(signal[1].abs()) as f32);
            }
            let gains = self.linked(&gains);
            let mut out = [0.0f64; CHANNELS];
            for band in 0..count {
                self.deepest_db[band] = self.deepest_db[band].min(gains[band]);
                let gain =
                    f64::from(db_to_gain(gains[band]) * self.band_gain[band].step(self.share));
                out[0] += signals[band][0] * gain;
                out[1] += signals[band][1] * gain;
            }
            *frame = [out[0] as f32, out[1] as f32];
        }
        for crossover in &mut self.crossovers {
            super::flush_denormals(crossover.state_mut());
        }
        super::flush_denormals(
            self.all_pass
                .as_flattened_mut()
                .as_flattened_mut()
                .as_flattened_mut(),
        );
    }

    fn settle(&mut self) {
        for (crossover, frequency) in self.crossovers.iter_mut().zip(&mut self.frequencies) {
            frequency.settle();
            crossover.tune(f64::from(frequency.value), self.sample_rate);
        }
        for dynamics in &mut self.dynamics {
            dynamics.settle();
        }
        for gain in &mut self.band_gain {
            gain.settle();
        }
        self.drive.settle();
        self.link_fade = 0.0;
    }

    fn reset(&mut self) {
        self.settle();
        for crossover in &mut self.crossovers {
            crossover.reset();
        }
        self.all_pass = [[[[0.0; 2]; CHANNELS]; MAX_BANDS - 1]; MAX_BANDS];
        for dynamics in &mut self.dynamics {
            dynamics.reset();
        }
        self.last_gains_db = [0.0; MAX_BANDS];
        self.last_linked_db = [0.0; MAX_BANDS];
        self.deepest_db = [0.0; MAX_BANDS];
    }
}
