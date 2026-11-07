// SPDX-License-Identifier: AGPL-3.0-or-later

//! A day's totals, per hour and for the whole day, per output and for the
//! whole station, for every server and for each one, from its minute
//! samples and sessions. See
//! `docs/statistics.md#what-is-measured`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use super::sessions::ListenerKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub output_id: u64,
    /// The server read: the master or a relay.
    pub server_id: u64,
    pub minute_ms: i64,
    pub listeners: u32,
    /// The listener list was read too.
    pub sessions: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub output_id: u64,
    pub server_id: u64,
    pub key: ListenerKey,
    pub started_ms: i64,
    pub last_seen_ms: i64,
    pub duration_s: u32,
}

/// A row of `listener_totals`.
#[derive(Clone, Debug, PartialEq)]
pub struct Total {
    /// `None`: the whole station.
    pub output_id: Option<u64>,
    /// `None`: every server.
    pub server_id: Option<u64>,
    /// `None`: the whole day.
    pub hour: Option<u8>,
    pub minutes: u32,
    pub peak: u32,
    pub average: f64,
    pub listening_hours: f64,
    /// `None` when no listener list was read in the period.
    pub sessions: Option<u32>,
    pub session_s: Option<u64>,
    pub distinct_listeners: Option<u32>,
}

pub(super) fn hour_of(ms: i64, tz: &TimeZone) -> u8 {
    Timestamp::from_millisecond(ms).map_or(0, |at| at.to_zoned(tz.clone()).hour().unsigned_abs())
}

/// The local hours a span of time covers.
fn hours_between(from_ms: i64, to_ms: i64, tz: &TimeZone) -> BTreeSet<u8> {
    let mut hours = BTreeSet::new();
    let mut at = from_ms;
    while at <= to_ms {
        hours.insert(hour_of(at, tz));
        // The next hour starts at a whole hour of UTC, or of the half and
        // quarter-hour time zones: stepping by 15 minutes finds it.
        at = (at.div_euclid(900_000) + 1) * 900_000;
    }
    hours
}

#[derive(Default)]
struct Minutes {
    minutes: u32,
    peak: u32,
    sum: u64,
    sessions: bool,
}

impl Minutes {
    fn add(&mut self, listeners: u32, sessions: bool) {
        self.minutes += 1;
        self.peak = self.peak.max(listeners);
        self.sum += u64::from(listeners);
        self.sessions |= sessions;
    }
}

#[derive(Default)]
struct Listening<'a> {
    sessions: u32,
    session_s: u64,
    keys: HashSet<&'a ListenerKey>,
}

/// Every row of the day: for every server (an output's minute adds up its
/// servers), then for each server on its own. `min_session_s`: sessions
/// shorter than this do not count.
pub fn day_totals(
    samples: &[Sample],
    spans: &[Span],
    tz: &TimeZone,
    min_session_s: u32,
) -> Vec<Total> {
    let mut outputs: BTreeMap<(u64, i64), (u32, bool)> = BTreeMap::new();
    for sample in samples {
        let minute = outputs
            .entry((sample.output_id, sample.minute_ms))
            .or_default();
        minute.0 += sample.listeners;
        minute.1 |= sample.sessions;
    }
    let every: Vec<Sample> = outputs
        .into_iter()
        .map(|((output_id, minute_ms), (listeners, sessions))| Sample {
            output_id,
            server_id: 0,
            minute_ms,
            listeners,
            sessions,
        })
        .collect();
    let mut rows = scope_totals(&every, spans, tz, min_session_s);
    let servers: BTreeSet<u64> = samples.iter().map(|sample| sample.server_id).collect();
    for server_id in servers {
        let samples: Vec<Sample> = samples
            .iter()
            .filter(|sample| sample.server_id == server_id)
            .cloned()
            .collect();
        let spans: Vec<Span> = spans
            .iter()
            .filter(|span| span.server_id == server_id)
            .cloned()
            .collect();
        rows.extend(
            scope_totals(&samples, &spans, tz, min_session_s)
                .into_iter()
                .map(|row| Total {
                    server_id: Some(server_id),
                    ..row
                }),
        );
    }
    rows
}

/// The rows of one set of samples and sessions, per output and for the
/// station, with `server_id` left to the caller.
fn scope_totals(
    samples: &[Sample],
    spans: &[Span],
    tz: &TimeZone,
    min_session_s: u32,
) -> Vec<Total> {
    // Per scope (an output, or the station: None), per hour (None: the day).
    let mut minutes: BTreeMap<(Option<u64>, Option<u8>), Minutes> = BTreeMap::new();
    let mut station: BTreeMap<i64, (u32, bool)> = BTreeMap::new();
    for sample in samples {
        let hour = hour_of(sample.minute_ms, tz);
        for period in [Some(hour), None] {
            minutes
                .entry((Some(sample.output_id), period))
                .or_default()
                .add(sample.listeners, sample.sessions);
        }
        let minute = station.entry(sample.minute_ms).or_default();
        minute.0 += sample.listeners;
        minute.1 |= sample.sessions;
    }
    for (&minute_ms, &(listeners, sessions)) in &station {
        let hour = hour_of(minute_ms, tz);
        for period in [Some(hour), None] {
            minutes
                .entry((None, period))
                .or_default()
                .add(listeners, sessions);
        }
    }

    let mut listening: BTreeMap<(Option<u64>, Option<u8>), Listening> = BTreeMap::new();
    for span in spans.iter().filter(|span| span.duration_s >= min_session_s) {
        let start_hour = hour_of(span.started_ms, tz);
        let hours = hours_between(span.started_ms, span.last_seen_ms, tz);
        for scope in [Some(span.output_id), None] {
            for period in [Some(start_hour), None] {
                let entry = listening.entry((scope, period)).or_default();
                entry.sessions += 1;
                entry.session_s += u64::from(span.duration_s);
            }
            listening
                .entry((scope, None))
                .or_default()
                .keys
                .insert(&span.key);
            for &hour in &hours {
                listening
                    .entry((scope, Some(hour)))
                    .or_default()
                    .keys
                    .insert(&span.key);
            }
        }
    }

    minutes
        .into_iter()
        .map(|((output_id, hour), measured)| {
            let listened = listening.get(&(output_id, hour));
            let (sessions, session_s, distinct_listeners) = if measured.sessions {
                listened.map_or((Some(0), Some(0), Some(0)), |listened| {
                    (
                        Some(listened.sessions),
                        Some(listened.session_s),
                        Some(u32::try_from(listened.keys.len()).unwrap_or(u32::MAX)),
                    )
                })
            } else {
                (None, None, None)
            };
            Total {
                output_id,
                server_id: None,
                hour,
                minutes: measured.minutes,
                peak: measured.peak,
                average: measured.sum as f64 / f64::from(measured.minutes),
                listening_hours: measured.sum as f64 / 60.0,
                sessions,
                session_s,
                distinct_listeners,
            }
        })
        .collect()
}

/// When a day starts and ends (the next one starts), in Unix ms.
pub fn day_bounds(day: Date, tz: &TimeZone) -> (i64, i64) {
    let start = |day: Date| {
        day.to_zoned(tz.clone())
            .map_or(0, |zoned| zoned.timestamp().as_millisecond())
    };
    let next = day.tomorrow().unwrap_or(day);
    (start(day), start(next))
}

/// The day (station time) of an instant.
pub fn day_of(ms: i64, tz: &TimeZone) -> Date {
    Timestamp::from_millisecond(ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(tz.clone())
        .date()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn sample(output_id: u64, minute: i64, listeners: u32) -> Sample {
        Sample {
            output_id,
            server_id: 1,
            minute_ms: minute * MIN,
            listeners,
            sessions: true,
        }
    }

    fn span(output_id: u64, n: u8, from_min: i64, to_min: i64) -> Span {
        Span {
            output_id,
            server_id: 1,
            key: [n; 32],
            started_ms: from_min * MIN,
            last_seen_ms: to_min * MIN,
            duration_s: u32::try_from((to_min - from_min) * 60).unwrap(),
        }
    }

    fn row(totals: &[Total], output_id: Option<u64>, hour: Option<u8>) -> &Total {
        totals
            .iter()
            .find(|t| t.output_id == output_id && t.server_id.is_none() && t.hour == hour)
            .unwrap()
    }

    #[test]
    fn adds_up_minutes_and_sessions() {
        let utc = TimeZone::UTC;
        // Output 1: 2 listeners for the first hour, 1 for 30 minutes after.
        // Output 2: 3 listeners for 10 minutes in the first hour.
        let mut samples: Vec<Sample> = (0..60).map(|m| sample(1, m, 2)).collect();
        samples.extend((60..90).map(|m| sample(1, m, 1)));
        samples.extend((0..10).map(|m| sample(2, m, 3)));
        let spans = [
            span(1, 1, 0, 90),
            span(1, 2, 0, 60),
            // A probe: too short to count.
            Span {
                duration_s: 5,
                ..span(1, 3, 20, 20)
            },
            // Listener 1 on output 2 too.
            span(2, 1, 0, 10),
            span(2, 4, 0, 10),
            span(2, 5, 0, 10),
        ];
        let totals = day_totals(&samples, &spans, &utc, 60);

        let day = row(&totals, Some(1), None);
        assert_eq!((day.minutes, day.peak), (90, 2));
        assert!((day.listening_hours - 2.5).abs() < 1e-9);
        assert!((day.average - 150.0 / 90.0).abs() < 1e-9);
        assert_eq!(day.sessions, Some(2));
        assert_eq!(day.session_s, Some(150 * 60));
        assert_eq!(day.distinct_listeners, Some(2));

        let second_hour = row(&totals, Some(1), Some(1));
        assert_eq!((second_hour.minutes, second_hour.peak), (30, 1));
        // Started in the first hour: listening then, not a new session.
        assert_eq!(second_hour.sessions, Some(0));
        assert_eq!(second_hour.distinct_listeners, Some(2));

        // The station's peak is the highest sum, its listeners counted once.
        let station = row(&totals, None, None);
        assert_eq!((station.minutes, station.peak), (90, 5));
        assert!((station.listening_hours - 3.0).abs() < 1e-9);
        assert_eq!(station.sessions, Some(5));
        assert_eq!(station.distinct_listeners, Some(4));
        assert!(totals.iter().all(|t| t.hour != Some(2)));
    }

    #[test]
    fn each_server_has_its_own_rows() {
        // Output 1: 2 listeners on the master (server 1), 3 on a relay
        // (server 2) for 10 minutes; listener 1 on both.
        let mut samples: Vec<Sample> = (0..10).map(|m| sample(1, m, 2)).collect();
        samples.extend((0..10).map(|m| Sample {
            server_id: 2,
            ..sample(1, m, 3)
        }));
        let relay = |n| Span {
            server_id: 2,
            ..span(1, n, 0, 10)
        };
        let spans = [
            span(1, 1, 0, 10),
            span(1, 2, 0, 10),
            relay(1),
            relay(3),
            relay(4),
        ];
        let totals = day_totals(&samples, &spans, &TimeZone::UTC, 60);
        let find = |output_id: Option<u64>, server_id: Option<u64>| {
            totals
                .iter()
                .find(|t| t.output_id == output_id && t.server_id == server_id && t.hour.is_none())
                .unwrap()
        };
        let every = find(Some(1), None);
        assert_eq!((every.peak, every.distinct_listeners), (5, Some(4)));
        let master = find(Some(1), Some(1));
        assert_eq!((master.peak, master.sessions), (2, Some(2)));
        let own = find(Some(1), Some(2));
        assert_eq!(
            (own.peak, own.sessions, own.distinct_listeners),
            (3, Some(3), Some(3))
        );
        assert!((own.listening_hours - 0.5).abs() < 1e-9);
        // The station on one server.
        assert_eq!(find(None, Some(2)).peak, 3);
        assert_eq!(totals.iter().filter(|t| t.server_id == Some(2)).count(), 4);
    }

    #[test]
    fn without_listener_lists_sessions_are_unknown() {
        let samples: Vec<Sample> = (0..30)
            .map(|m| Sample {
                sessions: false,
                ..sample(1, m, 4)
            })
            .collect();
        let totals = day_totals(&samples, &[], &TimeZone::UTC, 60);
        let day = row(&totals, Some(1), None);
        assert_eq!((day.minutes, day.peak), (30, 4));
        assert!((day.listening_hours - 2.0).abs() < 1e-9);
        assert_eq!(
            (day.sessions, day.session_s, day.distinct_listeners),
            (None, None, None)
        );
    }

    #[test]
    fn hours_follow_the_station_time_zone() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let day = jiff::civil::date(2026, 10, 25);
        // Clocks go back at 03:00: the day lasts 25 hours.
        let (start, end) = day_bounds(day, &paris);
        assert_eq!(end - start, 25 * 60 * MIN);
        assert_eq!(day_of(start, &paris), day);
        assert_eq!(day_of(end - 1, &paris), day);
        let samples: Vec<Sample> = (0..25 * 60)
            .map(|m| sample(1, start / MIN + m, 1))
            .collect();
        let totals = day_totals(&samples, &[], &paris, 60);
        // 02:00 to 03:00 happened twice.
        assert_eq!(row(&totals, Some(1), Some(2)).minutes, 120);
        assert_eq!(row(&totals, Some(1), Some(23)).minutes, 60);
        assert_eq!(row(&totals, Some(1), None).minutes, 1_500);
        assert_eq!(
            hours_between(start, start + 150 * MIN, &paris),
            BTreeSet::from([0, 1, 2])
        );
    }
}
