// SPDX-License-Identifier: AGPL-3.0-or-later

//! The best of 10 fills before an exact time: complete sequences of picks
//! up to it with the real rules, their ends searched for the best fit, and
//! the best one kept from a pick to the next (`docs/rotation.md#exact-times`).

use std::cell::RefCell;
use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

use crate::exact::{Choice, Window};
use crate::ratio::HeldByRatio;
use crate::select::{self, Pick, Steer};
use crate::{History, Library, Play, Position, Rng, TrackId};

/// Fills built for a pick.
pub const DRAWS: u32 = 10;
/// A second late counts as this many seconds early when fills are
/// compared: ending before the exact time is better than after it, which
/// brings the track before closer to the cut and its fade.
const LATE_WEIGHT: i64 = 3;
/// The most picks a fill makes before the exact slot.
pub const MAX_PICKS: usize = 40;
/// A track's last play counts this long ago at most (never played ones
/// too), for the tie between fills.
const MAX_AGE_MS: i64 = 30 * 24 * 3_600_000;
/// The fit search tries other tracks for this many slots at the end of a
/// fill...
pub const SEARCH_SLOTS: usize = 3;
/// ...the first this many of each in its own order, every one of the last...
pub const SEARCH_WIDTH: usize = 12;
/// ...and plays again this many of the best combinations.
pub const SEARCH_TRIES: usize = 4;
/// A fill ending this close to the exact time, in its window, with nothing
/// filled, is not searched.
const CLOSE_ENOUGH_MS: i64 = 1_000;

/// How a pick's fill was chosen before an exact time, in the rotation log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LookaheadLog {
    /// The exact time ahead (Unix ms).
    pub exact_at_unix_ms: i64,
    /// Fills built for this pick: 0 when it follows the fill kept.
    pub draws: u32,
    /// The pick follows the best fill of an earlier pick, played again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kept: bool,
    /// The fit search found the best fill's ending.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub search: bool,
    /// The best fill fades out the track before the exact slot: it ends
    /// after the cut (the end of the window, the exact time in hard mode).
    /// Fading nothing comes first: every fill did.
    pub faded: bool,
    /// The best fill: the tracks before the exact slot end inside its
    /// window (the 10 s before the exact time in hard mode).
    pub in_window: bool,
    /// Where they end, from the exact time; negative when early. Missing
    /// when no fill reached the exact slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<i64>,
    /// Time filled before the exact slot (medley or filler track).
    pub filling_ms: u32,
    /// Relaxation steps of its picks, added up.
    pub relaxed_steps: u32,
}

/// How a fill ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    /// the exact time (a second late weighing [`LATE_WEIGHT`] seconds
    /// early), the least filling, the fewest rules relaxed, the tracks
    /// played longest ago.
    fn rank(&self) -> (bool, bool, bool, i64, u32, u32, Reverse<i64>) {
        (
            self.end.is_none(),
            self.faded,
            !self.in_window(),
            self.end.map_or(i64::MAX, |(offset, _)| weighed(offset)),
            self.filling_ms,
            self.relaxed_steps,
            Reverse(self.age_ms),
        )
    }

    fn in_window(&self) -> bool {
        self.end.is_some_and(|(_, in_window)| in_window)
    }

    /// Nothing faded, in the window, with no more filling than `than`.
    fn holds(&self, than: &Ending) -> bool {
        !self.faded && self.in_window() && self.filling_ms <= than.filling_ms
    }

    /// Not worth searching: in the window, close to the exact time, with
    /// nothing filled.
    fn close_enough(&self) -> bool {
        !self.faded
            && self.filling_ms == 0
            && self
                .end
                .is_some_and(|(offset, in_window)| in_window && offset.abs() <= CLOSE_ENOUGH_MS)
    }

    fn add(&mut self, pick: &FillPick) {
        self.filling_ms += pick.filling_ms;
        self.relaxed_steps += pick.relaxed_steps;
        self.age_ms += pick.age_ms;
    }
}

/// An offset from the exact time, a second late weighing [`LATE_WEIGHT`]
/// seconds early.
fn weighed(offset: i64) -> i64 {
    if offset > 0 {
        offset.saturating_mul(LATE_WEIGHT)
    } else {
        -offset
    }
}

/// The best fill of an earlier pick, kept for the next ones: the tracks of
/// its picks still to queue (a linked slot's two together), and how it
/// ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    exact_at_ms: i64,
    picks: Vec<Vec<TrackId>>,
    ending: Ending,
    search: bool,
}

/// One pick of a fill, and the state after it.
#[derive(Clone, Debug)]
struct FillPick {
    tracks: Vec<TrackId>,
    /// Its planned start, and where the next pick starts.
    start: i64,
    next_at: i64,
    /// The plays it adds, and the clock's position after it.
    plays: Vec<(TrackId, i64)>,
    last: Option<Position>,
    /// A plain slot pick: one track, not a gap fill.
    plain: bool,
    /// The tracks its slot could play, for the fit search.
    options: Option<Vec<Choice>>,
    filling_ms: u32,
    relaxed_steps: u32,
    age_ms: i64,
}

/// A complete fill up to the exact slot.
struct Fill {
    /// Its first pick as made, and what its play ratio held back.
    first: Option<Pick>,
    first_held: Vec<HeldByRatio>,
    picks: Vec<FillPick>,
    ending: Ending,
    search: bool,
}

/// Where a fill starts from.
struct From<'a> {
    library: &'a Library,
    history: &'a History,
    at_ms: i64,
    last: Option<Position>,
    window: Window,
}

impl From<'_> {
    /// The state after the first `count` picks of `picks`.
    fn after(&self, picks: &[FillPick]) -> (History, i64, Option<Position>) {
        let mut history = self.history.clone();
        let mut at = self.at_ms;
        let mut last = self.last;
        for pick in picks {
            for (track, start) in &pick.plays {
                if let Some(track) = self.library.tracks.get(track) {
                    history.push(Play::of(track, *start));
                }
            }
            at = pick.next_at;
            if pick.last.is_some() {
                last = pick.last;
            }
        }
        (history, at, last)
    }

    /// A fill from `history` at `at_ms`, the picks wanting the tracks of
    /// `want` in turn; `record`: the slots record the tracks they may play.
    fn simulate(
        &self,
        history: &History,
        at_ms: i64,
        last: Option<Position>,
        want: &[Vec<TrackId>],
        record: bool,
        rng: &mut Rng,
    ) -> Fill {
        let library = self.library;
        let window = self.window;
        let mut history = history.clone();
        let mut last = last;
        let mut at = at_ms;
        let mut fill = Fill {
            first: None,
            first_held: Vec::new(),
            picks: Vec::new(),
            ending: Ending::default(),
            search: false,
        };
        'picks: for n in 0..MAX_PICKS {
            let options = RefCell::new(None);
            let steer = Steer {
                want: want.get(n).map_or(&[], Vec::as_slice),
                options: record.then_some(&options),
            };
            // The counters stay as they are: a track held back stays held.
            let mut held = Vec::new();
            let pick = select::pick(library, &history, at, last.as_ref(), &mut held, rng, steer);
            if n == 0 {
                fill.first_held = held;
                fill.first.clone_from(&pick);
            }
            let Some(pick) = pick else {
                break;
            };
            let plain = pick.linked.is_none()
                && pick.position.is_some()
                && pick.explanation.fill.is_none()
                && pick.explanation.exact.is_none();
            let mut done = FillPick {
                tracks: Vec::new(),
                start: pick.explanation.at_unix_ms,
                next_at: at,
                plays: Vec::new(),
                last: None,
                plain,
                options: options.into_inner(),
                filling_ms: 0,
                relaxed_steps: 0,
                age_ms: 0,
            };
            let mut reached = false;
            for pick in pick.into_picks() {
                if pick
                    .explanation
                    .exact
                    .is_some_and(|exact| exact.at_unix_ms == window.at_ms)
                {
                    let in_window = (window.from_ms..=window.cut_ms).contains(&at);
                    fill.ending.end = Some((at - window.at_ms, in_window));
                    fill.ending.faded = at > window.cut_ms;
                    reached = true;
                    break;
                }
                let Some(track) = library.tracks.get(&pick.track) else {
                    break 'picks;
                };
                let start = pick.explanation.at_unix_ms;
                done.filling_ms += pick.explanation.fill.as_ref().map_or(0, |f| f.length_ms);
                done.relaxed_steps += u32::from(pick.explanation.step);
                done.age_ms += history
                    .last_played(track.id)
                    .map_or(MAX_AGE_MS, |played| (start - played).min(MAX_AGE_MS));
                history.push(Play::of(track, start));
                done.tracks.push(track.id);
                done.plays.push((track.id, start));
                at = start + i64::from(pick.next_ms(library));
                if pick.position.is_some() {
                    last = pick.position;
                    done.last = pick.position;
                }
            }
            done.next_at = at;
            if !done.tracks.is_empty() {
                fill.ending.add(&done);
                fill.picks.push(done);
            }
            if reached {
                break;
            }
        }
        fill
    }

    /// The fit search at the end of `fill`: other tracks for its last
    /// [`SEARCH_SLOTS`] slot picks, whose lengths land best, played again
    /// with the real rules. The best of them when better than `fill`.
    fn search(&self, fill: &Fill, rng: &mut Rng) -> Option<Fill> {
        if fill.ending.end.is_none() || fill.ending.close_enough() {
            return None;
        }
        let slots: Vec<usize> = fill
            .picks
            .iter()
            .enumerate()
            .filter(|(_, pick)| pick.plain && pick.options.as_ref().is_some_and(|o| !o.is_empty()))
            .map(|(index, _)| index)
            .rev()
            .take(SEARCH_SLOTS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let &from = slots.first()?;
        let tries = combinations(self.library, self.window, &fill.picks[from..], &slots, from);
        let (history, at, last) = self.after(&fill.picks[..from]);
        let mut prefix = Ending::default();
        for pick in &fill.picks[..from] {
            prefix.add(pick);
        }
        let mut best: Option<Fill> = None;
        for want in tries {
            let mut draw = Rng::new(rng.next_u64());
            let mut tried = self.simulate(&history, at, last, &want, false, &mut draw);
            tried.ending.filling_ms += prefix.filling_ms;
            tried.ending.relaxed_steps += prefix.relaxed_steps;
            tried.ending.age_ms += prefix.age_ms;
            let better = best
                .as_ref()
                .map_or(fill.ending.rank(), |best| best.ending.rank());
            if tried.ending.rank() < better {
                best = Some(tried);
            }
        }
        let mut best = best?;
        let mut picks = fill.picks[..from].to_vec();
        picks.append(&mut best.picks);
        best.picks = picks;
        if from > 0 {
            best.first.clone_from(&fill.first);
            best.first_held.clone_from(&fill.first_held);
        }
        best.search = true;
        Some(best)
    }
}

/// The combinations the fit search plays again, best first: the tracks of
/// `picks` (from `from` in the fill), the slot picks at `slots` taking
/// other tracks among those they could play, whose lengths land best.
fn combinations(
    library: &Library,
    window: Window,
    picks: &[FillPick],
    slots: &[usize],
    from: usize,
) -> Vec<Vec<Vec<TrackId>>> {
    let Some(start) = picks.first().map(|pick| pick.start) else {
        return Vec::new();
    };
    let options: Vec<&[Choice]> = slots
        .iter()
        .map(|&slot| picks[slot - from].options.as_deref().unwrap_or(&[]))
        .collect();
    let widths: Vec<usize> = options
        .iter()
        .enumerate()
        .map(|(level, options)| {
            if level + 1 == slots.len() {
                options.len()
            } else {
                options.len().min(SEARCH_WIDTH)
            }
        })
        .collect();
    // (class, cost, order) for each combination, lower is better: landing
    // in the window, then ending before it (a gap to fill), each the
    // closest; then the slots' own order.
    let mut best: Vec<((u8, i64, usize), Vec<usize>)> = Vec::new();
    let mut chosen = vec![0usize; slots.len()];
    loop {
        if let Some(key) = landing(
            library, window, picks, slots, from, start, &options, &chosen,
        ) {
            let played = key.1;
            let key = key.0;
            let worse = best.len() >= SEARCH_TRIES && best.last().is_some_and(|(k, _)| key >= *k);
            let repeated = best
                .iter()
                .any(|(_, other)| other[..played] == chosen[..played]);
            if !worse && !repeated {
                let mut kept = chosen.clone();
                for later in kept.iter_mut().skip(played) {
                    *later = 0;
                }
                let at = best.partition_point(|(k, _)| *k <= key);
                best.insert(at, (key, kept));
                best.truncate(SEARCH_TRIES);
            }
        }
        // The next combination, the last slot turning fastest.
        let mut level = slots.len();
        loop {
            if level == 0 {
                return best
                    .into_iter()
                    .map(|(_, chosen)| want(picks, slots, from, &options, &chosen))
                    .collect();
            }
            level -= 1;
            chosen[level] += 1;
            if chosen[level] < widths[level] {
                break;
            }
            chosen[level] = 0;
        }
    }
}

/// How a combination lands, and how many of its slots play before the
/// exact time is due; `None` when a track would end after the cut or two of
/// its tracks would clash.
#[allow(clippy::too_many_arguments)]
fn landing(
    library: &Library,
    window: Window,
    picks: &[FillPick],
    slots: &[usize],
    from: usize,
    start: i64,
    options: &[&[Choice]],
    chosen: &[usize],
) -> Option<((u8, i64, usize), usize)> {
    let mut at = start;
    let mut played = 0;
    let mut tracks: Vec<TrackId> = Vec::new();
    for (index, pick) in picks.iter().enumerate() {
        if at >= window.from_ms {
            break;
        }
        match slots.iter().position(|&slot| slot == from + index) {
            Some(level) => {
                let choice = options[level].get(chosen[level])?;
                let track = library.tracks.get(&choice.track)?;
                let clashes = tracks.iter().any(|other| {
                    library.tracks.get(other).is_some_and(|other| {
                        other.id == track.id
                            || other.title_key.is_some() && other.title_key == track.title_key
                            || other.artists.iter().any(|a| track.artists.contains(a))
                    })
                });
                if clashes {
                    return None;
                }
                tracks.push(track.id);
                at += i64::from(choice.next_ms);
                played = level + 1;
            }
            // A gap fill is decided again when played; others keep their
            // tracks and lengths.
            None if !pick.plain && pick.filling_ms > 0 => {}
            None => at += pick.next_at - pick.start,
        }
        if at > window.cut_ms {
            return None;
        }
    }
    let order = chosen[..played].iter().sum();
    let key = if at >= window.from_ms {
        (0, weighed(at - window.at_ms), order)
    } else {
        (1, window.from_ms - at, order)
    };
    Some((key, played))
}

/// The tracks a combination wants, pick by pick from `from`.
fn want(
    picks: &[FillPick],
    slots: &[usize],
    from: usize,
    options: &[&[Choice]],
    chosen: &[usize],
) -> Vec<Vec<TrackId>> {
    picks
        .iter()
        .enumerate()
        .map(
            |(index, pick)| match slots.iter().position(|&slot| slot == from + index) {
                Some(level) => options[level]
                    .get(chosen[level])
                    .map(|choice| vec![choice.track])
                    .unwrap_or_default(),
                None if !pick.plain && pick.filling_ms > 0 => Vec::new(),
                None => pick.tracks.clone(),
            },
        )
        .collect()
}

/// The pick at `at_ms` before the exact time of `window`: the fill kept in
/// `plan` played again and followed while it still lands, else the best of
/// it and [`DRAWS`] new fills, each with its own seed drawn from `rng`, their
/// ends searched for the best fit. Returns the first pick, with what its
/// play ratio held back, and keeps the rest of the fill in `plan`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn best(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    window: Window,
    held_by_ratio: &mut Vec<HeldByRatio>,
    rng: &mut Rng,
    plan: &mut Option<Plan>,
) -> Option<Pick> {
    let start = From {
        library,
        history,
        at_ms,
        last: last.copied(),
        window,
    };
    let kept = plan
        .take()
        .filter(|plan| plan.exact_at_ms == window.at_ms)
        .map(|plan| {
            let mut draw = Rng::new(rng.next_u64());
            let fill = start.simulate(history, at_ms, last.copied(), &plan.picks, true, &mut draw);
            (fill, plan)
        });
    let mut draws = 0;
    let mut best = match kept {
        Some((mut fill, plan)) if fill.first.is_some() && fill.ending.holds(&plan.ending) => {
            fill.search = plan.search;
            fill
        }
        kept => {
            draws = DRAWS;
            // The fill kept first: it wins a tie.
            let mut best: Option<Fill> = None;
            let fills = kept
                .map(|(fill, _)| fill)
                .into_iter()
                .map(Some)
                .chain((0..DRAWS).map(|_| None));
            for fill in fills {
                let mut draw = Rng::new(rng.next_u64());
                let mut fill = fill.unwrap_or_else(|| {
                    start.simulate(history, at_ms, last.copied(), &[], true, &mut draw)
                });
                if let Some(found) = start.search(&fill, &mut draw)
                    && found.first.is_some()
                {
                    fill = found;
                }
                let better = best.as_ref().is_none_or(|best| {
                    // A fill with a first pick wins over one without.
                    (fill.first.is_none(), fill.ending.rank())
                        < (best.first.is_none(), best.ending.rank())
                });
                if better {
                    best = Some(fill);
                }
            }
            best?
        }
    };
    *held_by_ratio = std::mem::take(&mut best.first_held);
    let ending = best.ending;
    if ending.end.is_some() {
        *plan = Some(Plan {
            exact_at_ms: window.at_ms,
            picks: best
                .picks
                .iter()
                .skip(1)
                .map(|pick| pick.tracks.clone())
                .collect(),
            ending,
            search: best.search,
        });
    }
    best.first.map(|mut pick| {
        pick.explanation.lookahead = Some(LookaheadLog {
            exact_at_unix_ms: window.at_ms,
            draws,
            kept: draws == 0,
            search: best.search,
            faded: ending.faded,
            in_window: ending.in_window(),
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
        RotationSettings, Schedule, SeparationOverrides, Slot, Track, TrackId,
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
        station_of(mode, 400)
    }

    const NEWS_FLASH: TrackId = 10_000;

    fn station_of(mode: ExactMode, songs: u64) -> Library {
        let mut rng = Rng::new(42);
        let mut tracks: Vec<Track> = (1..=songs)
            .map(|id| {
                let length = 150_000 + u32::try_from(rng.below(180_000)).unwrap();
                let mix = length - 4_000 - u32::try_from(rng.below(5_000)).unwrap();
                track(id, length, Some(mix))
            })
            .collect();
        tracks.push(track(NEWS_FLASH, 120_000, None));
        tracks
            .extend((10_100..10_112).map(|id| track(id, 3_000 + (id as u32 - 10_100) * 450, None)));
        tracks.extend((10_200..10_210).map(|id| track(id, 6_000, None)));
        let songs: Vec<TrackId> = (1..=songs).collect();
        let fillers: Vec<TrackId> = (10_100..10_112).collect();
        let jingles: Vec<TrackId> = (10_200..10_210).collect();
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
        let mut news_category = category(NEWS, &[NEWS_FLASH]);
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

    /// How the exact times were met over a simulation.
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Timing {
        exact: usize,
        /// Started in their window.
        met: usize,
        /// Started after the exact time, on average and at worst (ms).
        late: usize,
        mean_late_ms: i64,
        max_late_ms: i64,
        /// The track before faded out, and the time cut from it (ms).
        faded: usize,
        cut_ms: i64,
        /// Time filled (medley or filler, ms).
        filled_ms: i64,
        /// Time a pick took on average and at worst (ms), and the picks.
        mean_pick_ms: f64,
        max_pick_ms: f64,
        picks: usize,
    }

    /// A simulation of `days` from midnight, pick by pick as
    /// [`simulate_with`] does, timing each pick.
    fn timing(library: &Library, days: i64, seed: u64, fills: bool) -> Timing {
        let start = std::time::Instant::now();
        let simulation = simulate_with(
            library,
            History::default(),
            None,
            0,
            days * 24 * HOUR,
            seed,
            fills,
        );
        let total_ms = start.elapsed().as_secs_f64() * 1_000.0;
        assert!(
            simulation.violations.is_empty(),
            "{:?}",
            simulation.violations
        );
        let exact: Vec<_> = simulation
            .picks
            .iter()
            .filter_map(|pick| pick.explanation.exact)
            .collect();
        let late: Vec<i64> = exact
            .iter()
            .map(|e| e.offset_ms)
            .filter(|&o| o > 0)
            .collect();
        let cut_ms = simulation
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
            .sum();
        let filled_ms = simulation
            .picks
            .iter()
            .filter_map(|pick| pick.explanation.fill.as_ref())
            .map(|fill| i64::from(fill.length_ms))
            .sum();
        let picks = simulation.picks.len();
        Timing {
            exact: exact.len(),
            met: exact.iter().filter(|e| e.in_window).count(),
            late: late.len(),
            mean_late_ms: late.iter().sum::<i64>() / i64::try_from(late.len().max(1)).unwrap(),
            max_late_ms: late.iter().copied().max().unwrap_or(0),
            faded: exact.iter().filter(|e| e.faded).count(),
            cut_ms,
            filled_ms,
            mean_pick_ms: total_ms / picks.max(1) as f64,
            max_pick_ms: max_pick_ms(library, seed, fills),
            picks,
        }
    }

    /// The longest pick over the hour before an exact time, timed alone.
    fn max_pick_ms(library: &Library, seed: u64, fills: bool) -> f64 {
        let mut history = History::default();
        let mut last = None;
        let mut at = 0;
        let mut plan = None;
        let mut rng = Rng::new(seed);
        let mut longest: f64 = 0.0;
        while at < 3 * HOUR {
            let start = std::time::Instant::now();
            let outcome = crate::select::outcome(
                library,
                &history,
                at,
                last.as_ref(),
                &mut rng,
                fills,
                &mut plan,
            );
            longest = longest.max(start.elapsed().as_secs_f64() * 1_000.0);
            let Some(pick) = outcome.pick else {
                at += MIN;
                continue;
            };
            for pick in pick.into_picks() {
                let track = &library.tracks[&pick.track];
                at = pick.explanation.at_unix_ms;
                history.push(Play::of(track, at));
                at += i64::from(pick.next_ms(library));
                if pick.position.is_some() {
                    last = pick.position;
                }
            }
        }
        longest
    }

    /// Prints how soft and hard exact times are met with and without the
    /// fills, on a station of `songs` songs.
    fn compare(days: i64, seeds: &[u64], songs: u64) -> Vec<(ExactMode, bool, u64, Timing)> {
        let mut rows = Vec::new();
        eprintln!(
            "| songs | days | mode | fills | seed | exact times | in window | faded | time cut | late | mean late | worst late | filled | mean pick | worst pick |"
        );
        for mode in [ExactMode::Soft, ExactMode::Hard] {
            let library = station_of(mode, songs);
            for &seed in seeds {
                for fills in [false, true] {
                    let t = timing(&library, days, seed, fills);
                    eprintln!(
                        "| {songs} | {days} | {mode:?} | {fills} | {seed} | {} | {} | {} | {:.1} s | {} | {:.1} s | {:.1} s | {:.1} s | {:.2} ms | {:.1} ms |",
                        t.exact,
                        t.met,
                        t.faded,
                        t.cut_ms as f64 / 1000.0,
                        t.late,
                        t.mean_late_ms as f64 / 1000.0,
                        t.max_late_ms as f64 / 1000.0,
                        t.filled_ms as f64 / 1000.0,
                        t.mean_pick_ms,
                        t.max_pick_ms,
                    );
                    rows.push((mode, fills, seed, t));
                }
            }
        }
        rows
    }

    #[test]
    fn the_best_of_ten_fills_fades_nothing_and_meets_as_many_windows() {
        let rows = compare(1, &[7], 400);
        for mode in [ExactMode::Soft, ExactMode::Hard] {
            let of = |fills: bool| {
                rows.iter()
                    .find(|(m, f, _, _)| *m == mode && *f == fills)
                    .map(|(_, _, _, t)| *t)
                    .unwrap()
            };
            let (greedy, best) = (of(false), of(true));
            assert_eq!(best.faded, 0, "{mode:?}: {greedy:?} {best:?}");
            // A day's last exact time may fall after the end, started
            // early in its window or not.
            assert!(best.met + 1 >= greedy.met, "{mode:?}: {greedy:?} {best:?}");
            assert!(best.met == best.exact, "{mode:?}: {best:?}");
            assert!(
                best.filled_ms <= greedy.filled_ms,
                "{mode:?}: {greedy:?} {best:?}"
            );
            assert!(best.late <= greedy.late, "{mode:?}: {greedy:?} {best:?}");
        }
    }

    /// Over a week, three seeds, 400 and 2000 songs, in release:
    /// `cargo test --release -p heron-rotation -- --ignored --nocapture`.
    #[test]
    #[ignore = "a week of simulations: run in release"]
    fn exact_times_soft_and_hard_over_a_week() {
        for songs in [400, 2_000] {
            compare(7, &[7, 11, 23], songs);
        }
    }

    #[test]
    fn every_pick_before_an_exact_time_builds_fills_or_follows_the_best() {
        let library = station(ExactMode::Soft);
        let simulation = simulate_with(&library, History::default(), None, 0, 2 * HOUR, 3, true);
        let mut kept = 0;
        for pick in &simulation.picks {
            if pick.explanation.exact.is_some() {
                assert!(pick.explanation.lookahead.is_none());
                continue;
            }
            let looked = pick
                .explanation
                .lookahead
                .expect("a pick before an exact time has its fill");
            assert_eq!(looked.exact_at_unix_ms % HOUR, 0);
            assert!(looked.exact_at_unix_ms > pick.start_unix_ms);
            if looked.kept {
                kept += 1;
                assert_eq!(looked.draws, 0);
            } else {
                assert_eq!(looked.draws, DRAWS);
            }
            assert!(!looked.faded && looked.in_window, "{looked:?}");
        }
        // Most picks follow the fill kept: a new one only when it no
        // longer lands.
        assert!(kept * 2 > simulation.picks.len(), "{kept}");
    }

    /// A clock of three songs before a hard top of the hour, from 49:00:
    /// songs of 2:00 to 2:30, and three that land together: 3:29, 3:20 and
    /// 4:10, ending a second before the exact time.
    fn three_to_land() -> (Library, History) {
        let mut library = station_of(ExactMode::Hard, 0);
        let lengths: Vec<u32> = (0..30).map(|i| 120_500 + i * 1_000).collect();
        let mut tracks: Vec<Track> = lengths
            .iter()
            .enumerate()
            .map(|(i, &length)| track(i as u64 + 1, length, None))
            .collect();
        tracks.push(track(100, 209_000, None));
        tracks.push(track(101, 200_000, None));
        tracks.push(track(102, 250_000, None));
        let ids: Vec<TrackId> = tracks.iter().map(|t| t.id).collect();
        for t in tracks {
            library.tracks.insert(t.id, t);
        }
        let mut songs = category(SONGS, &ids);
        songs.separation.draw_start_pct = Some(100);
        songs.separation.draw_max_pct = Some(100);
        library.categories.insert(SONGS, songs);
        library.settings.filler_category_id = None;
        let clock = library.clocks.get_mut(&1).unwrap();
        clock.slots.truncate(1);
        clock.slots.extend((0..3).map(|_| slot(SONGS, None)));
        // The three that land were played longest ago: the slots' order.
        let history = History::new(
            Vec::new(),
            ids.iter()
                .map(|&id| (id, if id >= 100 { -20 * HOUR } else { -HOUR }))
                .collect(),
        );
        (library, history)
    }

    #[test]
    fn the_fit_search_finds_the_tracks_whose_lengths_land() {
        let (library, history) = three_to_land();
        let last = Position {
            hour_start_unix_ms: 0,
            clock_id: 1,
            slot: 0,
        };
        let at = 49 * MIN;
        let window = library.clocks[&1].slots[0].exact.unwrap().window(HOUR);
        for seed in 0..5 {
            let mut plan = None;
            let pick = best(
                &library,
                &history,
                at,
                Some(&last),
                window,
                &mut Vec::new(),
                &mut Rng::new(seed),
                &mut plan,
            )
            .unwrap();
            let log = pick.explanation.lookahead.unwrap();
            assert!(!log.faded && log.in_window, "{log:?}");
            assert!(log.search, "{log:?}");
            // The three that land, or a fourth song played on once the
            // clock's slots ran out: 2:03, 2:07, 3:29 and 3:20 end at the
            // exact time itself.
            let offset = log.offset_ms.unwrap();
            assert!((-SEC..=0).contains(&offset), "{log:?}");
            assert!(plan.is_some());
        }
    }

    #[test]
    fn a_fill_that_fades_nothing_wins_even_with_some_filling() {
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
        // So does a slot started early, out of its window.
        let early = Ending {
            end: Some((-40 * SEC, false)),
            ..Ending::default()
        };
        assert!(early.rank() < faded.rank());
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
            end: Some((-SEC, true)),
            ..Ending::default()
        };
        assert!(unfilled.rank() < close.rank());
        // Late weighs three times early: 2 s late loses to 4 s early...
        let late = Ending {
            end: Some((2 * SEC, true)),
            ..Ending::default()
        };
        let early = Ending {
            end: Some((-4 * SEC, true)),
            ..Ending::default()
        };
        assert!(early.rank() < late.rank());
        // ...but wins over 8 s early, and 20 s late loses to 25 s early.
        assert!(late.rank() < far.rank());
        let very_late = Ending {
            end: Some((20 * SEC, true)),
            ..Ending::default()
        };
        let quite_early = Ending {
            end: Some((-25 * SEC, true)),
            ..Ending::default()
        };
        assert!(quite_early.rank() < very_late.rank());
        // A fill not reached loses to any other.
        assert!(faded.rank() < Ending::default().rank());
    }

    #[test]
    fn the_fills_replay_with_the_seed() {
        let library = station(ExactMode::Soft);
        let run = || simulate_with(&library, History::default(), None, 0, 3 * HOUR, 5, true);
        assert_eq!(run(), run());
    }
}
