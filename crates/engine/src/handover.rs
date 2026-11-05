// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hot handover: a new engine takes over from the running one, with its
//! queue, the position of the track on air and the open connections of its
//! outputs, so an upgrade never takes the station off air.
//!
//! The active engine listens on `handover.sock` in the state directory. A
//! new engine that finds it there becomes a standby; when the active engine
//! is asked to stop, it hands over instead of just stopping. Messages are
//! length-prefixed JSON; the connections travel as file descriptors
//! (`SCM_RIGHTS`) with the `Cut` message.
//!
//! See `docs/handover.md`.

use std::collections::HashMap;
use std::io::{IoSlice, IoSliceMut, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use heron_outputs::{ConnectionKind, HandedConnection};
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};
use serde::{Deserialize, Serialize};

use crate::config::EngineConfig;
use crate::engine::Engine;
use crate::mixer::frames_to_ms;
use crate::player::{HandoverPoint, SavedQueue};

/// Version of the messages below: engines only hand over to an engine
/// speaking the same.
pub const PROTOCOL: u32 = 1;
const SOCKET: &str = "handover.sock";
/// Outputs whose connections can be handed over, at most.
const MAX_CONNECTIONS: usize = 64;
const MAX_MESSAGE: usize = 16 << 20;

/// How far ahead the track on air stops, at first: the time the standby
/// has to open the track there.
const FIRST_LEAD_MS: u32 = 2_000;
/// A standby too slow gets a longer lead, this many times.
const ATTEMPTS: u32 = 3;
/// The standby must be ready this long before the stopping point, for the
/// stop to be planned in time.
const READY_MARGIN: Duration = Duration::from_millis(300);
/// With nothing on air, how long the standby has to get ready.
const READY_WITHOUT_STOP: Duration = Duration::from_secs(10);
/// The whole handover, within Docker's stop grace period (30 s).
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(25);
/// How long after its stop the track on air may take to stop there.
const HALT_MARGIN: Duration = Duration::from_secs(2);
/// How long an output has to send what is encoded and give up its connection.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(10);
/// Received descriptors are not inherited by child processes (Linux; the
/// engine is only developed on other systems).
#[cfg(any(target_os = "linux", target_os = "android"))]
const RECV_FLAGS: RecvFlags = RecvFlags::CMSG_CLOEXEC;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const RECV_FLAGS: RecvFlags = RecvFlags::empty();

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Both ways, first: the protocol each engine speaks.
    Hello { protocol: u32, version: String },
    /// Active → standby: get ready to go on with this queue, the track on
    /// air stopping where it says. A later `Prepare` replaces it.
    Prepare {
        attempt: u32,
        config: Box<EngineConfig>,
        queue: Box<SavedQueue>,
    },
    /// Standby → active: ready for the `Prepare` of `attempt`.
    Ready { attempt: u32 },
    /// Active → standby: stopped. The connections of `outputs` come with
    /// the message, in that order.
    /// `agc_gain_db`: the gated AGC's gain at the stop, while it is on; the
    /// standby starts from it. Absent from engines before the AGC chain, and
    /// ignored by them.
    Cut {
        outputs: Vec<HandedOver>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agc_gain_db: Option<f32>,
    },
    /// Standby → active: on air.
    Done,
    /// Active → standby: the handover is given up.
    Abort { reason: String },
}

/// One output's connection, in a `Cut`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandedOver {
    pub output: u64,
    pub kind: ConnectionKind,
}

#[derive(Debug, thiserror::Error)]
pub enum HandoverError {
    #[error("handover socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid message: {0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
}

impl From<rustix::io::Errno> for HandoverError {
    fn from(errno: rustix::io::Errno) -> Self {
        Self::Io(errno.into())
    }
}

/// One end of a handover connection.
pub struct Channel {
    stream: UnixStream,
}

impl Channel {
    fn new(stream: UnixStream) -> Self {
        Self { stream }
    }

    pub fn send(&mut self, message: &Message) -> Result<(), HandoverError> {
        self.send_with(message, &[])
    }

    /// Sends `message`, with `fds` attached to its first byte.
    pub fn send_with(&mut self, message: &Message, fds: &[OwnedFd]) -> Result<(), HandoverError> {
        let payload = serde_json::to_vec(message)
            .map_err(|error| HandoverError::Invalid(error.to_string()))?;
        let header = u32::try_from(payload.len())
            .map_err(|_| HandoverError::Invalid("message too long".to_owned()))?
            .to_be_bytes();
        let borrowed: Vec<_> = fds.iter().map(AsFd::as_fd).collect();
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_CONNECTIONS))];
        let mut control = SendAncillaryBuffer::new(&mut space);
        if !borrowed.is_empty() && !control.push(SendAncillaryMessage::ScmRights(&borrowed)) {
            return Err(HandoverError::Invalid("too many connections".to_owned()));
        }
        let parts = [IoSlice::new(&header), IoSlice::new(&payload)];
        let sent = rustix::net::sendmsg(&self.stream, &parts, &mut control, SendFlags::empty())?;
        // The rest, if the socket took only part of it.
        let total = header.len() + payload.len();
        if sent < total {
            let rest: Vec<u8> = header.iter().chain(&payload).skip(sent).copied().collect();
            self.stream.write_all(&rest)?;
        }
        Ok(())
    }

    /// The next message and the descriptors that came with it, waiting at
    /// most `timeout` (`None`: forever).
    pub fn recv(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<(Message, Vec<OwnedFd>), HandoverError> {
        // Refused on some systems once the peer closed: the read says so.
        let _ = self
            .stream
            .set_read_timeout(timeout.map(|timeout| timeout.max(Duration::from_millis(1))));
        let mut header = [0_u8; 4];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_CONNECTIONS))];
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let received = rustix::net::recvmsg(
            &self.stream,
            &mut [IoSliceMut::new(&mut header)],
            &mut control,
            RECV_FLAGS,
        )?;
        let mut fds = Vec::new();
        for message in control.drain() {
            if let RecvAncillaryMessage::ScmRights(received) = message {
                fds.extend(received);
            }
        }
        match received.bytes {
            0 => {
                return Err(HandoverError::Io(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )));
            }
            read if read < header.len() => self.stream.read_exact(&mut header[read..])?,
            _ => {}
        }
        let length = u32::from_be_bytes(header) as usize;
        if length > MAX_MESSAGE {
            return Err(HandoverError::Invalid(format!("{length} bytes")));
        }
        let mut payload = vec![0; length];
        self.stream.read_exact(&mut payload)?;
        let message = serde_json::from_slice(&payload)
            .map_err(|error| HandoverError::Invalid(error.to_string()))?;
        Ok((message, fds))
    }
}

fn hello() -> Message {
    Message::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

fn socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SOCKET)
}

/// The active engine's end: accepts the standby.
pub struct Listener {
    standby: Arc<Mutex<Option<Channel>>>,
    path: PathBuf,
    /// Identifies our socket file: another engine may have replaced it.
    inode: u64,
    /// Handed over: the socket is the new engine's now, left alone.
    handed_over: AtomicBool,
}

impl Listener {
    /// Listens on `handover.sock`, replacing the socket of the engine this
    /// one took over from (bound aside, then renamed over it).
    pub fn bind(state_dir: &Path) -> std::io::Result<Self> {
        let path = socket_path(state_dir);
        let aside = state_dir.join(format!("{SOCKET}.{}", std::process::id()));
        match std::fs::remove_file(&aside) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let listener = UnixListener::bind(&aside)?;
        // Read before the rename, which keeps it: the path itself may be
        // gone already, removed by the engine this one took over from.
        let inode = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&aside)?);
        std::fs::rename(&aside, &path)?;
        let standby: Arc<Mutex<Option<Channel>>> = Arc::default();
        let accepted = Arc::clone(&standby);
        std::thread::Builder::new()
            .name("heron-handover".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream.map_err(HandoverError::from).and_then(greet) {
                        Ok(channel) => {
                            tracing::info!("a standby engine is ready to take over");
                            if let Ok(mut standby) = accepted.lock() {
                                *standby = Some(channel);
                            }
                        }
                        Err(error) => tracing::warn!(%error, "standby engine refused"),
                    }
                }
            })?;
        tracing::info!(path = %path.display(), "listening for a standby engine");
        Ok(Self {
            standby,
            path,
            inode,
            handed_over: AtomicBool::new(false),
        })
    }

    /// The standby engine connected last, if any.
    pub fn take_standby(&self) -> Option<Channel> {
        self.standby.lock().ok()?.take()
    }
}

impl Drop for Listener {
    /// Removes the socket, unless a new engine replaced it: a starting
    /// engine finds nobody on air instead of a socket nobody answers.
    /// After a handover, the new engine is replacing it right now: checking
    /// then removing could remove the new socket, so it is left alone.
    fn drop(&mut self) {
        if self.handed_over.load(Ordering::Relaxed) {
            return;
        }
        let ours = std::fs::metadata(&self.path)
            .is_ok_and(|metadata| std::os::unix::fs::MetadataExt::ino(&metadata) == self.inode);
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Exchanges hellos with a new engine; keeps it if it speaks our protocol.
fn greet(stream: UnixStream) -> Result<Channel, HandoverError> {
    let mut channel = Channel::new(stream);
    channel.send(&hello())?;
    match channel.recv(Some(HELLO_TIMEOUT))? {
        (Message::Hello { protocol, .. }, _) if protocol == PROTOCOL => Ok(channel),
        (Message::Hello { protocol, version }, _) => Err(HandoverError::Failed(format!(
            "it speaks handover protocol {protocol} (version {version}), this engine {PROTOCOL}"
        ))),
        (message, _) => Err(HandoverError::Invalid(format!("{message:?}"))),
    }
}

/// What a starting engine found in the state directory.
pub enum Probe {
    /// No engine is running: start as usual.
    Alone,
    /// An engine is running and will hand over to this one.
    Standby(Channel),
    /// An engine is running but speaks another protocol: wait for it to stop.
    Incompatible { protocol: u32, version: String },
}

/// Looks for an active engine on `handover.sock`.
pub fn probe(state_dir: &Path) -> Probe {
    let Ok(stream) = UnixStream::connect(socket_path(state_dir)) else {
        return Probe::Alone;
    };
    let mut channel = Channel::new(stream);
    match channel.recv(Some(HELLO_TIMEOUT)) {
        Ok((Message::Hello { protocol, version }, _)) => {
            // Answered either way: the active engine logs a refusal.
            if let Err(error) = channel.send(&hello()) {
                tracing::warn!(%error, "the active engine went away");
                return Probe::Alone;
            }
            if protocol == PROTOCOL {
                Probe::Standby(channel)
            } else {
                Probe::Incompatible { protocol, version }
            }
        }
        Ok((message, _)) => {
            tracing::warn!(received = ?message, "unexpected message from the active engine");
            Probe::Alone
        }
        Err(error) => {
            tracing::warn!(%error, "the active engine did not answer");
            Probe::Alone
        }
    }
}

/// How a starting engine begins.
pub struct Start {
    /// Another engine is on air: wait for it ([`go_on_air`]).
    pub standby: bool,
    /// The engine on air, if it will hand over to this one.
    pub follow: Option<Channel>,
}

/// Looks for an engine on air ([`probe`]), and says what it means.
pub fn start(state_dir: &Path) -> Start {
    match probe(state_dir) {
        Probe::Alone => Start {
            standby: false,
            follow: None,
        },
        Probe::Standby(channel) => {
            tracing::info!("an engine is on air: standing by to take over from it");
            Start {
                standby: true,
                follow: Some(channel),
            }
        }
        Probe::Incompatible { protocol, version } => {
            tracing::warn!(
                protocol,
                version,
                "the engine on air cannot hand over to this one: starting once it stops"
            );
            Start {
                standby: true,
                follow: None,
            }
        }
    }
}

/// Standby: goes on air, with what the engine on air hands over (`follow`),
/// or from the saved state once that engine is gone. Blocking.
pub fn go_on_air(engine: &Engine, active: Option<Channel>, state_dir: &Path) -> Followed {
    let followed = match active {
        Some(channel) => follow(engine, channel),
        None => Followed::GivenUp("the engine on air speaks another protocol".to_owned()),
    };
    match &followed {
        Followed::TookOver => tracing::info!("took over from the previous engine"),
        Followed::GivenUp(reason) => {
            tracing::warn!(%reason, "nothing handed over: starting once the other engine stops");
            wait_until_alone(state_dir);
            engine.cold_start();
        }
    }
    followed
}

/// Active engine, asked to stop: hands over to the standby if one waits,
/// else saves the queue for the next start. Blocking.
pub fn stop(engine: &Engine, listener: &Listener, runtime: &tokio::runtime::Handle) {
    let Some(channel) = listener.take_standby() else {
        // Where the track on air is now, for the next start.
        engine.save_queue();
        return;
    };
    match hand_over(engine, channel, runtime) {
        Ok(()) => {
            listener.handed_over.store(true, Ordering::Relaxed);
            tracing::info!("the new engine is on air");
        }
        Err(error) => {
            tracing::error!(%error, "handover failed: stopping as usual");
            engine.save_queue();
        }
    }
}

/// Waits until no engine listens on `handover.sock` any more.
pub fn wait_until_alone(state_dir: &Path) {
    while UnixStream::connect(socket_path(state_dir)).is_ok() {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// How a standby's wait ended.
pub enum Followed {
    /// On air with what was handed over.
    TookOver,
    /// Nothing handed over: start from the saved state.
    GivenUp(String),
}

/// Standby: prepares what the active engine sends, until it cuts over or
/// gives up. Blocking.
pub fn follow(engine: &Engine, mut channel: Channel) -> Followed {
    loop {
        let (message, fds) = match channel.recv(None) {
            Ok(received) => received,
            Err(error) => {
                return Followed::GivenUp(format!("the active engine went away ({error})"));
            }
        };
        match message {
            Message::Prepare {
                attempt,
                config,
                queue,
            } => {
                tracing::info!(attempt, "preparing to take over");
                engine.prepare_takeover(*config, *queue);
                let started = Instant::now();
                while !engine.takeover_ready() && started.elapsed() < HANDOVER_TIMEOUT {
                    std::thread::sleep(POLL);
                }
                tracing::info!(
                    attempt,
                    ms = started.elapsed().as_millis(),
                    "ready to take over"
                );
                if let Err(error) = channel.send(&Message::Ready { attempt }) {
                    return Followed::GivenUp(format!("the active engine went away ({error})"));
                }
            }
            Message::Cut {
                outputs,
                agc_gain_db,
            } => {
                if outputs.len() != fds.len() {
                    tracing::warn!(
                        outputs = outputs.len(),
                        connections = fds.len(),
                        "connections missing: those outputs connect again"
                    );
                }
                let connections: HashMap<u64, HandedConnection> = outputs
                    .into_iter()
                    .zip(fds)
                    .map(|(handed, fd)| (handed.output, HandedConnection::from_fd(handed.kind, fd)))
                    .collect();
                let handed = connections.len();
                engine.complete_takeover(connections, agc_gain_db);
                tracing::info!(connections = handed, "took over: on air");
                if let Err(error) = channel.send(&Message::Done) {
                    tracing::warn!(%error, "cannot tell the old engine");
                }
                return Followed::TookOver;
            }
            Message::Abort { reason } => return Followed::GivenUp(reason),
            message => tracing::warn!(received = ?message, "unexpected handover message"),
        }
    }
}

/// Active engine, asked to stop: hands over to the standby on `channel`.
/// Blocking; call it outside the runtime's worker threads. On an error the
/// engine is still on air (or held), and stops as usual.
pub fn hand_over(
    engine: &Engine,
    mut channel: Channel,
    runtime: &tokio::runtime::Handle,
) -> Result<(), HandoverError> {
    let result = cut_over(engine, &mut channel, runtime);
    if let Err(error) = &result {
        let _ = channel.send(&Message::Abort {
            reason: error.to_string(),
        });
    }
    result
}

fn cut_over(
    engine: &Engine,
    channel: &mut Channel,
    runtime: &tokio::runtime::Handle,
) -> Result<(), HandoverError> {
    let deadline = Instant::now() + HANDOVER_TIMEOUT;
    let mut lead_ms = FIRST_LEAD_MS;
    let mut attempt = 0;
    let stop = loop {
        attempt += 1;
        let point = loop {
            // The stop falls within the handover's time: a later one would
            // keep the old engine past Docker's grace period.
            let left_ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis();
            let lead = u32::try_from(left_ms).unwrap_or(u32::MAX).min(lead_ms);
            if lead < FIRST_LEAD_MS {
                return Err(HandoverError::Failed(
                    "no moment to stop the track on air".to_owned(),
                ));
            }
            match engine.handover_point(lead) {
                HandoverPoint::Wait => std::thread::sleep(POLL),
                point => break point,
            }
        };
        let (stop, ready_by) = match point {
            HandoverPoint::At { position } => {
                let now = engine.on_air_position().unwrap_or(position);
                let ahead = Duration::from_millis(frames_to_ms(position.saturating_sub(now)));
                (
                    Some(position),
                    Instant::now() + ahead.saturating_sub(READY_MARGIN),
                )
            }
            // Nothing plays: hold now, so nothing starts before the cut.
            _ => {
                engine
                    .hold()
                    .map_err(|error| HandoverError::Failed(error.to_string()))?;
                (None, Instant::now() + READY_WITHOUT_STOP)
            }
        };
        let queue = engine.handed_over_queue(stop.map(frames_to_ms));
        tracing::info!(
            attempt,
            stop_ms = stop.map(frames_to_ms),
            waiting = queue.waiting.len(),
            "handing over to the standby engine"
        );
        channel.send(&Message::Prepare {
            attempt,
            config: Box::new(engine.config()),
            queue: Box::new(queue),
        })?;
        if wait_ready(channel, attempt, ready_by.min(deadline))? {
            break stop;
        }
        if stop.is_none() || attempt >= ATTEMPTS {
            return Err(HandoverError::Failed(
                "the standby engine was not ready in time".to_owned(),
            ));
        }
        lead_ms = lead_ms.saturating_mul(3);
        tracing::warn!(
            attempt,
            lead_ms,
            "the standby engine was not ready in time: again, further ahead"
        );
    };

    // Stop the track on air exactly where the standby will go on.
    let mut last_frame = None;
    if let Some(position) = stop {
        // The standby may be ready long before the stop (a retry, with the
        // track quick to open the second time): wait until it is due.
        let ahead = Duration::from_millis(frames_to_ms(
            position.saturating_sub(engine.on_air_position().unwrap_or(position)),
        ));
        engine
            .halt_at(position)
            .map_err(|error| HandoverError::Failed(error.to_string()))?;
        let halted_by = Instant::now() + ahead + HALT_MARGIN;
        let halted = loop {
            if let Some(halted) = engine.take_halted() {
                break halted;
            }
            if Instant::now() > halted_by {
                let _ = engine.unhold();
                return Err(HandoverError::Failed(
                    "the track on air did not stop".to_owned(),
                ));
            }
            std::thread::sleep(POLL);
        };
        if halted.position != position {
            tracing::warn!(halted.position, position, "stopped off the planned frame");
        }
        // The AGC chain's limiter delays the audio: the end of the fade-out
        // comes out of it that much later.
        last_frame = Some(halted.at_frame + engine.agc_latency_frames());
        // The audio up to the stop is rendered: give it time to reach the
        // encoders (the clock renders 50 ms ahead).
        std::thread::sleep(Duration::from_millis(100));
    }

    let released = engine.release_outputs(last_frame);
    let mut outputs = Vec::new();
    let mut fds = Vec::new();
    runtime.block_on(async {
        for (output, connection) in released {
            match tokio::time::timeout(RELEASE_TIMEOUT, connection).await {
                Ok(Ok(Some(connection))) => {
                    outputs.push(HandedOver {
                        output,
                        kind: connection.kind(),
                    });
                    fds.push(OwnedFd::from(connection));
                }
                Ok(_) => tracing::info!(output, "no connection to hand over"),
                Err(_) => tracing::warn!(output, "the output did not give up its connection"),
            }
        }
    });
    engine.stop_saving();
    channel.send_with(
        &Message::Cut {
            outputs: outputs.clone(),
            agc_gain_db: engine.agc_gain_db(),
        },
        &fds,
    )?;
    // Ours are copies: the standby has its own.
    drop(fds);
    match channel.recv(Some(Duration::from_secs(5))) {
        Ok((Message::Done, _)) => tracing::info!(connections = outputs.len(), "handed over"),
        Ok((message, _)) => tracing::warn!(received = ?message, "unexpected answer to the cut"),
        Err(error) => tracing::warn!(%error, "the new engine did not confirm"),
    }
    Ok(())
}

/// Waits for `Ready` of `attempt` until `deadline`. `false`: too late.
fn wait_ready(
    channel: &mut Channel,
    attempt: u32,
    deadline: Instant,
) -> Result<bool, HandoverError> {
    loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(false);
        };
        match channel.recv(Some(left)) {
            Ok((Message::Ready { attempt: ready }, _)) if ready == attempt => return Ok(true),
            // Late, for an earlier attempt.
            Ok((Message::Ready { .. }, _)) => {}
            Ok((message, _)) => {
                return Err(HandoverError::Invalid(format!("{message:?}")));
            }
            Err(HandoverError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn pair() -> (Channel, Channel) {
        let (a, b) = UnixStream::pair().unwrap();
        (Channel::new(a), Channel::new(b))
    }

    #[test]
    fn carries_messages_and_descriptors() {
        let (mut active, mut standby) = pair();
        active.send(&hello()).unwrap();
        let (message, fds) = standby.recv(Some(Duration::from_secs(1))).unwrap();
        assert_eq!(message, hello());
        assert!(fds.is_empty());

        // A socket travels with the message, still connected.
        let (mut kept, given) = UnixStream::pair().unwrap();
        let outputs = vec![HandedOver {
            output: 7,
            kind: ConnectionKind::Relayed,
        }];
        let cut = Message::Cut {
            outputs: outputs.clone(),
            agc_gain_db: Some(3.5),
        };
        active.send_with(&cut, &[OwnedFd::from(given)]).unwrap();
        let (message, mut fds) = standby.recv(Some(Duration::from_secs(1))).unwrap();
        assert_eq!(message, cut);
        assert_eq!(fds.len(), 1);
        let mut received = UnixStream::from(fds.remove(0));
        kept.write_all(b"still here").unwrap();
        let mut buffer = [0_u8; 10];
        received.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"still here");
    }

    #[test]
    fn a_cut_from_an_engine_before_the_agc_chain_has_no_gain() {
        let message: Message = serde_json::from_str(r#"{"type": "cut", "outputs": []}"#).unwrap();
        assert_eq!(
            message,
            Message::Cut {
                outputs: Vec::new(),
                agc_gain_db: None
            }
        );
        // Without a gain, nothing more is sent: older engines read it as before.
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(json, r#"{"type":"cut","outputs":[]}"#);
    }

    #[test]
    fn reports_a_closed_or_silent_peer() {
        let (mut active, standby) = pair();
        let timed_out = active.recv(Some(Duration::from_millis(20)));
        assert!(
            matches!(&timed_out, Err(HandoverError::Io(error)) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
            "{:?}",
            timed_out.err()
        );
        drop(standby);
        let closed = active.recv(Some(Duration::from_secs(1)));
        assert!(
            matches!(&closed, Err(HandoverError::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof),
            "{:?}",
            closed.err()
        );
    }

    #[test]
    fn refuses_invalid_messages() {
        let (mut active, standby) = pair();
        let mut raw = standby.stream;
        raw.write_all(&(MAX_MESSAGE as u32 + 1).to_be_bytes())
            .unwrap();
        assert!(matches!(
            active.recv(Some(Duration::from_secs(1))),
            Err(HandoverError::Invalid(_))
        ));

        let (mut active, standby) = pair();
        let mut raw = standby.stream;
        raw.write_all(&4_u32.to_be_bytes()).unwrap();
        raw.write_all(b"nope").unwrap();
        assert!(matches!(
            active.recv(Some(Duration::from_secs(1))),
            Err(HandoverError::Invalid(_))
        ));
    }

    fn inode(path: &Path) -> Option<u64> {
        std::fs::metadata(path)
            .ok()
            .map(|metadata| std::os::unix::fs::MetadataExt::ino(&metadata))
    }

    #[test]
    fn leaves_the_socket_to_the_engine_that_took_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_path(dir.path());

        // Alone: the socket goes with the engine.
        let alone = Listener::bind(dir.path()).unwrap();
        assert_eq!(inode(&path), Some(alone.inode));
        drop(alone);
        assert_eq!(inode(&path), None);

        // Replaced by another engine, without a handover: left to it.
        let old = Listener::bind(dir.path()).unwrap();
        let new = Listener::bind(dir.path()).unwrap();
        drop(old);
        assert_eq!(inode(&path), Some(new.inode));

        // Handed over: left alone even before the new engine replaces it,
        // which it may be doing at that very moment.
        new.handed_over.store(true, Ordering::Relaxed);
        drop(new);
        assert!(inode(&path).is_some());
    }

    #[test]
    fn waits_for_the_ready_of_this_attempt() {
        let (mut active, mut standby) = pair();
        standby.send(&Message::Ready { attempt: 1 }).unwrap();
        standby.send(&Message::Ready { attempt: 2 }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(wait_ready(&mut active, 2, deadline).unwrap());
        // Nothing more comes: too late.
        let deadline = Instant::now() + Duration::from_millis(20);
        assert!(!wait_ready(&mut active, 3, deadline).unwrap());
        standby.send(&Message::Done).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(matches!(
            wait_ready(&mut active, 3, deadline),
            Err(HandoverError::Invalid(_))
        ));
    }
}
