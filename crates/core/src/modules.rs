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
    /// A show (`docs/listener-interaction.md`): dedications (#131).
    Dedications,
    /// A show: listeners choose the next song (#132).
    Match,
    /// A show: the weekly chart (#134).
    Top30,
    /// A show without hours: listeners like or dislike the track playing
    /// (#133). On by default.
    Likes,
}

impl Module {
    pub const ALL: [Self; 15] = [
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
        Self::Dedications,
        Self::Match,
        Self::Top30,
        Self::Likes,
    ];

    /// The show modules, which listeners take part in from the website.
    pub const SHOWS: [Self; 4] = [Self::Dedications, Self::Match, Self::Top30, Self::Likes];

    /// Whether it is a show (`docs/listener-interaction.md`).
    pub fn is_show(self) -> bool {
        Self::SHOWS.contains(&self)
    }

    /// The module of a key (`dedications`).
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|module| module.key() == key)
    }

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
            Self::Dedications => "dedications",
            Self::Match => "match",
            Self::Top30 => "top30",
            Self::Likes => "likes",
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
            Self::Dedications => "dedications",
            Self::Match => "the match",
            Self::Top30 => "the top 30",
            Self::Likes => "likes",
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
/// module never stored takes its default: for the installation, the
/// modules that existed before they could be turned off are on, and the
/// shows off but Likes; for a station, everything is on ([`Modules::ALL_ON`]), so a
/// station only turns off what the installation has on.
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
    pub dedications: bool,
    #[serde(rename = "match")]
    pub r#match: bool,
    pub top30: bool,
    pub likes: bool,
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
            dedications: false,
            r#match: false,
            top30: false,
            likes: true,
        }
    }
}

impl Modules {
    /// The installation's, in the `settings` table.
    pub const SETTINGS_NAME: &str = "modules";
    /// A station's own, in the `station_settings` table.
    pub const STATION_SETTINGS_NAME: &str = "station_modules";

    /// Every module on: a station's switches never stored.
    pub const ALL_ON: Self = Self {
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
        dedications: true,
        r#match: true,
        top30: true,
        likes: true,
    };

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
            Module::Dedications => self.dedications,
            Module::Match => self.r#match,
            Module::Top30 => self.top30,
            Module::Likes => self.likes,
        }
    }

    /// Stored switches (any JSON) over `base`: a module they leave out, or
    /// give no boolean, keeps its value in `base`.
    pub fn over(base: Self, stored: &serde_json::Value) -> Self {
        let mut modules = base;
        for module in Module::ALL {
            if let Some(on) = stored
                .get(module.key())
                .and_then(serde_json::Value::as_bool)
            {
                modules.set(module, on);
            }
        }
        modules
    }

    fn set(&mut self, module: Module, on: bool) {
        let field = match module {
            Module::Medleys => &mut self.medleys,
            Module::CoverArt => &mut self.cover_art,
            Module::ListenerStatistics => &mut self.listener_statistics,
            Module::Webhooks => &mut self.webhooks,
            Module::Restrictions => &mut self.restrictions,
            Module::Eq => &mut self.eq,
            Module::Processing => &mut self.processing,
            Module::Trash => &mut self.trash,
            Module::Review => &mut self.review,
            Module::Artists => &mut self.artists,
            Module::Labels => &mut self.labels,
            Module::Dedications => &mut self.dedications,
            Module::Match => &mut self.r#match,
            Module::Top30 => &mut self.top30,
            Module::Likes => &mut self.likes,
        };
        *field = on;
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
            dedications: self.dedications && station.dedications,
            r#match: self.r#match && station.r#match,
            top30: self.top30 && station.top30,
            likes: self.likes && station.likes,
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
        assert!(
            Module::ALL[1..]
                .iter()
                .filter(|module| !module.is_show())
                .all(|module| stored.is_on(*module))
        );
        assert!(
            Module::SHOWS[..3]
                .iter()
                .all(|module| !stored.is_on(*module))
        );
        assert!(stored.is_on(Module::Likes), "likes are on by default");
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
    fn a_station_keeps_on_what_it_does_not_turn_off() {
        let stored = serde_json::json!({"medleys": false, "match": false, "later": true});
        let station = Modules::over(Modules::ALL_ON, &stored);
        assert!(!station.medleys && !station.r#match);
        assert!(station.dedications && station.top30 && station.webhooks);
        assert_eq!(
            Modules::over(Modules::ALL_ON, &serde_json::Value::Null),
            Modules::ALL_ON
        );
        let installation = Modules::over(Modules::default(), &serde_json::json!({"top30": true}));
        assert!(installation.top30 && !installation.dedications);
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
            assert_eq!(
                json[module.key()],
                !module.is_show() || module == Module::Likes,
                "{module:?}: the shows are off by default, but likes"
            );
            assert_eq!(Module::from_key(module.key()), Some(module));
            assert_eq!(
                serde_json::to_value(module).unwrap(),
                serde_json::Value::from(module.key())
            );
        }
    }
}
