// SPDX-License-Identifier: AGPL-3.0-or-later

//! The gated AGC: a slow gain bringing the programme's short-term loudness
//! (K-weighted, 3 s, as EBU R128) to a target, held while the momentary
//! loudness (400 ms) is under the gate, so silences, fades and pauses do not
//! make it rise.

use super::{Block, CHANNELS, db_to_gain, flush_denormals, share};

/// Loudness is measured in blocks of 100 ms.
const BLOCK_SECONDS: f64 = 0.1;
/// Short-term loudness: 3 s.
const SHORT_TERM_BLOCKS: usize = 30;
/// Momentary loudness: 400 ms.
const MOMENTARY_BLOCKS: usize = 4;
/// Time constant of a falling gain.
const DOWN_SECONDS: f64 = 1.0;
/// Time constant of a rising gain.
const UP_SECONDS: f64 = 4.0;
/// Highest `max_gain_db`.
pub const MAX_GAIN_DB: f32 = 18.0;

/// Settings of the gated AGC.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GatedAgcParams {
    pub enabled: bool,
    /// The loudness aimed for: the station's loudness target (LUFS).
    pub target_lufs: f32,
    /// Under this momentary loudness (LUFS) the gain holds.
    pub gate_lufs: f32,
    /// The gain stays within ± this (dB).
    pub max_gain_db: f32,
}

impl Default for GatedAgcParams {
    fn default() -> Self {
        Self {
            enabled: false,
            target_lufs: -16.0,
            gate_lufs: -45.0,
            max_gain_db: 6.0,
        }
    }
}

/// A biquad, normalized, run as a transposed direct form II. The K-weighting
/// filters never change, so this simplest form is enough.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
}

impl Biquad {
    fn tick(&self, state: &mut [f64; 2], x: f64) -> f64 {
        let y = self.b[0] * x + state[0];
        state[0] = self.b[1] * x - self.a[0] * y + state[1];
        state[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two filters of the K-weighting (ITU-R BS.1770): a high shelf of about
/// +4 dB above 1.5 kHz, then a high-pass at about 38 Hz. Designed for any
/// sample rate as libebur128 does.
fn k_weighting(sample_rate: f64) -> [Biquad; 2] {
    let (f0, gain_db, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (std::f64::consts::PI * f0 / sample_rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    };
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / sample_rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    };
    [shelf, high_pass]
}

/// The loudness (LUFS) of a sum of the channels' K-weighted mean squares.
fn loudness(mean_square: f64) -> f32 {
    (-0.691 + 10.0 * mean_square.max(1e-20).log10()) as f32
}

/// K-weighted loudness, in blocks of 100 ms.
#[derive(Clone, Debug)]
pub(crate) struct LoudnessMeter {
    filters: [Biquad; 2],
    /// Per filter, per channel.
    state: [[[f64; 2]; CHANNELS]; 2],
    block_frames: usize,
    frames: usize,
    sum: f64,
    /// Mean square of the last blocks, the latest at `latest`.
    blocks: [f64; SHORT_TERM_BLOCKS],
    latest: usize,
    filled: usize,
}

impl LoudnessMeter {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            filters: k_weighting(sample_rate),
            state: [[[0.0; 2]; CHANNELS]; 2],
            block_frames: (sample_rate * BLOCK_SECONDS).round() as usize,
            frames: 0,
            sum: 0.0,
            blocks: [0.0; SHORT_TERM_BLOCKS],
            latest: 0,
            filled: 0,
        }
    }

    /// Adds a frame; true when it completed a block.
    pub fn add(&mut self, frame: [f32; CHANNELS]) -> bool {
        for (channel, &sample) in frame.iter().enumerate() {
            let mut x = f64::from(sample);
            for (filter, state) in self.filters.iter().zip(&mut self.state) {
                x = filter.tick(&mut state[channel], x);
            }
            self.sum += x * x;
        }
        self.frames += 1;
        if self.frames < self.block_frames {
            return false;
        }
        self.latest = (self.latest + 1) % SHORT_TERM_BLOCKS;
        self.blocks[self.latest] = self.sum / self.block_frames as f64;
        self.filled = (self.filled + 1).min(SHORT_TERM_BLOCKS);
        self.frames = 0;
        self.sum = 0.0;
        flush_denormals(self.state.as_flattened_mut().as_flattened_mut());
        true
    }

    /// Loudness of the last `blocks` blocks (as many as there are).
    pub fn loudness(&self, blocks: usize) -> f32 {
        let count = blocks.min(self.filled).max(1);
        let sum: f64 = (0..count)
            .map(|back| self.blocks[(self.latest + SHORT_TERM_BLOCKS - back) % SHORT_TERM_BLOCKS])
            .sum();
        loudness(sum / count as f64)
    }

    pub fn filled(&self) -> usize {
        self.filled
    }

    pub fn block_frames(&self) -> usize {
        self.block_frames
    }

    pub fn reset(&mut self) {
        self.state = [[[0.0; 2]; CHANNELS]; 2];
        self.frames = 0;
        self.sum = 0.0;
        self.blocks = [0.0; SHORT_TERM_BLOCKS];
        self.filled = 0;
    }
}

/// The gated AGC on interleaved stereo.
pub struct GatedAgc {
    params: GatedAgcParams,
    meter: LoudnessMeter,
    down_share: f32,
    up_share: f32,
    gain_db: f32,
    /// The gain applied, gliding linearly to `gain_target` over a block.
    gain: f32,
    gain_target: f32,
    step: f32,
}

impl GatedAgc {
    pub fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        let meter = LoudnessMeter::new(rate);
        let block = meter.block_frames();
        Self {
            params: GatedAgcParams::default(),
            meter,
            down_share: share(DOWN_SECONDS, block, rate),
            up_share: share(UP_SECONDS, block, rate),
            gain_db: 0.0,
            gain: 1.0,
            gain_target: 1.0,
            step: 0.0,
        }
    }

    pub fn set(&mut self, params: &GatedAgcParams) {
        self.params = *params;
        self.params.max_gain_db = params.max_gain_db.clamp(0.0, MAX_GAIN_DB);
    }

    /// The gain now, in dB.
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// Starts from this gain: the gain of the engine handing over.
    pub fn start_at(&mut self, gain_db: f32) {
        self.gain_db = gain_db.clamp(-self.params.max_gain_db, self.params.max_gain_db);
        self.gain = db_to_gain(self.gain_db);
        self.gain_target = self.gain;
        self.step = 0.0;
    }

    /// Once per block of 100 ms: the gain moves towards the target.
    fn update(&mut self) {
        let max = self.params.max_gain_db;
        let held = self.meter.filled() < MOMENTARY_BLOCKS
            || self.meter.loudness(MOMENTARY_BLOCKS) < self.params.gate_lufs;
        let wanted = if held {
            // Holds, but comes back within a lowered maximum.
            self.gain_db.clamp(-max, max)
        } else {
            (self.params.target_lufs - self.meter.loudness(SHORT_TERM_BLOCKS)).clamp(-max, max)
        };
        let share = if wanted < self.gain_db {
            self.down_share
        } else {
            self.up_share
        };
        self.gain_db += (wanted - self.gain_db) * share;
        self.gain = self.gain_target;
        self.gain_target = db_to_gain(self.gain_db);
        self.step = (self.gain_target - self.gain) / self.meter.block_frames() as f32;
    }
}

impl Block for GatedAgc {
    fn process(&mut self, samples: &mut [f32]) {
        for frame in samples.as_chunks_mut::<CHANNELS>().0 {
            if self.meter.add(*frame) {
                self.update();
            }
            self.gain += self.step;
            frame[0] *= self.gain;
            frame[1] *= self.gain;
        }
    }

    fn settle(&mut self) {
        self.start_at(self.gain_db);
    }

    fn reset(&mut self) {
        self.meter.reset();
        self.start_at(0.0);
    }
}
