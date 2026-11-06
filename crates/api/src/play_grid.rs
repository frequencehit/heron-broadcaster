// SPDX-License-Identifier: AGPL-3.0-or-later

//! When a track played: its plays of the last 30 days, day by day, and the
//! hours of the week it played in, read from `play_history` for the grids
//! of the track page (`docs/api.md#plays-by-day-and-hour`). Nothing is
//! stored.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use heron_core::PlaySource;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::{self, ProgrammingSettings, StationSettings};

/// The days of the day-by-day grid, ending today.
pub const GRID_DAYS: u32 = 30;
/// The longest period of the hours of the week.
pub const MAX_DAYS: u32 = 365;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_play_grid))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GridQuery {
    /// This station's plays; every station's, added up, when left out.
    pub station_id: Option<u64>,
    /// The period of the hours of the week: 1 to 365 days ending today, 30
    /// by default.
    pub days: Option<u32>,
    /// Only the plays of this source.
    pub source: Option<PlaySource>,
}

/// One play, at its local time.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct GridPlay {
    pub started_at_unix_ms: i64,
    /// `2026-10-12`, in the grid's time zone.
    pub date: String,
    /// `08:12`, in the grid's time zone.
    pub time: String,
    pub station_id: u64,
    pub source: PlaySource,
}

/// A day of the last 30.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct GridDay {
    /// `2026-10-12`.
    pub date: String,
    /// 1 = Monday to 7 = Sunday.
    pub weekday: u8,
    /// Where the day starts and ends, for the history.
    pub start_unix_ms: i64,
    pub end_unix_ms: i64,
    /// Its plays, in order.
    pub plays: Vec<GridPlay>,
}

/// An hour of the week the track played in.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct GridHour {
    /// 1 = Monday to 7 = Sunday.
    pub weekday: u8,
    /// 0 to 23, as the clock shows it.
    pub hour: u8,
    /// Its plays, the latest first.
    pub plays: Vec<GridPlay>,
}

/// When a track played, by day and by hour of the week.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PlayGrid {
    /// The IANA time zone of the dates and times.
    pub time_zone: String,
    /// `2026-11-05`.
    pub today: String,
    /// The 30 days ending today, oldest first.
    pub days: Vec<GridDay>,
    /// The first day of the hours' period.
    pub hours_from: String,
    /// The hours of the week with plays, by weekday then hour.
    pub hours: Vec<GridHour>,
}

/// Which plays, seen from which time zone.
pub struct GridRequest {
    pub track_id: u64,
    /// Every station's when `None`.
    pub station_id: Option<u64>,
    /// The station whose time zone the grid follows.
    pub zone_station_id: u64,
    /// The period of the hours of the week.
    pub days: u32,
    pub source: Option<PlaySource>,
}

fn internal(error: jiff::Error) -> ApiError {
    ApiError::Internal(error.to_string())
}

fn start_of(date: Date, zone: &TimeZone) -> ApiResult<Timestamp> {
    Ok(date.to_zoned(zone.clone()).map_err(internal)?.timestamp())
}

fn days_before(date: Date, days: u32) -> ApiResult<Date> {
    date.checked_sub(i64::from(days).days()).map_err(internal)
}

fn source_from(text: &str) -> PlaySource {
    match text {
        "rotation" => PlaySource::Rotation,
        "request" => PlaySource::Request,
        "fallback" => PlaySource::Fallback,
        _ => PlaySource::Manual,
    }
}

fn source_text(source: PlaySource) -> &'static str {
    match source {
        PlaySource::Rotation => "rotation",
        PlaySource::Request => "request",
        PlaySource::Fallback => "fallback",
        PlaySource::Manual => "manual",
    }
}

/// The plays that count (as the track's play count counts them), from the
/// start of the period to the end of today, by day and by hour.
pub async fn grid(pool: &MySqlPool, request: &GridRequest, now: Timestamp) -> ApiResult<PlayGrid> {
    let programming: ProgrammingSettings =
        settings::load(pool, ProgrammingSettings::SETTINGS_NAME).await?;
    let station: StationSettings = settings::load_for(
        pool,
        request.zone_station_id,
        StationSettings::SETTINGS_NAME,
    )
    .await?;
    let zone = station.time_zone();
    let today = now.to_zoned(zone.clone()).date();
    let first_day = days_before(today, GRID_DAYS - 1)?;
    let hours_from = days_before(today, request.days - 1)?;
    let start = start_of(first_day.min(hours_from), &zone)?;
    let end = start_of(today.tomorrow().map_err(internal)?, &zone)?;

    let mut sql = sqlx::QueryBuilder::new(
        "SELECT CAST(UNIX_TIMESTAMP(h.started_at) * 1000 AS SIGNED), h.station_id,
                CAST(h.source AS CHAR)
         FROM play_history h
         WHERE h.track_id = ",
    );
    sql.push_bind(request.track_id)
        .push(" AND h.deck <> 'fx' AND h.started_at >= FROM_UNIXTIME(")
        .push_bind(start.as_millisecond() as f64 / 1_000.0)
        .push(") AND h.started_at < FROM_UNIXTIME(")
        .push_bind(end.as_millisecond() as f64 / 1_000.0)
        .push(") AND (h.played_ms >= ")
        .push_bind(programming.min_play_s * 1000)
        .push(" OR (h.played_ms IS NULL AND h.started_at <= FROM_UNIXTIME(")
        .push_bind(now.as_millisecond() as f64 / 1_000.0 - f64::from(programming.min_play_s))
        .push(")))");
    if let Some(station_id) = request.station_id {
        sql.push(" AND h.station_id = ").push_bind(station_id);
    }
    if let Some(source) = request.source {
        sql.push(" AND h.source = ").push_bind(source_text(source));
    }
    sql.push(" ORDER BY h.started_at, h.id");
    let rows: Vec<(i64, u64, String)> = sql.build_query_as().fetch_all(pool).await?;

    let mut by_day: BTreeMap<Date, Vec<GridPlay>> = BTreeMap::new();
    let mut by_hour: BTreeMap<(u8, u8), Vec<GridPlay>> = BTreeMap::new();
    for (started_at_unix_ms, station_id, source) in rows {
        let local = Timestamp::from_millisecond(started_at_unix_ms)
            .map_err(internal)?
            .to_zoned(zone.clone());
        let date = local.date();
        let play = GridPlay {
            started_at_unix_ms,
            date: date.to_string(),
            time: local.strftime("%H:%M").to_string(),
            station_id,
            source: source_from(&source),
        };
        if date >= hours_from {
            // The hour the clock shows: when it goes back, both 02:00 hours.
            let weekday = local.weekday().to_monday_one_offset().unsigned_abs();
            let hour = local.hour().unsigned_abs();
            by_hour
                .entry((weekday, hour))
                .or_default()
                .push(play.clone());
        }
        if date >= first_day {
            by_day.entry(date).or_default().push(play);
        }
    }

    let mut days = Vec::with_capacity(GRID_DAYS as usize);
    let mut date = first_day;
    while date <= today {
        let next = date.tomorrow().map_err(internal)?;
        days.push(GridDay {
            date: date.to_string(),
            weekday: date.weekday().to_monday_one_offset().unsigned_abs(),
            start_unix_ms: start_of(date, &zone)?.as_millisecond(),
            end_unix_ms: start_of(next, &zone)?.as_millisecond(),
            plays: by_day.remove(&date).unwrap_or_default(),
        });
        date = next;
    }
    let hours = by_hour
        .into_iter()
        .map(|((weekday, hour), mut plays)| {
            plays.reverse();
            GridHour {
                weekday,
                hour,
                plays,
            }
        })
        .collect();
    Ok(PlayGrid {
        time_zone: station.timezone,
        today: today.to_string(),
        days,
        hours_from: hours_from.to_string(),
        hours,
    })
}

/// When a track played: its plays of the last 30 days, day by day, and the
/// hours of the week it played in over `days` days, in the station's time
/// zone (the first station's with every station).
#[utoipa::path(get, path = "/tracks/{id}/plays/grid", tag = "library",
    params(("id" = u64, Path), GridQuery),
    responses((status = 200, body = PlayGrid), (status = "4XX", body = ErrorBody)))]
async fn get_play_grid(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(track_id): Path<u64>,
    Query(query): Query<GridQuery>,
) -> ApiResult<Json<PlayGrid>> {
    let days = query.days.unwrap_or(GRID_DAYS);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(ApiError::BadRequest(format!("days is 1 to {MAX_DAYS}")));
    }
    let track: Option<u64> = sqlx::query_scalar("SELECT id FROM tracks WHERE id = ?")
        .bind(track_id)
        .fetch_optional(&state.pool)
        .await?;
    if track.is_none() {
        return Err(ApiError::not_found(format!("track {track_id}")));
    }
    let zone_station_id = match query.station_id {
        Some(id) => {
            user.require_station(&state.pool, id).await?;
            id
        }
        None => state.stations.first().id(),
    };
    let request = GridRequest {
        track_id,
        station_id: query.station_id,
        zone_station_id,
        days,
        source: query.source,
    };
    Ok(Json(grid(&state.pool, &request, Timestamp::now()).await?))
}

#[cfg(test)]
mod tests {
    use heron_service::db::ScratchDatabase;

    use super::*;

    struct Fixture {
        db: ScratchDatabase,
        track: u64,
        second: u64,
    }

    /// A track and two stations in Paris.
    async fn fixture() -> Option<Fixture> {
        let db = ScratchDatabase::create().await?;
        let track = sqlx::query("INSERT INTO tracks (path) VALUES ('a.mp3')")
            .execute(&db.pool)
            .await
            .unwrap()
            .last_insert_id();
        let second = sqlx::query(
            "INSERT INTO stations (name, code, position) VALUES ('Second', 'second', 1)",
        )
        .execute(&db.pool)
        .await
        .unwrap()
        .last_insert_id();
        for station in [1, second] {
            let paris = StationSettings {
                timezone: "Europe/Paris".to_owned(),
                public_url: None,
            };
            settings::save_for(&db.pool, station, StationSettings::SETTINGS_NAME, &paris)
                .await
                .unwrap();
        }
        Some(Fixture { db, track, second })
    }

    impl Fixture {
        /// A play of the track at `at` (RFC 3339), on deck A unless `fx`.
        async fn play(&self, station: u64, at: &str, source: &str, played_ms: Option<u32>) {
            self.play_on(station, at, source, played_ms, "a").await;
        }

        async fn play_on(
            &self,
            station: u64,
            at: &str,
            source: &str,
            played_ms: Option<u32>,
            deck: &str,
        ) {
            let at: Timestamp = at.parse().unwrap();
            sqlx::query(
                "INSERT INTO play_history (station_id, track_id, started_at, played_ms, deck, source)
                 VALUES (?, ?, FROM_UNIXTIME(?), ?, ?, ?)",
            )
            .bind(station)
            .bind(self.track)
            .bind(at.as_millisecond() as f64 / 1_000.0)
            .bind(played_ms)
            .bind(deck)
            .bind(source)
            .execute(&self.db.pool)
            .await
            .unwrap();
        }

        async fn grid(
            &self,
            station_id: Option<u64>,
            days: u32,
            source: Option<PlaySource>,
            now: &str,
        ) -> PlayGrid {
            let request = GridRequest {
                track_id: self.track,
                station_id,
                zone_station_id: station_id.unwrap_or(1),
                days,
                source,
            };
            grid(&self.db.pool, &request, now.parse().unwrap())
                .await
                .unwrap()
        }
    }

    fn counts(grid: &PlayGrid) -> Vec<(String, usize)> {
        grid.days
            .iter()
            .filter(|day| !day.plays.is_empty())
            .map(|day| (day.date.clone(), day.plays.len()))
            .collect()
    }

    fn hours(grid: &PlayGrid) -> Vec<(u8, u8, Vec<String>)> {
        grid.hours
            .iter()
            .map(|cell| {
                let plays = cell
                    .plays
                    .iter()
                    .map(|play| format!("{} {}", play.date, play.time))
                    .collect();
                (cell.weekday, cell.hour, plays)
            })
            .collect()
    }

    /// Counts per day and per hour, one station or all, all plays or
    /// rotation only; the plays that do not count are left out.
    #[tokio::test]
    async fn counts_per_day_and_hour_per_station_and_source() {
        let Some(f) = fixture().await else {
            return;
        };
        let second = f.second;
        // Thursday 5 November 2026, 15:00 in Paris (UTC+1).
        let now = "2026-11-05T14:00:00Z";
        // Monday 2 November, 08:12 and 08:40 in Paris.
        f.play(1, "2026-11-02T07:12:00Z", "rotation", Some(200_000))
            .await;
        f.play(1, "2026-11-02T07:40:00Z", "request", Some(200_000))
            .await;
        // Monday 26 October, 08:05, on the second station, by hand.
        f.play(second, "2026-10-26T07:05:00Z", "manual", Some(200_000))
            .await;
        // Today, started long enough ago, still playing; another just
        // started does not count yet.
        f.play(1, "2026-11-05T13:58:00Z", "rotation", None).await;
        f.play(1, "2026-11-05T13:59:50Z", "rotation", None).await;
        // Not counted: too short, on the FX player, before the 30 days
        // (6 October), tomorrow.
        f.play(1, "2026-11-01T10:00:00Z", "rotation", Some(5_000))
            .await;
        f.play_on(1, "2026-11-01T11:00:00Z", "rotation", Some(200_000), "fx")
            .await;
        f.play(1, "2026-10-06T10:00:00Z", "rotation", Some(200_000))
            .await;
        f.play(1, "2026-11-05T23:30:00Z", "rotation", Some(200_000))
            .await;

        let grid = f.grid(Some(1), 30, None, now).await;
        assert_eq!(grid.time_zone, "Europe/Paris");
        assert_eq!(grid.today, "2026-11-05");
        assert_eq!(grid.days.len(), 30);
        assert_eq!(grid.days[0].date, "2026-10-07");
        assert_eq!(grid.days[0].weekday, 3);
        assert_eq!(grid.days[29].date, "2026-11-05");
        assert_eq!(
            counts(&grid),
            [("2026-11-02".to_owned(), 2), ("2026-11-05".to_owned(), 1)]
        );
        let monday = grid.days.iter().find(|d| d.date == "2026-11-02").unwrap();
        // Midnight to midnight in Paris.
        assert_eq!(
            monday.start_unix_ms,
            "2026-11-01T23:00:00Z"
                .parse::<Timestamp>()
                .unwrap()
                .as_millisecond()
        );
        assert_eq!(monday.end_unix_ms - monday.start_unix_ms, 86_400_000);
        let plays: Vec<(&str, PlaySource)> = monday
            .plays
            .iter()
            .map(|p| (p.time.as_str(), p.source))
            .collect();
        assert_eq!(
            plays,
            [
                ("08:12", PlaySource::Rotation),
                ("08:40", PlaySource::Request)
            ]
        );
        assert_eq!(
            hours(&f.grid(Some(1), 30, None, now).await),
            [
                (
                    1,
                    8,
                    vec!["2026-11-02 08:40".to_owned(), "2026-11-02 08:12".to_owned()]
                ),
                (4, 14, vec!["2026-11-05 14:58".to_owned()]),
            ]
        );

        // Rotation only.
        let rotation = f.grid(Some(1), 30, Some(PlaySource::Rotation), now).await;
        assert_eq!(
            counts(&rotation),
            [("2026-11-02".to_owned(), 1), ("2026-11-05".to_owned(), 1)]
        );

        // The second station; all stations, added up, each play with its
        // station.
        let on_second = f.grid(Some(second), 30, None, now).await;
        assert_eq!(counts(&on_second), [("2026-10-26".to_owned(), 1)]);
        let all = f.grid(None, 30, None, now).await;
        assert_eq!(
            counts(&all),
            [
                ("2026-10-26".to_owned(), 1),
                ("2026-11-02".to_owned(), 2),
                ("2026-11-05".to_owned(), 1)
            ]
        );
        let monday_8 = all
            .hours
            .iter()
            .find(|c| c.weekday == 1 && c.hour == 8)
            .unwrap();
        let stations: Vec<u64> = monday_8.plays.iter().map(|p| p.station_id).collect();
        assert_eq!(stations, [1, 1, second]);

        // A shorter period for the hours keeps the 30 days.
        let week = f.grid(Some(1), 7, None, now).await;
        assert_eq!(week.days.len(), 30);
        assert_eq!(week.hours_from, "2026-10-30");
        assert_eq!(hours(&week).len(), 2);
        // A longer one reaches further back.
        let year = f.grid(Some(1), 365, None, now).await;
        assert_eq!(year.days.len(), 30);
        assert_eq!(year.hours_from, "2025-11-06");
        assert!(
            hours(&year)
                .iter()
                .any(|(_, _, plays)| plays == &["2026-10-06 12:00"])
        );
        f.db.drop().await;
    }

    /// When the clocks go back, both 02:00 hours go in the 02:00 row; when
    /// they go forward, that day has no 02:00.
    #[tokio::test]
    async fn clock_changes_in_paris() {
        let Some(f) = fixture().await else {
            return;
        };
        // Sunday 29 March 2026: 02:00 becomes 03:00 at 01:00 UTC.
        f.play(1, "2026-03-29T00:30:00Z", "rotation", Some(200_000))
            .await; // 01:30
        f.play(1, "2026-03-29T01:10:00Z", "rotation", Some(200_000))
            .await; // 03:10
        // Sunday 25 October 2026: 03:00 becomes 02:00 at 01:00 UTC.
        f.play(1, "2026-10-25T00:20:00Z", "rotation", Some(200_000))
            .await; // 02:20, summer time
        f.play(1, "2026-10-25T01:20:00Z", "rotation", Some(200_000))
            .await; // 02:20, winter time
        f.play(1, "2026-10-25T02:10:00Z", "rotation", Some(200_000))
            .await; // 03:10

        let spring = f.grid(Some(1), 30, None, "2026-04-05T10:00:00Z").await;
        assert_eq!(
            hours(&spring),
            [
                (7, 1, vec!["2026-03-29 01:30".to_owned()]),
                (7, 3, vec!["2026-03-29 03:10".to_owned()]),
            ]
        );
        let short_day = spring.days.iter().find(|d| d.date == "2026-03-29").unwrap();
        assert_eq!(
            short_day.end_unix_ms - short_day.start_unix_ms,
            23 * 3_600_000
        );

        let autumn = f.grid(Some(1), 30, None, "2026-11-05T10:00:00Z").await;
        assert_eq!(
            hours(&autumn),
            [
                (
                    7,
                    2,
                    vec!["2026-10-25 02:20".to_owned(), "2026-10-25 02:20".to_owned()]
                ),
                (7, 3, vec!["2026-10-25 03:10".to_owned()]),
            ]
        );
        let long_day = autumn.days.iter().find(|d| d.date == "2026-10-25").unwrap();
        assert_eq!(long_day.plays.len(), 3);
        assert_eq!(
            long_day.end_unix_ms - long_day.start_unix_ms,
            25 * 3_600_000
        );

        // Both in a year.
        let year = f.grid(Some(1), 365, None, "2026-11-05T10:00:00Z").await;
        let sunday_2: usize = year
            .hours
            .iter()
            .filter(|c| c.weekday == 7 && c.hour == 2)
            .map(|c| c.plays.len())
            .sum();
        assert_eq!(sunday_2, 2);
        f.db.drop().await;
    }
}
