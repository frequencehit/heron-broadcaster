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
    Restrictions,
    Eq,
    Processing,
    Trash,
    Review,
    Artists,
    Labels,
}

impl Module {
    pub const ALL: [Self; 11] = [
        Self::Medleys,
        Self::CoverArt,
        Self::ListenerStatistics,
        Self::Webhooks,
        Self::Restrictions,
        Self::Eq,
        Self::Processing,
        Self::Trash,
        Self::Review,
        Self::Artists,
        Self::Labels,
    ];

    /// Its key, as in [`Modules`].
    pub fn key(self) -> &'static str {
        match self {
            Self::Medleys => "medleys",
            Self::CoverArt => "cover_art",
            Self::ListenerStatistics => "listener_statistics",
            Self::Webhooks => "webhooks",
            Self::Restrictions => "restrictions",
            Self::Eq => "eq",
            Self::Processing => "processing",
            Self::Trash => "trash",
            Self::Review => "review",
            Self::Artists => "artists",
            Self::Labels => "labels",
        }
    }

    /// For people (logs, errors).
    pub fn name(self) -> &'static str {
        match self {
            Self::Medleys => "medleys",
            Self::CoverArt => "cover art",
            Self::ListenerStatistics => "listener statistics",
            Self::Webhooks => "webhooks",
            Self::Restrictions => "restrictions",
            Self::Eq => "equalizer",
            Self::Processing => "processing",
            Self::Trash => "trash",
            Self::Review => "to organize",
            Self::Artists => "artists",
            Self::Labels => "labels",
        }
    }

    /// A module of the library, which the stations share: turned on and
    /// off for the installation only.
    pub fn installation_only(self) -> bool {
        matches!(
            self,
            Self::Trash | Self::Review | Self::Artists | Self::Labels
        )
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
    pub restrictions: bool,
    pub eq: bool,
    pub processing: bool,
    pub trash: bool,
    pub review: bool,
    pub artists: bool,
    pub labels: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            medleys: true,
            cover_art: true,
            listener_statistics: true,
            webhooks: true,
            restrictions: true,
            eq: true,
            processing: true,
            trash: true,
            review: true,
            artists: true,
            labels: true,
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
            Module::Restrictions => self.restrictions,
            Module::Eq => self.eq,
            Module::Processing => self.processing,
            Module::Trash => self.trash,
            Module::Review => self.review,
            Module::Artists => self.artists,
            Module::Labels => self.labels,
        }
    }

    /// What is on at both levels, `self` the installation's and `station`
    /// a station's: a module off for the installation is off for every
    /// station, and the library's follow the installation alone.
    pub fn and(self, station: Self) -> Self {
        Self {
            medleys: self.medleys && station.medleys,
            cover_art: self.cover_art && station.cover_art,
            listener_statistics: self.listener_statistics && station.listener_statistics,
            webhooks: self.webhooks && station.webhooks,
            restrictions: self.restrictions && station.restrictions,
            eq: self.eq && station.eq,
            processing: self.processing && station.processing,
            ..self
        }
    }

    /// A station's switches as stored: the library's modules on, since
    /// only the installation turns them off.
    pub fn for_station(self) -> Self {
        Self {
            trash: true,
            review: true,
            artists: true,
            labels: true,
            ..self
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
    fn the_library_modules_follow_the_installation_alone() {
        let library_off = Modules {
            trash: false,
            review: false,
            artists: false,
            labels: false,
            ..Modules::default()
        };
        assert_eq!(Modules::default().and(library_off), Modules::default());
        assert_eq!(library_off.and(Modules::default()), library_off);
        assert_eq!(library_off.for_station(), Modules::default());
        // A station that turns everything off keeps the library's on.
        let all_off: Modules = serde_json::from_value(serde_json::Value::Object(
            Module::ALL
                .iter()
                .map(|module| (module.key().to_owned(), false.into()))
                .collect(),
        ))
        .unwrap();
        let on = Modules::default().and(all_off);
        for module in Module::ALL {
            assert_eq!(on.is_on(module), module.installation_only(), "{module:?}");
        }
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
