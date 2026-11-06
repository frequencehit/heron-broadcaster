// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reads the stream servers once a minute: a sample per output, sessions
//! when the listener list can be read, then the day's totals. A server that
//! does not answer leaves a gap; nothing here touches the engine.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use heron_core::OutputKind;
use heron_outputs::icecast::IcecastConfig;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use super::icecast::{self, Client, ReadError};
use super::sessions::{self, Changes, Known, Seen};
use super::totals::{self, Sample, Span, Total};
use crate::error::{ApiError, ApiResult};
use crate::settings::{self, StationSettings, StatisticsSettings};

const MINUTE_MS: i64 = 60_000;
/// Into the minute, so that a clock slightly behind the server's still
/// samples the right minute.
const POLL_OFFSET_MS: i64 = 1_000;
const PURGE_INTERVAL_MS: i64 = 3_600_000;
const USER_AGENT_CHARS: usize = 512;
const ERROR_CHARS: usize = 300;

/// The stream server's admin credentials of an output
/// (`outputs.statistics`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredStatistics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_password: Option<String>,
}

impl StoredStatistics {
    pub const DEFAULT_ADMIN: &str = "admin";

    pub fn parse(json: Option<&str>) -> Self {
        json.and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default()
    }

    fn credentials(&self) -> Option<(String, String)> {
        let password = self.admin_password.clone().filter(|p| !p.is_empty())?;
        let username = self
            .admin_username
            .clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| Self::DEFAULT_ADMIN.to_owned());
        Some((username, password))
    }
}

/// An output's listeners at the last minute.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct OutputListeners {
    pub output_id: u64,
    pub name: String,
    /// Missing when its server could not be read.
    pub listeners: Option<u32>,
    pub sampled_at_unix_ms: i64,
    /// Whether the listener list was read: sessions are up to date.
    pub sessions: bool,
    /// Why the server, or its listener list, could not be read.
    pub error: Option<String>,
}

/// An output to read.
struct Target {
    id: u64,
    name: String,
    base: String,
    mount: String,
    credentials: Option<(String, String)>,
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

    async fn targets(&self, station_id: u64) -> ApiResult<Vec<Target>> {
        let rows: Vec<(u64, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, name, CAST(kind AS CHAR), CAST(config AS CHAR), CAST(statistics AS CHAR)
             FROM outputs WHERE enabled AND station_id = ? ORDER BY id",
        )
        .bind(station_id)
        .fetch_all(&self.pool)
        .await?;
        let mut targets = Vec::new();
        for (id, name, kind, config, statistics) in rows {
            // Shoutcast with its sinks (M11); HLS has no server to ask.
            if serde_json::from_value::<OutputKind>(serde_json::Value::String(kind)).ok()
                != Some(OutputKind::Icecast)
            {
                continue;
            }
            let config = serde_json::from_str(&config).unwrap_or(serde_json::Value::Null);
            let Ok(config) = IcecastConfig::parse(&name, &config) else {
                continue;
            };
            targets.push(Target {
                id,
                name,
                base: icecast::base_url(&config.host, config.port),
                mount: config.mount,
                credentials: StoredStatistics::parse(statistics.as_deref()).credentials(),
            });
        }
        Ok(targets)
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
        let targets = self.targets(station_id).await?;

        // One status request per server, every listener list at once.
        let mut requests = JoinSet::new();
        for base in targets
            .iter()
            .map(|t| t.base.clone())
            .collect::<BTreeSet<_>>()
        {
            let http = self.http.clone();
            requests.spawn(async move {
                let status = icecast::mount_listeners(&http, &base).await;
                (base, status)
            });
        }
        let mut lists = JoinSet::new();
        for target in &targets {
            if let Some((username, password)) = target.credentials.clone() {
                let (http, id, base, mount) = (
                    self.http.clone(),
                    target.id,
                    target.base.clone(),
                    target.mount.clone(),
                );
                lists.spawn(async move {
                    let clients =
                        icecast::list_clients(&http, &base, &mount, &username, &password).await;
                    (id, clients)
                });
            }
        }
        let mut statuses = BTreeMap::new();
        while let Some(Ok((base, status))) = requests.join_next().await {
            count_poll(status.is_ok());
            statuses.insert(base, status);
        }
        let mut clients: BTreeMap<u64, Result<Vec<Client>, ReadError>> = BTreeMap::new();
        while let Some(Ok((id, list))) = lists.join_next().await {
            count_poll(list.is_ok());
            clients.insert(id, list);
        }

        let minute_ms = now_ms - now_ms.rem_euclid(MINUTE_MS);
        let today = totals::day_of(now_ms, &tz);
        let (today_start, _) = totals::day_bounds(today, &tz);
        let day_key = self.day_key(today).await?;
        let mut days = BTreeSet::from([today]);
        for target in targets {
            let mut now = OutputListeners {
                output_id: target.id,
                name: target.name.clone(),
                listeners: None,
                sampled_at_unix_ms: minute_ms,
                sessions: false,
                error: None,
            };
            match statuses.get(&target.base) {
                Some(Ok(mounts)) => {
                    let listeners = mounts.get(&target.mount).copied().unwrap_or(0);
                    now.listeners = Some(listeners);
                    match clients.remove(&target.id) {
                        Some(Ok(list)) => {
                            now.sessions = true;
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
                                .track_sessions(target.id, &seen, now_ms, today, today_start)
                                .await?;
                            days.extend(touched);
                        }
                        Some(Err(error)) => {
                            now.error =
                                Some(truncate(&format!("listener list: {error}"), ERROR_CHARS));
                        }
                        None => {}
                    }
                    sqlx::query(
                        "INSERT INTO listener_samples (output_id, sampled_at, listeners, sessions)
                         VALUES (?, FROM_UNIXTIME(?), ?, ?)
                         ON DUPLICATE KEY UPDATE listeners = VALUES(listeners),
                                                 sessions = VALUES(sessions)",
                    )
                    .bind(target.id)
                    .bind(minute_ms / 1_000)
                    .bind(listeners)
                    .bind(now.sessions)
                    .execute(&self.pool)
                    .await?;
                    metrics::gauge!("heron_listeners", "output" => target.name)
                        .set(f64::from(listeners));
                }
                Some(Err(error)) => {
                    tracing::debug!(output = %target.name, %error, "cannot read the stream server");
                    now.error = Some(truncate(&error.to_string(), ERROR_CHARS));
                    metrics::gauge!("heron_listeners", "output" => target.name).set(f64::NAN);
                }
                None => {}
            }
            last.push(now);
        }

        for day in days {
            self.recompute(station_id, day, &tz, statistics.min_session_s)
                .await?;
        }
        Ok(today)
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

    /// Updates an output's sessions from its listener list. Returns the
    /// days whose sessions were split at midnight.
    async fn track_sessions(
        &self,
        output_id: u64,
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
             WHERE output_id = ? AND (ended_at IS NULL OR ended_at >= FROM_UNIXTIME(?))",
        )
        .bind(output_id)
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
        self.apply(output_id, changes, now_ms, today, today_start)
            .await?;
        Ok(split_days)
    }

    async fn apply(
        &self,
        output_id: u64,
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
                     (output_id, listener_key, user_agent, client_id, day, started_at,
                      last_seen_at, duration_s)
                 VALUES (?, ?, ?, ?, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), ?)",
            )
            .bind(output_id)
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
        let samples: Vec<(u64, i64, u32, bool)> = sqlx::query_as(
            "SELECT s.output_id, CAST(UNIX_TIMESTAMP(s.sampled_at) * 1000 AS SIGNED), s.listeners,
                    s.sessions
             FROM listener_samples s JOIN outputs o ON o.id = s.output_id
             WHERE o.station_id = ?
               AND s.sampled_at >= FROM_UNIXTIME(?) AND s.sampled_at < FROM_UNIXTIME(?)",
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

    #[test]
    fn credentials_need_a_password() {
        assert_eq!(StoredStatistics::parse(None).credentials(), None);
        assert_eq!(
            StoredStatistics::parse(Some(r#"{"admin_username": "admin"}"#)).credentials(),
            None
        );
        assert_eq!(
            StoredStatistics::parse(Some(r#"{"admin_password": "pw"}"#)).credentials(),
            Some(("admin".to_owned(), "pw".to_owned()))
        );
        assert_eq!(
            StoredStatistics::parse(Some(r#"{"admin_username": "ops", "admin_password": "pw"}"#))
                .credentials(),
            Some(("ops".to_owned(), "pw".to_owned()))
        );
    }
}
