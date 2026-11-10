// SPDX-License-Identifier: AGPL-3.0-or-later

//! Each user's status bar settings (`docs/web.md#status-bar`): the parts
//! shown, the time on the decks, the countdowns. Kept with the user's
//! preferences, as JSON; a field missing takes its default.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult};

/// How many of a countdown's last seconds blink or turn to the alert colour.
const SECONDS: std::ops::RangeInclusive<u8> = 1..=60;

/// The time on the decks: what it shows, in that order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeckTime {
    /// `1:23`
    #[default]
    Elapsed,
    /// `1:23 / 5:12`
    ElapsedTotal,
    /// `1:23 / −3:49`
    ElapsedRemaining,
    /// `1:23 / −3:49 / 5:12`
    ElapsedRemainingTotal,
    /// `1:23 / 5:12 / −3:49`
    ElapsedTotalRemaining,
    /// `−3:49`
    Remaining,
    /// `−3:49 / 5:12`
    RemainingTotal,
}

/// A user's status bar. Sent whole: a field left out takes its default,
/// so `{}` restores them all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct StatusBarSettings {
    /// The bar at the bottom of every page.
    pub show: bool,
    /// The decks part.
    pub decks: bool,
    /// The deck on air only, rather than A and B.
    pub on_air_only: bool,
    /// The last track played to its end.
    pub history: bool,
    /// Of this category type only; `null`: of any type. *Song* (1) by
    /// default.
    pub history_type_id: Option<u64>,
    /// The listeners and their record (with listener statistics on).
    pub listeners: bool,
    /// The next show to open, or track at an exact time.
    pub next_fixed: bool,
    /// How long the waiting tracks last.
    pub queue: bool,
    /// How many outputs are connected.
    pub outputs: bool,
    /// The machine's load (operators and admins only).
    pub system: bool,
    /// The station's clock.
    pub station_time: bool,
    pub time: DeckTime,
    /// `intro −0:07` on the deck on air until the vocals start.
    pub intro_countdown: bool,
    /// Its last seconds blink, rather than only turn to the alert colour.
    pub intro_blink: bool,
    /// Its last seconds: 1 to 60. On air's *vocals in* turns to the warning
    /// colour from there too.
    #[schema(minimum = 1, maximum = 60)]
    pub intro_seconds: u8,
    /// The time left before the next track, on the deck on air, in its last
    /// seconds.
    pub end_countdown: bool,
    pub end_blink: bool,
    /// Its last seconds: 1 to 60.
    #[schema(minimum = 1, maximum = 60)]
    pub end_seconds: u8,
    /// The intro's colour on the decks' progress (`#rrggbb`); `null`: the
    /// intro colour of the theme.
    #[schema(pattern = "^#[0-9a-f]{6}$")]
    pub intro_color: Option<String>,
    /// The track's colour on the decks' progress, past the intro
    /// (`#rrggbb`); `null`: each deck's own colour.
    #[schema(pattern = "^#[0-9a-f]{6}$")]
    pub track_color: Option<String>,
}

impl Default for StatusBarSettings {
    fn default() -> Self {
        Self {
            show: true,
            decks: true,
            on_air_only: false,
            history: false,
            history_type_id: Some(1),
            listeners: true,
            next_fixed: true,
            queue: false,
            outputs: false,
            system: true,
            station_time: false,
            time: DeckTime::Elapsed,
            intro_countdown: true,
            intro_blink: true,
            intro_seconds: 5,
            end_countdown: true,
            end_blink: false,
            end_seconds: 10,
            intro_color: None,
            track_color: None,
        }
    }
}

impl StatusBarSettings {
    /// Refuses countdown seconds outside 1 to 60, and colours that are not
    /// `#rrggbb` in lower case (`422`).
    pub fn check(&self) -> ApiResult<()> {
        for (name, color) in [
            ("intro_color", &self.intro_color),
            ("track_color", &self.track_color),
        ] {
            if let Some(color) = color
                && !is_color(color)
            {
                return Err(ApiError::Invalid(format!(
                    "{name} is a colour such as #2f6fd6, not {color:?}"
                )));
            }
        }
        for (name, seconds) in [
            ("intro_seconds", self.intro_seconds),
            ("end_seconds", self.end_seconds),
        ] {
            if !SECONDS.contains(&seconds) {
                return Err(ApiError::Invalid(format!(
                    "{name} is 1 to 60 seconds, not {seconds}"
                )));
            }
        }
        Ok(())
    }

    /// The stored settings; the defaults when there are none or they
    /// cannot be read.
    pub fn parse(json: Option<&str>) -> Self {
        json.and_then(|json| serde_json::from_str::<Self>(json).ok())
            .filter(|settings| settings.check().is_ok())
            .unwrap_or_default()
    }
}

/// `#rrggbb`, in lower case.
fn is_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..]
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_left_out_takes_its_default() {
        let settings: StatusBarSettings =
            serde_json::from_str(r#"{"queue": true, "time": "remaining_total"}"#).unwrap();
        assert!(settings.queue);
        assert_eq!(settings.time, DeckTime::RemainingTotal);
        assert_eq!(settings.intro_seconds, 5);
        assert_eq!(
            serde_json::from_str::<StatusBarSettings>("{}").unwrap(),
            StatusBarSettings::default()
        );
    }

    #[test]
    fn countdown_seconds_are_1_to_60() {
        for (seconds, ok) in [(0, false), (1, true), (60, true), (61, false)] {
            let settings = StatusBarSettings {
                end_seconds: seconds,
                ..StatusBarSettings::default()
            };
            assert_eq!(settings.check().is_ok(), ok, "{seconds}");
        }
    }

    #[test]
    fn unreadable_settings_are_the_defaults() {
        assert_eq!(StatusBarSettings::parse(None), StatusBarSettings::default());
        assert_eq!(
            StatusBarSettings::parse(Some("not json")),
            StatusBarSettings::default()
        );
        assert_eq!(
            StatusBarSettings::parse(Some(r#"{"intro_seconds": 0}"#)),
            StatusBarSettings::default()
        );
        assert!(
            StatusBarSettings::parse(Some(r#"{"history_type_id": null}"#))
                .history_type_id
                .is_none()
        );
    }

    #[test]
    fn colours_are_rrggbb() {
        for (color, ok) in [
            ("#2f6fd6", true),
            ("#2F6FD6", false),
            ("2f6fd6", false),
            ("#2f6fd", false),
            ("#2f6fdz", false),
            ("red", false),
        ] {
            let settings = StatusBarSettings {
                intro_color: Some(color.to_owned()),
                ..StatusBarSettings::default()
            };
            assert_eq!(settings.check().is_ok(), ok, "{color}");
        }
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(serde_json::from_str::<StatusBarSettings>(r#"{"later": true}"#).is_err());
    }
}
