// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two engines on one state directory: the running one hands over to the
//! new one, streaming to a fake Icecast server throughout.

#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use heron_core::playout::TrackPlayout;
use heron_core::{
    EncoderKind, EncoderProfile, OutputConfig, OutputKind, OutputsConfig, PlaySource,
};
use heron_engine::engine::{Engine, EngineOptions};
use heron_engine::handover::{self, Followed, Probe};
use heron_engine::player::TrackRequest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A fake Icecast server: counts source connections and the audio bytes
/// received, and answers metadata updates.
#[derive(Default)]
struct Server {
    sources: AtomicU32,
    bytes: AtomicU64,
    /// A source connection was closed.
    closed: AtomicBool,
    /// The stream, as received.
    stream: Mutex<Vec<u8>>,
}

impl Server {
    async fn start() -> (u16, Arc<Self>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(Self::default());
        let accepting = Arc::clone(&server);
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let server = Arc::clone(&accepting);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 8192];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    if !request.starts_with(b"PUT ") {
                        let _ = socket.write_all(b"HTTP/1.0 200 OK\r\n\r\n").await;
                        return;
                    }
                    server.sources.fetch_add(1, Ordering::Relaxed);
                    socket
                        .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                        .await
                        .unwrap();
                    loop {
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => {
                                server.bytes.fetch_add(read as u64, Ordering::Relaxed);
                                server
                                    .stream
                                    .lock()
                                    .unwrap()
                                    .extend_from_slice(&buffer[..read]);
                            }
                        }
                    }
                    server.closed.store(true, Ordering::Relaxed);
                });
            }
        });
        (port, server)
    }

    fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
}

/// The longest silence in the stream received, leaving out its first and
/// last 200 ms, in ms (10 ms windows below −40 dBFS).
fn longest_silence_ms(server: &Server) -> u64 {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stream.mp3");
    std::fs::write(&path, &*server.stream.lock().unwrap()).unwrap();
    let audio = heron_engine::decoder::decode_to_memory(&path, 0, u32::MAX);
    let window = 480 * 2;
    let windows: Vec<bool> = audio
        .chunks(window)
        .map(|chunk| {
            let power = chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32;
            power.sqrt() < 0.01
        })
        .collect();
    let inner = &windows[20..windows.len().saturating_sub(20)];
    let mut longest = 0;
    let mut run = 0;
    for &silent in inner {
        run = if silent { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest * 10
}

fn outputs(port: u16) -> OutputsConfig {
    OutputsConfig {
        encoder_profiles: vec![EncoderProfile {
            id: 1,
            name: "MP3 128k".to_owned(),
            encoder: EncoderKind::Mp3,
            bitrate_kbps: Some(128),
            sample_rate: 44_100,
            channels: 2,
            options: serde_json::Value::Null,
        }],
        outputs: vec![OutputConfig {
            id: 7,
            name: "fake icecast".to_owned(),
            kind: OutputKind::Icecast,
            encoder_profile_id: 1,
            config: serde_json::json!({
                "host": "127.0.0.1",
                "port": port,
                "mount": "/live",
                "password": "hackme",
            }),
            metadata_enabled: true,
            enabled: true,
        }],
    }
}

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
    }
}

fn engine(library: &Path, state: &Path, relay: Option<&Path>, standby: bool) -> Arc<Engine> {
    Engine::start(
        EngineOptions {
            standby,
            relay_socket: relay.map(Path::to_path_buf),
            ..EngineOptions::new(library.to_path_buf(), state.to_path_buf())
        },
        tokio::runtime::Handle::current(),
    )
    .unwrap()
}

/// The tests play in real time: one at a time, or they starve each other
/// of CPU and the measured silence grows.
static REAL_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn on_air(engine: &Engine) -> Option<(u64, u64)> {
    engine
        .player()
        .snapshot()
        .decks
        .into_iter()
        .find(|deck| deck.on_air)
        .and_then(|deck| Some((deck.entry?.entry_id, deck.position_ms)))
}

/// An engine on air with two tracks queued, streaming to `server`, through
/// a relay listening on `relay` when given.
async fn station(
    relay: Option<&Path>,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<Engine>,
    Arc<Server>,
) {
    let library = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (port, server) = Server::start().await;
    let active = engine(library.path(), state.path(), relay, false);
    active.set_outputs(&outputs(port)).unwrap();
    let tracks = vec![
        track(library.path(), "first.wav", 30.0),
        track(library.path(), "second.wav", 10.0),
    ];
    active.player().enqueue(tracks, None).unwrap();
    wait_for("the stream", Duration::from_secs(10), || {
        server.bytes() > 16_000 && on_air(&active).is_some()
    })
    .await;
    (library, state, active, server)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hands_over_the_queue_and_the_connection() {
    let _real_time = REAL_TIME.lock().await;
    hand_over(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hands_over_a_relayed_connection() {
    let _real_time = REAL_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let relay = dir.path().join("relay.sock");
    tokio::spawn(heron_outputs::connection::serve_relay(relay.clone()));
    wait_for("the relay", Duration::from_secs(2), || relay.exists()).await;
    hand_over(Some(&relay)).await;
}

async fn hand_over(relay: Option<&Path>) {
    let (library, state, active, server) = station(relay).await;
    let listener = handover::Listener::bind(state.path()).unwrap();

    let Probe::Standby(channel) = handover::probe(state.path()) else {
        panic!("no standby");
    };
    let standby = engine(library.path(), state.path(), relay, true);
    assert!(standby.is_standby());
    let following = Arc::clone(&standby);
    let follow = tokio::task::spawn_blocking(move || handover::follow(&following, channel));

    let mut channel = None;
    wait_for("the standby", Duration::from_secs(5), || {
        channel = listener.take_standby();
        channel.is_some()
    })
    .await;
    let (entry_id, _) = on_air(&active).unwrap();
    let waiting = active.player().saved().waiting;

    let handing = Arc::clone(&active);
    let runtime = tokio::runtime::Handle::current();
    let channel = channel.unwrap();
    let started = Instant::now();
    tokio::task::spawn_blocking(move || handover::hand_over(&handing, channel, &runtime))
        .await
        .unwrap()
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(matches!(follow.await.unwrap(), Followed::TookOver));

    // The new engine plays the same track, on the same connection, from
    // where the old one stopped (held there since).
    assert!(!standby.is_standby());
    let cut = Instant::now();
    wait_for("the track on air", Duration::from_secs(2), || {
        on_air(&standby).is_some_and(|(id, _)| id == entry_id)
    })
    .await;
    let (_, stopped_ms) = on_air(&active).unwrap();
    let (_, resumed_ms) = on_air(&standby).unwrap();
    let elapsed_ms = cut.elapsed().as_millis() as u64;
    assert!(
        resumed_ms >= stopped_ms && resumed_ms <= stopped_ms + elapsed_ms + 50,
        "stopped at {stopped_ms} ms, resumed at {resumed_ms} ms, {elapsed_ms} ms later"
    );
    assert_eq!(standby.player().saved().waiting, waiting);
    let before = server.bytes();
    tokio::time::sleep(Duration::from_secs(1)).await;
    // 128 kbps: about 16 kB a second.
    assert!(
        server.bytes() - before > 12_000,
        "{}",
        server.bytes() - before
    );
    assert_eq!(server.sources.load(Ordering::Relaxed), 1);
    assert!(!server.closed.load(Ordering::Relaxed));

    // The old engine is done: its outputs gave up their connection.
    drop(active);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!server.closed.load(Ordering::Relaxed));
    let (_, position_ms) = on_air(&standby).unwrap();
    assert!(position_ms > 2_000, "{position_ms}");

    // What listeners heard at the cut.
    let silence = longest_silence_ms(&server);
    println!("longest silence in the stream: {silence} ms");
    assert!(silence < 200, "{silence} ms of silence");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plays_on_when_the_standby_goes_away() {
    let _real_time = REAL_TIME.lock().await;
    let (_library, state, active, server) = station(None).await;
    let listener = handover::Listener::bind(state.path()).unwrap();
    let Probe::Standby(standby) = handover::probe(state.path()) else {
        panic!("no standby");
    };
    let mut channel = None;
    wait_for("the standby", Duration::from_secs(5), || {
        channel = listener.take_standby();
        channel.is_some()
    })
    .await;
    // The standby dies before preparing.
    drop(standby);

    let handing = Arc::clone(&active);
    let runtime = tokio::runtime::Handle::current();
    let channel = channel.unwrap();
    let handed =
        tokio::task::spawn_blocking(move || handover::hand_over(&handing, channel, &runtime))
            .await
            .unwrap();
    assert!(handed.is_err());

    // Still on air, on its own connection.
    let before = server.bytes();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(server.bytes() - before > 12_000);
    assert!(on_air(&active).is_some());
    assert!(!active.player().is_held());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refuses_a_standby_of_another_protocol() {
    let _real_time = REAL_TIME.lock().await;
    let state = tempfile::tempdir().unwrap();
    let listener = handover::Listener::bind(state.path()).unwrap();
    let path = state.path().join("handover.sock");
    let other = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut stream = std::os::unix::net::UnixStream::connect(path).unwrap();
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        let mut hello = vec![0; u32::from_be_bytes(header) as usize];
        stream.read_exact(&mut hello).unwrap();
        let answer = br#"{"type":"hello","protocol":999,"version":"9.9.9"}"#;
        stream
            .write_all(&(answer.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(answer).unwrap();
        stream
    });
    let _stream = other.await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(listener.take_standby().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starts_from_the_saved_state_when_nothing_is_handed_over() {
    let _real_time = REAL_TIME.lock().await;
    let (library, state, active, _server) = station(None).await;
    let listener = handover::Listener::bind(state.path()).unwrap();
    let Probe::Standby(channel) = handover::probe(state.path()) else {
        panic!("no standby");
    };
    let standby = engine(library.path(), state.path(), None, true);
    let following = Arc::clone(&standby);
    let state_dir = state.path().to_path_buf();
    let go_on_air = tokio::task::spawn_blocking(move || {
        handover::go_on_air(&following, Some(channel), &state_dir)
    });

    let mut channel = None;
    wait_for("the standby", Duration::from_secs(5), || {
        channel = listener.take_standby();
        channel.is_some()
    })
    .await;
    // The active engine stops the usual way and gives up the handover.
    let (entry_id, _) = on_air(&active).unwrap();
    active.save_queue();
    channel
        .unwrap()
        .send(&handover::Message::Abort {
            reason: "test".to_owned(),
        })
        .unwrap();
    drop(listener);
    drop(active);

    let followed = go_on_air.await.unwrap();
    assert!(matches!(followed, Followed::GivenUp(reason) if reason == "test"));
    assert!(!standby.is_standby());
    wait_for("the saved track", Duration::from_secs(5), || {
        on_air(&standby).is_some_and(|(id, _)| id == entry_id)
    })
    .await;
    assert_eq!(standby.config().outputs.outputs.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hands_over_a_paused_engine() {
    let _real_time = REAL_TIME.lock().await;
    let (library, state, active, server) = station(None).await;
    active.player().pause().unwrap();
    let listener = handover::Listener::bind(state.path()).unwrap();
    let start = handover::start(state.path());
    assert!(start.standby);
    let standby = engine(library.path(), state.path(), None, true);
    let following = Arc::clone(&standby);
    let state_dir = state.path().to_path_buf();
    let go_on_air = tokio::task::spawn_blocking(move || {
        handover::go_on_air(&following, start.follow, &state_dir)
    });
    wait_for("the standby", Duration::from_secs(5), || {
        state.path().join("handover.sock").exists()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (entry_id, _) = on_air(&active).unwrap();
    let waiting = active.player().saved().waiting;

    let stopping = Arc::clone(&active);
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || handover::stop(&stopping, &listener, &runtime))
        .await
        .unwrap();
    assert!(matches!(go_on_air.await.unwrap(), Followed::TookOver));

    // Still paused, on the same connection. As after a restart, the track
    // on air waits cued at its position until the engine resumes.
    let snapshot = standby.player().snapshot();
    assert!(snapshot.paused);
    assert_eq!(snapshot.cued.map(|entry| entry.entry_id), Some(entry_id));
    assert_eq!(snapshot.queue, waiting);
    assert_eq!(server.sources.load(Ordering::Relaxed), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saves_the_queue_when_nobody_takes_over() {
    let _real_time = REAL_TIME.lock().await;
    let (_library, state, active, _server) = station(None).await;
    let listener = handover::Listener::bind(state.path()).unwrap();
    let queue = state.path().join("queue.json");
    let _ = std::fs::remove_file(&queue);
    let stopping = Arc::clone(&active);
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || handover::stop(&stopping, &listener, &runtime))
        .await
        .unwrap();
    assert!(queue.exists());
    // The socket is gone with the listener: the next engine starts alone.
    assert!(!state.path().join("handover.sock").exists());
    assert!(matches!(handover::probe(state.path()), Probe::Alone));
}

/// A standby that never answers: the active engine plays on, not held.
async fn standby_failing(answer_prepare: bool) {
    let (_library, state, active, server) = station(None).await;
    let listener = handover::Listener::bind(state.path()).unwrap();
    let Probe::Standby(mut fake) = handover::probe(state.path()) else {
        panic!("no standby");
    };
    let fake = tokio::task::spawn_blocking(move || {
        // Reads what comes, never ready; or dies at the first Prepare.
        let mut prepares = 0;
        while let Ok((message, _)) = fake.recv(None) {
            if let handover::Message::Prepare { .. } = message {
                prepares += 1;
                if !answer_prepare {
                    break;
                }
            }
        }
        prepares
    });
    let mut channel = None;
    wait_for("the standby", Duration::from_secs(5), || {
        channel = listener.take_standby();
        channel.is_some()
    })
    .await;

    let handing = Arc::clone(&active);
    let runtime = tokio::runtime::Handle::current();
    let channel = channel.unwrap();
    let handed =
        tokio::task::spawn_blocking(move || handover::hand_over(&handing, channel, &runtime))
            .await
            .unwrap();
    assert!(handed.is_err());
    let prepares = fake.await.unwrap();
    assert_eq!(prepares, if answer_prepare { 3 } else { 1 });

    // Still on air, on its own connection, never held.
    assert!(!active.player().is_held());
    let before = server.bytes();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(server.bytes() - before > 12_000);
    // The same track, unless it ended during the attempts.
    assert!(on_air(&active).is_some());
    assert_eq!(server.sources.load(Ordering::Relaxed), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plays_on_when_the_standby_dies_while_preparing() {
    let _real_time = REAL_TIME.lock().await;
    standby_failing(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plays_on_when_the_standby_is_never_ready() {
    let _real_time = REAL_TIME.lock().await;
    standby_failing(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_old_engine_writes_nothing_after_handing_over() {
    let _real_time = REAL_TIME.lock().await;
    let (library, state, active, _server) = station(None).await;
    let listener = handover::Listener::bind(state.path()).unwrap();
    let Probe::Standby(channel) = handover::probe(state.path()) else {
        panic!("no standby");
    };
    let standby = engine(library.path(), state.path(), None, true);
    let following = Arc::clone(&standby);
    let follow = tokio::task::spawn_blocking(move || handover::follow(&following, channel));
    let mut channel = None;
    wait_for("the standby", Duration::from_secs(5), || {
        channel = listener.take_standby();
        channel.is_some()
    })
    .await;
    let handing = Arc::clone(&active);
    let runtime = tokio::runtime::Handle::current();
    let channel = channel.unwrap();
    tokio::task::spawn_blocking(move || handover::hand_over(&handing, channel, &runtime))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(follow.await.unwrap(), Followed::TookOver));

    // Only the old engine left running: it must not save the queue any more
    // (it saves every 5 s while playing).
    drop(standby);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let queue = state.path().join("queue.json");
    std::fs::remove_file(&queue).unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(
        !queue.exists(),
        "the old engine saved the queue after handing over"
    );
    drop(active);
}
