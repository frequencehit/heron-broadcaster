// SPDX-License-Identifier: AGPL-3.0-or-later

//! The audience figures of a period: TTSL, ATSL and the median session,
//! cume, AQH over clock quarter-hours, peak, sessions by length, and the
//! tune-in and tune-out of each hour of the day. From the sessions and
//! minute samples kept. See `docs/statistics.md#audience-figures`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::Serialize;
use utoipa::ToSchema;

use super::sessions::ListenerKey;
use super::totals;

/// Sessions shorter than this, in seconds, are left out: players and
/// directories probing the stream.
pub const PROBE_S: u32 = 5;
/// A listener counts in a quarter-hour heard this long in it.
const QUARTER_HOUR_LISTEN_MS: i64 = 5 * 60_000;
pub const QUARTER_HOUR_MS: i64 = 15 * 60_000;
/// Upper bounds of the session lengths, in seconds: under 1 min, 1–5 min,
/// 5–30 min, 30 min–2 h, the rest over 2 h.
const LENGTHS_S: [u32; 4] = [60, 300, 1_800, 7_200];

/// A session of at least [`PROBE_S`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listen {
    /// Its day, station time.
    pub day: Date,
    pub key: ListenerKey,
    pub started_ms: i64,
    pub last_seen_ms: i64,
    pub duration_s: u32,
    /// Ended (not at midnight): the listener left.
    pub left: bool,
}

/// What was measured in the period.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    /// Days with a minute sample.
    pub days: BTreeSet<Date>,
    /// Quarter-hours (Unix ms / [`QUARTER_HOUR_MS`]) with a listener list
    /// read.
    pub quarters: BTreeSet<i64>,
}

/// Sessions by length.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, ToSchema)]
pub struct SessionLengths {
    pub under_1_min: u32,
    pub from_1_to_5_min: u32,
    pub from_5_to_30_min: u32,
    pub from_30_min_to_2_h: u32,
    pub over_2_h: u32,
}

/// The audience figures of a period. Those from sessions are missing
/// without listener lists.
#[derive(Clone, Debug, Default, PartialEq, Serialize, ToSchema)]
pub struct Audience {
    /// First day (`YYYY-MM-DD`, station time).
    pub from: String,
    /// Last day, included.
    pub to: String,
    /// Days with a minute measured.
    pub days: u32,
    /// The most listeners at once.
    pub peak: Option<u32>,
    /// The (first) minute of the peak.
    pub peak_at_unix_ms: Option<i64>,
    /// Sessions of at least 5 s.
    pub sessions: Option<u32>,
    /// Total time spent listening: the sessions added up, in hours.
    pub ttsl_hours: Option<f64>,
    /// Average time spent listening: TTSL over the sessions, in seconds.
    pub atsl_s: Option<f64>,
    pub median_session_s: Option<f64>,
    /// Distinct listeners per day, on average over the days measured.
    pub cume: Option<f64>,
    /// Distinct listeners of each day, added up.
    pub listener_days: Option<u32>,
    /// Average quarter-hour audience: listeners heard 5 minutes or more in
    /// a clock quarter-hour, on average over the quarter-hours measured.
    pub aqh: Option<f64>,
    pub session_lengths: Option<SessionLengths>,
}

/// The figures of a period and of the period of the same length before.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct AudienceComparison {
    pub current: Audience,
    pub previous: Audience,
}

fn lengths(durations: &[u32]) -> SessionLengths {
    let mut by = SessionLengths::default();
    for &d in durations {
        let bucket = if d < LENGTHS_S[0] {
            &mut by.under_1_min
        } else if d < LENGTHS_S[1] {
            &mut by.from_1_to_5_min
        } else if d < LENGTHS_S[2] {
            &mut by.from_5_to_30_min
        } else if d < LENGTHS_S[3] {
            &mut by.from_30_min_to_2_h
        } else {
            &mut by.over_2_h
        };
        *bucket += 1;
    }
    by
}

fn median(sorted: &[u32]) -> Option<f64> {
    let middle = sorted.len() / 2;
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => sorted.get(middle).map(|&d| f64::from(d)),
        _ => Some((f64::from(sorted[middle - 1]) + f64::from(sorted[middle])) / 2.0),
    }
}

/// The listeners of each quarter-hour measured, on average.
fn aqh(listens: &[Listen], quarters: &BTreeSet<i64>) -> f64 {
    if quarters.is_empty() {
        return 0.0;
    }
    // Per quarter-hour and listener, how long they were heard in it.
    let mut heard: HashMap<(i64, &ListenerKey), i64> = HashMap::new();
    for listen in listens {
        let first = listen.started_ms.div_euclid(QUARTER_HOUR_MS);
        let last = listen.last_seen_ms.div_euclid(QUARTER_HOUR_MS);
        for quarter in first..=last {
            if !quarters.contains(&quarter) {
                continue;
            }
            let start = (quarter * QUARTER_HOUR_MS).max(listen.started_ms);
            let end = ((quarter + 1) * QUARTER_HOUR_MS).min(listen.last_seen_ms);
            *heard.entry((quarter, &listen.key)).or_default() += (end - start).max(0);
        }
    }
    let counted = heard
        .values()
        .filter(|&&ms| ms >= QUARTER_HOUR_LISTEN_MS)
        .count();
    counted as f64 / quarters.len() as f64
}

/// The figures of a period. `listens`: its sessions of at least
/// [`PROBE_S`]; `peak`: its highest minute and when.
pub fn audience(
    (from, to): (Date, Date),
    listens: &[Listen],
    measured: &Measured,
    peak: Option<(u32, i64)>,
) -> Audience {
    let mut figures = Audience {
        from: from.to_string(),
        to: to.to_string(),
        days: u32::try_from(measured.days.len()).unwrap_or(u32::MAX),
        peak: peak.map(|(listeners, _)| listeners),
        peak_at_unix_ms: peak.map(|(_, at)| at),
        ..Audience::default()
    };
    if measured.quarters.is_empty() {
        return figures;
    }
    let mut durations: Vec<u32> = listens.iter().map(|l| l.duration_s).collect();
    durations.sort_unstable();
    let total_s: u64 = durations.iter().map(|&d| u64::from(d)).sum();
    let sessions = u32::try_from(durations.len()).unwrap_or(u32::MAX);
    let mut per_day: BTreeMap<Date, HashSet<&ListenerKey>> = BTreeMap::new();
    for listen in listens {
        per_day.entry(listen.day).or_default().insert(&listen.key);
    }
    let listener_days: usize = per_day.values().map(HashSet::len).sum();
    let listener_days = u32::try_from(listener_days).unwrap_or(u32::MAX);
    figures.sessions = Some(sessions);
    figures.ttsl_hours = Some(total_s as f64 / 3_600.0);
    figures.atsl_s = (sessions > 0).then(|| total_s as f64 / f64::from(sessions));
    figures.median_session_s = median(&durations);
    figures.listener_days = Some(listener_days);
    figures.aqh = Some(aqh(listens, &measured.quarters));
    figures.session_lengths = Some(lengths(&durations));
    figures
}

/// The cume: distinct listeners per day, on average over the days a
/// listener list was read on (station time).
pub fn with_cume(mut figures: Audience, measured: &Measured, tz: &TimeZone) -> Audience {
    let days: BTreeSet<Date> = measured
        .quarters
        .iter()
        .map(|quarter| totals::day_of(quarter * QUARTER_HOUR_MS, tz))
        .collect();
    figures.cume = figures
        .listener_days
        .filter(|_| !days.is_empty())
        .map(|sum| f64::from(sum) / days.len() as f64);
    figures
}

/// Tune-in and tune-out of each hour of the day (station time): sessions
/// started and ended in it. A session continued after midnight did not
/// start there, one split at midnight did not end there.
pub fn tune_in_out(listens: &[Listen], tz: &TimeZone) -> BTreeMap<u8, (u32, u32)> {
    let mut hours: BTreeMap<u8, (u32, u32)> = BTreeMap::new();
    for listen in listens {
        if listen.started_ms != totals::day_bounds(listen.day, tz).0 {
            hours
                .entry(totals::hour_of(listen.started_ms, tz))
                .or_default()
                .0 += 1;
        }
        if listen.left {
            hours
                .entry(totals::hour_of(listen.last_seen_ms, tz))
                .or_default()
                .1 += 1;
        }
    }
    hours
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    const MIN: i64 = 60_000;

    fn listen(n: u8, from_min: i64, to_min: i64) -> Listen {
        Listen {
            day: date(2026, 10, 20),
            key: [n; 32],
            started_ms: DAY + from_min * MIN,
            last_seen_ms: DAY + to_min * MIN,
            duration_s: u32::try_from((to_min - from_min) * 60).unwrap(),
            left: true,
        }
    }

    /// 2026-10-20 00:00 UTC.
    const DAY: i64 = 1_792_454_400_000;

    fn measured(from_min: i64, to_min: i64) -> Measured {
        Measured {
            days: BTreeSet::from([date(2026, 10, 20)]),
            quarters: (DAY / QUARTER_HOUR_MS + from_min / 15..DAY / QUARTER_HOUR_MS + to_min / 15)
                .collect(),
        }
    }

    #[test]
    fn known_sessions_give_known_figures() {
        assert_eq!(totals::day_of(DAY, &TimeZone::UTC), date(2026, 10, 20));
        // 10:00 to 12:00 measured (8 quarter-hours).
        let listens = [
            // A: 10:00 to 12:30, 150 min, over 2 h.
            listen(1, 600, 750),
            // B: 10:10 to 10:40, 30 min: 5 min in 10:00, all of 10:15,
            // 10 min in 10:30.
            listen(2, 610, 640),
            // B again, 11:20 to 11:23: 3 min, not enough in 11:15.
            listen(2, 680, 683),
            // C: 10:50 to 10:50:40, under a minute.
            Listen {
                duration_s: 40,
                last_seen_ms: DAY + 650 * MIN + 40_000,
                ..listen(3, 650, 650)
            },
            // D: 11:00 to 11:20, 20 min.
            listen(4, 660, 680),
        ];
        let figures = audience(
            (date(2026, 10, 20), date(2026, 10, 20)),
            &listens,
            &measured(600, 720),
            Some((3, DAY + 615 * MIN)),
        );
        let figures = with_cume(figures, &measured(600, 720), &TimeZone::UTC);
        assert_eq!(figures.sessions, Some(5));
        let total_s = (150 + 30 + 3 + 20) * 60 + 40;
        assert!((figures.ttsl_hours.unwrap() - f64::from(total_s) / 3_600.0).abs() < 1e-9);
        assert!((figures.atsl_s.unwrap() - f64::from(total_s) / 5.0).abs() < 1e-9);
        // 40 s, 3, 20, 30, 150 min.
        assert_eq!(figures.median_session_s, Some(1_200.0));
        assert_eq!(figures.listener_days, Some(4));
        assert_eq!(figures.cume, Some(4.0));
        assert_eq!(
            figures.session_lengths,
            Some(SessionLengths {
                under_1_min: 1,
                from_1_to_5_min: 1,
                from_5_to_30_min: 1,
                from_30_min_to_2_h: 1,
                over_2_h: 1,
            })
        );
        // A in all 8; B in 10:00, 10:15 and 10:30; D in 11:00 and 11:15
        // (5 min); C and B's 3 minutes in none.
        assert!(
            (figures.aqh.unwrap() - 13.0 / 8.0).abs() < 1e-9,
            "{figures:?}"
        );
        assert_eq!(
            (figures.peak, figures.peak_at_unix_ms),
            (Some(3), Some(DAY + 615 * MIN))
        );
        assert_eq!(figures.days, 1);
    }

    #[test]
    fn without_listener_lists_only_the_peak_is_known() {
        let measured = Measured {
            days: BTreeSet::from([date(2026, 10, 20)]),
            quarters: BTreeSet::new(),
        };
        let figures = audience(
            (date(2026, 10, 20), date(2026, 10, 20)),
            &[],
            &measured,
            Some((7, DAY)),
        );
        let figures = with_cume(figures, &measured, &TimeZone::UTC);
        assert_eq!(figures.peak, Some(7));
        assert_eq!(
            (
                figures.sessions,
                figures.ttsl_hours,
                figures.cume,
                figures.aqh
            ),
            (None, None, None, None)
        );
        // Measured, nobody listening: zeros, no average.
        let figures = audience(
            (date(2026, 10, 20), date(2026, 10, 20)),
            &[],
            &measured_quarters(),
            Some((0, DAY)),
        );
        assert_eq!((figures.sessions, figures.atsl_s), (Some(0), None));
        assert_eq!(figures.aqh, Some(0.0));
    }

    fn measured_quarters() -> Measured {
        measured(0, 60)
    }

    #[test]
    fn the_cume_averages_the_days() {
        let day2 = |n: u8| Listen {
            day: date(2026, 10, 21),
            started_ms: DAY + 86_400_000 + 600 * MIN,
            last_seen_ms: DAY + 86_400_000 + 610 * MIN,
            ..listen(n, 600, 610)
        };
        // A day's keys cannot be linked with the next day's.
        let listens = [
            listen(1, 600, 610),
            listen(2, 600, 610),
            day2(1),
            day2(2),
            day2(3),
            day2(4),
        ];
        let mut measured = measured(600, 615);
        measured
            .quarters
            .insert((DAY + 86_400_000 + 600 * MIN) / QUARTER_HOUR_MS);
        measured.days.insert(date(2026, 10, 21));
        let figures = with_cume(
            audience(
                (date(2026, 10, 20), date(2026, 10, 21)),
                &listens,
                &measured,
                None,
            ),
            &measured,
            &TimeZone::UTC,
        );
        assert_eq!(figures.listener_days, Some(6));
        assert_eq!(figures.cume, Some(3.0));
        assert_eq!(figures.days, 2);
    }

    #[test]
    fn tune_in_and_out_by_hour() {
        let tz = TimeZone::UTC;
        let mut still = listen(2, 610, 700);
        still.left = false;
        // Continued after midnight: no tune-in.
        let continued = Listen {
            started_ms: DAY,
            ..listen(3, 0, 30)
        };
        let hours = tune_in_out(&[listen(1, 600, 650), still, continued], &tz);
        assert_eq!(hours.get(&10), Some(&(2, 1)));
        assert_eq!(hours.get(&11), None);
        assert_eq!(hours.get(&0), Some(&(0, 1)));
    }
}
