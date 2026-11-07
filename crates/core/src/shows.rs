// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shows: the modules listeners take part in from the station's website
//! (`docs/listener-interaction.md`). Their settings per station, their
//! opening hours in whole hours, and what is open when.
//!
//! Shared by `heron-api`, which keeps the settings and sends the
//! notifications, and `heron-public`, which tells widgets whether a show is
//! open, even while the API does not answer.

use std::collections::BTreeMap;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, Zoned};
use serde::{Deserialize, Serialize};

use crate::Module;

/// A show's entry: what the common base needs to know of it. Each show adds
/// its own events and placeholders here.
#[derive(Clone, Copy, Debug)]
pub struct Show {
    pub module: Module,
    /// Its name in English, for `{name}` when the station gave no title.
    pub name: &'static str,
    /// Whether it opens at set hours (Likes are always open, and take no
    /// hours from the others).
    pub has_hours: bool,
    /// Its own notification events, after the common ones: name, default
    /// message, on by default.
    pub events: &'static [(&'static str, &'static str, bool)],
    /// Its own placeholders in messages, after the common ones.
    pub placeholders: &'static [&'static str],
}

pub const SHOWS: [Show; 4] = [
    // Each accepted dedication as it plays, with its message; each one
    // accepted (#131).
    Show {
        module: Module::Dedications,
        name: "Dedications",
        has_hours: true,
        events: &[
            (
                DEDICATION_PLAYED,
                "♥ {artist} – {title}, for {to} from {from}: « {message} »",
                true,
            ),
            (
                DEDICATION_ACCEPTED,
                "Dedication accepted for {to}, around {time}",
                false,
            ),
        ],
        placeholders: &["artist", "title", "from", "to", "message", "time"],
    },
    Show {
        module: Module::Match,
        name: "The match",
        has_hours: true,
        events: &[],
        placeholders: &[],
    },
    // The chart when the week closes; each rank as it plays (#134).
    Show {
        module: Module::Top30,
        name: "Top 30",
        has_hours: true,
        events: &[
            (
                TOP_PUBLISHED,
                "This week's number 1: {artist} – {title}",
                true,
            ),
            (TOP_RANK, "Number {rank}: {artist} – {title}", false),
        ],
        placeholders: &["artist", "title", "top3", "rank"],
    },
    // No notification per vote: too many (#133).
    Show {
        module: Module::Likes,
        name: "Likes",
        has_hours: false,
        events: &[],
        placeholders: &[],
    },
];

/// The notification events every show has, with their default message.
pub const COMMON_EVENTS: [(&str, &str, bool); 3] = [
    (SHOW_UPCOMING, "{name} opens at {next}: {link}", true),
    (SHOW_STARTED, "{name} is open until {end}: {link}", true),
    (SHOW_ENDED, "{name} is closed. Back {next}", true),
];
pub const SHOW_UPCOMING: &str = "show.upcoming";
pub const SHOW_STARTED: &str = "show.started";
pub const SHOW_ENDED: &str = "show.ended";
/// The Top 30's chart, when its week closes.
pub const TOP_PUBLISHED: &str = "top.published";
/// A rank of the Top 30, as it starts playing in the show.
pub const TOP_RANK: &str = "top.rank";

/// An accepted dedication, as it goes on air.
pub const DEDICATION_PLAYED: &str = "dedication.played";
/// A dedication, when it is accepted.
pub const DEDICATION_ACCEPTED: &str = "dedication.accepted";

/// The placeholders of every show's messages.
pub const COMMON_PLACEHOLDERS: [&str; 4] = ["name", "link", "end", "next"];
/// The placeholders of the closed message.
pub const CLOSED_PLACEHOLDERS: [&str; 2] = ["day", "hour"];

/// The languages of the dates in messages.
pub const LANGUAGES: [&str; 4] = ["en", "fr", "es", "de"];

const MAX_RANGES: usize = 50;
const MAX_TITLE: usize = 60;
const MAX_CLOSED_MESSAGE: usize = 200;
const MAX_MESSAGE: usize = 500;
const MAX_URL: usize = 500;
const MAX_LEAD_MINUTES: u32 = 1_440;

impl Show {
    /// The entry of a show module; `None` for a module that is not a show.
    pub fn of(module: Module) -> Option<&'static Show> {
        SHOWS.iter().find(|show| show.module == module)
    }

    /// The settings group a station keeps its settings in
    /// (`station_settings`).
    pub fn settings_name(self) -> &'static str {
        match self.module {
            Module::Dedications => "show_dedications",
            Module::Match => "show_match",
            Module::Likes => "show_likes",
            _ => "show_top30",
        }
    }

    /// Every event it sends: the common ones (for a show with hours,
    /// which opens and closes), then its own.
    pub fn all_events(self) -> impl Iterator<Item = (&'static str, &'static str, bool)> {
        let common: &[_] = if self.has_hours { &COMMON_EVENTS } else { &[] };
        common.iter().copied().chain(self.events.iter().copied())
    }

    pub fn all_placeholders(self) -> impl Iterator<Item = &'static str> {
        COMMON_PLACEHOLDERS
            .into_iter()
            .chain(self.placeholders.iter().copied())
    }
}

/// Days and whole hours: Monday to Friday, 17:00 to 18:00.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct HourRange {
    /// 1 (Monday) to 7 (Sunday).
    pub days: Vec<u8>,
    /// 0 to 23.
    pub start_hour: u8,
    /// After `start_hour`, up to 24.
    pub end_hour: u8,
}

/// What the website shows outside the hours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Closed {
    /// A message with the next opening.
    #[default]
    Message,
    /// Nothing: the page and the small widget stay empty.
    Nothing,
}

/// The page and the small widget on the station's website.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct WebsiteSettings {
    /// The show's name on the website; empty: its name in the listener's
    /// language.
    pub title: String,
    /// `#rrggbb`.
    pub color: String,
    /// Whether the small widget shows (the dedicated page always does).
    pub small_widget: bool,
    pub closed: Closed,
    /// With `{day}` and `{hour}` for the next opening; empty: the widget's
    /// own sentence.
    pub closed_message: String,
    /// The website's page of the show, for `{link}`.
    pub page_url: Option<String>,
}

impl Default for WebsiteSettings {
    fn default() -> Self {
        Self {
            title: String::new(),
            color: "#c62828".to_owned(),
            small_widget: true,
            closed: Closed::Message,
            closed_message: String::new(),
            page_url: None,
        }
    }
}

/// One notification event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct EventSettings {
    pub enabled: bool,
    /// With `{name}`, `{link}`, `{end}`, `{next}` and the show's own
    /// placeholders.
    pub message: String,
}

/// Notifications through the station's webhooks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct NotificationSettings {
    /// Webhooks of the station that receive the show's events.
    pub webhook_ids: Vec<u64>,
    /// When `show.upcoming` is sent before the show opens, 1 to 1440.
    pub lead_minutes: u32,
    /// The language of the dates: `en`, `fr`, `es` or `de`.
    pub language: String,
    /// Each event by name (`show.started`).
    pub events: BTreeMap<String, EventSettings>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            webhook_ids: Vec::new(),
            lead_minutes: 60,
            language: "en".to_owned(),
            events: BTreeMap::new(),
        }
    }
}

/// A show's settings on a station.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct ShowSettings {
    /// Open during its hours; off, it stays closed.
    pub enabled: bool,
    /// In the station's time zone.
    pub hours: Vec<HourRange>,
    pub website: WebsiteSettings,
    pub notifications: NotificationSettings,
}

impl Default for ShowSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            hours: Vec::new(),
            website: WebsiteSettings::default(),
            notifications: NotificationSettings::default(),
        }
    }
}

impl ShowSettings {
    /// Gives every event of the show its default when it has none, and
    /// drops events the show does not send.
    pub fn complete(&mut self, show: &Show) {
        let events = &mut self.notifications.events;
        events.retain(|name, _| show.all_events().any(|(known, _, _)| known == name));
        for (name, message, enabled) in show.all_events() {
            events
                .entry(name.to_owned())
                .or_insert_with(|| EventSettings {
                    enabled,
                    message: message.to_owned(),
                });
        }
    }

    /// Checks everything but the overlaps with other shows and the
    /// webhooks, which need the station.
    pub fn check(&self, show: &Show) -> Result<(), String> {
        if !show.has_hours && !self.hours.is_empty() {
            return Err(format!("{} has no hours", show.name));
        }
        if self.hours.len() > MAX_RANGES {
            return Err(format!("at most {MAX_RANGES} ranges of hours"));
        }
        for range in &self.hours {
            range.check()?;
        }
        let website = &self.website;
        if website.title.chars().count() > MAX_TITLE {
            return Err(format!("the title is {MAX_TITLE} characters at most"));
        }
        if !is_color(&website.color) {
            return Err("the color is #rrggbb".to_owned());
        }
        if website.closed_message.chars().count() > MAX_CLOSED_MESSAGE {
            return Err(format!(
                "the closed message is {MAX_CLOSED_MESSAGE} characters at most"
            ));
        }
        check_placeholders(&website.closed_message, &CLOSED_PLACEHOLDERS)?;
        if let Some(url) = &website.page_url {
            let web = url.starts_with("https://") || url.starts_with("http://");
            if !web || url.len() > MAX_URL || url.contains(char::is_whitespace) {
                return Err("the page's address is an http or https address".to_owned());
            }
        }
        let notifications = &self.notifications;
        if !(1..=MAX_LEAD_MINUTES).contains(&notifications.lead_minutes) {
            return Err(format!("lead_minutes is 1 to {MAX_LEAD_MINUTES}"));
        }
        if !LANGUAGES.contains(&notifications.language.as_str()) {
            return Err(format!("language is one of {}", LANGUAGES.join(", ")));
        }
        let placeholders: Vec<&str> = show.all_placeholders().collect();
        for (name, event) in &notifications.events {
            if !show.all_events().any(|(known, _, _)| known == name) {
                return Err(format!("{} sends no {name}", show.name));
            }
            if event.message.chars().count() > MAX_MESSAGE {
                return Err(format!("messages are {MAX_MESSAGE} characters at most"));
            }
            check_placeholders(&event.message, &placeholders)
                .map_err(|error| format!("{name}: {error}"))?;
        }
        Ok(())
    }

    /// Its hours, as a week; empty while it is not enabled.
    pub fn open_week(&self) -> Week {
        if self.enabled {
            Week::of(&self.hours)
        } else {
            Week::default()
        }
    }
}

impl HourRange {
    fn check(&self) -> Result<(), String> {
        if self.days.is_empty() || self.days.iter().any(|day| !(1..=7).contains(day)) {
            return Err("days are 1 (Monday) to 7 (Sunday), at least one".to_owned());
        }
        if self.start_hour > 23 || self.end_hour <= self.start_hour || self.end_hour > 24 {
            return Err("hours are whole hours, the end after the start, up to 24".to_owned());
        }
        Ok(())
    }
}

fn is_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Splits a message into text and `{placeholder}` names. A `{` without a
/// closing `}`, or with spaces, is text.
fn parts(message: &str) -> Vec<Result<&str, &str>> {
    let mut parts = Vec::new();
    let mut rest = message;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end)
                if end > 0
                    && after[..end]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') =>
            {
                parts.push(Ok(&rest[..start]));
                parts.push(Err(&after[..end]));
                rest = &after[end + 1..];
            }
            _ => {
                parts.push(Ok(&rest[..=start]));
                rest = after;
            }
        }
    }
    parts.push(Ok(rest));
    parts
}

/// Refuses placeholders that are not in `known`.
pub fn check_placeholders(message: &str, known: &[&str]) -> Result<(), String> {
    for part in parts(message) {
        if let Err(name) = part
            && !known.contains(&name)
        {
            return Err(format!(
                "unknown placeholder {{{name}}} (known: {})",
                known
                    .iter()
                    .map(|name| format!("{{{name}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(())
}

/// Fills a message's placeholders; unknown ones stay as they are.
pub fn render(message: &str, values: &[(&str, String)]) -> String {
    let mut text = String::with_capacity(message.len());
    for part in parts(message) {
        match part {
            Ok(plain) => text.push_str(plain),
            Err(name) => match values.iter().find(|(key, _)| *key == name) {
                Some((_, value)) => text.push_str(value),
                None => {
                    text.push('{');
                    text.push_str(name);
                    text.push('}');
                }
            },
        }
    }
    text
}

/// The open hours of a week: bit `h` of day `d` (0 is Monday) is the hour
/// from `h`:00 to `h + 1`:00.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Week(pub [u32; 7]);

impl Week {
    pub fn of(ranges: &[HourRange]) -> Self {
        let mut week = Self::default();
        for range in ranges {
            let end = range.end_hour.min(24);
            for day in &range.days {
                let Some(hours) = week.0.get_mut(usize::from(day.saturating_sub(1))) else {
                    continue;
                };
                for hour in range.start_hour..end {
                    *hours |= 1 << hour;
                }
            }
        }
        week
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|hours| *hours == 0)
    }

    /// Whether the hour of `day` (1 is Monday) starting at `hour` is open.
    pub fn is_open(&self, day: u8, hour: u8) -> bool {
        self.0
            .get(usize::from(day.saturating_sub(1)))
            .is_some_and(|hours| hour < 24 && hours & (1 << hour) != 0)
    }

    /// The hours both weeks have, as ranges per day: `[(6, 18, 19)]` is
    /// Saturday 18:00 to 19:00.
    pub fn overlap(&self, other: &Self) -> Vec<(u8, u8, u8)> {
        let mut ranges = Vec::new();
        for (index, (a, b)) in self.0.iter().zip(other.0.iter()).enumerate() {
            let both = a & b;
            let mut hour = 0u8;
            while hour < 24 {
                if both & (1 << hour) != 0 {
                    let start = hour;
                    while hour < 24 && both & (1 << hour) != 0 {
                        hour += 1;
                    }
                    ranges.push((index as u8 + 1, start, hour));
                } else {
                    hour += 1;
                }
            }
        }
        ranges
    }

    fn is_open_at(&self, time: &Zoned) -> bool {
        let day = time.weekday().to_monday_one_offset() as u8;
        self.is_open(day, time.hour() as u8)
    }
}

/// The English name of a day, 1 (Monday) to 7.
pub fn day_name(day: u8) -> &'static str {
    [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ]
    .get(usize::from(day.saturating_sub(1)))
    .copied()
    .unwrap_or("?")
}

/// Whether a show is open now, and its next change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ShowStatus {
    pub open: bool,
    /// While open: when it closes.
    pub closes_at_unix_ms: Option<i64>,
    /// When it opens next (after it closes, while open); none within a week.
    pub next_opening_unix_ms: Option<i64>,
}

const HOUR: SignedDuration = SignedDuration::from_hours(1);
/// How far ahead a next opening is looked for.
const LOOKAHEAD_HOURS: i64 = 24 * 8;

/// The start of the hour `time` is in, in its time zone.
fn hour_start(time: &Zoned) -> Zoned {
    let rounded = time.with().minute(0).second(0).subsec_nanosecond(0).build();
    rounded.unwrap_or_else(|_| time.clone())
}

fn at(start: &Zoned, hours: i64) -> Option<Zoned> {
    start
        .timestamp()
        .checked_add(SignedDuration::from_hours(hours))
        .ok()
        .map(|time| time.to_zoned(start.time_zone().clone()))
}

/// Whether `week` is open at `now`, in `tz`, and its next change.
pub fn status(week: &Week, tz: &TimeZone, now: Timestamp) -> ShowStatus {
    if week.is_empty() {
        return ShowStatus::default();
    }
    let start = hour_start(&now.to_zoned(tz.clone()));
    let open = week.is_open_at(&start);
    let mut closes_at = None;
    let mut next_opening = None;
    let mut was_open = open;
    for hours in 1..=LOOKAHEAD_HOURS {
        let Some(time) = at(&start, hours) else {
            break;
        };
        let is_open = week.is_open_at(&time);
        if was_open && !is_open && closes_at.is_none() {
            closes_at = Some(time.timestamp().as_millisecond());
        }
        if !was_open && is_open {
            next_opening = Some(time.timestamp().as_millisecond());
            break;
        }
        was_open = is_open;
    }
    ShowStatus {
        open,
        closes_at_unix_ms: if open { closes_at } else { None },
        next_opening_unix_ms: next_opening,
    }
}

/// The opening `at` is in: when it started and when it closes. `None`
/// while closed.
pub fn opening(week: &Week, tz: &TimeZone, at: Timestamp) -> Option<(Timestamp, Timestamp)> {
    let state = status(week, tz, at);
    if !state.open {
        return None;
    }
    let closes = Timestamp::from_millisecond(state.closes_at_unix_ms?).ok()?;
    let mut start = hour_start(&at.to_zoned(tz.clone()));
    for _ in 0..LOOKAHEAD_HOURS {
        match self::at(&start, -1) {
            Some(before) if week.is_open_at(&before) => start = before,
            _ => break,
        }
    }
    Some((start.timestamp(), closes))
}

/// When the latest opening that started at or before `at` started, within
/// the last 8 days.
pub fn last_opening(week: &Week, tz: &TimeZone, at: Timestamp) -> Option<Timestamp> {
    if week.is_empty() {
        return None;
    }
    let mut time = hour_start(&at.to_zoned(tz.clone()));
    for _ in 0..LOOKAHEAD_HOURS {
        let before = self::at(&time, -1)?;
        if week.is_open_at(&time) && !week.is_open_at(&before) {
            return Some(time.timestamp());
        }
        time = before;
    }
    None
}

/// The next opening after `at` (or the one `at` is in, when it starts at
/// `at`): when it starts and when it closes.
pub fn next_opening(week: &Week, tz: &TimeZone, at: Timestamp) -> Option<(Timestamp, Timestamp)> {
    if let Some((start, closes)) = opening(week, tz, at)
        && start == at
    {
        return Some((start, closes));
    }
    let next = status(week, tz, at).next_opening_unix_ms?;
    let start = Timestamp::from_millisecond(next).ok()?;
    opening(week, tz, start)
}

/// A change a notification tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The show opens at `opens`, a set time later.
    Upcoming {
        opens: Timestamp,
    },
    Started,
    Ended,
}

/// The changes of `week` due in `(from, to]`, in order: `show.upcoming`
/// `lead` before an opening, `show.started` and `show.ended` on the hour.
pub fn changes(
    week: &Week,
    tz: &TimeZone,
    from: Timestamp,
    to: Timestamp,
    lead: SignedDuration,
) -> Vec<(Change, Timestamp)> {
    if week.is_empty() || to <= from {
        return Vec::new();
    }
    let start = hour_start(&from.to_zoned(tz.clone()));
    let mut found = Vec::new();
    let last = to.checked_add(lead).unwrap_or(to);
    let mut previous = at(&start, -1).is_some_and(|time| week.is_open_at(&time));
    for hours in 0.. {
        let Some(time) = at(&start, hours) else {
            break;
        };
        let boundary = time.timestamp();
        if boundary > last.checked_add(HOUR).unwrap_or(last) {
            break;
        }
        let open = week.is_open_at(&time);
        let mut push = |change: Change, at: Timestamp| {
            if from < at && at <= to {
                found.push((change, at));
            }
        };
        if open && !previous {
            if let Ok(warned) = boundary.checked_sub(lead) {
                push(Change::Upcoming { opens: boundary }, warned);
            }
            push(Change::Started, boundary);
        }
        if !open && previous {
            push(Change::Ended, boundary);
        }
        previous = open;
    }
    found.sort_by_key(|(_, at)| *at);
    found
}

/// `17:00`.
pub fn hour_text(time: Timestamp, tz: &TimeZone) -> String {
    let time = time.to_zoned(tz.clone());
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// The next opening in a message: `Monday 9 November 17:00`, in one of
/// [`LANGUAGES`] (English for any other).
pub fn date_text(time: Timestamp, tz: &TimeZone, language: &str) -> String {
    let zoned = time.to_zoned(tz.clone());
    let day = zoned.weekday().to_monday_zero_offset() as usize;
    let month = zoned.month() as usize - 1;
    let (days, months): (&[&str; 7], &[&str; 12]) = match language {
        "fr" => (
            &[
                "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
            ],
            &[
                "janvier",
                "février",
                "mars",
                "avril",
                "mai",
                "juin",
                "juillet",
                "août",
                "septembre",
                "octobre",
                "novembre",
                "décembre",
            ],
        ),
        "es" => (
            &[
                "lunes",
                "martes",
                "miércoles",
                "jueves",
                "viernes",
                "sábado",
                "domingo",
            ],
            &[
                "enero",
                "febrero",
                "marzo",
                "abril",
                "mayo",
                "junio",
                "julio",
                "agosto",
                "septiembre",
                "octubre",
                "noviembre",
                "diciembre",
            ],
        ),
        "de" => (
            &[
                "Montag",
                "Dienstag",
                "Mittwoch",
                "Donnerstag",
                "Freitag",
                "Samstag",
                "Sonntag",
            ],
            &[
                "Januar",
                "Februar",
                "März",
                "April",
                "Mai",
                "Juni",
                "Juli",
                "August",
                "September",
                "Oktober",
                "November",
                "Dezember",
            ],
        ),
        _ => (
            &[
                "Monday",
                "Tuesday",
                "Wednesday",
                "Thursday",
                "Friday",
                "Saturday",
                "Sunday",
            ],
            &[
                "January",
                "February",
                "March",
                "April",
                "May",
                "June",
                "July",
                "August",
                "September",
                "October",
                "November",
                "December",
            ],
        ),
    };
    let (day_name, month_name) = (days[day], months[month]);
    let date = zoned.day();
    let hour = hour_text(time, tz);
    match language {
        "es" => format!("{day_name} {date} de {month_name} {hour}"),
        "de" => format!("{day_name}, {date}. {month_name} {hour}"),
        _ => format!("{day_name} {date} {month_name} {hour}"),
    }
}

/// What the gateway and the widgets know of a show on a station.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ShowPublic {
    pub module: Module,
    /// The title on the website; `null`: the show's name in the listener's
    /// language.
    pub title: Option<String>,
    pub color: String,
    pub small_widget: bool,
    pub closed: Closed,
    /// `null`: the widget's own sentence.
    pub closed_message: Option<String>,
    pub page_url: Option<String>,
    /// Off: closed whatever its hours.
    pub enabled: bool,
    pub hours: Vec<HourRange>,
    /// The station's time zone (IANA).
    pub timezone: String,
    /// The show's own live data (the current match, the day's dedications);
    /// `null` until the show adds it.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub state: serde_json::Value,
}

impl ShowPublic {
    pub fn new(module: Module, settings: &ShowSettings, timezone: &str) -> Self {
        let website = &settings.website;
        let text = |value: &str| Some(value.trim().to_owned()).filter(|value| !value.is_empty());
        Self {
            module,
            title: text(&website.title),
            color: website.color.clone(),
            small_widget: website.small_widget,
            closed: website.closed,
            closed_message: text(&website.closed_message),
            page_url: website.page_url.clone(),
            enabled: settings.enabled,
            hours: settings.hours.clone(),
            timezone: timezone.to_owned(),
            state: serde_json::Value::Null,
        }
    }

    /// Whether it is open at `now`.
    pub fn status(&self, now: Timestamp) -> ShowStatus {
        if !self.enabled {
            return ShowStatus::default();
        }
        // A show without hours (Likes) is always open.
        if Show::of(self.module).is_some_and(|show| !show.has_hours) {
            return ShowStatus {
                open: true,
                ..ShowStatus::default()
            };
        }
        let tz = TimeZone::get(&self.timezone).unwrap_or(TimeZone::UTC);
        status(&Week::of(&self.hours), &tz, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openings_start_and_close_on_their_hours() {
        let week = Week::of(&[HourRange {
            days: vec![7],
            start_hour: 18,
            end_hour: 20,
        }]);
        let tz = TimeZone::get("Europe/Paris").unwrap();
        let at = |text: &str| {
            format!("{text}[Europe/Paris]")
                .parse::<Zoned>()
                .unwrap()
                .timestamp()
        };
        // Sunday 8 November 2026, 19:12.
        assert_eq!(
            opening(&week, &tz, at("2026-11-08T19:12")),
            Some((at("2026-11-08T18:00"), at("2026-11-08T20:00")))
        );
        assert_eq!(opening(&week, &tz, at("2026-11-08T20:00")), None);
        assert_eq!(
            last_opening(&week, &tz, at("2026-11-10T09:00")),
            Some(at("2026-11-08T18:00"))
        );
        assert_eq!(
            last_opening(&week, &tz, at("2026-11-08T18:00")),
            Some(at("2026-11-08T18:00"))
        );
        assert_eq!(
            last_opening(&week, &tz, at("2026-11-08T17:59")),
            Some(at("2026-11-01T18:00"))
        );
        assert_eq!(
            next_opening(&week, &tz, at("2026-11-08T17:00")),
            Some((at("2026-11-08T18:00"), at("2026-11-08T20:00")))
        );
        assert_eq!(
            next_opening(&week, &tz, at("2026-11-08T18:00")),
            Some((at("2026-11-08T18:00"), at("2026-11-08T20:00")))
        );
        assert_eq!(
            next_opening(&week, &tz, at("2026-11-08T18:30")),
            Some((at("2026-11-15T18:00"), at("2026-11-15T20:00")))
        );
    }

    fn range(days: &[u8], start_hour: u8, end_hour: u8) -> HourRange {
        HourRange {
            days: days.to_vec(),
            start_hour,
            end_hour,
        }
    }

    fn paris() -> TimeZone {
        TimeZone::get("Europe/Paris").unwrap()
    }

    fn time(text: &str) -> Timestamp {
        // Local time in Paris.
        format!("{text}[Europe/Paris]")
            .parse::<Zoned>()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn weeks_are_whole_hours_per_day() {
        let week = Week::of(&[range(&[1, 2, 3, 4, 5], 17, 18), range(&[7], 22, 24)]);
        assert!(week.is_open(1, 17) && week.is_open(5, 17));
        assert!(!week.is_open(1, 18) && !week.is_open(6, 17));
        assert!(week.is_open(7, 23) && !week.is_open(7, 21));
    }

    #[test]
    fn overlaps_are_given_as_ranges() {
        let dedications = Week::of(&[range(&[1, 2, 3, 4, 5, 6], 17, 19)]);
        let other = Week::of(&[range(&[6], 18, 20), range(&[2], 10, 12)]);
        assert_eq!(dedications.overlap(&other), vec![(6, 18, 19)]);
        let apart = Week::of(&[range(&[1], 19, 20)]);
        assert!(dedications.overlap(&apart).is_empty());
    }

    #[test]
    fn status_says_when_it_closes_and_opens_next() {
        // 9 November 2026 is a Monday.
        let week = Week::of(&[range(&[1, 2, 3, 4, 5], 17, 19)]);
        let open = status(&week, &paris(), time("2026-11-09T17:30"));
        assert!(open.open);
        assert_eq!(
            open.closes_at_unix_ms,
            Some(time("2026-11-09T19:00").as_millisecond())
        );
        assert_eq!(
            open.next_opening_unix_ms,
            Some(time("2026-11-10T17:00").as_millisecond())
        );
        let friday = status(&week, &paris(), time("2026-11-13T19:00"));
        assert!(!friday.open);
        assert_eq!(friday.closes_at_unix_ms, None);
        assert_eq!(
            friday.next_opening_unix_ms,
            Some(time("2026-11-16T17:00").as_millisecond())
        );
        assert_eq!(
            status(&Week::default(), &paris(), time("2026-11-09T17:30")),
            ShowStatus::default()
        );
    }

    #[test]
    fn changes_are_found_once_in_their_window() {
        let week = Week::of(&[range(&[1], 17, 19)]);
        let lead = SignedDuration::from_mins(60);
        let all = changes(
            &week,
            &paris(),
            time("2026-11-09T15:00"),
            time("2026-11-09T20:00"),
            lead,
        );
        assert_eq!(
            all,
            vec![
                (
                    Change::Upcoming {
                        opens: time("2026-11-09T17:00")
                    },
                    time("2026-11-09T16:00")
                ),
                (Change::Started, time("2026-11-09T17:00")),
                (Change::Ended, time("2026-11-09T19:00")),
            ]
        );
        // Ticks of 15 s find each change in exactly one window.
        let mut found = Vec::new();
        let mut from = time("2026-11-09T15:59:50");
        while from < time("2026-11-09T19:01") {
            let to = from.checked_add(SignedDuration::from_secs(15)).unwrap();
            found.extend(changes(&week, &paris(), from, to, lead));
            from = to;
        }
        assert_eq!(found, all);
        // An upcoming change due before the window opens is not sent late.
        let late = changes(
            &week,
            &paris(),
            time("2026-11-09T16:30"),
            time("2026-11-09T16:45"),
            lead,
        );
        assert!(late.is_empty());
    }

    #[test]
    fn hours_up_to_midnight_end_on_the_next_day() {
        let week = Week::of(&[range(&[7], 22, 24)]);
        let sunday = status(&week, &paris(), time("2026-11-15T23:10"));
        assert_eq!(
            sunday.closes_at_unix_ms,
            Some(time("2026-11-16T00:00").as_millisecond())
        );
    }

    #[test]
    fn settings_are_checked() {
        let show = Show::of(Module::Dedications).unwrap();
        let mut settings = ShowSettings::default();
        settings.complete(show);
        assert_eq!(settings.notifications.events.len(), 5);
        assert!(!settings.notifications.events[DEDICATION_ACCEPTED].enabled);
        assert_eq!(settings.check(show), Ok(()));
        let bad = |change: fn(&mut ShowSettings)| {
            let mut settings = settings.clone();
            change(&mut settings);
            settings.check(show).unwrap_err()
        };
        bad(|s| s.hours = vec![range(&[8], 1, 2)]);
        bad(|s| s.hours = vec![range(&[1], 18, 18)]);
        bad(|s| s.hours = vec![range(&[1], 23, 25)]);
        bad(|s| s.website.color = "red".to_owned());
        bad(|s| s.website.closed_message = "back {when}".to_owned());
        bad(|s| s.website.page_url = Some("javascript:alert(1)".to_owned()));
        bad(|s| s.notifications.lead_minutes = 0);
        bad(|s| s.notifications.language = "it".to_owned());
        assert!(
            bad(|s| s
                .notifications
                .events
                .get_mut(SHOW_STARTED)
                .unwrap()
                .message = "{rank}".to_owned())
            .contains("unknown placeholder {rank}")
        );
        bad(|s| {
            s.notifications.events.insert(
                "match.started".to_owned(),
                EventSettings {
                    enabled: true,
                    message: String::new(),
                },
            );
        });
        let mut ok = settings.clone();
        ok.hours = vec![range(&[1, 2], 0, 24)];
        ok.website.closed_message = "Back {day} at {hour}, {unknown".to_owned();
        assert_eq!(ok.check(show), Ok(()));
    }

    #[test]
    fn likes_have_no_hours_and_no_events() {
        let likes = Show::of(Module::Likes).unwrap();
        let mut settings = ShowSettings::default();
        settings.complete(likes);
        assert!(settings.notifications.events.is_empty());
        assert_eq!(settings.check(likes), Ok(()));
        settings.hours = vec![range(&[1], 17, 18)];
        assert!(settings.check(likes).unwrap_err().contains("no hours"));
        let public = ShowPublic::new(Module::Likes, &ShowSettings::default(), "Europe/Paris");
        assert!(public.status(Timestamp::now()).open);
        let off = ShowSettings {
            enabled: false,
            ..ShowSettings::default()
        };
        assert!(
            !ShowPublic::new(Module::Likes, &off, "UTC")
                .status(Timestamp::now())
                .open
        );
    }

    #[test]
    fn messages_fill_their_placeholders() {
        let values = [
            ("name", "Dedications".to_owned()),
            ("end", "18:00".to_owned()),
        ];
        assert_eq!(
            render("{name} until {end}, {other} {not one} {", &values),
            "Dedications until 18:00, {other} {not one} {"
        );
    }

    #[test]
    fn dates_are_written_in_the_language() {
        let at = time("2026-11-09T17:00");
        assert_eq!(date_text(at, &paris(), "en"), "Monday 9 November 17:00");
        assert_eq!(date_text(at, &paris(), "fr"), "lundi 9 novembre 17:00");
        assert_eq!(date_text(at, &paris(), "es"), "lunes 9 de noviembre 17:00");
        assert_eq!(date_text(at, &paris(), "de"), "Montag, 9. November 17:00");
        assert_eq!(hour_text(at, &paris()), "17:00");
    }
}
