// SPDX-License-Identifier: AGPL-3.0-or-later

//! The final stage: a look-ahead limiter or a clipper, at a ceiling.
//!
//! The limiter knows 5 ms ahead the gain each frame needs to stay under the
//! ceiling. The gain applied is the lowest needed over a window of L frames,
//! averaged over L frames, on audio delayed by L − 1 frames: every frame of
//! the window holding the needed gain of the frame now coming out is at or
//! under it, so their average is too, and the gain glides to each peak instead
//! of stepping. It then comes back with the release.
//!
//! The clipper clips at the ceiling, without delay. The limiter runs in both
//! modes, so switching between them crossfades two ready paths. A hard clip
//! at the ceiling follows both: it only catches rounding. The delayed audio
//! carries the ceiling it was limited for, so a ceiling coming down clips
//! nothing on its way.

use super::{Block, CHANNELS, GLIDE_SECONDS, Smoothed, db_to_gain, share};

/// Look-ahead.
const LOOKAHEAD_SECONDS: f64 = 0.005;
/// How long a change of mode crossfades.
const MODE_SECONDS: f64 = 0.05;
pub const MIN_CEILING_DB: f32 = -6.0;
pub const MIN_RELEASE_MS: f32 = 10.0;
pub const MAX_RELEASE_MS: f32 = 1_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FinalMode {
    #[default]
    Limiter,
    Clipper,
}

/// Settings of the final stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinalStageParams {
    pub enabled: bool,
    pub mode: FinalMode,
    pub ceiling_db: f32,
    pub release_ms: f32,
}

impl Default for FinalStageParams {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: FinalMode::Limiter,
            ceiling_db: -1.0,
            release_ms: 100.0,
        }
    }
}

/// The lowest of the last `window` values: a queue of the values that may
/// still be the lowest, rising, with the frame they came at. Fixed capacity.
struct MovingMinimum {
    values: Vec<(u64, f32)>,
    head: usize,
    len: usize,
}

impl MovingMinimum {
    fn new(window: usize) -> Self {
        Self {
            values: vec![(0, 1.0); window + 1],
            head: 0,
            len: 0,
        }
    }

    fn index(&self, offset: usize) -> usize {
        (self.head + offset) % self.values.len()
    }

    fn push(&mut self, time: u64, value: f32, window: usize) -> f32 {
        while self.len > 0 && self.values[self.index(self.len - 1)].1 >= value {
            self.len -= 1;
        }
        let back = self.index(self.len);
        self.values[back] = (time, value);
        self.len += 1;
        while self.values[self.head].0 + window as u64 <= time {
            self.head = self.index(1);
            self.len -= 1;
        }
        self.values[self.head].1
    }

    fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

pub struct FinalStage {
    sample_rate: f64,
    mode: FinalMode,
    /// 0 for the limiter, 1 for the clipper; between, a crossfade.
    clipper_mix: f32,
    mode_step: f32,
    ceiling: Smoothed,
    glide_share: f32,
    release_share: f32,
    window: usize,
    time: u64,
    minimum: MovingMinimum,
    /// The last `window` minima, and their sum.
    minima: Vec<f32>,
    minima_at: usize,
    sum: f64,
    gain: f32,
    /// The audio, `window − 1` frames behind, with the ceiling then.
    delay: Vec<([f32; CHANNELS], f32)>,
    delay_at: usize,
    /// The lowest gain since the meters were read.
    lowest: f32,
}

impl FinalStage {
    pub fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        let window = ((LOOKAHEAD_SECONDS * rate).round() as usize).max(2);
        let defaults = FinalStageParams::default();
        Self {
            sample_rate: rate,
            mode: defaults.mode,
            clipper_mix: 0.0,
            mode_step: (1.0 / (MODE_SECONDS * rate)) as f32,
            ceiling: Smoothed::new(db_to_gain(defaults.ceiling_db)),
            glide_share: share(GLIDE_SECONDS, 1, rate),
            release_share: share(f64::from(defaults.release_ms) / 1000.0, 1, rate),
            window,
            time: 0,
            minimum: MovingMinimum::new(window),
            minima: vec![1.0; window],
            minima_at: 0,
            sum: window as f64,
            gain: 1.0,
            delay: vec![([0.0; CHANNELS], 1.0); window - 1],
            delay_at: 0,
            lowest: 1.0,
        }
    }

    pub fn set(&mut self, params: &FinalStageParams) {
        self.mode = params.mode;
        self.ceiling.target = db_to_gain(params.ceiling_db.clamp(MIN_CEILING_DB, 0.0));
        let release = params.release_ms.clamp(MIN_RELEASE_MS, MAX_RELEASE_MS);
        self.release_share = share(f64::from(release) / 1000.0, 1, self.sample_rate);
    }

    /// Frames of delay: the limiter's, none once the clipper alone is heard.
    pub fn latency(&self) -> usize {
        if self.clipper_mix >= 1.0 {
            0
        } else {
            self.window - 1
        }
    }

    /// The lowest gain since the last call, in dB (0 or less).
    pub fn take_lowest_db(&mut self) -> f32 {
        super::gain_to_db(std::mem::replace(&mut self.lowest, 1.0)).min(0.0)
    }

    /// The limiter's gain for this frame, its needed gain coming in now.
    fn limiter_gain(&mut self, need: f32) -> f32 {
        let minimum = self.minimum.push(self.time, need, self.window);
        self.time += 1;
        self.sum += f64::from(minimum) - f64::from(self.minima[self.minima_at]);
        self.minima[self.minima_at] = minimum;
        self.minima_at += 1;
        if self.minima_at == self.window {
            self.minima_at = 0;
            // No drift from the running sum.
            self.sum = self.minima.iter().copied().map(f64::from).sum();
        }
        let average = (self.sum / self.window as f64) as f32;
        let released = self.gain + (1.0 - self.gain) * self.release_share;
        self.gain = average.min(released);
        self.gain
    }
}

impl Block for FinalStage {
    fn process(&mut self, samples: &mut [f32]) {
        let target = match self.mode {
            FinalMode::Limiter => 0.0,
            FinalMode::Clipper => 1.0,
        };
        for frame in samples.as_chunks_mut::<CHANNELS>().0 {
            let ceiling = self.ceiling.step(self.glide_share);
            let peak = frame[0].abs().max(frame[1].abs());
            let need = if peak > ceiling { ceiling / peak } else { 1.0 };
            let gain = self.limiter_gain(need);
            let (delayed, delayed_ceiling) =
                std::mem::replace(&mut self.delay[self.delay_at], (*frame, ceiling));
            self.delay_at = (self.delay_at + 1) % self.delay.len();
            if self.clipper_mix != target {
                self.clipper_mix = if target > self.clipper_mix {
                    (self.clipper_mix + self.mode_step).min(1.0)
                } else {
                    (self.clipper_mix - self.mode_step).max(0.0)
                };
            }
            let mix = self.clipper_mix;
            if mix < 1.0 {
                self.lowest = self.lowest.min(gain);
            }
            for (sample, delayed) in frame.iter_mut().zip(delayed) {
                let limited = (delayed * gain).clamp(-delayed_ceiling, delayed_ceiling);
                let clipped = sample.clamp(-ceiling, ceiling);
                *sample = if mix == 0.0 {
                    limited
                } else if mix == 1.0 {
                    clipped
                } else {
                    limited + (clipped - limited) * mix
                };
            }
            if mix > 0.0 && peak > ceiling {
                self.lowest = self.lowest.min(ceiling / peak);
            }
        }
    }

    fn settle(&mut self) {
        self.clipper_mix = match self.mode {
            FinalMode::Limiter => 0.0,
            FinalMode::Clipper => 1.0,
        };
        self.ceiling.settle();
    }

    fn reset(&mut self) {
        self.settle();
        self.time = 0;
        self.minimum.clear();
        self.minima.fill(1.0);
        self.minima_at = 0;
        self.sum = self.window as f64;
        self.gain = 1.0;
        self.delay.fill(([0.0; CHANNELS], self.ceiling.value));
        self.delay_at = 0;
        self.lowest = 1.0;
    }
}
