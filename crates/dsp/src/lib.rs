// SPDX-License-Identifier: AGPL-3.0-or-later

//! Audio processing: resampling, the deck EQ (M9) and the master AGC chain
//! (M10).

pub mod agc;
pub mod eq;
pub mod resample;

pub use agc::Chain;
pub use eq::GraphicEq;
pub use resample::StreamResampler;
