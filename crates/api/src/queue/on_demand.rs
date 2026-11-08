// SPDX-License-Identifier: AGPL-3.0-or-later

//! *Calculate now* (`docs/rotation.md#the-attempts-page`): the fills of the
//! upcoming exact time built on demand, from after what may not move, with
//! rotation's own queued picks before the exact slot counted as not queued
//! yet; and one of its attempts used in their place.

use std::collections::{HashMap, HashSet};

use heron_core::PlaySource;
use heron_rotation::{
    Attempt, Budget, Calculation, History, Library, Pick, Plan, Planning, Play, Position, Rng,
};
use sqlx::QueryBuilder;

use super::{QueueManager, entries};
use crate::auth::CurrentUser;
use crate::engine::QueueEntry;
use crate::error::{ApiError, ApiResult};
use crate::exact_times::{self, HandChoice, KeptCalculation, Trigger};
use crate::rotation;

/// The most picks an attempt played again makes before its exact slot.
const MAX_REPLAYED: usize = 60;

/// A calculation made on demand, and the queue it was made from.
#[derive(Clone, Debug)]
pub struct OnDemand {
    pub id: u64,
    pub exact_at_ms: i64,
    /// The waiting entries then, in order: its attempts can be used while
    /// the queue is the same.
    waiting: Vec<u64>,
}

/// Where the fills start when calculating now.
struct Ground {
    library: Library,
    history: History,
    at: i64,
    last: Option<Position>,
    engine_started_at: u64,
    /// The waiting entries, in order.
    waiting: Vec<u64>,
    /// Rotation's own picks before the exact slot: counted as not queued.
    movable: Vec<u64>,
    /// The exact slot when queued: the first entry of it (its linked
    /// jingle, when it has one), before which an attempt's tracks go.
    exact_first: Option<u64>,
}

fn conflict<T>(reason: &str) -> ApiResult<T> {
    Err(ApiError::Conflict(reason.to_owned()))
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

/// The waiting entries before the exact slot (`head_end`) that rotation
/// queued itself and may move: neither the one cued (nor its jingle), nor a
/// track queued by hand or a dedication, nor a match's, nor a show's
/// programme (no rotation log row). A shortened medley's second span goes
/// with its first.
fn movable_entries(
    head: &[&QueueEntry],
    fixed: &HashSet<u64>,
    logged: &HashMap<u64, u64>,
) -> Vec<u64> {
    let mut movable: Vec<u64> = Vec::new();
    for (index, entry) in head.iter().enumerate() {
        if entry.track.source != PlaySource::Rotation || fixed.contains(&entry.entry_id) {
            continue;
        }
        let span = entry.track.continues
            && index > 0
            && head[index - 1].linked_to == Some(entry.entry_id)
            && movable.contains(&head[index - 1].entry_id);
        if logged.contains_key(&entry.entry_id) || span {
            movable.push(entry.entry_id);
        }
    }
    movable
}

/// Where the exact slot's pair starts among `waiting`: its linked jingle,
/// or itself.
fn pair_start(waiting: &[&QueueEntry], exact: usize) -> usize {
    match exact.checked_sub(1) {
        Some(before) if waiting[before].linked_to == Some(waiting[exact].entry_id) => before,
        _ => exact,
    }
}

impl QueueManager {
    /// The rotation log rows of `entries` of the engine run started at
    /// `engine_started_at`, by entry.
    async fn logged(
        &self,
        engine_started_at: u64,
        entries: &[u64],
    ) -> ApiResult<HashMap<u64, u64>> {
        if entries.is_empty() {
            return Ok(HashMap::new());
        }
        let mut query =
            QueryBuilder::new("SELECT entry_id, MAX(id) FROM rotation_picks WHERE station_id = ");
        query
            .push_bind(self.station_id)
            .push(" AND engine_started_at = ")
            .push_bind(engine_started_at)
            .push(" AND entry_id IN (");
        let mut separated = query.separated(", ");
        for entry in entries {
            separated.push_bind(*entry);
        }
        query.push(") GROUP BY entry_id");
        let rows: Vec<(u64, u64)> = query.build_query_as().fetch_all(&self.pool).await?;
        Ok(rows.into_iter().collect())
    }

    /// The clock's position after the last rotation pick that stays: before
    /// row `before` (the exact slot's), the rows `left_out` aside.
    async fn position_before(
        &self,
        before: Option<u64>,
        left_out: &[u64],
    ) -> ApiResult<Option<Position>> {
        let mut query = QueryBuilder::new(
            "SELECT clock_id, slot, CAST(UNIX_TIMESTAMP(hour_start) * 1000 AS SIGNED)
             FROM rotation_picks
             WHERE clock_id IS NOT NULL AND slot IS NOT NULL AND hour_start IS NOT NULL
               AND station_id = ",
        );
        query.push_bind(self.station_id);
        if let Some(before) = before {
            query.push(" AND id < ").push_bind(before);
        }
        if !left_out.is_empty() {
            query.push(" AND id NOT IN (");
            let mut separated = query.separated(", ");
            for row in left_out {
                separated.push_bind(*row);
            }
            query.push(")");
        }
        query.push(" ORDER BY id DESC LIMIT 1");
        let row: Option<(u64, u32, i64)> =
            query.build_query_as().fetch_optional(&self.pool).await?;
        Ok(row.map(|(clock_id, slot, hour_start_unix_ms)| Position {
            hour_start_unix_ms,
            clock_id,
            slot,
        }))
    }

    /// The queue as rotation sees it when calculating now: what may not
    /// move, and after it the fills' start; the exact slot when queued.
    async fn ground(&self) -> ApiResult<Ground> {
        let state = self.engine.fetch_state().await?;
        let now = rotation::now_ms();
        let alternatives =
            crate::matches::alternatives(&self.pool, self.station_id, &state).await?;
        let lock = crate::matches::lock(&self.pool, self.station_id, &state).await?;
        let waiting: Vec<&QueueEntry> = state.waiting().collect();
        let ids: Vec<u64> = waiting.iter().map(|entry| entry.entry_id).collect();
        let engine_started_at = state.engine_started_at_unix_ms;
        let logged = self.logged(engine_started_at, &ids).await?;
        let exact = waiting
            .iter()
            .position(|entry| entry.track.start_at_unix_ms.is_some());
        let head_end = exact.map_or(waiting.len(), |exact| pair_start(&waiting, exact));
        let head = &waiting[..head_end];
        // The one cued and its jingle, and a match's tracks, stay.
        let mut fixed: HashSet<u64> = state
            .cued_jingle
            .iter()
            .map(|entry| entry.entry_id)
            .chain(state.cued.iter().map(|entry| entry.entry_id))
            .collect();
        if let Some(lock) = &lock {
            fixed.extend(ids.iter().copied().filter(|&id| lock.holds(id)));
        }
        let movable = movable_entries(head, &fixed, &logged);
        let movable_rows: Vec<u64> = movable
            .iter()
            .filter_map(|entry| logged.get(entry).copied())
            .collect();
        // The clock's position before the exact slot's pair, rotation's
        // picks counted as not queued.
        let before = exact.and_then(|exact| {
            waiting[head_end..=exact]
                .iter()
                .filter_map(|entry| logged.get(&entry.entry_id).copied())
                .min()
        });
        let last = self.position_before(before, &movable_rows).await?;
        let kept: HashSet<u64> = head
            .iter()
            .map(|entry| entry.entry_id)
            .filter(|id| !movable.contains(id))
            .collect();
        let mut ground = state.clone();
        ground.queue.retain(|entry| kept.contains(&entry.entry_id));
        if ground
            .cued
            .as_ref()
            .is_some_and(|entry| !kept.contains(&entry.entry_id))
        {
            ground.cued = None;
        }
        if ground
            .cued_jingle
            .as_ref()
            .is_some_and(|entry| !kept.contains(&entry.entry_id))
        {
            ground.cued_jingle = None;
        }
        let (library, history) = self.rotation_ground(now).await?;
        let (planned, at) = rotation::planned_starts(&ground, now, alternatives);
        let history = rotation::with_queue(&self.pool, &library, history, &planned).await?;
        Ok(Ground {
            library,
            history,
            at,
            last,
            engine_started_at,
            waiting: ids,
            movable,
            exact_first: exact.map(|exact| waiting[pair_start(&waiting, exact)].entry_id),
        })
    }

    /// *Calculate now*: builds the fills of the upcoming exact time from
    /// after what may not move, and stores them as a calculation made on
    /// demand by `user`. Nothing in the queue changes. `asked`: the exact
    /// time the user means, which must be the upcoming one (`409`).
    pub async fn calculate_now(
        &self,
        asked: Option<i64>,
        user: &CurrentUser,
    ) -> ApiResult<(u64, i64)> {
        let Ground {
            library,
            history,
            at,
            last,
            waiting,
            ..
        } = self.ground().await?;
        // Up to 200 fills or 3 s: off the async workers.
        let calculation: Option<Calculation> = tokio::task::spawn_blocking(move || {
            let mut plan = None;
            heron_rotation::next_planned_with(
                &library,
                &history,
                at,
                last.as_ref(),
                &mut Rng::new(rotation::seed()),
                &mut plan,
                Planning {
                    budget: Budget::LIVE,
                    record: true,
                },
            )
            .calculation
        })
        .await
        .map_err(internal)?;
        let Some(calculation) = calculation else {
            return conflict(
                "no exact time ahead to calculate: the upcoming one's slot has started, or what \
                 is queued and cannot move reaches its window already",
            );
        };
        let exact_at = calculation.exact_at_unix_ms;
        if asked.is_some_and(|asked| asked != exact_at) {
            return conflict("only the upcoming exact time can be calculated now");
        }
        let id = exact_times::store(
            &self.pool,
            self.station_id,
            &calculation,
            Trigger::OnDemand,
            None,
            Some(user.id),
        )
        .await?;
        if let Ok(mut kept) = self.kept.lock() {
            kept.on_demand = Some(OnDemand {
                id,
                exact_at_ms: exact_at,
                waiting,
            });
        }
        tracing::info!(
            station = self.station_id,
            calculation = id,
            exact_at,
            fills = calculation.fills,
            user = %user.username,
            "exact time: calculated now"
        );
        Ok((id, exact_at))
    }

    /// The calculation made on demand whose attempts can be used, if `id`.
    pub(super) fn on_demand(&self, id: u64) -> Option<OnDemand> {
        self.kept
            .lock()
            .ok()
            .and_then(|kept| kept.on_demand.clone())
            .filter(|on_demand| on_demand.id == id)
    }

    /// Takes `out` out of the queue, a track before its linked jingle, and
    /// their picks out of the rotation log: they will not play, and the
    /// clock's position goes back to before them.
    pub(super) async fn take_out(&self, engine_started_at: u64, out: &[u64]) -> ApiResult<()> {
        for entry in out.iter().rev() {
            match self.engine.remove(*entry).await {
                Ok(()) | Err(ApiError::Invalid(_) | ApiError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        if out.is_empty() {
            return Ok(());
        }
        let mut query = QueryBuilder::new("DELETE FROM rotation_picks WHERE station_id = ");
        query
            .push_bind(self.station_id)
            .push(" AND engine_started_at = ")
            .push_bind(engine_started_at)
            .push(" AND entry_id IN (");
        let mut separated = query.separated(", ");
        for entry in out {
            separated.push_bind(*entry);
        }
        query.push(")");
        query.build().execute(&self.pool).await?;
        tracing::info!(
            station = self.station_id,
            removed = out.len(),
            "exact time: the tracks the attempt chosen by hand replaces taken out"
        );
        Ok(())
    }

    /// Uses `attempt` of the calculation made on demand: the queue must be
    /// as it was then. Rotation's picks before the exact slot are taken
    /// out; when the exact slot is not queued, the attempt is the fill
    /// kept, else its tracks, played again with the real rules, are queued
    /// before it at once. Called with the queue manager busy.
    pub(super) async fn use_on_demand(
        &self,
        on_demand: OnDemand,
        attempt: &Attempt,
        user: &CurrentUser,
    ) -> ApiResult<()> {
        let ground = self.ground().await?;
        if ground.waiting != on_demand.waiting {
            return conflict(
                "the queue changed since this calculation (a track added, removed, moved or \
                 started): calculate again",
            );
        }
        let plan = Plan::chosen(on_demand.exact_at_ms, attempt, 0);
        let choice = HandChoice {
            calculation_id: on_demand.id,
            attempt: attempt.number,
            exact_at_unix_ms: on_demand.exact_at_ms,
            user_id: user.id,
            username: user.username.clone(),
            at_unix_ms: rotation::now_ms(),
            ending: attempt.ending,
        };
        let Some(exact_first) = ground.exact_first else {
            // The exact slot is not queued: the attempt is the fill kept,
            // queued as the queue's depth asks.
            self.take_out(ground.engine_started_at, &ground.movable)
                .await?;
            if let Ok(mut kept) = self.kept.lock() {
                kept.plan = Some(plan);
                kept.calculation = Some(KeptCalculation {
                    id: on_demand.id,
                    exact_at_ms: on_demand.exact_at_ms,
                    queued: Vec::new(),
                    blocked: false,
                });
                kept.choice = Some(choice);
                kept.next_trigger = None;
                kept.on_demand = None;
            }
            self.wake();
            return Ok(());
        };
        let Ground {
            library,
            history,
            at,
            last,
            engine_started_at,
            movable,
            ..
        } = ground;
        let replayed =
            tokio::task::spawn_blocking(move || replay(library, history, at, last, plan))
                .await
                .map_err(internal)?;
        let Some((mut picks, counters)) = replayed else {
            return conflict("this attempt no longer lands with the real rules: calculate again");
        };
        for pick in &mut picks {
            if let Some(lookahead) = pick.explanation.lookahead.as_mut() {
                lookahead.calculation_id = Some(on_demand.id);
            }
        }
        let ids: Vec<u64> = picks.iter().map(|pick| pick.track).collect();
        let tracks = self.tracks(&ids, PlaySource::Rotation).await?;
        let (tracks, linked, firsts) = entries(&picks, tracks)?;
        self.take_out(engine_started_at, &movable).await?;
        let state = self.engine.fetch_state().await?;
        let Some(position) = state
            .waiting()
            .position(|entry| entry.entry_id == exact_first)
            .and_then(|position| u32::try_from(position).ok())
        else {
            return conflict("the exact slot left the queue meanwhile");
        };
        if crate::matches::lock(&self.pool, self.station_id, &state)
            .await?
            .is_some_and(|lock| lock.refuses(position, None))
        {
            return Err(crate::matches::between());
        }
        let entry_ids = self.engine.enqueue(tracks, Some(position), linked).await?;
        let mut rows = Vec::new();
        for (pick, index) in picks.iter().zip(firsts) {
            let Some(entry_id) = entry_ids.get(index) else {
                break;
            };
            let entry = (state.engine_started_at_unix_ms, *entry_id);
            rows.push(rotation::record_pick(&self.pool, self.station_id, pick, Some(entry)).await?);
        }
        // Queued before the exact slot: the clock's position stays the
        // exact slot's, or what follows it.
        if !rows.is_empty() {
            let mut query =
                QueryBuilder::new("UPDATE rotation_picks SET slot = NULL WHERE id IN (");
            let mut separated = query.separated(", ");
            for row in &rows {
                separated.push_bind(*row);
            }
            query.push(")");
            query.build().execute(&self.pool).await?;
        }
        rotation::save_counters(&self.pool, self.station_id, &counters).await?;
        if let Ok(mut kept) = self.kept.lock() {
            kept.plan = None;
            kept.calculation = None;
            kept.choice = Some(choice);
            kept.next_trigger = None;
            kept.on_demand = None;
        }
        tracing::info!(
            station = self.station_id,
            tracks = ids.len(),
            "exact time: the attempt chosen by hand queued before its exact slot"
        );
        Ok(())
    }
}

/// An attempt played again: its picks, and the play ratio counters they
/// move.
type Replayed = (Vec<Pick>, Vec<(u64, u8)>);

/// `plan` played again pick by pick with the real rules from `at`, up to
/// its exact slot (left out: it is queued): its picks and the play ratio
/// counters they move; `None` when it no longer lands.
fn replay(
    mut library: Library,
    mut history: History,
    mut at: i64,
    mut last: Option<Position>,
    plan: Plan,
) -> Option<Replayed> {
    let mut plan = Some(plan);
    let mut rng = Rng::new(rotation::seed());
    let mut picks = Vec::new();
    let mut counters = Vec::new();
    // A fill built when it no longer holds is not used: one is enough.
    let planning = Planning {
        budget: Budget {
            max_fills: 1,
            max_duration: None,
        },
        record: false,
    };
    for _ in 0..MAX_REPLAYED {
        let outcome = heron_rotation::next_planned_with(
            &library,
            &history,
            at,
            last.as_ref(),
            &mut rng,
            &mut plan,
            planning,
        );
        let pick = outcome.pick.clone()?;
        if pick.explanation.exact.is_some() {
            return Some((picks, counters));
        }
        if !pick
            .explanation
            .lookahead
            .is_some_and(|lookahead| lookahead.kept && lookahead.by_hand)
        {
            return None;
        }
        counters.extend(library.count_down(outcome.offered()));
        for pick in pick.into_picks() {
            let track = library.tracks.get(&pick.track)?;
            let start = pick.explanation.at_unix_ms;
            history.push(Play::of(track, start));
            at = start + i64::from(pick.next_ms(&library));
            if pick.position.is_some() {
                last = pick.position;
            }
            picks.push(pick);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::QueuedTrack;

    fn entry(id: u64, source: PlaySource) -> QueueEntry {
        QueueEntry {
            entry_id: id,
            track: QueuedTrack {
                track_id: Some(id),
                path: format!("{id}.flac"),
                title: None,
                artist: None,
                album: None,
                duration_ms: 200_000,
                cue_in_ms: 0,
                cue_out_ms: 200_000,
                source,
                no_metadata: false,
                over_intro: false,
                intro_ms: None,
                start_at_unix_ms: None,
                continues: false,
            },
            linked_to: None,
            next_start_ms: None,
        }
    }

    #[test]
    fn only_rotations_own_picks_may_move() {
        let cued = entry(1, PlaySource::Rotation);
        let picked = entry(2, PlaySource::Rotation);
        let by_hand = entry(3, PlaySource::Manual);
        let dedication = entry(4, PlaySource::Request);
        let mut medley = entry(5, PlaySource::Rotation);
        medley.linked_to = Some(6);
        let mut span = entry(6, PlaySource::Rotation);
        span.track.continues = true;
        let show = entry(7, PlaySource::Rotation);
        let matched = entry(8, PlaySource::Rotation);
        let head = [
            &cued,
            &picked,
            &by_hand,
            &dedication,
            &medley,
            &span,
            &show,
            &matched,
        ];
        let fixed = HashSet::from([1, 8]);
        // The show's programme has no rotation log row.
        let logged = HashMap::from([(1, 10), (2, 11), (5, 12), (8, 13)]);
        assert_eq!(movable_entries(&head, &fixed, &logged), vec![2, 5, 6]);
    }

    /// A station whose clock has a soft news flash at the top of each hour,
    /// then songs of 2:30 to 5:30.
    fn station() -> Library {
        use std::collections::HashMap as Map;

        use heron_rotation::{
            Category, Clock, ExactMode, ExactTime, LibraryParts, Member, Method, RotationSettings,
            Schedule, SeparationOverrides, Slot, Track,
        };
        use jiff::tz::TimeZone;
        let mut rng = Rng::new(42);
        let track = |id: u64, length_ms: u32| Track {
            id,
            title_key: Some(format!("title {id}")),
            album_key: Some(format!("album {id}")),
            artists: vec![1_000 + id],
            categories: vec![],
            length_ms,
            mix_ms: None,
            ratio: 10,
            dayparts: None,
            daypart_days: None,
        };
        let mut tracks: Vec<Track> = (1..=200)
            .map(|id| track(id, 150_000 + u32::try_from(rng.below(180_000)).unwrap()))
            .collect();
        tracks.push(track(500, 120_000));
        let category = |id: u64, members: &[u64], separation: SeparationOverrides| Category {
            id,
            name: format!("C{id}"),
            separation,
            members: members
                .iter()
                .map(|&track| Member { track, weight: 1.0 })
                .collect(),
        };
        let often = SeparationOverrides {
            track_min: Some(10),
            title_min: Some(10),
            artist_min: Some(0),
            album_min: Some(0),
            ..SeparationOverrides::default()
        };
        let slot = |category: u64, exact: Option<ExactTime>| Slot {
            category,
            method: Method::LeastRecentlyPlayed,
            enforce_rules: true,
            separation: SeparationOverrides::default(),
            linked_to_next: false,
            exact,
        };
        let news = ExactTime {
            at_s: 0,
            mode: ExactMode::Soft,
            before_s: 10,
            after_s: 30,
            medley_fill: false,
            cut_live: false,
        };
        let mut slots = vec![slot(2, Some(news))];
        slots.extend((0..16).map(|_| slot(1, None)));
        Library::new(
            LibraryParts {
                tracks,
                categories: vec![
                    category(
                        1,
                        &(1..=200).collect::<Vec<_>>(),
                        SeparationOverrides::default(),
                    ),
                    category(2, &[500], often),
                ],
                clocks: vec![Clock {
                    id: 1,
                    name: "Hour".into(),
                    looping: false,
                    slots,
                }],
                schedule: Schedule {
                    hours: Map::from_iter(
                        (1..=7).flat_map(|day| (0..24).map(move |hour| ((day, hour), 1))),
                    ),
                    ..Schedule::default()
                },
                settings: RotationSettings::default(),
                ..LibraryParts::default()
            },
            TimeZone::UTC,
        )
    }

    #[test]
    fn an_attempt_played_again_queues_its_tracks_up_to_the_exact_slot() {
        let library = station();
        // 2026-11-09 18:40 UTC, the news at 19:00.
        let at = 1_794_249_600_000;
        let calculation = heron_rotation::next_planned_with(
            &library,
            &History::default(),
            at,
            None,
            &mut Rng::new(3),
            &mut None,
            Planning {
                budget: Budget::REPLAYABLE,
                record: true,
            },
        )
        .calculation
        .expect("an exact time ahead");
        assert_eq!(calculation.exact_at_unix_ms, at + 20 * 60_000);
        // The second best that lands, if any; else the best.
        let mut landing: Vec<&Attempt> = calculation
            .attempts
            .iter()
            .filter(|attempt| exact_times::choosable(&attempt.ending))
            .collect();
        landing.sort_by_key(|attempt| attempt.rank);
        let attempt = landing
            .get(1)
            .or(landing.first())
            .copied()
            .expect("one lands");
        let plan = Plan::chosen(calculation.exact_at_unix_ms, attempt, 0);
        let (picks, counters) =
            replay(library.clone(), History::default(), at, None, plan).expect("it still lands");
        let tracks: Vec<u64> = picks.iter().map(|pick| pick.track).collect();
        let wanted: Vec<u64> = attempt.tracks.iter().map(|track| track.track_id).collect();
        assert_eq!(tracks, wanted);
        assert!(picks.iter().all(|pick| pick.explanation.exact.is_none()));
        assert!(picks.iter().all(|pick| {
            pick.explanation
                .lookahead
                .is_some_and(|lookahead| lookahead.by_hand && lookahead.kept)
        }));
        assert_eq!(counters.len(), picks.len());
        // Ten minutes later it no longer lands.
        let plan = Plan::chosen(calculation.exact_at_unix_ms, attempt, 0);
        assert!(replay(library, History::default(), at + 10 * 60_000, None, plan).is_none());
    }

    #[test]
    fn an_exact_slots_pair_starts_at_its_jingle() {
        let mut jingle = entry(1, PlaySource::Rotation);
        jingle.linked_to = Some(2);
        let exact = entry(2, PlaySource::Rotation);
        let other = entry(3, PlaySource::Rotation);
        assert_eq!(pair_start(&[&jingle, &exact], 1), 0);
        assert_eq!(pair_start(&[&other, &exact], 1), 1);
        assert_eq!(pair_start(&[&exact], 0), 0);
    }
}
