// SPDX-License-Identifier: AGPL-3.0-or-later

//! The queue manager: feeds the engine with library tracks.
//!
//! - Builds the tracks the engine plays from the database: path, tags, cue
//!   points and crossfade point from the analysis, per-track overrides.
//! - Keeps the queue from running dry (`queue.autofill`) with the tracks
//!   rotation picks (`heron-rotation`), each recorded in the selection log.
//! - Follows the library: queued tracks whose file the librarian moved, or
//!   whose cue points were changed, are updated; tracks whose file
//!   disappeared are taken out of the queue.
//!
//! The engine holds the queue: when the API is down it plays what it has.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use heron_core::playout::CrossfadeOverrides;
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
}

const TRACK_COLUMNS: &str = "
    t.id, t.path, t.title, t.artist, t.album, t.duration_ms,
    t.missing_since IS NOT NULL AS missing,
    a.track_id IS NOT NULL AS analyzed,
    a.cue_in_ms, a.cue_out_ms, a.fade_out_start_ms, a.integrated_lufs, a.true_peak_db,
    o.cue_in_ms AS override_cue_in_ms, o.cue_out_ms AS override_cue_out_ms,
    o.gap_killer_enabled, CAST(o.crossfade AS CHAR) AS crossfade,
    EXISTS (SELECT 1 FROM track_categories tc
            JOIN categories c ON c.id = tc.category_id
            JOIN category_types ct ON ct.id = c.type_id
            WHERE tc.track_id = t.id AND NOT ct.sends_metadata) AS no_metadata
    FROM tracks t
    LEFT JOIN track_analysis a ON a.track_id = t.id
    LEFT JOIN track_overrides o ON o.track_id = t.id";

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
                crossfade: self.overrides().crossfade,
                ..base
            };
        }
        base.with_overrides(&self.overrides())
    }

    /// Why the track cannot be played, if it cannot.
    fn unplayable(&self) -> Option<String> {
        if self.missing {
            return Some(format!("track {}: its file is missing", self.id));
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
        })
    }
}

impl QueueManager {
    pub fn new(pool: MySqlPool, engine: Arc<EngineLink>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            engine,
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
        Ok(query.build_query_as().fetch_all(&self.pool).await?)
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

    /// How a library track plays: analysis and overrides combined.
    pub async fn playout(&self, id: u64) -> ApiResult<TrackPlayout> {
        let rows = self.rows(&[id]).await?;
        let row = rows
            .first()
            .ok_or_else(|| ApiError::NotFound(format!("track {id} not found")))?;
        Ok(row.playout())
    }

    pub async fn run(self: Arc<Self>) {
        let mut engine = self.engine.state();
        let mut last_follow = Instant::now();
        loop {
            let state = engine.borrow_and_update().clone();
            if let Some(state) = state {
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
            settings::load(&self.pool, QueueSettings::SETTINGS_NAME).await?;
        // A cued fallback track gives way to anything queued.
        let waiting = state
            .waiting()
            .filter(|entry| entry.track.source != PlaySource::Fallback)
            .count();
        let missing = (settings.min_queued as usize).saturating_sub(waiting);
        if !settings.autofill || missing == 0 || self.idle() {
            return Ok(());
        }
        let library = rotation::load_library(&self.pool).await?;
        let now = rotation::now_ms();
        let history = rotation::load_history(&self.pool, &library, now).await?;
        let (planned, mut at) = rotation::planned_starts(state, now);
        let mut history = rotation::with_queue(&self.pool, &library, history, &planned).await?;
        let mut last = rotation::last_position(&self.pool).await?;
        let mut rng = Rng::new(rotation::seed());
        let mut picks = Vec::new();
        for _ in 0..missing {
            let Some(pick) = heron_rotation::next(&library, &history, at, last.as_ref(), &mut rng)
            else {
                break;
            };
            let Some(track) = library.tracks.get(&pick.track) else {
                break;
            };
            history.push(Play::of(track, at));
            at += i64::from(track.length_ms);
            if pick.position.is_some() {
                last = pick.position;
            }
            picks.push(pick);
        }
        if picks.is_empty() {
            if library.tracks.is_empty() {
                tracing::debug!("autofill: no playable track in the library");
            } else {
                tracing::warn!(
                    "rotation: restrictions forbid every track for now, \
                     the engine's fallback directory takes over"
                );
            }
            self.rest();
            return Ok(());
        }
        let ids: Vec<u64> = picks.iter().map(|pick| pick.track).collect();
        let tracks = self.tracks(&ids, PlaySource::Rotation).await?;
        let entry_ids = self.engine.enqueue(tracks, None).await?;
        for (pick, entry_id) in picks.iter().zip(&entry_ids) {
            let entry = (state.engine_started_at_unix_ms, *entry_id);
            rotation::record_pick(&self.pool, pick, Some(entry)).await?;
        }
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
                    && (whole
                        || (playout.cue_in_ms == entry.track.cue_in_ms
                            && playout.cue_out_ms == entry.track.cue_out_ms))
            };
            match row {
                Some(row) if !row.missing && unchanged(row) => {}
                Some(row) if !row.missing => {
                    let track = row.to_proto(entry.track.source)?;
                    match self.engine.replace(entry.entry_id, track).await {
                        Ok(()) => tracing::info!(
                            entry = entry.entry_id,
                            from = %entry.track.path,
                            to = %row.path,
                            "queued track updated: moved, or its cue points changed"
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
                        "queued track left the library: removed from the queue"
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
    fn missing_and_silent_tracks_are_refused() {
        let mut missing = row();
        missing.missing = true;
        assert!(missing.unplayable().is_some());
        let mut silent = row();
        silent.cue_out_ms = Some(150);
        assert!(silent.unplayable().is_some());
        assert!(row().unplayable().is_none());
    }
}
