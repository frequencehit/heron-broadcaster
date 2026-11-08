// SPDX-License-Identifier: AGPL-3.0-or-later

//! Picking the next track: which slot, which rules, which relaxation.

use std::collections::HashMap;

use jiff::civil::DateTime;
use serde::{Deserialize, Serialize};

use crate::dayparts::{self, DaypartFrom, DaypartLog};
use crate::draw::{self, DrawDepth, DrawLog};
use crate::exact::{
    self, Choice, ExactLog, ExactMode, ExactTime, FillKind, FillLog, Fit, FitMode, MedleyCut,
    Window,
};
use crate::lookahead::{self, Calculation, LookaheadLog, Plan, Planning};
use crate::ratio::{self, HeldByRatio};
use crate::restrict::{Subject, forbidding};
use crate::rules::{AcrossWindows, FALLBACK_STEP, LAST_STEP, Windows, describe_step};
use crate::{
    ArtistId, CategoryId, Clock, ClockId, Elsewhere, History, Library, Method, Position, Rng,
    ScheduleId, StationId, Track, TrackId,
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
    /// A medley shortened to fill the gap before an exact time: it plays
    /// as two spans of its file.
    pub medley_cut: Option<MedleyCut>,
    /// The exact slot's start time for the engine (Unix ms): what still
    /// plays then is faded out and this track starts.
    pub start_at_ms: Option<i64>,
}

impl Pick {
    /// How long it plays: its track's length, or its medley cut's.
    pub fn length_ms(&self, library: &Library) -> u32 {
        self.medley_cut.as_ref().map_or_else(
            || library.tracks.get(&self.track).map_or(0, |t| t.length_ms),
            |cut| cut.length_ms,
        )
    }

    /// How long from its start to the next track's: its track's mix point
    /// ([`Track::next_ms`]), or its medley cut's length.
    pub fn next_ms(&self, library: &Library) -> u32 {
        self.medley_cut.as_ref().map_or_else(
            || library.tracks.get(&self.track).map_or(0, Track::next_ms),
            |cut| cut.length_ms,
        )
    }

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
    /// The draw depth the pick was drawn at. Missing for a random slot, at
    /// step 12, and in picks logged before the draw depth.
    #[serde(default)]
    pub draw: Option<DrawLog>,
    /// The slot is linked to the next one: the next pick is that slot's
    /// track, picked with this one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linked: bool,
    /// The slot has an exact time: how it was met.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<ExactLog>,
    /// The pick fills the gap before an exact time: with a medley or a
    /// filler track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<FillLog>,
    /// Picked looking ahead before an exact time: the endings simulated
    /// and how the best one meets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookahead: Option<LookaheadLog>,
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
    /// The next exact time is due: the slots before it are passed over.
    ExactTimeDue,
    /// Its tracks would all end after the next exact time's window (soft).
    NoLongerFits,
    /// Its exact time has passed in this run of the clock.
    ExactTimePassed,
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
pub(crate) struct Call<'l> {
    category_id: Option<CategoryId>,
    category_name: Option<String>,
    /// Tracks and their weights, in a fixed order.
    pool: Vec<(&'l Track, f64)>,
    windows: Windows,
}

impl<'l> Call<'l> {
    /// `None`: the station's library.
    pub(crate) fn new(
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
    pub(crate) fn allowed(
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

/// The relaxation steps a slot goes through: none but the first when
/// filling the gap before an exact time.
fn steps(enforce_rules: bool, first_only: bool) -> Vec<u8> {
    if first_only {
        vec![0]
    } else if enforce_rules {
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
    /// The draw depth of the slot's category; `None` for a random slot.
    draw: Option<DrawDepth>,
    /// Filling a gap: the rules as configured only, never relaxed.
    first_only: bool,
    /// Whether what played before counts (restrictions after a category, a
    /// type never twice in a row): not for a jingle over a track's intro.
    after_previous: bool,
}

/// Counts a track held back by `rule` in `blocked`.
fn count(blocked: &mut Blocked, rule: SeparationRule) {
    match rule {
        SeparationRule::Track => blocked.track += 1,
        SeparationRule::Title => blocked.title += 1,
        SeparationRule::Artist => blocked.artist += 1,
        SeparationRule::Album => blocked.album += 1,
        SeparationRule::Daypart => blocked.daypart += 1,
        SeparationRule::DaypartDays => blocked.daypart_days += 1,
    }
}

/// Tries the relaxation steps in order and picks with the slot's method
/// among the first step's candidates, within the slot's draw depth: at step
/// 0 (and 11, in the fallback category) the draw widens from its start to
/// its maximum before the next step is tried.
///
/// The play ratio decides after the method: a track it holds back is added
/// to `held` (its counter moves, see [`Library::count_down`]) and the method
/// picks again among the others. Tracks in `held` are never offered again.
///
/// Before an exact time, `fit` keeps the candidates that fit best
/// ([`Fit::keep`]); when none fits at a step, the slot no longer fits
/// ([`Miss::TooLong`]) rather than being relaxed.
fn fill(
    library: &Library,
    history: &History,
    at_ms: i64,
    slot: &SlotCall<'_>,
    held_by_ratio: &mut Vec<HeldByRatio>,
    rng: &mut Rng,
    fit: Option<&Fit<'_>>,
) -> Result<(TrackId, Explanation, Option<MedleyCut>), Miss> {
    let local = library.local(at_ms).datetime();
    let previous = history
        .previous(at_ms)
        .filter(|_| slot.after_previous)
        .map(|play| play.categories.as_slice());
    // The days of the slot's category, or of a track's own.
    let days = slot
        .call
        .pool
        .iter()
        .map(|(track, _)| library.for_track(slot.call.windows, track).daypart_days)
        .fold(slot.call.windows.daypart_days, u8::max);
    let recent = Recent::new(library, history, at_ms, days);
    let (allowed, restricted) = slot.call.allowed(library, local, previous);
    let across = if slot.enforce_rules {
        library.settings.across.windows()
    } else {
        AcrossWindows::OFF
    };
    let mut blocked = Blocked::default();
    let mut held: Option<(Option<i64>, HeldElsewhere)> = None;
    // A fill's later picks are never logged: what only the log needs is
    // left out.
    let quick = fit.is_some_and(|fit| fit.quick);
    if slot.enforce_rules && !quick {
        for (track, _) in &allowed {
            let windows = library.for_track(slot.call.windows, track);
            match recent.blocking(library, track, windows, at_ms) {
                Some(rule) => count(&mut blocked, rule),
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
    // Ordered once per pick for the draw, played longest ago first, and
    // reused by every step.
    let ordered = slot.draw.map(|_| draw::order(&allowed, history, rng));
    type Fallback<'l> = (Call<'l>, Vec<(&'l Track, f64)>, Option<DrawDepth>);
    let mut fallback: Option<Fallback<'_>> = None;
    let gap_fill = fit.is_some_and(|fit| fit.mode == FitMode::Medley) || slot.first_only;
    for step in steps(slot.enforce_rules, gap_fill) {
        let across = if step < FALLBACK_STEP {
            across.at_step(step)
        } else {
            AcrossWindows::OFF
        };
        // The windows before relaxation: none in a slot without rules.
        let (call, pool, configured, depth) = if step < FALLBACK_STEP {
            let configured = slot.enforce_rules.then_some(slot.call.windows);
            let pool = ordered.as_ref().unwrap_or(&allowed);
            (&slot.call, pool, configured, slot.draw)
        } else {
            let (call, pool, depth) = fallback.get_or_insert_with(|| {
                let category = library.fallback_category();
                let call = Call::new(library, category.map(|c| c.id), None);
                let (pool, _) = call.allowed(library, local, previous);
                // The fallback category's own draw depth.
                let depth = slot
                    .draw
                    .map(|_| library.draw(category.map(|c| &c.separation), None));
                let pool = if depth.is_some() {
                    draw::order(&pool, history, rng)
                } else {
                    pool
                };
                (call, pool, depth)
            });
            (&*call, &*pool, Some(call.windows), *depth)
        };
        // Step 12 takes the least recently played track: no draw.
        let depth = depth.filter(|_| step < LAST_STEP);
        // Each track's, with its own daypart values, relaxed at this step.
        let windows_of = |track: &Track| {
            configured.map_or(Windows::OFF, |windows| {
                library.for_track(windows, track).at_step(step)
            })
        };
        let passes = |track: &Track| {
            recent
                .blocking(library, track, windows_of(track), at_ms)
                .is_none()
                && held_elsewhere(library, history, track, across, at_ms).is_none()
        };
        // The draw's sizes at this step, widest last: never beyond the
        // maximum.
        let sizes: Vec<(Option<u8>, usize)> = match depth {
            Some(depth) => depth
                .depths(step)
                .into_iter()
                .map(|pct| (Some(pct), draw::size(pool.len(), pct)))
                .collect(),
            None => vec![(None, pool.len())],
        };
        let reach = sizes.last().map_or(0, |(_, size)| *size);
        // The track a fill played again wants, when it passes the rules.
        let wanted = fit
            .and_then(|fit| fit.want)
            .and_then(|want| pool.iter().find(|(track, _)| track.id == want))
            .filter(|(track, _)| passes(track))
            .copied();
        let passing: Vec<(usize, &Track, f64)> = pool[..reach]
            .iter()
            .enumerate()
            .filter(|(_, (track, _))| passes(track))
            .map(|(index, (track, weight))| (index, *track, *weight))
            .collect();
        let method = if step == LAST_STEP {
            Method::LeastRecentlyPlayed
        } else {
            slot.method
        };
        // Least recently played draws at random within the draw.
        let chooser = match (depth, method) {
            (Some(_), Method::LeastRecentlyPlayed) => Method::Random,
            _ => method,
        };
        let mut picked = None;
        let mut tried = None;
        let mut too_long = false;
        // A track that would be cut (hard mode) only when no draw, however
        // wide, has one that is not.
        let passes: &[bool] = if fit.is_some_and(Fit::cuts) {
            &[false, true]
        } else {
            &[true]
        };
        for (&late_ok, (pct, size)) in passes
            .iter()
            .flat_map(|late_ok| sizes.iter().map(move |size| (late_ok, *size)))
        {
            if picked.is_some() {
                break;
            }
            // A wider depth with no more tracks has nothing new.
            if tried == Some((late_ok, size)) {
                continue;
            }
            tried = Some((late_ok, size));
            // A fill played again wants a track of the draw: ties with its
            // last track (never played ones) are in the draw too, whatever
            // order they were drawn in.
            let edge = size
                .checked_sub(1)
                .and_then(|last| pool.get(last))
                .map(|(track, _)| history.last_played(track.id));
            let held = |track: &Track| held_by_ratio.iter().any(|held| held.track_id == track.id);
            let mut candidates: Vec<_> = passing
                .iter()
                .filter(|(index, track, _)| *index < size && !held(track))
                .map(|(_, track, weight)| (*track, *weight))
                .collect();
            if let Some((track, weight)) = wanted
                && !candidates.iter().any(|(other, _)| other.id == track.id)
                && !held(track)
                && edge.is_some_and(|edge| history.last_played(track.id) <= edge)
            {
                candidates.push((track, weight));
            }
            let among = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
            // The ratio is never relaxed: tracks it holds back leave the
            // draw to the others, then to a wider draw and the next step.
            while !candidates.is_empty() {
                let fitted: Vec<exact::Fitted<'_>> = match fit {
                    Some(fit) => fit.keep(&candidates, at_ms, late_ok),
                    None => candidates.iter().map(|(t, w)| (*t, *w, None)).collect(),
                };
                if fitted.is_empty() {
                    too_long = true;
                    break;
                }
                let wanted = fit.and_then(|fit| {
                    fit.record(&candidates, at_ms, late_ok, |track| {
                        ratio::plays(track.ratio, library.counter(track.id))
                    });
                    fit.wanted(&candidates, at_ms, late_ok)
                });
                let choices: Vec<(&Track, f64)> = fitted.iter().map(|(t, w, _)| (*t, *w)).collect();
                let (track, weight) =
                    wanted.unwrap_or_else(|| choose(chooser, &choices, history, rng));
                let cut = fitted
                    .iter()
                    .find(|(other, ..)| other.id == track.id)
                    .and_then(|(_, _, cut)| cut.clone());
                let counter = library.counter(track.id);
                if ratio::plays(track.ratio, counter) {
                    picked = Some((track, weight, counter, among, pct, size, cut));
                    break;
                }
                held_by_ratio.push(HeldByRatio {
                    track_id: track.id,
                    ratio: track.ratio,
                    counter,
                });
                candidates.retain(|(other, _)| other.id != track.id);
            }
        }
        let Some((track, weight, counter, among, pct, size, cut)) = picked else {
            if too_long {
                // Tracks pass the rules here, but none fits the time left.
                return Err(Miss::TooLong);
            }
            continue;
        };
        let draw = depth.zip(pct).map(|(depth, pct)| {
            let mut blocked = Blocked::default();
            for (other, _) in pool[..size].iter().filter(|_| !quick) {
                if let Some(rule) = recent.blocking(library, other, windows_of(other), at_ms) {
                    count(&mut blocked, rule);
                } else if held_elsewhere(library, history, other, across, at_ms).is_some() {
                    blocked.across += 1;
                }
            }
            DrawLog {
                pct,
                start_pct: depth.start_pct,
                max_pct: depth.max_pct,
                tracks: u32::try_from(size).unwrap_or(u32::MAX),
                blocked,
            }
        });
        let windows = windows_of(track);
        let own = library.for_track(slot.call.windows, track);
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
            candidates: among,
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
                separation: own.dayparts,
                days: own.daypart_days,
                separation_from: DaypartFrom::of(track.dayparts),
                days_from: DaypartFrom::of(track.daypart_days),
                last_daypart: history
                    .last_played(track.id)
                    .map(|last| dayparts::of(recent.daypart_count, library.local(last).datetime())),
            }),
            draw,
            linked: false,
            exact: None,
            fill: None,
            lookahead: None,
        };
        return Ok((track.id, explanation, cut));
    }
    Err(Miss::Nothing)
}

/// Why [`fill`] picked nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Miss {
    /// Nothing passes the rules, even relaxed.
    Nothing,
    /// Tracks pass the rules, but none fits before the exact time.
    TooLong,
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
    /// Before an exact time, with [`Planning::record`]: the fill kept
    /// followed, or the fills built, each an attempt.
    pub calculation: Option<Calculation>,
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
    next_planned(library, history, at_ms, last, rng, &mut None)
}

/// [`next_outcome`], following the best fill kept in `plan` before an
/// exact time, and keeping the new one there: pass the same `plan` from a
/// pick to the next (`docs/rotation.md#exact-times`).
pub fn next_planned(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    rng: &mut Rng,
    plan: &mut Option<Plan>,
) -> Outcome {
    outcome(library, history, at_ms, last, rng, true, plan)
}

/// [`next_planned`] with its own [`Planning`]: the queue manager's budget,
/// and the attempts recorded in [`Outcome::calculation`].
pub fn next_planned_with(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    rng: &mut Rng,
    plan: &mut Option<Plan>,
    planning: Planning,
) -> Outcome {
    outcome_with(library, history, at_ms, last, rng, true, plan, planning)
}

/// The exact time ahead of a pick at `at_ms`, as rotation plans for it:
/// its window, for a match choosing its two tracks.
pub fn exact_ahead(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
) -> Option<Window> {
    next_exact(library, history, at_ms, last).map(|ahead| ahead.window)
}

/// [`next_planned`], building fills before an exact time or not (to
/// measure what they gain).
pub(crate) fn outcome(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    rng: &mut Rng,
    fills: bool,
    plan: &mut Option<Plan>,
) -> Outcome {
    outcome_with(
        library,
        history,
        at_ms,
        last,
        rng,
        fills,
        plan,
        Planning::default(),
    )
}

/// [`outcome`] with its own [`Planning`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn outcome_with(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    rng: &mut Rng,
    fills: bool,
    plan: &mut Option<Plan>,
    planning: Planning,
) -> Outcome {
    let mut held_by_ratio = Vec::new();
    let mut calculation = None;
    let ahead = next_exact(library, history, at_ms, last).filter(|_| fills);
    let pick = match ahead {
        Some(ahead) => lookahead::best(
            library,
            history,
            at_ms,
            last,
            &ahead,
            &mut held_by_ratio,
            rng,
            plan,
            planning,
            &mut calculation,
        ),
        None => {
            *plan = None;
            pick(
                library,
                history,
                at_ms,
                last,
                &mut held_by_ratio,
                rng,
                Steer::default(),
            )
        }
    }
    .map(|mut pick| {
        pick.explanation.held_by_ratio = held_by_ratio.clone();
        pick
    });
    Outcome {
        pick,
        held_by_ratio,
        calculation,
    }
}

/// A track of `category` to play at `at_ms`, picked as a slot calling that
/// category would (least recently played, rules enforced and relaxed step
/// by step), outside any clock: a show's pick (the match's two tracks,
/// `docs/listener-interaction.md#match`). Without `enforce_rules`, the
/// separation rules do not hold (an announcement jingle), as for a slot
/// that does not enforce them, nor does what played before (restrictions
/// after a category, a type never twice in a row): it plays over the next
/// track's intro. Tracks in `excluded` are never offered.
/// `None` when nothing of the category can be picked: the fallback
/// category does not stand in for it.
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
        draw: Some(library.draw(
            library.categories.get(&category).map(|c| &c.separation),
            None,
        )),
        first_only: false,
        after_previous: enforce_rules,
    };
    let mut held = Vec::new();
    let (track, mut explanation, _) =
        fill(library, history, at_ms, &slot, &mut held, rng, None).ok()?;
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
        medley_cut: None,
        start_at_ms: None,
    })
}

/// The next exact time ahead of a pick, for [`lookahead`].
pub(crate) struct Ahead {
    pub window: Window,
    pub mode: ExactMode,
    /// The exact slot's clock, and the slot from 0 in it.
    pub clock_id: ClockId,
    pub slot: u32,
}

/// The next exact time not due yet for a pick at `at_ms`, as [`pick`]
/// finds it.
fn next_exact(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
) -> Option<Ahead> {
    let hour_start = hour_start_of(library, at_ms);
    let clock = library.clock_at(&library.local(at_ms))?;
    let (played, first) = run(clock, hour_start, last);
    let previous_start = history.previous(at_ms).map(|play| play.start_ms);
    let upcoming = upcoming(
        library,
        clock,
        first,
        played,
        hour_start,
        at_ms,
        previous_start,
        &mut Vec::new(),
    )?;
    if upcoming.status != Status::Ahead {
        return None;
    }
    Some(Ahead {
        window: upcoming.window,
        mode: upcoming.exact.mode,
        clock_id: upcoming.clock.id,
        slot: u32::try_from(upcoming.index).unwrap_or(u32::MAX),
    })
}

/// The slot played last in this run of `clock`, if any, and the first
/// slot to try.
fn run(clock: &Clock, hour_start: i64, last: Option<&Position>) -> (Option<usize>, usize) {
    let count = clock.slots.len();
    let played = match last {
        Some(last)
            if last.clock_id == clock.id
                && (clock.looping || last.hour_start_unix_ms == hour_start) =>
        {
            Some(last.slot as usize % count)
        }
        _ => None,
    };
    (played, played.map_or(0, |slot| (slot + 1) % count))
}

/// Start of the station hour of `at_ms` (Unix ms).
fn hour_start_of(library: &Library, at_ms: i64) -> i64 {
    let local = library.local(at_ms);
    let into_hour = (i64::from(local.minute()) * 60 + i64::from(local.second())) * 1_000
        + i64::from(local.millisecond());
    at_ms - into_hour
}

/// Where the next exact time stands for a pick at `at_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// The pick ends before its window: tracks are fitted to it.
    Ahead,
    /// Its slot starts now: at `start`, the track before faded or not.
    Due { start: i64, faded: bool },
    /// Its time has passed in this run of the clock.
    Missed,
}

fn status(window: Window, at_ms: i64, previous_start: Option<i64>) -> Status {
    if at_ms < window.from_ms {
        Status::Ahead
    } else if at_ms <= window.cut_ms {
        Status::Due {
            start: at_ms,
            faded: false,
        }
    } else if previous_start.is_some_and(|start| start < window.cut_ms) {
        // The track before crosses the cut: it is faded out there.
        Status::Due {
            start: window.cut_ms,
            faded: true,
        }
    } else {
        Status::Missed
    }
}

/// The next exact time after the clock's position.
struct Upcoming<'c> {
    clock: &'c Clock,
    /// The slot starting at it (the first of a linked pair).
    index: usize,
    /// Its place in this run from the first slot to try; `None` for the
    /// next hour's first slot.
    offset: Option<usize>,
    hour_start: i64,
    exact: ExactTime,
    window: Window,
    status: Status,
}

const HOUR_MS: i64 = 3_600_000;

/// The next exact time from `first` in this run of `clock` (the slots whose
/// time has passed are added to `missed`), else the next hour's first slot.
/// The slots before `first` played in this run already: a clock that does
/// not loop does not go back to them, and the slot just played (`played`)
/// is never due again.
#[allow(clippy::too_many_arguments)]
fn upcoming<'c>(
    library: &'c Library,
    clock: &'c Clock,
    first: usize,
    played: Option<usize>,
    hour_start: i64,
    at_ms: i64,
    previous_start: Option<i64>,
    missed: &mut Vec<usize>,
) -> Option<Upcoming<'c>> {
    let count = clock.slots.len();
    let run = if clock.looping {
        count
    } else {
        count.saturating_sub(first)
    };
    for offset in 0..run {
        let index = (first + offset) % count;
        if clock.follows_linked(index) || played == Some(index) {
            continue;
        }
        let Some(exact) = clock.exact(index) else {
            continue;
        };
        let window = exact.window(hour_start + i64::from(exact.at_s) * 1_000);
        match status(window, at_ms, previous_start) {
            Status::Missed => missed.push(index),
            status => {
                return Some(Upcoming {
                    clock,
                    index,
                    offset: Some(offset),
                    hour_start,
                    exact,
                    window,
                    status,
                });
            }
        }
    }
    if clock.looping {
        return None;
    }
    let next_hour = hour_start + HOUR_MS;
    let next = library.clock_at(&library.local(next_hour))?;
    let exact = next.exact(0).filter(|_| !next.follows_linked(0))?;
    let window = exact.window(next_hour + i64::from(exact.at_s) * 1_000);
    match status(window, at_ms, previous_start) {
        Status::Missed => None,
        status => Some(Upcoming {
            clock: next,
            index: 0,
            offset: None,
            hour_start: next_hour,
            exact,
            window,
            status,
        }),
    }
}

/// Picks from the slots of a clock, and the gaps before its exact times.
struct Picker<'a, 'h> {
    library: &'a Library,
    history: &'h History,
    held_by_ratio: &'a mut Vec<HeldByRatio>,
    rng: &'a mut Rng,
    steer: Steer<'a>,
}

/// What a fill played again, or the fit search, asks of a pick before an
/// exact time (`docs/rotation.md#exact-times`).
#[derive(Clone, Copy, Default)]
pub(crate) struct Steer<'a> {
    /// The tracks the pick should play (a linked slot's two), when its
    /// rules allow them.
    pub want: &'a [TrackId],
    /// Where its slot records the tracks it may play.
    pub options: Option<&'a std::cell::RefCell<Option<Vec<Choice>>>>,
    /// A pick never logged (a fill's later ones): what only the log needs
    /// is left out.
    pub quick: bool,
}

impl Picker<'_, '_> {
    fn named(&self, mut explanation: Explanation, at_ms: i64) -> Explanation {
        let schedule = self.library.schedule_at(&self.library.local(at_ms));
        explanation.schedule_id = Some(schedule.id);
        explanation.schedule_name = Some(schedule.name.clone());
        explanation
    }

    /// The slot at `index` of `clock` at `at_ms`, after `history`: passed
    /// over (and why) or picked.
    fn slot(
        &mut self,
        clock: &Clock,
        index: usize,
        hour_start: i64,
        history: &History,
        at_ms: i64,
        fit: Option<&Fit<'_>>,
    ) -> Result<Pick, SkipReason> {
        let library = self.library;
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
        let category = library.categories.get(&slot.category);
        let call = SlotCall {
            call: Call::new(library, Some(slot.category), Some(&slot.separation)),
            method: slot.method,
            enforce_rules: slot.enforce_rules,
            draw: (slot.method != Method::Random)
                .then(|| library.draw(category.map(|c| &c.separation), Some(&slot.separation))),
            first_only: false,
            after_previous: true,
        };
        let position = u32::try_from(index).unwrap_or(u32::MAX);
        // A looping clock's position is in the hour the track plays in.
        let hour_start = if clock.looping {
            hour_start_of(library, at_ms)
        } else {
            hour_start
        };
        let (track, mut explanation, _) = fill(
            library,
            history,
            at_ms,
            &call,
            self.held_by_ratio,
            self.rng,
            fit,
        )
        .map_err(|miss| match miss {
            Miss::Nothing => SkipReason::NothingAllowed,
            Miss::TooLong => SkipReason::NoLongerFits,
        })?;
        explanation.clock_id = Some(clock.id);
        explanation.clock_name = Some(clock.name.clone());
        explanation.slot = Some(position);
        Ok(Pick {
            track,
            position: Some(Position {
                hour_start_unix_ms: hour_start,
                clock_id: clock.id,
                slot: position,
            }),
            explanation: self.named(explanation, at_ms),
            linked: None,
            medley_cut: None,
            start_at_ms: None,
        })
    }

    /// The slot at `index`, with the next slot's track when it is linked
    /// to it; or the slots passed over (and why), and how many slots that
    /// took.
    fn group(
        &mut self,
        clock: &Clock,
        index: usize,
        hour_start: i64,
        at_ms: i64,
        fit: Option<&Fit<'_>>,
    ) -> Result<Pick, (Vec<(usize, SkipReason)>, usize)> {
        let library = self.library;
        let history = self.history;
        let count = clock.slots.len();
        let mut pick = self
            .slot(clock, index, hour_start, history, at_ms, fit)
            .map_err(|reason| (vec![(index, reason)], 1))?;
        if !clock.linked(index) {
            return Ok(pick);
        }
        // The next slot's track, right after this one: in this run of the
        // clock, or neither plays.
        let next = (index + 1) % count;
        let next_at = at_ms + i64::from(pick.next_ms(library));
        let same_run = library
            .clock_at(&library.local(next_at))
            .is_some_and(|other| other.id == clock.id)
            && (clock.looping || hour_start_of(library, next_at) == hour_start);
        if !same_run {
            let reason = SkipReason::LinkedTrackAfterHour;
            return Err((vec![(index, reason), (next, reason)], 2));
        }
        let mut with_jingle = history.clone();
        if let Some(track) = library.tracks.get(&pick.track) {
            with_jingle.push(crate::Play::of(track, at_ms));
        }
        let linked_fit = fit.map(|fit| Fit {
            want: self.steer.want.get(1).copied(),
            options: None,
            ..*fit
        });
        match self.slot(
            clock,
            next,
            hour_start,
            &with_jingle,
            next_at,
            linked_fit.as_ref(),
        ) {
            Ok(linked) => {
                pick.explanation.linked = true;
                pick.linked = Some(Box::new(linked));
                Ok(pick)
            }
            Err(reason) => Err((
                vec![(index, SkipReason::LinkedTrackNotFound), (next, reason)],
                2,
            )),
        }
    }

    /// The exact slot `upcoming` starting at `start`.
    fn exact(
        &mut self,
        upcoming: &Upcoming<'_>,
        start: i64,
        faded: bool,
    ) -> Result<Pick, (Vec<(usize, SkipReason)>, usize)> {
        let mut pick = self.group(
            upcoming.clock,
            upcoming.index,
            upcoming.hour_start,
            start,
            None,
        )?;
        pick.explanation.exact = Some(ExactLog::new(
            &upcoming.exact,
            upcoming.window,
            start,
            faded,
        ));
        pick.start_at_ms = Some(upcoming.window.cut_ms);
        Ok(pick)
    }

    /// Fills the gap before `upcoming`'s window: a medley landing in it
    /// (soft, with the fill on), else a track of the filler category.
    fn gap(&mut self, upcoming: &Upcoming<'_>, at_ms: i64) -> Option<Pick> {
        let library = self.library;
        let soft = upcoming.exact.mode == ExactMode::Soft;
        let medley = exact::medley_category(library).filter(|_| soft && upcoming.exact.medley_fill);
        let tries = [
            medley.map(|category| (category, FitMode::Medley, FillKind::Medley)),
            library.settings.filler_category_id.map(|category| {
                (
                    category,
                    FitMode::Prefer { late_ok: !soft },
                    FillKind::Filler,
                )
            }),
        ];
        for (category_id, mode, kind) in tries.into_iter().flatten() {
            let Some(category) = library.categories.get(&category_id) else {
                continue;
            };
            let call = SlotCall {
                call: Call::new(library, Some(category_id), None),
                method: Method::LeastRecentlyPlayed,
                enforce_rules: true,
                draw: Some(library.draw(Some(&category.separation), None)),
                first_only: true,
                after_previous: true,
            };
            let fit = Fit {
                window: upcoming.window,
                mode,
                medleys: Some(&library.medleys),
                want: self.steer.want.first().copied(),
                options: None,
                quick: self.steer.quick,
            };
            let Ok((track, mut explanation, cut)) = fill(
                library,
                self.history,
                at_ms,
                &call,
                self.held_by_ratio,
                self.rng,
                Some(&fit),
            ) else {
                continue;
            };
            let length_ms = cut.as_ref().map_or_else(
                || library.tracks.get(&track).map_or(0, |t| t.length_ms),
                |cut| cut.length_ms,
            );
            explanation.clock_id = Some(upcoming.clock.id);
            explanation.clock_name = Some(upcoming.clock.name.clone());
            explanation.fill = Some(FillLog {
                kind,
                slot: u32::try_from(upcoming.index).unwrap_or(u32::MAX),
                exact_at_unix_ms: upcoming.window.at_ms,
                medley_id: (kind == FillKind::Medley)
                    .then(|| library.medleys.get(&track).map(|medley| medley.id))
                    .flatten(),
                hooks_left_out: cut.as_ref().map_or(0, |cut| cut.hooks_left_out),
                parts_left_out: cut
                    .as_ref()
                    .map(|cut| cut.parts_left_out.clone())
                    .unwrap_or_default(),
                length_ms,
            });
            return Some(Pick {
                track,
                position: None,
                explanation: self.named(explanation, at_ms),
                linked: None,
                medley_cut: cut,
                start_at_ms: None,
            });
        }
        None
    }
}

pub(crate) fn pick(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    held_by_ratio: &mut Vec<HeldByRatio>,
    rng: &mut Rng,
    steer: Steer<'_>,
) -> Option<Pick> {
    let local = library.local(at_ms);
    let hour_start = hour_start_of(library, at_ms);
    let mut picker = Picker {
        library,
        history,
        held_by_ratio,
        rng,
        steer,
    };
    let Some(clock) = library.clock_at(&local) else {
        let slot = SlotCall {
            call: Call::new(library, None, None),
            method: Method::LeastRecentlyPlayed,
            enforce_rules: true,
            draw: Some(library.draw(None, None)),
            first_only: false,
            after_previous: true,
        };
        return fill(
            library,
            history,
            at_ms,
            &slot,
            picker.held_by_ratio,
            picker.rng,
            None,
        )
        .ok()
        .map(|(track, explanation, _)| Pick {
            track,
            position: None,
            explanation: picker.named(explanation, at_ms),
            linked: None,
            medley_cut: None,
            start_at_ms: None,
        });
    };
    let count = clock.slots.len();
    let (played, first) = run(clock, hour_start, last);
    let mut skipped = Vec::new();
    let skip = |skipped: &mut Vec<SkippedSlot>, clock: &Clock, index: usize, reason| {
        skipped.push(SkippedSlot {
            slot: u32::try_from(index).unwrap_or(u32::MAX),
            category_id: clock.slots[index].category,
            reason,
        });
    };
    let done = |mut pick: Pick, skipped: Vec<SkippedSlot>| {
        pick.explanation.skipped_slots = skipped;
        Some(pick)
    };
    let previous_start = history.previous(at_ms).map(|play| play.start_ms);
    let mut missed = Vec::new();
    let mut upcoming = upcoming(
        library,
        clock,
        first,
        played,
        hour_start,
        at_ms,
        previous_start,
        &mut missed,
    );
    let mut k = 0;
    if let Some(due) = upcoming.as_ref()
        && let Status::Due { start, faded } = due.status
    {
        // The clock jumps to the exact slot: the slots before it pass.
        match picker.exact(due, start, faded) {
            Ok(pick) => {
                for offset in 0..due.offset.unwrap_or(count) {
                    let index = (first + offset) % count;
                    skip(&mut skipped, clock, index, SkipReason::ExactTimeDue);
                }
                return done(pick, skipped);
            }
            Err((skips, taken)) => {
                for (index, reason) in skips {
                    skip(&mut skipped, due.clock, index, reason);
                }
                // The slots after it, as if it were not there; this run's
                // slots when it was the next hour's.
                k = due.offset.map_or(0, |offset| offset + taken);
            }
        }
        upcoming = None;
    }
    let mut fitting = true;
    let before_run = skipped.len();
    loop {
        // A song that would be cut is never chosen, in hard mode either:
        // the slot gives way, and only a filler may be cut ([`Picker::gap`]).
        let fit = upcoming.as_ref().filter(|_| fitting).map(|upcoming| Fit {
            window: upcoming.window,
            mode: FitMode::Prefer { late_ok: false },
            medleys: None,
            want: picker.steer.want.first().copied(),
            options: picker.steer.options,
            quick: picker.steer.quick,
        });
        while k < count {
            let index = (first + k) % count;
            k += 1;
            // A clock starting over within the hour reaches its exact
            // slots again: their time has passed.
            let passed = || {
                !clock.looping
                    && first + k > count
                    && clock.exact(index).is_some_and(|exact| {
                        exact
                            .window(hour_start + i64::from(exact.at_s) * 1_000)
                            .cut_ms
                            < at_ms
                    })
            };
            if missed.contains(&index) || passed() {
                skip(&mut skipped, clock, index, SkipReason::ExactTimePassed);
                if clock.linked(index) {
                    skip(
                        &mut skipped,
                        clock,
                        (index + 1) % count,
                        SkipReason::ExactTimePassed,
                    );
                    k += 1;
                }
                continue;
            }
            if let Some(ahead) = upcoming.as_ref().filter(|_| fitting)
                && ahead.offset.is_some()
                && ahead.index == index
            {
                // The exact slot before its window: fill the gap, else
                // it starts early.
                if let Some(pick) = picker.gap(ahead, at_ms) {
                    return done(pick, skipped);
                }
                match picker.exact(ahead, at_ms, false) {
                    Ok(pick) => return done(pick, skipped),
                    Err((skips, taken)) => {
                        for (index, reason) in skips {
                            skip(&mut skipped, clock, index, reason);
                        }
                        k += taken - 1;
                        // The slots after it are not fitted any more.
                        fitting = false;
                        continue;
                    }
                }
            }
            let fit = fit.as_ref().filter(|_| fitting);
            match picker.group(clock, index, hour_start, at_ms, fit) {
                Ok(pick) => return done(pick, skipped),
                Err((skips, taken)) => {
                    for (index, reason) in skips {
                        skip(&mut skipped, clock, index, reason);
                    }
                    k += taken - 1;
                }
            }
        }
        // Nothing fitted in this run before the next hour's exact time:
        // fill the gap, else play on, cut at the end of its window.
        match upcoming.as_ref().filter(|_| fitting) {
            Some(ahead) if ahead.offset.is_none() => {
                if let Some(pick) = picker.gap(ahead, at_ms) {
                    return done(pick, skipped);
                }
                fitting = false;
                k = 0;
                skipped.truncate(before_run);
            }
            _ => return None,
        }
    }
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
            mix_ms: None,
            ratio: 10,
            dayparts: None,
            daypart_days: None,
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
            exact: None,
            separation: SeparationOverrides::default(),
        }
    }

    /// A draw of the whole category: the method alone decides.
    fn whole_draw() -> SeparationOverrides {
        SeparationOverrides {
            draw_start_pct: Some(100),
            draw_max_pct: Some(100),
            ..SeparationOverrides::default()
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
    fn an_announcement_follows_a_jingle() {
        // Categories 1 and 2 are jingles: never two in a row.
        let mut library = library(vec![slot(1, Method::Random)]);
        library.no_two_in_a_row = HashMap::from([(1, 7), (2, 7)]);
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], 9 * HOUR));
        let at = 9 * HOUR + MIN;
        let mut rng = Rng::new(3);
        // A slot's pick keeps the rule; an announcement, over the next
        // track's intro, does not.
        assert!(from_category(&library, &history, at, 2, &[], true, &mut rng).is_none());
        let pick = from_category(&library, &history, at, 2, &[], false, &mut rng).unwrap();
        assert_eq!(pick.explanation.category_id, Some(2));
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
    fn least_recently_played_draws_among_the_oldest_three_at_least() {
        let library = library(vec![slot(1, Method::LeastRecentlyPlayed)]);
        let mut rng = Rng::new(2);
        // 1 to 4 played (long ago: no separation), 5 never: 20 % of 5 is
        // 1, so the draw is the 3 oldest, never played included.
        let last_played = HashMap::from([
            (1, -900 * MIN),
            (2, -1000 * MIN),
            (3, -800 * MIN),
            (4, -700 * MIN),
        ]);
        let history = History::new(vec![], last_played);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..100 {
            let pick = next(&library, &history, 0, None, &mut rng).unwrap();
            seen.insert(pick.track);
            let draw = pick.explanation.draw.unwrap();
            assert_eq!(
                (draw.pct, draw.start_pct, draw.max_pct, draw.tracks),
                (20, 20, 50, 3)
            );
            assert_eq!(pick.explanation.candidates, 3);
            assert_eq!(pick.explanation.method, Method::LeastRecentlyPlayed);
        }
        // At random among them: the never played one is not ahead.
        assert_eq!(seen, [1, 2, 5].into());
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
        library.categories.get_mut(&1).unwrap().separation = whole_draw();
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
            exact: None,
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
                separation_from: DaypartFrom::Category,
                days_from: DaypartFrom::Category,
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

    #[test]
    fn a_track_s_own_daypart_separation_replaces_its_category_s() {
        // The category keeps out the 3 dayparts after a play; track 1's
        // own 0 turns it off for it. All of them played in A.
        let mut library = daypart_library(4, 3);
        library.tracks.get_mut(&1).unwrap().dayparts = Some(0);
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], 10 * MIN));
        for id in 2..=5 {
            history.push(Play::of(&library.tracks[&id], 30 * MIN));
        }
        let mut rng = Rng::new(25);
        let pick = next(&library, &history, 6 * HOUR + 30 * MIN, None, &mut rng).unwrap();
        assert_eq!((pick.track, pick.explanation.step), (1, 0));
        assert_eq!(pick.explanation.blocked.daypart, 4);
        assert_eq!(pick.explanation.windows.dayparts, 0);
        let log = pick.explanation.daypart.unwrap();
        assert_eq!(
            (log.separation, log.separation_from, log.days_from),
            (0, DaypartFrom::Track, DaypartFrom::Category)
        );
        // Its own 1, in a category without separation, keeps it out of B
        // only; the others of the category are free.
        let mut library = daypart_library(4, 0);
        library.tracks.get_mut(&1).unwrap().dayparts = Some(1);
        assert!(held_by_daypart(&library, 30 * MIN, 6 * HOUR + 30 * MIN));
        assert!(!held_by_daypart(&library, 30 * MIN, 12 * HOUR));
        // The same in every category the track is in.
        library.clocks.get_mut(&1).unwrap().slots = vec![slot(2, Method::LeastRecentlyPlayed)];
        library
            .categories
            .get_mut(&2)
            .unwrap()
            .members
            .push(Member {
                track: 1,
                weight: 1.0,
            });
        library.tracks.get_mut(&1).unwrap().categories.push(2);
        assert!(held_by_daypart(&library, 30 * MIN, 6 * HOUR + 30 * MIN));
    }

    #[test]
    fn a_track_s_own_values_are_cut_to_the_station_s_maximum() {
        let mut library = daypart_library(4, 0);
        let track = library.tracks.get_mut(&1).unwrap();
        track.dayparts = Some(10);
        track.daypart_days = Some(9);
        let track = library.tracks[&1].clone();
        let windows = library.for_track(Windows::OFF, &track);
        assert_eq!((windows.dayparts, windows.daypart_days), (6, 7));
        library.settings.daypart_count = 6;
        assert_eq!(library.for_track(Windows::OFF, &track).dayparts, 10);
        // Unset: the category's stays.
        let other = library.tracks[&2].clone();
        let category = Windows {
            dayparts: 2,
            daypart_days: 1,
            ..Windows::OFF
        };
        assert_eq!(library.for_track(category, &other), category);
        // The history reaches back as far as the track's own values do.
        assert!(library.horizon_ms() >= 7 * DAY + 4 * HOUR);
    }

    #[test]
    fn a_track_s_own_days_in_the_same_daypart_replace_its_category_s() {
        // The category keeps the same daypart out 2 days; track 1 only 1.
        let mut library = daypart_library(4, 0);
        for id in 1..=5 {
            library.tracks.get_mut(&id).unwrap().daypart_days = (id == 1).then_some(1);
        }
        library
            .categories
            .get_mut(&1)
            .unwrap()
            .separation
            .daypart_days = Some(2);
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], 8 * HOUR));
        for id in 2..=5 {
            history.push(Play::of(&library.tracks[&id], 8 * HOUR + 10 * MIN));
        }
        let mut rng = Rng::new(26);
        // Saturday's B: the others are kept out (2 days), track 1 is not.
        let pick = next(&library, &history, 2 * DAY + 9 * HOUR, None, &mut rng).unwrap();
        assert_eq!((pick.track, pick.explanation.step), (1, 0));
        assert_eq!(pick.explanation.blocked.daypart_days, 4);
        let log = pick.explanation.daypart.unwrap();
        assert_eq!((log.days, log.days_from), (1, DaypartFrom::Track));
    }

    #[test]
    fn a_track_s_own_daypart_separation_is_relaxed_like_the_category_s() {
        // Every track of the category has its own 2, played in A.
        let mut library = daypart_library(4, 0);
        let mut history = History::default();
        for id in 1..=5 {
            library.tracks.get_mut(&id).unwrap().dayparts = Some(2);
            history.push(Play::of(&library.tracks[&id], (id as i64) * MIN));
        }
        let mut rng = Rng::new(27);
        // C: halved to 1 at step 3, it is free.
        let pick = next(&library, &history, 12 * HOUR + 30 * MIN, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 3);
        assert_eq!(pick.explanation.windows.dayparts, 1);
        assert_eq!(pick.explanation.blocked.daypart, 5);
        let log = pick.explanation.daypart.unwrap();
        assert_eq!(
            (log.separation, log.separation_from),
            (2, DaypartFrom::Track)
        );
        // B: still kept out halved, dropped at step 4.
        let pick = next(&library, &history, 6 * HOUR + 30 * MIN, None, &mut rng).unwrap();
        assert_eq!(pick.explanation.step, 4);
        assert_eq!(pick.explanation.windows.dayparts, 0);
    }

    #[test]
    fn a_track_s_own_daypart_separation_holds_in_hours_without_a_clock() {
        // No clock: the station's library, without a category value.
        let mut library = daypart_library(4, 0);
        library.schedule.hours.clear();
        let mut history = History::default();
        for id in 1..=20 {
            if id < 20 {
                library.tracks.get_mut(&id).unwrap().dayparts = Some(1);
            }
            // 20 played first: in the draw.
            let at = if id == 20 { 0 } else { id as i64 };
            history.push(Play::of(&library.tracks[&id], at * MIN));
        }
        let mut rng = Rng::new(28);
        let pick = next(&library, &history, 6 * HOUR + 30 * MIN, None, &mut rng).unwrap();
        assert_eq!((pick.track, pick.explanation.step), (20, 0));
        assert_eq!(pick.explanation.blocked.daypart, 19);
    }

    #[test]
    fn a_slot_without_rules_ignores_a_track_s_own_daypart_separation() {
        let mut library = daypart_library(4, 0);
        library.clocks.get_mut(&1).unwrap().slots[0].enforce_rules = false;
        library.tracks.get_mut(&1).unwrap().dayparts = Some(3);
        let mut history = History::default();
        history.push(Play::of(&library.tracks[&1], 30 * MIN));
        for id in 2..=5 {
            history.push(Play::of(&library.tracks[&id], HOUR));
        }
        let mut rng = Rng::new(29);
        // Track 1, the oldest, is in the draw, and plays without rules.
        let picks: Vec<_> = (0..30)
            .map(|_| next(&library, &history, 6 * HOUR + 30 * MIN, None, &mut rng).unwrap())
            .collect();
        assert!(picks.iter().all(|pick| pick.explanation.step == 0));
        assert!(picks.iter().any(|pick| pick.track == 1));
    }

    /// Track 1 at 0.2, offered first at every pick (its weight dwarfs the
    /// others'), in a slot without separation.
    fn ratio_library() -> Library {
        let mut free = slot(1, Method::Weighted);
        free.enforce_rules = false;
        let mut library = library(vec![free]);
        library.categories.get_mut(&1).unwrap().separation = whole_draw();
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
            category.separation = whole_draw();
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

    /// 20 tracks in category 1, played long ago, 1 the oldest; the
    /// `blocked` oldest share an artist with track 21, played 10 minutes
    /// ago (outside the category): artist separation holds them back
    /// until it is dropped at step 6.
    fn draw_library(blocked: u64, method: Method) -> (Library, History) {
        let mut tracks: Vec<_> = (1..=21).map(track).collect();
        for track in tracks.iter_mut().take(blocked as usize) {
            track.artists = vec![999];
        }
        tracks[20].artists = vec![999];
        let library = Library::new(
            LibraryParts {
                tracks,
                categories: vec![category(1, &(1..=20).collect::<Vec<_>>())],
                clocks: vec![Clock {
                    id: 1,
                    name: "Clock".into(),
                    looping: false,
                    slots: vec![slot(1, method)],
                }],
                schedule: Schedule {
                    hours: HashMap::from_iter((0..24).map(|h| ((4, h), 1))),
                    ..Schedule::default()
                },
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        );
        let mut history = History::default();
        for id in 1..=20 {
            history.push(Play::of(
                &library.tracks[&id],
                -(1_000 + (21 - id as i64) * 10) * MIN,
            ));
        }
        history.push(Play::of(&library.tracks[&21], -10 * MIN));
        (library, history)
    }

    /// The picks of 40 calls at 0, and their draws.
    fn draws(library: &Library, history: &History, seed: u64) -> Vec<(TrackId, u8, DrawLog)> {
        let mut rng = Rng::new(seed);
        (0..40)
            .map(|_| {
                let pick = next(library, history, 0, None, &mut rng).unwrap();
                (
                    pick.track,
                    pick.explanation.step,
                    pick.explanation.draw.unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn the_draw_widens_by_ten_points_before_any_relaxation() {
        // The 4 oldest held back: 20 % (4 tracks) has nothing, 30 % has 2.
        let (library, history) = draw_library(4, Method::LeastRecentlyPlayed);
        for (track, step, draw) in draws(&library, &history, 30) {
            assert_eq!(step, 0);
            assert!([5, 6].contains(&track), "{track}");
            assert_eq!((draw.pct, draw.tracks, draw.blocked.artist), (30, 6, 4));
        }
        // The 8 oldest: 20, 30 and 40 % have nothing, 50 % has 2.
        let (library, history) = draw_library(8, Method::LeastRecentlyPlayed);
        for (track, step, draw) in draws(&library, &history, 31) {
            assert_eq!(step, 0);
            assert!([9, 10].contains(&track), "{track}");
            assert_eq!((draw.pct, draw.tracks, draw.blocked.artist), (50, 10, 8));
        }
    }

    #[test]
    fn rules_are_relaxed_with_the_draw_at_its_maximum() {
        // The 10 oldest, half of the category, held back: the draw never
        // goes beyond 50 %, the artist separation is dropped instead.
        let (library, history) = draw_library(10, Method::LeastRecentlyPlayed);
        let picks = draws(&library, &history, 32);
        for (track, step, draw) in &picks {
            assert_eq!(*step, 6);
            assert!((1..=10).contains(track), "{track}");
            assert_eq!((draw.pct, draw.tracks, draw.blocked.artist), (50, 10, 0));
        }
        assert!(
            picks
                .iter()
                .map(|(track, ..)| track)
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 3
        );
        // Weighted slots draw the same way.
        let (library, history) = draw_library(10, Method::Weighted);
        for (track, step, draw) in draws(&library, &history, 33) {
            assert_eq!((step, draw.pct), (6, 50));
            assert!((1..=10).contains(&track), "{track}");
        }
    }

    #[test]
    fn two_rounds_of_a_category_do_not_play_in_the_same_order() {
        let mut library = library(vec![slot(3, Method::LeastRecentlyPlayed)]);
        library.settings.separation = Windows::OFF.into();
        let mut rng = Rng::new(7);
        let mut history = History::default();
        let mut order = Vec::new();
        for n in 0..20 {
            let pick = next(&library, &history, n * 4 * MIN, None, &mut rng).unwrap();
            history.push(Play::of(&library.tracks[&pick.track], n * 4 * MIN));
            // Never from the most recently played half.
            assert!(pick.explanation.draw.unwrap().tracks <= 5);
            order.push(pick.track);
        }
        assert_ne!(order[..10], order[10..]);
        // Never played tracks do not play in their ID order.
        assert_ne!(order[..10], (11..=20).collect::<Vec<_>>()[..]);
    }

    #[test]
    fn a_track_held_back_by_its_ratio_leaves_its_place_in_the_draw() {
        // Track 1, the oldest, at 0.1 and a counter of 5: held back.
        let (mut library, history) = draw_library(0, Method::LeastRecentlyPlayed);
        library.tracks.get_mut(&1).unwrap().ratio = 1;
        library.counters.insert(1, 5);
        let mut rng = Rng::new(34);
        for _ in 0..30 {
            let outcome = next_outcome(&library, &history, 0, None, &mut rng);
            let pick = outcome.pick.unwrap();
            assert!((2..=4).contains(&pick.track), "{}", pick.track);
            assert_eq!(pick.explanation.draw.unwrap().pct, 20);
            assert!(outcome.held_by_ratio.iter().all(|held| held.track_id == 1));
        }
        // The whole draw held back: it widens.
        for id in 1..=4 {
            library.tracks.get_mut(&id).unwrap().ratio = 1;
            library.counters.insert(id, 5);
        }
        let outcome = next_outcome(&library, &history, 0, None, &mut rng);
        let pick = outcome.pick.unwrap();
        assert!([5, 6].contains(&pick.track));
        assert_eq!(
            (pick.explanation.step, pick.explanation.draw.unwrap().pct),
            (0, 30)
        );
        assert_eq!(outcome.held_by_ratio.len(), 4);
    }

    #[test]
    fn a_slot_overrides_its_category_s_draw_depth() {
        let (mut library, history) = draw_library(0, Method::LeastRecentlyPlayed);
        library.categories.get_mut(&1).unwrap().separation = SeparationOverrides {
            draw_start_pct: Some(50),
            draw_max_pct: Some(80),
            ..SeparationOverrides::default()
        };
        let (_, _, draw) = draws(&library, &history, 35)[0];
        assert_eq!(
            (draw.pct, draw.start_pct, draw.max_pct, draw.tracks),
            (50, 50, 80, 10)
        );
        library.clocks.get_mut(&1).unwrap().slots[0].separation = whole_draw();
        let picks = draws(&library, &history, 36);
        assert_eq!((picks[0].2.pct, picks[0].2.tracks), (100, 20));
        assert!(picks.iter().any(|(track, ..)| *track > 10));
        // A random slot has no draw.
        library.clocks.get_mut(&1).unwrap().slots[0].method = Method::Random;
        let pick = next(&library, &history, 0, None, &mut Rng::new(37)).unwrap();
        assert_eq!(pick.explanation.draw, None);
    }

    #[test]
    fn hours_without_a_clock_draw_with_the_station_s_depth() {
        let (mut library, history) = draw_library(0, Method::LeastRecentlyPlayed);
        library.schedule.hours.clear();
        library.settings.draw = DrawDepth {
            start_pct: 30,
            max_pct: 60,
        };
        let pick = next(&library, &history, 0, None, &mut Rng::new(38)).unwrap();
        let draw = pick.explanation.draw.unwrap();
        // The station's library: the category's 20 tracks (21 is in none).
        assert_eq!((draw.pct, draw.max_pct, draw.tracks), (30, 60, 6));
    }

    #[test]
    fn the_fallback_category_draws_with_its_own_depth() {
        let mut library = library(vec![slot(1, Method::LeastRecentlyPlayed)]);
        library.settings.fallback_category_id = Some(2);
        library.categories.get_mut(&2).unwrap().separation = SeparationOverrides {
            draw_start_pct: Some(40),
            draw_max_pct: Some(60),
            ..SeparationOverrides::default()
        };
        // Category 1 played within the hour: its track separation, even
        // quartered, holds it back.
        let mut history = History::default();
        for id in 1..=5 {
            history.push(Play::of(&library.tracks[&id], -(id as i64) * MIN));
        }
        let pick = next(&library, &history, 0, None, &mut Rng::new(39)).unwrap();
        assert_eq!(pick.explanation.step, 11);
        assert!((6..=10).contains(&pick.track));
        let draw = pick.explanation.draw.unwrap();
        assert_eq!(
            (draw.pct, draw.start_pct, draw.max_pct, draw.tracks),
            (40, 40, 60, 3)
        );
    }
}
