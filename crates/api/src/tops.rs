// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tops (`docs/listener-interaction.md#tops`): several charts per
//! station, each ranking the tracks of a period by their plays and likes,
//! frozen when the period ends, then played on air in its show, from the
//! last rank to number 1, then a new release.
//!
//! - A top's period is the last 7 days (frozen when a show's programme is
//!   built), or a week or a month from a reset to the next (frozen at the
//!   reset). Its chart is ranked from `play_history` and `play_votes`, and
//!   kept when it is frozen (`top_charts`, `top_entries`).
//! - A top's show is a recurrence with hours; on a slot two tops share, the
//!   rarer recurrence plays ([`openings_of`]).
//! - A show's programme is built when the queue manager first needs a
//!   track planned in it ([`due`]), kept (`top_shows`) and queued in order:
//!   no separation rules, the restrictions checked. Nothing a top does may
//!   stop the queue fill: a top that cannot be read is left out.
//! - The gateway's state hides the ranks still to come while a show plays
//!   ([`public_state`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use heron_core::Module;
use heron_core::shows::{
    self as core_shows, HourRange, NotificationSettings, Opening, Show, ShowPublic, ShowSettings,
    ShowStatus, WebsiteSettings,
};
use heron_rotation::Library;
use heron_rotation::restrict::{Subject, forbidding};
use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan};
use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, QueryBuilder};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::{self, StationSettings};
use crate::shows::{Notice, WebhookChoice};
use crate::stations::{Station, Stations};
use crate::webhooks::{WebhookEvent, Webhooks};

/// How often the background work looks for charts to freeze and ranks
/// that started playing.
const TICK: Duration = Duration::from_secs(15);
/// The sizes a top can have.
pub const SIZES: [u32; 7] = [3, 5, 10, 20, 30, 40, 50];
const MAX_MIN_VOTES: u64 = 1_000;
const MAX_CATEGORIES: usize = 500;
const MAX_NAME: usize = 60;
const MAX_TOPS: u64 = 50;
/// The ranks after the size shown with the live chart.
const RUNNERS_UP: usize = 2;
/// The tracks without enough votes shown with the live chart.
const NOT_ENOUGH: usize = 5;
/// A programme started keeps playing this long after its slot at most.
const OVERRUN_SECONDS: u32 = 3_600;
/// How far ahead a top's shows are looked for.
const LOOKAHEAD_DAYS: i64 = 62;
/// Past charts listed per page by default, and at most.
const CHARTS_PAGE: u32 = 20;
const MAX_CHARTS_PAGE: u32 = 100;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_tops, create_top))
        .routes(routes!(get_top, put_top, delete_top))
        .routes(routes!(get_top_chart))
        .routes(routes!(list_top_charts))
        .routes(routes!(remove_top_track, restore_top_track))
        .routes(routes!(choose_top_new_release))
        .routes(routes!(get_top_show))
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

fn seconds(at: Timestamp) -> f64 {
    at.as_millisecond() as f64 / 1_000.0
}

// ---------------------------------------------------------------------------
// Settings

/// What a top's chart counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum TopPeriod {
    /// The 7 days before each show, frozen when its programme is built.
    #[serde(rename = "rolling_7_days")]
    Rolling7Days,
    /// From a reset, a day of the week and a time, to the next.
    #[default]
    #[serde(rename = "week")]
    Week,
    /// From a reset, a day of the month and a time, to the next.
    #[serde(rename = "month")]
    Month,
}

/// How often a top's show comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotEvery {
    /// Settings saved before the days were chosen one by one: every day
    /// of the week. Read as `week` with the seven days.
    Day,
    Week,
    Month,
}

/// Which of the month's days: the 1st Sunday, the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotNth {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

impl SlotNth {
    fn number(self) -> i8 {
        match self {
            Self::First => 1,
            Self::Second => 2,
            Self::Third => 3,
            Self::Fourth => 4,
            Self::Last => -1,
        }
    }
}

/// A top's show: a recurrence and hours, in the station's time zone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TopSlot {
    pub every: SlotEvery,
    /// Its days, 1 (Monday) to 7, at least one: every week, or the `nth`
    /// of each in the month.
    #[serde(default)]
    pub weekdays: Vec<u8>,
    /// Settings saved before: a single day, read as `weekdays`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekday: Option<u8>,
    /// For `month`.
    #[serde(default)]
    pub nth: Option<SlotNth>,
    /// `HH:MM`.
    pub start: String,
    /// `HH:MM`, after `start`, up to `24:00`.
    pub end: String,
}

/// A top's settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TopSettings {
    /// 1 to 60 characters: its title on the website unless the website
    /// gives another.
    pub name: String,
    /// Off: it plays nothing, its page on the website stays closed and it
    /// sends nothing; its charts are still frozen.
    pub enabled: bool,
    /// 3, 5, 10, 20, 30, 40 or 50.
    pub size: u32,
    pub period: TopPeriod,
    /// The reset of a week, 1 (Monday) to 7.
    pub reset_weekday: u8,
    /// The reset of a month, 1 to 28.
    pub reset_day: u8,
    /// `HH:MM`, 00:00 to 23:59.
    pub reset_time: String,
    /// The score's share of plays, 0 to 100 by steps of 10; the rest is
    /// the share of likes.
    pub plays_percent: u8,
    /// Votes a track needs over the period to be ranked, 0 to 1000.
    pub min_votes: u64,
    /// The categories whose tracks can enter the chart; none: the
    /// station's music.
    pub category_ids: Vec<u64>,
    /// The jingles between the tracks of the show.
    pub jingle_category_id: Option<u64>,
    /// Where the new release at the end of the show comes from.
    pub new_release_category_id: Option<u64>,
    /// The ranks still to come hidden on the website during the show.
    pub hide_unplayed: bool,
    /// Its show; none: nothing plays.
    pub slot: Option<TopSlot>,
    pub website: WebsiteSettings,
    pub notifications: NotificationSettings,
}

impl Default for TopSettings {
    fn default() -> Self {
        Self {
            name: "Top 30".to_owned(),
            enabled: true,
            size: 30,
            period: TopPeriod::Week,
            reset_weekday: 7,
            reset_day: 1,
            reset_time: "17:00".to_owned(),
            plays_percent: 0,
            min_votes: crate::likes::MIN_VOTES,
            category_ids: Vec::new(),
            jingle_category_id: None,
            new_release_category_id: None,
            hide_unplayed: true,
            slot: None,
            website: WebsiteSettings::default(),
            notifications: NotificationSettings::default(),
        }
    }
}

/// Minutes into the day of `HH:MM`; `24:00` only when `end`.
pub fn minutes(text: &str, end: bool) -> Option<u32> {
    let (hours, mins) = text.split_once(':')?;
    if hours.len() != 2 || mins.len() != 2 {
        return None;
    }
    let hours: u32 = hours.parse().ok()?;
    let mins: u32 = mins.parse().ok()?;
    if mins > 59 {
        return None;
    }
    let total = hours * 60 + mins;
    (total < 24 * 60 || (end && total == 24 * 60)).then_some(total)
}

impl TopSlot {
    /// Its start and end, in minutes into the day.
    fn range(&self) -> (u32, u32) {
        (
            minutes(&self.start, false).unwrap_or(0),
            minutes(&self.end, true).unwrap_or(0),
        )
    }

    /// Settings saved before in the new form: every day as the seven
    /// days of every week, a single `weekday` as `weekdays`; the days in
    /// order, each once.
    pub fn normalize(&mut self) {
        if self.every == SlotEvery::Day {
            self.every = SlotEvery::Week;
            self.weekdays = (1..=7).collect();
        }
        if let Some(day) = self.weekday.take()
            && self.weekdays.is_empty()
        {
            self.weekdays.push(day);
        }
        self.weekdays.sort_unstable();
        self.weekdays.dedup();
    }

    /// Its days of the week.
    fn days(&self) -> Vec<Weekday> {
        if self.every == SlotEvery::Day {
            return (1..=7)
                .filter_map(|day| Weekday::from_monday_one_offset(day).ok())
                .collect();
        }
        let mut days: Vec<u8> = self
            .weekdays
            .iter()
            .chain(self.weekday.iter().filter(|_| self.weekdays.is_empty()))
            .copied()
            .collect();
        days.sort_unstable();
        days.dedup();
        days.into_iter()
            .filter_map(|day| Weekday::from_monday_one_offset(day as i8).ok())
            .collect()
    }

    fn check(&self) -> Result<(), String> {
        let (Some(start), Some(end)) = (minutes(&self.start, false), minutes(&self.end, true))
        else {
            return Err("the show's hours are HH:MM, up to 24:00".to_owned());
        };
        if end <= start {
            return Err("the show ends after it starts, the same day".to_owned());
        }
        if self.every != SlotEvery::Day {
            let days: Vec<u8> = self.weekdays.iter().chain(&self.weekday).copied().collect();
            if days.is_empty() || !days.iter().all(|day| (1..=7).contains(day)) {
                return Err("the show's days are 1 (Monday) to 7, at least one".to_owned());
            }
        }
        if (self.every == SlotEvery::Month) != self.nth.is_some() {
            return Err("nth is for a show every month, and only for it".to_owned());
        }
        Ok(())
    }

    /// How rare it is: a day of the month before a day of every week.
    fn rarity(&self) -> u8 {
        match self.every {
            SlotEvery::Day | SlotEvery::Week => 1,
            SlotEvery::Month => 2,
        }
    }

    /// Whether it plays on `date`.
    pub fn occurs_on(&self, date: Date) -> bool {
        let days = self.days();
        match self.every {
            SlotEvery::Day | SlotEvery::Week => days.contains(&date.weekday()),
            SlotEvery::Month => {
                let nth = self.nth.map_or(1, SlotNth::number);
                days.iter()
                    .any(|day| date.nth_weekday_of_month(nth, *day).ok() == Some(date))
            }
        }
    }

    /// Whether both can play on the same day.
    fn can_coincide(&self, other: &Self) -> bool {
        let (ours, theirs) = (self.days(), other.days());
        if !ours.iter().any(|day| theirs.contains(day)) {
            return false;
        }
        match (self.every, other.every) {
            (SlotEvery::Month, SlotEvery::Month) => {
                let nths = (self.nth, other.nth);
                nths.0 == nths.1
                    || matches!(
                        nths,
                        (Some(SlotNth::Fourth), Some(SlotNth::Last))
                            | (Some(SlotNth::Last), Some(SlotNth::Fourth))
                    )
            }
            _ => true,
        }
    }

    /// Whether their hours overlap on a day both play.
    fn overlaps(&self, other: &Self) -> bool {
        let (a, b) = (self.range(), other.range());
        a.0 < b.1 && b.0 < a.1 && self.can_coincide(other)
    }

    /// The days of the week and the whole hours it can take.
    pub fn hours(&self) -> HourRange {
        let (start, end) = self.range();
        HourRange {
            days: self
                .days()
                .iter()
                .map(|day| day.to_monday_one_offset() as u8)
                .collect(),
            start_hour: (start / 60) as u8,
            end_hour: end.div_ceil(60).min(24) as u8,
        }
    }

    /// Its opening on `date`, in `tz`.
    fn opening_on(&self, date: Date, tz: &TimeZone) -> Option<(Timestamp, Timestamp)> {
        let (start, end) = self.range();
        let at = |minutes: u32| -> Option<Timestamp> {
            let day = if minutes >= 24 * 60 {
                date.tomorrow().ok()?.at(0, 0, 0, 0)
            } else {
                date.at((minutes / 60) as i8, (minutes % 60) as i8, 0, 0)
            };
            day.to_zoned(tz.clone()).ok().map(|zoned| zoned.timestamp())
        };
        Some((at(start)?, at(end)?))
    }
}

impl TopSettings {
    pub fn check(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(format!("the name is 1 to {MAX_NAME} characters"));
        }
        if !SIZES.contains(&self.size) {
            return Err("size is 3, 5, 10, 20, 30, 40 or 50".to_owned());
        }
        if !(1..=7).contains(&self.reset_weekday) {
            return Err("reset_weekday is 1 (Monday) to 7".to_owned());
        }
        if !(1..=28).contains(&self.reset_day) {
            return Err("reset_day is 1 to 28".to_owned());
        }
        if minutes(&self.reset_time, false).is_none() {
            return Err("reset_time is HH:MM, 00:00 to 23:59".to_owned());
        }
        if self.plays_percent > 100 || !self.plays_percent.is_multiple_of(10) {
            return Err("plays_percent is 0 to 100, by steps of 10".to_owned());
        }
        if self.min_votes > MAX_MIN_VOTES {
            return Err(format!("min_votes is 0 to {MAX_MIN_VOTES}"));
        }
        if self.category_ids.len() > MAX_CATEGORIES {
            return Err(format!("at most {MAX_CATEGORIES} categories"));
        }
        if let Some(slot) = &self.slot {
            slot.check()?;
        }
        self.show_settings().check(top_show())
    }

    /// Its website and notifications as a show's, to check and complete.
    fn show_settings(&self) -> ShowSettings {
        ShowSettings {
            enabled: self.enabled,
            hours: Vec::new(),
            website: self.website.clone(),
            notifications: self.notifications.clone(),
        }
    }

    /// Gives every event its default when it has none.
    fn complete(&mut self) {
        let mut show = self.show_settings();
        show.complete(top_show());
        self.notifications = show.notifications;
    }

    /// Its name in messages and on the website.
    pub fn title(&self) -> String {
        Some(self.website.title.trim())
            .filter(|title| !title.is_empty())
            .unwrap_or(self.name.trim())
            .to_owned()
    }
}

/// The tops' entry among the shows.
static TOPS: Show = core_shows::SHOWS[2];

fn top_show() -> &'static Show {
    Show::of(Module::Top30).unwrap_or(&TOPS)
}

/// A top of a station.
#[derive(Clone, Debug)]
pub struct Top {
    pub id: u64,
    pub station_id: u64,
    pub settings: TopSettings,
}

impl Top {
    /// Its name in the gateway's routes.
    pub fn key(&self) -> String {
        format!("top-{}", self.id)
    }

    fn planned(&self) -> Planned {
        Planned {
            id: self.id,
            enabled: self.settings.enabled,
            slot: self.settings.slot.clone(),
        }
    }
}

/// A stored top: its settings, or why they cannot be read.
type Stored = (u64, u64, u32, Result<TopSettings, String>);

/// Reads stored settings strictly; settings that do not pass their check
/// count as unreadable too.
fn parse(json: &str) -> Result<TopSettings, String> {
    let mut settings: TopSettings =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    if let Some(slot) = settings.slot.as_mut() {
        slot.normalize();
    }
    settings.check()?;
    settings.complete();
    Ok(settings)
}

async fn stored(pool: &MySqlPool, station_id: u64) -> ApiResult<Vec<Stored>> {
    let rows: Vec<(u64, u64, u32, String)> = sqlx::query_as(
        "SELECT id, station_id, CAST(position AS UNSIGNED), CAST(settings AS CHAR)
         FROM tops WHERE station_id = ? ORDER BY position, id",
    )
    .bind(station_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, station, position, json)| (id, station, position, parse(&json)))
        .collect())
}

/// The station's tops that can be read, in order. One that cannot is
/// left out with a warning: a top never stops the others, or the queue.
pub async fn tops_of(pool: &MySqlPool, station_id: u64) -> ApiResult<Vec<Top>> {
    Ok(stored(pool, station_id)
        .await?
        .into_iter()
        .filter_map(|(id, station_id, _, settings)| match settings {
            Ok(settings) => Some(Top {
                id,
                station_id,
                settings,
            }),
            Err(error) => {
                tracing::warn!(station = station_id, top = id, %error, "a top's settings cannot be read: it is left out");
                None
            }
        })
        .collect())
}

async fn time_zone(pool: &MySqlPool, station_id: u64) -> ApiResult<TimeZone> {
    let station: StationSettings =
        settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await?;
    Ok(station.time_zone())
}

// ---------------------------------------------------------------------------
// Shows: when each top plays

/// What the schedule needs of a top.
#[derive(Clone, Debug)]
pub struct Planned {
    pub id: u64,
    pub enabled: bool,
    pub slot: Option<TopSlot>,
}

impl Planned {
    /// Whether `self` takes a day's slot from `other`: rarer, or as rare
    /// and first (two of the same rarity cannot be saved overlapping).
    fn before(&self, other: &Self) -> bool {
        match (&self.slot, &other.slot) {
            (Some(mine), Some(theirs)) => {
                mine.rarity() > theirs.rarity()
                    || (mine.rarity() == theirs.rarity() && self.id < other.id)
            }
            _ => false,
        }
    }
}

/// The shows of top `id` that close after `from`, over `days` days, the
/// days it gives way to a rarer top left out.
pub fn openings_of(
    tops: &[Planned],
    id: u64,
    tz: &TimeZone,
    from: Timestamp,
    days: i64,
) -> Vec<(Timestamp, Timestamp)> {
    let Some(top) = tops.iter().find(|top| top.id == id && top.enabled) else {
        return Vec::new();
    };
    let Some(slot) = &top.slot else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let first = from.to_zoned(tz.clone()).date();
    for offset in -1..=days {
        let Ok(date) = first.checked_add(offset.days()) else {
            continue;
        };
        if !slot.occurs_on(date) {
            continue;
        }
        let taken = tops.iter().any(|other| {
            other.id != id
                && other.enabled
                && other.before(top)
                && other.slot.as_ref().is_some_and(|theirs| {
                    theirs.occurs_on(date) && {
                        let (a, b) = (slot.range(), theirs.range());
                        a.0 < b.1 && b.0 < a.1
                    }
                })
        });
        if taken {
            continue;
        }
        if let Some((opens, closes)) = slot.opening_on(date, tz)
            && closes > from
        {
            found.push((opens, closes));
        }
    }
    found
}

/// The show open at `at`, of any top: its top, when it opens and closes.
pub fn opening_at(
    tops: &[Planned],
    tz: &TimeZone,
    at: Timestamp,
) -> Option<(u64, Timestamp, Timestamp)> {
    tops.iter().find_map(|top| {
        openings_of(tops, top.id, tz, at, 1)
            .into_iter()
            .find(|(opens, closes)| *opens <= at && at < *closes)
            .map(|(opens, closes)| (top.id, opens, closes))
    })
}

/// The tops of the same rarity whose show overlaps `slot` on a day both
/// play: they cannot be saved together.
fn same_slot<'a>(slot: &TopSlot, others: &'a [Top]) -> Option<&'a Top> {
    others.iter().find(|other| {
        other
            .settings
            .slot
            .as_ref()
            .is_some_and(|theirs| theirs.rarity() == slot.rarity() && slot.overlaps(theirs))
    })
}

/// A top that takes this one's slot on some days.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct GivesWay {
    pub top_id: u64,
    pub name: String,
    pub slot: TopSlot,
}

fn gives_way(top: &Top, all: &[Top]) -> Vec<GivesWay> {
    let mine = top.planned();
    let Some(slot) = &top.settings.slot else {
        return Vec::new();
    };
    all.iter()
        .filter(|other| other.id != top.id && other.settings.enabled)
        .filter(|other| other.planned().before(&mine))
        .filter_map(|other| {
            let theirs = other.settings.slot.as_ref()?;
            slot.overlaps(theirs).then(|| GivesWay {
                top_id: other.id,
                name: other.settings.name.clone(),
                slot: theirs.clone(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Periods

/// A chart's period, named by the day it is frozen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChartPeriod {
    pub key: Date,
    pub from: Timestamp,
    pub to: Timestamp,
}

/// A week's or a month's reset on `date`.
fn reset_on(settings: &TopSettings, tz: &TimeZone, date: Date) -> ApiResult<Timestamp> {
    let minutes = minutes(&settings.reset_time, false).unwrap_or(0);
    Ok(date
        .at((minutes / 60) as i8, (minutes % 60) as i8, 0, 0)
        .to_zoned(tz.clone())
        .map_err(internal)?
        .timestamp())
}

/// The week or the month `at` is in: from the last reset to the next. The
/// last 7 days up to `at` for a top of the last 7 days.
pub fn period_at(settings: &TopSettings, tz: &TimeZone, at: Timestamp) -> ApiResult<ChartPeriod> {
    let today = at.to_zoned(tz.clone()).date();
    let (start, next) = match settings.period {
        TopPeriod::Rolling7Days => return Ok(rolling(at, tz)),
        TopPeriod::Week => {
            let wanted = settings.reset_weekday.clamp(1, 7) as i64;
            let today_number = i64::from(today.weekday().to_monday_one_offset());
            let back = (today_number - wanted).rem_euclid(7);
            let mut day = today.checked_sub(back.days()).map_err(internal)?;
            if reset_on(settings, tz, day)? > at {
                day = day.checked_sub(7.days()).map_err(internal)?;
            }
            (day, day.checked_add(7.days()).map_err(internal)?)
        }
        TopPeriod::Month => {
            let day_of_month = settings.reset_day.clamp(1, 28) as i8;
            let mut day = today.with().day(day_of_month).build().map_err(internal)?;
            if reset_on(settings, tz, day)? > at {
                day = day.checked_sub(1.month()).map_err(internal)?;
            }
            (day, day.checked_add(1.month()).map_err(internal)?)
        }
    };
    Ok(ChartPeriod {
        key: next,
        from: reset_on(settings, tz, start)?,
        to: reset_on(settings, tz, next)?,
    })
}

/// The 7 days before a show opening at `opens`, frozen for it.
pub fn rolling(opens: Timestamp, tz: &TimeZone) -> ChartPeriod {
    ChartPeriod {
        key: opens.to_zoned(tz.clone()).date(),
        from: opens
            .checked_sub(SignedDuration::from_hours(7 * 24))
            .unwrap_or(opens),
        to: opens,
    }
}

/// The latest week or month frozen at `at`.
pub fn latest_frozen(
    settings: &TopSettings,
    tz: &TimeZone,
    at: Timestamp,
) -> ApiResult<ChartPeriod> {
    let current = period_at(settings, tz, at)?;
    let before = current
        .from
        .checked_sub(SignedDuration::from_secs(1))
        .map_err(internal)?;
    period_at(settings, tz, before)
}

/// The week or month frozen on `key`; `None` when no reset ends one that
/// day.
fn period_of_key(
    settings: &TopSettings,
    tz: &TimeZone,
    key: Date,
) -> ApiResult<Option<ChartPeriod>> {
    if settings.period == TopPeriod::Rolling7Days {
        return Ok(None);
    }
    let reset = reset_on(settings, tz, key)?;
    let before = reset
        .checked_sub(SignedDuration::from_secs(1))
        .map_err(internal)?;
    let period = period_at(settings, tz, before)?;
    Ok((period.key == key).then_some(period))
}

/// A chart as a path names it, by the day it is frozen.
fn parse_key(text: &str) -> ApiResult<Date> {
    text.parse()
        .map_err(|_| ApiError::BadRequest(format!("{text:?} is not a date: 2026-11-08")))
}

// ---------------------------------------------------------------------------
// The score

/// A track's plays and votes over a period.
#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct Candidate {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub plays: u64,
    pub likes: u64,
    pub dislikes: u64,
}

/// A candidate with its score.
#[derive(Clone, Debug, PartialEq)]
pub struct Scored {
    pub candidate: Candidate,
    /// Its plays against the most played's, 0 to 1.
    pub plays_share: f64,
    /// The share of likes, 0 when it has no vote.
    pub share: f64,
    pub score: f64,
}

impl Scored {
    fn votes(&self) -> u64 {
        self.candidate.likes + self.candidate.dislikes
    }
}

/// The candidates by score: those with enough votes, the others, and the
/// most played's plays. A score of 0 is not ranked. Ties go to the most
/// votes, then the most plays, then the oldest track.
pub fn rank(
    candidates: Vec<Candidate>,
    plays_percent: u8,
    min_votes: u64,
) -> (Vec<Scored>, Vec<Scored>, u64) {
    let max_plays = candidates.iter().map(|c| c.plays).max().unwrap_or(0);
    let p = f64::from(plays_percent.min(100)) / 100.0;
    let mut scored: Vec<Scored> = candidates
        .into_iter()
        .map(|candidate| {
            let plays_share = if max_plays == 0 {
                0.0
            } else {
                candidate.plays as f64 / max_plays as f64
            };
            let share = crate::likes::share(candidate.likes, candidate.dislikes);
            // Rounded, so that equal scores compare equal.
            let score = ((p * plays_share + (1.0 - p) * share) * 1e9).round() / 1e9;
            Scored {
                candidate,
                plays_share,
                share,
                score,
            }
        })
        .filter(|scored| scored.score > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(b.votes().cmp(&a.votes()))
            .then(b.candidate.plays.cmp(&a.candidate.plays))
            .then(a.candidate.track_id.cmp(&b.candidate.track_id))
    });
    let (ranked, below) = scored
        .into_iter()
        .partition(|scored| scored.votes() >= min_votes);
    (ranked, below, max_plays)
}

/// The categories whose tracks can enter the chart; `None`: every track
/// (a station without categories rotates the whole library).
async fn eligible(
    pool: &MySqlPool,
    station_id: u64,
    settings: &TopSettings,
) -> ApiResult<Option<Vec<u64>>> {
    if !settings.category_ids.is_empty() {
        return Ok(Some(settings.category_ids.clone()));
    }
    let rows: Vec<(u64, bool)> = sqlx::query_as(
        "SELECT c.id, COALESCE(ct.sends_metadata, TRUE) AS music
         FROM categories c
         JOIN category_stations cs ON cs.category_id = c.id AND cs.station_id = ?
         LEFT JOIN category_types ct ON ct.id = c.type_id",
    )
    .bind(station_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        rows.into_iter()
            .filter(|(_, music)| *music)
            .map(|(id, _)| id)
            .collect(),
    ))
}

/// ` AND EXISTS (the track in the categories)`, nothing for `None`.
fn in_categories(query: &mut QueryBuilder<sqlx::MySql>, column: &str, categories: Option<&[u64]>) {
    let Some(categories) = categories else {
        return;
    };
    query.push(format!(
        " AND EXISTS (SELECT 1 FROM track_categories tc
                      WHERE tc.track_id = {column} AND tc.category_id IN ("
    ));
    let mut ids = query.separated(", ");
    for id in categories {
        ids.push_bind(*id);
    }
    query.push("))");
}

/// ` AND NOT EXISTS (taken out of the chart)`.
fn not_removed(query: &mut QueryBuilder<sqlx::MySql>, column: &str, top_id: u64, key: Date) {
    query
        .push(format!(
            " AND NOT EXISTS (SELECT 1 FROM top_removals r
                              WHERE r.track_id = {column} AND r.top_id = "
        ))
        .push_bind(top_id)
        .push(" AND r.chart_key = ")
        .push_bind(key.to_string())
        .push(")");
}

/// The eligible tracks played or voted on between `from` and `to`, not
/// taken out of the chart `key`: their plays (outside the tops' shows) and
/// their votes.
async fn candidates(
    pool: &MySqlPool,
    top: &Top,
    key: Date,
    from: Timestamp,
    to: Timestamp,
) -> ApiResult<Vec<Candidate>> {
    let categories = eligible(pool, top.station_id, &top.settings).await?;
    if categories.as_ref().is_some_and(Vec::is_empty) {
        return Ok(Vec::new());
    }
    let categories = categories.as_deref();
    let mut query = QueryBuilder::new(
        "SELECT h.track_id, CAST(COUNT(*) AS UNSIGNED)
         FROM play_history h JOIN tracks t ON t.id = h.track_id
         WHERE h.deck <> 'fx' AND t.deleted_at IS NULL AND h.station_id = ",
    );
    query
        .push_bind(top.station_id)
        .push(" AND h.started_at >= FROM_UNIXTIME(")
        .push_bind(seconds(from))
        .push(") AND h.started_at < FROM_UNIXTIME(")
        .push_bind(seconds(to))
        .push(
            ") AND NOT EXISTS (SELECT 1 FROM top_shows s
                              WHERE s.station_id = h.station_id
                                AND h.started_at >= s.opens_at AND h.started_at < s.closes_at)",
        );
    in_categories(&mut query, "h.track_id", categories);
    not_removed(&mut query, "h.track_id", top.id, key);
    query.push(" GROUP BY h.track_id");
    let plays: HashMap<u64, u64> = query
        .build_query_as::<(u64, u64)>()
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    let mut query = QueryBuilder::new(
        "SELECT v.track_id, CAST(SUM(v.likes) AS UNSIGNED), CAST(SUM(v.dislikes) AS UNSIGNED)
         FROM play_votes v JOIN tracks t ON t.id = v.track_id
         WHERE t.deleted_at IS NULL AND v.station_id = ",
    );
    query
        .push_bind(top.station_id)
        .push(" AND v.started_at >= FROM_UNIXTIME(")
        .push_bind(seconds(from))
        .push(") AND v.started_at < FROM_UNIXTIME(")
        .push_bind(seconds(to))
        .push(")");
    in_categories(&mut query, "v.track_id", categories);
    not_removed(&mut query, "v.track_id", top.id, key);
    query.push(" GROUP BY v.track_id");
    let votes: HashMap<u64, (u64, u64)> = query
        .build_query_as::<(u64, u64, u64)>()
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(id, likes, dislikes)| (id, (likes, dislikes)))
        .collect();
    let ids: Vec<u64> = plays
        .keys()
        .chain(votes.keys())
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let labels = labels(pool, &ids).await?;
    Ok(ids
        .into_iter()
        .map(|track_id| {
            let (likes, dislikes) = votes.get(&track_id).copied().unwrap_or((0, 0));
            let (artist, title) = labels.get(&track_id).cloned().unwrap_or((None, None));
            Candidate {
                track_id,
                artist,
                title,
                plays: plays.get(&track_id).copied().unwrap_or(0),
                likes,
                dislikes,
            }
        })
        .collect())
}

type Label = (u64, Option<String>, Option<String>);

/// The artist and title of tracks.
async fn labels(
    pool: &MySqlPool,
    ids: &[u64],
) -> ApiResult<HashMap<u64, (Option<String>, Option<String>)>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut query = QueryBuilder::new("SELECT id, artist, title FROM tracks WHERE id IN (");
    let mut separated = query.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
    query.push(")");
    let rows: Vec<Label> = query.build_query_as().fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, artist, title)| (id, (artist, title)))
        .collect())
}

// ---------------------------------------------------------------------------
// The chart

/// How a track moved against the top's previous chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Movement {
    Up,
    Down,
    Same,
    /// Never in one of its charts before.
    New,
    /// In an earlier chart, not the previous one.
    Reentry,
}

impl Movement {
    fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Same => "same",
            Self::New => "new",
            Self::Reentry => "reentry",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "up" => Self::Up,
            "down" => Self::Down,
            "same" => Self::Same,
            "reentry" => Self::Reentry,
            _ => Self::New,
        }
    }
}

/// How a track at `rank` moved: its rank in the previous chart, and how
/// many earlier charts it was in.
pub fn movement(rank: u32, previous: Option<u32>, earlier: u32) -> Movement {
    match previous {
        Some(previous) if previous > rank => Movement::Up,
        Some(previous) if previous < rank => Movement::Down,
        Some(_) => Movement::Same,
        None if earlier > 0 => Movement::Reentry,
        None => Movement::New,
    }
}

/// A rank of a chart.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ChartEntry {
    pub rank: u32,
    /// Missing once the track is deleted.
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// Its plays over the period, outside the tops' shows (for the Top 30's
    /// charts: its plays with votes).
    pub plays: u64,
    /// Its plays against the most played's, 0 to 1; `null` for the Top
    /// 30's charts.
    pub plays_share: Option<f64>,
    pub likes: u64,
    pub dislikes: u64,
    /// The share of likes, 0 to 1.
    pub share: f64,
    /// 0 to 1.
    pub score: f64,
    pub movement: Movement,
    /// Its rank in the top's previous chart.
    pub previous_rank: Option<u32>,
    /// The top's charts it was in, this one included.
    pub charts: u32,
}

/// A track below the minimum votes, with the rank its score would give it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NotRanked {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub plays: u64,
    pub plays_share: f64,
    pub likes: u64,
    pub dislikes: u64,
    pub share: f64,
    pub score: f64,
    pub would_rank: u32,
}

/// A track taken out of a chart.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct Removed {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// Who took it out; missing once the user is deleted.
    pub removed_by: Option<String>,
    pub removed_at_unix_ms: i64,
}

/// The new release at the end of the show.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NewRelease {
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// Proposed by Heron, not chosen by hand.
    pub proposed: bool,
}

/// A top's chart.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Chart {
    /// The day it is frozen: `2026-11-08`.
    pub key: String,
    pub from_unix_ms: i64,
    /// When it is frozen.
    pub to_unix_ms: i64,
    /// Frozen and kept; else the chart as it stands now.
    pub frozen: bool,
    /// A show was built from it: it does not change any more.
    pub locked: bool,
    /// The votes on the eligible tracks over the period.
    pub votes: u64,
    /// The most played eligible track's plays.
    pub max_plays: u64,
    pub plays_percent: u8,
    pub min_votes: u64,
    pub size: u32,
    pub entries: Vec<ChartEntry>,
    /// The next ranks after the size (the live chart only).
    pub runners_up: Vec<ChartEntry>,
    /// The tracks below the minimum votes, the best first (the live chart
    /// only).
    pub not_enough_votes: Vec<NotRanked>,
    pub removed: Vec<Removed>,
    /// `null` without a new release category.
    pub new_release: Option<NewRelease>,
}

/// The ranks with each track's movement against the top's previous chart
/// and its charts, from the charts kept.
async fn with_movements(
    pool: &MySqlPool,
    top_id: u64,
    key: Date,
    ranked: &[Scored],
) -> ApiResult<Vec<ChartEntry>> {
    let previous_key: Option<String> = sqlx::query_scalar(
        "SELECT CAST(chart_key AS CHAR) FROM top_charts
         WHERE top_id = ? AND frozen_at IS NOT NULL AND chart_key < ?
         ORDER BY chart_key DESC LIMIT 1",
    )
    .bind(top_id)
    .bind(key.to_string())
    .fetch_optional(pool)
    .await?;
    let previous: HashMap<u64, u32> = match previous_key {
        Some(previous) => sqlx::query_as::<_, (u64, u64)>(
            "SELECT track_id, CAST(`rank` AS UNSIGNED) FROM top_entries
             WHERE top_id = ? AND chart_key = ? AND track_id IS NOT NULL",
        )
        .bind(top_id)
        .bind(previous)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(track, rank)| (track, rank as u32))
        .collect(),
        None => HashMap::new(),
    };
    let earlier: HashMap<u64, u64> = sqlx::query_as::<_, (u64, u64)>(
        "SELECT track_id, CAST(COUNT(*) AS UNSIGNED) FROM top_entries
         WHERE top_id = ? AND chart_key < ? AND track_id IS NOT NULL
         GROUP BY track_id",
    )
    .bind(top_id)
    .bind(key.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    Ok(ranked
        .iter()
        .enumerate()
        .map(|(index, scored)| {
            let rank = index as u32 + 1;
            let track = &scored.candidate;
            let previous_rank = previous.get(&track.track_id).copied();
            let before = earlier.get(&track.track_id).copied().unwrap_or(0) as u32;
            ChartEntry {
                rank,
                track_id: Some(track.track_id),
                artist: track.artist.clone(),
                title: track.title.clone(),
                plays: track.plays,
                plays_share: Some(scored.plays_share),
                likes: track.likes,
                dislikes: track.dislikes,
                share: scored.share,
                score: scored.score,
                movement: movement(rank, previous_rank, before),
                previous_rank,
                charts: before + 1,
            }
        })
        .collect())
}

/// Ranks a period and keeps its chart, in place of the one kept (after a
/// track was taken out). Returns its entries.
pub async fn freeze(
    pool: &MySqlPool,
    top: &Top,
    period: &ChartPeriod,
) -> ApiResult<Vec<ChartEntry>> {
    let settings = &top.settings;
    let found = candidates(pool, top, period.key, period.from, period.to).await?;
    let votes: u64 = found.iter().map(|c| c.likes + c.dislikes).sum();
    let (mut ranked, _, max_plays) = rank(found, settings.plays_percent, settings.min_votes);
    ranked.truncate(settings.size as usize);
    let entries = with_movements(pool, top.id, period.key, &ranked).await?;
    let key = period.key.to_string();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO top_charts (top_id, chart_key, from_at, to_at, frozen_at, votes, max_plays,
                                 plays_percent, min_votes, size)
         VALUES (?, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), NOW(6), ?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE from_at = VALUES(from_at), to_at = VALUES(to_at),
             frozen_at = VALUES(frozen_at), votes = VALUES(votes), max_plays = VALUES(max_plays),
             plays_percent = VALUES(plays_percent), min_votes = VALUES(min_votes),
             size = VALUES(size)",
    )
    .bind(top.id)
    .bind(&key)
    .bind(seconds(period.from))
    .bind(seconds(period.to))
    .bind(votes)
    .bind(max_plays)
    .bind(settings.plays_percent)
    .bind(settings.min_votes)
    .bind(settings.size)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM top_entries WHERE top_id = ? AND chart_key = ?")
        .bind(top.id)
        .bind(&key)
        .execute(&mut *tx)
        .await?;
    if !entries.is_empty() {
        let mut query = QueryBuilder::new(
            "INSERT INTO top_entries (top_id, chart_key, `rank`, track_id, artist, title, likes,
                                      dislikes, plays, score, movement, previous_rank, charts) ",
        );
        query.push_values(&entries, |mut row, entry| {
            row.push_bind(top.id)
                .push_bind(&key)
                .push_bind(entry.rank)
                .push_bind(entry.track_id)
                .push_bind(&entry.artist)
                .push_bind(&entry.title)
                .push_bind(entry.likes)
                .push_bind(entry.dislikes)
                .push_bind(entry.plays)
                .push_bind(entry.score)
                .push_bind(entry.movement.as_str())
                .push_bind(entry.previous_rank)
                .push_bind(entry.charts);
        });
        query.build().execute(&mut *tx).await?;
    }
    tx.commit().await?;
    tracing::info!(
        station = top.station_id,
        top = top.id,
        chart = %key,
        tracks = entries.len(),
        "top frozen"
    );
    Ok(entries)
}

/// A frozen chart's row.
#[derive(sqlx::FromRow)]
struct FrozenRow {
    from_ms: i64,
    to_ms: i64,
    votes: u64,
    max_plays: Option<u64>,
    plays_percent: u8,
    min_votes: u64,
    size: u32,
}

async fn frozen_row(pool: &MySqlPool, top_id: u64, key: Date) -> ApiResult<Option<FrozenRow>> {
    Ok(sqlx::query_as(
        "SELECT CAST(UNIX_TIMESTAMP(from_at) * 1000 AS SIGNED) AS from_ms,
                CAST(UNIX_TIMESTAMP(to_at) * 1000 AS SIGNED) AS to_ms,
                votes, max_plays, plays_percent, min_votes, CAST(size AS UNSIGNED) AS size
         FROM top_charts WHERE top_id = ? AND chart_key = ? AND frozen_at IS NOT NULL",
    )
    .bind(top_id)
    .bind(key.to_string())
    .fetch_optional(pool)
    .await?)
}

/// Whether a show was built from the chart.
async fn locked(pool: &MySqlPool, top_id: u64, key: Date) -> ApiResult<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM top_shows WHERE top_id = ? AND chart_key = ? LIMIT 1")
            .bind(top_id)
            .bind(key.to_string())
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

#[derive(sqlx::FromRow)]
struct StoredEntry {
    rank: u64,
    track_id: Option<u64>,
    artist: Option<String>,
    title: Option<String>,
    likes: u64,
    dislikes: u64,
    plays: u64,
    score: f64,
    movement: String,
    previous_rank: Option<u64>,
    charts: u64,
}

/// A frozen chart's entries.
async fn stored_entries(
    pool: &MySqlPool,
    top_id: u64,
    key: Date,
    max_plays: Option<u64>,
) -> ApiResult<Vec<ChartEntry>> {
    let rows: Vec<StoredEntry> = sqlx::query_as(
        "SELECT CAST(`rank` AS UNSIGNED) AS `rank`, track_id, artist, title, likes, dislikes,
                plays, score, CAST(movement AS CHAR) AS movement,
                CAST(previous_rank AS UNSIGNED) AS previous_rank,
                CAST(charts AS UNSIGNED) AS charts
         FROM top_entries WHERE top_id = ? AND chart_key = ? ORDER BY `rank`",
    )
    .bind(top_id)
    .bind(key.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ChartEntry {
            rank: row.rank as u32,
            track_id: row.track_id,
            artist: row.artist,
            title: row.title,
            likes: row.likes,
            dislikes: row.dislikes,
            share: crate::likes::share(row.likes, row.dislikes),
            plays: row.plays,
            plays_share: max_plays
                .filter(|max| *max > 0)
                .map(|max| row.plays as f64 / max as f64),
            score: row.score,
            movement: Movement::parse(&row.movement),
            previous_rank: row.previous_rank.map(|rank| rank as u32),
            charts: row.charts as u32,
        })
        .collect())
}

/// ` AND {column} NOT IN (ids)`, nothing without IDs.
fn not_in(query: &mut QueryBuilder<sqlx::MySql>, column: &str, ids: &[u64]) {
    if ids.is_empty() {
        return;
    }
    query.push(format!(" AND {column} NOT IN ("));
    let mut separated = query.separated(", ");
    for id in ids {
        separated.push_bind(*id);
    }
    query.push(")");
}

/// The new release of a chart: chosen by hand, else the most liked track
/// of the new release category over the period that is not in `chart`,
/// else the track of the category added last.
async fn new_release(
    pool: &MySqlPool,
    top: &Top,
    period: &ChartPeriod,
    chart: &[u64],
) -> ApiResult<Option<NewRelease>> {
    let Some(category) = top.settings.new_release_category_id else {
        return Ok(None);
    };
    let chosen: Option<Option<u64>> = sqlx::query_scalar(
        "SELECT new_release_track_id FROM top_charts WHERE top_id = ? AND chart_key = ?",
    )
    .bind(top.id)
    .bind(period.key.to_string())
    .fetch_optional(pool)
    .await?;
    if let Some(id) = chosen.flatten() {
        let found: Option<Label> = sqlx::query_as(
            "SELECT id, artist, title FROM tracks WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        if let Some((track_id, artist, title)) = found {
            return Ok(Some(NewRelease {
                track_id,
                artist,
                title,
                proposed: false,
            }));
        }
    }
    let mut query = QueryBuilder::new(
        "SELECT v.track_id, t.artist, t.title
         FROM play_votes v JOIN tracks t ON t.id = v.track_id
         JOIN track_categories tc ON tc.track_id = v.track_id AND tc.category_id = ",
    );
    query
        .push_bind(category)
        .push(" WHERE v.station_id = ")
        .push_bind(top.station_id)
        .push(" AND v.started_at >= FROM_UNIXTIME(")
        .push_bind(seconds(period.from))
        .push(") AND v.started_at < FROM_UNIXTIME(")
        .push_bind(seconds(period.to))
        .push(") AND t.deleted_at IS NULL");
    not_in(&mut query, "v.track_id", chart);
    query.push(
        " GROUP BY v.track_id, t.artist, t.title
         HAVING SUM(v.likes) + SUM(v.dislikes) > 0
         ORDER BY SUM(v.likes) / (SUM(v.likes) + SUM(v.dislikes)) DESC,
                  SUM(v.likes) + SUM(v.dislikes) DESC, v.track_id
         LIMIT 1",
    );
    let mut found: Option<Label> = query.build_query_as().fetch_optional(pool).await?;
    if found.is_none() {
        let mut query = QueryBuilder::new(
            "SELECT t.id, t.artist, t.title FROM tracks t
             JOIN track_categories tc ON tc.track_id = t.id AND tc.category_id = ",
        );
        query
            .push_bind(category)
            .push(" WHERE t.deleted_at IS NULL AND t.enabled");
        not_in(&mut query, "t.id", chart);
        query.push(" ORDER BY t.created_at DESC, t.id DESC LIMIT 1");
        found = query.build_query_as().fetch_optional(pool).await?;
    }
    Ok(found.map(|(track_id, artist, title)| NewRelease {
        track_id,
        artist,
        title,
        proposed: true,
    }))
}

async fn removed(pool: &MySqlPool, top_id: u64, key: Date) -> ApiResult<Vec<Removed>> {
    Ok(sqlx::query_as(
        "SELECT r.track_id, t.artist, t.title, u.username AS removed_by,
                CAST(UNIX_TIMESTAMP(r.removed_at) * 1000 AS SIGNED) AS removed_at_unix_ms
         FROM top_removals r JOIN tracks t ON t.id = r.track_id
         LEFT JOIN users u ON u.id = r.removed_by
         WHERE r.top_id = ? AND r.chart_key = ? ORDER BY r.removed_at, r.track_id",
    )
    .bind(top_id)
    .bind(key.to_string())
    .fetch_all(pool)
    .await?)
}

/// The live chart's period: for a top of the last 7 days, the 7 days up
/// to now, named by its next show (today without one).
fn live_period(
    top: &Top,
    tops: &[Planned],
    tz: &TimeZone,
    now: Timestamp,
) -> ApiResult<ChartPeriod> {
    if top.settings.period != TopPeriod::Rolling7Days {
        return period_at(&top.settings, tz, now);
    }
    let next = openings_of(tops, top.id, tz, now, LOOKAHEAD_DAYS)
        .into_iter()
        .find(|(opens, _)| *opens > now);
    let mut period = rolling(now, tz);
    if let Some((opens, _)) = next {
        period.key = opens.to_zoned(tz.clone()).date();
    }
    Ok(period)
}

/// A chart as the pages show it: frozen, or live.
async fn chart(
    pool: &MySqlPool,
    top: &Top,
    period: &ChartPeriod,
    now: Timestamp,
) -> ApiResult<Chart> {
    let settings = &top.settings;
    let mut row = frozen_row(pool, top.id, period.key).await?;
    // A week or a month that ended is frozen as it is read.
    if row.is_none() && period.to <= now && settings.period != TopPeriod::Rolling7Days {
        freeze(pool, top, period).await?;
        row = frozen_row(pool, top.id, period.key).await?;
    }
    let mut chart = match row {
        Some(row) => Chart {
            key: period.key.to_string(),
            from_unix_ms: row.from_ms,
            to_unix_ms: row.to_ms,
            frozen: true,
            locked: false,
            votes: row.votes,
            max_plays: row.max_plays.unwrap_or(0),
            plays_percent: row.plays_percent,
            min_votes: row.min_votes,
            size: row.size,
            entries: stored_entries(pool, top.id, period.key, row.max_plays).await?,
            runners_up: Vec::new(),
            not_enough_votes: Vec::new(),
            removed: Vec::new(),
            new_release: None,
        },
        None => {
            // Plays still to come do not count yet: up to a minute ahead.
            let to = now
                .checked_add(SignedDuration::from_mins(1))
                .unwrap_or(now)
                .min(period.to);
            let found = candidates(pool, top, period.key, period.from, to).await?;
            let votes: u64 = found.iter().map(|c| c.likes + c.dislikes).sum();
            let (ranked, below, max_plays) =
                rank(found, settings.plays_percent, settings.min_votes);
            let size = settings.size as usize;
            let shown = &ranked[..ranked.len().min(size + RUNNERS_UP)];
            let mut entries = with_movements(pool, top.id, period.key, shown).await?;
            let runners_up = entries.split_off(entries.len().min(size));
            let not_enough_votes = below
                .iter()
                .take(NOT_ENOUGH)
                .map(|scored| NotRanked {
                    track_id: scored.candidate.track_id,
                    artist: scored.candidate.artist.clone(),
                    title: scored.candidate.title.clone(),
                    plays: scored.candidate.plays,
                    plays_share: scored.plays_share,
                    likes: scored.candidate.likes,
                    dislikes: scored.candidate.dislikes,
                    share: scored.share,
                    score: scored.score,
                    would_rank: ranked.iter().filter(|r| r.score > scored.score).count() as u32 + 1,
                })
                .collect();
            Chart {
                key: period.key.to_string(),
                from_unix_ms: period.from.as_millisecond(),
                to_unix_ms: period.to.as_millisecond(),
                frozen: false,
                locked: false,
                votes,
                max_plays,
                plays_percent: settings.plays_percent,
                min_votes: settings.min_votes,
                size: settings.size,
                entries,
                runners_up,
                not_enough_votes,
                removed: Vec::new(),
                new_release: None,
            }
        }
    };
    let ids: Vec<u64> = chart
        .entries
        .iter()
        .filter_map(|entry| entry.track_id)
        .collect();
    chart.locked = locked(pool, top.id, period.key).await?;
    chart.new_release = new_release(pool, top, period, &ids).await?;
    chart.removed = removed(pool, top.id, period.key).await?;
    Ok(chart)
}

// ---------------------------------------------------------------------------
// Routes: the tops

/// The next show of a top.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
pub struct NextShow {
    pub opens_at_unix_ms: i64,
    pub closes_at_unix_ms: i64,
}

/// A top, as the pages show it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TopView {
    pub id: u64,
    pub position: u32,
    /// Its page and widget on the website: `top-{id}`.
    pub key: String,
    pub settings: TopSettings,
    /// Why its stored settings cannot be read (they are shown as the
    /// defaults); `null` when they can.
    pub error: Option<String>,
    /// Whether its show is on now, when it closes, and when the next opens.
    pub status: ShowStatus,
    pub next_show: Option<NextShow>,
    /// The tops that take its slot on some days.
    pub gives_way: Vec<GivesWay>,
    /// The station's time zone (IANA), which its times follow.
    pub timezone: String,
    /// The station's webhooks, by name, to choose from.
    pub webhooks: Vec<WebhookChoice>,
}

async fn webhook_choices(pool: &MySqlPool, station_id: u64) -> ApiResult<Vec<WebhookChoice>> {
    Ok(
        sqlx::query_as("SELECT id, name FROM webhooks WHERE station_id = ? ORDER BY name, id")
            .bind(station_id)
            .fetch_all(pool)
            .await?,
    )
}

/// Every top of the station as the pages show it.
async fn views(pool: &MySqlPool, station_id: u64) -> ApiResult<Vec<TopView>> {
    let all = stored(pool, station_id).await?;
    let readable: Vec<Top> = all
        .iter()
        .filter_map(|(id, station_id, _, settings)| {
            settings.as_ref().ok().map(|settings| Top {
                id: *id,
                station_id: *station_id,
                settings: settings.clone(),
            })
        })
        .collect();
    let planned: Vec<Planned> = readable.iter().map(Top::planned).collect();
    let station: StationSettings =
        settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await?;
    let tz = station.time_zone();
    let webhooks = webhook_choices(pool, station_id).await?;
    let now = Timestamp::now();
    Ok(all
        .into_iter()
        .map(|(id, _, position, settings)| {
            let (settings, error) = match settings {
                Ok(settings) => (settings, None),
                Err(error) => (TopSettings::default(), Some(error)),
            };
            let top = Top {
                id,
                station_id,
                settings,
            };
            let openings = if error.is_none() {
                openings_of(&planned, id, &tz, now, LOOKAHEAD_DAYS)
            } else {
                Vec::new()
            };
            let status = core_shows::status_of_openings(&as_openings(&openings), now);
            TopView {
                id,
                position,
                key: top.key(),
                error,
                status,
                next_show: openings.iter().find(|(_, closes)| *closes > now).map(
                    |(opens, closes)| NextShow {
                        opens_at_unix_ms: opens.as_millisecond(),
                        closes_at_unix_ms: closes.as_millisecond(),
                    },
                ),
                gives_way: gives_way(&top, &readable),
                settings: top.settings,
                timezone: station.timezone.clone(),
                webhooks: webhooks.clone(),
            }
        })
        .collect())
}

fn as_openings(openings: &[(Timestamp, Timestamp)]) -> Vec<Opening> {
    openings
        .iter()
        .map(|(opens, closes)| Opening {
            opens_at_unix_ms: opens.as_millisecond(),
            closes_at_unix_ms: closes.as_millisecond(),
        })
        .collect()
}

/// The station's top of a path, readable.
async fn top_of(pool: &MySqlPool, station_id: u64, top_id: u64) -> ApiResult<Top> {
    let found = stored(pool, station_id)
        .await?
        .into_iter()
        .find(|(id, _, _, _)| *id == top_id)
        .ok_or_else(|| ApiError::not_found(format!("top {top_id}")))?;
    let (id, station_id, _, settings) = found;
    let settings = settings.map_err(|error| {
        ApiError::Conflict(format!(
            "the top's settings cannot be read ({error}): save them again"
        ))
    })?;
    Ok(Top {
        id,
        station_id,
        settings,
    })
}

/// Checks a top's settings against the station: its categories and
/// webhooks are the station's (`422`), no top of the same rarity overlaps
/// its show (`422`), and its show touches no hour of the Match or the
/// Dedications (`409`).
async fn checked(
    pool: &MySqlPool,
    station_id: u64,
    top_id: Option<u64>,
    mut settings: TopSettings,
) -> ApiResult<TopSettings> {
    settings.check().map_err(ApiError::Invalid)?;
    if let Some(slot) = settings.slot.as_mut() {
        slot.normalize();
    }
    settings.complete();
    settings.name = settings.name.trim().to_owned();
    settings.category_ids.sort_unstable();
    settings.category_ids.dedup();
    let ours: Vec<u64> =
        sqlx::query_scalar("SELECT category_id FROM category_stations WHERE station_id = ?")
            .bind(station_id)
            .fetch_all(pool)
            .await?;
    let named = settings
        .category_ids
        .iter()
        .chain(settings.jingle_category_id.iter())
        .chain(settings.new_release_category_id.iter());
    for id in named {
        if !ours.contains(id) {
            return Err(ApiError::Invalid(format!(
                "category {id} is not one of the station's"
            )));
        }
    }
    let webhooks: Vec<u64> = sqlx::query_scalar("SELECT id FROM webhooks WHERE station_id = ?")
        .bind(station_id)
        .fetch_all(pool)
        .await?;
    if let Some(id) = settings
        .notifications
        .webhook_ids
        .iter()
        .find(|id| !webhooks.contains(id))
    {
        return Err(ApiError::Invalid(format!(
            "webhook {id} is not one of the station's"
        )));
    }
    settings.notifications.webhook_ids.sort_unstable();
    settings.notifications.webhook_ids.dedup();
    if let Some(slot) = &settings.slot {
        let others: Vec<Top> = tops_of(pool, station_id)
            .await?
            .into_iter()
            .filter(|other| Some(other.id) != top_id)
            .collect();
        if let Some(other) = same_slot(slot, &others) {
            return Err(ApiError::Invalid(format!(
                "“{}” plays on the same slot, as often: one of them must move",
                other.settings.name
            )));
        }
        crate::shows::check_free(pool, station_id, &slot.hours(), None).await?;
    }
    Ok(settings)
}

/// The station's tops, in order.
#[utoipa::path(get, path = "/tops", tag = "top30",
    responses((status = 200, body = Vec<TopView>), (status = "4XX", body = ErrorBody)))]
async fn list_tops(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Vec<TopView>>> {
    Ok(Json(views(&state.pool, station.id()).await?))
}

/// Creates a top, at the end of the list. Operators.
#[utoipa::path(post, path = "/tops", tag = "top30",
    request_body = TopSettings,
    responses((status = 201, body = TopView), (status = "4XX", body = ErrorBody)))]
async fn create_top(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Json(settings): Json<TopSettings>,
) -> ApiResult<(StatusCode, Json<TopView>)> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tops WHERE station_id = ?")
        .bind(station.id())
        .fetch_one(pool)
        .await?;
    if count as u64 >= MAX_TOPS {
        return Err(ApiError::Invalid(format!("at most {MAX_TOPS} tops")));
    }
    let settings = checked(pool, station.id(), None, settings).await?;
    let json = serde_json::to_string(&settings).map_err(internal)?;
    let id = sqlx::query(
        "INSERT INTO tops (station_id, position, settings)
         SELECT ?, COALESCE(MAX(position) + 1, 0), ? FROM tops WHERE station_id = ?",
    )
    .bind(station.id())
    .bind(json)
    .bind(station.id())
    .execute(pool)
    .await?
    .last_insert_id();
    tracing::info!(user = %user.username, station = station.id(), top = id, "top created");
    let view = views(pool, station.id())
        .await?
        .into_iter()
        .find(|view| view.id == id)
        .ok_or_else(|| ApiError::not_found(format!("top {id}")))?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn view_of(pool: &MySqlPool, station_id: u64, top_id: u64) -> ApiResult<TopView> {
    views(pool, station_id)
        .await?
        .into_iter()
        .find(|view| view.id == top_id)
        .ok_or_else(|| ApiError::not_found(format!("top {top_id}")))
}

/// A top of the station.
#[utoipa::path(get, path = "/tops/{top_id}", tag = "top30",
    params(("top_id" = u64, Path, description = "The top")),
    responses((status = 200, body = TopView), (status = "4XX", body = ErrorBody)))]
async fn get_top(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(top_id): Path<u64>,
) -> ApiResult<Json<TopView>> {
    Ok(Json(view_of(&state.pool, station.id(), top_id).await?))
}

/// Replaces a top's settings. Operators.
#[utoipa::path(put, path = "/tops/{top_id}", tag = "top30",
    params(("top_id" = u64, Path, description = "The top")),
    request_body = TopSettings,
    responses((status = 200, body = TopView), (status = "4XX", body = ErrorBody)))]
async fn put_top(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(top_id): Path<u64>,
    Json(settings): Json<TopSettings>,
) -> ApiResult<Json<TopView>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    // Its own: an unreadable top can be saved again.
    view_of(pool, station.id(), top_id).await?;
    let settings = checked(pool, station.id(), Some(top_id), settings).await?;
    let json = serde_json::to_string(&settings).map_err(internal)?;
    sqlx::query("UPDATE tops SET settings = ? WHERE id = ? AND station_id = ?")
        .bind(json)
        .bind(top_id)
        .bind(station.id())
        .execute(pool)
        .await?;
    tracing::info!(user = %user.username, station = station.id(), top = top_id, "top changed");
    Ok(Json(view_of(pool, station.id(), top_id).await?))
}

/// Deletes a top with its charts. Operators.
#[utoipa::path(delete, path = "/tops/{top_id}", tag = "top30",
    params(("top_id" = u64, Path, description = "The top")),
    responses((status = 204), (status = "4XX", body = ErrorBody)))]
async fn delete_top(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(top_id): Path<u64>,
) -> ApiResult<StatusCode> {
    user.require(Role::Operator)?;
    let deleted = sqlx::query("DELETE FROM tops WHERE id = ? AND station_id = ?")
        .bind(top_id)
        .bind(station.id())
        .execute(&state.pool)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found(format!("top {top_id}")));
    }
    tracing::info!(user = %user.username, station = station.id(), top = top_id, "top deleted");
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Routes: the charts

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChartQuery {
    /// A chart, by the day it is frozen (`2026-11-08`); the live one by
    /// default.
    pub key: Option<String>,
}

/// The chart a request names, the live one by default; `404` for a chart
/// to come or a day no chart is frozen.
async fn requested(
    pool: &MySqlPool,
    top: &Top,
    key: Option<&str>,
    now: Timestamp,
) -> ApiResult<ChartPeriod> {
    let tz = time_zone(pool, top.station_id).await?;
    let planned: Vec<Planned> = tops_of(pool, top.station_id)
        .await?
        .iter()
        .map(Top::planned)
        .collect();
    let live = live_period(top, &planned, &tz, now)?;
    let Some(key) = key else {
        return Ok(live);
    };
    let key = parse_key(key)?;
    if key == live.key {
        return Ok(live);
    }
    if key > live.key {
        return Err(ApiError::not_found(format!("the chart of {key}")));
    }
    if let Some(row) = frozen_row(pool, top.id, key).await? {
        return Ok(ChartPeriod {
            key,
            from: Timestamp::from_millisecond(row.from_ms).map_err(internal)?,
            to: Timestamp::from_millisecond(row.to_ms).map_err(internal)?,
        });
    }
    // A show built from a chart not frozen yet (an empty one).
    period_of_key(&top.settings, &tz, key)?
        .ok_or_else(|| ApiError::not_found(format!("the chart of {key}")))
}

/// A top's chart: the live one by default, a frozen one by its key.
#[utoipa::path(get, path = "/tops/{top_id}/chart", tag = "top30",
    params(("top_id" = u64, Path, description = "The top"), ChartQuery),
    responses((status = 200, body = Chart), (status = "4XX", body = ErrorBody)))]
async fn get_top_chart(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(top_id): Path<u64>,
    Query(query): Query<ChartQuery>,
) -> ApiResult<Json<Chart>> {
    let pool = &state.pool;
    let top = top_of(pool, station.id(), top_id).await?;
    let now = Timestamp::now();
    let period = requested(pool, &top, query.key.as_deref(), now).await?;
    Ok(Json(chart(pool, &top, &period, now).await?))
}

/// A frozen chart in the list of past charts.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PastChart {
    pub key: String,
    pub from_unix_ms: i64,
    pub to_unix_ms: i64,
    /// Its tracks.
    pub size: u32,
    pub number_one: Option<PastNumberOne>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PastNumberOne {
    pub track_id: Option<u64>,
    pub artist: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ChartsQuery {
    /// Charts before this one (its key), for the next page.
    pub before: Option<String>,
    /// 1 to 100, 20 by default.
    pub limit: Option<u32>,
}

/// A top's frozen charts, the latest first.
#[utoipa::path(get, path = "/tops/{top_id}/charts", tag = "top30",
    params(("top_id" = u64, Path, description = "The top"), ChartsQuery),
    responses((status = 200, body = Vec<PastChart>), (status = "4XX", body = ErrorBody)))]
async fn list_top_charts(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(top_id): Path<u64>,
    Query(query): Query<ChartsQuery>,
) -> ApiResult<Json<Vec<PastChart>>> {
    let limit = query.limit.unwrap_or(CHARTS_PAGE);
    if !(1..=MAX_CHARTS_PAGE).contains(&limit) {
        return Err(ApiError::BadRequest(format!(
            "limit is 1 to {MAX_CHARTS_PAGE}"
        )));
    }
    let exists: Option<u64> =
        sqlx::query_scalar("SELECT id FROM tops WHERE id = ? AND station_id = ?")
            .bind(top_id)
            .bind(station.id())
            .fetch_optional(&state.pool)
            .await?;
    exists.ok_or_else(|| ApiError::not_found(format!("top {top_id}")))?;
    let mut sql = QueryBuilder::new(
        "SELECT CAST(c.chart_key AS CHAR),
                CAST(UNIX_TIMESTAMP(c.from_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(c.to_at) * 1000 AS SIGNED),
                CAST((SELECT COUNT(*) FROM top_entries e
                      WHERE e.top_id = c.top_id AND e.chart_key = c.chart_key) AS UNSIGNED),
                CAST(o.`rank` AS UNSIGNED), o.track_id, o.artist, o.title
         FROM top_charts c
         LEFT JOIN top_entries o ON o.top_id = c.top_id AND o.chart_key = c.chart_key
                                AND o.`rank` = 1
         WHERE c.frozen_at IS NOT NULL AND c.top_id = ",
    );
    sql.push_bind(top_id);
    if let Some(before) = &query.before {
        sql.push(" AND c.chart_key < ")
            .push_bind(parse_key(before)?.to_string());
    }
    sql.push(" ORDER BY c.chart_key DESC LIMIT ")
        .push_bind(limit);
    type Row = (
        String,
        i64,
        i64,
        u64,
        Option<u64>,
        Option<u64>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sql.build_query_as().fetch_all(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(key, from, to, size, one, track_id, artist, title)| PastChart {
                    key,
                    from_unix_ms: from,
                    to_unix_ms: to,
                    size: size as u32,
                    number_one: one.map(|_| PastNumberOne {
                        track_id,
                        artist,
                        title,
                    }),
                },
            )
            .collect(),
    ))
}

/// A chart that can still change: not to come, no show built from it.
async fn changeable(
    pool: &MySqlPool,
    top: &Top,
    key: &str,
    now: Timestamp,
) -> ApiResult<ChartPeriod> {
    let period = requested(pool, top, Some(key), now).await?;
    if locked(pool, top.id, period.key).await? {
        return Err(ApiError::Conflict(
            "the show was built from this chart: it does not change any more".to_owned(),
        ));
    }
    Ok(period)
}

/// Freezes the chart again when it was frozen: its ranks follow the change.
async fn refreeze(pool: &MySqlPool, top: &Top, period: &ChartPeriod) -> ApiResult<()> {
    if frozen_row(pool, top.id, period.key).await?.is_some() {
        freeze(pool, top, period).await?;
    }
    Ok(())
}

/// Takes a track out of a chart: the tracks after it move up. `409` once a
/// show was built from the chart. Operators.
#[utoipa::path(post, path = "/tops/{top_id}/charts/{key}/removals/{track_id}", tag = "top30",
    params(("top_id" = u64, Path, description = "The top"),
           ("key" = String, Path, description = "The chart: the day it is frozen"),
           ("track_id" = u64, Path, description = "The track")),
    responses((status = 200, body = Chart), (status = "4XX", body = ErrorBody)))]
async fn remove_top_track(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path((top_id, key, track_id)): Path<(u64, String, u64)>,
) -> ApiResult<Json<Chart>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let top = top_of(pool, station.id(), top_id).await?;
    let now = Timestamp::now();
    let period = changeable(pool, &top, &key, now).await?;
    let exists: Option<u64> = sqlx::query_scalar("SELECT id FROM tracks WHERE id = ?")
        .bind(track_id)
        .fetch_optional(pool)
        .await?;
    exists.ok_or_else(|| ApiError::not_found(format!("track {track_id}")))?;
    sqlx::query(
        "INSERT IGNORE INTO top_removals (top_id, chart_key, track_id, removed_by, removed_at)
         VALUES (?, ?, ?, ?, NOW(6))",
    )
    .bind(top.id)
    .bind(period.key.to_string())
    .bind(track_id)
    .bind(user.id)
    .execute(pool)
    .await?;
    refreeze(pool, &top, &period).await?;
    tracing::info!(user = %user.username, top = top.id, chart = %period.key, track = track_id, "taken out of a top");
    Ok(Json(chart(pool, &top, &period, now).await?))
}

/// Puts back a track taken out of a chart. `409` once a show was built
/// from the chart. Operators.
#[utoipa::path(delete, path = "/tops/{top_id}/charts/{key}/removals/{track_id}", tag = "top30",
    params(("top_id" = u64, Path, description = "The top"),
           ("key" = String, Path, description = "The chart: the day it is frozen"),
           ("track_id" = u64, Path, description = "The track")),
    responses((status = 200, body = Chart), (status = "4XX", body = ErrorBody)))]
async fn restore_top_track(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path((top_id, key, track_id)): Path<(u64, String, u64)>,
) -> ApiResult<Json<Chart>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let top = top_of(pool, station.id(), top_id).await?;
    let now = Timestamp::now();
    let period = changeable(pool, &top, &key, now).await?;
    let deleted =
        sqlx::query("DELETE FROM top_removals WHERE top_id = ? AND chart_key = ? AND track_id = ?")
            .bind(top.id)
            .bind(period.key.to_string())
            .bind(track_id)
            .execute(pool)
            .await?
            .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found(format!(
            "track {track_id} taken out of that chart"
        )));
    }
    refreeze(pool, &top, &period).await?;
    tracing::info!(user = %user.username, top = top.id, chart = %period.key, track = track_id, "put back in a top");
    Ok(Json(chart(pool, &top, &period, now).await?))
}

/// The new release chosen by hand; `null` goes back to the proposed one.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewReleaseChoice {
    pub track_id: Option<u64>,
}

/// Chooses a chart's new release among the tracks of the new release
/// category, or goes back to the proposed one. `409` once a show was built
/// from the chart. Operators.
#[utoipa::path(put, path = "/tops/{top_id}/charts/{key}/new-release", tag = "top30",
    params(("top_id" = u64, Path, description = "The top"),
           ("key" = String, Path, description = "The chart: the day it is frozen")),
    request_body = NewReleaseChoice,
    responses((status = 200, body = Chart), (status = "4XX", body = ErrorBody)))]
async fn choose_top_new_release(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path((top_id, key)): Path<(u64, String)>,
    Json(choice): Json<NewReleaseChoice>,
) -> ApiResult<Json<Chart>> {
    user.require(Role::Operator)?;
    let pool = &state.pool;
    let top = top_of(pool, station.id(), top_id).await?;
    let now = Timestamp::now();
    let period = changeable(pool, &top, &key, now).await?;
    if let Some(track_id) = choice.track_id {
        let Some(category) = top.settings.new_release_category_id else {
            return Err(ApiError::Invalid(
                "the top has no new release category".to_owned(),
            ));
        };
        let found: Option<u64> = sqlx::query_scalar(
            "SELECT t.id FROM tracks t
             JOIN track_categories tc ON tc.track_id = t.id AND tc.category_id = ?
             WHERE t.id = ? AND t.deleted_at IS NULL",
        )
        .bind(category)
        .bind(track_id)
        .fetch_optional(pool)
        .await?;
        if found.is_none() {
            return Err(ApiError::Invalid(format!(
                "track {track_id} is not in the new release category"
            )));
        }
    }
    sqlx::query(
        "INSERT INTO top_charts (top_id, chart_key, from_at, to_at, new_release_track_id)
         VALUES (?, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), ?)
         ON DUPLICATE KEY UPDATE new_release_track_id = VALUES(new_release_track_id)",
    )
    .bind(top.id)
    .bind(period.key.to_string())
    .bind(seconds(period.from))
    .bind(seconds(period.to))
    .bind(choice.track_id)
    .execute(pool)
    .await?;
    tracing::info!(user = %user.username, top = top.id, chart = %period.key, track = ?choice.track_id, "top new release chosen");
    Ok(Json(chart(pool, &top, &period, now).await?))
}

// ---------------------------------------------------------------------------
// The show

/// What an item of the show is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Jingle,
    Track,
    NewRelease,
}

/// An item of a show's programme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Item {
    pub kind: ItemKind,
    /// The rank of a track, and of the jingle before it.
    pub rank: Option<u32>,
    pub track_id: u64,
    pub artist: Option<String>,
    pub title: Option<String>,
    /// What is played of it.
    pub length_ms: u32,
    pub planned_at_unix_ms: i64,
    /// Why it is passed over (a restriction, a track that cannot play).
    pub skipped: Option<String>,
    /// Sent to the queue.
    #[serde(default)]
    pub queued: bool,
}

/// A show's programme: its items in order, and the ranks left out to fit
/// its slot.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Programme {
    pub items: Vec<Item>,
    pub left_out: Vec<u32>,
}

/// The jingles of the show: one per rank when the category has them, and
/// the others in turn, the least recently played first.
#[derive(Clone, Debug, Default)]
pub struct Jingles {
    pub by_rank: HashMap<u32, u64>,
    pub others: Vec<u64>,
}

/// The rank a jingle's title ends with, after a dash or `#`: `Top 30 – 12`
/// and `Top 30 #12` are 12's; `Top 30` is no rank's.
pub fn rank_of_title(title: &str) -> Option<u32> {
    let title = title.trim();
    let (before, after) = title.rsplit_once(['-', '–', '—', '#'])?;
    if before.trim().is_empty() {
        return None;
    }
    let rank: u32 = after.trim().parse().ok()?;
    (rank > 0).then_some(rank)
}

/// Why `track` cannot play at `at_ms` after a track of `previous`
/// categories; `None` when it can.
fn passed_over(
    library: &Library,
    track: u64,
    at_ms: i64,
    previous: Option<&[u64]>,
) -> Option<String> {
    let Some(found) = library.tracks.get(&track) else {
        return Some("cannot play: disabled, missing or not analyzed".to_owned());
    };
    let subject = Subject {
        track,
        artists: &found.artists,
        categories: &found.categories,
    };
    forbidding(
        &library.restrictions,
        &subject,
        library.local(at_ms).datetime(),
        previous,
    )
    .map(|restriction| format!("restriction {} forbids it then", restriction.id))
}

fn length(library: &Library, track: u64) -> u32 {
    library
        .tracks
        .get(&track)
        .map_or(0, |track| track.length_ms)
}

/// The items for `ranks` (rank and track, in rank order), from the last
/// rank to number 1, each after its jingle, then the new release after a
/// jingle, from `opens_ms`. A track passed over goes without its jingle.
fn sequence(
    library: &Library,
    ranks: &[(u32, u64)],
    jingles: &Jingles,
    new_release: Option<u64>,
    opens_ms: i64,
) -> Vec<Item> {
    let mut items = Vec::new();
    let mut at = opens_ms;
    let mut previous: Option<Vec<u64>> = None;
    // The next of the other jingles that may play.
    let mut turn = 0usize;
    let order = ranks
        .iter()
        .rev()
        .map(|(rank, track)| (ItemKind::Track, Some(*rank), *track))
        .chain(new_release.map(|track| (ItemKind::NewRelease, None, track)));
    for (kind, rank, track) in order {
        // The rank's own jingle, else the next other one restrictions
        // allow; it plays when its track does.
        let allowed =
            |jingle: &u64| passed_over(library, *jingle, at, previous.as_deref()).is_none();
        let others = jingles.others.len();
        let jingle = rank
            .and_then(|rank| jingles.by_rank.get(&rank).copied())
            .filter(allowed)
            .map(|jingle| (jingle, None))
            .or_else(|| {
                (0..others)
                    .map(|step| (turn + step) % others)
                    .map(|index| (jingles.others[index], Some(index)))
                    .find(|(jingle, _)| allowed(jingle))
            });
        let (track_at, track_previous) = match jingle {
            Some((jingle, _)) => (
                at + i64::from(length(library, jingle)),
                library.tracks.get(&jingle).map(|j| j.categories.clone()),
            ),
            None => (at, previous.clone()),
        };
        if let Some(reason) = passed_over(library, track, track_at, track_previous.as_deref()) {
            items.push(Item {
                kind,
                rank,
                track_id: track,
                artist: None,
                title: None,
                length_ms: length(library, track),
                planned_at_unix_ms: at,
                skipped: Some(reason),
                queued: false,
            });
            continue;
        }
        if let Some((jingle, other)) = jingle {
            if let Some(index) = other {
                turn = index + 1;
            }
            let length_ms = length(library, jingle);
            items.push(Item {
                kind: ItemKind::Jingle,
                rank,
                track_id: jingle,
                artist: None,
                title: None,
                length_ms,
                planned_at_unix_ms: at,
                skipped: None,
                queued: false,
            });
            at += i64::from(length_ms);
        }
        let length_ms = length(library, track);
        items.push(Item {
            kind,
            rank,
            track_id: track,
            artist: None,
            title: None,
            length_ms,
            planned_at_unix_ms: at,
            skipped: None,
            queued: false,
        });
        at += i64::from(length_ms);
        previous = library.tracks.get(&track).map(|t| t.categories.clone());
    }
    items
}

/// Plans a show from `ranks` (rank and track): from the last rank to
/// number 1 with their jingles, then the new release, fitted to its slot
/// by leaving out the lowest ranks. Separation rules do not apply; the
/// restrictions do.
pub fn plan(
    library: &Library,
    ranks: &[(u32, u64)],
    jingles: &Jingles,
    new_release: Option<u64>,
    opens_ms: i64,
    closes_ms: i64,
) -> Programme {
    let mut ranks = ranks.to_vec();
    ranks.sort_by_key(|(rank, _)| *rank);
    let mut keep = ranks.len();
    loop {
        let items = sequence(library, &ranks[..keep], jingles, new_release, opens_ms);
        let total: i64 = items
            .iter()
            .filter(|item| item.skipped.is_none())
            .map(|item| i64::from(item.length_ms))
            .sum();
        if opens_ms + total <= closes_ms || keep <= 1 {
            return Programme {
                items,
                left_out: ranks[keep..].iter().map(|(rank, _)| *rank).collect(),
            };
        }
        keep -= 1;
    }
}

/// The jingles of the jingle category, playable on the station.
async fn load_jingles(pool: &MySqlPool, top: &Top, library: &Library) -> ApiResult<Jingles> {
    let Some(category) = top.settings.jingle_category_id else {
        return Ok(Jingles::default());
    };
    // Track, title, when it last played on the station (ms).
    let rows: Vec<(u64, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT t.id, t.title,
                (SELECT CAST(UNIX_TIMESTAMP(MAX(h.started_at)) * 1000 AS SIGNED)
                 FROM play_history h WHERE h.track_id = t.id AND h.station_id = ?)
         FROM tracks t JOIN track_categories tc ON tc.track_id = t.id AND tc.category_id = ?
         WHERE t.deleted_at IS NULL ORDER BY t.id",
    )
    .bind(top.station_id)
    .bind(category)
    .fetch_all(pool)
    .await?;
    let mut jingles = Jingles::default();
    let mut others = Vec::new();
    for (id, title, last) in rows {
        if !library.tracks.contains_key(&id) {
            continue;
        }
        match title.as_deref().and_then(rank_of_title) {
            Some(rank) => {
                jingles.by_rank.entry(rank).or_insert(id);
            }
            None => others.push((last.unwrap_or(i64::MIN), id)),
        }
    }
    others.sort();
    jingles.others = others.into_iter().map(|(_, id)| id).collect();
    Ok(jingles)
}

/// Writes the artist and title of each item.
async fn label(pool: &MySqlPool, items: &mut [Item]) -> ApiResult<()> {
    let ids: Vec<u64> = items
        .iter()
        .map(|item| item.track_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let labels = labels(pool, &ids).await?;
    for item in items {
        if let Some((artist, title)) = labels.get(&item.track_id) {
            item.artist.clone_from(artist);
            item.title.clone_from(title);
        }
    }
    Ok(())
}

/// The chart a show opening at `opens` plays: the latest frozen before it
/// (frozen now if it was not yet); for a top of the last 7 days, the 7
/// days before it, frozen for it.
fn chart_for(top: &Top, tz: &TimeZone, opens: Timestamp) -> ApiResult<ChartPeriod> {
    match top.settings.period {
        TopPeriod::Rolling7Days => Ok(rolling(opens, tz)),
        _ => latest_frozen(&top.settings, tz, opens),
    }
}

/// Builds the programme of the show opening at `opens`, from its chart
/// (frozen now if it was not yet). With `freeze` off, a chart of the last
/// 7 days is only looked at, as it stands.
async fn build(
    pool: &MySqlPool,
    top: &Top,
    tz: &TimeZone,
    opens: Timestamp,
    closes: Timestamp,
    freeze_it: bool,
) -> ApiResult<(ChartPeriod, Programme)> {
    let period = chart_for(top, tz, opens)?;
    let now = Timestamp::now();
    let entries = match frozen_row(pool, top.id, period.key).await? {
        Some(row) => stored_entries(pool, top.id, period.key, row.max_plays).await?,
        None if freeze_it || period.to <= now => freeze(pool, top, &period).await?,
        None => {
            // What it would be now: the 7 days up to now.
            let live = rolling(now, tz);
            let found = candidates(pool, top, period.key, live.from, now).await?;
            let (mut ranked, _, _) =
                rank(found, top.settings.plays_percent, top.settings.min_votes);
            ranked.truncate(top.settings.size as usize);
            with_movements(pool, top.id, period.key, &ranked).await?
        }
    };
    let ranks: Vec<(u32, u64)> = entries
        .iter()
        .filter_map(|entry| entry.track_id.map(|track| (entry.rank, track)))
        .collect();
    if ranks.is_empty() {
        return Ok((period, Programme::default()));
    }
    let library = crate::rotation::load_library(pool, top.station_id).await?;
    let jingles = load_jingles(pool, top, &library).await?;
    let chart: Vec<u64> = ranks.iter().map(|(_, track)| *track).collect();
    let new_release = new_release(pool, top, &period, &chart)
        .await?
        .map(|release| release.track_id);
    let mut programme = plan(
        &library,
        &ranks,
        &jingles,
        new_release,
        opens.as_millisecond(),
        closes.as_millisecond(),
    );
    label(pool, &mut programme.items).await?;
    Ok((period, programme))
}

/// A show kept: its top, when it opens, the chart it plays, its programme
/// and how many items are queued.
#[derive(sqlx::FromRow)]
struct Kept {
    top_id: u64,
    opens_ms: i64,
    closes_ms: i64,
    chart_key: Option<String>,
    programme: String,
    next_item: u64,
}

impl Kept {
    fn programme(&self) -> ApiResult<Programme> {
        serde_json::from_str(&self.programme).map_err(internal)
    }
}

const KEPT_COLUMNS: &str = "top_id, CAST(UNIX_TIMESTAMP(opens_at) * 1000 AS SIGNED) AS opens_ms,
    CAST(UNIX_TIMESTAMP(closes_at) * 1000 AS SIGNED) AS closes_ms,
    CAST(chart_key AS CHAR) AS chart_key, CAST(programme AS CHAR) AS programme,
    CAST(next_item AS UNSIGNED) AS next_item";

async fn kept_at(pool: &MySqlPool, top_id: u64, opens: Timestamp) -> ApiResult<Option<Kept>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {KEPT_COLUMNS} FROM top_shows WHERE top_id = ? AND opens_at = FROM_UNIXTIME(?)"
    )))
    .bind(top_id)
    .bind(seconds(opens))
    .fetch_optional(pool)
    .await?)
}

/// The latest show of the station (or of a top) that opened by `at` and
/// is still within its overrun at `since`.
async fn playing(
    pool: &MySqlPool,
    station_id: u64,
    top_id: Option<u64>,
    at: Timestamp,
    since: Timestamp,
) -> ApiResult<Option<Kept>> {
    let mut query = QueryBuilder::new(format!(
        "SELECT {KEPT_COLUMNS} FROM top_shows WHERE station_id = "
    ));
    query
        .push_bind(station_id)
        .push(" AND opens_at <= FROM_UNIXTIME(")
        .push_bind(seconds(at))
        .push(") AND closes_at > FROM_UNIXTIME(")
        .push_bind(seconds(since))
        .push(format!(") - INTERVAL {OVERRUN_SECONDS} SECOND"));
    if let Some(top_id) = top_id {
        query.push(" AND top_id = ").push_bind(top_id);
    }
    query.push(" ORDER BY opens_at DESC LIMIT 1");
    Ok(query.build_query_as().fetch_optional(pool).await?)
}

/// The items of the programme to queue next, from `next`: `wanted` that
/// play at most (a jingle comes with its track), and the index after them.
pub fn take(items: &[Item], next: usize, wanted: usize) -> (Vec<Item>, usize) {
    let mut taken: Vec<Item> = Vec::new();
    let mut index = next;
    while index < items.len() && taken.len() < wanted {
        let item = &items[index];
        index += 1;
        if item.skipped.is_none() {
            taken.push(item.clone());
        }
    }
    while taken
        .last()
        .is_some_and(|item| item.kind == ItemKind::Jingle)
        && index < items.len()
    {
        let item = &items[index];
        index += 1;
        if item.skipped.is_none() {
            taken.push(item.clone());
        }
    }
    (taken, index)
}

/// What the queue manager queues of a show now.
pub struct Due {
    pub top_id: u64,
    pub opens_ms: i64,
    pub items: Vec<Item>,
    /// `next_item` before and after them.
    pub from: usize,
    pub next: usize,
}

fn taken(kept: &Kept, wanted: usize) -> ApiResult<Option<Due>> {
    let programme = kept.programme()?;
    let from = kept.next_item as usize;
    let (items, next) = take(&programme.items, from, wanted);
    Ok((!items.is_empty()).then_some(Due {
        top_id: kept.top_id,
        opens_ms: kept.opens_ms,
        items,
        from,
        next,
    }))
}

/// The items of a top's show the queue needs, for a track planned at
/// `at_ms`: the programme playing on, else the one of the show `at_ms` is
/// in, built now. `None`: rotation picks (the module off, no show then,
/// the programme played, an empty chart). A top that cannot be read is
/// left out.
pub async fn due(
    pool: &MySqlPool,
    station_id: u64,
    at_ms: i64,
    wanted: usize,
) -> ApiResult<Option<Due>> {
    if wanted == 0 || !crate::modules::is_on(pool, Some(station_id), Module::Top30).await? {
        return Ok(None);
    }
    let at = Timestamp::from_millisecond(at_ms).map_err(internal)?;
    let tops = tops_of(pool, station_id).await?;
    if let Some(kept) = playing(pool, station_id, None, at, at).await?
        && tops
            .iter()
            .any(|top| top.id == kept.top_id && top.settings.enabled)
        && let Some(due) = taken(&kept, wanted)?
    {
        return Ok(Some(due));
    }
    let tz = time_zone(pool, station_id).await?;
    let planned: Vec<Planned> = tops.iter().map(Top::planned).collect();
    let Some((top_id, opens, closes)) = opening_at(&planned, &tz, at) else {
        return Ok(None);
    };
    let Some(top) = tops.iter().find(|top| top.id == top_id) else {
        return Ok(None);
    };
    if kept_at(pool, top.id, opens).await?.is_none() {
        let (period, programme) = build(pool, top, &tz, opens, closes, true).await?;
        let json = serde_json::to_string(&programme).map_err(internal)?;
        sqlx::query(
            "INSERT IGNORE INTO top_shows (top_id, station_id, opens_at, closes_at, chart_key,
                                           programme, built_at)
             VALUES (?, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), ?, ?, NOW(6))",
        )
        .bind(top.id)
        .bind(station_id)
        .bind(seconds(opens))
        .bind(seconds(closes))
        .bind(period.key.to_string())
        .bind(json)
        .execute(pool)
        .await?;
        tracing::info!(
            station = station_id,
            top = top.id,
            chart = %period.key,
            items = programme.items.len(),
            left_out = ?programme.left_out,
            "top show built"
        );
    }
    match kept_at(pool, top.id, opens).await? {
        Some(kept) => taken(&kept, wanted),
        None => Ok(None),
    }
}

/// Records that the queue took the items before `next`, unless another
/// call did meanwhile.
pub async fn queued(
    pool: &MySqlPool,
    top_id: u64,
    opens_ms: i64,
    from: usize,
    next: usize,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE top_shows SET next_item = ?
         WHERE top_id = ? AND opens_at = FROM_UNIXTIME(?) AND next_item = ?",
    )
    .bind(next as u32)
    .bind(top_id)
    .bind(opens_ms as f64 / 1_000.0)
    .bind(from as u32)
    .execute(pool)
    .await?;
    Ok(())
}

/// A top's show on now, or its next one.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ShowPlan {
    pub opens_at_unix_ms: i64,
    pub closes_at_unix_ms: i64,
    /// The chart it plays; `null` without one.
    pub key: Option<String>,
    /// Its programme is built: the one on air. Else what would be built now.
    pub built: bool,
    pub items: Vec<Item>,
    /// The ranks left out to fit the slot.
    pub left_out: Vec<u32>,
    pub music_ms: i64,
    pub jingles_ms: i64,
    pub new_release_ms: i64,
    pub total_ms: i64,
}

fn show_plan(
    opens: i64,
    closes: i64,
    key: Option<String>,
    built: bool,
    programme: Programme,
) -> ShowPlan {
    let sum = |kind: ItemKind| -> i64 {
        programme
            .items
            .iter()
            .filter(|item| item.kind == kind && item.skipped.is_none())
            .map(|item| i64::from(item.length_ms))
            .sum()
    };
    let (music_ms, jingles_ms, new_release_ms) = (
        sum(ItemKind::Track),
        sum(ItemKind::Jingle),
        sum(ItemKind::NewRelease),
    );
    ShowPlan {
        opens_at_unix_ms: opens,
        closes_at_unix_ms: closes,
        key,
        built,
        items: programme.items,
        left_out: programme.left_out,
        music_ms,
        jingles_ms,
        new_release_ms,
        total_ms: music_ms + jingles_ms + new_release_ms,
    }
}

/// A top's show on now, else its next one within 62 days, with its
/// programme: the one built, or what would be built now. `null` without
/// one.
#[utoipa::path(get, path = "/tops/{top_id}/show", tag = "top30",
    params(("top_id" = u64, Path, description = "The top")),
    responses((status = 200, body = Option<ShowPlan>), (status = "4XX", body = ErrorBody)))]
async fn get_top_show(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(top_id): Path<u64>,
) -> ApiResult<Json<Option<ShowPlan>>> {
    let pool = &state.pool;
    let top = top_of(pool, station.id(), top_id).await?;
    let tz = time_zone(pool, station.id()).await?;
    let now = Timestamp::now();
    let planned: Vec<Planned> = tops_of(pool, station.id())
        .await?
        .iter()
        .map(Top::planned)
        .collect();
    let Some((opens, closes)) = openings_of(&planned, top.id, &tz, now, LOOKAHEAD_DAYS)
        .into_iter()
        .find(|(_, closes)| *closes > now)
    else {
        return Ok(Json(None));
    };
    if let Some(kept) = kept_at(pool, top.id, opens).await? {
        let mut programme = kept.programme()?;
        for (index, item) in programme.items.iter_mut().enumerate() {
            item.queued = index < kept.next_item as usize;
        }
        return Ok(Json(Some(show_plan(
            kept.opens_ms,
            kept.closes_ms,
            kept.chart_key,
            true,
            programme,
        ))));
    }
    let (period, programme) = build(pool, &top, &tz, opens, closes, false).await?;
    Ok(Json(Some(show_plan(
        opens.as_millisecond(),
        closes.as_millisecond(),
        Some(period.key.to_string()),
        false,
        programme,
    ))))
}

// ---------------------------------------------------------------------------
// On the website

/// What has played of a show: the highest rank played (the ranks from it
/// down are revealed), the rank on air, and whether the new release
/// played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reveal {
    pub from_rank: Option<u32>,
    pub on_air_rank: Option<u32>,
    pub new_release: bool,
}

/// What `played` (the tracks started since the show opened, the one on
/// air included) reveals of `items`.
pub fn reveal(items: &[Item], played: &HashSet<u64>, on_air: Option<u64>) -> Reveal {
    let mut found = Reveal::default();
    for item in items.iter().filter(|item| item.skipped.is_none()) {
        match item.kind {
            ItemKind::Track if played.contains(&item.track_id) => {
                if let Some(rank) = item.rank {
                    found.from_rank = Some(found.from_rank.map_or(rank, |from| from.min(rank)));
                    if on_air == Some(item.track_id) {
                        found.on_air_rank = Some(rank);
                    }
                }
            }
            ItemKind::NewRelease if played.contains(&item.track_id) => found.new_release = true,
            _ => {}
        }
    }
    found
}

#[derive(Clone, Debug, Serialize)]
struct PublicEntry {
    rank: u32,
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    movement: Option<Movement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_rank: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    charts: Option<u32>,
    on_air: bool,
}

#[derive(Clone, Debug, Serialize)]
struct PublicNewRelease {
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PublicChart {
    key: String,
    entries: Vec<PublicEntry>,
}

#[derive(Clone, Debug, Serialize)]
struct PublicState {
    name: String,
    chart: Option<PublicChart>,
    new_release: Option<PublicNewRelease>,
    live: bool,
    on_air_rank: Option<u32>,
    votes_count_for_unix_ms: Option<i64>,
}

/// A top's live state for the gateway: the chart of its last show that
/// started, its ranks still to come hidden while it plays (their tracks are
/// not in it), and the show the votes of now count for next.
async fn public_state(
    pool: &MySqlPool,
    top: &Top,
    openings: &[(Timestamp, Timestamp)],
    tz: &TimeZone,
    now: Timestamp,
) -> ApiResult<serde_json::Value> {
    let settings = &top.settings;
    // Week and month: the first show after the period's freeze; the last
    // 7 days: the next show.
    let counts_from = match settings.period {
        TopPeriod::Rolling7Days => now,
        _ => period_at(settings, tz, now)?.to,
    };
    let votes_count_for = openings
        .iter()
        .find(|(opens, _)| *opens >= counts_from && *opens > now)
        .map(|(opens, _)| opens.as_millisecond());
    let mut public = PublicState {
        name: settings.title(),
        chart: None,
        new_release: None,
        live: false,
        on_air_rank: None,
        votes_count_for_unix_ms: votes_count_for.filter(|_| settings.enabled),
    };
    let last: Option<Kept> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {KEPT_COLUMNS} FROM top_shows
         WHERE top_id = ? AND opens_at <= NOW(6) AND chart_key IS NOT NULL
           AND JSON_LENGTH(programme, '$.items') > 0
         ORDER BY opens_at DESC LIMIT 1"
    )))
    .bind(top.id)
    .fetch_optional(pool)
    .await?;
    let Some(kept) = last else {
        return serde_json::to_value(public).map_err(internal);
    };
    let Some(key) = kept
        .chart_key
        .as_deref()
        .and_then(|key| key.parse::<Date>().ok())
    else {
        return serde_json::to_value(public).map_err(internal);
    };
    let programme = kept.programme()?;
    let entries = stored_entries(pool, top.id, key, None).await?;
    let now_ms = now.as_millisecond();
    let live = settings.enabled
        && openings
            .iter()
            .any(|(opens, closes)| opens.as_millisecond() == kept.opens_ms && now < *closes)
        && kept.opens_ms <= now_ms;
    let found = if live && settings.hide_unplayed {
        let played: HashSet<u64> = sqlx::query_scalar::<_, u64>(
            "SELECT DISTINCT track_id FROM play_history
             WHERE station_id = ? AND started_at >= FROM_UNIXTIME(?) AND deck <> 'fx'
               AND track_id IS NOT NULL",
        )
        .bind(top.station_id)
        .bind(kept.opens_ms as f64 / 1_000.0)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
        let on_air: Option<u64> = sqlx::query_scalar(
            "SELECT track_id FROM play_history
             WHERE station_id = ? AND ended_at IS NULL AND deck <> 'fx' AND track_id IS NOT NULL
             ORDER BY started_at DESC, id DESC LIMIT 1",
        )
        .bind(top.station_id)
        .fetch_optional(pool)
        .await?;
        reveal(&programme.items, &played, on_air)
    } else if live {
        // Every rank shown; the one on air still highlighted.
        let on_air: Option<u64> = sqlx::query_scalar(
            "SELECT track_id FROM play_history
             WHERE station_id = ? AND ended_at IS NULL AND deck <> 'fx' AND track_id IS NOT NULL
             ORDER BY started_at DESC, id DESC LIMIT 1",
        )
        .bind(top.station_id)
        .fetch_optional(pool)
        .await?;
        let played: HashSet<u64> = on_air.into_iter().collect();
        Reveal {
            from_rank: Some(1),
            on_air_rank: reveal(&programme.items, &played, on_air).on_air_rank,
            new_release: true,
        }
    } else {
        Reveal {
            from_rank: Some(1),
            on_air_rank: None,
            new_release: true,
        }
    };
    let shown = |rank: u32| found.from_rank.is_some_and(|from| rank >= from);
    public.live = live;
    public.on_air_rank = found.on_air_rank;
    public.chart = Some(PublicChart {
        key: key.to_string(),
        entries: entries
            .into_iter()
            .map(|entry| {
                if shown(entry.rank) {
                    PublicEntry {
                        rank: entry.rank,
                        hidden: false,
                        artist: entry.artist,
                        title: entry.title,
                        movement: Some(entry.movement),
                        previous_rank: entry.previous_rank,
                        charts: Some(entry.charts),
                        on_air: found.on_air_rank == Some(entry.rank),
                    }
                } else {
                    PublicEntry {
                        rank: entry.rank,
                        hidden: true,
                        artist: None,
                        title: None,
                        movement: None,
                        previous_rank: None,
                        charts: None,
                        on_air: false,
                    }
                }
            })
            .collect(),
    });
    public.new_release = programme
        .items
        .iter()
        .find(|item| item.kind == ItemKind::NewRelease && item.skipped.is_none())
        .map(|item| {
            if found.new_release {
                PublicNewRelease {
                    hidden: false,
                    artist: item.artist.clone(),
                    title: item.title.clone(),
                }
            } else {
                PublicNewRelease {
                    hidden: true,
                    artist: None,
                    title: None,
                }
            }
        });
    serde_json::to_value(public).map_err(internal)
}

/// Each top of the station as a show for the gateway, in order. A top that
/// fails shows without its live state.
pub async fn public_shows(
    pool: &MySqlPool,
    station_id: u64,
    timezone: &str,
) -> ApiResult<Vec<ShowPublic>> {
    let tz = TimeZone::get(timezone).unwrap_or(TimeZone::UTC);
    let tops = tops_of(pool, station_id).await?;
    let planned: Vec<Planned> = tops.iter().map(Top::planned).collect();
    let now = Timestamp::now();
    let from = now
        .checked_sub(SignedDuration::from_hours(24))
        .unwrap_or(now);
    let mut shows = Vec::new();
    for top in &tops {
        let openings = openings_of(&planned, top.id, &tz, from, LOOKAHEAD_DAYS);
        let mut public = ShowPublic::new(Module::Top30, &top.settings.show_settings(), timezone);
        public.key = top.key();
        public.title = Some(top.settings.title());
        public.openings = Some(as_openings(&openings));
        public.state = public_state(pool, top, &openings, &tz, now)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(station = station_id, top = top.id, %error, "a top's public state cannot be read");
                serde_json::Value::Null
            });
        shows.push(public);
    }
    Ok(shows)
}

// ---------------------------------------------------------------------------
// In the background

/// Freezes each top's charts as they end, sending `top.published`, and
/// sends `top.rank` as ranks start playing. Times that passed while the
/// API was stopped send nothing. Never returns.
pub async fn run(pool: MySqlPool, stations: Arc<Stations>, webhooks: Arc<Webhooks>) {
    let mut last = Timestamp::now();
    loop {
        tokio::time::sleep(TICK).await;
        let now = Timestamp::now();
        for station in stations.all() {
            let code = station.code();
            if let Err(error) = tick(&pool, &webhooks, station.id(), &code, last, now).await {
                tracing::warn!(station = %code, %error, "tops: cannot freeze or notify");
            }
        }
        last = now;
    }
}

/// `1. Artist – Title, 2. …`, the first three.
fn top3(entries: &[ChartEntry]) -> String {
    entries
        .iter()
        .take(3)
        .map(|entry| {
            format!(
                "{}. {}",
                entry.rank,
                track_text(&entry.artist, &entry.title)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn track_text(artist: &Option<String>, title: &Option<String>) -> String {
    [artist.as_deref(), title.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" – ")
}

/// A top as its notifications name it.
fn notice(top: &Top) -> Notice<'_> {
    Notice {
        show: top_show(),
        key: top.key(),
        name: top.settings.title(),
        website: &top.settings.website,
        notifications: &top.settings.notifications,
    }
}

/// When a show closes and when the next one opens.
type Times = (Option<i64>, Option<i64>);

/// When the show open at `at` closes, and when the next one opens.
fn end_and_next(openings: &[(Timestamp, Timestamp)], at: Timestamp) -> (Option<i64>, Option<i64>) {
    let status = core_shows::status_of_openings(&as_openings(openings), at);
    (status.closes_at_unix_ms, status.next_opening_unix_ms)
}

/// The background work of one station between `from` and `to`: each top
/// on its own, one failing never stopping the others.
pub(crate) async fn tick(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station_id: u64,
    code: &str,
    from: Timestamp,
    to: Timestamp,
) -> ApiResult<()> {
    if !crate::modules::is_on(pool, Some(station_id), Module::Top30).await? {
        return Ok(());
    }
    let tz = time_zone(pool, station_id).await?;
    let tops = tops_of(pool, station_id).await?;
    let planned: Vec<Planned> = tops.iter().map(Top::planned).collect();
    for top in &tops {
        if let Err(error) = tick_top(pool, webhooks, top, &planned, &tz, code, from, to).await {
            tracing::warn!(station = %code, top = top.id, %error, "a top: cannot freeze or notify");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn tick_top(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    top: &Top,
    planned: &[Planned],
    tz: &TimeZone,
    code: &str,
    from: Timestamp,
    to: Timestamp,
) -> ApiResult<()> {
    let openings = openings_of(
        planned,
        top.id,
        tz,
        from.checked_sub(SignedDuration::from_hours(24))
            .unwrap_or(from),
        LOOKAHEAD_DAYS,
    );
    // The charts frozen now: the reset of a week or a month, or the show of
    // a top of the last 7 days.
    let mut frozen_now: Vec<ChartPeriod> = Vec::new();
    match top.settings.period {
        TopPeriod::Rolling7Days => {
            for (opens, _) in &openings {
                if from < *opens && *opens <= to {
                    let period = rolling(*opens, tz);
                    if frozen_row(pool, top.id, period.key).await?.is_none() {
                        freeze(pool, top, &period).await?;
                    }
                    frozen_now.push(period);
                }
            }
        }
        _ => {
            let period = latest_frozen(&top.settings, tz, to)?;
            if frozen_row(pool, top.id, period.key).await?.is_none() {
                freeze(pool, top, &period).await?;
            }
            // Whoever froze it (a page read at the reset), it froze now.
            if from < period.to && period.to <= to {
                frozen_now.push(period);
            }
        }
    }
    if !top.settings.enabled {
        return Ok(());
    }
    for period in frozen_now {
        let entries = stored_entries(pool, top.id, period.key, None).await?;
        if let Some(first) = entries.first() {
            let values = [
                ("artist", first.artist.clone().unwrap_or_default()),
                ("title", first.title.clone().unwrap_or_default()),
                ("top3", top3(&entries)),
            ];
            let (end, next) = end_and_next(&openings, period.to);
            crate::shows::send_notice(
                pool,
                webhooks,
                top.station_id,
                code,
                &notice(top),
                WebhookEvent::TopPublished,
                (end, next),
                &values,
            )
            .await?;
        }
    }
    // Ranks that started playing.
    let Some(kept) = playing(pool, top.station_id, Some(top.id), to, from).await? else {
        return Ok(());
    };
    let started: Vec<(u64, i64)> = sqlx::query_as(
        "SELECT track_id, CAST(UNIX_TIMESTAMP(started_at) * 1000 AS SIGNED) FROM play_history
         WHERE station_id = ? AND deck <> 'fx' AND track_id IS NOT NULL
           AND started_at > FROM_UNIXTIME(?) AND started_at <= FROM_UNIXTIME(?)
           AND started_at >= FROM_UNIXTIME(?)
         ORDER BY started_at, id",
    )
    .bind(top.station_id)
    .bind(seconds(from))
    .bind(seconds(to))
    .bind(kept.opens_ms as f64 / 1_000.0)
    .fetch_all(pool)
    .await?;
    if started.is_empty() {
        return Ok(());
    }
    let programme = kept.programme()?;
    for (track, at_ms) in started {
        let Some(item) = programme.items.iter().find(|item| {
            item.kind == ItemKind::Track && item.track_id == track && item.skipped.is_none()
        }) else {
            continue;
        };
        let values = [
            ("rank", item.rank.unwrap_or_default().to_string()),
            ("artist", item.artist.clone().unwrap_or_default()),
            ("title", item.title.clone().unwrap_or_default()),
        ];
        let at = Timestamp::from_millisecond(at_ms).unwrap_or(to);
        crate::shows::send_notice(
            pool,
            webhooks,
            top.station_id,
            code,
            &notice(top),
            WebhookEvent::TopRank,
            end_and_next(&openings, at),
            &values,
        )
        .await?;
    }
    Ok(())
}

/// Sends each top's `show.upcoming`, `show.started` and `show.ended` due
/// in `(from, to]`, at its shows' times. A top that fails is left out.
pub(crate) async fn notify_shows(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station_id: u64,
    code: &str,
    tz: &TimeZone,
    from: Timestamp,
    to: Timestamp,
) -> ApiResult<()> {
    let tops = tops_of(pool, station_id).await?;
    let planned: Vec<Planned> = tops.iter().map(Top::planned).collect();
    let since = from
        .checked_sub(SignedDuration::from_hours(48))
        .unwrap_or(from);
    for top in tops.iter().filter(|top| top.settings.enabled) {
        let notifications = &top.settings.notifications;
        if notifications.webhook_ids.is_empty() {
            continue;
        }
        let lead = SignedDuration::from_mins(i64::from(notifications.lead_minutes));
        let openings = openings_of(&planned, top.id, tz, since, LOOKAHEAD_DAYS);
        let mut changes: Vec<(WebhookEvent, Timestamp, Times)> = Vec::new();
        for (opens, closes) in &openings {
            if let Ok(warned) = opens.checked_sub(lead) {
                changes.push((
                    WebhookEvent::ShowUpcoming,
                    warned,
                    (Some(closes.as_millisecond()), Some(opens.as_millisecond())),
                ));
            }
            changes.push((
                WebhookEvent::ShowStarted,
                *opens,
                end_and_next(&openings, *opens),
            ));
            changes.push((
                WebhookEvent::ShowEnded,
                *closes,
                end_and_next(&openings, *closes),
            ));
        }
        changes.sort_by_key(|(_, at, _)| *at);
        for (event, at, times) in changes {
            if !(from < at && at <= to) {
                continue;
            }
            if let Err(error) = crate::shows::send_notice(
                pool,
                webhooks,
                station_id,
                code,
                &notice(top),
                event,
                times,
                &[],
            )
            .await
            {
                tracing::warn!(station = %code, top = top.id, %error, "a top's notification cannot be sent");
            }
        }
    }
    Ok(())
}

/// The whole hours of the station's tops' shows, by top name, for the
/// other shows' check: an hour belongs to one show at most.
pub(crate) async fn hours_taken(
    pool: &MySqlPool,
    station_id: u64,
) -> ApiResult<Vec<(String, HourRange)>> {
    Ok(tops_of(pool, station_id)
        .await?
        .into_iter()
        .filter_map(|top| {
            let hours = top.settings.slot.as_ref()?.hours();
            Some((top.settings.name, hours))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use heron_rotation::{
        Category, LibraryParts, Member, Mode, Restriction, RotationSettings, Rule, Target, Track,
    };

    fn paris() -> TimeZone {
        TimeZone::get("Europe/Paris").unwrap()
    }

    fn at(text: &str) -> Timestamp {
        format!("{text}[Europe/Paris]")
            .parse::<jiff::Zoned>()
            .unwrap()
            .timestamp()
    }

    fn date(text: &str) -> Date {
        text.parse().unwrap()
    }

    fn slot(
        every: SlotEvery,
        weekday: Option<u8>,
        nth: Option<SlotNth>,
        start: &str,
        end: &str,
    ) -> TopSlot {
        TopSlot {
            every,
            weekdays: weekday.into_iter().collect(),
            weekday: None,
            nth,
            start: start.to_owned(),
            end: end.to_owned(),
        }
    }

    #[test]
    fn a_week_runs_from_one_reset_to_the_next() {
        let settings = TopSettings::default();
        let tz = paris();
        // Sunday 8 November 2026, 16:59: the week from the 1st, 17:00.
        let week = period_at(&settings, &tz, at("2026-11-08T16:59")).unwrap();
        assert_eq!(week.key, date("2026-11-08"));
        assert_eq!(week.from, at("2026-11-01T17:00"));
        assert_eq!(week.to, at("2026-11-08T17:00"));
        let frozen = latest_frozen(&settings, &tz, at("2026-11-08T16:59")).unwrap();
        assert_eq!(frozen.key, date("2026-11-01"));
        // From 17:00 it is frozen, and the next one runs: the show at
        // 18:00 plays it.
        let frozen = latest_frozen(&settings, &tz, at("2026-11-08T18:00")).unwrap();
        assert_eq!(frozen.key, date("2026-11-08"));
        let next = period_at(&settings, &tz, at("2026-11-08T17:00")).unwrap();
        assert_eq!(
            (next.key, next.from),
            (date("2026-11-15"), at("2026-11-08T17:00"))
        );
        // Across the end of summer time: 25 October 2026.
        let week = period_at(&settings, &tz, at("2026-10-25T12:00")).unwrap();
        assert_eq!(week.to, at("2026-10-25T17:00"));
        assert_eq!(week.from, at("2026-10-18T17:00"));
        // A key is the day of a reset, else no chart.
        assert_eq!(
            period_of_key(&settings, &tz, date("2026-11-01")).unwrap(),
            Some(frozen_at(&settings, "2026-11-01T17:00"))
        );
        assert_eq!(
            period_of_key(&settings, &tz, date("2026-11-02")).unwrap(),
            None
        );
    }

    fn frozen_at(settings: &TopSettings, text: &str) -> ChartPeriod {
        latest_frozen(settings, &paris(), at(text)).unwrap()
    }

    #[test]
    fn a_month_runs_from_its_reset_day() {
        let settings = TopSettings {
            period: TopPeriod::Month,
            reset_day: 1,
            reset_time: "00:00".to_owned(),
            ..TopSettings::default()
        };
        let tz = paris();
        let month = period_at(&settings, &tz, at("2026-11-08T12:00")).unwrap();
        assert_eq!(month.from, at("2026-11-01T00:00"));
        assert_eq!(month.to, at("2026-12-01T00:00"));
        assert_eq!(month.key, date("2026-12-01"));
        // The 1st Sunday of December plays November.
        let played = latest_frozen(&settings, &tz, at("2026-12-06T18:00")).unwrap();
        assert_eq!(
            (played.from, played.to),
            (at("2026-11-01T00:00"), at("2026-12-01T00:00"))
        );
        // Reset on the 15th at 09:30: before it, last month's.
        let mid = TopSettings {
            reset_day: 15,
            reset_time: "09:30".to_owned(),
            ..settings
        };
        let month = period_at(&mid, &tz, at("2026-11-15T09:00")).unwrap();
        assert_eq!(month.from, at("2026-10-15T09:30"));
        assert_eq!(month.to, at("2026-11-15T09:30"));
    }

    #[test]
    fn the_last_7_days_end_at_the_show() {
        let period = rolling(at("2026-11-08T20:00"), &paris());
        assert_eq!(period.key, date("2026-11-08"));
        assert_eq!(period.from, at("2026-11-01T20:00"));
        assert_eq!(period.to, at("2026-11-08T20:00"));
    }

    #[test]
    fn settings_are_checked() {
        assert_eq!(TopSettings::default().check(), Ok(()));
        let bad = |change: fn(&mut TopSettings)| {
            let mut settings = TopSettings::default();
            change(&mut settings);
            settings.check().is_err()
        };
        assert!(bad(|s| s.size = 12));
        assert!(bad(|s| s.name = " ".to_owned()));
        assert!(bad(|s| s.plays_percent = 55));
        assert!(bad(|s| s.plays_percent = 110));
        assert!(bad(|s| s.min_votes = 1_001));
        assert!(bad(|s| s.reset_day = 29));
        assert!(bad(|s| s.reset_weekday = 0));
        assert!(bad(|s| s.reset_time = "24:00".to_owned()));
        assert!(bad(|s| s.reset_time = "7:00".to_owned()));
        assert!(bad(
            |s| s.slot = Some(slot(SlotEvery::Week, None, None, "18:00", "20:00"))
        ));
        assert!(bad(
            |s| s.slot = Some(slot(SlotEvery::Day, None, None, "20:00", "18:00"))
        ));
        assert!(bad(|s| s.slot = Some(slot(
            SlotEvery::Day,
            None,
            Some(SlotNth::First),
            "18:00",
            "20:00"
        ))));
        assert!(bad(|s| s.website.color = "red".to_owned()));
        let mut ok = TopSettings {
            min_votes: 0,
            plays_percent: 100,
            ..TopSettings::default()
        };
        ok.slot = Some(slot(
            SlotEvery::Month,
            Some(7),
            Some(SlotNth::Last),
            "22:30",
            "24:00",
        ));
        assert_eq!(ok.check(), Ok(()));
        // Stored settings are read strictly: a field unknown is unreadable.
        assert!(parse(r#"{"name": "Top", "colour": 1}"#).is_err());
        assert!(parse(r#"{"name": "Top", "size": 12}"#).is_err());
        let parsed = parse(r#"{"name": "Top 3"}"#).unwrap();
        assert!(parsed.notifications.events.contains_key("top.published"));
        assert_eq!(minutes("24:00", true), Some(1_440));
        assert_eq!(minutes("23:60", false), None);
    }

    #[test]
    fn slots_play_on_their_days() {
        let first_sunday = slot(
            SlotEvery::Month,
            Some(7),
            Some(SlotNth::First),
            "18:00",
            "20:00",
        );
        assert!(first_sunday.occurs_on(date("2026-12-06")));
        assert!(!first_sunday.occurs_on(date("2026-12-13")));
        let last_sunday = slot(
            SlotEvery::Month,
            Some(7),
            Some(SlotNth::Last),
            "18:00",
            "20:00",
        );
        assert!(last_sunday.occurs_on(date("2026-11-29")));
        assert!(!last_sunday.occurs_on(date("2026-11-22")));
        let sundays = slot(SlotEvery::Week, Some(7), None, "18:00", "20:00");
        assert!(sundays.occurs_on(date("2026-11-22")));
        assert!(!sundays.occurs_on(date("2026-11-23")));
        assert!(slot(SlotEvery::Day, None, None, "20:00", "21:00").occurs_on(date("2026-11-23")));
        // The 4th and the last can be the same day: they overlap.
        let fourth = slot(
            SlotEvery::Month,
            Some(7),
            Some(SlotNth::Fourth),
            "19:00",
            "21:00",
        );
        assert!(fourth.overlaps(&last_sunday));
        assert!(!first_sunday.overlaps(&last_sunday));
        assert!(sundays.overlaps(&first_sunday));
        let mondays = slot(SlotEvery::Week, Some(1), None, "18:00", "20:00");
        assert!(!mondays.overlaps(&sundays));
        let later = slot(SlotEvery::Week, Some(7), None, "20:00", "21:00");
        assert!(!later.overlaps(&sundays), "back to back");
        assert_eq!(
            sundays.hours(),
            HourRange {
                days: vec![7],
                start_hour: 18,
                end_hour: 20
            }
        );
        let minutes_in = slot(SlotEvery::Day, None, None, "18:30", "19:10");
        assert_eq!(
            (minutes_in.hours().start_hour, minutes_in.hours().end_hour),
            (18, 20)
        );
    }

    #[test]
    fn the_rarer_top_takes_the_slot_that_day() {
        let tz = paris();
        let weekly = Planned {
            id: 1,
            enabled: true,
            slot: Some(slot(SlotEvery::Week, Some(7), None, "18:00", "20:00")),
        };
        let monthly = Planned {
            id: 2,
            enabled: true,
            slot: Some(slot(
                SlotEvery::Month,
                Some(7),
                Some(SlotNth::First),
                "18:00",
                "20:00",
            )),
        };
        // Every evening but Sundays: Monday to Saturday.
        let daily = Planned {
            id: 3,
            enabled: true,
            slot: Some(TopSlot {
                weekdays: (1..=6).collect(),
                ..slot(SlotEvery::Week, None, None, "19:30", "20:30")
            }),
        };
        let tops = vec![weekly.clone(), monthly.clone(), daily.clone()];
        let from = at("2026-11-23T00:00");
        // Sundays of 29 November to 13 December: the 6th is the monthly's.
        let sundays: Vec<Timestamp> = openings_of(&tops, 1, &tz, from, 21)
            .into_iter()
            .map(|(opens, _)| opens)
            .collect();
        assert_eq!(sundays, [at("2026-11-29T18:00"), at("2026-12-13T18:00")]);
        let monthly_shows = openings_of(&tops, 2, &tz, from, 21);
        assert_eq!(
            monthly_shows,
            [(at("2026-12-06T18:00"), at("2026-12-06T20:00"))]
        );
        // Every day but Sundays, which the weekly and the monthly take.
        let days = openings_of(&tops, 3, &tz, from, 6);
        assert!(
            days.iter()
                .all(|(opens, _)| opens.to_zoned(tz.clone()).weekday() != Weekday::Sunday)
        );
        assert_eq!(days.len(), 6);
        assert_eq!(
            opening_at(&tops, &tz, at("2026-12-06T19:45")),
            Some((2, at("2026-12-06T18:00"), at("2026-12-06T20:00")))
        );
        assert_eq!(
            opening_at(&tops, &tz, at("2026-12-07T19:45")),
            Some((3, at("2026-12-07T19:30"), at("2026-12-07T20:30")))
        );
        // A top that is not enabled takes nothing.
        let off = vec![
            weekly.clone(),
            Planned {
                enabled: false,
                ..monthly.clone()
            },
        ];
        assert_eq!(openings_of(&off, 1, &tz, from, 21).len(), 3);
        assert!(openings_of(&off, 2, &tz, from, 21).is_empty());
        // The list says who takes the slot.
        let top = |planned: &Planned, name: &str| Top {
            id: planned.id,
            station_id: 1,
            settings: TopSettings {
                name: name.to_owned(),
                slot: planned.slot.clone(),
                ..TopSettings::default()
            },
        };
        let all = vec![
            top(&weekly, "Top 30"),
            top(&monthly, "Monthly"),
            top(&daily, "Evening"),
        ];
        let given: Vec<u64> = gives_way(&all[0], &all).iter().map(|g| g.top_id).collect();
        assert_eq!(given, [2]);
        assert!(gives_way(&all[1], &all).is_empty());
        assert!(gives_way(&all[2], &all).is_empty());
        // Two of the same rarity on the same slot cannot be saved: every
        // day of the week takes Sunday too.
        let other_sunday = slot(SlotEvery::Week, Some(7), None, "19:00", "21:00");
        assert_eq!(same_slot(&other_sunday, &all).map(|t| t.id), Some(1));
        let every_day = TopSlot {
            weekdays: (1..=7).collect(),
            ..slot(SlotEvery::Week, None, None, "19:00", "21:00")
        };
        assert!(same_slot(&every_day, &all).is_some());
        let saturday = slot(SlotEvery::Week, Some(6), None, "21:00", "22:00");
        assert!(same_slot(&saturday, &all).is_none());
        let saturday_evening = slot(SlotEvery::Week, Some(6), None, "20:00", "21:00");
        assert_eq!(same_slot(&saturday_evening, &all).map(|t| t.id), Some(3));
    }

    #[test]
    fn a_show_s_days_are_chosen_one_by_one() {
        // Monday, Wednesday and Friday.
        let three = TopSlot {
            weekdays: vec![5, 1, 3, 3],
            ..slot(SlotEvery::Week, None, None, "20:00", "21:00")
        };
        assert_eq!(three.check(), Ok(()));
        let days: Vec<bool> = (23..=29)
            .map(|day| three.occurs_on(date(&format!("2026-11-{day}"))))
            .collect();
        assert_eq!(days, [true, false, true, false, true, false, false]);
        assert_eq!(three.hours().days, [1, 3, 5]);
        // The 1st Saturday and Sunday of the month.
        let first_weekend = TopSlot {
            weekdays: vec![6, 7],
            ..slot(
                SlotEvery::Month,
                None,
                Some(SlotNth::First),
                "18:00",
                "20:00",
            )
        };
        let on: Vec<u32> = (1..=31)
            .filter(|day| first_weekend.occurs_on(date(&format!("2026-08-{day:02}"))))
            .collect();
        assert_eq!(on, [1, 2]);
        // No day: refused.
        let none = slot(SlotEvery::Week, None, None, "20:00", "21:00");
        assert!(none.check().is_err());
        assert!(
            TopSlot {
                weekdays: vec![8],
                ..none.clone()
            }
            .check()
            .is_err()
        );
        // Settings saved before: every day, or a single day.
        let mut legacy: TopSlot = serde_json::from_str(
            r#"{"every": "day", "weekday": null, "nth": null, "start": "20:00", "end": "21:00"}"#,
        )
        .unwrap();
        assert_eq!(legacy.check(), Ok(()));
        legacy.normalize();
        assert_eq!(
            (legacy.every, legacy.weekdays.clone()),
            (SlotEvery::Week, (1..=7).collect())
        );
        let mut single: TopSlot = serde_json::from_str(
            r#"{"every": "week", "weekday": 7, "nth": null, "start": "20:00", "end": "21:00"}"#,
        )
        .unwrap();
        assert!(single.occurs_on(date("2026-11-29")));
        single.normalize();
        assert_eq!((single.weekdays.clone(), single.weekday), (vec![7], None));
        let saved = serde_json::to_value(&single).unwrap();
        assert!(saved.get("weekday").is_none(), "{saved}");
    }

    #[test]
    fn a_show_at_midnight_and_across_summer_time() {
        let tz = paris();
        let late = Planned {
            id: 1,
            enabled: true,
            slot: Some(slot(SlotEvery::Week, Some(7), None, "23:00", "24:00")),
        };
        let shows = openings_of(&[late], 1, &tz, at("2026-10-24T00:00"), 2);
        assert_eq!(shows, [(at("2026-10-25T23:00"), at("2026-10-26T00:00"))]);
    }

    fn candidate(track_id: u64, plays: u64, likes: u64, dislikes: u64) -> Candidate {
        Candidate {
            track_id,
            artist: None,
            title: None,
            plays,
            likes,
            dislikes,
        }
    }

    fn ids(scored: &[Scored]) -> Vec<u64> {
        scored.iter().map(|s| s.candidate.track_id).collect()
    }

    #[test]
    fn the_score_mixes_plays_and_likes() {
        let tracks = || {
            vec![
                candidate(1, 20, 9, 1),  // most played, 90 %
                candidate(2, 10, 10, 0), // 100 %
                candidate(3, 5, 3, 7),
                candidate(4, 18, 1, 0), // one vote
                candidate(5, 0, 0, 0),
            ]
        };
        // Likes only, as the Top 30 ranked them.
        let (ranked, below, max) = rank(tracks(), 0, 10);
        assert_eq!(max, 20);
        assert_eq!(ids(&ranked), [2, 1, 3]);
        assert_eq!(ids(&below), [4]);
        // Plays only: the most played first.
        let (ranked, below, _) = rank(tracks(), 100, 0);
        assert_eq!(ids(&ranked), [1, 4, 2, 3]);
        assert!(below.is_empty());
        assert_eq!(ranked[0].score, 1.0);
        assert_eq!(ranked[2].plays_share, 0.5);
        // Half and half: 1 is 0.5 + 0.45, 2 is 0.25 + 0.5.
        let (ranked, _, _) = rank(tracks(), 50, 0);
        assert_eq!(ids(&ranked), [1, 4, 2, 3]);
        assert!((ranked[0].score - 0.95).abs() < 1e-9);
        // A track without a vote counts 0 for the likes.
        let (ranked, _, _) = rank(vec![candidate(6, 4, 0, 0), candidate(7, 2, 0, 0)], 30, 0);
        assert_eq!(ids(&ranked), [6, 7]);
        assert!((ranked[0].score - 0.3).abs() < 1e-9);
    }

    #[test]
    fn ties_go_to_votes_then_plays_then_the_oldest() {
        let tracks = vec![
            candidate(9, 4, 5, 5),
            candidate(8, 4, 10, 10),
            candidate(7, 6, 5, 5),
            candidate(6, 4, 5, 5),
        ];
        let (ranked, _, _) = rank(tracks, 0, 0);
        assert_eq!(ids(&ranked), [8, 7, 6, 9]);
    }

    #[test]
    fn movements_against_the_previous_chart() {
        assert_eq!(movement(2, Some(3), 1), Movement::Up);
        assert_eq!(movement(3, Some(1), 1), Movement::Down);
        assert_eq!(movement(1, Some(1), 4), Movement::Same);
        assert_eq!(movement(9, None, 2), Movement::Reentry);
        assert_eq!(movement(6, None, 0), Movement::New);
    }

    #[test]
    fn jingles_of_a_rank_end_with_it() {
        assert_eq!(rank_of_title("Top 30 – 12"), Some(12));
        assert_eq!(rank_of_title("Top 30 #7"), Some(7));
        assert_eq!(rank_of_title("Top 30 - 1 "), Some(1));
        assert_eq!(rank_of_title("Top 30"), None);
        assert_eq!(rank_of_title("Top 30 – opening"), None);
        assert_eq!(rank_of_title("#12"), None);
    }

    /// Tracks 1 to 40 of 3 minutes (jingles 101 to 103 of 10 seconds), in
    /// category 1 (music) or 2 (jingles).
    fn library(restrictions: Vec<Restriction>) -> Library {
        let track = |id: u64, length_ms: u32| Track {
            id,
            title_key: Some(format!("t{id}")),
            album_key: None,
            artists: vec![id],
            categories: Vec::new(),
            length_ms,
            mix_ms: None,
            ratio: 10,
            dayparts: None,
            daypart_days: None,
        };
        let mut tracks: Vec<Track> = (1..=40).map(|id| track(id, 180_000)).collect();
        tracks.extend((101..=103).map(|id| track(id, 10_000)));
        let members = |ids: Vec<u64>| -> Vec<Member> {
            ids.into_iter()
                .map(|track| Member { track, weight: 1.0 })
                .collect()
        };
        Library::new(
            LibraryParts {
                tracks,
                categories: vec![
                    Category {
                        id: 1,
                        name: "Music".to_owned(),
                        separation: Default::default(),
                        members: members((1..=40).collect()),
                    },
                    Category {
                        id: 2,
                        name: "Jingles".to_owned(),
                        separation: Default::default(),
                        members: members((101..=103).collect()),
                    },
                ],
                similar: Vec::new(),
                restrictions,
                clocks: Vec::new(),
                schedule: Default::default(),
                schedules: Vec::new(),
                settings: RotationSettings::default(),
                medley_category: None,
                medleys: Vec::new(),
                no_two_in_a_row: Vec::new(),
                counters: HashMap::new(),
                station_ratios: HashMap::new(),
                modules_off: Vec::new(),
            },
            paris(),
        )
    }

    fn kinds(programme: &Programme) -> Vec<(ItemKind, Option<u32>, u64)> {
        programme
            .items
            .iter()
            .filter(|item| item.skipped.is_none())
            .map(|item| (item.kind, item.rank, item.track_id))
            .collect()
    }

    #[test]
    fn the_show_counts_down_then_plays_the_new_release() {
        let library = library(Vec::new());
        let jingles = Jingles {
            by_rank: HashMap::from([(1, 103)]),
            others: vec![101, 102],
        };
        let opens = at("2026-11-08T18:00").as_millisecond();
        let closes = at("2026-11-08T20:00").as_millisecond();
        // Three ranked tracks: fewer than 30, the show counts down from 3.
        let ranks = [(1, 7), (2, 8), (3, 9)];
        let programme = plan(&library, &ranks, &jingles, Some(40), opens, closes);
        use ItemKind::{Jingle, NewRelease, Track as T};
        assert_eq!(
            kinds(&programme),
            [
                (Jingle, Some(3), 101),
                (T, Some(3), 9),
                (Jingle, Some(2), 102),
                (T, Some(2), 8),
                (Jingle, Some(1), 103),
                (T, Some(1), 7),
                (Jingle, None, 101),
                (NewRelease, None, 40),
            ]
        );
        assert!(programme.left_out.is_empty());
        // Planned one after the other.
        assert_eq!(programme.items[0].planned_at_unix_ms, opens);
        assert_eq!(programme.items[1].planned_at_unix_ms, opens + 10_000);
        assert_eq!(programme.items[2].planned_at_unix_ms, opens + 190_000);
        // Without jingles, the tracks follow one another.
        let programme = plan(&library, &ranks, &Jingles::default(), None, opens, closes);
        assert_eq!(
            kinds(&programme),
            [(T, Some(3), 9), (T, Some(2), 8), (T, Some(1), 7)]
        );
    }

    #[test]
    fn the_lowest_ranks_are_left_out_to_fit_the_slot() {
        let library = library(Vec::new());
        let jingles = Jingles {
            by_rank: HashMap::new(),
            others: vec![101],
        };
        let opens = at("2026-11-08T18:00").as_millisecond();
        let closes = at("2026-11-08T19:00").as_millisecond();
        // 30 tracks of 3 minutes do not fit in an hour.
        let ranks: Vec<(u32, u64)> = (1..=30).map(|rank| (rank, u64::from(rank))).collect();
        let programme = plan(&library, &ranks, &jingles, Some(40), opens, closes);
        // 17 tracks and the new release, each after 10 s of jingle: 57 min;
        // 18: 60 min 10 s.
        assert_eq!(programme.left_out, (18..=30).collect::<Vec<u32>>());
        let first = &programme.items[1];
        assert_eq!((first.kind, first.rank), (ItemKind::Track, Some(17)));
        let last = programme.items.last().unwrap();
        assert_eq!(last.kind, ItemKind::NewRelease);
        let end = last.planned_at_unix_ms + i64::from(last.length_ms);
        assert!(end <= closes);
    }

    #[test]
    fn restrictions_apply_in_the_show() {
        // Track 8 is banned on Sundays, jingle 102 too.
        let ban = |id: u64, track: u64| Restriction {
            id,
            target: Target::Track(track),
            rule: Rule::Daypart {
                mode: Mode::Ban,
                days: vec![7],
                start_hour: 0,
                end_hour: 24,
            },
        };
        let library = library(vec![ban(1, 8), ban(2, 102)]);
        let jingles = Jingles {
            by_rank: HashMap::new(),
            others: vec![101, 102],
        };
        let opens = at("2026-11-08T18:00").as_millisecond();
        let closes = at("2026-11-08T20:00").as_millisecond();
        let ranks = [(1, 7), (2, 8), (3, 9), (4, 999)];
        let programme = plan(&library, &ranks, &jingles, None, opens, closes);
        use ItemKind::{Jingle, Track as T};
        assert_eq!(
            kinds(&programme),
            [
                (Jingle, Some(3), 101),
                (T, Some(3), 9),
                (Jingle, Some(1), 101),
                (T, Some(1), 7),
            ]
        );
        let skipped: Vec<(Option<u32>, &str)> = programme
            .items
            .iter()
            .filter_map(|item| item.skipped.as_deref().map(|why| (item.rank, why)))
            .collect();
        assert_eq!(
            skipped,
            [
                (Some(4), "cannot play: disabled, missing or not analyzed"),
                (Some(2), "restriction 1 forbids it then"),
            ]
        );
    }

    fn item(kind: ItemKind, rank: Option<u32>, track_id: u64) -> Item {
        Item {
            kind,
            rank,
            track_id,
            artist: None,
            title: None,
            length_ms: 1_000,
            planned_at_unix_ms: 0,
            skipped: None,
            queued: false,
        }
    }

    #[test]
    fn the_queue_takes_a_jingle_with_its_track() {
        use ItemKind::{Jingle, NewRelease, Track as T};
        let mut items = vec![
            item(Jingle, Some(3), 101),
            item(T, Some(3), 9),
            item(T, Some(2), 8),
            item(Jingle, Some(1), 101),
            item(T, Some(1), 7),
            item(NewRelease, None, 40),
        ];
        items[2].skipped = Some("restriction".to_owned());
        let (taken, next) = take(&items, 0, 1);
        assert_eq!(
            taken.iter().map(|i| i.track_id).collect::<Vec<_>>(),
            [101, 9]
        );
        assert_eq!(next, 2);
        // The track passed over is not queued.
        let (taken, next) = take(&items, next, 2);
        assert_eq!(
            taken.iter().map(|i| i.track_id).collect::<Vec<_>>(),
            [101, 7]
        );
        assert_eq!(next, 5);
        let (taken, next) = take(&items, next, 3);
        assert_eq!(taken.iter().map(|i| i.track_id).collect::<Vec<_>>(), [40]);
        assert_eq!(next, 6);
        assert!(take(&items, next, 3).0.is_empty());
    }

    #[test]
    fn ranks_show_as_they_play() {
        use ItemKind::{Jingle, NewRelease, Track as T};
        let mut items = vec![
            item(Jingle, Some(4), 101),
            item(T, Some(4), 10),
            item(T, Some(3), 9),
            item(Jingle, Some(2), 101),
            item(T, Some(2), 8),
            item(T, Some(1), 7),
            item(NewRelease, None, 40),
        ];
        items[2].skipped = Some("restriction".to_owned());
        assert_eq!(reveal(&items, &HashSet::new(), None), Reveal::default());
        let played = HashSet::from([101, 10]);
        assert_eq!(
            reveal(&items, &played, Some(10)),
            Reveal {
                from_rank: Some(4),
                on_air_rank: Some(4),
                new_release: false
            }
        );
        // Number 3 was passed over: it shows once number 2 plays.
        let played = HashSet::from([101, 10, 8]);
        assert_eq!(reveal(&items, &played, Some(101)).from_rank, Some(2));
        let played = HashSet::from([101, 10, 8, 7, 40]);
        let found = reveal(&items, &played, Some(40));
        assert_eq!(found.from_rank, Some(1));
        assert_eq!(found.on_air_rank, None);
        assert!(found.new_release);
    }
}
