// SPDX-License-Identifier: AGPL-3.0-or-later

//! The library as seen through the API: browsing, searching, sorting and
//! filtering tracks, their details, analysis, overrides and categories,
//! and transition previews.
//!
//! The API never writes files: tags and moves belong to `heron-librarian`.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use heron_core::library::TagField;
use heron_core::playout::CrossfadeOverrides;
use heron_core::{LibraryPath, PlayoutSettings, TrackOverrides, plan_transition};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, QueryBuilder, Row};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::{self, ProgrammingSettings};
use crate::stations::Station;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_tracks))
        .routes(routes!(get_track, patch_track))
        .routes(routes!(put_overrides, delete_overrides))
        .routes(routes!(put_intro))
        .routes(routes!(put_hook))
        .routes(routes!(put_categories))
        .routes(routes!(preview_transition))
        .routes(routes!(get_waveform))
        .routes(routes!(get_audio))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackFilter {
    /// Present, enabled, analyzed and audible: what autofill picks from.
    Playable,
    Missing,
    /// Not analyzed yet (or analysis failed).
    Unanalyzed,
    /// Analyzed, with nothing audible.
    Silent,
    Disabled,
    /// Identification not confident enough, waiting for review.
    NeedsReview,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrackSort {
    /// Artist, album, then path.
    #[default]
    Artist,
    Title,
    Album,
    /// Most recently added first.
    Added,
    /// Least recently played first, never played before all.
    LastPlayed,
    /// Most played first.
    PlayCount,
    /// Lowest bitrate first.
    Bitrate,
    /// Shortest first.
    Duration,
    /// Year of first release (MusicBrainz, else the year tag), oldest first.
    Year,
    /// Slowest first.
    Bpm,
    /// In the order of the mood scale.
    Mood,
    /// In the order of the sound code scale.
    SoundCode,
    Genre,
    /// Quietest first: the lowest level between the cue points.
    LevelMin,
    /// Quietest first: the highest level between the cue points.
    LevelMax,
}

/// Which tracks: the filters of `GET /tracks`, also used to select tracks
/// for the actions on a selection. Lists of IDs are comma-separated.
#[derive(Clone, Debug, Default, Serialize, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct TrackQuery {
    /// Words found in the title, artist, album, album artist, genre or path.
    pub q: Option<String>,
    pub category_id: Option<u64>,
    #[param(inline)]
    pub filter: Option<TrackFilter>,
    /// Tracks with every one of these labels (IDs, comma-separated).
    pub labels_all: Option<String>,
    /// Tracks with at least one of these labels.
    pub labels_any: Option<String>,
    /// Tracks with none of these labels.
    pub labels_none: Option<String>,
    /// Count plays on this station only: play counts, last played, their
    /// sort, `never_played` and `not_played_days`. Every station's when
    /// left out.
    pub station_id: Option<u64>,
    /// Tracks never played.
    pub never_played: Option<bool>,
    /// Tracks not played for this many days (including those never played).
    pub not_played_days: Option<u32>,
    /// Tracks added in the last this many days.
    pub added_days: Option<u32>,
    /// Tracks created by this import batch (an import from the buffer zone).
    pub batch_id: Option<u64>,
    /// Tracks whose bitrate is below this many kbps.
    pub bitrate_below: Option<u32>,
    pub era: Option<String>,
    pub mood: Option<String>,
    pub sound_code: Option<String>,
    pub genre: Option<String>,
    pub year_min: Option<u16>,
    pub year_max: Option<u16>,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    #[param(inline)]
    pub sort: Option<TrackSort>,
    /// Reverses the order of `sort`.
    pub desc: Option<bool>,
    /// 1 to 500, 50 by default.
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct LabelRef {
    pub id: u64,
    pub name: String,
    pub color: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct TrackSummary {
    pub id: u64,
    /// Relative to the library root.
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub duration_ms: Option<u32>,
    /// Disabled tracks are never picked by autofill or rotation.
    pub enabled: bool,
    pub missing: bool,
    pub analyzed: bool,
    pub silent: bool,
    /// Where it plays from and to in its file (cue points, overrides
    /// applied), for previews; none for the start or the end of the file.
    pub cue_in_ms: Option<u64>,
    pub cue_out_ms: Option<u64>,
    /// When the scanner first found the file.
    pub added_at_unix_ms: i64,
    /// Plays that count (FX player aside, at least the programming
    /// settings' `min_play_s`).
    pub last_played_at_unix_ms: Option<i64>,
    pub play_count: u64,
    /// `MP3`, `AAC`, `FLAC`, …
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u32>,
    /// `cbr`, `vbr` or `lossless`; unknown for some formats (AAC).
    pub bitrate_mode: Option<String>,
    /// Of first release: from MusicBrainz, else the year tag.
    pub year: Option<u16>,
    /// From the year, on the station's eras (decades by default).
    pub era: Option<String>,
    /// Set by hand, else measured.
    pub bpm: Option<f64>,
    pub mood: Option<String>,
    pub sound_code: Option<String>,
    /// The lowest and highest peak between the cue points, in dBFS (−72:
    /// that or below); none until computed from the waveform.
    pub level_min_db: Option<f64>,
    pub level_max_db: Option<f64>,
    #[sqlx(skip)]
    pub labels: Vec<LabelRef>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrackPage {
    pub items: Vec<TrackSummary>,
    /// Matching tracks in all pages.
    pub total: u64,
}

/// What the track list takes from the programming settings, and the
/// station whose plays it counts (every station's when none).
#[derive(Clone)]
pub(crate) struct ListContext {
    settings: ProgrammingSettings,
    station_id: Option<u64>,
}

impl ListContext {
    pub async fn load(pool: &sqlx::MySqlPool) -> ApiResult<Self> {
        Ok(Self {
            settings: settings::load(pool, ProgrammingSettings::SETTINGS_NAME).await?,
            station_id: None,
        })
    }

    /// Counting the plays of one station.
    fn on_station(&self, station_id: Option<u64>) -> Self {
        Self {
            settings: self.settings.clone(),
            station_id,
        }
    }

    /// `SELECT` and `FROM` of track summaries.
    pub fn push_select(&self, query: &mut QueryBuilder<sqlx::MySql>) {
        query.push(
            "SELECT t.id, t.path, t.title, t.artist, t.album, t.album_artist, t.genre,
                    t.duration_ms, t.enabled,
                    t.missing_since IS NOT NULL AS missing,
                    a.track_id IS NOT NULL AS analyzed,
                    COALESCE(a.cue_out_ms <= a.cue_in_ms, FALSE) AS silent,
                    CAST(COALESCE(tov.cue_in_ms, IF(tov.gap_killer_enabled = FALSE, NULL, a.cue_in_ms))
                         AS UNSIGNED) AS cue_in_ms,
                    CAST(COALESCE(tov.cue_out_ms, IF(tov.gap_killer_enabled = FALSE, NULL, a.cue_out_ms))
                         AS UNSIGNED) AS cue_out_ms,
                    CAST(UNIX_TIMESTAMP(t.created_at) * 1000 AS SIGNED) AS added_at_unix_ms,
                    CAST(UNIX_TIMESTAMP(h.last_played) * 1000 AS SIGNED) AS last_played_at_unix_ms,
                    CAST(COALESCE(h.plays, 0) AS UNSIGNED) AS play_count,
                    t.codec, t.bitrate_kbps, CAST(t.bitrate_mode AS CHAR) AS bitrate_mode,
                    COALESCE(mb.first_release_year, t.year) AS year, ",
        );
        self.push_era(query);
        query.push(
            " AS era, CAST(COALESCE(p.bpm, a.bpm) AS DOUBLE) AS bpm, p.mood, p.sound_code,
              CAST(l.min_db AS DOUBLE) AS level_min_db, CAST(l.max_db AS DOUBLE) AS level_max_db ",
        );
        self.push_from(query);
    }

    /// The tracks and everything their summaries need.
    pub fn push_from(&self, query: &mut QueryBuilder<sqlx::MySql>) {
        query.push(
            " FROM tracks t
              LEFT JOIN track_analysis a ON a.track_id = t.id
              LEFT JOIN track_overrides tov ON tov.track_id = t.id
              LEFT JOIN track_musicbrainz mb ON mb.track_id = t.id
              LEFT JOIN track_programming p ON p.track_id = t.id
              -- Levels of an earlier analysis are not the file's any more.
              LEFT JOIN track_levels l ON l.track_id = t.id AND l.analyzed_at = a.analyzed_at
              LEFT JOIN (SELECT track_id, COUNT(*) AS plays, MAX(started_at) AS last_played
                         FROM play_history
                         WHERE track_id IS NOT NULL AND deck <> 'fx'",
        );
        if let Some(station_id) = self.station_id {
            query.push(" AND station_id = ").push_bind(station_id);
        }
        query.push(" AND (played_ms >= ");
        let min_play_s = self.settings.min_play_s;
        query.push_bind(min_play_s * 1000);
        // Still playing, or its end was missed: it counts once long enough.
        query.push(" OR (played_ms IS NULL AND started_at <= NOW(6) - INTERVAL ");
        query.push_bind(min_play_s);
        query.push(" SECOND)) GROUP BY track_id) h ON h.track_id = t.id");
    }

    /// The era of the year of first release.
    fn push_era(&self, query: &mut QueryBuilder<sqlx::MySql>) {
        let year = "COALESCE(mb.first_release_year, t.year)";
        if self.settings.eras.is_empty() {
            query.push(format!(
                "CASE WHEN {year} IS NULL THEN NULL
                      WHEN {year} >= 2000 OR {year} < 1910 THEN CONCAT({year} DIV 10 * 10, 's')
                      ELSE CONCAT({year} DIV 10 % 10 * 10, 's') END"
            ));
            return;
        }
        query.push("CASE");
        for era in &self.settings.eras {
            query.push(format!(" WHEN {year} IS NOT NULL"));
            if let Some(from) = era.from_year {
                query.push(format!(" AND {year} >= ")).push_bind(from);
            }
            if let Some(to) = era.to_year {
                query.push(format!(" AND {year} <= ")).push_bind(to);
            }
            query.push(" THEN ").push_bind(era.name.clone());
        }
        query.push(" END");
    }

    /// `WHERE` of the filters.
    pub fn push_filters(
        &self,
        query: &mut QueryBuilder<sqlx::MySql>,
        params: &TrackQuery,
    ) -> ApiResult<()> {
        // Tracks in the trash are listed by `/library/trash`.
        query.push(" WHERE t.deleted_at IS NULL");
        for word in params.q.iter().flat_map(|q| q.split_whitespace()).take(10) {
            let pattern = like_pattern(word);
            // Paths compare as bytes: search case- and accent-insensitively.
            query.push(
                " AND CONVERT(CONCAT_WS(' ', t.title, t.artist, t.album, t.album_artist, t.genre,
                                        t.path) USING utf8mb4)
                      COLLATE utf8mb4_unicode_ci LIKE ",
            );
            query.push_bind(pattern);
        }
        if let Some(category_id) = params.category_id {
            query.push(" AND EXISTS (SELECT 1 FROM track_categories tc WHERE tc.track_id = t.id AND tc.category_id = ");
            query.push_bind(category_id);
            query.push(")");
        }
        let condition = match params.filter {
            None => "",
            Some(TrackFilter::Playable) => {
                " AND t.enabled AND t.missing_since IS NULL AND a.cue_out_ms > a.cue_in_ms"
            }
            Some(TrackFilter::Missing) => " AND t.missing_since IS NOT NULL",
            Some(TrackFilter::Unanalyzed) => " AND a.track_id IS NULL",
            Some(TrackFilter::Silent) => " AND a.cue_out_ms <= a.cue_in_ms",
            Some(TrackFilter::Disabled) => " AND NOT t.enabled",
            Some(TrackFilter::NeedsReview) => {
                " AND EXISTS (SELECT 1 FROM track_identifications i
                              WHERE i.track_id = t.id AND i.status = 'needs_review')"
            }
        };
        query.push(condition);

        let labels = |name: &str, ids: &Option<String>| -> ApiResult<Vec<u64>> {
            ids.iter()
                .flat_map(|ids| ids.split(','))
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(|id| {
                    id.parse()
                        .map_err(|_| ApiError::BadRequest(format!("{name}: {id:?} is not an ID")))
                })
                .collect()
        };
        for id in labels("labels_all", &params.labels_all)? {
            query.push(" AND EXISTS (SELECT 1 FROM track_labels tl WHERE tl.track_id = t.id AND tl.label_id = ");
            query.push_bind(id).push(")");
        }
        for (ids, negate) in [
            (labels("labels_any", &params.labels_any)?, ""),
            (labels("labels_none", &params.labels_none)?, " NOT"),
        ] {
            if ids.is_empty() {
                continue;
            }
            query.push(format!(
                " AND{negate} EXISTS (SELECT 1 FROM track_labels tl WHERE tl.track_id = t.id AND tl.label_id IN ("
            ));
            let mut separated = query.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
            query.push("))");
        }

        if params.never_played == Some(true) {
            query.push(" AND h.track_id IS NULL");
        }
        if let Some(days) = params.not_played_days {
            query.push(" AND (h.last_played IS NULL OR h.last_played < NOW(6) - INTERVAL ");
            query.push_bind(days).push(" DAY)");
        }
        if let Some(days) = params.added_days {
            query.push(" AND t.created_at >= NOW(6) - INTERVAL ");
            query.push_bind(days).push(" DAY");
        }
        if let Some(batch_id) = params.batch_id {
            query.push(
                " AND EXISTS (SELECT 1 FROM library_moves m
                              WHERE m.track_id = t.id AND m.from_root <> 'library'
                                AND m.batch_id = ",
            );
            query.push_bind(batch_id).push(")");
        }
        if let Some(kbps) = params.bitrate_below {
            query.push(" AND t.bitrate_kbps < ").push_bind(kbps);
        }
        if let Some(era) = &params.era {
            query.push(" AND (");
            self.push_era(query);
            query.push(") = ").push_bind(era.clone());
        }
        for (column, value) in [
            ("p.mood", &params.mood),
            ("p.sound_code", &params.sound_code),
            ("t.genre", &params.genre),
        ] {
            if let Some(value) = value {
                query
                    .push(format!(" AND {column} = "))
                    .push_bind(value.clone());
            }
        }
        if let Some(year) = params.year_min {
            query
                .push(" AND COALESCE(mb.first_release_year, t.year) >= ")
                .push_bind(year);
        }
        if let Some(year) = params.year_max {
            query
                .push(" AND COALESCE(mb.first_release_year, t.year) <= ")
                .push_bind(year);
        }
        if let Some(bpm) = params.bpm_min {
            query.push(" AND COALESCE(p.bpm, a.bpm) >= ").push_bind(bpm);
        }
        if let Some(bpm) = params.bpm_max {
            query.push(" AND COALESCE(p.bpm, a.bpm) <= ").push_bind(bpm);
        }
        Ok(())
    }

    /// `ORDER BY` of `sort`. Tracks without the value come last.
    fn push_order(&self, query: &mut QueryBuilder<sqlx::MySql>, sort: TrackSort, desc: bool) {
        let direction = |ascending: bool| if ascending != desc { "" } else { " DESC" };
        let scale = |query: &mut QueryBuilder<sqlx::MySql>, column: &str, scale: &[String]| {
            query.push(format!(" ORDER BY {column} IS NULL, "));
            if scale.is_empty() {
                query.push(column.to_owned());
            } else {
                // Values off the scale (it changed) after those on it.
                query.push(format!("FIELD({column}"));
                for value in scale.iter().rev() {
                    query.push(", ").push_bind(value.clone());
                }
                query.push(")");
            }
            query.push(format!("{}, t.artist, t.title", direction(false)));
        };
        match sort {
            TrackSort::Artist => query.push(format!(
                " ORDER BY t.artist IS NULL, t.artist{0}, t.album{0}, t.path{0}",
                direction(true)
            )),
            TrackSort::Title => query.push(format!(
                " ORDER BY t.title IS NULL, t.title{0}, t.artist{0}",
                direction(true)
            )),
            TrackSort::Album => query.push(format!(
                " ORDER BY t.album IS NULL, t.album{0}, t.path{0}",
                direction(true)
            )),
            TrackSort::Genre => query.push(format!(
                " ORDER BY t.genre IS NULL, t.genre{0}, t.artist{0}, t.title{0}",
                direction(true)
            )),
            TrackSort::Added => query.push(format!(
                " ORDER BY t.created_at{0}, t.id{0}",
                direction(false)
            )),
            TrackSort::LastPlayed => query.push(format!(
                " ORDER BY h.last_played IS NOT NULL{0}, h.last_played{0}, t.id{0}",
                direction(true)
            )),
            TrackSort::PlayCount => query.push(format!(
                " ORDER BY COALESCE(h.plays, 0){0}, t.artist, t.title",
                direction(false)
            )),
            TrackSort::Bitrate => query.push(format!(
                " ORDER BY t.bitrate_kbps IS NULL, t.bitrate_kbps{0}, t.artist, t.title",
                direction(true)
            )),
            TrackSort::LevelMin => query.push(format!(
                " ORDER BY l.min_db IS NULL, l.min_db{0}, t.artist, t.title",
                direction(true)
            )),
            TrackSort::LevelMax => query.push(format!(
                " ORDER BY l.max_db IS NULL, l.max_db{0}, t.artist, t.title",
                direction(true)
            )),
            TrackSort::Duration => query.push(format!(
                " ORDER BY t.duration_ms IS NULL, t.duration_ms{0}, t.artist, t.title",
                direction(true)
            )),
            TrackSort::Year => query.push(format!(
                " ORDER BY COALESCE(mb.first_release_year, t.year) IS NULL,
                           COALESCE(mb.first_release_year, t.year){0}, t.artist, t.title",
                direction(true)
            )),
            TrackSort::Bpm => query.push(format!(
                " ORDER BY COALESCE(p.bpm, a.bpm) IS NULL, COALESCE(p.bpm, a.bpm){0},
                           t.artist, t.title",
                direction(true)
            )),
            TrackSort::Mood => {
                scale(query, "p.mood", &self.settings.moods);
                query
            }
            TrackSort::SoundCode => {
                scale(query, "p.sound_code", &self.settings.sound_codes);
                query
            }
        };
    }

    /// The tracks matching `params`, a page at a time.
    pub async fn page(&self, pool: &sqlx::MySqlPool, params: &TrackQuery) -> ApiResult<TrackPage> {
        let context = self.on_station(params.station_id);
        context.page_on_station(pool, params).await
    }

    async fn page_on_station(
        &self,
        pool: &sqlx::MySqlPool,
        params: &TrackQuery,
    ) -> ApiResult<TrackPage> {
        let limit = params.limit.unwrap_or(50);
        if !(1..=500).contains(&limit) {
            return Err(ApiError::BadRequest("limit is 1 to 500".to_owned()));
        }
        let mut count = QueryBuilder::new("SELECT CAST(COUNT(*) AS SIGNED)");
        self.push_from(&mut count);
        self.push_filters(&mut count, params)?;
        let total: i64 = count.build_query_scalar().fetch_one(pool).await?;

        let mut query = QueryBuilder::new("");
        self.push_select(&mut query);
        self.push_filters(&mut query, params)?;
        self.push_order(
            &mut query,
            params.sort.unwrap_or_default(),
            params.desc.unwrap_or(false),
        );
        query.push(" LIMIT ").push_bind(limit);
        query.push(" OFFSET ").push_bind(params.offset.unwrap_or(0));
        let mut items: Vec<TrackSummary> = query.build_query_as().fetch_all(pool).await?;
        add_labels(pool, &mut items).await?;
        Ok(TrackPage {
            items,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// The summaries of `ids`, in no particular order.
    pub async fn summaries(
        &self,
        pool: &sqlx::MySqlPool,
        ids: &[u64],
    ) -> ApiResult<Vec<TrackSummary>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut query = QueryBuilder::new("");
        self.push_select(&mut query);
        query.push(" WHERE t.id IN (");
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(*id);
        }
        query.push(")");
        let mut items: Vec<TrackSummary> = query.build_query_as().fetch_all(pool).await?;
        add_labels(pool, &mut items).await?;
        Ok(items)
    }

    /// The IDs of every track matching `params`, at most `max`.
    pub async fn ids(
        &self,
        pool: &sqlx::MySqlPool,
        params: &TrackQuery,
        max: usize,
    ) -> ApiResult<Vec<u64>> {
        let context = self.on_station(params.station_id);
        let mut query = QueryBuilder::new("SELECT t.id");
        context.push_from(&mut query);
        context.push_filters(&mut query, params)?;
        query
            .push(" ORDER BY t.id LIMIT ")
            .push_bind(max as u64 + 1);
        let ids: Vec<u64> = query.build_query_scalar().fetch_all(pool).await?;
        if ids.len() > max {
            return Err(ApiError::Invalid(format!(
                "the selection has more than {max} tracks"
            )));
        }
        Ok(ids)
    }
}

/// Fills in the labels of `tracks`.
async fn add_labels(pool: &sqlx::MySqlPool, tracks: &mut [TrackSummary]) -> ApiResult<()> {
    if tracks.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::new(
        "SELECT tl.track_id, l.id, l.name, l.color
         FROM track_labels tl JOIN labels l ON l.id = tl.label_id
         WHERE tl.track_id IN (",
    );
    let mut separated = query.separated(", ");
    for track in tracks.iter() {
        separated.push_bind(track.id);
    }
    query.push(") ORDER BY l.name");
    let rows: Vec<(u64, u64, String, Option<String>)> =
        query.build_query_as().fetch_all(pool).await?;
    let mut by_track: std::collections::HashMap<u64, Vec<LabelRef>> =
        std::collections::HashMap::new();
    for (track_id, id, name, color) in rows {
        by_track
            .entry(track_id)
            .or_default()
            .push(LabelRef { id, name, color });
    }
    for track in tracks {
        track.labels = by_track.remove(&track.id).unwrap_or_default();
    }
    Ok(())
}

/// Escapes `%`, `_` and `\` for LIKE.
fn like_pattern(word: &str) -> String {
    let mut pattern = String::from("%");
    for c in word.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

/// Library tracks, searched, filtered and sorted, a page at a time.
#[utoipa::path(get, path = "/tracks", tag = "library",
    params(TrackQuery),
    responses((status = 200, body = TrackPage), (status = "4XX", body = ErrorBody)))]
async fn list_tracks(
    _user: CurrentUser,
    State(state): State<AppState>,
    Query(params): Query<TrackQuery>,
) -> ApiResult<Json<TrackPage>> {
    let context = ListContext::load(&state.pool).await?;
    Ok(Json(context.page(&state.pool, &params).await?))
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct TrackAnalysis {
    pub integrated_lufs: Option<f64>,
    pub true_peak_db: Option<f64>,
    pub cue_in_ms: u32,
    pub cue_out_ms: u32,
    /// The crossfade point: where the next track starts.
    pub fade_out_start_ms: Option<u32>,
    pub has_internal_silence: bool,
    /// The measured tempo.
    pub bpm: Option<f64>,
    pub analyzed_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct TrackCategory {
    pub category_id: u64,
    pub name: String,
    pub color: Option<String>,
    pub weight: f64,
}

#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct TrackIdentification {
    /// `identified` or `needs_review`.
    pub status: String,
    /// Why the track needs review.
    pub reason: Option<String>,
    pub score: Option<f64>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
}

/// How the engine will play the track: analysis and overrides combined.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EffectivePlayout {
    pub cue_in_ms: u32,
    pub cue_out_ms: u32,
    pub mix_point_ms: Option<u32>,
    /// Loudness normalization gain with the current playout settings.
    pub gain_db: f64,
}

/// The file and its tags, as the scanner read them.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct TrackFile {
    pub size: Option<u64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    /// The year tag.
    pub year_tag: Option<u16>,
    pub track_number: Option<u16>,
    pub disc_number: Option<u16>,
    pub musicbrainz_recording_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub missing_since_unix_ms: Option<i64>,
}

/// What MusicBrainz says about the track.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct TrackMusicBrainz {
    pub album: Option<String>,
    pub record_label: Option<String>,
    pub catalog_number: Option<String>,
    pub isrc: Option<String>,
    pub first_release_year: Option<u16>,
    /// Most voted first: suggestions for the genre tag.
    #[sqlx(skip)]
    pub genres: Vec<String>,
    pub fetched_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TrackDetail {
    #[serde(flatten)]
    pub summary: TrackSummary,
    pub file: TrackFile,
    /// The tempo set by hand, replacing the measured one.
    pub bpm_by_hand: Option<f64>,
    pub musicbrainz: Option<TrackMusicBrainz>,
    /// In the trash since then (purged after `trash_days`).
    pub deleted_at_unix_ms: Option<i64>,
    pub analysis: Option<TrackAnalysis>,
    pub overrides: TrackOverrides,
    /// Tags kept in Heron's database (library setting `write_tags` off),
    /// shown instead of the file's: `POST /tracks/tag-overrides/clear`
    /// gives them back to the file.
    pub tag_overrides: Vec<TagField>,
    /// Intro, hook and next-track points, from an import.
    pub markers: Option<crate::import::TrackMarkers>,
    pub playout: EffectivePlayout,
    pub categories: Vec<TrackCategory>,
    pub identification: Option<TrackIdentification>,
    /// Plays and last play in total and per station.
    pub plays: crate::history::Plays,
}

/// A track with its details, analysis, overrides, categories and
/// identification.
#[utoipa::path(get, path = "/tracks/{id}", tag = "library",
    params(("id" = u64, Path)),
    responses((status = 200, body = TrackDetail), (status = "4XX", body = ErrorBody)))]
async fn get_track(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
) -> ApiResult<Json<TrackDetail>> {
    let context = ListContext::load(&state.pool).await?;
    let summary = context
        .summaries(&state.pool, &[id])
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("track"))?;
    let file: TrackFile = sqlx::query_as(
        "SELECT file_size AS size, sample_rate, channels, year AS year_tag, track_number,
                disc_number, musicbrainz_recording_id, musicbrainz_release_id,
                CAST(UNIX_TIMESTAMP(missing_since) * 1000 AS SIGNED) AS missing_since_unix_ms
         FROM tracks WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    let bpm_by_hand: Option<f64> =
        sqlx::query_scalar("SELECT CAST(bpm AS DOUBLE) FROM track_programming WHERE track_id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .flatten();
    let musicbrainz = load_musicbrainz(&state, id).await?;
    let deleted_at_unix_ms: Option<i64> = sqlx::query_scalar(
        "SELECT CAST(UNIX_TIMESTAMP(deleted_at) * 1000 AS SIGNED) FROM tracks WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    let analysis: Option<TrackAnalysis> = sqlx::query_as(
        "SELECT integrated_lufs, true_peak_db, cue_in_ms, cue_out_ms, fade_out_start_ms,
                has_internal_silence, CAST(bpm AS DOUBLE) AS bpm,
                CAST(UNIX_TIMESTAMP(analyzed_at) * 1000 AS SIGNED) AS analyzed_at_unix_ms
         FROM track_analysis WHERE track_id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let categories: Vec<TrackCategory> = sqlx::query_as(
        "SELECT c.id AS category_id, c.name, c.color, tc.weight
         FROM track_categories tc JOIN categories c ON c.id = tc.category_id
         WHERE tc.track_id = ? ORDER BY c.name",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let identification: Option<TrackIdentification> = sqlx::query_as(
        "SELECT CAST(status AS CHAR) AS status, reason, score, recording_mbid, release_mbid
         FROM track_identifications WHERE track_id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let overrides = load_overrides(&state, id).await?;
    let tag_overrides = heron_library::details::overrides(&state.pool, id)
        .await?
        .into_keys()
        .collect();
    let markers = crate::import::markers(&state.pool, id).await?;
    let playout = station.queue.playout(id, None).await?;
    let settings: PlayoutSettings =
        settings::load_for(&state.pool, station.id(), PlayoutSettings::SETTINGS_NAME).await?;
    Ok(Json(TrackDetail {
        summary,
        file,
        bpm_by_hand,
        musicbrainz,
        deleted_at_unix_ms,
        analysis,
        overrides,
        tag_overrides,
        markers,
        playout: EffectivePlayout {
            cue_in_ms: playout.cue_in_ms,
            cue_out_ms: playout.cue_out_ms,
            mix_point_ms: playout.mix_point_ms,
            gain_db: playout.gain_db(&settings.loudness),
        },
        categories,
        identification,
        plays: crate::history::plays(&state.pool, crate::history::PlaysOf::Track(id)).await?,
    }))
}

async fn load_musicbrainz(state: &AppState, id: u64) -> ApiResult<Option<TrackMusicBrainz>> {
    let found = sqlx::query(
        "SELECT album, record_label, catalog_number, isrc, first_release_year,
                CAST(genres AS CHAR) AS genres,
                CAST(UNIX_TIMESTAMP(fetched_at) * 1000 AS SIGNED) AS fetched_at_unix_ms
         FROM track_musicbrainz WHERE track_id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = found else {
        return Ok(None);
    };
    let mut details = TrackMusicBrainz::from_row(&row)?;
    let genres: Option<String> = row.try_get("genres")?;
    details.genres = genres
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    Ok(Some(details))
}

async fn load_overrides(state: &AppState, id: u64) -> ApiResult<TrackOverrides> {
    type OverridesRow = (Option<u32>, Option<u32>, Option<bool>, Option<String>);
    let row: Option<OverridesRow> = sqlx::query_as(
        "SELECT cue_in_ms, cue_out_ms, gap_killer_enabled, CAST(crossfade AS CHAR)
         FROM track_overrides WHERE track_id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((cue_in_ms, cue_out_ms, gap_killer_enabled, crossfade)) = row else {
        return Ok(TrackOverrides::default());
    };
    Ok(TrackOverrides {
        cue_in_ms,
        cue_out_ms,
        gap_killer_enabled,
        crossfade: crossfade
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
    })
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackPatch {
    pub enabled: Option<bool>,
}

/// Enables or disables a track. Operators and admins.
#[utoipa::path(patch, path = "/tracks/{id}", tag = "library",
    params(("id" = u64, Path)), request_body = TrackPatch,
    responses((status = 200, body = TrackDetail), (status = "4XX", body = ErrorBody)))]
async fn patch_track(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(id): Path<u64>,
    Json(patch): Json<TrackPatch>,
) -> ApiResult<Json<TrackDetail>> {
    user.require(Role::Operator)?;
    if let Some(enabled) = patch.enabled {
        sqlx::query("UPDATE tracks SET enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(id)
            .execute(&state.pool)
            .await?;
    }
    get_track(user, State(state), station, Path(id)).await
}

/// Replaces the track's overrides: cue points, Gap Killer, crossfade.
/// Missing fields use the analysis and station settings. Queued entries of
/// the track are updated within seconds. Operators and admins.
#[utoipa::path(put, path = "/tracks/{id}/overrides", tag = "library",
    params(("id" = u64, Path)), request_body = TrackOverrides,
    responses((status = 200, body = TrackOverrides), (status = "4XX", body = ErrorBody)))]
async fn put_overrides(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(overrides): Json<TrackOverrides>,
) -> ApiResult<Json<TrackOverrides>> {
    user.require(Role::Operator)?;
    if let (Some(cue_in), Some(cue_out)) = (overrides.cue_in_ms, overrides.cue_out_ms)
        && cue_in >= cue_out
    {
        return Err(ApiError::Invalid(
            "cue_in_ms must be before cue_out_ms".to_owned(),
        ));
    }
    for fade in [overrides.crossfade.fade_in, overrides.crossfade.fade_out]
        .into_iter()
        .flatten()
    {
        if fade.level_pct > 100 {
            return Err(ApiError::Invalid("fade levels are 0 to 100 %".to_owned()));
        }
    }
    let crossfade = (overrides.crossfade != CrossfadeOverrides::default())
        .then(|| serde_json::to_string(&overrides.crossfade))
        .transpose()
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let result = sqlx::query(
        "INSERT INTO track_overrides (track_id, cue_in_ms, cue_out_ms, crossfade, gap_killer_enabled)
         VALUES (?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE cue_in_ms = VALUES(cue_in_ms), cue_out_ms = VALUES(cue_out_ms),
             crossfade = VALUES(crossfade), gap_killer_enabled = VALUES(gap_killer_enabled)",
    )
    .bind(id)
    .bind(overrides.cue_in_ms)
    .bind(overrides.cue_out_ms)
    .bind(crossfade)
    .bind(overrides.gap_killer_enabled)
    .execute(&state.pool)
    .await;
    match result {
        Err(error) if crate::error::is_still_used(&error) => {
            return Err(ApiError::not_found("track"));
        }
        other => other?,
    };
    state.stations.wake_all();
    Ok(Json(load_overrides(&state, id).await?))
}

/// Removes the track's overrides. Operators and admins.
#[utoipa::path(delete, path = "/tracks/{id}/overrides", tag = "library",
    params(("id" = u64, Path)),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_overrides(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    sqlx::query("DELETE FROM track_overrides WHERE track_id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    state.stations.wake_all();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IntroInput {
    /// Where the vocals start, in ms on the file's timeline; `null` removes it.
    pub intro_ms: Option<u32>,
}

/// Sets where the track's intro ends (the vocals start): jingles of a type
/// playing over the intro end there. Operators and admins.
#[utoipa::path(put, path = "/tracks/{id}/intro", tag = "library",
    params(("id" = u64, Path)), request_body = IntroInput,
    responses((status = 200, body = crate::import::TrackMarkers), (status = "4XX", body = ErrorBody)))]
async fn put_intro(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(input): Json<IntroInput>,
) -> ApiResult<Json<crate::import::TrackMarkers>> {
    user.require(Role::Operator)?;
    let length = track_length(&state, id).await?;
    if let Some(intro_ms) = input.intro_ms
        && (intro_ms == 0 || length.is_some_and(|length| intro_ms > length))
    {
        return Err(ApiError::Invalid(
            "intro_ms is within the file, after its start".to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO track_markers (track_id, intro_ms) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE intro_ms = VALUES(intro_ms)",
    )
    .bind(id)
    .bind(input.intro_ms)
    .execute(&state.pool)
    .await?;
    // Queued entries of the track follow.
    state.stations.wake_all();
    let markers = crate::import::markers(&state.pool, id).await?;
    Ok(Json(markers.unwrap_or_default()))
}

/// The length of a track that is not in the trash (unknown: none).
async fn track_length(state: &AppState, id: u64) -> ApiResult<Option<u32>> {
    let length: Option<Option<u32>> =
        sqlx::query_scalar("SELECT duration_ms FROM tracks WHERE id = ? AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    length.ok_or_else(|| ApiError::not_found("track"))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HookInput {
    /// Where the hook starts and ends, in ms on the file's timeline; both
    /// `null` remove it.
    pub hook_in_ms: Option<u32>,
    pub hook_out_ms: Option<u32>,
}

/// Sets the track's hook (its most recognizable part). Operators and admins.
#[utoipa::path(put, path = "/tracks/{id}/hook", tag = "library",
    params(("id" = u64, Path)), request_body = HookInput,
    responses((status = 200, body = crate::import::TrackMarkers), (status = "4XX", body = ErrorBody)))]
async fn put_hook(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(input): Json<HookInput>,
) -> ApiResult<Json<crate::import::TrackMarkers>> {
    user.require(Role::Operator)?;
    let length = track_length(&state, id).await?;
    match (input.hook_in_ms, input.hook_out_ms) {
        (None, None) => {}
        (Some(hook_in), Some(hook_out))
            if hook_in < hook_out && length.is_none_or(|length| hook_out <= length) => {}
        _ => {
            return Err(ApiError::Invalid(
                "the hook starts before it ends, within the file (both null removes it)".to_owned(),
            ));
        }
    }
    sqlx::query(
        "INSERT INTO track_markers (track_id, hook_in_ms, hook_out_ms) VALUES (?, ?, ?)
         ON DUPLICATE KEY UPDATE hook_in_ms = VALUES(hook_in_ms), hook_out_ms = VALUES(hook_out_ms)",
    )
    .bind(id)
    .bind(input.hook_in_ms)
    .bind(input.hook_out_ms)
    .execute(&state.pool)
    .await?;
    let markers = crate::import::markers(&state.pool, id).await?;
    Ok(Json(markers.unwrap_or_default()))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CategoryAssignment {
    pub category_id: u64,
    /// Relative chance of being picked within the category, 1 by default.
    #[serde(default = "one")]
    pub weight: f64,
}

fn one() -> f64 {
    1.0
}

/// Replaces the track's categories. Operators and admins.
#[utoipa::path(put, path = "/tracks/{id}/categories", tag = "library",
    params(("id" = u64, Path)), request_body = Vec<CategoryAssignment>,
    responses((status = 200, body = Vec<TrackCategory>), (status = "4XX", body = ErrorBody)))]
async fn put_categories(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Json(categories): Json<Vec<CategoryAssignment>>,
) -> ApiResult<Json<Vec<TrackCategory>>> {
    user.require(Role::Operator)?;
    if categories
        .iter()
        .any(|c| !c.weight.is_finite() || c.weight <= 0.0)
    {
        return Err(ApiError::Invalid("weights must be above 0".to_owned()));
    }
    let mut tx = state.pool.begin().await?;
    let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM tracks WHERE id = ? FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(ApiError::not_found("track"));
    }
    sqlx::query("DELETE FROM track_categories WHERE track_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for category in &categories {
        sqlx::query(
            "INSERT INTO track_categories (track_id, category_id, weight) VALUES (?, ?, ?)
             ON DUPLICATE KEY UPDATE weight = VALUES(weight)",
        )
        .bind(id)
        .bind(category.category_id)
        .bind(category.weight)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if crate::error::is_still_used(&error) {
                ApiError::Invalid(format!("no category {}", category.category_id))
            } else {
                error.into()
            }
        })?;
    }
    tx.commit().await?;
    let assigned: Vec<TrackCategory> = sqlx::query_as(
        "SELECT c.id AS category_id, c.name, c.color, tc.weight
         FROM track_categories tc JOIN categories c ON c.id = tc.category_id
         WHERE tc.track_id = ? ORDER BY c.name",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(assigned))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TransitionRequest {
    pub from_track_id: u64,
    pub to_track_id: u64,
    /// Settings to try; the stored ones when missing.
    pub settings: Option<PlayoutSettings>,
    /// Overrides of the outgoing track to try; its own when missing.
    #[serde(default)]
    pub from_overrides: Option<TrackOverrides>,
    /// Overrides of the incoming track to try; its own when missing.
    #[serde(default)]
    pub to_overrides: Option<TrackOverrides>,
}

/// A gain ramp, in ms on its track's own timeline.
#[derive(Debug, Serialize, ToSchema)]
pub struct RampView {
    pub start_ms: u32,
    pub length_ms: u32,
    pub curve: heron_core::FadeCurve,
    /// Level at the quiet end of the ramp, 0 to 1.
    pub level: f32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TransitionPreview {
    /// When the incoming track starts, on the outgoing track's timeline.
    pub next_start_ms: u32,
    /// How long both tracks play at once (0: back to back).
    pub overlap_ms: u32,
    pub fade_out: Option<RampView>,
    pub fade_in: Option<RampView>,
    pub outgoing: EffectivePlayout,
    pub incoming: EffectivePlayout,
}

/// Plans the transition between two tracks the way the engine would, for
/// previews. Nothing is played.
#[utoipa::path(post, path = "/playout/transition", tag = "library",
    request_body = TransitionRequest,
    responses((status = 200, body = TransitionPreview), (status = "4XX", body = ErrorBody)))]
async fn preview_transition(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(request): Json<TransitionRequest>,
) -> ApiResult<Json<TransitionPreview>> {
    let settings = match request.settings {
        Some(settings) => settings,
        None => {
            settings::load_for(&state.pool, station.id(), PlayoutSettings::SETTINGS_NAME).await?
        }
    };
    let outgoing = station
        .queue
        .playout(request.from_track_id, request.from_overrides.as_ref())
        .await?;
    let incoming = station
        .queue
        .playout(request.to_track_id, request.to_overrides.as_ref())
        .await?;
    let transition = plan_transition(&outgoing, &incoming, &settings.crossfade);
    let ramp = |ramp: heron_core::playout::Ramp| RampView {
        start_ms: ramp.start_ms,
        length_ms: ramp.length_ms,
        curve: ramp.curve,
        level: ramp.level,
    };
    let effective = |playout: &heron_core::TrackPlayout| EffectivePlayout {
        cue_in_ms: playout.cue_in_ms,
        cue_out_ms: playout.cue_out_ms,
        mix_point_ms: playout.mix_point_ms,
        gain_db: playout.gain_db(&settings.loudness),
    };
    Ok(Json(TransitionPreview {
        next_start_ms: transition.next_start_ms,
        overlap_ms: transition.overlap_ms(&outgoing),
        fade_out: transition.fade_out.map(ramp),
        fade_in: transition.fade_in.map(ramp),
        outgoing: effective(&outgoing),
        incoming: effective(&incoming),
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Waveform {
    /// Length of one point.
    pub interval_ms: u16,
    /// Level of point 0; 255 is full scale (0 dBFS), on a dB scale.
    pub floor_db: f32,
    /// One byte per point, base64: the highest sample peak in the interval.
    pub peaks: String,
}

/// The track's waveform overview, computed with its analysis. 404 until
/// the track is analyzed.
#[utoipa::path(get, path = "/tracks/{id}/waveform", tag = "library",
    params(("id" = u64, Path)),
    responses((status = 200, body = Waveform), (status = "4XX", body = ErrorBody)))]
async fn get_waveform(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Response> {
    let row: Option<(u16, Vec<u8>)> =
        sqlx::query_as("SELECT interval_ms, peaks FROM track_waveforms WHERE track_id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let (interval_ms, peaks) = row.ok_or_else(|| ApiError::not_found("waveform"))?;
    let mut response = Json(Waveform {
        interval_ms,
        floor_db: heron_analysis::WAVEFORM_FLOOR_DB,
        peaks: base64::engine::general_purpose::STANDARD.encode(peaks),
    })
    .into_response();
    // Changes only when the track is analyzed again.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=60"),
    );
    Ok(response)
}

/// The track's audio file as it is in the library, for previews in the
/// browser. Supports range requests.
#[utoipa::path(get, path = "/tracks/{id}/audio", tag = "library",
    params(("id" = u64, Path)),
    responses(
        (status = 200, description = "The file", content_type = "application/octet-stream"),
        (status = 206, description = "Part of the file"),
        (status = "4XX", body = ErrorBody)))]
async fn get_audio(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
    request: Request,
) -> ApiResult<Response> {
    let path: Option<(String, bool)> =
        sqlx::query_as("SELECT path, missing_since IS NOT NULL FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let (path, missing) = path.ok_or_else(|| ApiError::not_found("track"))?;
    if missing {
        return Err(ApiError::not_found("file"));
    }
    let file = LibraryPath::from_stored(path).resolve(state.library.root());
    let service = tower_http::services::ServeFile::new(file);
    let (parts, _) = request.into_parts();
    let request = Request::from_parts(parts, Body::empty());
    let response = tower::ServiceExt::oneshot(service, request)
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(ApiError::not_found("file"));
    }
    Ok(response.map(Body::new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_words_are_escaped() {
        assert_eq!(like_pattern("50%_off\\"), "%50\\%\\_off\\\\%");
        assert_eq!(like_pattern("miles"), "%miles%");
    }
}
