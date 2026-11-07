// SPDX-License-Identifier: AGPL-3.0-or-later

//! The queue manager: feeds the engine with library tracks.
//!
//! - Builds the tracks the engine plays from the database: path, tags, cue
//!   points and crossfade point from the analysis, per-track overrides.
//! - Keeps the queue from running dry (`queue.autofill`) with the tracks
//!   rotation picks (`heron-rotation`), each recorded in the rotation log;
//!   a jingle of a linked clock slot is queued linked to its track.
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
use heron_rotation::{Play, Rng};
use sqlx::{MySqlPool, QueryBuilder};
use tokio::sync::Notify;

use crate::engine::{EngineLink, PlayerState, source_to_proto};
use crate::error::{ApiError, ApiResult};
use crate::rotation::{self, SILENT_MS};
use crate::settings::{self, QueueSettings};

const TICK: Duration = Duration::from_secs(2);
const FOLLOW_INTERVAL: Duration = Duration::from_secs(5);

/// Wait before trying again when rotation could pick nothing.
const NOTHING_TO_PICK: Duration = Duration::from_secs(30);

pub struct QueueManager {
    pool: MySqlPool,
    engine: Arc<EngineLink>,
    station_id: u64,
    /// The other stations, for the rules across stations.
    stations: std::sync::Weak<crate::stations::Stations>,
    wake: Notify,
    /// Rotation could pick nothing: no new try before then.
    idle_until: Mutex<Option<Instant>>,
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
        let settings: QueueSettings =
            settings::load_for(&self.pool, self.station_id, QueueSettings::SETTINGS_NAME).await?;
        // A cued fallback track gives way to anything queued.
        let waiting = state
            .waiting()
            .filter(|entry| entry.track.source != PlaySource::Fallback)
            .count();
        let missing = (settings.min_queued as usize).saturating_sub(waiting);
        if !settings.autofill || missing == 0 {
            return Ok(());
        }
        let now = rotation::now_ms();
        // A show on air (the Top 30) queues its programme in its hours.
        let (_, planned_end) = rotation::planned_starts(state, now);
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
        let (planned, mut at) = rotation::planned_starts(state, now);
        let mut history = rotation::with_queue(&self.pool, &library, history, &planned).await?;
        let mut last = rotation::last_position(&self.pool, self.station_id).await?;
        let mut rng = Rng::new(rotation::seed());
        let mut picks = Vec::new();
        // Play ratio counters moved by these picks, saved once queued.
        let mut counters = Vec::new();
        // A match in the show's hours: its tracks after one of the picks.
        let mut planner = crate::matches::planner(&self.pool, self.station_id).await?;
        let mut planned_match = None;
        'picking: for _ in 0..missing {
            let outcome =
                heron_rotation::next_outcome(&library, &history, at, last.as_ref(), &mut rng);
            counters.extend(library.count_down(outcome.offered()));
            let Some(pick) = outcome.pick else {
                break;
            };
            let group = pick.into_picks();
            if group
                .iter()
                .any(|pick| !library.tracks.contains_key(&pick.track))
            {
                break;
            }
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
            if let Some(planner) = planner.as_mut()
                && let Some(planned) =
                    planner.after(&library, &history, at, &mut picks, &mut rng)
            {
                planned_match = Some(planned);
                break;
            }
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
        let ids: Vec<u64> = picks.iter().map(|pick| pick.track).collect();
        let tracks = self.tracks(&ids, PlaySource::Rotation).await?;
        let (tracks, linked, firsts) = entries(&picks, tracks)?;
        let entry_ids = self.engine.enqueue(tracks, None, linked).await?;
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
        if let Some(planned) = planned_match {
            crate::matches::queued(
                &self.pool,
                self.station_id,
                &planned,
                &picks,
                state.engine_started_at_unix_ms,
                &pick_entries,
                &log_ids,
            )
            .await?;
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
            self.engine.enqueue(tracks, None, linked).await?;
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
        let (planned, _) = rotation::planned_starts(state, now);
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
