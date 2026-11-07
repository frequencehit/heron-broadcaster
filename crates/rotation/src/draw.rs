// SPDX-License-Identifier: AGPL-3.0-or-later

//! The draw depth: a slot draws among the share of its category's tracks
//! played longest ago, widened before the rules are relaxed
//! (`docs/rotation.md#draw-depth`).

use serde::{Deserialize, Serialize};

use crate::rules::{FALLBACK_STEP, SeparationOverrides};
use crate::select::Blocked;
use crate::{History, Rng, Track};

/// A draw has at least this many tracks (all of them in a smaller pool).
pub const MIN_TRACKS: usize = 3;

/// The draw widens by this many points at a time.
pub const WIDEN_PCT: u8 = 10;

/// A draw depth: where it starts and how far it widens, in percent of the
/// tracks the restrictions allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct DrawDepth {
    /// 1 to 100.
    pub start_pct: u8,
    /// 1 to 100, not below the start.
    pub max_pct: u8,
}

impl Default for DrawDepth {
    fn default() -> Self {
        Self {
            start_pct: 20,
            max_pct: 50,
        }
    }
}

impl DrawDepth {
    /// This depth, with what a category or a clock slot overrides.
    pub fn with(self, overrides: &SeparationOverrides) -> Self {
        Self {
            start_pct: overrides.draw_start_pct.unwrap_or(self.start_pct),
            max_pct: overrides.draw_max_pct.unwrap_or(self.max_pct),
        }
    }

    /// Within 1 to 100, a start above the maximum at the maximum.
    pub fn clamped(self) -> Self {
        let max_pct = self.max_pct.clamp(1, 100);
        Self {
            start_pct: self.start_pct.clamp(1, max_pct),
            max_pct,
        }
    }

    pub fn check(&self) -> Result<(), String> {
        check_pct(Some(self.start_pct), Some(self.max_pct))
    }

    /// The depths a relaxation step draws at, in order: from the start, 10
    /// points at a time, to the maximum at step 0 and in the fallback
    /// category (step 11); the maximum at the other steps.
    pub fn depths(self, step: u8) -> Vec<u8> {
        let draw = self.clamped();
        if step != 0 && step != FALLBACK_STEP {
            return vec![draw.max_pct];
        }
        let mut depths = Vec::new();
        let mut pct = draw.start_pct;
        while pct < draw.max_pct {
            depths.push(pct);
            pct = pct.saturating_add(WIDEN_PCT);
        }
        depths.push(draw.max_pct);
        depths
    }
}

/// Checks a draw depth's start and maximum, when set: 1 to 100, the start
/// not above the maximum.
pub fn check_pct(start: Option<u8>, max: Option<u8>) -> Result<(), String> {
    if [start, max]
        .into_iter()
        .flatten()
        .any(|pct| !(1..=100).contains(&pct))
    {
        return Err("the draw depth is 1 to 100 %".to_owned());
    }
    if let (Some(start), Some(max)) = (start, max)
        && start > max
    {
        return Err("the draw depth starts at its maximum or below".to_owned());
    }
    Ok(())
}

/// How many tracks a draw of `pct` % of `pool` tracks has: rounded down,
/// at least [`MIN_TRACKS`], at most the pool.
pub fn size(pool: usize, pct: u8) -> usize {
    (pool * usize::from(pct) / 100).max(MIN_TRACKS).min(pool)
}

/// `pool` ordered for a draw: played longest ago first, never played ones
/// as the oldest; tracks with the same last play (never played ones
/// included) in a random order.
pub(crate) fn order<'l>(
    pool: &[(&'l Track, f64)],
    history: &History,
    rng: &mut Rng,
) -> Vec<(&'l Track, f64)> {
    let key = |track: &Track| history.last_played(track.id);
    let mut ordered = pool.to_vec();
    ordered.sort_by_key(|(track, _)| (key(track), track.id));
    let mut start = 0;
    while start < ordered.len() {
        let last = key(ordered[start].0);
        let end = start
            + ordered[start..]
                .iter()
                .take_while(|(track, _)| key(track) == last)
                .count();
        // Fisher–Yates over the run of ties.
        for i in (start + 1..end).rev() {
            let j = start + rng.below(i - start + 1);
            ordered.swap(i, j);
        }
        start = end;
    }
    ordered
}

/// The draw a pick was made in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DrawLog {
    /// The depth the pick was drawn at, in percent.
    pub pct: u8,
    /// The depth the slot starts at and its maximum.
    pub start_pct: u8,
    pub max_pct: u8,
    /// Tracks in the draw at that depth.
    pub tracks: u32,
    /// Of them, held back by each rule at the pick's step (the first rule
    /// that applies counts).
    pub blocked: Blocked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draw_has_three_tracks_at_least() {
        assert_eq!(size(5, 20), 3);
        assert_eq!(size(2, 20), 2);
        assert_eq!(size(100, 20), 20);
        assert_eq!(size(12, 20), 3);
        assert_eq!(size(7, 50), 3);
        assert_eq!(size(10, 100), 10);
        assert_eq!(size(0, 20), 0);
    }

    #[test]
    fn it_widens_by_ten_points_at_step_0_and_11_only() {
        let draw = DrawDepth::default();
        assert_eq!(draw.depths(0), [20, 30, 40, 50]);
        assert_eq!(draw.depths(11), [20, 30, 40, 50]);
        for step in 1..=10 {
            assert_eq!(draw.depths(step), [50]);
        }
        let odd = DrawDepth {
            start_pct: 15,
            max_pct: 42,
        };
        assert_eq!(odd.depths(0), [15, 25, 35, 42]);
        let flat = DrawDepth {
            start_pct: 60,
            max_pct: 30,
        };
        assert_eq!(flat.depths(0), [30]);
    }

    #[test]
    fn overrides_replace_the_start_or_the_maximum() {
        let draw = DrawDepth::default().with(&SeparationOverrides {
            draw_max_pct: Some(80),
            ..SeparationOverrides::default()
        });
        assert_eq!(
            draw,
            DrawDepth {
                start_pct: 20,
                max_pct: 80
            }
        );
        assert!(check_pct(Some(0), None).is_err());
        assert!(check_pct(None, Some(101)).is_err());
        assert!(check_pct(Some(40), Some(30)).is_err());
        assert!(check_pct(Some(40), None).is_ok());
        assert!(DrawDepth::default().check().is_ok());
    }
}
