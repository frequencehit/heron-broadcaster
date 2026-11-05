// SPDX-License-Identifier: AGPL-3.0-or-later

//! Simulation: days of programming generated offline, then audited.
//!
//! The audit does not trust the rotation log: it recomputes each pick's
//! windows from its slot and relaxation step, and compares every pick with
//! the plays before it, one by one.

use serde::{Deserialize, Serialize};

use crate::restrict::{Subject, forbidding};
use crate::rules::{FALLBACK_STEP, LAST_STEP};
use crate::select::SeparationRule;
use crate::{
    AcrossWindows, CategoryId, Elsewhere, Explanation, History, Library, Play, Position, Rng,
    TrackId, Windows, next,
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
    /// Picks per relaxation step, 0 to 10: every pick past step 0 is a
    /// logged relaxation.
    pub steps: Vec<u32>,
    pub violations: Vec<Violation>,
}

/// Programming from `start_ms` for `duration_ms`, after `history`; weights
/// move with each play as configured.
pub fn simulate(
    library: &Library,
    mut history: History,
    mut last: Option<Position>,
    start_ms: i64,
    duration_ms: i64,
    seed: u64,
) -> Simulation {
    let mut working = library.clone();
    let before = history.plays().to_vec();
    let horizon = library.horizon_ms();
    let mut rng = Rng::new(seed);
    let mut picks = Vec::new();
    let mut gap_minutes = 0;
    let end = start_ms + duration_ms;
    let mut at = start_ms;
    while at < end {
        let Some(pick) = next(&working, &history, at, last.as_ref(), &mut rng) else {
            gap_minutes += 1;
            at += GAP_MS;
            continue;
        };
        let Some(track) = working.tracks.get(&pick.track) else {
            break;
        };
        history.push(Play::of(track, at));
        picks.push(SimulatedPick {
            track_id: pick.track,
            start_unix_ms: at,
            length_ms: track.length_ms,
            explanation: pick.explanation,
        });
        at += i64::from(track.length_ms).max(MIN_LENGTH_MS);
        if pick.position.is_some() {
            last = pick.position;
        }
        let factor = working.settings.weights.play_factor;
        working.reweigh(pick.track, factor);
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

/// The windows and category a pick must respect, from its slot and step.
fn expected(library: &Library, explanation: &Explanation) -> (Windows, Option<CategoryId>) {
    let base = library.settings.separation;
    let step = explanation.step;
    if step >= FALLBACK_STEP {
        let fallback = library.fallback_category();
        let windows = if step >= LAST_STEP {
            Windows::OFF
        } else {
            base.with(&fallback.map(|c| c.separation).unwrap_or_default())
                .at_step(step)
        };
        return (windows, fallback.map(|c| c.id));
    }
    let slot = explanation
        .clock_id
        .zip(explanation.slot)
        .and_then(|(clock, slot)| library.clocks.get(&clock)?.slots.get(slot as usize));
    match slot {
        // No clock: the whole library, rules enforced.
        None => (base.at_step(step), None),
        Some(slot) if !slot.enforce_rules => (Windows::OFF, Some(slot.category)),
        Some(slot) => {
            let category = library
                .categories
                .get(&slot.category)
                .map(|c| c.separation)
                .unwrap_or_default();
            (
                base.with(&category).with(&slot.separation).at_step(step),
                Some(slot.category),
            )
        }
    }
}

/// Checks each pick against the plays before it (`before`, then the
/// earlier picks): separation as relaxed at its step, restrictions always.
pub fn audit(library: &Library, before: &[Play], picks: &[SimulatedPick]) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut timeline: Vec<Play> = before.to_vec();
    timeline.sort_by_key(|play| play.start_ms);
    for (index, pick) in picks.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        let Some(track) = library.tracks.get(&pick.track_id) else {
            violations.push(Violation::Category {
                pick: index,
                expected: None,
            });
            continue;
        };
        let at = pick.start_unix_ms;
        let (windows, category) = expected(library, &pick.explanation);
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
        Category, Clock, LibraryParts, Member, Method, Mode, Restriction, Rule, Schedule,
        SeparationOverrides, Slot, Target, Track,
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
                windows: library.settings.separation,
                pool: 3,
                restricted: 0,
                blocked: crate::Blocked::default(),
                candidates: 1,
                last_played_unix_ms: None,
                weight: 1.0,
                skipped_slots: vec![],
                across: AcrossWindows::OFF,
                held_elsewhere: None,
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
        // Halved at step 3, dropped at 4.
        assert!(across(1, noon + 20 * MIN, 3).is_empty());
        assert_eq!(across(1, noon + 10 * MIN, 3).len(), 1);
        assert!(across(1, noon + 10 * MIN, 4).is_empty());
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
        // (step 3: 60 min), not before.
        let close = |step| [pick(&library, 1, 0, 0), pick(&library, 2, 100 * MIN, step)];
        assert!(audit(&library, &[], &close(3)).is_empty());
        assert_eq!(
            rules(&audit(&library, &[], &close(2))),
            [SeparationRule::Artist]
        );
        let repeat = |step| [pick(&library, 1, 0, 0), pick(&library, 1, 70 * MIN, step)];
        // Step 8 keeps a quarter of the track window: 60 min.
        assert!(audit(&library, &[], &repeat(8)).is_empty());
        assert_eq!(
            rules(&audit(&library, &[], &repeat(7))),
            [SeparationRule::Track]
        );
        // Step 10: no rules at all.
        let immediate = [pick(&library, 1, 0, 0), pick(&library, 1, MIN, 10)];
        assert!(audit(&library, &[], &immediate).is_empty());
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
        let violations = audit(&library, &[], &[pick(&library, 2, noon, 10)]);
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
