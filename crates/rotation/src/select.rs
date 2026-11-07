// SPDX-License-Identifier: AGPL-3.0-or-later

//! Picking the next track: which slot, which rules, which relaxation.

use std::collections::HashMap;

use jiff::civil::DateTime;
use serde::{Deserialize, Serialize};

use crate::dayparts::{self, DaypartLog};
use crate::ratio::{self, HeldByRatio};
use crate::restrict::{Subject, forbidding};
use crate::rules::{AcrossWindows, FALLBACK_STEP, LAST_STEP, Windows, describe_step};
use crate::{
    ArtistId, CategoryId, ClockId, Elsewhere, History, Library, Method, Position, Rng, ScheduleId,
    StationId, Track, TrackId,
};

/// The next track, where it sits in the clock and why it was picked.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub track: TrackId,
    /// Missing when no clock is scheduled.
    pub position: Option<Position>,
    pub explanation: Explanation,
    /// The track of the next slot, picked with this one because this
    /// pick's slot is linked to it: it plays right after this track.
    pub linked: Option<Box<Pick>>,
}

impl Pick {
    /// This pick, then the one linked to it, in play order.
    pub fn into_picks(mut self) -> Vec<Pick> {
        let linked = self.linked.take();
        let mut picks = vec![self];
        if let Some(linked) = linked {
            picks.extend(linked.into_picks());
        }
        picks
    }
}

/// Why a track was picked: the rotation log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Explanation {
    /// When the track is planned to start (Unix ms).
    pub at_unix_ms: i64,
    /// The schedule playing at `at_unix_ms`: the default one, or another
    /// during its periods. Missing in picks logged before schedules had
    /// periods.
    #[serde(default)]
    pub schedule_id: Option<ScheduleId>,
    #[serde(default)]
    pub schedule_name: Option<String>,
    /// Missing when no clock is scheduled: the station's library rotates.
    pub clock_id: Option<ClockId>,
    pub clock_name: Option<String>,
    /// Index of the slot in the clock, from 0.
    pub slot: Option<u32>,
    /// Category the slot calls.
    pub slot_category_id: Option<CategoryId>,
    /// Category the track was picked from: the fallback category from
    /// step 9; missing for the station's library.
    pub category_id: Option<CategoryId>,
    pub category_name: Option<String>,
    /// Applied: least recently played at step 10.
    pub method: Method,
    pub enforce_rules: bool,
    /// Relaxation step, 0 (none) to 12.
    pub step: u8,
    pub step_description: String,
    /// Separation windows applied at this step.
    pub windows: Windows,
    /// Windows across stations applied at this step.
    #[serde(default)]
    pub across: AcrossWindows,
    /// Tracks in the slot's category.
    pub pool: u32,
    /// Of them, forbidden by restrictions.
    pub restricted: u32,
    /// Tracks of the slot's category held back by each separation rule
    /// with the rules as configured (the first rule that applies counts).
    pub blocked: Blocked,
    /// Tracks the method chose among.
    pub candidates: u32,
    /// When the picked track last played; missing if never.
    pub last_played_unix_ms: Option<i64>,
    /// Weight of the picked track in its category.
    pub weight: f64,
    /// Slots passed over because nothing could be picked, even relaxed.
    pub skipped_slots: Vec<SkippedSlot>,
    /// Of the tracks the rules across stations held back with the windows
    /// as configured, the least recently played, and what held it back.
    #[serde(default)]
    pub held_elsewhere: Option<HeldElsewhere>,
    /// The picked track's play ratio on the station, in tenths, as rotation
    /// used it: the station's own, else the track's default. Missing in
    /// picks logged before play ratios per station.
    #[serde(default)]
    pub ratio: Option<u8>,
    /// The picked track's play ratio counter when it was picked, 10 down
    /// to 1. Missing in picks logged before play ratios.
    #[serde(default)]
    pub counter: Option<u8>,
    /// Tracks offered before this one and held back by their play ratio,
    /// in the order they were offered.
    #[serde(default)]
    pub held_by_ratio: Vec<HeldByRatio>,
    /// The pick's daypart, and the one the track last played in. Missing
    /// in picks logged before daypart separation.
    #[serde(default)]
    pub daypart: Option<DaypartLog>,
    /// The slot is linked to the next one: the next pick is that slot's
    /// track, picked with this one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linked: bool,
}

/// A track another station's play or planned track held back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HeldElsewhere {
    pub track_id: TrackId,
    pub rule: SeparationRule,
    pub station_id: StationId,
    /// When the other station played it, or plans to (Unix ms).
    pub at_unix_ms: i64,
    /// In the other station's queue, not played yet.
    pub planned: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Blocked {
    pub track: u32,
    pub title: u32,
    pub artist: u32,
    pub album: u32,
    /// By daypart separation. Missing in picks logged before it.
    #[serde(default)]
    pub daypart: u32,
    /// By the same daypart on the following days (and not by daypart
    /// separation). Missing in picks logged before it.
    #[serde(default)]
    pub daypart_days: u32,
    /// By the rules across stations (and by none of the station's own).
    #[serde(default)]
    pub across: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SkippedSlot {
    pub slot: u32,
    pub category_id: CategoryId,
    /// Missing in picks logged before reasons were (nothing allowed).
    #[serde(default)]
    pub reason: SkipReason,
}

/// Why a slot was passed over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Nothing could be picked, even relaxed.
    #[default]
    NothingAllowed,
    /// The track before is of the slot category's type, which never plays
    /// twice in a row (two jingles, for instance).
    SameTypeInARow,
    /// The slot calls the category of a module turned off for the station
    /// (its medley category, while medleys are off).
    ModuleOff,
    /// The slot and the next one, which it is linked to: the next slot's
    /// track would start after the end of the hour (or in another clock).
    LinkedTrackAfterHour,
    /// The slot is linked to the next one, which found nothing.
    LinkedTrackNotFound,
}

/// A separation rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SeparationRule {
    Track,
    Title,
    Artist,
    Album,
    /// Daypart separation: in a daypart kept out after the track's last
    /// play.
    Daypart,
    /// The same daypart on the following days: in the daypart the track
    /// played in on one of the days before.
    DaypartDays,
}

/// When each track, title, album and artist last started, up to `at`.
struct Recent<'h> {
    tracks: HashMap<TrackId, i64>,
    titles: HashMap<&'h str, i64>,
    albums: HashMap<&'h str, i64>,
    artists: HashMap<ArtistId, i64>,
    /// The station's division of the day, and `at` in station time.
    daypart_count: u8,
    local: DateTime,
    /// The last day (see [`dayparts::day`]) each track played in the
    /// daypart of `at`, before `at`'s day, within the days looked at.
    same_daypart: HashMap<TrackId, i64>,
}

impl<'h> Recent<'h> {
    /// `days`: how many days back the same daypart is looked at; 0 skips
    /// it.
    fn new(library: &Library, history: &'h History, at_ms: i64, days: u8) -> Self {
        let mut recent = Self {
            tracks: HashMap::new(),
            titles: HashMap::new(),
            albums: HashMap::new(),
            artists: HashMap::new(),
            daypart_count: library.daypart_count(),
            local: library.local(at_ms).datetime(),
            same_daypart: HashMap::new(),
        };
        // Only the plays that can be in the daypart on one of those days
        // are turned into station times.
        let since =
            at_ms - i64::from(dayparts::days_lookback_min(recent.daypart_count, days)) * 60_000;
        let daypart = dayparts::of(recent.daypart_count, recent.local);
        let today = dayparts::day(recent.local);
        // In order: later plays overwrite earlier ones.
        for play in history.plays().iter().take_while(|p| p.start_ms <= at_ms) {
            if let Some(track) = play.track {
                recent.tracks.insert(track, play.start_ms);
                if days > 0 && play.start_ms >= since {
                    let local = library.local(play.start_ms).datetime();
                    let day = dayparts::day(local);
                    if day < today && dayparts::of(recent.daypart_count, local) == daypart {
                        recent.same_daypart.insert(track, day);
                    }
                }
            }
            if let Some(title) = &play.title_key {
                recent.titles.insert(title, play.start_ms);
            }
            if let Some(album) = &play.album_key {
                recent.albums.insert(album, play.start_ms);
            }
            for artist in &play.artists {
                recent.artists.insert(*artist, play.start_ms);
            }
        }
        recent
    }

    /// The first rule `windows` hold `track` back by at `at_ms`.
    fn blocking(
        &self,
        library: &Library,
        track: &Track,
        windows: Windows,
        at_ms: i64,
    ) -> Option<SeparationRule> {
        let within = |last: Option<&i64>, minutes: u32| {
            minutes > 0 && last.is_some_and(|last| at_ms - last < i64::from(minutes) * 60_000)
        };
        if within(self.tracks.get(&track.id), windows.track_min) {
            return Some(SeparationRule::Track);
        }
        let key_within = |map: &HashMap<&str, i64>, key: &Option<String>, minutes| {
            key.as_deref()
                .is_some_and(|key| within(map.get(key), minutes))
        };
        if key_within(&self.titles, &track.title_key, windows.title_min) {
            return Some(SeparationRule::Title);
        }
        if windows.artist_min > 0 {
            let artist_within =
                |artist: &ArtistId| within(self.artists.get(artist), windows.artist_min);
            let held = track.artists.iter().any(|artist| {
                artist_within(artist)
                    || library
                        .similar
                        .get(artist)
                        .is_some_and(|similar| similar.iter().any(artist_within))
            });
            if held {
                return Some(SeparationRule::Artist);
            }
        }
        if windows.dayparts > 0
            && self.tracks.get(&track.id).is_some_and(|last| {
                dayparts::holds(
                    self.daypart_count,
                    windows.dayparts,
                    library.local(*last).datetime(),
                    self.local,
                )
            })
        {
            return Some(SeparationRule::Daypart);
        }
        if windows.daypart_days > 0
            && self.same_daypart.get(&track.id).is_some_and(|day| {
                dayparts::day(self.local) - day <= i64::from(windows.daypart_days)
            })
        {
            return Some(SeparationRule::DaypartDays);
        }
        if key_within(&self.albums, &track.album_key, windows.album_min) {
            return Some(SeparationRule::Album);
        }
        None
    }
}

/// The rule across stations, and the other station's play or planned
/// track, that hold `track` back at `at_ms`: within a window before or
/// after it.
pub(crate) fn held_elsewhere<'h>(
    library: &Library,
    history: &'h History,
    track: &Track,
    windows: AcrossWindows,
    at_ms: i64,
) -> Option<(SeparationRule, &'h Elsewhere)> {
    let longest = i64::from(windows.longest_min()) * 60_000;
    if longest == 0 {
        return None;
    }
    let elsewhere = history.elsewhere();
    let from = elsewhere.partition_point(|other| other.play.start_ms <= at_ms - longest);
    let near = |minutes: u32, start: i64| {
        minutes > 0 && (at_ms - start).abs() < i64::from(minutes) * 60_000
    };
    let same_key = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
    elsewhere[from..]
        .iter()
        .take_while(|other| other.play.start_ms < at_ms + longest)
        .find_map(|other| {
            let play = &other.play;
            let start = play.start_ms;
            let rule = if near(windows.track_min, start) && play.track == Some(track.id) {
                SeparationRule::Track
            } else if near(windows.title_min, start) && same_key(&track.title_key, &play.title_key)
            {
                SeparationRule::Title
            } else if near(windows.artist_min, start)
                && track
                    .artists
                    .iter()
                    .any(|a| play.artists.iter().any(|b| library.same_artist(*a, *b)))
            {
                SeparationRule::Artist
            } else {
                return None;
            };
            Some((rule, other))
        })
}

/// A category as a slot calls it.
struct Call<'l> {
    category_id: Option<CategoryId>,
    category_name: Option<String>,
    /// Tracks and their weights, in a fixed order.
    pool: Vec<(&'l Track, f64)>,
    windows: Windows,
}

impl<'l> Call<'l> {
    /// `None`: the station's library.
    fn new(
        library: &'l Library,
        category: Option<CategoryId>,
        extra: Option<&crate::SeparationOverrides>,
    ) -> Self {
        match category {
            Some(id) => {
                let category = library.categories.get(&id);
                let windows = library.windows(category.map(|c| &c.separation), extra);
                Self {
                    category_id: Some(id),
                    category_name: category.map(|c| c.name.clone()),
                    pool: category
                        .into_iter()
                        .flat_map(|c| &c.members)
                        .filter_map(|m| library.tracks.get(&m.track).map(|t| (t, m.weight)))
                        .collect(),
                    windows,
                }
            }
            None => {
                let mut pool: Vec<_> = library
                    .tracks
                    .values()
                    .filter(|track| library.in_station_library(track))
                    .map(|t| (t, 1.0))
                    .collect();
                pool.sort_by_key(|(track, _)| track.id);
                Self {
                    category_id: None,
                    category_name: None,
                    pool,
                    windows: library.windows(None, None),
                }
            }
        }
    }

    /// The tracks restrictions allow at `at`, and how many they forbid.
    fn allowed(
        &self,
        library: &Library,
        at: DateTime,
        previous: Option<&[CategoryId]>,
    ) -> (Vec<(&'l Track, f64)>, u32) {
        let mut restricted = 0;
        let allowed = self
            .pool
            .iter()
            .filter(|(track, _)| {
                let subject = Subject {
                    track: track.id,
                    artists: &track.artists,
                    categories: &track.categories,
                };
                let forbidden = forbidding(&library.restrictions, &subject, at, previous).is_some()
                    || library.repeated_type(previous, &track.categories).is_some();
                restricted += u32::from(forbidden);
                !forbidden
            })
            .copied()
            .collect();
        (allowed, restricted)
    }
}

/// The relaxation steps a slot goes through.
fn steps(enforce_rules: bool) -> Vec<u8> {
    if enforce_rules {
        (0..=LAST_STEP).collect()
    } else {
        vec![0, FALLBACK_STEP, LAST_STEP]
    }
}

fn choose<'l>(
    method: Method,
    candidates: &[(&'l Track, f64)],
    history: &History,
    rng: &mut Rng,
) -> (&'l Track, f64) {
    match method {
        Method::LeastRecentlyPlayed => {
            // Never played first, then the oldest; ties at random.
            let key = |track: &Track| history.last_played(track.id).unwrap_or(i64::MIN);
            let oldest = candidates
                .iter()
                .map(|(track, _)| key(track))
                .min()
                .unwrap_or(i64::MIN);
            let ties: Vec<_> = candidates
                .iter()
                .filter(|(track, _)| key(track) == oldest)
                .collect();
            *ties[rng.below(ties.len())]
        }
        Method::Random => candidates[rng.below(candidates.len())],
        Method::Weighted => {
            let total: f64 = candidates.iter().map(|(_, weight)| weight.max(0.0)).sum();
            let mut target = rng.unit() * total;
            for candidate in candidates {
                target -= candidate.1.max(0.0);
                if target < 0.0 {
                    return *candidate;
                }
            }
            candidates[candidates.len() - 1]
        }
    }
}

/// How a slot calls its category.
struct SlotCall<'l> {
    call: Call<'l>,
    method: Method,
    enforce_rules: bool,
}

/// Tries the relaxation steps in order and picks with the slot's method
/// among the first step's candidates.
///
/// The play ratio decides after the method: a track it holds back is added
/// to `held` (its counter moves, see [`Library::count_down`]) and the method
/// picks again among the others. Tracks in `held` are never offered again.
fn fill(
    library: &Library,
    history: &History,
    at_ms: i64,
    slot: &SlotCall<'_>,
    held_by_ratio: &mut Vec<HeldByRatio>,
    rng: &mut Rng,
) -> Option<(TrackId, Explanation)> {
    let local = library.local(at_ms).datetime();
    let previous = history
        .previous(at_ms)
        .map(|play| play.categories.as_slice());
    let recent = Recent::new(library, history, at_ms, slot.call.windows.daypart_days);
    let (allowed, restricted) = slot.call.allowed(library, local, previous);
    let across = if slot.enforce_rules {
        library.settings.across.windows()
    } else {
        AcrossWindows::OFF
    };
    let mut blocked = Blocked::default();
    let mut held: Option<(Option<i64>, HeldElsewhere)> = None;
    if slot.enforce_rules {
        for (track, _) in &allowed {
            match recent.blocking(library, track, slot.call.windows, at_ms) {
                Some(SeparationRule::Track) => blocked.track += 1,
                Some(SeparationRule::Title) => blocked.title += 1,
                Some(SeparationRule::Artist) => blocked.artist += 1,
                Some(SeparationRule::Album) => blocked.album += 1,
                Some(SeparationRule::Daypart) => blocked.daypart += 1,
                Some(SeparationRule::DaypartDays) => blocked.daypart_days += 1,
                None => {
                    if let Some((rule, other)) =
                        held_elsewhere(library, history, track, across, at_ms)
                    {
                        blocked.across += 1;
                        let last = history.last_played(track.id);
                        // Never played first, then the oldest; then by ID.
                        let wanted = held.as_ref().is_none_or(|(held_last, held)| {
                            (last, track.id) < (*held_last, held.track_id)
                        });
                        if wanted {
                            held = Some((
                                last,
                                HeldElsewhere {
                                    track_id: track.id,
                                    rule,
                                    station_id: other.station_id,
                                    at_unix_ms: other.play.start_ms,
                                    planned: other.planned,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }
    let mut fallback: Option<(Call<'_>, Vec<(&Track, f64)>)> = None;
    for step in steps(slot.enforce_rules) {
        let across = if step < FALLBACK_STEP {
            across.at_step(step)
        } else {
            AcrossWindows::OFF
        };
        let (call, pool, windows) = if step < FALLBACK_STEP {
            let windows = if slot.enforce_rules {
                slot.call.windows.at_step(step)
            } else {
                Windows::OFF
            };
            (&slot.call, &allowed, windows)
        } else {
            let (call, pool) = fallback.get_or_insert_with(|| {
                let call = Call::new(library, library.fallback_category().map(|c| c.id), None);
                let (pool, _) = call.allowed(library, local, previous);
                (call, pool)
            });
            let windows = call.windows.at_step(step);
            (&*call, &*pool, windows)
        };
        let mut candidates: Vec<_> = pool
            .iter()
            .filter(|(track, _)| {
                !held_by_ratio.iter().any(|held| held.track_id == track.id)
                    && recent.blocking(library, track, windows, at_ms).is_none()
                    && held_elsewhere(library, history, track, across, at_ms).is_none()
            })
            .copied()
            .collect();
        let count = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
        let method = if step == LAST_STEP {
            Method::LeastRecentlyPlayed
        } else {
            slot.method
        };
        // The ratio is never relaxed: tracks it holds back leave the slot
        // to the others, then to the next step.
        let picked = loop {
            if candidates.is_empty() {
                break None;
            }
            let (track, weight) = choose(method, &candidates, history, rng);
            let counter = library.counter(track.id);
            if ratio::plays(track.ratio, counter) {
                break Some((track, weight, counter));
            }
            held_by_ratio.push(HeldByRatio {
                track_id: track.id,
                ratio: track.ratio,
                counter,
            });
            candidates.retain(|(other, _)| other.id != track.id);
        };
        let Some((track, weight, counter)) = picked else {
            continue;
        };
        let explanation = Explanation {
            at_unix_ms: at_ms,
            schedule_id: None,
            schedule_name: None,
            clock_id: None,
            clock_name: None,
            slot: None,
            slot_category_id: slot.call.category_id,
            category_id: call.category_id,
            category_name: call.category_name.clone(),
            method,
            enforce_rules: slot.enforce_rules,
            step,
            step_description: describe_step(step).to_owned(),
            windows,
            across,
            pool: u32::try_from(slot.call.pool.len()).unwrap_or(u32::MAX),
            restricted,
            blocked,
            candidates: count,
            last_played_unix_ms: history.last_played(track.id),
            weight,
            skipped_slots: Vec::new(),
            held_elsewhere: held.map(|(_, held)| held),
            ratio: Some(track.ratio),
            counter: Some(counter),
            held_by_ratio: Vec::new(),
            daypart: Some(DaypartLog {
                count: recent.daypart_count,
                daypart: dayparts::of(recent.daypart_count, local),
                separation: slot.call.windows.dayparts,
                days: slot.call.windows.daypart_days,
                last_daypart: history
                    .last_played(track.id)
                    .map(|last| dayparts::of(recent.daypart_count, library.local(last).datetime())),
            }),
            linked: false,
        };
        return Some((track.id, explanation));
    }
    None
}

/// The track to play at `at_ms` (Unix ms), after `history`, the last
/// rotation pick having been at `last`.
///
/// - The clock scheduled for the station hour of `at_ms` runs its slots in
///   order, from the first one at each new hour, and starts over when it
///   runs out before the hour ends. A looping clock carries on from its
///   last slot instead, whatever the hour.
/// - Without a scheduled clock, the station's library rotates: least recently
///   played, rules enforced.
/// - `None` only when restrictions (or play ratios) forbid every track a
///   slot, and the fallback, could offer: the engine's fallback directory
///   takes over.
///
/// The play ratio counters do not move: see [`next_outcome`].
pub fn next(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    rng: &mut Rng,
) -> Option<Pick> {
    next_outcome(library, history, at_ms, last, rng).pick
}

/// What a call to rotation gave: the pick, if any, and the tracks offered
/// and held back by their play ratio (also in the pick's explanation).
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub pick: Option<Pick>,
    pub held_by_ratio: Vec<HeldByRatio>,
}

impl Outcome {
    /// The tracks whose play ratio counter moves: those held back, then the
    /// one picked.
    pub fn offered(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.held_by_ratio
            .iter()
            .map(|held| held.track_id)
            .chain(self.pick.iter().flat_map(|pick| {
                std::iter::successors(Some(pick), |pick| pick.linked.as_deref())
                    .map(|pick| pick.track)
            }))
    }
}

/// As [`next`], with the tracks held back by their play ratio even when
/// nothing could be picked: pass [`Outcome::offered`] to
/// [`Library::count_down`] to move their counters.
pub fn next_outcome(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    rng: &mut Rng,
) -> Outcome {
    let mut held_by_ratio = Vec::new();
    let pick = pick(library, history, at_ms, last, &mut held_by_ratio, rng).map(|mut pick| {
        pick.explanation.held_by_ratio = held_by_ratio.clone();
        pick
    });
    Outcome {
        pick,
        held_by_ratio,
    }
}

/// A track of `category` to play at `at_ms`, picked as a slot calling that
/// category would (least recently played, rules enforced and relaxed step
/// by step), outside any clock: a show's pick (the match's two tracks,
/// `docs/listener-interaction.md#match`). Without `enforce_rules`, the
/// separation rules do not hold (an announcement jingle), as for a slot
/// that does not enforce them. Tracks in `excluded` are never offered. `None` when nothing of the category can be picked: the
/// fallback category does not stand in for it.
///
/// The play ratio decides as for a slot, but its counters do not move.
pub fn from_category(
    library: &Library,
    history: &History,
    at_ms: i64,
    category: CategoryId,
    excluded: &[TrackId],
    enforce_rules: bool,
    rng: &mut Rng,
) -> Option<Pick> {
    if library.modules_off.contains(&category) {
        return None;
    }
    let mut call = Call::new(library, Some(category), None);
    call.pool.retain(|(track, _)| !excluded.contains(&track.id));
    let slot = SlotCall {
        call,
        method: Method::LeastRecentlyPlayed,
        enforce_rules,
    };
    let mut held = Vec::new();
    let (track, mut explanation) = fill(library, history, at_ms, &slot, &mut held, rng)?;
    if explanation.category_id != Some(category) {
        return None;
    }
    let schedule = library.schedule_at(&library.local(at_ms));
    explanation.schedule_id = Some(schedule.id);
    explanation.schedule_name = Some(schedule.name.clone());
    explanation.held_by_ratio = held;
    Some(Pick {
        track,
        position: None,
        explanation,
        linked: None,
    })
}

/// Start of the station hour of `at_ms` (Unix ms).
fn hour_start_of(library: &Library, at_ms: i64) -> i64 {
    let local = library.local(at_ms);
    let into_hour = (i64::from(local.minute()) * 60 + i64::from(local.second())) * 1_000
        + i64::from(local.millisecond());
    at_ms - into_hour
}

fn pick(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    held_by_ratio: &mut Vec<HeldByRatio>,
    rng: &mut Rng,
) -> Option<Pick> {
    let local = library.local(at_ms);
    let hour_start = hour_start_of(library, at_ms);
    let named = |mut explanation: Explanation, at_ms: i64| {
        let schedule = library.schedule_at(&library.local(at_ms));
        explanation.schedule_id = Some(schedule.id);
        explanation.schedule_name = Some(schedule.name.clone());
        explanation
    };
    let Some(clock) = library.clock_at(&local) else {
        let slot = SlotCall {
            call: Call::new(library, None, None),
            method: Method::LeastRecentlyPlayed,
            enforce_rules: true,
        };
        return fill(library, history, at_ms, &slot, held_by_ratio, rng).map(
            |(track, explanation)| Pick {
                track,
                position: None,
                explanation: named(explanation, at_ms),
                linked: None,
            },
        );
    };
    let count = clock.slots.len();
    let first = match last {
        Some(last)
            if last.clock_id == clock.id
                && (clock.looping || last.hour_start_unix_ms == hour_start) =>
        {
            (last.slot as usize + 1) % count
        }
        _ => 0,
    };
    let mut skipped = Vec::new();
    // The slot at `index` at `at_ms`, after `history`: passed over (and
    // why) or picked.
    let mut try_slot = |index: usize,
                        history: &History,
                        at_ms: i64,
                        held_by_ratio: &mut Vec<HeldByRatio>|
     -> Result<Pick, SkipReason> {
        let slot = &clock.slots[index];
        // A module turned off: passed over, never relaxed.
        if library.modules_off.contains(&slot.category) {
            return Err(SkipReason::ModuleOff);
        }
        let previous = history
            .previous(at_ms)
            .map(|play| play.categories.as_slice());
        // A jingle slot after a jingle: passed over, never relaxed.
        if library
            .repeated_type(previous, std::slice::from_ref(&slot.category))
            .is_some()
        {
            return Err(SkipReason::SameTypeInARow);
        }
        let call = SlotCall {
            call: Call::new(library, Some(slot.category), Some(&slot.separation)),
            method: slot.method,
            enforce_rules: slot.enforce_rules,
        };
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        let (track, mut explanation) = fill(library, history, at_ms, &call, held_by_ratio, rng)
            .ok_or(SkipReason::NothingAllowed)?;
        explanation.clock_id = Some(clock.id);
        explanation.clock_name = Some(clock.name.clone());
        explanation.slot = Some(index);
        Ok(Pick {
            track,
            position: Some(Position {
                hour_start_unix_ms: hour_start_of(library, at_ms),
                clock_id: clock.id,
                slot: index,
            }),
            explanation: named(explanation, at_ms),
            linked: None,
        })
    };
    let skip = |skipped: &mut Vec<SkippedSlot>, index: usize, reason| {
        skipped.push(SkippedSlot {
            slot: u32::try_from(index).unwrap_or(u32::MAX),
            category_id: clock.slots[index].category,
            reason,
        });
    };
    let mut k = 0;
    while k < count {
        let index = (first + k) % count;
        k += 1;
        let mut pick = match try_slot(index, history, at_ms, held_by_ratio) {
            Ok(pick) => pick,
            Err(reason) => {
                skip(&mut skipped, index, reason);
                continue;
            }
        };
        if clock.linked(index) {
            // The next slot's track, right after this one: in this run of
            // the clock, or neither plays.
            let next = (index + 1) % count;
            let length = library.tracks.get(&pick.track).map_or(0, |t| t.length_ms);
            let next_at = at_ms + i64::from(length);
            let same_run = library
                .clock_at(&library.local(next_at))
                .is_some_and(|other| other.id == clock.id)
                && (clock.looping || hour_start_of(library, next_at) == hour_start);
            if !same_run {
                skip(&mut skipped, index, SkipReason::LinkedTrackAfterHour);
                skip(&mut skipped, next, SkipReason::LinkedTrackAfterHour);
                k += 1;
                continue;
            }
            let mut with_jingle = history.clone();
            if let Some(track) = library.tracks.get(&pick.track) {
                with_jingle.push(crate::Play::of(track, at_ms));
            }
            match try_slot(next, &with_jingle, next_at, held_by_ratio) {
                Ok(linked) => {
                    pick.explanation.linked = true;
                    pick.linked = Some(Box::new(linked));
                }
                Err(reason) => {
                    skip(&mut skipped, index, SkipReason::LinkedTrackNotFound);
                    skip(&mut skipped, next, reason);
                    k += 1;
                    continue;
                }
            }
        }
        pick.explanation.skipped_slots = skipped;
        return Some(pick);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use jiff::tz::TimeZone;

    use super::*;
    use crate::{
        Category, Clock, LibraryParts, Member, Play, Schedule, SeparationOverrides, Slot,
        StationWindows,
    };

    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;

    fn track(id: TrackId) -> Track {
        Track {
            id,
            title_key: Some(format!("title {id}")),
            album_key: Some(format!("album {id}")),
            artists: vec![100 + id],
            categories: vec![],
            length_ms: 240_000,
            ratio: 10,
        }
    }

    fn category(id: CategoryId, tracks: &[TrackId]) -> Category {
        Category {
            id,
            name: format!("C{id}"),
            separation: SeparationOverrides::default(),
            members: tracks
                .iter()
                .map(|&track| Member { track, weight: 1.0 })
                .collect(),
        }
    }

    fn slot(category: CategoryId, method: Method) -> Slot {
        Slot {
            category,
            method,
            enforce_rules: true,
            linked_to_next: false,
            separation: SeparationOverrides::default(),
        }
    }

    fn library(slots: Vec<Slot>) -> Library {
        Library::new(
            LibraryParts {
                tracks: (1..=20).map(track).collect(),
                categories: vec![
                    category(1, &[1, 2, 3, 4, 5]),
                    category(2, &[6, 7, 8, 9, 10]),
                    category(3, &[11, 12, 13, 14, 15, 16, 17, 18, 19, 20]),
                ],
                clocks: vec![Clock {
                    id: 1,
                    name: "Clock".into(),
                    looping: false,
                    slots,
                }],
                // Every hour of 1970-01-01 (a Thursday) but 02:00.
                schedule: Schedule {
                    hours: HashMap::from_iter((0..24).filter(|h| *h != 2).map(|h| ((4, h), 1))),
                    ..Schedule::default()
                },
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        )
    }

    #[test]
    fn a_category_gives_its_tracks_with_its_rules_and_nothing_else() {
        let library = library(vec![slot(1, Method::Random)]);
        let mut history = History::default();
        // Track 7 played a minute ago: track separation holds it back.
        history.push(Play::of(&library.tracks[&7], 9 * HOUR));
        let mut rng = Rng::new(3);
        for _ in 0..20 {
            let pick =
                from_category(&library, &history, 9 * HOUR + MIN, 2, &[6], true, &mut rng).unwrap();
            assert!([8, 9, 10].contains(&pick.track), "{}", pick.track);
            assert_eq!(pick.explanation.category_id, Some(2));
            assert_eq!(pick.position, None);
        }
        // Nothing left in the category: no pick, not even from the fallback.
        assert!(
            from_category(
                &library,
                &history,
                HOUR,
                2,
                &[6, 7, 8, 9, 10],
                true,
                &mut rng
            )
            .is_none()
        );
    }

    #[test]
    fn slots_run_in_order_and_start_over_each_hour() {
        let library = library(vec![
            slot(1, Method::Random),
            slot(2, Method::Random),
            slot(3, Method::Random),
        ]);
        let mut rng = Rng::new(1);
        let history = History::default();
        let first = next(&library, &history, 10 * MIN, None, &mut rng).unwrap();
        assert_eq!(first.position.unwrap().slot, 0);
        assert_eq!(first.explanation.category_id, Some(1));
        let second = next(
            &library,
            &history,
            14 * MIN,
            first.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(second.position.unwrap().slot, 1);
        let third = next(
            &library,
            &history,
            18 * MIN,
            second.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        // Runs out before the hour ends: starts over.
        let fourth = next(
            &library,
            &history,
            22 * MIN,
            third.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(fourth.position.unwrap().slot, 0);
        // A new hour starts at the first slot.
        let next_hour = next(
            &library,
            &history,
            HOUR + MIN,
            second.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(next_hour.position.unwrap().slot, 0);
        assert_eq!(next_hour.position.unwrap().hour_start_unix_ms, HOUR);
        // 02:00 has no clock: the library rotates.
        let unscheduled = next(&library, &history, 2 * HOUR + MIN, None, &mut rng).unwrap();
        assert_eq!(unscheduled.position, None);
        assert_eq!(unscheduled.explanation.category_id, None);
        assert_eq!(unscheduled.explanation.pool, 20);
    }

    #[test]
    fn looping_clocks_carry_on_across_hours() {
        let mut library = library(vec![
            slot(1, Method::Random),
            slot(2, Method::Random),
            slot(3, Method::Random),
        ]);
        library.clocks.get_mut(&1).unwrap().looping = true;
        let mut rng = Rng::new(8);
        let history = History::default();
        let first = next(&library, &history, 50 * MIN, None, &mut rng).unwrap();
        assert_eq!(first.position.unwrap().slot, 0);
        let second = next(
            &library,
            &history,
            58 * MIN,
            first.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        // The next hour carries on with the next slot, then wraps around.
        let third = next(
            &library,
            &history,
            HOUR + 2 * MIN,
            second.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(third.position.unwrap().slot, 2);
        assert_eq!(third.position.unwrap().hour_start_unix_ms, HOUR);
        let fourth = next(
            &library,
            &history,
            HOUR + 6 * MIN,
            third.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(fourth.position.unwrap().slot, 0);
        // After an unscheduled hour (02:00), it carries on where it stopped.
        let after = next(
            &library,
            &history,
            3 * HOUR,
            fourth.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(after.position.unwrap().slot, 1);
    }

    #[test]
    fn least_recently_played_prefers_never_played_then_oldest() {
        let library = library(vec![slot(1, Method::LeastRecentlyPlayed)]);
        let mut rng = Rng::new(2);
        // 1 to 4 played (long ago: no separation), 5 never.
        let last_played = HashMap::from([
            (1, -900 * MIN),
            (2, -1000 * MIN),
            (3, -800 * MIN),
            (4, -700 * MIN),
        ]);
        let history = History::new(vec![], last_played);
        let pick = next(&library, &history, 0, None, &mut rng).unwrap();
        assert_eq!(pick.track, 5);
        assert_eq!(pick.explanation.last_played_unix_ms, None);
        let mut history = history;
        history.push(Play::of(&library.tracks[&5], -720 * MIN));
        assert_eq!(
            next(&library, &history, 0, None, &mut rng).unwrap().track,
            2
        );
    }

    #[test]
    fn separation_holds_back_recent_tracks_and_explains_it() {
        let library = library(vec![slot(1, Method::Random)]);
        let mut rng = Rng::new(3);
        let mut history = History::default();
        for (i, id) in [1, 2, 3].into_iter().enumerate() {
            history.push(Play::of(&library.tracks[&id], -(i as i64 + 1) * 10 * MIN));
        }
        for _ in 0..50 {
            let pick = next(&library, &history, 0, None, &mut rng).unwrap();
            assert!([4, 5].contains(&pick.track));
            assert_eq!(pick.explanation.step, 0);
            assert_eq!(pick.explanation.candidates, 2);
            assert_eq!(pick.explanation.blocked.track, 3);
        }
    }

    #[test]
    fn relaxes_before_repeating_and_falls_back_last() {
        let library = library(vec![slot(1, Method::Random)]);
        let mut rng = Rng::new(4);
        let mut history = History::default();
        // The whole category played within the last 50 minutes.
        for id in 1..=5 {
            history.push(Play::of(&library.tracks[&id], -(id as i64) * 10 * MIN));
        }
        let pick = next(&library, &history, 0, None, &mut rng).unwrap();
        // Track separation is never dropped in the slot's category: the
        // fallback (the station's library) gives a track at step 11.
        assert_eq!(pick.explanation.step, 11);
        assert_eq!(pick.explanation.category_id, None);
        assert!(pick.track > 5);
        assert_eq!(pick.explanation.windows.track_min, 60);
        // Played 70 to 110 minutes ago: track separation quartered (60) is enough.
        let mut history = History::default();
        for id in 1..=5 {
            history.push(Play::of(&library.tracks[&id], -(id as i64 + 6) * 10 * MIN));
        }
        let pick = next(&library, &history, 0, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 10);
        assert_eq!(pick.explanation.category_id, Some(1));
    }

    #[test]
    fn hours_without_a_clock_rotate_the_station_s_library() {
        let mut library = library(vec![slot(1, Method::Random)]);
        // Track 21 is in none of the station's categories; 3 is kept out,
        // and its track 11 is in category 2 too.
        library.tracks.insert(21, track(21));
        library.tracks.get_mut(&11).unwrap().categories.push(2);
        library.settings.kept_out_category_ids = vec![3];
        let mut rng = Rng::new(7);
        let mut history = History::default();
        let mut picked = Vec::new();
        // 02:00 has no clock.
        for n in 0..15 {
            let at = 2 * HOUR + n * 4 * MIN;
            let pick = next(&library, &history, at, None, &mut rng).unwrap();
            assert_eq!(pick.explanation.clock_id, None);
            history.push(Play::of(&library.tracks[&pick.track], at));
            picked.push(pick.track);
        }
        assert!(
            picked.iter().all(|track| (1..=10).contains(track)),
            "{picked:?}"
        );
        // The fallback without a category is the station's library too.
        let mut history = History::default();
        for id in 1..=5 {
            history.push(Play::of(&library.tracks[&id], -(id as i64) * 10 * MIN));
        }
        let pick = next(&library, &history, 0, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 11);
        assert!((6..=10).contains(&pick.track), "{}", pick.track);
        // A station with no category yet rotates the whole library.
        library.categories.clear();
        let pick = next(&library, &History::default(), 2 * HOUR, None, &mut rng).unwrap();
        assert!(library.tracks.contains_key(&pick.track));
        assert!(library.in_station_library(&library.tracks[&21]));
    }

    #[test]
    fn slots_without_rules_skip_separation() {
        let mut free = slot(1, Method::Random);
        free.enforce_rules = false;
        let library = library(vec![free]);
        let mut rng = Rng::new(5);
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], -MIN));
        let picks: Vec<_> = (0..200)
            .map(|_| next(&library, &history, 0, None, &mut rng).unwrap())
            .collect();
        assert!(picks.iter().any(|p| p.track == 1));
        assert!(
            picks
                .iter()
                .all(|p| p.explanation.step == 0 && !p.explanation.enforce_rules)
        );
    }

    #[test]
    fn weighted_follows_the_weights() {
        let mut library = library(vec![slot(1, Method::Weighted)]);
        for member in &mut library.categories.get_mut(&1).unwrap().members {
            member.weight = if member.track == 1 { 9.0 } else { 0.25 };
        }
        let mut rng = Rng::new(6);
        let history = History::default();
        let heavy = (0..1_000)
            .filter(|_| next(&library, &history, 0, None, &mut rng).unwrap().track == 1)
            .count();
        // 9 / (9 + 4 × 0.25) = 90 %.
        assert!((850..950).contains(&heavy), "{heavy}");
        library.reweigh(1, 0.5);
        assert_eq!(library.categories[&1].members[0].weight, 4.5);
        library.reweigh(1, 0.01);
        assert_eq!(library.categories[&1].members[0].weight, crate::MIN_WEIGHT);
    }

    #[test]
    fn slots_with_nothing_allowed_are_skipped() {
        let mut library = library(vec![slot(1, Method::Random), slot(2, Method::Random)]);
        library.settings.fallback_category_id = Some(3);
        // Categories 2 and the fallback are banned all day.
        for category in [2, 3] {
            library.restrictions.push(crate::Restriction {
                id: category,
                target: crate::Target::Category(category),
                rule: crate::Rule::Daypart {
                    mode: crate::Mode::Ban,
                    days: (1..=7).collect(),
                    start_hour: 0,
                    end_hour: 24,
                },
            });
        }
        let mut rng = Rng::new(7);
        let history = History::default();
        let first = next(&library, &history, 0, None, &mut rng).unwrap();
        assert_eq!(first.position.unwrap().slot, 0);
        // Slot 1 has nothing it may play: slot 0 plays again.
        let second = next(&library, &history, MIN, first.position.as_ref(), &mut rng).unwrap();
        assert_eq!(second.position.unwrap().slot, 0);
        assert_eq!(
            second.explanation.skipped_slots,
            [SkippedSlot {
                slot: 1,
                category_id: 2,
                reason: SkipReason::NothingAllowed,
            }]
        );
        // With every category banned, nothing is picked.
        library.restrictions.push(crate::Restriction {
            id: 1,
            target: crate::Target::Category(1),
            rule: crate::Rule::NotAfter { category_id: 1 },
        });
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], -MIN));
        assert!(next(&library, &history, 0, None, &mut rng).is_none());
    }

    fn linked(category: CategoryId) -> Slot {
        Slot {
            linked_to_next: true,
            ..slot(category, Method::Random)
        }
    }

    fn skipped(slot: u32, category_id: CategoryId, reason: SkipReason) -> SkippedSlot {
        SkippedSlot {
            slot,
            category_id,
            reason,
        }
    }

    #[test]
    fn a_linked_slot_is_picked_with_the_next_slot() {
        let library = library(vec![
            linked(1),
            slot(2, Method::Random),
            slot(3, Method::Random),
        ]);
        let mut rng = Rng::new(3);
        let history = History::default();
        let jingle = next(&library, &history, 10 * MIN, None, &mut rng).unwrap();
        assert_eq!(jingle.position.unwrap().slot, 0);
        assert!(jingle.explanation.linked);
        let track = jingle.linked.as_deref().unwrap();
        assert_eq!(track.position.unwrap().slot, 1);
        assert_eq!(track.explanation.category_id, Some(2));
        // Planned for when the jingle ends (4 minutes).
        assert_eq!(track.explanation.at_unix_ms, 14 * MIN);
        assert!(!track.explanation.linked && track.linked.is_none());
        let picks = jingle.clone().into_picks();
        assert_eq!(picks.len(), 2);
        // The clock carries on after the linked track.
        let after = next(
            &library,
            &history,
            18 * MIN,
            picks[1].position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(after.position.unwrap().slot, 2);
        // Both counters move.
        let outcome = next_outcome(&library, &history, 10 * MIN, None, &mut Rng::new(3));
        assert_eq!(outcome.offered().count(), 2);
    }

    #[test]
    fn a_linked_slot_does_not_play_when_its_track_starts_after_the_hour() {
        let library = library(vec![
            linked(1),
            slot(2, Method::Random),
            slot(3, Method::Random),
        ]);
        let mut rng = Rng::new(3);
        let history = History::default();
        // At 55:00, the track starts at 59:00: both play.
        let pair = next(&library, &history, 55 * MIN, None, &mut rng).unwrap();
        assert!(pair.linked.is_some());
        // At 57:00, the track would start at 01:01:00: neither plays, the
        // slot after them does.
        let pick = next(&library, &history, 57 * MIN, None, &mut rng).unwrap();
        assert_eq!(pick.position.unwrap().slot, 2);
        assert!(pick.linked.is_none());
        assert_eq!(
            pick.explanation.skipped_slots,
            [
                skipped(0, 1, SkipReason::LinkedTrackAfterHour),
                skipped(1, 2, SkipReason::LinkedTrackAfterHour),
            ]
        );
        // 02:00 has no clock: at 01:57, the jingle is dropped too.
        let pick = next(&library, &history, HOUR + 57 * MIN, None, &mut rng).unwrap();
        assert_eq!(pick.position.unwrap().slot, 2);
    }

    #[test]
    fn the_last_slot_links_to_the_first_one_within_the_hour() {
        let library = library(vec![
            slot(1, Method::Random),
            slot(2, Method::Random),
            linked(3),
        ]);
        let mut rng = Rng::new(5);
        let history = History::default();
        let last = Position {
            hour_start_unix_ms: 0,
            clock_id: 1,
            slot: 1,
        };
        let pick = next(&library, &history, 30 * MIN, Some(&last), &mut rng).unwrap();
        assert_eq!(pick.position.unwrap().slot, 2);
        let first = pick.linked.as_deref().unwrap();
        assert_eq!(first.position.unwrap().slot, 0);
        assert_eq!(first.position.unwrap().hour_start_unix_ms, 0);
        // At the end of the hour the new hour's clock does not count.
        let pick = next(&library, &history, 57 * MIN, Some(&last), &mut rng).unwrap();
        assert!(pick.linked.is_none());
        assert_eq!(pick.position.unwrap().slot, 1);
        assert_eq!(
            pick.explanation.skipped_slots,
            [
                skipped(2, 3, SkipReason::LinkedTrackAfterHour),
                skipped(0, 1, SkipReason::LinkedTrackAfterHour),
            ]
        );
    }

    #[test]
    fn a_looping_clock_keeps_a_linked_pair_across_hours() {
        let mut library = library(vec![
            linked(1),
            slot(2, Method::Random),
            slot(3, Method::Random),
        ]);
        library.clocks.get_mut(&1).unwrap().looping = true;
        let mut rng = Rng::new(5);
        let pick = next(&library, &History::default(), 57 * MIN, None, &mut rng).unwrap();
        let track = pick.linked.as_deref().unwrap();
        assert_eq!(track.position.unwrap().hour_start_unix_ms, HOUR);
    }

    #[test]
    fn a_linked_slot_is_skipped_when_its_track_finds_nothing() {
        let mut library = library(vec![
            linked(1),
            slot(9, Method::Random),
            slot(3, Method::Random),
        ]);
        // Category 9 and the fallback category are empty.
        library.categories.insert(9, category(9, &[]));
        library.categories.insert(8, category(8, &[]));
        library.settings.fallback_category_id = Some(8);
        let mut rng = Rng::new(5);
        let pick = next(&library, &History::default(), 10 * MIN, None, &mut rng).unwrap();
        assert_eq!(pick.position.unwrap().slot, 2);
        assert_eq!(
            pick.explanation.skipped_slots,
            [
                skipped(0, 1, SkipReason::LinkedTrackNotFound),
                skipped(1, 9, SkipReason::NothingAllowed),
            ]
        );
        // A next slot passed over as the same type twice in a row is
        // nothing found too.
        let mut library = self::library(vec![
            linked(1),
            slot(2, Method::Random),
            slot(3, Method::Random),
        ]);
        library.no_two_in_a_row = HashMap::from([(1, 7), (2, 7)]);
        let pick = next(&library, &History::default(), 10 * MIN, None, &mut rng).unwrap();
        assert_eq!(pick.position.unwrap().slot, 2);
        assert_eq!(
            pick.explanation.skipped_slots,
            [
                skipped(0, 1, SkipReason::LinkedTrackNotFound),
                skipped(1, 2, SkipReason::SameTypeInARow),
            ]
        );
    }

    #[test]
    fn linked_slots_do_not_chain() {
        let library = library(vec![linked(1), linked(2), slot(3, Method::Random)]);
        let clock = &library.clocks[&1];
        // Slot 0 is followed by a linked slot: not linked; slot 1 is.
        assert!(!clock.linked(0) && clock.linked(1) && !clock.linked(2));
        let pick = next(
            &library,
            &History::default(),
            10 * MIN,
            None,
            &mut Rng::new(1),
        )
        .unwrap();
        assert_eq!(pick.position.unwrap().slot, 0);
        assert!(pick.linked.is_none());
        // A clock of one slot cannot link it to itself.
        let library = self::library(vec![linked(1)]);
        assert!(!library.clocks[&1].linked(0));
    }

    #[test]
    fn slots_of_a_module_turned_off_are_passed_over() {
        let mut library = library(vec![slot(1, Method::Random), slot(2, Method::Random)]);
        library.modules_off.insert(2);
        let mut rng = Rng::new(7);
        let history = History::default();
        let first = next(&library, &history, 0, None, &mut rng).unwrap();
        assert_eq!(first.position.unwrap().slot, 0);
        // Slot 1 calls category 2: passed over, slot 0 plays again.
        let second = next(&library, &history, MIN, first.position.as_ref(), &mut rng).unwrap();
        assert_eq!(second.position.unwrap().slot, 0);
        assert_eq!(second.explanation.step, 0);
        assert_eq!(
            second.explanation.skipped_slots,
            [SkippedSlot {
                slot: 1,
                category_id: 2,
                reason: SkipReason::ModuleOff,
            }]
        );
    }

    #[test]
    fn a_type_never_plays_twice_in_a_row() {
        // Categories 1 and 2 are jingles (type 7), category 3 songs.
        let mut library = library(vec![
            slot(1, Method::Random),
            slot(2, Method::Random),
            slot(3, Method::Random),
        ]);
        library.no_two_in_a_row = HashMap::from([(1, 7), (2, 7)]);
        let mut rng = Rng::new(3);
        let mut history = History::default();
        let first = next(&library, &history, 0, None, &mut rng).unwrap();
        assert_eq!(first.position.unwrap().slot, 0);
        history.push(Play::of(&library.tracks[&first.track], 0));

        // Slot 1, another jingle category, is passed over for the song slot.
        let second = next(
            &library,
            &history,
            4 * MIN,
            first.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(second.position.unwrap().slot, 2);
        assert_eq!(
            second.explanation.skipped_slots,
            [SkippedSlot {
                slot: 1,
                category_id: 2,
                reason: SkipReason::SameTypeInARow,
            }]
        );
        assert!(library.tracks[&second.track].categories.contains(&3));

        // After a song, a jingle may play.
        history.push(Play::of(&library.tracks[&second.track], 4 * MIN));
        let third = next(
            &library,
            &history,
            8 * MIN,
            second.position.as_ref(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(third.position.unwrap().slot, 0);

        // Without a clock (02:00), the station's library rotates: no jingle
        // right after a jingle there either.
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], 2 * HOUR));
        for minute in 0..20 {
            let pick = next(
                &library,
                &history,
                2 * HOUR + (minute + 1) * MIN,
                None,
                &mut rng,
            )
            .unwrap();
            assert!(
                !library.tracks[&pick.track]
                    .categories
                    .iter()
                    .any(|c| *c == 1 || *c == 2)
            );
            assert!(pick.explanation.restricted >= 10, "{:?}", pick.explanation);
        }
    }

    /// Only track separation (30 min) and category 1's daypart separation
    /// `dayparts`, with `count` dayparts.
    fn daypart_library(count: u8, dayparts: u8) -> Library {
        let mut library = library(vec![slot(1, Method::LeastRecentlyPlayed)]);
        library.settings.daypart_count = count;
        library.settings.separation = Windows {
            track_min: 30,
            ..Windows::OFF
        }
        .into();
        library.categories.get_mut(&1).unwrap().separation.dayparts = Some(dayparts);
        // The clock every hour of the week: hours without one have no
        // daypart separation.
        library.schedule.hours = (1..=7)
            .flat_map(|day| (0..24).map(move |hour| ((day, hour), 1)))
            .collect();
        library
    }

    /// Whether track 1, played at `played`, is held back by its daypart at
    /// `at` (Thursday 1970-01-01 is day 0, Friday day 1).
    fn held_by_daypart(library: &Library, played: i64, at: i64) -> bool {
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], played));
        let mut rng = Rng::new(21);
        let pick = next(library, &history, at, None, &mut rng).unwrap();
        // Least recently played: track 1 is the only one ever played, so
        // it is picked last unless held back; the others never are.
        assert_eq!(pick.explanation.step, 0);
        pick.explanation.blocked.daypart == 1
    }

    #[test]
    fn a_daypart_separation_of_one_keeps_out_the_next_daypart() {
        let library = daypart_library(4, 1);
        // Played in A (00:30).
        assert!(
            !held_by_daypart(&library, 30 * MIN, 5 * HOUR),
            "same daypart"
        );
        assert!(held_by_daypart(&library, 30 * MIN, 6 * HOUR + 30 * MIN));
        assert!(held_by_daypart(&library, 30 * MIN, 11 * HOUR + 59 * MIN));
        assert!(!held_by_daypart(&library, 30 * MIN, 12 * HOUR));
        // Played in D (22:00): the next day's A follows it.
        assert!(held_by_daypart(&library, 22 * HOUR, 24 * HOUR + 30 * MIN));
        assert!(!held_by_daypart(&library, 22 * HOUR, 30 * HOUR));
        // Off: 0, or no value of its own.
        let off = daypart_library(4, 0);
        assert!(!held_by_daypart(&off, 30 * MIN, 6 * HOUR + 30 * MIN));
        let mut none = daypart_library(4, 0);
        none.categories.get_mut(&1).unwrap().separation.dayparts = None;
        assert!(!held_by_daypart(&none, 30 * MIN, 6 * HOUR + 30 * MIN));
    }

    #[test]
    fn daypart_separation_is_the_categorys_only() {
        // The station has none: hours without a clock rotate the library
        // without it, and a category without a value of its own has none.
        let library = daypart_library(4, 3);
        assert_eq!(library.windows(None, None).dayparts, 0);
        assert_eq!(
            library
                .windows(Some(&SeparationOverrides::default()), None)
                .dayparts,
            0
        );
        assert_eq!(
            library
                .windows(Some(&library.categories[&1].separation), None)
                .dayparts,
            3
        );
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], 30 * MIN));
        let mut unscheduled = library.clone();
        unscheduled.schedule.hours.clear();
        let pick = next(
            &unscheduled,
            &history,
            6 * HOUR + 30 * MIN,
            None,
            &mut Rng::new(21),
        )
        .unwrap();
        assert_eq!(pick.explanation.clock_id, None);
        assert_eq!(pick.explanation.blocked.daypart, 0);
    }

    #[test]
    fn a_daypart_separation_of_six_runs_into_the_next_day() {
        let library = daypart_library(4, 6);
        // Played in A (01:00): kept out of B, C, D, then Friday's A, B, C.
        assert!(held_by_daypart(&library, HOUR, 18 * HOUR));
        assert!(held_by_daypart(&library, HOUR, 24 * HOUR + HOUR));
        assert!(held_by_daypart(&library, HOUR, 24 * HOUR + 17 * HOUR));
        assert!(!held_by_daypart(&library, HOUR, 24 * HOUR + 18 * HOUR));
        // The history reaches back far enough: 7 dayparts and an hour.
        assert!(library.horizon_ms() >= 43 * HOUR);
    }

    #[test]
    fn six_dayparts_are_four_hours_long() {
        // Played at 03:00: with 6 dayparts, 04:30 is the next one (B).
        assert!(held_by_daypart(
            &daypart_library(6, 1),
            3 * HOUR,
            4 * HOUR + 30 * MIN
        ));
        assert!(!held_by_daypart(
            &daypart_library(4, 1),
            3 * HOUR,
            4 * HOUR + 30 * MIN
        ));
        // Up to 10 with 6 dayparts; a category's 10 is cut to 6 with 4.
        let library = daypart_library(6, 10);
        assert!(held_by_daypart(&library, 21 * HOUR, 48 * HOUR + 15 * HOUR));
        assert!(!held_by_daypart(&library, 21 * HOUR, 48 * HOUR + 16 * HOUR));
        let mut library = daypart_library(4, 0);
        library.categories.get_mut(&1).unwrap().separation.dayparts = Some(10);
        assert_eq!(
            library
                .windows(Some(&library.categories[&1].separation), None)
                .dayparts,
            6
        );
        // A slot never overrides it.
        let slot = SeparationOverrides {
            dayparts: Some(3),
            ..SeparationOverrides::default()
        };
        assert_eq!(library.windows(None, Some(&slot)).dayparts, 0);
    }

    #[test]
    fn daypart_separation_is_relaxed_after_album_and_before_artist() {
        // Every track of the category played in A, artists 2 hours ago.
        let mut library = library(vec![slot(1, Method::Random)]);
        library.settings.separation = StationWindows {
            album_min: 600,
            artist_min: 120,
            title_min: 0,
            track_min: 30,
        };
        library.categories.get_mut(&1).unwrap().separation.dayparts = Some(2);
        let mut history = History::default();
        for id in 1..=5 {
            history.push(Play::of(&library.tracks[&id], (id as i64) * MIN));
        }
        let mut rng = Rng::new(22);
        // At 12:30 (C, two dayparts after A): album separation holds until
        // it is halved (step 1), the dayparts until they are halved (step
        // 3): C is no longer kept out.
        let pick = next(&library, &history, 12 * HOUR + 30 * MIN, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 3);
        assert_eq!(pick.explanation.windows.dayparts, 1);
        assert_eq!(pick.explanation.windows.artist_min, 120);
        assert_eq!(pick.explanation.blocked.daypart, 5);
        assert_eq!(
            pick.explanation.daypart,
            Some(DaypartLog {
                count: 4,
                daypart: 2,
                separation: 2,
                days: 0,
                last_daypart: Some(0),
            })
        );
        // At 06:30 (B): halved, 1 still holds; dropped at step 4, with
        // artist separation as configured.
        let pick = next(&library, &history, 6 * HOUR + 30 * MIN, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 4);
        assert_eq!(
            pick.explanation.step_description,
            "daypart separation dropped"
        );
        assert_eq!(pick.explanation.windows.dayparts, 0);
        assert_eq!(pick.explanation.windows.artist_min, 120);
        assert_eq!(pick.explanation.daypart.unwrap().daypart, 1);
        // At 01:00, artist separation holds too: relaxed after dayparts.
        let pick = next(&library, &history, HOUR, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 6);
        assert_eq!(pick.explanation.blocked.artist, 5);
    }

    /// What held track 1 back at `at`, played at `played`, with category
    /// 1's daypart separation `dayparts` and the same daypart kept out
    /// `days` following days, 4 dayparts: (by daypart separation, by the
    /// same daypart).
    fn held_by_days(dayparts: u8, days: u8, played: &[i64], at: i64) -> (u32, u32) {
        let mut library = daypart_library(4, dayparts);
        library
            .categories
            .get_mut(&1)
            .unwrap()
            .separation
            .daypart_days = Some(days);
        let mut history = History::default();
        for start in played {
            history.push(Play::of(&library.tracks[&1], *start));
        }
        let mut rng = Rng::new(23);
        let pick = next(&library, &history, at, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 0);
        assert_eq!(pick.explanation.daypart.unwrap().days, days);
        let blocked = pick.explanation.blocked;
        (blocked.daypart, blocked.daypart_days)
    }

    const DAY: i64 = 24 * HOUR;

    #[test]
    fn the_same_daypart_is_kept_out_on_the_following_days() {
        // Played in B on Thursday at 08:00, one day: Friday's B is kept
        // out, Saturday's is not; nor are Friday's A and C, nor the rest
        // of Thursday's B (the track separation decides).
        let b = [8 * HOUR];
        assert_eq!(held_by_days(0, 1, &b, DAY + 7 * HOUR), (0, 1));
        assert_eq!(held_by_days(0, 1, &b, DAY + 11 * HOUR + 59 * MIN), (0, 1));
        assert_eq!(held_by_days(0, 1, &b, 2 * DAY + 7 * HOUR), (0, 0));
        assert_eq!(held_by_days(0, 1, &b, DAY + 5 * HOUR), (0, 0));
        assert_eq!(held_by_days(0, 1, &b, DAY + 12 * HOUR), (0, 0));
        assert_eq!(held_by_days(0, 1, &b, 10 * HOUR), (0, 0));
        assert_eq!(held_by_days(0, 0, &b, DAY + 7 * HOUR), (0, 0), "off");
        // Seven days: the next seven days' B, not the eighth's.
        assert_eq!(held_by_days(0, 7, &b, 7 * DAY + 9 * HOUR), (0, 1));
        assert_eq!(held_by_days(0, 7, &b, 8 * DAY + 9 * HOUR), (0, 0));
        // Across midnight: played in D at 23:50, the next day's A is free,
        // its D is not.
        let d = [23 * HOUR + 50 * MIN];
        assert_eq!(held_by_days(0, 1, &d, DAY + 10 * MIN), (0, 0));
        assert_eq!(held_by_days(0, 1, &d, DAY + 19 * HOUR), (0, 1));
        // Every play in the daypart counts, not only the last one: played
        // in B, then in D the same evening.
        let both = [8 * HOUR, 20 * HOUR];
        assert_eq!(held_by_days(0, 1, &both, DAY + 8 * HOUR), (0, 1));
        assert_eq!(held_by_days(0, 0, &both, DAY + 8 * HOUR), (0, 0));
    }

    #[test]
    fn the_same_daypart_works_with_or_without_daypart_separation() {
        // Played in A at 01:00. Daypart separation 1 keeps B out, and
        // counts for it there; the next day's A is for the same daypart.
        let a = [HOUR];
        assert_eq!(held_by_days(1, 1, &a, 7 * HOUR), (1, 0));
        assert_eq!(held_by_days(1, 1, &a, 13 * HOUR), (0, 0));
        assert_eq!(held_by_days(1, 1, &a, DAY + HOUR), (0, 1));
        assert_eq!(held_by_days(1, 0, &a, DAY + HOUR), (0, 0));
        // Daypart separation 6 covers the next day's A already: nothing
        // changes there.
        assert_eq!(held_by_days(6, 1, &a, DAY + HOUR), (1, 0));
        assert_eq!(held_by_days(6, 2, &a, 2 * DAY + HOUR), (0, 1));
    }

    #[test]
    fn the_same_daypart_is_the_categorys_only() {
        let mut library = daypart_library(4, 0);
        library
            .categories
            .get_mut(&1)
            .unwrap()
            .separation
            .daypart_days = Some(7);
        assert_eq!(library.windows(None, None).daypart_days, 0);
        let category = &library.categories[&1].separation;
        assert_eq!(library.windows(Some(category), None).daypart_days, 7);
        let slot = SeparationOverrides {
            daypart_days: Some(3),
            ..SeparationOverrides::default()
        };
        assert_eq!(library.windows(None, Some(&slot)).daypart_days, 0);
        assert_eq!(library.windows(Some(category), Some(&slot)).daypart_days, 7);
        // The history reaches back to the start of the daypart seven days
        // before, and an hour more.
        assert!(library.horizon_ms() >= 7 * DAY + 7 * HOUR);
    }

    #[test]
    fn the_same_daypart_is_relaxed_with_daypart_separation() {
        // Every track of the category played in B on Thursday.
        let mut library = daypart_library(4, 0);
        let mut history = History::default();
        for id in 1..=5 {
            history.push(Play::of(&library.tracks[&id], 8 * HOUR + (id as i64) * MIN));
        }
        let mut rng = Rng::new(24);
        // Two days: Friday's B is kept out at step 3 (halved to 1), the
        // pick comes at step 4 (dropped).
        library
            .categories
            .get_mut(&1)
            .unwrap()
            .separation
            .daypart_days = Some(2);
        let pick = next(&library, &history, DAY + 9 * HOUR, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 4);
        assert_eq!(pick.explanation.windows.daypart_days, 0);
        assert_eq!(pick.explanation.blocked.daypart_days, 5);
        assert_eq!(pick.explanation.blocked.daypart, 0);
        // Saturday's B is two days later: halved to 1, it is free.
        let pick = next(&library, &history, 2 * DAY + 9 * HOUR, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 3);
        assert_eq!(pick.explanation.windows.daypart_days, 1);
        let log = pick.explanation.daypart.unwrap();
        assert_eq!((log.daypart, log.days, log.last_daypart), (1, 2, Some(1)));
    }

    /// Track 1 at 0.2, offered first at every pick (its weight dwarfs the
    /// others'), in a slot without separation.
    fn ratio_library() -> Library {
        let mut free = slot(1, Method::Weighted);
        free.enforce_rules = false;
        let mut library = library(vec![free]);
        library.tracks.get_mut(&1).unwrap().ratio = 2;
        for member in &mut library.categories.get_mut(&1).unwrap().members {
            member.weight = if member.track == 1 { 1e9 } else { 1.0 };
        }
        library
    }

    #[test]
    fn a_ratio_of_two_plays_twice_in_ten_offers() {
        let mut library = ratio_library();
        let mut rng = Rng::new(11);
        let history = History::default();
        let mut played = Vec::new();
        for n in 0..20 {
            let outcome = next_outcome(&library, &history, n * MIN, None, &mut rng);
            let pick = outcome.pick.clone().unwrap();
            if pick.track == 1 {
                assert!(outcome.held_by_ratio.is_empty());
                assert!(pick.explanation.held_by_ratio.is_empty());
                played.push(pick.explanation.counter.unwrap());
            } else {
                // Held back: another track of the slot plays instead.
                let counter = library.counter(1);
                let held = [HeldByRatio {
                    track_id: 1,
                    ratio: 2,
                    counter,
                }];
                assert_eq!(outcome.held_by_ratio, held);
                assert_eq!(pick.explanation.held_by_ratio, held);
                assert!((2..=5).contains(&pick.track));
                assert_eq!(pick.explanation.step, 0);
            }
            library.count_down(outcome.offered());
        }
        // Exactly 4 plays in 20 offers, at 10 and 9 of each turn.
        assert_eq!(played, [10, 9, 10, 9]);
        assert_eq!(library.counter(1), 10);
    }

    #[test]
    fn each_station_plays_a_track_at_its_own_ratio() {
        // The same tracks on three stations: track 1 at its default (1) on
        // the first, at 0.2 on the second, disabled on the third. Its
        // weight dwarfs the others': it is offered first at every pick.
        let station = |station_ratios: HashMap<TrackId, u8>| {
            let mut free = slot(1, Method::Weighted);
            free.enforce_rules = false;
            let mut category = category(1, &[1, 2, 3, 4, 5]);
            for member in &mut category.members {
                member.weight = if member.track == 1 { 1e9 } else { 1.0 };
            }
            Library::new(
                LibraryParts {
                    tracks: (1..=5).map(track).collect(),
                    categories: vec![category],
                    clocks: vec![Clock {
                        id: 1,
                        name: "Clock".into(),
                        looping: false,
                        slots: vec![free],
                    }],
                    schedule: Schedule {
                        hours: HashMap::from_iter((0..24).map(|h| ((4, h), 1))),
                        ..Schedule::default()
                    },
                    station_ratios,
                    ..LibraryParts::default()
                },
                TimeZone::UTC,
            )
        };
        let plays = |mut library: Library| {
            let mut rng = Rng::new(21);
            let history = History::default();
            let mut ratios = Vec::new();
            for n in 0..20 {
                let outcome = next_outcome(&library, &history, n * MIN, None, &mut rng);
                let pick = outcome.pick.clone().unwrap();
                if pick.track == 1 {
                    ratios.push(pick.explanation.ratio.unwrap());
                }
                library.count_down(outcome.offered());
            }
            ratios
        };
        assert_eq!(plays(station(HashMap::new())), [10; 20]);
        // The station's ratio, logged with each pick.
        assert_eq!(plays(station(HashMap::from([(1, 2)]))), [2; 4]);
        let disabled = station(HashMap::from([(1, 0)]));
        assert!(!disabled.tracks.contains_key(&1));
        assert!(
            disabled.categories[&1]
                .members
                .iter()
                .all(|member| member.track != 1)
        );
        assert!(plays(disabled).is_empty());
    }

    #[test]
    fn a_ratio_is_never_relaxed() {
        // Track 1 is the only track: no other candidate, no fallback.
        let mut library = Library::new(
            LibraryParts {
                tracks: vec![Track {
                    ratio: 1,
                    ..track(1)
                }],
                categories: vec![category(1, &[1])],
                clocks: vec![Clock {
                    id: 1,
                    name: "Clock".into(),
                    looping: false,
                    slots: vec![slot(1, Method::Random)],
                }],
                schedule: Schedule {
                    hours: HashMap::from_iter((0..24).map(|h| ((4, h), 1))),
                    ..Schedule::default()
                },
                counters: HashMap::from([(1, 5)]),
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        );
        let mut rng = Rng::new(12);
        let history = History::default();
        for counter in (1..=5).rev() {
            let outcome = next_outcome(&library, &history, 0, None, &mut rng);
            assert_eq!(outcome.pick, None, "at {counter}");
            // Offered once, whatever the steps and the fallback.
            assert_eq!(
                outcome.held_by_ratio,
                [HeldByRatio {
                    track_id: 1,
                    ratio: 1,
                    counter,
                }]
            );
            assert_eq!(
                library.count_down(outcome.offered()),
                [(1, ratio::after(counter))]
            );
        }
        let outcome = next_outcome(&library, &history, 0, None, &mut rng);
        assert_eq!(outcome.pick.unwrap().explanation.counter, Some(10));
        // `next` alone never moves the counters.
        assert_eq!(library.counter(1), 10);
        assert!(next(&library, &history, 0, None, &mut rng).is_some());
        assert_eq!(library.counter(1), 10);
    }

    #[test]
    fn plays_outside_rotation_do_not_move_the_counter() {
        let library = ratio_library();
        let mut rng = Rng::new(13);
        let mut history = History::default();
        // Track 1 queued by hand, or requested, twice: in the history only.
        history.push(Play::of(&library.tracks[&1], -2 * MIN));
        history.push(Play::of(&library.tracks[&1], -MIN));
        assert_eq!(library.counter(1), 10);
        // Rotation still offers it at 10: it plays.
        let pick = next(&library, &history, 0, None, &mut rng).unwrap();
        assert_eq!(pick.track, 1);
        assert_eq!(pick.explanation.counter, Some(10));
    }
}
