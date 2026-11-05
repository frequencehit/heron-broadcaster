// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings of the master AGC chain, stored under `agc` in each station's
//! settings and pushed to its engine. See
//! `docs/audio-engine.md#master-agc-chain`.

use serde::{Deserialize, Serialize};

/// The whole chain. Bypassed by default; turned on as it is, it is the final
/// limiter alone.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct AgcSettings {
    /// Skips the chain, whatever the blocks' switches say.
    pub bypass_all: bool,
    pub gated_agc: GatedAgcSettings,
    pub stereo_expander: StereoExpanderSettings,
    pub bass_eq: BassEqSettings,
    pub five_band: FiveBandSettings,
    pub dual_band: DualBandSettings,
    pub final_stage: FinalStageSettings,
}

impl Default for AgcSettings {
    fn default() -> Self {
        Self {
            bypass_all: true,
            gated_agc: GatedAgcSettings::default(),
            stereo_expander: StereoExpanderSettings::default(),
            bass_eq: BassEqSettings::default(),
            five_band: FiveBandSettings::default(),
            dual_band: DualBandSettings::default(),
            final_stage: FinalStageSettings::default(),
        }
    }
}

/// The gated AGC: a slow gain to the station's loudness target.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct GatedAgcSettings {
    pub enabled: bool,
    /// Under this momentary loudness (LUFS) the gain holds: −70 to −20.
    pub gate_lufs: f32,
    /// The gain stays within ± this, in dB: 0 to 18.
    pub max_gain_db: f32,
}

impl Default for GatedAgcSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            gate_lufs: -45.0,
            max_gain_db: 6.0,
        }
    }
}

/// The stereo expander: widens programmes close to mono.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct StereoExpanderSettings {
    pub enabled: bool,
    /// Most boost of the side, 0 to 100 % of 6 dB.
    pub level: f32,
    /// How much of the missing width is made up, 0 to 100 %.
    pub depth: f32,
    /// Programme level (dBFS) under which nothing is boosted: −60 to 0.
    pub threshold_db: f32,
}

impl Default for StereoExpanderSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            level: 50.0,
            depth: 50.0,
            threshold_db: -40.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum BassShape {
    #[default]
    Shelf,
    Peak,
}

/// The bass EQ: one shelf or peak.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct BassEqSettings {
    pub enabled: bool,
    pub shape: BassShape,
    /// 40 to 250 Hz.
    pub frequency_hz: f32,
    /// −12 to +12 dB.
    pub gain_db: f32,
}

impl Default for BassEqSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            shape: BassShape::Shelf,
            frequency_hz: 100.0,
            gain_db: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum BandMode {
    #[default]
    Compressor,
    Expander,
    Limiter,
}

/// One band of the 5-band or the dual-band processor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct BandSettings {
    pub mode: BandMode,
    /// −60 to 0 dBFS.
    pub threshold_db: f32,
    /// 1 to 20 (ignored by the limiter).
    pub ratio: f32,
    /// 0.5 to 200 ms.
    pub attack_ms: f32,
    /// 10 to 2,000 ms.
    pub release_ms: f32,
    /// 0 to 500 ms.
    pub hold_ms: f32,
    /// After the dynamics, −12 to +12 dB.
    pub gain_db: f32,
}

impl Default for BandSettings {
    fn default() -> Self {
        Self {
            mode: BandMode::Compressor,
            threshold_db: -20.0,
            ratio: 3.0,
            attack_ms: 20.0,
            release_ms: 250.0,
            hold_ms: 0.0,
            gain_db: 0.0,
        }
    }
}

/// The 5-band processor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct FiveBandSettings {
    pub enabled: bool,
    /// 0 to 12 dB.
    pub overdrive_db: f32,
    /// Rising, at least a third of an octave apart, 40 to 16,000 Hz.
    pub crossovers_hz: [f32; 4],
    /// `links[i]` links band `i` and band `i + 1`.
    pub links: [bool; 4],
    pub bands: [BandSettings; 5],
}

impl Default for FiveBandSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            overdrive_db: 0.0,
            crossovers_hz: [100.0, 400.0, 1_600.0, 6_400.0],
            links: [false; 4],
            bands: [BandSettings::default(); 5],
        }
    }
}

/// The dual-band processor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct DualBandSettings {
    pub enabled: bool,
    /// 60 to 2,000 Hz.
    pub crossover_hz: f32,
    pub link: bool,
    /// Low, then high.
    pub bands: [BandSettings; 2],
}

impl Default for DualBandSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            crossover_hz: 200.0,
            link: false,
            bands: [BandSettings::default(); 2],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FinalMode {
    /// Look-ahead, 5 ms of delay: clean.
    #[default]
    Limiter,
    /// No delay: louder, and it distorts when driven hard.
    Clipper,
}

/// The final stage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct FinalStageSettings {
    pub enabled: bool,
    pub mode: FinalMode,
    /// −6 to 0 dBFS.
    pub ceiling_db: f32,
    /// The limiter's, 10 to 1,000 ms.
    pub release_ms: f32,
}

impl Default for FinalStageSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: FinalMode::Limiter,
            ceiling_db: -1.0,
            release_ms: 100.0,
        }
    }
}

/// Crossovers are at least this far apart: a third of an octave.
pub const AGC_MIN_CROSSOVER_RATIO: f32 = 1.259_921;

/// `value` is within `low..=high` (and a number).
fn within(name: &str, value: f32, low: f32, high: f32) -> Result<(), String> {
    if (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(format!("{name} is from {low} to {high}"))
    }
}

impl AgcSettings {
    /// Name of the settings group.
    pub const SETTINGS_NAME: &str = "agc";

    /// Every value within its range; crossovers rising and apart.
    pub fn check(&self) -> Result<(), String> {
        let agc = &self.gated_agc;
        within("gated_agc.gate_lufs", agc.gate_lufs, -70.0, -20.0)?;
        within("gated_agc.max_gain_db", agc.max_gain_db, 0.0, 18.0)?;
        let expander = &self.stereo_expander;
        within("stereo_expander.level", expander.level, 0.0, 100.0)?;
        within("stereo_expander.depth", expander.depth, 0.0, 100.0)?;
        within(
            "stereo_expander.threshold_db",
            expander.threshold_db,
            -60.0,
            0.0,
        )?;
        within(
            "bass_eq.frequency_hz",
            self.bass_eq.frequency_hz,
            40.0,
            250.0,
        )?;
        within("bass_eq.gain_db", self.bass_eq.gain_db, -12.0, 12.0)?;
        let five = &self.five_band;
        within("five_band.overdrive_db", five.overdrive_db, 0.0, 12.0)?;
        for (index, &frequency) in five.crossovers_hz.iter().enumerate() {
            within(
                &format!("five_band.crossovers_hz[{index}]"),
                frequency,
                40.0,
                16_000.0,
            )?;
        }
        if five
            .crossovers_hz
            .windows(2)
            .any(|pair| pair[1] < pair[0] * AGC_MIN_CROSSOVER_RATIO)
        {
            return Err(
                "five_band.crossovers_hz rise, at least a third of an octave apart".to_owned(),
            );
        }
        for (index, band) in five.bands.iter().enumerate() {
            band.check(&format!("five_band.bands[{index}]"))?;
        }
        within(
            "dual_band.crossover_hz",
            self.dual_band.crossover_hz,
            60.0,
            2_000.0,
        )?;
        for (index, band) in self.dual_band.bands.iter().enumerate() {
            band.check(&format!("dual_band.bands[{index}]"))?;
        }
        within(
            "final_stage.ceiling_db",
            self.final_stage.ceiling_db,
            -6.0,
            0.0,
        )?;
        within(
            "final_stage.release_ms",
            self.final_stage.release_ms,
            10.0,
            1_000.0,
        )
    }

    /// Some block changes the sound: the chain is on with a block on.
    pub fn is_processing(&self) -> bool {
        !self.bypass_all
            && (self.gated_agc.enabled
                || self.stereo_expander.enabled
                || self.bass_eq.enabled
                || self.five_band.enabled
                || self.dual_band.enabled
                || self.final_stage.enabled)
    }
}

impl BandSettings {
    fn check(&self, name: &str) -> Result<(), String> {
        within(
            &format!("{name}.threshold_db"),
            self.threshold_db,
            -60.0,
            0.0,
        )?;
        within(&format!("{name}.ratio"), self.ratio, 1.0, 20.0)?;
        within(&format!("{name}.attack_ms"), self.attack_ms, 0.5, 200.0)?;
        within(
            &format!("{name}.release_ms"),
            self.release_ms,
            10.0,
            2_000.0,
        )?;
        within(&format!("{name}.hold_ms"), self.hold_ms, 0.0, 500.0)?;
        within(&format!("{name}.gain_db"), self.gain_db, -12.0, 12.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_bypassed_with_the_final_limiter_on() {
        let settings: AgcSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, AgcSettings::default());
        assert!(settings.bypass_all);
        assert!(settings.final_stage.enabled);
        assert!(!settings.five_band.enabled);
        assert_eq!(settings.check(), Ok(()));
        assert!(!settings.is_processing());
    }

    #[test]
    fn values_are_checked() {
        let mut settings = AgcSettings::default();
        settings.five_band.bands[2].ratio = 0.5;
        assert_eq!(
            settings.check(),
            Err("five_band.bands[2].ratio is from 1 to 20".to_owned())
        );
        settings.five_band.bands[2].ratio = f32::NAN;
        assert!(settings.check().is_err());
        settings.five_band.bands[2].ratio = 20.0;
        assert_eq!(settings.check(), Ok(()));
        settings.final_stage.ceiling_db = 0.5;
        assert!(settings.check().is_err());
    }

    #[test]
    fn crossovers_rise_and_stay_apart() {
        let mut settings = AgcSettings::default();
        settings.five_band.crossovers_hz = [100.0, 400.0, 300.0, 6_400.0];
        assert!(settings.check().is_err());
        settings.five_band.crossovers_hz = [100.0, 120.0, 1_600.0, 6_400.0];
        assert!(settings.check().is_err());
        settings.five_band.crossovers_hz = [100.0, 126.0, 1_600.0, 6_400.0];
        assert_eq!(settings.check(), Ok(()));
    }

    #[test]
    fn modes_and_shapes_are_words() {
        let json = r#"{"bypass_all": false, "bass_eq": {"shape": "peak"},
            "dual_band": {"bands": [{"mode": "limiter"}, {"mode": "expander"}]},
            "final_stage": {"mode": "clipper"}}"#;
        let settings: AgcSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.bass_eq.shape, BassShape::Peak);
        assert_eq!(settings.dual_band.bands[1].mode, BandMode::Expander);
        assert_eq!(settings.final_stage.mode, FinalMode::Clipper);
        assert!(settings.is_processing());
        assert!(serde_json::from_str::<AgcSettings>(r#"{"five_band": {"bands": [{}]}}"#).is_err());
        assert!(serde_json::from_str::<AgcSettings>(r#"{"bypass": true}"#).is_err());
    }
}
