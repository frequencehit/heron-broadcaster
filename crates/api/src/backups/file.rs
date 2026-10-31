// SPDX-License-Identifier: AGPL-3.0-or-later

//! The configuration file: everything set by hand, what is set on each
//! track included, as JSON (docs/backups.md).
//!
//! Each table is saved as its rows, with their IDs: what the file holds
//! whole keeps its IDs on a restore, so what refers to it (in the file and
//! in the settings) still does. Tracks and artists are rebuilt by a scan
//! under other IDs: the file names them by path and by name instead.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{MySql, MySqlPool, QueryBuilder, Transaction};
use utoipa::ToSchema;

use crate::error::{ApiError, ApiResult};

/// The layout of the file; a Heron reads its own and older ones.
pub const FORMAT: u32 = 1;

/// Why a backup was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Daily,
    Manual,
    BeforeRestore,
    Uploaded,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Manual => "manual",
            Self::BeforeRestore => "before_restore",
            Self::Uploaded => "uploaded",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [
            Self::Daily,
            Self::Manual,
            Self::BeforeRestore,
            Self::Uploaded,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == name)
    }
}

/// The configuration file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub format: u32,
    pub heron_version: String,
    /// The last migration of the Heron that wrote it.
    pub schema: i64,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub kind: Kind,
    /// Each table's rows.
    pub tables: BTreeMap<String, Vec<Map<String, Value>>>,
    /// The library paths of the tracks turned off.
    #[serde(default)]
    pub disabled_tracks: Vec<String>,
}

#[derive(Clone, Copy)]
enum Col {
    Plain(&'static str),
    /// A JSON document stored as text: saved as JSON.
    Json(&'static str),
    /// Binary: saved in hexadecimal.
    Hex(&'static str),
    /// A track's ID: saved as its library path, under the column's name
    /// without `_id`.
    Track(&'static str),
    /// An artist's ID: saved as the artist's name, likewise.
    Artist(&'static str),
    /// A restriction's target: a track's path, an artist's name or a
    /// category's ID, after its `target`.
    Target,
}

impl Col {
    fn column(self) -> &'static str {
        match self {
            Self::Plain(name)
            | Self::Json(name)
            | Self::Hex(name)
            | Self::Track(name)
            | Self::Artist(name) => name,
            Self::Target => "target_id",
        }
    }

    /// Its key in the file.
    fn key(self) -> &'static str {
        match self {
            Self::Track(name) | Self::Artist(name) => name.strip_suffix("_id").unwrap_or(name),
            other => other.column(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Rows are updated in place by ID, those the file does not have
    /// deleted: what refers to them from outside the file (play history,
    /// listener statistics, the buffer zone) stays attached.
    Upsert,
    /// As `Upsert`, but rows the file does not have are kept (stations,
    /// with their history).
    Keep,
    /// Every row deleted, the file's inserted.
    Replace,
    /// As `Replace`; rows naming a track or an artist that is not found
    /// are left out and reported.
    Library,
}

struct Table {
    name: &'static str,
    cols: &'static [Col],
    mode: Mode,
    /// Unique text columns, renamed while rows are updated so that names
    /// can be swapped.
    unique: &'static [&'static str],
    /// Which rows the file holds.
    filter: Option<&'static str>,
}

use Col::{Artist, Hex, Json, Plain, Target, Track};

const fn table(
    name: &'static str,
    cols: &'static [Col],
    mode: Mode,
    unique: &'static [&'static str],
) -> Table {
    Table {
        name,
        cols,
        mode,
        unique,
        filter: None,
    }
}

/// The tables, parents before children.
const TABLES: &[Table] = &[
    table(
        "settings",
        &[Plain("name"), Json("value")],
        Mode::Replace,
        &[],
    ),
    table(
        "stations",
        &[
            Plain("id"),
            Plain("name"),
            Plain("code"),
            Plain("enabled"),
            Plain("position"),
        ],
        Mode::Keep,
        &["code"],
    ),
    table(
        "station_settings",
        &[Plain("station_id"), Plain("name"), Json("value")],
        Mode::Replace,
        &[],
    ),
    table(
        "users",
        &[
            Plain("id"),
            Plain("username"),
            Plain("password_hash"),
            Plain("role"),
            Plain("enabled"),
            Plain("theme"),
            Plain("language"),
            Plain("full_width"),
        ],
        Mode::Upsert,
        &["username"],
    ),
    table(
        "user_station_roles",
        &[Plain("user_id"), Plain("station_id"), Plain("role")],
        Mode::Replace,
        &[],
    ),
    table(
        "api_tokens",
        &[
            Plain("id"),
            Plain("user_id"),
            Plain("name"),
            Hex("token_hash"),
            Plain("created_at"),
            Plain("expires_at"),
            Plain("revoked_at"),
        ],
        Mode::Replace,
        &[],
    ),
    table(
        "encoder_profiles",
        &[
            Plain("id"),
            Plain("name"),
            Plain("encoder"),
            Plain("bitrate_kbps"),
            Plain("sample_rate"),
            Plain("channels"),
            Json("options"),
        ],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "outputs",
        &[
            Plain("id"),
            Plain("station_id"),
            Plain("name"),
            Plain("kind"),
            Plain("encoder_profile_id"),
            Json("config"),
            Json("statistics"),
            Plain("metadata_enabled"),
            Plain("enabled"),
        ],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "eq_presets",
        &[Plain("id"), Plain("name"), Json("gains_db")],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "category_types",
        &[
            Plain("id"),
            Plain("name"),
            Plain("no_two_in_a_row"),
            Plain("sends_metadata"),
            Plain("plays_over_intro"),
        ],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "categories",
        &[
            Plain("id"),
            Plain("name"),
            Plain("color"),
            Plain("type_id"),
            Plain("fx_player"),
            Json("separation"),
        ],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "category_stations",
        &[Plain("category_id"), Plain("station_id")],
        Mode::Replace,
        &[],
    ),
    table(
        "labels",
        &[Plain("id"), Plain("name"), Plain("color")],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "clocks",
        &[
            Plain("id"),
            Plain("station_id"),
            Plain("name"),
            Plain("looping"),
        ],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "clock_slots",
        &[
            Plain("id"),
            Plain("clock_id"),
            Plain("position"),
            Plain("category_id"),
            Plain("selection_method"),
            Plain("enforce_rules"),
            Json("separation"),
        ],
        Mode::Replace,
        &[],
    ),
    table(
        "schedules",
        &[
            Plain("id"),
            Plain("station_id"),
            Plain("name"),
            Plain("is_default"),
        ],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "schedule_periods",
        &[
            Plain("id"),
            Plain("schedule_id"),
            Plain("starts_at"),
            Plain("ends_at"),
            Plain("every_year"),
        ],
        Mode::Replace,
        &[],
    ),
    table(
        "clock_schedule",
        &[
            Plain("schedule_id"),
            Plain("day_of_week"),
            Plain("hour"),
            Plain("clock_id"),
        ],
        Mode::Replace,
        &[],
    ),
    table(
        "restrictions",
        &[
            Plain("id"),
            Plain("target"),
            Target,
            Json("rule"),
            Plain("enabled"),
            Plain("note"),
        ],
        Mode::Library,
        &[],
    ),
    table(
        "restriction_stations",
        &[Plain("restriction_id"), Plain("station_id")],
        Mode::Library,
        &[],
    ),
    table(
        "medleys",
        &[
            Plain("id"),
            Plain("name"),
            Json("recipe"),
            Plain("path"),
            Track("track_id"),
            Json("rendered_layout"),
            Plain("rendered_at"),
        ],
        Mode::Upsert,
        &["name"],
    ),
    table(
        "medley_stations",
        &[Plain("medley_id"), Plain("station_id"), Plain("position")],
        Mode::Replace,
        &[],
    ),
    table(
        "medley_parts",
        &[
            Plain("medley_id"),
            Plain("position"),
            Plain("kind"),
            Track("track_id"),
            Plain("from_ms"),
            Plain("to_ms"),
            Plain("shift_ms"),
            Plain("move_ms"),
        ],
        Mode::Library,
        &[],
    ),
    table(
        "webhooks",
        &[
            Plain("id"),
            Plain("station_id"),
            Plain("name"),
            Plain("url"),
            Plain("enabled"),
            Json("events"),
            Plain("include_fx"),
            Plain("secret"),
            Plain("content_type"),
            Plain("body_template"),
        ],
        Mode::Upsert,
        &["name"],
    ),
    // What is set on each track.
    table(
        "track_categories",
        &[Track("track_id"), Plain("category_id"), Plain("weight")],
        Mode::Library,
        &[],
    ),
    table(
        "track_labels",
        &[Track("track_id"), Plain("label_id")],
        Mode::Library,
        &[],
    ),
    table(
        "track_overrides",
        &[
            Track("track_id"),
            Plain("cue_in_ms"),
            Plain("cue_out_ms"),
            Json("crossfade"),
            Plain("gap_killer_enabled"),
        ],
        Mode::Library,
        &[],
    ),
    table(
        "track_markers",
        &[
            Track("track_id"),
            Plain("intro_ms"),
            Plain("intro2_ms"),
            Plain("hook_in_ms"),
            Plain("hook_out_ms"),
            Plain("next_soft_ms"),
            Plain("next_hard_ms"),
        ],
        Mode::Library,
        &[],
    ),
    table(
        "track_programming",
        &[
            Track("track_id"),
            Plain("bpm"),
            Plain("mood"),
            Plain("sound_code"),
        ],
        Mode::Library,
        &[],
    ),
    table(
        "track_tag_overrides",
        &[
            Track("track_id"),
            Plain("field"),
            Plain("value"),
            Plain("updated_by"),
        ],
        Mode::Library,
        &[],
    ),
    // Artists curated by hand.
    Table {
        name: "artist_similar",
        cols: &[Artist("artist_id"), Artist("similar_id")],
        mode: Mode::Library,
        unique: &[],
        filter: None,
    },
    Table {
        name: "artist_suggestions",
        cols: &[
            Artist("artist_id"),
            Artist("other_id"),
            Plain("source"),
            Plain("reason"),
            Plain("status"),
        ],
        mode: Mode::Library,
        unique: &[],
        // Pending suggestions are found again.
        filter: Some("status <> 'pending'"),
    },
];

/// A name or a code, set aside while the rows are written.
fn set_aside(column: &str) -> String {
    format!("{column} = CONCAT(CHAR(1), id)")
}

/// What a column is read as, in the `JSON_OBJECT` of its row.
fn select(col: Col) -> String {
    match col {
        Col::Plain(name) => format!("t.{name}"),
        Col::Json(name) => format!("JSON_EXTRACT(t.{name}, '$')"),
        Col::Hex(name) => format!("HEX(t.{name})"),
        Col::Track(name) => format!("(SELECT path FROM tracks WHERE id = t.{name})"),
        Col::Artist(name) => format!("(SELECT name FROM artists WHERE id = t.{name})"),
        Col::Target => "CASE t.target \
             WHEN 'track' THEN (SELECT path FROM tracks WHERE id = t.target_id) \
             WHEN 'artist' THEN (SELECT name FROM artists WHERE id = t.target_id) \
             ELSE t.target_id END"
            .to_owned(),
    }
}

/// Reads the configuration, as one snapshot of the database.
pub async fn export(pool: &MySqlPool, kind: Kind, created_at: String) -> ApiResult<ConfigFile> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .ok();
    let mut tables = BTreeMap::new();
    for table in TABLES {
        let fields: Vec<String> = table
            .cols
            .iter()
            .map(|col| format!("'{}', {}", col.key(), select(*col)))
            .collect();
        let filter = table
            .filter
            .map(|filter| format!(" WHERE {filter}"))
            .unwrap_or_default();
        let order: Vec<&str> = table.cols.iter().map(|col| col.column()).collect();
        let sql = format!(
            "SELECT JSON_OBJECT({}) FROM {} t{filter} ORDER BY {}",
            fields.join(", "),
            table.name,
            order
                .iter()
                .map(|column| format!("t.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_all(&mut *tx)
            .await?;
        let rows = rows
            .iter()
            .map(|row| serde_json::from_str(row))
            .collect::<Result<Vec<Map<String, Value>>, _>>()
            .map_err(|error| ApiError::Internal(format!("{}: {error}", table.name)))?;
        tables.insert(table.name.to_owned(), rows);
    }
    let disabled_tracks: Vec<String> =
        sqlx::query_scalar("SELECT path FROM tracks WHERE NOT enabled ORDER BY path")
            .fetch_all(&mut *tx)
            .await?;
    let schema: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success")
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(ConfigFile {
        format: FORMAT,
        heron_version: env!("CARGO_PKG_VERSION").to_owned(),
        schema,
        created_at,
        kind,
        tables,
        disabled_tracks,
    })
}

/// What a restore did.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct RestoreReport {
    /// The backup taken just before.
    pub before_restore: String,
    /// Rows written, by table.
    pub restored: BTreeMap<String, usize>,
    /// Rows left out, by table: they name a track or an artist the library
    /// does not have.
    pub left_out: BTreeMap<String, usize>,
    /// The library paths that were not found (the first 100).
    pub missing_tracks: Vec<String>,
    pub missing_tracks_total: usize,
    /// The artists that were not found (the first 100).
    pub missing_artists: Vec<String>,
    pub missing_artists_total: usize,
}

/// Checks a file before anything is changed.
pub fn check(file: &ConfigFile, schema: i64) -> ApiResult<()> {
    if file.format > FORMAT {
        return Err(ApiError::Invalid(format!(
            "this backup is of format {}, written by Heron {}: this Heron reads format {FORMAT} \
             and older, upgrade it first",
            file.format, file.heron_version
        )));
    }
    if file.schema > schema {
        return Err(ApiError::Invalid(format!(
            "this backup was written by Heron {}, whose database is newer than this one's: \
             upgrade this Heron first",
            file.heron_version
        )));
    }
    for (name, rows) in &file.tables {
        let Some(table) = TABLES.iter().find(|table| table.name == name) else {
            return Err(ApiError::Invalid(format!("unknown table {name}")));
        };
        let keys: HashSet<&str> = table.cols.iter().map(|col| col.key()).collect();
        for row in rows {
            if let Some(key) = row.keys().find(|key| !keys.contains(key.as_str())) {
                return Err(ApiError::Invalid(format!("{name}: unknown column {key}")));
            }
            for value in row.values() {
                if value.is_array() && !table.cols.iter().any(|col| matches!(col, Col::Json(_))) {
                    return Err(ApiError::Invalid(format!("{name}: unexpected list")));
                }
            }
        }
    }
    Ok(())
}

/// The IDs of the library's tracks and artists, by path and name.
struct Library {
    tracks: HashMap<String, u64>,
    artists: HashMap<String, u64>,
    missing_tracks: BTreeMap<String, ()>,
    missing_artists: BTreeMap<String, ()>,
}

impl Library {
    async fn load(tx: &mut Transaction<'_, MySql>) -> ApiResult<Self> {
        let tracks: Vec<(String, u64)> = sqlx::query_as("SELECT path, id FROM tracks")
            .fetch_all(&mut **tx)
            .await?;
        let artists: Vec<(String, u64)> =
            sqlx::query_as("SELECT name, MIN(id) FROM artists GROUP BY name")
                .fetch_all(&mut **tx)
                .await?;
        Ok(Self {
            tracks: tracks.into_iter().collect(),
            artists: artists.into_iter().collect(),
            missing_tracks: BTreeMap::new(),
            missing_artists: BTreeMap::new(),
        })
    }

    fn track(&mut self, path: &Value) -> Option<u64> {
        let path = path.as_str()?;
        let id = self.tracks.get(path).copied();
        if id.is_none() {
            self.missing_tracks.insert(path.to_owned(), ());
        }
        id
    }

    fn artist(&mut self, name: &Value) -> Option<u64> {
        let name = name.as_str()?;
        let id = self.artists.get(name).copied();
        if id.is_none() {
            self.missing_artists.insert(name.to_owned(), ());
        }
        id
    }
}

/// A value to write, its references resolved: `None` when the track or
/// artist it names is gone.
fn resolve(col: Col, row: &Map<String, Value>, library: &mut Library) -> Option<Value> {
    let value = row.get(col.key()).cloned().unwrap_or(Value::Null);
    match col {
        Col::Track(_) if !value.is_null() => library.track(&value).map(Value::from),
        Col::Artist(_) if !value.is_null() => library.artist(&value).map(Value::from),
        Col::Target => match row.get("target").and_then(Value::as_str) {
            Some("track") => library.track(&value).map(Value::from),
            Some("artist") => library.artist(&value).map(Value::from),
            _ => Some(value),
        },
        Col::Json(_) if !value.is_null() => Some(Value::String(value.to_string())),
        _ => Some(value),
    }
}

fn push_value(query: &mut QueryBuilder<MySql>, col: Col, value: Value) {
    let hex = matches!(col, Col::Hex(_));
    if hex {
        query.push("UNHEX(");
    }
    match value {
        Value::Null => {
            query.push("NULL");
        }
        Value::Bool(value) => {
            query.push_bind(value);
        }
        Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                query.push_bind(value);
            } else if let Some(value) = number.as_i64() {
                query.push_bind(value);
            } else {
                query.push_bind(number.as_f64().unwrap_or_default());
            }
        }
        Value::String(value) => {
            query.push_bind(value);
        }
        other => {
            query.push_bind(other.to_string());
        }
    }
    if hex {
        query.push(")");
    }
}

/// Writes the file's configuration over the database's, in `tx`.
pub async fn restore(
    tx: &mut Transaction<'_, MySql>,
    file: &ConfigFile,
    report: &mut RestoreReport,
) -> ApiResult<()> {
    let mut library = Library::load(tx).await?;
    let empty = Vec::new();
    let rows_of = |table: &Table| file.tables.get(table.name).unwrap_or(&empty);

    // Tables replaced whole are emptied first, children before parents.
    for table in TABLES.iter().rev() {
        if matches!(table.mode, Mode::Replace | Mode::Library) {
            let filter = table
                .filter
                .map(|filter| format!(" WHERE {filter}"))
                .unwrap_or_default();
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DELETE FROM {}{filter}",
                table.name
            )))
            .execute(&mut **tx)
            .await?;
        }
    }
    // Names and codes set aside, so that rows can swap them. Stations the
    // file does not have keep theirs.
    for table in TABLES {
        for column in table.unique {
            let mut query: QueryBuilder<MySql> =
                QueryBuilder::new(format!("UPDATE {} SET {}", table.name, set_aside(column)));
            if table.mode == Mode::Keep {
                query.push(" WHERE id IN (0");
                for row in rows_of(table) {
                    query.push(", ");
                    query.push_bind(row.get("id").and_then(Value::as_u64).unwrap_or(0));
                }
                query.push(")");
            }
            query.build().execute(&mut **tx).await?;
        }
    }
    // The file's rows, parents before children.
    let mut deletes = Vec::new();
    for table in TABLES {
        let mut left_out = 0;
        let columns: Vec<&str> = table.cols.iter().map(|col| col.column()).collect();
        let mut resolved: Vec<Vec<Value>> = Vec::new();
        for row in rows_of(table) {
            let values: Option<Vec<Value>> = table
                .cols
                .iter()
                .map(|col| resolve(*col, row, &mut library))
                .collect();
            match values {
                Some(values) => resolved.push(values),
                None => left_out += 1,
            }
        }
        // A restriction left out leaves its stations out too.
        if table.name == "restriction_stations" {
            let kept: HashSet<u64> = sqlx::query_scalar("SELECT id FROM restrictions")
                .fetch_all(&mut **tx)
                .await?
                .into_iter()
                .collect();
            let before = resolved.len();
            resolved.retain(|values| values[0].as_u64().is_some_and(|id| kept.contains(&id)));
            left_out += before - resolved.len();
        }
        for chunk in resolved.chunks(500) {
            let mut query: QueryBuilder<MySql> = QueryBuilder::new(format!(
                "INSERT INTO {} ({}) VALUES ",
                table.name,
                columns.join(", ")
            ));
            for (index, values) in chunk.iter().enumerate() {
                query.push(if index == 0 { "(" } else { ", (" });
                for (position, (col, value)) in table.cols.iter().zip(values).enumerate() {
                    if position > 0 {
                        query.push(", ");
                    }
                    push_value(&mut query, *col, value.clone());
                }
                query.push(")");
            }
            if matches!(table.mode, Mode::Upsert | Mode::Keep) {
                let updates: Vec<String> = columns
                    .iter()
                    .filter(|column| **column != "id")
                    .map(|column| format!("{column} = VALUES({column})"))
                    .collect();
                query.push(" ON DUPLICATE KEY UPDATE ");
                query.push(updates.join(", "));
            }
            query.build().execute(&mut **tx).await.map_err(|error| {
                ApiError::Invalid(format!("{}: cannot restore its rows: {error}", table.name))
            })?;
        }
        if table.mode == Mode::Upsert {
            let ids: Vec<u64> = rows_of(table)
                .iter()
                .filter_map(|row| row.get("id").and_then(Value::as_u64))
                .collect();
            deletes.push((table.name, ids));
        }
        report
            .restored
            .insert(table.name.to_owned(), resolved.len());
        if left_out > 0 {
            report.left_out.insert(table.name.to_owned(), left_out);
        }
    }
    // Rows the file does not have, children first: a row kept may have
    // referred to one that goes until the file's rows were written.
    for (name, ids) in deletes.into_iter().rev() {
        let mut query: QueryBuilder<MySql> = QueryBuilder::new(format!("DELETE FROM {name}"));
        if !ids.is_empty() {
            query.push(" WHERE id NOT IN (");
            let mut separated = query.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
            query.push(")");
        }
        query.build().execute(&mut **tx).await.map_err(|error| {
            ApiError::Invalid(format!(
                "{name}: cannot delete what the backup does not have: {error}"
            ))
        })?;
    }
    // Tracks turned off.
    sqlx::query("UPDATE tracks SET enabled = TRUE WHERE NOT enabled")
        .execute(&mut **tx)
        .await?;
    let mut disabled = 0;
    for path in &file.disabled_tracks {
        if let Some(id) = library.track(&Value::from(path.as_str())) {
            sqlx::query("UPDATE tracks SET enabled = FALSE WHERE id = ?")
                .bind(id)
                .execute(&mut **tx)
                .await?;
            disabled += 1;
        }
    }
    report
        .restored
        .insert("disabled_tracks".to_owned(), disabled);
    let left = file.disabled_tracks.len() - disabled;
    if left > 0 {
        report.left_out.insert("disabled_tracks".to_owned(), left);
    }
    report.missing_tracks_total = library.missing_tracks.len();
    report.missing_tracks = library.missing_tracks.into_keys().take(100).collect();
    report.missing_artists_total = library.missing_artists.len();
    report.missing_artists = library.missing_artists.into_keys().take(100).collect();
    Ok(())
}
