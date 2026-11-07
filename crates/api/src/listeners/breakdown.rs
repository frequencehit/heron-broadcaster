// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shares of the listeners and of the time spent listening by country,
//! device or player, with the small groups added up as *Other* so that a
//! small station does not single out a listener. See
//! `docs/statistics.md#shares`.

use std::collections::{BTreeMap, HashSet};

use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::sessions::ListenerKey;

/// A group with fewer distinct listeners than this on every day of the
/// period is added to *Other*.
pub const MIN_GROUP_LISTENERS: usize = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum By {
    #[default]
    Country,
    Device,
    Player,
}

/// What a session counts in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// A country code, a device or a player family.
    Named(String),
    /// A country the database does not know.
    Unknown,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShareKind {
    Named,
    Unknown,
    /// The small groups, and what says nothing of itself.
    Other,
}

/// A country, device or player over a period.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Share {
    pub kind: ShareKind,
    /// The country code (`FR`), the device (`mobile`, `desktop`,
    /// `smart_speaker`, `car`, `tv`) or the player family; missing for
    /// *Unknown* and *Other*.
    pub name: Option<String>,
    /// Distinct listeners of each day, added up.
    pub listeners: u32,
    /// Its share of the listeners, 0 to 1.
    pub listener_share: f64,
    pub sessions: u32,
    /// Time spent listening, in hours.
    pub ttsl_hours: f64,
    /// Its share of the time spent listening, 0 to 1.
    pub ttsl_share: f64,
}

/// A session of at least 5 s, and what it counts in.
pub struct Counted {
    pub day: Date,
    pub key: ListenerKey,
    pub group: Group,
    pub duration_s: u32,
}

#[derive(Default)]
struct Sum<'a> {
    days: BTreeMap<Date, HashSet<&'a ListenerKey>>,
    sessions: u32,
    seconds: u64,
}

impl Sum<'_> {
    fn listeners(&self) -> usize {
        self.days.values().map(HashSet::len).sum()
    }
}

/// The groups of a period, the most listened first, then *Unknown* and
/// *Other*.
pub fn shares(sessions: &[Counted]) -> Vec<Share> {
    let mut groups: BTreeMap<&Group, Sum> = BTreeMap::new();
    for session in sessions {
        let sum = groups.entry(&session.group).or_default();
        sum.days
            .entry(session.day)
            .or_default()
            .insert(&session.key);
        sum.sessions += 1;
        sum.seconds += u64::from(session.duration_s);
    }
    // (kind, name, listener-days, sessions, seconds)
    let mut rows: Vec<(ShareKind, Option<String>, usize, u32, u64)> = Vec::new();
    let mut other = (0usize, 0u32, 0u64);
    for (group, sum) in &groups {
        let small = sum
            .days
            .values()
            .all(|keys| keys.len() < MIN_GROUP_LISTENERS);
        match group {
            Group::Named(name) if !small => rows.push((
                ShareKind::Named,
                Some(name.clone()),
                sum.listeners(),
                sum.sessions,
                sum.seconds,
            )),
            Group::Unknown => rows.push((
                ShareKind::Unknown,
                None,
                sum.listeners(),
                sum.sessions,
                sum.seconds,
            )),
            Group::Named(_) | Group::Other => {
                other.0 += sum.listeners();
                other.1 += sum.sessions;
                other.2 += sum.seconds;
            }
        }
    }
    if other.1 > 0 {
        rows.push((ShareKind::Other, None, other.0, other.1, other.2));
    }
    let listeners: usize = rows.iter().map(|row| row.2).sum();
    let seconds: u64 = rows.iter().map(|row| row.4).sum();
    let ratio = |part: f64, whole: f64| if whole > 0.0 { part / whole } else { 0.0 };
    let mut shares: Vec<Share> = rows
        .into_iter()
        .map(
            |(kind, name, group_listeners, sessions, group_seconds)| Share {
                kind,
                name,
                listeners: u32::try_from(group_listeners).unwrap_or(u32::MAX),
                listener_share: ratio(group_listeners as f64, listeners as f64),
                sessions,
                ttsl_hours: group_seconds as f64 / 3_600.0,
                ttsl_share: ratio(group_seconds as f64, seconds as f64),
            },
        )
        .collect();
    let rank = |kind: ShareKind| match kind {
        ShareKind::Named => 0,
        ShareKind::Unknown => 1,
        ShareKind::Other => 2,
    };
    shares.sort_by(|a, b| {
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then(b.listeners.cmp(&a.listeners))
            .then(b.ttsl_hours.total_cmp(&a.ttsl_hours))
            .then(a.name.cmp(&b.name))
    });
    shares
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn counted(day: i8, n: u8, group: Group, minutes: u32) -> Counted {
        Counted {
            day: date(2026, 10, day),
            key: [n; 32],
            group,
            duration_s: minutes * 60,
        }
    }

    fn named(name: &str) -> Group {
        Group::Named(name.to_owned())
    }

    #[test]
    fn small_groups_are_other() {
        let mut sessions = Vec::new();
        // FR: 5 listeners on the 20th, 2 on the 21st.
        for n in 0..5 {
            sessions.push(counted(20, n, named("FR"), 30));
        }
        sessions.push(counted(21, 0, named("FR"), 30));
        sessions.push(counted(21, 1, named("FR"), 30));
        // BE: one listener every day, 4 on the 21st: never 5 on a day.
        sessions.push(counted(20, 10, named("BE"), 60));
        for n in 10..14 {
            sessions.push(counted(21, n, named("BE"), 60));
        }
        // IS: one listener, twice.
        sessions.push(counted(20, 20, named("IS"), 120));
        sessions.push(counted(20, 20, named("IS"), 60));
        // Not found: shown as such.
        sessions.push(counted(20, 30, Group::Unknown, 15));
        let shares = shares(&sessions);
        let rows: Vec<(ShareKind, Option<&str>, u32, u32)> = shares
            .iter()
            .map(|s| (s.kind, s.name.as_deref(), s.listeners, s.sessions))
            .collect();
        assert_eq!(
            rows,
            [
                (ShareKind::Named, Some("FR"), 7, 7),
                (ShareKind::Unknown, None, 1, 1),
                (ShareKind::Other, None, 6, 7),
            ]
        );
        // 14 listener-days; 7 × 30 + 15 + 5 × 60 + 180 minutes.
        assert!((shares[0].listener_share - 7.0 / 14.0).abs() < 1e-9);
        assert!((shares[2].ttsl_hours - 8.0).abs() < 1e-9);
        assert!((shares[0].ttsl_share - 210.0 / 705.0).abs() < 1e-9);
        let total: f64 = shares.iter().map(|s| s.ttsl_share).sum();
        assert!((total - 1.0).abs() < 1e-9);
    }

    #[test]
    fn nothing_gives_no_groups() {
        assert!(shares(&[]).is_empty());
        let other = shares(&[counted(20, 1, Group::Other, 10)]);
        assert_eq!(
            (other[0].kind, other[0].listener_share),
            (ShareKind::Other, 1.0)
        );
    }
}
