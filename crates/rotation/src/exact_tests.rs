// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exact times: landing in the window, the gaps before it, the cuts.

use std::collections::HashMap;

use jiff::tz::TimeZone;

use crate::{
    Category, CategoryId, Clock, ExactMode, ExactTime, FillKind, History, Library, LibraryParts,
    Medley, MedleyCut, Member, Met, Method, Pick, Play, Position, Rng, RotationSettings, Schedule,
    SeparationOverrides, SkipReason, Slot, Track, TrackId, next, simulate,
};

const SEC: i64 = 1_000;
const MIN: i64 = 60 * SEC;
const HOUR: i64 = 60 * MIN;

const SONGS: CategoryId = 1;
const NEWS: CategoryId = 2;
const FILLER: CategoryId = 3;
const MEDLEYS: CategoryId = 4;
const JINGLES: CategoryId = 5;

fn track(id: TrackId, length_ms: i64) -> Track {
    Track {
        id,
        title_key: Some(format!("title {id}")),
        album_key: Some(format!("album {id}")),
        artists: vec![100 + id],
        categories: vec![],
        length_ms: u32::try_from(length_ms).unwrap(),
        mix_ms: None,
        ratio: 10,
        dayparts: None,
        daypart_days: None,
    }
}

fn category(id: CategoryId, tracks: &[TrackId]) -> Category {
    Category {
        id,
        name: format!("C{id}"),
        // The whole category drawn: only the lengths decide.
        separation: SeparationOverrides {
            draw_start_pct: Some(100),
            draw_max_pct: Some(100),
            ..SeparationOverrides::default()
        },
        members: tracks
            .iter()
            .map(|&track| Member { track, weight: 1.0 })
            .collect(),
    }
}

fn slot(category: CategoryId) -> Slot {
    Slot {
        category,
        method: Method::Random,
        enforce_rules: true,
        separation: SeparationOverrides::default(),
        linked_to_next: false,
        exact: None,
    }
}

fn exact(at_s: u32, mode: ExactMode) -> ExactTime {
    ExactTime {
        at_s,
        mode,
        before_s: 10,
        after_s: 30,
        medley_fill: false,
        cut_live: false,
    }
}

fn with_exact(category: CategoryId, exact: ExactTime) -> Slot {
    Slot {
        exact: Some(exact),
        ..slot(category)
    }
}

/// Songs of the given lengths (IDs from 1), a news flash (50), filler
/// jingles (60, 61), a 46 s medley (70) and a jingle (80).
fn library(slots: Vec<Slot>, songs: &[i64]) -> Library {
    let mut tracks: Vec<Track> = songs
        .iter()
        .enumerate()
        .map(|(index, length)| track(index as u64 + 1, *length))
        .collect();
    let song_ids: Vec<TrackId> = tracks.iter().map(|t| t.id).collect();
    tracks.extend([
        track(50, 120 * SEC),
        track(60, 4 * SEC),
        track(61, 4 * SEC),
        track(70, 46 * SEC),
        track(80, 5 * SEC),
    ]);
    Library::new(
        LibraryParts {
            tracks,
            categories: vec![
                category(SONGS, &song_ids),
                category(NEWS, &[50]),
                category(FILLER, &[60, 61]),
                category(MEDLEYS, &[70]),
                category(JINGLES, &[80]),
            ],
            clocks: vec![Clock {
                id: 1,
                name: "Clock".into(),
                looping: false,
                slots,
            }],
            schedule: Schedule {
                hours: HashMap::from_iter((0..24).map(|h| ((4, h), 1))),
                ..Schedule::default()
            },
            medley_category: Some(MEDLEYS),
            medleys: vec![(
                70,
                Medley {
                    id: 7,
                    // Its last liner + hook pair left out: 46 s to 31 s.
                    cuts: vec![MedleyCut {
                        hooks_left_out: 1,
                        parts_left_out: vec![4, 5],
                        length_ms: 31_000,
                        first_to_ms: 25_000,
                        second_from_ms: 40_000,
                        crossfade_ms: 500,
                    }],
                },
            )],
            ..LibraryParts::default()
        },
        TimeZone::UTC,
    )
}

fn at(slot: u32, hour: i64) -> Position {
    Position {
        hour_start_unix_ms: hour * HOUR,
        clock_id: 1,
        slot,
    }
}

fn pick(library: &Library, history: &History, at_ms: i64, last: Position, seed: u64) -> Pick {
    next(library, history, at_ms, Some(&last), &mut Rng::new(seed)).unwrap()
}

/// The top of the hour as the next hour's first slot: news, then songs.
fn top_of_hour(mode: ExactMode, songs: &[i64]) -> Library {
    library(vec![with_exact(NEWS, exact(0, mode)), slot(SONGS)], songs)
}

#[test]
fn before_an_exact_time_tracks_landing_in_the_window_are_preferred() {
    // At 57:00, 175 s ends at 59:55, inside 59:50 to 00:30.
    let library = top_of_hour(ExactMode::Soft, &[100_000, 175_000, 300_000]);
    for seed in 0..20 {
        let pick = pick(&library, &History::default(), 57 * MIN, at(0, 0), seed);
        assert_eq!(pick.track, 2, "seed {seed}");
    }
    // Far from it, every track ends before the window: all are drawn.
    let picked: std::collections::HashSet<TrackId> = (0..40)
        .map(|seed| pick(&library, &History::default(), 10 * MIN, at(0, 0), seed).track)
        .collect();
    assert_eq!(picked.len(), 3);
}

#[test]
fn tracks_ending_before_the_window_come_before_those_ending_after_it() {
    // At 55:00 nothing lands: 100 s ends early, 400 s late.
    let library = top_of_hour(ExactMode::Soft, &[100_000, 400_000]);
    for seed in 0..20 {
        assert_eq!(
            pick(&library, &History::default(), 55 * MIN, at(0, 0), seed).track,
            1
        );
    }
}

#[test]
fn a_soft_slot_that_no_longer_fits_gives_way_to_the_filler() {
    let mut library = top_of_hour(ExactMode::Soft, &[120_000, 180_000]);
    library.settings.filler_category_id = Some(FILLER);
    let pick = pick(
        &library,
        &History::default(),
        59 * MIN + 40 * SEC,
        at(1, 0),
        1,
    );
    assert!([60, 61].contains(&pick.track));
    let fill = pick.explanation.fill.as_ref().unwrap();
    assert_eq!(fill.kind, FillKind::Filler);
    assert_eq!(fill.exact_at_unix_ms, HOUR);
    assert!(pick.position.is_none(), "the clock does not move");
    assert!(
        pick.explanation
            .skipped_slots
            .iter()
            .any(|skipped| skipped.slot == 1 && skipped.reason == SkipReason::NoLongerFits)
    );
}

#[test]
fn without_a_filler_a_soft_slot_plays_on_and_is_faded_at_the_end_of_the_window() {
    let library = top_of_hour(ExactMode::Soft, &[120_000, 180_000]);
    let first = pick(&library, &History::default(), 59 * MIN, at(1, 0), 1);
    // Nothing fits: the song plays, and the news follows at 00:30, faded.
    assert_eq!(library.categories[&SONGS].members.len(), 2);
    assert!(first.explanation.fill.is_none() && first.explanation.exact.is_none());
    let song = &library.tracks[&first.track];
    let history = History::new(vec![Play::of(song, 59 * MIN)], HashMap::new());
    let end = 59 * MIN + i64::from(song.length_ms);
    let news = pick(&library, &history, end, first.position.unwrap(), 1);
    assert_eq!(news.track, 50);
    let log = news.explanation.exact.unwrap();
    assert_eq!(news.explanation.at_unix_ms, HOUR + 30 * SEC);
    assert_eq!(news.start_at_ms, Some(HOUR + 30 * SEC));
    assert!(log.faded && log.in_window);
    assert_eq!((log.met, log.offset_ms), (Met::Late, 30 * SEC));
}

#[test]
fn a_track_ending_inside_the_window_plays_out() {
    let library = top_of_hour(ExactMode::Soft, &[200_000]);
    let song = &library.tracks[&1];
    let history = History::new(vec![Play::of(song, 57 * MIN)], HashMap::new());
    // It ends at 00:20, inside the window: the news follows it.
    let news = pick(&library, &history, HOUR + 20 * SEC, at(1, 0), 1);
    let log = news.explanation.exact.unwrap();
    assert_eq!(news.explanation.at_unix_ms, HOUR + 20 * SEC);
    assert!(!log.faded && log.in_window);
    assert_eq!((log.met, log.offset_ms), (Met::Late, 20 * SEC));
    // The engine still fades out at the end of the window, if it must.
    assert_eq!(news.start_at_ms, Some(HOUR + 30 * SEC));
}

#[test]
fn hard_cuts_what_plays_at_the_exact_time() {
    let library = top_of_hour(ExactMode::Hard, &[300_000]);
    let song = &library.tracks[&1];
    let history = History::new(vec![Play::of(song, 58 * MIN)], HashMap::new());
    let news = pick(&library, &history, 63 * MIN, at(1, 0), 1);
    let log = news.explanation.exact.unwrap();
    assert_eq!(news.explanation.at_unix_ms, HOUR);
    assert_eq!(news.start_at_ms, Some(HOUR));
    assert!(log.faded && log.in_window);
    assert_eq!((log.met, log.before_s, log.after_s), (Met::OnTime, 0, 0));
    // In hard mode a track crossing the exact time still plays: it is cut.
    let first = pick(&library, &History::default(), 58 * MIN, at(1, 0), 1);
    assert_eq!(first.track, 1);
}

#[test]
fn the_gap_before_the_window_is_filled_with_a_medley_whole_or_shortened() {
    let soft = ExactTime {
        before_s: 10,
        after_s: 5,
        medley_fill: true,
        ..exact(0, ExactMode::Soft)
    };
    let library = library(vec![with_exact(NEWS, soft), slot(SONGS)], &[120_000]);
    // At 59:15, the whole medley (46 s) ends at 00:01: inside 59:50 to 00:05.
    let whole = pick(
        &library,
        &History::default(),
        59 * MIN + 15 * SEC,
        at(1, 0),
        1,
    );
    assert_eq!(whole.track, 70);
    assert!(whole.medley_cut.is_none());
    let fill = whole.explanation.fill.as_ref().unwrap();
    assert_eq!(
        (fill.kind, fill.medley_id, fill.hooks_left_out),
        (FillKind::Medley, Some(7), 0)
    );
    assert_eq!(fill.length_ms, 46_000);
    // At 59:30 it would end at 00:16: its last liner + hook pair is left
    // out, 31 s, ending at 00:01.
    let short = pick(
        &library,
        &History::default(),
        59 * MIN + 30 * SEC,
        at(1, 0),
        1,
    );
    assert_eq!(short.track, 70);
    let cut = short.medley_cut.as_ref().unwrap();
    assert_eq!(cut.length_ms, 31_000);
    assert_eq!(short.length_ms(&library), 31_000);
    let fill = short.explanation.fill.as_ref().unwrap();
    assert_eq!(
        (fill.hooks_left_out, fill.parts_left_out.clone()),
        (1, vec![4, 5])
    );
    // At 59:00 neither lands, and there is no filler: the song plays on.
    let none = pick(&library, &History::default(), 59 * MIN, at(1, 0), 1);
    assert_eq!(none.track, 1);
    // Medleys turned off: no medley.
    let mut off = library.clone();
    off.modules_off.insert(MEDLEYS);
    let off_pick = pick(&off, &History::default(), 59 * MIN + 15 * SEC, at(1, 0), 1);
    assert_ne!(off_pick.track, 70);
}

#[test]
fn the_medley_fill_follows_the_usual_rules() {
    let soft = ExactTime {
        medley_fill: true,
        ..exact(0, ExactMode::Soft)
    };
    let library = library(vec![with_exact(NEWS, soft), slot(SONGS)], &[120_000]);
    // The medley played ten minutes ago: track separation holds it back.
    let medley = &library.tracks[&70];
    let history = History::new(vec![Play::of(medley, 49 * MIN)], HashMap::new());
    let pick = pick(&library, &history, 59 * MIN + 15 * SEC, at(1, 0), 1);
    assert_ne!(pick.track, 70);
}

#[test]
fn an_exact_slot_reached_before_its_window_starts_early_without_a_fill() {
    // The flash at :30:00 is the second slot; the first ran out at 29:00.
    let library = library(
        vec![
            slot(SONGS),
            with_exact(NEWS, exact(30 * 60, ExactMode::Soft)),
            slot(SONGS),
        ],
        &[100_000],
    );
    let pick = pick(&library, &History::default(), 29 * MIN, at(0, 0), 1);
    assert_eq!(pick.track, 50);
    let log = pick.explanation.exact.unwrap();
    assert_eq!(
        (log.met, log.offset_ms, log.in_window),
        (Met::Early, -MIN, false)
    );
    // With a filler category, a filler goes first.
    let mut filled = library.clone();
    filled.settings.filler_category_id = Some(FILLER);
    let first = next(
        &filled,
        &History::default(),
        29 * MIN,
        Some(&at(0, 0)),
        &mut Rng::new(1),
    );
    assert!(first.unwrap().explanation.fill.is_some());
}

#[test]
fn the_clock_jumps_to_a_due_exact_slot() {
    let library = library(
        vec![
            slot(SONGS),
            slot(SONGS),
            with_exact(NEWS, exact(30 * 60, ExactMode::Soft)),
            slot(SONGS),
        ],
        &[100_000],
    );
    // At 29:55 after slot 0: slot 1 is passed over, the flash starts.
    let pick = pick(
        &library,
        &History::default(),
        29 * MIN + 55 * SEC,
        at(0, 0),
        1,
    );
    assert_eq!(pick.track, 50);
    assert_eq!(pick.position.unwrap().slot, 2);
    assert_eq!(
        pick.explanation.skipped_slots[0].reason,
        SkipReason::ExactTimeDue
    );
    assert_eq!(pick.explanation.exact.unwrap().met, Met::Early);
    // Its time passed in this run: it is passed over, never played late.
    let later = next(
        &library,
        &History::default(),
        40 * MIN,
        Some(&at(1, 0)),
        &mut Rng::new(1),
    )
    .unwrap();
    assert_eq!(later.position.unwrap().slot, 3);
    assert_eq!(
        later.explanation.skipped_slots[0].reason,
        SkipReason::ExactTimePassed
    );
}

#[test]
fn a_linked_slot_starts_with_its_exact_slot() {
    let mut jingle = slot(JINGLES);
    jingle.linked_to_next = true;
    let library = library(
        vec![
            slot(SONGS),
            jingle,
            with_exact(NEWS, exact(30 * 60, ExactMode::Hard)),
        ],
        &[100_000],
    );
    let song = &library.tracks[&1];
    let history = History::new(vec![Play::of(song, 28 * MIN)], HashMap::new());
    let pick = pick(&library, &history, 30 * MIN + 10 * SEC, at(0, 0), 1);
    assert_eq!(pick.track, 80);
    assert_eq!(pick.explanation.at_unix_ms, 30 * MIN);
    assert_eq!(pick.start_at_ms, Some(30 * MIN));
    let news = pick.linked.as_deref().unwrap();
    assert_eq!(
        (news.track, news.explanation.at_unix_ms),
        (50, 30 * MIN + 5 * SEC)
    );
}

#[test]
fn simulations_meet_the_exact_times_and_audit_them() {
    let soft = ExactTime {
        medley_fill: true,
        ..exact(0, ExactMode::Soft)
    };
    let mut library = library(
        vec![
            with_exact(NEWS, soft),
            slot(SONGS),
            slot(SONGS),
            slot(SONGS),
        ],
        &(0..40).map(|i| 150_000 + i * 7_000).collect::<Vec<_>>(),
    );
    library.settings = RotationSettings {
        filler_category_id: Some(FILLER),
        ..library.settings.clone()
    };
    // The one news flash plays every hour: its separation allows it.
    if let Some(news) = library.categories.get_mut(&NEWS) {
        news.separation.track_min = Some(30);
        news.separation.title_min = Some(30);
        news.separation.artist_min = Some(30);
        news.separation.album_min = Some(30);
    }
    let simulation = simulate(&library, History::default(), None, 0, 6 * HOUR, 3);
    let news: Vec<_> = simulation
        .picks
        .iter()
        .filter(|pick| pick.track_id == 50)
        .collect();
    // One every hour; the one of 6:00 too when it starts early in its
    // window, before the simulation ends.
    assert!((6..=7).contains(&news.len()), "{}", news.len());
    for pick in &news {
        let into_hour = pick.start_unix_ms.rem_euclid(HOUR);
        assert!(
            into_hour <= 30 * SEC || into_hour >= HOUR - 10 * SEC,
            "{into_hour}"
        );
    }
    assert!(
        simulation.violations.is_empty(),
        "{:?}",
        simulation.violations
    );
}

#[test]
fn the_audit_flags_a_missed_window() {
    let library = library(
        vec![
            slot(SONGS),
            with_exact(NEWS, exact(30 * 60, ExactMode::Soft)),
            slot(SONGS),
        ],
        &[100_000],
    );
    let simulation = simulate(
        &library,
        History::default(),
        Some(at(0, 0)),
        29 * MIN,
        MIN,
        1,
    );
    assert!(
        simulation
            .violations
            .iter()
            .any(|violation| matches!(violation, crate::Violation::ExactTime { .. }))
    );
}

#[test]
fn the_repeated_hour_runs_its_exact_times_again() {
    let zone = TimeZone::get("Europe/Paris").unwrap();
    let songs: Vec<i64> = (0..40).map(|i| 150_000 + i * 7_000).collect();
    let mut library = top_of_hour(ExactMode::Hard, &songs);
    library.time_zone = zone;
    library.schedule.hours =
        HashMap::from_iter((1..=7).flat_map(|d| (0..24).map(move |h| ((d, h), 1))));
    // 2026-10-25 00:00 UTC is 02:00 summer time; 01:00 UTC is 02:00 winter time.
    let start = jiff::Timestamp::from_second(1_792_886_400)
        .unwrap()
        .as_millisecond();
    let simulation = simulate(&library, History::default(), None, start, 3 * HOUR, 5);
    let news: Vec<i64> = simulation
        .picks
        .iter()
        .filter(|pick| pick.track_id == 50)
        .map(|pick| pick.start_unix_ms)
        .collect();
    // 02:00 summer, 02:00 winter, 03:00: one each real hour.
    assert_eq!(news, vec![start, start + HOUR, start + 2 * HOUR]);
    assert!(
        simulation.violations.is_empty(),
        "{:?}",
        simulation.violations
    );
}

#[test]
fn after_its_exact_slot_the_clock_goes_on_with_the_next_slot() {
    // The news at the top of the hour, started at 00:20 inside its window:
    // the songs follow it, the news is not due again (#157).
    for mode in [ExactMode::Soft, ExactMode::Hard] {
        let library = top_of_hour(mode, &[180_000, 200_000]);
        let news = &library.tracks[&50];
        let start = HOUR + 20 * SEC;
        let history = History::new(vec![Play::of(news, start)], HashMap::new());
        let end = start + i64::from(news.length_ms);
        for seed in 0..10 {
            let next = pick(&library, &history, end, at(0, 1), seed);
            assert_ne!(next.track, 50, "{mode:?}, seed {seed}");
            assert!(next.explanation.exact.is_none());
            assert_eq!(next.position.map(|position| position.slot), Some(1));
        }
    }
}

#[test]
fn after_its_exact_slot_a_looping_clock_does_not_run_it_again_this_hour() {
    let mut library = top_of_hour(ExactMode::Soft, &[180_000, 200_000]);
    for clock in library.clocks.values_mut() {
        clock.looping = true;
    }
    let news = &library.tracks[&50];
    let start = HOUR + 20 * SEC;
    let history = History::new(vec![Play::of(news, start)], HashMap::new());
    let next = pick(
        &library,
        &history,
        start + i64::from(news.length_ms),
        at(0, 1),
        1,
    );
    assert_ne!(next.track, 50);
}

#[test]
fn a_hard_exact_time_draws_wider_before_cutting_a_track() {
    // The three songs played longest ago would cross the exact time; the
    // others end before it.
    let mut lengths = vec![300 * SEC; 3];
    lengths.extend([60 * SEC; 7]);
    let mut library = library(
        vec![
            with_exact(NEWS, exact(0, ExactMode::Hard)),
            slot(SONGS),
            Slot {
                method: Method::LeastRecentlyPlayed,
                ..slot(SONGS)
            },
        ],
        &lengths,
    );
    if let Some(songs) = library.categories.get_mut(&SONGS) {
        songs.separation.draw_start_pct = Some(30);
        songs.separation.draw_max_pct = Some(100);
    }
    let history = History::new(
        Vec::new(),
        (1..=10)
            .map(|id| (id, if id <= 3 { -20 * HOUR } else { -10 * HOUR }))
            .collect(),
    );
    for seed in 0..10 {
        let pick = crate::select::outcome(
            &library,
            &history,
            58 * MIN + 30 * SEC,
            Some(&at(1, 0)),
            &mut Rng::new(seed),
            false,
        )
        .pick
        .unwrap();
        assert!(pick.track > 3, "{}", pick.track);
    }
}
