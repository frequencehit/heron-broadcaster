// SPDX-License-Identifier: AGPL-3.0-or-later

//! Importing track details from another radio automation software, from a
//! JSON Lines export (one audio file per line): tags, categories, fades,
//! markers (intro, hook, next track), problems as labels, and the date each
//! track was added. Admins only.
//!
//! 1. `POST /library/import/preview` matches each row to a library track by
//!    its file name and says what would change, and where Heron already has
//!    values of its own that differ (conflicts). Nothing is written.
//! 2. The person picks the categories' and problems' names in Heron and
//!    settles every conflict; the web UI sends the values to apply.
//! 3. `POST /library/import/apply` applies them in one transaction, and keeps
//!    what each track had before, so `POST /library/imports/{id}/undo` puts it
//!    back. See `docs/import.md`.

use std::collections::{BTreeMap, HashMap};

use axum::Json;
use axum::extract::{DefaultBodyLimit, Path, State};
use heron_core::PlayoutSettings;
use heron_core::library::TagField;
use heron_core::playout::{CrossfadeOverrides, Fade};
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Row, Transaction};
use utoipa::ToSchema;
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouterExt};
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings;

/// Rows of one import, at most.
pub const MAX_ROWS: usize = 50_000;
/// Request bodies of the import (a station's whole library, as JSON).
const MAX_BODY: usize = 64 * 1024 * 1024;
/// A file whose length differs from the export's by more than this is
/// probably another edit of the song.
const DURATION_TOLERANCE_MS: i64 = 2_000;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(preview).layer(DefaultBodyLimit::max(MAX_BODY)))
        .routes(routes!(apply).layer(DefaultBodyLimit::max(MAX_BODY)))
        .routes(routes!(list_imports))
        .routes(routes!(undo))
}

// The export ------------------------------------------------------------

/// One audio file of the other software's export. Times are in seconds, on
/// the timeline of the file the software measured them on; 0 means "not
/// set". Unknown fields are ignored.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
#[serde(default)]
pub struct ImportSourceRow {
    /// The file's name; a folder in front of it is ignored.
    pub filename: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    /// `1997`, as text or as a number.
    #[schema(value_type = Option<String>)]
    pub year: Option<serde_json::Value>,
    /// When the track was added: `2022-03-11 09:35:38`.
    pub created_at: Option<String>,
    pub category_id: Option<u64>,
    pub category_name: Option<String>,
    pub category_parent_id: Option<u64>,
    /// 0: the software never schedules this category.
    pub category_can_schedule: Option<u8>,
    /// 1: a category of jingles.
    pub category_is_jingle: Option<u8>,
    /// 0: no problem.
    pub problem_id: Option<u64>,
    pub problem_name: Option<String>,
    /// Length of the file the export describes, to check it is the same.
    pub final_duration: Option<f64>,
    /// Where the file was cut; the other times are shifted by it.
    pub cue_in: Option<f64>,
    pub intro: Option<f64>,
    pub intro2: Option<f64>,
    /// Length of the fade-in.
    pub fade_in: Option<f64>,
    /// Length of the fade-out.
    pub fade_out: Option<f64>,
    pub hook_in: Option<f64>,
    pub hook_out: Option<f64>,
    pub start_next_soft: Option<f64>,
    pub start_next_hard: Option<f64>,
}

// What an import sets on a track ------------------------------------------

/// Values an import sets on a track; a missing value leaves Heron's alone.
/// Times in ms on the track's file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ImportValues {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub year: Option<String>,
    pub fade_in_ms: Option<u32>,
    pub fade_out_ms: Option<u32>,
    pub intro_ms: Option<u32>,
    pub intro2_ms: Option<u32>,
    pub hook_in_ms: Option<u32>,
    pub hook_out_ms: Option<u32>,
    pub next_soft_ms: Option<u32>,
    pub next_hard_ms: Option<u32>,
    /// The export's category (see `ImportCategoryChoice`).
    pub category: Option<u64>,
    /// The export's problem (see `ImportProblemChoice`).
    pub problem: Option<u64>,
    /// When the track was added: `2022-03-11 09:35:38`, station time.
    pub added_at: Option<String>,
}

/// A value Heron may already have, set in Heron: a conflict when the
/// import's differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportField {
    Title,
    Artist,
    Year,
    FadeIn,
    FadeOut,
    Intro,
    Intro2,
    HookIn,
    HookOut,
    NextSoft,
    NextHard,
}

impl ImportValues {
    fn text(&self, field: ImportField) -> Option<String> {
        let ms = |value: Option<u32>| value.map(|ms| ms.to_string());
        match field {
            ImportField::Title => self.title.clone(),
            ImportField::Artist => self.artist.clone(),
            ImportField::Year => self.year.clone(),
            ImportField::FadeIn => ms(self.fade_in_ms),
            ImportField::FadeOut => ms(self.fade_out_ms),
            ImportField::Intro => ms(self.intro_ms),
            ImportField::Intro2 => ms(self.intro2_ms),
            ImportField::HookIn => ms(self.hook_in_ms),
            ImportField::HookOut => ms(self.hook_out_ms),
            ImportField::NextSoft => ms(self.next_soft_ms),
            ImportField::NextHard => ms(self.next_hard_ms),
        }
    }
}

const FIELDS: [ImportField; 11] = [
    ImportField::Title,
    ImportField::Artist,
    ImportField::Year,
    ImportField::FadeIn,
    ImportField::FadeOut,
    ImportField::Intro,
    ImportField::Intro2,
    ImportField::HookIn,
    ImportField::HookOut,
    ImportField::NextSoft,
    ImportField::NextHard,
];

/// Removes the backslashes some PHP code stores before quotes (`What\'s Up`).
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && matches!(next, '\'' | '"' | '\\')
        {
            out.push(next);
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

fn clean_text(field: TagField, value: Option<&str>) -> Option<String> {
    field.normalize(&unescape(value?)).ok()
}

fn year(value: Option<&serde_json::Value>) -> Option<String> {
    let text = match value? {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        _ => return None,
    };
    TagField::Year.normalize(&text).ok()
}

/// `YYYY-MM-DD HH:MM:SS`, or `None`.
fn added_at(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let bytes = value.as_bytes();
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    let shaped = bytes.len() == 19
        && digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && bytes[10] == b' '
        && digits(11..13)
        && bytes[13] == b':'
        && digits(14..16)
        && bytes[16] == b':'
        && digits(17..19);
    (shaped && !value.starts_with("0000")).then(|| value.to_owned())
}

/// What a row of the export proposes, and what was left out of it.
fn proposed(row: &ImportSourceRow) -> (ImportValues, Vec<String>) {
    let mut warnings = Vec::new();
    let positive = |value: Option<f64>| value.filter(|seconds| *seconds > 0.0);
    let length_ms = positive(row.final_duration).map(|seconds| (seconds * 1000.0).round() as i64);
    let cue_in = positive(row.cue_in).unwrap_or(0.0);
    // A point on the file the software measured, moved onto the file cut at
    // its cue-in.
    let mut point = |name: &str, value: Option<f64>| -> Option<u32> {
        let seconds = positive(value)?;
        let ms = ((seconds - cue_in) * 1000.0).round() as i64;
        if ms <= 0 || length_ms.is_some_and(|length| ms > length) {
            warnings.push(format!(
                "{name} at {seconds} s falls outside the file once moved by the cue-in ({cue_in} s): left out"
            ));
            return None;
        }
        u32::try_from(ms).ok()
    };
    let intro_ms = point("intro", row.intro);
    let intro2_ms = point("intro2", row.intro2);
    let hook_in_ms = point("hook_in", row.hook_in);
    let hook_out_ms = point("hook_out", row.hook_out);
    let next_soft_ms = point("start_next_soft", row.start_next_soft);
    let next_hard_ms = point("start_next_hard", row.start_next_hard);
    let length = |value: Option<f64>| {
        positive(value)
            .map(|seconds| (seconds * 1000.0).round())
            .filter(|ms| *ms <= 60_000.0)
            .map(|ms| ms as u32)
    };
    let values = ImportValues {
        title: clean_text(TagField::Title, row.title.as_deref()),
        artist: clean_text(TagField::Artist, row.artist.as_deref()),
        year: year(row.year.as_ref()),
        fade_in_ms: length(row.fade_in),
        fade_out_ms: length(row.fade_out),
        intro_ms,
        intro2_ms,
        hook_in_ms,
        hook_out_ms,
        next_soft_ms,
        next_hard_ms,
        category: row.category_id.filter(|id| *id > 0),
        problem: row.problem_id.filter(|id| *id > 0),
        added_at: added_at(row.created_at.as_deref()),
    };
    (values, warnings)
}

/// The base name of a path (`6-final-mp3/A - B.mp3` → `A - B.mp3`).
fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

// Heron's side ------------------------------------------------------------

/// The values a track has set in Heron, for the fields an import sets.
async fn current_values(
    executor: &mut sqlx::MySqlConnection,
    track_id: u64,
) -> ApiResult<ImportValues> {
    let mut values = ImportValues::default();
    let tags = heron_library::details::overrides(&mut *executor, track_id).await?;
    let tag = |field: TagField| tags.get(&field).cloned().flatten();
    values.title = tag(TagField::Title);
    values.artist = tag(TagField::Artist);
    values.year = tag(TagField::Year);
    let crossfade = crossfade(&mut *executor, track_id).await?;
    let fade = |fade: Option<Fade>| fade.filter(|fade| fade.enabled).map(|fade| fade.time_ms);
    values.fade_in_ms = fade(crossfade.fade_in);
    values.fade_out_ms = fade(crossfade.fade_out);
    if let Some(markers) = markers(&mut *executor, track_id).await? {
        values.intro_ms = markers.intro_ms;
        values.intro2_ms = markers.intro2_ms;
        values.hook_in_ms = markers.hook_in_ms;
        values.hook_out_ms = markers.hook_out_ms;
        values.next_soft_ms = markers.next_soft_ms;
        values.next_hard_ms = markers.next_hard_ms;
    }
    Ok(values)
}

/// The track's crossfade overrides; none when it has no row.
async fn crossfade(
    executor: &mut sqlx::MySqlConnection,
    track_id: u64,
) -> ApiResult<CrossfadeOverrides> {
    let json: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CAST(crossfade AS CHAR) FROM track_overrides WHERE track_id = ?",
    )
    .bind(track_id)
    .fetch_optional(&mut *executor)
    .await?;
    Ok(json
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

/// Intro, hook and next-track points of a track (`track_markers`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct TrackMarkers {
    pub intro_ms: Option<u32>,
    pub intro2_ms: Option<u32>,
    pub hook_in_ms: Option<u32>,
    pub hook_out_ms: Option<u32>,
    pub next_soft_ms: Option<u32>,
    pub next_hard_ms: Option<u32>,
}

pub async fn markers(
    executor: impl sqlx::MySqlExecutor<'_>,
    track_id: u64,
) -> Result<Option<TrackMarkers>, sqlx::Error> {
    sqlx::query_as(
        "SELECT intro_ms, intro2_ms, hook_in_ms, hook_out_ms, next_soft_ms, next_hard_ms
         FROM track_markers WHERE track_id = ?",
    )
    .bind(track_id)
    .fetch_optional(executor)
    .await
}

// Preview -----------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportPreviewRequest {
    /// The export's rows, in order.
    pub rows: Vec<ImportSourceRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportRowStatus {
    /// One library track has this file name.
    Matched,
    /// No library track has this file name.
    NotFound,
    /// Another row of the export has the same file name.
    DuplicateInExport,
    /// Several library tracks have this file name.
    DuplicateInLibrary,
    /// The row has no file name.
    Invalid,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImportRow {
    /// 1 for the first row of the export.
    pub line: u32,
    pub filename: String,
    pub status: ImportRowStatus,
    pub track_id: Option<u64>,
    /// The library file it matched.
    pub path: Option<String>,
    pub length_ms: Option<u32>,
    /// The export's length of the file.
    pub source_length_ms: Option<u32>,
    /// The lengths differ by more than 2 s: probably another edit.
    pub length_mismatch: bool,
    /// What the import would set.
    pub values: ImportValues,
    /// What Heron has set itself, for the same fields.
    pub current: ImportValues,
    /// Fields set in Heron that the import would change.
    pub conflicts: Vec<ImportField>,
    /// Values of the export left out, and why.
    pub warnings: Vec<String>,
}

/// A category of the export.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportCategory {
    pub id: u64,
    pub name: String,
    pub parent_id: Option<u64>,
    /// The other software never schedules it.
    pub can_schedule: bool,
    pub is_jingle: bool,
    /// Rows in it.
    pub rows: u64,
    /// The Heron category of the same name, if there is one.
    pub heron_category_id: Option<u64>,
}

/// A problem of the export.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportProblem {
    pub id: u64,
    pub name: String,
    pub rows: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ImportPreview {
    pub rows: Vec<ImportRow>,
    pub categories: Vec<ImportCategory>,
    pub problems: Vec<ImportProblem>,
}

/// Matches an export's rows to library tracks and says what an import would
/// change. Writes nothing. Admins only.
#[utoipa::path(post, path = "/library/import/preview", tag = "library",
    request_body = ImportPreviewRequest,
    responses((status = 200, body = ImportPreview), (status = "4XX", body = ErrorBody)))]
async fn preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<ImportPreviewRequest>,
) -> ApiResult<Json<ImportPreview>> {
    user.require(Role::Admin)?;
    if request.rows.len() > MAX_ROWS {
        return Err(ApiError::Invalid(format!("at most {MAX_ROWS} rows")));
    }

    // Library tracks by file name.
    let tracks: Vec<(u64, String, Option<u32>)> = sqlx::query_as(
        "SELECT id, path, duration_ms FROM tracks WHERE deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut by_name: HashMap<String, Vec<(u64, String, Option<u32>)>> = HashMap::new();
    for track in tracks {
        by_name
            .entry(base_name(&track.1).to_lowercase())
            .or_default()
            .push(track);
    }
    let mut in_export: HashMap<String, u32> = HashMap::new();
    for row in &request.rows {
        *in_export
            .entry(base_name(&row.filename).to_lowercase())
            .or_default() += 1;
    }

    let mut connection = state.pool.acquire().await?;
    let mut rows = Vec::with_capacity(request.rows.len());
    let mut categories: BTreeMap<u64, ImportCategory> = BTreeMap::new();
    let mut problems: BTreeMap<u64, ImportProblem> = BTreeMap::new();
    for (index, row) in request.rows.iter().enumerate() {
        let filename = base_name(&row.filename).to_owned();
        let key = filename.to_lowercase();
        let (values, warnings) = proposed(row);
        if let Some(id) = values.category {
            let category = categories.entry(id).or_insert_with(|| ImportCategory {
                id,
                name: row
                    .category_name
                    .as_deref()
                    .map(unescape)
                    .unwrap_or_else(|| format!("Category {id}")),
                parent_id: row.category_parent_id.filter(|id| *id > 0),
                can_schedule: row.category_can_schedule != Some(0),
                is_jingle: row.category_is_jingle == Some(1),
                rows: 0,
                heron_category_id: None,
            });
            category.rows += 1;
        }
        if let Some(id) = values.problem {
            let problem = problems.entry(id).or_insert_with(|| ImportProblem {
                id,
                name: row
                    .problem_name
                    .as_deref()
                    .map(unescape)
                    .unwrap_or_else(|| format!("Problem {id}")),
                rows: 0,
            });
            problem.rows += 1;
        }

        let found = by_name.get(&key).map(Vec::as_slice).unwrap_or_default();
        let status = if filename.trim().is_empty() {
            ImportRowStatus::Invalid
        } else if in_export.get(&key).copied().unwrap_or(0) > 1 {
            ImportRowStatus::DuplicateInExport
        } else {
            match found.len() {
                0 => ImportRowStatus::NotFound,
                1 => ImportRowStatus::Matched,
                _ => ImportRowStatus::DuplicateInLibrary,
            }
        };
        let source_length_ms = row
            .final_duration
            .filter(|seconds| *seconds > 0.0)
            .map(|seconds| (seconds * 1000.0).round() as u32);
        let mut preview = ImportRow {
            line: u32::try_from(index + 1).unwrap_or(u32::MAX),
            filename,
            status,
            track_id: None,
            path: None,
            length_ms: None,
            source_length_ms,
            length_mismatch: false,
            values,
            current: ImportValues::default(),
            conflicts: Vec::new(),
            warnings,
        };
        if status == ImportRowStatus::Matched {
            let (track_id, path, length_ms) = found[0].clone();
            preview.track_id = Some(track_id);
            preview.path = Some(path);
            preview.length_ms = length_ms;
            preview.length_mismatch = length_ms
                .zip(source_length_ms)
                .is_some_and(|(a, b)| (i64::from(a) - i64::from(b)).abs() > DURATION_TOLERANCE_MS);
            preview.current = current_values(&mut connection, track_id).await?;
            preview.conflicts = FIELDS
                .into_iter()
                .filter(|field| {
                    let (now, new) = (preview.current.text(*field), preview.values.text(*field));
                    now.is_some() && new.is_some() && now != new
                })
                .collect();
        }
        rows.push(preview);
    }
    drop(connection);

    let names: Vec<(u64, String)> = sqlx::query_as("SELECT id, name FROM categories")
        .fetch_all(&state.pool)
        .await?;
    for category in categories.values_mut() {
        category.heron_category_id = names
            .iter()
            .find(|(_, name)| name.to_lowercase() == category.name.to_lowercase())
            .map(|(id, _)| *id);
    }
    Ok(Json(ImportPreview {
        rows,
        categories: categories.into_values().collect(),
        problems: problems.into_values().collect(),
    }))
}

// Apply -------------------------------------------------------------------

/// What an export's category becomes in Heron.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportCategoryChoice {
    /// The export's category.
    pub id: u64,
    /// The Heron category its tracks go in, created if missing; none leaves
    /// them out of any category.
    pub name: Option<String>,
    /// Offered by the FX player (set when the category is created).
    #[serde(default)]
    pub fx_player: bool,
    /// Its type (`GET /category-types`), set when the category is created.
    #[serde(default)]
    pub type_id: Option<u64>,
    /// Its tracks are disabled: rotation never picks them.
    #[serde(default)]
    pub disable_tracks: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportProblemAction {
    /// Nothing is done.
    Ignore,
    /// The tracks get a label.
    Label,
    /// The tracks get a label and are disabled.
    LabelAndDisable,
}

/// What an export's problem becomes in Heron.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportProblemChoice {
    pub id: u64,
    pub action: ImportProblemAction,
    /// The label's name, created if missing.
    pub label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportApplyRow {
    pub track_id: u64,
    pub values: ImportValues,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportApplyRequest {
    #[serde(default)]
    pub categories: Vec<ImportCategoryChoice>,
    #[serde(default)]
    pub problems: Vec<ImportProblemChoice>,
    /// The values to set on each track, conflicts settled.
    pub rows: Vec<ImportApplyRow>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
pub struct ImportSummary {
    pub tracks: u64,
    pub categories_created: Vec<String>,
    pub labels_created: Vec<String>,
    pub disabled: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrackImport {
    pub id: u64,
    pub created_by: Option<String>,
    pub created_at_unix_ms: i64,
    pub summary: ImportSummary,
    pub undone_at_unix_ms: Option<i64>,
}

/// What a track had before an import (`track_import_changes.previous`).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Previous {
    /// The tag overrides of the fields the import set (`None`: there was none).
    tag_overrides: BTreeMap<TagField, Option<Option<String>>>,
    title: Option<String>,
    artist: Option<String>,
    year: Option<u16>,
    created_at: Option<String>,
    enabled: bool,
    /// The crossfade overrides, as stored.
    crossfade: Option<String>,
    has_overrides_row: bool,
    markers: Option<TrackMarkers>,
    categories_added: Vec<u64>,
    labels_added: Vec<u64>,
}

/// Applies an import: sets each track's values, puts tracks in categories,
/// labels them, disables them, as one transaction that can be undone.
/// Admins only.
#[utoipa::path(post, path = "/library/import/apply", tag = "library",
    request_body = ImportApplyRequest,
    responses((status = 200, body = TrackImport), (status = "4XX", body = ErrorBody)))]
async fn apply(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<ImportApplyRequest>,
) -> ApiResult<Json<TrackImport>> {
    user.require(Role::Admin)?;
    if request.rows.is_empty() {
        return Err(ApiError::Invalid("no track to import".to_owned()));
    }
    if request.rows.len() > MAX_ROWS {
        return Err(ApiError::Invalid(format!("at most {MAX_ROWS} rows")));
    }
    let mut seen = std::collections::HashSet::new();
    for row in &request.rows {
        if !seen.insert(row.track_id) {
            return Err(ApiError::Invalid(format!(
                "track {} is listed twice",
                row.track_id
            )));
        }
        check(&row.values)?;
    }
    let categories: HashMap<u64, ImportCategoryChoice> = request
        .categories
        .iter()
        .map(|choice| (choice.id, choice.clone()))
        .collect();
    let problems: HashMap<u64, ImportProblemChoice> = request
        .problems
        .iter()
        .map(|choice| (choice.id, choice.clone()))
        .collect();
    let playout: PlayoutSettings = settings::load_for(
        &state.pool,
        state.stations.first().id(),
        PlayoutSettings::SETTINGS_NAME,
    )
    .await?;

    let mut tx = state.pool.begin().await?;
    let import_id = sqlx::query("INSERT INTO track_imports (created_by, summary) VALUES (?, '{}')")
        .bind(user.id)
        .execute(&mut *tx)
        .await?
        .last_insert_id();
    let mut summary = ImportSummary::default();
    let mut category_ids: HashMap<String, u64> = HashMap::new();
    let mut label_ids: HashMap<String, u64> = HashMap::new();

    for row in &request.rows {
        let exists: Option<bool> =
            sqlx::query_scalar("SELECT enabled FROM tracks WHERE id = ? AND deleted_at IS NULL")
                .bind(row.track_id)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(ApiError::Invalid(format!(
                "track {} does not exist",
                row.track_id
            )));
        }
        let values = &row.values;
        let mut previous = snapshot(&mut tx, row.track_id, values).await?;
        let mut disable = false;

        // Tags, kept in Heron's database.
        for (field, value) in [
            (TagField::Title, &values.title),
            (TagField::Artist, &values.artist),
            (TagField::Year, &values.year),
        ] {
            if let Some(value) = value {
                set_tag(&mut tx, row.track_id, field, value, user.id).await?;
            }
        }

        // Fades: the station's, with the export's length.
        if values.fade_in_ms.is_some() || values.fade_out_ms.is_some() {
            let mut crossfade = crossfade(&mut tx, row.track_id).await?;
            if let Some(time_ms) = values.fade_in_ms {
                crossfade.fade_in = Some(Fade {
                    enabled: true,
                    time_ms,
                    ..playout.crossfade.fade_in
                });
            }
            if let Some(time_ms) = values.fade_out_ms {
                crossfade.fade_out = Some(Fade {
                    enabled: true,
                    time_ms,
                    ..playout.crossfade.fade_out
                });
            }
            let json = serde_json::to_string(&crossfade)
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            sqlx::query(
                "INSERT INTO track_overrides (track_id, crossfade) VALUES (?, ?)
                 ON DUPLICATE KEY UPDATE crossfade = VALUES(crossfade)",
            )
            .bind(row.track_id)
            .bind(json)
            .execute(&mut *tx)
            .await?;
        }

        // Markers.
        let marks = [
            values.intro_ms,
            values.intro2_ms,
            values.hook_in_ms,
            values.hook_out_ms,
            values.next_soft_ms,
            values.next_hard_ms,
        ];
        if marks.iter().any(Option::is_some) {
            let mut query = sqlx::query(
                "INSERT INTO track_markers
                     (track_id, intro_ms, intro2_ms, hook_in_ms, hook_out_ms, next_soft_ms, next_hard_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?)
                 ON DUPLICATE KEY UPDATE
                     intro_ms = COALESCE(VALUES(intro_ms), intro_ms),
                     intro2_ms = COALESCE(VALUES(intro2_ms), intro2_ms),
                     hook_in_ms = COALESCE(VALUES(hook_in_ms), hook_in_ms),
                     hook_out_ms = COALESCE(VALUES(hook_out_ms), hook_out_ms),
                     next_soft_ms = COALESCE(VALUES(next_soft_ms), next_soft_ms),
                     next_hard_ms = COALESCE(VALUES(next_hard_ms), next_hard_ms)",
            )
            .bind(row.track_id);
            for mark in marks {
                query = query.bind(mark);
            }
            query.execute(&mut *tx).await?;
        }

        // Category.
        if let Some(choice) = values.category.and_then(|id| categories.get(&id)) {
            if let Some(name) = choice
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                let category = match category_ids.get(name) {
                    Some(id) => *id,
                    None => {
                        let id = category(&mut tx, name, choice, &mut summary).await?;
                        category_ids.insert(name.to_owned(), id);
                        id
                    }
                };
                let added = sqlx::query(
                    "INSERT IGNORE INTO track_categories (track_id, category_id) VALUES (?, ?)",
                )
                .bind(row.track_id)
                .bind(category)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if added > 0 {
                    previous.categories_added.push(category);
                }
            }
            disable |= choice.disable_tracks;
        }

        // Problem.
        if let Some(choice) = values.problem.and_then(|id| problems.get(&id))
            && choice.action != ImportProblemAction::Ignore
        {
            if let Some(name) = choice
                .label
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                let label = match label_ids.get(name) {
                    Some(id) => *id,
                    None => {
                        let id = label(&mut tx, name, &mut summary).await?;
                        label_ids.insert(name.to_owned(), id);
                        id
                    }
                };
                let added = sqlx::query(
                    "INSERT IGNORE INTO track_labels (track_id, label_id) VALUES (?, ?)",
                )
                .bind(row.track_id)
                .bind(label)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if added > 0 {
                    previous.labels_added.push(label);
                }
            }
            disable |= choice.action == ImportProblemAction::LabelAndDisable;
        }

        if disable {
            let changed = sqlx::query(
                "UPDATE tracks SET enabled = FALSE, disabled_by = ?, disabled_at = NOW(6)
                 WHERE id = ? AND enabled",
            )
            .bind(user.id)
            .bind(row.track_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            summary.disabled += changed;
        }
        if let Some(added_at) = &values.added_at {
            sqlx::query("UPDATE tracks SET created_at = ? WHERE id = ?")
                .bind(added_at)
                .bind(row.track_id)
                .execute(&mut *tx)
                .await?;
        }

        let previous = serde_json::to_string(&previous)
            .map_err(|error| ApiError::Internal(error.to_string()))?;
        sqlx::query(
            "INSERT INTO track_import_changes (import_id, track_id, previous) VALUES (?, ?, ?)",
        )
        .bind(import_id)
        .bind(row.track_id)
        .bind(previous)
        .execute(&mut *tx)
        .await?;
        summary.tracks += 1;
    }

    let json =
        serde_json::to_string(&summary).map_err(|error| ApiError::Internal(error.to_string()))?;
    sqlx::query("UPDATE track_imports SET summary = ? WHERE id = ?")
        .bind(json)
        .bind(import_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.stations.wake_all();
    tracing::info!(user = %user.username, import_id, tracks = summary.tracks, "tracks imported");
    Ok(Json(find_import(&state, import_id).await?))
}

/// Refuses values the database cannot hold.
fn check(values: &ImportValues) -> ApiResult<()> {
    for (field, value) in [
        (TagField::Title, &values.title),
        (TagField::Artist, &values.artist),
        (TagField::Year, &values.year),
    ] {
        if let Some(value) = value {
            field
                .normalize(value)
                .map_err(|error| ApiError::Invalid(format!("{field:?}: {error}")))?;
        }
    }
    if let Some(value) = &values.added_at
        && added_at(Some(value)).is_none()
    {
        return Err(ApiError::Invalid(format!(
            "added_at {value:?} is not a date (YYYY-MM-DD HH:MM:SS)"
        )));
    }
    for fade in [values.fade_in_ms, values.fade_out_ms]
        .into_iter()
        .flatten()
    {
        if fade > 60_000 {
            return Err(ApiError::Invalid("fades are at most 60 s".to_owned()));
        }
    }
    Ok(())
}

/// What the track has before the import, for the fields it sets.
async fn snapshot(
    tx: &mut Transaction<'_, MySql>,
    track_id: u64,
    values: &ImportValues,
) -> ApiResult<Previous> {
    let row = sqlx::query(
        "SELECT title, artist, year, enabled,
                DATE_FORMAT(created_at, '%Y-%m-%d %H:%i:%s.%f') AS created_at
         FROM tracks WHERE id = ?",
    )
    .bind(track_id)
    .fetch_one(&mut **tx)
    .await?;
    let tags = heron_library::details::overrides(&mut **tx, track_id).await?;
    let mut tag_overrides = BTreeMap::new();
    for (field, set) in [
        (TagField::Title, values.title.is_some()),
        (TagField::Artist, values.artist.is_some()),
        (TagField::Year, values.year.is_some()),
    ] {
        if set {
            tag_overrides.insert(field, tags.get(&field).cloned());
        }
    }
    let overrides: Option<Option<String>> = sqlx::query_scalar(
        "SELECT CAST(crossfade AS CHAR) FROM track_overrides WHERE track_id = ?",
    )
    .bind(track_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(Previous {
        tag_overrides,
        title: row.try_get("title")?,
        artist: row.try_get("artist")?,
        year: row.try_get("year")?,
        created_at: row.try_get("created_at")?,
        enabled: row.try_get("enabled")?,
        has_overrides_row: overrides.is_some(),
        crossfade: overrides.flatten(),
        markers: markers(&mut **tx, track_id).await?,
        categories_added: Vec::new(),
        labels_added: Vec::new(),
    })
}

/// Keeps a tag in Heron's database and shows it in the track's columns.
async fn set_tag(
    tx: &mut Transaction<'_, MySql>,
    track_id: u64,
    field: TagField,
    value: &str,
    user_id: u64,
) -> ApiResult<()> {
    let value = field
        .normalize(value)
        .map_err(|error| ApiError::Invalid(format!("{field:?}: {error}")))?;
    sqlx::query(
        "INSERT INTO track_tag_overrides (track_id, field, value, updated_by) VALUES (?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE value = VALUES(value), updated_by = VALUES(updated_by)",
    )
    .bind(track_id)
    .bind(field.as_str())
    .bind(&value)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    let query = match field {
        TagField::Title => "UPDATE tracks SET title = ? WHERE id = ?",
        TagField::Artist => "UPDATE tracks SET artist = ? WHERE id = ?",
        _ => "UPDATE tracks SET year = ? WHERE id = ?",
    };
    let query = sqlx::query(query);
    let query = if field == TagField::Year {
        query.bind(value.parse::<u16>().ok())
    } else {
        query.bind(value)
    };
    query.bind(track_id).execute(&mut **tx).await?;
    Ok(())
}

/// The category named `name`, created if missing.
async fn category(
    tx: &mut Transaction<'_, MySql>,
    name: &str,
    choice: &ImportCategoryChoice,
    summary: &mut ImportSummary,
) -> ApiResult<u64> {
    let existing: Option<u64> = sqlx::query_scalar("SELECT id FROM categories WHERE name = ?")
        .bind(name)
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = sqlx::query("INSERT INTO categories (name, type_id, fx_player) VALUES (?, ?, ?)")
        .bind(name)
        .bind(choice.type_id)
        .bind(choice.fx_player)
        .execute(&mut **tx)
        .await
        .map_err(|error| {
            if crate::error::is_still_used(&error) {
                ApiError::Invalid(format!("category {name:?}: no such category type"))
            } else {
                error.into()
            }
        })?
        .last_insert_id();
    // The first station's, until an import chooses its stations.
    sqlx::query(
        "INSERT INTO category_stations (category_id, station_id) SELECT ?, MIN(id) FROM stations",
    )
    .bind(id)
    .execute(&mut **tx)
    .await?;
    summary.categories_created.push(name.to_owned());
    Ok(id)
}

/// The label named `name`, created if missing.
async fn label(
    tx: &mut Transaction<'_, MySql>,
    name: &str,
    summary: &mut ImportSummary,
) -> ApiResult<u64> {
    let name: String = name.chars().take(100).collect();
    let existing: Option<u64> = sqlx::query_scalar("SELECT id FROM labels WHERE name = ?")
        .bind(&name)
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = sqlx::query("INSERT INTO labels (name) VALUES (?)")
        .bind(&name)
        .execute(&mut **tx)
        .await?
        .last_insert_id();
    summary.labels_created.push(name);
    Ok(id)
}

// History and undo ----------------------------------------------------------

async fn find_import(state: &AppState, id: u64) -> ApiResult<TrackImport> {
    imports(state, Some(id))
        .await?
        .pop()
        .ok_or_else(|| ApiError::not_found("import"))
}

async fn imports(state: &AppState, id: Option<u64>) -> ApiResult<Vec<TrackImport>> {
    let rows = sqlx::query(
        "SELECT i.id, u.username, CAST(i.summary AS CHAR) AS summary,
                CAST(UNIX_TIMESTAMP(i.created_at) * 1000 AS SIGNED) AS created_at,
                CAST(UNIX_TIMESTAMP(i.undone_at) * 1000 AS SIGNED) AS undone_at
         FROM track_imports i LEFT JOIN users u ON u.id = i.created_by
         WHERE ? IS NULL OR i.id = ?
         ORDER BY i.id DESC LIMIT 50",
    )
    .bind(id)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let summary: String = row.try_get("summary")?;
            Ok(TrackImport {
                id: row.try_get("id")?,
                created_by: row.try_get("username")?,
                created_at_unix_ms: row.try_get("created_at")?,
                summary: serde_json::from_str(&summary).unwrap_or_default(),
                undone_at_unix_ms: row.try_get("undone_at")?,
            })
        })
        .collect()
}

/// The last imports, newest first. Admins only.
#[utoipa::path(get, path = "/library/imports", tag = "library",
    responses((status = 200, body = Vec<TrackImport>), (status = "4XX", body = ErrorBody)))]
async fn list_imports(
    user: CurrentUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<TrackImport>>> {
    user.require(Role::Admin)?;
    Ok(Json(imports(&state, None).await?))
}

/// Undoes an import: every track gets back what it had before it, as one
/// transaction. Categories and labels it created stay. Admins only.
#[utoipa::path(post, path = "/library/imports/{id}/undo", tag = "library",
    params(("id" = u64, Path)),
    responses((status = 200, body = TrackImport), (status = "4XX", body = ErrorBody)))]
async fn undo(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> ApiResult<Json<TrackImport>> {
    user.require(Role::Admin)?;
    let mut tx = state.pool.begin().await?;
    let undone: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT CAST(UNIX_TIMESTAMP(undone_at) AS SIGNED) FROM track_imports WHERE id = ? FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    match undone {
        None => return Err(ApiError::not_found("import")),
        Some(Some(_)) => {
            return Err(ApiError::Conflict(
                "this import was undone already".to_owned(),
            ));
        }
        Some(None) => {}
    }
    let changes: Vec<(u64, String)> = sqlx::query_as(
        "SELECT track_id, CAST(previous AS CHAR) FROM track_import_changes WHERE import_id = ?",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    for (track_id, previous) in changes {
        let previous: Previous = serde_json::from_str(&previous)
            .map_err(|error| ApiError::Internal(format!("import {id}: {error}")))?;
        restore(&mut tx, track_id, &previous).await?;
    }
    sqlx::query("UPDATE track_imports SET undone_at = NOW(6), undone_by = ? WHERE id = ?")
        .bind(user.id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.stations.wake_all();
    tracing::info!(user = %user.username, import_id = id, "import undone");
    Ok(Json(find_import(&state, id).await?))
}

async fn restore(
    tx: &mut Transaction<'_, MySql>,
    track_id: u64,
    previous: &Previous,
) -> ApiResult<()> {
    for (field, value) in &previous.tag_overrides {
        match value {
            None => {
                sqlx::query("DELETE FROM track_tag_overrides WHERE track_id = ? AND field = ?")
                    .bind(track_id)
                    .bind(field.as_str())
                    .execute(&mut **tx)
                    .await?;
            }
            Some(value) => {
                sqlx::query(
                    "INSERT INTO track_tag_overrides (track_id, field, value) VALUES (?, ?, ?)
                     ON DUPLICATE KEY UPDATE value = VALUES(value)",
                )
                .bind(track_id)
                .bind(field.as_str())
                .bind(value)
                .execute(&mut **tx)
                .await?;
            }
        }
    }
    sqlx::query(
        "UPDATE tracks SET title = ?, artist = ?, year = ?,
                disabled_by = IF(?, NULL, disabled_by),
                disabled_at = IF(?, NULL, disabled_at),
                enabled = ?,
                created_at = COALESCE(?, created_at)
         WHERE id = ?",
    )
    .bind(&previous.title)
    .bind(&previous.artist)
    .bind(previous.year)
    .bind(previous.enabled)
    .bind(previous.enabled)
    .bind(previous.enabled)
    .bind(&previous.created_at)
    .bind(track_id)
    .execute(&mut **tx)
    .await?;
    if previous.has_overrides_row {
        sqlx::query("UPDATE track_overrides SET crossfade = ? WHERE track_id = ?")
            .bind(&previous.crossfade)
            .bind(track_id)
            .execute(&mut **tx)
            .await?;
    } else {
        // Created by the import, with its crossfade only, unless changed since.
        sqlx::query(
            "DELETE FROM track_overrides WHERE track_id = ?
             AND cue_in_ms IS NULL AND cue_out_ms IS NULL AND gap_killer_enabled IS NULL",
        )
        .bind(track_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query("UPDATE track_overrides SET crossfade = NULL WHERE track_id = ?")
            .bind(track_id)
            .execute(&mut **tx)
            .await?;
    }
    match &previous.markers {
        None => {
            sqlx::query("DELETE FROM track_markers WHERE track_id = ?")
                .bind(track_id)
                .execute(&mut **tx)
                .await?;
        }
        Some(markers) => {
            sqlx::query(
                "UPDATE track_markers SET intro_ms = ?, intro2_ms = ?, hook_in_ms = ?,
                        hook_out_ms = ?, next_soft_ms = ?, next_hard_ms = ?
                 WHERE track_id = ?",
            )
            .bind(markers.intro_ms)
            .bind(markers.intro2_ms)
            .bind(markers.hook_in_ms)
            .bind(markers.hook_out_ms)
            .bind(markers.next_soft_ms)
            .bind(markers.next_hard_ms)
            .bind(track_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    for category in &previous.categories_added {
        sqlx::query("DELETE FROM track_categories WHERE track_id = ? AND category_id = ?")
            .bind(track_id)
            .bind(category)
            .execute(&mut **tx)
            .await?;
    }
    for label in &previous.labels_added {
        sqlx::query("DELETE FROM track_labels WHERE track_id = ? AND label_id = ?")
            .bind(track_id)
            .bind(label)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backslashes_before_quotes_are_removed() {
        assert_eq!(unescape(r"What\'s Up"), "What's Up");
        assert_eq!(unescape(r#"Say \"Hi\""#), r#"Say "Hi""#);
        assert_eq!(unescape(r"AC\\DC"), r"AC\DC");
        assert_eq!(unescape(r"C:\path"), r"C:\path");
    }

    #[test]
    fn rows_are_moved_onto_the_cut_file() {
        let row = ImportSourceRow {
            filename: "6-final-mp3/AC-DC - Back in Black.mp3".into(),
            title: Some(r"Back in Black".into()),
            artist: Some(r"AC\'DC".into()),
            year: Some(serde_json::json!("1980")),
            created_at: Some("2021-02-07 20:36:51".into()),
            category_id: Some(10),
            problem_id: Some(0),
            final_duration: Some(239.308),
            cue_in: Some(5.2551),
            intro: Some(26.1614),
            intro2: Some(0.0),
            fade_in: Some(0.0),
            fade_out: Some(0.5),
            hook_in: Some(99.0409),
            hook_out: Some(108.473),
            // Before the cue-in: outside the cut file.
            start_next_soft: Some(3.0),
            ..ImportSourceRow::default()
        };
        let (values, warnings) = proposed(&row);
        assert_eq!(values.intro_ms, Some(20_906));
        assert_eq!(values.hook_in_ms, Some(93_786));
        assert_eq!(values.intro2_ms, None);
        assert_eq!(values.fade_in_ms, None);
        assert_eq!(values.fade_out_ms, Some(500));
        assert_eq!(values.next_soft_ms, None);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(values.artist.as_deref(), Some("AC'DC"));
        assert_eq!(values.year.as_deref(), Some("1980"));
        assert_eq!(values.category, Some(10));
        assert_eq!(values.problem, None);
        assert_eq!(values.added_at.as_deref(), Some("2021-02-07 20:36:51"));
        assert_eq!(base_name(&row.filename), "AC-DC - Back in Black.mp3");
    }

    #[test]
    fn odd_values_are_left_out() {
        let row = ImportSourceRow {
            year: Some(serde_json::json!(1975)),
            created_at: Some("0000-00-00 00:00:00".into()),
            fade_out: Some(90.0),
            ..ImportSourceRow::default()
        };
        let (values, _) = proposed(&row);
        assert_eq!(values.year.as_deref(), Some("1975"));
        assert_eq!(values.added_at, None);
        assert_eq!(values.fade_out_ms, None);
        let row = ImportSourceRow {
            year: Some(serde_json::json!("soon")),
            ..ImportSourceRow::default()
        };
        assert_eq!(proposed(&row).0.year, None);
    }
}
