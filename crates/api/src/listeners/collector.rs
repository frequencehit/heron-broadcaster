// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reads the stream servers once a minute: each server an output uses once
//! (its status page lists every mount) and each of its relays, a sample per
//! output and server, sessions when the listener list can be read, then the
//! day's totals. A relay's own connection to its master is left out of the
//! master's count. A server that does not answer leaves a gap for its part;
//! nothing here touches the engine. See `docs/statistics.md`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use heron_core::OutputKind;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::Serialize;
use sqlx::MySqlPool;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use super::icecast::{self, Client, ReadError};
use super::sessions::{self, Changes, Known, Seen};
use super::totals::{self, Sample, Span, Total};
use crate::error::{ApiError, ApiResult};
use crate::servers::StoredServer;
use crate::settings::{self, StationSettings, StatisticsSettings};

const MINUTE_MS: i64 = 60_000;
/// Into the minute, so that a clock slightly behind the server's still
/// samples the right minute.
const POLL_OFFSET_MS: i64 = 1_000;
const PURGE_INTERVAL_MS: i64 = 3_600_000;
const USER_AGENT_CHARS: usize = 512;
const ERROR_CHARS: usize = 300;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// A server's part of an output's listeners at the last minute: its
/// master, or a relay.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ServerPart {
    pub server_id: u64,
    pub name: String,
    /// The master it relays, for a relay.
    pub relay_of: Option<u64>,
    /// Missing when the server could not be read.
    pub listeners: Option<u32>,
    /// Whether its listener list was read.
    pub sessions: bool,
    pub error: Option<String>,
}

/// An output's listeners at the last minute.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct OutputListeners {
    pub output_id: u64,
    pub name: String,
    /// Its servers' parts added up; missing when none could be read.
    pub listeners: Option<u32>,
    pub sampled_at_unix_ms: i64,
    /// Whether every listener list was read: sessions are up to date.
    pub sessions: bool,
    /// Why a server, or its listener list, could not be read.
    pub error: Option<String>,
    /// Each server's part: the master's first, then the relays'.
    pub servers: Vec<ServerPart>,
}

/// An output to read.
struct Target {
    id: u64,
    name: String,
    server_id: u64,
    mount: String,
}

pub struct Collector {
    pool: MySqlPool,
    http: reqwest::Client,
    last: RwLock<Vec<OutputListeners>>,
    recompute: Notify,
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn seconds(ms: i64) -> f64 {
    ms as f64 / 1_000.0
}

async fn station_ids(pool: &MySqlPool) -> ApiResult<Vec<u64>> {
    Ok(sqlx::query_scalar("SELECT id FROM stations ORDER BY id")
        .fetch_all(pool)
        .await?)
}

fn truncate(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

fn base(server: &StoredServer) -> String {
    icecast::base_url(&server.host, server.port, server.tls)
}

/// The addresses a relay's host resolves to: its connection to the master
/// comes from one of them.
async fn addresses(host: &str, port: u16) -> HashSet<IpAddr> {
    let lookup = tokio::net::lookup_host((host, port));
    match tokio::time::timeout(RESOLVE_TIMEOUT, lookup).await {
        Ok(Ok(found)) => found.map(|address| address.ip().to_canonical()).collect(),
        _ => HashSet::new(),
    }
}

/// Whether a listener of the master is a relay's own connection: from the
/// relay's address, with the player Icecast relays connect with.
fn is_relay_connection(client: &Client, relay: &HashSet<IpAddr>) -> bool {
    let from_relay = client
        .address
        .parse::<IpAddr>()
        .is_ok_and(|address| relay.contains(&address.to_canonical()));
    from_relay && relay_player(client)
}

fn relay_player(client: &Client) -> bool {
    client
        .user_agent
        .get(..7)
        .is_some_and(|start| start.eq_ignore_ascii_case("icecast"))
}

/// The master's listeners of a mount, without its relays' connections:
/// the count to keep, and the listeners left for sessions.
pub(super) fn without_relays(
    count: u32,
    list: Option<&[Client]>,
    relays: &[(bool, bool, HashSet<IpAddr>)],
) -> (u32, Option<Vec<Client>>) {
    let mut subtract = 0u32;
    let mut kept = list.map(<[Client]>::to_vec);
    // (relay has the mount, subtract one, its addresses)
    for (has_mount, subtract_one, relay) in relays {
        match kept.as_mut() {
            Some(clients) if !subtract_one => {
                let before = clients.len();
                clients.retain(|client| !is_relay_connection(client, relay));
                subtract += u32::try_from(before - clients.len()).unwrap_or(u32::MAX);
            }
            _ if *has_mount => {
                subtract += 1;
                // Its session is not recorded either, when it can be told.
                if let Some(clients) = kept.as_mut()
                    && let Some(at) = clients.iter().position(relay_player)
                {
                    clients.remove(at);
                }
            }
            _ => {}
        }
    }
    (count.saturating_sub(subtract), kept)
}
impl Collector {
    pub fn new(pool: MySqlPool) -> anyhow::Result<Arc<Self>> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("HeronBroadcaster/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Arc::new(Self {
            pool,
            http,
            last: RwLock::new(Vec::new()),
            recompute: Notify::new(),
        }))
    }

    /// Each enabled output's listeners at the last minute.
    pub fn last(&self) -> Vec<OutputListeners> {
        self.last
            .read()
            .map(|last| last.clone())
            .unwrap_or_default()
    }

    /// The statistics settings changed: recompute the totals the sessions
    /// kept still cover.
    pub fn settings_changed(&self) {
        self.recompute.notify_one();
    }

    pub async fn run(self: Arc<Self>) {
        let mut next_purge = 0;
        loop {
            let now = now_ms();
            let wait = MINUTE_MS - (now - POLL_OFFSET_MS).rem_euclid(MINUTE_MS);
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(wait.unsigned_abs())) => {
                    next_purge = self.minute(now_ms(), next_purge).await;
                }
                () = self.recompute.notified() => self.recompute_now(now_ms()).await,
            }
        }
    }

    /// A minute's work: the reading, and the purge when it is due. Returns
    /// when the next purge is due.
    pub async fn minute(&self, now_ms: i64, next_purge: i64) -> i64 {
        if let Err(error) = self.poll(now_ms).await {
            tracing::warn!(%error, "cannot record listener statistics");
        }
        if now_ms < next_purge {
            return next_purge;
        }
        match self.purge(now_ms).await {
            Ok(()) => now_ms + PURGE_INTERVAL_MS,
            Err(error) => {
                tracing::warn!(%error, "cannot purge listener statistics");
                next_purge
            }
        }
    }

    pub(super) async fn recompute_now(&self, now_ms: i64) {
        if let Err(error) = self.recompute_kept(now_ms).await {
            tracing::warn!(%error, "cannot recompute the listener totals");
        }
    }

    /// The enabled Icecast outputs of a station, and its servers.
    async fn targets(&self, station_id: u64) -> ApiResult<(Vec<Target>, Vec<StoredServer>)> {
        let mut connection = self.pool.acquire().await?;
        let outputs = crate::outputs::load_outputs(&mut connection, Some(station_id)).await?;
        let servers = crate::servers::load(&mut connection, Some(station_id)).await?;
        let targets = outputs
            .into_iter()
            // Shoutcast with its sinks (M11); HLS has no server to ask.
            .filter(|output| output.enabled && output.kind == OutputKind::Icecast)
            .filter_map(|output| {
                let mount = output.config.get("mount")?.as_str()?.to_owned();
                let server_id = output.server_id?;
                servers
                    .iter()
                    .any(|server| server.id == server_id && server.relay_of.is_none())
                    .then_some(Target {
                        id: output.id,
                        name: output.name,
                        server_id,
                        mount,
                    })
            })
            .filter(|target| target.mount.starts_with('/'))
            .collect();
        Ok((targets, servers))
    }

    /// Reads a station's servers. Returns the station's day.
    async fn poll_station(
        &self,
        station_id: u64,
        now_ms: i64,
        statistics: &StatisticsSettings,
        last: &mut Vec<OutputListeners>,
    ) -> ApiResult<Date> {
        let station: StationSettings =
            settings::load_for(&self.pool, station_id, StationSettings::SETTINGS_NAME).await?;
        let tz = station.time_zone();
        let (targets, servers) = self.targets(station_id).await?;
        let masters: BTreeSet<u64> = targets.iter().map(|t| t.server_id).collect();
        let read: Vec<&StoredServer> = servers
            .iter()
            .filter(|server| {
                masters.contains(&server.id)
                    || server
                        .relay_of
                        .is_some_and(|master| masters.contains(&master))
            })
            .collect();
        let relays_of = |master: u64| {
            read.iter()
                .copied()
                .filter(move |server| server.relay_of == Some(master))
        };

        // One status request per server, every listener list at once.
        let mut requests = JoinSet::new();
        for server in &read {
            let (http, id, base) = (self.http.clone(), server.id, base(server));
            requests.spawn(async move { (id, icecast::mount_listeners(&http, &base).await) });
        }
        let mut wanted: BTreeSet<(u64, String)> = BTreeSet::new();
        for target in &targets {
            wanted.insert((target.server_id, target.mount.clone()));
            for relay in relays_of(target.server_id) {
                if let Some(mount) = relay.relayed(&target.mount) {
                    wanted.insert((relay.id, mount));
                }
            }
        }
        let mut lists = JoinSet::new();
        for (server_id, mount) in wanted {
            let Some(server) = read.iter().find(|server| server.id == server_id) else {
                continue;
            };
            if let Some((username, password)) = server.admin_credentials() {
                let (http, base) = (self.http.clone(), base(server));
                lists.spawn(async move {
                    let clients =
                        icecast::list_clients(&http, &base, &mount, &username, &password).await;
                    ((server_id, mount), clients)
                });
            }
        }
        let mut relay_addresses: HashMap<u64, HashSet<IpAddr>> = HashMap::new();
        for relay in read.iter().filter(|server| server.relay_of.is_some()) {
            if !relay.relay_subtract_one {
                relay_addresses.insert(relay.id, addresses(&relay.host, relay.port).await);
            }
        }
        let mut statuses: BTreeMap<u64, Result<HashMap<String, u32>, ReadError>> = BTreeMap::new();
        while let Some(Ok((id, status))) = requests.join_next().await {
            count_poll(status.is_ok());
            statuses.insert(id, status);
        }
        let mut clients: BTreeMap<(u64, String), Result<Vec<Client>, ReadError>> = BTreeMap::new();
        while let Some(Ok((key, list))) = lists.join_next().await {
            count_poll(list.is_ok());
            clients.insert(key, list);
        }

        let minute_ms = now_ms - now_ms.rem_euclid(MINUTE_MS);
        let today = totals::day_of(now_ms, &tz);
        let (today_start, _) = totals::day_bounds(today, &tz);
        let day_key = self.day_key(today).await?;
        let mut days = BTreeSet::from([today]);
        for target in targets {
            let Some(master) = read.iter().find(|server| server.id == target.server_id) else {
                continue;
            };
            let relays: Vec<(&StoredServer, String)> = relays_of(master.id)
                .filter_map(|relay| Some((relay, relay.relayed(&target.mount)?)))
                .collect();
            let mut parts = Vec::new();
            // Each server: its count and listener list for the output.
            let mut readings = vec![(*master, target.mount.clone())];
            readings.extend(relays.iter().map(|(relay, mount)| (*relay, mount.clone())));
            for (server, mount) in readings {
                let mut part = ServerPart {
                    server_id: server.id,
                    name: server.name.clone(),
                    relay_of: server.relay_of,
                    listeners: None,
                    sessions: false,
                    error: None,
                };
                let status = match statuses.get(&server.id) {
                    Some(Ok(mounts)) => mounts,
                    Some(Err(error)) => {
                        tracing::debug!(server = %server.name, %error, "cannot read the stream server");
                        part.error = Some(truncate(&error.to_string(), ERROR_CHARS));
                        parts.push(part);
                        continue;
                    }
                    None => {
                        parts.push(part);
                        continue;
                    }
                };
                let count = status.get(&mount).copied().unwrap_or(0);
                let list = match clients.remove(&(server.id, mount.clone())) {
                    Some(Ok(list)) => Some(list),
                    Some(Err(error)) => {
                        part.error =
                            Some(truncate(&format!("listener list: {error}"), ERROR_CHARS));
                        None
                    }
                    None => None,
                };
                let (count, list) = if server.relay_of.is_none() {
                    let relays: Vec<(bool, bool, HashSet<IpAddr>)> = relays
                        .iter()
                        .map(|(relay, relay_mount)| {
                            let has_mount = matches!(
                                statuses.get(&relay.id),
                                Some(Ok(mounts)) if mounts.contains_key(relay_mount)
                            );
                            (
                                has_mount,
                                relay.relay_subtract_one,
                                relay_addresses.get(&relay.id).cloned().unwrap_or_default(),
                            )
                        })
                        .collect();
                    without_relays(count, list.as_deref(), &relays)
                } else {
                    (count, list)
                };
                part.listeners = Some(count);
                if let Some(list) = list {
                    part.sessions = true;
                    let seen = list
                        .iter()
                        .map(|client| Seen {
                            client_id: client.id,
                            key: sessions::listener_key(
                                &day_key,
                                &client.address,
                                &client.user_agent,
                            ),
                            user_agent: truncate(&client.user_agent, USER_AGENT_CHARS),
                            connected_ms: now_ms
                                - i64::try_from(client.connected_s)
                                    .unwrap_or(i64::MAX / 2_000)
                                    .saturating_mul(1_000),
                        })
                        .collect::<Vec<_>>();
                    let touched = self
                        .track_sessions(target.id, server.id, &seen, now_ms, today, today_start)
                        .await?;
                    days.extend(touched);
                }
                sqlx::query(
                    "INSERT INTO listener_samples (output_id, server_id, sampled_at, listeners,
                                                   sessions)
                     VALUES (?, ?, FROM_UNIXTIME(?), ?, ?)
                     ON DUPLICATE KEY UPDATE listeners = VALUES(listeners),
                                             sessions = VALUES(sessions)",
                )
                .bind(target.id)
                .bind(server.id)
                .bind(minute_ms / 1_000)
                .bind(count)
                .bind(part.sessions)
                .execute(&self.pool)
                .await?;
                parts.push(part);
            }
            let read_parts: Vec<&ServerPart> = parts
                .iter()
                .filter(|part| part.listeners.is_some())
                .collect();
            let listeners = (!read_parts.is_empty())
                .then(|| read_parts.iter().filter_map(|part| part.listeners).sum());
            let error = parts
                .iter()
                .filter_map(|part| {
                    let error = part.error.as_ref()?;
                    Some(if part.relay_of.is_some() {
                        format!("{}: {error}", part.name)
                    } else {
                        error.clone()
                    })
                })
                .collect::<Vec<_>>();
            metrics::gauge!("heron_listeners", "output" => target.name.clone())
                .set(listeners.map_or(f64::NAN, f64::from));
            last.push(OutputListeners {
                output_id: target.id,
                name: target.name,
                listeners,
                sampled_at_unix_ms: minute_ms,
                sessions: !read_parts.is_empty() && read_parts.iter().all(|part| part.sessions),
                error: (!error.is_empty()).then(|| truncate(&error.join("; "), ERROR_CHARS)),
                servers: parts,
            });
        }

        for day in days {
            self.recompute(station_id, day, &tz, statistics.min_session_s)
                .await?;
        }
        Ok(today)
    }

    /// Reads the servers of every station with listener statistics on,
    /// and records the minute of `now_ms`.
    pub async fn poll(&self, now_ms: i64) -> ApiResult<()> {
        let statistics: StatisticsSettings =
            settings::load(&self.pool, StatisticsSettings::SETTINGS_NAME).await?;
        let mut last = Vec::new();
        let mut earliest_today: Option<Date> = None;
        for station_id in station_ids(&self.pool).await? {
            // Turned off: its servers are not read (docs/modules.md).
            if !crate::modules::on_for(&self.pool, station_id)
                .await?
                .listener_statistics
            {
                continue;
            }
            let today = self
                .poll_station(station_id, now_ms, &statistics, &mut last)
                .await?;
            earliest_today = Some(earliest_today.map_or(today, |earliest| earliest.min(today)));
        }
        // Today's key is all the next sessions need: keys of earlier days
        // go, so that days cannot be linked. Stations in other time zones
        // may still be on the day before.
        if let Some(today) = earliest_today {
            sqlx::query("DELETE FROM listener_day_keys WHERE day < ?")
                .bind(today.to_string())
                .execute(&self.pool)
                .await?;
        }
        if let Ok(mut stored) = self.last.write() {
            *stored = last;
        }
        Ok(())
    }

    /// The day's key, made the first time it is needed.
    async fn day_key(&self, day: Date) -> ApiResult<[u8; 32]> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key)
            .map_err(|error| ApiError::Internal(format!("no randomness: {error}")))?;
        sqlx::query("INSERT IGNORE INTO listener_day_keys (day, `key`) VALUES (?, ?)")
            .bind(day.to_string())
            .bind(key.as_slice())
            .execute(&self.pool)
            .await?;
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT `key` FROM listener_day_keys WHERE day = ?")
                .bind(day.to_string())
                .fetch_one(&self.pool)
                .await?;
        stored
            .try_into()
            .map_err(|_| ApiError::Internal("invalid listener day key".to_owned()))
    }

    /// Updates an output's sessions on a server from its listener list.
    /// Returns the days whose sessions were split at midnight.
    async fn track_sessions(
        &self,
        output_id: u64,
        server_id: u64,
        seen: &[Seen],
        now_ms: i64,
        today: Date,
        today_start: i64,
    ) -> ApiResult<Vec<Date>> {
        type Row = (u64, Vec<u8>, String, u64, String, i64, i64, bool);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, listener_key, user_agent, client_id, CAST(day AS CHAR),
                    CAST(UNIX_TIMESTAMP(started_at) * 1000 AS SIGNED),
                    CAST(UNIX_TIMESTAMP(last_seen_at) * 1000 AS SIGNED),
                    ended_at IS NULL
             FROM listener_sessions
             WHERE output_id = ? AND server_id = ?
               AND (ended_at IS NULL OR ended_at >= FROM_UNIXTIME(?))",
        )
        .bind(output_id)
        .bind(server_id)
        .bind(seconds(now_ms - 2 * sessions::RECONNECT_MS))
        .fetch_all(&self.pool)
        .await?;
        let known: Vec<Known> = rows
            .into_iter()
            .filter_map(
                |(id, key, user_agent, client_id, day, started_ms, last_seen_ms, open)| {
                    Some(Known {
                        id,
                        key: key.try_into().ok()?,
                        user_agent,
                        client_id,
                        day: day.parse().ok()?,
                        started_ms,
                        last_seen_ms,
                        open,
                    })
                },
            )
            .collect();
        let changes = sessions::reconcile(&known, seen, now_ms, today, today_start);
        let split_days = changes
            .split
            .iter()
            .filter_map(|id| known.iter().find(|s| s.id == *id).map(|s| s.day))
            .collect();
        self.apply(output_id, server_id, changes, now_ms, today, today_start)
            .await?;
        Ok(split_days)
    }

    async fn apply(
        &self,
        output_id: u64,
        server_id: u64,
        changes: Changes,
        now_ms: i64,
        today: Date,
        today_start: i64,
    ) -> ApiResult<()> {
        let mut tx = self.pool.begin().await?;
        for (id, client_id) in changes.continued {
            sqlx::query(
                "UPDATE listener_sessions
                 SET client_id = ?, last_seen_at = FROM_UNIXTIME(?), ended_at = NULL,
                     duration_s = GREATEST(0, TIMESTAMPDIFF(SECOND, started_at, FROM_UNIXTIME(?)))
                 WHERE id = ?",
            )
            .bind(client_id)
            .bind(seconds(now_ms))
            .bind(seconds(now_ms))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        for id in changes.closed {
            sqlx::query("UPDATE listener_sessions SET ended_at = last_seen_at WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        for id in changes.split {
            sqlx::query(
                "UPDATE listener_sessions
                 SET last_seen_at = FROM_UNIXTIME(?), ended_at = FROM_UNIXTIME(?), split = TRUE,
                     duration_s = GREATEST(0, TIMESTAMPDIFF(SECOND, started_at, FROM_UNIXTIME(?)))
                 WHERE id = ?",
            )
            .bind(seconds(today_start))
            .bind(seconds(today_start))
            .bind(seconds(today_start))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        for session in changes.opened {
            sqlx::query(
                "INSERT INTO listener_sessions
                     (output_id, server_id, listener_key, user_agent, client_id, day,
                      started_at, last_seen_at, duration_s)
                 VALUES (?, ?, ?, ?, ?, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), ?)",
            )
            .bind(output_id)
            .bind(server_id)
            .bind(session.key.as_slice())
            .bind(&session.user_agent)
            .bind(session.client_id)
            .bind(today.to_string())
            .bind(seconds(session.started_ms))
            .bind(seconds(now_ms))
            .bind(u32::try_from((now_ms - session.started_ms).max(0) / 1_000).unwrap_or(u32::MAX))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Recomputes a station's day totals from its samples and sessions. A
    /// day without samples (purged, or before statistics) keeps its totals.
    pub async fn recompute(
        &self,
        station_id: u64,
        day: Date,
        tz: &TimeZone,
        min_session_s: u32,
    ) -> ApiResult<()> {
        let (start, end) = totals::day_bounds(day, tz);
        // An output's minute adds up its servers (master and relays).
        let samples: Vec<(u64, i64, u32, bool)> = sqlx::query_as(
            "SELECT s.output_id, CAST(UNIX_TIMESTAMP(s.sampled_at) * 1000 AS SIGNED),
                    CAST(SUM(s.listeners) AS UNSIGNED), MAX(s.sessions)
             FROM listener_samples s JOIN outputs o ON o.id = s.output_id
             WHERE o.station_id = ?
               AND s.sampled_at >= FROM_UNIXTIME(?) AND s.sampled_at < FROM_UNIXTIME(?)
             GROUP BY s.output_id, s.sampled_at",
        )
        .bind(station_id)
        .bind(seconds(start))
        .bind(seconds(end))
        .fetch_all(&self.pool)
        .await?;
        if samples.is_empty() {
            return Ok(());
        }
        let samples: Vec<Sample> = samples
            .into_iter()
            .map(|(output_id, minute_ms, listeners, sessions)| Sample {
                output_id,
                minute_ms,
                listeners,
                sessions,
            })
            .collect();
        let spans: Vec<(u64, Vec<u8>, i64, i64, u32)> = sqlx::query_as(
            "SELECT s.output_id, s.listener_key,
                    CAST(UNIX_TIMESTAMP(s.started_at) * 1000 AS SIGNED),
                    CAST(UNIX_TIMESTAMP(s.last_seen_at) * 1000 AS SIGNED), s.duration_s
             FROM listener_sessions s JOIN outputs o ON o.id = s.output_id
             WHERE o.station_id = ? AND s.day = ?",
        )
        .bind(station_id)
        .bind(day.to_string())
        .fetch_all(&self.pool)
        .await?;
        let spans: Vec<Span> = spans
            .into_iter()
            .filter_map(|(output_id, key, started_ms, last_seen_ms, duration_s)| {
                Some(Span {
                    output_id,
                    key: key.try_into().ok()?,
                    started_ms,
                    last_seen_ms,
                    duration_s,
                })
            })
            .collect();
        let rows = totals::day_totals(&samples, &spans, tz, min_session_s);
        self.store(station_id, day, &rows).await
    }

    async fn store(&self, station_id: u64, day: Date, rows: &[Total]) -> ApiResult<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM listener_totals WHERE station_id = ? AND day = ?")
            .bind(station_id)
            .bind(day.to_string())
            .execute(&mut *tx)
            .await?;
        if !rows.is_empty() {
            let mut insert = sqlx::QueryBuilder::new(
                "INSERT INTO listener_totals (station_id, output_id, day, hour, minutes, peak,
                     average, listening_hours, sessions, session_s, distinct_listeners) ",
            );
            insert.push_values(rows, |mut values, row| {
                values
                    .push_bind(station_id)
                    .push_bind(row.output_id)
                    .push_bind(day.to_string())
                    .push_bind(row.hour)
                    .push_bind(row.minutes)
                    .push_bind(row.peak)
                    .push_bind(row.average)
                    .push_bind(row.listening_hours)
                    .push_bind(row.sessions)
                    .push_bind(row.session_s)
                    .push_bind(row.distinct_listeners);
            });
            insert.build().execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Recomputes every day whose samples and sessions are all still kept,
    /// on every station.
    async fn recompute_kept(&self, now_ms: i64) -> ApiResult<()> {
        let statistics: StatisticsSettings =
            settings::load(&self.pool, StatisticsSettings::SETTINGS_NAME).await?;
        let cutoff = now_ms - i64::from(statistics.retention_days) * 86_400_000;
        for station_id in station_ids(&self.pool).await? {
            let station: StationSettings =
                settings::load_for(&self.pool, station_id, StationSettings::SETTINGS_NAME).await?;
            let tz = station.time_zone();
            let today = totals::day_of(now_ms, &tz);
            let mut day = totals::day_of(cutoff, &tz);
            let mut days = 0;
            while day <= today {
                if totals::day_bounds(day, &tz).0 >= cutoff {
                    self.recompute(station_id, day, &tz, statistics.min_session_s)
                        .await?;
                    days += 1;
                }
                let Ok(next) = day.tomorrow() else { break };
                day = next;
            }
            tracing::info!(station = station_id, days, "listener totals recomputed");
        }
        Ok(())
    }

    /// Deletes the sessions and samples older than the retention.
    async fn purge(&self, now_ms: i64) -> ApiResult<()> {
        let statistics: StatisticsSettings =
            settings::load(&self.pool, StatisticsSettings::SETTINGS_NAME).await?;
        let cutoff = seconds(now_ms - i64::from(statistics.retention_days) * 86_400_000);
        let sessions = sqlx::query(
            "DELETE FROM listener_sessions WHERE ended_at IS NOT NULL AND started_at < FROM_UNIXTIME(?)",
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await?
        .rows_affected();
        let samples =
            sqlx::query("DELETE FROM listener_samples WHERE sampled_at < FROM_UNIXTIME(?)")
                .bind(cutoff)
                .execute(&self.pool)
                .await?
                .rows_affected();
        if sessions + samples > 0 {
            tracing::info!(sessions, samples, "old listener statistics purged");
        }
        Ok(())
    }
}

fn count_poll(ok: bool) {
    metrics::counter!(
        "heron_api_listener_polls_total",
        "result" => if ok { "ok" } else { "error" }
    )
    .increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: u64, address: &str, player: &str) -> Client {
        Client {
            id,
            address: address.to_owned(),
            user_agent: player.to_owned(),
            connected_s: 60,
        }
    }

    #[test]
    fn relays_connections_are_left_out() {
        let relay: HashSet<IpAddr> = ["192.0.2.50".parse().unwrap()].into();
        let list = [
            client(1, "192.0.2.1", "VLC/3.0"),
            client(2, "192.0.2.50", "Icecast 2.4.4"),
            // A listener on the relay's machine, with a player.
            client(3, "192.0.2.50", "mpv"),
            client(4, "::ffff:192.0.2.50", "Icecast 2.5.0"),
        ];
        // Known by address and player.
        let (count, kept) = without_relays(4, Some(&list), &[(true, false, relay.clone())]);
        assert_eq!(count, 2);
        let kept: Vec<u64> = kept.unwrap().iter().map(|c| c.id).collect();
        assert_eq!(kept, [1, 3]);
        // Behind NAT: one per mount the relay has, its session left out.
        let (count, kept) = without_relays(4, Some(&list[..2]), &[(true, true, HashSet::new())]);
        assert_eq!((count, kept.unwrap().len()), (3, 1));
        // Without the master's list: one per mount, when the relay has it.
        let none: [(bool, bool, HashSet<IpAddr>); 2] =
            [(true, false, relay.clone()), (false, false, relay)];
        assert_eq!(without_relays(5, None, &none), (4, None));
        assert_eq!(without_relays(0, None, &none), (0, None));
    }
}
