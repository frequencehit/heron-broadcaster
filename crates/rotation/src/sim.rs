// SPDX-License-Identifier: AGPL-3.0-or-later

//! Simulation: days of programming generated offline, then audited.
//!
//! The audit does not trust the rotation log: it recomputes each pick's
//! windows from its slot and relaxation step, and compares every pick with
//! the plays before it, one by one.

use serde::{Deserialize, Serialize};

use crate::restrict::{Subject, forbidding};
use crate::rules::{FALLBACK_STEP, LAST_STEP};
use crate::select::{self, Call, SeparationRule};
use crate::{
    AcrossWindows, CategoryId, DrawDepth, Elsewhere, ExactTime, Explanation, FillKind, History,
    Library, Method, Play, Position, Rng, Track, TrackId, Windows, dayparts, draw, ratio,
};

/// Shortest length a simulated track is counted for.
const MIN_LENGTH_MS: i64 = 1_000;

/// Time skipped when nothing can be picked (the fallback directory plays).
const GAP_MS: i64 = 60_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SimulatedPick {
    pub track_id: TrackId,
    pub start_unix_ms: i64,
    pub length_ms: u32,
    pub explanation: Explanation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Violation {
    /// Played closer to an earlier play than its windows allow.
    Separation {
        /// Index in the picks.
        pick: u32,
        rule: SeparationRule,
        earlier_unix_ms: i64,
        window_min: u32,
    },
    /// Played where a restriction forbids it.
    Restriction { pick: u32, restriction_id: u64 },
    /// Not from the category its slot and step call for.
    Category {
        pick: u32,
        expected: Option<CategoryId>,
    },
    /// Right after a track of a type that never plays twice in a row.
    InARow { pick: u32, type_id: crate::TypeId },
    /// Closer to another station's play or planned track than the windows
    /// across stations allow at its step.
    AcrossStations {
        pick: u32,
        rule: SeparationRule,
        station_id: crate::StationId,
        other_unix_ms: i64,
        window_min: u32,
    },
    /// Played while its play ratio counter was in its held-back steps.
    Ratio { pick: u32, ratio: u8, counter: u8 },
    /// Played in a daypart its last play keeps it out of, with the daypart
    /// separation at its step.
    Daypart {
        pick: u32,
        /// The track's last play before it.
        earlier_unix_ms: i64,
        dayparts: u8,
    },
    /// Played in the daypart it played in on one of the days before, with
    /// the days at its step.
    DaypartDays {
        pick: u32,
        /// The track's play in that daypart.
        earlier_unix_ms: i64,
        days: u8,
    },
    /// Drawn from beyond its draw depth: `rank` tracks of its pool, allowed
    /// by the restrictions, played longer ago than it (or never), and its
    /// draw had `tracks`.
    Draw { pick: u32, rank: u32, tracks: u32 },
    /// A slot with an exact time missed its window (it started early, or
    /// is not logged as an exact pick): `at_unix_ms` is its exact time,
    /// missing when the pick does not say it.
    ExactTime {
        pick: u32,
        at_unix_ms: Option<i64>,
        start_unix_ms: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Simulation {
    pub start_unix_ms: i64,
    pub end_unix_ms: i64,
    pub picks: Vec<SimulatedPick>,
    /// Minutes nothing could be picked (restrictions forbade every track):
    /// the engine's fallback directory would play.
    pub gap_minutes: u32,
    /// Picks per relaxation step, 0 to 12: every pick past step 0 is a
    /// logged relaxation.
    pub steps: Vec<u32>,
    pub violations: Vec<Violation>,
}

/// Programming from `start_ms` for `duration_ms`, after `history`; weights
/// move with each play as configured, and the play ratio counters from the
/// library's, in memory.
pub fn simulate(
    library: &Library,
    history: History,
    last: Option<Position>,
    start_ms: i64,
    duration_ms: i64,
    seed: u64,
) -> Simulation {
    simulate_with(library, history, last, start_ms, duration_ms, seed, true)
}

/// [`simulate`], looking ahead before exact times or not (to measure what
/// it gains).
pub(crate) fn simulate_with(
    library: &Library,
    mut history: History,
    mut last: Option<Position>,
    start_ms: i64,
    duration_ms: i64,
    seed: u64,
    lookahead: bool,
) -> Simulation {
    let mut working = library.clone();
    let before = history.plays().to_vec();
    let horizon = library.horizon_ms();
    let mut rng = Rng::new(seed);
    let mut picks: Vec<SimulatedPick> = Vec::new();
    let mut gap_minutes = 0;
    let end = start_ms + duration_ms;
    let mut at = start_ms;
    // Tracks held back by their ratio while nothing could be picked: their
    // counters moved, and the next pick lists them for the audit.
    let mut held_in_gaps = Vec::new();
    'run: while at < end {
        let outcome = select::outcome(&working, &history, at, last.as_ref(), &mut rng, lookahead);
        working.count_down(outcome.offered());
        let Some(mut pick) = outcome.pick else {
            held_in_gaps.extend(outcome.held_by_ratio);
            gap_minutes += 1;
            at += GAP_MS;
            continue;
        };
        if !held_in_gaps.is_empty() {
            held_in_gaps.append(&mut pick.explanation.held_by_ratio);
            pick.explanation.held_by_ratio = std::mem::take(&mut held_in_gaps);
        }
        // A linked slot's pick comes with the next slot's.
        for pick in pick.into_picks() {
            let Some(track) = working.tracks.get(&pick.track) else {
                break 'run;
            };
            // An exact time starts its slot at its own planned start: the
            // track before is cut there.
            let start = pick.explanation.at_unix_ms;
            if start < at
                && let Some(previous) = picks.last_mut()
            {
                let played = start - previous.start_unix_ms;
                previous.length_ms = u32::try_from(played.max(0)).unwrap_or(0);
            }
            at = start;
            let length_ms = pick.length_ms(&working);
            let next_ms = pick.next_ms(&working);
            history.push(Play::of(track, at));
            picks.push(SimulatedPick {
                track_id: pick.track,
                start_unix_ms: at,
                length_ms,
                explanation: pick.explanation,
            });
            // The next track starts at its mix point.
            at += i64::from(next_ms).max(MIN_LENGTH_MS);
            if pick.position.is_some() {
                last = pick.position;
            }
            let factor = working.settings.weights.play_factor;
            working.reweigh(pick.track, factor);
        }
        history.forget_before(at - horizon);
    }
    let mut steps = vec![0; usize::from(LAST_STEP) + 1];
    for pick in &picks {
        steps[usize::from(pick.explanation.step.min(LAST_STEP))] += 1;
    }
    let mut violations = audit(library, &before, &picks);
    violations.extend(audit_across(library, history.elsewhere(), &picks));
    Simulation {
        start_unix_ms: start_ms,
        end_unix_ms: end,
        picks,
        gap_minutes,
        steps,
        violations,
    }
}

/// The windows and category a pick of `track` must respect, from its
/// slot, the track's own daypart values and its step.
fn expected(
    library: &Library,
    explanation: &Explanation,
    track: &Track,
) -> (Windows, Option<CategoryId>) {
    let step = explanation.step;
    let relaxed = |windows| library.for_track(windows, track).at_step(step);
    // A gap filled before an exact time: its category's rules, never relaxed.
    if let Some(category) = fill_category(library, explanation) {
        let separation = library.categories.get(&category).map(|c| &c.separation);
        return (relaxed(library.windows(separation, None)), Some(category));
    }
    if step >= FALLBACK_STEP {
        let fallback = library.fallback_category();
        let windows = if step >= LAST_STEP {
            Windows::OFF
        } else {
            relaxed(library.windows(fallback.map(|c| &c.separation), None))
        };
        return (windows, fallback.map(|c| c.id));
    }
    let slot = explanation
        .clock_id
        .zip(explanation.slot)
        .and_then(|(clock, slot)| library.clocks.get(&clock)?.slots.get(slot as usize));
    match slot {
        // No clock: the whole library, rules enforced.
        None => (relaxed(library.windows(None, None)), None),
        Some(slot) if !slot.enforce_rules => (Windows::OFF, Some(slot.category)),
        Some(slot) => {
            let category = library
                .categories
                .get(&slot.category)
                .map(|c| &c.separation);
            (
                relaxed(library.windows(category, Some(&slot.separation))),
                Some(slot.category),
            )
        }
    }
}

/// The category a pick filling the gap before an exact time comes from.
fn fill_category(library: &Library, explanation: &Explanation) -> Option<CategoryId> {
    match explanation.fill.as_ref()?.kind {
        FillKind::Medley => library.medley_category,
        FillKind::Filler => library.settings.filler_category_id,
    }
}

/// The exact time a pick of an exact slot must meet, from its clock: its
/// slot, or the next one when it is linked to it. `None` for other picks.
fn expected_exact(library: &Library, explanation: &Explanation) -> Option<ExactTime> {
    if explanation.fill.is_some() {
        return None;
    }
    let clock = library.clocks.get(&explanation.clock_id?)?;
    let index = explanation.slot? as usize;
    if index >= clock.slots.len() || clock.follows_linked(index) {
        return None;
    }
    clock.exact(index)
}

/// The draw depth a pick must come from, from its slot and step, and the
/// category its pool is (`None`: the station's library); no depth for a
/// random slot and at step 12.
fn expected_draw(
    library: &Library,
    explanation: &Explanation,
) -> Option<(DrawDepth, Option<CategoryId>)> {
    let step = explanation.step;
    if step >= LAST_STEP {
        return None;
    }
    let slot = explanation
        .clock_id
        .zip(explanation.slot)
        .and_then(|(clock, slot)| library.clocks.get(&clock)?.slots.get(slot as usize));
    if let Some(category) = fill_category(library, explanation) {
        let separation = library.categories.get(&category).map(|c| &c.separation);
        return Some((library.draw(separation, None), Some(category)));
    }
    if slot.is_some_and(|slot| slot.method == Method::Random) {
        return None;
    }
    if step >= FALLBACK_STEP {
        let fallback = library.fallback_category();
        return Some((
            library.draw(fallback.map(|c| &c.separation), None),
            fallback.map(|c| c.id),
        ));
    }
    Some(match slot {
        None => (library.draw(None, None), None),
        Some(slot) => {
            let category = library.categories.get(&slot.category);
            (
                library.draw(category.map(|c| &c.separation), Some(&slot.separation)),
                Some(slot.category),
            )
        }
    })
}

/// Checks each pick against the plays before it (`before`, then the
/// earlier picks): separation as relaxed at its step (daypart separation
/// against the track's last play, the same daypart against its plays on
/// the days before), restrictions always,
/// the draw depth (the tracks of its pool played longer ago than it are
/// fewer than its draw had: the one logged at step 0 and 11, within the
/// start and the maximum, the maximum at the other steps), and the play
/// ratio always: the counters start from the library's and move with the
/// tracks each pick lists as held back, then with the pick.
pub fn audit(library: &Library, before: &[Play], picks: &[SimulatedPick]) -> Vec<Violation> {
    let mut violations = Vec::new();
    // Counters moved so far; the library's before.
    let mut counters: std::collections::HashMap<TrackId, u8> = std::collections::HashMap::new();
    let mut count_down = |track: TrackId| {
        let counter = counters
            .get(&track)
            .copied()
            .unwrap_or_else(|| library.counter(track));
        counters.insert(track, ratio::after(counter));
        counter
    };
    let mut timeline: Vec<Play> = before.to_vec();
    timeline.sort_by_key(|play| play.start_ms);
    // When each track last played, so far.
    let mut last_played: std::collections::HashMap<TrackId, i64> = std::collections::HashMap::new();
    for play in &timeline {
        if let Some(track) = play.track {
            last_played.insert(track, play.start_ms);
        }
    }
    for (index, pick) in picks.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        let Some(track) = library.tracks.get(&pick.track_id) else {
            violations.push(Violation::Category {
                pick: index,
                expected: None,
            });
            continue;
        };
        for held in &pick.explanation.held_by_ratio {
            count_down(held.track_id);
        }
        let counter = count_down(track.id);
        if !ratio::plays(track.ratio, counter) {
            violations.push(Violation::Ratio {
                pick: index,
                ratio: track.ratio,
                counter,
            });
        }
        let at = pick.start_unix_ms;
        let (windows, category) = expected(library, &pick.explanation, track);
        if pick.explanation.category_id != category
            || category.is_some_and(|c| !track.categories.contains(&c))
        {
            violations.push(Violation::Category {
                pick: index,
                expected: category,
            });
        }
        let longest = i64::from(windows.longest_min()) * 60_000;
        for earlier in timeline
            .iter()
            .rev()
            .filter(|p| p.start_ms <= at)
            .take_while(|p| at - p.start_ms < longest)
        {
            let gap = at - earlier.start_ms;
            let close = |minutes: u32| gap < i64::from(minutes) * 60_000;
            let same_artist = track
                .artists
                .iter()
                .any(|a| earlier.artists.iter().any(|b| library.same_artist(*a, *b)));
            let same_key = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
            let checks = [
                (
                    SeparationRule::Track,
                    earlier.track == Some(track.id),
                    windows.track_min,
                ),
                (
                    SeparationRule::Title,
                    same_key(&track.title_key, &earlier.title_key),
                    windows.title_min,
                ),
                (SeparationRule::Artist, same_artist, windows.artist_min),
                (
                    SeparationRule::Album,
                    same_key(&track.album_key, &earlier.album_key),
                    windows.album_min,
                ),
            ];
            for (rule, same, window_min) in checks {
                if same && close(window_min) {
                    violations.push(Violation::Separation {
                        pick: index,
                        rule,
                        earlier_unix_ms: earlier.start_ms,
                        window_min,
                    });
                }
            }
        }
        if windows.dayparts > 0
            && let Some(last) = timeline
                .iter()
                .rev()
                .find(|p| p.start_ms <= at && p.track == Some(track.id))
            && dayparts::holds(
                library.daypart_count(),
                windows.dayparts,
                library.local(last.start_ms).datetime(),
                library.local(at).datetime(),
            )
        {
            violations.push(Violation::Daypart {
                pick: index,
                earlier_unix_ms: last.start_ms,
                dayparts: windows.dayparts,
            });
        }
        if windows.daypart_days > 0 {
            let count = library.daypart_count();
            let local = library.local(at).datetime();
            let since =
                at - i64::from(dayparts::days_lookback_min(count, windows.daypart_days)) * 60_000;
            if let Some(earlier) = timeline
                .iter()
                .rev()
                .filter(|p| p.start_ms <= at)
                .take_while(|p| p.start_ms >= since)
                .find(|p| {
                    p.track == Some(track.id)
                        && dayparts::holds_days(
                            count,
                            windows.daypart_days,
                            library.local(p.start_ms).datetime(),
                            local,
                        )
                })
            {
                violations.push(Violation::DaypartDays {
                    pick: index,
                    earlier_unix_ms: earlier.start_ms,
                    days: windows.daypart_days,
                });
            }
        }
        let previous = timeline
            .iter()
            .rev()
            .find(|p| p.start_ms < at)
            .map(|p| p.categories.as_slice());
        let subject = Subject {
            track: track.id,
            artists: &track.artists,
            categories: &track.categories,
        };
        let local = library.local(at).datetime();
        if let Some(restriction) = forbidding(&library.restrictions, &subject, local, previous) {
            violations.push(Violation::Restriction {
                pick: index,
                restriction_id: restriction.id,
            });
        }
        if let Some(type_id) = library.repeated_type(previous, &track.categories) {
            violations.push(Violation::InARow {
                pick: index,
                type_id,
            });
        }
        if let Some((depth, category)) = expected_draw(library, &pick.explanation) {
            let logged = pick.explanation.draw.map(|draw| draw.pct);
            let step = pick.explanation.step;
            let pct = if step == 0 || step == FALLBACK_STEP {
                logged
                    .unwrap_or(depth.start_pct)
                    .clamp(depth.start_pct, depth.max_pct)
            } else {
                depth.max_pct
            };
            let (pool, _) = Call::new(library, category, None).allowed(library, local, previous);
            let key = last_played.get(&track.id).copied();
            let rank = if key.is_none() {
                0
            } else {
                pool.iter()
                    .filter(|(other, _)| {
                        other.id != track.id && last_played.get(&other.id).copied() < key
                    })
                    .count()
            };
            let tracks = draw::size(pool.len(), pct);
            if rank >= tracks {
                violations.push(Violation::Draw {
                    pick: index,
                    rank: u32::try_from(rank).unwrap_or(u32::MAX),
                    tracks: u32::try_from(tracks).unwrap_or(u32::MAX),
                });
            }
        }
        if let Some(exact) = expected_exact(library, &pick.explanation) {
            let logged = pick.explanation.exact.map(|log| log.at_unix_ms);
            let met = logged.is_some_and(|at_ms| {
                let local = library.local(at_ms);
                let into_hour =
                    u32::from(local.minute() as u8) * 60 + u32::from(local.second() as u8);
                let window = exact.window(at_ms);
                into_hour == exact.at_s && (window.from_ms..=window.cut_ms).contains(&at)
            });
            if !met {
                violations.push(Violation::ExactTime {
                    pick: index,
                    at_unix_ms: logged,
                    start_unix_ms: at,
                });
            }
        }
        last_played.insert(track.id, at);
        timeline.push(Play::of(track, at));
    }
    violations
}

/// The windows across stations a pick must respect, from its slot and step.
fn expected_across(library: &Library, explanation: &Explanation) -> AcrossWindows {
    if explanation.step >= FALLBACK_STEP || !explanation.enforce_rules {
        return AcrossWindows::OFF;
    }
    library.settings.across.windows().at_step(explanation.step)
}

/// Checks each pick against the other stations' plays and planned tracks
/// (`elsewhere`), before or after it, with the windows across stations as
/// relaxed at its step.
pub fn audit_across(
    library: &Library,
    elsewhere: &[Elsewhere],
    picks: &[SimulatedPick],
) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (index, pick) in picks.iter().enumerate() {
        let Some(track) = library.tracks.get(&pick.track_id) else {
            continue;
        };
        let windows = expected_across(library, &pick.explanation);
        let at = pick.start_unix_ms;
        for other in elsewhere {
            let play = &other.play;
            let gap = (at - play.start_ms).abs();
            let same_key = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
            let same_artist = track
                .artists
                .iter()
                .any(|a| play.artists.iter().any(|b| library.same_artist(*a, *b)));
            for (rule, same, window_min) in [
                (
                    SeparationRule::Track,
                    play.track == Some(track.id),
                    windows.track_min,
                ),
                (
                    SeparationRule::Title,
                    same_key(&track.title_key, &play.title_key),
                    windows.title_min,
                ),
                (SeparationRule::Artist, same_artist, windows.artist_min),
            ] {
                if same && gap < i64::from(window_min) * 60_000 {
                    violations.push(Violation::AcrossStations {
                        pick: u32::try_from(index).unwrap_or(u32::MAX),
                        rule,
                        station_id: other.station_id,
                        other_unix_ms: play.start_ms,
                        window_min,
                    });
                }
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use jiff::tz::TimeZone;

    use super::*;
    use crate::{
        Category, Clock, HeldByRatio, LibraryParts, Member, Method, Mode, Restriction, Rule,
        Schedule, SeparationOverrides, Slot, Target, Track, next,
    };

    const MIN: i64 = 60_000;

    fn library() -> Library {
        let track = |id, artist, title: &str| Track {
            id,
            title_key: Some(title.to_owned()),
            album_key: None,
            artists: vec![artist],
            categories: vec![],
            length_ms: 200_000,
            mix_ms: None,
            ratio: 10,
            dayparts: None,
            daypart_days: None,
        };
        Library::new(
            LibraryParts {
                tracks: vec![
                    track(1, 10, "a"),
                    track(2, 11, "b"),
                    track(3, 12, "a"),
                    track(4, 13, "d"),
                ],
                categories: vec![Category {
                    id: 1,
                    name: "All".into(),
                    separation: SeparationOverrides::default(),
                    members: (1..=3).map(|track| Member { track, weight: 1.0 }).collect(),
                }],
                similar: vec![(10, 11)],
                restrictions: vec![Restriction {
                    id: 7,
                    target: Target::Track(2),
                    rule: Rule::Daypart {
                        mode: Mode::Ban,
                        days: vec![1, 2, 3, 4, 5, 6, 7],
                        start_hour: 12,
                        end_hour: 13,
                    },
                }],
                clocks: vec![Clock {
                    id: 1,
                    name: "Clock".into(),
                    looping: false,
                    slots: vec![Slot {
                        category: 1,
                        method: Method::Random,
                        enforce_rules: true,
                        linked_to_next: false,
                        exact: None,
                        separation: SeparationOverrides::default(),
                    }],
                }],
                schedule: Schedule {
                    hours: (1..=7)
                        .flat_map(|d| (0..24).map(move |h| ((d, h), 1)))
                        .collect(),
                    ..Schedule::default()
                },
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        )
    }

    /// A pick claiming to come from step `step` of the clock's only slot.
    fn pick(library: &Library, track: TrackId, at: i64, step: u8) -> SimulatedPick {
        SimulatedPick {
            track_id: track,
            start_unix_ms: at,
            length_ms: 200_000,
            explanation: Explanation {
                at_unix_ms: at,
                schedule_id: None,
                schedule_name: None,
                clock_id: Some(1),
                clock_name: None,
                slot: Some(0),
                slot_category_id: Some(1),
                category_id: if step >= FALLBACK_STEP { None } else { Some(1) },
                category_name: None,
                method: Method::Random,
                enforce_rules: true,
                step,
                step_description: String::new(),
                windows: library.settings.separation.into(),
                pool: 3,
                restricted: 0,
                blocked: crate::Blocked::default(),
                candidates: 1,
                last_played_unix_ms: None,
                weight: 1.0,
                skipped_slots: vec![],
                across: AcrossWindows::OFF,
                held_elsewhere: None,
                ratio: None,
                counter: None,
                held_by_ratio: vec![],
                daypart: None,
                draw: None,
                linked: false,
                exact: None,
                fill: None,
                lookahead: None,
            },
        }
    }

    fn with_across(mut library: Library, artist_min: u32) -> Library {
        library.settings.across = crate::AcrossStations {
            artist_min,
            ..crate::AcrossStations::default()
        };
        library
    }

    fn elsewhere(track: &crate::Track, at: i64, planned: bool) -> Elsewhere {
        Elsewhere {
            station_id: 2,
            planned,
            play: Play::of(track, at),
        }
    }

    /// Another station's play or planned track, before or after a pick,
    /// within the window across stations at its step.
    #[test]
    fn picks_are_audited_against_other_stations() {
        let library = with_across(library(), 30);
        let noon = 12 * 60 * MIN;
        // Artist 10 (track 1) on station 2 at noon; similar artist 11 is track 2.
        let others = [elsewhere(&library.tracks[&1], noon, false)];
        let across = |track, at, step| {
            audit_across(&library, &others, &[pick(&library, track, at, step)])
                .into_iter()
                .map(|violation| match violation {
                    Violation::AcrossStations {
                        rule, station_id, ..
                    } => (rule, station_id),
                    other => panic!("{other:?}"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(across(1, noon + 10 * MIN, 0), [(SeparationRule::Artist, 2)]);
        assert_eq!(
            across(1, noon - 10 * MIN, 0),
            [(SeparationRule::Artist, 2)],
            "planned after"
        );
        assert_eq!(
            across(2, noon + 10 * MIN, 0),
            [(SeparationRule::Artist, 2)],
            "similar"
        );
        assert!(across(1, noon + 40 * MIN, 0).is_empty());
        // Halved at step 5, dropped at 6.
        assert!(across(1, noon + 20 * MIN, 5).is_empty());
        assert_eq!(across(1, noon + 10 * MIN, 5).len(), 1);
        assert!(across(1, noon + 10 * MIN, 6).is_empty());
        let off = audit_across(
            &with_across(library.clone(), 0),
            &others,
            &[pick(&library, 1, noon, 0)],
        );
        assert!(off.is_empty());
    }

    /// Two stations sharing a library, each planning in turn with the
    /// other's plays: over a week, an artist never plays on both within 30
    /// minutes unless the later of the two picks logged that rule relaxed.
    #[test]
    fn two_stations_keep_their_artists_apart_for_a_week() {
        let tracks: Vec<crate::Track> = (1..=120)
            .map(|id| crate::Track {
                id,
                title_key: Some(format!("title {id}")),
                album_key: None,
                artists: vec![100 + id % 60],
                categories: vec![],
                length_ms: 200_000,
                mix_ms: None,
                ratio: 10,
                dayparts: None,
                daypart_days: None,
            })
            .collect();
        let library = Library::new(
            LibraryParts {
                tracks,
                settings: crate::RotationSettings {
                    across: crate::AcrossStations {
                        artist_min: 30,
                        title_min: 30,
                        track_min: 60,
                        station_ids: vec![],
                    },
                    ..crate::RotationSettings::default()
                },
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        );
        let start = 1_700_000_000_000;
        let end = start + 7 * 24 * 60 * MIN;
        let mut rng = Rng::new(7);
        let mut histories = [History::default(), History::default()];
        let mut at = [start, start + 90_000];
        let mut picks: [Vec<SimulatedPick>; 2] = [Vec::new(), Vec::new()];
        while at[0] < end || at[1] < end {
            let me = usize::from(at[1] < at[0]);
            let other = 1 - me;
            let elsewhere = picks[other]
                .iter()
                .rev()
                .take_while(|pick| pick.start_unix_ms > at[me] - 120 * MIN)
                .map(|pick| Elsewhere {
                    station_id: other as u64 + 1,
                    planned: pick.start_unix_ms > at[me],
                    play: Play::of(&library.tracks[&pick.track_id], pick.start_unix_ms),
                })
                .collect();
            let history = std::mem::take(&mut histories[me]).with_elsewhere(elsewhere);
            let pick = next(&library, &history, at[me], None, &mut rng).unwrap();
            let track = &library.tracks[&pick.track];
            histories[me] = history;
            histories[me].push(Play::of(track, at[me]));
            picks[me].push(SimulatedPick {
                track_id: pick.track,
                start_unix_ms: at[me],
                length_ms: track.length_ms,
                explanation: pick.explanation,
            });
            at[me] += i64::from(track.length_ms);
            histories[me].forget_before(at[me] - library.horizon_ms());
        }
        assert!(picks[0].len() > 3_000 && picks[1].len() > 3_000);
        let mut relaxed = 0;
        for me in 0..2 {
            let other = 1 - me;
            for (index, pick) in picks[me].iter().enumerate() {
                // The other station's plays planned before this pick: of
                // two picks, the later one is the one that answers.
                let from = picks[other]
                    .partition_point(|o| o.start_unix_ms <= pick.start_unix_ms - 120 * MIN);
                let to = picks[other].partition_point(|o| o.start_unix_ms < pick.start_unix_ms);
                let before: Vec<Elsewhere> = picks[other][from..to]
                    .iter()
                    .map(|o| Elsewhere {
                        station_id: other as u64 + 1,
                        planned: false,
                        play: Play::of(&library.tracks[&o.track_id], o.start_unix_ms),
                    })
                    .collect();
                let violations = audit_across(&library, &before, std::slice::from_ref(pick));
                assert!(
                    violations.is_empty(),
                    "station {me} pick {index}: {violations:?}"
                );
                relaxed += u32::from(pick.explanation.step > 0);
            }
        }
        // 120 tracks of 60 artists hold: almost nothing is relaxed.
        assert!(relaxed < 100, "{relaxed} relaxed picks");
        let held: u32 = picks
            .iter()
            .flatten()
            .map(|pick| pick.explanation.blocked.across)
            .sum();
        assert!(held > 0, "the rules across stations held nothing back");
        assert!(
            picks
                .iter()
                .flatten()
                .any(|pick| pick.explanation.held_elsewhere.is_some())
        );
    }

    fn rules(violations: &[Violation]) -> Vec<SeparationRule> {
        violations
            .iter()
            .filter_map(|v| match v {
                Violation::Separation { rule, .. } => Some(*rule),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn catches_repeats_titles_and_similar_artists() {
        let library = library();
        // Track 1 again after 100 min: track and title (240), artist (120).
        let picks = [pick(&library, 1, 0, 0), pick(&library, 1, 100 * MIN, 0)];
        assert_eq!(
            rules(&audit(&library, &[], &picks)),
            [
                SeparationRule::Track,
                SeparationRule::Title,
                SeparationRule::Artist
            ]
        );
        // Track 2's artist is similar to track 1's.
        let picks = [pick(&library, 1, 0, 0), pick(&library, 2, 100 * MIN, 0)];
        assert_eq!(
            rules(&audit(&library, &[], &picks)),
            [SeparationRule::Artist]
        );
        // Track 3 has the same title as track 1.
        let picks = [pick(&library, 1, 0, 0), pick(&library, 3, 200 * MIN, 0)];
        assert_eq!(
            rules(&audit(&library, &[], &picks)),
            [SeparationRule::Title]
        );
        // Plays before the simulation count too.
        let before = [Play::of(&library.tracks[&1], -10 * MIN)];
        let picks = [pick(&library, 2, 0, 0)];
        assert_eq!(
            rules(&audit(&library, &before, &picks)),
            [SeparationRule::Artist]
        );
    }

    #[test]
    fn relaxed_picks_are_checked_against_relaxed_windows() {
        let library = library();
        // Artist 100 min apart: fine once artist separation is halved
        // (step 5: 60 min), not before.
        let close = |step| [pick(&library, 1, 0, 0), pick(&library, 2, 100 * MIN, step)];
        assert!(audit(&library, &[], &close(5)).is_empty());
        assert_eq!(
            rules(&audit(&library, &[], &close(4))),
            [SeparationRule::Artist]
        );
        let repeat = |step| [pick(&library, 1, 0, 0), pick(&library, 1, 70 * MIN, step)];
        // Step 10 keeps a quarter of the track window: 60 min.
        assert!(audit(&library, &[], &repeat(10)).is_empty());
        assert_eq!(
            rules(&audit(&library, &[], &repeat(9))),
            [SeparationRule::Track]
        );
        // Step 12: no rules at all.
        let immediate = [pick(&library, 1, 0, 0), pick(&library, 1, MIN, 12)];
        assert!(audit(&library, &[], &immediate).is_empty());
    }

    #[test]
    fn catches_a_track_back_in_a_daypart_kept_out() {
        let mut library = library();
        library.settings.separation = Windows::OFF.into();
        library.categories.get_mut(&1).unwrap().separation.dayparts = Some(1);
        let hour = 60 * MIN;
        // Played in A at 01:00, again in B at 07:00.
        let again = |at, step| [pick(&library, 1, hour, 0), pick(&library, 1, at, step)];
        assert_eq!(
            audit(&library, &[], &again(7 * hour, 0)),
            [Violation::Daypart {
                pick: 1,
                earlier_unix_ms: hour,
                dayparts: 1,
            }]
        );
        // Halved at step 3 (1 becomes 0); C is never kept out.
        assert!(audit(&library, &[], &again(7 * hour, 3)).is_empty());
        assert!(audit(&library, &[], &again(13 * hour, 0)).is_empty());
        // Plays before the simulation count, across midnight (D, then A);
        // only the last one does.
        let before = [Play::of(&library.tracks[&1], -2 * hour)];
        let at_five = [pick(&library, 1, 5 * hour, 0)];
        assert_eq!(audit(&library, &before, &at_five).len(), 1);
        let before = [before[0].clone(), Play::of(&library.tracks[&1], 30 * MIN)];
        assert!(audit(&library, &before, &at_five).is_empty());
    }

    #[test]
    fn audits_a_track_s_own_daypart_values() {
        let mut library = library();
        library.settings.separation = Windows::OFF.into();
        library.categories.get_mut(&1).unwrap().separation.dayparts = Some(1);
        let hour = 60 * MIN;
        let again = |library: &Library, at| [pick(library, 1, hour, 0), pick(library, 1, at, 0)];
        // Its own 0 replaces the category's 1: B is free.
        library.tracks.get_mut(&1).unwrap().dayparts = Some(0);
        assert!(audit(&library, &[], &again(&library, 7 * hour)).is_empty());
        // Its own 2 keeps C out too.
        library.tracks.get_mut(&1).unwrap().dayparts = Some(2);
        assert_eq!(
            audit(&library, &[], &again(&library, 13 * hour)),
            [Violation::Daypart {
                pick: 1,
                earlier_unix_ms: hour,
                dayparts: 2,
            }]
        );
        // Its own days in the same daypart, the category having none.
        library.tracks.get_mut(&1).unwrap().dayparts = None;
        library.tracks.get_mut(&1).unwrap().daypart_days = Some(1);
        assert_eq!(
            audit(&library, &[], &again(&library, 25 * hour)),
            [Violation::DaypartDays {
                pick: 1,
                earlier_unix_ms: hour,
                days: 1,
            }]
        );
    }

    #[test]
    fn catches_a_pick_from_beyond_its_draw() {
        let mut library = library();
        library.clocks.get_mut(&1).unwrap().slots[0].method = Method::LeastRecentlyPlayed;
        // 10 tracks in the category: 1, 2, 3, 10 to 16, played in that order
        // long ago (no separation).
        let ids = [1, 2, 3, 10, 11, 12, 13, 14, 15, 16];
        for id in 10..=16 {
            let mut track = library.tracks[&4].clone();
            track.id = id;
            track.title_key = Some(format!("t{id}"));
            track.artists = vec![100 + id];
            track.categories = vec![1];
            library.tracks.insert(id, track);
            let members = &mut library.categories.get_mut(&1).unwrap().members;
            members.push(Member {
                track: id,
                weight: 1.0,
            });
        }
        let before: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(rank, id)| Play::of(&library.tracks[id], -(2_000 - rank as i64 * 10) * MIN))
            .collect();
        let drawn = |track, step, pct: Option<u8>| {
            let mut pick = pick(&library, track, 0, step);
            pick.explanation.draw = pct.map(|pct| crate::DrawLog {
                pct,
                start_pct: 20,
                max_pct: 50,
                tracks: 0,
                blocked: crate::Blocked::default(),
            });
            audit(&library, &before, &[pick])
        };
        // The oldest is in any draw; the most recent in none.
        assert_eq!(drawn(1, 0, Some(20)), []);
        assert_eq!(
            drawn(16, 0, Some(20)),
            [Violation::Draw {
                pick: 0,
                rank: 9,
                tracks: 3
            }]
        );
        // The fifth oldest: in a draw widened to 50 %, not in one of 20 %.
        assert_eq!(drawn(11, 0, Some(50)), []);
        assert_eq!(
            drawn(11, 0, Some(20)),
            [Violation::Draw {
                pick: 0,
                rank: 4,
                tracks: 3
            }]
        );
        // Past step 0 the draw is at its maximum, whatever is logged; never
        // beyond it.
        assert_eq!(drawn(11, 5, Some(20)), []);
        assert!(matches!(
            drawn(13, 5, Some(100))[..],
            [Violation::Draw { tracks: 5, .. }]
        ));
        // Step 12 has no draw.
        assert!(
            !drawn(16, 12, None)
                .iter()
                .any(|v| matches!(v, Violation::Draw { .. }))
        );
    }

    #[test]
    fn catches_restrictions_and_wrong_categories() {
        let library = library();
        let noon = 12 * 60 * MIN;
        let violations = audit(&library, &[], &[pick(&library, 2, noon, 0)]);
        assert_eq!(
            violations,
            [Violation::Restriction {
                pick: 0,
                restriction_id: 7
            }]
        );
        // Relaxation never excuses a restriction.
        let violations = audit(&library, &[], &[pick(&library, 2, noon, 12)]);
        assert!(matches!(violations[..], [Violation::Restriction { .. }]));
        // Track 4 is not in the slot's category.
        let violations = audit(&library, &[], &[pick(&library, 4, 0, 0)]);
        assert_eq!(
            violations,
            [Violation::Category {
                pick: 0,
                expected: Some(1)
            }]
        );
    }

    #[test]
    fn catches_a_track_played_while_its_ratio_holds_it_back() {
        let mut library = library();
        library.tracks.get_mut(&1).unwrap().ratio = 2;
        library.tracks.get_mut(&2).unwrap().ratio = 1;
        library.counters.insert(2, 3);
        let hour = 60 * MIN;
        // Track 1 plays at 10 and 9, then at 8 it is held back.
        let picks = [
            pick(&library, 1, 0, 0),
            pick(&library, 1, 5 * hour, 12),
            pick(&library, 1, 10 * hour, 12),
        ];
        assert_eq!(
            audit(&library, &[], &picks),
            [Violation::Ratio {
                pick: 2,
                ratio: 2,
                counter: 8
            }]
        );
        // Held back by its ratio, listed in the pick: never flagged, and
        // its counter moves.
        let mut third = pick(&library, 3, 10 * hour, 0);
        third.explanation.held_by_ratio = vec![HeldByRatio {
            track_id: 1,
            ratio: 2,
            counter: 8,
        }];
        let fourth = pick(&library, 2, 15 * hour, 0);
        let picks = [picks[0].clone(), picks[1].clone(), third, fourth];
        // Track 2 starts at 3 of 10: held back at 0.1.
        assert_eq!(
            audit(&library, &[], &picks),
            [Violation::Ratio {
                pick: 3,
                ratio: 1,
                counter: 3
            }]
        );
    }

    #[test]
    fn catches_a_type_twice_in_a_row() {
        let mut library = library();
        // The slot's category is of a type that never plays twice in a row.
        library.no_two_in_a_row = HashMap::from([(1, 2)]);
        let picks = [pick(&library, 1, 0, 0), pick(&library, 3, 300 * MIN, 0)];
        assert_eq!(
            audit(&library, &[], &picks),
            [Violation::InARow {
                pick: 1,
                type_id: 2
            }]
        );
        // Not after a track of another type.
        let before = [Play::of(&library.tracks[&4], -MIN)];
        assert!(audit(&library, &before, &picks[..1]).is_empty());
    }
}
