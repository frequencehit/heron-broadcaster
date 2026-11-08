// SPDX-License-Identifier: AGPL-3.0-or-later

//! The fills before an exact time: complete sequences of picks up to it
//! with the real rules, built 10 at a time until the best one lands, their
//! ends searched for the best fit, and the best one kept from a pick to the
//! next; each fill an attempt the attempts page shows
//! (`docs/rotation.md#exact-times`).

use std::cell::RefCell;
use std::cmp::Reverse;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::exact::{Choice, ExactMode, FillKind, Window};
use crate::ratio::HeldByRatio;
use crate::select::{self, Ahead, Pick, Steer};
use crate::{CategoryId, History, Library, Play, Position, Rng, TrackId};

/// Fills built in a batch: more batches follow while the best fill does
/// not land.
pub const DRAWS: u32 = 10;
/// The most fills built for a pick...
pub const MAX_FILLS: u32 = 200;
/// ...and the longest the queue manager builds them for.
pub const MAX_BUILD: Duration = Duration::from_secs(3);
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
/// ...every combination of the tracks they may play, this many at most
/// (beyond, the first tracks of each slot in its order)...
pub const MAX_COMBINATIONS: u64 = 200_000;
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
    /// The fill it follows was chosen by hand on the attempts page.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_hand: bool,
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
    /// The calculation it comes from, on the attempts page: set by the
    /// queue manager once stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calculation_id: Option<u64>,
}

/// How many fills a pick may build before an exact time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// The most fills built.
    pub max_fills: u32,
    /// The longest they are built for, checked after each fill; `None`:
    /// no limit, so that simulations replay the same.
    pub max_duration: Option<Duration>,
}

impl Budget {
    /// Simulations and tests: [`MAX_FILLS`], whatever the time.
    pub const REPLAYABLE: Self = Self {
        max_fills: MAX_FILLS,
        max_duration: None,
    };
    /// The queue manager: [`MAX_FILLS`] or [`MAX_BUILD`], whichever comes
    /// first.
    pub const LIVE: Self = Self {
        max_fills: MAX_FILLS,
        max_duration: Some(MAX_BUILD),
    };
}

impl Default for Budget {
    fn default() -> Self {
        Self::REPLAYABLE
    }
}

/// How a pick plans before an exact time: its [`Budget`], and whether it
/// gives back what it did ([`crate::Outcome::calculation`]) with every fill
/// it built as an [`Attempt`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Planning {
    pub budget: Budget,
    pub record: bool,
}

/// How a track of an attempt was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Chosen {
    /// By its slot's method, in the slot's order.
    Slot,
    /// By the fit search, for its length.
    FitSearch,
    /// As in the fill kept, played again.
    Kept,
    /// A track of the filler category, filling the gap.
    Filler,
    /// A medley filling the gap, shortened or whole.
    Medley,
}

/// A track of an attempt, from the end of the queue to the exact slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AttemptTrack {
    pub track_id: TrackId,
    /// Its pick in the fill, from 0: a linked slot's two tracks share one.
    pub pick: u32,
    /// Its clock slot, from 0; missing for a gap fill or without a clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<CategoryId>,
    /// Its planned start (Unix ms).
    pub start_unix_ms: i64,
    /// From its start to the next track's (its mix point).
    pub next_ms: u32,
    pub chosen: Chosen,
    /// Relaxation step of its pick, 0 (none) to 12.
    pub step: u8,
    /// A medley: hooks left out (0: whole), and the parts left out by
    /// their position in it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hooks_left_out: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts_left_out: Vec<u32>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// How an attempt meets the exact time: the criteria fills are ranked by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AttemptEnding {
    /// Where the tracks before the exact slot end, from the exact time;
    /// negative when early. Missing when the fill did not reach it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<i64>,
    pub in_window: bool,
    /// They end after the cut: the last one is faded out.
    pub faded: bool,
    pub filling_ms: u32,
    pub relaxed_steps: u32,
    /// Ages of the tracks' last plays, added up (30 days at most each).
    pub age_ms: i64,
}

impl AttemptEnding {
    /// Ends in the window, fades nothing out, fills nothing: building
    /// fills stops at one like it.
    pub fn lands(&self) -> bool {
        !self.faded && self.in_window && self.filling_ms == 0
    }
}

/// What the fit search did at the end of an attempt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchStats {
    /// The slots searched (from 0, in their clock), and the tracks each
    /// could play.
    pub slots: Vec<u32>,
    pub options: Vec<u32>,
    /// Too many combinations: only the first tracks of each slot were
    /// tried ([`MAX_COMBINATIONS`]).
    pub capped: bool,
    /// Combinations tried, and of them those landing in the window.
    pub combinations: u64,
    pub landing: u64,
    /// The best combinations played again with the real rules.
    pub played_again: u32,
    /// One of them replaced the attempt's ending.
    pub found: bool,
}

/// A fill built before an exact time, as the attempts page shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Attempt {
    /// From 1, in the order built.
    pub number: u32,
    /// Its batch of [`DRAWS`], from 1; 0 for the fill kept, played again.
    pub batch: u32,
    /// The fill kept by an earlier pick, played again: it competes with
    /// the new ones.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub previous: bool,
    /// Its seed: the same seed builds the same fill.
    #[serde(with = "seed")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub seed: u64,
    /// Its rank among the calculation's attempts, from 1 (the best).
    pub rank: u32,
    pub tracks: Vec<AttemptTrack>,
    pub ending: AttemptEnding,
    /// Missing when its end was not searched (it landed close enough, or
    /// another one did).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchStats>,
}

/// Seeds as decimal strings: JSON numbers lose 64-bit integers.
mod seed {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(seed: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&seed.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Why building fills stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// The pick followed the fill kept: none built.
    Followed,
    /// The best one lands.
    Landed,
    /// [`Budget::max_fills`] built.
    MaxFills,
    /// [`Budget::max_duration`] spent.
    Time,
}

/// What a pick before an exact time did: the fill kept followed, or fills
/// built, each an attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Calculation {
    /// The exact time (Unix ms), its mode, window and slot (from 0).
    pub exact_at_unix_ms: i64,
    pub mode: ExactMode,
    pub window_from_unix_ms: i64,
    pub window_cut_unix_ms: i64,
    pub exact_slot: u32,
    /// Where the fills start: the end of the queue.
    pub from_unix_ms: i64,
    /// New fills built (0 when the fill kept was followed), in batches.
    pub fills: u32,
    pub batches: u32,
    pub stop: Stop,
    /// The attempt kept, by its number; missing when followed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<u32>,
    /// How the fill kept ends.
    pub ending: AttemptEnding,
    /// The fill followed was chosen by hand.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_hand: bool,
    /// A fill chosen by hand no longer held: rotation chooses by itself
    /// again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hand_dropped: bool,
    /// The time it took.
    pub duration_ms: u32,
    /// Every fill built, in the order built (rank them by
    /// [`Attempt::rank`]); empty when followed.
    pub attempts: Vec<Attempt>,
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
    /// played longest ago. A fill in the window fades nothing out, so it
    /// beats every fill out of it.
    fn rank(&self) -> Rank {
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

    fn log(&self) -> AttemptEnding {
        AttemptEnding {
            offset_ms: self.end.map(|(offset, _)| offset),
            in_window: self.in_window(),
            faded: self.faded,
            filling_ms: self.filling_ms,
            relaxed_steps: self.relaxed_steps,
            age_ms: self.age_ms,
        }
    }

    fn of(ending: &AttemptEnding) -> Self {
        Self {
            end: ending.offset_ms.map(|offset| (offset, ending.in_window)),
            faded: ending.faded,
            filling_ms: ending.filling_ms,
            relaxed_steps: ending.relaxed_steps,
            age_ms: ending.age_ms,
        }
    }
}

/// How a fill ranks, lower is better ([`Ending::rank`]).
type Rank = (bool, bool, bool, i64, u32, u32, Reverse<i64>);

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
    chosen_by_hand: bool,
}

impl Plan {
    /// An attempt of `calculation` chosen by hand, as the fill kept: its
    /// picks from `from_pick` on (those before it are queued already).
    /// The next picks follow it while it still lands, then rotation builds
    /// fills again ([`Calculation::hand_dropped`]).
    pub fn chosen(calculation: &Calculation, attempt: &Attempt, from_pick: u32) -> Self {
        let mut picks: Vec<Vec<TrackId>> = Vec::new();
        let mut last = None;
        for track in attempt
            .tracks
            .iter()
            .filter(|track| track.pick >= from_pick)
        {
            if last == Some(track.pick) {
                if let Some(pick) = picks.last_mut() {
                    pick.push(track.track_id);
                }
            } else {
                picks.push(vec![track.track_id]);
                last = Some(track.pick);
            }
        }
        Self {
            exact_at_ms: calculation.exact_at_unix_ms,
            picks,
            ending: Ending::of(&attempt.ending),
            search: attempt.search.as_ref().is_some_and(|search| search.found),
            chosen_by_hand: true,
        }
    }

    /// The exact time it plans for (Unix ms).
    pub fn exact_at_unix_ms(&self) -> i64 {
        self.exact_at_ms
    }

    /// It was chosen by hand on the attempts page.
    pub fn chosen_by_hand(&self) -> bool {
        self.chosen_by_hand
    }
}

/// One pick of a fill, and the state after it.
#[derive(Clone, Debug)]
struct FillPick {
    tracks: Vec<TrackId>,
    /// Its tracks, for the attempts page.
    details: Vec<AttemptTrack>,
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

impl Fill {
    /// Lower is better: a fill with a first pick wins over one without.
    fn rank(&self) -> (bool, Rank) {
        (self.first.is_none(), self.ending.rank())
    }

    fn tracks(&self) -> Vec<AttemptTrack> {
        self.picks
            .iter()
            .enumerate()
            .flat_map(|(index, pick)| {
                let index = u32::try_from(index).unwrap_or(u32::MAX);
                pick.details.iter().cloned().map(move |mut track| {
                    track.pick = index;
                    track
                })
            })
            .collect()
    }
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
    /// `want` in turn (those they take counting as chosen `wanted`);
    /// `record`: the slots record the tracks they may play.
    #[allow(clippy::too_many_arguments)]
    fn simulate(
        &self,
        history: &History,
        at_ms: i64,
        last: Option<Position>,
        want: &[Vec<TrackId>],
        wanted: Chosen,
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
            let wants = want.get(n).map_or(&[][..], Vec::as_slice);
            let steer = Steer {
                want: wants,
                options: record.then_some(&options),
                // Only a fill's first pick may be queued and logged.
                quick: n > 0,
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
                details: Vec::new(),
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
                let gap = pick.explanation.fill.as_ref();
                done.filling_ms += gap.map_or(0, |f| f.length_ms);
                done.relaxed_steps += u32::from(pick.explanation.step);
                done.age_ms += history
                    .last_played(track.id)
                    .map_or(MAX_AGE_MS, |played| (start - played).min(MAX_AGE_MS));
                history.push(Play::of(track, start));
                let next_ms = pick.next_ms(library);
                done.details.push(AttemptTrack {
                    track_id: track.id,
                    pick: 0,
                    slot: pick.explanation.slot.filter(|_| gap.is_none()),
                    category_id: pick.explanation.category_id,
                    start_unix_ms: start,
                    next_ms,
                    chosen: match gap.map(|gap| gap.kind) {
                        Some(FillKind::Medley) => Chosen::Medley,
                        Some(FillKind::Filler) => Chosen::Filler,
                        None if wants.contains(&track.id) => wanted,
                        None => Chosen::Slot,
                    },
                    step: pick.explanation.step,
                    hooks_left_out: gap.map_or(0, |gap| gap.hooks_left_out),
                    parts_left_out: gap
                        .map(|gap| gap.parts_left_out.clone())
                        .unwrap_or_default(),
                });
                done.tracks.push(track.id);
                done.plays.push((track.id, start));
                at = start + i64::from(next_ms);
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
    /// with the real rules. The best of them when better than `fill`, and
    /// what the search did (`None` when there was nothing to search).
    fn search(&self, fill: &Fill, rng: &mut Rng) -> (Option<Fill>, Option<SearchStats>) {
        if fill.ending.end.is_none() || fill.ending.close_enough() {
            return (None, None);
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
        let Some(&from) = slots.first() else {
            return (None, None);
        };
        let (tries, mut stats) =
            combinations(self.library, self.window, &fill.picks[from..], &slots, from);
        stats.slots = slots
            .iter()
            .map(|&slot| {
                fill.picks[slot]
                    .details
                    .first()
                    .and_then(|track| track.slot)
                    .unwrap_or_default()
            })
            .collect();
        stats.played_again = u32::try_from(tries.len()).unwrap_or(u32::MAX);
        let (history, at, last) = self.after(&fill.picks[..from]);
        let mut prefix = Ending::default();
        for pick in &fill.picks[..from] {
            prefix.add(pick);
        }
        let mut best: Option<Fill> = None;
        for (want, searched) in tries {
            let mut draw = Rng::new(rng.next_u64());
            let mut tried =
                self.simulate(&history, at, last, &want, Chosen::Slot, false, &mut draw);
            tried.ending.filling_ms += prefix.filling_ms;
            tried.ending.relaxed_steps += prefix.relaxed_steps;
            tried.ending.age_ms += prefix.age_ms;
            for track in tried.picks.iter_mut().flat_map(|pick| &mut pick.details) {
                if track.chosen == Chosen::Slot && searched.contains(&track.track_id) {
                    track.chosen = Chosen::FitSearch;
                }
            }
            let better = best
                .as_ref()
                .map_or(fill.ending.rank(), |best| best.ending.rank());
            if tried.ending.rank() < better {
                best = Some(tried);
            }
        }
        let Some(mut best) = best else {
            return (None, Some(stats));
        };
        stats.found = true;
        let mut picks = fill.picks[..from].to_vec();
        picks.append(&mut best.picks);
        best.picks = picks;
        if from > 0 {
            best.first.clone_from(&fill.first);
            best.first_held.clone_from(&fill.first_held);
        }
        best.search = true;
        (Some(best), Some(stats))
    }
}

/// A combination the fit search plays again: the tracks each pick wants,
/// and those it chose for the slots searched.
type Try = (Vec<Vec<TrackId>>, Vec<TrackId>);

/// The combinations the fit search plays again, best first: the tracks of
/// `picks` (from `from` in the fill), the slot picks at `slots` taking
/// other tracks among those they could play, whose lengths land best; and
/// what the search did.
fn combinations(
    library: &Library,
    window: Window,
    picks: &[FillPick],
    slots: &[usize],
    from: usize,
) -> (Vec<Try>, SearchStats) {
    let options: Vec<&[Choice]> = slots
        .iter()
        .map(|&slot| picks[slot - from].options.as_deref().unwrap_or(&[]))
        .collect();
    let mut widths: Vec<usize> = options.iter().map(|options| options.len()).collect();
    let mut stats = SearchStats {
        options: widths
            .iter()
            .map(|&width| u32::try_from(width).unwrap_or(u32::MAX))
            .collect(),
        ..SearchStats::default()
    };
    // Beyond MAX_COMBINATIONS, the first tracks of each slot: the widest
    // slots give way first.
    let product = |widths: &[usize]| {
        widths.iter().fold(1_u64, |product, &width| {
            product.saturating_mul(width as u64)
        })
    };
    while product(&widths) > MAX_COMBINATIONS {
        stats.capped = true;
        if let Some(widest) = widths.iter_mut().max() {
            *widest -= 1;
        }
    }
    let Some(start) = picks.first().map(|pick| pick.start) else {
        return (Vec::new(), stats);
    };
    let mut search = Search {
        library,
        window,
        picks,
        levels: (0..picks.len())
            .map(|index| slots.iter().position(|&slot| slot == from + index))
            .collect(),
        options: &options,
        widths: &widths,
        chosen: Vec::with_capacity(slots.len()),
        tracks: Vec::with_capacity(slots.len()),
        best: Vec::new(),
        stats: &mut stats,
    };
    search.walk(0, start);
    let best = std::mem::take(&mut search.best);
    let tries = best
        .into_iter()
        .map(|(_, mut chosen)| {
            let searched = chosen
                .iter()
                .zip(&options)
                .filter_map(|(&index, options)| options.get(index).map(|choice| choice.track))
                .collect();
            chosen.resize(slots.len(), 0);
            (want(picks, slots, from, &options, &chosen), searched)
        })
        .collect();
    (tries, stats)
}

/// The fit search's walk through the combinations, the last slot turning
/// fastest: a combination ends where the exact time is due (the slots
/// after it do not play), or where a track would end after the cut or two
/// of its tracks would clash (those after it are not tried).
struct Search<'a, 's> {
    library: &'a Library,
    window: Window,
    picks: &'a [FillPick],
    /// For each pick, its slot's level in the search.
    levels: Vec<Option<usize>>,
    options: &'a [&'a [Choice]],
    widths: &'a [usize],
    /// The tracks chosen so far, by their index in each slot's options.
    chosen: Vec<usize>,
    tracks: Vec<&'a crate::Track>,
    /// (class, cost, order) for each combination kept, lower is better:
    /// landing in the window, then ending before it (a gap to fill), each
    /// the closest; then the slots' own order.
    best: Vec<((u8, i64, usize), Vec<usize>)>,
    stats: &'s mut SearchStats,
}

impl Search<'_, '_> {
    fn walk(&mut self, index: usize, at: i64) {
        let window = self.window;
        let Some(pick) = self.picks.get(index).filter(|_| at < window.from_ms) else {
            self.end(at);
            return;
        };
        match self.levels[index] {
            Some(level) => {
                for option in 0..self.widths[level] {
                    let Some(choice) = self.options[level].get(option) else {
                        break;
                    };
                    let Some(track) = self.library.tracks.get(&choice.track) else {
                        self.stats.combinations += 1;
                        continue;
                    };
                    let clashes = self.tracks.iter().any(|other| {
                        other.id == track.id
                            || other.title_key.is_some() && other.title_key == track.title_key
                            || other.artists.iter().any(|a| track.artists.contains(a))
                    });
                    let next = at + i64::from(choice.next_ms);
                    if clashes || next > window.cut_ms {
                        self.stats.combinations += 1;
                        continue;
                    }
                    self.chosen.push(option);
                    self.tracks.push(track);
                    self.walk(index + 1, next);
                    self.tracks.pop();
                    self.chosen.pop();
                }
            }
            // A gap fill is decided again when played; others keep their
            // tracks and lengths.
            None if !pick.plain && pick.filling_ms > 0 => self.walk(index + 1, at),
            None => {
                let next = at + (pick.next_at - pick.start);
                if next > window.cut_ms {
                    self.stats.combinations += 1;
                } else {
                    self.walk(index + 1, next);
                }
            }
        }
    }

    /// A combination that ends at `at`.
    fn end(&mut self, at: i64) {
        let window = self.window;
        self.stats.combinations += 1;
        let order = self.chosen.iter().sum();
        let key = if at >= window.from_ms {
            self.stats.landing += 1;
            (0, weighed(at - window.at_ms), order)
        } else {
            (1, window.from_ms - at, order)
        };
        let worse =
            self.best.len() >= SEARCH_TRIES && self.best.last().is_some_and(|(k, _)| key >= *k);
        if worse {
            return;
        }
        let at = self.best.partition_point(|(k, _)| *k <= key);
        self.best.insert(at, (key, self.chosen.clone()));
        self.best.truncate(SEARCH_TRIES);
    }
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

/// The fills built for a pick, as attempts.
struct Attempts {
    record: bool,
    list: Vec<Attempt>,
    /// Each attempt's rank, to rank them once all are built.
    ranks: Vec<(bool, Rank)>,
}

impl Attempts {
    fn push(&mut self, fill: &Fill, batch: u32, seed: u64, search: Option<SearchStats>) -> u32 {
        let number = u32::try_from(self.ranks.len() + 1).unwrap_or(u32::MAX);
        self.ranks.push(fill.rank());
        if self.record {
            self.list.push(Attempt {
                number,
                batch,
                previous: batch == 0,
                seed,
                rank: 0,
                tracks: fill.tracks(),
                ending: fill.ending.log(),
                search,
            });
        }
        number
    }

    /// The attempts ranked, best first in rank and in the order built
    /// otherwise: the first of equal ones wins, as when they were built.
    fn ranked(mut self) -> Vec<Attempt> {
        let mut order: Vec<usize> = (0..self.list.len()).collect();
        order.sort_by_key(|&index| self.ranks[index]);
        for (rank, index) in order.into_iter().enumerate() {
            self.list[index].rank = u32::try_from(rank + 1).unwrap_or(u32::MAX);
        }
        self.list
    }
}

/// The pick at `at_ms` before the exact time `ahead`: the fill kept in
/// `plan` played again and followed while it still lands, else the best of
/// it and new fills, [`DRAWS`] at a time until the best one lands or the
/// `budget` is spent, each with its own seed drawn from `rng`, their ends
/// searched for the best fit. Returns the first pick, with what its play
/// ratio held back, keeps the rest of the fill in `plan`, and with
/// [`Planning::record`] says what it did in `calculation`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn best(
    library: &Library,
    history: &History,
    at_ms: i64,
    last: Option<&Position>,
    ahead: &Ahead,
    held_by_ratio: &mut Vec<HeldByRatio>,
    rng: &mut Rng,
    plan: &mut Option<Plan>,
    planning: Planning,
    calculation: &mut Option<Calculation>,
) -> Option<Pick> {
    let started = Instant::now();
    let window = ahead.window;
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
            let seed = rng.next_u64();
            let mut draw = Rng::new(seed);
            let fill = start.simulate(
                history,
                at_ms,
                last.copied(),
                &plan.picks,
                Chosen::Kept,
                true,
                &mut draw,
            );
            (fill, plan, seed, draw)
        });
    let by_hand = kept
        .as_ref()
        .is_some_and(|(_, plan, _, _)| plan.chosen_by_hand);
    let mut draws = 0;
    let mut batches = 0;
    let mut stop = Stop::Followed;
    let mut kept_number = None;
    let mut attempts = Attempts {
        record: planning.record,
        list: Vec::new(),
        ranks: Vec::new(),
    };
    let mut best = match kept {
        Some((mut fill, plan, _, _)) if fill.first.is_some() && fill.ending.holds(&plan.ending) => {
            fill.search = plan.search;
            fill
        }
        kept => {
            let budget = planning.budget;
            let mut best: Option<Fill> = None;
            // Searches `fill`'s end, records it and keeps it if better.
            let mut consider = |mut fill: Fill, mut draw: Rng, batch: u32, seed: u64| {
                // Once a fill lands within a second with nothing filled,
                // the others are not searched: it saves the time of the
                // search, for less than a second to gain.
                let (found, stats) = if best.as_ref().is_none_or(|best| !best.ending.close_enough())
                {
                    start.search(&fill, &mut draw)
                } else {
                    (None, None)
                };
                if let Some(found) = found
                    && found.first.is_some()
                {
                    fill = found;
                }
                let number = attempts.push(&fill, batch, seed, stats);
                if best.as_ref().is_none_or(|best| fill.rank() < best.rank()) {
                    best = Some(fill);
                    kept_number = Some(number);
                }
                best.as_ref().is_some_and(|best| best.ending.log().lands())
            };
            // The fill kept first: it wins a tie.
            let mut landed = false;
            if let Some((fill, _, seed, draw)) = kept {
                landed = consider(fill, draw, 0, seed);
            }
            'batches: loop {
                if draws >= budget.max_fills {
                    stop = Stop::MaxFills;
                    break;
                }
                batches += 1;
                for _ in 0..DRAWS {
                    if draws >= budget.max_fills {
                        stop = Stop::MaxFills;
                        break 'batches;
                    }
                    let spent = budget
                        .max_duration
                        .is_some_and(|max| started.elapsed() >= max);
                    if draws > 0 && spent {
                        stop = Stop::Time;
                        break 'batches;
                    }
                    let seed = rng.next_u64();
                    let mut draw = Rng::new(seed);
                    let fill = start.simulate(
                        history,
                        at_ms,
                        last.copied(),
                        &[],
                        Chosen::Slot,
                        true,
                        &mut draw,
                    );
                    draws += 1;
                    landed = consider(fill, draw, batches, seed);
                }
                if landed {
                    stop = Stop::Landed;
                    break;
                }
            }
            best?
        }
    };
    *held_by_ratio = std::mem::take(&mut best.first_held);
    let ending = best.ending;
    let followed = stop == Stop::Followed;
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
            chosen_by_hand: followed && by_hand,
        });
    }
    if planning.record {
        *calculation = Some(Calculation {
            exact_at_unix_ms: window.at_ms,
            mode: ahead.mode,
            window_from_unix_ms: window.from_ms,
            window_cut_unix_ms: window.cut_ms,
            exact_slot: ahead.slot,
            from_unix_ms: at_ms,
            fills: draws,
            batches: if followed { 0 } else { batches },
            stop,
            kept: kept_number,
            ending: ending.log(),
            by_hand: followed && by_hand,
            hand_dropped: !followed && by_hand,
            duration_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
            attempts: attempts.ranked(),
        });
    }
    best.first.map(|mut pick| {
        pick.explanation.lookahead = Some(LookaheadLog {
            exact_at_unix_ms: window.at_ms,
            draws,
            kept: followed,
            by_hand: followed && by_hand,
            search: best.search,
            faded: ending.faded,
            in_window: ending.in_window(),
            offset_ms: ending.end.map(|(offset, _)| offset),
            filling_ms: ending.filling_ms,
            relaxed_steps: ending.relaxed_steps,
            calculation_id: None,
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

    /// The time a whole re-plan takes (10 fills built and searched, none
    /// kept) right after each exact time of a day, with the day's plays as
    /// history: on average and at worst (ms).
    fn ten_fills_ms(library: &Library, seed: u64) -> (f64, f64) {
        let day = simulate_with(library, History::default(), None, 0, 24 * HOUR, seed, true);
        let mut times = Vec::new();
        for (index, pick) in day.picks.iter().enumerate() {
            if pick.explanation.exact.is_none() || index + 1 >= day.picks.len() {
                continue;
            }
            let next = &day.picks[index + 1];
            let history = History::new(
                day.picks[..=index]
                    .iter()
                    .map(|pick| Play::of(&library.tracks[&pick.track_id], pick.start_unix_ms))
                    .collect(),
                HashMap::new(),
            );
            let last = Position {
                hour_start_unix_ms: pick.start_unix_ms.div_euclid(HOUR) * HOUR
                    + if pick.start_unix_ms.rem_euclid(HOUR) > 30 * MIN {
                        HOUR
                    } else {
                        0
                    },
                clock_id: 1,
                slot: 0,
            };
            let start = std::time::Instant::now();
            let outcome = crate::select::outcome(
                library,
                &history,
                next.start_unix_ms,
                Some(&last),
                &mut Rng::new(seed),
                true,
                &mut None,
            );
            times.push(start.elapsed().as_secs_f64() * 1_000.0);
            assert!(
                outcome
                    .pick
                    .is_some_and(|pick| pick.explanation.lookahead.is_some())
            );
        }
        let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
        (mean, times.iter().copied().fold(0.0, f64::max))
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
            for mode in [ExactMode::Soft, ExactMode::Hard] {
                let (mean, worst) = ten_fills_ms(&station_of(mode, songs), 7);
                eprintln!(
                    "{songs} songs, {mode:?}: a whole re-plan at the start of the hour, {mean:.1} ms on average, {worst:.1} ms at worst"
                );
            }
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
                assert!(
                    looked.draws % DRAWS == 0 && looked.draws <= MAX_FILLS,
                    "{looked:?}"
                );
            }
            assert!(!looked.faded && looked.in_window, "{looked:?}");
        }
        // Most picks follow the fill kept: a new one only when it no
        // longer lands.
        assert!(kept * 2 > simulation.picks.len(), "{kept}");
    }

    /// The top of the hour ahead, in hard mode.
    fn ahead(window: Window) -> Ahead {
        Ahead {
            window,
            mode: ExactMode::Hard,
            slot: 0,
        }
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
        // A fill drawn at random ends out of the window, or far in it: the
        // search finds tracks of the same slots that land.
        let start = From {
            library: &library,
            history: &history,
            at_ms: at,
            last: Some(last),
            window,
        };
        let mut searched = 0;
        for seed in 0..20 {
            let mut rng = Rng::new(seed);
            let fill = start.simulate(&history, at, Some(last), &[], Chosen::Slot, true, &mut rng);
            if fill.ending.close_enough() {
                continue;
            }
            searched += 1;
            let (found, stats) = start.search(&fill, &mut rng);
            let found = found.expect("a better fit");
            let stats = stats.expect("searched");
            assert!(
                stats.found && stats.landing > 0 && stats.played_again > 0,
                "{stats:?}"
            );
            assert!(found.search && found.first.is_some());
            let (offset, in_window) = found.ending.end.unwrap();
            assert!(in_window && !found.ending.faded, "{:?}", found.ending);
            assert!(found.ending.rank() < fill.ending.rank());
            // The three that land, or a fourth song played on once the
            // clock's slots ran out (2:03, 2:07, 3:29 and 3:20 end at the
            // exact time itself).
            assert!((-SEC..=0).contains(&offset), "{:?}", found.ending);
        }
        assert!(searched > 10, "{searched}");
        // And the pick: its fill lands.
        let mut plan = None;
        let pick = best(
            &library,
            &history,
            at,
            Some(&last),
            &ahead(window),
            &mut Vec::new(),
            &mut Rng::new(1),
            &mut plan,
            Planning::default(),
            &mut None,
        )
        .unwrap();
        let log = pick.explanation.lookahead.unwrap();
        assert!(!log.faded && log.in_window, "{log:?}");
        assert!(plan.is_some());
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

    /// What a pick at `at_ms` did before the exact time, with attempts.
    fn calculate(
        library: &Library,
        history: &History,
        at_ms: i64,
        last: Option<Position>,
        seed: u64,
        plan: &mut Option<Plan>,
        budget: Budget,
    ) -> (Option<Pick>, Calculation) {
        let outcome = crate::select::outcome_with(
            library,
            history,
            at_ms,
            last.as_ref(),
            &mut Rng::new(seed),
            true,
            plan,
            Planning {
                budget,
                record: true,
            },
        );
        (
            outcome.pick,
            outcome.calculation.expect("an exact time ahead"),
        )
    }

    #[test]
    fn building_stops_after_one_batch_when_the_best_lands() {
        let library = station(ExactMode::Soft);
        let (pick, calculation) = calculate(
            &library,
            &History::default(),
            40 * MIN,
            None,
            3,
            &mut None,
            Budget::REPLAYABLE,
        );
        assert_eq!(calculation.stop, Stop::Landed, "{calculation:?}");
        assert_eq!((calculation.fills, calculation.batches), (DRAWS, 1));
        assert!(calculation.ending.lands());
        assert_eq!(calculation.attempts.len(), DRAWS as usize);
        let log = pick.unwrap().explanation.lookahead.unwrap();
        assert_eq!(log.draws, DRAWS);
        // Ranked from 1, the kept one first.
        let mut ranks: Vec<u32> = calculation.attempts.iter().map(|a| a.rank).collect();
        ranks.sort_unstable();
        assert_eq!(ranks, (1..=DRAWS).collect::<Vec<_>>());
        let kept = calculation
            .attempts
            .iter()
            .find(|attempt| Some(attempt.number) == calculation.kept)
            .unwrap();
        assert_eq!(kept.rank, 1);
        assert_eq!(kept.ending, calculation.ending);
        // Each attempt goes from the end of the queue to the exact slot.
        for attempt in &calculation.attempts {
            assert_eq!(attempt.batch, 1);
            let first = attempt.tracks.first().unwrap();
            assert_eq!((first.pick, first.start_unix_ms), (0, 40 * MIN));
            let last = attempt.tracks.last().unwrap();
            assert_eq!(
                attempt.ending.offset_ms,
                Some(last.start_unix_ms + i64::from(last.next_ms) - HOUR),
                "{attempt:?}"
            );
            for pair in attempt.tracks.windows(2) {
                assert_eq!(
                    pair[1].start_unix_ms,
                    pair[0].start_unix_ms + i64::from(pair[0].next_ms)
                );
            }
        }
    }

    /// Three song slots before a hard top of the hour, songs of 4:00:
    /// from 50:00 none lands, two end at 58:00 and the third would cross
    /// the exact time.
    fn none_lands() -> Library {
        let mut library = station_of(ExactMode::Hard, 0);
        let ids: Vec<TrackId> = (1..=20).collect();
        for &id in &ids {
            library.tracks.insert(id, track(id, 240_000, None));
        }
        library.categories.insert(SONGS, category(SONGS, &ids));
        library.settings.filler_category_id = None;
        let clock = library.clocks.get_mut(&1).unwrap();
        clock.slots.truncate(1);
        clock.slots.extend((0..3).map(|_| slot(SONGS, None)));
        library
    }

    #[test]
    fn building_goes_on_while_none_lands_up_to_the_most_fills() {
        let library = none_lands();
        let last = Some(Position {
            hour_start_unix_ms: 0,
            clock_id: 1,
            slot: 0,
        });
        let history = History::default();
        let (_, calculation) = calculate(
            &library,
            &history,
            50 * MIN,
            last,
            1,
            &mut None,
            Budget::REPLAYABLE,
        );
        assert_eq!(calculation.stop, Stop::MaxFills);
        assert_eq!((calculation.fills, calculation.batches), (MAX_FILLS, 20));
        assert_eq!(calculation.attempts.len(), MAX_FILLS as usize);
        assert!(!calculation.ending.in_window, "{:?}", calculation.ending);
        assert!(calculation.attempts.iter().all(|a| !a.ending.in_window));
        // The budget's fills, in batches of 10...
        let budget = Budget {
            max_fills: 25,
            max_duration: None,
        };
        let (_, calculation) = calculate(&library, &history, 50 * MIN, last, 1, &mut None, budget);
        assert_eq!(calculation.stop, Stop::MaxFills);
        assert_eq!((calculation.fills, calculation.batches), (25, 3));
        assert_eq!(calculation.attempts.last().unwrap().batch, 3);
        // ...or its time, one fill at least.
        let budget = Budget {
            max_fills: MAX_FILLS,
            max_duration: Some(Duration::ZERO),
        };
        let (_, calculation) = calculate(&library, &history, 50 * MIN, last, 1, &mut None, budget);
        assert_eq!(calculation.stop, Stop::Time);
        assert_eq!((calculation.fills, calculation.batches), (1, 1));
    }

    #[test]
    fn the_fit_search_tries_every_combination() {
        // Three slots of 30 tracks of 100 s, but for one of each: the
        // 21st, 26th and 30th, of 200, 200 and 195 s, landing together in
        // the 10 s before a hard exact time at 600 s. The search used to
        // try the first 12 tracks of each slot but the last.
        let window = Window {
            at_ms: 600 * SEC,
            from_ms: 590 * SEC,
            cut_ms: 600 * SEC,
        };
        let mut library = station_of(ExactMode::Hard, 0);
        let lands = [(20, 200), (25, 200), (29, 195)];
        let mut picks = Vec::new();
        let mut start = 0;
        for (level, (index, length_s)) in lands.into_iter().enumerate() {
            let level = level as u64;
            let options: Vec<Choice> = (0..30)
                .map(|i| {
                    let id = 1 + level * 100 + i;
                    let next_ms = if i == index {
                        length_s * 1_000
                    } else {
                        100_000
                    };
                    library.tracks.insert(id, track(id, next_ms, None));
                    Choice { track: id, next_ms }
                })
                .collect();
            let first = options[0];
            picks.push(FillPick {
                tracks: vec![first.track],
                details: Vec::new(),
                start,
                next_at: start + i64::from(first.next_ms),
                plays: Vec::new(),
                last: None,
                plain: true,
                options: Some(options),
                filling_ms: 0,
                relaxed_steps: 0,
                age_ms: 0,
            });
            start += i64::from(first.next_ms);
        }
        let (tries, stats) = combinations(&library, window, &picks, &[0, 1, 2], 0);
        assert_eq!(tries[0].0, vec![vec![21], vec![126], vec![230]]);
        assert_eq!(tries[0].1, vec![21, 126, 230]);
        assert_eq!(stats.options, vec![30, 30, 30]);
        assert_eq!((stats.combinations, stats.landing), (27_000, 1));
        assert!(!stats.capped);
        assert_eq!(tries.len(), SEARCH_TRIES);
    }

    #[test]
    fn the_fit_search_keeps_to_the_most_combinations() {
        let window = Window {
            at_ms: 1_000 * SEC,
            from_ms: 990 * SEC,
            cut_ms: 1_000 * SEC,
        };
        let mut library = station_of(ExactMode::Hard, 0);
        let mut picks = Vec::new();
        for level in 0..3_u64 {
            let options: Vec<Choice> = (0..100)
                .map(|i| {
                    let id = 1 + level * 1_000 + i;
                    library.tracks.insert(id, track(id, 100_000, None));
                    Choice {
                        track: id,
                        next_ms: 100_000,
                    }
                })
                .collect();
            picks.push(FillPick {
                tracks: vec![options[0].track],
                details: Vec::new(),
                start: i64::try_from(level).unwrap() * 100 * SEC,
                next_at: (i64::try_from(level).unwrap() + 1) * 100 * SEC,
                plays: Vec::new(),
                last: None,
                plain: true,
                options: Some(options),
                filling_ms: 0,
                relaxed_steps: 0,
                age_ms: 0,
            });
        }
        // 100 × 100 × 100: the first 58 of each, 195 112 combinations.
        let (_, stats) = combinations(&library, window, &picks, &[0, 1, 2], 0);
        assert!(stats.capped);
        assert_eq!(stats.options, vec![100, 100, 100]);
        assert_eq!(stats.combinations, 59 * 58 * 58);
        assert!(stats.combinations <= MAX_COMBINATIONS);
    }

    #[test]
    fn the_attempts_replay_with_the_seed() {
        let library = station(ExactMode::Hard);
        let run = || {
            let (pick, mut calculation) = calculate(
                &library,
                &History::default(),
                35 * MIN,
                None,
                11,
                &mut None,
                Budget::REPLAYABLE,
            );
            calculation.duration_ms = 0;
            (pick, calculation)
        };
        let (pick, calculation) = run();
        assert!(!calculation.attempts.is_empty());
        assert_eq!((pick, calculation), run());
    }

    #[test]
    fn an_attempt_chosen_by_hand_is_followed_while_it_lands() {
        let library = station(ExactMode::Soft);
        let history = History::default();
        let (_, calculation) = calculate(
            &library,
            &history,
            40 * MIN,
            None,
            3,
            &mut None,
            Budget::REPLAYABLE,
        );
        let chosen = calculation
            .attempts
            .iter()
            .filter(|attempt| attempt.ending.in_window && !attempt.ending.faded)
            .max_by_key(|attempt| attempt.rank)
            .unwrap();
        let mut plan = Some(Plan::chosen(&calculation, chosen, 0));
        assert!(plan.as_ref().unwrap().chosen_by_hand());
        // Played again from where it was built: followed.
        let (pick, followed) = calculate(
            &library,
            &history,
            40 * MIN,
            None,
            5,
            &mut plan,
            Budget::REPLAYABLE,
        );
        let pick = pick.unwrap();
        assert_eq!(pick.track, chosen.tracks[0].track_id);
        let log = pick.explanation.lookahead.unwrap();
        assert!(log.kept && log.by_hand && log.draws == 0, "{log:?}");
        assert_eq!(followed.stop, Stop::Followed);
        assert!(followed.by_hand && !followed.hand_dropped);
        assert!(followed.attempts.is_empty() && followed.kept.is_none());
        assert!(plan.as_ref().unwrap().chosen_by_hand());
    }

    #[test]
    fn an_attempt_chosen_by_hand_that_no_longer_lands_is_dropped() {
        // An attempt said to land, whose tracks no longer do: two songs of
        // 4:00 from 50:00, then a third crossing the exact time.
        let library = none_lands();
        let last = Some(Position {
            hour_start_unix_ms: 0,
            clock_id: 1,
            slot: 0,
        });
        let (_, calculation) = calculate(
            &library,
            &History::default(),
            50 * MIN,
            last,
            1,
            &mut None,
            Budget {
                max_fills: 10,
                max_duration: None,
            },
        );
        let mut attempt = calculation.attempts[0].clone();
        attempt.ending = AttemptEnding {
            offset_ms: Some(-5 * SEC),
            in_window: true,
            ..AttemptEnding::default()
        };
        let mut plan = Some(Plan::chosen(&calculation, &attempt, 0));
        let (pick, rebuilt) = calculate(
            &library,
            &History::default(),
            50 * MIN,
            last,
            5,
            &mut plan,
            Budget {
                max_fills: 10,
                max_duration: None,
            },
        );
        assert!(rebuilt.hand_dropped && !rebuilt.by_hand, "{rebuilt:?}");
        assert_eq!(rebuilt.fills, 10);
        // The fill chosen by hand competes, played again.
        assert!(rebuilt.attempts[0].previous && rebuilt.attempts[0].batch == 0);
        assert!(
            rebuilt.attempts[0]
                .tracks
                .iter()
                .all(|t| t.chosen == Chosen::Kept)
        );
        assert_eq!(rebuilt.attempts.len(), 11);
        let log = pick.unwrap().explanation.lookahead.unwrap();
        assert!(!log.by_hand && !log.kept, "{log:?}");
        assert!(!plan.unwrap().chosen_by_hand());
    }

    #[test]
    fn the_fills_replay_with_the_seed() {
        let library = station(ExactMode::Soft);
        let run = || simulate_with(&library, History::default(), None, 0, 3 * HOUR, 5, true);
        assert_eq!(run(), run());
    }
}
