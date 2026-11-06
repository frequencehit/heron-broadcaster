// SPDX-License-Identifier: AGPL-3.0-or-later

//! M6 acceptance: a week of programming on a realistic station breaks no
//! rule outside logged relaxations, and never breaks a restriction.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use heron_rotation::names::{artist_names, normalize, title_key};
use heron_rotation::{
    Category, Clock, History, Library, LibraryParts, Member, Method, Mode, Restriction,
    RotationSettings, Rule, Schedule, SeparationOverrides, Simulation, Slot, Target, Track,
    Violation, simulate,
};
use jiff::civil::date;
use jiff::tz::TimeZone;

const DAY_MS: i64 = 24 * 3_600_000;

const POWER: u64 = 1;
const CURRENT: u64 = 2;
const GOLD: u64 = 3;
const BALLADS: u64 = 4;
const CHRISTMAS: u64 = 5;
const LONG: u64 = 6;

/// Names linked to artists the way heron-api links them: by normalized name.
struct Artists {
    by_name: HashMap<String, u64>,
}

impl Artists {
    fn id(&mut self, name: &str) -> u64 {
        let next = self.by_name.len() as u64 + 1;
        *self.by_name.entry(normalize(name)).or_insert(next)
    }
}

struct Station {
    parts: LibraryParts,
    artists: Artists,
}

/// Deterministic pseudo-random numbers for building the library.
struct Dice(u64);

impl Dice {
    fn roll(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

fn station() -> Station {
    let mut dice = Dice(42);
    let mut artists = Artists {
        by_name: HashMap::new(),
    };
    let mut tracks = Vec::new();
    let mut members: HashMap<u64, Vec<Member>> = HashMap::new();
    let mut add = |artists: &mut Artists,
                   dice: &mut Dice,
                   category: u64,
                   artist: String,
                   title: String,
                   album: String| {
        let id = tracks.len() as u64 + 1;
        let names = artist_names(Some(&artist), Some(&title));
        tracks.push(Track {
            id,
            title_key: title_key(Some(&title)),
            album_key: title_key(Some(&album)),
            artists: names.iter().map(|name| artists.id(name)).collect(),
            categories: vec![],
            length_ms: 150_000 + dice.roll(180_000) as u32,
            // Every ninth track plays 3 times out of 10.
            ratio: if id.is_multiple_of(9) { 3 } else { 10 },
        });
        members.entry(category).or_default().push(Member {
            track: id,
            weight: 0.5 + dice.roll(10) as f64 / 4.0,
        });
        id
    };
    // 120 artists, 4 albums of 6 tracks each, spread over the categories.
    for a in 0..120 {
        for b in 0..4 {
            for t in 0..6 {
                let category = match (a + b * 7 + t * 3) % 10 {
                    0 => POWER,
                    1..=3 => CURRENT,
                    4..=8 => GOLD,
                    _ => BALLADS,
                };
                add(
                    &mut artists,
                    &mut dice,
                    category,
                    format!("Artist {a}"),
                    format!("Song {a}-{b}-{t}"),
                    format!("Album {a}-{b}"),
                );
            }
        }
    }
    // Names of the same artist, and featurings.
    for (artist, title) in [
        ("Neal Morse", "Testimony"),
        ("NEAL  MORSE", "Momentum"),
        ("Néal Morse", "Sola Scriptura"),
        ("The Neal Morse Band", "The Grand Experiment"),
        ("The Neal Morse Band", "Alive Again"),
        ("Transatlantic", "We All Need Some Light"),
        ("Artist 3 feat. Neal Morse", "Duet"),
        ("Artist 4", "Song (feat. Transatlantic)"),
        ("Neal Morse", "Wish You Were Here (Remastered 2011)"),
        ("Pink Floyd", "Wish You Were Here"),
    ] {
        add(
            &mut artists,
            &mut dice,
            POWER,
            artist.to_owned(),
            title.to_owned(),
            format!("{title} album"),
        );
    }
    for t in 0..30 {
        add(
            &mut artists,
            &mut dice,
            CHRISTMAS,
            format!("Artist {}", t % 15),
            format!("Christmas {t}"),
            "Christmas".to_owned(),
        );
    }
    for t in 0..20 {
        add(
            &mut artists,
            &mut dice,
            LONG,
            format!("Artist {}", 60 + t),
            format!("Epic {t}"),
            format!("Epic album {t}"),
        );
    }
    let neal = artists.id("Neal Morse");
    let band = artists.id("The Neal Morse Band");
    let transatlantic = artists.id("Transatlantic");
    let explicit = artists.id("Artist 7");
    let banned_track = 5;
    let categories = members
        .into_iter()
        .map(|(id, members)| Category {
            id,
            name: format!("Category {id}"),
            separation: match id {
                // Power rotation repeats its titles sooner.
                POWER => SeparationOverrides {
                    title_min: Some(150),
                    track_min: Some(150),
                    ..SeparationOverrides::default()
                },
                GOLD => SeparationOverrides {
                    track_min: Some(24 * 60),
                    ..SeparationOverrides::default()
                },
                _ => SeparationOverrides::default(),
            },
            members,
        })
        .collect();
    let slot = |category, method| Slot {
        category,
        method,
        enforce_rules: true,
        separation: SeparationOverrides::default(),
    };
    let day = Clock {
        id: 1,
        name: "Day".into(),
        looping: false,
        slots: vec![
            slot(POWER, Method::Weighted),
            slot(CURRENT, Method::LeastRecentlyPlayed),
            slot(GOLD, Method::Random),
            slot(BALLADS, Method::LeastRecentlyPlayed),
            slot(CURRENT, Method::Weighted),
            slot(CHRISTMAS, Method::LeastRecentlyPlayed),
            slot(GOLD, Method::LeastRecentlyPlayed),
            Slot {
                // Artist separation shortened for this slot.
                separation: SeparationOverrides {
                    artist_min: Some(60),
                    ..SeparationOverrides::default()
                },
                ..slot(POWER, Method::Random)
            },
            slot(LONG, Method::Random),
            slot(BALLADS, Method::Random),
        ],
    };
    let night = Clock {
        id: 2,
        name: "Night".into(),
        // Runs from midnight to 5, carrying on across the hours.
        looping: true,
        slots: vec![
            slot(GOLD, Method::Random),
            slot(BALLADS, Method::Random),
            Slot {
                enforce_rules: false,
                ..slot(LONG, Method::Random)
            },
            slot(GOLD, Method::Weighted),
        ],
    };
    let mut schedule = Schedule::default();
    for day_of_week in 1..=7 {
        for hour in 0..24 {
            let clock = match hour {
                0..=4 => Some(2),
                // Sunday afternoon is not scheduled: the library rotates.
                13..=15 if day_of_week == 7 => None,
                _ => Some(1),
            };
            if let Some(clock) = clock {
                schedule.hours.insert((day_of_week, hour), clock);
            }
        }
    }
    let restrictions = vec![
        Restriction {
            id: 1,
            target: Target::Category(BALLADS),
            rule: Rule::NotAfter {
                category_id: BALLADS,
            },
        },
        Restriction {
            id: 2,
            target: Target::Artist(explicit),
            rule: Rule::Daypart {
                mode: Mode::Ban,
                days: vec![1, 2, 3, 4, 5, 6, 7],
                start_hour: 6,
                end_hour: 22,
            },
        },
        Restriction {
            id: 3,
            target: Target::Category(CHRISTMAS),
            rule: Rule::DateRange {
                mode: Mode::Allow,
                start: date(2000, 12, 1),
                end: date(2000, 1, 6),
                every_year: true,
            },
        },
        Restriction {
            id: 4,
            target: Target::Category(LONG),
            rule: Rule::Daypart {
                mode: Mode::Ban,
                days: vec![1, 2, 3, 4, 5],
                start_hour: 6,
                end_hour: 10,
            },
        },
        Restriction {
            id: 5,
            target: Target::Track(banned_track),
            rule: Rule::DateRange {
                mode: Mode::Ban,
                start: date(2026, 10, 1),
                end: date(2026, 12, 31),
                every_year: false,
            },
        },
    ];
    Station {
        parts: LibraryParts {
            tracks,
            categories,
            similar: vec![(neal, band), (band, transatlantic)],
            restrictions,
            clocks: vec![day, night],
            schedule,
            schedules: Vec::new(),
            settings: RotationSettings {
                fallback_category_id: Some(GOLD),
                ..RotationSettings::default()
            },
            no_two_in_a_row: Vec::new(),
            counters: HashMap::new(),
            modules_off: Vec::new(),
        },
        artists,
    }
}

fn paris() -> TimeZone {
    TimeZone::get("Europe/Paris").unwrap()
}

/// Monday 2026-10-19 00:00 in Paris; the clocks go back on Sunday 25th.
fn start() -> i64 {
    date(2026, 10, 19)
        .at(0, 0, 0, 0)
        .to_zoned(paris())
        .unwrap()
        .timestamp()
        .as_millisecond()
}

fn week(library: &Library, seed: u64) -> Simulation {
    simulate(library, History::default(), None, start(), 7 * DAY_MS, seed)
}

#[test]
fn a_week_breaks_no_rule_outside_logged_relaxations() {
    let library = Library::new(station().parts, paris());
    for seed in [1, 2, 3] {
        let simulation = week(&library, seed);
        assert!(
            simulation.violations.is_empty(),
            "seed {seed}: {:#?}",
            &simulation.violations[..simulation.violations.len().min(10)]
        );
        assert_eq!(simulation.gap_minutes, 0);
        let picks = simulation.picks.len();
        assert!(picks > 2_000, "{picks} picks in a week");
        assert_eq!(simulation.steps.iter().sum::<u32>() as usize, picks);
        // The library is large enough: most picks need no relaxation.
        assert!(
            simulation.steps[0] as usize > picks * 9 / 10,
            "{:?}",
            simulation.steps
        );
        // Picks are back to back.
        let last = simulation.picks.last().unwrap();
        assert!(last.start_unix_ms + i64::from(last.length_ms) >= simulation.end_unix_ms);
    }
}

/// Daypart separation over a week that crosses the clocks going back
/// (Sunday 25 October 2026 in Paris): 2 for Current, 1 for Gold, none for
/// the other categories, 4 or 6 dayparts. No pick plays in a daypart its last play keeps it out of,
/// unless it logged the rule relaxed (step 3 halves it, 4 drops it).
#[test]
fn a_week_keeps_tracks_out_of_the_next_dayparts() {
    for count in [4u8, 6] {
        let mut station = station();
        station.parts.settings.daypart_count = count;
        for category in &mut station.parts.categories {
            category.separation.dayparts = match category.id {
                CURRENT => Some(2),
                GOLD => Some(1),
                _ => None,
            };
        }
        let library = Library::new(station.parts, paris());
        let simulation = week(&library, 5);
        assert!(
            simulation.violations.is_empty(),
            "{count} dayparts: {:?}",
            &simulation.violations[..simulation.violations.len().min(10)]
        );
        let held: u32 = simulation
            .picks
            .iter()
            .map(|pick| pick.explanation.blocked.daypart)
            .sum();
        assert!(held > 0, "{count} dayparts: nothing held back");
        // Checked again here, from the station times: hours / (24 / count),
        // days apart.
        let daypart = |ms: i64| {
            let local = jiff::Timestamp::from_millisecond(ms)
                .unwrap()
                .to_zoned(paris());
            let days = local
                .date()
                .since((jiff::Unit::Day, date(2026, 1, 1)))
                .unwrap()
                .get_days();
            i64::from(days) * i64::from(count) + i64::from(local.hour()) / (24 / i64::from(count))
        };
        let mut last: HashMap<u64, i64> = HashMap::new();
        let mut checked = 0;
        for pick in &simulation.picks {
            let explanation = &pick.explanation;
            let log = explanation.daypart.unwrap();
            assert_eq!(log.count, count);
            assert_eq!(
                i64::from(log.daypart),
                daypart(pick.start_unix_ms).rem_euclid(i64::from(count))
            );
            if let Some(previous) = last.insert(pick.track_id, pick.start_unix_ms) {
                assert_eq!(
                    log.last_daypart.map(i64::from),
                    Some(daypart(previous).rem_euclid(i64::from(count)))
                );
                let after = daypart(pick.start_unix_ms) - daypart(previous);
                // Only the slot's category's value counts.
                let own = match explanation.slot_category_id {
                    Some(CURRENT) => 2,
                    Some(GOLD) => 1,
                    _ => 0,
                };
                if explanation.enforce_rules && explanation.step == 0 {
                    assert_eq!(explanation.windows.dayparts, own, "{explanation:?}");
                }
                if explanation.enforce_rules && explanation.step < 3 {
                    assert!(
                        !(1..=i64::from(explanation.windows.dayparts)).contains(&after),
                        "{count} dayparts: track {} {after} dayparts after its last play",
                        pick.track_id
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 500, "{checked}");
    }
}

/// The same daypart on the following days over the same week: 1 day for
/// Current (without daypart separation), 2 for Gold (with a daypart
/// separation of 1). No pick plays in the daypart it played in on the days
/// its category keeps out, unless it logged the rule relaxed.
#[test]
fn a_week_keeps_tracks_out_of_the_same_daypart_the_next_days() {
    let mut station = station();
    for category in &mut station.parts.categories {
        (
            category.separation.dayparts,
            category.separation.daypart_days,
        ) = match category.id {
            CURRENT => (None, Some(1)),
            GOLD => (Some(1), Some(2)),
            _ => (None, None),
        };
    }
    let library = Library::new(station.parts, paris());
    let simulation = week(&library, 6);
    assert!(
        simulation.violations.is_empty(),
        "{:?}",
        &simulation.violations[..simulation.violations.len().min(10)]
    );
    let held: u32 = simulation
        .picks
        .iter()
        .map(|pick| pick.explanation.blocked.daypart_days)
        .sum();
    assert!(held > 0, "nothing held back");
    // Checked again here, from the station times: (day, daypart).
    let part = |ms: i64| {
        let local = jiff::Timestamp::from_millisecond(ms)
            .unwrap()
            .to_zoned(paris());
        let day = local
            .date()
            .since((jiff::Unit::Day, date(2026, 1, 1)))
            .unwrap()
            .get_days();
        (i64::from(day), local.hour() / 6)
    };
    let mut plays: HashMap<u64, Vec<i64>> = HashMap::new();
    let mut checked = 0;
    for pick in &simulation.picks {
        let explanation = &pick.explanation;
        let own = match explanation.slot_category_id {
            Some(CURRENT) => 1,
            Some(GOLD) => 2,
            _ => 0,
        };
        assert_eq!(explanation.daypart.unwrap().days, own, "{explanation:?}");
        let (day, daypart) = part(pick.start_unix_ms);
        let earlier = plays.entry(pick.track_id).or_default();
        if explanation.enforce_rules && explanation.step < 3 {
            assert_eq!(explanation.windows.daypart_days, own);
            for (before, before_part) in earlier.iter().map(|ms| part(*ms)) {
                assert!(
                    before_part != daypart || !(1..=i64::from(own)).contains(&(day - before)),
                    "track {} in the same daypart {} days after",
                    pick.track_id,
                    day - before
                );
            }
            checked += 1;
        }
        earlier.push(pick.start_unix_ms);
    }
    assert!(checked > 500, "{checked}");
}

#[test]
fn a_week_follows_the_play_ratios() {
    let mut station = station();
    // Start some counters in their held-back steps.
    station.parts.counters = HashMap::from([(9, 7), (18, 1), (27, 10)]);
    let library = Library::new(station.parts, paris());
    let simulation = week(&library, 8);
    assert!(
        simulation.violations.is_empty(),
        "{:?}",
        simulation.violations
    );
    let held: usize = simulation
        .picks
        .iter()
        .map(|pick| pick.explanation.held_by_ratio.len())
        .sum();
    assert!(held > 10, "{held} tracks held back by their ratio");
    // Each track at 0.3 plays at 10, 9 and 8 and is held back from 7 down
    // to 1: replayed here from the picks.
    let mut counters = library.counters.clone();
    let mut offers: HashMap<u64, (u32, u32)> = HashMap::new();
    for pick in &simulation.picks {
        let offered = pick
            .explanation
            .held_by_ratio
            .iter()
            .map(|held| (held.track_id, false))
            .chain([(pick.track_id, true)]);
        for (track, played) in offered {
            let counter = counters.get(&track).copied().unwrap_or(10);
            if library.tracks[&track].ratio == 3 {
                assert_eq!(played, counter > 7, "track {track} at {counter}");
                let entry = offers.entry(track).or_default();
                entry.0 += 1;
                entry.1 += u32::from(played);
            }
            counters.insert(track, if counter == 1 { 10 } else { counter - 1 });
        }
    }
    let (offered, played) = offers.values().fold((0, 0), |(o, p), (offered, played)| {
        (o + offered, p + played)
    });
    assert!(offered > 20, "{offered} offers");
    assert!(
        played > 0 && played < offered,
        "{played} played of {offered}"
    );
}

#[test]
fn a_week_never_breaks_a_restriction() {
    let station = station();
    let library = Library::new(station.parts, paris());
    let simulation = week(&library, 4);
    assert!(
        !simulation
            .violations
            .iter()
            .any(|v| matches!(v, Violation::Restriction { .. }))
    );
    let mut balladless = true;
    for (index, pick) in simulation.picks.iter().enumerate() {
        let track = &library.tracks[&pick.track_id];
        let local = library.local(pick.start_unix_ms);
        // Christmas is out of season in October.
        assert!(!track.categories.contains(&CHRISTMAS));
        assert_ne!(track.id, 5);
        if track.categories.contains(&LONG) {
            let weekday = local.weekday().to_monday_one_offset();
            assert!(!(weekday <= 5 && (6..10).contains(&local.hour())));
        }
        if track.categories.contains(&BALLADS) && index > 0 {
            let previous = &library.tracks[&simulation.picks[index - 1].track_id];
            assert!(!previous.categories.contains(&BALLADS));
        }
        balladless &= !track.categories.contains(&BALLADS);
    }
    assert!(!balladless, "ballads were played");
    // The Christmas slot is relaxed into the fallback category, never
    // broken: its picks come from Gold.
    let christmas_slots: Vec<_> = simulation
        .picks
        .iter()
        .filter(|p| p.explanation.slot_category_id == Some(CHRISTMAS))
        .collect();
    assert!(!christmas_slots.is_empty());
    for pick in christmas_slots {
        assert!(pick.explanation.step >= 11);
        assert_eq!(pick.explanation.category_id, Some(GOLD));
        assert_eq!(pick.explanation.restricted, 30);
    }
}

#[test]
fn names_of_an_artist_and_similar_artists_keep_their_distance() {
    let mut station = station();
    let artist = station.artists.id("Neal Morse");
    assert_eq!(station.artists.id("néal   MORSE"), artist);
    let library = Library::new(station.parts, paris());
    let similar = |a: u64, b: u64| library.same_artist(a, b);
    for seed in [5, 6] {
        let simulation = week(&library, seed);
        let window = |pick: &heron_rotation::SimulatedPick| {
            // The slot that shortens artist separation to an hour.
            if pick.explanation.slot == Some(7) && pick.explanation.clock_id == Some(1) {
                60
            } else {
                120
            }
        };
        for (index, pick) in simulation.picks.iter().enumerate() {
            // Steps 5 and up relax artist separation, and are logged; a
            // slot can be set not to enforce the rules.
            if pick.explanation.step >= 5 || !pick.explanation.enforce_rules {
                continue;
            }
            let track = &library.tracks[&pick.track_id];
            for earlier in simulation.picks[..index].iter().rev() {
                let gap_min = (pick.start_unix_ms - earlier.start_unix_ms) / 60_000;
                if gap_min >= window(pick) {
                    break;
                }
                let other = &library.tracks[&earlier.track_id];
                let shared = track
                    .artists
                    .iter()
                    .any(|a| other.artists.iter().any(|b| similar(*a, *b)));
                assert!(
                    !shared,
                    "tracks {} and {} share an artist {gap_min} min apart",
                    other.id, track.id
                );
            }
        }
        // The Neal Morse tracks were played.
        assert!(
            simulation
                .picks
                .iter()
                .any(|p| library.tracks[&p.track_id].artists.contains(&artist))
        );
    }
}

#[test]
fn a_small_library_relaxes_step_by_step_and_logs_it() {
    // Three artists, eight tracks: separation cannot hold.
    let mut parts = station().parts;
    parts.tracks.truncate(8);
    for track in &mut parts.tracks {
        track.artists = vec![track.id % 3];
    }
    parts.categories.retain(|c| c.id == GOLD || c.id == POWER);
    for category in &mut parts.categories {
        category.members = (1..=8)
            .filter(|t| (t % 2 == 0) == (category.id == GOLD))
            .map(|track| Member { track, weight: 1.0 })
            .collect();
    }
    parts.clocks = vec![Clock {
        id: 1,
        name: "Small".into(),
        looping: false,
        slots: vec![
            Slot {
                category: POWER,
                method: Method::LeastRecentlyPlayed,
                enforce_rules: true,
                separation: SeparationOverrides::default(),
            },
            Slot {
                category: GOLD,
                method: Method::Random,
                enforce_rules: true,
                separation: SeparationOverrides::default(),
            },
        ],
    }];
    parts.restrictions.clear();
    parts.schedule.hours = (1..=7)
        .flat_map(|d| (0..24).map(move |h| ((d, h), 1)))
        .collect();
    let library = Library::new(parts, TimeZone::UTC);
    let simulation = simulate(&library, History::default(), None, 0, 2 * DAY_MS, 9);
    assert!(
        simulation.violations.is_empty(),
        "{:#?}",
        simulation.violations
    );
    assert_eq!(simulation.gap_minutes, 0);
    let relaxed: u32 = simulation.steps[1..].iter().sum();
    assert!(relaxed > 0, "{:?}", simulation.steps);
    // Track separation is never dropped inside the slot's category.
    for pick in &simulation.picks {
        if pick.explanation.step < 11 {
            assert!(pick.explanation.windows.track_min > 0);
        }
        assert!(!pick.explanation.step_description.is_empty());
    }
}

#[test]
fn restrictions_hold_even_when_nothing_else_can_play() {
    // Every track banned at night: the night is a gap for the fallback
    // directory, never a broken restriction.
    let mut parts = station().parts;
    parts.restrictions = parts
        .categories
        .iter()
        .map(|category| Restriction {
            id: category.id,
            target: Target::Category(category.id),
            rule: Rule::Daypart {
                mode: Mode::Ban,
                days: vec![1, 2, 3, 4, 5, 6, 7],
                start_hour: 1,
                end_hour: 3,
            },
        })
        .collect();
    let library = Library::new(parts, paris());
    let simulation = simulate(&library, History::default(), None, start(), DAY_MS, 3);
    assert!(
        simulation.violations.is_empty(),
        "{:#?}",
        simulation.violations
    );
    assert!(simulation.gap_minutes >= 110, "{}", simulation.gap_minutes);
    for pick in &simulation.picks {
        let hour = library.local(pick.start_unix_ms).hour();
        assert!(!(1..3).contains(&hour));
    }
}

#[test]
fn simulations_replay_with_their_seed() {
    let library = Library::new(station().parts, paris());
    let a = simulate(&library, History::default(), None, start(), DAY_MS, 11);
    let b = simulate(&library, History::default(), None, start(), DAY_MS, 11);
    assert_eq!(a, b);
}
