// SPDX-License-Identifier: AGPL-3.0-or-later

//! The queue manager: feeds the engine with library tracks.
//!
//! - Builds the tracks the engine plays from the database: path, tags, cue
//!   points and crossfade point from the analysis, per-track overrides.
//! - Keeps the queue from running dry (`queue.autofill`) with the tracks
//!   rotation picks (`heron-rotation`), each recorded in the rotation log;
//!   a jingle of a linked clock slot is queued linked to its track.
//! - Before an exact time, calculates again when the queue changes where it
//!   ends, and replaces the picks it queued from the fill kept.
//! - Follows the library: queued tracks whose file the librarian moved, or
//!   whose cue points were changed, are updated; tracks whose file
//!   disappeared are taken out of the queue.
//!
//! The engine holds the queue: when the API is down it plays what it has.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use heron_core::FadeCurve;
use heron_core::playout::{CrossfadeMode, CrossfadeOverrides, Fade};
use heron_core::{PlaySource, TrackOverrides, TrackPlayout};
use heron_proto::engine as pb;
use heron_rotation::{Budget, Planning, Play, Rng, Stop};
use sqlx::{MySqlPool, QueryBuilder};
use tokio::sync::Notify;

use crate::engine::{EngineLink, PlayerState, QueueEntry, source_to_proto};
use crate::error::{ApiError, ApiResult};
use crate::exact_times::{self, HandChoice, Kept, KeptCalculation, QueuedPick, Trigger};
use crate::rotation::{self, SILENT_MS};
use crate::settings::{self, QueueSettings};

const TICK: Duration = Duration::from_secs(2);
const FOLLOW_INTERVAL: Duration = Duration::from_secs(5);

/// Wait before trying again when rotation could pick nothing.
const NOTHING_TO_PICK: Duration = Duration::from_secs(30);

/// How far the end of the queue before an exact time moves before the
/// queue manager calculates again (`docs/rotation.md#exact-times`).
const MOVED_MS: i64 = 2_000;
/// How long it waits for the queue to settle first: a burst of changes
/// gives one calculation.
const SETTLE: Duration = Duration::from_secs(2);

pub struct QueueManager {
    pool: MySqlPool,
    engine: Arc<EngineLink>,
    station_id: u64,
    /// The other stations, for the rules across stations.
    stations: std::sync::Weak<crate::stations::Stations>,
    wake: Notify,
    /// Rotation could pick nothing: no new try before then.
    idle_until: Mutex<Option<Instant>>,
    /// The best fill before the next exact time, followed by the next
    /// picks while it still lands (`docs/rotation.md#exact-times`), the
    /// calculation it comes from and the attempt chosen by hand
    /// (`docs/rotation.md#the-attempts-page`).
    kept: Mutex<Kept>,
    /// Held while the queue is filled, and while an attempt replaces the
    /// fill kept.
    busy: tokio::sync::Mutex<()>,
    /// Every track: the match whose winner was tried as the next vote's
    /// track, so as not to try again.
    chain_tried: Mutex<Option<u64>>,
    /// Where the queue ends before the exact time, watched.
    watch: Mutex<Watch>,
}

/// Where the waiting tracks before the exact time of the calculation kept
/// end, once the last pick was queued, to tell when the queue changed.
#[derive(Default)]
struct Watch {
    /// The calculation it watches the queue of.
    calculation: Option<u64>,
    /// The end, from the exact time's queue as it was.
    baseline: Option<i64>,
    /// Entries just queued, not in the engine's state yet: the end is
    /// taken again once they are.
    awaiting: Vec<u64>,
    /// The end moved: when to calculate again, and where it then was.
    pending: Option<(Instant, i64)>,
}

/// Where the exact slot is in `queue`, its linked jingle first: the first
/// entry starting at `cut` at the latest, the end of its window.
fn exact_index(queue: &[QueueEntry], cut: i64) -> Option<usize> {
    let mut index = queue
        .iter()
        .position(|entry| entry.track.start_at_unix_ms == Some(cut))?;
    while index > 0 && queue[index - 1].linked_to == Some(queue[index].entry_id) {
        index -= 1;
    }
    Some(index)
}

/// Where the waiting tracks before the exact slot (starting at `cut` at
/// the latest) end, as the queue's expected starts count them; `None`
/// once the exact slot is cued.
fn end_before_exact(
    state: &PlayerState,
    now: i64,
    alternatives: Option<(u64, u64)>,
    cut: i64,
) -> Option<i64> {
    if state
        .cued
        .as_ref()
        .is_some_and(|entry| entry.track.start_at_unix_ms == Some(cut))
    {
        return None;
    }
    let mut before = state.clone();
    if let Some(index) = exact_index(&before.queue, cut) {
        before.queue.truncate(index);
    }
    Some(rotation::planned_starts(&before, now, alternatives).1)
}

/// A library track as the engine needs it.
#[derive(Debug, sqlx::FromRow)]
struct TrackRow {
    id: u64,
    path: String,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration_ms: Option<u32>,
    /// Disabled (or at a play ratio of 0): does not play at all.
    enabled: bool,
    missing: bool,
    analyzed: bool,
    cue_in_ms: Option<u32>,
    cue_out_ms: Option<u32>,
    fade_out_start_ms: Option<u32>,
    integrated_lufs: Option<f64>,
    true_peak_db: Option<f64>,
    override_cue_in_ms: Option<u32>,
    override_cue_out_ms: Option<u32>,
    gap_killer_enabled: Option<bool>,
    crossfade: Option<String>,
    /// In a category whose type sends no metadata (a jingle).
    no_metadata: bool,
    /// In a category whose type plays over the next track's intro.
    over_intro: bool,
    /// Where the intro ends, from the import (`track_markers`).
    intro_ms: Option<u32>,
    /// A rendered medley.
    medley: bool,
    /// Medleys are turned off for the station: a medley does not play.
    #[sqlx(skip)]
    medleys_off: bool,
    /// Disabled on the station (a ratio of 0 there): does not play there.
    #[sqlx(skip)]
    disabled_here: bool,
}

const TRACK_COLUMNS: &str = "
    t.id, t.path, t.title, t.artist, t.album, t.duration_ms, t.enabled,
    t.missing_since IS NOT NULL AS missing,
    a.track_id IS NOT NULL AS analyzed,
    a.cue_in_ms, a.cue_out_ms, a.fade_out_start_ms, a.integrated_lufs, a.true_peak_db,
    o.cue_in_ms AS override_cue_in_ms, o.cue_out_ms AS override_cue_out_ms,
    o.gap_killer_enabled, CAST(o.crossfade AS CHAR) AS crossfade,
    EXISTS (SELECT 1 FROM track_categories tc
            JOIN categories c ON c.id = tc.category_id
            JOIN category_types ct ON ct.id = c.type_id
            WHERE tc.track_id = t.id AND NOT ct.sends_metadata)
        OR EXISTS (SELECT 1 FROM medleys md WHERE md.track_id = t.id) AS no_metadata,
    EXISTS (SELECT 1 FROM track_categories tc
            JOIN categories c ON c.id = tc.category_id
            JOIN category_types ct ON ct.id = c.type_id
            WHERE tc.track_id = t.id AND ct.plays_over_intro) AS over_intro,
    m.intro_ms,
    EXISTS (SELECT 1 FROM medleys md WHERE md.track_id = t.id) AS medley
    FROM tracks t
    LEFT JOIN track_analysis a ON a.track_id = t.id
    LEFT JOIN track_overrides o ON o.track_id = t.id
    LEFT JOIN track_markers m ON m.track_id = t.id";

impl TrackRow {
    /// Overrides as stored; invalid crossfade JSON counts as none.
    fn overrides(&self) -> TrackOverrides {
        let crossfade = self
            .crossfade
            .as_deref()
            .and_then(|json| match serde_json::from_str::<CrossfadeOverrides>(json) {
                Ok(overrides) => Some(overrides),
                Err(error) => {
                    tracing::warn!(track = self.id, %error, "ignoring invalid crossfade overrides");
                    None
                }
            })
            .unwrap_or_default();
        TrackOverrides {
            cue_in_ms: self.override_cue_in_ms,
            cue_out_ms: self.override_cue_out_ms,
            gap_killer_enabled: self.gap_killer_enabled,
            crossfade,
        }
    }

    fn playout(&self) -> TrackPlayout {
        self.playout_with(&self.overrides())
    }

    /// How the track plays with these overrides instead of its own.
    fn playout_with(&self, overrides: &TrackOverrides) -> TrackPlayout {
        let base = match (self.analyzed, self.cue_in_ms, self.cue_out_ms) {
            (true, Some(cue_in_ms), Some(cue_out_ms)) => TrackPlayout {
                duration_ms: self.duration_ms.unwrap_or(cue_out_ms).max(cue_out_ms),
                cue_in_ms,
                cue_out_ms,
                mix_point_ms: self.fade_out_start_ms,
                integrated_lufs: self.integrated_lufs,
                true_peak_db: self.true_peak_db,
                crossfade: CrossfadeOverrides::default(),
            },
            _ => TrackPlayout::unanalyzed(self.duration_ms.unwrap_or(0)),
        };
        if !self.analyzed && self.duration_ms.is_none() {
            // Unknown length: the engine plays the whole file, overrides aside.
            return TrackPlayout {
                cue_out_ms: 0,
                crossfade: overrides.crossfade,
                ..base
            };
        }
        base.with_overrides(overrides)
    }

    /// Whether it may stay queued: in the library, enabled (on the
    /// station too), and not a medley while medleys are off.
    fn plays(&self) -> bool {
        self.enabled && !self.disabled_here && !self.missing && !(self.medley && self.medleys_off)
    }

    /// Why the track cannot be played, if it cannot.
    fn unplayable(&self) -> Option<String> {
        if self.missing {
            return Some(format!("track {}: its file is missing", self.id));
        }
        if !self.enabled {
            return Some(format!("track {}: it is disabled", self.id));
        }
        if self.disabled_here {
            return Some(format!("track {}: it is disabled on this station", self.id));
        }
        if self.medley && self.medleys_off {
            return Some(format!(
                "track {}: it is a medley, and medleys are turned off",
                self.id
            ));
        }
        let playout = self.playout();
        if self.analyzed && playout.playable_ms() < SILENT_MS {
            return Some(format!("track {}: nothing audible to play", self.id));
        }
        None
    }

    fn to_proto(&self, source: PlaySource) -> ApiResult<pb::Track> {
        let playout = self.playout();
        let crossfade_overrides_json = if playout.crossfade == CrossfadeOverrides::default() {
            String::new()
        } else {
            serde_json::to_string(&playout.crossfade)
                .map_err(|error| ApiError::Internal(error.to_string()))?
        };
        Ok(pb::Track {
            track_id: Some(self.id),
            path: self.path.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            duration_ms: playout.duration_ms,
            cue_in_ms: playout.cue_in_ms,
            cue_out_ms: playout.cue_out_ms,
            mix_point_ms: playout.mix_point_ms,
            integrated_lufs: playout.integrated_lufs,
            true_peak_db: playout.true_peak_db,
            crossfade_overrides_json,
            source: source_to_proto(source).into(),
            no_metadata: self.no_metadata,
            over_intro: self.over_intro,
            intro_ms: self.intro_ms,
            start_at_unix_ms: None,
            cut_fade_ms: 0,
            continues: false,
        })
    }
}

/// The queue entries of rotation's picks: an exact slot's track with its
/// start time, a shortened medley as two spans of its file, the first
/// linked to the second. Returns the tracks, the indices of those linked to
/// the one after them, and each pick's first entry.
fn entries(
    picks: &[heron_rotation::Pick],
    tracks: Vec<pb::Track>,
) -> ApiResult<(Vec<pb::Track>, Vec<u32>, Vec<usize>)> {
    let json = |overrides: &CrossfadeOverrides| {
        serde_json::to_string(overrides).map_err(|error| ApiError::Internal(error.to_string()))
    };
    let mut queued = Vec::with_capacity(tracks.len());
    let mut linked = Vec::new();
    let mut firsts = Vec::with_capacity(picks.len());
    for (pick, mut track) in picks.iter().zip(tracks) {
        firsts.push(queued.len());
        let index = u32::try_from(queued.len()).unwrap_or(u32::MAX);
        if pick.explanation.linked {
            linked.push(index);
        }
        if let Some(start_at) = pick.start_at_ms {
            track.start_at_unix_ms = Some(start_at);
            track.cut_fade_ms = heron_rotation::exact::CUT_FADE_MS;
        }
        let Some(cut) = &pick.medley_cut else {
            queued.push(track);
            continue;
        };
        let fade = Fade {
            enabled: true,
            curve: FadeCurve::Linear,
            time_ms: cut.crossfade_ms,
            level_pct: 0,
        };
        let mut rest = track.clone();
        track.cue_out_ms = cut.first_to_ms;
        track.mix_point_ms = None;
        track.crossfade_overrides_json = json(&CrossfadeOverrides {
            fade_out: Some(fade),
            mode: Some(CrossfadeMode::Fixed),
            fixed_ms: Some(cut.crossfade_ms),
            crossfade: Some(true),
            ..CrossfadeOverrides::default()
        })?;
        rest.cue_in_ms = cut.second_from_ms;
        rest.start_at_unix_ms = None;
        rest.cut_fade_ms = 0;
        rest.continues = true;
        rest.crossfade_overrides_json = json(&CrossfadeOverrides {
            fade_in: Some(fade),
            ..CrossfadeOverrides::default()
        })?;
        linked.push(index);
        queued.push(track);
        queued.push(rest);
    }
    Ok((queued, linked, firsts))
}

impl QueueManager {
    pub fn new(
        pool: MySqlPool,
        engine: Arc<EngineLink>,
        station_id: u64,
        stations: std::sync::Weak<crate::stations::Stations>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            engine,
            station_id,
            stations,
            wake: Notify::new(),
            idle_until: Mutex::new(None),
            kept: Mutex::new(Kept::default()),
            busy: tokio::sync::Mutex::new(()),
            chain_tried: Mutex::new(None),
            watch: Mutex::new(Watch::default()),
        })
    }

    /// Checks the queue now (settings, library or rotation changed).
    pub fn wake(&self) {
        if let Ok(mut idle) = self.idle_until.lock() {
            *idle = None;
        }
        self.wake.notify_one();
    }

    fn idle(&self) -> bool {
        self.idle_until
            .lock()
            .ok()
            .and_then(|idle| *idle)
            .is_some_and(|until| Instant::now() < until)
    }

    fn rest(&self) {
        if let Ok(mut idle) = self.idle_until.lock() {
            *idle = Some(Instant::now() + NOTHING_TO_PICK);
        }
    }

    async fn rows(&self, ids: &[u64]) -> ApiResult<Vec<TrackRow>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut query = QueryBuilder::new(format!("SELECT {TRACK_COLUMNS} WHERE t.id IN ("));
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        query.push(")");
        let mut rows: Vec<TrackRow> = query.build_query_as().fetch_all(&self.pool).await?;
        let mut query = QueryBuilder::new(
            "SELECT track_id FROM track_station_ratios WHERE NOT enabled AND station_id = ",
        );
        query.push_bind(self.station_id).push(" AND track_id IN (");
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        query.push(")");
        let disabled: Vec<u64> = query.build_query_scalar().fetch_all(&self.pool).await?;
        for row in &mut rows {
            row.disabled_here = disabled.contains(&row.id);
        }
        if rows.iter().any(|row| row.medley)
            && !crate::modules::on_for(&self.pool, self.station_id)
                .await?
                .medleys
        {
            for row in &mut rows {
                row.medleys_off = true;
            }
        }
        Ok(rows)
    }

    /// The engine's version of library tracks, in the order given. Fails
    /// when one does not exist or cannot be played.
    pub async fn tracks(&self, ids: &[u64], source: PlaySource) -> ApiResult<Vec<pb::Track>> {
        let rows = self.rows(ids).await?;
        ids.iter()
            .map(|id| {
                let row = rows
                    .iter()
                    .find(|row| row.id == *id)
                    .ok_or_else(|| ApiError::NotFound(format!("track {id} not found")))?;
                if let Some(reason) = row.unplayable() {
                    return Err(ApiError::Invalid(reason));
                }
                row.to_proto(source)
            })
            .collect()
    }

    /// How a library track plays: analysis and overrides combined, its
    /// own overrides or, for previews, `overrides` not saved yet.
    pub async fn playout(
        &self,
        id: u64,
        overrides: Option<&TrackOverrides>,
    ) -> ApiResult<TrackPlayout> {
        let rows = self.rows(&[id]).await?;
        let row = rows
            .first()
            .ok_or_else(|| ApiError::NotFound(format!("track {id} not found")))?;
        Ok(overrides.map_or_else(|| row.playout(), |overrides| row.playout_with(overrides)))
    }

    pub async fn run(self: Arc<Self>) {
        let mut engine = self.engine.state();
        let mut last_follow = Instant::now();
        loop {
            let state = engine.borrow_and_update().clone();
            if let Some(state) = state {
                if let Err(error) = self.watch_queue(&state).await {
                    tracing::warn!(%error, "cannot check the queue against the fill kept");
                }
                if let Err(error) = self.trim_before_exact(&state).await {
                    tracing::warn!(%error, "cannot check the queue against its exact time");
                }
                if let Err(error) = self.fill(&state).await {
                    tracing::warn!(%error, "cannot fill the queue");
                }
                if last_follow.elapsed() >= FOLLOW_INTERVAL {
                    last_follow = Instant::now();
                    if let Err(error) = self.follow_library(&state).await {
                        tracing::warn!(%error, "cannot check queued tracks against the library");
                    }
                }
            }
            tokio::select! {
                () = self.wake.notified() => {}
                () = tokio::time::sleep(TICK) => {}
                _ = engine.changed() => {}
            }
        }
    }

    /// Adds the tracks rotation picks until `min_queued` are waiting.
    async fn fill(&self, state: &PlayerState) -> ApiResult<()> {
        let _busy = self.busy.lock().await;
        self.fill_locked(state, 0).await
    }

    /// [`Self::fill`], `self.busy` held, with at least `at_least` picks.
    async fn fill_locked(&self, state: &PlayerState, at_least: usize) -> ApiResult<()> {
        let settings: QueueSettings =
            settings::load_for(&self.pool, self.station_id, QueueSettings::SETTINGS_NAME).await?;
        // A cued fallback track gives way to anything queued.
        let waiting = state
            .waiting()
            .filter(|entry| entry.track.source != PlaySource::Fallback)
            .count();
        let missing = (settings.min_queued as usize)
            .saturating_sub(waiting)
            .max(at_least);
        if !settings.autofill {
            return Ok(());
        }
        // Every track: the winner of the match just closed carries the next
        // vote, whatever the queue's depth.
        let tried = self.chain_tried.lock().ok().and_then(|tried| *tried);
        // A match that fails never stops the music: the queue fills as
        // without one.
        let chained = crate::matches::chained(&self.pool, self.station_id, state)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "cannot read the last match: the queue fills without it");
                None
            })
            .filter(|(id, _, _)| tried != Some(*id));
        if missing == 0 && chained.is_none() {
            return Ok(());
        }
        // Every track: nothing goes behind the match waiting or open.
        let holds = crate::matches::holds_queue(&self.pool, self.station_id)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "cannot read the match waiting: the queue fills without it");
                false
            });
        if chained.is_none() && holds {
            return Ok(());
        }
        let now = rotation::now_ms();
        // A match's two tracks: one plays.
        let alternatives = crate::matches::alternatives(&self.pool, self.station_id, state)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "cannot read the match's tracks: the queue fills without it");
                None
            });
        // A show on air (the Top 30) queues its programme in its hours.
        let (_, planned_end) = rotation::planned_starts(state, now, alternatives);
        if self.fill_show(planned_end, missing).await? {
            return Ok(());
        }
        if self.idle() {
            return Ok(());
        }
        let mut library = rotation::load_library(&self.pool, self.station_id).await?;
        let history = rotation::load_history(&self.pool, self.station_id, &library, now).await?;
        let history = match self.stations.upgrade() {
            Some(stations) => {
                let elsewhere =
                    rotation::load_elsewhere(&self.pool, &stations, self.station_id, &library, now)
                        .await?;
                history.with_elsewhere(elsewhere)
            }
            None => history,
        };
        let (planned, mut at) = rotation::planned_starts(state, now, alternatives);
        let mut history = rotation::with_queue(&self.pool, &library, history, &planned).await?;
        let mut last = rotation::last_position(&self.pool, self.station_id).await?;
        let mut rng = Rng::new(rotation::seed());
        let mut picks = Vec::new();
        // The picks made before an exact time (their range in `picks`),
        // and the calculation they come from.
        let mut ahead: Vec<(std::ops::Range<usize>, u64)> = Vec::new();
        // Play ratio counters moved by these picks, saved once queued.
        let mut counters = Vec::new();
        // A match in the show's hours: its tracks after one of the picks.
        let mut planner = crate::matches::planner(&self.pool, self.station_id)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "cannot plan the match: the queue fills without it");
                None
            });
        let mut planned_match = None;
        // The best fill before an exact time, kept from a fill to the next
        // while it still lands, and the calculation it comes from.
        // A copy, written back once the picks are made: the attempts page
        // reads the state as it was meanwhile.
        let mut kept = self
            .kept
            .lock()
            .map(|kept| kept.clone())
            .unwrap_or_default();
        let mut plan = kept.plan.take();
        if let (Some((id, entry, track)), Some(planner)) = (chained, planner.as_mut()) {
            if let Ok(mut tried) = self.chain_tried.lock() {
                *tried = Some(id);
            }
            let start = planned
                .iter()
                .find(|(waiting, _)| waiting.entry_id == entry)
                .map_or(at, |(_, start)| *start);
            let mut ground = crate::matches::Ground {
                library: &library,
                history: &mut history,
                at: &mut at,
                last: &mut last,
                plan: &mut plan,
                rng: &mut rng,
            };
            planned_match = planner.behind(&mut ground, &mut picks, entry, track, start);
        }
        let missing = if planned_match.is_some() { 0 } else { missing };
        'picking: for _ in 0..missing {
            let outcome = heron_rotation::next_planned_with(
                &library,
                &history,
                at,
                last.as_ref(),
                &mut rng,
                &mut plan,
                Planning {
                    budget: Budget::LIVE,
                    record: true,
                },
            );
            counters.extend(library.count_down(outcome.offered()));
            let Some(mut pick) = outcome.pick else {
                break;
            };
            let from = match &outcome.calculation {
                Some(calculation) => self.calculated(&mut kept, calculation, &mut pick).await,
                None => None,
            };
            let group = pick.into_picks();
            if group
                .iter()
                .any(|pick| !library.tracks.contains_key(&pick.track))
            {
                break;
            }
            let first = picks.len();
            for pick in group {
                let Some(track) = library.tracks.get(&pick.track) else {
                    break 'picking;
                };
                // An exact slot's planned start jumps to its time.
                let start = pick.explanation.at_unix_ms;
                history.push(Play::of(track, start));
                at = start + i64::from(pick.next_ms(&library));
                if pick.position.is_some() {
                    last = pick.position;
                }
                picks.push(pick);
            }
            if let Some(from) = from {
                ahead.push((first..picks.len(), from));
            }
            if let Some(planner) = planner.as_mut() {
                let mut ground = crate::matches::Ground {
                    library: &library,
                    history: &mut history,
                    at: &mut at,
                    last: &mut last,
                    plan: &mut plan,
                    rng: &mut rng,
                };
                if let Some(planned) = planner.after(&mut ground, &mut picks) {
                    planned_match = Some(planned);
                    break;
                }
            }
        }
        if planned_match.is_some() {
            // The match's tracks are locked: no attempt replaces them.
            if let Some(calculation) = kept.calculation.as_mut() {
                calculation.blocked = true;
            }
            ahead.clear();
        }
        // The choice by hand holds until its exact time, its fill followed
        // to the end, unless rotation built fills of its own for it.
        if kept.choice.as_ref().is_some_and(|choice| {
            choice.exact_at_unix_ms <= now
                || plan.as_ref().is_some_and(|plan| {
                    plan.exact_at_unix_ms() == choice.exact_at_unix_ms && !plan.chosen_by_hand()
                })
        }) {
            kept.choice = None;
        }
        kept.plan = plan;
        if let Ok(mut slot) = self.kept.lock() {
            *slot = kept;
        }
        if picks.is_empty() && chained.is_some() {
            // Every track: the winner carries no match, and the queue is
            // deep enough.
            return Ok(());
        }
        if picks.is_empty() {
            // Tracks held back by their ratio moved: they play sooner.
            rotation::save_counters(&self.pool, self.station_id, &counters).await?;
            if library.tracks.is_empty() {
                tracing::debug!("autofill: no playable track in the library");
            } else {
                tracing::warn!(
                    "rotation: no slot of the clock found a track for now \
                     (restrictions, rules or an exact time), \
                     the engine's fallback directory takes over"
                );
            }
            self.rest();
            return Ok(());
        }
        // Every track: the announcement of the match behind the winner goes
        // over the winner's intro, right before it.
        let mut jingle_entry = None;
        if let Some(planned) = &planned_match
            && let (Some(announcement), Some(vote_entry)) =
                (&planned.announce_before, planned.vote_entry)
            && let Some(position) = state
                .waiting()
                .position(|entry| entry.entry_id == vote_entry)
                .and_then(|position| u32::try_from(position).ok())
        {
            let tracks = self
                .tracks(&[announcement.track], PlaySource::Rotation)
                .await?;
            match self
                .engine
                .enqueue(tracks, Some(position), Vec::new())
                .await
            {
                Ok(ids) => {
                    jingle_entry = ids.first().copied();
                    if let Some(entry_id) = jingle_entry {
                        let entry = (state.engine_started_at_unix_ms, entry_id);
                        rotation::record_pick(
                            &self.pool,
                            self.station_id,
                            announcement,
                            Some(entry),
                        )
                        .await?;
                    }
                }
                Err(error) => tracing::warn!(%error, "cannot queue the match's announcement"),
            }
        }
        let ids: Vec<u64> = picks.iter().map(|pick| pick.track).collect();
        let tracks = self.tracks(&ids, PlaySource::Rotation).await?;
        let (tracks, linked, firsts) = entries(&picks, tracks)?;
        let entry_ids = self.engine.enqueue(tracks, None, linked).await?;
        self.awaiting(entry_ids.iter().copied().chain(jingle_entry));
        self.queued_from(&ahead, &picks, &firsts, &entry_ids);
        let mut log_ids = Vec::new();
        // Each pick's first entry: a shortened medley queues two.
        let mut pick_entries = Vec::new();
        for (pick, index) in picks.iter().zip(firsts) {
            let Some(entry_id) = entry_ids.get(index) else {
                break;
            };
            pick_entries.push(*entry_id);
            let entry = (state.engine_started_at_unix_ms, *entry_id);
            log_ids
                .push(rotation::record_pick(&self.pool, self.station_id, pick, Some(entry)).await?);
        }
        if let Some(planned) = planned_match
            && let Err(error) = crate::matches::queued(
                &self.pool,
                self.station_id,
                &planned,
                &picks,
                state.engine_started_at_unix_ms,
                &pick_entries,
                &log_ids,
                jingle_entry,
            )
            .await
        {
            // Its tracks stay queued, as ordinary tracks.
            tracing::warn!(%error, "cannot record the match queued");
        }
        rotation::save_counters(&self.pool, self.station_id, &counters).await?;
        tracing::info!(
            tracks = ids.len(),
            "autofill: queued the tracks rotation picked"
        );
        metrics::counter!("heron_api_autofill_tracks_total").increment(ids.len() as u64);
        let relaxed = picks
            .iter()
            .filter(|pick| pick.explanation.step > 0)
            .count();
        metrics::counter!("heron_api_rotation_relaxed_total").increment(relaxed as u64);
        Ok(())
    }

    /// Stores what a pick before an exact time did (`docs/rotation.md#the-
    /// attempts-page`), and links its log to it. Returns the calculation
    /// the queue follows the fill of, if stored.
    async fn calculated(
        &self,
        kept: &mut Kept,
        calculation: &heron_rotation::Calculation,
        pick: &mut heron_rotation::Pick,
    ) -> Option<u64> {
        let exact_at = calculation.exact_at_unix_ms;
        let follows = kept
            .calculation
            .as_ref()
            .filter(|kept| kept.exact_at_ms == exact_at)
            .map(|kept| kept.id);
        let trigger = if calculation.stop == Stop::Followed {
            Trigger::Followed
        } else if calculation.hand_dropped {
            tracing::warn!(
                station = self.station_id,
                exact_at,
                "the attempt chosen by hand no longer lands before the exact time: \
                 rotation chooses by itself again"
            );
            kept.choice = None;
            Trigger::HandDropped
        } else if let Some(trigger) = kept.next_trigger {
            trigger
        } else if follows.is_some() {
            Trigger::QueueChanged
        } else {
            match exact_times::calculated_before(&self.pool, self.station_id, exact_at).await {
                Ok(true) => Trigger::Restart,
                Ok(false) => Trigger::First,
                Err(error) => {
                    tracing::warn!(%error, "cannot read the calculations of the exact time");
                    Trigger::First
                }
            }
        };
        let followed = trigger == Trigger::Followed;
        if !followed {
            kept.next_trigger = None;
        }
        let link = if followed { follows } else { None };
        let id = match exact_times::store(&self.pool, self.station_id, calculation, trigger, link)
            .await
        {
            Ok(id) => Some(id),
            Err(error) => {
                tracing::warn!(%error, "cannot store the exact time's calculation");
                None
            }
        };
        if let Some(lookahead) = pick.explanation.lookahead.as_mut() {
            lookahead.calculation_id = id;
        }
        if followed {
            return follows;
        }
        kept.calculation = id.map(|id| KeptCalculation {
            id,
            exact_at_ms: exact_at,
            cut_ms: calculation.window_cut_unix_ms,
            queued: Vec::new(),
            blocked: false,
        });
        id
    }

    /// Keeps the entries of the picks queued from the calculation the
    /// queue follows, to replace them with an attempt chosen by hand.
    fn queued_from(
        &self,
        planned: &[(std::ops::Range<usize>, u64)],
        picks: &[heron_rotation::Pick],
        firsts: &[usize],
        entry_ids: &[u64],
    ) {
        let Ok(mut kept) = self.kept.lock() else {
            return;
        };
        let Some(calculation) = kept.calculation.as_mut() else {
            return;
        };
        for (range, from) in planned {
            if *from != calculation.id {
                continue;
            }
            let entries_of = |index: usize| {
                let first = firsts.get(index).copied().unwrap_or(entry_ids.len());
                let end = firsts.get(index + 1).copied().unwrap_or(entry_ids.len());
                entry_ids.get(first..end).unwrap_or_default().to_vec()
            };
            calculation.queued.push(QueuedPick {
                entries: range.clone().flat_map(entries_of).collect(),
                tracks: picks
                    .get(range.clone())
                    .unwrap_or_default()
                    .iter()
                    .map(|pick| pick.track)
                    .collect(),
            });
        }
    }

    /// The calculation whose fill the queue follows.
    pub fn kept_calculation(&self) -> Option<u64> {
        self.kept
            .lock()
            .ok()
            .and_then(|kept| kept.calculation.as_ref().map(|calculation| calculation.id))
    }

    /// The attempt chosen by hand the queue follows.
    pub fn choice(&self) -> Option<HandChoice> {
        let now = rotation::now_ms();
        self.kept
            .lock()
            .ok()
            .and_then(|kept| kept.choice.clone())
            .filter(|choice| choice.exact_at_unix_ms > now)
    }

    /// Uses `attempt` of calculation `id` (before the exact time at
    /// `exact_at_ms`) instead of the fill kept: the picks queued from the
    /// calculation where they part from it are taken out, and the next
    /// picks follow it. `409` when the queue no longer follows the
    /// calculation, or has moved on since.
    pub async fn use_attempt(
        &self,
        id: u64,
        exact_at_ms: i64,
        attempt: &heron_rotation::Attempt,
        user: &crate::auth::CurrentUser,
    ) -> ApiResult<()> {
        let _busy = self.busy.lock().await;
        let conflict = |reason: &str| Err(ApiError::Conflict(reason.to_owned()));
        let queued = {
            let kept = self
                .kept
                .lock()
                .map_err(|_| ApiError::Internal("the queue manager's state".to_owned()))?;
            match kept.calculation.as_ref() {
                Some(calculation) if calculation.id == id && calculation.blocked => {
                    return conflict("a match is planned with its tracks: they cannot change");
                }
                Some(calculation) if calculation.id == id => calculation.queued.clone(),
                _ => {
                    return conflict(
                        "the queue no longer follows this calculation: \
                         a newer one replaced it, or the API restarted",
                    );
                }
            }
        };
        let state = self.engine.fetch_state().await?;
        let waiting: Vec<u64> = state.waiting().map(|entry| entry.entry_id).collect();
        // Nothing queued after its picks: neither the exact slot, nor a
        // track added at the end.
        let last = queued.last().and_then(|pick| pick.entries.last());
        if last.is_none() || waiting.last() != last {
            return conflict("the queue has moved on since this calculation");
        }
        let picks = exact_times::attempt_picks(attempt);
        let same = queued
            .iter()
            .zip(&picks)
            .take_while(|(queued, pick)| queued.tracks == **pick)
            .count();
        let out: Vec<u64> = queued[same..]
            .iter()
            .flat_map(|pick| pick.entries.iter().copied())
            .collect();
        if out.iter().any(|entry| !waiting.contains(entry)) {
            return conflict("a track queued from this calculation has started since");
        }
        // A track first: its linked jingle goes with it.
        for entry in out.iter().rev() {
            match self.engine.remove(*entry).await {
                Ok(()) | Err(ApiError::Invalid(_) | ApiError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        if !out.is_empty() {
            tracing::info!(
                station = self.station_id,
                removed = out.len(),
                "exact time: the tracks the attempt chosen by hand replaces taken out"
            );
        }
        if let Ok(mut kept) = self.kept.lock() {
            if let Some(calculation) = kept.calculation.as_mut() {
                calculation.queued.truncate(same);
            }
            kept.plan = Some(heron_rotation::Plan::chosen(
                exact_at_ms,
                attempt,
                u32::try_from(same).unwrap_or(u32::MAX),
            ));
            kept.choice = Some(HandChoice {
                calculation_id: id,
                attempt: attempt.number,
                exact_at_unix_ms: exact_at_ms,
                user_id: user.id,
                username: user.username.clone(),
                at_unix_ms: rotation::now_ms(),
                ending: attempt.ending,
            });
            kept.next_trigger = None;
        }
        self.wake();
        Ok(())
    }

    /// Entries just queued: the end of the queue before the exact time is
    /// taken again once the engine's state has them.
    fn awaiting(&self, entries: impl IntoIterator<Item = u64>) {
        if let Ok(mut watch) = self.watch.lock() {
            watch.awaiting.extend(entries);
            watch.baseline = None;
            watch.pending = None;
        }
    }

    /// Before an exact time, calculates again once the end of the queue
    /// before it has moved and settled (`docs/rotation.md#exact-times`).
    async fn watch_queue(&self, state: &PlayerState) -> ApiResult<()> {
        let now = rotation::now_ms();
        let Some((id, exact_at, cut)) = self.kept.lock().ok().and_then(|kept| {
            kept.calculation
                .as_ref()
                .map(|calculation| (calculation.id, calculation.exact_at_ms, calculation.cut_ms))
        }) else {
            return Ok(());
        };
        if exact_at <= now || state.paused {
            if let Ok(mut watch) = self.watch.lock() {
                watch.pending = None;
            }
            return Ok(());
        }
        let alternatives = crate::matches::alternatives(&self.pool, self.station_id, state)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "cannot read the match's tracks");
                None
            });
        let end = end_before_exact(state, now, alternatives, cut);
        let due = {
            let Ok(mut watch) = self.watch.lock() else {
                return Ok(());
            };
            if watch.calculation != Some(id) {
                watch.calculation = Some(id);
                watch.baseline = None;
                watch.pending = None;
            }
            // Engine entry IDs only grow: a state with a newer entry is
            // newer than the entries queued before it.
            let newest = state
                .waiting()
                .chain(state.decks.iter().filter_map(|deck| deck.entry.as_ref()))
                .map(|entry| entry.entry_id)
                .max()
                .unwrap_or(0);
            watch.awaiting.retain(|entry| *entry > newest);
            match end {
                _ if !watch.awaiting.is_empty() => false,
                None => {
                    watch.pending = None;
                    false
                }
                Some(end) => match (watch.baseline, watch.pending) {
                    (None, _) => {
                        watch.baseline = Some(end);
                        watch.pending = None;
                        false
                    }
                    (Some(baseline), _) if (end - baseline).abs() <= MOVED_MS => {
                        watch.pending = None;
                        false
                    }
                    (Some(_), Some((at, seen))) if (end - seen).abs() <= MOVED_MS => {
                        Instant::now() >= at
                    }
                    (Some(_), _) => {
                        watch.pending = Some((Instant::now() + SETTLE, end));
                        false
                    }
                },
            }
        };
        if !due {
            return Ok(());
        }
        let result = self.replace(Trigger::QueueChanged, None).await;
        if let Ok(mut watch) = self.watch.lock() {
            // The queue as it now is, whatever happened.
            watch.baseline = None;
            watch.pending = None;
        }
        match result {
            Ok(_) => Ok(()),
            Err(ApiError::Conflict(reason)) => {
                tracing::info!(
                    station = self.station_id,
                    exact_at,
                    %reason,
                    "exact time: the queue changed, its picks cannot be replaced"
                );
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Calculates the exact time the queue follows a fill of again, now:
    /// the picks queued from the fill kept that wait (not the cued one),
    /// the exact slot's and those after it are taken out, and the queue is
    /// filled again from where it ends with new fills, as many picks as
    /// were taken out (`docs/rotation.md#exact-times`). Tracks queued by
    /// hand, requests, a show's items and a match's tracks stay. Returns
    /// the new calculation; `409` when there is none to redo, nothing can
    /// be replaced, or `exact_at` is not the exact time it plans for.
    pub async fn replace(&self, trigger: Trigger, exact_at: Option<i64>) -> ApiResult<u64> {
        let _busy = self.busy.lock().await;
        let conflict = |reason: &str| ApiError::Conflict(reason.to_owned());
        let now = rotation::now_ms();
        let calculation = self
            .kept
            .lock()
            .ok()
            .and_then(|kept| kept.calculation.clone())
            .filter(|calculation| calculation.exact_at_ms > now)
            .ok_or_else(|| {
                conflict(
                    "the queue follows no calculation for an exact time ahead: \
                     there is none, or the API restarted",
                )
            })?;
        if exact_at.is_some_and(|at| at != calculation.exact_at_ms) {
            return Err(conflict(
                "only the next exact time, the one the queue plans for, can be calculated again",
            ));
        }
        let settings: QueueSettings =
            settings::load_for(&self.pool, self.station_id, QueueSettings::SETTINGS_NAME).await?;
        if !settings.autofill {
            return Err(conflict(
                "autofill is off: nothing would fill the queue again",
            ));
        }
        if self.idle() {
            return Err(conflict("rotation could pick nothing a moment ago"));
        }
        if crate::matches::holds_queue(&self.pool, self.station_id).await? {
            return Err(conflict(
                "a match is waiting or open: nothing goes behind it",
            ));
        }
        let state = self.engine.fetch_state().await?;
        let queue = &state.queue;
        let rows = self.picks_of(&state).await?;
        let kept_of = |entry: u64| {
            calculation
                .queued
                .iter()
                .position(|pick| pick.entries.contains(&entry))
        };
        let exact = exact_index(queue, calculation.cut_ms);
        let end = exact.unwrap_or(queue.len());
        // After the exact slot, rotation's picks only: they go with it.
        if let Some(index) = exact {
            let foreign = queue[index..].iter().any(|entry| {
                !(rows.contains(&entry.entry_id)
                    || kept_of(entry.entry_id).is_some()
                    || entry.track.continues)
            });
            if foreign {
                return Err(conflict(
                    "something rotation did not pick is queued after the exact slot",
                ));
            }
        }
        // Before it: the picks of the fill kept that only picks of it and
        // tracks queued by hand follow.
        let mut from = end;
        while from > 0 {
            let entry = &queue[from - 1];
            let stays = matches!(entry.track.source, PlaySource::Manual | PlaySource::Request);
            if !stays && kept_of(entry.entry_id).is_none() {
                break;
            }
            from -= 1;
        }
        let range: Vec<u64> = queue[from..end]
            .iter()
            .map(|entry| entry.entry_id)
            .collect();
        // Whole picks only: one with an entry cued stays.
        let picks: Vec<usize> = (0..calculation.queued.len())
            .filter(|index| {
                calculation.queued[*index]
                    .entries
                    .iter()
                    .all(|entry| range.contains(entry))
            })
            .collect();
        let mut out: Vec<u64> = queue[from..end]
            .iter()
            .map(|entry| entry.entry_id)
            .filter(|entry| kept_of(*entry).is_some_and(|index| picks.contains(&index)))
            .collect();
        if let Some(index) = exact {
            out.extend(queue[index..].iter().map(|entry| entry.entry_id));
        }
        if out.is_empty() {
            return Err(conflict(
                "the picks queued from the fill kept are on air or cued: none to replace",
            ));
        }
        let refill = picks.len() + usize::from(exact.is_some());
        let previous = calculation.id;
        // From now on the queue no longer follows it: none of its attempts
        // can be chosen while the new fills are built.
        if let Ok(mut kept) = self.kept.lock() {
            let by_hand = kept.choice.is_some()
                || kept
                    .plan
                    .as_ref()
                    .is_some_and(heron_rotation::Plan::chosen_by_hand);
            if by_hand && trigger == Trigger::QueueChanged {
                tracing::warn!(
                    station = self.station_id,
                    exact_at = calculation.exact_at_ms,
                    "the queue changed: the attempt chosen by hand is dropped, \
                     rotation chooses by itself again"
                );
            }
            kept.plan = None;
            kept.choice = None;
            kept.calculation = None;
            kept.next_trigger = Some(if by_hand && trigger == Trigger::QueueChanged {
                Trigger::HandDropped
            } else {
                trigger
            });
        }
        // From the end: a track first, its linked jingle goes with it.
        let mut removed = Vec::new();
        for entry in out.iter().rev() {
            match self.engine.remove(*entry).await {
                Ok(()) | Err(ApiError::NotFound(_)) => removed.push(*entry),
                // Started meanwhile: it plays.
                Err(ApiError::Invalid(_)) => {}
                Err(error) => return Err(error),
            }
        }
        // They never play: the rotation log forgets them, and the clock
        // goes on from the pick before them.
        if !removed.is_empty() {
            let mut forget = QueryBuilder::new("DELETE FROM rotation_picks WHERE station_id = ");
            forget
                .push_bind(self.station_id)
                .push(" AND engine_started_at = ")
                .push_bind(state.engine_started_at_unix_ms)
                .push(" AND entry_id IN (");
            let mut ids = forget.separated(", ");
            for entry in &removed {
                ids.push_bind(*entry);
            }
            forget.push(")");
            forget.build().execute(&self.pool).await?;
        }
        tracing::info!(
            station = self.station_id,
            exact_at = calculation.exact_at_ms,
            removed = removed.len(),
            trigger = ?trigger,
            "exact time: calculated again, the picks queued from the fill kept replaced"
        );
        let state = self.engine.fetch_state().await?;
        let filled = self.fill_locked(&state, refill).await;
        let latest = self.kept.lock().ok().and_then(|mut kept| {
            // Not consumed: no pick was made before the exact time.
            kept.next_trigger = None;
            kept.calculation.as_ref().map(|calculation| calculation.id)
        });
        filled?;
        match latest {
            Some(id) if id != previous => Ok(id),
            _ => Err(conflict(
                "the queue reaches the exact time's window already: no fill to build",
            )),
        }
    }

    /// The waiting entries rotation picked, by their rotation log.
    async fn picks_of(&self, state: &PlayerState) -> ApiResult<Vec<u64>> {
        let entries: Vec<u64> = state.queue.iter().map(|entry| entry.entry_id).collect();
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let mut select =
            QueryBuilder::new("SELECT entry_id FROM rotation_picks WHERE station_id = ");
        select
            .push_bind(self.station_id)
            .push(" AND engine_started_at = ")
            .push_bind(state.engine_started_at_unix_ms)
            .push(" AND entry_id IN (");
        let mut ids = select.separated(", ");
        for entry in &entries {
            ids.push_bind(*entry);
        }
        select.push(")");
        let rows: Vec<(u64,)> = select.build_query_as().fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|(entry,)| entry).collect())
    }

    /// Drops the attempt chosen by hand: the next pick builds new fills.
    /// False when there was none.
    pub async fn back_to_automatic(&self) -> bool {
        let _busy = self.busy.lock().await;
        let dropped = match self.kept.lock() {
            Ok(mut kept) => {
                let by_hand = kept
                    .plan
                    .as_ref()
                    .is_some_and(heron_rotation::Plan::chosen_by_hand);
                // Followed to its end, the choice may have no fill left.
                let dropped = by_hand || kept.choice.is_some();
                if by_hand {
                    kept.plan = None;
                    kept.next_trigger = Some(Trigger::Automatic);
                }
                kept.choice = None;
                dropped
            }
            Err(_) => false,
        };
        if dropped {
            self.wake();
        }
        dropped
    }

    /// Queues what the Top 30's show has to play for a track planned at
    /// `at` (`docs/listener-interaction.md#top-30`): its programme's next
    /// items, each jingle linked to its track. False when the show has
    /// nothing to play: rotation picks.
    async fn fill_show(&self, at: i64, missing: usize) -> ApiResult<bool> {
        let Some(due) = crate::top30::due(&self.pool, self.station_id, at, missing).await? else {
            return Ok(false);
        };
        let ids: Vec<u64> = due.items.iter().map(|item| item.track_id).collect();
        let rows = self.rows(&ids).await?;
        let plays = |id: u64| rows.iter().any(|row| row.id == id && row.plays());
        // What can no longer play is passed over, a track with its jingle.
        let mut items: Vec<&crate::top30::Item> = Vec::new();
        for item in &due.items {
            if plays(item.track_id) {
                items.push(item);
            } else if item.kind != crate::top30::ItemKind::Jingle
                && items
                    .last()
                    .is_some_and(|last| last.kind == crate::top30::ItemKind::Jingle)
            {
                items.pop();
            }
        }
        if !items.is_empty() {
            let ids: Vec<u64> = items.iter().map(|item| item.track_id).collect();
            let tracks = self.tracks(&ids, PlaySource::Rotation).await?;
            let linked = items
                .windows(2)
                .enumerate()
                .filter(|(_, pair)| {
                    pair[0].kind == crate::top30::ItemKind::Jingle
                        && pair[1].kind != crate::top30::ItemKind::Jingle
                })
                .map(|(index, _)| u32::try_from(index).unwrap_or(u32::MAX))
                .collect();
            let entry_ids = self.engine.enqueue(tracks, None, linked).await?;
            self.awaiting(entry_ids);
            tracing::info!(tracks = ids.len(), "top 30: queued the show's next tracks");
        }
        crate::top30::queued(
            &self.pool,
            self.station_id,
            due.opens_ms,
            due.from,
            due.next,
        )
        .await?;
        Ok(true)
    }

    /// Before a soft exact time, takes out the tracks queued by hand that
    /// would end after its window, and any track that would start after
    /// it: they no longer fit. In hard mode the engine cuts them.
    async fn trim_before_exact(&self, state: &PlayerState) -> ApiResult<()> {
        let now = rotation::now_ms();
        if !state
            .waiting()
            .any(|entry| entry.track.start_at_unix_ms.is_some())
        {
            return Ok(());
        }
        let alternatives = crate::matches::alternatives(&self.pool, self.station_id, state).await?;
        let (planned, _) = rotation::planned_starts(state, now, alternatives);
        let Some(timed) = planned
            .iter()
            .position(|(entry, _)| entry.track.start_at_unix_ms.is_some())
        else {
            return Ok(());
        };
        let (exact, _) = planned[timed];
        let Some(cut_at) = exact.track.start_at_unix_ms else {
            return Ok(());
        };
        let mode: Option<String> = sqlx::query_scalar(
            "SELECT JSON_VALUE(explanation, '$.exact.mode') FROM rotation_picks
             WHERE station_id = ? AND engine_started_at = ? AND entry_id = ?",
        )
        .bind(self.station_id)
        .bind(state.engine_started_at_unix_ms)
        .bind(exact.entry_id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        if mode.as_deref() != Some("soft") {
            return Ok(());
        }
        for (entry, start) in &planned[..timed] {
            let end = start + rotation::planned_length_ms(entry);
            let by_hand = matches!(entry.track.source, PlaySource::Manual | PlaySource::Request);
            // The exact slot's own linked jingle goes with it.
            if entry.linked_to == Some(exact.entry_id) {
                continue;
            }
            if *start >= cut_at || (by_hand && end > cut_at) {
                match self.engine.remove(entry.entry_id).await {
                    Ok(()) => tracing::info!(
                        entry = entry.entry_id,
                        path = %entry.track.path,
                        "no longer fits before the exact time: removed from the queue"
                    ),
                    Err(ApiError::Invalid(_) | ApiError::NotFound(_)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    /// Updates queued tracks whose file moved, and takes out those whose
    /// file is gone.
    async fn follow_library(&self, state: &PlayerState) -> ApiResult<()> {
        let ids: Vec<u64> = state
            .waiting()
            .filter_map(|entry| entry.track.track_id)
            .collect();
        let rows = self.rows(&ids).await?;
        for entry in state.waiting() {
            let Some(track_id) = entry.track.track_id else {
                continue;
            };
            let row = rows.iter().find(|row| row.id == track_id);
            let unchanged = |row: &TrackRow| {
                let playout = row.playout();
                // 0 and 0: the whole file, whose length the engine finds out.
                let whole = playout.cue_in_ms == 0 && playout.cue_out_ms == 0;
                row.path == entry.track.path
                    && row.intro_ms == entry.track.intro_ms
                    && row.over_intro == entry.track.over_intro
                    && (whole
                        || (playout.cue_in_ms == entry.track.cue_in_ms
                            && playout.cue_out_ms == entry.track.cue_out_ms))
            };
            // A shortened medley's spans have cue points of their own.
            let span = |row: &TrackRow| row.medley && row.path == entry.track.path;
            match row {
                Some(row) if row.plays() && (unchanged(row) || span(row)) => {}
                Some(row) if row.plays() => {
                    let mut track = row.to_proto(entry.track.source)?;
                    if let Some(start_at) = entry.track.start_at_unix_ms {
                        track.start_at_unix_ms = Some(start_at);
                        track.cut_fade_ms = heron_rotation::exact::CUT_FADE_MS;
                    }
                    match self.engine.replace(entry.entry_id, track).await {
                        Ok(()) => tracing::info!(
                            entry = entry.entry_id,
                            from = %entry.track.path,
                            to = %row.path,
                            "queued track updated: moved, or its cue points or intro changed"
                        ),
                        // It started meanwhile, or was removed.
                        Err(ApiError::Invalid(_) | ApiError::NotFound(_)) => {}
                        Err(error) => return Err(error),
                    }
                }
                _ => match self.engine.remove(entry.entry_id).await {
                    Ok(()) => tracing::warn!(
                        entry = entry.entry_id,
                        path = %entry.track.path,
                        "queued track left the library, was disabled (everywhere or on this \
                         station) or is a medley turned off: \
                         removed from the queue"
                    ),
                    Err(ApiError::Invalid(_) | ApiError::NotFound(_)) => {}
                    Err(error) => return Err(error),
                },
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> TrackRow {
        TrackRow {
            id: 7,
            path: "A/B/01 C.flac".to_owned(),
            title: Some("C".to_owned()),
            artist: Some("A".to_owned()),
            album: Some("B".to_owned()),
            duration_ms: Some(200_000),
            enabled: true,
            missing: false,
            analyzed: true,
            cue_in_ms: Some(150),
            cue_out_ms: Some(198_000),
            fade_out_start_ms: Some(196_000),
            integrated_lufs: Some(-11.0),
            true_peak_db: Some(-0.5),
            override_cue_in_ms: None,
            override_cue_out_ms: None,
            gap_killer_enabled: None,
            crossfade: None,
            no_metadata: false,
            over_intro: false,
            intro_ms: None,
            medley: false,
            medleys_off: false,
            disabled_here: false,
        }
    }

    #[test]
    fn analyzed_tracks_carry_their_cue_points() {
        let track = row().to_proto(PlaySource::Rotation).unwrap();
        assert_eq!((track.cue_in_ms, track.cue_out_ms), (150, 198_000));
        assert_eq!(track.mix_point_ms, Some(196_000));
        assert_eq!(track.source(), pb::PlaySource::Rotation);
        assert!(track.crossfade_overrides_json.is_empty());
    }

    #[test]
    fn overrides_apply() {
        let mut row = row();
        row.gap_killer_enabled = Some(false);
        row.override_cue_out_ms = Some(190_000);
        row.crossfade = Some(r#"{"crossfade": false}"#.to_owned());
        let track = row.to_proto(PlaySource::Manual).unwrap();
        assert_eq!((track.cue_in_ms, track.cue_out_ms), (0, 190_000));
        assert_eq!(
            serde_json::from_str::<CrossfadeOverrides>(&track.crossfade_overrides_json)
                .unwrap()
                .crossfade,
            Some(false)
        );
    }

    #[test]
    fn unanalyzed_tracks_play_whole() {
        let mut unanalyzed = row();
        unanalyzed.analyzed = false;
        let track = unanalyzed.to_proto(PlaySource::Manual).unwrap();
        assert_eq!((track.cue_in_ms, track.cue_out_ms), (0, 200_000));
        unanalyzed.duration_ms = None;
        let track = unanalyzed.to_proto(PlaySource::Manual).unwrap();
        // 0 and 0: the engine plays the whole file.
        assert_eq!((track.cue_in_ms, track.cue_out_ms), (0, 0));
    }

    #[test]
    fn missing_silent_and_disabled_tracks_are_refused() {
        let mut missing = row();
        missing.missing = true;
        assert!(missing.unplayable().is_some());
        let mut disabled = row();
        disabled.enabled = false;
        assert!(
            disabled
                .unplayable()
                .is_some_and(|reason| reason.contains("disabled"))
        );
        let mut silent = row();
        silent.cue_out_ms = Some(150);
        assert!(silent.unplayable().is_some());
        assert!(row().unplayable().is_none());
        let mut medley = row();
        medley.medley = true;
        assert!(medley.unplayable().is_none() && medley.plays());
        medley.medleys_off = true;
        assert!(medley.unplayable().is_some() && !medley.plays());
        let mut elsewhere = row();
        elsewhere.disabled_here = true;
        assert!(!elsewhere.plays());
        assert!(
            elsewhere
                .unplayable()
                .is_some_and(|reason| reason.contains("disabled on this station"))
        );
    }

    fn pick(track: u64) -> heron_rotation::Pick {
        let library = heron_rotation::Library::new(
            heron_rotation::LibraryParts {
                tracks: vec![heron_rotation::Track {
                    id: track,
                    title_key: None,
                    album_key: None,
                    artists: vec![],
                    categories: vec![],
                    length_ms: 46_000,
                    mix_ms: None,
                    ratio: 10,
                    dayparts: None,
                    daypart_days: None,
                }],
                ..heron_rotation::LibraryParts::default()
            },
            jiff::tz::TimeZone::UTC,
        );
        let mut rng = heron_rotation::Rng::new(1);
        heron_rotation::next(
            &library,
            &heron_rotation::History::default(),
            0,
            None,
            &mut rng,
        )
        .unwrap()
    }

    #[test]
    fn a_shortened_medley_is_queued_as_two_linked_spans() {
        let mut medley = pick(7);
        medley.medley_cut = Some(heron_rotation::MedleyCut {
            hooks_left_out: 1,
            parts_left_out: vec![4, 5],
            length_ms: 31_000,
            first_to_ms: 25_000,
            second_from_ms: 37_500,
            crossfade_ms: 500,
        });
        let mut news = pick(8);
        news.start_at_ms = Some(3_600_000);
        let track = row().to_proto(PlaySource::Rotation).unwrap();
        let (tracks, linked, firsts) =
            entries(&[medley, news], vec![track.clone(), track]).unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!((linked, firsts), (vec![0], vec![0, 2]));
        assert_eq!(tracks[0].cue_out_ms, 25_000);
        assert!(
            tracks[0]
                .crossfade_overrides_json
                .contains("\"crossfade\":true")
        );
        assert_eq!(tracks[1].cue_in_ms, 37_500);
        assert!(tracks[1].continues && !tracks[0].continues);
        assert_eq!(tracks[2].start_at_unix_ms, Some(3_600_000));
        assert_eq!(tracks[2].cut_fade_ms, heron_rotation::exact::CUT_FADE_MS);
    }
}
