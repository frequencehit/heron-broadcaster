// SPDX-License-Identifier: AGPL-3.0-or-later

//! Never off air (#221), as the engine alone sees it: with the API gone,
//! the queue plays out and the fallback directory takes over; without a
//! playable fallback track, `/healthz` warns before the queue runs dry.

#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use heron_core::PlaySource;
use heron_core::playout::TrackPlayout;
use heron_engine::engine::{Engine, EngineOptions};
use heron_engine::player::TrackRequest;

fn write_tone(path: &Path, seconds: f64) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..(seconds * 44_100.0) as u64 {
        let value = ((n as f64 * 0.06).sin() * 0.3 * f64::from(i16::MAX)) as i16;
        writer.write_sample(value).unwrap();
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
}

fn track(library: &Path, name: &str, seconds: f64) -> TrackRequest {
    write_tone(&library.join(name), seconds);
    TrackRequest {
        track_id: None,
        path: name.to_owned(),
        title: Some(name.to_owned()),
        artist: Some("Tester".to_owned()),
        album: None,
        playout: TrackPlayout::unanalyzed((seconds * 1_000.0) as u32),
        source: PlaySource::Manual,
        no_metadata: false,
        over_intro: false,
        intro_ms: None,
        start_at: None,
        continues: false,
    }
}

fn engine(library: &Path, state: &Path, fallback: Option<&Path>) -> Arc<Engine> {
    Engine::start(
        EngineOptions {
            fallback_dir: fallback.map(Path::to_path_buf),
            ..EngineOptions::new(library.to_path_buf(), state.to_path_buf())
        },
        tokio::runtime::Handle::current(),
    )
    .unwrap()
}

/// The source of the track on air, if any.
fn on_air(engine: &Engine) -> Option<PlaySource> {
    engine
        .player()
        .snapshot()
        .decks
        .into_iter()
        .find(|deck| deck.on_air)
        .and_then(|deck| deck.entry)
        .map(|entry| entry.track.source)
}

async fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Real time: one test at a time, or they starve each other of CPU.
static REAL_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread")]
async fn without_the_api_the_queue_plays_out_then_the_fallback_directory() {
    let _real_time = REAL_TIME.lock().await;
    let library = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    write_tone(&fallback.path().join("station id.wav"), 4.0);
    let engine = engine(library.path(), state.path(), Some(fallback.path()));
    wait_for(
        "the fallback directory scanned",
        Duration::from_secs(10),
        || engine.player().fallback_playable() == Some(1),
    )
    .await;
    assert!(
        !engine.warnings().iter().any(|w| w.contains("fallback")),
        "{:?}",
        engine.warnings()
    );

    // What the API queued before it stopped: nothing comes after.
    let tracks = vec![
        track(library.path(), "a.wav", 1.5),
        track(library.path(), "b.wav", 1.5),
    ];
    engine.player().enqueue(tracks, None).unwrap();
    wait_for("the queue on air", Duration::from_secs(5), || {
        on_air(&engine) == Some(PlaySource::Manual)
    })
    .await;
    // The queue plays out, then the fallback directory, never dead air.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            !engine.player().dead_air(),
            "dead air: {:?}",
            engine.warnings()
        );
        if on_air(&engine) == Some(PlaySource::Fallback) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the fallback directory never played"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_fallback_directory_warns_before_the_queue_runs_dry() {
    let _real_time = REAL_TIME.lock().await;
    let library = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    // A file that cannot be decoded is not a track that can play.
    std::fs::write(fallback.path().join("broken.mp3"), b"not audio").unwrap();
    let engine = engine(library.path(), state.path(), Some(fallback.path()));
    engine
        .player()
        .enqueue(vec![track(library.path(), "a.wav", 3.0)], None)
        .unwrap();
    wait_for("the queue on air", Duration::from_secs(5), || {
        on_air(&engine).is_some()
    })
    .await;
    // Music on air, the warning already there.
    wait_for("the warning", Duration::from_secs(20), || {
        engine.warnings().iter().any(|warning| {
            warning == "the fallback directory has no playable track: nothing plays if the queue runs dry"
        })
    })
    .await;
    assert!(on_air(&engine).is_some() && !engine.player().dead_air());
}

#[tokio::test(flavor = "multi_thread")]
async fn no_fallback_directory_warns_too() {
    let _real_time = REAL_TIME.lock().await;
    let library = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let engine = engine(library.path(), state.path(), None);
    assert!(
        engine
            .warnings()
            .contains(&"no fallback directory: nothing plays if the queue runs dry".to_owned()),
        "{:?}",
        engine.warnings()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pause_is_timed() {
    let _real_time = REAL_TIME.lock().await;
    let library = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let engine = engine(library.path(), state.path(), None);
    assert_eq!(engine.paused_for(), None);
    engine.player().pause().unwrap();
    wait_for("the pause", Duration::from_secs(5), || {
        engine.paused_for().is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(engine.paused_for().unwrap() >= Duration::from_millis(200));
    engine.player().resume().unwrap();
    wait_for("the resume", Duration::from_secs(5), || {
        engine.paused_for().is_none()
    })
    .await;
}
