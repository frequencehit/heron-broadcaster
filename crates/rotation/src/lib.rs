// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rotation: categories, separation rules, restrictions, clocks and track
//! selection (`docs/rotation.md`).
//!
//! Rotation is a pure function of the library, the play history, the rules,
//! the clock and the time: [`next`] returns the next track and why it was
//! picked. Nothing here touches the database, so days of programming can be
//! simulated offline ([`simulate`]) and checked ([`audit`]).

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

pub mod dayparts;
pub mod draw;
pub mod names;
pub mod ratio;
pub mod restrict;
mod rng;
pub mod rules;
pub mod schedule;
mod select;
mod sim;

pub use dayparts::{DaypartFrom, DaypartLog};
pub use draw::{DrawDepth, DrawLog};
pub use ratio::HeldByRatio;
pub use restrict::{Mode, Restriction, Rule, Target};
pub use rng::Rng;
pub use rules::{AcrossWindows, SeparationOverrides, StationWindows, Windows};
pub use schedule::{Period, Schedule, ScheduleId};
pub use select::{
    Blocked, Explanation, Outcome, Pick, SeparationRule, SkipReason, SkippedSlot, next,
    next_outcome,
};
pub use sim::{SimulatedPick, Simulation, Violation, audit, simulate};

pub type TrackId = u64;
pub type ArtistId = u64;
pub type CategoryId = u64;
pub type ClockId = u64;
pub type StationId = u64;
/// A category type (`category_types`): Song, Jingle, Ad…
pub type TypeId = u64;

/// How a clock slot picks among the tracks its category allows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    LeastRecentlyPlayed,
    Random,
    /// At random, in proportion to the tracks' weights in the category.
    Weighted,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LeastRecentlyPlayed => "least_recently_played",
            Self::Random => "random",
            Self::Weighted => "weighted",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "least_recently_played" => Some(Self::LeastRecentlyPlayed),
            "random" => Some(Self::Random),
            "weighted" => Some(Self::Weighted),
            _ => None,
        }
    }
}

/// Weights move with plays and requests (multiplied by these factors).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct WeightSettings {
    /// Each play multiplies the track's weights by this, 0.5 to 1 (1: weights stay).
    pub play_factor: f64,
    /// Each request multiplies the track's weights by this, 1 to 2 (1: weights stay).
    pub request_factor: f64,
}

impl Default for WeightSettings {
    fn default() -> Self {
        Self {
            play_factor: 1.0,
            request_factor: 1.0,
        }
    }
}

/// Weights stay within these bounds.
pub const MIN_WEIGHT: f64 = 0.1;
pub const MAX_WEIGHT: f64 = 10.0;

/// Station-wide rotation settings, stored under the `rotation` settings name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct RotationSettings {
    pub separation: StationWindows,
    /// The station's division of the day for daypart separation: 4
    /// dayparts of 6 hours, or 6 of 4 hours. The separation itself is each
    /// category's.
    pub daypart_count: u8,
    /// Where relaxation ends (steps 9 and 10). The station's library when
    /// unset.
    pub fallback_category_id: Option<CategoryId>,
    /// Categories whose tracks are kept out of the station's library (hours
    /// without a clock, the fallback without a category), even when another
    /// of their categories is the station's: tracks set aside.
    pub kept_out_category_ids: Vec<CategoryId>,
    pub weights: WeightSettings,
    /// Rules across stations: off by default.
    pub across: AcrossStations,
    /// The draw depth of the categories and slots that set none of their
    /// own, and of hours without a clock.
    pub draw: DrawDepth,
}

impl Default for RotationSettings {
    fn default() -> Self {
        Self {
            separation: StationWindows::default(),
            daypart_count: dayparts::DEFAULT_COUNT,
            fallback_category_id: None,
            kept_out_category_ids: Vec::new(),
            weights: WeightSettings::default(),
            across: AcrossStations::default(),
            draw: DrawDepth::default(),
        }
    }
}

/// What a station keeps apart from the other stations' plays and queues.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct AcrossStations {
    /// The same artist (or a similar one) within, in minutes; 0: off.
    pub artist_min: u32,
    /// The same title (any version) within.
    pub title_min: u32,
    /// The same track within.
    pub track_min: u32,
    /// The stations it looks at; all the others when empty.
    pub station_ids: Vec<StationId>,
}

impl AcrossStations {
    pub fn windows(&self) -> AcrossWindows {
        AcrossWindows {
            artist_min: self.artist_min,
            title_min: self.title_min,
            track_min: self.track_min,
        }
    }
}

impl RotationSettings {
    pub const SETTINGS_NAME: &str = "rotation";

    pub fn check(&self) -> Result<(), String> {
        Windows::from(self.separation).check()?;
        if !dayparts::COUNTS.contains(&self.daypart_count) {
            return Err("daypart_count is 4 or 6".to_owned());
        }
        self.across.windows().check()?;
        self.draw.check()?;
        let weights = &self.weights;
        if !(0.5..=1.0).contains(&weights.play_factor) {
            return Err("weights.play_factor is between 0.5 and 1".to_owned());
        }
        if !(1.0..=2.0).contains(&weights.request_factor) {
            return Err("weights.request_factor is between 1 and 2".to_owned());
        }
        Ok(())
    }
}

/// A playable library track, as rotation sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: TrackId,
    /// See [`names::title_key`].
    pub title_key: Option<String>,
    pub album_key: Option<String>,
    pub artists: Vec<ArtistId>,
    /// Categories the track is in; filled by [`Library::new`].
    pub categories: Vec<CategoryId>,
    /// Playable length (between the cue points).
    pub length_ms: u32,
    /// Play ratio in tenths, 1 to 10: plays this many times out of the 10
    /// times rotation picks it ([`ratio`]).
    pub ratio: u8,
    /// The track's own daypart separation, in place of its categories'
    /// (0: off for it); `None`: its category's ([`Library::for_track`]).
    pub dayparts: Option<u8>,
    /// The track's own same daypart on the following days, 0 to 7, in
    /// place of its categories'; `None`: its category's.
    pub daypart_days: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Member {
    pub track: TrackId,
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub id: CategoryId,
    pub name: String,
    pub separation: SeparationOverrides,
    pub members: Vec<Member>,
}

/// How a clock slot calls its category.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub category: CategoryId,
    pub method: Method,
    pub enforce_rules: bool,
    pub separation: SeparationOverrides,
    /// Plays only if the next slot's track plays right after it (a jingle
    /// announcing it): picked together, or not at all.
    pub linked_to_next: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clock {
    pub id: ClockId,
    pub name: String,
    pub slots: Vec<Slot>,
    /// Loops across hours: carries on from its last slot instead of
    /// starting over at each hour (SAM Broadcaster's clockwheel).
    pub looping: bool,
}

impl Clock {
    /// Whether the slot at `index` is linked to the slot after it (the
    /// first one after the last). A linked slot followed by another linked
    /// slot is not: the API refuses such chains.
    pub fn linked(&self, index: usize) -> bool {
        let count = self.slots.len();
        count > 1
            && self.slots[index].linked_to_next
            && !self.slots[(index + 1) % count].linked_to_next
    }
}

/// Everything rotation needs to know about the station.
#[derive(Clone, Debug)]
pub struct Library {
    pub tracks: HashMap<TrackId, Track>,
    pub categories: HashMap<CategoryId, Category>,
    /// Artists separation treats as the same artist, both ways.
    pub similar: HashMap<ArtistId, Vec<ArtistId>>,
    pub restrictions: Vec<Restriction>,
    pub clocks: HashMap<ClockId, Clock>,
    /// The default schedule.
    pub schedule: Schedule,
    /// The other schedules, which play during their periods.
    pub schedules: Vec<Schedule>,
    pub settings: RotationSettings,
    pub time_zone: TimeZone,
    /// Categories whose type never plays twice in a row, and their type
    /// (`docs/rotation.md#never-two-in-a-row`).
    pub no_two_in_a_row: HashMap<CategoryId, TypeId>,
    /// The station's play ratio counters, 10 down to 1; a track missing
    /// here is at 10.
    pub counters: HashMap<TrackId, u8>,
    /// Categories of a module turned off for the station (its medley
    /// category, while medleys are off): clock slots calling them are
    /// passed over, never relaxed.
    pub modules_off: HashSet<CategoryId>,
}

/// Library pieces, before [`Library::new`] ties them together.
#[derive(Clone, Debug, Default)]
pub struct LibraryParts {
    pub tracks: Vec<Track>,
    pub categories: Vec<Category>,
    /// Pairs of similar artists, in either direction.
    pub similar: Vec<(ArtistId, ArtistId)>,
    pub restrictions: Vec<Restriction>,
    pub clocks: Vec<Clock>,
    /// The default schedule.
    pub schedule: Schedule,
    /// The other schedules, which play during their periods.
    pub schedules: Vec<Schedule>,
    pub settings: RotationSettings,
    /// Categories whose type never plays twice in a row, and their type.
    pub no_two_in_a_row: Vec<(CategoryId, TypeId)>,
    /// The station's play ratio counters.
    pub counters: HashMap<TrackId, u8>,
    /// The station's own play ratios, in tenths, in place of the tracks'
    /// defaults; 0 disables the track on the station
    /// ([`ratio::on_station`]).
    pub station_ratios: HashMap<TrackId, u8>,
    /// Categories of a module turned off for the station.
    pub modules_off: Vec<CategoryId>,
}

impl Library {
    /// Ties the pieces together: tracks take the station's ratios and
    /// those it disabled are left out, categories keep only playable
    /// tracks, and tracks learn their categories.
    pub fn new(parts: LibraryParts, time_zone: TimeZone) -> Self {
        let mut tracks: HashMap<TrackId, Track> = parts
            .tracks
            .into_iter()
            .filter_map(|mut track| {
                track.categories.clear();
                track.ratio =
                    ratio::on_station(track.ratio, parts.station_ratios.get(&track.id).copied())?;
                Some((track.id, track))
            })
            .collect();
        let mut categories = HashMap::new();
        for mut category in parts.categories {
            category.members.retain(|member| {
                tracks.get_mut(&member.track).is_some_and(|track| {
                    if !track.categories.contains(&category.id) {
                        track.categories.push(category.id);
                    }
                    true
                })
            });
            categories.insert(category.id, category);
        }
        let mut similar: HashMap<ArtistId, Vec<ArtistId>> = HashMap::new();
        for (a, b) in parts.similar {
            if a == b {
                continue;
            }
            for (from, to) in [(a, b), (b, a)] {
                let list = similar.entry(from).or_default();
                if !list.contains(&to) {
                    list.push(to);
                }
            }
        }
        Self {
            tracks,
            categories,
            similar,
            restrictions: parts.restrictions,
            clocks: parts.clocks.into_iter().map(|c| (c.id, c)).collect(),
            schedule: parts.schedule,
            schedules: parts.schedules,
            settings: parts.settings,
            time_zone,
            no_two_in_a_row: parts.no_two_in_a_row.into_iter().collect(),
            counters: parts.counters,
            modules_off: parts.modules_off.into_iter().collect(),
        }
    }

    /// The track's play ratio counter on the station, 10 down to 1.
    pub fn counter(&self, track: TrackId) -> u8 {
        self.counters
            .get(&track)
            .copied()
            .filter(|counter| (1..=ratio::FULL).contains(counter))
            .unwrap_or(ratio::FULL)
    }

    /// Moves the counters of the tracks rotation offered (picked, or held
    /// back by their ratio) one step down, and returns their new values.
    pub fn count_down(&mut self, offered: impl IntoIterator<Item = TrackId>) -> Vec<(TrackId, u8)> {
        offered
            .into_iter()
            .map(|track| {
                let counter = ratio::after(self.counter(track));
                self.counters.insert(track, counter);
                (track, counter)
            })
            .collect()
    }

    /// The type that would play twice in a row: one that never does, of a
    /// category of `categories` and of a category of the track before
    /// (`previous`, `None` when unknown).
    pub fn repeated_type(
        &self,
        previous: Option<&[CategoryId]>,
        categories: &[CategoryId],
    ) -> Option<TypeId> {
        let previous = previous?;
        categories
            .iter()
            .filter_map(|category| self.no_two_in_a_row.get(category))
            .find(|kind| {
                previous
                    .iter()
                    .any(|category| self.no_two_in_a_row.get(category) == Some(*kind))
            })
            .copied()
    }

    /// Whether separation counts `a` and `b` as the same artist.
    pub fn same_artist(&self, a: ArtistId, b: ArtistId) -> bool {
        a == b || self.similar.get(&a).is_some_and(|list| list.contains(&b))
    }

    /// The category relaxation ends in; `None` for the station's library.
    pub fn fallback_category(&self) -> Option<&Category> {
        self.settings
            .fallback_category_id
            .and_then(|id| self.categories.get(&id))
    }

    /// Whether `track` is in the station's library: in one of its
    /// categories and in none it keeps out. A station with no category
    /// yet has the whole library.
    pub fn in_station_library(&self, track: &Track) -> bool {
        if self.categories.is_empty() {
            return true;
        }
        !track.categories.is_empty()
            && !track
                .categories
                .iter()
                .any(|category| self.settings.kept_out_category_ids.contains(category))
    }

    /// The schedule playing at `at`: the default one, or another during
    /// its periods.
    pub fn schedule_at(&self, at: &jiff::Zoned) -> &Schedule {
        schedule::playing(&self.schedule, &self.schedules, at.datetime())
    }

    /// The clock scheduled at `at`, by the schedule playing then.
    pub fn clock_at(&self, at: &jiff::Zoned) -> Option<&Clock> {
        let day = at.weekday().to_monday_one_offset() as u8;
        let hour = at.hour() as u8;
        self.schedule_at(at)
            .hours
            .get(&(day, hour))
            .and_then(|id| self.clocks.get(id))
            .filter(|clock| !clock.slots.is_empty())
    }

    /// Moves the track's weights after a play or a request.
    pub fn reweigh(&mut self, track: TrackId, factor: f64) {
        if factor == 1.0 {
            return;
        }
        let Some(categories) = self.tracks.get(&track).map(|t| t.categories.clone()) else {
            return;
        };
        for id in categories {
            if let Some(category) = self.categories.get_mut(&id) {
                for member in category.members.iter_mut().filter(|m| m.track == track) {
                    member.weight = (member.weight * factor).clamp(MIN_WEIGHT, MAX_WEIGHT);
                }
            }
        }
    }

    /// The station's division of the day: 4 or 6 dayparts.
    pub fn daypart_count(&self) -> u8 {
        dayparts::count(self.settings.daypart_count)
    }

    /// The station's windows, overridden by a category, then by a clock
    /// slot. The daypart separation and the same daypart on the following
    /// days are the category's only (none without one), the separation cut
    /// to the longest the station's division allows.
    pub fn windows(
        &self,
        category: Option<&SeparationOverrides>,
        slot: Option<&SeparationOverrides>,
    ) -> Windows {
        let mut windows = Windows::from(self.settings.separation);
        if let Some(category) = category {
            windows = windows.with(category);
        }
        if let Some(slot) = slot {
            windows = windows.with(&SeparationOverrides {
                dayparts: None,
                daypart_days: None,
                ..*slot
            });
        }
        windows.dayparts = windows
            .dayparts
            .min(dayparts::max_separation(self.daypart_count()));
        windows.daypart_days = windows.daypart_days.min(dayparts::MAX_DAYS);
        windows
    }

    /// The station's draw depth, overridden by a category, then by a clock
    /// slot; a start above the maximum is at the maximum.
    pub fn draw(
        &self,
        category: Option<&SeparationOverrides>,
        slot: Option<&SeparationOverrides>,
    ) -> DrawDepth {
        let mut draw = self.settings.draw;
        for overrides in [category, slot].into_iter().flatten() {
            draw = draw.with(overrides);
        }
        draw.clamped()
    }

    /// `windows`, a slot's before relaxation, with the track's own daypart
    /// separation and same daypart on the following days in place of the
    /// category's, cut like them to the station's division.
    pub fn for_track(&self, mut windows: Windows, track: &Track) -> Windows {
        if let Some(separation) = track.dayparts {
            windows.dayparts = separation.min(dayparts::max_separation(self.daypart_count()));
        }
        if let Some(days) = track.daypart_days {
            windows.daypart_days = days.min(dayparts::MAX_DAYS);
        }
        windows
    }

    /// How far back separation can look, in ms: the longest window the
    /// settings, a category or a slot uses (an hour at least), or the
    /// dayparts the longest daypart separation covers, or the days the
    /// same daypart is kept out on (a category's or a track's own).
    pub fn horizon_ms(&self) -> i64 {
        let count = self.daypart_count();
        let reach = |windows: Windows| {
            windows
                .longest_min()
                .max(dayparts::lookback_min(count, windows.dayparts))
                .max(dayparts::days_lookback_min(count, windows.daypart_days))
        };
        let mut longest = reach(self.windows(None, None));
        for category in self.categories.values() {
            longest = longest.max(reach(self.windows(Some(&category.separation), None)));
        }
        for slot in self.clocks.values().flat_map(|clock| &clock.slots) {
            let category = self.categories.get(&slot.category).map(|c| &c.separation);
            longest = longest.max(reach(self.windows(category, Some(&slot.separation))));
        }
        // Tracks with their own daypart values.
        for track in self.tracks.values() {
            if track.dayparts.is_some() || track.daypart_days.is_some() {
                longest = longest.max(reach(self.for_track(Windows::OFF, track)));
            }
        }
        i64::from(longest.max(60)) * 60_000
    }

    /// How far back and ahead the rules across stations look, in ms.
    pub fn across_horizon_ms(&self) -> i64 {
        i64::from(self.settings.across.windows().longest_min()) * 60_000
    }

    /// Station time at `at_ms` (Unix milliseconds).
    pub fn local(&self, at_ms: i64) -> jiff::Zoned {
        Timestamp::from_millisecond(at_ms)
            .unwrap_or(Timestamp::UNIX_EPOCH)
            .to_zoned(self.time_zone.clone())
    }
}

/// A track that played, or is queued to play at a planned time.
#[derive(Clone, Debug, PartialEq)]
pub struct Play {
    /// Missing for files outside the library and deleted tracks.
    pub track: Option<TrackId>,
    pub artists: Vec<ArtistId>,
    pub title_key: Option<String>,
    pub album_key: Option<String>,
    pub categories: Vec<CategoryId>,
    pub start_ms: i64,
}

impl Play {
    pub fn of(track: &Track, start_ms: i64) -> Self {
        Self {
            track: Some(track.id),
            artists: track.artists.clone(),
            title_key: track.title_key.clone(),
            album_key: track.album_key.clone(),
            categories: track.categories.clone(),
            start_ms,
        }
    }
}

/// Another station's play, or a track in its queue at its planned time.
#[derive(Clone, Debug, PartialEq)]
pub struct Elsewhere {
    pub station_id: StationId,
    /// Queued, not played yet.
    pub planned: bool,
    pub play: Play,
}

/// What was played (and what is queued), as far back as separation looks,
/// and when each track last played; and what the other stations played
/// and will play, for the rules across stations.
#[derive(Clone, Debug, Default)]
pub struct History {
    plays: Vec<Play>,
    last_played: HashMap<TrackId, i64>,
    elsewhere: Vec<Elsewhere>,
}

impl History {
    /// `last_played` may go further back than `plays`.
    pub fn new(mut plays: Vec<Play>, mut last_played: HashMap<TrackId, i64>) -> Self {
        plays.sort_by_key(|play| play.start_ms);
        for play in &plays {
            if let Some(track) = play.track {
                let last = last_played.entry(track).or_insert(play.start_ms);
                *last = (*last).max(play.start_ms);
            }
        }
        Self {
            plays,
            last_played,
            elsewhere: Vec::new(),
        }
    }

    /// With the other stations' plays and planned tracks.
    #[must_use]
    pub fn with_elsewhere(mut self, mut elsewhere: Vec<Elsewhere>) -> Self {
        elsewhere.sort_by_key(|other| other.play.start_ms);
        self.elsewhere = elsewhere;
        self
    }

    pub fn elsewhere(&self) -> &[Elsewhere] {
        &self.elsewhere
    }

    pub fn push(&mut self, play: Play) {
        if let Some(track) = play.track {
            let last = self.last_played.entry(track).or_insert(play.start_ms);
            *last = (*last).max(play.start_ms);
        }
        let at = self
            .plays
            .partition_point(|other| other.start_ms <= play.start_ms);
        self.plays.insert(at, play);
    }

    pub fn plays(&self) -> &[Play] {
        &self.plays
    }

    pub fn last_played(&self, track: TrackId) -> Option<i64> {
        self.last_played.get(&track).copied()
    }

    /// The play just before `at_ms`.
    pub fn previous(&self, at_ms: i64) -> Option<&Play> {
        let end = self.plays.partition_point(|play| play.start_ms < at_ms);
        end.checked_sub(1).map(|index| &self.plays[index])
    }

    /// Forgets plays started before `ms` (their last-played times stay).
    pub fn forget_before(&mut self, ms: i64) {
        let end = self.plays.partition_point(|play| play.start_ms < ms);
        self.plays.drain(..end);
    }
}

/// Where the clock is: the slot of the last rotation pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Position {
    /// Start of the station hour the clock runs for (Unix ms). An hour
    /// repeated when the clocks go back runs its clock again.
    pub hour_start_unix_ms: i64,
    pub clock_id: ClockId,
    /// Index of the slot, from 0.
    pub slot: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similar_artists_are_similar_both_ways() {
        let library = Library::new(
            LibraryParts {
                similar: vec![(1, 2), (2, 1), (3, 3), (2, 4)],
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        );
        assert!(library.same_artist(1, 2) && library.same_artist(2, 1));
        assert!(library.same_artist(4, 2));
        // Not transitive: 1 and 4 are both similar to 2 only.
        assert!(!library.same_artist(1, 4));
        assert_eq!(library.similar[&2].len(), 2);
    }

    #[test]
    fn categories_keep_playable_tracks_only() {
        let track = |id| Track {
            id,
            title_key: None,
            album_key: None,
            artists: vec![],
            categories: vec![],
            length_ms: 180_000,
            ratio: 10,
            dayparts: None,
            daypart_days: None,
        };
        let library = Library::new(
            LibraryParts {
                tracks: vec![track(1), track(2)],
                categories: vec![Category {
                    id: 9,
                    name: "Gold".into(),
                    separation: SeparationOverrides::default(),
                    members: [1, 3].map(|track| Member { track, weight: 1.0 }).to_vec(),
                }],
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        );
        assert_eq!(library.categories[&9].members.len(), 1);
        assert_eq!(library.tracks[&1].categories, [9]);
        assert!(library.tracks[&2].categories.is_empty());
    }

    #[test]
    fn history_keeps_plays_in_order() {
        let play = |track, start_ms| Play {
            track: Some(track),
            artists: vec![],
            title_key: None,
            album_key: None,
            categories: vec![],
            start_ms,
        };
        let mut history = History::new(vec![play(1, 300), play(2, 100)], HashMap::from([(1, 50)]));
        assert_eq!(history.last_played(1), Some(300));
        history.push(play(3, 200));
        let order: Vec<_> = history.plays().iter().map(|p| p.track).collect();
        assert_eq!(order, [Some(2), Some(3), Some(1)]);
        assert_eq!(history.previous(300).and_then(|p| p.track), Some(3));
        assert!(history.previous(100).is_none());
        history.forget_before(250);
        assert_eq!(history.plays().len(), 1);
        assert_eq!(history.last_played(2), Some(100));
    }
}
