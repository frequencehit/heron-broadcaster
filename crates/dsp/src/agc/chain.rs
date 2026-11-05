// SPDX-License-Identifier: AGPL-3.0-or-later

//! The chain: the blocks in order, each with its switch, and a switch for the
//! whole chain. Switching crossfades over 50 ms. A block that is off is not
//! computed; a chain that is bypassed, or whose blocks are all off, leaves the
//! audio as it is, bit for bit.

use super::{
    BassEq, BassEqParams, Block, CHANNELS, DualBandParams, FinalStage, FinalStageParams,
    FiveBandParams, GatedAgc, GatedAgcParams, Multiband, StereoExpander, StereoExpanderParams,
};

/// Frames processed at a time: the size of the dry copies kept for
/// crossfades, on the stack.
const CHUNK_FRAMES: usize = 64;
/// How long switching a block or the chain crossfades.
const SWITCH_SECONDS: f64 = 0.05;

/// Settings of the whole chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainParams {
    /// Skips the chain, whatever its blocks' switches say.
    pub bypass_all: bool,
    pub gated_agc: GatedAgcParams,
    pub stereo_expander: StereoExpanderParams,
    pub bass_eq: BassEqParams,
    pub five_band: FiveBandParams,
    pub dual_band: DualBandParams,
    pub final_stage: FinalStageParams,
}

impl Default for ChainParams {
    fn default() -> Self {
        Self {
            bypass_all: true,
            gated_agc: GatedAgcParams::default(),
            stereo_expander: StereoExpanderParams::default(),
            bass_eq: BassEqParams::default(),
            five_band: FiveBandParams::default(),
            dual_band: DualBandParams::default(),
            final_stage: FinalStageParams::default(),
        }
    }
}

/// What the chain did since the meters were last read: the gated AGC's gain
/// (`None` while it is off), and the deepest gain reduction of each band and
/// of the final stage, in dB (0 or less).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChainMeters {
    pub agc_gain_db: Option<f32>,
    pub five_band_db: [f32; 5],
    pub dual_band_db: [f32; 2],
    pub final_stage_db: f32,
}

/// A block and its switch.
struct Slot<B> {
    block: B,
    enabled: bool,
    /// 0 off, 1 on; between, a crossfade.
    mix: f32,
}

impl<B: Block> Slot<B> {
    fn new(block: B) -> Self {
        Self {
            block,
            enabled: false,
            mix: 0.0,
        }
    }

    fn is_on(&self) -> bool {
        self.mix > 0.0
    }

    fn run(&mut self, samples: &mut [f32], step: f32) {
        let target = if self.enabled { 1.0 } else { 0.0 };
        if self.mix == target {
            if self.enabled {
                self.block.process(samples);
            }
            return;
        }
        let mut dry = [0.0f32; CHUNK_FRAMES * CHANNELS];
        let dry = &mut dry[..samples.len()];
        dry.copy_from_slice(samples);
        self.block.process(samples);
        self.mix = crossfade(samples, dry, self.mix, target, step);
        if self.mix == 0.0 {
            self.block.reset();
        }
    }

    /// At its settings and its switch's position at once, the sound kept.
    #[cfg(test)]
    fn settle(&mut self) {
        self.block.settle();
        self.mix = if self.enabled { 1.0 } else { 0.0 };
    }

    /// Back to rest, at its switch's position.
    fn restart(&mut self) {
        self.block.reset();
        self.mix = if self.enabled { 1.0 } else { 0.0 };
    }
}

/// Mixes `wet` with `dry` frame by frame as `mix` moves to `target` by `step`;
/// returns where `mix` got to.
fn crossfade(wet: &mut [f32], dry: &[f32], mut mix: f32, target: f32, step: f32) -> f32 {
    for (wet, dry) in wet
        .as_chunks_mut::<CHANNELS>()
        .0
        .iter_mut()
        .zip(dry.as_chunks::<CHANNELS>().0)
    {
        mix = if target > mix {
            (mix + step).min(target)
        } else {
            (mix - step).max(target)
        };
        for (wet, dry) in wet.iter_mut().zip(dry) {
            *wet = dry + (*wet - dry) * mix;
        }
    }
    mix
}

/// The master AGC chain, on interleaved stereo.
pub struct Chain {
    switch_step: f32,
    bypass_all: bool,
    /// 0 bypassed, 1 on; between, a crossfade.
    mix: f32,
    agc: Slot<GatedAgc>,
    expander: Slot<StereoExpander>,
    bass: Slot<BassEq>,
    five: Slot<Multiband>,
    dual: Slot<Multiband>,
    last: Slot<FinalStage>,
}

impl Chain {
    pub fn new(sample_rate: u32) -> Self {
        let mut chain = Self {
            switch_step: (1.0 / (SWITCH_SECONDS * f64::from(sample_rate))) as f32,
            bypass_all: true,
            mix: 0.0,
            agc: Slot::new(GatedAgc::new(sample_rate)),
            expander: Slot::new(StereoExpander::new(sample_rate)),
            bass: Slot::new(BassEq::new(sample_rate)),
            five: Slot::new(Multiband::new(sample_rate, 5)),
            dual: Slot::new(Multiband::new(sample_rate, 2)),
            last: Slot::new(FinalStage::new(sample_rate)),
        };
        chain.restart(&ChainParams::default());
        chain
    }

    /// New settings, glided to.
    pub fn set(&mut self, params: &ChainParams) {
        self.bypass_all = params.bypass_all;
        self.agc.enabled = params.gated_agc.enabled;
        self.agc.block.set(&params.gated_agc);
        self.expander.enabled = params.stereo_expander.enabled;
        self.expander.block.set(&params.stereo_expander);
        self.bass.enabled = params.bass_eq.enabled;
        self.bass.block.set(&params.bass_eq);
        self.five.enabled = params.five_band.enabled;
        self.five.block.set_five(&params.five_band);
        self.dual.enabled = params.dual_band.enabled;
        self.dual.block.set_dual(&params.dual_band);
        self.last.enabled = params.final_stage.enabled;
        self.last.block.set(&params.final_stage);
    }

    /// New settings, there at once, from rest: before the first sample of an
    /// engine.
    pub fn restart(&mut self, params: &ChainParams) {
        self.set(params);
        self.agc.restart();
        self.expander.restart();
        self.bass.restart();
        self.five.restart();
        self.dual.restart();
        self.last.restart();
        self.mix = if self.bypass_all { 0.0 } else { 1.0 };
    }

    /// New settings at once, without gliding nor crossfading: what the
    /// glides avoid, for the tests to compare with.
    #[cfg(test)]
    pub(crate) fn set_at_once(&mut self, params: &ChainParams) {
        self.set(params);
        self.agc.settle();
        self.expander.settle();
        self.bass.settle();
        self.five.settle();
        self.dual.settle();
        self.last.settle();
        self.mix = if self.bypass_all { 0.0 } else { 1.0 };
    }

    /// The chain is computed: on, or crossfading to its bypass.
    pub fn is_active(&self) -> bool {
        !self.bypass_all || self.mix > 0.0
    }

    /// Frames by which the chain delays the audio now.
    pub fn latency(&self) -> usize {
        if self.is_active() && self.last.is_on() {
            self.last.block.latency()
        } else {
            0
        }
    }

    /// The gated AGC's gain, while it is on: what a new engine taking over
    /// starts from.
    pub fn agc_gain_db(&self) -> Option<f32> {
        (self.is_active() && self.agc.is_on()).then(|| self.agc.block.gain_db())
    }

    /// Starts the gated AGC from this gain (after [`Chain::restart`]).
    pub fn start_agc_at(&mut self, gain_db: f32) {
        self.agc.block.start_at(gain_db);
    }

    /// Processes interleaved stereo in place; nothing while bypassed.
    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.is_active() {
            return;
        }
        let whole = samples.len() / CHANNELS * CHANNELS;
        let step = self.switch_step;
        let target = if self.bypass_all { 0.0 } else { 1.0 };
        for chunk in samples[..whole].chunks_mut(CHUNK_FRAMES * CHANNELS) {
            if !self.is_active() {
                // Bypassed within this call: the rest goes out untouched.
                break;
            }
            let mut dry = [0.0f32; CHUNK_FRAMES * CHANNELS];
            let dry = &mut dry[..chunk.len()];
            let fading = self.mix != target;
            if fading {
                dry.copy_from_slice(chunk);
            }
            self.agc.run(chunk, step);
            self.expander.run(chunk, step);
            self.bass.run(chunk, step);
            self.five.run(chunk, step);
            self.dual.run(chunk, step);
            self.last.run(chunk, step);
            if fading {
                self.mix = crossfade(chunk, dry, self.mix, target, step);
            }
        }
        if !self.is_active() {
            // Bypassed: from rest when it comes back.
            self.agc.restart();
            self.expander.restart();
            self.bass.restart();
            self.five.restart();
            self.dual.restart();
            self.last.restart();
        }
    }

    /// What the chain did since the last call.
    pub fn take_meters(&mut self) -> ChainMeters {
        let five = self.five.block.take_deepest_db();
        let dual = self.dual.block.take_deepest_db();
        let active = self.is_active();
        let on = |slot_on: bool| active && slot_on;
        ChainMeters {
            agc_gain_db: self.agc_gain_db(),
            five_band_db: if on(self.five.is_on()) {
                five
            } else {
                [0.0; 5]
            },
            dual_band_db: if on(self.dual.is_on()) {
                [dual[0], dual[1]]
            } else {
                [0.0; 2]
            },
            final_stage_db: {
                let lowest = self.last.block.take_lowest_db();
                if on(self.last.is_on()) { lowest } else { 0.0 }
            },
        }
    }
}
