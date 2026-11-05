// SPDX-License-Identifier: AGPL-3.0-or-later

//! 2nd-order Butterworth filters as trapezoidal state-variable filters (their
//! state stays valid while the frequency moves), and the Linkwitz-Riley
//! crossover of the 4th order built from them.
//!
//! A Linkwitz-Riley crossover's low and high bands add up to a 2nd-order
//! all-pass ([`Butterworth::all_pass`]): (s² − √2 s + 1) / (s² + √2 s + 1).
//! Each band is −6 dB at the crossover.

use std::f64::consts::{PI, SQRT_2};

use super::CHANNELS;

/// The two integrators of one filter on one channel.
pub(crate) type State = [f64; 2];

#[derive(Clone, Copy, Debug)]
pub(crate) struct Butterworth {
    a1: f64,
    a2: f64,
    a3: f64,
}

impl Butterworth {
    pub fn new(frequency: f64, sample_rate: f64) -> Self {
        let g = (PI * frequency / sample_rate).tan();
        let a1 = 1.0 / (1.0 + g * (g + SQRT_2));
        let a2 = g * a1;
        Self { a1, a2, a3: g * a2 }
    }

    /// Low-pass, band-pass and high-pass outputs.
    fn run(&self, state: &mut State, x: f64) -> (f64, f64, f64) {
        let v3 = x - state[1];
        let v1 = self.a1 * state[0] + self.a2 * v3;
        let v2 = state[1] + self.a2 * state[0] + self.a3 * v3;
        state[0] = 2.0 * v1 - state[0];
        state[1] = 2.0 * v2 - state[1];
        (v2, v1, x - SQRT_2 * v1 - v2)
    }

    pub fn high_pass(&self, state: &mut State, x: f64) -> f64 {
        self.run(state, x).2
    }

    /// The all-pass the two bands of the crossover add up to.
    pub fn all_pass(&self, state: &mut State, x: f64) -> f64 {
        x - 2.0 * SQRT_2 * self.run(state, x).1
    }
}

/// A Linkwitz-Riley crossover of the 4th order, on stereo.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Crossover {
    pub filter: Butterworth,
    /// The first stage, then the second low-pass and the second high-pass.
    state: [[State; CHANNELS]; 3],
}

impl Crossover {
    pub fn new(frequency: f64, sample_rate: f64) -> Self {
        Self {
            filter: Butterworth::new(frequency, sample_rate),
            state: [[[0.0; 2]; CHANNELS]; 3],
        }
    }

    pub fn tune(&mut self, frequency: f64, sample_rate: f64) {
        self.filter = Butterworth::new(frequency, sample_rate);
    }

    /// Splits a sample of `channel` into its low and high bands.
    pub fn split(&mut self, channel: usize, x: f64) -> (f64, f64) {
        let (low, _, high) = self.filter.run(&mut self.state[0][channel], x);
        let (low, _, _) = self.filter.run(&mut self.state[1][channel], low);
        let (_, _, high) = self.filter.run(&mut self.state[2][channel], high);
        (low, high)
    }

    pub fn state_mut(&mut self) -> &mut [f64] {
        self.state.as_flattened_mut().as_flattened_mut()
    }

    pub fn reset(&mut self) {
        self.state = [[[0.0; 2]; CHANNELS]; 3];
    }
}
