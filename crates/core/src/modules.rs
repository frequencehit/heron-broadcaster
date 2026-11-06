// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modules: parts of Heron an installation or a station can turn off
//! (`docs/modules.md`).

use serde::{Deserialize, Serialize};

/// A module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Module {
    Medleys,
    CoverArt,
    ListenerStatistics,
    Webhooks,
}

impl Module {
    pub const ALL: [Self; 4] = [
        Self::Medleys,
        Self::CoverArt,
        Self::ListenerStatistics,
        Self::Webhooks,
    ];

    /// Its key, as in [`Modules`].
    pub fn key(self) -> &'static str {
        match self {
            Self::Medleys => "medleys",
            Self::CoverArt => "cover_art",
            Self::ListenerStatistics => "listener_statistics",
            Self::Webhooks => "webhooks",
        }
    }

    /// For people (logs, errors).
    pub fn name(self) -> &'static str {
        match self {
            Self::Medleys => "medleys",
            Self::CoverArt => "cover art",
            Self::ListenerStatistics => "listener statistics",
            Self::Webhooks => "webhooks",
        }
    }
}

/// Which modules are on. Stored as the `modules` settings (the
/// installation) and the `station_modules` settings of each station; a
/// module never stored takes its default: the modules that existed before
/// they could be turned off are on.
// The shows to come (#131, #132, #134) are off by default, Likes (#133) on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct Modules {
    pub medleys: bool,
    pub cover_art: bool,
    pub listener_statistics: bool,
    pub webhooks: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            medleys: true,
            cover_art: true,
            listener_statistics: true,
            webhooks: true,
        }
    }
}

impl Modules {
    /// The installation's, in the `settings` table.
    pub const SETTINGS_NAME: &str = "modules";
    /// A station's own, in the `station_settings` table.
    pub const STATION_SETTINGS_NAME: &str = "station_modules";

    pub fn is_on(&self, module: Module) -> bool {
        match module {
            Module::Medleys => self.medleys,
            Module::CoverArt => self.cover_art,
            Module::ListenerStatistics => self.listener_statistics,
            Module::Webhooks => self.webhooks,
        }
    }

    /// What is on at both levels: a module off for the installation is
    /// off for every station.
    pub fn and(self, other: Self) -> Self {
        Self {
            medleys: self.medleys && other.medleys,
            cover_art: self.cover_art && other.cover_art,
            listener_statistics: self.listener_statistics && other.listener_statistics,
            webhooks: self.webhooks && other.webhooks,
        }
    }

    /// Reads stored switches; invalid ones count as the defaults.
    pub fn parse(json: Option<&str>) -> Self {
        json.and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_modules_are_on() {
        assert_eq!(Modules::parse(None), Modules::default());
        let stored = Modules::parse(Some(r#"{"medleys": false, "later": true}"#));
        assert!(!stored.is_on(Module::Medleys));
        assert!(Module::ALL[1..].iter().all(|module| stored.is_on(*module)));
        assert_eq!(Modules::parse(Some("not json")), Modules::default());
    }

    #[test]
    fn off_for_the_installation_is_off_everywhere() {
        let installation = Modules {
            webhooks: false,
            ..Modules::default()
        };
        let station = Modules {
            medleys: false,
            ..Modules::default()
        };
        let on = installation.and(station);
        assert!(!on.webhooks && !on.medleys && on.cover_art && on.listener_statistics);
    }

    #[test]
    fn keys_match_the_fields() {
        let json = serde_json::to_value(Modules::default()).unwrap();
        for module in Module::ALL {
            assert_eq!(json[module.key()], true, "{module:?}");
            assert_eq!(
                serde_json::to_value(module).unwrap(),
                serde_json::Value::from(module.key())
            );
        }
    }
}
