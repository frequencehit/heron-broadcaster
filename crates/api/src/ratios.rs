// SPDX-License-Identifier: AGPL-3.0-or-later

//! Play ratios per station (`docs/rotation.md#play-ratio`): a track's
//! `play_ratio` is its default, and a station can have its own
//! (`track_station_ratios`), 0 disabling the track on that station only.
//! Also: resetting all of a station's counters, and importing ratios from
//! a CSV file with one column per station.

use std::collections::{BTreeMap, HashMap};

use axum::Json;
use axum::extract::{DefaultBodyLimit, Query, State};
use serde::{Deserialize, Serialize};
use sqlx::{MySql, MySqlPool, QueryBuilder, Transaction};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouterExt};
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::stations::Station;
use crate::tracks::TrackSummary;

/// A ratio import, at most.
const MAX_IMPORT: usize = 8 * 1024 * 1024;
/// Unmatched rows and invalid values listed, at most.
const MAX_LISTED: usize = 500;
/// IDs per statement.
const CHUNK: usize = 1_000;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(import_ratios).layer(DefaultBodyLimit::max(MAX_IMPORT)))
        .routes(routes!(reset_station_counters))
}

/// A track's play ratio on a station.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct StationRatio {
    pub station_id: u64,
    /// The station's own, in tenths, 1 to 10, kept while the track is
    /// disabled there; `null`: the track's default.
    pub play_ratio: Option<u8>,
    /// Off: disabled on this station only.
    pub enabled: bool,
    /// What rotation uses there: 0 when the track is disabled there or
    /// everywhere.
    pub ratio: u8,
    /// Its counter there, 10 down to 1 (the track's page only).
    pub counter: Option<u8>,
    /// Who disabled it there (a username), while it is.
    pub disabled_by: Option<String>,
    pub disabled_at_unix_ms: Option<i64>,
}

/// The ratio rotation uses on a station.
fn effective(enabled: bool, default: u8, own: Option<u8>, here: bool) -> u8 {
    if enabled && here {
        own.unwrap_or(default)
    } else {
        0
    }
}

/// A station's row, as stored: track, own ratio, enabled, by, at.
type Row = (u64, Option<u8>, bool, Option<String>, Option<i64>);

const ROW_COLUMNS: &str = "sr.track_id, sr.play_ratio, sr.enabled, u.username,
     IF(sr.enabled, NULL, CAST(UNIX_TIMESTAMP(sr.disabled_at) * 1000 AS SIGNED))";

/// Fills in the `station_ratio` of `tracks` listed on `station_id`.
pub async fn add_to_summaries(
    pool: &MySqlPool,
    station_id: u64,
    tracks: &mut [TrackSummary],
) -> ApiResult<()> {
    if tracks.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::new(format!(
        "SELECT {ROW_COLUMNS} FROM track_station_ratios sr
         LEFT JOIN users u ON u.id = sr.disabled_by AND NOT sr.enabled
         WHERE sr.station_id = "
    ));
    query.push_bind(station_id).push(" AND sr.track_id IN (");
    let mut separated = query.separated(", ");
    for track in tracks.iter() {
        separated.push_bind(track.id);
    }
    query.push(")");
    let rows: Vec<Row> = query.build_query_as().fetch_all(pool).await?;
    let mut by_track: HashMap<u64, Row> = rows.into_iter().map(|row| (row.0, row)).collect();
    for track in tracks {
        let (_, own, here, by, at) = by_track
            .remove(&track.id)
            .unwrap_or((track.id, None, true, None, None));
        track.station_ratio = Some(StationRatio {
            station_id,
            play_ratio: own,
            enabled: here,
            ratio: effective(track.enabled, track.play_ratio, own, here),
            counter: None,
            disabled_by: by,
            disabled_at_unix_ms: at,
        });
    }
    Ok(())
}

/// The track's ratio and counter on every station, in the stations' order.
pub async fn of_track(
    pool: &MySqlPool,
    track_id: u64,
    enabled: bool,
    default: u8,
) -> ApiResult<Vec<StationRatio>> {
    type Line = (
        u64,
        u64,
        Option<u8>,
        Option<bool>,
        Option<String>,
        Option<i64>,
    );
    let rows: Vec<Line> = sqlx::query_as(
        "SELECT s.id, CAST(COALESCE(c.counter, 10) AS UNSIGNED), sr.play_ratio, sr.enabled,
                u.username,
                IF(sr.enabled, NULL, CAST(UNIX_TIMESTAMP(sr.disabled_at) * 1000 AS SIGNED))
         FROM stations s
         LEFT JOIN track_ratio_counters c ON c.station_id = s.id AND c.track_id = ?
         LEFT JOIN track_station_ratios sr ON sr.station_id = s.id AND sr.track_id = ?
         LEFT JOIN users u ON u.id = sr.disabled_by AND NOT sr.enabled
         ORDER BY s.position, s.id",
    )
    .bind(track_id)
    .bind(track_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(station_id, counter, own, here, by, at)| {
            let here = here.unwrap_or(true);
            StationRatio {
                station_id,
                play_ratio: own,
                enabled: here,
                ratio: effective(enabled, default, own, here),
                counter: Some(u8::try_from(counter).unwrap_or(heron_rotation::ratio::FULL)),
                disabled_by: by,
                disabled_at_unix_ms: at,
            }
        })
        .collect())
}

/// What changes on a station for some tracks.
#[derive(Clone, Copy, Debug, Default)]
pub struct StationChange {
    /// Disables (`false`) or enables them on the station.
    pub enabled: Option<bool>,
    /// Their own ratio there, in tenths, 1 to 10.
    pub play_ratio: Option<u8>,
    /// Back to the default ratio, enabled there.
    pub default_ratio: bool,
}

impl StationChange {
    /// From the fields of a request: a ratio of 0 disables.
    pub fn new(
        enabled: Option<bool>,
        play_ratio: Option<u8>,
        default_ratio: bool,
    ) -> ApiResult<Self> {
        let enabled = crate::selection::check_ratio(enabled, play_ratio)?;
        if default_ratio && (enabled == Some(false) || play_ratio.is_some()) {
            return Err(ApiError::Invalid(
                "default_ratio goes back to the default: no play_ratio, and not disabled"
                    .to_owned(),
            ));
        }
        Ok(Self {
            enabled,
            play_ratio: play_ratio.filter(|ratio| *ratio > 0),
            default_ratio,
        })
    }
}

/// Pushes ` AND track_id IN (…)` of `ids`.
fn push_ids(query: &mut QueryBuilder<MySql>, ids: &[u64]) {
    query.push(" AND track_id IN (");
    let mut separated = query.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
    query.push(")");
}

/// Applies `change` to `ids` on `station_id`, `user_id` disabling them;
/// answers how many tracks changed.
pub async fn change_on_station(
    tx: &mut Transaction<'_, MySql>,
    ids: &[u64],
    station_id: u64,
    change: StationChange,
    user_id: u64,
) -> ApiResult<u64> {
    let mut changed = 0;
    for chunk in ids.chunks(CHUNK) {
        if change.default_ratio {
            let mut query =
                QueryBuilder::new("DELETE FROM track_station_ratios WHERE station_id = ");
            query.push_bind(station_id);
            push_ids(&mut query, chunk);
            changed += query.build().execute(&mut **tx).await?.rows_affected();
            continue;
        }
        let mut query =
            QueryBuilder::new("INSERT IGNORE INTO track_station_ratios (track_id, station_id) ");
        query.push_values(chunk, |mut row, id| {
            row.push_bind(*id).push_bind(station_id);
        });
        query.build().execute(&mut **tx).await?;
        let mut touched = 0;
        if let Some(ratio) = change.play_ratio {
            let mut query = QueryBuilder::new("UPDATE track_station_ratios SET play_ratio = ");
            query
                .push_bind(ratio)
                .push(" WHERE NOT (play_ratio <=> ")
                .push_bind(ratio)
                .push(") AND station_id = ")
                .push_bind(station_id);
            push_ids(&mut query, chunk);
            touched = touched.max(query.build().execute(&mut **tx).await?.rows_affected());
        }
        match change.enabled {
            Some(false) => {
                let mut query = QueryBuilder::new(
                    "UPDATE track_station_ratios SET enabled = FALSE, disabled_at = NOW(6),
                     disabled_by = ",
                );
                query
                    .push_bind(user_id)
                    .push(" WHERE enabled AND station_id = ")
                    .push_bind(station_id);
                push_ids(&mut query, chunk);
                touched = touched.max(query.build().execute(&mut **tx).await?.rows_affected());
            }
            Some(true) => {
                let mut query = QueryBuilder::new(
                    "UPDATE track_station_ratios
                     SET enabled = TRUE, disabled_by = NULL, disabled_at = NULL
                     WHERE NOT enabled AND station_id = ",
                );
                query.push_bind(station_id);
                push_ids(&mut query, chunk);
                touched = touched.max(query.build().execute(&mut **tx).await?.rows_affected());
            }
            None => {}
        }
        changed += touched;
        // A row that says nothing the default does not: gone.
        let mut query = QueryBuilder::new(
            "DELETE FROM track_station_ratios WHERE play_ratio IS NULL AND enabled AND station_id = ",
        );
        query.push_bind(station_id);
        push_ids(&mut query, chunk);
        query.build().execute(&mut **tx).await?;
    }
    Ok(changed)
}

/// Puts the counters of `ids` on `station_id` back to 10.
pub async fn reset_counters_on(
    tx: &mut Transaction<'_, MySql>,
    ids: &[u64],
    station_id: u64,
) -> ApiResult<()> {
    for chunk in ids.chunks(CHUNK) {
        let mut query = QueryBuilder::new("DELETE FROM track_ratio_counters WHERE station_id = ");
        query.push_bind(station_id);
        push_ids(&mut query, chunk);
        query.build().execute(&mut **tx).await?;
    }
    Ok(())
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CountersReset {
    /// Counters that were not at 10.
    pub reset: u64,
}

/// Puts every play ratio counter of the station back to 10. Operators and
/// admins of the station.
#[utoipa::path(post, path = "/rotation/counters/reset", tag = "rotation",
    responses((status = 200, body = CountersReset), (status = "4XX", body = ErrorBody)))]
async fn reset_station_counters(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<CountersReset>> {
    user.require(Role::Operator)?;
    let reset =
        sqlx::query("DELETE FROM track_ratio_counters WHERE station_id = ? AND counter <> 10")
            .bind(station.id())
            .execute(&state.pool)
            .await?
            .rows_affected();
    sqlx::query("DELETE FROM track_ratio_counters WHERE station_id = ?")
        .bind(station.id())
        .execute(&state.pool)
        .await?;
    tracing::info!(user = %user.username, station = station.id(), reset,
        "play ratio counters reset to 10");
    Ok(Json(CountersReset { reset }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImportParams {
    /// Checks the file and answers what it would change, writing nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// Why a row matches no track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Unmatched {
    /// No track has that path, or that file name.
    NotFound,
    /// Several tracks have that file name.
    Several,
    /// The row names no track.
    Empty,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UnmatchedRow {
    /// Its line in the file, from 1 (the header).
    pub line: usize,
    /// The path or file name it gives.
    pub name: String,
    pub reason: Unmatched,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InvalidValue {
    pub line: usize,
    pub column: String,
    pub value: String,
}

/// What an import of ratios found, and did.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct RatioImport {
    pub dry_run: bool,
    /// `path` or `filename`.
    pub matched_by: String,
    /// The columns of ratios: station codes, and `default`.
    pub columns: Vec<String>,
    /// Columns that are neither a station's code nor `default`.
    pub unknown_columns: Vec<String>,
    /// Rows read, the header aside.
    pub rows: u64,
    /// Rows matching one track.
    pub matched: u64,
    /// Valid ratios in the rows matched.
    pub values: u64,
    /// Ratios set to another value, tracks disabled or enabled (none in a
    /// dry run).
    pub changed: u64,
    /// The first 500.
    pub unmatched: Vec<UnmatchedRow>,
    pub unmatched_total: u64,
    /// The first 500.
    pub invalid: Vec<InvalidValue>,
    pub invalid_total: u64,
}

/// The lines of a CSV file, as fields, with their line numbers (from 1).
/// Fields are separated by `separator`; a field in double quotes can hold
/// one, a line break or `""` (a quote).
fn parse_csv(text: &str, separator: char) -> Vec<(usize, Vec<String>)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = Vec::new();
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut line = 1;
    let mut start = 1;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            }
            '"' if field.is_empty() => quoted = true,
            '\n' if quoted => {
                line += 1;
                field.push(c);
            }
            c if c == separator && !quoted => fields.push(std::mem::take(&mut field)),
            '\r' if !quoted => {}
            '\n' => {
                fields.push(std::mem::take(&mut field));
                if fields.iter().any(|field| !field.trim().is_empty()) {
                    lines.push((start, std::mem::take(&mut fields)));
                }
                fields.clear();
                line += 1;
                start = line;
            }
            c => field.push(c),
        }
    }
    fields.push(field);
    if fields.iter().any(|field| !field.trim().is_empty()) {
        lines.push((start, fields));
    }
    lines
}

/// The separator of a CSV file: a semicolon when its first line has more
/// semicolons than commas (decimals written `0,2`), else a comma.
fn separator(text: &str) -> char {
    let header = text.lines().next().unwrap_or("");
    if header.matches(';').count() > header.matches(',').count() {
        ';'
    } else {
        ','
    }
}

/// A ratio as written in the file, in tenths: `0.2`, `0,2`, `1`, `0`.
fn parse_ratio(value: &str) -> Option<u8> {
    let number: f64 = value.trim().replace(',', ".").parse().ok()?;
    let tenths = number * 10.0;
    let rounded = tenths.round();
    ((0.0..=10.0).contains(&rounded) && (tenths - rounded).abs() < 1e-6).then_some(rounded as u8)
}

/// The file name of a library path, for matching, ignoring case.
fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_lowercase()
}

/// Where a column's ratios go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    Default,
    Station(u64),
}

/// Sets play ratios from a CSV file (the body, `text/csv`): a `path` or
/// `filename` column naming the track, a column per station code, and
/// `default`. A ratio is 0 to 1 in steps of 0.1 (`0.2` or `0,2`); 0
/// disables the track (on that station, or everywhere), another ratio sets
/// it and enables the track; an empty cell changes nothing. Operators of
/// every station named, and the shared role for `default`.
#[utoipa::path(post, path = "/tracks/ratios/import", tag = "library",
    params(ImportParams),
    request_body(content = String, content_type = "text/csv"),
    responses((status = 200, body = RatioImport), (status = "4XX", body = ErrorBody)))]
async fn import_ratios(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(params): Query<ImportParams>,
    body: String,
) -> ApiResult<Json<RatioImport>> {
    let pool = &state.pool;
    let mut lines = parse_csv(&body, separator(&body)).into_iter();
    let Some((_, header)) = lines.next() else {
        return Err(ApiError::Invalid("the file is empty".to_owned()));
    };
    let header: Vec<String> = header
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    let by_path = header.iter().position(|name| name == "path");
    let by_name = header.iter().position(|name| name == "filename");
    let (name_column, matched_by) = match (by_path, by_name) {
        (Some(column), _) => (column, "path"),
        (None, Some(column)) => (column, "filename"),
        (None, None) => {
            return Err(ApiError::Invalid(
                "the file needs a path or a filename column".to_owned(),
            ));
        }
    };
    let stations: Vec<(u64, String)> = sqlx::query_as("SELECT id, code FROM stations")
        .fetch_all(pool)
        .await?;
    let mut report = RatioImport {
        dry_run: params.dry_run,
        matched_by: matched_by.to_owned(),
        ..RatioImport::default()
    };
    let mut targets: Vec<(usize, Target)> = Vec::new();
    for (index, name) in header.iter().enumerate() {
        if index == name_column || (name == "filename" || name == "path") {
            continue;
        }
        if targets.iter().any(|(other, _)| header[*other] == *name) {
            return Err(ApiError::Invalid(format!(
                "the column {name} is there twice"
            )));
        }
        let target = if name == "default" {
            user.require(Role::Operator)?;
            Some(Target::Default)
        } else {
            match stations.iter().find(|(_, code)| code == name) {
                // A station the user does not see is not one.
                Some((id, _)) if user.require_station(pool, *id).await.is_ok() => {
                    user.require_on(pool, *id, Role::Operator).await?;
                    Some(Target::Station(*id))
                }
                _ => None,
            }
        };
        match target {
            Some(target) => {
                targets.push((index, target));
                report.columns.push(name.clone());
            }
            None if name.is_empty() => {}
            None => report.unknown_columns.push(name.clone()),
        }
    }

    let tracks: Vec<(String, u64)> =
        sqlx::query_as("SELECT path, id FROM tracks WHERE deleted_at IS NULL")
            .fetch_all(pool)
            .await?;
    let mut found: HashMap<String, Vec<u64>> = HashMap::new();
    for (path, id) in tracks {
        let key = if matched_by == "path" {
            path
        } else {
            file_name(&path)
        };
        found.entry(key).or_default().push(id);
    }
    // Per column, the tracks of each ratio: the last row naming a track wins.
    let mut values: BTreeMap<Target, HashMap<u64, u8>> = BTreeMap::new();
    for (line, fields) in lines {
        report.rows += 1;
        let name = fields.get(name_column).map_or("", |name| name.trim());
        let key = if matched_by == "path" {
            name.to_owned()
        } else {
            file_name(name)
        };
        let ids = found.get(&key).map_or(&[][..], Vec::as_slice);
        let track_id = match ids {
            [id] => *id,
            _ => {
                report.unmatched_total += 1;
                if report.unmatched.len() < MAX_LISTED {
                    report.unmatched.push(UnmatchedRow {
                        line,
                        name: name.to_owned(),
                        reason: match ids.len() {
                            _ if name.is_empty() => Unmatched::Empty,
                            0 => Unmatched::NotFound,
                            _ => Unmatched::Several,
                        },
                    });
                }
                continue;
            }
        };
        report.matched += 1;
        for (column, target) in &targets {
            let value = fields.get(*column).map_or("", |value| value.trim());
            if value.is_empty() {
                continue;
            }
            match parse_ratio(value) {
                Some(ratio) => {
                    report.values += 1;
                    values.entry(*target).or_default().insert(track_id, ratio);
                }
                None => {
                    report.invalid_total += 1;
                    if report.invalid.len() < MAX_LISTED {
                        report.invalid.push(InvalidValue {
                            line,
                            column: header[*column].clone(),
                            value: value.to_owned(),
                        });
                    }
                }
            }
        }
    }
    if params.dry_run {
        return Ok(Json(report));
    }
    let mut tx = pool.begin().await?;
    for (target, tracks) in values {
        let mut by_ratio: BTreeMap<u8, Vec<u64>> = BTreeMap::new();
        for (track_id, ratio) in tracks {
            by_ratio.entry(ratio).or_default().push(track_id);
        }
        for (ratio, mut ids) in by_ratio {
            ids.sort_unstable();
            report.changed += match target {
                Target::Station(station_id) => {
                    let change = StationChange::new(Some(ratio > 0), Some(ratio), false)?;
                    change_on_station(&mut tx, &ids, station_id, change, user.id).await?
                }
                Target::Default if ratio == 0 => {
                    crate::selection::set_enabled_on(&mut tx, &ids, false, user.id).await?
                }
                Target::Default => {
                    let enabled =
                        crate::selection::set_enabled_on(&mut tx, &ids, true, user.id).await?;
                    enabled.max(crate::selection::set_play_ratio(&mut tx, &ids, ratio).await?)
                }
            };
        }
    }
    tx.commit().await?;
    tracing::info!(user = %user.username, rows = report.rows, matched = report.matched,
        changed = report.changed, "play ratios imported");
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_csv_with_quotes_and_either_separator() {
        let text = "\u{feff}path;fm;default\r\n\"A/B; C.flac\";0,2;\"1\"\r\n\r\nD.mp3;;0\n";
        assert_eq!(separator(text), ';');
        let lines = parse_csv(text, ';');
        assert_eq!(
            lines,
            [
                (1, vec!["path".into(), "fm".into(), "default".into()]),
                (2, vec!["A/B; C.flac".into(), "0,2".into(), "1".into()]),
                (4, vec!["D.mp3".into(), String::new(), "0".into()]),
            ]
        );
        let text = "filename,fm\n\"Say \"\"Hi\"\".mp3\",0.5";
        assert_eq!(separator(text), ',');
        assert_eq!(
            parse_csv(text, ',')[1],
            (2, vec!["Say \"Hi\".mp3".into(), "0.5".into()])
        );
    }

    #[test]
    fn ratios_are_tenths_from_zero_to_one() {
        assert_eq!(parse_ratio("0.2"), Some(2));
        assert_eq!(parse_ratio(" 0,3 "), Some(3));
        assert_eq!(parse_ratio("1"), Some(10));
        assert_eq!(parse_ratio("0"), Some(0));
        assert_eq!(parse_ratio("0.70"), Some(7));
        assert_eq!(parse_ratio("0.25"), None);
        assert_eq!(parse_ratio("2"), None);
        assert_eq!(parse_ratio("-0.1"), None);
        assert_eq!(parse_ratio("often"), None);
    }

    #[test]
    fn file_names_ignore_folders_and_case() {
        assert_eq!(file_name("Artist/Album/01 Title.FLAC"), "01 title.flac");
        assert_eq!(file_name("C:\\Music\\Song.mp3"), "song.mp3");
    }

    #[test]
    fn a_station_ratio_of_zero_disables_there() {
        assert_eq!(effective(true, 10, None, true), 10);
        assert_eq!(effective(true, 10, Some(3), true), 3);
        assert_eq!(effective(true, 4, Some(3), false), 0);
        assert_eq!(effective(false, 4, Some(3), true), 0);
        let change = StationChange::new(None, Some(0), false).unwrap();
        assert_eq!(change.enabled, Some(false));
        assert_eq!(change.play_ratio, None);
        assert!(StationChange::new(None, Some(3), true).is_err());
    }
}
