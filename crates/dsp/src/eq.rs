// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 10-band graphic EQ of the decks.
//!
//! A low shelf at 60 Hz, eight peaking bands, a high shelf at 16 kHz: the
//! designs of Robert Bristow-Johnson's *Audio EQ Cookbook* ([`Coefficients`]),
//! run as state-variable filters ([`Svf`]), which change gain smoothly. Each
//! peaking band is as wide as the distances to its neighbours, and the filter
//! gains are solved from the sliders ([`filter_gains`]) so the curve meets
//! every slider at its band. Gains glide to new settings instead of stepping,
//! and a bypassed EQ is not computed at all.
//!
//! [`GraphicEq::process`] runs on the audio thread: it never allocates.
//! [`filter_gains`] does not allocate either, but runs on the control thread.
//! See `docs/audio-engine.md#deck-eq`.

use std::f64::consts::{FRAC_1_SQRT_2, PI, TAU};

/// Number of bands.
pub const BANDS: usize = 10;
/// Frequency of each band, in Hz: where the curve meets its slider.
pub const FREQUENCIES: [f64; BANDS] = [
    60.0, 170.0, 310.0, 600.0, 1_000.0, 3_000.0, 6_000.0, 12_000.0, 14_000.0, 16_000.0,
];
/// Highest boost and deepest cut of a slider, in dB.
pub const MAX_GAIN_DB: f32 = 12.0;
/// Highest boost and deepest cut of a filter, in dB: solving the filter
/// gains from the sliders may need more than the sliders' range.
pub const MAX_FILTER_GAIN_DB: f64 = 24.0;
/// Quality factor of the shelves: a slope of 1, no overshoot.
pub(crate) const SHELF_Q: f64 = FRAC_1_SQRT_2;
/// Midpoints of the low and high shelves, an octave inside their bands'
/// frequencies: at 60 Hz and 16 kHz the shelves are on their plateau, so
/// the curve holds the slider's gain out to 20 Hz and 20 kHz. With the
/// midpoints at the bands, meeting a slider there takes twice its gain,
/// which the curve then reaches beyond.
const SHELF_MIDPOINTS: (f64, f64) = (120.0, 8_000.0);
/// Rounds of [`filter_gains`]: the response is not linear in the gain.
const SOLVE_ROUNDS: usize = 4;
/// Interleaved stereo.
const CHANNELS: usize = 2;
/// Coefficients are recomputed this often while gains glide (0.7 ms at
/// 48 kHz), and interpolated sample by sample in between.
const UPDATE_FRAMES: usize = 32;
/// Time constant of the glide to new gains: there in about 50 ms.
const SMOOTHING_TAU_S: f64 = 0.015;
/// A glide ends when every band is this close to its target.
const SETTLED_DB: f64 = 0.001;
/// Filter state below this is flushed to zero, so silence never leaves
/// denormal numbers in the recursion (slow on some CPUs).
const DENORMAL_FLOOR: f64 = 1e-150;

/// The shape of a band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    LowShelf,
    Peaking,
    HighShelf,
}

/// The shape of each band.
pub const SHAPES: [Shape; BANDS] = [
    Shape::LowShelf,
    Shape::Peaking,
    Shape::Peaking,
    Shape::Peaking,
    Shape::Peaking,
    Shape::Peaking,
    Shape::Peaking,
    Shape::Peaking,
    Shape::Peaking,
    Shape::HighShelf,
];

/// The frequency a band's filter is designed at: the shelves' midpoints,
/// or the peaking bands' centres.
pub fn design_frequency(band: usize) -> f64 {
    match SHAPES[band] {
        Shape::LowShelf => SHELF_MIDPOINTS.0,
        Shape::HighShelf => SHELF_MIDPOINTS.1,
        Shape::Peaking => FREQUENCIES[band],
    }
}

/// Quality factor of a band. A peaking band is as wide as the distances to
/// its neighbours, on average: the bands are not evenly spaced.
pub fn band_q(band: usize) -> f64 {
    if SHAPES[band] != Shape::Peaking {
        return SHELF_Q;
    }
    let below = (FREQUENCIES[band] / FREQUENCIES[band - 1]).log2();
    let above = (FREQUENCIES[band + 1] / FREQUENCIES[band]).log2();
    let octaves = f64::midpoint(below, above);
    let ratio = 2f64.powf(octaves);
    ratio.sqrt() / (ratio - 1.0)
}

/// Coefficients of one biquad, normalized (`a0` = 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefficients {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefficients {
    /// A peaking filter: `gain_db` at `frequency`, 0 dB far from it.
    pub fn peaking(sample_rate: f64, frequency: f64, q: f64, gain_db: f64) -> Self {
        let w0 = TAU * frequency / sample_rate;
        let (cos_w0, alpha) = (w0.cos(), w0.sin() / (2.0 * q));
        let a = 10f64.powf(gain_db / 40.0);
        Self::normalized(
            [1.0 + alpha * a, -2.0 * cos_w0, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos_w0, 1.0 - alpha / a],
        )
    }

    /// A shelf: `gain_db` below `frequency` (or above it, `high`), half of
    /// it in dB at `frequency`, 0 dB on the other side.
    pub fn shelf(sample_rate: f64, frequency: f64, q: f64, gain_db: f64, high: bool) -> Self {
        let w0 = TAU * frequency / sample_rate;
        let (cos_w0, alpha) = (w0.cos(), w0.sin() / (2.0 * q));
        let a = 10f64.powf(gain_db / 40.0);
        let root = 2.0 * a.sqrt() * alpha;
        // The high shelf is the low shelf with the sign of cos(w0) flipped
        // in the right places.
        let s = if high { -1.0 } else { 1.0 };
        Self::normalized(
            [
                a * ((a + 1.0) - s * (a - 1.0) * cos_w0 + root),
                s * 2.0 * a * ((a - 1.0) - s * (a + 1.0) * cos_w0),
                a * ((a + 1.0) - s * (a - 1.0) * cos_w0 - root),
            ],
            [
                (a + 1.0) + s * (a - 1.0) * cos_w0 + root,
                -s * 2.0 * ((a - 1.0) + s * (a + 1.0) * cos_w0),
                (a + 1.0) + s * (a - 1.0) * cos_w0 - root,
            ],
        )
    }

    /// The design of a band of the EQ at `gain_db`.
    pub fn band(sample_rate: f64, band: usize, gain_db: f64) -> Self {
        let (frequency, q) = (design_frequency(band), band_q(band));
        match SHAPES[band] {
            Shape::Peaking => Self::peaking(sample_rate, frequency, q, gain_db),
            Shape::LowShelf => Self::shelf(sample_rate, frequency, q, gain_db, false),
            Shape::HighShelf => Self::shelf(sample_rate, frequency, q, gain_db, true),
        }
    }

    fn normalized(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }

    /// Gain of the filter at `frequency`, in dB.
    pub fn response_db(&self, sample_rate: f64, frequency: f64) -> f64 {
        let w = TAU * frequency / sample_rate;
        // |H(e^jw)|² with z^-1 = e^-jw.
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        10.0 * ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).log10()
    }
}

/// Gain at `frequency` of the filters at `filter_gains_db`, in dB.
pub fn filters_response_db(
    sample_rate: f64,
    filter_gains_db: &[f64; BANDS],
    frequency: f64,
) -> f64 {
    filter_gains_db
        .iter()
        .enumerate()
        .map(|(band, &gain)| {
            Coefficients::band(sample_rate, band, gain).response_db(sample_rate, frequency)
        })
        .sum()
}

/// The highest point of the curve of the filters at `filter_gains_db`,
/// from 20 Hz to 20 kHz, in dB; 0 when they only cut. How much headroom
/// the EQ takes. Does not allocate.
pub fn max_boost_db(sample_rate: f64, filter_gains_db: &[f64; BANDS]) -> f64 {
    const POINTS: u32 = 200;
    (0..=POINTS)
        .map(|point| {
            let frequency = 20.0 * 1_000f64.powf(f64::from(point) / f64::from(POINTS));
            filters_response_db(sample_rate, filter_gains_db, frequency)
        })
        .fold(0.0, f64::max)
}

/// Gain of the EQ at `frequency` for these sliders, in dB: what its curve
/// shows. For many frequencies, solve [`filter_gains`] once and use
/// [`filters_response_db`].
pub fn response_db(sample_rate: f64, sliders_db: &[f32; BANDS], frequency: f64) -> f64 {
    filters_response_db(
        sample_rate,
        &filter_gains(sample_rate, sliders_db),
        frequency,
    )
}

/// The filter gains whose summed response, at each band's centre, is that
/// band's slider: neighbouring bands overlap, so a filter at its slider's
/// gain would not do. Flat sliders give filters at 0 dB. Limited to
/// ±[`MAX_FILTER_GAIN_DB`]. Does not allocate.
pub fn filter_gains(sample_rate: f64, sliders_db: &[f32; BANDS]) -> [f64; BANDS] {
    let target = clamp_gains(*sliders_db).map(f64::from);
    let mut gains = target;
    for _ in 0..SOLVE_ROUNDS {
        // How much each filter moves each band's centre, per dB of its gain,
        // around the gains found so far.
        let mut matrix = [[0.0; BANDS]; BANDS];
        for (band, &gain) in gains.iter().enumerate() {
            let probe = if gain.abs() > 0.5 { gain } else { 1.0 };
            let coefficients = Coefficients::band(sample_rate, band, probe);
            for (row, &centre) in matrix.iter_mut().zip(&FREQUENCIES) {
                row[band] = coefficients.response_db(sample_rate, centre) / probe;
            }
        }
        let error: [f64; BANDS] = std::array::from_fn(|band| {
            target[band] - filters_response_db(sample_rate, &gains, FREQUENCIES[band])
        });
        let Some(step) = solve(matrix, error) else {
            break;
        };
        for (gain, step) in gains.iter_mut().zip(step) {
            *gain = (*gain + step).clamp(-MAX_FILTER_GAIN_DB, MAX_FILTER_GAIN_DB);
        }
    }
    gains
}

/// Solves `matrix · x = vector` (Gauss–Jordan with partial pivoting).
fn solve(mut matrix: [[f64; BANDS]; BANDS], mut vector: [f64; BANDS]) -> Option<[f64; BANDS]> {
    for column in 0..BANDS {
        let pivot = (column..BANDS)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))?;
        if matrix[pivot][column].abs() < 1e-12 {
            return None;
        }
        matrix.swap(column, pivot);
        vector.swap(column, pivot);
        for row in 0..BANDS {
            if row != column {
                let factor = matrix[row][column] / matrix[column][column];
                let pivot_row = matrix[column];
                for (value, pivot_value) in matrix[row].iter_mut().zip(pivot_row) {
                    *value -= factor * pivot_value;
                }
                vector[row] -= factor * vector[column];
            }
        }
    }
    Some(std::array::from_fn(|row| vector[row] / matrix[row][row]))
}

/// Clamps slider gains to ±[`MAX_GAIN_DB`]; NaN becomes 0 dB.
pub fn clamp_gains(gains_db: [f32; BANDS]) -> [f32; BANDS] {
    gains_db.map(|gain| {
        if gain.is_nan() {
            0.0
        } else {
            gain.clamp(-MAX_GAIN_DB, MAX_GAIN_DB)
        }
    })
}

/// One band as a trapezoidal state-variable filter (Andrew Simper,
/// *Linear Trap Optimised SVF*): the bell and shelves of the cookbook with
/// exactly their response, but a state that stays meaningful when the gain
/// changes while it plays: no click on a glide. The output mixes the input
/// and the band-pass and low-pass outputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Svf {
    a1: f64,
    a2: f64,
    a3: f64,
    m0: f64,
    m1: f64,
    m2: f64,
}

impl Svf {
    /// `warped` is tan(π f / fs), which does not depend on the gain.
    pub(crate) fn new(shape: Shape, warped: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (g, k, m0, m1, m2) = match shape {
            Shape::Peaking => {
                let k = 1.0 / (q * a);
                (warped, k, 1.0, k * (a * a - 1.0), 0.0)
            }
            Shape::LowShelf => {
                let k = 1.0 / q;
                (warped / a.sqrt(), k, 1.0, k * (a - 1.0), a * a - 1.0)
            }
            Shape::HighShelf => {
                let k = 1.0 / q;
                (warped * a.sqrt(), k, a * a, k * (1.0 - a) * a, 1.0 - a * a)
            }
        };
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            m0,
            m1,
            m2,
        }
    }

    /// Filters one sample; `state` holds the two integrators of a channel.
    pub(crate) fn tick(&self, state: &mut [f64; 2], x: f64) -> f64 {
        let [ic1, ic2] = state;
        let v3 = x - *ic2;
        let v1 = self.a1 * *ic1 + self.a2 * v3;
        let v2 = *ic2 + self.a2 * *ic1 + self.a3 * v3;
        *ic1 = 2.0 * v1 - *ic1;
        *ic2 = 2.0 * v2 - *ic2;
        self.m0 * x + self.m1 * v1 + self.m2 * v2
    }

    const ZERO: Self = Self {
        a1: 0.0,
        a2: 0.0,
        a3: 0.0,
        m0: 0.0,
        m1: 0.0,
        m2: 0.0,
    };

    /// What to add at each of `frames` samples to go from `self` to `to`.
    fn step_to(&self, to: &Self, frames: usize) -> Self {
        let n = frames as f64;
        Self {
            a1: (to.a1 - self.a1) / n,
            a2: (to.a2 - self.a2) / n,
            a3: (to.a3 - self.a3) / n,
            m0: (to.m0 - self.m0) / n,
            m1: (to.m1 - self.m1) / n,
            m2: (to.m2 - self.m2) / n,
        }
    }

    fn add(&mut self, step: &Self) {
        self.a1 += step.a1;
        self.a2 += step.a2;
        self.a3 += step.a3;
        self.m0 += step.m0;
        self.m1 += step.m1;
        self.m2 += step.m2;
    }
}

/// The 10-band EQ of one deck, on interleaved stereo. It takes filter
/// gains: solve them from the sliders with [`filter_gains`].
pub struct GraphicEq {
    /// Per band: tan(π f / fs).
    warped: [f64; BANDS],
    filters: [Svf; BANDS],
    /// Added to `filters` at each sample while gliding.
    steps: [Svf; BANDS],
    /// The two integrators of each band, per channel.
    state: [[[f64; 2]; CHANNELS]; BANDS],
    current_db: [f64; BANDS],
    target_db: [f64; BANDS],
    /// The EQ is wanted (it may still be gliding to flat when not).
    enabled: bool,
    /// The EQ is computed: enabled, or gliding to flat before the bypass.
    active: bool,
    gliding: bool,
    /// The gains reached their targets: the coefficients get there at the
    /// end of this update.
    last_update: bool,
    /// Frames until the coefficients are next recomputed.
    until_update: usize,
    /// Share of the way to the target covered at each update.
    smoothing: f64,
}

impl GraphicEq {
    /// A bypassed, flat EQ.
    pub fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        let warped: [f64; BANDS] =
            std::array::from_fn(|band| (PI * design_frequency(band) / rate).tan());
        let update_s = UPDATE_FRAMES as f64 / rate;
        let mut eq = Self {
            warped,
            filters: [Svf::ZERO; BANDS],
            steps: [Svf::ZERO; BANDS],
            state: [[[0.0; 2]; CHANNELS]; BANDS],
            current_db: [0.0; BANDS],
            target_db: [0.0; BANDS],
            enabled: false,
            active: false,
            gliding: false,
            last_update: false,
            until_update: 0,
            smoothing: 1.0 - (-update_s / SMOOTHING_TAU_S).exp(),
        };
        eq.filters = eq.filters_at(&eq.current_db);
        eq
    }

    /// New filter gains, reached with a glide. Enabling glides from flat;
    /// bypassing glides to flat, then stops computing the EQ.
    pub fn set(&mut self, enabled: bool, filter_gains_db: [f32; BANDS]) {
        if enabled && !self.active {
            self.state = [[[0.0; 2]; CHANNELS]; BANDS];
            self.current_db = [0.0; BANDS];
            self.filters = self.filters_at(&self.current_db);
            self.active = true;
        }
        self.enabled = enabled;
        self.target_db = if enabled {
            filter_gains_db.map(|gain| {
                if gain.is_nan() {
                    0.0
                } else {
                    f64::from(gain).clamp(-MAX_FILTER_GAIN_DB, MAX_FILTER_GAIN_DB)
                }
            })
        } else {
            [0.0; BANDS]
        };
        self.gliding = self.active;
        self.last_update = false;
        self.until_update = 0;
    }

    /// New filter gains at once, without a glide, and the filters at rest:
    /// for an engine starting.
    pub fn restart(&mut self, enabled: bool, filter_gains_db: [f32; BANDS]) {
        self.set(enabled, filter_gains_db);
        self.restart_track();
    }

    /// The filters at rest and the gains at their targets, without a glide:
    /// a new track starts on the deck.
    pub fn restart_track(&mut self) {
        self.state = [[[0.0; 2]; CHANNELS]; BANDS];
        self.current_db = self.target_db;
        self.gliding = false;
        self.last_update = false;
        self.active = self.enabled;
        self.filters = self.filters_at(&self.current_db);
        self.steps = [Svf::ZERO; BANDS];
    }

    /// The EQ is computed (enabled, or gliding to its bypass).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Filters interleaved stereo in place. Does nothing while bypassed.
    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.active {
            return;
        }
        let whole = samples.len() / CHANNELS * CHANNELS;
        let mut frames = &mut samples[..whole];
        while !frames.is_empty() {
            if self.gliding && self.until_update == 0 {
                self.glide();
                if !self.active {
                    return;
                }
                self.until_update = UPDATE_FRAMES;
            }
            let length = if self.gliding {
                self.until_update.min(frames.len() / CHANNELS)
            } else {
                frames.len() / CHANNELS
            };
            let (chunk, rest) = frames.split_at_mut(length * CHANNELS);
            if self.gliding {
                self.filter::<true>(chunk);
            } else {
                self.filter::<false>(chunk);
            }
            if self.gliding {
                self.until_update -= length;
            }
            frames = rest;
        }
        self.flush_denormals();
    }

    fn filter<const GLIDING: bool>(&mut self, samples: &mut [f32]) {
        for frame in samples.as_chunks_mut::<CHANNELS>().0 {
            if GLIDING {
                for (filter, step) in self.filters.iter_mut().zip(&self.steps) {
                    filter.add(step);
                }
            }
            for (channel, sample) in frame.iter_mut().enumerate() {
                let mut x = f64::from(*sample);
                for (filter, state) in self.filters.iter().zip(&mut self.state) {
                    let [ic1, ic2] = &mut state[channel];
                    let v3 = x - *ic2;
                    let v1 = filter.a1 * *ic1 + filter.a2 * v3;
                    let v2 = *ic2 + filter.a2 * *ic1 + filter.a3 * v3;
                    *ic1 = 2.0 * v1 - *ic1;
                    *ic2 = 2.0 * v2 - *ic2;
                    x = filter.m0 * x + filter.m1 * v1 + filter.m2 * v2;
                }
                *sample = x as f32;
            }
        }
    }

    /// One step of the glide towards the targets: the coefficients move to
    /// the next gains over the next [`UPDATE_FRAMES`].
    fn glide(&mut self) {
        if self.last_update {
            // The coefficients are there: exactly, from here.
            self.gliding = false;
            self.last_update = false;
            self.filters = self.filters_at(&self.target_db);
            self.steps = [Svf::ZERO; BANDS];
            if !self.enabled {
                // Flat now: bypass from here.
                self.active = false;
                self.state = [[[0.0; 2]; CHANNELS]; BANDS];
            }
            return;
        }
        let mut settled = true;
        for (current, target) in self.current_db.iter_mut().zip(&self.target_db) {
            *current += (target - *current) * self.smoothing;
            if (target - *current).abs() < SETTLED_DB {
                *current = *target;
            } else {
                settled = false;
            }
        }
        self.last_update = settled;
        let next = self.filters_at(&self.current_db);
        for ((step, filter), next) in self.steps.iter_mut().zip(&self.filters).zip(&next) {
            *step = filter.step_to(next, UPDATE_FRAMES);
        }
    }

    fn filters_at(&self, gains_db: &[f64; BANDS]) -> [Svf; BANDS] {
        std::array::from_fn(|band| {
            Svf::new(
                SHAPES[band],
                self.warped[band],
                band_q(band),
                gains_db[band],
            )
        })
    }

    fn flush_denormals(&mut self) {
        for value in self.state.as_flattened_mut().as_flattened_mut() {
            if value.abs() < DENORMAL_FLOOR {
                *value = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests;
