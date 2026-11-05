// SPDX-License-Identifier: AGPL-3.0-or-later

//! Playout settings (crossfading, loudness normalization) and the planning of
//! transitions between two tracks.
//!
//! Everything here is a pure function of the analysis, the overrides and the
//! settings, so transitions are deterministic and the UI can preview them.
//! See `docs/requirements.md#2-playout`.

use serde::{Deserialize, Serialize};

/// Shape of a fade.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FadeCurve {
    #[default]
    Linear,
    /// Raised cosine: gentle at both ends.
    SCurve,
    /// Rises fast, then levels off (a fade-out holds, then drops).
    Logarithmic,
    /// Rises slowly, then fast (a fade-out drops, then trails off).
    Exponential,
}

impl FadeCurve {
    /// Gain of a fade-in at `progress` (0 to 1): 0 at the start, 1 at the end.
    pub fn fade_in(self, progress: f32) -> f32 {
        let x = progress.clamp(0.0, 1.0);
        match self {
            Self::Linear => x,
            Self::SCurve => 0.5 - 0.5 * (std::f32::consts::PI * x).cos(),
            Self::Logarithmic => (1.0 + 9.0 * x).log10(),
            Self::Exponential => (10f32.powf(x) - 1.0) / 9.0,
        }
    }
}

/// One side of a crossfade.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Fade {
    pub enabled: bool,
    pub curve: FadeCurve,
    pub time_ms: u32,
    /// Level the fade-out goes down to, or the fade-in starts from, in percent.
    pub level_pct: u8,
}

impl Default for Fade {
    fn default() -> Self {
        Self {
            enabled: true,
            curve: FadeCurve::Linear,
            time_ms: 2_500,
            level_pct: 0,
        }
    }
}

/// Where the next track starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum CrossfadeMode {
    /// At the crossfade point found by the analysis (dB trigger, min and max
    /// fade time, see `AnalysisSettings`).
    #[default]
    Auto,
    /// A fixed time before the cue-out.
    Fixed,
}

/// Station-wide crossfade settings, modeled on SAM Broadcaster's
/// "Cross-Fading" screen. Stored under `playout` in the `settings` table.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct CrossfadeSettings {
    /// Fade-out of the outgoing track, from the moment the next one starts.
    pub fade_out: Fade,
    /// Fade-in of the incoming track.
    pub fade_in: Fade,
    pub mode: CrossfadeMode,
    /// With [`CrossfadeMode::Fixed`], how long before the cue-out the next track starts.
    pub fixed_ms: u32,
    /// Tracks this long or shorter (jingles, IDs) are never crossfaded: they
    /// start right after the previous track and the next starts right after them.
    pub min_track_ms: u32,
    /// Fade-out of the current track when it is skipped.
    pub skip_fade_ms: u32,
}

impl Default for CrossfadeSettings {
    fn default() -> Self {
        Self {
            fade_out: Fade::default(),
            fade_in: Fade {
                enabled: false,
                time_ms: 1_000,
                ..Fade::default()
            },
            mode: CrossfadeMode::Auto,
            fixed_ms: 2_000,
            min_track_ms: 20_000,
            skip_fade_ms: 1_000,
        }
    }
}

/// Per-track crossfade overrides (`track_overrides.crossfade`). Missing fields
/// use the station settings. The fade-out, mode and fixed time apply to the
/// transition out of the track; the fade-in to the transition into it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct CrossfadeOverrides {
    pub fade_out: Option<Fade>,
    pub fade_in: Option<Fade>,
    pub mode: Option<CrossfadeMode>,
    pub fixed_ms: Option<u32>,
    /// `false` plays the transition out of this track back to back, without overlap.
    pub crossfade: Option<bool>,
}

/// Loudness normalization from the analyzed integrated loudness (EBU R128).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct LoudnessSettings {
    pub enabled: bool,
    pub target_lufs: f64,
    /// Quiet tracks are boosted by at most this much.
    pub max_gain_db: f64,
    /// Boosts never push the track's true peak above this. There is no
    /// limiter before M10, so a boost that clips would distort.
    pub peak_ceiling_dbtp: f64,
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            target_lufs: -16.0,
            max_gain_db: 12.0,
            peak_ceiling_dbtp: -1.0,
        }
    }
}

/// The engine's silence watchdog: after this long of silence on the master
/// bus (below −50 dBFS), it skips to the next track, then to the fallback
/// directory if the silence goes on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct SilenceWatchdogSettings {
    pub enabled: bool,
    /// Longer than the analysis' longest silence inside a track (15 s), so
    /// it never cuts a track the analysis kept whole.
    pub timeout_ms: u32,
}

impl Default for SilenceWatchdogSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 20_000,
        }
    }
}

/// Playout settings, stored under `playout` in the `settings` table and
/// pushed to the engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct PlayoutSettings {
    pub crossfade: CrossfadeSettings,
    pub loudness: LoudnessSettings,
    pub silence_watchdog: SilenceWatchdogSettings,
}

impl PlayoutSettings {
    /// Name of the row in the `settings` table.
    pub const SETTINGS_NAME: &str = "playout";
}

/// Per-track overrides (`track_overrides`). Missing fields use the analysis
/// and station settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct TrackOverrides {
    pub cue_in_ms: Option<u32>,
    pub cue_out_ms: Option<u32>,
    pub gap_killer_enabled: Option<bool>,
    pub crossfade: CrossfadeOverrides,
}

/// Why a track is played (`play_history.source`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PlaySource {
    /// Picked by the rotation (the queue manager's autofill until M6).
    Rotation,
    /// A listener request.
    Request,
    /// From the fallback directory, when the queue ran dry.
    Fallback,
    /// Queued by hand.
    #[default]
    Manual,
}

/// What the engine needs to know about a track to play it. Times are in
/// milliseconds from the start of the decoded audio.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackPlayout {
    pub duration_ms: u32,
    pub cue_in_ms: u32,
    pub cue_out_ms: u32,
    /// Where the next track starts in [`CrossfadeMode::Auto`] (the analysis'
    /// `fade_out_start_ms`).
    pub mix_point_ms: Option<u32>,
    pub integrated_lufs: Option<f64>,
    pub true_peak_db: Option<f64>,
    #[serde(default)]
    pub crossfade: CrossfadeOverrides,
}

impl TrackPlayout {
    /// A track without analysis: played whole, back to back, at its own level.
    pub fn unanalyzed(duration_ms: u32) -> Self {
        Self {
            duration_ms,
            cue_in_ms: 0,
            cue_out_ms: duration_ms,
            mix_point_ms: None,
            integrated_lufs: None,
            true_peak_db: None,
            crossfade: CrossfadeOverrides::default(),
        }
    }

    /// Applies per-track overrides. With the Gap Killer off, the whole file
    /// plays; explicit cue points win over both.
    pub fn with_overrides(mut self, overrides: &TrackOverrides) -> Self {
        if overrides.gap_killer_enabled == Some(false) {
            self.cue_in_ms = 0;
            self.cue_out_ms = self.duration_ms;
        }
        if let Some(cue_in) = overrides.cue_in_ms {
            self.cue_in_ms = cue_in;
        }
        if let Some(cue_out) = overrides.cue_out_ms {
            self.cue_out_ms = cue_out;
        }
        self.cue_out_ms = self.cue_out_ms.min(self.duration_ms);
        self.cue_in_ms = self.cue_in_ms.min(self.cue_out_ms);
        self.crossfade = overrides.crossfade;
        self
    }

    /// Length of the part that is played.
    pub fn playable_ms(&self) -> u32 {
        self.cue_out_ms.saturating_sub(self.cue_in_ms)
    }

    /// Gain bringing the track to the target loudness, in dB.
    pub fn gain_db(&self, settings: &LoudnessSettings) -> f64 {
        let Some(lufs) = self.integrated_lufs.filter(|_| settings.enabled) else {
            return 0.0;
        };
        let gain = (settings.target_lufs - lufs).min(settings.max_gain_db);
        if gain <= 0.0 {
            return gain;
        }
        match self.true_peak_db {
            Some(peak) => gain.min((settings.peak_ceiling_dbtp - peak).max(0.0)),
            None => gain,
        }
    }
}

/// A gain ramp on one track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    /// Start of the ramp, in ms on the track's own timeline.
    pub start_ms: u32,
    pub length_ms: u32,
    pub curve: FadeCurve,
    /// Level at the quiet end of the ramp (0 to 1).
    pub level: f32,
}

impl Ramp {
    fn from_fade(fade: &Fade, start_ms: u32, length_ms: u32) -> Option<Self> {
        (fade.enabled && length_ms > 0).then(|| Self {
            start_ms,
            length_ms,
            curve: fade.curve,
            level: f32::from(fade.level_pct.min(100)) / 100.0,
        })
    }

    /// Gain of a fade-in at `progress` (0 to 1).
    pub fn fade_in_gain(&self, progress: f32) -> f32 {
        self.level + (1.0 - self.level) * self.curve.fade_in(progress)
    }

    /// Gain of a fade-out at `progress` (0 to 1).
    pub fn fade_out_gain(&self, progress: f32) -> f32 {
        self.level + (1.0 - self.level) * self.curve.fade_in(1.0 - progress)
    }
}

/// How one track hands over to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    /// When the incoming track starts, in ms on the outgoing track's timeline.
    /// Equal to the outgoing cue-out when the tracks play back to back.
    pub next_start_ms: u32,
    /// Fade-out of the outgoing track, on its timeline.
    pub fade_out: Option<Ramp>,
    /// Fade-in of the incoming track, on its timeline (starts at its cue-in).
    pub fade_in: Option<Ramp>,
}

impl Transition {
    /// How long both tracks play at once.
    pub fn overlap_ms(&self, outgoing: &TrackPlayout) -> u32 {
        outgoing.cue_out_ms.saturating_sub(self.next_start_ms)
    }
}

/// Plans the transition from `outgoing` to `incoming`.
pub fn plan_transition(
    outgoing: &TrackPlayout,
    incoming: &TrackPlayout,
    settings: &CrossfadeSettings,
) -> Transition {
    let back_to_back = Transition {
        next_start_ms: outgoing.cue_out_ms,
        fade_out: None,
        fade_in: None,
    };
    let overrides = &outgoing.crossfade;
    if overrides.crossfade == Some(false) {
        return back_to_back;
    }
    // A short track (a jingle) that sets where the next track starts keeps
    // that point, without fades: the tracks only overlap.
    if outgoing.playable_ms() <= settings.min_track_ms {
        let own_point = match (overrides.mode, overrides.fixed_ms) {
            (Some(CrossfadeMode::Fixed), Some(fixed_ms)) => {
                Some(outgoing.cue_out_ms.saturating_sub(fixed_ms))
            }
            _ => None,
        };
        return own_point
            .filter(|point| (outgoing.cue_in_ms..outgoing.cue_out_ms).contains(point))
            .map_or(back_to_back, |point| Transition {
                next_start_ms: point
                    .max(outgoing.cue_out_ms.saturating_sub(incoming.playable_ms())),
                fade_out: None,
                fade_in: None,
            });
    }
    if incoming.playable_ms() <= settings.min_track_ms {
        return back_to_back;
    }

    let mix_point = match overrides.mode.unwrap_or(settings.mode) {
        CrossfadeMode::Auto => outgoing.mix_point_ms,
        CrossfadeMode::Fixed => Some(
            outgoing
                .cue_out_ms
                .saturating_sub(overrides.fixed_ms.unwrap_or(settings.fixed_ms)),
        ),
    };
    // A mix point outside the played part (cue points overridden after the
    // analysis) is ignored rather than guessed.
    let Some(next_start_ms) = mix_point
        .filter(|point| (outgoing.cue_in_ms..outgoing.cue_out_ms).contains(point))
        .map(|point| point.max(outgoing.cue_out_ms.saturating_sub(incoming.playable_ms())))
    else {
        return back_to_back;
    };

    let overlap = outgoing.cue_out_ms - next_start_ms;
    let fade_out_settings = overrides.fade_out.unwrap_or(settings.fade_out);
    let fade_in_settings = incoming.crossfade.fade_in.unwrap_or(settings.fade_in);
    Transition {
        next_start_ms,
        fade_out: Ramp::from_fade(
            &fade_out_settings,
            next_start_ms,
            fade_out_settings.time_ms.min(overlap),
        ),
        fade_in: Ramp::from_fade(
            &fade_in_settings,
            incoming.cue_in_ms,
            fade_in_settings.time_ms.min(incoming.playable_ms()),
        ),
    }
}

/// Converts a gain in dB to a linear factor.
pub fn db_to_gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(cue_in_ms: u32, cue_out_ms: u32, mix_point_ms: Option<u32>) -> TrackPlayout {
        TrackPlayout {
            duration_ms: cue_out_ms + 500,
            cue_in_ms,
            cue_out_ms,
            mix_point_ms,
            integrated_lufs: Some(-14.0),
            true_peak_db: Some(-0.5),
            crossfade: CrossfadeOverrides::default(),
        }
    }

    #[test]
    fn curves_go_from_zero_to_one() {
        for curve in [
            FadeCurve::Linear,
            FadeCurve::SCurve,
            FadeCurve::Logarithmic,
            FadeCurve::Exponential,
        ] {
            assert!(curve.fade_in(0.0).abs() < 1e-6, "{curve:?}");
            assert!((curve.fade_in(1.0) - 1.0).abs() < 1e-6, "{curve:?}");
            let mut previous = 0.0;
            for step in 1..=100 {
                let gain = curve.fade_in(step as f32 / 100.0);
                assert!(gain >= previous, "{curve:?} is not monotonic");
                previous = gain;
            }
        }
        assert!(FadeCurve::Logarithmic.fade_in(0.5) > 0.5);
        assert!(FadeCurve::Exponential.fade_in(0.5) < 0.5);
    }

    #[test]
    fn auto_crossfade_starts_at_the_analyzed_mix_point() {
        let settings = CrossfadeSettings::default();
        let outgoing = track(1_000, 180_000, Some(178_200));
        let incoming = track(300, 200_000, Some(198_000));
        let transition = plan_transition(&outgoing, &incoming, &settings);
        assert_eq!(transition.next_start_ms, 178_200);
        assert_eq!(transition.overlap_ms(&outgoing), 1_800);
        let fade_out = transition.fade_out.unwrap();
        assert_eq!((fade_out.start_ms, fade_out.length_ms), (178_200, 1_800));
        assert_eq!(transition.fade_in, None);
    }

    #[test]
    fn fixed_crossfade_starts_before_the_cue_out() {
        let settings = CrossfadeSettings {
            mode: CrossfadeMode::Fixed,
            fixed_ms: 4_000,
            fade_in: Fade {
                enabled: true,
                time_ms: 1_500,
                level_pct: 20,
                ..Fade::default()
            },
            ..CrossfadeSettings::default()
        };
        let outgoing = track(0, 120_000, Some(119_000));
        let incoming = track(250, 90_000, None);
        let transition = plan_transition(&outgoing, &incoming, &settings);
        assert_eq!(transition.next_start_ms, 116_000);
        assert_eq!(transition.fade_out.unwrap().length_ms, 2_500);
        let fade_in = transition.fade_in.unwrap();
        assert_eq!((fade_in.start_ms, fade_in.length_ms), (250, 1_500));
        assert!((fade_in.fade_in_gain(0.0) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn short_tracks_play_back_to_back() {
        let settings = CrossfadeSettings::default();
        let song = track(0, 180_000, Some(178_000));
        let jingle = track(100, 8_000, Some(7_000));
        for (outgoing, incoming) in [(&song, &jingle), (&jingle, &song)] {
            let transition = plan_transition(outgoing, incoming, &settings);
            assert_eq!(transition.next_start_ms, outgoing.cue_out_ms);
            assert_eq!(transition.fade_out, None);
            assert_eq!(transition.fade_in, None);
        }
    }

    #[test]
    fn per_track_overrides_win() {
        let settings = CrossfadeSettings::default();
        let mut outgoing = track(0, 180_000, Some(178_000));
        let incoming = track(0, 180_000, None);
        outgoing.crossfade.crossfade = Some(false);
        assert_eq!(
            plan_transition(&outgoing, &incoming, &settings).next_start_ms,
            180_000
        );

        outgoing.crossfade = CrossfadeOverrides {
            mode: Some(CrossfadeMode::Fixed),
            fixed_ms: Some(6_000),
            ..CrossfadeOverrides::default()
        };
        assert_eq!(
            plan_transition(&outgoing, &incoming, &settings).next_start_ms,
            174_000
        );
    }

    #[test]
    fn missing_or_stale_mix_point_plays_back_to_back() {
        let settings = CrossfadeSettings::default();
        let incoming = track(0, 180_000, None);
        let no_point = track(0, 180_000, None);
        assert_eq!(
            plan_transition(&no_point, &incoming, &settings).next_start_ms,
            180_000
        );
        let overrides = TrackOverrides {
            cue_out_ms: Some(150_000),
            ..TrackOverrides::default()
        };
        let cut = track(0, 180_000, Some(178_000)).with_overrides(&overrides);
        assert_eq!(
            plan_transition(&cut, &incoming, &settings).next_start_ms,
            150_000
        );
    }

    #[test]
    fn gap_killer_override_plays_the_whole_file() {
        let overrides = TrackOverrides {
            gap_killer_enabled: Some(false),
            ..TrackOverrides::default()
        };
        let playout = track(1_200, 170_000, Some(168_000)).with_overrides(&overrides);
        assert_eq!((playout.cue_in_ms, playout.cue_out_ms), (0, 170_500));
    }

    #[test]
    fn loudness_gain_reaches_the_target() {
        let settings = LoudnessSettings::default();
        let loud = TrackPlayout {
            integrated_lufs: Some(-8.0),
            true_peak_db: Some(1.0),
            ..TrackPlayout::unanalyzed(1_000)
        };
        assert_eq!(loud.gain_db(&settings), -8.0);

        // A boost stops at the peak ceiling.
        let quiet = TrackPlayout {
            integrated_lufs: Some(-24.0),
            true_peak_db: Some(-4.0),
            ..TrackPlayout::unanalyzed(1_000)
        };
        assert_eq!(quiet.gain_db(&settings), 3.0);

        let silent = TrackPlayout::unanalyzed(1_000);
        assert_eq!(silent.gain_db(&settings), 0.0);
        let disabled = LoudnessSettings {
            enabled: false,
            ..settings
        };
        assert_eq!(loud.gain_db(&disabled), 0.0);
    }

    #[test]
    fn settings_read_partial_json() {
        let settings: PlayoutSettings =
            serde_json::from_str(r#"{"crossfade": {"fade_out": {"curve": "s_curve"}}}"#).unwrap();
        assert_eq!(settings.crossfade.fade_out.curve, FadeCurve::SCurve);
        assert_eq!(settings.crossfade.fade_out.time_ms, 2_500);
        assert_eq!(settings.crossfade.min_track_ms, 20_000);
    }

    #[test]
    fn a_short_track_keeps_its_own_point_without_fades() {
        let settings = CrossfadeSettings::default();
        let mut jingle = track(0, 2_000, None);
        jingle.crossfade.mode = Some(CrossfadeMode::Fixed);
        jingle.crossfade.fixed_ms = Some(500);
        let song = track(0, 200_000, Some(195_000));
        let plan = plan_transition(&jingle, &song, &settings);
        assert_eq!(plan.next_start_ms, 1_500);
        assert_eq!(plan.fade_out, None);
        assert_eq!(plan.fade_in, None);
        // Into another jingle too.
        assert_eq!(
            plan_transition(&jingle, &jingle, &settings).next_start_ms,
            1_500
        );
        // Without its own point, or with crossfading off: back to back.
        let plain = track(0, 2_000, None);
        assert_eq!(
            plan_transition(&plain, &song, &settings).next_start_ms,
            2_000
        );
        jingle.crossfade.crossfade = Some(false);
        assert_eq!(
            plan_transition(&jingle, &song, &settings).next_start_ms,
            2_000
        );
    }

    #[test]
    fn a_long_track_into_a_short_one_stays_back_to_back() {
        let settings = CrossfadeSettings::default();
        let mut song = track(0, 200_000, Some(195_000));
        song.crossfade.mode = Some(CrossfadeMode::Fixed);
        song.crossfade.fixed_ms = Some(4_000);
        let jingle = track(0, 2_000, None);
        let plan = plan_transition(&song, &jingle, &settings);
        assert_eq!(plan.next_start_ms, 200_000);
    }
}
