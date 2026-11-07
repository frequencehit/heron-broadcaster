// SPDX-License-Identifier: AGPL-3.0-or-later

//! Separation rules and their relaxation (`docs/rotation.md`).

use serde::{Deserialize, Serialize};

/// Separation windows, in minutes; 0 turns a rule off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Windows {
    /// Do not play the same album within.
    pub album_min: u32,
    /// Do not play the same artist (or a similar one) within.
    pub artist_min: u32,
    /// Do not play the same title (any version) within.
    pub title_min: u32,
    /// Do not play the same track within.
    pub track_min: u32,
    /// Daypart separation: a track played in a daypart does not play in
    /// this many dayparts after it ([`crate::dayparts`]); 0: off.
    pub dayparts: u8,
    /// The same daypart on the following days: a track played in a
    /// daypart does not play in that daypart on this many days after
    /// ([`crate::dayparts::holds_days`]), 0 to 7; 0: off.
    pub daypart_days: u8,
}

impl Default for Windows {
    fn default() -> Self {
        Self {
            album_min: 60,
            artist_min: 120,
            title_min: 240,
            track_min: 240,
            dayparts: 0,
            daypart_days: 0,
        }
    }
}

/// The station's separation windows, in minutes (`rotation` settings); 0
/// turns a rule off. Daypart separation is not among them: it is each
/// category's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct StationWindows {
    /// Do not play the same album within.
    pub album_min: u32,
    /// Do not play the same artist (or a similar one) within.
    pub artist_min: u32,
    /// Do not play the same title (any version) within.
    pub title_min: u32,
    /// Do not play the same track within.
    pub track_min: u32,
}

impl Default for StationWindows {
    fn default() -> Self {
        Windows::default().into()
    }
}

impl From<Windows> for StationWindows {
    fn from(windows: Windows) -> Self {
        Self {
            album_min: windows.album_min,
            artist_min: windows.artist_min,
            title_min: windows.title_min,
            track_min: windows.track_min,
        }
    }
}

impl From<StationWindows> for Windows {
    /// Without daypart separation: a category sets it.
    fn from(windows: StationWindows) -> Self {
        Self {
            album_min: windows.album_min,
            artist_min: windows.artist_min,
            title_min: windows.title_min,
            track_min: windows.track_min,
            dayparts: 0,
            daypart_days: 0,
        }
    }
}

/// Separation windows overridden by a category or a clock slot; missing
/// ones are inherited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct SeparationOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_min: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_min: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_min: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_min: Option<u32>,
    /// Daypart separation: a category's own (none or 0: off), never a
    /// clock slot's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dayparts: Option<u8>,
    /// The same daypart on the following days: a category's own (none or
    /// 0: off), never a clock slot's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daypart_days: Option<u8>,
    /// The draw depth's start, in percent, 1 to 100
    /// ([`crate::draw::DrawDepth`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draw_start_pct: Option<u8>,
    /// The draw depth's maximum, in percent, 1 to 100.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draw_max_pct: Option<u8>,
}

impl SeparationOverrides {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Last relaxation step: the fallback category's least recently played
/// track, without rules.
pub const LAST_STEP: u8 = 12;

/// First step taken in the fallback category.
pub const FALLBACK_STEP: u8 = 11;

/// The longest any window can be: one week.
pub const MAX_WINDOW_MIN: u32 = 7 * 24 * 60;

impl Windows {
    pub const OFF: Self = Self {
        album_min: 0,
        artist_min: 0,
        title_min: 0,
        track_min: 0,
        dayparts: 0,
        daypart_days: 0,
    };

    pub fn with(self, overrides: &SeparationOverrides) -> Self {
        Self {
            album_min: overrides.album_min.unwrap_or(self.album_min),
            artist_min: overrides.artist_min.unwrap_or(self.artist_min),
            title_min: overrides.title_min.unwrap_or(self.title_min),
            track_min: overrides.track_min.unwrap_or(self.track_min),
            dayparts: overrides.dayparts.unwrap_or(self.dayparts),
            daypart_days: overrides.daypart_days.unwrap_or(self.daypart_days),
        }
    }

    /// The windows at a relaxation step, from these as configured. Steps 11
    /// and 12 apply to the fallback category's windows.
    pub fn at_step(self, step: u8) -> Self {
        let mut windows = self;
        if step >= 1 {
            windows.album_min /= 2;
        }
        if step >= 2 {
            windows.album_min = 0;
        }
        // The same daypart on the following days goes with daypart
        // separation.
        if step >= 3 {
            windows.dayparts /= 2;
            windows.daypart_days /= 2;
        }
        if step >= 4 {
            windows.dayparts = 0;
            windows.daypart_days = 0;
        }
        if step >= 5 {
            windows.artist_min /= 2;
        }
        if step >= 6 {
            windows.artist_min = 0;
        }
        if step >= 7 {
            windows.title_min /= 2;
        }
        if step >= 8 {
            windows.title_min = 0;
        }
        if step >= 9 {
            windows.track_min = self.track_min / 2;
        }
        if step >= 10 {
            windows.track_min = self.track_min / 4;
        }
        if step >= LAST_STEP {
            windows.track_min = 0;
        }
        windows
    }

    /// The longest window in minutes (daypart separation aside).
    pub fn longest_min(&self) -> u32 {
        self.album_min
            .max(self.artist_min)
            .max(self.title_min)
            .max(self.track_min)
    }

    /// Checks the windows are within a week, the daypart separation
    /// within the longest any division allows, and the same daypart kept
    /// out at most a week.
    pub fn check(&self) -> Result<(), String> {
        if self.longest_min() > MAX_WINDOW_MIN {
            return Err(format!(
                "separation windows are at most {MAX_WINDOW_MIN} minutes"
            ));
        }
        if self.dayparts > crate::dayparts::MAX_SEPARATION {
            return Err(format!(
                "daypart separation is at most {} dayparts",
                crate::dayparts::MAX_SEPARATION
            ));
        }
        if self.daypart_days > crate::dayparts::MAX_DAYS {
            return Err(format!(
                "the same daypart is kept out at most {} days",
                crate::dayparts::MAX_DAYS
            ));
        }
        Ok(())
    }
}

/// Windows across stations (`docs/stations.md#rules-across-stations`):
/// another station's play or planned track holds a track back within them,
/// before or after it. 0 turns a rule off; all are off by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct AcrossWindows {
    /// The same artist (or a similar one).
    pub artist_min: u32,
    /// The same title (any version).
    pub title_min: u32,
    /// The same track.
    pub track_min: u32,
}

impl AcrossWindows {
    pub const OFF: Self = Self {
        artist_min: 0,
        title_min: 0,
        track_min: 0,
    };

    /// Relaxed with the station's own rule of the same kind: artist halved
    /// at step 5 and dropped at 6, title at 7 and 8, track at 9 and 10.
    pub fn at_step(self, step: u8) -> Self {
        let mut windows = self;
        if step >= 5 {
            windows.artist_min /= 2;
        }
        if step >= 6 {
            windows.artist_min = 0;
        }
        if step >= 7 {
            windows.title_min /= 2;
        }
        if step >= 8 {
            windows.title_min = 0;
        }
        if step >= 9 {
            windows.track_min /= 2;
        }
        if step >= 10 {
            windows.track_min = 0;
        }
        windows
    }

    pub fn longest_min(&self) -> u32 {
        self.artist_min.max(self.title_min).max(self.track_min)
    }

    pub fn is_off(&self) -> bool {
        self.longest_min() == 0
    }

    pub fn check(&self) -> Result<(), String> {
        if self.longest_min() > MAX_WINDOW_MIN {
            return Err(format!(
                "windows across stations are at most {MAX_WINDOW_MIN} minutes"
            ));
        }
        Ok(())
    }
}

/// What a relaxation step relaxes, for people.
pub fn describe_step(step: u8) -> &'static str {
    match step {
        0 => "all rules as configured",
        1 => "album separation halved",
        2 => "album separation dropped",
        3 => "daypart separation halved",
        4 => "daypart separation dropped",
        5 => "artist separation halved",
        6 => "artist separation dropped",
        7 => "title separation halved",
        8 => "title separation dropped",
        9 => "track separation halved",
        10 => "track separation quartered",
        11 => "fallback category, track separation at a quarter",
        _ => "fallback category, least recently played, no rules",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaxation_follows_the_documented_table() {
        let windows = Windows {
            dayparts: 3,
            ..Windows::default()
        };
        let at = |step| {
            let w = windows.at_step(step);
            (
                w.album_min,
                w.dayparts,
                w.artist_min,
                w.title_min,
                w.track_min,
            )
        };
        assert_eq!(at(0), (60, 3, 120, 240, 240));
        assert_eq!(at(1), (30, 3, 120, 240, 240));
        assert_eq!(at(2), (0, 3, 120, 240, 240));
        // Daypart separation: after album, before artist; halved rounded
        // down, then dropped.
        assert_eq!(at(3), (0, 1, 120, 240, 240));
        assert_eq!(at(4), (0, 0, 120, 240, 240));
        assert_eq!(at(5), (0, 0, 60, 240, 240));
        assert_eq!(at(6), (0, 0, 0, 240, 240));
        assert_eq!(at(7), (0, 0, 0, 120, 240));
        assert_eq!(at(8), (0, 0, 0, 0, 240));
        assert_eq!(at(9), (0, 0, 0, 0, 120));
        assert_eq!(at(10), (0, 0, 0, 0, 60));
        assert_eq!(at(11), (0, 0, 0, 0, 60));
        assert_eq!(at(12), (0, 0, 0, 0, 0));
        for (separation, halved) in [(1, 0), (2, 1), (6, 3), (10, 5)] {
            let windows = Windows {
                dayparts: separation,
                ..Windows::OFF
            };
            assert_eq!(windows.at_step(3).dayparts, halved);
        }
        assert!(
            Windows {
                dayparts: 11,
                ..Windows::OFF
            }
            .check()
            .is_err()
        );
    }

    #[test]
    fn the_same_daypart_on_the_following_days_relaxes_with_daypart_separation() {
        // On its own (daypart separation off) or with it: halved rounded
        // down at step 3, dropped at step 4, as daypart separation.
        for dayparts in [0, 2] {
            let windows = Windows {
                dayparts,
                daypart_days: 7,
                ..Windows::default()
            };
            let days: Vec<u8> = (0..=12)
                .map(|step| windows.at_step(step).daypart_days)
                .collect();
            assert_eq!(days, [7, 7, 7, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
        for (days, halved) in [(1, 0), (2, 1), (3, 1), (7, 3)] {
            let windows = Windows {
                daypart_days: days,
                ..Windows::OFF
            };
            assert_eq!(windows.at_step(3).daypart_days, halved);
        }
        let days = |daypart_days| {
            Windows {
                daypart_days,
                ..Windows::OFF
            }
            .check()
        };
        assert!(days(7).is_ok());
        assert!(days(8).is_err());
    }

    #[test]
    fn windows_across_stations_relax_with_the_station_s_own() {
        let across = AcrossWindows {
            artist_min: 30,
            title_min: 60,
            track_min: 120,
        };
        let at = |step| {
            let w = across.at_step(step);
            (w.artist_min, w.title_min, w.track_min)
        };
        assert_eq!(at(0), (30, 60, 120));
        assert_eq!(at(4), (30, 60, 120));
        assert_eq!(at(5), (15, 60, 120));
        assert_eq!(at(6), (0, 60, 120));
        assert_eq!(at(7), (0, 30, 120));
        assert_eq!(at(8), (0, 0, 120));
        assert_eq!(at(9), (0, 0, 60));
        assert_eq!(at(10), (0, 0, 0));
        assert!(AcrossWindows::default().is_off());
    }

    #[test]
    fn overrides_replace_some_windows() {
        let windows = Windows::default().with(&SeparationOverrides {
            track_min: Some(90),
            artist_min: Some(0),
            dayparts: Some(2),
            ..SeparationOverrides::default()
        });
        assert_eq!(
            windows,
            Windows {
                album_min: 60,
                artist_min: 0,
                title_min: 240,
                track_min: 90,
                dayparts: 2,
                daypart_days: 0,
            }
        );
        let parsed: SeparationOverrides = serde_json::from_str(r#"{"daypart_days": 2}"#).unwrap();
        assert_eq!(Windows::OFF.with(&parsed).daypart_days, 2);
        assert!(serde_json::from_str::<StationWindows>(r#"{"daypart_days": 1}"#).is_err());
        // The station's windows have no daypart separation: it is each
        // category's.
        assert!(serde_json::from_str::<StationWindows>(r#"{"dayparts": 1}"#).is_err());
        assert_eq!(Windows::from(StationWindows::default()), Windows::default());
        let parsed: SeparationOverrides = serde_json::from_str(r#"{"title_min": 30}"#).unwrap();
        assert_eq!(parsed.title_min, Some(30));
        assert!(!parsed.is_empty());
    }
}
