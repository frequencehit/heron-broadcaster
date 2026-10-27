// SPDX-License-Identifier: AGPL-3.0-or-later

//! The player driving a mixer rendered offline, with real files.
//!
//! Files are decoded in memory, synchronously, so the tests do not depend on
//! how fast decode threads run on the test machine (they have tests of their own).

use heron_core::PlaySource;
use heron_core::playout::CrossfadeOverrides;
use tempfile::TempDir;

use super::*;
use crate::decoder::decode_to_memory;
use crate::mixer::{CHANNELS, Mixer, VecSource};

const BLOCK: usize = 4_800;

struct Station {
    library: TempDir,
    player: Player,
    mixer: Mixer,
    events: broadcast::Receiver<PlayerEvent>,
    now_playing: watch::Receiver<NowPlaying>,
}

impl Station {
    fn new() -> Self {
        Self::build(tempfile::tempdir().unwrap(), None)
    }

    /// A station that plays `fallback` (a directory) when its queue runs dry.
    fn with_fallback(fallback: &Path) -> Self {
        let library = tempfile::tempdir().unwrap();
        let fallback = Fallback::start(fallback.to_path_buf(), library.path());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while fallback.analyzed() < fallback.len() || fallback.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "fallback never analyzed"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Self::build(library, Some(fallback))
    }

    fn build(library: TempDir, fallback: Option<Fallback>) -> Self {
        let (mixer, handle) = Mixer::new(BLOCK);
        let (events_tx, events) = broadcast::channel(64);
        let (now_playing_tx, now_playing) = watch::channel(NowPlaying::default());
        let mut player = Player::new(
            library.path().to_path_buf(),
            PlayoutSettings::default(),
            handle,
            events_tx,
            now_playing_tx,
        )
        .with_source_loader(|file, cue_in_ms, cue_out_ms| {
            if file.path.ends_with("stalled.wav") {
                return Ok(Box::new(Stalled));
            }
            let audio = decode_to_memory(&file.path, cue_in_ms, cue_out_ms);
            Ok(Box::new(VecSource::new(audio)))
        });
        if let Some(fallback) = fallback {
            player = player.with_fallback(fallback);
        }
        Self {
            library,
            player,
            mixer,
            events,
            now_playing,
        }
    }

    /// A new engine over the same library, resuming the queue saved by this one.
    fn restart(self) -> Self {
        let saved = self.player.saved();
        let mut station = Self::build(self.library, None);
        station.player.restore(saved);
        station
    }

    /// Writes a 44.1 kHz stereo WAV of `seconds` of a tone into the library.
    fn file(&self, name: &str, seconds: f64) -> TrackRequest {
        self.file_at(name, seconds, 0.3)
    }

    /// Writes a WAV of `seconds` of a tone of `amplitude` (0 for silence).
    fn file_at(&self, name: &str, seconds: f64, amplitude: f64) -> TrackRequest {
        write_tone(&self.library.path().join(name), seconds, amplitude);
        let duration_ms = (seconds * 1_000.0) as u32;
        TrackRequest {
            track_id: None,
            path: name.to_owned(),
            title: Some(name.to_owned()),
            artist: Some("Tester".to_owned()),
            album: None,
            playout: TrackPlayout::unanalyzed(duration_ms),
            source: PlaySource::Manual,
            no_metadata: false,
        }
    }
}

/// A file on an unreachable share: its decoder never delivers anything.
struct Stalled;

impl AudioSource for Stalled {
    fn read(&mut self, _out: &mut [f32]) -> usize {
        0
    }

    fn is_finished(&self) -> bool {
        false
    }

    fn is_ready(&self) -> bool {
        false
    }
}

fn write_tone(path: &Path, seconds: f64, amplitude: f64) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..(seconds * 44_100.0) as u64 {
        let value = (n as f64 * 0.06).sin() * amplitude * f64::from(i16::MAX);
        writer.write_sample(value as i16).unwrap();
        writer.write_sample(value as i16).unwrap();
    }
    writer.finalize().unwrap();
}

impl Station {
    /// Renders `seconds` of audio, the player handling events between blocks.
    fn play(&mut self, seconds: f64) -> Vec<f32> {
        let mut output = Vec::new();
        let mut block = vec![0.0; BLOCK * CHANNELS];
        for _ in 0..(seconds * 10.0).round() as usize {
            self.player.tick();
            self.mixer.render(&mut block);
            output.extend_from_slice(&block);
        }
        self.player.tick();
        output
    }

    fn events(&mut self) -> Vec<PlayerEvent> {
        std::iter::from_fn(|| self.events.try_recv().ok()).collect()
    }
}

fn started_sources(events: &[PlayerEvent]) -> Vec<(String, PlaySource)> {
    events
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::Started { entry, .. } => {
                Some((entry.track.path.clone(), entry.track.source))
            }
            PlayerEvent::Ended { .. } => None,
        })
        .collect()
}

fn started(events: &[PlayerEvent]) -> Vec<(Deck, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::Started { deck, entry, .. } => Some((*deck, entry.track.path.clone())),
            PlayerEvent::Ended { .. } => None,
        })
        .collect()
}

#[test]
fn plays_the_queue_in_order_on_alternating_decks() {
    let mut station = Station::new();
    let tracks = vec![
        station.file("one.wav", 1.0),
        station.file("two.wav", 1.0),
        station.file("three.wav", 1.0),
    ];
    station.player.enqueue(tracks, None).unwrap();
    station.play(3.5);

    let events = station.events();
    assert_eq!(
        started(&events),
        [
            (Deck::A, "one.wav".to_owned()),
            (Deck::B, "two.wav".to_owned()),
            (Deck::A, "three.wav".to_owned()),
        ]
    );
    let ended = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                PlayerEvent::Ended {
                    played_ms: 1_000,
                    reason: EndReason::Finished,
                    ..
                }
            )
        })
        .count();
    assert_eq!(ended, 3);
    assert_eq!(station.player.snapshot().underruns, 0);
    assert_eq!(
        station.now_playing.borrow().title.as_deref(),
        Some("three.wav")
    );
}

#[test]
fn a_jingle_leaves_the_stream_on_the_song_before() {
    let mut station = Station::new();
    let mut jingle = station.file("jingle.wav", 1.0);
    jingle.no_metadata = true;
    let tracks = vec![
        station.file("song.wav", 1.0),
        jingle,
        station.file("next.wav", 1.0),
    ];
    station.player.enqueue(tracks, None).unwrap();
    station.play(1.5);
    // The jingle is on air; the stream still says the song.
    assert_eq!(
        station.player.snapshot().decks[1]
            .entry
            .as_ref()
            .map(|e| e.track.path.as_str()),
        Some("jingle.wav")
    );
    assert_eq!(
        station.now_playing.borrow().title.as_deref(),
        Some("song.wav")
    );
    station.play(1.0);
    assert_eq!(
        station.now_playing.borrow().title.as_deref(),
        Some("next.wav")
    );
}

#[test]
fn crossfades_at_the_mix_point() {
    let mut station = Station::new();
    let mut first = station.file("first.wav", 24.0);
    first.playout.mix_point_ms = Some(22_000);
    let second = station.file("second.wav", 24.0);
    station.player.enqueue(vec![first, second], None).unwrap();
    station.play(23.0);

    let snapshot = station.player.snapshot();
    let (a, b) = (&snapshot.decks[0], &snapshot.decks[1]);
    // The second track started at 22 s, 1 s ago, on Deck B; the first fades out.
    assert_eq!(b.position_ms, 1_000);
    assert!(b.on_air && !a.on_air);
    assert_eq!(a.position_ms, 23_000);
    assert_eq!(
        started(&station.events()),
        [
            (Deck::A, "first.wav".to_owned()),
            (Deck::B, "second.wav".to_owned()),
        ]
    );
}

#[test]
fn per_track_overrides_disable_the_crossfade() {
    let mut station = Station::new();
    let mut first = station.file("first.wav", 24.0);
    first.playout.mix_point_ms = Some(22_000);
    first.playout.crossfade = CrossfadeOverrides {
        crossfade: Some(false),
        ..CrossfadeOverrides::default()
    };
    let second = station.file("second.wav", 24.0);
    station.player.enqueue(vec![first, second], None).unwrap();
    station.play(23.0);
    let snapshot = station.player.snapshot();
    assert_eq!(snapshot.decks[1].entry, None);
    assert!(snapshot.decks[0].on_air);
}

#[test]
fn applies_the_loudness_gain() {
    let mut station = Station::new();
    let mut track = station.file("loud.wav", 1.0);
    track.playout.integrated_lufs = Some(-10.0);
    station.player.enqueue(vec![track], None).unwrap();
    let audio = station.play(0.5);
    // -16 LUFS target: 6 dB down from a 0.3 sine.
    let peak = audio.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!((peak - 0.3 * 0.501).abs() < 0.01, "peak {peak}");
}

#[test]
fn skip_starts_the_next_track() {
    let mut station = Station::new();
    let tracks = vec![station.file("one.wav", 5.0), station.file("two.wav", 5.0)];
    station.player.enqueue(tracks, None).unwrap();
    station.play(1.0);
    station.player.skip().unwrap();
    station.play(1.5);
    let events = station.events();
    assert!(events.iter().any(|event| matches!(
        event,
        PlayerEvent::Ended {
            reason: EndReason::Skipped,
            played_ms: 2_000,
            ..
        }
    )));
    assert_eq!(started(&events).len(), 2);
}

#[test]
fn queue_changes_reach_the_cued_track() {
    let mut station = Station::new();
    let tracks = vec![station.file("one.wav", 2.0), station.file("two.wav", 1.0)];
    let ids = station.player.enqueue(tracks, None).unwrap();
    station.play(0.5);
    assert_eq!(
        station.player.snapshot().cued.map(|entry| entry.entry_id),
        Some(ids[1])
    );

    // A track inserted first takes the place of the cued one.
    let urgent = station.file("urgent.wav", 1.0);
    station.player.enqueue(vec![urgent], Some(0)).unwrap();
    assert!(matches!(
        station.player.remove(ids[0]),
        Err(PlayerError::OnAir(_))
    ));
    station.play(3.0);
    let order: Vec<String> = started(&station.events())
        .into_iter()
        .map(|(_, path)| path)
        .collect();
    assert_eq!(order, ["one.wav", "urgent.wav", "two.wav"]);
}

#[test]
fn unplayable_files_are_skipped() {
    let mut station = Station::new();
    std::fs::write(station.library.path().join("broken.mp3"), b"not audio").unwrap();
    let broken = TrackRequest {
        path: "broken.mp3".to_owned(),
        ..station.file("unused.wav", 0.1)
    };
    let tracks = vec![
        station.file("one.wav", 1.0),
        broken,
        station.file("two.wav", 1.0),
    ];
    station.player.enqueue(tracks, None).unwrap();
    station.play(2.5);
    let order: Vec<String> = started(&station.events())
        .into_iter()
        .map(|(_, path)| path)
        .collect();
    assert_eq!(order, ["one.wav", "two.wav"]);
}

#[test]
fn paths_cannot_leave_the_library() {
    let root = Path::new("/music");
    assert_eq!(
        resolve_library_path(root, "Jazz/So What.flac"),
        Some(PathBuf::from("/music/Jazz/So What.flac"))
    );
    for path in [
        "../etc/passwd",
        "/etc/passwd",
        "a//b",
        "a/./b",
        "",
        "a\\..\\b",
    ] {
        assert_eq!(resolve_library_path(root, path), None, "{path}");
    }
    let mut station = Station::new();
    let mut track = station.file("one.wav", 0.1);
    track.path = "../one.wav".to_owned();
    assert!(matches!(
        station.player.enqueue(vec![track], None),
        Err(PlayerError::InvalidPath(_))
    ));
}

#[test]
fn fx_plays_over_the_music() {
    let mut station = Station::new();
    let music = station.file("music.wav", 2.0);
    let jingle = station.file("jingle.wav", 0.5);
    station.player.enqueue(vec![music], None).unwrap();
    station.play(0.3);
    station.player.play_fx(jingle).unwrap();
    station.play(1.0);
    let events = station.events();
    assert!(started(&events).contains(&(Deck::Fx, "jingle.wav".to_owned())));
    // The FX player does not change what is on air.
    assert_eq!(
        station.now_playing.borrow().title.as_deref(),
        Some("music.wav")
    );
}

#[test]
fn queue_entries_move() {
    let mut station = Station::new();
    let tracks = vec![
        station.file("one.wav", 1.0),
        station.file("two.wav", 0.5),
        station.file("three.wav", 0.5),
        station.file("four.wav", 0.5),
    ];
    let ids = station.player.enqueue(tracks, None).unwrap();
    station.play(0.2);
    assert!(matches!(
        station.player.move_entry(ids[0], 1),
        Err(PlayerError::OnAir(_))
    ));
    // The cued track goes last, and the last one is played next.
    station.player.move_entry(ids[1], 9).unwrap();
    station.player.move_entry(ids[3], 0).unwrap();
    // Moving the cued track to the front changes nothing.
    station.play(0.1);
    station.player.move_entry(ids[3], 0).unwrap();
    assert!(matches!(
        station.player.move_entry(999, 0),
        Err(PlayerError::NotFound(999))
    ));
    station.play(3.0);
    let order: Vec<String> = started(&station.events())
        .into_iter()
        .map(|(_, path)| path)
        .collect();
    assert_eq!(order, ["one.wav", "four.wav", "three.wav", "two.wav"]);
}

#[test]
fn waiting_entries_take_a_new_track() {
    let mut station = Station::new();
    let tracks = vec![
        station.file("one.wav", 1.0),
        station.file("two.wav", 0.5),
        station.file("three.wav", 0.5),
    ];
    let ids = station.player.enqueue(tracks, None).unwrap();
    station.play(0.2);
    // The cued entry and a queued one, as if their files had moved.
    let moved = station.file("moved two.wav", 0.5);
    station.player.replace(ids[1], moved).unwrap();
    let moved = station.file("moved three.wav", 0.5);
    station.player.replace(ids[2], moved).unwrap();
    let on_air = station.file("unused.wav", 0.5);
    assert!(matches!(
        station.player.replace(ids[0], on_air),
        Err(PlayerError::OnAir(_))
    ));
    station.play(3.0);
    let events = station.events();
    let order: Vec<(u64, String)> = events
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::Started { entry, .. } => Some((entry.entry_id, entry.track.path.clone())),
            PlayerEvent::Ended { .. } => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            (ids[0], "one.wav".to_owned()),
            (ids[1], "moved two.wav".to_owned()),
            (ids[2], "moved three.wav".to_owned()),
        ]
    );
    let ended = events
        .iter()
        .filter(|event| matches!(event, PlayerEvent::Ended { .. }))
        .count();
    assert_eq!(ended, 3, "each entry plays once: {events:?}");
}

#[test]
fn positions_count_the_cued_track() {
    let mut station = Station::new();
    let tracks = vec![
        station.file("one.wav", 1.0),
        station.file("two.wav", 0.5),
        station.file("three.wav", 0.5),
    ];
    station.player.enqueue(tracks, None).unwrap();
    station.play(0.2);
    // Waiting: two (cued), three. Position 1 is right after the cued track.
    let inserted = station.file("inserted.wav", 0.5);
    station.player.enqueue(vec![inserted], Some(1)).unwrap();
    let waiting: Vec<String> = {
        let snapshot = station.player.snapshot();
        snapshot
            .cued
            .into_iter()
            .chain(snapshot.queue)
            .map(|entry| entry.track.path)
            .collect()
    };
    assert_eq!(waiting, ["two.wav", "inserted.wav", "three.wav"]);
}

#[test]
fn the_fallback_directory_plays_when_the_queue_runs_dry() {
    let fallback = tempfile::tempdir().unwrap();
    write_tone(&fallback.path().join("filler.wav"), 2.0, 0.3);
    let mut station = Station::with_fallback(fallback.path());
    let one = station.file("one.wav", 1.0);
    station.player.enqueue(vec![one], None).unwrap();
    station.play(1.5);
    assert!(!station.player.dead_air());

    // Queued while a fallback track is cued: it goes first.
    let two = station.file("two.wav", 1.0);
    station.player.enqueue(vec![two], None).unwrap();
    station.play(3.0);
    let sources = started_sources(&station.events());
    assert_eq!(
        sources,
        [
            ("one.wav".to_owned(), PlaySource::Manual),
            ("filler.wav".to_owned(), PlaySource::Fallback),
            ("two.wav".to_owned(), PlaySource::Manual),
            ("filler.wav".to_owned(), PlaySource::Fallback),
        ]
    );
    // Never saved: a restart picks a new one.
    assert!(
        station
            .player
            .saved()
            .waiting
            .iter()
            .all(|entry| entry.track.source != PlaySource::Fallback)
    );
}

#[test]
fn dead_air_without_anything_to_play() {
    let mut station = Station::new();
    station.play(1.0);
    assert!(!station.player.dead_air(), "not before 5 s");
    station.play(5.0);
    assert!(station.player.dead_air());
    let one = station.file("one.wav", 1.0);
    station.player.enqueue(vec![one], None).unwrap();
    station.play(0.2);
    assert!(!station.player.dead_air());
}

#[test]
fn the_silence_watchdog_skips_then_goes_to_the_fallback_directory() {
    let fallback = tempfile::tempdir().unwrap();
    write_tone(&fallback.path().join("filler.wav"), 2.0, 0.3);
    let mut station = Station::with_fallback(fallback.path());
    let mut settings = PlayoutSettings::default();
    settings.silence_watchdog.timeout_ms = 2_000;
    station.player.set_settings(settings);
    let tracks = vec![
        station.file_at("silent one.wav", 30.0, 0.0),
        station.file_at("silent two.wav", 30.0, 0.0),
        station.file("music.wav", 1.0),
    ];
    station.player.enqueue(tracks, None).unwrap();
    station.play(8.0);
    let events = station.events();
    let paths: Vec<String> = started_sources(&events)
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        paths[..4],
        [
            "silent one.wav",
            "silent two.wav",
            "filler.wav",
            "music.wav"
        ]
    );
    let skipped = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                PlayerEvent::Ended {
                    reason: EndReason::Skipped,
                    ..
                }
            )
        })
        .count();
    assert_eq!(skipped, 2);
}

#[test]
fn the_silence_watchdog_ignores_pauses() {
    let mut station = Station::new();
    let mut settings = PlayoutSettings::default();
    settings.silence_watchdog.timeout_ms = 1_000;
    station.player.set_settings(settings);
    let music = station.file("music.wav", 10.0);
    station.player.enqueue(vec![music], None).unwrap();
    station.play(0.5);
    station.player.pause().unwrap();
    station.play(3.0);
    station.player.resume().unwrap();
    station.play(0.5);
    assert!(
        !station
            .events()
            .iter()
            .any(|event| matches!(event, PlayerEvent::Ended { .. }))
    );
}

#[test]
fn a_restart_resumes_the_track_on_air_and_the_queue() {
    let mut station = Station::new();
    let tracks = vec![
        station.file("one.wav", 10.0),
        station.file("two.wav", 1.0),
        station.file("three.wav", 1.0),
    ];
    let ids = station.player.enqueue(tracks, None).unwrap();
    station.play(3.0);
    let saved = station.player.saved();
    let on_air = saved.on_air.clone().unwrap();
    assert_eq!(on_air.entry.entry_id, ids[0]);
    assert_eq!(on_air.position_ms, 3_000);
    let waiting: Vec<u64> = saved.waiting.iter().map(|entry| entry.entry_id).collect();
    assert_eq!(waiting, ids[1..]);

    let mut station = station.restart();
    station.play(1.0);
    let snapshot = station.player.snapshot();
    let deck = snapshot.decks.iter().find(|deck| deck.on_air).unwrap();
    assert_eq!(deck.entry.as_ref().unwrap().entry_id, ids[0]);
    assert!(
        deck.position_ms.abs_diff(4_000) <= 100,
        "{}",
        deck.position_ms
    );
    station.play(8.0);
    let order: Vec<(u64, String)> = station
        .events()
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::Started { entry, .. } => Some((entry.entry_id, entry.track.path.clone())),
            PlayerEvent::Ended { .. } => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            (ids[0], "one.wav".to_owned()),
            (ids[1], "two.wav".to_owned()),
            (ids[2], "three.wav".to_owned()),
        ]
    );
    // New entries do not reuse the IDs of the saved ones.
    let four = station.file("four.wav", 1.0);
    assert!(station.player.enqueue(vec![four], None).unwrap()[0] > ids[2]);
}

#[test]
fn a_track_that_cannot_start_gives_way_to_the_fallback_directory() {
    let fallback = tempfile::tempdir().unwrap();
    write_tone(&fallback.path().join("filler.wav"), 2.0, 0.3);
    let mut station = Station::with_fallback(fallback.path());
    let tracks = vec![
        station.file("stalled.wav", 30.0),
        station.file("music.wav", 1.0),
    ];
    station.player.enqueue(tracks, None).unwrap();
    station.play(9.0);
    assert!(started(&station.events()).is_empty(), "still waiting");
    station.play(4.0);
    let paths: Vec<String> = started_sources(&station.events())
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(paths[..2], ["filler.wav", "music.wav"]);
    // Dropped, not retried.
    assert!(
        station
            .player
            .snapshot()
            .queue
            .iter()
            .all(|entry| entry.track.path != "stalled.wav")
    );
}

#[test]
fn a_track_that_just_started_is_saved_once() {
    let mut station = Station::new();
    let tracks = vec![
        station.file("one.wav", 1.0),
        station.file("two.wav", 2.0),
        station.file("three.wav", 1.0),
    ];
    let ids = station.player.enqueue(tracks, None).unwrap();
    station.play(0.5);
    // The audio thread starts "two" before the player hears of it.
    let mut block = vec![0.0; BLOCK * CHANNELS];
    for _ in 0..8 {
        station.mixer.render(&mut block);
    }
    let saved = station.player.saved();
    assert_eq!(saved.on_air.unwrap().entry.entry_id, ids[1]);
    let waiting: Vec<u64> = saved.waiting.iter().map(|entry| entry.entry_id).collect();
    assert_eq!(waiting, [ids[2]]);
}
