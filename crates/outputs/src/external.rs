// SPDX-License-Identifier: AGPL-3.0-or-later

//! The external FFmpeg encoder: the master bus is piped into an `ffmpeg`
//! binary the station provides, and what it writes out is the stream. It is
//! how a station gets codecs Heron cannot ship, such as HE-AAC (AAC+) with
//! `libfdk_aac`, whose license is incompatible with the AGPL (see
//! `docs/outputs.md` and `docs/decisions.md`).
//!
//! The binary is the engine's `HERON_FFMPEG` (`ffmpeg` on the `PATH` when
//! unset): a deployment setting, never an API one. Profiles choose the codec
//! and a few safe options, never free command-line arguments.
//!
//! ```text
//! encoder thread ──bounded──► writer thread ──stdin──► ffmpeg ──stdout──► reader thread ──► encoder thread
//! ```
//!
//! [`Encoder::encode`] never blocks on the child: audio is handed to a
//! writer thread (and dropped if the child does not keep up), and what the
//! child wrote so far is collected. A child that exits, or writes nothing for
//! [`STALL`] while it is fed, is killed and started again after a growing
//! delay; meanwhile the profile's outputs get no audio, and the other
//! profiles are not affected.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use heron_core::EncoderProfile;
use serde::Deserialize;

use crate::ConfigError;
use crate::encoder::{EncodeError, Encoder, INPUT_CHANNELS, INPUT_RATE, StreamInfo};

/// The engine's setting naming the `ffmpeg` binary.
pub const FFMPEG_ENV: &str = "HERON_FFMPEG";
/// A child fed this long without writing anything is stuck.
pub const STALL: Duration = Duration::from_secs(10);
/// First delay before starting a failed child again; it doubles up to
/// [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Audio blocks waiting for the child, at most (several seconds).
const INPUT_QUEUE: usize = 256;
/// How long a flush waits for the child to finish its stream.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);
/// Lines of the child's errors kept for reports.
const STDERR_LINES: usize = 8;

/// `encoder_profiles.options` of the external encoder.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalOptions {
    /// FFmpeg's audio encoder: `libfdk_aac`, `aac`, `libopus`…
    pub codec: String,
    /// The encoder's profile (`-profile:a`): `aac_he`, `aac_he_v2`…
    #[serde(default)]
    pub profile: Option<String>,
    /// Variable bitrate mode, 1 to 5 (`-vbr`, `libfdk_aac`), instead of the
    /// profile's bitrate.
    #[serde(default)]
    pub vbr: Option<u8>,
    /// The stream's container: `adts` (AAC), `mp3` or `ogg`. Found from the
    /// codec when missing.
    #[serde(default)]
    pub format: Option<String>,
}

/// The containers a stream server can take as a continuous stream.
const FORMATS: [(&str, &str); 3] = [
    ("adts", "audio/aac"),
    ("mp3", "audio/mpeg"),
    ("ogg", "audio/ogg"),
];

/// A name FFmpeg takes as one argument, and nothing else.
fn plain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// What an `ffmpeg` run is given: the command line, without the binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub codec: String,
    pub args: Vec<String>,
}

/// Checks a profile of the external encoder and builds its command line and
/// stream description. Starts nothing.
pub fn invocation(profile: &EncoderProfile) -> Result<(Invocation, StreamInfo), ConfigError> {
    let invalid = |reason: String| ConfigError::Profile {
        profile: profile.name.clone(),
        reason,
    };
    if profile.options.is_null() {
        return Err(invalid(
            "the external FFmpeg encoder needs options: at least {\"codec\": \"libfdk_aac\"}"
                .to_owned(),
        ));
    }
    let options: ExternalOptions = serde_json::from_value(profile.options.clone())
        .map_err(|error| invalid(format!("invalid options: {error}")))?;
    if !plain(&options.codec) {
        return Err(invalid(format!(
            "{:?} is not an FFmpeg encoder name",
            options.codec
        )));
    }
    if let Some(name) = options.profile.as_deref().filter(|name| !plain(name)) {
        return Err(invalid(format!("{name:?} is not an encoder profile name")));
    }
    if options.vbr.is_some_and(|vbr| !(1..=5).contains(&vbr)) {
        return Err(invalid("vbr is 1 to 5".to_owned()));
    }
    if !matches!(profile.channels, 1 | 2) {
        return Err(invalid(format!("{} channels", profile.channels)));
    }
    if !(8_000..=96_000).contains(&profile.sample_rate) {
        return Err(invalid(format!("{} Hz", profile.sample_rate)));
    }
    let format = match options.format.as_deref() {
        Some(format) => format,
        None if options.codec.contains("aac") => "adts",
        None if options.codec.contains("mp3") => "mp3",
        None => "ogg",
    };
    let Some((format, content_type)) = FORMATS.iter().find(|(name, _)| *name == format) else {
        return Err(invalid(format!(
            "the {format:?} container cannot be streamed: adts, mp3 or ogg"
        )));
    };
    let bitrate_kbps = match (options.vbr, profile.bitrate_kbps) {
        (Some(_), _) => None,
        (None, Some(kbps)) if (8..=512).contains(&kbps) => Some(kbps),
        (None, Some(kbps)) => return Err(invalid(format!("{kbps} kbps"))),
        (None, None) => return Err(invalid("a bitrate, or a vbr mode".to_owned())),
    };

    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-f",
        "f32le",
        "-ar",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.push(INPUT_RATE.to_string());
    args.extend(["-ac".to_owned(), INPUT_CHANNELS.to_string()]);
    args.extend(["-i", "pipe:0", "-vn", "-c:a"].map(str::to_owned));
    args.push(options.codec.clone());
    if let Some(name) = &options.profile {
        args.extend(["-profile:a".to_owned(), name.clone()]);
    }
    match (options.vbr, bitrate_kbps) {
        (Some(vbr), _) => args.extend(["-vbr".to_owned(), vbr.to_string()]),
        (None, Some(kbps)) => args.extend(["-b:a".to_owned(), format!("{kbps}k")]),
        (None, None) => {}
    }
    args.extend(["-ar".to_owned(), profile.sample_rate.to_string()]);
    args.extend(["-ac".to_owned(), profile.channels.to_string()]);
    args.extend(["-f".to_owned(), (*format).to_owned()]);
    args.extend(["-flush_packets", "1", "pipe:1"].map(str::to_owned));
    Ok((
        Invocation {
            codec: options.codec,
            args,
        },
        StreamInfo {
            content_type,
            sample_rate: profile.sample_rate,
            channels: profile.channels,
            bitrate_kbps,
        },
    ))
}

/// The `ffmpeg` binary the engine is set up with.
pub fn configured_program() -> PathBuf {
    std::env::var_os(FFMPEG_ENV)
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from)
}

/// The first line of `program -version` (`ffmpeg version 7.1 …`), when it runs.
pub fn version(program: &PathBuf) -> Option<String> {
    let output = Command::new(program)
        .arg("-version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
}

/// Whether `program` runs and has the encoder `codec`.
pub fn check(program: &PathBuf, codec: &str) -> Result<(), String> {
    let output = Command::new(program)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!(
                "cannot run {} ({error}): set {FFMPEG_ENV} to an ffmpeg binary in the engine's container",
                program.display()
            )
        })?;
    let listing = String::from_utf8_lossy(&output.stdout);
    // Lines like ` A....D libfdk_aac           Fraunhofer FDK AAC`.
    let found = listing
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .any(|name| name == codec);
    if found {
        Ok(())
    } else {
        Err(format!(
            "{} has no {codec} encoder (an FFmpeg built with it is needed, e.g. --enable-libfdk-aac --enable-nonfree for libfdk_aac)",
            program.display()
        ))
    }
}

/// A child `ffmpeg` and the threads around it.
struct Running {
    child: Child,
    input: SyncSender<Vec<u8>>,
    output: Receiver<Vec<u8>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// The last time the child wrote something, or was started.
    last_output: Instant,
    /// Audio handed to the child since it last wrote something.
    fed: bool,
}

impl Running {
    fn start(program: &PathBuf, invocation: &Invocation) -> Result<Self, String> {
        check(program, &invocation.codec)?;
        let mut child = Command::new(program)
            .args(&invocation.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
        let (Some(mut stdin), Some(mut stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("ffmpeg's pipes are missing".to_owned());
        };

        let (input, inputs) = std::sync::mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
        std::thread::Builder::new()
            .name("heron-ffmpeg-in".to_owned())
            .spawn(move || {
                // Ends when the encoder drops its sender (stdin then closes,
                // and ffmpeg finishes its stream) or the child goes away.
                while let Ok(bytes) = inputs.recv() {
                    if stdin.write_all(&bytes).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;

        let (outputs, output) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("heron-ffmpeg-out".to_owned())
            .spawn(move || {
                let mut buffer = vec![0_u8; 16 * 1024];
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if outputs.send(buffer[..read].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;

        let lines = Arc::new(Mutex::new(VecDeque::new()));
        let thread_lines = Arc::clone(&lines);
        std::thread::Builder::new()
            .name("heron-ffmpeg-err".to_owned())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if let Ok(mut lines) = thread_lines.lock() {
                        if lines.len() == STDERR_LINES {
                            lines.pop_front();
                        }
                        lines.push_back(line);
                    }
                }
            })
            .map_err(|error| error.to_string())?;

        Ok(Self {
            child,
            input,
            output,
            stderr: lines,
            last_output: Instant::now(),
            fed: false,
        })
    }

    /// What the child said last, for a report.
    fn errors(&self) -> String {
        self.stderr
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join(" / "))
            .unwrap_or_default()
    }

    /// Collects what the child wrote. `Err` when it is gone.
    fn collect(&mut self, out: &mut Vec<u8>) -> Result<(), ()> {
        loop {
            match self.output.try_recv() {
                Ok(bytes) => {
                    out.extend_from_slice(&bytes);
                    self.last_output = Instant::now();
                    self.fed = false;
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => return Err(()),
            }
        }
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The external FFmpeg encoder of one profile.
pub struct ExternalEncoder {
    name: String,
    info: StreamInfo,
    program: PathBuf,
    invocation: Invocation,
    running: Option<Running>,
    /// When the child may be started (again).
    next_start: Instant,
    backoff: Duration,
    bytes: Vec<u8>,
}

impl ExternalEncoder {
    /// Checks the profile; the child starts with the first audio.
    pub fn new(profile: &EncoderProfile) -> Result<Self, ConfigError> {
        Self::with_program(profile, configured_program())
    }

    pub fn with_program(profile: &EncoderProfile, program: PathBuf) -> Result<Self, ConfigError> {
        let (invocation, info) = invocation(profile)?;
        Ok(Self {
            name: profile.name.clone(),
            info,
            program,
            invocation,
            running: None,
            next_start: Instant::now(),
            backoff: FIRST_BACKOFF,
            bytes: Vec::new(),
        })
    }

    /// The child's process ID, while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.running.as_ref().map(|running| running.child.id())
    }

    /// Stops the child and waits before the next start.
    fn fail(&mut self, reason: String) -> EncodeError {
        if let Some(running) = self.running.take() {
            running.stop();
        }
        self.next_start = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
        metrics::counter!("heron_external_encoder_failures_total", "profile" => self.name.clone())
            .increment(1);
        EncodeError::External(reason)
    }
}

impl Encoder for ExternalEncoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn encode(&mut self, pcm: &[f32], out: &mut Vec<u8>) -> Result<(), EncodeError> {
        if self.running.is_none() {
            if Instant::now() < self.next_start {
                return Err(EncodeError::Waiting);
            }
            match Running::start(&self.program, &self.invocation) {
                Ok(running) => {
                    tracing::info!(profile = %self.name, pid = running.child.id(), "ffmpeg started");
                    self.running = Some(running);
                }
                Err(reason) => return Err(self.fail(reason)),
            }
        }
        let Some(running) = self.running.as_mut() else {
            return Err(EncodeError::Waiting);
        };

        self.bytes.clear();
        self.bytes.reserve(pcm.len() * 4);
        for sample in pcm {
            self.bytes.extend_from_slice(&sample.to_le_bytes());
        }
        match running.input.try_send(std::mem::take(&mut self.bytes)) {
            Ok(()) => running.fed = true,
            Err(TrySendError::Full(_)) => {
                metrics::counter!("heron_external_encoder_dropped_chunks_total", "profile" => self.name.clone())
                    .increment(1);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }

        let gone = running.collect(out).is_err();
        let exited = matches!(running.child.try_wait(), Ok(Some(_)));
        if gone || exited {
            let errors = running.errors();
            let status = running
                .child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| status.to_string())
                .unwrap_or_else(|| "stopped".to_owned());
            return Err(self.fail(format!("ffmpeg exited ({status}): {errors}")));
        }
        if running.fed && running.last_output.elapsed() > STALL {
            let errors = running.errors();
            return Err(self.fail(format!(
                "ffmpeg wrote nothing for {} s: {errors}",
                STALL.as_secs()
            )));
        }
        // Working: the next failure starts again quickly.
        if !out.is_empty() {
            self.backoff = FIRST_BACKOFF;
        }
        Ok(())
    }

    fn flush(&mut self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        let Some(running) = self.running.take() else {
            return Ok(());
        };
        let Running {
            mut child,
            input,
            output,
            ..
        } = running;
        // Closing stdin lets ffmpeg finish its stream.
        drop(input);
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match output.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(bytes) => out.extend_from_slice(&bytes),
                Err(_) => break,
            }
            if left.is_zero() {
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        Ok(())
    }
}

impl Drop for ExternalEncoder {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use heron_core::EncoderKind;
    use serde_json::json;

    use super::*;

    pub(crate) fn profile(options: serde_json::Value, bitrate_kbps: Option<u16>) -> EncoderProfile {
        EncoderProfile {
            id: 9,
            name: "AAC+".to_owned(),
            encoder: EncoderKind::ExternalFfmpeg,
            bitrate_kbps,
            sample_rate: 44_100,
            channels: 2,
            options,
        }
    }

    #[test]
    fn builds_the_command_line() {
        let (call, info) = invocation(&profile(
            json!({"codec": "libfdk_aac", "profile": "aac_he_v2"}),
            Some(48),
        ))
        .unwrap();
        assert_eq!(call.codec, "libfdk_aac");
        let line = call.args.join(" ");
        assert!(
            line.contains("-f f32le -ar 48000 -ac 2 -i pipe:0"),
            "{line}"
        );
        assert!(
            line.contains("-c:a libfdk_aac -profile:a aac_he_v2 -b:a 48k"),
            "{line}"
        );
        assert!(
            line.ends_with("-ar 44100 -ac 2 -f adts -flush_packets 1 pipe:1"),
            "{line}"
        );
        assert_eq!(info.content_type, "audio/aac");
        assert_eq!(info.bitrate_kbps, Some(48));

        let (call, info) =
            invocation(&profile(json!({"codec": "libfdk_aac", "vbr": 2}), None)).unwrap();
        assert!(call.args.join(" ").contains("-vbr 2"));
        assert_eq!(info.bitrate_kbps, None);
        let (_, info) = invocation(&profile(json!({"codec": "libopus"}), Some(64))).unwrap();
        assert_eq!(info.content_type, "audio/ogg");
    }

    #[test]
    fn refuses_what_could_reach_the_command_line() {
        for options in [
            serde_json::Value::Null,
            json!({"codec": "aac -y /etc/passwd"}),
            json!({"codec": "aac", "profile": "x;rm"}),
            json!({"codec": "aac", "format": "mp4"}),
            json!({"codec": "aac", "vbr": 9}),
            json!({"codec": "aac", "args": ["-y"]}),
        ] {
            assert!(
                invocation(&profile(options.clone(), Some(96))).is_err(),
                "{options}"
            );
        }
        assert!(
            invocation(&profile(json!({"codec": "aac"}), None)).is_err(),
            "no bitrate"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_version_is_the_first_line_the_program_prints() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("fake-ffmpeg");
        std::fs::write(
            &program,
            "#!/bin/sh\necho '  ffmpeg version 9.0 test  '\necho second\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            version(&program).as_deref(),
            Some("ffmpeg version 9.0 test")
        );

        let silent = dir.path().join("silent");
        std::fs::write(&silent, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&silent, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(version(&silent), None, "no output");
        assert_eq!(version(&dir.path().join("absent")), None, "not found");
    }
}
