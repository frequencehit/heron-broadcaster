// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-engine ctl`: a command-line client for the engine's gRPC interface.
//!
//! For operators and tests. Until the API's queue manager (M4) feeds the
//! engine, `enqueue` and `autofill` analyze files themselves, the way the
//! analyzer would, to send cue and crossfade points.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Subcommand;
use heron_core::{AnalysisSettings, LibraryPath};
use heron_proto::engine as pb;
use heron_proto::engine::engine_client::EngineClient;
use lofty::file::TaggedFileExt;
use lofty::tag::Accessor;
use tonic::transport::Channel;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// gRPC address of the engine.
    #[arg(
        long,
        env = "HERON_ENGINE_URL",
        default_value = "http://127.0.0.1:50051"
    )]
    engine: String,

    #[command(subcommand)]
    command: CtlCommand,
}

#[derive(Debug, Subcommand)]
enum CtlCommand {
    /// Show the decks, the queue and the outputs.
    State,
    /// Show the levels (dBFS).
    Levels,
    /// Queue files (paths in the library), analyzing them first.
    Enqueue {
        /// Play them next instead of last.
        #[arg(long)]
        next: bool,
        /// Play them whole, at their own level, as the API does with tracks
        /// not analyzed yet. Silent files are accepted (to try the silence
        /// watchdog).
        #[arg(long)]
        no_analysis: bool,
        files: Vec<PathBuf>,
    },
    /// Keep at least `--min` tracks queued, picked at random from the library
    /// (or from the given directories), until interrupted.
    Autofill {
        #[arg(long, default_value_t = 3)]
        min: usize,
        dirs: Vec<PathBuf>,
    },
    /// Remove an entry from the queue.
    Remove {
        entry_id: u64,
    },
    /// Move an entry of the queue to `position` (0 is next).
    Move {
        entry_id: u64,
        position: u32,
    },
    /// Empty the queue.
    Clear,
    Skip,
    Pause,
    Resume,
    /// Play a file on the FX player.
    Fx {
        file: PathBuf,
    },
    StopFx,
    /// Print the configuration (playout settings and outputs) as JSON.
    Config,
    /// Replace the encoder profiles and outputs (JSON file, `OutputsConfig`).
    SetOutputs {
        file: PathBuf,
    },
    /// Replace the playout settings (JSON file, `PlayoutSettings`).
    SetSettings {
        file: PathBuf,
    },
    /// Print track changes as they happen.
    Watch,
}

pub fn run(args: Args, library_dir: &Path) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(execute(args, library_dir))
}

async fn execute(args: Args, library_dir: &Path) -> anyhow::Result<()> {
    let library_dir = &std::path::absolute(library_dir)?;
    let mut client = EngineClient::connect(args.engine.clone())
        .await
        .with_context(|| format!("cannot reach the engine at {}", args.engine))?;
    match args.command {
        CtlCommand::State => print_state(&mut client).await?,
        CtlCommand::Levels => {
            let levels = client
                .get_levels(pb::GetLevelsRequest {})
                .await?
                .into_inner();
            for (name, level) in [
                ("master", levels.master),
                ("deck A", levels.deck_a),
                ("deck B", levels.deck_b),
                ("FX", levels.fx),
            ] {
                let level = level.unwrap_or_default();
                println!(
                    "{name:>7}  peak {:6.1} {:6.1}   rms {:6.1} {:6.1}",
                    level.peak_left, level.peak_right, level.rms_left, level.rms_right
                );
            }
        }
        CtlCommand::Enqueue {
            next,
            no_analysis,
            files,
        } => {
            let mut tracks = Vec::new();
            for file in &files {
                tracks.push(if no_analysis {
                    prepare_unanalyzed(library_dir, file)?
                } else {
                    prepare(library_dir, file)?
                });
            }
            let response = client
                .enqueue(pb::EnqueueRequest {
                    tracks,
                    position: next.then_some(0),
                })
                .await?
                .into_inner();
            println!("queued entries {:?}", response.entry_ids);
        }
        CtlCommand::Autofill { min, dirs } => autofill(&mut client, library_dir, min, dirs).await?,
        CtlCommand::Remove { entry_id } => {
            client
                .remove_from_queue(pb::RemoveFromQueueRequest { entry_id })
                .await?;
        }
        CtlCommand::Move { entry_id, position } => {
            client
                .move_queue_entry(pb::MoveQueueEntryRequest { entry_id, position })
                .await?;
        }
        CtlCommand::Clear => {
            client.clear_queue(pb::ClearQueueRequest {}).await?;
        }
        CtlCommand::Skip => {
            client.skip(pb::SkipRequest {}).await?;
        }
        CtlCommand::Pause => {
            client.pause(pb::PauseRequest {}).await?;
        }
        CtlCommand::Resume => {
            client.resume(pb::ResumeRequest {}).await?;
        }
        CtlCommand::Fx { file } => {
            let track = prepare(library_dir, &file)?;
            let response = client
                .play_fx(pb::PlayFxRequest { track: Some(track) })
                .await?
                .into_inner();
            println!("playing entry {}", response.entry_id);
        }
        CtlCommand::StopFx => {
            client.stop_fx(pb::StopFxRequest {}).await?;
        }
        CtlCommand::Config => {
            let config = client
                .get_config(pb::GetConfigRequest {})
                .await?
                .into_inner();
            let playout: serde_json::Value = serde_json::from_str(&config.playout_settings_json)?;
            let outputs: serde_json::Value = serde_json::from_str(&config.outputs_json)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "playout": playout,
                    "outputs": outputs,
                }))?
            );
        }
        CtlCommand::SetOutputs { file } => {
            let outputs_json = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            client
                .set_outputs(pb::SetOutputsRequest { outputs_json })
                .await?;
            println!("outputs applied");
        }
        CtlCommand::SetSettings { file } => {
            let settings_json = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            client
                .set_playout_settings(pb::SetPlayoutSettingsRequest { settings_json })
                .await?;
            println!("settings applied");
        }
        CtlCommand::Watch => {
            let mut events = client
                .watch_events(pb::WatchEventsRequest {})
                .await?
                .into_inner();
            while let Some(event) = events.message().await? {
                match event.event {
                    Some(pb::engine_event::Event::TrackStarted(started)) => println!(
                        "{} started  {:?} {}",
                        event.timestamp_unix_ms,
                        started.deck(),
                        describe(started.entry.as_ref())
                    ),
                    Some(pb::engine_event::Event::TrackEnded(ended)) => println!(
                        "{} ended    {:?} {} after {:.1} s ({:?})",
                        event.timestamp_unix_ms,
                        ended.deck(),
                        describe(ended.entry.as_ref()),
                        ended.played_ms as f64 / 1_000.0,
                        ended.reason()
                    ),
                    None => {}
                }
            }
        }
    }
    Ok(())
}

async fn print_state(client: &mut EngineClient<Channel>) -> anyhow::Result<()> {
    let state = client.get_state(pb::GetStateRequest {}).await?.into_inner();
    println!(
        "rendered {:.1} s, underruns {}{}",
        state.rendered_ms as f64 / 1_000.0,
        state.underruns,
        if state.paused { ", PAUSED" } else { "" }
    );
    for deck in &state.decks {
        println!(
            "{:?}{} {:>7.1} / {:>7.1} s  {}",
            deck.deck(),
            if deck.on_air { " (on air)" } else { "" },
            deck.position_ms as f64 / 1_000.0,
            deck.length_ms as f64 / 1_000.0,
            describe(deck.entry.as_ref())
        );
    }
    println!("cued: {}", describe(state.cued.as_ref()));
    for entry in &state.queue {
        println!("  queued: {}", describe(Some(entry)));
    }
    for output in &state.outputs {
        println!(
            "output {:?}: {}{} ({} bytes sent, {} chunks dropped)",
            output.name,
            output.state,
            output
                .error
                .as_ref()
                .map_or(String::new(), |error| format!(": {error}")),
            output.bytes_sent,
            output.dropped_chunks
        );
    }
    Ok(())
}

fn describe(entry: Option<&pb::QueueEntry>) -> String {
    let Some(entry) = entry else {
        return "-".to_owned();
    };
    let Some(track) = &entry.track else {
        return format!("#{}", entry.entry_id);
    };
    let source = match track.source() {
        pb::PlaySource::Rotation => "rotation",
        pb::PlaySource::Request => "request",
        pb::PlaySource::Fallback => "fallback",
        pb::PlaySource::Manual | pb::PlaySource::Unspecified => "manual",
    };
    format!(
        "#{} {} - {} [{}] ({source})",
        entry.entry_id,
        track.artist.as_deref().unwrap_or("?"),
        track.title.as_deref().unwrap_or("?"),
        track.path
    )
}

/// The absolute path of `file` and its path in the library.
fn locate(library_dir: &Path, file: &Path) -> anyhow::Result<(PathBuf, LibraryPath)> {
    let absolute = if file.is_absolute() {
        file.to_path_buf()
    } else if file.exists() {
        std::env::current_dir()?.join(file)
    } else {
        library_dir.join(file)
    };
    let path = LibraryPath::from_file(library_dir, &absolute).with_context(|| {
        format!(
            "{} is not in the library ({})",
            file.display(),
            library_dir.display()
        )
    })?;
    Ok((absolute, path))
}

/// The track message for a library file without analysis: played whole.
fn prepare_unanalyzed(library_dir: &Path, file: &Path) -> anyhow::Result<pb::Track> {
    let (absolute, path) = locate(library_dir, file)?;
    Ok(pb::Track {
        path: path.as_str().to_owned(),
        title: absolute
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned()),
        source: pb::PlaySource::Manual.into(),
        ..pb::Track::default()
    })
}

/// Builds the track message for a library file: tags and analysis.
fn prepare(library_dir: &Path, file: &Path) -> anyhow::Result<pb::Track> {
    let (absolute, path) = locate(library_dir, file)?;
    let analysis = heron_analysis::analyze_file(&absolute, &AnalysisSettings::default())
        .with_context(|| format!("cannot analyze {}", absolute.display()))?;
    if analysis.is_silent() {
        bail!("{} is silent", absolute.display());
    }
    let (title, artist, album) = match lofty::read_from_path(&absolute) {
        Ok(tagged) => {
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
            (
                tag.and_then(|tag| tag.title().map(|value| value.into_owned())),
                tag.and_then(|tag| tag.artist().map(|value| value.into_owned())),
                tag.and_then(|tag| tag.album().map(|value| value.into_owned())),
            )
        }
        Err(_) => (None, None, None),
    };
    let title = title.or_else(|| {
        absolute
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
    });
    Ok(pb::Track {
        track_id: None,
        path: path.as_str().to_owned(),
        title,
        artist,
        album,
        duration_ms: analysis.duration_ms,
        cue_in_ms: analysis.cue_in_ms,
        cue_out_ms: analysis.cue_out_ms,
        mix_point_ms: analysis.fade_out_start_ms,
        integrated_lufs: analysis.integrated_lufs,
        true_peak_db: analysis.true_peak_db,
        crossfade_overrides_json: String::new(),
        source: pb::PlaySource::Manual.into(),
        no_metadata: false,
    })
}

const AUDIO_EXTENSIONS: [&str; 11] = [
    "mp3", "m4a", "mp4", "aac", "flac", "ogg", "oga", "opus", "wav", "aif", "aiff",
];

fn list_audio(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            list_audio(&path, files);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| AUDIO_EXTENSIONS.contains(&extension.to_lowercase().as_str()))
        {
            files.push(path);
        }
    }
}

/// A stand-in for the queue manager: keeps the queue from running dry.
async fn autofill(
    client: &mut EngineClient<Channel>,
    library_dir: &Path,
    min: usize,
    dirs: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let roots = if dirs.is_empty() {
        vec![library_dir.to_path_buf()]
    } else {
        dirs
    };
    let mut files = Vec::new();
    for root in &roots {
        list_audio(&library_dir.join(root), &mut files);
    }
    if files.is_empty() {
        bail!("no audio files found in {roots:?}");
    }
    println!("autofill: {} files", files.len());
    let mut prepared: HashMap<PathBuf, Option<pb::Track>> = HashMap::new();
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |since| since.as_nanos() as u64)
        | 1;
    loop {
        let state = client.get_state(pb::GetStateRequest {}).await?.into_inner();
        let waiting = state.queue.len() + usize::from(state.cued.is_some());
        if waiting < min {
            // xorshift: no need for a random number crate.
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let file = &files[(seed % files.len() as u64) as usize];
            let track = prepared
                .entry(file.clone())
                .or_insert_with(|| match prepare(library_dir, file) {
                    Ok(track) => Some(track),
                    Err(error) => {
                        eprintln!("skipping {}: {error:#}", file.display());
                        None
                    }
                })
                .clone();
            if let Some(track) = track {
                let text = describe(Some(&pb::QueueEntry {
                    entry_id: 0,
                    track: Some(track.clone()),
                }));
                client
                    .enqueue(pb::EnqueueRequest {
                        tracks: vec![track],
                        position: None,
                    })
                    .await?;
                println!("queued {text}");
            }
            continue;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
