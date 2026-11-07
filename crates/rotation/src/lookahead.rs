// SPDX-License-Identifier: AGPL-3.0-or-later

//! Looking ahead before an exact time: several endings simulated up to it
//! with the real rules, only the random draws differing, and the first pick
//! of the best one kept (`docs/rotation.md#exact-times`).

use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

use crate::exact::Window;
use crate::ratio::HeldByRatio;
use crate::select::{self, Pick};
use crate::{History, Library, Play, Position, Rng};

/// Rotation looks ahead once the exact time is this close to a pick...
pub const REACH_MS: i64 = 15 * 60_000;
/// ...or this many slots, or fewer, are left before it.
pub const REACH_SLOTS: usize = 4;
/// Endings simulated for a pick.
pub const DRAWS: u32 = 8;
/// The most picks an ending simulates before the exact slot.
pub const MAX_PICKS: usize = 20;
/// A track's last play counts this long ago at most (never played ones
/// too), for the tie between endings.
const MAX_AGE_MS: i64 = 30 * 24 * 3_600_000;

/// How a pick looked ahead, in the rotation log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LookaheadLog {
    /// The exact time looked ahead to (Unix ms).
    pub exact_at_unix_ms: i64,
    /// Endings simulated.
    pub draws: u32,
    /// The best ending fades out the track before the exact slot: it ends
    /// after the cut (the end of the window, the exact time in hard mode).
    /// Every ending simulated did.
    #[serde(default)]
    pub faded: bool,
    /// The best ending: the tracks before the exact slot end inside its
    /// window (at the exact time in hard mode).
    pub in_window: bool,
    /// Where they end, from the exact time; negative when early. Missing
    /// when no ending reached the exact slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<i64>,
    /// Time filled before the exact slot (medley or filler track).
    pub filling_ms: u32,
    /// Relaxation steps of its picks, added up.
    pub relaxed_steps: u32,
}

/// One simulated ending.
#[derive(Default)]
struct Ending {
    /// Where the tracks before the exact slot end, from the exact time,
    /// and whether in its window; `None` when it was not reached.
    end: Option<(i64, bool)>,
    /// They end after the cut: the last one is faded out.
    faded: bool,
    filling_ms: u32,
    relaxed_steps: u32,
    /// Ages of the tracks' last plays, added up.
    age_ms: i64,
}

impl Ending {
    /// Lower is better: nothing faded out, in the window, the closest to
    /// the exact time, the least filling, the fewest rules relaxed, the
    /// tracks played longest ago.
    fn rank(&self) -> (bool, bool, bool, i64, u32, u32, Reverse<i64>) {
        (
            self.end.is_none(),
            self.faded,
            !self.end.is_some_and(|(_, in_window)| in_window),
            self.end.map_or(i64::MAX, |(offset, _)| offset.abs()),
            self.filling_ms,
            self.relaxed_steps,
            Reverse(self.age_ms),
        )
    }
}

/// The first pick of an ending simulated from `at_ms` up to the exact time
/// of `window`, the tracks its play ratio held back, and the ending.
fn simulate(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    window: Window,
    rng: &mut Rng,
) -> (Option<Pick>, Vec<HeldByRatio>, Ending) {
    let mut history = history.clone();
    let mut last = last.copied();
    let mut at = at_ms;
    let mut first = None;
    let mut first_held = Vec::new();
    let mut ending = Ending::default();
    'picks: for n in 0..MAX_PICKS {
        // The counters stay as they are: a track held back stays held.
        let mut held = Vec::new();
        let pick = select::pick(library, &history, at, last.as_ref(), &mut held, rng);
        if n == 0 {
            first_held = held;
            first.clone_from(&pick);
        }
        let Some(pick) = pick else {
            break;
        };
        for pick in pick.into_picks() {
            if pick
                .explanation
                .exact
                .is_some_and(|exact| exact.at_unix_ms == window.at_ms)
            {
                let in_window = (window.from_ms..=window.cut_ms).contains(&at);
                ending.end = Some((at - window.at_ms, in_window));
                ending.faded = at > window.cut_ms;
                break 'picks;
            }
            let Some(track) = library.tracks.get(&pick.track) else {
                break 'picks;
            };
            let start = pick.explanation.at_unix_ms;
            ending.filling_ms += pick.explanation.fill.as_ref().map_or(0, |f| f.length_ms);
            ending.relaxed_steps += u32::from(pick.explanation.step);
            ending.age_ms += history
                .last_played(track.id)
                .map_or(MAX_AGE_MS, |played| (start - played).min(MAX_AGE_MS));
            history.push(Play::of(track, start));
            at = start + i64::from(pick.next_ms(library));
            if pick.position.is_some() {
                last = pick.position;
            }
        }
    }
    (first, first_held, ending)
}

/// The pick at `at_ms` before the exact time of `window`: [`DRAWS`]
/// endings simulated, each with its own seed drawn from `rng`, and the
/// first pick of the best one, with what its play ratio held back.
pub(crate) fn best(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    window: Window,
    held_by_ratio: &mut Vec<HeldByRatio>,
    rng: &mut Rng,
) -> Option<Pick> {
    let mut best: Option<(Option<Pick>, Vec<HeldByRatio>, Ending)> = None;
    for _ in 0..DRAWS {
        let mut draw = Rng::new(rng.next_u64());
        let simulated = simulate(library, history, at_ms, last, window, &mut draw);
        let better = best.as_ref().is_none_or(|(pick, _, ending)| {
            // An ending with a first pick wins over one without.
            (simulated.0.is_none(), simulated.2.rank()) < (pick.is_none(), ending.rank())
        });
        if better {
            best = Some(simulated);
        }
    }
    let (pick, held, ending) = best?;
    *held_by_ratio = held;
    pick.map(|mut pick| {
        pick.explanation.lookahead = Some(LookaheadLog {
            exact_at_unix_ms: window.at_ms,
            draws: DRAWS,
            faded: ending.faded,
            in_window: ending.end.is_some_and(|(_, in_window)| in_window),
            offset_ms: ending.end.map(|(offset, _)| offset),
            filling_ms: ending.filling_ms,
            relaxed_steps: ending.relaxed_steps,
        });
        pick
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use jiff::tz::TimeZone;

    use super::*;
    use crate::sim::simulate_with;
    use crate::{
        Category, CategoryId, Clock, ExactMode, ExactTime, LibraryParts, Member, Method,
        RotationSettings, Schedule, SeparationOverrides, Simulation, Slot, Track, TrackId,
    };

    const SEC: i64 = 1_000;
    const MIN: i64 = 60 * SEC;
    const HOUR: i64 = 60 * MIN;
    const SONGS: CategoryId = 1;
    const NEWS: CategoryId = 2;
    const FILLER: CategoryId = 3;
    const JINGLES: CategoryId = 4;

    fn track(id: TrackId, length_ms: u32, mix_ms: Option<u32>) -> Track {
        Track {
            id,
            title_key: Some(format!("title {id}")),
            album_key: Some(format!("album {id}")),
            artists: vec![1_000 + id],
            categories: vec![],
            length_ms,
            mix_ms,
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

    fn slot(category: CategoryId, exact: Option<ExactTime>) -> Slot {
        Slot {
            category,
            method: Method::LeastRecentlyPlayed,
            enforce_rules: true,
            separation: SeparationOverrides::default(),
            linked_to_next: false,
            exact,
        }
    }

    /// A test station: 400 songs of 2:30 to 5:30 with their mix points 4 to
    /// 9 s before their end, a news flash at the top of each hour (soft,
    /// −10 to +30 s, or hard), songs and a jingle every three songs, and
    /// filler jingles of 3 to 8 s.
    fn station(mode: ExactMode) -> Library {
        let mut rng = Rng::new(42);
        let mut tracks: Vec<Track> = (1..=400)
            .map(|id| {
                let length = 150_000 + u32::try_from(rng.below(180_000)).unwrap();
                let mix = length - 4_000 - u32::try_from(rng.below(5_000)).unwrap();
                track(id, length, Some(mix))
            })
            .collect();
        tracks.push(track(500, 120_000, None));
        tracks.extend((600..612).map(|id| track(id, 3_000 + (id as u32 - 600) * 450, None)));
        tracks.extend((700..710).map(|id| track(id, 6_000, None)));
        let songs: Vec<TrackId> = (1..=400).collect();
        let fillers: Vec<TrackId> = (600..612).collect();
        let jingles: Vec<TrackId> = (700..710).collect();
        let news = ExactTime {
            at_s: 0,
            mode,
            before_s: 10,
            after_s: 30,
            medley_fill: false,
            cut_live: false,
        };
        let mut slots = vec![slot(NEWS, Some(news))];
        for index in 0..18 {
            slots.push(slot(if index % 3 == 2 { JINGLES } else { SONGS }, None));
        }
        let mut news_category = category(NEWS, &[500]);
        // The one news flash plays every hour.
        news_category.separation = SeparationOverrides {
            track_min: Some(30),
            title_min: Some(30),
            artist_min: Some(30),
            album_min: Some(30),
            ..SeparationOverrides::default()
        };
        // Jingles and fillers repeat often.
        let short = SeparationOverrides {
            track_min: Some(10),
            title_min: Some(10),
            artist_min: Some(0),
            album_min: Some(0),
            ..SeparationOverrides::default()
        };
        let mut filler_category = category(FILLER, &fillers);
        filler_category.separation = short;
        let mut jingle_category = category(JINGLES, &jingles);
        jingle_category.separation = short;
        Library::new(
            LibraryParts {
                tracks,
                categories: vec![
                    category(SONGS, &songs),
                    news_category,
                    filler_category,
                    jingle_category,
                ],
                clocks: vec![Clock {
                    id: 1,
                    name: "Hour".into(),
                    looping: false,
                    slots,
                }],
                schedule: Schedule {
                    hours: HashMap::from_iter(
                        (1..=7).flat_map(|day| (0..24).map(move |hour| ((day, hour), 1))),
                    ),
                    ..Schedule::default()
                },
                settings: RotationSettings {
                    filler_category_id: Some(FILLER),
                    ..RotationSettings::default()
                },
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        )
    }

    /// Time filled before the exact times, and the exact times met in
    /// their window.
    fn measure(simulation: &Simulation) -> (i64, usize, usize) {
        let filled = simulation
            .picks
            .iter()
            .filter_map(|pick| pick.explanation.fill.as_ref())
            .map(|fill| i64::from(fill.length_ms))
            .sum();
        let exact: Vec<_> = simulation
            .picks
            .iter()
            .filter_map(|pick| pick.explanation.exact)
            .collect();
        let met = exact.iter().filter(|exact| exact.in_window).count();
        (filled, met, exact.len())
    }

    fn days(days: i64, lookahead: bool) -> (i64, usize, usize) {
        let library = station(ExactMode::Soft);
        let simulation = simulate_with(
            &library,
            History::default(),
            None,
            0,
            days * 24 * HOUR,
            7,
            lookahead,
        );
        assert!(
            simulation.violations.is_empty(),
            "{:?}",
            simulation.violations
        );
        measure(&simulation)
    }

    fn fills_less_and_meets_as_many_windows(days_simulated: i64) {
        let (greedy_filled, greedy_met, greedy_exact) = days(days_simulated, false);
        let (filled, met, exact) = days(days_simulated, true);
        eprintln!(
            "soft, {days_simulated} days: without lookahead {greedy_filled} ms filled, \
             {greedy_met}/{greedy_exact} windows met; \
             with it {filled} ms filled, {met}/{exact} windows met"
        );
        assert!(filled < greedy_filled);
        assert!(met >= greedy_met);
    }

    #[test]
    fn looking_ahead_fills_less_and_meets_as_many_windows() {
        fills_less_and_meets_as_many_windows(1);
    }

    /// The measure over a week: slow unoptimized, run it with
    /// `cargo test --release -p heron-rotation -- --ignored --nocapture`.
    #[test]
    #[ignore = "a week of simulation, run in release"]
    fn looking_ahead_fills_less_and_meets_as_many_windows_over_a_week() {
        fills_less_and_meets_as_many_windows(7);
    }

    /// In hard mode, how much of the tracks before the exact times is cut
    /// over `days`, without lookahead and with it.
    fn hard_cuts(days: i64) -> (i64, i64) {
        let cut = |lookahead: bool| {
            let library = station(ExactMode::Hard);
            let simulation = simulate_with(
                &library,
                History::default(),
                None,
                0,
                days * 24 * HOUR,
                7,
                lookahead,
            );
            simulation
                .picks
                .windows(2)
                .filter(|pair| pair[1].explanation.exact.is_some_and(|exact| exact.faded))
                .map(|pair| {
                    let planned = library
                        .tracks
                        .get(&pair[0].track_id)
                        .map_or(0, |track| i64::from(track.next_ms()));
                    pair[0].start_unix_ms + planned - pair[1].start_unix_ms
                })
                .sum::<i64>()
        };
        let (greedy, ahead) = (cut(false), cut(true));
        eprintln!("hard, {days} days: cut without lookahead {greedy} ms, with it {ahead} ms");
        (greedy, ahead)
    }

    #[test]
    fn looking_ahead_cuts_less_at_hard_exact_times() {
        let (greedy, ahead) = hard_cuts(1);
        assert!(ahead < greedy);
    }

    #[test]
    #[ignore = "a week of simulation, run in release"]
    fn looking_ahead_cuts_less_at_hard_exact_times_over_a_week() {
        let (greedy, ahead) = hard_cuts(7);
        assert!(ahead < greedy);
    }

    #[test]
    fn picks_look_ahead_within_a_quarter_of_an_hour_or_four_slots() {
        let library = station(ExactMode::Soft);
        let simulation = simulate_with(&library, History::default(), None, 0, 2 * HOUR, 3, true);
        for pick in &simulation.picks {
            let to_exact = HOUR - pick.start_unix_ms.rem_euclid(HOUR);
            let looked = pick.explanation.lookahead;
            if pick.explanation.exact.is_some() || pick.explanation.slot.is_none() {
                continue;
            }
            if to_exact <= REACH_MS {
                let looked = looked.expect("a pick near the exact time looks ahead");
                assert_eq!(looked.draws, DRAWS);
                assert_eq!(looked.exact_at_unix_ms % HOUR, 0);
            } else if to_exact > 30 * MIN {
                // 18 slots in the hour: more than 4 left this early.
                assert!(looked.is_none(), "{}", pick.start_unix_ms);
            }
        }
    }

    #[test]
    fn an_ending_that_fades_nothing_wins_even_with_some_filling() {
        // A filler jingle of 4 s to land 25 s after the exact time...
        let filled = Ending {
            end: Some((25 * SEC, true)),
            faded: false,
            filling_ms: 4_000,
            relaxed_steps: 0,
            age_ms: 0,
        };
        // ...beats a song crossing the end of the window, faded out there.
        let faded = Ending {
            end: Some((35 * SEC, false)),
            faded: true,
            filling_ms: 0,
            relaxed_steps: 0,
            age_ms: MAX_AGE_MS,
        };
        assert!(filled.rank() < faded.rank());
        // Without a fade, the closest to the exact time wins, then the
        // least filling.
        let close = Ending {
            end: Some((-2 * SEC, true)),
            filling_ms: 4_000,
            ..Ending::default()
        };
        let far = Ending {
            end: Some((-8 * SEC, true)),
            ..Ending::default()
        };
        assert!(close.rank() < far.rank());
        let unfilled = Ending {
            end: Some((2 * SEC, true)),
            ..Ending::default()
        };
        assert!(unfilled.rank() < close.rank());
        // An ending not reached loses to any other.
        assert!(faded.rank() < Ending::default().rank());
    }

    #[test]
    fn looking_ahead_replays_with_the_seed() {
        let library = station(ExactMode::Soft);
        let run = || simulate_with(&library, History::default(), None, 0, 3 * HOUR, 5, true);
        assert_eq!(run(), run());
    }
}
