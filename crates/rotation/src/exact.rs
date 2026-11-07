// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exact times: clock slots that start at a given time of the hour, hard
//! (what plays is cut) or soft (rotation plans the tracks before it to end
//! in a window), and the medleys and fillers that fill the gap before them
//! (`docs/rotation.md#exact-times`).

use serde::{Deserialize, Serialize};

use crate::{CategoryId, TrackId};

/// How long what plays fades out at a hard exact time, or at the end of a
/// soft window.
pub const CUT_FADE_MS: u32 = 1_500;
/// The longest a soft window reaches before or after its exact time.
pub const MAX_WINDOW_S: u32 = 300;
/// The shortest crossfade between the two spans of a shortened medley.
pub const MIN_MEDLEY_CROSSFADE_MS: u32 = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExactMode {
    /// At the exact time, what plays fades out and the slot starts.
    #[default]
    Hard,
    /// The slot starts in a window around the exact time: nothing is cut
    /// unless a track would end after it.
    Soft,
}

/// A clock slot's exact time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ExactTime {
    /// Seconds into the station hour, 0 to 3599.
    pub at_s: u32,
    #[serde(default)]
    pub mode: ExactMode,
    /// Soft: how early the slot may start, in seconds (0 to 300).
    #[serde(default = "default_before")]
    pub before_s: u32,
    /// Soft: how late it may start; a track ending later is faded out then.
    #[serde(default = "default_after")]
    pub after_s: u32,
    /// Soft: a gap before the window is filled with a station medley,
    /// shortened from its end when too long.
    #[serde(default)]
    pub medley_fill: bool,
    /// The exact time cuts a live DJ (ad breaks). Stored for M11: there is
    /// no live DJ yet.
    #[serde(default)]
    pub cut_live: bool,
}

fn default_before() -> u32 {
    10
}

fn default_after() -> u32 {
    30
}

impl ExactTime {
    pub fn check(&self) -> Result<(), String> {
        if self.at_s >= 3_600 {
            return Err("an exact time is 0 to 3599 seconds into the hour".to_owned());
        }
        if self.before_s > MAX_WINDOW_S || self.after_s > MAX_WINDOW_S {
            return Err(format!(
                "a soft window reaches at most {MAX_WINDOW_S} seconds before or after its exact time"
            ));
        }
        Ok(())
    }

    /// The window for an exact time at `at_ms`: from when the slot may
    /// start to when what plays is cut. Both are `at_ms` in hard mode.
    pub fn window(&self, at_ms: i64) -> Window {
        match self.mode {
            ExactMode::Hard => Window {
                at_ms,
                from_ms: at_ms,
                cut_ms: at_ms,
            },
            ExactMode::Soft => Window {
                at_ms,
                from_ms: at_ms - i64::from(self.before_s) * 1_000,
                cut_ms: at_ms + i64::from(self.after_s) * 1_000,
            },
        }
    }
}

/// When an exact slot may start, in Unix ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// The exact time.
    pub at_ms: i64,
    /// The earliest start.
    pub from_ms: i64,
    /// The latest: what still plays then is faded out.
    pub cut_ms: i64,
}

/// Where a track ending at some time falls against a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Landing {
    /// Ends inside the window.
    Lands,
    /// Ends before it: more can follow.
    Early,
    /// Ends after it: it would be cut.
    Late,
}

impl Window {
    pub(crate) fn landing(&self, end_ms: i64) -> Landing {
        if end_ms > self.cut_ms {
            Landing::Late
        } else if end_ms >= self.from_ms {
            Landing::Lands
        } else {
            Landing::Early
        }
    }
}

/// How an exact time was met, in the rotation log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Met {
    /// Within a second of the exact time.
    OnTime,
    Early,
    Late,
}

/// An exact slot's pick: its exact time, and how rotation met it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExactLog {
    /// The exact time (Unix ms).
    pub at_unix_ms: i64,
    pub mode: ExactMode,
    /// The soft window, in seconds; 0 in hard mode.
    pub before_s: u32,
    pub after_s: u32,
    pub met: Met,
    /// Planned start minus the exact time; negative when early.
    pub offset_ms: i64,
    /// The planned start is inside the window (the exact time in hard mode).
    pub in_window: bool,
    /// What plays before it is faded out at `cut_at_unix_ms`.
    pub faded: bool,
    /// When the engine fades out what still plays and starts the slot: the
    /// exact time (hard) or the end of the window (soft).
    pub cut_at_unix_ms: i64,
}

impl ExactLog {
    pub(crate) fn new(exact: &ExactTime, window: Window, start_ms: i64, faded: bool) -> Self {
        let offset_ms = start_ms - window.at_ms;
        let (before_s, after_s) = match exact.mode {
            ExactMode::Hard => (0, 0),
            ExactMode::Soft => (exact.before_s, exact.after_s),
        };
        Self {
            at_unix_ms: window.at_ms,
            mode: exact.mode,
            before_s,
            after_s,
            met: if offset_ms.abs() <= 1_000 {
                Met::OnTime
            } else if offset_ms < 0 {
                Met::Early
            } else {
                Met::Late
            },
            offset_ms,
            in_window: (window.from_ms..=window.cut_ms).contains(&start_ms),
            faded,
            cut_at_unix_ms: window.cut_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FillKind {
    /// One of the station's medleys, shortened or whole.
    Medley,
    /// A track of the station's filler category.
    Filler,
}

/// A pick filling the gap before an exact time, in the rotation log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FillLog {
    pub kind: FillKind,
    /// The exact slot it fills the gap before (index in its clock).
    pub slot: u32,
    /// The exact time (Unix ms).
    pub exact_at_unix_ms: i64,
    /// The medley's ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medley_id: Option<u64>,
    /// Hooks left out of the medley (0: whole).
    #[serde(default)]
    pub hooks_left_out: u32,
    /// The parts left out, by their position in the medley from 0.
    #[serde(default)]
    pub parts_left_out: Vec<u32>,
    /// How long it plays.
    pub length_ms: u32,
}

/// A way of playing a medley shorter: its last liner + hook pairs left out,
/// as two spans of its rendered file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MedleyCut {
    pub hooks_left_out: u32,
    /// The parts left out, by their position in the medley from 0.
    pub parts_left_out: Vec<u32>,
    /// How long it plays: both spans, less the crossfade between them.
    pub length_ms: u32,
    /// The first span: from the track's cue-in to here (ms in the file).
    pub first_to_ms: u32,
    /// The second span: from here to the track's cue-out.
    pub second_from_ms: u32,
    /// The crossfade between the two spans.
    pub crossfade_ms: u32,
}

/// A rendered medley's track, and how it can be shortened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Medley {
    pub id: u64,
    /// Shortened plays, the fewest hooks left out first.
    pub cuts: Vec<MedleyCut>,
}

/// The candidates' lengths for a pick: each track's whole length, and for
/// medleys in a gap fill their shortened lengths too.
pub(crate) struct Fit<'a> {
    pub window: Window,
    pub mode: FitMode,
    pub medleys: Option<&'a std::collections::HashMap<TrackId, Medley>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FitMode {
    /// Before an exact time: tracks that land first, then those ending
    /// early, then (hard mode) those cut; `None` when only late ones are
    /// left in soft mode (the slot no longer fits).
    Prefer { late_ok: bool },
    /// A medley filling a gap: it must land, the fewest hooks left out first.
    Medley,
}

/// A track chosen to fit, and the medley cut it plays with.
pub(crate) type Fitted<'l> = (&'l crate::Track, f64, Option<MedleyCut>);

impl Fit<'_> {
    /// The candidates the method chooses among, from `start_ms`: the best
    /// class present. Empty when none fits.
    pub(crate) fn keep<'l>(
        &self,
        candidates: &[(&'l crate::Track, f64)],
        start_ms: i64,
    ) -> Vec<Fitted<'l>> {
        let end = |length: u32| start_ms + i64::from(length);
        match self.mode {
            FitMode::Prefer { late_ok } => {
                let classes: Vec<Landing> = candidates
                    .iter()
                    .map(|(track, _)| self.window.landing(end(track.length_ms)))
                    .collect();
                let Some(best) = classes.iter().min().copied() else {
                    return Vec::new();
                };
                if best == Landing::Late && !late_ok {
                    return Vec::new();
                }
                candidates
                    .iter()
                    .zip(&classes)
                    .filter(|(_, class)| **class == best)
                    .map(|((track, weight), _)| (*track, *weight, None))
                    .collect()
            }
            FitMode::Medley => {
                // Each candidate's way of landing with the fewest hooks left out.
                let fitting: Vec<(u32, Fitted<'l>)> = candidates
                    .iter()
                    .filter_map(|(track, weight)| {
                        if self.window.landing(end(track.length_ms)) == Landing::Lands {
                            return Some((0, (*track, *weight, None)));
                        }
                        let medley = self.medleys?.get(&track.id)?;
                        medley
                            .cuts
                            .iter()
                            .find(|cut| self.window.landing(end(cut.length_ms)) == Landing::Lands)
                            .map(|cut| (cut.hooks_left_out, (*track, *weight, Some(cut.clone()))))
                    })
                    .collect();
                let Some(fewest) = fitting.iter().map(|(cuts, _)| *cuts).min() else {
                    return Vec::new();
                };
                fitting
                    .into_iter()
                    .filter(|(cuts, _)| *cuts == fewest)
                    .map(|(_, fitted)| fitted)
                    .collect()
            }
        }
    }
}

/// The categories a gap is filled from.
pub(crate) fn medley_category(library: &crate::Library) -> Option<CategoryId> {
    library
        .medley_category
        .filter(|category| !library.modules_off.contains(category))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_times_parse_with_their_defaults() {
        let exact: ExactTime = serde_json::from_str(r#"{"at_s": 1800, "mode": "soft"}"#).unwrap();
        assert_eq!((exact.before_s, exact.after_s), (10, 30));
        assert!(!exact.medley_fill && !exact.cut_live);
        assert!(exact.check().is_ok());
        let window = exact.window(100_000);
        assert_eq!((window.from_ms, window.cut_ms), (90_000, 130_000));
        let hard = ExactTime {
            mode: ExactMode::Hard,
            ..exact
        };
        let window = hard.window(100_000);
        assert_eq!((window.from_ms, window.cut_ms), (100_000, 100_000));
        assert!(
            ExactTime {
                at_s: 3_600,
                ..exact
            }
            .check()
            .is_err()
        );
        assert!(
            ExactTime {
                after_s: 301,
                ..exact
            }
            .check()
            .is_err()
        );
    }

    #[test]
    fn landing_against_a_window() {
        let window = Window {
            at_ms: 100,
            from_ms: 90,
            cut_ms: 130,
        };
        assert_eq!(window.landing(89), Landing::Early);
        assert_eq!(window.landing(90), Landing::Lands);
        assert_eq!(window.landing(130), Landing::Lands);
        assert_eq!(window.landing(131), Landing::Late);
    }
}
