// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AGC chain's settings as the mixer takes them: the stored settings
//! ([`AgcSettings`]) and the station's loudness target, which the gated AGC
//! aims for.

use heron_core::agc::{
    AgcSettings, BandMode, BandSettings, BassShape, DualBandSettings, FinalMode, FiveBandSettings,
};
use heron_dsp::agc as dsp;

/// The parameters of the chain for `settings`, the gated AGC aiming for
/// `target_lufs`.
pub fn chain_params(settings: &AgcSettings, target_lufs: f64) -> dsp::ChainParams {
    let agc = &settings.gated_agc;
    let expander = &settings.stereo_expander;
    let bass = &settings.bass_eq;
    let last = &settings.final_stage;
    dsp::ChainParams {
        bypass_all: settings.bypass_all,
        gated_agc: dsp::GatedAgcParams {
            enabled: agc.enabled,
            target_lufs: target_lufs as f32,
            gate_lufs: agc.gate_lufs,
            max_gain_db: agc.max_gain_db,
        },
        stereo_expander: dsp::StereoExpanderParams {
            enabled: expander.enabled,
            level: expander.level,
            depth: expander.depth,
            threshold_db: expander.threshold_db,
        },
        bass_eq: dsp::BassEqParams {
            enabled: bass.enabled,
            shape: match bass.shape {
                BassShape::Shelf => dsp::BassShape::Shelf,
                BassShape::Peak => dsp::BassShape::Peak,
            },
            frequency_hz: bass.frequency_hz,
            gain_db: bass.gain_db,
        },
        five_band: five_band(&settings.five_band),
        dual_band: dual_band(&settings.dual_band),
        final_stage: dsp::FinalStageParams {
            enabled: last.enabled,
            mode: match last.mode {
                FinalMode::Limiter => dsp::FinalMode::Limiter,
                FinalMode::Clipper => dsp::FinalMode::Clipper,
            },
            ceiling_db: last.ceiling_db,
            release_ms: last.release_ms,
        },
    }
}

fn band(band: &BandSettings) -> dsp::BandParams {
    dsp::BandParams {
        mode: match band.mode {
            BandMode::Compressor => dsp::BandMode::Compressor,
            BandMode::Expander => dsp::BandMode::Expander,
            BandMode::Limiter => dsp::BandMode::Limiter,
        },
        threshold_db: band.threshold_db,
        ratio: band.ratio,
        attack_ms: band.attack_ms,
        release_ms: band.release_ms,
        hold_ms: band.hold_ms,
        gain_db: band.gain_db,
    }
}

fn five_band(settings: &FiveBandSettings) -> dsp::FiveBandParams {
    dsp::FiveBandParams {
        enabled: settings.enabled,
        overdrive_db: settings.overdrive_db,
        crossovers_hz: settings.crossovers_hz,
        links: settings.links,
        bands: settings.bands.each_ref().map(band),
    }
}

fn dual_band(settings: &DualBandSettings) -> dsp::DualBandParams {
    dsp::DualBandParams {
        enabled: settings.enabled,
        crossover_hz: settings.crossover_hz,
        link: settings.link,
        bands: settings.bands.each_ref().map(band),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_chains_defaults() {
        let params = chain_params(&AgcSettings::default(), -16.0);
        assert_eq!(params, dsp::ChainParams::default());
    }

    #[test]
    fn the_agc_aims_for_the_loudness_target() {
        let params = chain_params(&AgcSettings::default(), -14.0);
        assert_eq!(params.gated_agc.target_lufs, -14.0);
    }
}
