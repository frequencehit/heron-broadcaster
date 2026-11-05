// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dayparts and daypart separation (`docs/rotation.md#daypart-separation`).
//!
//! A station divides its day into 4 dayparts of 6 hours or 6 of 4 hours,
//! in its time zone, named A, B, C… A track played in a daypart does not
//! play in the N dayparts that follow it, across midnight too.

use jiff::civil::{Date, DateTime};
use serde::{Deserialize, Serialize};

/// The default division of the day.
pub const DEFAULT_COUNT: u8 = 4;

/// The divisions offered: 4 dayparts of 6 hours, or 6 of 4 hours.
pub const COUNTS: [u8; 2] = [4, 6];

/// The longest daypart separation of any division.
pub const MAX_SEPARATION: u8 = 10;

/// The most days the same daypart is kept out on after a play.
pub const MAX_DAYS: u8 = 7;

/// `count` as rotation uses it: 6, or 4 for anything else.
pub fn count(count: u8) -> u8 {
    if count == 6 { 6 } else { DEFAULT_COUNT }
}

/// The longest daypart separation with `count` dayparts: 6 with 4, 10
/// with 6.
pub fn max_separation(count: u8) -> u8 {
    if self::count(count) == 6 {
        MAX_SEPARATION
    } else {
        6
    }
}

/// Hours in a daypart.
pub fn hours(count: u8) -> u8 {
    24 / self::count(count)
}

/// The daypart of a station time, from 0 (A). A repeated hour, when the
/// clocks go back, stays in its daypart.
pub fn of(count: u8, at: DateTime) -> u8 {
    // An hour is 0 to 23.
    u8::try_from(at.hour()).unwrap_or(0) / hours(count)
}

/// `A`, `B`…
pub fn letter(daypart: u8) -> char {
    char::from(b'A' + daypart.min(25))
}

/// Days since 1970-01-01 (civil dates, no time zone).
fn days(date: Date) -> i64 {
    // Howard Hinnant's days_from_civil.
    let (year, month, day) = (
        i64::from(date.year()),
        i64::from(date.month()),
        i64::from(date.day()),
    );
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_era_days - 719_468
}

/// Dayparts since 1970-01-01's A: the next daypart, across midnight too,
/// is the next number.
pub fn ordinal(count: u8, at: DateTime) -> i64 {
    days(at.date()) * i64::from(self::count(count)) + i64::from(of(count, at))
}

/// Whether a play at `last` keeps a track out at `at` (station times),
/// with a daypart separation of `separation`: `at` is in one of the
/// `separation` dayparts after the one of `last`. The daypart of `last`
/// itself is not kept out.
pub fn holds(count: u8, separation: u8, last: DateTime, at: DateTime) -> bool {
    let after = ordinal(count, at) - ordinal(count, last);
    (1..=i64::from(separation)).contains(&after)
}

/// The station day of a station time: days since 1970-01-01.
pub fn day(at: DateTime) -> i64 {
    days(at.date())
}

/// Whether a play at `played` keeps a track out at `at` (station times)
/// with the same daypart kept out on the `days` following days: `at` is
/// in the daypart of `played` (the same letter), 1 to `days` days later.
pub fn holds_days(count: u8, days: u8, played: DateTime, at: DateTime) -> bool {
    of(count, played) == of(count, at) && (1..=i64::from(days)).contains(&(day(at) - day(played)))
}

/// How far back a play can still hold a track back with the same daypart
/// kept out on `days` following days, in minutes: the days and the
/// daypart, and an hour for the clocks going back. 0 when off.
pub fn days_lookback_min(count: u8, days: u8) -> u32 {
    if days == 0 {
        return 0;
    }
    u32::from(days) * 24 * 60 + u32::from(hours(count)) * 60 + 60
}

/// How far back a play can still hold a track back with `separation`, in
/// minutes: the dayparts it covers and the one played in, and an hour for
/// the clocks going back. 0 when off.
pub fn lookback_min(count: u8, separation: u8) -> u32 {
    if separation == 0 {
        return 0;
    }
    (u32::from(separation) + 1) * u32::from(hours(count)) * 60 + 60
}

/// The dayparts at a pick, for the rotation log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DaypartLog {
    /// The station's division of the day: 4 or 6 dayparts.
    pub count: u8,
    /// The pick's daypart, from 0 (A).
    pub daypart: u8,
    /// The daypart separation of the slot's category, before relaxation;
    /// `windows.dayparts` is the one applied.
    pub separation: u8,
    /// The days the same daypart is kept out on after a play, from the
    /// slot's category, before relaxation; `windows.daypart_days` is the
    /// one applied. Missing in picks logged before it.
    #[serde(default)]
    pub days: u8,
    /// The daypart the picked track last played in, from 0; missing if
    /// never.
    pub last_daypart: Option<u8>,
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use jiff::tz::TimeZone;
    use jiff::{Timestamp, ToSpan};

    use super::*;

    fn at(day: i8, hour: i8, minute: i8) -> DateTime {
        date(2026, 11, day).at(hour, minute, 0, 0)
    }

    #[test]
    fn days_are_counted_from_1970() {
        assert_eq!(days(date(1970, 1, 1)), 0);
        assert_eq!(days(date(1969, 12, 31)), -1);
        assert_eq!(days(date(2000, 3, 1)), 11_017);
        let mut day = date(2023, 12, 25);
        for _ in 0..800 {
            let next = day.tomorrow().unwrap();
            assert_eq!(days(next), days(day) + 1, "{day}");
            day = next;
        }
    }

    #[test]
    fn four_dayparts_of_six_hours() {
        let dayparts: Vec<u8> = (0..24).map(|h| of(4, at(5, h, 0))).collect();
        assert_eq!(
            dayparts,
            [
                0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3
            ]
        );
        assert_eq!(of(4, at(5, 5, 59)), 0);
        assert_eq!(of(4, at(5, 23, 59)), 3);
        assert_eq!(letter(3), 'D');
        // Anything else is 4.
        assert_eq!(of(5, at(5, 18, 0)), 3);
    }

    #[test]
    fn six_dayparts_of_four_hours() {
        let dayparts: Vec<u8> = (0..24).step_by(4).map(|h| of(6, at(5, h, 0))).collect();
        assert_eq!(dayparts, [0, 1, 2, 3, 4, 5]);
        assert_eq!(of(6, at(5, 3, 59)), 0);
        assert_eq!(letter(of(6, at(5, 20, 0))), 'F');
    }

    #[test]
    fn separation_one_keeps_out_the_next_daypart_only() {
        // Played in A at 04:12.
        let played = at(5, 4, 12);
        let held = |hour| holds(4, 1, played, at(5, hour, 0));
        assert!(!held(5), "the same daypart is for the track separation");
        assert!(held(6) && held(11));
        assert!(!held(12) && !held(23));
        // D, then the next day's A follows it.
        let late = at(5, 22, 0);
        assert!(holds(4, 1, late, at(6, 0, 10)));
        assert!(holds(4, 1, late, at(6, 5, 59)));
        assert!(!holds(4, 1, late, at(6, 6, 0)));
        assert!(!holds(4, 0, late, at(6, 0, 10)), "0 is off");
    }

    #[test]
    fn separation_six_runs_into_the_next_day() {
        // 4 dayparts: played in A, kept out of B, C, D, then the next day's
        // A, B and C; plays again from the next day's D, 36 hours later.
        let played = at(5, 1, 0);
        assert!(holds(4, 6, played, at(5, 6, 0)));
        assert!(holds(4, 6, played, at(6, 0, 0)));
        assert!(holds(4, 6, played, at(6, 17, 59)));
        assert!(!holds(4, 6, played, at(6, 18, 0)));
        assert!(!holds(4, 6, played, at(5, 3, 0)));
        // 6 dayparts, up to 10: played in F, kept out of the next day's
        // six and of A to D the day after; plays again from its E (16:00).
        let played = at(5, 21, 0);
        assert!(holds(6, 10, played, at(6, 0, 0)));
        assert!(holds(6, 10, played, at(7, 11, 59)));
        assert!(holds(6, 10, played, at(7, 15, 59)));
        assert!(!holds(6, 10, played, at(7, 16, 0)));
        assert_eq!(max_separation(4), 6);
        assert_eq!(max_separation(6), 10);
    }

    #[test]
    fn the_lookback_covers_every_daypart_kept_out() {
        assert_eq!(lookback_min(4, 0), 0);
        // Played at the very start of A, held until the end of D: 24 h.
        assert_eq!(lookback_min(4, 3), 24 * 60 + 60);
        assert_eq!(lookback_min(6, 10), 44 * 60 + 60);
    }

    #[test]
    fn the_same_daypart_is_kept_out_on_the_following_days() {
        // Played in B on Thursday 5 November, one day: Friday's B is kept
        // out, Saturday's is not.
        let played = at(5, 8, 0);
        assert!(holds_days(4, 1, played, at(6, 6, 0)));
        assert!(holds_days(4, 1, played, at(6, 11, 59)));
        assert!(!holds_days(4, 1, played, at(7, 6, 0)));
        // Only that daypart: Friday's A and C are free, and so is the same
        // day's B (the track separation decides).
        assert!(!holds_days(4, 1, played, at(6, 5, 59)));
        assert!(!holds_days(4, 1, played, at(6, 12, 0)));
        assert!(!holds_days(4, 1, played, at(5, 11, 0)));
        assert!(!holds_days(4, 0, played, at(6, 8, 0)), "0 is off");
        // Seven days: the B of the next seven days, not the eighth's.
        assert!(holds_days(4, 7, played, at(12, 7, 0)));
        assert!(!holds_days(4, 7, played, at(13, 7, 0)));
        // Across midnight and a month: played in D at 23:50, the next
        // day's D is kept out, its A is not.
        let late = date(2026, 11, 30).at(23, 50, 0, 0);
        assert!(!holds_days(4, 1, late, date(2026, 12, 1).at(0, 10, 0, 0)));
        assert!(holds_days(4, 1, late, date(2026, 12, 1).at(18, 0, 0, 0)));
        // With 6 dayparts, the letters are of 4 hours: 08:00 is C.
        assert!(holds_days(6, 2, at(5, 9, 0), at(7, 11, 0)));
        assert!(!holds_days(6, 2, at(5, 9, 0), at(7, 12, 0)));
    }

    #[test]
    fn the_days_lookback_reaches_the_start_of_the_daypart() {
        assert_eq!(days_lookback_min(4, 0), 0);
        // Played at the start of B, held until the end of B X days later.
        assert_eq!(days_lookback_min(4, 1), 24 * 60 + 6 * 60 + 60);
        assert_eq!(days_lookback_min(6, 7), 7 * 24 * 60 + 4 * 60 + 60);
    }

    /// Station times of the instants from `start`, every `step` minutes.
    fn instants(zone: &TimeZone, start: DateTime, hours: i64) -> Vec<(Timestamp, DateTime)> {
        let start = start.to_zoned(zone.clone()).unwrap().timestamp();
        (0..hours * 4)
            .map(|quarter| {
                let instant = start.checked_add((quarter * 15).minutes()).unwrap();
                (instant, instant.to_zoned(zone.clone()).datetime())
            })
            .collect()
    }

    /// 2026-03-29, Europe/Paris: 02:00 does not exist, A is 5 hours long.
    #[test]
    fn clocks_going_forward_shorten_a_daypart() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let times: Vec<_> = instants(&paris, date(2026, 3, 29).at(0, 0, 0, 0), 24)
            .into_iter()
            .filter(|(_, local)| local.date() == date(2026, 3, 29))
            .collect();
        assert_eq!(times.len(), 23 * 4);
        let in_a = times.iter().filter(|(_, local)| of(4, *local) == 0).count();
        let in_b = times.iter().filter(|(_, local)| of(4, *local) == 1).count();
        assert_eq!((in_a, in_b), (5 * 4, 6 * 4));
        // Played at 01:30, the next daypart still starts at 06:00.
        let played = date(2026, 3, 29).at(1, 30, 0, 0);
        assert!(!holds(4, 1, played, date(2026, 3, 29).at(3, 0, 0, 0)));
        assert!(holds(4, 1, played, date(2026, 3, 29).at(6, 0, 0, 0)));
    }

    /// 2026-10-25, Europe/Paris: 02:00 to 02:59 happens twice, both in A,
    /// which is 7 hours long.
    #[test]
    fn clocks_going_back_keep_the_repeated_hour_in_its_daypart() {
        let paris = TimeZone::get("Europe/Paris").unwrap();
        let times = instants(&paris, date(2026, 10, 25).at(0, 0, 0, 0), 25);
        assert!(
            times
                .iter()
                .all(|(_, local)| local.date() == date(2026, 10, 25))
        );
        let at_two: Vec<_> = times
            .iter()
            .filter(|(_, local)| local.hour() == 2)
            .collect();
        assert_eq!(at_two.len(), 8, "the hour is repeated");
        assert!(at_two.iter().all(|(_, local)| of(4, *local) == 0));
        let in_a = times.iter().filter(|(_, local)| of(4, *local) == 0).count();
        assert_eq!(in_a, 7 * 4);
        // Played in the first 02:30, the second one is the same daypart.
        let (first, second) = (at_two[2], at_two[6]);
        assert_eq!(first.1, second.1);
        assert!(first.0 < second.0);
        assert!(!holds(4, 1, first.1, second.1));
    }
}
