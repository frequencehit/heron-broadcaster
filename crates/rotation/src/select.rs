// SPDX-License-Identifier: AGPL-3.0-or-later

//! Picking the next track: which slot, which rules, which relaxation.

use std::collections::HashMap;

use jiff::civil::DateTime;
use serde::{Deserialize, Serialize};

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
}

/// Why a track was picked: the selection log.
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
    /// Relaxation step, 0 (none) to 10.
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
}

/// When each track, title, album and artist last started, up to `at`.
struct Recent<'h> {
    tracks: HashMap<TrackId, i64>,
    titles: HashMap<&'h str, i64>,
    albums: HashMap<&'h str, i64>,
    artists: HashMap<ArtistId, i64>,
}

impl<'h> Recent<'h> {
    fn new(history: &'h History, at_ms: i64) -> Self {
        let mut recent = Self {
            tracks: HashMap::new(),
            titles: HashMap::new(),
            albums: HashMap::new(),
            artists: HashMap::new(),
        };
        // In order: later plays overwrite earlier ones.
        for play in history.plays().iter().take_while(|p| p.start_ms <= at_ms) {
            if let Some(track) = play.track {
                recent.tracks.insert(track, play.start_ms);
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
        let base = library.settings.separation;
        match category {
            Some(id) => {
                let category = library.categories.get(&id);
                let windows = base
                    .with(&category.map(|c| c.separation).unwrap_or_default())
                    .with(&extra.copied().unwrap_or_default());
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
                    windows: base,
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
fn fill(
    library: &Library,
    history: &History,
    at_ms: i64,
    slot: &SlotCall<'_>,
    rng: &mut Rng,
) -> Option<(TrackId, Explanation)> {
    let local = library.local(at_ms).datetime();
    let previous = history
        .previous(at_ms)
        .map(|play| play.categories.as_slice());
    let recent = Recent::new(history, at_ms);
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
        let candidates: Vec<_> = pool
            .iter()
            .filter(|(track, _)| {
                recent.blocking(library, track, windows, at_ms).is_none()
                    && held_elsewhere(library, history, track, across, at_ms).is_none()
            })
            .copied()
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let method = if step == LAST_STEP {
            Method::LeastRecentlyPlayed
        } else {
            slot.method
        };
        let (track, weight) = choose(method, &candidates, history, rng);
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
            candidates: u32::try_from(candidates.len()).unwrap_or(u32::MAX),
            last_played_unix_ms: history.last_played(track.id),
            weight,
            skipped_slots: Vec::new(),
            held_elsewhere: held.map(|(_, held)| held),
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
/// - `None` only when restrictions forbid every track a slot, and the
///   fallback, could offer: the engine's fallback directory takes over.
pub fn next(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    rng: &mut Rng,
) -> Option<Pick> {
    let local = library.local(at_ms);
    let into_hour = (i64::from(local.minute()) * 60 + i64::from(local.second())) * 1_000
        + i64::from(local.millisecond());
    let hour_start = at_ms - into_hour;
    let schedule = library.schedule_at(&local);
    let named = |mut explanation: Explanation| {
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
        return fill(library, history, at_ms, &slot, rng).map(|(track, explanation)| Pick {
            track,
            position: None,
            explanation: named(explanation),
        });
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
    let previous = history
        .previous(at_ms)
        .map(|play| play.categories.as_slice());
    for index in (0..count).map(|k| (first + k) % count) {
        let slot = &clock.slots[index];
        // A jingle slot after a jingle: passed over, never relaxed.
        if library
            .repeated_type(previous, std::slice::from_ref(&slot.category))
            .is_some()
        {
            skipped.push(SkippedSlot {
                slot: u32::try_from(index).unwrap_or(u32::MAX),
                category_id: slot.category,
                reason: SkipReason::SameTypeInARow,
            });
            continue;
        }
        let call = SlotCall {
            call: Call::new(library, Some(slot.category), Some(&slot.separation)),
            method: slot.method,
            enforce_rules: slot.enforce_rules,
        };
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        match fill(library, history, at_ms, &call, rng) {
            Some((track, mut explanation)) => {
                explanation.clock_id = Some(clock.id);
                explanation.clock_name = Some(clock.name.clone());
                explanation.slot = Some(index);
                explanation.skipped_slots = skipped;
                let explanation = named(explanation);
                return Some(Pick {
                    track,
                    position: Some(Position {
                        hour_start_unix_ms: hour_start,
                        clock_id: clock.id,
                        slot: index,
                    }),
                    explanation,
                });
            }
            None => skipped.push(SkippedSlot {
                slot: index,
                category_id: slot.category,
                reason: SkipReason::NothingAllowed,
            }),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use jiff::tz::TimeZone;

    use super::*;
    use crate::{Category, Clock, LibraryParts, Member, Play, Schedule, SeparationOverrides, Slot};

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
        // fallback (the station's library) gives a track at step 9.
        assert_eq!(pick.explanation.step, 9);
        assert_eq!(pick.explanation.category_id, None);
        assert!(pick.track > 5);
        assert_eq!(pick.explanation.windows.track_min, 60);
        // Played 70 to 110 minutes ago: track separation quartered (60) is enough.
        let mut history = History::default();
        for id in 1..=5 {
            history.push(Play::of(&library.tracks[&id], -(id as i64 + 6) * 10 * MIN));
        }
        let pick = next(&library, &history, 0, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 8);
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
        assert_eq!(pick.explanation.step, 9);
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
}
